use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use alloy_primitives::{B256, U256};
use axum::{
    extract::{Path, Query, ConnectInfo},
    http::HeaderMap,
    response::IntoResponse,
    Extension,
};
use helix_common::{
    chain_info::ChainInfo,
    metadata_provider::MetadataProvider,
    metrics::GetHeaderMetric,
    resign_builder_bid, task,
    utils::{extract_request_id, utcnow_ms, utcnow_ns},
    BidRequest, GetHeaderTrace, RequestTimings,
};
use helix_database::DatabaseService;
use helix_types::BlsPublicKeyBytes;
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::time::sleep;
use tracing::{debug, error, info, warn, Instrument};

use super::ProposerApi;
use crate::{
    gossiper::types::RequestPayloadParams,
    proposer::{error::ProposerApiError, GetHeaderParams, GET_HEADER_REQUEST_CUTOFF_MS},
    router::Terminating,
    Api, HEADER_TIMEOUT_MS,
};

impl<A: Api> ProposerApi<A> {
    /// Retrieves the best bid header for the specified slot, parent hash, and public key.
    ///
    /// This function accepts a slot number, parent hash and public_key.
    /// 1. Validates that the request's slot is not older than the head slot.
    /// 2. Validates the request timestamp to ensure it's not too late.
    /// 3. Fetches the best bid for the given parameters from the auctioneer.
    ///
    /// The function returns a JSON response containing the best bid if found.
    ///
    /// Implements this API: <https://ethereum.github.io/builder-specs/#/Builder/getHeader>
    #[tracing::instrument(skip_all, fields(id =% extract_request_id(&headers)), err)]
    pub async fn get_header(
        Extension(proposer_api): Extension<Arc<ProposerApi<A>>>,
        Extension(timings): Extension<RequestTimings>,
        Extension(Terminating(terminating)): Extension<Terminating>,
        headers: HeaderMap,
        Path(GetHeaderParams { slot, parent_hash, pubkey }): Path<GetHeaderParams>,
        Query(query_params): Query<HashMap<String, String>>,
        ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ) -> Result<impl IntoResponse, ProposerApiError> {
        if terminating.load(Ordering::Relaxed) || proposer_api.auctioneer.kill_switch_enabled() {
            return Err(ProposerApiError::ServiceUnavailableError);
        }

        let mut trace = GetHeaderTrace { receive: timings.on_receive_ns, ..Default::default() };

        let (head_slot, duty) = proposer_api.curr_slot_info.slot_info();
        debug!(
            %head_slot,
            request_ts = trace.receive,
            %slot,
            %parent_hash,
            %pubkey,
        );

        let bid_request = BidRequest { slot: slot.into(), parent_hash, pubkey };

        // Dont allow requests for past slots
        if bid_request.slot < head_slot {
            debug!("request for past slot");
            return Err(ProposerApiError::RequestForPastSlot {
                request_slot: bid_request.slot,
                head_slot,
            });
        }

        // Only return a bid if there is a proposer connected this slot.
        let Some(duty) = duty else {
            debug!("proposer duty not found");
            return Err(ProposerApiError::ProposerNotRegistered);
        };

        let ms_into_slot = match validate_bid_request_time(&proposer_api.chain_info, &bid_request) {
            Ok(ms_into_slot) => ms_into_slot,
            Err(err) => {
                warn!(%err, "invalid bid request time");
                return Err(err);
            }
        };
        trace.validation_complete = utcnow_ns();

        let user_agent = proposer_api.metadata_provider.get_metadata(&headers);

        let mut mev_boost = false;

        let header_start_ms = get_x_mev_boost_header_start_ms(&headers);
        if let Some(request_initiated_ms) = header_start_ms {
            let latency = utcnow_ms().saturating_sub(request_initiated_ms);
            debug!(%request_initiated_ms, %latency, "mev-boost start ts header found");
            mev_boost = true;
        }

        let client_timeout_ms = headers
            .get(HEADER_TIMEOUT_MS)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| match h.parse::<u64>() {
                Ok(delay) => {
                    // TODO: move to debug at some point
                    info!("header timeout ms: {}", delay);
                    Some(delay)
                }
                Err(err) => {
                    warn!(%err, "invalid header timeout ms");
                    None
                }
            });

        let delay_ms = Duration::from_millis(
            proposer_api.compute_delay(headers,
                                       header_start_ms,
                                       remote_addr,
                                       ms_into_slot,
                                       &query_params,
                                       duty.entry.preferences.header_delay,
                                       client_timeout_ms).await);
        let mut get_header_metric = GetHeaderMetric::new(delay_ms);

        debug!(target: "timing_games", 
               ?delay_ms,
               %ms_into_slot,
               slot,
               pubkey = ?bid_request.pubkey,
               "timing game sleep");

        if delay_ms > Duration::ZERO {
            sleep(delay_ms).await;
        }

        get_header_metric.record();

        // Get best bid from auctioneer
        let get_best_bid_res = proposer_api.shared_best_header.load(
            bid_request.slot.into(),
            &bid_request.parent_hash,
            &bid_request.pubkey,
        );

        let now_ns = utcnow_ns();
        trace.best_bid_fetched = now_ns;
        debug!(trace = ?trace, "best bid fetched");

        let Some(bid) = get_best_bid_res else {
            warn!("no bid found");
            return Err(ProposerApiError::NoBidPrepared);
        };
        if bid.value == U256::ZERO {
            warn!("best bid value is 0");
            return Err(ProposerApiError::BidValueZero);
        }

        // Try to fetch a merged block if block merging is enabled
        let bid = if proposer_api.relay_config.block_merging_config.is_enabled {
            let merged_block_bid = proposer_api
                .shared_best_merged
                .load(bid_request.slot.into(), &bid_request.parent_hash);
            let max_merged_bid_age_ms =
                proposer_api.relay_config.block_merging_config.max_merged_bid_age_ms;

            let now_ms = Duration::from_nanos(now_ns).as_millis() as u64;

            match merged_block_bid {
                None => bid,
                // If the current best bid has equal or higher value, we use that
                Some((_, merged_bid)) if merged_bid.value <= bid.value => bid,
                // If the merged bid is stale, we use the current best bid
                Some((time, _)) if time < now_ms - max_merged_bid_age_ms => bid,
                // Otherwise, we use the merged bid
                Some((_, merged_bid)) => merged_bid,
            }
        } else {
            bid
        };

        let bid_block_hash = bid.header.block_hash;
        debug!(
            value = ?bid.value,
            block_hash =% bid_block_hash,
            "delivering bid",
        );

        // Save trace to DB
        save_get_header_call(
            proposer_api.db.clone(),
            slot,
            bid_request.parent_hash,
            bid_request.pubkey,
            bid_block_hash,
            trace,
            mev_boost,
            user_agent.clone(),
        )
        .await;

        let proposer_pubkey_clone = bid_request.pubkey;

        let fork = proposer_api.chain_info.current_fork_name();

        let signed_bid = resign_builder_bid(bid, &proposer_api.signing_context, fork);
        proposer_api.auctioneer.mark_header_served(&bid_block_hash);

        if user_agent.is_some() && is_mev_boost_client(&user_agent.unwrap()) {
            // Request payload in the background
            task::spawn(file!(), line!(), async move {
                proposer_api
                    .gossiper
                    .request_payload(RequestPayloadParams {
                        slot,
                        proposer_pub_key: proposer_pubkey_clone,
                        block_hash: bid_block_hash,
                    })
                    .await
            });
        }

        let signed_bid = serde_json::to_value(signed_bid)?;
        info!(block_hash =% bid_block_hash, "delivering bid");

        Ok(axum::Json(signed_bid))
    }

    pub async fn compute_delay(
        &self,
        headers: HeaderMap,
        header_start_ms: Option<u64>,
        remote_addr: SocketAddr,
        ms_into_slot: u64,
        query_params: &HashMap<String, String>,
        header_delay_pref: bool,
        client_timeout_ms: Option<u64>,
    ) -> u64 {
        let mut delay_ms = 0u64;

        let user_agent = headers
            .get("User-Agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Check for delay from user agent
        let mut delayed = self
            .relay_config
            .timing_game_config
            .delayed_header_user_agents
            .iter()
            .any(|d_ua| user_agent.contains(d_ua));

        // Check for delay from validator preferences
        // This defaults to true, so if a validator specifically declines then that takes priority over UA
        delayed = if header_delay_pref {
            delayed
        } else {
            false
        };

        // Check for delay from query params
        let mut receive_by_ms = self.relay_config
            .timing_game_config
            .get_header_response_receive_by_ms;
        if let Some(header_delay_str) = query_params.get("headerDelay") {
            if self.relay_config.timing_game_config.get_header_response_receive_by_ms != 0 {
                if let Ok(user_delay) = header_delay_str.parse::<u64>() {
                    receive_by_ms = user_delay;
                    delayed = true;
                }
                if receive_by_ms == 0 {
                    delayed = false;
                }
            }
        }

        // Adjust delay based on client timeout if provided
        // TODO: make 50 configurable
        if let Some(timeout_ms) = client_timeout_ms {
            if receive_by_ms > timeout_ms {
                receive_by_ms = timeout_ms.saturating_sub(50);
            }
        }

        let elapsed_ms : u64 = ms_into_slot;
        if delayed {
            let client_ip = get_client_ip(&headers, remote_addr);
            // Use ms_into_slot as max_elapsed_ms
            let (elapsed_ms, response_ms) = self.latency_estimator.estimate_timing(header_start_ms, client_ip, ms_into_slot).await;
            if elapsed_ms + response_ms < receive_by_ms {
                delay_ms = receive_by_ms - elapsed_ms - response_ms;
            }
        }

        // Determine the cutoff time.
        let mut cutoff = GET_HEADER_REQUEST_CUTOFF_MS;
        if let Some(header_cutoff_str) = query_params.get("headerCutoff") {
            if self.relay_config.timing_game_config.get_header_response_receive_by_ms != 0 {
                if let Ok(user_cutoff) = header_cutoff_str.parse::<u64>() {
                    if user_cutoff <= GET_HEADER_REQUEST_CUTOFF_MS {
                        cutoff = user_cutoff;
                    }
                }
            }
        }

        // Ensure we don't delay beyond the cutoff.
        delay_ms = cutoff
            .checked_sub(ms_into_slot)
            .map(|remaining| delay_ms.min(remaining))
            .unwrap_or(0);

        if delayed {
            info!(
                "Delaying getHeader response: elapsed_ms: {}, response_ms: {}, cutoff: {}, delay_ms: {}",
                elapsed_ms,
                delay_ms,
                cutoff,
                delay_ms
            );
        }

        delay_ms
    }
}

async fn save_get_header_call<DB: DatabaseService + 'static>(
    db: Arc<DB>,
    slot: u64,
    parent_hash: B256,
    public_key: BlsPublicKeyBytes,
    best_block_hash: B256,
    trace: GetHeaderTrace,
    mev_boost: bool,
    user_agent: Option<String>,
) {
    task::spawn(
        file!(),
        line!(),
        async move {
            if let Err(err) = db
                .save_get_header_call(
                    slot,
                    parent_hash,
                    public_key,
                    best_block_hash,
                    trace,
                    mev_boost,
                    user_agent,
                )
                .await
            {
                error!(%err, "error saving get header call to database");
            }
        }
        .in_current_span(),
    );
}

/// Validates that the bid request is not sent too late within the current slot.
///
/// - Only allows requests for the current slot until a certain cutoff time.
///
/// Returns how many ms we are into the slot if ok.
fn validate_bid_request_time(
    chain_info: &ChainInfo,
    bid_request: &BidRequest,
) -> Result<u64, ProposerApiError> {
    let curr_timestamp_ms = utcnow_ms();
    let slot_start_timestamp = chain_info.genesis_time_in_secs +
        (bid_request.slot.as_u64() * chain_info.seconds_per_slot());
    let ms_into_slot = curr_timestamp_ms.saturating_sub(slot_start_timestamp * 1000);

    if ms_into_slot > GET_HEADER_REQUEST_CUTOFF_MS {
        warn!(curr_timestamp_ms = curr_timestamp_ms, slot = %bid_request.slot, "get_request");

        return Err(ProposerApiError::GetHeaderRequestTooLate {
            ms_into_slot: ms_into_slot as u64,
            cutoff: GET_HEADER_REQUEST_CUTOFF_MS as u64,
        });
    }

    Ok(ms_into_slot.max(0))
}

pub fn is_mev_boost_client(client_name: &str) -> bool {
    let keywords = ["Kiln", "mev-boost", "commit-boost", "Vouch"];
    keywords.iter().any(|&keyword| client_name.contains(keyword))
}

/// Fetches the timestamp set by the mev-boost client when initialising the `get_header` request.
fn get_x_mev_boost_header_start_ms(header_map: &HeaderMap) -> Option<u64> {
    const MEV_BOOST_START_TIME_HEADER: &str = "X-MEVBoost-StartTimeUnixMS";
    const DATE_MS_HEADER: &str = "Date-Milliseconds";

    let header =
        header_map.get(DATE_MS_HEADER).or_else(|| header_map.get(MEV_BOOST_START_TIME_HEADER))?;
    let start_time_str = header.to_str().ok()?;
    let start_time_ms: u64 = start_time_str.parse().ok()?;
    Some(start_time_ms)
}

// Extract the client IP from request headers or fall back to the remote address.
// Always returns a String, should basically always have some sort of IP.
pub fn get_client_ip(headers: &HeaderMap, remote_addr: SocketAddr) -> String {
    if let Some(real_ip) = headers.get("X-Real-IP").and_then(|v| v.to_str().ok()) {
        return real_ip.to_string();
    }
    if let Some(forwarded) = headers.get("X-Forwarded-For").and_then(|v| v.to_str().ok()) {
        let ip = forwarded.split(',').next().unwrap_or_default().trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    remote_addr.ip().to_string()
}
