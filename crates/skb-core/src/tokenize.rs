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
        if max_tokens == 0 {
            return Err(SkbError::new(
                ErrorCode::Tokenize,
                "chunk max_tokens must be at least 1",
            ));
        }
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("chunk encode: {e}")))?;
        let ids = encoding.get_ids();
        let offsets = encoding.get_offsets();

        if ids.is_empty() {
            return Ok(vec![]);
        }

        // Byte offsets of markdown heading lines; chunk boundaries prefer to
        // break right before a heading so sections stay intact (spec §5.1).
        let headings = heading_starts(text);

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < ids.len() {
            let window_end = (start + max_tokens).min(ids.len());
            let mut end = window_end;

            // Break before the first token that ENDS after the next heading
            // inside the window: the token containing the heading start goes
            // to the next chunk, keeping the heading line intact even when no
            // token starts exactly on the heading byte offset. A heading that
            // falls inside the chunk's FIRST token (e.g. a "##" merged with
            // the preceding newline) is the chunk's own heading and never
            // triggers a break.
            let first_token_end = offsets.get(start).map(|o| o.1).unwrap_or(0);
            let first_heading = headings.partition_point(|&h| h <= first_token_end);
            if let Some(&heading_off) = headings.get(first_heading) {
                if let Some(i) = (start + 1..window_end)
                    .find(|&i| offsets.get(i).map(|o| o.1).unwrap_or(0) > heading_off)
                {
                    end = i;
                }
            }

            let (chunk_offsets_start, chunk_offsets_end) = if let (Some(first), Some(last)) = (
                offsets.get(start),
                offsets.get(end.min(offsets.len()).saturating_sub(1)),
            ) {
                (first.0, last.1)
            } else {
                (0, text.len())
            };
            let content = &text[chunk_offsets_start..chunk_offsets_end.min(text.len())];

            let heading = heading_at(text, &headings, chunk_offsets_start, chunk_offsets_end);
            chunks.push(Chunk {
                content: content.to_string(),
                token_count: end - start,
                idx: chunks.len(),
                heading,
            });

            if end >= ids.len() {
                break;
            }
            let size = end - start;
            start = if overlap >= size { end } else { end - overlap };
        }

        Ok(chunks)
    }
}

/// Byte offsets of markdown heading lines (`^#{1,6}\s`), for boundary-aware
/// chunking. Scanning is byte-oriented and cheap enough for large inputs.
fn heading_starts(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut line_start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            if is_heading_line(&text[line_start..i]) {
                out.push(line_start);
            }
            line_start = i + 1;
        }
    }
    if line_start < text.len() && is_heading_line(&text[line_start..]) {
        out.push(line_start);
    }
    out
}

fn is_heading_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut hashes = 0;
    for &b in bytes.iter().take(7) {
        if b == b'#' {
            hashes += 1;
        } else {
            break;
        }
    }
    (1..=6).contains(&hashes) && matches!(bytes.get(hashes), Some(b' ') | Some(b'\t'))
}

/// The heading that owns a chunk spanning `[start, end)`: the first heading
/// inside the chunk, or the nearest heading before it.
fn heading_at(text: &str, headings: &[usize], start: usize, end: usize) -> Option<String> {
    let start_idx = headings.partition_point(|&h| h < start);
    if let Some(&h) = headings.get(start_idx) {
        if h < end {
            return heading_text(text, h);
        }
    }
    if start_idx == 0 {
        return None;
    }
    heading_text(text, headings[start_idx - 1])
}

fn heading_text(text: &str, start: usize) -> Option<String> {
    let end = text[start..]
        .find('\n')
        .map(|e| start + e)
        .unwrap_or(text.len());
    let line = &text[start..end];
    let trimmed = line.trim_start_matches('#').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_heading_lines() {
        assert!(is_heading_line("# Title"));
        assert!(is_heading_line("#\tTitle"));
        assert!(is_heading_line("## Sub"));
        assert!(is_heading_line("###### Deep"));
        assert!(!is_heading_line("####### Too deep"));
        assert!(!is_heading_line("#NoSpace"));
        assert!(!is_heading_line("text # not heading"));
        assert!(!is_heading_line(""));
    }

    #[test]
    fn collects_heading_offsets() {
        let text = "# A\nbody\n## B\nmore\n";
        assert_eq!(heading_starts(text), vec![0, 9]);
    }

    #[test]
    fn heading_at_returns_owning_or_preceding_heading() {
        let text = "# A\nbody\n## B\nmore\n";
        let headings = heading_starts(text);
        assert_eq!(heading_at(text, &headings, 0, 20), Some("A".into()));
        assert_eq!(heading_at(text, &headings, 2, 8), Some("A".into()));
        assert_eq!(heading_at(text, &headings, 9, 20), Some("B".into()));
        assert_eq!(heading_at(text, &headings, 20, 40), Some("B".into()));
        // A chunk spanning the boundary into ## B belongs to B.
        assert_eq!(heading_at(text, &headings, 5, 14), Some("B".into()));
    }

    #[test]
    fn heading_at_returns_none_before_first_heading() {
        let text = "intro\n# A\n";
        let headings = heading_starts(text);
        assert_eq!(heading_at(text, &headings, 0, 2), None);
        assert_eq!(heading_at(text, &headings, 6, 20), Some("A".into()));
    }

    fn fixture_tokenizer(path: &std::path::Path, word: &str) {
        crate::testutil::write_fixture_tokenizer(path, word);
    }

    #[test]
    fn chunk_breaks_at_headings_and_records_heading() {
        let dir = std::path::PathBuf::from("./target/skb-test-tok-chunk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_path = dir.join("tokenizer.json");
        fixture_tokenizer(&tok_path, "alpha");
        let tok = TokenizersImpl::from_path(&tok_path).unwrap();

        // max_tokens=20: the "## Second" heading falls inside the first
        // window, so the chunk must break right before it and each section
        // keeps its own heading instead of being merged together.
        let text = "# First\nalpha alpha alpha alpha\n## Second\nalpha alpha\n";
        let chunks = tok.chunk(text, 20, 0).unwrap();
        assert!(chunks.len() >= 2, "expected at least two sections");
        assert_eq!(chunks[0].heading.as_deref(), Some("First"));
        assert!(chunks[0].content.contains("# First"));
        let second = chunks
            .iter()
            .find(|c| c.heading.as_deref() == Some("Second"));
        let second = second.expect("## Second section must form its own chunk");
        assert!(second.content.starts_with("## Second"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chunk_makes_progress_with_tiny_max_tokens() {
        let dir = std::path::PathBuf::from("./target/skb-test-tok-progress");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_path = dir.join("tokenizer.json");
        fixture_tokenizer(&tok_path, "alpha");
        let tok = TokenizersImpl::from_path(&tok_path).unwrap();

        let text = "alpha alpha alpha alpha alpha alpha";
        let chunks = tok.chunk(text, 2, 1).unwrap();
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.token_count <= 2));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
