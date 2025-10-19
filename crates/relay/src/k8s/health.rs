use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{http::StatusCode, response::IntoResponse};

/// Health check endpoint for K8s readinessProbe
/// Returns 200 OK if this pod is the leader and not terminating
/// Returns 503 SERVICE_UNAVAILABLE if follower or terminating
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
}

