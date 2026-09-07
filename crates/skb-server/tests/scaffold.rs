//! Scaffold acceptance tests (plan todo 2): health, openapi.json, unknown
//! route, and the port-0 `SKB_SERVER_PORT=` stdout protocol.
//!
//! SurrealKv holds a cross-process exclusive lock (SPIKE.md): every test uses
//! a UNIQUE store path under ./target and the suite runs with
//! `--test-threads=1`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skb_core::config::Config;
use skb_core::KnowledgeBase;
use skb_server::{build_router, AppState, ServerConfig};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

static TEST_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn unique_id() -> usize {
    TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// Mock-embedder state with a unique store path under ./target (never /tmp).
/// Returns the store path for cleanup.
async fn test_state() -> (AppState, PathBuf) {
    let n = unique_id();
    let mut core = Config::default();
    core.embedding.onnx_path = "mock".into();
    core.embedding.dimension = 8;
    core.storage.path = PathBuf::from(format!("./target/skb-server-test-{n}"));
    let db_path = core.storage.path.clone();
    let kb = KnowledgeBase::open(core)
        .await
        .expect("open mock knowledge base");
    let state = AppState {
        kb: Arc::new(kb),
        server_cfg: ServerConfig::default(),
    };
    (state, db_path)
}

#[tokio::test]
async fn health_returns_200_ok() {
    let (state, db) = test_state().await;
    let response = build_router(state)
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "ok");
    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn openapi_json_lists_health_path() {
    let (state, db) = test_state().await;
    let response = build_router(state)
        .oneshot(
            Request::get("/api/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("/api/health"),
        "openapi.json must list /api/health, got: {body}"
    );
    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let (state, db) = test_state().await;
    let response = build_router(state)
        .oneshot(
            Request::get("/api/definitely-not-a-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(db);
}

/// Spawn the real binary with `--port 0`: the FIRST stdout line must be
/// `SKB_SERVER_PORT=<n>` (tracing goes to stderr), and the announced port
/// must serve `/api/health`.
#[tokio::test]
async fn server_with_port_zero_announces_port_and_serves_health() {
    let n = unique_id();
    let db = format!("./target/skb-server-test-spawn-{n}");

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_skb-server"))
        .args(["--port", "0"])
        .env("SKB_STORAGE_PATH", &db)
        .env("SKB_EMBEDDING_ONNX_PATH", "mock")
        .env("SKB_EMBEDDING_DIMENSION", "8")
        .env("SKB_EMBEDDING_TOKENIZER", "auto")
        .env("SKB_EMBEDDING_MODEL", "BAAI/bge-m3")
        .env("SKB_SERVER_HOST", "127.0.0.1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn skb-server binary");

    // Drain stderr so failures are diagnosable and the pipe never fills up.
    let stderr_task = tokio::spawn({
        let mut stderr = child.stderr.take().expect("piped stderr");
        async move {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).into_owned()
        }
    });

    let stdout = child.stdout.take().expect("piped stdout");
    let first_line = tokio::time::timeout(Duration::from_secs(120), async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stdout).lines();
        lines
            .next_line()
            .await
            .expect("stdout closed before any line (startup failure?)")
            .expect("read first stdout line")
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for the server to bind"));
    assert!(
        first_line.starts_with("SKB_SERVER_PORT="),
        "first stdout line must be the port protocol line, got: {first_line:?}"
    );
    let port: u16 = first_line["SKB_SERVER_PORT=".len()..]
        .parse()
        .expect("announced port must be a number");
    assert!(port > 0, "ephemeral port must be nonzero");

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to announced port");
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "health must answer 200, got: {response}"
    );
    assert!(
        response.contains("{\"status\":\"ok\"}"),
        "health body must be the ok JSON, got: {response}"
    );

    child.kill().await.ok();
    let stderr = stderr_task.await.unwrap();
    assert!(
        !stderr.contains("panic"),
        "server must not panic; stderr was: {stderr}"
    );
    let _ = std::fs::remove_dir_all(db);
}
