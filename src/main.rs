use tracing::Level;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt,
    layer::SubscriberExt,
};

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("Tracing Init error")]
    TracingInit(#[from] tracing::dispatcher::SetGlobalDefaultError),
}

fn main() -> Result<(), Error> {
    let stdout_logger = fmt::layer();

    let registry = tracing_subscriber::registry().with(
        // Filters for cmdline logging
        stdout_logger.with_filter(
            Targets::default()
                .with_target("hutter", Level::DEBUG)
                .with_default(Level::INFO),
        ),
    );
    tracing::subscriber::set_global_default(registry)?;

    // Geekbench 5
    // 1705 single, 9085 multi

    tracing::info!("Hello World!");

    Ok(())
}
