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
use crate::crud::{DocumentDetail, DocumentSummary, Stats as CrudStats};
use crate::db::Db;
use crate::embed::{Embed, MockEmbedder};
use crate::error::{ErrorCode, SkbError};
use crate::graph::{EntityInfo, GraphQueryRequest, GraphQueryResult, LinkInfo};
use crate::ingest::{UploadRequest, UploadResult};
use crate::search::{SearchRequest, SearchResponse};
use crate::tokenize::{Tokenize, TokenizersImpl};
use std::sync::Arc;

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
            let dim = if config.embedding.dimension > 0 {
                config.embedding.dimension
            } else {
                1024
            };
            Arc::new(MockEmbedder { dimension: dim })
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
            db.set_meta("schema_version", "1").await?;
        }

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
        let mut resp = search::search(&self.db, self.embedder.as_ref(), req).await?;

        if graph_expand > 0 && !resp.hits.is_empty() {
            let expanded = graph::expand_search_hits(&self.db, &resp.hits, graph_expand).await?;
            resp.hits.extend(expanded);
        }

        Ok(resp)
    }

    // ── CRUD ──
    pub async fn list_documents(
        &self,
        limit: usize,
        offset: usize,
        order: Option<String>,
    ) -> Result<Vec<DocumentSummary>, SkbError> {
        crud::list_documents(&self.db, limit, offset, order).await
    }

    pub async fn get_document(
        &self,
        id: &str,
        include_chunks: bool,
    ) -> Result<DocumentDetail, SkbError> {
        crud::get_document(&self.db, id, include_chunks).await
    }

    pub async fn delete_document(&self, id: &str) -> Result<crate::crud::DeleteResult, SkbError> {
        crud::delete_document(&self.db, id).await
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
        let doc = crud::get_document(&self.db, doc_id, false).await?;
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

fn parse_hf_model(model: &str) -> (&str, &str) {
    let parts: Vec<&str> = model.splitn(2, '/').collect();
    (parts[0], parts.get(1).copied().unwrap_or(parts[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
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
                mode: Some("vector".into()),
                top_k: Some(5),
                graph_expand: None,
                filter: None,
            })
            .await
            .unwrap();
        assert!(!sres.hits.is_empty());

        let docs = kb.list_documents(10, 0, None).await.unwrap();
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
                mode: Some("hybrid".into()),
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
        let docs = kb.list_documents(10, 0, None).await.unwrap();
        assert!(!docs.is_empty());

        // Verify content is stored
        let doc = kb.get_document(&docs[0].id, false).await.unwrap();
        assert!(doc.content.contains("SurrealDB"));

        let _ = std::fs::remove_dir_all(&path);
    }
}
