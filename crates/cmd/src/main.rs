use helix_api::service::ApiService;
use helix_common::{LoggingConfig, RelayConfig};
use helix_utils::set_panic_hook;
use tokio::runtime::Builder;

use tracing_appender::rolling::Rotation;

use tikv_jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

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
            set_panic_hook(config.postgres.region_name.clone(), config.discord_webhook_url.clone(), None);
            let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

            tracing_subscriber::fmt()
                .with_env_filter(filter_layer)
                .init();
        }
        LoggingConfig::File { dir_path, file_name } => {
            set_panic_hook(config.postgres.region_name.clone(), config.discord_webhook_url.clone(), Some(format!("{}/crash.log", dir_path.clone())));
            let file_appender = tracing_appender::rolling::Builder::new()
                .filename_prefix(file_name)
                .max_log_files(14)
                .rotation(Rotation::DAILY)
                .build(dir_path)
                .expect("failed to create log appender!");
            
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            let filter_layer = tracing_subscriber::EnvFilter::from_default_env();

            tracing_subscriber::fmt()
                .with_env_filter(filter_layer)
                .with_writer(non_blocking)
                .init();
            _guard = Some(guard);
        }
    }

    ApiService::run(config.clone()).await;

    if config.website.enabled {
        tokio::spawn(async move {
            if let Err(e) = WebsiteService::run(config.clone()).await {
                tracing::error!("Website server error: {}", e);
            }
        });
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
