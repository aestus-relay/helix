use std::sync::Arc;
use helix_database::postgres::postgres_db_service::PostgresDatabaseService;
use helix_common::chain_info::ChainInfo;
use helix_common::WebsiteConfig;
use std::sync::RwLock;


#[derive(Clone)]
pub struct AppState {
    pub db_pool: Arc<PostgresDatabaseService>,
    pub chain_info: Arc<ChainInfo>,
    pub website_config: WebsiteConfig,
}