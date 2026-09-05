//! Blog registry handlers (plan todo 7): the public published-post listing
//! and the author-only publish guard, plus the `blog_post` consistency
//! helpers shared with the document CRUD handlers.
//!
//! All statements are fixed SQL with string-bound parameters; record links
//! are built with `type::record(table, key)` — user input is never
//! interpolated (same discipline as the T4 backlinks walk; skb-server has no
//! direct surrealdb dependency).

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use serde_json::Value;
use skb_core::error::{ErrorCode, SkbError};
use utoipa::ToSchema;

use crate::api::AppState;
use crate::auth::AuthUser;
use crate::dto::ErrorResponse;
use crate::error::ApiError;

const POST_OWNER_SQL: &str = "SELECT author.email AS author_email, published FROM blog_post \
     WHERE document = type::record('document', $key) LIMIT 1";
const AUTHOR_ID_SQL: &str = "SELECT meta::id(id) AS user_key FROM user WHERE email = $email";
const CREATE_POST_SQL: &str =
    "CREATE blog_post SET document = type::record('document', $doc_key), \
     author = type::record('user', $user_key), published = $published";
const MIGRATE_POST_SQL: &str =
    "UPDATE blog_post SET document = type::record('document', $new_key) \
     WHERE document = type::record('document', $old_key)";
const DELETE_POST_SQL: &str =
    "DELETE FROM blog_post WHERE document = type::record('document', $key)";
const PUBLISH_POST_SQL: &str =
    "UPDATE blog_post SET published = true WHERE document = type::record('document', $key)";
const LIST_POSTS_SQL: &str = "SELECT meta::id(document) AS document_id, document.title AS title, \
     created_at, author.email AS author \
     FROM blog_post WHERE published = true ORDER BY created_at DESC";

/// Map a query/step failure onto 500 `E_DB` with a stable context prefix.
fn db_err<E: std::fmt::Display>(context: &'static str) -> impl Fn(E) -> ApiError {
    move |e| ApiError::from(SkbError::new(ErrorCode::Db, format!("{context}: {e}")))
}

/// Bare record key of a full `document:<key>` id (accepts a bare key, same
/// normalization as the backlinks handler).
fn document_key(id: &str) -> &str {
    id.split_once(':').map(|(_, key)| key).unwrap_or(id)
}

/// Owner + publication state of the `blog_post` row for a document.
pub struct BlogPostOwner {
    pub email: String,
    pub published: bool,
}

/// Author email and published flag of the blog_post row for a document, or
/// `None` when the document has no registry row. The registry is the single
/// source of truth for "is this a blog document" — the flexible
/// `metadata.app` marker is advisory and can be dropped by a later PUT.
pub async fn blog_post_owner(
    state: &AppState,
    document_id: &str,
) -> Result<Option<BlogPostOwner>, ApiError> {
    let mut r = state
        .kb
        .db()
        .db
        .query(POST_OWNER_SQL)
        .bind(("key", document_key(document_id).to_string()))
        .await
        .map_err(db_err("blog_post owner"))?;
    let rows: Vec<Value> = r.take(0).map_err(db_err("blog_post owner take"))?;
    let Some(email) = rows
        .first()
        .and_then(|row| row.get("author_email"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let published = rows
        .first()
        .and_then(|row| row.get("published"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Some(BlogPostOwner { email, published }))
}

/// Create the `blog_post` registry row for a freshly uploaded blog document
/// (author resolved from the JWT email). The `published` flag is passed
/// explicitly so failed delete flows can restore the exact prior state.
pub async fn create_blog_post(
    state: &AppState,
    document_id: &str,
    author_email: &str,
    published: bool,
) -> Result<(), ApiError> {
    let mut r = state
        .kb
        .db()
        .db
        .query(AUTHOR_ID_SQL)
        .bind(("email", author_email.to_string()))
        .await
        .map_err(db_err("author lookup"))?;
    let rows: Vec<Value> = r.take(0).map_err(db_err("author lookup take"))?;
    let Some(user_key) = rows
        .first()
        .and_then(|row| row.get("user_key"))
        .and_then(Value::as_str)
    else {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            format!("author not found: {author_email}"),
        )));
    };
    let mut r = state
        .kb
        .db()
        .db
        .query(CREATE_POST_SQL)
        .bind(("doc_key", document_key(document_id).to_string()))
        .bind(("user_key", user_key.to_string()))
        .bind(("published", published))
        .await
        .map_err(db_err("blog_post create"))?;
    let _created: Vec<Value> = r.take(0).map_err(db_err("blog_post create take"))?;
    Ok(())
}

/// Re-point the blog_post row at the replacement document after a PUT minted
/// a new id (author and published state are preserved by the UPDATE).
pub async fn migrate_blog_post(
    state: &AppState,
    old_document_id: &str,
    new_document_id: &str,
) -> Result<(), ApiError> {
    let mut r = state
        .kb
        .db()
        .db
        .query(MIGRATE_POST_SQL)
        .bind(("old_key", document_key(old_document_id).to_string()))
        .bind(("new_key", document_key(new_document_id).to_string()))
        .await
        .map_err(db_err("blog_post migrate"))?;
    let _updated: Vec<Value> = r.take(0).map_err(db_err("blog_post migrate take"))?;
    Ok(())
}

/// Drop the blog_post row for a deleted document (dangling-reference
/// prevention).
pub async fn delete_blog_post(state: &AppState, document_id: &str) -> Result<(), ApiError> {
    let mut r = state
        .kb
        .db()
        .db
        .query(DELETE_POST_SQL)
        .bind(("key", document_key(document_id).to_string()))
        .await
        .map_err(db_err("blog_post delete"))?;
    let _deleted: Vec<Value> = r.take(0).map_err(db_err("blog_post delete take"))?;
    Ok(())
}

/// Item of the `GET /api/blog/posts` response array.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlogPostSummary {
    /// Full document record id (`document:<key>`).
    pub document_id: String,
    /// Title from the underlying document record.
    pub title: String,
    pub created_at: String,
    /// Author email.
    pub author: String,
}

fn summary(row: &Value) -> BlogPostSummary {
    let field = |name: &str| {
        row.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    BlogPostSummary {
        document_id: format!("document:{}", field("document_id")),
        title: field("title"),
        created_at: field("created_at"),
        author: field("author"),
    }
}

/// Published blog posts, newest first. Public — no auth.
#[utoipa::path(
    get,
    path = "/api/blog/posts",
    responses(
        (status = 200, description = "Published blog posts", body = [BlogPostSummary]),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn list_posts(
    State(state): State<AppState>,
) -> Result<Json<Vec<BlogPostSummary>>, ApiError> {
    let mut r = state
        .kb
        .db()
        .db
        .query(LIST_POSTS_SQL)
        .await
        .map_err(db_err("blog posts"))?;
    let rows: Vec<Value> = r.take(0).map_err(db_err("blog posts take"))?;
    Ok(Json(rows.iter().map(summary).collect()))
}

/// Body of the successful publish response.
#[derive(Debug, Serialize, ToSchema)]
pub struct PublishResponse {
    pub document_id: String,
    pub published: bool,
}

/// Publish the blog post for a document (author JWT required; readers and
/// unauthenticated callers get 401, an unconfigured secret 503).
#[utoipa::path(
    post,
    path = "/api/blog/posts/{document_id}/publish",
    params(("document_id" = String, Path, description = "Document record id (`document:<key>`)")),
    responses(
        (status = 200, description = "Post published", body = PublishResponse),
        (status = 401, description = "Missing/invalid token or non-author role", body = ErrorResponse),
        (status = 404, description = "No blog post for the document", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
        (status = 503, description = "JWT secret not configured", body = ErrorResponse),
    )
)]
pub async fn publish_post(
    State(state): State<AppState>,
    user: AuthUser,
    Path(document_id): Path<String>,
) -> Result<Json<PublishResponse>, ApiError> {
    user.require_author()?;
    let mut r = state
        .kb
        .db()
        .db
        .query(PUBLISH_POST_SQL)
        .bind(("key", document_key(&document_id).to_string()))
        .await
        .map_err(db_err("blog publish"))?;
    let updated: Vec<Value> = r.take(0).map_err(db_err("blog publish take"))?;
    if updated.is_empty() {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::DocumentNotFound,
            format!("no blog post for {document_id}"),
        )));
    }
    Ok(Json(PublishResponse {
        document_id,
        published: true,
    }))
}
