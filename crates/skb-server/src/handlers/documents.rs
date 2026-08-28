//! Document CRUD handlers (plan todo 3).
//!
//! PUT is a composite over core (which has no update API): fetch old → one
//! `force`-less upload → branch on the response. See [`update_document`].

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use skb_core::crud::{DeleteDocumentRequest, DocumentSummary, GetDocumentRequest, ListQuery};
use skb_core::error::{ErrorCode, SkbError};
use skb_core::ingest::UploadRequest;

use crate::api::AppState;
use crate::auth::OptionalAuth;
use crate::dto::documents::{
    DocumentDetailResponse, DocumentSummaryResponse, ErrorResponse, GetDocumentParams,
    ListDocumentsParams, UpdateDocumentResponse, UploadDocumentRequest, UploadDocumentResponse,
};
use crate::error::ApiError;
use crate::handlers::blog;

/// Scan bound for the cursor emulation in [`list_with_cursor`]: core caps
/// list limits at 10_000 (its `MAX_LIST_LIMIT`, also this app's document
/// ceiling — the MCP documents resource uses the same bound).
const CURSOR_SCAN_LIMIT: usize = 10_000;

/// Ingest a new document. The request body is a transparent
/// [`UploadRequest`] passthrough; exactly one of `path`/`url`/`content`/
/// `content_base64` must be set (enforced by core).
///
/// Uploads marked `metadata.app == "blog"` require an author JWT (401 for
/// missing/invalid tokens or reader role, 503 when the JWT secret is
/// unconfigured) and auto-create the `blog_post` registry row; other
/// uploads are unchanged and need no auth.
#[utoipa::path(
    post,
    path = "/api/documents",
    request_body = UploadDocumentRequest,
    responses(
        (status = 201, description = "Document ingested", body = UploadDocumentResponse),
        (status = 400, description = "Invalid upload request", body = ErrorResponse),
        (status = 401, description = "Blog upload without an author session", body = ErrorResponse),
        (status = 415, description = "Unsupported source format", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
        (status = 503, description = "JWT secret not configured", body = ErrorResponse),
    )
)]
pub async fn create_document(
    State(state): State<AppState>,
    auth: OptionalAuth,
    Json(req): Json<UploadDocumentRequest>,
) -> Result<(StatusCode, Json<UploadDocumentResponse>), ApiError> {
    req.validate()?;
    let is_blog = req
        .metadata
        .as_ref()
        .is_some_and(|m| m.get("app").is_some_and(|v| v == "blog"));
    let author = if is_blog {
        Some(auth.0?.require_author()?)
    } else {
        None
    };
    let result = state.kb.upload(req.into()).await?;
    if let (Some(user), Some(document_id)) = (author, result.document_id.clone()) {
        blog::create_blog_post(&state, &document_id, &user.email).await?;
    }
    Ok((StatusCode::CREATED, Json(result.into())))
}

/// List document summaries with optional limit/offset/order and the
/// `<created_at>,<id>` keyset cursor.
#[utoipa::path(
    get,
    path = "/api/documents",
    params(ListDocumentsParams),
    responses(
        (status = 200, description = "Document summaries", body = [DocumentSummaryResponse]),
        (status = 400, description = "Invalid query parameters", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn list_documents(
    State(state): State<AppState>,
    Query(params): Query<ListDocumentsParams>,
) -> Result<Json<Vec<DocumentSummaryResponse>>, ApiError> {
    let query = params.into_core()?;
    let docs = list_with_cursor(&state, query).await?;
    Ok(Json(docs.into_iter().map(Into::into).collect()))
}

/// Core's keyset-cursor SQL compares row values `(created_at, id) < (...)`,
/// which SurrealQL 3.x cannot parse — `list_documents` errors whenever
/// `after` is set (the CLI never passes it, so the path is dead there). Until
/// core fixes the statement, emulate the cursor by composition: fetch the
/// ordered store (bounded by [`CURSOR_SCAN_LIMIT`]) and slice strictly after
/// the cursor document. A cursor that matches nothing (deleted doc, or a
/// store beyond the scan bound) is a 400, never a silently wrong page.
async fn list_with_cursor(
    state: &AppState,
    query: ListQuery,
) -> Result<Vec<DocumentSummary>, ApiError> {
    // Range/combination checks (limit bounds, after+title/offset rejection)
    // stay owned by core.
    query.validate()?;
    let Some((cursor_created, cursor_id)) = query.after else {
        return state
            .kb
            .list_documents(&query)
            .await
            .map_err(ApiError::from);
    };
    let all = state
        .kb
        .list_documents(&ListQuery {
            limit: Some(CURSOR_SCAN_LIMIT),
            offset: None,
            order: query.order,
            after: None,
        })
        .await?;
    let split = all
        .iter()
        .position(|d| d.created_at == cursor_created && d.id == cursor_id)
        .ok_or_else(|| {
            SkbError::new(
                ErrorCode::Validation,
                "after cursor does not match any document in the ordered store",
            )
        })?;
    Ok(all
        .into_iter()
        .skip(split + 1)
        .take(query.limit.unwrap_or(50))
        .collect())
}

/// Fetch one document's detail; `?include_chunks=true` adds the chunks.
#[utoipa::path(
    get,
    path = "/api/documents/{id}",
    params(
        ("id" = String, Path, description = "Document record id (`document:<key>`)"),
        GetDocumentParams,
    ),
    responses(
        (status = 200, description = "Document detail", body = DocumentDetailResponse),
        (status = 404, description = "Document not found", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn get_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<GetDocumentParams>,
) -> Result<Json<DocumentDetailResponse>, ApiError> {
    let req = GetDocumentRequest {
        id,
        include_chunks: params.include_chunks,
    };
    let doc = state.kb.get_document(&req).await?;
    Ok(Json(doc.into()))
}

/// Replace a document's content. Core has no update API, so this is a
/// composite: fetch the old document (404 when absent), run ONE upload with
/// `force` stripped, then branch on the response:
///
/// - new id (`content` changed): delete the old document, return the new id —
///   clients re-point stored references at it.
/// - `skipped` (identical sha256 already ingested): keep the old document and
///   return its id — deleting here would destroy the only copy.
///
/// `force` is never honored on PUT: `force=true` upserts in place (keeping
/// the id), and the subsequent old-id delete would destroy the just-updated
/// document.
#[utoipa::path(
    put,
    path = "/api/documents/{id}",
    request_body = UploadDocumentRequest,
    params(("id" = String, Path, description = "Document record id (`document:<key>`)")),
    responses(
        (status = 200, description = "Current document id after the update", body = UpdateDocumentResponse),
        (status = 400, description = "Invalid upload request", body = ErrorResponse),
        (status = 404, description = "Document not found", body = ErrorResponse),
        (status = 415, description = "Unsupported source format", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn update_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UploadDocumentRequest>,
) -> Result<Json<UpdateDocumentResponse>, ApiError> {
    req.validate()?;
    // The old document must exist; a miss maps to E_DOCUMENT_NOT_FOUND (404).
    let old = state
        .kb
        .get_document(&GetDocumentRequest {
            id: id.clone(),
            include_chunks: None,
        })
        .await?;
    // Blog registry state must be read before the old document is deleted.
    let was_blog = blog::document_is_blog(&state, &old.id).await?;
    let mut core_req = UploadRequest::from(req);
    core_req.force = None;
    let result = state.kb.upload(core_req).await?;
    let document_id = match result.document_id {
        Some(new_id) if new_id != id => {
            if was_blog {
                blog::migrate_blog_post(&state, &old.id, &new_id).await?;
            }
            state
                .kb
                .delete_document(&DeleteDocumentRequest { id })
                .await?;
            new_id
        }
        // Same id cannot occur with force stripped (identical sha256 is
        // skipped first); if it ever did, keeping the record is the safe arm.
        Some(new_id) => new_id,
        // status "skipped": identical sha256 already ingested — keep the old doc.
        None => id,
    };
    Ok(Json(UpdateDocumentResponse { document_id }))
}

/// Delete a document and its chunks. Responds 204 with no body. The
/// `blog_post` registry row (if any) is dropped first so the published-post
/// listing never references a deleted document; the targeted delete is a
/// no-op for documents without a post.
#[utoipa::path(
    delete,
    path = "/api/documents/{id}",
    params(("id" = String, Path, description = "Document record id (`document:<key>`)")),
    responses(
        (status = 204, description = "Document deleted"),
        (status = 404, description = "Document not found", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Unconditional: a PUT can migrate a post onto a replacement document
    // whose metadata no longer carries app=blog, so the metadata marker is
    // not a reliable existence test. A missing document still surfaces as
    // the core-owned 404 below.
    blog::delete_blog_post(&state, &id).await?;
    state
        .kb
        .delete_document(&DeleteDocumentRequest { id })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
