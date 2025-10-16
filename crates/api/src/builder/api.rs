use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};

use alloy_consensus::{Bytes48, TxEip4844, TxType};
use alloy_primitives::{Address, B256};
use axum::{Extension, http::StatusCode, response::IntoResponse};
use bytes::Bytes;
use dashmap::DashMap;
use helix_common::{
    api::{
        builder_api::{BuilderGetValidatorsResponseEntry, InclusionListWithKey},
        proposer_api::ValidatorRegistrationInfo,
    },
    bid_sorter::{BestGetHeader, BidSorterMessage},
    bid_submission::BidSubmission,
    chain_info::ChainInfo,
    local_cache::LocalCache,
    metrics::{SimulatorMetrics, HYDRATION_LATENCY},
    simulator::BlockSimError,
    utils::utcnow_ns,
    BuilderConfig, BuilderInfo, RelayConfig, SubmissionTrace, ValidatorPreferences,
};
use helix_database::DatabaseService;
use helix_housekeeper::{CurrentSlotInfo, PayloadAttributesUpdate};
use helix_types::{
    BlobWithMetadata, BlobWithMetadataV1, BlobWithMetadataV2, BlobsBundle, BlobsBundleVersion,
    BlockMergingData, BundleOrder, KzgCommitment, MergeableBundle, MergeableOrder, MergeableOrders,
    MergeableTransaction, Order, SignedBidSubmission, Transactions,
};
use tracing::error;

use crate::{
    Api, auctioneer::AuctioneerHandle, gossiper::grpc_gossiper::GrpcGossiperClientManager,
};

pub(crate) const MAX_PAYLOAD_LENGTH: usize = 1024 * 1024 * 20; // 20MB

#[derive(Clone)]
pub struct BuilderApi<A: Api> {
    pub local_cache: Arc<LocalCache>,
    pub db: Arc<PostgresDatabaseService>,
    pub chain_info: Arc<ChainInfo>,
    pub gossiper: Arc<GrpcGossiperClientManager>,
    pub curr_slot_info: CurrentSlotInfo,
    pub relay_config: Arc<RelayConfig>,
    /// Subscriber for TopBid updates, SSZ encoded
    pub top_bid_tx: tokio::sync::broadcast::Sender<Bytes>,
    /// Failsafe: if we fail to demote we pause all optimistic submissions
    pub failsafe_triggered: Arc<AtomicBool>,
    pub auctioneer_handle: AuctioneerHandle,
    pub api_provider: Arc<A::ApiProvider>,
}

impl<A: Api> BuilderApi<A> {
    pub fn new(
        local_cache: Arc<LocalCache>,
        db: Arc<PostgresDatabaseService>,
        chain_info: Arc<ChainInfo>,
        gossiper: Arc<GrpcGossiperClientManager>,
        relay_config: RelayConfig,
        curr_slot_info: CurrentSlotInfo,
        top_bid_tx: tokio::sync::broadcast::Sender<Bytes>,
        auctioneer_handle: AuctioneerHandle,
        api_provider: Arc<A::ApiProvider>,
    ) -> Self {
        Self {
            local_cache,
            db,
            chain_info,
            gossiper,
            relay_config: Arc::new(relay_config),
            curr_slot_info,
            top_bid_tx,
            failsafe_triggered: Arc::new(false.into()),
            auctioneer_handle,
            api_provider,
        }
    }

    /// Implements this API: <https://flashbots.github.io/relay-specs/#/Builder/getValidators>
    pub async fn get_validators(
        Extension(api): Extension<Arc<BuilderApi<A>>>,
    ) -> impl IntoResponse {
        if let Some(duty_bytes) = api.curr_slot_info.proposer_duties_response() {
            (StatusCode::OK, duty_bytes.0).into_response()
        } else {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }

    pub(crate) fn fetch_payload_attributes(
        &self,
        slot: Slot,
        parent_hash: B256,
        block_hash: &B256,
    ) -> Result<PayloadAttributesUpdate, BuilderApiError> {
        let Some(payload_attributes) = self.curr_slot_info.payload_attributes(parent_hash, slot)
        else {
            warn!(%block_hash, "payload attributes not yet known");
            return Err(BuilderApiError::PayloadAttributesNotYetKnown);
        };

        if payload_attributes.slot != slot {
            warn!(
                got =% slot,
                expected =% payload_attributes.slot,
                "payload attributes slot mismatch with payload attributes"
            );
            return Err(BuilderApiError::PayloadSlotMismatchWithPayloadAttributes {
                got: slot,
                expected: payload_attributes.slot,
            });
        }

        Ok(payload_attributes)
    }

    /// Check for block hashes that have already been processed.
    /// If this is the first time the hash has been seen it will insert the hash into the set.
    ///
    /// This function should not be called by functions that only process the payload.
    pub(crate) fn check_for_duplicate_block_hash(
        &self,
        block_hash: &B256,
    ) -> Result<(), BuilderApiError> {
        match self.auctioneer.seen_or_insert_block_hash(block_hash) {
            false => Ok(()),
            true => Err(BuilderApiError::DuplicateBlockHash { block_hash: *block_hash }),
        }
    }

    pub(crate) fn verify_signature(
        &self,
        payload: &SignedBidSubmission,
        skip_sigverify: bool,
        trace: &mut SubmissionTrace,
    ) -> Result<(), BuilderApiError> {
        if skip_sigverify {
            trace!("skipping signature verification");
        } else {
            // Verify the payload signature
            if let Err(err) = payload.verify_signature(self.chain_info.builder_domain) {
                warn!(%err, "failed to verify signature");
                return Err(BuilderApiError::SignatureVerificationFailed);
            }
            trace!("verified signature");
        }
        trace.skip_sigverify = skip_sigverify;
        trace.signature = utcnow_ns();

        Ok(())
    }

    /// If the proposer has specified a list of trusted builders ensure
    /// that the submitting builder pubkey is in that list.
    /// Verifies that if the proposer has specified a list of trusted builders,
    /// the builder submitting a request is in that list.
    ///
    /// The auctioneer maintains a mapping of builder public keys to corresponding IDs.
    /// This function retrieves the ID associated with the builder's public key from the auctioneer.
    /// It then checks if this ID is included in the list of trusted builders specified by the
    /// proposer.
    pub(crate) fn check_if_trusted_builder(
        next_duty: &BuilderGetValidatorsResponseEntry,
        builder_info: &BuilderInfo,
    ) -> bool {
        if let Some(trusted_builders) = &next_duty.entry.preferences.trusted_builders {
            // Handle case where proposer specifies an empty list.
            if trusted_builders.is_empty() {
                return true;
            }

            if let Some(builder_id) = &builder_info.builder_id {
                trusted_builders.contains(builder_id)
            } else if let Some(ids) = &builder_info.builder_ids {
                ids.iter().any(|id| trusted_builders.contains(id))
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Simulates a new block payload.
    ///
    /// 1. Checks the current top bid value from the auctioneer.
    /// 3. Invokes the block simulator for validation.
    pub(crate) async fn simulate_submission(
        &self,
        payload: &SignedBidSubmission,
        builder_info: &BuilderInfo,
        trace: &mut SubmissionTrace,
        registration_info: ValidatorRegistrationInfo,
        payload_attributes: &PayloadAttributesUpdate,
    ) -> Result<bool, BuilderApiError> {
        let curr_best = self.shared_best_header.best_bid(payload.slot().as_u64());
        let is_top_bid = payload.value() > curr_best;

        debug!("validating block");

        let current_slot_coord =
            (payload.slot().as_u64(), *payload.proposer_public_key(), *payload.parent_hash());

        let inclusion_list = self
            .current_inclusion_list
            .read()
            .as_ref()
            .filter(|il| il.key == current_slot_coord)
            .map(|il| il.inclusion_list.clone());

        let request = BlockSimRequest::new(
            registration_info.registration.message.gas_limit,
            payload,
            registration_info.preferences,
            payload_attributes.payload_attributes.parent_beacon_block_root,
            inclusion_list,
        );

        if self.relay_config.is_local_dev {
            return Ok(true)
        }

        let sim_optimistic = self.should_process_optimistically(&request, builder_info);
        let (res_tx, res_rx) = oneshot::channel();
        let sim_request = SimulatorRequest {
            request,
            on_receive_ns: trace.receive,
            is_top_bid,
            is_optimistic: sim_optimistic,
            res_tx,
        };

        if sim_optimistic {
            debug!("skipping simulation");
            trace.simulation = utcnow_ns();
            let cloned = self.clone();
            let builder_info = builder_info.clone();
            tokio::spawn(async move {
                let _ = cloned.send_simulation(res_rx, sim_request, &builder_info).await;
            });

            Ok(true)
        } else if let Err(err) = self.send_simulation(res_rx, sim_request, builder_info).await {
            trace.simulation = utcnow_ns();

            match &err {
                BlockSimError::BlockValidationFailed(reason) => {
                    warn!(err = %reason, "block validation failed");
                    Err(BuilderApiError::BlockValidationError(err))
                }

                BlockSimError::SimulationDropped => Err(BuilderApiError::BlockValidationError(err)),

                _ => {
                    error!(%err, "error simulating block");
                    Err(BuilderApiError::InternalError)
                }
            }
        } else {
            trace.simulation = utcnow_ns();

            debug!(
                sim_latency = trace.simulation.saturating_sub(trace.signature),
                "block simulation successful"
            );

            Ok(false)
        }
    }

    fn should_process_optimistically(
        &self,
        request: &BlockSimRequest,
        builder_info: &BuilderInfo,
    ) -> bool {
        if builder_info.is_optimistic && request.message.value <= builder_info.collateral {
            if request.proposer_preferences.filtering.is_regional() &&
                !builder_info.can_process_regional_slot_optimistically()
            {
                return false;
            }

            if self.failsafe_triggered.load(Ordering::Relaxed) ||
                !self.accept_optimistic.load(Ordering::Relaxed)
            {
                return false;
            }

            return true;
        }

        false
    }

    async fn send_simulation(
        &self,
        res_rx: oneshot::Receiver<SimResult>,
        request: SimulatorRequest,
        builder_info: &BuilderInfo,
    ) -> Result<(), BlockSimError> {
        let bid_slot = request.bid_slot();
        let builder = *request.builder_pubkey();
        let block_hash = request.request.message.block_hash;

        if let Err(err) = self.sim_requests_tx.send(request).await {
            error!(%err, "failed to send sim to manager, this should never happen");
            return Err(BlockSimError::NoSimulatorAvailable)
        }

        let Ok(res) = res_rx.await else {
            warn!("request was dropped by manager");
            return Err(BlockSimError::SimulationDropped)
        };

        if let Err(err) = res {
            if builder_info.is_optimistic {
                if err.is_already_known() {
                    warn!(
                        %builder,
                        %block_hash,
                        "Block already known. Skipping demotion"
                    );
                    return Ok(());
                }

                if err.is_too_old() {
                    warn!(
                        %builder,
                        %block_hash,
                        "Block is too old. Skipping demotion"
                    );
                    return Ok(());
                }

                if err.is_temporary() {
                    // this will have paused already optimistic simulations in the sim manager
                    warn!(
                        %builder,
                        %block_hash,
                        %err,
                        "Temporary error. Skipping demotion"
                    );
                    return Ok(());
                }

                warn!(
                    %builder,
                    %block_hash,
                    %err,
                    "Block simulation resulted in an error. Demoting builder...",
                );

                self.demote_builder_due_to_error(bid_slot, &builder, &block_hash, err.to_string())
                    .await;
            }

            return Err(err);
        }

        Ok(())
    }

    /// Demotes a builder in the `auctioneer` and `db`.
    ///
    /// If demotion fails, the failsafe is triggered to halt all optimistic simulations.
    async fn demote_builder_due_to_error(
        &self,
        slot: u64,
        builder_public_key: &BlsPublicKeyBytes,
        block_hash: &B256,
        reason: String,
    ) {
        SimulatorMetrics::demotion_count();

        if let Err(err) = self.auctioneer.demote_builder(builder_public_key) {
            self.failsafe_triggered.store(true, Ordering::Relaxed);
            error!(
                builder=%builder_public_key,
                err=%err,
                "Failed to demote builder in auctioneer"
            );
        }

        if let Err(err) =
            self.db.db_demote_builder(slot, builder_public_key, block_hash, reason).await
        {
            self.failsafe_triggered.store(true, Ordering::Relaxed);
            error!(
                builder=%builder_public_key,
                err=%err,
                "Failed to demote builder in database"
            );
        }
    }

    /// Checks if the builder has enough collateral to submit an optimistic bid.
    /// Or if the builder is not optimistic.
    ///
    /// This function compares the builder's collateral with the block value for a bid submission.
    /// If the builder's collateral is less than the required value, it returns an error.
    pub(crate) fn check_builder_collateral(
        payload: &impl BidSubmission,
        builder_info: &BuilderInfo,
    ) -> Result<(), BuilderApiError> {
        if !builder_info.is_optimistic {
            warn!(
                builder=%payload.builder_public_key(),
                "builder is not optimistic"
            );
            return Err(BuilderApiError::BuilderNotOptimistic {
                builder_pub_key: *payload.builder_public_key(),
            });
        } else if builder_info.collateral < payload.value() {
            warn!(
                builder=?payload.builder_public_key(),
                collateral=%builder_info.collateral,
                collateral_required=%payload.value(),
                "builder does not have enough collateral"
            );
            return Err(BuilderApiError::NotEnoughOptimisticCollateral {
                builder_pub_key: *payload.builder_public_key(),
                collateral: builder_info.collateral,
                collateral_required: payload.value(),
                is_optimistic: builder_info.is_optimistic,
            });
        }

        // Builder has enough collateral
        Ok(())
    }

    /// Fetch the builder's information. Auto-register if unknown.
    /// 
    /// If a builder is not found in the cache, this method will:
    /// 1. Log a warning about the new builder
    /// 2. Update the in-memory cache immediately (prevents repeated warnings)
    /// 3. Persist to the database asynchronously (survives cache refresh)
    /// 4. Return default BuilderInfo with conservative settings
    pub(crate) fn fetch_builder_info(&self, builder_pub_key: &BlsPublicKeyBytes) -> BuilderInfo {
        match self.auctioneer.get_builder_info(builder_pub_key) {
            Ok(info) => info,
            Err(_err) => {
                // First time seeing this builder - auto-register with basic access
                warn!(
                    builder=?builder_pub_key,
                    "New builder detected - auto-registering with basic access"
                );
                
                let default_info = BuilderInfo {
                    collateral: U256::ZERO,
                    is_optimistic: false,
                    is_optimistic_for_regional_filtering: false,
                    builder_id: None,
                    builder_ids: None,
                    api_key: None,
                };
                
                // Update cache immediately (prevents repeated warnings)
                let builder_config = BuilderConfig {
                    pub_key: *builder_pub_key,
                    builder_info: default_info.clone(),
                };
                self.auctioneer.update_builder_infos(&[builder_config], false);
                
                // Persist to database async (survives cache refresh)
                let db = self.db.clone();
                let builder_pub_key_clone = *builder_pub_key;
                let info_clone = default_info.clone();
                tokio::spawn(async move {
                    if let Err(e) = db.store_builder_info(&builder_pub_key_clone, &info_clone).await {
                        error!(
                            builder=?builder_pub_key_clone,
                            error=%e,
                            "Failed to persist auto-registered builder to database"
                        );
                    }
                });
                
                default_info
            }
        }
    }

    pub(crate) async fn demote_builder(
        &self,
        slot: u64,
        builder: &BlsPublicKeyBytes,
        block_hash: &B256,
        err: &BuilderApiError,
    ) {
        if let BuilderApiError::BlockValidationError(sim_err) = err {
            if sim_err.is_temporary() {
                return;
            }
        }

        error!(%err, %builder, "verification failed. Demoting builder!");

        if let Err(err) = self.auctioneer.demote_builder(builder) {
            error!(%err, %builder, "failed to demote builder in auctioneer");
        }

        if let Err(err) =
            self.db.db_demote_builder(slot, builder, block_hash, err.to_string()).await
        {
            error!(%err,  %builder, "Failed to demote builder in database");
        }
    }

    /// Validates the sequence number and updates the local cache, returns error if we've seen a
    /// higher sequence number for the same builder and bid slot.
    ///
    /// Assume the slot is already validated
    pub(crate) fn check_and_update_sequence_number(
        &self,
        builder_pubkey: &BlsPublicKeyBytes,
        bid_slot: Slot,
        headers: &HeaderMap,
    ) -> Result<(), BuilderApiError> {
        let Some(new_seq) = headers
            .get(HEADER_SEQUENCE)
            .and_then(|seq| seq.to_str().ok())
            .and_then(|seq| seq.parse::<u64>().ok())
        else {
            return Ok(());
        };

        if let Some(mut entry) = self.sequence_numbers.get_mut(builder_pubkey) {
            let (old_slot, old_seq) = entry.value_mut();

            if bid_slot < *old_slot {
                // this shouldn't really happen, ignore
            } else if bid_slot > *old_slot {
                // first seq for slot, reset
                *old_slot = bid_slot;
                *old_seq = new_seq;
            } else if new_seq > *old_seq {
                // higher sequence number, update
                *old_seq = new_seq;
            } else {
                // stale or duplicated sequence number
                return Err(BuilderApiError::OutOfSequence {
                    seen: *old_seq,
                    this: new_seq,
                    bid_slot: bid_slot.as_u64(),
                });
            }
        } else {
            self.sequence_numbers.insert(*builder_pubkey, (bid_slot, new_seq));
        }

        Ok(())
    }
}

/// `decode_payload` decodes the payload into a `SignedBidSubmission` object.
///
/// - Supports both SSZ and JSON encodings for deserialization.
/// - Automatically falls back to JSON if SSZ deserialization fails.
/// - Handles GZIP-compressed payloads.
///
/// Returns (skip_sigverify, payload)
#[tracing::instrument(skip_all)]
pub async fn decode_payload<A: Api>(
    bid_slot: u64,
    api: &BuilderApi<A>,
    headers: &HeaderMap,
    body_bytes: bytes::Bytes,
    trace: &mut SubmissionTrace,
) -> Result<(bool, SignedBidSubmissionWithMergingData), BuilderApiError> {
    const TRUE_HEADER: HeaderValue = HeaderValue::from_static("true");
    const HEADER_IS_MERGEABLE: &str = "x-mergeable";

    let has_mergeable_data =
        matches!(headers.get(HEADER_IS_MERGEABLE), Some(header) if header == TRUE_HEADER);

    let decoder = SubmissionDecoder::from_headers(headers);

    let should_hydrate = headers.get(HEADER_HYDRATE).is_some();
    let (skip_sigverify, payload_with_merging_data): (bool, SignedBidSubmissionWithMergingData) =
        if should_hydrate {
            let dehydrated_payload: DehydratedBidSubmission = decoder.decode(body_bytes)?;

            // caches are per builder and the builder pubkey is still unvalidated so we rely on the
            // api key pubkey for safety
            let skip_sigverify = headers.get(HEADER_API_KEY).is_some_and(|key| {
                api.auctioneer.validate_api_key(key, dehydrated_payload.builder_pubkey())
            });

            if !skip_sigverify {
                return Err(BuilderApiError::UntrustedBuilderOnDehydratedPayload);
            }

            let start = Instant::now();
            let (tx, rx) = oneshot::channel();
            api.hydration_tx
                .send((bid_slot, dehydrated_payload, tx))
                .await
                .inspect_err(|_| error!("failed to send dehydrated payload to hydration task"))
                .map_err(|_| BuilderApiError::InternalError)?;

            let res = match tokio::time::timeout(Duration::from_millis(500), rx).await {
                Ok(Ok(res)) => res.map_err(BuilderApiError::HydrationError),
                _ => {
                    error!("timed out waiting for hydrated payload");
                    Err(BuilderApiError::InternalError)
                }
            }?;

            HYDRATION_LATENCY.observe(start.elapsed().as_micros() as f64);

            // TODO: add support for merging data on dehydrated payloads
            let payload_with_merging_data = SignedBidSubmissionWithMergingData {
                submission: res,
                merging_data: Default::default(),
            };
            (skip_sigverify, payload_with_merging_data)
        } else {
            let payload_with_merging_data: SignedBidSubmissionWithMergingData =
                if has_mergeable_data {
                    decoder.decode(body_bytes)?
                } else {
                    let submission: SignedBidSubmission = decoder.decode(body_bytes)?;
                    SignedBidSubmissionWithMergingData {
                        submission,
                        merging_data: Default::default(),
                    }
                };
            let payload = &payload_with_merging_data.submission;

            let skip_sigverify = headers.get(HEADER_API_KEY).is_some_and(|key| {
                api.auctioneer.validate_api_key(key, payload.builder_public_key())
            });

            (skip_sigverify, payload_with_merging_data)
        };

    let payload = &payload_with_merging_data.submission;

    payload.validate_payload_ssz_lengths()?;

    trace.decode = utcnow_ns();
    debug!(
        skip_sigverify,
        timestamp_after_decoding = trace.decode,
        decode_latency_ns = trace.decode.saturating_sub(trace.receive),
        builder_pub_key = ?payload.builder_public_key(),
        block_hash = ?payload.block_hash(),
        proposer_pubkey = ?payload.proposer_public_key(),
        parent_hash = ?payload.parent_hash(),
        value = ?payload.value(),
        num_tx = payload.execution_payload_ref().transactions.len(),
        "payload info"
    );

    Ok((skip_sigverify, payload_with_merging_data))
}

/// - Validates the expected block.timestamp.
/// - Ensures that the fee recipients in the payload and proposer duty match.
/// - Ensures that the slot in the payload and payload attributes match.
/// - Validates that the block hash in the payload and message are the same.
/// - Validates that the parent hash in the payload and message are the same.
pub(crate) fn sanity_check_block_submission(
    payload: &impl BidSubmission,
    next_duty: &BuilderGetValidatorsResponseEntry,
    payload_attributes: &PayloadAttributesUpdate,
    chain_info: &ChainInfo,
) -> Result<(), BuilderApiError> {
    // Check block is for current fork
    if chain_info.current_fork_name() != payload.fork_name() {
        return Err(BuilderApiError::InvalidPayloadType {
            fork_name: chain_info.current_fork_name(),
        });
    }

    // checks internal consistency of the payload
    payload.validate()?;

    let bid_trace = payload.bid_trace();

    let expected_timestamp =
        chain_info.genesis_time_in_secs + (bid_trace.slot * chain_info.seconds_per_slot());
    if payload.timestamp() != expected_timestamp {
        return Err(BuilderApiError::IncorrectTimestamp {
            got: payload.timestamp(),
            expected: expected_timestamp,
        });
    }

    // Check duty
    if next_duty.entry.registration.message.fee_recipient != *payload.proposer_fee_recipient() {
        return Err(BuilderApiError::FeeRecipientMismatch {
            got: *payload.proposer_fee_recipient(),
            expected: next_duty.entry.registration.message.fee_recipient,
        });
    }

    if payload.slot() != next_duty.slot {
        return Err(BuilderApiError::SlotMismatch {
            got: payload.slot().into(),
            expected: next_duty.slot.into(),
        });
    }

    if next_duty.entry.registration.message.pubkey != bid_trace.proposer_pubkey {
        return Err(BuilderApiError::ProposerPublicKeyMismatch {
            got: bid_trace.proposer_pubkey,
            expected: next_duty.entry.registration.message.pubkey,
        });
    }

    // Check payload attrs
    if *payload.prev_randao() != payload_attributes.payload_attributes.prev_randao {
        return Err(BuilderApiError::PrevRandaoMismatch {
            got: *payload.prev_randao(),
            expected: payload_attributes.payload_attributes.prev_randao,
        });
    }

    let withdrawals_root = payload.withdrawals_root();

    let expected_withdrawals_root = payload_attributes.withdrawals_root;

    if withdrawals_root != expected_withdrawals_root {
        return Err(BuilderApiError::WithdrawalsRootMismatch {
            got: withdrawals_root,
            expected: expected_withdrawals_root,
        });
    }

    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum OrderValidationError {
    #[error("payload fee recipient ({got}) is not builder address ({expected})")]
    FeeRecipientMismatch { got: Address, expected: Address },
    #[error("invalid block merging tx index, got {got} with a tx count of {len}")]
    InvalidTxIndex { got: usize, len: usize },
    #[error("blob transaction does not reference any blobs")]
    EmptyBlobTransaction,
    #[error("blob transaction references blobs not in the block")]
    MissingBlobs,
    #[error("flagged indices reference tx outside of bundle")]
    FlaggedIndicesOutOfBounds,
}

/// Expands the references in [`BlockMergingData`] from the transactions in the
/// payload of the given submission. If any bundle references a transaction not in
/// the payload, it will be silently ignored.
pub fn get_mergeable_orders(
    payload: &SignedBidSubmission,
    merging_data: BlockMergingData,
) -> Result<MergeableOrders, OrderValidationError> {
    let execution_payload = payload.execution_payload_ref();
    if execution_payload.fee_recipient != merging_data.builder_address {
        return Err(OrderValidationError::FeeRecipientMismatch {
            got: merging_data.builder_address,
            expected: execution_payload.fee_recipient,
        });
    }
    let block_blobs_bundles = payload.blobs_bundle();
    let blob_versioned_hashes: Vec<_> =
        block_blobs_bundles.commitments().iter().map(|c| calculate_versioned_hash(*c)).collect();
    let txs = &execution_payload.transactions;

    // Expand all orders to include the tx's bytes, checking for missing blobs.
    let mergeable_orders = merging_data
        .merge_orders
        .into_iter()
        .map(|order| order_to_mergeable(order, txs, &blob_versioned_hashes))
        .collect::<Result<Vec<_>, _>>()?;

    // Stores all block blobs inside a map keyed by versioned hash
    let blobs = blobs_bundle_to_hashmap(blob_versioned_hashes, &block_blobs_bundles);

    Ok(MergeableOrders::new(merging_data.builder_address, mergeable_orders, blobs))
}

fn blobs_bundle_to_hashmap(
    blob_versioned_hashes: Vec<B256>,
    bundle: &BlobsBundle,
) -> HashMap<B256, BlobWithMetadata> {
    let version = bundle.version();
    blob_versioned_hashes
        .into_iter()
        .zip(bundle.iter_blobs())
        .map(|(versioned_hash, (blob, commitment, proofs))| match version {
            BlobsBundleVersion::V1 => (
                versioned_hash,
                BlobWithMetadata::V1(BlobWithMetadataV1 {
                    commitment: *commitment,
                    proof: proofs[0],
                    blob: blob.clone(),
                }),
            ),
            BlobsBundleVersion::V2 => (
                versioned_hash,
                BlobWithMetadata::V2(BlobWithMetadataV2 {
                    commitment: *commitment,
                    proofs: proofs.to_vec(),
                    blob: blob.clone(),
                }),
            ),
        })
        .collect()
}

fn order_to_mergeable(
    order: Order,
    txs: &Transactions,
    blob_versioned_hashes: &[B256],
) -> Result<MergeableOrder, OrderValidationError> {
    match order {
        Order::Tx(tx) => {
            let Some(raw_tx) = txs.get(tx.index) else {
                return Err(OrderValidationError::InvalidTxIndex { got: tx.index, len: txs.len() });
            };
            if is_blob_transaction(raw_tx) {
                // If the tx references bundles not in the block, we drop it
                validate_blobs(raw_tx, blob_versioned_hashes)?;
            }

            let transaction = Bytes::from(raw_tx.to_vec());
            let mergeable_tx =
                MergeableTransaction { transaction, can_revert: tx.can_revert }.into();
            Ok(mergeable_tx)
        }
        Order::Bundle(bundle) => {
            bundle.validate().map_err(|_| OrderValidationError::FlaggedIndicesOutOfBounds)?;

            let transactions = bundle
                .txs
                .iter()
                .map(|tx_index| {
                    let Some(raw_tx) = txs.get(*tx_index) else {
                        return Err(OrderValidationError::InvalidTxIndex {
                            got: *tx_index,
                            len: txs.len(),
                        });
                    };

                    if is_blob_transaction(raw_tx) {
                        // If the tx references bundles not in the block, we drop the bundle
                        validate_blobs(raw_tx, blob_versioned_hashes)?;
                    }

                    Ok(Bytes::from_owner(raw_tx.to_vec()))
                })
                .collect::<Result<_, OrderValidationError>>()?;

            let BundleOrder { reverting_txs, dropping_txs, .. } = bundle;

            let mergeable_bundle =
                MergeableBundle { transactions, reverting_txs, dropping_txs }.into();
            Ok(mergeable_bundle)
        }
    }
}

fn is_blob_transaction(raw_tx: &[u8]) -> bool {
    // First byte is always the transaction type, or >= 0xc0 for legacy
    // (source: https://eips.ethereum.org/EIPS/eip-2718)
    raw_tx.first().is_some_and(|&b| b == TxType::Eip4844)
}

fn get_tx_versioned_hashes(mut raw_tx: &[u8]) -> Vec<B256> {
    use alloy_consensus::transaction::RlpEcdsaDecodableTx;
    TxEip4844::rlp_decode_with_signature(&mut raw_tx)
        .map(|(b, _)| b.blob_versioned_hashes)
        .unwrap_or(vec![])
}

fn validate_blobs(
    raw_tx: &[u8],
    blob_versioned_hashes: &[B256],
) -> Result<(), OrderValidationError> {
    let versioned_hashes = get_tx_versioned_hashes(raw_tx);
    let num_blobs = versioned_hashes.len();
    if num_blobs == 0 {
        return Err(OrderValidationError::EmptyBlobTransaction);
    }
    let mut missing_blobs =
        versioned_hashes.iter().map(|h| !blob_versioned_hashes.iter().any(|vh| vh == h));
    if missing_blobs.any(|f| f) {
        return Err(OrderValidationError::MissingBlobs);
    }
    Ok(())
}

fn calculate_versioned_hash(commitment: Bytes48) -> B256 {
    KzgCommitment(*commitment).calculate_versioned_hash()
}
