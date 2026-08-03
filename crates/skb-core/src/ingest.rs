use crate::config::Config;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::graph;
use crate::tokenize::{Chunk, Tokenize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Upload, embed, persist, and graph-index one document.
pub async fn upload(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
    req: UploadRequest,
) -> Result<UploadResult, SkbError> {
    let force = req.force.unwrap_or(false);
    let doc = extract_document_data(req, config)?;
    let existed = doc_exists(db, &doc.sha256).await?;
    if !force && existed {
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

    if force && existed {
        delete_existing(db, &doc.sha256).await?;
    }

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
    let (doc_id, chunk_ids) = store_document(db, &doc, &chunks, &embeddings).await?;
    let mut entities = Vec::new();
    for (chunk_id, chunk) in chunk_ids.iter().zip(chunks.iter()) {
        graph::index_chunk_entities(db, chunk_id, &chunk.content).await?;
        entities.extend(
            graph::extract_entities(&chunk.content)
                .into_iter()
                .map(|entity| entity.name),
        );
    }
    entities.sort();
    entities.dedup();

    Ok(UploadResult {
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
    })
}

fn extract_document_data(req: UploadRequest, config: &Config) -> Result<DocumentData, SkbError> {
    let (content, source, source_type, file_title) = if let Some(path) = &req.path {
        let path = std::path::Path::new(path);
        validate_path(path, config)?;
        let text = read_file(path)?;
        let ft = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();
        (text, path.display().to_string(), "file".to_string(), ft)
    } else if let Some(url) = &req.url {
        let text = fetch_url(url, config)?;
        (text, url.clone(), "url".to_string(), url.clone())
    } else if let Some(content) = &req.content {
        (
            content.clone(),
            "inline".to_string(),
            "text".to_string(),
            "untitled".to_string(),
        )
    } else if let Some(b64) = &req.content_base64 {
        let bytes = base64_decode(b64)?;
        let text = String::from_utf8(bytes)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("invalid utf8: {e}")))?;
        (
            text,
            "base64".to_string(),
            "text".to_string(),
            "untitled".to_string(),
        )
    } else {
        return Err(SkbError::new(
            ErrorCode::Validation,
            "one of path, url, content, content_base64 is required",
        ));
    };

    let extracted = extract_text(&content, &source);
    let mut hasher = Sha256::new();
    hasher.update(extracted.as_bytes());
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let mime = mime_for(&file_title);
    Ok(DocumentData {
        title: req.title.unwrap_or(file_title),
        source,
        source_type,
        content: extracted,
        sha256,
        tags: req.tags.unwrap_or_default(),
        metadata: req.metadata.unwrap_or_default(),
        mime,
    })
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

fn read_file(path: &std::path::Path) -> Result<String, SkbError> {
    let lower = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if lower == "pdf" {
        let bytes = std::fs::read(path)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("read pdf: {e}")))?;
        pdf_extract::extract_text_from_mem(&bytes)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf extract: {e}")))
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("read file: {e}")))
    }
}

fn fetch_url(url_str: &str, config: &Config) -> Result<String, SkbError> {
    let mut resp = ureq::get(url_str)
        .call()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("fetch url: {e}")))?;
    let max_bytes = (config.upload.max_file_mb * 1024 * 1024) as usize;
    let is_pdf = url_str.to_lowercase().ends_with(".pdf")
        || resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/pdf"));
    let body = resp
        .body_mut()
        .read_to_vec()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("read url: {e}")))?;
    if body.len() > max_bytes {
        return Err(SkbError::new(
            ErrorCode::Validation,
            "response exceeds max file size",
        ));
    }
    if is_pdf {
        pdf_extract::extract_text_from_mem(&body)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("pdf extract: {e}")))
    } else {
        String::from_utf8(body)
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("url not utf8: {e}")))
    }
}

fn base64_decode(b64: &str) -> Result<Vec<u8>, SkbError> {
    use base64::Engine;
    let compact: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|e| SkbError::new(ErrorCode::Validation, format!("base64: {e}")))
}

fn validate_path(path: &std::path::Path, config: &Config) -> Result<(), SkbError> {
    let allowed = &config.upload.allowed_dirs;
    if allowed.is_empty() {
        return Ok(());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("resolve path: {e}")))?;
    for dir in allowed {
        let can_dir = dir
            .canonicalize()
            .map_err(|e| SkbError::new(ErrorCode::Io, format!("resolve allowed dir: {e}")))?;
        if canonical.starts_with(&can_dir) {
            return Ok(());
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

async fn doc_exists(db: &Db, sha256: &str) -> Result<bool, SkbError> {
    let query = format!("SELECT count() AS c FROM document WHERE sha256 = '{sha256}' GROUP ALL");
    let mut r = db
        .db
        .query(&query)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("doc_exists: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("doc_exists take: {e}")))?;
    let count = rows.first().and_then(|v| v["c"].as_u64()).unwrap_or(0);
    Ok(count > 0)
}

async fn delete_existing(db: &Db, sha256: &str) -> Result<(), SkbError> {
    let del = format!(
        "DELETE FROM chunk WHERE document.sha256 = '{sha256}'; \
         DELETE FROM document WHERE sha256 = '{sha256}';"
    );
    db.db
        .query(&del)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete: {e}")))?;
    Ok(())
}

async fn store_document(
    db: &Db,
    doc: &DocumentData,
    chunks: &[Chunk],
    embeddings: &[Vec<f32>],
) -> Result<(String, Vec<String>), SkbError> {
    let title = doc.title.replace('\'', "\\'");
    let source = doc.source.replace('\'', "\\'");
    let content = doc.content.replace('\'', "\\'").replace('\n', "\\n");
    let mime = doc
        .mime
        .as_deref()
        .map(|value| format!("'{}'", value.replace('\'', "\\'")))
        .unwrap_or_else(|| "NONE".to_string());
    let tags_str = serde_json::to_string(&doc.tags).unwrap_or_else(|_| "[]".into());
    let meta_str = serde_json::to_string(&doc.metadata).unwrap_or_else(|_| "{}".into());

    // Create document with auto ID, returning the generated id
    let sql = format!(
        "CREATE document SET \
         title = '{title}', source = '{source}', source_type = '{stype}', \
         sha256 = '{sha}', content = '{content}', mime = {mime}, \
         tags = {tags}, metadata = {meta} \
         RETURN string::concat('document:', meta::id(id)) AS did",
        title = title,
        source = source,
        stype = doc.source_type,
        sha = doc.sha256,
        content = content,
        mime = mime,
        tags = tags_str,
        meta = meta_str,
    );
    let mut r = db
        .db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("create doc: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("create doc take: {e}")))?;
    let doc_id = rows
        .first()
        .and_then(|v| v["did"].as_str())
        .unwrap_or("unknown")
        .to_string();
    if doc_id == "unknown" {
        return Err(SkbError::new(ErrorCode::Db, "failed to get document id"));
    }

    // Create chunks, capturing each generated id
    let mut chunk_ids = Vec::with_capacity(chunks.len());
    for (i, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let emb_str = serde_json::to_string(emb).unwrap_or_else(|_| "[]".into());
        let c = chunk.content.replace('\'', "\\'").replace('\n', "\\n");
        let chunk_sql = format!(
            "CREATE chunk SET document = {doc_id}, idx = {i}, content = '{c}', \
             token_count = {tc}, embedding = {emb} \
             RETURN string::concat('chunk:', meta::id(id)) AS cid",
            tc = chunk.token_count,
            emb = emb_str,
        );
        let mut cr = db
            .db
            .query(&chunk_sql)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("chunk {i}: {e}")))?;
        let rows: Vec<serde_json::Value> = cr
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("chunk {i} take: {e}")))?;
        let cid = rows
            .first()
            .and_then(|v| v["cid"].as_str())
            .unwrap_or("")
            .to_string();
        if cid.is_empty() {
            return Err(SkbError::new(ErrorCode::Db, "failed to get chunk id"));
        }
        chunk_ids.push(cid);
    }

    Ok((doc_id, chunk_ids))
}
