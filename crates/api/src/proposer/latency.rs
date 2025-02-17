use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::warn;

#[derive(Serialize)]
struct LatencyRequest {
    ip: String,
}

#[derive(Deserialize, Debug)]
pub struct LatencyResponse {
    pub ip: String,
    pub port: i64,
    pub rtt_min: i64,
    pub rtt_av: i64,
    pub method: String,
    pub created_at: String,
    pub updated_at: String,
}

// A latency estimator that queries an external latency service
pub struct LatencyEstimator {
    latency_service_uri: String,
    client: Client,
    // Default client round-trip time in milliseconds
    default_client_rtt_ms: u64,
    // Scale factor to compute handshake delay from RTT
    rtt_to_handshake_scale: f64,
    // Scale factor to compute response delay from RTT
    rtt_to_response_scale: f64,
}

impl LatencyEstimator {
    // Create a new latency estimator.
    pub fn new(
        latency_service_uri: String,
        default_client_rtt_ms: u64,
        rtt_to_handshake_scale: f64,
        rtt_to_response_scale: f64,
        latency_request_timeout_ms: u64,
    ) -> Self {
        let timeout = Duration::from_millis(latency_request_timeout_ms);
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build reqwest client");
        Self {
            latency_service_uri,
            client,
            default_client_rtt_ms,
            rtt_to_handshake_scale,
            rtt_to_response_scale,
        }
    }

    // Query the latency service for the given IP.
    // Returns `Some(response)` on success or `None` if any error occurs.
    pub async fn get_rtt(&self, ip: &str) -> Option<LatencyResponse> {
        let req_data = LatencyRequest {
            ip: ip.to_string(),
        };

        let resp = self
            .client
            .get(&self.latency_service_uri)
            .json(&req_data)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            warn!(
                "latency service returned status code: {}",
                resp.status()
            );
            return None;
        }
        resp.json::<LatencyResponse>().await.ok()
    }

    // Estimate the elapsed time since the client initiated the request and the
    // time needed for a response.
    // Returns a tuple `(elapsed_ms, response_ms)`.
    pub async fn estimate_timing(
        &self,
        header_start_ms: Option<u64>,
        client_ip: String,
        max_elapsed_ms: u64,
    ) -> (u64, u64) {
        let start_instant = Instant::now();

        // Use either header start time or latency service for RTT
        let (rtt, elapsed_ms) = if let Some(header_start) = header_start_ms {
            let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(dur) => dur.as_millis() as u64,
                Err(_) => {
                    warn!("System time error; falling back to max_elapsed_ms");
                    return (
                        max_elapsed_ms,
                        (self.default_client_rtt_ms as f64 * self.rtt_to_response_scale) as u64,
                    );
                }
            };

            let elapsed_from_header = now.saturating_sub(header_start);
            let rtt = (elapsed_from_header as f64 / self.rtt_to_handshake_scale) as u64;
            (rtt, elapsed_from_header)
        } else {
            // Query latency service for RTT
            let mut rtt = self.default_client_rtt_ms;
            if let Some(resp_data) = self.get_rtt(&client_ip).await {
                if resp_data.rtt_av >= 0 {
                    rtt = resp_data.rtt_av as u64;
                }
            } else {
                warn!("Failed to query latency service for IP {}", client_ip);
            }
            let elapsed_from_latency = (rtt as f64 * self.rtt_to_handshake_scale) as u64;
            (rtt, elapsed_from_latency)
        };

        // Cap at maxElapsedMs, and add time spent in this fn
        let elapsed_ms = elapsed_ms.min(max_elapsed_ms)
            + start_instant.elapsed().as_millis() as u64;
        let response_ms = (rtt as f64 * self.rtt_to_response_scale) as u64;

        (elapsed_ms, response_ms)
    }
}
