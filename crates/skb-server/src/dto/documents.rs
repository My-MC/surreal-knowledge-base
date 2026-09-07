//! Server-owned document DTOs (plan todo 3).
//!
//! Separate types from [`skb_core`] so the OpenAPI document is fully owned
//! here; conversions are one-directional `From` impls — requests convert
//! server → core, responses core → server. No serde duplication of core
//! structs.

use serde::{Deserialize, Serialize};
use skb_core::crud::{ChunkInfo, DocumentDetail, DocumentSummary, ListQuery, OrderBy};
use skb_core::error::{ErrorCode, SkbError};
use skb_core::ingest::{UploadRequest, UploadResult};
use std::collections::HashMap;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

/// Body of `POST /api/documents` and `PUT /api/documents/{id}`: a transparent
/// [`UploadRequest`] passthrough. `path` is deliberately NOT accepted over
/// HTTP — a server-side file read from external input would be a path
/// traversal surface (`/etc/passwd` is a valid "source"); use
/// `content` / `content_base64` / `url`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct UploadDocumentRequest {
    /// URL to fetch and ingest.
    pub url: Option<String>,
    /// Inline UTF-8 content.
    pub content: Option<String>,
    /// Inline base64-encoded binary content.
    pub content_base64: Option<String>,
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, String>>,
    /// Only honored on POST; the PUT handler strips it (see its handler docs).
    pub force: Option<bool>,
}

impl UploadDocumentRequest {
    /// Server-side guard: blank `content` makes ingest return
    /// `status:"empty"` with `document_id: None`, which the PUT flow cannot
    /// distinguish from the dedup-skip branch — reject it with 400 before
    /// reaching core. Core's other `validate()` rules are NOT duplicated.
    pub fn validate(&self) -> Result<(), SkbError> {
        if let Some(content) = self.content.as_deref() {
            if content.trim().is_empty() {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "content must not be empty or whitespace-only",
                ));
            }
        }
        Ok(())
    }
}

impl From<UploadDocumentRequest> for UploadRequest {
    fn from(dto: UploadDocumentRequest) -> Self {
        Self {
            // No `path`: server-side file reads are never exposed to HTTP
            // callers (CWE-22); the CLI and MCP keep the field.
            path: None,
            url: dto.url,
            content: dto.content,
            content_base64: dto.content_base64,
            title: dto.title,
            tags: dto.tags,
            metadata: dto.metadata,
            force: dto.force,
        }
    }
}

/// Body of the successful `POST /api/documents` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UploadDocumentResponse {
    /// `None` when the upload was skipped (identical sha256 already ingested).
    pub document_id: Option<String>,
    pub title: String,
    /// `created` | `updated` | `skipped` | `empty`.
    pub status: String,
    pub chunks: usize,
    pub tokens: usize,
    pub sha256: String,
    pub entities: Vec<String>,
}

impl From<UploadResult> for UploadDocumentResponse {
    fn from(r: UploadResult) -> Self {
        Self {
            document_id: r.document_id,
            title: r.title,
            status: r.status,
            chunks: r.chunks,
            tokens: r.tokens,
            sha256: r.sha256,
            entities: r.entities,
        }
    }
}

/// Item of the `GET /api/documents` response array.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DocumentSummaryResponse {
    pub id: String,
    pub title: String,
    pub source: String,
    pub sha256: String,
    pub chunk_count: usize,
    pub created_at: String,
}

impl From<DocumentSummary> for DocumentSummaryResponse {
    fn from(s: DocumentSummary) -> Self {
        Self {
            id: s.id,
            title: s.title,
            source: s.source,
            sha256: s.sha256,
            chunk_count: s.chunk_count,
            created_at: s.created_at,
        }
    }
}

/// Chunk of a document detail response (`include_chunks=true`).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChunkInfoResponse {
    pub idx: usize,
    pub content: String,
    pub token_count: usize,
    pub heading: Option<String>,
}

impl From<ChunkInfo> for ChunkInfoResponse {
    fn from(c: ChunkInfo) -> Self {
        Self {
            idx: c.idx,
            content: c.content,
            token_count: c.token_count,
            heading: c.heading,
        }
    }
}

/// Body of the `GET /api/documents/{id}` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DocumentDetailResponse {
    pub id: String,
    pub title: String,
    pub source: String,
    pub source_type: String,
    pub sha256: String,
    pub content: String,
    pub chunks: Option<Vec<ChunkInfoResponse>>,
    pub created_at: String,
}

impl From<DocumentDetail> for DocumentDetailResponse {
    fn from(d: DocumentDetail) -> Self {
        Self {
            id: d.id,
            title: d.title,
            source: d.source,
            source_type: d.source_type,
            sha256: d.sha256,
            content: d.content,
            chunks: d
                .chunks
                .map(|chunks| chunks.into_iter().map(Into::into).collect()),
            created_at: d.created_at,
        }
    }
}

/// Body of the `PUT /api/documents/{id}` response. The client must re-point
/// stored references at `document_id` (it differs from the PUT target id when
/// the content changed).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UpdateDocumentResponse {
    pub document_id: String,
}

/// Query parameters of `GET /api/documents`.
///
/// `parameter_in = Query` is explicit: utoipa's derive defaults to path
/// parameters, which openapi-typescript then emits as required path
/// placeholders — these are optional query string options, not path segments.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListDocumentsParams {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// `created_desc` | `created_asc` | `title_asc` | `title_desc`.
    pub order: Option<String>,
    /// Keyset cursor `<created_at>,<id>` as returned by the previous page.
    pub after: Option<String>,
}

impl ListDocumentsParams {
    /// Convert to the core [`ListQuery`], restoring the `after` tuple.
    /// Malformed `after` (no comma) is a 400 `E_VALIDATION`; the remaining
    /// range checks stay in core's `ListQuery::validate`.
    pub fn into_core(self) -> Result<ListQuery, SkbError> {
        let order = self.order.as_deref().map(OrderBy::from_str).transpose()?;
        let after = match self.after.as_deref() {
            None => None,
            Some(raw) => {
                let (created_at, id) = raw.split_once(',').ok_or_else(|| {
                    SkbError::new(
                        ErrorCode::Validation,
                        "after must be `<created_at>,<id>` (comma-joined)",
                    )
                })?;
                Some((created_at.to_string(), id.to_string()))
            }
        };
        Ok(ListQuery {
            limit: self.limit,
            offset: self.offset,
            order,
            after,
        })
    }
}

/// Query parameters of `GET /api/documents/{id}`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GetDocumentParams {
    /// Include the document's chunks in the response.
    pub include_chunks: Option<bool>,
}

/// Error body shape returned by every failing endpoint (`{"code","message"}`).
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    /// Machine-readable code, e.g. `E_VALIDATION`.
    pub code: String,
    pub message: String,
}
