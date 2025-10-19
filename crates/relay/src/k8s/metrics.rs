use prometheus::{IntCounter, IntGauge, Histogram};

lazy_static::lazy_static! {
    /// Prometheus registry for K8s metrics
    static ref K8S_METRICS_REGISTRY: prometheus::Registry = prometheus::Registry::new();

    /// Current leader state (1=leader, 0=follower)
    pub static ref LEADER_ELECTION_STATE: IntGauge =
        prometheus::register_int_gauge_with_registry!(
            "helix_leader_election_state",
            "Current leader state (1=leader, 0=follower)",
            &K8S_METRICS_REGISTRY
        ).unwrap();

    /// Total number of successful lease renewals
    pub static ref LEASE_RENEWALS_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "helix_lease_renewals_total",
            "Total number of successful lease renewals",
            &K8S_METRICS_REGISTRY
        ).unwrap();

    /// Total number of lease renewal failures
    pub static ref LEASE_FAILURES_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "helix_lease_failures_total",
            "Total number of lease renewal failures",
            &K8S_METRICS_REGISTRY
        ).unwrap();

    /// Total number of leader transitions
    pub static ref LEADER_TRANSITIONS_TOTAL: IntCounter =
        prometheus::register_int_counter_with_registry!(
            "helix_leader_transitions_total",
            "Total number of leader transitions",
            &K8S_METRICS_REGISTRY
        ).unwrap();

    /// Time waited for slot completion during shutdown
    pub static ref SLOT_COMPLETION_WAIT_SECONDS: Histogram =
        prometheus::register_histogram_with_registry!(
            "helix_slot_completion_wait_seconds",
            "Time waited for slot completion during shutdown",
            &K8S_METRICS_REGISTRY
        ).unwrap();
}

