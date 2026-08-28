//! Search / expand / graph-query / backlinks acceptance tests (plan todo 4):
//! hybrid search with scores, the expand envelope, core-owned depth
//! validation, and the reverse-mentions backlinks walk.
//!
//! SurrealKv holds a cross-process exclusive lock (SPIKE.md): every test uses
//! a UNIQUE store path under ./target and the suite runs with
//! `--test-threads=1`.

mod common;

use axum::http::StatusCode;
use common::{send, test_router, test_state, upload};
use serde_json::json;

#[tokio::test]
async fn hybrid_search_returns_scored_hits_with_fields() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    // Two docs sharing the seeded term; both also carry the [[Alpha]] entity.
    let doc_a = upload(
        router.clone(),
        "Alpha engine notes with unique zzzsearchterm content [[Alpha]].",
        "doc-a",
    )
    .await;
    let doc_b = upload(
        router.clone(),
        "Beta project docs referencing [[Alpha]] and the zzzsearchterm engine.",
        "doc-b",
    )
    .await;

    // Empty query → 400 E_VALIDATION (core-owned validation passthrough).
    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/search",
        Some(json!({"query": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "E_VALIDATION");

    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/search",
        Some(json!({"query": "zzzsearchterm alpha", "mode": "hybrid", "top_k": 10})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search failed: {body}");
    assert_eq!(body["mode"], "hybrid");
    assert!(body["elapsed_ms"].is_u64());
    let hits = body["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty(), "expected hits for the seeded term");
    for hit in hits {
        assert!(
            hit["score"].as_f64().unwrap_or(0.0) > 0.0,
            "score must be positive: {hit}"
        );
        assert!(hit["document_id"].is_string(), "{hit}");
        assert!(hit["chunk_idx"].is_u64(), "{hit}");
        assert!(hit["content"].is_string(), "{hit}");
        assert!(hit["title"].is_string(), "{hit}");
        assert!(hit["source"].is_string(), "{hit}");
    }
    // SearchHit.document_id is the bare record key (core's meta::id passthrough),
    // while upload() returns the full `document:<key>` id.
    fn bare(full: &str) -> &str {
        full.strip_prefix("document:").unwrap_or(full)
    }
    let hit_docs: Vec<&str> = hits
        .iter()
        .filter_map(|h| h["document_id"].as_str())
        .collect();
    assert!(
        hit_docs.contains(&bare(&doc_a)),
        "doc A must hit, got {hit_docs:?}"
    );
    assert!(
        hit_docs.contains(&bare(&doc_b)),
        "doc B must hit, got {hit_docs:?}"
    );

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn search_expand_returns_hits_and_entity_origins() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    upload(
        router.clone(),
        "Foo engine overview with zzzexpandterm body. See [[Foo]] for more.",
        "foo-doc",
    )
    .await;

    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/search",
        Some(json!({"query": "zzzexpandterm", "mode": "hybrid"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search failed: {body}");
    let hits = body["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty());

    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/search/expand",
        Some(json!({"hits": hits, "max_expand": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "expand failed: {body}");
    assert!(body["hits"].is_array());
    let origins = body["entity_origins"].as_object().expect("origins object");
    assert!(
        !origins.is_empty(),
        "the hit chunk mentions Foo, origins must be populated"
    );
    let foo_recorded = origins.values().any(|entities| {
        entities
            .as_array()
            .is_some_and(|e| e.iter().any(|v| v == "Foo"))
    });
    assert!(
        foo_recorded,
        "entity_origins must record Foo for the hit: {origins:?}"
    );

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn graph_query_validates_depth_and_returns_nodes_edges() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    upload(
        router.clone(),
        "Graph seed doc mentioning [[Graphtest]] entity.",
        "graph-doc",
    )
    .await;

    // depth=6 → 400 E_VALIDATION (core owns the 1-5 range).
    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/graph/query",
        Some(json!({"from": "Graphtest", "depth": 6})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "E_VALIDATION");

    // depth=1 → 200 with nodes/edges arrays; the seeded entity resolves.
    let (status, body) = send(
        router.clone(),
        "POST",
        "/api/graph/query",
        Some(json!({"from": "Graphtest", "depth": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "graph query failed: {body}");
    let nodes = body["nodes"].as_array().expect("nodes array");
    assert!(!nodes.is_empty(), "seeded entity must resolve to a node");
    assert_eq!(nodes[0]["name"], "Graphtest");
    assert!(body["edges"].is_array());

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn backlinks_reverse_mentions_walk_finds_linking_document() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    // Doc A exposes entity "Foo" via a heading (rule-based extraction:
    // headings longer than two chars become section entities).
    let doc_a = upload(
        router.clone(),
        "# Foo\n\nFoo body with zzzbacklinkterm.",
        "doc-a",
    )
    .await;
    // Doc B links [[Foo]] — its chunk mentions the same entity record.
    let doc_b = upload(
        router.clone(),
        "See [[Foo]] for the zzzbacklinkterm details.",
        "doc-b",
    )
    .await;

    let (status, body) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{doc_a}/backlinks"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "backlinks failed: {body}");
    let docs = body["documents"].as_array().expect("documents array");
    let ids: Vec<&str> = docs.iter().filter_map(|d| d["id"].as_str()).collect();
    assert!(
        ids.contains(&doc_b.as_str()),
        "doc B must backlink doc A, got {ids:?}"
    );
    assert!(
        !ids.contains(&doc_a.as_str()),
        "a document is never its own backlink"
    );
    for entry in docs {
        assert!(entry["title"].is_string(), "{entry}");
    }

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn backlinks_missing_id_404_and_entityless_document_empty() {
    let (state, db) = test_state().await;
    let router = test_router(state);

    let (status, body) = send(
        router.clone(),
        "GET",
        "/api/documents/document:does-not-exist/backlinks",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "E_DOCUMENT_NOT_FOUND");

    // No links, tags or headings → no entities → empty backlinks, 200.
    let doc = upload(
        router.clone(),
        "Plain body without any extractable entities at all.",
        "plain",
    )
    .await;
    let (status, body) = send(
        router.clone(),
        "GET",
        &format!("/api/documents/{doc}/backlinks"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["documents"], json!([]));

    let _ = std::fs::remove_dir_all(db);
}
