use crate::config::Config;
use crate::db::Db;
use crate::db::MetaStore;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::tokenize::Tokenize;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Progress callback for long-running operations: `(completed, total)`.
/// Mapped to MCP progress notifications and CLI progress output (spec §7.1).
pub type ProgressFn = dyn Fn(usize, usize) + Send + Sync;

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

/// Rebuild every document's chunks and graph mentions (spec §5.4).
///
/// - No model/dimension change: per-document transactions (as before).
/// - Model or dimension change: the schema (embedding field + HNSW index) is
///   redefined and every document rebuilt. The transition is split into
///   atomic steps because SurrealDB's `DEFINE INDEX` rebuild cannot see
///   uncommitted deletes inside the same transaction; each step is idempotent
///   and the stored `meta` is only updated at the end, so any interruption
///   leaves a detectable `E_MODEL_MISMATCH` state that a re-run of `reindex`
///   completes (spec §9-5).
pub async fn reindex(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
    req: &ReindexRequest,
    progress: Option<&ProgressFn>,
) -> Result<ReindexResult, SkbError> {
    let dry_run = req.dry_run;
    let dimension = embedder.dimension();
    let stored_dim = db
        .get_meta("embedding_dimension")
        .await?
        .and_then(|v| v.parse::<usize>().ok());
    let dimension_changed = stored_dim.is_some_and(|d| d != dimension);
    // A previous dimension-change reindex was interrupted mid-rebuild: the
    // marker is still set and the stored dimension already matches, so a
    // re-run must finish rebuilding chunks AND ensure the HNSW index exists.
    let recovering = db
        .get_meta("reindex_in_progress")
        .await?
        .as_deref()
        .is_some_and(|v| v == "1")
        && !dimension_changed;

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

    if dry_run {
        let mut dry_entity_names = std::collections::HashSet::new();
        for doc in docs.iter() {
            let content = doc["content"].as_str().unwrap_or("");
            if content.is_empty() {
                continue;
            }
            let chunks = tokenizer.chunk(
                content,
                config.chunking.max_tokens,
                config.chunking.overlap_tokens,
            )?;
            if chunks.is_empty() {
                continue;
            }
            dry_entity_names.extend(
                chunks
                    .iter()
                    .flat_map(|c| crate::graph::extract_entities(&c.content))
                    .map(|e| e.name),
            );
            result.documents_processed += 1;
            result.chunks_created += chunks.len();
            result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
        }
        result.entities_extracted = dry_entity_names.len();
        // Emit the same final notification as the normal path so callbacks
        // recognize dry-run completion (docs.len(), docs.len()).
        if let Some(report) = progress {
            report(docs.len(), docs.len());
        }
        return Ok(result);
    }

    // Mark the rebuild as in progress before touching data so an interruption
    // (mid-wipe, mid-rebuild, or mid-index) is detectable on the next open with
    // the *new* config, which would otherwise pass the model/dimension/
    // fingerprint comparison and silently return empty or partial results.
    db.set_meta("reindex_in_progress", "1").await?;

    if dimension_changed {
        // 1. Atomic transition: wipe old chunks/mentions and redefine the
        //    embedding field for the new dimension (the HNSW index is rebuilt
        //    after all new chunks exist — its rebuild cannot see uncommitted
        //    deletes, and the field must exist again before inserts).
        let tx = db
            .db
            .clone()
            .begin()
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex begin: {e}")))?;
        let transition = transition_dimension(&tx, dimension).await;
        match transition {
            Ok(()) => {
                tx.commit()
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex commit: {e}")))?;
            }
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        }
        // Mark the transition immediately: from here on the stored dimension
        // matches the schema, so any interruption is detectable and a re-run
        // of `reindex` completes the rebuild (spec §9-5).
        db.set_meta("embedding_dimension", &dimension.to_string())
            .await?;
        db.set_meta("embedding_model", &config.embedding.model)
            .await?;
        result = rebuild_all(db, embedder, tokenizer, config, &docs, progress).await?;
        // 2. Rebuild the HNSW index over the fresh new-dimension chunks.
        redefine_index_transaction(db, dimension).await?;
        update_metas(db, embedder, tokenizer, config).await?;
    } else if recovering {
        // An interrupted dimension-change reindex left the marker set but the
        // stored dimension already matches: the wipe/transition committed but
        // chunks were only partially rebuilt. Rebuild everything, then ensure
        // the HNSW index exists (it may be missing from the interruption).
        result = rebuild_all(db, embedder, tokenizer, config, &docs, progress).await?;
        redefine_index_transaction(db, dimension).await?;
        update_metas(db, embedder, tokenizer, config).await?;
    } else {
        result = rebuild_all(db, embedder, tokenizer, config, &docs, progress).await?;
        // Always refresh metadata after a successful rebuild: even a
        // tokenizer-only change must record the new fingerprint (§5.4).
        update_metas(db, embedder, tokenizer, config).await?;
    }
    // The rebuild completed successfully: clear the in-progress marker so the
    // store opens normally with the new config.
    db.set_meta("reindex_in_progress", "0").await?;

    // Guarantee the final progress notification reaches 100% even when the
    // loop skipped documents (e.g. empty content), so clients see completion.
    if let Some(report) = progress {
        report(docs.len(), docs.len());
    }

    Ok(result)
}

type LocalTransaction = surrealdb::method::Transaction<surrealdb::engine::local::Db>;

/// Wipe old chunks/mentions and redefine the embedding field for a new
/// dimension. Atomic: on failure nothing is committed.
async fn transition_dimension(tx: &LocalTransaction, dimension: usize) -> Result<(), SkbError> {
    tx.query("DELETE FROM mentions; DELETE FROM chunk;")
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex wipe: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex wipe check: {e}")))?;
    let sql = format!(
        "REMOVE INDEX IF EXISTS chunk_embedding_hnsw ON chunk; \
         REMOVE FIELD IF EXISTS embedding ON chunk; \
         DEFINE FIELD embedding ON chunk TYPE array<float> \
             ASSERT array::len($value) = {dimension};"
    );
    tx.query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex redefine: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex redefine check: {e}")))?;
    Ok(())
}

/// (Re)build the HNSW index over the current chunks (which must already carry
/// embeddings of the target dimension). Runs after the rebuild so its scan
/// only sees committed, correct-dimension vectors.
async fn redefine_index(tx: &LocalTransaction, dimension: usize) -> Result<(), SkbError> {
    // Remove a possibly-existing index first so recovery from an interrupted
    // reindex can recreate it even when `DEFINE INDEX` would otherwise fail
    // on an existing definition.
    tx.query("REMOVE INDEX IF EXISTS chunk_embedding_hnsw ON chunk;")
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex index remove: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex index remove check: {e}")))?;
    let sql = format!(
        "DEFINE INDEX chunk_embedding_hnsw ON chunk \
         FIELDS embedding HNSW DIMENSION {dimension} DIST COSINE;"
    );
    tx.query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex index: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex index check: {e}")))?;
    Ok(())
}

/// Rebuild the HNSW index inside its own transaction (commit on success,
/// cancel on failure), so callers do not duplicate the begin/commit handling.
async fn redefine_index_transaction(db: &Db, dimension: usize) -> Result<(), SkbError> {
    let tx = db
        .db
        .clone()
        .begin()
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex begin: {e}")))?;
    let indexed = redefine_index(&tx, dimension).await;
    match indexed {
        Ok(()) => {
            tx.commit()
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex commit: {e}")))?;
            Ok(())
        }
        Err(e) => {
            let _ = tx.cancel().await;
            Err(e)
        }
    }
}

/// Rebuild every document's chunks and mentions, one transaction per document
/// (spec §5.4).
async fn rebuild_all(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
    docs: &[serde_json::Value],
    progress: Option<&ProgressFn>,
) -> Result<ReindexResult, SkbError> {
    let total = docs.len();
    let mut result = ReindexResult {
        documents_processed: 0,
        chunks_created: 0,
        tokens_total: 0,
        entities_extracted: 0,
    };
    let mut all_entity_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, doc) in docs.iter().enumerate() {
        let did = doc["did"].as_str().unwrap_or("");
        let content = doc["content"].as_str().unwrap_or("");
        if did.is_empty() || content.is_empty() {
            continue;
        }
        let chunks = tokenizer.chunk(
            content,
            config.chunking.max_tokens,
            config.chunking.overlap_tokens,
        )?;
        if chunks.is_empty() {
            continue;
        }
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = embed_in_batches(embedder, &texts, config.embedding.batch_size)?;

        let tx = db
            .db
            .clone()
            .begin()
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex begin: {e}")))?;
        let rebuilt = rebuild_document(&tx, did, content, &chunks, &embeddings).await;
        let entity_names = match rebuilt {
            Ok(names) => {
                tx.commit()
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex commit: {e}")))?;
                names
            }
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        };
        entity_names.into_iter().for_each(|n| {
            all_entity_names.insert(n);
        });

        result.documents_processed += 1;
        result.chunks_created += chunks.len();
        result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
        // The final (total, total) notification is emitted once by the caller
        // after the rebuild completes; skip it here to avoid duplicates.
        if let Some(report) = progress {
            if i + 1 < total {
                report(i + 1, total);
            }
        }
    }
    result.entities_extracted = all_entity_names.len();

    Ok(result)
}

/// Record the resolved model/tokenizer metadata after a successful rebuild
/// (spec §5.4 rule 3). All writes share one transaction so a partial failure
/// cannot leave stale model/dimension/fingerprint metadata behind.
async fn update_metas(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
    config: &Config,
) -> Result<(), SkbError> {
    let tx = db
        .db
        .clone()
        .begin()
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex meta begin: {e}")))?;
    let result = async {
        tx.set_meta("embedding_model", &config.embedding.model)
            .await?;
        tx.set_meta("embedding_dimension", &embedder.dimension().to_string())
            .await?;
        tx.set_meta(
            "embedding_max_input_tokens",
            &embedder.max_input_tokens().to_string(),
        )
        .await?;
        tx.set_meta("schema_version", "1").await?;
        let source = crate::tokenizer_source_for(config);
        let meta = crate::tokenizer_fingerprint(&source, &tokenizer.config_json()?)?;
        crate::save_tokenizer_meta(&tx, config, &source, &meta).await?;
        Ok::<(), SkbError>(())
    }
    .await;
    match result {
        Ok(()) => {
            tx.commit()
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex meta commit: {e}")))?;
            Ok(())
        }
        Err(e) => {
            let _ = tx.cancel().await;
            Err(e)
        }
    }
}

/// Replace one document's chunks and mentions within the supplied transaction.
async fn rebuild_document(
    tx: &LocalTransaction,
    did: &str,
    content: &str,
    chunks: &[crate::tokenize::Chunk],
    embeddings: &[Vec<f32>],
) -> Result<Vec<String>, SkbError> {
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

    let mut entity_names = std::collections::HashSet::new();
    for (cid, chunk) in chunk_ids.iter().zip(chunks.iter()) {
        let names =
            crate::graph::index_chunk_entities_in_transaction(tx, cid, &chunk.content).await?;
        entity_names.extend(names);
    }
    // Rebuild the heading-section part-of hierarchy for the whole document,
    // matching the ingest flow (spec §5.1).
    crate::graph::link_section_hierarchy(tx, content).await?;
    Ok(entity_names.into_iter().collect())
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
