use tracing_subscriber::EnvFilter;

pub fn init() {
    let filter = EnvFilter::new("telepirate=trace");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_env_filter(filter)
        .with_target(false)
        .init();
    let version = env!("CARGO_PKG_VERSION");
    info!("Version {version} started up.");
}
