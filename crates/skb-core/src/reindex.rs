use crate::config::Config;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::tokenize::Tokenize;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct ReindexResult {
    pub documents_processed: usize,
    pub chunks_created: usize,
    pub tokens_total: usize,
    pub entities_extracted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReindexRequest {
    /// Only report what a reindex would do, without mutating the database.
    #[serde(default)]
    pub dry_run: bool,
}

pub async fn reindex(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
    req: &ReindexRequest,
) -> Result<ReindexResult, SkbError> {
    let dry_run = req.dry_run;
    // Get all documents
    let find =
        "SELECT meta::id(id) AS did, title, source, source_type, content, sha256 FROM document";
    let mut r = db
        .db
        .query(find)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex find: {e}")))?;
    let docs: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex take: {e}")))?;

    let mut result = ReindexResult {
        documents_processed: 0,
        chunks_created: 0,
        tokens_total: 0,
        entities_extracted: 0,
    };

    for doc in docs.iter() {
        let did = doc["did"].as_str().unwrap_or("");
        let content = doc["content"].as_str().unwrap_or("");
        if did.is_empty() || content.is_empty() {
            continue;
        }

        // Delete old mentions + chunks for this document (no-op in dry-run)
        if !dry_run {
            let del_sql = format!(
                "LET $chunks = (SELECT value id FROM chunk WHERE meta::id(document) = '{did}'); \
                 DELETE FROM mentions WHERE ->in IN $chunks; \
                 DELETE FROM chunk WHERE meta::id(document) = '{did}';"
            );
            db.db
                .query(&del_sql)
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex del: {e}")))?;
        }

        // Re-chunk
        let chunks = tokenizer.chunk(
            content,
            config.chunking.max_tokens,
            config.chunking.overlap_tokens,
        )?;

        if chunks.is_empty() {
            continue;
        }

        if dry_run {
            let entities = crate::graph::extract_entities(content);
            result.entities_extracted += entities.len();
            result.documents_processed += 1;
            result.chunks_created += chunks.len();
            result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
            continue;
        }

        // Re-embed
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = embed_in_batches(embedder, &texts, config.embedding.batch_size)?;

        // Re-create chunks
        let mut chunk_ids: Vec<String> = Vec::with_capacity(chunks.len());
        for (i, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let emb_str = serde_json::to_string(emb).unwrap_or_else(|_| "[]".into());
            let c = chunk.content.replace('\'', "\\'").replace('\n', "\\n");
            let chunk_sql = format!(
                "CREATE chunk SET document = document:⟨{did}⟩, idx = {i}, content = '{c}', \
                 token_count = {tc}, embedding = {emb} \
                 RETURN string::concat('chunk:', meta::id(id)) AS cid",
                tc = chunk.token_count,
                emb = emb_str,
            );
            let mut cr = db
                .db
                .query(&chunk_sql)
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex chunk: {e}")))?;
            let rows: Vec<serde_json::Value> = cr
                .take(0)
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex chunk take: {e}")))?;
            if let Some(cid) = rows.first().and_then(|v| v["cid"].as_str()) {
                chunk_ids.push(cid.to_string());
            }
        }

        // Re-extract entities and rebuild mentions per chunk
        for (cid, chunk) in chunk_ids.iter().zip(chunks.iter()) {
            if let Ok(n) = crate::graph::index_chunk_entities(db, cid, &chunk.content).await {
                result.entities_extracted += n;
            }
        }

        result.documents_processed += 1;
        result.chunks_created += chunks.len();
        result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
    }

    Ok(result)
}

fn embed_in_batches(
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
