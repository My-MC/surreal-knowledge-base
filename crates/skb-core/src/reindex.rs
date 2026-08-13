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
///   uncommitted deletes inside the same transaction; each step is idempotent.
/// - Interruption/recovery is tracked via the `reindex_in_progress` marker:
///   "dim" means a dimension transition was interrupted and the next run
///   re-executes the dimension-change path (transition + redefine_index);
///   "meta" means a metadata/tokenizer interruption, recovered by
///   rebuild_all + update_metas without wiping chunks/indexes/fields.
///   Metadata may be updated before rebuild_all (the transition writes the
///   new dimension immediately); recovery is determined by the marker, not
///   by "metadata updated only at the end".
pub async fn reindex(
    db: &Db,
    embedder: std::sync::Arc<dyn Embed>,
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
    // An active reindex-in-progress marker means a previous run was
    // interrupted. "dim" requires the full dimension-change recovery path
    // (including the transition and redefine_index) even when stored_dim
    // already matches; "meta" interruptions resume through rebuild_all +
    // update_metas without wiping chunks/indexes/fields (dimension_changed
    // stays false, so the else branch below is taken).
    let marker = db.get_meta("reindex_in_progress").await?;
    let interrupted_dim = marker.as_deref() == Some("dim");
    let dimension_changed = dimension_changed || interrupted_dim;

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
            let did = doc["did"].as_str().unwrap_or("");
            let content = doc["content"].as_str().unwrap_or("");
            // Same did/content filter as the execution path so dry-run counts
            // match rebuild_all.
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
            // Match the execution path: extract entities PER CHUNK, not from
            // the full document content.
            for chunk in chunks.iter() {
                dry_entity_names.extend(
                    crate::graph::extract_entities(&chunk.content)
                        .into_iter()
                        .map(|e| e.name),
                );
            }
            result.documents_processed += 1;
            result.chunks_created += chunks.len();
            result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
        }
        // One assignment after the loop: unique entity count across all docs.
        result.entities_extracted = dry_entity_names.len();
        // Dry runs are side-effect free: no marker, no metadata writes.
        return Ok(result);
    }

    // Reindex-in-progress marker: set before the transition begins so an
    // interrupted reindex (crash, kill) is detected on the next normal open;
    // deleted only after update_metas completes. The value records WHAT was
    // interrupted: "dim" requires the full dimension transition (wipe +
    // field redefinition + redefine_index) on rerun, "meta" only the
    // rebuild_all + metadata path. `open_inner` treats both as active.
    db.set_meta(
        "reindex_in_progress",
        if dimension_changed { "dim" } else { "meta" },
    )
    .await?;

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
        result = rebuild_all(db, embedder.clone(), tokenizer, config, &docs, progress).await?;
        // 2. Rebuild the HNSW index over the fresh new-dimension chunks.
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
            }
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        }
        update_metas(db, embedder.as_ref(), tokenizer, config).await?;
    } else {
        result = rebuild_all(db, embedder.clone(), tokenizer, config, &docs, progress).await?;
        // Always refresh metadata after a successful rebuild: even a
        // tokenizer-only change must record the new fingerprint (§5.4).
        update_metas(db, embedder.as_ref(), tokenizer, config).await?;
    }

    // Reindex completed: delete the in-progress marker (set above), then
    // report completion so CLI/MCP progress reaches 100% even when the last
    // documents were skipped by the empty did/content/chunks checks.
    crate::db::delete_meta(&db.db, "reindex_in_progress").await?;
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

/// Rebuild every document's chunks and mentions, one transaction per document
/// (spec §5.4).
async fn rebuild_all(
    db: &Db,
    embedder: std::sync::Arc<dyn Embed>,
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
            // Skipped documents still report progress so the final
            // notification always reaches total (MCP + CLI).
            if let Some(report) = progress {
                report(i + 1, total);
            }
            continue;
        }
        let chunks = tokenizer.chunk(
            content,
            config.chunking.max_tokens,
            config.chunking.overlap_tokens,
        )?;
        if chunks.is_empty() {
            if let Some(report) = progress {
                report(i + 1, total);
            }
            continue;
        }
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let batch_size = config.embedding.batch_size;
        let embedder = embedder.clone();
        let embeddings = tokio::task::spawn_blocking(move || {
            embed_in_batches(embedder.as_ref(), &texts, batch_size)
        })
        .await
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("embed join: {e}")))??;

        // begin -> rebuild_document -> commit retried as a whole on retryable
        // write conflicts (embedded SurrealKV; a transaction cannot be
        // re-committed, so a fresh one is started per attempt;
        // rebuild_document is idempotent for a given (did, chunks)).
        let entity_names =
            rebuild_document_retrying(db, did, content, &chunks, &embeddings).await?;
        entity_names.into_iter().for_each(|n| {
            all_entity_names.insert(n);
        });

        result.documents_processed += 1;
        result.chunks_created += chunks.len();
        result.tokens_total += chunks.iter().map(|c| c.token_count).sum::<usize>();
        if let Some(report) = progress {
            report(i + 1, total);
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

/// Rebuild one document's chunks within a transaction, retrying the whole
/// begin -> rebuild -> commit sequence on retryable write conflicts (embedded
/// SurrealKV; a transaction cannot be re-committed, so a fresh one is started
/// per attempt; rebuild_document is idempotent for a given (did, chunks)).
async fn rebuild_document_retrying(
    db: &Db,
    did: &str,
    content: &str,
    chunks: &[crate::tokenize::Chunk],
    embeddings: &[Vec<f32>],
) -> Result<Vec<String>, SkbError> {
    const ATTEMPTS: usize = 8;
    let mut last = None;
    let mut delay = std::time::Duration::from_millis(50);
    for attempt in 0..ATTEMPTS {
        let tx = db
            .db
            .clone()
            .begin()
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("reindex begin: {e}")))?;
        match rebuild_document(&tx, did, content, chunks, embeddings).await {
            Ok(names) => match tx.commit().await {
                Ok(_) => return Ok(names),
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
                    return Err(SkbError::new(ErrorCode::Db, format!("reindex commit: {e}")));
                }
            },
            Err(e) => {
                let _ = tx.cancel().await;
                return Err(e);
            }
        }
    }
    Err(SkbError::new(
        ErrorCode::Db,
        format!(
            "reindex commit: {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        ),
    ))
}

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
    // Recreate the heading part-of hierarchy from the ORIGINAL document body,
    // matching ingest so section links are rebuilt when extraction rules
    // change.
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
