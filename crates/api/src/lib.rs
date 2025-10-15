#![allow(clippy::too_many_arguments)]

use std::sync::Arc;

use helix_common::{RelayConfig, api_provider::ApiProvider, local_cache::LocalCache};
pub use service::start_api_service;

pub mod admin_service;
pub mod auctioneer;
pub mod builder;
pub mod gossip;
pub mod gossiper;
pub mod integration_tests;
pub mod middleware;
pub mod proposer;
pub mod relay_data;
pub mod router;
pub mod service;

mod grpc {
    include!(concat!(env!("OUT_DIR"), "/gossip.rs"));
}

pub fn start_api_service<A: Api>(
    config: RelayConfig,
    db: Arc<A::DatabaseService>,
    auctioneer: Arc<LocalCache>,
    chain_info: Arc<ChainInfo>,
    relay_signing_context: Arc<RelaySigningContext>,
    multi_beacon_client: Arc<MultiBeaconClient>,
    metadata_provider: Arc<A::MetadataProvider>,
    current_slot_info: CurrentSlotInfo,
    known_validators_loaded: Arc<AtomicBool>,
    terminating: Arc<AtomicBool>,
    is_leader: Arc<AtomicBool>,
    sorter_tx: crossbeam_channel::Sender<BidSorterMessage>,
    top_bid_tx: tokio::sync::broadcast::Sender<Bytes>,
    shared_best_header: BestGetHeader,
) {
    tokio::spawn(run_api_service::<A>(
        config.clone(),
        db,
        auctioneer,
        current_slot_info,
        chain_info,
        relay_signing_context,
        multi_beacon_client,
        metadata_provider,
        known_validators_loaded,
        terminating,
        is_leader,
        sorter_tx,
        top_bid_tx,
        shared_best_header,
    ));
}

pub fn start_admin_service(auctioneer: Arc<LocalCache>, config: &RelayConfig) {
    tokio::spawn(admin_service::run_admin_service(auctioneer, config.clone()));
}

pub trait Api: Clone + Send + Sync + 'static {
    type ApiProvider: ApiProvider;
}

pub const HEADER_API_KEY: &str = "x-api-key";
pub const HEADER_SEQUENCE: &str = "x-sequence";
pub const HEADER_HYDRATE: &str = "x-hydrate";
pub const HEADER_IS_MERGEABLE: &str = "x-mergeable";
