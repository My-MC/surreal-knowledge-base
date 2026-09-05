//! Chat SSE acceptance tests (plan todo 6): event ordering against a mocked
//! OpenAI upstream (wiremock), and in-band error events for LLM failures.
//!
//! SurrealKv holds a cross-process exclusive lock (SPIKE.md): every test uses
//! a UNIQUE store path under ./target and the suite runs with
//! `--test-threads=1`. The LLM client reads its env per request, so each test
//! points `SKB_LLM_BASE_URL` at its own mock before calling.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{test_router, test_state, upload};
use serde_json::Value;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// POST /api/chat/stream through the in-process router; returns the status,
/// content-type and the FULL SSE body (oneshot collects until the stream
/// ends, i.e. after `done` or the terminal `error` event).
async fn post_chat(router: axum::Router, message: &str) -> (StatusCode, String, String) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/chat/stream")
        .header("content-type", "application/json")
        .body(Body::from(format!(r#"{{"message":"{message}"}}"#)))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .expect("content-type header")
        .to_str()
        .expect("content-type is ASCII")
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        content_type,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

/// Parse an SSE body into (event name, data) pairs in wire order. Keep-alive
/// comment lines (`:`) are ignored; multi-line data is joined with `\n`.
fn parse_sse(body: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let mut name = String::new();
    let mut data = String::new();
    for line in body.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        } else if line.is_empty() && (!name.is_empty() || !data.is_empty()) {
            events.push((std::mem::take(&mut name), std::mem::take(&mut data)));
        }
    }
    events
}

fn event_names(events: &[(String, String)]) -> Vec<&str> {
    events.iter().map(|(name, _)| name.as_str()).collect()
}

fn data_of(events: &[(String, String)], name: &str) -> Value {
    let (_, data) = events
        .iter()
        .find(|(event, _)| event == name)
        .unwrap_or_else(|| panic!("no {name} event in {events:?}"));
    serde_json::from_str(data).unwrap_or_else(|e| panic!("{name} data is not JSON: {e}: {data}"))
}

/// Mount a mock OpenAI upstream answering `POST /chat/completions` (the path
/// the client derives from `SKB_LLM_BASE_URL` = `server.uri()`, which carries
/// no `/v1` suffix) with the given status/body, and point the env at it.
async fn mount_llm(status: u16, sse_body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;
    std::env::set_var("SKB_LLM_BASE_URL", server.uri());
    std::env::remove_var("SKB_LLM_API_KEY");
    server
}

#[tokio::test]
async fn chat_stream_emits_citation_tokens_done_in_order() {
    let mock_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let _server = mount_llm(200, mock_body).await;

    let (state, db) = test_state().await;
    let router = test_router(state);
    upload(
        router.clone(),
        "Alpha engine notes with unique zzzchatterm content [[Alpha]].",
        "chat-doc",
    )
    .await;

    let (status, content_type, body) = post_chat(router, "zzzchatterm").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("text/event-stream"),
        "content-type must be SSE: {content_type}"
    );

    let events = parse_sse(&body);
    let names = event_names(&events);
    assert_eq!(
        names.first(),
        Some(&"citation"),
        "citation must be first: {body}"
    );
    assert_eq!(names.last(), Some(&"done"), "done must be last: {body}");
    assert!(!names.contains(&"error"), "no error expected: {body}");

    let citation = data_of(&events, "citation");
    let hits = citation["hits"].as_array().expect("citation hits array");
    assert!(!hits.is_empty(), "expected search hits in citation: {body}");
    assert!(
        hits[0]["document_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("document:")),
        "citation ids must be the full record id: {citation}"
    );
    assert!(hits[0]["title"].is_string(), "{citation}");
    assert!(hits[0]["score"].is_number(), "{citation}");

    let tokens: String = events
        .iter()
        .filter(|(name, _)| name == "token")
        .map(|(_, data)| {
            serde_json::from_str::<Value>(data).expect("token data is JSON")["text"]
                .as_str()
                .expect("token text")
                .to_string()
        })
        .collect();
    assert_eq!(
        tokens, "Hello, world",
        "token fragments must concatenate in order: {body}"
    );

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn chat_stream_llm_500_yields_terminal_in_band_error() {
    let _server = mount_llm(500, "upstream exploded").await;

    let (state, db) = test_state().await;
    let router = test_router(state);
    upload(
        router.clone(),
        "Beta notes with unique zzzerrterm content.",
        "err-doc",
    )
    .await;

    let (status, _content_type, body) = post_chat(router, "zzzerrterm").await;
    assert_eq!(status, StatusCode::OK, "SSE errors are in-band: {body}");

    let events = parse_sse(&body);
    let names = event_names(&events);
    assert!(
        names.contains(&"citation"),
        "search succeeded → citation first: {body}"
    );
    assert!(
        !names.contains(&"token"),
        "no tokens may follow a 500: {body}"
    );
    assert!(!names.contains(&"done"), "error replaces done: {body}");
    assert_eq!(
        names.last(),
        Some(&"error"),
        "error must be terminal: {body}"
    );

    let error = data_of(&events, "error");
    assert_eq!(error["code"], "E_LLM_STATUS", "{body}");
    assert!(error["message"].is_string(), "{error}");

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn chat_stream_llm_unreachable_yields_terminal_in_band_error() {
    // A synchronously-closed port: wiremock's Drop shuts down asynchronously,
    // so a dropped MockServer can still answer (404) for a short window. A
    // dropped std listener closes its fd inside drop() → guaranteed refusal.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    std::env::set_var("SKB_LLM_BASE_URL", format!("http://127.0.0.1:{port}"));
    std::env::remove_var("SKB_LLM_API_KEY");

    let (state, db) = test_state().await;
    let router = test_router(state);
    upload(
        router.clone(),
        "Gamma notes with unique zzzdeadterm content.",
        "dead-doc",
    )
    .await;

    let (status, _content_type, body) = post_chat(router, "zzzdeadterm").await;
    assert_eq!(status, StatusCode::OK, "SSE errors are in-band: {body}");

    let events = parse_sse(&body);
    let names = event_names(&events);
    assert!(names.contains(&"citation"), "{body}");
    assert!(!names.contains(&"token"), "{body}");
    assert!(!names.contains(&"done"), "{body}");
    assert_eq!(
        names.last(),
        Some(&"error"),
        "error must be terminal: {body}"
    );

    let error = data_of(&events, "error");
    assert_eq!(error["code"], "E_LLM_CONNECTION", "{body}");

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn chat_stream_rejects_oversized_sse_frames() {
    let oversized = format!("data: x{}\n\n", "a".repeat(200_000));
    let _server = mount_llm(200, &oversized).await;

    let (state, db) = test_state().await;
    let router = test_router(state);
    upload(
        router.clone(),
        "Delta notes with unique zzzframeterm content.",
        "frame-doc",
    )
    .await;

    let (_status, _content_type, body) = post_chat(router, "zzzframeterm").await;
    let events = parse_sse(&body);
    assert_eq!(
        event_names(&events).last(),
        Some(&"error"),
        "oversized frame must be terminal: {body}"
    );
    let error = data_of(&events, "error");
    assert_eq!(error["code"], "E_LLM_PROTOCOL", "{body}");

    let _ = std::fs::remove_dir_all(db);
}

#[tokio::test]
async fn chat_stream_llm_error_body_is_size_capped() {
    let huge = format!("E{}", "x".repeat(1_000_000));
    let _server = mount_llm(500, &huge).await;

    let (state, db) = test_state().await;
    let router = test_router(state);
    upload(
        router.clone(),
        "Epsilon notes with unique zzzerrbodyterm content.",
        "errbody-doc",
    )
    .await;

    let (_status, _content_type, body) = post_chat(router, "zzzerrbodyterm").await;
    let events = parse_sse(&body);
    let error = data_of(&events, "error");
    assert_eq!(error["code"], "E_LLM_STATUS", "{body}");
    let message = error["message"].as_str().expect("error message");
    assert!(
        message.len() < 64 * 1024,
        "error body must be capped, got {} bytes",
        message.len()
    );

    let _ = std::fs::remove_dir_all(db);
}

/// Given: SKB_LLM_API_KEY set and SKB_LLM_BASE_URL on plain http.
/// When:  resolving the LLM client.
/// Then:  E_LLM_CONFIG — bearer tokens and prompts never travel cleartext.
#[tokio::test]
async fn llm_api_key_rejects_http_base_url() {
    std::env::set_var("SKB_LLM_API_KEY", "secret-token");
    std::env::set_var("SKB_LLM_BASE_URL", "http://127.0.0.1:1/v1");
    let err = match skb_server::llm::LlmClient::from_env() {
        Err(err) => err,
        Ok(_) => panic!("http + api key must fail"),
    };
    std::env::remove_var("SKB_LLM_API_KEY");
    assert_eq!(err.code(), "E_LLM_CONFIG");
}
