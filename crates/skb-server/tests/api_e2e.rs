//! API E2E (plan todo 5): spawns the REAL built `skb-server` binary via
//! `env!("CARGO_BIN_EXE_skb-server")` (never `cargo run` inside tests —
//! target-lock deadlock risk), waits for the `SKB_SERVER_PORT=<n>` stdout
//! line (KB open takes ~8s cold, so the timeout is generous), then drives
//! REAL HTTP over TCP: health 200 → upload 201 → search returns the
//! uploaded document's hit.
//!
//! SurrealKv holds a cross-process exclusive lock (SPIKE.md): the store
//! path is unique per run under ./target and the suite runs with
//! `--test-threads=1`.

use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

static E2E_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn e2e_db_path() -> String {
    let n = E2E_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("./target/skb-server-test-e2e-{}-{n}", std::process::id())
}

/// One raw HTTP/1.1 request with `Connection: close`; returns
/// (status code, body string).
async fn http_request(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("connect to 127.0.0.1:{port}: {e}"));
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status: u16 = response
        .split_whitespace()
        .nth(1)
        .expect("HTTP status line present")
        .parse()
        .expect("status code is numeric");
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_string();
    (status, body)
}

#[tokio::test]
async fn real_binary_serves_health_upload_search_over_tcp() {
    let db = e2e_db_path();

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

    // Drain stderr so tracing never blocks on a full pipe and failures stay
    // diagnosable.
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let stdout = child.stdout.take().expect("piped stdout");
    let first_line = tokio::time::timeout(Duration::from_secs(120), async move {
        BufReader::new(stdout).lines().next_line().await
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for the SKB_SERVER_PORT line"))
    .expect("stdout closed before any line (startup failure?)")
    .expect("read first stdout line");
    assert!(
        first_line.starts_with("SKB_SERVER_PORT="),
        "first stdout line must be the port protocol line, got: {first_line:?}"
    );
    let port: u16 = first_line["SKB_SERVER_PORT=".len()..]
        .parse()
        .expect("announced port must be a number");

    // health → 200 {"status":"ok"}
    let (status, body) = http_request(port, "GET", "/api/health", None).await;
    assert_eq!(status, 200, "health failed: {body}");
    let health: Value = serde_json::from_str(&body).expect("health body is JSON");
    assert_eq!(health["status"], "ok");

    // upload → 201 with a document:<key> id
    let content = "E2E marker doc about zzzapiterm and SurrealDB vector search.";
    let (status, body) = http_request(
        port,
        "POST",
        "/api/documents",
        Some(&serde_json::json!({"content": content, "title": "e2e-doc"}).to_string()),
    )
    .await;
    assert_eq!(status, 201, "upload failed: {body}");
    let uploaded: Value = serde_json::from_str(&body).expect("upload body is JSON");
    let doc_id = uploaded["document_id"]
        .as_str()
        .expect("document_id present")
        .to_string();
    assert!(
        doc_id.starts_with("document:"),
        "id must be the full record id: {doc_id}"
    );

    // search → the uploaded doc's hit, id matching the upload response
    let (status, body) = http_request(
        port,
        "POST",
        "/api/search",
        Some(
            &serde_json::json!({"query": "zzzapiterm", "mode": "hybrid", "top_k": 10}).to_string(),
        ),
    )
    .await;
    assert_eq!(status, 200, "search failed: {body}");
    let search: Value = serde_json::from_str(&body).expect("search body is JSON");
    let hits = search["hits"].as_array().expect("hits array");
    assert!(
        hits.iter()
            .any(|h| h["document_id"].as_str() == Some(doc_id.as_str())),
        "uploaded doc must hit with the prefixed id {doc_id}: {search}"
    );

    // Explicit kill + reap (kill_on_drop is only the backstop).
    child.kill().await.ok();
    let _ = child.wait().await;
    let stderr = stderr_task.await.unwrap();
    assert!(
        !stderr.contains("panic"),
        "server must not panic; stderr was: {stderr}"
    );

    let _ = std::fs::remove_dir_all(db);
}
