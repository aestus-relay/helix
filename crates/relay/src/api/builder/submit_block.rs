use std::{net::SocketAddr, sync::Arc};

use axum::{Extension, extract::ConnectInfo};
use flux::timing::Nanos;
use helix_common::{
    self, RequestTimings, SubmissionTrace, api_provider::ApiProvider,
    metrics::SUB_CLIENT_TO_SERVER_LATENCY, utils::extract_request_id,
};
use http::HeaderMap;
use tracing::{error, trace};

use super::api::BuilderApi;
use crate::{
    api::{Api, builder::error::BuilderApiError},
    auctioneer::{InternalBidSubmissionHeader, SubmissionResultSender},
};

const HEADER_SEND_TS: &str = "x-send-ts";

impl<A: Api> BuilderApi<A> {
    /// Implements this API: <https://flashbots.github.io/relay-specs/#/Builder/submitBlock>
    #[tracing::instrument(skip_all, err(level = tracing::Level::TRACE),
        fields(
        id = tracing::field::Empty,
        slot = tracing::field::Empty,
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
        let request_id = extract_request_id(&headers);

        tracing::Span::current().record("id", tracing::field::display(request_id));

        trace!("start handler");

        let client_ip = get_client_ip(&headers, remote_addr);
        tracing::Span::current().record("client_ip", &client_ip);

        let mut trace = SubmissionTrace::init_from_timings(timings);
        trace.metadata = api.api_provider.get_metadata(&headers);

        observe_client_to_server_latency(&headers, trace.receive_ns);

        let header = InternalBidSubmissionHeader::from_http_headers(request_id, headers);
        let (tx, rx) = tokio::sync::oneshot::channel();
        if api
            .auctioneer_handle
            .block_submission(None, header, body, trace, SubmissionResultSender::OneShot(tx), None)
            .is_err()
        {
            error!("failed sending request to worker");
            return Err(BuilderApiError::InternalError);
        }

        let res = match rx.await {
            Ok((_, res)) => res,
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

fn observe_client_to_server_latency(headers: &HeaderMap, receive_ns: u64) {
    if let Some(send_ts) = headers.get(HEADER_SEND_TS) {
        if let Some(send_ts) = send_ts.to_str().ok().and_then(Nanos::from_rfc3339) {
            SUB_CLIENT_TO_SERVER_LATENCY
                .with_label_values(&["http"])
                .observe((receive_ns.saturating_sub(send_ts.0) / 1000) as f64);
        }
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
