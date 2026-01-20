use std::{net::SocketAddr, sync::Arc};

use axum::{Extension, extract::ConnectInfo};
use helix_common::{
    self, RequestTimings, SubmissionTrace, api_provider::ApiProvider, utils::extract_request_id,
};
use http::HeaderMap;
use tracing::{error, trace};

use super::api::BuilderApi;
use crate::api::{Api, builder::error::BuilderApiError};

impl<A: Api> BuilderApi<A> {
    /// Implements this API: <https://flashbots.github.io/relay-specs/#/Builder/submitBlock>
    #[tracing::instrument(skip_all, err(level = tracing::Level::TRACE),
        fields(
        id =% extract_request_id(&headers),
        slot = tracing::field::Empty, // submission slot
        builder_pubkey = tracing::field::Empty,
        builder_id = tracing::field::Empty,
        block_hash = tracing::field::Empty,
        client_ip = tracing::field::Empty,
    ))]
    pub async fn submit_block(
        Extension(api): Extension<Arc<BuilderApi<A>>>,
        Extension(timings): Extension<RequestTimings>,
        ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
        headers: HeaderMap,
        body: bytes::Bytes,
    ) -> Result<(), BuilderApiError> {
        trace!("start handler");

        let client_ip = get_client_ip(&headers, remote_addr);
        tracing::Span::current().record("client_ip", &client_ip);

        let mut trace = SubmissionTrace::init_from_timings(timings);
        trace.metadata = api.api_provider.get_metadata(&headers);

        let Ok(rx) = api.auctioneer_handle.block_submission(headers, body, trace) else {
            error!("failed sending request to worker");
            return Err(BuilderApiError::InternalError);
        };

        let res = match rx.await {
            Ok(res) => res,
            Err(_) => Err(BuilderApiError::RequestTimeout),
        };

        if let Err(err) = &res &&
            err.should_report()
        {
            let status = "error";
            let (error_type, error, error_message) = match err {
                BuilderApiError::BidValidation(val_err) => (
                    "validation",
                    val_err.error_variant(),
                    val_err.error_details(),
                ),
                BuilderApiError::BlockSimulation(sim_err) => (
                    "simulation",
                    sim_err.error_variant(),
                    sim_err.error_details(),
                ),
                _ => ("other", "", String::new()),
            };
            error!(status, error_type, error, error_message);
        }

        res
    }
}

fn get_client_ip(headers: &HeaderMap, remote_addr: SocketAddr) -> String {
    if let Some(real_ip) = headers.get("X-Real-IP").and_then(|v| v.to_str().ok()) {
        return real_ip.to_string();
    }
    if let Some(forwarded) = headers.get("X-Forwarded-For").and_then(|v| v.to_str().ok()) {
        let ip = forwarded.split(',').next().unwrap_or_default().trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    remote_addr.ip().to_string()
}
