use std::sync::Arc;
use std::time::{Duration, Instant};

use helix_common::local_cache::LocalCache;
use helix_housekeeper::CurrentSlotInfo;
use tracing::{info, warn};

use crate::metrics::SLOT_COMPLETION_WAIT_SECONDS;

#[derive(Debug, Clone, Copy)]
pub enum TransitionReason {
    /// External shutdown signal (SIGTERM/SIGINT)
    Shutdown,
    /// Voluntary rotation at slot boundary
    Rotation,
    /// Lost lease unexpectedly (network issue, evicted)
    LeaseLost,
}

impl TransitionReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Shutdown => "shutdown",
            Self::Rotation => "rotation",
            Self::LeaseLost => "lease_lost",
        }
    }
}

/// Wait for safe transition point based on slot state
/// 
/// This function waits for either:
/// 1. get_payload to be delivered for the current slot, OR
/// 2. A timeout (4 seconds into the next slot by default)
///
/// This prevents dropping bids mid-slot and ensures proposers who received
/// a header can successfully call get_payload.
///
/// Used for both graceful shutdown and voluntary leader rotation.
pub async fn wait_for_safe_transition(
    current_slot_info: &CurrentSlotInfo,
    auctioneer: &Arc<LocalCache>,
    timeout_secs: u64,
    reason: TransitionReason,
) {
    let start = Instant::now();
    let current_slot = current_slot_info.head_slot();
    
    info!(
        current_slot = current_slot.as_u64(),
        timeout_secs,
        reason = reason.as_str(),
        "Waiting for safe transition point"
    );

    let timeout = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        // Check if payload has been delivered for current slot
        if let Some(delivered_slot) = auctioneer.get_last_slot_delivered() {
            if delivered_slot >= current_slot.as_u64() {
                let elapsed = start.elapsed();
                info!(
                    elapsed_secs = elapsed.as_secs_f64(),
                    delivered_slot,
                    current_slot = current_slot.as_u64(),
                    reason = reason.as_str(),
                    "Payload delivered, safe to transition"
                );
                SLOT_COMPLETION_WAIT_SECONDS.observe(elapsed.as_secs_f64());
                return;
            }
        }

        // Sleep briefly before checking again
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Timeout reached
    let elapsed = start.elapsed();
    warn!(
        elapsed_secs = elapsed.as_secs_f64(),
        current_slot = current_slot.as_u64(),
        reason = reason.as_str(),
        "Timed out waiting for safe transition point, proceeding anyway"
    );
    SLOT_COMPLETION_WAIT_SECONDS.observe(elapsed.as_secs_f64());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wait_for_safe_transition_immediate() {
        let (sorter_tx, _) = crossbeam_channel::bounded(1);
        let auctioneer = Arc::new(LocalCache::new(sorter_tx));
        let slot_info = CurrentSlotInfo::default();

        // Set delivered slot to current slot
        let _ = auctioneer.check_and_set_last_slot_and_hash_delivered(
            slot_info.head_slot().as_u64(),
            &alloy_primitives::B256::default(),
        );

        let start = Instant::now();
        wait_for_safe_transition(&slot_info, &auctioneer, 4, TransitionReason::Shutdown).await;
        let elapsed = start.elapsed();

        // Should return immediately since slot is already delivered
        assert!(elapsed < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn test_wait_for_safe_transition_timeout() {
        let (sorter_tx, _) = crossbeam_channel::bounded(1);
        let auctioneer = Arc::new(LocalCache::new(sorter_tx));
        let slot_info = CurrentSlotInfo::default();

        let start = Instant::now();
        wait_for_safe_transition(&slot_info, &auctioneer, 1, TransitionReason::Rotation).await;
        let elapsed = start.elapsed();

        // Should wait for the full timeout
        assert!(elapsed >= Duration::from_secs(1));
        assert!(elapsed < Duration::from_millis(1200)); // Allow some margin
    }
}

