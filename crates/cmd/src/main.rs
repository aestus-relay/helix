use helix_api::service::ApiService;
use helix_common::{LoggingConfig, RelayConfig};
use tokio::runtime::Builder;
use helix_website::website_service::WebsiteService;

async fn run() {
    let config = match RelayConfig::load() {
        Ok(config) => config,
        Err(e) => {
            println!("Failed to load config: {:?}", e);
            std::process::exit(1);
        }
    };

    let _guard;

    match &config.logging {
        LoggingConfig::Console => {
            tracing_subscriber::fmt().init();
        }
        LoggingConfig::File { dir_path, file_name } => {
            let file_appender = tracing_appender::rolling::daily(dir_path, file_name);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

            tracing_subscriber::fmt()
                .with_env_filter(filter_layer)
                .with_writer(non_blocking)
                .init();
            _guard = Some(guard);
        }
    }

    let mut handles = Vec::new();

    if !config.router_config.enabled_routes.is_empty() {
        let api_config = config.clone();
        let api_handle = tokio::spawn(async move {
            tracing::info!("Starting API service...");
            ApiService::run(api_config).await;
        });
        handles.push(api_handle);
    } else {
        tracing::warn!("No relay API routes are enabled.");
    }

    if config.website.enabled {
        let website_config = config.clone();
        let website_handle = tokio::spawn(async move {
            loop {
                tracing::info!("Starting website service...");
                match WebsiteService::run(website_config.clone()).await {
                    Ok(_) => {
                        tracing::error!("Website service unexpectedly completed. Restarting...");
                    },
                    Err(e) => {
                        tracing::error!("Website server error: {}. Restarting...", e);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
        handles.push(website_handle);
    }

    if !handles.is_empty() {
        // Await all tasks to prevent the runtime from shutting down
        futures::future::join_all(handles).await;
    } else {
        tracing::error!("No services are enabled.");
        std::process::exit(1);
    }
}

fn main() {
    let num_cpus = num_cpus::get();
    let worker_threads = num_cpus;
    let max_blocking_threads = num_cpus / 2;

    let rt = Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .max_blocking_threads(max_blocking_threads)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(run());
}
