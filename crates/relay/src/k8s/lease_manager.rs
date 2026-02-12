use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use rustls::crypto::{ring, CryptoProvider};

use helix_common::{chain_info::ChainInfo, K8sLeaderElectionConfig};

use crate::housekeeper::CurrentSlotInfo;
use k8s_openapi::api::coordination::v1::Lease;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use k8s_openapi::chrono::Utc;
use kube::{
    api::{Api, Patch, PatchParams, PostParams},
    Client,
};
use parking_lot::RwLock;
use rand::Rng;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::{
    metrics::{LEADER_ELECTION_STATE, LEADER_TRANSITIONS_TOTAL, LEASE_FAILURES_TOTAL, LEASE_RENEWALS_TOTAL},
    slot_aware_shutdown::{wait_for_safe_transition, TransitionReason},
};

// Constants for lease management
const ROTATION_BACKOFF_MULTIPLIER: f64 = 3.0;
const RETRY_JITTER_PERCENT: f64 = 0.3; // 30% jitter to prevent synchronized retries

/// Ensure TLS crypto provider is installed (required for kube-rs)
fn ensure_tls_provider() {
    if CryptoProvider::get_default().is_none() {
        let _ = ring::default_provider().install_default();
    }
}

/// Calculate random jitter to prevent synchronized retries
/// Returns a value between 0.0 and RETRY_JITTER_PERCENT
fn calculate_random_jitter() -> f64 {
    let mut rng = rand::rng();
    rng.random_range(0.0..RETRY_JITTER_PERCENT)
}

/// Sleep with jittered retry period to prevent thundering herd
/// Jitter spreads out follower retry attempts to reduce lease contention
async fn sleep_with_jitter(base_interval_secs: f64) {
    let jitter = calculate_random_jitter();
    let jittered_interval = base_interval_secs * (1.0 + jitter);
    
    debug!(
        base_interval = base_interval_secs,
        jitter_pct = jitter * 100.0,
        jittered_interval,
        "Sleeping with jitter to prevent synchronized retries"
    );
    
    sleep(Duration::from_secs_f64(jittered_interval)).await;
}

#[derive(thiserror::Error, Debug)]
pub enum LeaseError {
    #[error("Kubernetes API error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("Failed to read pod identity")]
    PodIdentityError,

    #[error("Failed to read namespace")]
    NamespaceError,

    #[error("Lease acquisition failed")]
    AcquisitionFailed,
}

/// Manages Kubernetes Lease-based leader election
#[derive(Clone)]
pub struct LeaseManager {
    config: K8sLeaderElectionConfig,
    client: Client,
    namespace: String,
    pod_name: String,
    is_leader: Arc<AtomicBool>,
    current_slot_info: CurrentSlotInfo,
    chain_info: Arc<ChainInfo>,
    shutdown_signal: Arc<AtomicBool>,
    leadership_acquired_slot: Arc<RwLock<Option<u64>>>,
    /// Cancellation token for the current leadership epoch.
    /// Cancelled in `on_lost_leadership()` to close all WebSocket connections.
    /// Replaced with a fresh token in `on_elected_leader()`.
    ws_cancellation: Arc<RwLock<CancellationToken>>,
}

impl LeaseManager {
    /// Create a new LeaseManager
    pub async fn new(
        config: K8sLeaderElectionConfig,
        is_leader: Arc<AtomicBool>,
        ws_cancellation: Arc<RwLock<CancellationToken>>,
        current_slot_info: &CurrentSlotInfo,
        chain_info: Arc<ChainInfo>,
    ) -> Result<Self, LeaseError> {
        // Ensure TLS provider is installed (required for kube-rs)
        ensure_tls_provider();

        let client = Client::try_default().await?;

        // Read pod name from hostname environment variable (set by K8s)
        let pod_name = std::env::var("HOSTNAME").map_err(|_| LeaseError::PodIdentityError)?;

        // Read namespace from service account
        let namespace = tokio::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
            .await
            .map_err(|_| LeaseError::NamespaceError)?;

        info!(
            pod_name,
            namespace,
            lease_name = config.lease_name,
            "Initialized K8s lease manager"
        );

        Ok(Self {
            config,
            client,
            namespace,
            pod_name,
            is_leader,
            chain_info,
            current_slot_info: current_slot_info.clone(),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            leadership_acquired_slot: Arc::new(RwLock::new(None)),
            ws_cancellation,
        })
    }

    /// Start the lease manager background task
    pub async fn start(&self) -> Result<(), LeaseError> {
        let manager = self.clone();
        
        tokio::spawn(async move {
            if let Err(err) = manager.run().await {
                error!(%err, "Lease manager error");
            }
        });

        Ok(())
    }

    /// Main lease management loop
    async fn run(&self) -> Result<(), LeaseError> {
        loop {
            // Check if shutdown was requested
            if self.shutdown_signal.load(Ordering::Relaxed) {
                info!("Shutdown signal received, exiting lease manager");
                break;
            }

            // Try to acquire or renew the lease
            match self.try_acquire_lease().await {
                Ok(acquired) => {
                    if acquired {
                        // Atomic verification: update_lease() already verified we're the holder
                        // No separate verification needed - if update_lease() returns true,
                        // we already confirmed ownership from the patch response
                        self.on_elected_leader();
                        
                        // Keep renewing until we lose it or rotate
                        let result = self.renew_lease_loop().await;
                        
                        self.on_lost_leadership();
                        
                        // Check if this was a voluntary rotation vs error
                        let was_rotation = result.is_ok();
                        
                        if was_rotation {
                            // Voluntary rotation - wait longer to let others compete
                            let backoff_secs = self.config.retry_period_seconds * ROTATION_BACKOFF_MULTIPLIER;
                            info!(
                                backoff_secs,
                                "Rotation complete, backing off to allow others to acquire lease"
                            );
                            sleep_with_jitter(backoff_secs).await;
                        } else {
                            // Error/lost lease unexpectedly - retry sooner with jitter
                            if let Err(err) = result {
                                error!(%err, "Lease renewal failed");
                            }
                            sleep_with_jitter(self.config.retry_period_seconds).await;
                        }
                    } else {
                        // We're a follower, wait before retrying with jitter
                        sleep_with_jitter(self.config.retry_period_seconds).await;
                    }
                }
                Err(err) => {
                    error!(%err, "Failed to acquire lease");
                    LEASE_FAILURES_TOTAL.inc();
                    sleep_with_jitter(self.config.retry_period_seconds).await;
                }
            }
        }

        Ok(())
    }

    /// Try to acquire the lease
    async fn try_acquire_lease(&self) -> Result<bool, LeaseError> {
        let leases: Api<Lease> = Api::namespaced(self.client.clone(), &self.namespace);

        // Try to get existing lease
        match leases.get(&self.config.lease_name).await {
            Ok(lease) => {
                // Lease exists, check if we can take it over
                if let Some(spec) = &lease.spec {
                    if let Some(holder_identity) = &spec.holder_identity {
                        if holder_identity == &self.pod_name {
                            // We already hold the lease
                            return Ok(true);
                        }

                        // Check if lease is expired
                        if let Some(renew_time) = &spec.renew_time {
                            let now = Utc::now();
                            
                            let time_since_renew = now.signed_duration_since(renew_time.0);
                            let time_since_renew_secs = time_since_renew.num_milliseconds() as f64 / 1000.0;
                            
                            if time_since_renew_secs > self.config.lease_duration_seconds {
                                // Lease expired, try to take it
                                info!(
                                    pod_name = self.pod_name,
                                    current_holder = holder_identity,
                                    seconds_since_renew = time_since_renew_secs,
                                    lease_duration = self.config.lease_duration_seconds,
                                    "Lease expired, attempting to acquire"
                                );
                                return self.update_lease(leases, true).await;
                            } else {
                                // Lease is fresh, log and wait
                                debug!(
                                    pod_name = self.pod_name,
                                    current_holder = holder_identity,
                                    seconds_since_renew = time_since_renew_secs,
                                    lease_duration = self.config.lease_duration_seconds,
                                    "Lease is active, waiting"
                                );
                            }
                        }

                        // Lease is held by someone else and not expired
                        return Ok(false);
                    }
                }

                // Lease exists but has no holder, try to acquire it
                self.update_lease(leases, true).await
            }
            Err(kube::Error::Api(err)) if err.code == 404 => {
                // Lease doesn't exist, create it
                info!("Lease does not exist, creating");
                self.create_lease(leases).await
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Create a new lease
    async fn create_lease(&self, leases: Api<Lease>) -> Result<bool, LeaseError> {
        let now = MicroTime(Utc::now());
        
        let lease = Lease {
            metadata: ObjectMeta {
                name: Some(self.config.lease_name.clone()),
                namespace: Some(self.namespace.clone()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::coordination::v1::LeaseSpec {
                holder_identity: Some(self.pod_name.clone()),
                lease_duration_seconds: Some(self.config.lease_duration_seconds as i32),
                acquire_time: Some(now.clone()),
                renew_time: Some(now),
                lease_transitions: Some(0),
            }),
        };

        match leases.create(&PostParams::default(), &lease).await {
            Ok(created_lease) => {
                // Verify we're the holder (detect race conditions)
                if let Some(spec) = &created_lease.spec {
                    if let Some(holder) = &spec.holder_identity {
                        if holder == &self.pod_name {
                            info!("Successfully created and acquired lease");
                            return Ok(true);
                        } else {
                            warn!(
                                expected = self.pod_name,
                                actual = holder,
                                "Created lease but not the holder (race condition)"
                            );
                            return Ok(false);
                        }
                    }
                }
                // Shouldn't happen, but be safe
                warn!("Created lease but couldn't verify holder");
                Ok(false)
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                // Someone else created it first
                info!("Lease was created by another pod");
                Ok(false)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Update an existing lease
    /// Returns Ok(true) if we successfully acquired/renewed and are the holder
    /// Returns Ok(false) if there was a conflict or we're not the holder
    /// Uses atomic verification: verifies ownership from the patch response (no separate API call)
    /// 
    /// During renewal, we do NOT include holder_identity in the patch.
    /// This prevents Strategic Merge PATCH from allowing non-leaders to steal the lease.
    /// Only during acquisition do we set holder_identity.
    async fn update_lease(&self, leases: Api<Lease>, is_acquisition: bool) -> Result<bool, LeaseError> {
        let now = MicroTime(Utc::now());
        
        // For renewal, only update renew_time - do not change holder_identity
        // For acquisition, we can change holder_identity to claim ownership
        let lease_spec = if is_acquisition {
            // Acquisition: set holder_identity to claim the lease
            k8s_openapi::api::coordination::v1::LeaseSpec {
                holder_identity: Some(self.pod_name.clone()),
                lease_duration_seconds: Some(self.config.lease_duration_seconds as i32),
                renew_time: Some(now.clone()),
                acquire_time: Some(now),
                ..Default::default()
            }
        } else {
            // Renewal: only update renew_time and lease_duration_seconds
            // Leave holder_identity unchanged - this prevents split-brain scenarios
            k8s_openapi::api::coordination::v1::LeaseSpec {
                lease_duration_seconds: Some(self.config.lease_duration_seconds as i32),
                renew_time: Some(now.clone()),
                ..Default::default()
            }
        };

        let patch = serde_json::json!({
            "spec": lease_spec
        });

        debug!(
            pod_name = self.pod_name,
            is_acquisition,
            "Patching lease with {}",
            if is_acquisition { "holder_identity set (acquisition)" } else { "renew_time only (renewal)" }
        );

        match leases
            .patch(
                &self.config.lease_name,
                &PatchParams::default(),
                &Patch::Strategic(&patch),  // Strategic merge handles concurrent updates more gracefully
            )
            .await
        {
            Ok(updated_lease) => {
                // Atomic verification: verify immediately from the returned object (no extra API call needed)
                let holder = updated_lease
                    .spec
                    .and_then(|spec| spec.holder_identity)
                    .unwrap_or_default();
                
                if holder == self.pod_name {
                    if is_acquisition {
                        info!(
                            pod_name = self.pod_name,
                            "Successfully acquired lease (verified atomically)"
                        );
                    }
                    Ok(true)  // We successfully acquired/renewed and are the holder
                } else {
                    // Race condition: patch succeeded but we're not the holder
                    // During renewal: this means we lost leadership (another pod acquired it)
                    // During acquisition: concurrent acquisition race
                    warn!(
                        pod_name = self.pod_name,
                        actual_holder = holder,
                        is_acquisition,
                        "Lease updated but not the holder (race condition detected) - stepping down"
                    );
                    Ok(false)
                }
            }
            Err(kube::Error::Api(err)) if err.code == 409 => {
                // Conflict, someone else updated it
                debug!(
                    pod_name = self.pod_name,
                    is_acquisition,
                    "Lease patch conflict (409) - another pod updated it"
                );
                Ok(false)
            }
            Err(err) => {
                error!(
                    %err,
                    pod_name = self.pod_name,
                    is_acquisition,
                    "Failed to patch lease"
                );
                Err(err.into())
            }
        }
    }

    /// Continuously renew the lease while we're the leader
    async fn renew_lease_loop(&self) -> Result<(), LeaseError> {
        let leases: Api<Lease> = Api::namespaced(self.client.clone(), &self.namespace);
        // Use renew_deadline for leader renewals (gives head start before followers can steal)
        let renew_interval = Duration::from_secs_f64(self.config.renew_deadline_seconds);

        debug!(
            renew_interval_ms = renew_interval.as_millis(),
            "Starting lease renewal loop"
        );

        loop {
            // Check if shutdown was requested
            if self.shutdown_signal.load(Ordering::Relaxed) {
                info!("Shutdown requested, stopping lease renewal");
                break;
            }

            sleep(renew_interval).await;

            debug!(
                renew_interval_ms = renew_interval.as_millis(),
                "Renewing lease"
            );

            // Check if we should rotate (only if rotation tracking is active)
            if let Some(interval) = self.config.rotation_interval_slots {
                // Extract start_slot value to avoid holding lock across await
                let start_slot_opt = *self.leadership_acquired_slot.read();
                
                if let Some(start_slot) = start_slot_opt {
                    let current_slot = self.current_slot_info.head_slot().as_u64();
                    let slots_held = current_slot.saturating_sub(start_slot);
                    
                    if slots_held >= interval {
                        info!(
                            slots_held,
                            rotation_interval = interval,
                            start_slot,
                            current_slot,
                            "Rotation interval reached, initiating slot-aware transition"
                        );
                        
                        // Wait for slot completion before rotating
                        wait_for_safe_transition(
                            &self.current_slot_info,
                            &self.chain_info,
                            self.config.slot_completion_timeout_seconds,
                            TransitionReason::Rotation,
                        ).await;
                        
                        // Release lease to allow others to take over
                        self.release_lease().await;
                        
                        // Exit renewal loop, will trigger re-election
                        return Ok(());
                    }
                } else {
                    // Rotation tracking not yet active, initialize it if slot is valid
                    if let Some(start_slot) = self.initialize_rotation_tracking() {
                        *self.leadership_acquired_slot.write() = Some(start_slot);
                        info!(start_slot, "Rotation tracking initialized (slot became valid)");
                    }
                }
            }

            // Normal lease renewal
            match self.update_lease(leases.clone(), false).await {
                Ok(true) => {
                    // Atomic verification: update_lease() already verified we're the holder
                    // from the patch response, so no separate verification needed here
                    LEASE_RENEWALS_TOTAL.inc();
                    debug!(pod_name = self.pod_name, "Lease renewed successfully");
                }
                Ok(false) => {
                    // Renewal failed: either we're not the holder (lost leadership) or conflict occurred
                    warn!(
                        pod_name = self.pod_name,
                        "Lost lease during renewal - another pod is now the leader"
                    );
                    return Err(LeaseError::AcquisitionFailed);
                }
                Err(err) => {
                    error!(
                        %err,
                        pod_name = self.pod_name,
                        "Failed to renew lease - API error occurred"
                    );
                    LEASE_FAILURES_TOTAL.inc();
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    /// Initialize rotation tracking if slot is valid
    fn initialize_rotation_tracking(&self) -> Option<u64> {
        let current_slot = self.current_slot_info.head_slot().as_u64();
        if current_slot > 0 {
            Some(current_slot)
        } else {
            None
        }
    }

    /// Called when this pod becomes the leader
    fn on_elected_leader(&self) {
        let start_slot = self.initialize_rotation_tracking();
        *self.leadership_acquired_slot.write() = start_slot;
        
        // Mint a fresh cancellation token for this leadership epoch.
        // Any new WebSocket connections will clone this token.
        *self.ws_cancellation.write() = CancellationToken::new();

        match start_slot {
            Some(slot) => {
                info!(
                    pod_name = self.pod_name,
                    current_slot = slot,
                    "Became leader with rotation tracking enabled"
                );
            }
            None => {
                info!(
                    pod_name = self.pod_name,
                    "Became leader, rotation tracking deferred until slot initialized"
                );
            }
        }
        
        self.is_leader.store(true, Ordering::Relaxed);
        LEADER_ELECTION_STATE.set(1);
        LEADER_TRANSITIONS_TOTAL.inc();
    }

    /// Called when this pod loses leadership
    fn on_lost_leadership(&self) {
        warn!(pod_name = self.pod_name, "Lost leadership");

        // Cancel all WebSocket connections from this leadership epoch.
        // This must happen before flipping is_leader so that tasks waking
        // from the cancellation still see a consistent state.
        self.ws_cancellation.read().cancel();
        
        // Clear leadership slot tracking
        *self.leadership_acquired_slot.write() = None;
        
        self.is_leader.store(false, Ordering::Relaxed);
        LEADER_ELECTION_STATE.set(0);
        LEADER_TRANSITIONS_TOTAL.inc();
    }

    /// Release the lease (for rotation or shutdown)
    async fn release_lease(&self) {
        info!("Releasing lease");
        let leases: Api<Lease> = Api::namespaced(self.client.clone(), &self.namespace);
        
        if let Err(err) = leases.delete(&self.config.lease_name, &Default::default()).await {
            warn!(%err, "Failed to delete lease during release");
        }
        
        self.on_lost_leadership();
    }

    /// Gracefully shutdown: wait for slot completion then release lease
    pub async fn graceful_shutdown(&self) -> Result<(), LeaseError> {
        info!("Starting graceful shutdown");
        
        // Wait for current slot to complete
        wait_for_safe_transition(
            &self.current_slot_info,
            &self.chain_info,
            self.config.slot_completion_timeout_seconds,
            TransitionReason::Shutdown,
        )
        .await;

        // Signal the lease renewal loop to stop
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Release the lease
        if self.is_leader.load(Ordering::Relaxed) {
            self.release_lease().await;
        }

        Ok(())
    }
}