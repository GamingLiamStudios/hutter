use std::{
    collections::{
        BTreeMap,
        HashMap,
        HashSet,
    },
    fs::File,
    io::{
        BufRead,
        BufReader,
        Read,
        Seek,
    },
    str::{
        FromStr,
        Utf8Error,
    },
};

use bstr::{
    BStr,
    BString,
};
use bytemuck::checked;
use tracing::Level;
use tracing_subscriber::{
    Layer,
    filter::Targets,
    fmt,
    layer::SubscriberExt,
};
use unicode_normalization::UnicodeNormalization;
use unicode_script::{
    Script,
    ScriptExtension,
    UnicodeScript,
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

#[derive(thiserror::Error, Debug)]
enum NGramError {
    #[error("IO Error")]
    Io(#[from] std::io::Error),

    #[error("Input does not contain valid utf8 string")]
    Utf8,
}

#[derive(Debug)]
struct NGramTable {
    pub tokens:    Box<[BString]>,
    largest_ngram: usize,
}

#[derive(Debug, Default)]
struct StrChunk {
    pub data:   String,
    pub script: Script,
}

impl NGramTable {
    const MAX_NGRAM_LENGTH: usize = 16;

    fn str_from_buf(buf: &[u8]) -> Result<&'_ str, NGramError> {
        let mut length = buf.len();
        while length > 0 {
            match str::from_utf8(&buf[..length]) {
                Ok(result) => return Ok(result),
                Err(err) => {
                    length = err.valid_up_to();
                },
            }
        }
        Err(NGramError::Utf8)
    }

    fn from_stream<const N: usize>(mut input: impl Read + Seek) -> Result<Self, NGramError> {
        // Pre-process into chunks
        let mut buf = vec![0u8; 2usize.pow(16)];
        let mut buf_len = 0;

        let mut chunks = Vec::new();
        let mut chunk = StrChunk::default();
        loop {
            while buf_len < buf.len() {
                let n_read = input.read(&mut buf[buf_len..])?;
                buf_len += n_read;

                if n_read == 0 {
                    break;
                }
            }

            if buf_len == 0 {
                break;
            }

            let buf_str = Self::str_from_buf(&buf[..buf_len])?;
            for ch in buf_str.chars() {
                if chunk.data.is_empty() {
                    chunk.script = ch.script();
                }

                if chunk.script == ch.script() {
                    chunk.data.extend(ch.nfd());
                } else {
                    let mut new_chunk = StrChunk {
                        script: ch.script(),
                        ..StrChunk::default()
                    };
                    new_chunk.data.extend(ch.nfd());

                    std::mem::swap(&mut chunk, &mut new_chunk);

                    // Move whitespace to start of next chunk
                    while let Some(ch) = new_chunk.data.chars().last() {
                        if !ch.is_whitespace() {
                            break;
                        }
                        let Some(ch) = new_chunk.data.pop() else {
                            unreachable!("bleh")
                        };
                        chunk.data.insert(0, ch);
                    }
                    if !new_chunk.data.is_empty() {
                        chunks.push(new_chunk);
                    }
                }
            }

            let to_flush = buf_str.len();
            buf.copy_within(to_flush..buf_len, 0);
            buf_len -= to_flush;
        }
        chunks.push(chunk);

        tracing::info!(num_chunks = chunks.len(), "Split into chunks");
        tracing::debug!(chunks = ?chunks[..16]);

        panic!("crap")
    }
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
    let enwiki = File::open("datasets/enwik9")?;
    let mut buffered = BufReader::new(enwiki);

    let tokens = NGramTable::from_stream::<{ 2usize.pow(12) }>(buffered)?;
    tracing::info!(num_tokens = tokens.tokens.len(), "Generated Token set");

    Ok(())
}
