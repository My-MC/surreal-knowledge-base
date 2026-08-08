use crate::config::Config;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::tokenize::Tokenize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReindexResult {
    pub documents_processed: usize,
    pub chunks_created: usize,
    pub tokens_total: usize,
    pub entities_extracted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ReindexRequest {
    /// Only report what a reindex would do, without mutating the database.
    #[serde(default)]
    pub dry_run: bool,
}

/// Rebuild every document's chunks and graph mentions atomically per document.
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
        let tx = db
            .db
            .clone()
            .begin()
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex begin: {e}")))?;
        let rebuilt = rebuild_document(&tx, did, &chunks, &embeddings).await;
        let entities_extracted = match rebuilt {
            Ok(count) => {
                tx.commit()
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex commit: {e}")))?;
                count
            }
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        };
        result.entities_extracted += entities_extracted;

        result.documents_processed += 1;
        result.chunks_created += chunks.len();
        result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
    }

    if !dry_run {
        // Record the resolved model/tokenizer metadata (spec §5.4). The
        // fingerprint comparison already happened in `KnowledgeBase::open`;
        // after a successful rebuild the stored values are refreshed.
        db.set_meta("embedding_model", &config.embedding.model)
            .await?;
        db.set_meta("embedding_dimension", &embedder.dimension().to_string())
            .await?;
        db.set_meta(
            "embedding_max_input_tokens",
            &embedder.max_input_tokens().to_string(),
        )
        .await?;
        db.set_meta("schema_version", "1").await?;
        let source = crate::tokenizer_source_for(config);
        let meta = crate::tokenizer_fingerprint(&source, &tokenizer.config_json()?)?;
        crate::save_tokenizer_meta(db, config, &source, &meta).await?;
    }

    Ok(result)
}

type LocalTransaction = surrealdb::method::Transaction<surrealdb::engine::local::Db>;

/// Replace one document's chunks and mentions within the supplied transaction.
async fn rebuild_document(
    tx: &LocalTransaction,
    did: &str,
    chunks: &[crate::tokenize::Chunk],
    embeddings: &[Vec<f32>],
) -> Result<usize, SkbError> {
    let document = surrealdb::types::RecordId::new("document", did);
    tx.query(
        "DELETE FROM mentions WHERE in.document = $document; \
         DELETE FROM chunk WHERE document = $document;",
    )
    .bind(("document", document.clone()))
    .await
    .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex del: {e}")))?
    .check()
    .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex del check: {e}")))?;

    let mut chunk_ids = Vec::with_capacity(chunks.len());
    for (i, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        let chunk_sql = "CREATE chunk SET document = $document, idx = $idx, \
                         content = $content, token_count = $token_count, \
                         heading = $heading, embedding = $embedding \
                         RETURN string::concat('chunk:', meta::id(id)) AS cid";
        let mut response = tx
            .query(chunk_sql)
            .bind(("document", document.clone()))
            .bind(("idx", i as i64))
            .bind(("content", chunk.content.clone()))
            .bind(("token_count", chunk.token_count as i64))
            .bind(("heading", chunk.heading.clone()))
            .bind(("embedding", emb.clone()))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex chunk: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex chunk check: {e}")))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex chunk take: {e}")))?;
        let cid = rows
            .first()
            .and_then(|v| v["cid"].as_str())
            .ok_or_else(|| {
                SkbError::new(
                    ErrorCode::Db,
                    format!("reindex chunk {i} did not return a chunk id"),
                )
            })?;
        chunk_ids.push(cid.to_string());
    }

    let mut entities = 0;
    for (cid, chunk) in chunk_ids.iter().zip(chunks.iter()) {
        entities +=
            crate::graph::index_chunk_entities_in_transaction(tx, cid, &chunk.content).await?;
    }
    Ok(entities)
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
