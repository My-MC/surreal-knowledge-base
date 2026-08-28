//! Server-owned search DTOs (plan todo 4).
//!
//! Same split as [`crate::dto::documents`]: types are separate from
//! [`skb_core::search`] so the OpenAPI document is fully owned here;
//! conversions are one-directional `From` impls — requests convert
//! server → core, responses core → server. Mode parsing and every range
//! check stay in core.

use serde::{Deserialize, Serialize};
use skb_core::config::SearchMode;
use skb_core::error::SkbError;
use skb_core::search::SearchHit as CoreSearchHit;
use skb_core::search::SearchRequest as CoreSearchRequest;
use skb_core::search::SearchResponse as CoreSearchResponse;
use std::collections::HashMap;
use std::str::FromStr;
use utoipa::ToSchema;

/// Body of `POST /api/search`: a transparent search passthrough. An omitted
/// `mode`/`top_k` is filled by `KnowledgeBase::search` from the core config
/// (`config.search.default_mode` / `config.search.top_k`).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SearchRequest {
    pub query: String,
    /// `hybrid` | `vector` | `keyword`; omitted → core config default.
    pub mode: Option<String>,
    pub top_k: Option<usize>,
    /// Graph-expansion hop depth (0-5, core-validated); omitted → none.
    pub graph_expand: Option<usize>,
    /// Document-field post-filter (title/source/source_type/mime/sha256).
    pub filter: Option<HashMap<String, String>>,
}

impl SearchRequest {
    /// Convert to the core request. Unknown `mode` values are rejected by
    /// core's `SearchMode::from_str` (E_VALIDATION) — the enum is core-owned.
    pub fn into_core(self) -> Result<CoreSearchRequest, SkbError> {
        let mode = self.mode.as_deref().map(SearchMode::from_str).transpose()?;
        Ok(CoreSearchRequest {
            query: self.query,
            mode,
            top_k: self.top_k,
            graph_expand: self.graph_expand,
            filter: self.filter,
        })
    }
}

/// One search result.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SearchHit {
    pub document_id: String,
    pub chunk_idx: usize,
    pub content: String,
    pub score: f64,
    pub title: Option<String>,
    pub source: Option<String>,
    /// Query terms found in the chunk (keyword / hybrid keyword leg).
    pub highlights: Option<Vec<String>>,
    /// Entities that led to this hit via graph expansion.
    pub matched_entities: Option<Vec<String>>,
}

impl From<CoreSearchHit> for SearchHit {
    fn from(hit: CoreSearchHit) -> Self {
        Self {
            document_id: hit.document_id,
            chunk_idx: hit.chunk_idx,
            content: hit.content,
            score: hit.score,
            title: hit.title,
            source: hit.source,
            highlights: hit.highlights,
            matched_entities: hit.matched_entities,
        }
    }
}

impl From<SearchHit> for CoreSearchHit {
    fn from(hit: SearchHit) -> Self {
        Self {
            document_id: hit.document_id,
            chunk_idx: hit.chunk_idx,
            content: hit.content,
            score: hit.score,
            title: hit.title,
            source: hit.source,
            highlights: hit.highlights,
            matched_entities: hit.matched_entities,
        }
    }
}

/// Body of the `POST /api/search` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    /// Mode actually used (the configured default when the request omitted it).
    pub mode: String,
    pub elapsed_ms: u64,
}

impl From<CoreSearchResponse> for SearchResponse {
    fn from(resp: CoreSearchResponse) -> Self {
        Self {
            hits: resp.hits.into_iter().map(Into::into).collect(),
            mode: resp.mode.as_str().to_string(),
            elapsed_ms: resp.elapsed_ms,
        }
    }
}
