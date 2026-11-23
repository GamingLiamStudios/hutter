use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs::File,
    io::{
        BufReader,
        BufWriter,
        Read,
        Write,
    },
    path::Path,
};

use rustc_hash::{
    FxHashMap,
    FxHashSet,
};
use unicode_normalization::UnicodeNormalization;
use unicode_script::{
    Script,
    UnicodeScript,
};
use wincode::{
    SchemaRead,
    SchemaWrite,
};

#[derive(thiserror::Error, Debug)]
pub enum NGramError {
    #[error("IO Error")]
    Io(#[from] std::io::Error),

    #[error("Serialization Error")]
    Serialize(#[from] wincode::WriteError),

    #[error("Deserialization Error")]
    Deserialize(#[from] wincode::ReadError),

    #[error("Input does not contain valid utf8 string")]
    Utf8,

    #[error("Requested Vocab size is too large")]
    VocabSize,

    #[error("Output buffer is too small; must be at least sie of input")]
    OutputLen,
}

#[derive(Debug, Default)]
pub struct NGramTable {
    pub merge_table: FxHashMap<Pair, (u32, u32)>,
    pub vocab:       Box<[Box<[u8]>]>,
}

#[derive(SchemaRead, SchemaWrite)]
struct NGramTableSerialized {
    pairs: Box<[Pair]>,
}

#[derive(Debug, Default, Hash, PartialEq, Eq)]
struct StrChunk {
    pub chars:  String,
    pub script: Script,
}

#[derive(Debug)]
struct Word {
    pub tokens: Vec<u32>,
    pub count:  usize,
}

#[derive(Debug, Default)]
struct Preprocessor {
    chunks:     Vec<String>,
    chunk:      StrChunk,
    whitespace: String,
}

impl Preprocessor {
    #[inline]
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

    fn process_stream(
        &mut self,
        mut input: impl Read,
    ) -> Result<(), NGramError> {
        let mut buf = vec![0u8; 2usize.pow(20)]; // Process in 1MB chunks
        let mut buf_len = 0;

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
            self.process_str(buf_str);

            let to_flush = buf_str.len();
            buf.copy_within(to_flush..buf_len, 0);
            buf_len -= to_flush;
        }

        Ok(())
    }

    #[inline]
    fn process_str(
        &mut self,
        input: &str,
    ) {
        for ch in input.chars() {
            let script = ch.script();

            if ch.is_whitespace() {
                self.whitespace.push(ch);
                continue;
            }

            if self.chunk.chars.is_empty() {
                self.chunk.script = script;
            }

            if self.chunk.script == script && self.whitespace.is_empty() {
                self.chunk.chars.extend(ch.nfd());
            } else {
                let mut new_chunk = StrChunk {
                    script,
                    ..StrChunk::default()
                };
                new_chunk.chars += self.whitespace.as_str();
                new_chunk.chars.extend(ch.nfd());

                self.whitespace.clear();

                std::mem::swap(&mut self.chunk, &mut new_chunk);

                self.chunks.push(new_chunk.chars);
            }
        }
    }

    fn words(mut self) -> Box<[String]> {
        if !self.chunk.chars.is_empty() {
            self.chunks.push(self.chunk.chars);
        }

        if !self.whitespace.is_empty() {
            self.chunks.push(self.whitespace);
        }

        self.chunks.into_boxed_slice()
    }

    fn word_frequency(mut self) -> Box<[Word]> {
        if !self.chunk.chars.is_empty() {
            self.chunks.push(self.chunk.chars);
        }

        if !self.whitespace.is_empty() {
            self.chunks.push(self.whitespace);
        }

        let mut chunks = FxHashMap::default();
        for chunk in self.chunks {
            *chunks.entry(chunk).or_default() += 1;
        }
        tracing::debug!(num_chunks = chunks.len(), "Split into chunks");

        chunks
            .into_iter()
            .map(|(chunk, count)| {
                let mut tokens = Vec::with_capacity(chunk.len());
                tokens.extend(chunk.bytes().map(u32::from));

                Word { tokens, count }
            })
            .collect()
    }
}

type Pair = (u32, u32);

impl NGramTable {
    pub const EOF_TOKEN: u32 = 256;
    const MIN_NGRAM_INSTANCES: usize = 10;

    fn pairs_of(t: &[u32]) -> impl Iterator<Item = Pair> {
        t.windows(2).map(|w| (w[0], w[1]))
    }

    fn apply_merge(
        toks: &mut Vec<u32>,
        pair: Pair,
        new_id: u32,
    ) {
        let mut out = Vec::with_capacity(toks.len());
        let mut i = 0;
        while i < toks.len() {
            if i + 1 < toks.len() && (toks[i], toks[i + 1]) == pair {
                out.push(new_id);
                i += 2;
            } else {
                out.push(toks[i]);
                i += 1;
            }
        }
        *toks = out;
    }

    #[allow(clippy::missing_errors_doc)]
    pub fn from_stream<const N: usize>(input: impl Read) -> Result<Self, NGramError> {
        // Pre-process input into chunks
        let mut preprocessor = Preprocessor::default();
        preprocessor.process_stream(input)?;

        let mut words = preprocessor.word_frequency();
        words.sort_unstable_by_key(|word| word.count);

        // Create dictionary of all possible pairs
        let mut counts: FxHashMap<Pair, usize> = FxHashMap::default();
        let mut index: FxHashMap<Pair, FxHashSet<usize>> = FxHashMap::default();
        for (i, word) in words.iter().enumerate() {
            for p in Self::pairs_of(&word.tokens) {
                *counts.entry(p).or_default() += word.count;
                index.entry(p).or_default().insert(i);
            }
        }

        let mut heap: BinaryHeap<(usize, Reverse<Pair>)> =
            counts.iter().map(|(&p, &c)| (c, Reverse(p))).collect();

        let mut merges = Vec::new();
        let mut next_id = Self::default_vocab().len();

        while next_id < N {
            let Some((c, Reverse(pair))) = heap.pop() else {
                break;
            };

            // Stale heap invalidation
            let cur = counts.get(&pair).copied().unwrap_or(0);
            if cur != c {
                if cur > 0 {
                    heap.push((cur, Reverse(pair)));
                }
                continue;
            }

            if cur < Self::MIN_NGRAM_INSTANCES {
                break;
            }

            let new_id = u32::try_from(next_id).map_err(|_| NGramError::VocabSize)?;
            next_id += 1;
            merges.push(pair);

            let mut touched = FxHashSet::default();
            for wi in index.remove(&pair).unwrap_or_default() {
                let word = &mut words[wi];

                // Remove all instances of the pairs this word makes
                for p in Self::pairs_of(&word.tokens) {
                    *counts.entry(p).or_default() -= word.count;
                    touched.insert(p);
                }

                Self::apply_merge(&mut word.tokens, pair, new_id);

                // Add all pairs the merged word contains
                for p in Self::pairs_of(&word.tokens) {
                    *counts.entry(p).or_default() += word.count;
                    index.entry(p).or_default().insert(wi);
                    touched.insert(p);
                }
            }

            // Add all (valid) pairs to heap
            for p in touched {
                match counts.get(&p).copied() {
                    Some(c) if c > 0 => {
                        heap.push((c, Reverse(p)));
                    },
                    Some(_) => {
                        counts.remove(&p);
                    },
                    None => {},
                }
            }
        }

        // Build vocab
        Self::build(&merges)
    }

    fn default_vocab() -> Vec<Box<[u8]>> {
        let mut vocab: Vec<_> = (0..=255u8).map(|x| vec![x].into_boxed_slice()).collect();

        // Add any reserved tokens
        vocab.push(Box::new([])); // 256 - EOF_TOKEN

        vocab
    }

    fn build(merges: &[Pair]) -> Result<Self, NGramError> {
        let mut vocab = Self::default_vocab();

        let first_id = vocab.len();
        let mut merge_table = FxHashMap::default();
        merge_table.reserve(merges.len());

        for (r, (lht, rht)) in merges.iter().copied().enumerate() {
            let x = u32::try_from(r).map_err(|_| NGramError::VocabSize)?;
            let y = u32::try_from(first_id + r).map_err(|_| NGramError::VocabSize)?;

            merge_table.insert((lht, rht), (x, y));

            let l_idx = usize::try_from(lht).map_err(|_| NGramError::VocabSize)?;
            let r_idx = usize::try_from(rht).map_err(|_| NGramError::VocabSize)?;

            let mut tok = vocab[l_idx].clone().into_vec();
            tok.extend_from_slice(&vocab[r_idx]);
            vocab.push(tok.into_boxed_slice());
        }

        Ok(Self {
            merge_table,
            vocab: vocab.into_boxed_slice(),
        })
    }

    pub fn encode(
        &self,
        input: &str,
    ) -> Box<[u32]> {
        let mut preprocessor = Preprocessor::default();
        preprocessor.process_str(input);
        let processed_input = preprocessor.words();

        let mut result = Vec::new();
        for word in processed_input {
            result.extend(word.bytes().map(u32::from));
        }

        loop {
            let best = Self::pairs_of(&result)
                .filter_map(|p| self.merge_table.get(&p).map(|&(r, id)| (r, p, id)))
                .min_by_key(|&(r, ..)| r);
            match best {
                Some((_, p, id)) => Self::apply_merge(&mut result, p, id),
                None => break,
            }
        }

        result.push(Self::EOF_TOKEN);
        result.into_boxed_slice()
    }

    #[allow(clippy::missing_errors_doc)]
    pub fn decode(
        &self,
        input: &[u32],
    ) -> Result<String, NGramError> {
        let mut bytes = Vec::new();
        for tok in input {
            let idx = usize::try_from(*tok).map_err(|_| NGramError::VocabSize)?;
            bytes.extend_from_slice(&self.vocab[idx]);
        }

        let result = str::from_utf8(&bytes)
            .map_err(|_| NGramError::Utf8)?
            .nfc()
            .collect();
        Ok(result)
    }

    #[allow(clippy::missing_errors_doc)]
    pub fn save(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), NGramError> {
        let packed = NGramTableSerialized {
            pairs: self.merge_table.keys().copied().collect(),
        };

        let mut file = BufWriter::new(File::create(path)?);
        wincode::serialize_into(&mut file, &packed)?;
        file.flush()?;

        Ok(())
    }

    #[allow(clippy::missing_errors_doc)]
    pub fn load(path: impl AsRef<Path>) -> Result<Self, NGramError> {
        let file = File::open(path)?;
        let buffered = BufReader::new(file);

        let packed: NGramTableSerialized = wincode::deserialize_from(buffered)?;
        Self::build(&packed.pairs)
    }
}

#[cfg(test)]
mod test {
    use std::str::Utf8Error;

    #[test]
    fn test_preprocessor() -> Result<(), Utf8Error> {
        use unicode_normalization::UnicodeNormalization;
        const TEST_STRING: &str = "This is a test. これはテストだ。 Tämä on kokeilu.";

        let mut preprocessor = super::Preprocessor::default();
        preprocessor.process_str(TEST_STRING);
        let words = preprocessor.words();
        let mut bytes = Vec::new();
        for word in words {
            bytes.extend_from_slice(word.as_bytes());
        }

        let result: String = str::from_utf8(&bytes)?.nfc().collect();
        assert_eq!(result.as_str(), TEST_STRING);

        Ok(())
    }
}
