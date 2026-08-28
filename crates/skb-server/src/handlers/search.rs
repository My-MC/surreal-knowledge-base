//! Search handlers (plan todo 4).

use axum::extract::State;
use axum::Json;

use crate::api::AppState;
use crate::dto::search::{SearchRequest, SearchResponse};
use crate::dto::ErrorResponse;
use crate::error::ApiError;

/// Hybrid/vector/keyword search. The body is a transparent passthrough:
/// mode/top_k defaults and every range check are core-owned
/// (`KnowledgeBase::search` fills them from `config.search`).
#[utoipa::path(
    post,
    path = "/api/search",
    request_body = SearchRequest,
    responses(
        (status = 200, description = "Ranked search hits", body = SearchResponse),
        (status = 400, description = "Invalid search request", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, ApiError> {
    let resp = state.kb.search(req.into_core()?).await?;
    Ok(Json(resp.into()))
}
