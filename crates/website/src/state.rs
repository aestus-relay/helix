use std::sync::Arc;
use helix_database::postgres::postgres_db_service::PostgresDatabaseService;
use helix_common::chain_info::ChainInfo;
use helix_common::WebsiteConfig;
use std::sync::RwLock;
use crate::templates::IndexTemplate;
use tokio::sync::watch;
use tracing::{info, error, debug, warn};

#[derive(Clone)]
pub struct AppState {
    pub db_pool: Arc<PostgresDatabaseService>,
    pub chain_info: Arc<ChainInfo>,
    pub website_config: WebsiteConfig,
    pub cached_templates: Arc<RwLock<CachedTemplates>>,
    pub latest_slot_info: Arc<LatestSlotInfo>,
}

pub struct CachedTemplates {
    pub default: IndexTemplate,
    pub by_value_desc: IndexTemplate,
    pub by_value_asc: IndexTemplate,
}

pub struct LatestSlotInfo {
    pub slot: watch::Receiver<u64>,
}

impl LatestSlotInfo {
    pub fn new(initial_slot: u64) -> (Self, watch::Sender<u64>) {
        let (tx, rx) = watch::channel(initial_slot);
        (LatestSlotInfo { slot: rx }, tx)
    }

    pub fn get_latest_slot(&self) -> u64 {
        let slot = *self.slot.borrow();
        debug!("Getting latest slot: {}", slot);
        slot
    }
}