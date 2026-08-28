//! Shared harness for skb-server integration tests (in-process router on a
//! mock-embedder KnowledgeBase). SurrealKv holds a cross-process exclusive
//! lock (SPIKE.md): every store path is unique per process AND per test, and
//! suites run with `--test-threads=1`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use skb_core::config::Config;
use skb_core::KnowledgeBase;
use skb_server::{build_router, AppState, ServerConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

static TEST_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn unique_id() -> usize {
    TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// Mock-embedder core Config with a store path unique per process AND per
/// test: the pid qualifier keeps a re-run off the dirty store a previously
/// failed run left behind (a reused store would skip uploads with identical
/// content). Never /tmp. Returns (config, store path for cleanup).
pub fn test_config() -> (Config, PathBuf) {
    let n = unique_id();
    let mut core = Config::default();
    core.embedding.onnx_path = "mock".into();
    core.embedding.dimension = 8;
    core.storage.path = PathBuf::from(format!(
        "./target/skb-server-test-docs-{}-{n}",
        std::process::id()
    ));
    let db_path = core.storage.path.clone();
    (core, db_path)
}

/// Mock-embedder state with a unique store path (see `test_config`).
/// Returns the store path for cleanup.
pub async fn test_state() -> (AppState, PathBuf) {
    let (core, db_path) = test_config();
    let kb = KnowledgeBase::open(core)
        .await
        .expect("open mock knowledge base");
    let state = AppState {
        kb: Arc::new(kb),
        server_cfg: ServerConfig::default(),
    };
    (state, db_path)
}

/// Start the full router on an ephemeral 127.0.0.1 port in a background
/// task (port 0 = the OS picks the port, mirroring the binary's
/// `--port 0` protocol). Returns (base URL, store path for cleanup, task
/// handle — abort it to tear the server down).
// Harness infrastructure for later todos (T6 SSE tests drive a base URL);
// current test binaries do not consume it yet, and each test file compiles
// this module separately, which trips dead_code per binary.
#[allow(dead_code)]
pub async fn spawn_server() -> (String, PathBuf, tokio::task::JoinHandle<()>) {
    let (state, db_path) = test_state().await;
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let port = listener.local_addr().expect("local_addr").port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("in-process server failed");
    });
    (format!("http://127.0.0.1:{port}"), db_path, handle)
}

/// Build the full application router for a test state.
pub fn test_router(state: AppState) -> axum::Router {
    build_router(state)
}

/// Drive one request through the in-process router; returns status + parsed
/// JSON body (`Null` for empty bodies such as 204).
pub async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

/// POST a content/title document and assert the 201 + id contract.
pub async fn upload(router: axum::Router, content: &str, title: &str) -> String {
    let (status, body) = send(
        router,
        "POST",
        "/api/documents",
        Some(serde_json::json!({"content": content, "title": title})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "upload failed: {body}");
    body["document_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no document_id in 201 body: {body}"))
        .into()
}
