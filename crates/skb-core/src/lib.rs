pub mod config;
pub mod crud;
pub mod db;
pub mod embed;
pub mod error;
pub mod graph;
pub mod ingest;
pub mod reindex;
pub mod search;
pub mod tokenize;

use crate::config::Config;
use crate::crud::{
    DeleteDocumentRequest, DeleteResult, DocumentDetail, DocumentSummary, GetDocumentRequest,
    ListQuery, Stats as CrudStats,
};
use crate::db::Db;
use crate::embed::{Embed, MockEmbedder};
use crate::error::{ErrorCode, SkbError};
use crate::graph::{EntityInfo, GraphQueryRequest, GraphQueryResult, LinkInfo};
use crate::ingest::{UploadRequest, UploadResult};
use crate::search::{SearchRequest, SearchResponse};
use crate::tokenize::{Tokenize, TokenizersImpl};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Schema version of the tokenizer fingerprint format (spec §5.4). Bump when
/// the canonicalization rules or the covered fields change.
pub const TOKENIZER_FINGERPRINT_SCHEMA: &str = "1";

/// `tokenizers` crate version the fingerprint is bound to. Keep in sync with
/// `crates/skb-core/Cargo.toml`; bumping `tokenizers` (or the serializer)
/// changes the canonical JSON output, so a fingerprint mismatch is expected and
/// users must `skb reindex` (§5.4 rule 3).
pub const TOKENIZER_CRATE_VERSION: &str = "0.23";

pub struct KnowledgeBase {
    db: Db,
    embedder: Arc<dyn Embed>,
    tokenizer: Arc<dyn Tokenize>,
    config: Config,
}

impl KnowledgeBase {
    /// Open the knowledge base, refusing to operate when the stored
    /// model/dimension/tokenizer no longer match the configuration
    /// (`E_MODEL_MISMATCH`, spec §5.4). Use [`KnowledgeBase::open_for_reindex`]
    /// to rebuild after such a change.
    pub async fn open(config: Config) -> Result<Self, SkbError> {
        Self::open_inner(config, false).await
    }

    /// Open the knowledge base even when the stored model/dimension/tokenizer
    /// mismatch the configuration, so that `reindex` can rebuild it
    /// (spec §9-5: management path from the mismatch state).
    pub async fn open_for_reindex(config: Config) -> Result<Self, SkbError> {
        Self::open_inner(config, true).await
    }

    async fn open_inner(config: Config, allow_mismatch: bool) -> Result<Self, SkbError> {
        let db = Db::open(&config).await?;

        let tokenizer_path = resolve_tokenizer_path(&config)?;
        let tokenizer = Arc::new(TokenizersImpl::from_path(&tokenizer_path)?);

        let embedder: Arc<dyn Embed> = if config.embedding.onnx_path == "mock" {
            // MockEmbedder models a fixed 8-dimension embedder; any explicit
            // `embedding.dimension` must agree with it (spec §5.4 rule 2).
            Arc::new(MockEmbedder {
                dimension: embed::MOCK_EMBEDDER_DIMENSION,
            })
        } else {
            #[cfg(feature = "ort")]
            {
                Arc::new(embed::ort_embedder::OrtEmbedder::load(
                    &config.embedding,
                    tokenizer.clone(),
                )?)
            }
            #[cfg(not(feature = "ort"))]
            {
                return Err(SkbError::new(
                    ErrorCode::Embedding,
                    "OrtEmbedder requires the 'ort' feature. Build with: cargo build --features ort\n\
                     Or use onnx_path = \"mock\" for testing",
                ));
            }
        };

        // Resolve dimension / max_input_tokens from the model and validate the
        // chunking bounds before touching the schema (spec §5.4).
        let config =
            config.resolve_embedding_settings(embedder.dimension(), embedder.max_input_tokens())?;

        let dimension = embedder.dimension();

        // Compare the stored model/dimension BEFORE migrate so a mismatch never
        // modifies the schema, field, index or meta (spec §9-5). A brand-new
        // database has no meta table yet and takes the initialization path.
        let is_new = db.is_new_database().await?;
        if !is_new && !allow_mismatch {
            // An interrupted reindex leaves the store in a partial state (old
            // chunks wiped, new ones not all rebuilt) that normal model/
            // dimension/fingerprint comparisons cannot detect once the meta has
            // been updated to the new values. Refuse to operate until the
            // rebuild is re-run to completion (spec §9-5).
            if db.get_meta("reindex_in_progress").await?.as_deref() == Some("1") {
                return Err(SkbError::new(
                    ErrorCode::ModelMismatch,
                    "an interrupted reindex left the database incomplete. Run reindex to rebuild.",
                ));
            }
            if let Some(ref stored) = db.get_meta("embedding_model").await? {
                if stored != &config.embedding.model {
                    return Err(SkbError::new(
                        ErrorCode::ModelMismatch,
                        format!(
                            "config: '{}', stored: '{}'. Run reindex to switch models.",
                            config.embedding.model, stored
                        ),
                    ));
                }
            }
            if let Some(ref stored) = db.get_meta("embedding_dimension").await? {
                if stored != &dimension.to_string() {
                    return Err(SkbError::new(
                        ErrorCode::ModelMismatch,
                        format!(
                            "config dimension: '{dimension}', stored: '{stored}'. Run reindex to rebuild."
                        ),
                    ));
                }
            }
        }

        db.migrate(dimension).await?;

        if is_new {
            db.set_meta("embedding_model", &config.embedding.model)
                .await?;
            db.set_meta("embedding_dimension", &dimension.to_string())
                .await?;
            db.set_meta(
                "embedding_max_input_tokens",
                &config.embedding.max_input_tokens.to_string(),
            )
            .await?;
            db.set_meta("schema_version", "1").await?;
        } else if !allow_mismatch {
            // Backfill metadata for stores created before these keys existed
            // (e.g. migrate succeeded but set_meta was interrupted). Without
            // them the model/dimension comparison in open skips (None), so a
            // config change would go undetected. In allow_mismatch mode
            // (open_for_reindex) only a successful reindex may record the new
            // values, so the backfill is skipped there — matching the
            // tokenizer fingerprint handling below.
            if db.get_meta("embedding_model").await?.is_none() {
                db.set_meta("embedding_model", &config.embedding.model)
                    .await?;
            }
            if db.get_meta("embedding_dimension").await?.is_none() {
                db.set_meta("embedding_dimension", &dimension.to_string())
                    .await?;
            }
            if db.get_meta("embedding_max_input_tokens").await?.is_none() {
                db.set_meta(
                    "embedding_max_input_tokens",
                    &config.embedding.max_input_tokens.to_string(),
                )
                .await?;
            }
        }

        // Tokenizer fingerprint: compute, then compare against the stored
        // fingerprint (spec §5.4 rule 3). A mismatch requires a reindex;
        // the reindex path records the new fingerprint instead.
        let tokenizer_source = tokenizer_source_for(&config);
        let tokenizer_meta = tokenizer_fingerprint(&tokenizer_source, &tokenizer.config_json()?)?;
        if !allow_mismatch {
            sync_tokenizer_meta(&db, &config, &tokenizer_source, &tokenizer_meta).await?;
        }
        // In allow_mismatch mode (open_for_reindex) the stored metadata is left
        // untouched: only a successful reindex may write the new fingerprint,
        // so a store that is opened for reindex but never rebuilt still fails
        // the normal `open` with E_MODEL_MISMATCH (spec §9-5).

        tracing::info!(model=%config.embedding.model, dim=dimension, "KnowledgeBase opened");

        Ok(Self {
            db,
            embedder,
            tokenizer,
            config,
        })
    }

    // ── Upload ──
    pub async fn upload(&self, req: UploadRequest) -> Result<UploadResult, SkbError> {
        ingest::upload(
            &self.db,
            self.embedder.as_ref(),
            self.tokenizer.as_ref(),
            &self.config,
            req,
        )
        .await
    }

    // ── Search ──
    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse, SkbError> {
        let graph_expand = req.graph_expand.unwrap_or(0);
        let mut req = req;
        if req.mode.is_none() {
            req.mode = Some(self.config.search.default_mode);
        }
        if req.top_k.is_none() {
            req.top_k = Some(self.config.search.top_k);
        }
        let top_k = req.top_k.unwrap_or(10);
        let mut resp = search::search(
            &self.db,
            self.embedder.as_ref(),
            self.config.search.rrf_k,
            req,
        )
        .await?;

        if graph_expand > 0 && !resp.hits.is_empty() {
            // Enrich original hits with their chunk's entities, then merge the
            // expanded hits. Direct hits always fill top_k first — expanded
            // hits only take the remaining slots — so graph expansion can
            // never displace the primary results (spec §6).
            let (expanded, origin_entities) =
                graph::expand_search_hits(&self.db, &resp.hits, graph_expand).await?;
            for hit in resp.hits.iter_mut() {
                let entities = origin_entities
                    .get(&format!("{}/{}", hit.document_id, hit.chunk_idx))
                    .cloned()
                    .unwrap_or_default();
                if !entities.is_empty() {
                    hit.matched_entities = Some(entities);
                }
            }
            let mut direct = std::mem::take(&mut resp.hits);
            direct.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.document_id.cmp(&b.document_id))
                    .then_with(|| a.chunk_idx.cmp(&b.chunk_idx))
            });
            let mut expanded = expanded;
            expanded.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.document_id.cmp(&b.document_id))
                    .then_with(|| a.chunk_idx.cmp(&b.chunk_idx))
            });
            let direct_count = direct.len().min(top_k);
            let mut merged: Vec<_> = direct.into_iter().take(direct_count).collect();
            merged.extend(
                expanded
                    .into_iter()
                    .take(top_k.saturating_sub(direct_count)),
            );
            resp.hits = merged;
        }

        Ok(resp)
    }

    // ── CRUD ──
    pub async fn list_documents(&self, q: &ListQuery) -> Result<Vec<DocumentSummary>, SkbError> {
        crud::list_documents(&self.db, q).await
    }

    /// Return at most `max` documents plus whether more remain. Paging is
    /// performed in one place so callers do not hold a lock across repeated
    /// async fetches (the MCP `skb://documents` resource, spec §8.3).
    pub async fn document_snapshot(
        &self,
        max: usize,
    ) -> Result<(Vec<DocumentSummary>, bool), SkbError> {
        const PAGE: usize = 100;
        let mut docs = Vec::new();
        let mut offset = 0;
        let mut hit_offset_cap = false;
        loop {
            let page = self
                .list_documents(&ListQuery {
                    limit: Some(PAGE),
                    offset: Some(offset),
                    order: None,
                })
                .await?;
            let page_len = page.len();
            docs.extend(page);
            if page_len < PAGE || docs.len() > max {
                break;
            }
            // ListQuery rejects offsets above MAX_LIST_OFFSET; stop paginating
            // before that point and report truncation when the requested max
            // cannot be fully evaluated within the supported offset range.
            if offset + PAGE > crate::crud::MAX_LIST_OFFSET {
                hit_offset_cap = true;
                break;
            }
            offset += PAGE;
        }
        let truncated = docs.len() > max || (hit_offset_cap && docs.len() < max);
        docs.truncate(max);
        Ok((docs, truncated))
    }

    pub async fn get_document(&self, req: &GetDocumentRequest) -> Result<DocumentDetail, SkbError> {
        crud::get_document(&self.db, req).await
    }

    pub async fn delete_document(
        &self,
        req: &DeleteDocumentRequest,
    ) -> Result<DeleteResult, SkbError> {
        crud::delete_document(&self.db, req).await
    }

    pub async fn stats(&self) -> Result<CrudStats, SkbError> {
        crud::stats(&self.db, self.embedder.as_ref()).await
    }

    pub async fn doctor(&self) -> Result<crate::crud::DoctorReport, SkbError> {
        crud::doctor(&self.db, self.embedder.as_ref(), self.tokenizer.as_ref()).await
    }

    /// Execute raw SurrealQL (CLI-only escape hatch; never exposed via MCP,
    /// spec §11.1). Returns the JSON result of every statement.
    pub async fn query_surql(&self, surql: &str) -> Result<serde_json::Value, SkbError> {
        if surql.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "query must not be empty",
            ));
        }
        let r = self
            .db
            .db
            .query(surql)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("query: {e}")))?;
        // Check statement-level errors first so a failing statement surfaces
        // instead of being swallowed while collecting values.
        let mut r = r
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("query check: {e}")))?;
        // Each statement's result is exposed as its own JSON value, bounded by
        // the actual statement count so a legitimate Value::None (e.g. RETURN
        // NONE or COMMIT) is preserved rather than truncating the list.
        let n = r.num_statements();
        let mut statements: Vec<serde_json::Value> = Vec::new();
        for idx in 0..n {
            match r.take::<surrealdb::types::Value>(idx) {
                Ok(value) => statements.push(value.into_json_value()),
                Err(e) => {
                    return Err(SkbError::new(
                        ErrorCode::Db,
                        format!("query take {idx}: {e}"),
                    ))
                }
            }
        }
        Ok(serde_json::json!({ "statements": statements }))
    }

    // ── Graph ──
    pub async fn upsert_entity(&self, entity: &EntityInfo) -> Result<(), SkbError> {
        graph::upsert_entity(&self.db, entity).await
    }

    pub async fn link_entities(&self, link: &LinkInfo) -> Result<(), SkbError> {
        graph::link(&self.db, link).await
    }

    pub async fn graph_query(&self, req: &GraphQueryRequest) -> Result<GraphQueryResult, SkbError> {
        graph::graph_query(&self.db, req).await
    }

    pub async fn extract_and_save_entities(
        &self,
        doc_id: &str,
    ) -> Result<Vec<EntityInfo>, SkbError> {
        let doc = crud::get_document(
            &self.db,
            &GetDocumentRequest {
                id: doc_id.to_string(),
                include_chunks: Some(false),
            },
        )
        .await?;
        let entities = graph::extract_entities(&doc.content);

        for entity in entities.iter() {
            let _ = graph::upsert_entity(&self.db, entity).await;
        }

        Ok(entities)
    }

    // ── Reindex ──
    pub async fn reindex(
        &self,
        req: &reindex::ReindexRequest,
        progress: Option<&reindex::ProgressFn>,
    ) -> Result<reindex::ReindexResult, SkbError> {
        reindex::reindex(
            &self.db,
            self.embedder.as_ref(),
            self.tokenizer.as_ref(),
            &self.config,
            req,
            progress,
        )
        .await
    }

    // ── Accessors ──
    pub fn embedder(&self) -> &Arc<dyn Embed> {
        &self.embedder
    }
    pub fn tokenizer(&self) -> &Arc<dyn Tokenize> {
        &self.tokenizer
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn db(&self) -> &Db {
        &self.db
    }
}

fn resolve_tokenizer_path(config: &Config) -> Result<std::path::PathBuf, SkbError> {
    if config.embedding.tokenizer != "auto" {
        return Ok(std::path::PathBuf::from(&config.embedding.tokenizer));
    }
    let client = hf_hub::HFClientSync::new()
        .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("hf-hub client: {e}")))?;
    let (owner, model_name) = parse_hf_model(&config.embedding.model);
    let repo = client.model(owner, model_name);
    repo.download_file()
        .filename("tokenizer.json")
        .send()
        .map_err(|e| SkbError::new(ErrorCode::Tokenize, format!("download tokenizer: {e}")))
}

/// Tokenizer fingerprint metadata for one resolved tokenizer (spec §5.4 rule 3).
pub(crate) struct TokenizerMeta {
    fingerprint: String,
    algorithm: String,
    tokenizers_version: String,
}

/// Resolved tokenizer acquisition source used for fingerprinting: the model id
/// for `"auto"`, the explicit path otherwise (spec §5.4).
pub(crate) fn tokenizer_source_for(config: &Config) -> String {
    if config.embedding.tokenizer == "auto" {
        config.embedding.model.clone()
    } else {
        config.embedding.tokenizer.clone()
    }
}

/// Compute the tokenizer fingerprint (spec §5.4): SHA-256 over the canonical
/// JSON serialization of the tokenizer configuration (vocabulary, normalizer,
/// pre-tokenizer, post-processor, decoder, ...) plus the acquisition source and
/// the fingerprint schema version.
pub(crate) fn tokenizer_fingerprint(
    source: &str,
    config: &serde_json::Value,
) -> Result<TokenizerMeta, SkbError> {
    let algorithm = config
        .pointer("/model/type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let canonical = serde_json::to_string(&serde_json::json!({
        "schema": TOKENIZER_FINGERPRINT_SCHEMA,
        "tokenizers": TOKENIZER_CRATE_VERSION,
        "source": source,
        "algorithm": algorithm,
        "config": config,
    }))
    .map_err(|e| {
        SkbError::new(
            ErrorCode::Tokenize,
            format!("canonicalize tokenizer config: {e}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let fingerprint = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok(TokenizerMeta {
        fingerprint,
        algorithm,
        tokenizers_version: TOKENIZER_CRATE_VERSION.to_string(),
    })
}

/// Compare the computed tokenizer fingerprint against `meta` and persist it on
/// first use. A mismatch with the stored fingerprint yields `E_MODEL_MISMATCH`
/// and requires a reindex (spec §5.4 rule 3).
pub(crate) async fn sync_tokenizer_meta(
    db: &Db,
    config: &Config,
    source: &str,
    meta: &TokenizerMeta,
) -> Result<(), SkbError> {
    let stored = db.get_meta("tokenizer_fingerprint").await?;
    if let Some(stored) = stored {
        if stored != meta.fingerprint {
            return Err(SkbError::new(
                ErrorCode::ModelMismatch,
                "tokenizer fingerprint mismatch. Run reindex to rebuild with the new tokenizer.",
            ));
        }
        // Fingerprint matches: write the accompanying metadata back
        // idempotently so stale/missing tokenizer, tokenizer_source and
        // tokenizer_algorithm values from older stores are refreshed.
        return save_tokenizer_meta(db, config, source, meta).await;
    }
    save_tokenizer_meta(db, config, source, meta).await
}

/// Persist the tokenizer metadata unconditionally (used after a successful
/// reindex, spec §5.4 rule 3). Generic over the store so it can run inside a
/// transaction.
pub(crate) async fn save_tokenizer_meta<S: crate::db::MetaStore>(
    store: &S,
    config: &Config,
    source: &str,
    meta: &TokenizerMeta,
) -> Result<(), SkbError> {
    store
        .set_meta("tokenizer", &config.embedding.tokenizer)
        .await?;
    store.set_meta("tokenizer_source", source).await?;
    store
        .set_meta("tokenizer_algorithm", &meta.algorithm)
        .await?;
    store
        .set_meta("tokenizer_version", &meta.tokenizers_version)
        .await?;
    store
        .set_meta("tokenizer_fingerprint_schema", TOKENIZER_FINGERPRINT_SCHEMA)
        .await?;
    store
        .set_meta("tokenizer_fingerprint", &meta.fingerprint)
        .await?;
    Ok(())
}

fn parse_hf_model(model: &str) -> (&str, &str) {
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    (parts[0], parts.get(1).copied().unwrap_or(parts[0]))
}

/// Shared #[cfg(test)] helpers used by unit tests across the crate.
#[cfg(test)]
pub(crate) mod testutil {
    /// Write a minimal but valid `tokenizer.json` (single-token BPE) for
    /// fingerprint/chunking tests; `word` changes the vocabulary so
    /// fingerprints differ.
    pub fn write_fixture_tokenizer(path: &std::path::Path, word: &str) {
        use tokenizers::models::bpe::BPE;
        use tokenizers::pre_tokenizers::whitespace::WhitespaceSplit;
        use tokenizers::Tokenizer;

        let mut vocab = ahash::AHashMap::default();
        vocab.insert("<unk>".to_string(), 0);
        vocab.insert(word.to_string(), 1);
        let bpe = BPE::builder()
            .vocab_and_merges(vocab, vec![])
            .unk_token("<unk>".to_string())
            .build()
            .unwrap();
        let mut tok = Tokenizer::new(bpe);
        // Word-based splitting keeps heading lines in one token run (per-char
        // fallback tokens would split "## Beta" across chunks).
        tok.with_pre_tokenizer(Some(WhitespaceSplit));
        std::fs::write(path, serde_json::to_string(&tok).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMode;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn tokenizer_crate_version_matches_manifest() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            manifest.contains(&format!(
                "tokenizers = {{ version = \"{TOKENIZER_CRATE_VERSION}\""
            )) || manifest.contains(&format!("tokenizers = \"{TOKENIZER_CRATE_VERSION}\"")),
            "TOKENIZER_CRATE_VERSION ({TOKENIZER_CRATE_VERSION}) must match Cargo.toml"
        );
    }

    fn is_upload_source(name: &str) -> bool {
        matches!(name, "path" | "url" | "content" | "content_base64")
    }

    fn mock_config() -> Config {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut config = Config::default();
        config.embedding.onnx_path = "mock".to_string();
        config.embedding.dimension = 8;
        config.storage.path = std::path::PathBuf::from(format!("./target/skb-test-{n}"));
        config
    }

    async fn setup() -> KnowledgeBase {
        let config = mock_config();
        let _ = std::fs::remove_dir_all(&config.storage.path);
        KnowledgeBase::open(config).await.unwrap()
    }

    /// Build a config with a small `max_file_mb` for upload-safety tests.
    fn small_limit_config(max_file_mb: u64) -> Config {
        let mut config = mock_config();
        config.upload.max_file_mb = max_file_mb;
        config
    }

    /// Minimal valid PDF with `page_count` empty pages, hand-crafted with a
    /// classic xref table (lopdf 0.42's writer produces inline streams its own
    /// parser rejects, so files are generated directly).
    fn make_pdf(page_count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut offsets = vec![0usize];
        let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
            let id = offsets.len();
            offsets.push(out.len());
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        };
        out.extend_from_slice(b"%PDF-1.4\n");
        obj(
            &mut out,
            &mut offsets,
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
        );
        let kids: Vec<String> = (3..3 + page_count).map(|i| format!("{i} 0 R")).collect();
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {page_count} >>",
                kids.join(" ")
            ),
        );
        for _ in 0..page_count {
            obj(
                &mut out,
                &mut offsets,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
            );
        }
        let xref_start = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in offsets.iter().skip(1) {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        let size = offsets.len();
        out.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }

    fn base64_of(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn cleanup(kb: &KnowledgeBase) {
        let _ = std::fs::remove_dir_all(&kb.config().storage.path);
    }

    /// In-process reopening of the same SurrealKv path needs the previous
    /// connection's router task to finish the datastore shutdown (file lock);
    /// the open helpers below retry transient Db (lock) errors instead of a
    /// fixed sleep.
    async fn open_expecting_error(config: Config) -> SkbError {
        // A prior open's file lock may still be releasing; retry on Db errors
        // until the real validation error surfaces.
        const ATTEMPTS: usize = 8;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            match KnowledgeBase::open(config.clone()).await {
                Ok(_) => panic!("expected open to fail"),
                Err(e) if matches!(e.code, ErrorCode::Db) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                Err(e) => return e,
            }
        }
        last.expect("ATTEMPTS is non-zero")
    }

    /// Open and drop a KnowledgeBase, retrying transient embedded-SurrealKv
    /// file-lock failures instead of relying on a fixed sleep. Only used for
    /// in-process reopen sequences in tests.
    async fn open_retrying(config: Config) -> Result<KnowledgeBase, SkbError> {
        const ATTEMPTS: usize = 8;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            match KnowledgeBase::open(config.clone()).await {
                Ok(kb) => return Ok(kb),
                // Only transient file-lock failures are retried; persistent
                // errors (e.g. ModelMismatch) return immediately.
                Err(e) if !matches!(e.code, ErrorCode::Db) => return Err(e),
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
            }
        }
        Err(last.expect("ATTEMPTS is non-zero"))
    }

    /// `open_for_reindex` variant that retries transient file-lock failures.
    async fn open_for_reindex_retrying(config: Config) -> Result<KnowledgeBase, SkbError> {
        const ATTEMPTS: usize = 8;
        let mut last = None;
        for _ in 0..ATTEMPTS {
            match KnowledgeBase::open_for_reindex(config.clone()).await {
                Ok(kb) => return Ok(kb),
                Err(e) => {
                    last = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
            }
        }
        Err(last.expect("ATTEMPTS is non-zero"))
    }

    #[tokio::test]
    async fn test_graph_expansion_n_hop_with_rerank() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some(
                "[[Alpha]] project has unique zzzkeyword content about the alpha engine.".into(),
            ),
            content_base64: None,
            title: Some("doc-a".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();
        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some(
                "[[Beta]] project documents the beta engine with related details.".into(),
            ),
            content_base64: None,
            title: Some("doc-b".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        // Alpha mentions in doc A's chunks become entities; relate Alpha -> Beta.
        kb.link_entities(&LinkInfo {
            from: "Alpha".into(),
            to: "Beta".into(),
            relation: "related".into(),
            weight: Some(1.0),
        })
        .await
        .unwrap();

        let resp = kb
            .search(SearchRequest {
                query: "unique zzzkeyword alpha engine".into(),
                mode: Some(SearchMode::Hybrid),
                top_k: Some(10),
                graph_expand: Some(2),
                filter: None,
            })
            .await
            .unwrap();

        // Doc A is the direct hit and must rank above the graph-expanded doc B.
        assert!(!resp.hits.is_empty());
        assert!(resp.hits[0].title.as_deref() == Some("doc-a"));
        let doc_a = &resp.hits[0];
        assert!(
            doc_a
                .matched_entities
                .as_deref()
                .is_some_and(|e| e.iter().any(|n| n == "Alpha")),
            "direct hit must carry its chunk's entities"
        );
        let doc_b = resp
            .hits
            .iter()
            .find(|h| h.title.as_deref() == Some("doc-b"));
        let doc_b = doc_b.expect("doc B must be found via 2-hop expansion");
        assert!(
            doc_b
                .matched_entities
                .as_deref()
                .is_some_and(|e| e.iter().any(|n| n == "Beta")),
            "expanded hit must record the connecting entity"
        );
        assert!(
            doc_b.score < resp.hits[0].score,
            "re-rank must keep direct hits first"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_chunk_heading_persisted() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();
        let content = format!(
            "# Overview\n\n{}\n\n## Details\n\n{}",
            "intro text for the overview section. ".repeat(60),
            "detailed body text. ".repeat(60),
        );

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some(content),
            content_base64: None,
            title: Some("headings".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        let docs = kb
            .list_documents(&ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
        let doc = kb
            .get_document(&GetDocumentRequest {
                id: docs[0].id.clone(),
                include_chunks: Some(true),
            })
            .await
            .unwrap();
        let chunks = doc.chunks.unwrap();
        assert!(chunks.len() >= 2);
        assert!(
            chunks
                .iter()
                .any(|c| c.heading.as_deref() == Some("Overview")),
            "overview section must keep its heading"
        );
        assert!(
            chunks
                .iter()
                .any(|c| c.heading.as_deref() == Some("Details")),
            "details section must keep its heading"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_search_response_has_title_source_and_highlights() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some("full text search highlights the query words here".into()),
            content_base64: None,
            title: Some("highlight-doc".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        let kw = kb
            .search(SearchRequest {
                query: "highlights query".into(),
                mode: Some(SearchMode::Keyword),
                top_k: Some(5),
                graph_expand: None,
                filter: None,
            })
            .await
            .unwrap();
        assert!(!kw.hits.is_empty());
        let hit = &kw.hits[0];
        assert_eq!(hit.title.as_deref(), Some("highlight-doc"));
        assert!(hit.source.is_some());
        let hl = hit
            .highlights
            .as_ref()
            .expect("keyword hits have highlights");
        assert!(hl.contains(&"highlights".to_string()));
        assert!(hl.contains(&"query".to_string()));

        let vec = kb
            .search(SearchRequest {
                query: "highlights".into(),
                mode: Some(SearchMode::Vector),
                top_k: Some(5),
                graph_expand: None,
                filter: None,
            })
            .await
            .unwrap();
        assert!(vec.hits[0].title.is_some());
        assert!(
            vec.hits[0].highlights.is_none(),
            "vector mode has no highlights"
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    /// Embedder that reports one dimension but emits vectors of another —
    /// used to force a chunk-write failure inside the reindex transaction.
    struct WrongDimEmbedder {
        declared: usize,
        actual: usize,
    }

    impl Embed for WrongDimEmbedder {
        fn dimension(&self) -> usize {
            self.declared
        }
        fn max_input_tokens(&self) -> usize {
            8192
        }
        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SkbError> {
            Ok(texts.iter().map(|_| vec![0.0f32; self.actual]).collect())
        }
    }

    #[tokio::test]
    async fn test_model_mismatch_blocks_open_and_reindex_recovers() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::path::PathBuf::from(format!("./target/skb-test-mm-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_path = dir.join("tokenizer.json");
        crate::testutil::write_fixture_tokenizer(&tok_path, "alpha");

        let mut config_a = Config::default();
        config_a.embedding.onnx_path = "mock".to_string();
        config_a.embedding.dimension = 8;
        config_a.embedding.model = "model-a".to_string();
        config_a.embedding.tokenizer = tok_path.display().to_string();
        config_a.storage.path = dir.join("db");

        let kb = KnowledgeBase::open(config_a.clone()).await.unwrap();
        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some("some document body".into()),
            content_base64: None,
            title: Some("doc".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();
        drop(kb);

        // Same database, different model: normal open refuses to operate.
        let mut config_b = config_a.clone();
        config_b.embedding.model = "model-b".to_string();
        let err = open_expecting_error(config_b.clone()).await;
        assert!(matches!(err.code, ErrorCode::ModelMismatch));
        let kb = open_for_reindex_retrying(config_b.clone()).await.unwrap();
        let result = kb
            .reindex(&reindex::ReindexRequest::default(), None)
            .await
            .unwrap();
        assert_eq!(result.documents_processed, 1);
        drop(kb);

        // After the rebuild the new model opens normally.
        open_retrying(config_b).await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_dimension_change_redefines_schema_and_recovers() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::path::PathBuf::from(format!("./target/skb-test-dimchg-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_path = dir.join("tokenizer.json");
        crate::testutil::write_fixture_tokenizer(&tok_path, "alpha");

        let mut config = Config::default();
        config.embedding.onnx_path = "mock".to_string();
        config.embedding.dimension = 8;
        config.embedding.tokenizer = tok_path.display().to_string();
        config.storage.path = dir.join("db");

        let kb = KnowledgeBase::open(config.clone()).await.unwrap();
        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some("a document with some content".into()),
            content_base64: None,
            title: Some("doc".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        // Reindex with a 16-dimension embedder: schema must be redefined.
        let dim16 = MockEmbedder { dimension: 16 };
        let result = reindex::reindex(
            kb.db(),
            &dim16,
            kb.tokenizer().as_ref(),
            kb.config(),
            &reindex::ReindexRequest::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.documents_processed, 1);
        let stored_dim = kb.db().get_meta("embedding_dimension").await.unwrap();
        assert_eq!(stored_dim.as_deref(), Some("16"));
        drop(kb);
        let err = open_expecting_error(config.clone()).await;
        assert!(matches!(err.code, ErrorCode::ModelMismatch));
        let kb = open_for_reindex_retrying(config.clone()).await.unwrap();
        let result = kb
            .reindex(&reindex::ReindexRequest::default(), None)
            .await
            .unwrap();
        assert_eq!(result.documents_processed, 1);
        drop(kb);
        open_retrying(config).await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_dimension_change_interruption_is_detectable_and_recovers() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::path::PathBuf::from(format!("./target/skb-test-dimrb-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_path = dir.join("tokenizer.json");
        crate::testutil::write_fixture_tokenizer(&tok_path, "alpha");

        let mut config = Config::default();
        config.embedding.onnx_path = "mock".to_string();
        config.embedding.dimension = 8;
        config.embedding.tokenizer = tok_path.display().to_string();
        config.storage.path = dir.join("db");

        let kb = KnowledgeBase::open(config.clone()).await.unwrap();
        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some("a document with some content".into()),
            content_base64: None,
            title: Some("doc".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        // Declared 16 (drives the schema transition) but emits 8-dim vectors:
        // the rebuild fails after the transition committed.
        let broken = WrongDimEmbedder {
            declared: 16,
            actual: 8,
        };
        let err = reindex::reindex(
            kb.db(),
            &broken,
            kb.tokenizer().as_ref(),
            kb.config(),
            &reindex::ReindexRequest::default(),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Db));
        drop(kb);

        // The interrupted state is detectable: a plain open with the old
        // config must refuse to operate.
        let err = open_expecting_error(config.clone()).await;
        assert!(matches!(err.code, ErrorCode::ModelMismatch));
        let kb = open_for_reindex_retrying(config.clone()).await.unwrap();
        let result = kb
            .reindex(&reindex::ReindexRequest::default(), None)
            .await
            .unwrap();
        assert_eq!(result.documents_processed, 1);
        drop(kb);
        open_retrying(config).await.unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_reindex_reports_progress() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();
        for i in 0..2 {
            kb.upload(UploadRequest {
                path: None,
                url: None,
                content: Some(format!("document number {i} with body text")),
                content_base64: None,
                title: Some(format!("doc-{i}")),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap();
        }

        let updates: std::sync::Arc<std::sync::Mutex<Vec<(usize, usize)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress = {
            let updates = updates.clone();
            move |done: usize, total: usize| {
                updates.lock().unwrap().push((done, total));
            }
        };
        let result = kb
            .reindex(&reindex::ReindexRequest::default(), Some(&progress))
            .await
            .unwrap();
        assert_eq!(result.documents_processed, 2);

        let updates = updates.lock().unwrap();
        assert!(!updates.is_empty(), "progress callback must be invoked");
        let (last_done, last_total) = *updates.last().unwrap();
        assert_eq!(last_total, 2);
        assert_eq!(last_done, 2);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_open_rejects_dimension_mismatch() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut config = Config::default();
        config.embedding.onnx_path = "mock".to_string();
        config.embedding.dimension = 16; // mock detects 8
        config.storage.path = std::path::PathBuf::from(format!("./target/skb-test-dim-{n}"));
        let _ = std::fs::remove_dir_all(&config.storage.path);

        let err = open_expecting_error(config.clone()).await;
        assert!(matches!(err.code, ErrorCode::Validation));

        let _ = std::fs::remove_dir_all(&config.storage.path);
    }

    #[tokio::test]
    async fn test_open_rejects_max_input_tokens_mismatch() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut config = Config::default();
        config.embedding.onnx_path = "mock".to_string();
        config.embedding.dimension = 8;
        config.embedding.max_input_tokens = 4096; // mock detects 8192
        config.storage.path = std::path::PathBuf::from(format!("./target/skb-test-max-{n}"));
        let _ = std::fs::remove_dir_all(&config.storage.path);

        let err = open_expecting_error(config.clone()).await;
        assert!(matches!(err.code, ErrorCode::Validation));

        let _ = std::fs::remove_dir_all(&config.storage.path);
    }

    #[test]
    fn test_tokenizer_fingerprint_mismatch_on_restart() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::path::PathBuf::from(format!("./target/skb-test-tok-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_a = dir.join("tokenizer-a.json");
        let tok_b = dir.join("tokenizer-b.json");
        crate::testutil::write_fixture_tokenizer(&tok_a, "alpha");
        crate::testutil::write_fixture_tokenizer(&tok_b, "beta");

        let mut config_a = Config::default();
        config_a.embedding.onnx_path = "mock".to_string();
        config_a.embedding.dimension = 8;
        config_a.embedding.tokenizer = tok_a.display().to_string();
        config_a.storage.path = dir.join("db");

        // Closing an embedded SurrealKv releases its file lock asynchronously
        // (the connection router task runs the datastore shutdown), so a reopen
        // of the same path in-process may transiently fail on the file lock.
        // open_retrying absorbs that race with bounded retries instead of a
        // fixed sleep (test stability on slow CI).
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // First open: fingerprint persisted.
            open_retrying(config_a.clone()).await.unwrap();

            // Restart with the same tokenizer: consistent.
            open_retrying(config_a.clone()).await.unwrap();

            // Different tokenizer file: E_MODEL_MISMATCH (reindex required).
            // open_retrying also absorbs the transient file-lock race on this
            // final reopen; the persistent mismatch surfaces as the last error.
            let mut config_b = config_a;
            config_b.embedding.tokenizer = tok_b.display().to_string();
            let err = match open_retrying(config_b).await {
                Ok(_) => panic!("expected open to fail with a mismatch"),
                Err(e) => e,
            };
            assert!(matches!(err.code, ErrorCode::ModelMismatch));
        });
        drop(rt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_for_reindex_without_rebuild_still_mismatches() {
        // Opening in allow_mismatch mode must NOT write the new fingerprint:
        // if the store is never rebuilt, the next normal open still reports
        // E_MODEL_MISMATCH so stale chunks are never used silently (spec §9-5).
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::path::PathBuf::from(format!("./target/skb-test-mismatch-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tok_a = dir.join("tokenizer-a.json");
        let tok_b = dir.join("tokenizer-b.json");
        crate::testutil::write_fixture_tokenizer(&tok_a, "alpha");
        crate::testutil::write_fixture_tokenizer(&tok_b, "beta");

        let mut config_a = Config::default();
        config_a.embedding.onnx_path = "mock".to_string();
        config_a.embedding.dimension = 8;
        config_a.embedding.tokenizer = tok_a.display().to_string();
        config_a.storage.path = dir.join("db");

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // First open with tokenizer-a: fingerprint persisted.
            open_retrying(config_a.clone()).await.unwrap();

            // open_for_reindex with tokenizer-b succeeds in allow_mismatch mode.
            let mut config_b = config_a.clone();
            config_b.embedding.tokenizer = tok_b.display().to_string();
            // Absorb the same transient file-lock race as open_retrying.
            open_for_reindex_retrying(config_b.clone())
                .await
                .expect("open_for_reindex must succeed");

            // But the new fingerprint was NOT recorded: a normal open still
            // reports E_MODEL_MISMATCH (rebuild required).
            let err = match open_retrying(config_b).await {
                Ok(_) => panic!("expected open to fail with a mismatch"),
                Err(e) => e,
            };
            assert!(matches!(err.code, ErrorCode::ModelMismatch));
        });
        drop(rt);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_open() {
        let kb = setup().await;
        assert_eq!(kb.embedder().dimension(), 8);
        cleanup(&kb);
    }

    #[tokio::test]
    async fn test_get_document_missing_id_is_not_found() {
        let kb = setup().await;
        let err = kb
            .get_document(&GetDocumentRequest {
                id: "document:missing".into(),
                include_chunks: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::DocumentNotFound));
        cleanup(&kb);
    }

    #[tokio::test]
    async fn test_upload_rejects_oversized_base64() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut config = small_limit_config(1);
        config.storage.path = std::path::PathBuf::from(format!("./target/skb-test-b64-{n}"));
        let _ = std::fs::remove_dir_all(&config.storage.path);
        let kb = KnowledgeBase::open(config).await.unwrap();
        let path = kb.config().storage.path.clone();

        let big = vec![b'x'; 2 * 1024 * 1024];
        let err = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: None,
                content_base64: Some(base64_of(&big)),
                title: None,
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_oversized_inline_content() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut config = small_limit_config(1);
        config.storage.path = std::path::PathBuf::from(format!("./target/skb-test-ct-{n}"));
        let _ = std::fs::remove_dir_all(&config.storage.path);
        let kb = KnowledgeBase::open(config).await.unwrap();
        let path = kb.config().storage.path.clone();

        let big = "x".repeat(2 * 1024 * 1024);
        let err = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: Some(big),
                content_base64: None,
                title: None,
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_unsupported_binary() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        // PNG magic bytes, not UTF-8 text.
        let png = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00];
        let err = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: None,
                content_base64: Some(base64_of(&png)),
                title: Some("image.png".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::UnsupportedFormat));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_private_ip_url() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        let err = kb
            .upload(UploadRequest {
                path: None,
                url: Some("http://127.0.0.1:9/x.md".into()),
                content: None,
                content_base64: None,
                title: None,
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_pdf_page_bomb() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        let bomb = make_pdf(crate::ingest::MAX_PDF_PAGES + 1);
        let err = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: None,
                content_base64: Some(base64_of(&bomb)),
                title: Some("bomb.pdf".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));
        assert!(err.message.contains("pages"));

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_accepts_small_pdf() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        let pdf = make_pdf(1);
        let result = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: None,
                content_base64: Some(base64_of(&pdf)),
                title: Some("small.pdf".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap();
        // Empty page text extracts to nothing -> no chunks.
        assert_eq!(result.status, "empty");
        assert_eq!(result.sha256.len(), 64);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_force_replaces_chunks() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();
        let content = "SurrealDB is a multi-model database. ".repeat(200);

        let first = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: Some(content.clone()),
                content_base64: None,
                title: Some("force-test".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap();
        assert_eq!(first.status, "created");
        assert!(first.chunks > 0);

        // Same content without force: skipped.
        let skipped = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: Some(content.clone()),
                content_base64: None,
                title: Some("force-test".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap();
        assert_eq!(skipped.status, "skipped");

        // Same content with force: replaced (updated).
        let updated = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: Some(content.clone()),
                content_base64: None,
                title: Some("force-test".into()),
                tags: None,
                metadata: None,
                force: Some(true),
            })
            .await
            .unwrap();
        assert_eq!(updated.status, "updated");
        assert_eq!(updated.chunks, first.chunks);

        let docs = kb
            .list_documents(&ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
        assert_eq!(docs.len(), 1);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rolls_back_on_store_failure() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        // An embedder with the wrong dimension makes the chunk write fail
        // inside the transaction; the document must not survive.
        let bad_embedder = MockEmbedder { dimension: 3 };
        let err = ingest::upload(
            kb.db(),
            &bad_embedder,
            kb.tokenizer().as_ref(),
            kb.config(),
            UploadRequest {
                path: None,
                url: None,
                content: Some("some text that will fail to store".into()),
                content_base64: None,
                title: Some("rollback-test".into()),
                tags: None,
                metadata: None,
                force: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Db));

        let docs = kb
            .list_documents(&ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
        assert!(docs.is_empty());

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_and_search() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        let result = kb.upload(UploadRequest {
            path: None, url: None,
            content: Some("SurrealDB is a multi-model database with HNSW vector search and BM25 full-text search.".into()),
            content_base64: None, title: Some("test-doc".into()),
            tags: Some(vec!["test".into()]), metadata: None, force: None,
        }).await.unwrap();
        assert_eq!(result.status, "created");

        let sres = kb
            .search(SearchRequest {
                query: "database".into(),
                mode: Some(SearchMode::Vector),
                top_k: Some(5),
                graph_expand: None,
                filter: None,
            })
            .await
            .unwrap();
        assert!(!sres.hits.is_empty());

        let docs = kb
            .list_documents(&ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
        assert!(!docs.is_empty());

        let stats = kb.stats().await.unwrap();
        assert!(stats.document_count >= 1);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_hybrid_search() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content_base64: None,
            content: Some("Vector search uses HNSW for fast nearest neighbor lookup.".into()),
            title: Some("doc1".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content_base64: None,
            content: Some(
                "BM25 is a keyword-based retrieval function for full-text search.".into(),
            ),
            title: Some("doc2".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        let sres = kb
            .search(SearchRequest {
                query: "vector search".into(),
                mode: Some(SearchMode::Hybrid),
                top_k: Some(5),
                graph_expand: None,
                filter: None,
            })
            .await
            .unwrap();
        assert_eq!(sres.mode, SearchMode::Hybrid);
        // Both documents must surface as distinct hits (regression: hybrid RRF
        // used to merge every row under an empty chunk id).
        assert!(!sres.hits.is_empty());
        let titles: Vec<Option<&str>> = sres.hits.iter().map(|h| h.title.as_deref()).collect();
        assert!(titles.contains(&Some("doc1")), "hits: {titles:?}");
        assert!(titles.contains(&Some("doc2")), "hits: {titles:?}");

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_multiple_sources() {
        let kb = setup().await;

        let err = kb
            .upload(UploadRequest {
                path: Some("a.md".into()),
                url: Some("https://example.com/a.md".into()),
                content: None,
                content_base64: None,
                title: None,
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));
        assert!(err.message.contains("only one"));

        let err = kb
            .upload(UploadRequest {
                path: None,
                url: None,
                content: None,
                content_base64: None,
                title: Some("empty".into()),
                tags: None,
                metadata: None,
                force: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Validation));

        cleanup(&kb);
    }

    #[test]
    fn upload_request_schema_has_one_of() {
        let schema = schemars::schema_for!(UploadRequest);
        let value = serde_json::to_value(&schema).unwrap();
        let one_of = value["oneOf"].as_array().expect("oneOf missing");
        assert_eq!(one_of.len(), 4);
        assert!(one_of
            .iter()
            .any(|e| e["required"] == serde_json::json!(["path"])));
        assert!(one_of
            .iter()
            .any(|e| e["required"] == serde_json::json!(["content_base64"])));
        // Each oneOf branch must null out the alternative input sources so the
        // branches are mutually exclusive (spec §12.3, one source only).
        for e in one_of.iter() {
            let required = e["required"].as_array().unwrap();
            let required_name = required[0].as_str().unwrap();
            let nulled = e["properties"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(name, _)| {
                    name.as_str() != required_name && is_upload_source(name.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                nulled.len(),
                3,
                "branch {required_name} must null 3 sources"
            );
            for (_, schema) in nulled {
                assert_eq!(schema["type"], serde_json::json!("null"));
            }
        }
    }

    #[test]
    fn graph_query_schema_marks_from_required() {
        let schema = schemars::schema_for!(GraphQueryRequest);
        let value = serde_json::to_value(&schema).unwrap();
        assert_eq!(value["required"], serde_json::json!(["from"]));
    }

    #[tokio::test]
    async fn test_section_hierarchy_part_of_direction() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content: Some("# Alpha\n\nbody\n\n## Beta\n\nmore body\n".into()),
            content_base64: None,
            title: Some("hierarchy".into()),
            tags: None,
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        // Beta is part of Alpha: the edge must point Beta ->part-of-> Alpha.
        let result = kb
            .graph_query(&GraphQueryRequest {
                from: "Beta".into(),
                relation: Some("part-of".into()),
                depth: Some(1),
                limit: Some(10),
            })
            .await
            .unwrap();
        assert!(
            result.edges.iter().any(|e| {
                e.from == "entity:⟨Beta⟩" && e.to == "entity:⟨Alpha⟩" && e.relation == "part-of"
            }),
            "expected Beta ->part-of-> Alpha, got {:?}",
            result.edges
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_graph_and_reindex() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

        kb.upload(UploadRequest {
            path: None,
            url: None,
            content_base64: None,
            content: Some(
                "# SurrealDB\n\nMulti-model database. [HNSW](https://hns.wiki). #database".into(),
            ),
            title: Some("graph-test".into()),
            tags: Some(vec!["ml".into()]),
            metadata: None,
            force: None,
        })
        .await
        .unwrap();

        // Verify document was created
        let docs = kb
            .list_documents(&ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
        assert!(!docs.is_empty());

        // Verify content is stored
        let doc = kb
            .get_document(&GetDocumentRequest {
                id: docs[0].id.clone(),
                include_chunks: Some(false),
            })
            .await
            .unwrap();
        assert!(doc.content.contains("SurrealDB"));

        let graph = kb
            .graph_query(&GraphQueryRequest {
                from: docs[0].id.clone(),
                relation: None,
                depth: Some(1),
                limit: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(
            graph.nodes.first().map(|node| node.kind.as_str()),
            Some("document")
        );

        for name in ["A", "B", "C"] {
            kb.upsert_entity(&EntityInfo {
                name: name.into(),
                kind: "test".into(),
                description: None,
            })
            .await
            .unwrap();
        }
        for (from, to) in [("A", "B"), ("B", "C")] {
            kb.link_entities(&LinkInfo {
                from: from.into(),
                to: to.into(),
                relation: "related".into(),
                weight: None,
            })
            .await
            .unwrap();
        }
        let multi_hop = kb
            .graph_query(&GraphQueryRequest {
                from: "A".into(),
                relation: None,
                depth: Some(2),
                limit: Some(10),
            })
            .await
            .unwrap();
        assert!(multi_hop.nodes.iter().any(|node| node.name == "B"));
        assert!(multi_hop.nodes.iter().any(|node| node.name == "C"));

        let reindexed = kb
            .reindex(&reindex::ReindexRequest::default(), None)
            .await
            .unwrap();
        assert_eq!(reindexed.documents_processed, 1);

        let _ = std::fs::remove_dir_all(&path);
    }
}
