use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tracing::{info, error};

use helix_common::{RelayConfig, NetworkConfig};
use helix_database::postgres::postgres_db_service::PostgresDatabaseService;
use helix_common::chain_info::ChainInfo;

use crate::handlers;
use crate::state::AppState;
use crate::database_queries::WebsiteDatabaseService;

pub struct WebsiteService {}

impl WebsiteService {
    pub async fn run(config: RelayConfig) -> Result<(), Box<dyn std::error::Error>> {
        // Initialize PostgresDB
        let postgres_db = PostgresDatabaseService::from_relay_config(&config).unwrap();
        //postgres_db.run_migrations().await;
        //postgres_db.init_region(&config).await;
        let db = Arc::new(postgres_db);

        // Initialize ChainInfo
        let chain_info = Arc::new(match config.network_config {
            NetworkConfig::Mainnet => ChainInfo::for_mainnet(),
            NetworkConfig::Goerli => ChainInfo::for_goerli(),
            NetworkConfig::Sepolia => ChainInfo::for_sepolia(),
            NetworkConfig::Holesky => ChainInfo::for_holesky(),
            NetworkConfig::Custom { ref dir_path, ref genesis_validator_root, genesis_time } => {
                ChainInfo::for_custom(dir_path.clone(), genesis_validator_root.clone(), genesis_time)
                    .expect("Failed to load custom chain info")
            },
        });

        let state = AppState {
            db_pool: db.clone(),
            chain_info: chain_info.clone(),
            website_config: config.website.clone(),
        };

        let app = Router::new()
            .route("/", get(handlers::index))
            .with_state(state);

        let addr: String = format!("{}:{}", config.website.listen_address, config.website.port).parse()?;
        let addr: SocketAddr = addr.parse().expect("Invalid listen address");
        let listener = TcpListener::bind(&addr).await?;
        info!("Website listening on {}", addr);

        axum::serve(listener, app.into_make_service()).await?;

        Ok(())
    }
}