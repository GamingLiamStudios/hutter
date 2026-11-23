use std::{
    fs::File,
    io::BufReader,
};

use hutter::{
    NGramError,
    NGramTable,
};
use tracing::Level;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt,
    layer::SubscriberExt,
};
use zip::{
    ZipArchive,
    result::ZipError,
};

#[derive(thiserror::Error, Debug)]
enum Error {
    #[error("Tracing Init error")]
    TracingInit(#[from] tracing::dispatcher::SetGlobalDefaultError),

    #[error("IO Error")]
    IO(#[from] std::io::Error),

    #[error("ZIP decoding error")]
    ZipArchive(#[from] ZipError),

    #[error("Failed to create NGramTable")]
    NGram(#[from] NGramError),
}

fn main() -> Result<(), Error> {
    let stdout_logger = fmt::layer();

    let registry = tracing_subscriber::registry().with(
        // Filters for cmdline logging
        stdout_logger.with_filter(
            Targets::default()
                .with_target("tokenizer", Level::DEBUG)
                .with_default(Level::INFO),
        ),
    );
    tracing::subscriber::set_global_default(registry)?;

    // Geekbench 5
    // 1705 single, 9085 multi

    tracing::info!("Hello World!");
    let enwiki = File::open("datasets/enwik9.zip")?;
    let mut zipfile = ZipArchive::new(BufReader::new(enwiki))?;

    let dataset = zipfile.by_index(0)?;

    let tokens = NGramTable::from_stream::<{ 2usize.pow(12) }>(dataset)?;
    tracing::info!(num_tokens = tokens.vocab.len(), "Generated Token set");

    tokens.save("datasets/vocab.bin")?;

    Ok(())
}
