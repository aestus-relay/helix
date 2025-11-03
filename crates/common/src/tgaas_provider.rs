use axum::http::HeaderMap;
use std::collections::HashMap;
use std::net::SocketAddr;
use tracing::{debug, info, warn};

use crate::{
    api_provider::{
        ApiProvider,
        TimingResult},
    config::TimingGameConfig,
    latency::LatencyEstimator,
    utils::utcnow_ms,
    HEADER_TIMEOUT_MS, GET_HEADER_REQUEST_CUTOFF_MS,
};
        

#[derive(Clone)]
pub struct TgaasApiProvider {
    timing_game_config: TimingGameConfig,
    latency_estimator: LatencyEstimator,
}

impl TgaasApiProvider {
    pub fn new(timing_game_config: TimingGameConfig) -> Self {
        let latency_estimator = LatencyEstimator::new(&timing_game_config);
        Self {
            timing_game_config,
            latency_estimator,
        }
    }
}

impl ApiProvider for TgaasApiProvider {
    fn get_metadata(&self, headers: &axum::http::HeaderMap) -> Option<String> {
        headers.get("User-Agent")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    fn get_timing(
        &self,
        _params: &crate::api::proposer_api::GetHeaderParams,
        headers: &axum::http::HeaderMap,
        query_params: &HashMap<String, String>,
        remote_addr: SocketAddr,
        preferences: &crate::ValidatorPreferences,
        ms_into_slot: u64,
    ) -> Result<TimingResult, &'static str> {
        let mut mev_boost = false;

        // Parse headers for start time, timeout, and user agent
        let header_start_ms = get_x_mev_boost_header_start_ms(&headers);
        let mut elapsed_ms = ms_into_slot;
        if let Some(request_initiated_ms) = header_start_ms {
            elapsed_ms = utcnow_ms().saturating_sub(request_initiated_ms);
            debug!(%request_initiated_ms, %elapsed_ms, "mev-boost start ts header found");
            mev_boost = true;
        }

        let client_timeout_ms = headers
            .get(HEADER_TIMEOUT_MS)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| match h.parse::<u64>() {
                Ok(delay) => {
                    info!("header timeout ms: {}", delay);
                    Some(delay)
                }
                Err(err) => {
                    warn!(%err, "invalid header timeout ms");
                    None
                }
            });

        let user_agent = headers
            .get("User-Agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Check for delay from user agent
        let mut delayed = is_mev_boost_client(user_agent);

        // TODO: find out why preferences are always false
        // delayed = preferences.header_delay && delayed;

        // Check for delay from query params
        let mut receive_by_ms = self.timing_game_config
            .get_header_response_receive_by_ms;
        if let Some(header_delay_str) = query_params.get("headerDelay") {
            if self.timing_game_config.get_header_response_receive_by_ms != 0 {
                if let Ok(user_delay) = header_delay_str.parse::<u64>() {
                    receive_by_ms = user_delay;
                    delayed = true;
                }
                if receive_by_ms == 0 {
                    delayed = false;
                }
            }
        }

        info!("Delay determined: {}, ua: {}, pref: {}", delayed, user_agent, preferences.header_delay);

        // Adjust delay based on client timeout if provided
        // TODO: make 50 configurable
        if let Some(timeout_ms) = client_timeout_ms {
            if receive_by_ms > timeout_ms {
                receive_by_ms = timeout_ms.saturating_sub(50);
            }
        }

        let mut delay_ms = 0u64;
        let mut response_ms = 0u64;
        if delayed {
            let client_ip = get_client_ip(&headers, remote_addr);
            // Use ms_into_slot as max_elapsed_ms
            (elapsed_ms, response_ms) = self.latency_estimator.estimate_timing(header_start_ms, client_ip, ms_into_slot);
            delay_ms = receive_by_ms.saturating_sub(elapsed_ms + response_ms);
        }

        // Determine the cutoff time.
        let mut cutoff = GET_HEADER_REQUEST_CUTOFF_MS;
        if let Some(header_cutoff_str) = query_params.get("headerCutoff") {
            if self.timing_game_config.get_header_response_receive_by_ms != 0 {
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
                "Delaying getHeader response: elapsed_ms: {}, response_ms: {}, receive_by_ms: {}, cutoff: {}, delay_ms: {}",
                elapsed_ms,
                response_ms,
                receive_by_ms,
                cutoff,
                delay_ms
            );
        }

        Ok(TimingResult {
            sleep_time: if delay_ms > 0 {
                Some(std::time::Duration::from_millis(delay_ms))
            } else {
                None
            },
            is_mev_boost: mev_boost,
        })
    }
}

fn is_mev_boost_client(client_name: &str) -> bool {
    let keywords = ["Kiln", "mev-boost", "commit-boost", "Vouch"];
    keywords.iter().any(|&keyword| client_name.contains(keyword))
}

/// Fetches the ms timestamp set by the mev-boost client
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
fn get_client_ip(headers: &HeaderMap, remote_addr: SocketAddr) -> String {
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
