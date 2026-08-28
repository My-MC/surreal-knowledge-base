//! Router, application state and the OpenAPI document.
//!
//! Later todos add their endpoints to [`build_router`] and their schemas to
//! [`ApiDoc`]; keep `with_state` as the final builder step.

use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use skb_core::KnowledgeBase;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa::ToSchema;
use utoipa_swagger_ui::SwaggerUi;

use crate::config::ServerConfig;
use crate::dto::documents::{
    DocumentDetailResponse, DocumentSummaryResponse, UpdateDocumentResponse, UploadDocumentRequest,
    UploadDocumentResponse,
};
use crate::handlers::documents;

/// Shared handler state. `kb` is the single embedded-DB owner for the whole
/// process (see SPIKE.md); `server_cfg` carries the resolved listen address.
#[derive(Clone)]
pub struct AppState {
    pub kb: Arc<KnowledgeBase>,
    pub server_cfg: ServerConfig,
}

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

/// Liveness probe. Never touches the database.
#[utoipa::path(
    get,
    path = "/api/health",
    responses((status = 200, description = "Server is alive", body = HealthResponse))
)]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        documents::create_document,
        documents::list_documents,
        documents::get_document,
        documents::update_document,
        documents::delete_document,
    ),
    components(schemas(
        HealthResponse,
        UploadDocumentRequest,
        UploadDocumentResponse,
        DocumentSummaryResponse,
        DocumentDetailResponse,
        UpdateDocumentResponse,
        crate::dto::ErrorResponse,
    ))
)]
pub struct ApiDoc;

/// Full application router: JSON API + Swagger UI serving `/api/openapi.json`.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route(
            "/api/documents",
            post(documents::create_document).get(documents::list_documents),
        )
        .route(
            "/api/documents/{id}",
            get(documents::get_document)
                .put(documents::update_document)
                .delete(documents::delete_document),
        )
        .merge(SwaggerUi::new("/swagger-ui").url("/api/openapi.json", ApiDoc::openapi()))
        .with_state(state)
}
