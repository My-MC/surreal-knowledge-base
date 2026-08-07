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

pub struct KnowledgeBase {
    db: Db,
    embedder: Arc<dyn Embed>,
    tokenizer: Arc<dyn Tokenize>,
    config: Config,
}

impl KnowledgeBase {
    pub async fn open(config: Config) -> Result<Self, SkbError> {
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
        db.migrate(dimension).await?;

        let stored_model = db.get_meta("embedding_model").await?;
        if let Some(ref stored) = stored_model {
            if stored != &config.embedding.model {
                return Err(SkbError::new(
                    ErrorCode::ModelMismatch,
                    format!(
                        "config: '{}', stored: '{}'. Run reindex to switch models.",
                        config.embedding.model, stored
                    ),
                ));
            }
        } else {
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
        }

        // Tokenizer fingerprint: compute, then compare against the stored
        // fingerprint (spec §5.4 rule 3). A mismatch requires a reindex.
        let tokenizer_source = tokenizer_source_for(&config);
        let tokenizer_meta = tokenizer_fingerprint(&tokenizer_source, &tokenizer.config_json()?)?;
        sync_tokenizer_meta(&db, &config, &tokenizer_source, &tokenizer_meta).await?;

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
        let mut resp = search::search(
            &self.db,
            self.embedder.as_ref(),
            self.config.search.rrf_k,
            req,
        )
        .await?;

        if graph_expand > 0 && !resp.hits.is_empty() {
            let expanded = graph::expand_search_hits(&self.db, &resp.hits, graph_expand).await?;
            resp.hits.extend(expanded);
        }

        Ok(resp)
    }

    // ── CRUD ──
    pub async fn list_documents(&self, q: &ListQuery) -> Result<Vec<DocumentSummary>, SkbError> {
        crud::list_documents(&self.db, q).await
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

    pub async fn doctor(&self) -> Result<String, SkbError> {
        crud::doctor(&self.db, self.embedder.as_ref(), self.tokenizer.as_ref()).await
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
            &crate::crud::GetDocumentRequest {
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
    ) -> Result<reindex::ReindexResult, SkbError> {
        reindex::reindex(
            &self.db,
            self.embedder.as_ref(),
            self.tokenizer.as_ref(),
            &self.config,
            req,
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
        return Ok(());
    }
    save_tokenizer_meta(db, config, source, meta).await
}

/// Persist the tokenizer metadata unconditionally (used after a successful
/// reindex, spec §5.4 rule 3).
pub(crate) async fn save_tokenizer_meta(
    db: &Db,
    config: &Config,
    source: &str,
    meta: &TokenizerMeta,
) -> Result<(), SkbError> {
    db.set_meta("tokenizer", &config.embedding.tokenizer)
        .await?;
    db.set_meta("tokenizer_source", source).await?;
    db.set_meta("tokenizer_algorithm", &meta.algorithm).await?;
    db.set_meta("tokenizer_fingerprint_schema", TOKENIZER_FINGERPRINT_SCHEMA)
        .await?;
    db.set_meta("tokenizer_fingerprint", &meta.fingerprint)
        .await?;
    Ok(())
}

fn parse_hf_model(model: &str) -> (&str, &str) {
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    (parts[0], parts.get(1).copied().unwrap_or(parts[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchMode;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

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

    fn cleanup(kb: &KnowledgeBase) {
        let _ = std::fs::remove_dir_all(&kb.config().storage.path);
    }

    /// Write a minimal but valid `tokenizer.json` (single-token BPE) for
    /// fingerprint tests; `word` changes the vocabulary so fingerprints differ.
    fn write_fixture_tokenizer(path: &std::path::Path, word: &str) {
        use tokenizers::models::bpe::BPE;
        use tokenizers::Tokenizer;

        let mut vocab = ahash::AHashMap::default();
        vocab.insert("<unk>".to_string(), 0);
        vocab.insert(word.to_string(), 1);
        let bpe = BPE::builder()
            .vocab_and_merges(vocab, vec![])
            .unk_token("<unk>".to_string())
            .build()
            .unwrap();
        let tok = Tokenizer::new(bpe);
        std::fs::write(path, serde_json::to_string(&tok).unwrap()).unwrap();
    }

    async fn open_expecting_error(config: Config) -> SkbError {
        match KnowledgeBase::open(config).await {
            Ok(_) => panic!("expected open to fail"),
            Err(e) => e,
        }
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
        write_fixture_tokenizer(&tok_a, "alpha");
        write_fixture_tokenizer(&tok_b, "beta");

        let mut config_a = Config::default();
        config_a.embedding.onnx_path = "mock".to_string();
        config_a.embedding.dimension = 8;
        config_a.embedding.tokenizer = tok_a.display().to_string();
        config_a.storage.path = dir.join("db");

        // Closing an embedded SurrealKv releases its file lock asynchronously
        // (the connection router task runs the datastore shutdown), so a short
        // yield is needed before reopening the same path in-process.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // First open: fingerprint persisted.
            KnowledgeBase::open(config_a.clone()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            // Restart with the same tokenizer: consistent.
            KnowledgeBase::open(config_a.clone()).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;

            // Different tokenizer file: E_MODEL_MISMATCH (reindex required).
            let mut config_b = config_a;
            config_b.embedding.tokenizer = tok_b.display().to_string();
            let err = open_expecting_error(config_b).await;
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
        assert_eq!(sres.mode, "hybrid");
        assert!(!sres.hits.is_empty());

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn test_upload_rejects_multiple_sources() {
        let kb = setup().await;
        let path = kb.config().storage.path.clone();

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

        let _ = std::fs::remove_dir_all(&path);
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
    }

    #[test]
    fn graph_query_schema_marks_from_required() {
        let schema = schemars::schema_for!(GraphQueryRequest);
        let value = serde_json::to_value(&schema).unwrap();
        assert_eq!(value["required"], serde_json::json!(["from"]));
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
            .reindex(&reindex::ReindexRequest::default())
            .await
            .unwrap();
        assert_eq!(reindexed.documents_processed, 1);

        let _ = std::fs::remove_dir_all(&path);
    }
}
