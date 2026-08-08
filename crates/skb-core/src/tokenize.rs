use crate::error::{ErrorCode, SkbError};
use tokenizers::Tokenizer as HfTokenizer;

pub trait Tokenize: Send + Sync {
    fn encode(&self, text: &str) -> Result<Vec<u32>, SkbError>;
    fn decode(&self, ids: &[u32]) -> Result<String, SkbError>;
    fn vocab_size(&self) -> usize;
    fn chunk(&self, text: &str, max_tokens: usize, overlap: usize) -> Result<Vec<Chunk>, SkbError>;
    /// Canonical JSON serialization of the tokenizer configuration: model
    /// (vocabulary), normalizer, pre-tokenizer, post-processor, decoder and
    /// other fields as loaded from `tokenizer.json`. Deterministic for a given
    /// file; used for fingerprinting (§5.4).
    fn config_json(&self) -> Result<serde_json::Value, SkbError>;
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub content: String,
    pub token_count: usize,
    pub idx: usize,
    pub heading: Option<String>,
}

pub struct TokenizersImpl {
    tokenizer: HfTokenizer,
}

impl TokenizersImpl {
    pub fn load(model_path: &str) -> Result<Self, SkbError> {
        // model_path is either a HuggingFace model ID or local path
        // For simplicity, assume it's loaded from the embedding model's tokenizer.json
        let tok = HfTokenizer::from_file(model_path)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("load tokenizer: {e}")))?;
        Ok(Self { tokenizer: tok })
    }

    pub fn from_path(path: &std::path::Path) -> Result<Self, SkbError> {
        let tok = HfTokenizer::from_file(path)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("load tokenizer: {e}")))?;
        Ok(Self { tokenizer: tok })
    }
}

impl Tokenize for TokenizersImpl {
    fn encode(&self, text: &str) -> Result<Vec<u32>, SkbError> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("encode: {e}")))?;
        Ok(encoding.get_ids().to_vec())
    }

    fn decode(&self, ids: &[u32]) -> Result<String, SkbError> {
        self.tokenizer
            .decode(ids, true)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("decode: {e}")))
    }

    fn vocab_size(&self) -> usize {
        self.tokenizer.get_vocab_size(true)
    }

    fn config_json(&self) -> Result<serde_json::Value, SkbError> {
        serde_json::to_value(&self.tokenizer).map_err(|e| {
            SkbError::new(
                ErrorCode::Tokenize,
                format!("serialize tokenizer config: {e}"),
            )
        })
    }

    fn chunk(&self, text: &str, max_tokens: usize, overlap: usize) -> Result<Vec<Chunk>, SkbError> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("chunk encode: {e}")))?;
        let ids = encoding.get_ids();
        let offsets = encoding.get_offsets();

        if ids.is_empty() {
            return Ok(vec![]);
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < ids.len() {
            let end = (start + max_tokens).min(ids.len());
            let chunk_ids = &ids[start..end];

            let (chunk_offsets_start, chunk_offsets_end) = if let (Some(first), Some(last)) = (
                offsets.get(start),
                offsets.get(end.min(offsets.len()).saturating_sub(1)),
            ) {
                (first.0, last.1)
            } else {
                (0, text.len())
            };

            let content = &text[chunk_offsets_start..chunk_offsets_end.min(text.len())];

            chunks.push(Chunk {
                content: content.to_string(),
                token_count: chunk_ids.len(),
                idx: chunks.len(),
                heading: None,
            });

            if end >= ids.len() {
                break;
            }
            start = end.saturating_sub(overlap.min(end - start));
        }

        Ok(chunks)
    }
}
