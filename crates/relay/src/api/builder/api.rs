use std::sync::Arc;

use axum::{Extension, http::StatusCode, response::IntoResponse};
use helix_common::{RelayConfig, api::builder_api::TopBidUpdate, local_cache::LocalCache};
use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    api::Api, auctioneer::AuctioneerHandle,
    database::postgres::postgres_db_service::PostgresDatabaseService, housekeeper::CurrentSlotInfo,
};

pub(crate) const MAX_PAYLOAD_LENGTH: usize = 1024 * 1024 * 20; // 20MB

#[derive(Clone)]
pub struct BuilderApi<A: Api> {
    pub local_cache: Arc<LocalCache>,
    pub db: Arc<PostgresDatabaseService>,
    pub curr_slot_info: CurrentSlotInfo,
    pub relay_config: Arc<RelayConfig>,
    /// Subscriber for TopBid updates, SSZ encoded
    pub top_bid_tx: tokio::sync::broadcast::Sender<TopBidUpdate>,
    pub auctioneer_handle: AuctioneerHandle,
    pub api_provider: Arc<A::ApiProvider>,
    /// Cancellation token for the current leadership epoch.
    /// Cloned per WebSocket connection so they close on leader rotation.
    pub ws_cancellation: Arc<RwLock<CancellationToken>>,
}

impl<A: Api> BuilderApi<A> {
    pub fn new(
        local_cache: Arc<LocalCache>,
        db: Arc<PostgresDatabaseService>,
        relay_config: RelayConfig,
        curr_slot_info: CurrentSlotInfo,
        top_bid_tx: tokio::sync::broadcast::Sender<TopBidUpdate>,
        auctioneer_handle: AuctioneerHandle,
        api_provider: Arc<A::ApiProvider>,
        ws_cancellation: Arc<RwLock<CancellationToken>>,
    ) -> Self {
        Self {
            local_cache,
            db,
            relay_config: Arc::new(relay_config),
            curr_slot_info,
            top_bid_tx,
            auctioneer_handle,
            api_provider,
            ws_cancellation,
        }
    }

    /// Implements this API: <https://flashbots.github.io/relay-specs/#/Builder/getValidators>
    pub async fn get_validators(
        Extension(api): Extension<Arc<BuilderApi<A>>>,
    ) -> impl IntoResponse {
        if let Some(duty_bytes) = api.curr_slot_info.proposer_duties_response() {
            (StatusCode::OK, duty_bytes.0).into_response()
        } else {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
