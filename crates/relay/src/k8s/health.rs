use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{http::StatusCode, response::IntoResponse};

/// Leader-specific health check endpoint for gateway active health checks
/// Returns 200 OK if this pod is the leader and not terminating
/// Returns 503 SERVICE_UNAVAILABLE if follower or terminating
///
/// Used by gateway implementations to route traffic only to the leader pod.
/// This endpoint should NOT be used for Kubernetes readinessProbe (use health_ready instead).
pub async fn health_leader(
    is_leader: Arc<AtomicBool>,
    terminating: Arc<AtomicBool>,
) -> impl IntoResponse {
    // If terminating, always return unavailable
    if terminating.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    // Return OK only if we're the leader
    if is_leader.load(Ordering::Relaxed) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Generic health check endpoint for Kubernetes readinessProbe
/// Returns 200 OK if pod is initialized and not terminating
/// Returns 503 SERVICE_UNAVAILABLE if terminating or still initializing
///
/// All pods (leader + followers) report ready to allow Kubernetes rolling updates.
/// Traffic routing to leader-only is handled by the gateway using health_leader.
pub async fn health_ready(
    known_validators_loaded: Arc<AtomicBool>,
    terminating: Arc<AtomicBool>,
) -> impl IntoResponse {
    // If terminating, always return unavailable
    if terminating.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    // Return unavailable if still initializing (validators not loaded)
    if !known_validators_loaded.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }

    // Pod is ready for service (both leader and followers report ready)
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_leader_when_leader() {
        let is_leader = Arc::new(AtomicBool::new(true));
        let terminating = Arc::new(AtomicBool::new(false));

        let response = health_leader(is_leader, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_leader_when_follower() {
        let is_leader = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        let response = health_leader(is_leader, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_leader_when_terminating() {
        let is_leader = Arc::new(AtomicBool::new(true));
        let terminating = Arc::new(AtomicBool::new(true));

        let response = health_leader(is_leader, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_ready_when_ready() {
        let known_validators_loaded = Arc::new(AtomicBool::new(true));
        let terminating = Arc::new(AtomicBool::new(false));

        let response = health_ready(known_validators_loaded, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_ready_when_initializing() {
        let known_validators_loaded = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        let response = health_ready(known_validators_loaded, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_ready_when_terminating() {
        let known_validators_loaded = Arc::new(AtomicBool::new(true));
        let terminating = Arc::new(AtomicBool::new(true));

        let response = health_ready(known_validators_loaded, terminating).await;
        assert_eq!(response.into_response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

