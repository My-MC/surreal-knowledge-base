//! Document CRUD acceptance tests (plan todo 3): upload → list → get →
//! PUT composite → delete, plus the empty-content guard, 404s, the
//! dedup-skip re-PUT branch, and the `after` keyset cursor.
//!
//! SurrealKv holds a cross-process exclusive lock (SPIKE.md): every test uses
//! a UNIQUE store path under ./target and the suite runs with
//! `--test-threads=1`.

mod common;

use axum::http::StatusCode;
use common::{send, test_router, test_state, upload};
use serde_json::{json, Value};

#[tokio::test]
async fn full_crud_flow_upload_list_get_put_delete() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    // POST → 201
    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/documents",
        Some(json!({"content": "# Hello\n\nworld body", "title": "Hello"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "created");
    let old_id = body["document_id"].as_str().unwrap().to_string();
    assert!(old_id.starts_with("document:"), "id format: {old_id}");

    // GET list → ≥1 entry with the expected fields
    let (status, body) = send(router.clone(), "GET", "/api/documents", None).await;
    assert_eq!(status, StatusCode::OK);
    let list = body.as_array().expect("list is an array");
    assert!(!list.is_empty());
    let summary = list
        .iter()
        .find(|d| d["id"] == old_id.as_str())
        .expect("uploaded doc listed");
    assert_eq!(summary["title"], "Hello");
    assert_eq!(summary["chunk_count"], 1);
    assert!(summary["created_at"].as_str().is_some());
    assert!(summary["sha256"].as_str().is_some());

    // GET by id → 200 detail
    let (status, body) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{old_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], old_id.as_str());
    assert_eq!(body["title"], "Hello");
    assert_eq!(body["content"], "# Hello\n\nworld body");
    assert_eq!(body["source_type"], "text");

    // PUT with changed content → new id
    let (status, body) = send(
        router.clone(),
        "PUT",
        &format!("/api/documents/{old_id}"),
        Some(json!({"content": "# Hello v2\n\nchanged body", "title": "Hello"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT failed: {body}");
    let new_id = body["document_id"].as_str().unwrap().to_string();
    assert_ne!(new_id, old_id, "changed content must mint a new id");

    // old id → 404, new id → 200 with the new content
    let (status, _) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{old_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{new_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["content"], "# Hello v2\n\nchanged body");

    // DELETE → 204, then 404
    let (status, body) = send(
        router.clone(),
        "DELETE",
        &format!("/api/documents/{new_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null, "204 must have no body");
    let (status, _) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{new_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn empty_or_blank_content_returns_400_validation() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    for content in ["", "   \n\t "] {
        let (status, body) = send(
            router.clone(),
            "POST",
            "/api/documents",
            Some(json!({"content": content})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "content={content:?}");
        assert_eq!(body["code"], "E_VALIDATION");
    }

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn missing_document_get_and_delete_return_404() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    for method in ["GET", "DELETE"] {
        let (status, body) = send(
            router.clone(),
            method,
            "/api/documents/document:does-not-exist",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(body["code"], "E_DOCUMENT_NOT_FOUND", "{method}");
    }

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn same_content_reput_returns_old_id_and_keeps_document() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    let old_id = upload(router.clone(), "# Same\n\nidentical body", "Same").await;

    // Re-PUT with byte-identical content: dedup-skip branch must keep the
    // old document alive and report its id.
    let (status, body) = send(
        router.clone(),
        "PUT",
        &format!("/api/documents/{old_id}"),
        Some(json!({"content": "# Same\n\nidentical body", "title": "Same"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "re-PUT failed: {body}");
    assert_eq!(
        body["document_id"],
        old_id.as_str(),
        "skipped branch must return the old id"
    );

    let (status, body) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{old_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "old doc must survive the re-PUT");
    assert_eq!(body["content"], "# Same\n\nidentical body");

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn content_and_url_rejected_and_existing_docs_unaffected() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    let id = upload(router.clone(), "# Keep\n\nuntouched body", "Keep").await;

    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/documents",
        Some(json!({
            "content": "# Both\n\nsources at once",
            "url": "https://example.com/doc.md"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "E_VALIDATION");

    let (status, body) = send(router.clone(), "GET", &format!("/api/documents/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "existing doc must be unaffected");
    assert_eq!(body["content"], "# Keep\n\nuntouched body");

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn list_after_cursor_rejects_malformed_and_pages_deterministically() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    let first = upload(router.clone(), "# Page one\n\nfirst doc", "Page one").await;
    let second = upload(router.clone(), "# Page two\n\nsecond doc", "Page two").await;

    // Malformed cursor (no comma) → 400 E_VALIDATION.
    let (status, body) = send(
        router.clone(),
        "GET",
        "/api/documents?after=not-a-cursor",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "E_VALIDATION");

    // Page 1 (limit 1), then resume with after=<created_at>,<id>.
    let (status, body) = send(router.clone(), "GET", "/api/documents?limit=1", None).await;
    assert_eq!(status, StatusCode::OK);
    let page1 = body.as_array().unwrap();
    assert_eq!(page1.len(), 1);
    let cursor = format!(
        "{},{}",
        page1[0]["created_at"].as_str().unwrap(),
        page1[0]["id"].as_str().unwrap()
    );
    let (status, body) = send(
        router.clone(),
        "GET",
        // Comma and colon are legal raw in a query value; no encoding needed.
        &format!("/api/documents?limit=1&after={cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cursor page failed: {body}");
    let page2 = body.as_array().unwrap();
    assert_eq!(page2.len(), 1, "second page must hold the remaining doc");
    let page2_id = page2[0]["id"].as_str().unwrap();
    assert!(
        page2_id == first || page2_id == second,
        "page 2 must be one of the seeded docs, got {page2_id}"
    );
    assert_ne!(page2_id, page1[0]["id"], "pages must not overlap");

    let _ = std::fs::remove_dir_all(db);
}
