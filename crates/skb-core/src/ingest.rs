use crate::config::Config;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::graph;
use crate::tokenize::{Chunk, Tokenize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};
use url::Url;

/// Hard limits for upload safety (spec §12.3 / §15). Not configurable on
/// purpose: they are resource guards against malicious or malformed inputs.
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_PDF_PAGES: usize = 200;
pub const MAX_PROCESS_SECONDS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(extend("oneOf" = [
    {
        "required": ["path"],
        "properties": {
            "path": {"type": "string"},
            "url": {"type": "null"},
            "content": {"type": "null"},
            "content_base64": {"type": "null"}
        }
    },
    {
        "required": ["url"],
        "properties": {
            "path": {"type": "null"},
            "url": {"type": "string"},
            "content": {"type": "null"},
            "content_base64": {"type": "null"}
        }
    },
    {
        "required": ["content"],
        "properties": {
            "path": {"type": "null"},
            "url": {"type": "null"},
            "content": {"type": "string"},
            "content_base64": {"type": "null"}
        }
    },
    {
        "required": ["content_base64"],
        "properties": {
            "path": {"type": "null"},
            "url": {"type": "null"},
            "content": {"type": "null"},
            "content_base64": {"type": "string"}
        }
    },
]))]
pub struct UploadRequest {
    pub path: Option<String>,
    pub url: Option<String>,
    pub content: Option<String>,
    pub content_base64: Option<String>,
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, String>>,
    pub force: Option<bool>,
}

impl UploadRequest {
    /// Exactly one of `path`, `url`, `content`, `content_base64` must be set.
    pub fn validate(&self) -> Result<(), SkbError> {
        let sources = [
            self.path.is_some(),
            self.url.is_some(),
            self.content.is_some(),
            self.content_base64.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        match sources {
            0 => Err(SkbError::new(
                ErrorCode::Validation,
                "one of path, url, content, content_base64 is required",
            )),
            1 => Ok(()),
            _ => Err(SkbError::new(
                ErrorCode::Validation,
                "only one of path, url, content, content_base64 may be specified",
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UploadResult {
    pub document_id: Option<String>,
    pub title: String,
    pub status: String,
    pub chunks: usize,
    pub tokens: usize,
    pub sha256: String,
    pub entities: Vec<String>,
}

#[derive(Debug, Clone)]
struct DocumentData {
    title: String,
    source: String,
    source_type: String,
    content: String,
    sha256: String,
    tags: Vec<String>,
    metadata: HashMap<String, String>,
    mime: Option<String>,
}

/// Upload, embed, persist, and graph-index one document. Document, chunks and
/// mentions are committed in a single transaction; a failure rolls back the
/// whole document (spec §12.3).
pub async fn upload(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
    req: UploadRequest,
) -> Result<UploadResult, SkbError> {
    req.validate()?;
    let force = req.force.unwrap_or(false);
    let doc = extract_document_data(req, config).await?;

    let chunks = tokenizer.chunk(
        &doc.content,
        config.chunking.max_tokens,
        config.chunking.overlap_tokens,
    )?;

    if chunks.is_empty() {
        return Ok(UploadResult {
            document_id: None,
            title: doc.title,
            status: "empty".into(),
            chunks: 0,
            tokens: 0,
            sha256: doc.sha256,
            entities: vec![],
        });
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = embed_batch(embedder, &texts, config.embedding.batch_size)?;
    let total_tokens: usize = chunks.iter().map(|c| c.token_count).sum();

    // The transaction starts BEFORE the duplicate check so the lookup and the
    // write share one transaction (a concurrent identical upload cannot both
    // create the document). The begin -> check -> store -> commit sequence is
    // retried as a whole on retryable SurrealKV write conflicts (a
    // transaction cannot be re-committed; store_and_index is idempotent for a
    // given (doc, chunks) pair).
    const ATTEMPTS: usize = 8;
    let mut last = None;
    let mut delay = std::time::Duration::from_millis(50);
    for attempt in 0..ATTEMPTS {
        let tx = db
            .db
            .clone()
            .begin()
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload begin: {e}")))?;
        let existed = doc_id_by_sha(&tx, &doc.sha256).await?.is_some();
        if !force && existed {
            let _ = tx.cancel().await;
            return Ok(UploadResult {
                document_id: None,
                title: doc.title,
                status: "skipped".into(),
                chunks: 0,
                tokens: 0,
                sha256: doc.sha256,
                entities: vec![],
            });
        }

        let stored = store_and_index(&tx, &doc, &chunks, &embeddings, force && existed).await;
        let (doc_id, entities) = match stored {
            Ok(pair) => pair,
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        };

        match tx.commit().await {
            Ok(_) => {
                return Ok(UploadResult {
                    document_id: Some(doc_id),
                    title: doc.title,
                    status: if existed {
                        "updated".into()
                    } else {
                        "created".into()
                    },
                    chunks: chunks.len(),
                    tokens: total_tokens,
                    sha256: doc.sha256,
                    entities,
                });
            }
            Err(e) if e.to_string().contains("Transaction write conflict") => {
                last = Some(e);
                if attempt + 1 < ATTEMPTS {
                    tokio::time::sleep(delay).await;
                    delay = delay
                        .saturating_mul(2)
                        .min(std::time::Duration::from_millis(800));
                }
            }
            Err(e) => {
                // commit consumed the transaction; nothing to cancel.
                return Err(SkbError::new(ErrorCode::Db, format!("upload commit: {e}")));
            }
        }
    }
    Err(SkbError::new(
        ErrorCode::Db,
        format!(
            "upload commit: {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        ),
    ))
}

type LocalTransaction = surrealdb::method::Transaction<surrealdb::engine::local::Db>;

/// Create the document, its chunks and the graph mentions inside `tx`.
/// When `replace_existing` is set (force re-upload), the previous document,
/// chunks and mentions are removed first — atomically with the new data.
async fn store_and_index(
    tx: &LocalTransaction,
    doc: &DocumentData,
    chunks: &[Chunk],
    embeddings: &[Vec<f32>],
    replace_existing: bool,
) -> Result<(String, Vec<String>), SkbError> {
    let mut document_id: Option<String> = None;
    if replace_existing {
        // Preserve the document's id (upsert semantics, spec §4.2): update the
        // existing record in place and only replace chunks/mentions. The
        // existence check above ran in the same transaction, so a vanished
        // record here is a genuine error.
        let did = doc_id_by_sha(tx, &doc.sha256)
            .await?
            .ok_or_else(|| SkbError::new(ErrorCode::Db, "document vanished during upload"))?;
        document_id = Some(format!("document:{did}"));
        let record = surrealdb::types::RecordId::new("document", did);
        tx.query(
            "UPDATE $document SET title = $title, source = $source, \
             source_type = $source_type, sha256 = $sha256, content = $content, \
             mime = $mime, tags = $tags, metadata = $metadata; \
             DELETE FROM mentions WHERE in.document = $document; \
             DELETE FROM chunk WHERE document = $document;",
        )
        .bind(("document", record.clone()))
        .bind(("title", doc.title.clone()))
        .bind(("source", doc.source.clone()))
        .bind(("source_type", doc.source_type.clone()))
        .bind(("sha256", doc.sha256.clone()))
        .bind(("content", doc.content.clone()))
        .bind(("mime", doc.mime.clone()))
        .bind(("tags", doc.tags.clone()))
        .bind(("metadata", doc.metadata.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload replace: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload replace check: {e}")))?;
    }

    let doc_id = if let Some(existing) = document_id {
        // Force re-upload keeps the existing record's id (upsert semantics).
        existing
    } else {
        let sql = "CREATE document SET title = $title, source = $source, \
                   source_type = $source_type, sha256 = $sha256, content = $content, \
                   mime = $mime, tags = $tags, metadata = $metadata \
                   RETURN string::concat('document:', meta::id(id)) AS did";
        let mut r = tx
            .query(sql)
            .bind(("title", doc.title.clone()))
            .bind(("source", doc.source.clone()))
            .bind(("source_type", doc.source_type.clone()))
            .bind(("sha256", doc.sha256.clone()))
            .bind(("content", doc.content.clone()))
            .bind(("mime", doc.mime.clone()))
            .bind(("tags", doc.tags.clone()))
            .bind(("metadata", doc.metadata.clone()))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload doc: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload doc check: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload doc take: {e}")))?;
        rows.first()
            .and_then(|v| v["did"].as_str())
            .ok_or_else(|| SkbError::new(ErrorCode::Db, "failed to get document id"))?
            .to_string()
    };
    let document = surrealdb::types::RecordId::new("document", record_key(&doc_id)?);

    let chunk_sql = "CREATE chunk SET document = $document, idx = $idx, \
                     content = $content, token_count = $token_count, \
                     embedding = $embedding, heading = $heading \
                     RETURN string::concat('chunk:', meta::id(id)) AS cid";
    let mut chunk_ids = Vec::with_capacity(chunks.len());
    for (i, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let mut r = tx
            .query(chunk_sql)
            .bind(("document", document.clone()))
            .bind(("idx", i as i64))
            .bind(("content", chunk.content.clone()))
            .bind(("token_count", chunk.token_count as i64))
            .bind(("embedding", emb.clone()))
            .bind(("heading", chunk.heading.clone()))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload chunk {i}: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload chunk {i} check: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("upload chunk take: {e}")))?;
        let cid = rows
            .first()
            .and_then(|v| v["cid"].as_str())
            .ok_or_else(|| SkbError::new(ErrorCode::Db, format!("chunk {i} did not return an id")))?
            .to_string();
        chunk_ids.push(cid);
    }

    let mut entities = Vec::new();
    for (cid, chunk) in chunk_ids.iter().zip(chunks.iter()) {
        let names = graph::index_chunk_entities_in_transaction(tx, cid, &chunk.content).await?;
        entities.extend(names);
    }
    // Heading hierarchy: sections become part-of their nearest ancestor.
    // First remove every existing part-of edge whose child is a section
    // mentioned by THIS document's chunks, so each child keeps only its
    // current nearest parent (a force re-upload with a restructured document
    // cannot leave stale edges to an old parent). Scoped via the chunk
    // mentions edge so other documents' hierarchies are untouched even when
    // they share section names.
    for section in graph::extract_sections(&doc.content) {
        tx.query(
            "DELETE FROM related_to WHERE relation = 'part-of' \
             AND in = $child \
             AND $child IN array::flatten(SELECT VALUE ->mentions->entity \
                                          FROM chunk WHERE document = $document)",
        )
        .bind(("child", graph::entity_record_id(&section.name)?))
        .bind(("document", document.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("section edge cleanup: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("section edge cleanup check: {e}")))?;
    }
    graph::link_section_hierarchy(tx, &doc.content).await?;
    entities.sort();
    entities.dedup();

    Ok((doc_id, entities))
}

async fn doc_id_by_sha(tx: &LocalTransaction, sha256: &str) -> Result<Option<String>, SkbError> {
    let mut r = tx
        .query("SELECT meta::id(id) AS did FROM document WHERE sha256 = $sha256 LIMIT 1")
        .bind(("sha256", sha256.to_string()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("doc lookup: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("doc lookup check: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("doc lookup take: {e}")))?;
    Ok(rows
        .first()
        .and_then(|v| v["did"].as_str())
        .map(|s| s.to_string()))
}

fn record_key(doc_id: &str) -> Result<&str, SkbError> {
    doc_id
        .strip_prefix("document:")
        .ok_or_else(|| SkbError::new(ErrorCode::Db, "unexpected document id format"))
}

async fn extract_document_data(
    req: UploadRequest,
    config: &Config,
) -> Result<DocumentData, SkbError> {
    enum RawInput {
        Text(String),
        Bytes(Vec<u8>),
    }

    // Filesystem and network reads must not block the async runtime; they run
    // on a blocking thread (spawn_blocking) so the MCP server stays responsive
    // while a large file or slow URL is being fetched (spec §12.3).
    let (raw, source, source_type, file_title, content_type_hint) = if let Some(path) = &req.path {
        let source = path.clone();
        let path = std::path::PathBuf::from(path);
        let ft = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();
        let config = config.clone();
        let raw = tokio::task::spawn_blocking(move || -> Result<RawInput, SkbError> {
            // The canonicalized path returned by validate_path is the one
            // used for metadata and reads, so the validated path is the path
            // actually opened (no TOCTOU re-resolution window).
            let canonical = validate_path(&path, &config)?;
            let meta = std::fs::metadata(&canonical)
                .map_err(|e| SkbError::new(ErrorCode::Io, format!("stat file: {e}")))?;
            check_size(meta.len(), &config)?;
            let bytes = read_file_bytes(&canonical)?;
            Ok(RawInput::Bytes(bytes))
        })
        .await
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("file read join: {e}")))??;
        (raw, source, "file".to_string(), ft, None)
    } else if let Some(url) = &req.url {
        let url_fetch = url.clone();
        let config = config.clone();
        let (bytes, content_type) =
            tokio::task::spawn_blocking(move || fetch_url(&url_fetch, &config))
                .await
                .map_err(|e| SkbError::new(ErrorCode::Io, format!("url fetch join: {e}")))??;
        (
            RawInput::Bytes(bytes),
            url.clone(),
            "url".to_string(),
            url.clone(),
            content_type,
        )
    } else if let Some(content) = &req.content {
        check_size(content.len() as u64, config)?;
        (
            RawInput::Text(content.clone()),
            "inline".to_string(),
            "text".to_string(),
            "untitled".to_string(),
            None,
        )
    } else if let Some(b64) = &req.content_base64 {
        let bytes = base64_decode_checked(b64, config)?;
        (
            RawInput::Bytes(bytes),
            "base64".to_string(),
            "text".to_string(),
            "untitled".to_string(),
            None,
        )
    } else {
        return Err(SkbError::new(
            ErrorCode::Validation,
            "one of path, url, content, content_base64 is required",
        ));
    };

    let mime_hint = mime_for(&source)
        .or_else(|| req.title.as_deref().and_then(mime_for))
        .or_else(|| mime_for(&file_title))
        .or(content_type_hint);

    let (content, mime) = match raw {
        RawInput::Text(text) => {
            check_size(text.len() as u64, config)?;
            (extract_text(&text, &source), mime_hint)
        }
        RawInput::Bytes(bytes) => {
            extract_from_bytes(&bytes, &source, mime_hint.as_deref(), config).await?
        }
    };
    check_size(content.len() as u64, config)?;

    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    Ok(DocumentData {
        title: req.title.unwrap_or(file_title),
        source,
        source_type,
        content,
        sha256,
        tags: req.tags.unwrap_or_default(),
        metadata: req.metadata.unwrap_or_default(),
        mime,
    })
}

/// Decode base64 as arbitrary binary (spec §12.2) and classify it: PDF by
/// magic bytes, text by UTF-8 validity, anything else is rejected.
async fn extract_from_bytes(
    bytes: &[u8],
    source: &str,
    mime_hint: Option<&str>,
    config: &Config,
) -> Result<(String, Option<String>), SkbError> {
    check_size(bytes.len() as u64, config)?;
    let is_pdf = is_pdf_bytes(bytes) || mime_hint == Some("application/pdf");
    if is_pdf {
        let text = extract_pdf_checked(bytes).await?;
        return Ok((text, Some("application/pdf".to_string())));
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok((
            extract_text(text, source),
            mime_hint
                .map(str::to_string)
                .or(Some("text/plain".to_string())),
        )),
        Err(_) => Err(SkbError::new(
            ErrorCode::UnsupportedFormat,
            "binary input is not a supported format (text, markdown, html, pdf)",
        )),
    }
}

fn is_pdf_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// Upper bound on PDF parse/extract jobs running concurrently across all
/// requests. The blocking tasks are not cancellable after the wall-clock
/// timeout fires, so without a cap a flood of non-terminating PDFs could
/// accumulate blocking workers and their input buffers.
const MAX_CONCURRENT_PDF_JOBS: usize = 4;

static PDF_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();

fn pdf_semaphore() -> &'static tokio::sync::Semaphore {
    PDF_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_PDF_JOBS))
}

/// Extract PDF text with resource guards: page count, wall-clock time and
/// output size (PDF bomb mitigation, spec §12.3). The page-count check is the
/// primary PDF-bomb defense; parsing and extraction run on blocking threads
/// under a wall-clock timeout so a slow document cannot hang the request. A
/// fixed concurrency cap bounds how many blocking jobs may pile up.
async fn extract_pdf_checked(bytes: &[u8]) -> Result<String, SkbError> {
    let start = Instant::now();
    let shared = std::sync::Arc::new(bytes.to_vec());
    // The permit is acquired (async) before the job starts and moved into the
    // spawn_blocking closure so it is held for the full lifetime of the
    // blocking task — a timed-out caller cannot release the slot while the
    // job still runs, so actual concurrent PDF jobs never exceed
    // MAX_CONCURRENT_PDF_JOBS.
    let parse_permit = pdf_semaphore()
        .acquire()
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("pdf semaphore: {e}")))?;
    // The permit wait consumed part of the shared wall-clock budget; the
    // remaining time bounds the parse itself. If nothing is left, do not
    // launch the blocking job at all.
    let remaining = Duration::from_secs(MAX_PROCESS_SECONDS).saturating_sub(start.elapsed());
    if remaining.is_zero() {
        return Err(SkbError::new(
            ErrorCode::Validation,
            "pdf semaphore wait exceeded time limit",
        ));
    }
    let doc = tokio::time::timeout(
        remaining,
        tokio::task::spawn_blocking({
            let shared = shared.clone();
            move || {
                let _permit = parse_permit;
                lopdf::Document::load_mem(&shared)
                    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf parse: {e}")))
            }
        }),
    )
    .await
    .map_err(|_| SkbError::new(ErrorCode::Validation, "pdf parsing exceeded time limit"))?
    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf parse join: {e}")))?
    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf parse: {e}")))?;
    let pages = doc.get_pages().len();
    if pages > MAX_PDF_PAGES {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!("pdf has {pages} pages, limit is {MAX_PDF_PAGES}"),
        ));
    }
    // The extraction itself is synchronous and cannot be cancelled; the
    // timeout bounds how long the caller waits. Parse and extraction share
    // one MAX_PROCESS_SECONDS budget so a slow parse cannot be followed by a
    // full second timeout. The permit is acquired FIRST, then the remaining
    // budget is recalculated so the wait for the slot does not count as
    // processing time.
    let extract_permit = pdf_semaphore()
        .acquire()
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("pdf semaphore: {e}")))?;
    let remaining = Duration::from_secs(MAX_PROCESS_SECONDS).saturating_sub(start.elapsed());
    let text = tokio::time::timeout(
        remaining,
        tokio::task::spawn_blocking({
            let shared = shared.clone();
            move || {
                let _permit = extract_permit;
                pdf_extract::extract_text_from_mem(&shared)
                    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf extract: {e}")))
            }
        }),
    )
    .await
    .map_err(|_| SkbError::new(ErrorCode::Validation, "pdf extraction exceeded time limit"))?
    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf extract join: {e}")))?
    .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf extract: {e}")))?;
    Ok(text)
}

fn mime_for(name: &str) -> Option<String> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "yaml" | "yml" => "application/x-yaml",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "rs" => "text/x-rust",
        _ => "",
    };
    (!mime.is_empty()).then(|| mime.to_string())
}

fn extract_text(content: &str, source: &str) -> String {
    let lower = source.to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") || source.contains("markdown") {
        content.to_string()
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        html_to_text(content)
    } else {
        content.to_string()
    }
}

fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            text.push(c);
        }
    }
    let mut result = String::new();
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result
}

fn read_file_bytes(path: &std::path::Path) -> Result<Vec<u8>, SkbError> {
    // `std::fs::read` is synchronous; a wall-clock check after it completes
    // cannot interrupt the read, so it is intentionally omitted here. Size
    // guarding happens before the read in the caller (spec §12.3).
    std::fs::read(path).map_err(|e| SkbError::new(ErrorCode::Io, format!("read file: {e}")))
}

fn max_upload_bytes(config: &Config) -> u64 {
    config.upload.max_file_mb.saturating_mul(1024 * 1024)
}

fn check_size(len: u64, config: &Config) -> Result<(), SkbError> {
    let max = max_upload_bytes(config);
    if len > max {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!(
                "input size {len} bytes exceeds upload.max_file_mb ({})",
                config.upload.max_file_mb
            ),
        ));
    }
    Ok(())
}

/// Fetch a URL with SSRF guards (spec §15 / §12.3):
/// - http/https schemes only
/// - DNS resolution validated before every request (private/loopback/
///   link-local/multicast/metadata addresses are rejected)
/// - redirects are followed manually, re-validating each hop
/// - size-limited streaming read; the whole fetch (all redirect hops) is
///   bounded by one MAX_PROCESS_SECONDS deadline
fn fetch_url(url_str: &str, config: &Config) -> Result<(Vec<u8>, Option<String>), SkbError> {
    fetch_url_with_validator(url_str, config, validate_url_host)
}

/// Shared implementation of `fetch_url`; the host validator is injected so
/// tests can exercise redirect and size-limit handling against a loopback
/// server without tripping the SSRF guard (which rejects loopback).
fn fetch_url_with_validator(
    url_str: &str,
    config: &Config,
    validate: impl Fn(&Url) -> Result<(), SkbError>,
) -> Result<(Vec<u8>, Option<String>), SkbError> {
    let max_bytes = max_upload_bytes(config);
    let deadline = Instant::now() + Duration::from_secs(MAX_PROCESS_SECONDS);

    let mut current = url_str.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "url fetch exceeded time limit",
            ));
        }
        let url = Url::parse(&current)
            .map_err(|e| SkbError::new(ErrorCode::Validation, format!("invalid url: {e}")))?;
        validate(&url)?;

        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(remaining))
            .build()
            .new_agent();
        let mut resp = agent
            .get(&current)
            .call()
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("fetch url: {e}")))?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    SkbError::new(ErrorCode::Io, format!("redirect {status} without location"))
                })?;
            let next = url.join(location).map_err(|e| {
                SkbError::new(ErrorCode::Validation, format!("invalid redirect url: {e}"))
            })?;
            current = next.to_string();
            continue;
        }
        if status != 200 {
            return Err(SkbError::new(
                ErrorCode::Io,
                format!("url returned status {status}"),
            ));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or("").trim().to_string());
        let body = resp
            .body_mut()
            .with_config()
            .limit(max_bytes + 1)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => {
                    SkbError::new(ErrorCode::Validation, "response exceeds max file size")
                }
                other => SkbError::new(ErrorCode::Io, format!("read url: {other}")),
            })?;
        if body.len() as u64 > max_bytes {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "response exceeds max file size",
            ));
        }
        return Ok((body, content_type));
    }
    Err(SkbError::new(
        ErrorCode::Io,
        format!("too many redirects (max {MAX_REDIRECTS})"),
    ))
}

/// Validate scheme and that the host resolves only to public addresses.
/// Resolution happens immediately before the request so the validated answer
/// is as fresh as possible (residual DNS-rebinding window is inherent to the
/// transport; the resolver/connector pinning is not exposed by ureq).
pub fn validate_url_host(url: &Url) -> Result<(), SkbError> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!("unsupported url scheme '{other}', only http/https are allowed"),
            ))
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| SkbError::new(ErrorCode::Validation, "url has no host"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("dns resolve '{host}': {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(SkbError::new(
            ErrorCode::Io,
            format!("no addresses for '{host}'"),
        ));
    }
    for addr in addrs {
        reject_blocked_ip(addr.ip())?;
    }
    Ok(())
}

fn reject_blocked_ip(ip: IpAddr) -> Result<(), SkbError> {
    if is_blocked_ip(ip) {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!("url resolves to a blocked address: {ip}"),
        ));
    }
    Ok(())
}

/// True for addresses that must never be fetched: loopback, private, link-local,
/// multicast, unspecified, broadcast, documentation, CGNAT, benchmarking and
/// reserved ranges (incl. the 169.254.169.254 cloud metadata address), plus
/// their IPv6 equivalents.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (::/96)
            // addresses must be judged by their IPv4 form, otherwise
            // ::ffff:127.0.0.1 etc. would bypass the SSRF guard. Native v6
            // ranges are checked first so ::1 etc. cannot slip through the
            // v4-compatible conversion (::1 -> 0.0.0.1).
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || is_documentation_v6(v6)
                || is_nat64_v6(v6)
                || is_6to4_v6(v6)
                || is_teredo_v6(v6)
            {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_v4(v4);
            }
            if let Some(v4) = v6.to_ipv4() {
                return is_blocked_v4(v4);
            }
            false
        }
    }
}

fn is_blocked_v4(v4: std::net::Ipv4Addr) -> bool {
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_multicast()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || is_cgnat(v4)
        || is_benchmarking(v4)
        || is_reserved_v4(v4)
        || v4.octets() == [169, 254, 169, 254]
        // 0.0.0.0/8 — "this network" (block the whole range, not only the
        // unspecified address).
        || v4.octets()[0] == 0
        // 192.88.99.0/24 — 6to4 relay anycast.
        || (v4.octets()[0] == 192 && v4.octets()[1] == 88 && v4.octets()[2] == 99)
}

/// 100.64.0.0/10 — shared address space (CGNAT).
fn is_cgnat(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 100 && (o[1] & 0xC0) == 0x40
}

/// 198.18.0.0/15 — benchmarking.
fn is_benchmarking(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 198 && (o[1] & 0xFE) == 0x12
}

/// 240.0.0.0/4 — reserved for future use.
fn is_reserved_v4(v4: std::net::Ipv4Addr) -> bool {
    v4.octets()[0] >= 240
}

/// 2001:db8::/32 — documentation range.
fn is_documentation_v6(v6: std::net::Ipv6Addr) -> bool {
    v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8
}

/// 64:ff9b::/96 — NAT64 well-known prefix (maps to IPv4 destinations).
fn is_nat64_v6(v6: std::net::Ipv6Addr) -> bool {
    let s = v6.segments();
    s[0] == 0x64 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0
}

/// 2002::/16 — 6to4 (lower 32 bits embed an IPv4 destination).
fn is_6to4_v6(v6: std::net::Ipv6Addr) -> bool {
    v6.segments()[0] == 0x2002
}

/// 2001::/32 — Teredo (lower 32 bits embed an IPv4 destination, obfuscated).
fn is_teredo_v6(v6: std::net::Ipv6Addr) -> bool {
    let s = v6.segments();
    s[0] == 0x2001 && s[1] == 0x0000
}

fn base64_decode_checked(b64: &str, config: &Config) -> Result<Vec<u8>, SkbError> {
    use base64::Engine;
    // Check the raw input length before allocating the whitespace-stripped
    // copy, so an oversized payload is rejected without duplicating it.
    // Whitespace only shrinks the payload, so the decoded-length estimate
    // from the original length is a safe upper bound.
    check_size(base64::decoded_len_estimate(b64.len()) as u64, config)?;
    let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let estimated = base64::decoded_len_estimate(compact.len());
    check_size(estimated as u64, config)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|e| SkbError::new(ErrorCode::Validation, format!("base64: {e}")))?;
    check_size(bytes.len() as u64, config)?;
    Ok(bytes)
}

/// Validate that `path` is inside an allowed directory and return the
/// canonicalized path. Callers must use the returned path for the subsequent
/// metadata / read operations so the validated path is the one actually read
/// (no re-resolution window between validation and use).
fn validate_path(path: &std::path::Path, config: &Config) -> Result<std::path::PathBuf, SkbError> {
    // Always canonicalize so the returned path is the resolved one used for
    // the subsequent metadata / read operations (no re-resolution window).
    let canonical = path
        .canonicalize()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("resolve path: {e}")))?;
    let allowed = &config.upload.allowed_dirs;
    if allowed.is_empty() {
        return Ok(canonical);
    }
    for dir in allowed {
        let can_dir = dir
            .canonicalize()
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("resolve allowed dir: {e}")))?;
        if canonical.starts_with(&can_dir) {
            return Ok(canonical);
        }
    }
    Err(SkbError::new(
        ErrorCode::Validation,
        format!("path not in allowed directories: {}", canonical.display()),
    ))
}

fn embed_batch(
    embedder: &dyn Embed,
    texts: &[String],
    batch_size: usize,
) -> Result<Vec<Vec<f32>>, SkbError> {
    let mut all = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(batch_size) {
        all.extend(embedder.embed_batch(chunk)?);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn upload_requires_exactly_one_source() {
        let req = UploadRequest {
            path: None,
            url: None,
            content: None,
            content_base64: None,
            title: None,
            tags: None,
            metadata: None,
            force: None,
        };
        assert!(req.validate().is_err());

        let req = UploadRequest {
            path: Some("a.md".into()),
            url: Some("https://example.com".into()),
            content: None,
            content_base64: None,
            title: None,
            tags: None,
            metadata: None,
            force: None,
        };
        assert!(req.validate().is_err());

        let req = UploadRequest {
            path: Some("a.md".into()),
            url: None,
            content: None,
            content_base64: None,
            title: None,
            tags: None,
            metadata: None,
            force: None,
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn blocked_ip_ranges() {
        let blocked: Vec<&str> = vec![
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "169.254.1.1",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "192.88.99.1",
            "192.0.2.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "ff02::1",
            "::",
        ];
        for ip in blocked {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(is_blocked_ip(ip), "{ip} should be blocked");
        }
        // IPv4-mapped / IPv4-compatible IPv6 must be judged by their IPv4 form.
        let mapped: Vec<&str> = vec![
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:192.168.1.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "::ffff:100.64.0.1",
        ];
        for ip in mapped {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(is_blocked_ip(ip), "{ip} should be blocked");
        }
        let extra_v6: Vec<&str> = vec![
            "2001:db8::1",
            "64:ff9b::c000:0201",
            "64:ff9b::7f00:1",
            "2002:7f00:0001::",
            "2001:0000:0:0:0:0:0101:0101",
        ];
        for ip in extra_v6 {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(is_blocked_ip(ip), "{ip} should be blocked");
        }
        // Only the complete 64:ff9b::/96 prefix is NAT64; an address with a
        // non-zero segment inside the prefix must not match.
        let not_nat64: Vec<&str> = vec!["64:ff9b:0:0:1::1", "64:ff9b:0:1::c000:0201"];
        for ip in not_nat64 {
            let v6: std::net::Ipv6Addr = ip.parse().unwrap();
            assert!(!is_nat64_v6(v6), "{ip} must not match NAT64 /96");
            assert!(!is_blocked_ip(v6.into()), "{ip} should be allowed");
        }
        let allowed: Vec<&str> = vec!["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700::1111"];
        for ip in allowed {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!is_blocked_ip(ip), "{ip} should be allowed");
        }
    }

    #[test]
    fn url_validation_rejects_non_http_schemes() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(matches!(
            validate_url_host(&url),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn url_validation_rejects_blocked_hosts_without_network() {
        for host in [
            "http://127.0.0.1:9/x",
            "http://192.168.1.1/x",
            "http://169.254.169.254/latest",
        ] {
            let url = Url::parse(host).unwrap();
            assert!(
                matches!(
                    validate_url_host(&url),
                    Err(SkbError {
                        code: ErrorCode::Validation,
                        ..
                    })
                ),
                "{host} should be rejected"
            );
        }
    }

    #[test]
    fn url_validation_accepts_public_ip_literals() {
        let url = Url::parse("http://8.8.8.8/x").unwrap();
        assert!(validate_url_host(&url).is_ok());
    }

    #[test]
    fn pdf_magic_detection() {
        assert!(is_pdf_bytes(b"%PDF-1.7\n..."));
        assert!(!is_pdf_bytes(b"plain text"));
        assert!(!is_pdf_bytes(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn base64_decode_enforces_size_limit() {
        let mut config = Config::default();
        config.upload.max_file_mb = 1;
        let big = vec![b'x'; 2 * 1024 * 1024];
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, big);
        assert!(matches!(
            base64_decode_checked(&b64, &config),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    /// Serve an HTTP response repeatedly on a loopback listener in a background
    /// thread, returning the base URL. The response is shared with the thread
    /// via Arc so it is reclaimed when the test ends.
    fn serve_repeatedly(response: String, times: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = std::sync::Arc::new(response);
        std::thread::spawn(move || {
            for stream in listener.incoming().take(times) {
                let mut stream = stream.unwrap();
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/x")
    }

    #[test]
    fn fetch_url_rejects_too_many_redirects() {
        // A 302 pointing back at itself never terminates; the manual redirect
        // loop must cap at MAX_REDIRECTS.
        let url = serve_repeatedly(
            "HTTP/1.1 302 Found\r\nlocation: /self\r\ncontent-length: 0\r\n\r\n".to_string(),
            MAX_REDIRECTS + 1,
        );
        let config = Config::default();
        assert!(matches!(
            fetch_url_with_validator(&url, &config, |_| Ok(())),
            Err(SkbError {
                code: ErrorCode::Io,
                ..
            })
        ));
    }

    #[test]
    fn fetch_url_rejects_body_over_size_limit() {
        // A body larger than upload.max_file_mb must be rejected with a
        // Validation error by the streaming size cap.
        let body = "x".repeat(2 * 1024 * 1024);
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let url = serve_repeatedly(response, 1);
        let mut config = Config::default();
        config.upload.max_file_mb = 1;
        assert!(matches!(
            fetch_url_with_validator(&url, &config, |_| Ok(())),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }
}
