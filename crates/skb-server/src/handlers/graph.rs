//! Graph handlers (plan todo 4): search expansion, graph query, and the
//! server-owned document backlinks walk.

use axum::extract::{Path, State};
use axum::Json;
use serde_json::Value;
use skb_core::crud::GetDocumentRequest;
use skb_core::error::{ErrorCode, SkbError};
use skb_core::graph::expand_search_hits;
use std::collections::HashSet;

use crate::api::AppState;
use crate::dto::graph::{
    BacklinkDocument, BacklinksResponse, ExpandRequest, ExpandResponse, GraphQueryRequest,
    GraphQueryResult,
};
use crate::dto::ErrorResponse;
use crate::error::ApiError;

/// Expand search hits along the knowledge graph (spec §6): each hit's chunk
/// mentions entities, `related_to` edges extend the frontier for
/// `max_expand - 1` further hops, and chunks mentioning any frontier entity
/// come back with a hop-decayed score. Bounded by core (frontier caps, hop
/// decay); no server-side validation is added.
#[utoipa::path(
    post,
    path = "/api/search/expand",
    request_body = ExpandRequest,
    responses(
        (status = 200, description = "Expanded hits plus per-hit origin entities", body = ExpandResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn expand_search(
    State(state): State<AppState>,
    Json(req): Json<ExpandRequest>,
) -> Result<Json<ExpandResponse>, ApiError> {
    // max_expand feeds the traversal's hop loop; without this bound a single
    // request could drive an unbounded walk. Reject early with the shared
    // 400 contract instead of clamping silently.
    if req.max_expand > skb_core::search::MAX_GRAPH_EXPAND {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            format!(
                "max_expand must be at most {}",
                skb_core::search::MAX_GRAPH_EXPAND
            ),
        )));
    }
    let hits: Vec<skb_core::search::SearchHit> = req.hits.into_iter().map(Into::into).collect();
    let (expanded, entity_origins) =
        expand_search_hits(state.kb.db(), &hits, req.max_expand).await?;
    Ok(Json(ExpandResponse {
        hits: expanded.into_iter().map(Into::into).collect(),
        entity_origins,
    }))
}

/// Traverse entity relations from an entity name or record id. Validation
/// (depth 1-5, limit bounds, empty `from`) is core-owned: out-of-range depth
/// surfaces as 400 E_VALIDATION, a missing start document as 404.
#[utoipa::path(
    post,
    path = "/api/graph/query",
    request_body = GraphQueryRequest,
    responses(
        (status = 200, description = "Graph nodes and edges", body = GraphQueryResult),
        (status = 400, description = "Invalid graph query", body = ErrorResponse),
        (status = 404, description = "Start document not found", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn graph_query(
    State(state): State<AppState>,
    Json(req): Json<GraphQueryRequest>,
) -> Result<Json<GraphQueryResult>, ApiError> {
    let result = state.kb.graph_query(&req.into()).await?;
    Ok(Json(result.into()))
}

/// SERVER-OWNED FIXED SQL (spec chapter arrives with plan todo 8). Parameter
/// binding ONLY — user input is never interpolated. Reverse-mentions walk:
/// document → its chunks → `mentions` edges (in = chunk, out = entity) →
/// entity names → other mentioning chunks → their documents. The second
/// statement starts FROM the `mentions` relation table: a forward traversal
/// in WHERE (`chunk WHERE ->mentions->entity.name IN …`) matches nothing on
/// surrealdb 3.x, while `out.name IN $names` over the edge rows works.
const BACKLINK_ENTITIES_SQL: &str =
    "SELECT ->mentions->entity.name AS names FROM chunk WHERE meta::id(document) = $key";
const BACKLINK_DOCUMENTS_SQL: &str =
    "SELECT meta::id(in.document) AS document_id, in.document.title AS title \
     FROM mentions WHERE out.name IN $names";

/// Documents whose chunks mention any entity this document's chunks mention
/// (reverse `mentions` walk). A document with no extracted entities has no
/// backlinks (200, empty list); a missing id is 404 E_DOCUMENT_NOT_FOUND.
#[utoipa::path(
    get,
    path = "/api/documents/{id}/backlinks",
    params(("id" = String, Path, description = "Document record id (`document:<key>`)")),
    responses(
        (status = 200, description = "Backlinking documents", body = BacklinksResponse),
        (status = 404, description = "Document not found", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn document_backlinks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BacklinksResponse>, ApiError> {
    // Existence + id-format checks are core-owned (404 E_DOCUMENT_NOT_FOUND /
    // 400 E_VALIDATION for a non-`document:<key>` id).
    let doc = state
        .kb
        .get_document(&GetDocumentRequest {
            id: id.clone(),
            include_chunks: Some(false),
        })
        .await?;
    // Core validated the id as `document:<key>`; the bare key feeds the
    // meta::id() comparison in the SQL above.
    let key = doc
        .id
        .split_once(':')
        .map(|(_, key)| key)
        .unwrap_or(&doc.id);

    let mut r = state
        .kb
        .db()
        .db
        .query(BACKLINK_ENTITIES_SQL)
        .bind(("key", key.to_string()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("backlinks entities: {e}")))?;
    let rows: Vec<Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("backlinks entities take: {e}")))?;

    // `->mentions->entity.name` is a scalar for a single edge and an array
    // for several (same normalization core's `to_string_vec` performs).
    let mut names: Vec<String> = Vec::new();
    for row in &rows {
        match row.get("names") {
            Some(Value::String(name)) => names.push(name.clone()),
            Some(Value::Array(items)) => names.extend(
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(std::string::ToString::to_string)),
            ),
            _ => {}
        }
    }
    if names.is_empty() {
        return Ok(Json(BacklinksResponse {
            documents: Vec::new(),
        }));
    }

    let mut r = state
        .kb
        .db()
        .db
        .query(BACKLINK_DOCUMENTS_SQL)
        .bind(("names", names))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("backlinks documents: {e}")))?;
    let rows: Vec<Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("backlinks documents take: {e}")))?;

    let mut seen: HashSet<String> = HashSet::new();
    let mut documents = Vec::new();
    for row in &rows {
        let Some(doc_key) = row.get("document_id").and_then(Value::as_str) else {
            continue;
        };
        // The document is never its own backlink; multi-chunk matches dedup.
        if doc_key == key || !seen.insert(doc_key.to_string()) {
            continue;
        }
        documents.push(BacklinkDocument {
            id: format!("document:{doc_key}"),
            title: row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    Ok(Json(BacklinksResponse { documents }))
}
