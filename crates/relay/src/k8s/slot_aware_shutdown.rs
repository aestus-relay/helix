use std::sync::Arc;
use std::time::Duration;

use helix_common::{chain_info::ChainInfo, local_cache::LocalCache, utils::utcnow_sec};

use crate::housekeeper::CurrentSlotInfo;
use tracing::{info, warn};

use super::metrics::SLOT_COMPLETION_WAIT_SECONDS;

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

/// Wait for safe transition point based on slot advancement
/// 
/// This function waits for either:
///   1. An on_new_slot() event from the auctioneer
///   2. A fail-safe timeout 4 seconds into the next slot
///
/// Used for both graceful shutdown and voluntary leader rotation.
///
/// This prevents dropping bids mid-slot and ensures proposers who received
/// a header can successfully call get_payload.
pub async fn wait_for_safe_transition(
    current_slot_info: &CurrentSlotInfo,
    chain_info: &ChainInfo,
    _auctioneer: &Arc<LocalCache>,  // Kept for API compatibility but unused
    timeout_secs: u64,
    reason: TransitionReason,
) {
    let start_time = tokio::time::Instant::now();
    let initial_slot = current_slot_info.head_slot();
    
    // Calculate when the next slot starts (in Unix timestamp)
    let next_slot = initial_slot.as_u64() + 1;
    let next_slot_start_timestamp = 
        chain_info.genesis_time_in_secs + (next_slot * chain_info.seconds_per_slot());
    
    // Deadline is timeout_secs into the slot
    let deadline_timestamp = next_slot_start_timestamp + timeout_secs;
    let now = utcnow_sec();
    
    // Calculate how long to wait until the deadline
    let wait_duration = if deadline_timestamp > now {
        Duration::from_secs(deadline_timestamp - now)
    } else {
        // Deadline already passed, no need to wait
        Duration::from_secs(0)
    };
    
    let deadline = tokio::time::Instant::now() + wait_duration;
    
    info!(
        current_slot = initial_slot.as_u64(),
        next_slot,
        timeout_secs_into_slot = timeout_secs,
        wait_duration_secs = wait_duration.as_secs(),
        reason = reason.as_str(),
        "Waiting for safe transition point (slot advancement or timeout at slot boundary)"
    );

    while tokio::time::Instant::now() < deadline {
        let current_slot = current_slot_info.head_slot();
        
        // Slot advancement means auctioneer received new slot event and called
        // ctx.on_new_slot(), which cleans up all previous slot state.
        // This is our signal that:
        // 1. Previous payload was delivered (if we were in Broadcasting), OR
        // 2. Slot timed out/was missed (if we were in Sorting), OR  
        // 3. Slot just started (if we were in Slot)
        // In all cases: safe to transition leadership
        if current_slot > initial_slot {
            let elapsed = start_time.elapsed();
            info!(
                elapsed_secs = elapsed.as_secs_f64(),
                initial_slot = initial_slot.as_u64(),
                current_slot = current_slot.as_u64(),
                reason = reason.as_str(),
                "Slot advanced (auctioneer cleaned up), safe to transition"
            );
            SLOT_COMPLETION_WAIT_SECONDS.observe(elapsed.as_secs_f64());
            return;
        }

        // Sleep briefly before checking again
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Reached timeout (4s into next slot) without slot advancement
    // Fail safe could trigger if:
    // - Housekeeper is delayed in processing the new slot
    // - System is under extreme load
    let elapsed = start_time.elapsed();
    warn!(
        elapsed_secs = elapsed.as_secs_f64(),
        current_slot = current_slot_info.head_slot().as_u64(),
        timeout_secs_into_slot = timeout_secs,
        reason = reason.as_str(),
        "Reached timeout ({}s into next slot) without slot advancement, proceeding anyway",
        timeout_secs
    );
    SLOT_COMPLETION_WAIT_SECONDS.observe(elapsed.as_secs_f64());
}