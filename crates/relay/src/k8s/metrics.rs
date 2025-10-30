use helix_common::metrics::RELAY_METRICS_REGISTRY;
use prometheus::{IntCounter, IntGauge, Histogram};

lazy_static::lazy_static! {
    /// Current leader state (1=leader, 0=follower)
    pub static ref LEADER_ELECTION_STATE: IntGauge =
        prometheus::register_int_gauge_with_registry!(
            "leader_election_state",
            "Current leader state (1=leader, 0=follower)",
            &RELAY_METRICS_REGISTRY
        ).unwrap();

    /// Total number of successful lease renewals
    pub static ref LEASE_RENEWALS_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "lease_renewals_total",
            "Total number of successful lease renewals",
            &RELAY_METRICS_REGISTRY
        ).unwrap();

    /// Total number of lease renewal failures
    pub static ref LEASE_FAILURES_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "lease_failures_total",
            "Total number of lease renewal failures",
            &RELAY_METRICS_REGISTRY
        ).unwrap();

    /// Total number of leader transitions
    pub static ref LEADER_TRANSITIONS_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "leader_transitions_total",
            "Total number of leader transitions",
            &RELAY_METRICS_REGISTRY
        ).unwrap();

    /// Time waited for slot completion during shutdown
    pub static ref SLOT_COMPLETION_WAIT_SECONDS: Histogram =
        prometheus::register_histogram_with_registry!(
            "slot_completion_wait_seconds",
            "Time waited for slot completion during shutdown",
            &RELAY_METRICS_REGISTRY
        ).unwrap();
}

/// Initialize K8s metrics to ensure they are registered with the registry
/// This must be called before the metrics server starts gathering metrics
/// Forces lazy_static initialization by accessing each metric, which registers
/// them with RELAY_METRICS_REGISTRY immediately
pub fn init_k8s_metrics() {
    // Force lazy_static initialization by accessing each metric
    // This registers them with RELAY_METRICS_REGISTRY immediately
    let _ = &*LEADER_ELECTION_STATE;
    let _ = &*LEASE_RENEWALS_TOTAL;
    let _ = &*LEASE_FAILURES_TOTAL;
    let _ = &*LEADER_TRANSITIONS_TOTAL;
    let _ = &*SLOT_COMPLETION_WAIT_SECONDS;
}

