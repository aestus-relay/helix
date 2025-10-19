use std::sync::Arc;
use std::time::{Duration, Instant};

use helix_common::local_cache::LocalCache;

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
///   1. The slot to advance beyond the initial slot, OR
///   2. A timeout (4 seconds by default)
///
/// Slot advancement indicates the auctioneer has received a new slot event and
/// called on_new_slot(), which cleans up all previous slot state. This means:
///
///   - If payload was delivered: Broadcasting state → new slot → cleanup happened
///   - If slot was empty/missed: Sorting/Slot state → new slot → cleanup happened
///
/// In all cases, slot advancement = safe transition point.
///
/// This prevents dropping bids mid-slot and ensures proposers who received
/// a header can successfully call get_payload.
///
/// Used for both graceful shutdown and voluntary leader rotation.
pub async fn wait_for_safe_transition(
    current_slot_info: &CurrentSlotInfo,
    _auctioneer: &Arc<LocalCache>,  // Kept for API compatibility but unused
    timeout_secs: u64,
    reason: TransitionReason,
) {
    let start = Instant::now();
    let initial_slot = current_slot_info.head_slot();
    
    info!(
        current_slot = initial_slot.as_u64(),
        timeout_secs,
        reason = reason.as_str(),
        "Waiting for safe transition point (slot advancement indicates auctioneer cleanup completed)"
    );

    let timeout = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let current_slot = current_slot_info.head_slot();
        
        // Slot advancement means auctioneer received new slot event and called
        // ctx.on_new_slot(), which cleans up all previous slot state.
        // This is our signal that:
        // 1. Previous payload was delivered (if we were in Broadcasting), OR
        // 2. Slot timed out/was missed (if we were in Sorting), OR  
        // 3. Slot just started (if we were in Slot)
        // In all cases: safe to transition leadership
        if current_slot > initial_slot {
            let elapsed = start.elapsed();
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

    // Timeout reached - this is the safety net
    // Could happen if:
    // - We're at the very end of a slot and next slot hasn't started
    // - Housekeeper is delayed
    // - System is under extreme load
    let elapsed = start.elapsed();
    warn!(
        elapsed_secs = elapsed.as_secs_f64(),
        current_slot = current_slot_info.head_slot().as_u64(),
        reason = reason.as_str(),
        "Timed out waiting for slot advancement, proceeding anyway (safety timeout)"
    );
    SLOT_COMPLETION_WAIT_SECONDS.observe(elapsed.as_secs_f64());
}