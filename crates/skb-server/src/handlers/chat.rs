//! Chat SSE handler (plan todo 6): `POST /api/chat/stream`.
//!
//! Pipeline: kb.search (top_k 6, graph_expand from `SKB_CHAT_EXPAND_DEPTH`)
//! → `event: citation` with all hits → prompt built from hit chunks under a
//! char-based token budget (`SKB_CHAT_TOKEN_BUDGET`) → LLM stream forwarded
//! as `event: token` → `event: done`. Any failure emits `event: error` and
//! ends the stream normally — HTTP status stays 200 (SSE errors are in-band).

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, KeepAliveStream, Sse};
use axum::Json;
use serde_json::json;
use skb_core::search::SearchRequest as CoreSearchRequest;
use skb_core::search::MAX_GRAPH_EXPAND;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::AppState;
use crate::dto::chat::ChatStreamRequest;
use crate::dto::search::SearchHit;
use crate::llm::LlmClient;

const DEFAULT_EXPAND_DEPTH: usize = 2;
const DEFAULT_TOKEN_BUDGET: usize = 4000;
const CHAT_TOP_K: usize = 6;

/// SSE event stream: items are always `Ok` — failures travel as `error`
/// events, never as stream errors. `KeepAliveStream` is the wrapper added by
/// `.keep_alive(KeepAlive::default())`.
pub type ChatEventStream = KeepAliveStream<ReceiverStream<Result<Event, std::convert::Infallible>>>;

/// Streaming chat over the knowledge base. The response is
/// `text/event-stream`: `citation` (all search hits) → `token`×N → `done`,
/// or `error` (in-band, terminal) with HTTP 200 throughout.
#[utoipa::path(
    post,
    path = "/api/chat/stream",
    request_body = ChatStreamRequest,
    responses(
        (status = 200, description = "SSE stream: citation → token* → done; pipeline failures are terminal in-band error events (HTTP status stays 200)", content_type = "text/event-stream"),
    )
)]
pub async fn chat_stream(
    State(state): State<AppState>,
    Json(req): Json<ChatStreamRequest>,
) -> Sse<ChatEventStream> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(pipeline(state, req.message, tx));
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn pipeline(
    state: AppState,
    message: String,
    tx: tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
) {
    let search_req = CoreSearchRequest {
        query: message.clone(),
        mode: None,
        top_k: Some(CHAT_TOP_K),
        graph_expand: Some(expand_depth()),
        filter: None,
    };
    let hits: Vec<SearchHit> = match state.kb.search(search_req).await {
        Ok(resp) => resp.hits.into_iter().map(Into::into).collect(),
        Err(e) => {
            send_error(&tx, e.code.code_str(), &e.message).await;
            return;
        }
    };

    send_json(&tx, "citation", &json!({ "hits": &hits })).await;

    let prompt = build_prompt(&message, &hits, token_budget());
    let client = match LlmClient::from_env() {
        Ok(client) => client,
        Err(e) => {
            send_error(&tx, e.code(), &e.to_string()).await;
            return;
        }
    };
    let mut stream = match client.stream_chat(&prompt).await {
        Ok(stream) => stream,
        Err(e) => {
            send_error(&tx, e.code(), &e.to_string()).await;
            return;
        }
    };
    loop {
        match stream.next_fragment().await {
            Ok(Some(text)) => send_json(&tx, "token", &json!({ "text": text })).await,
            Ok(None) => break,
            Err(e) => {
                send_error(&tx, e.code(), &e.to_string()).await;
                return;
            }
        }
    }
    send_json(&tx, "done", &json!({})).await;
}

/// `SKB_CHAT_EXPAND_DEPTH` (default 2), capped at core's `MAX_GRAPH_EXPAND`;
/// unparseable values fall back to the default.
fn expand_depth() -> usize {
    std::env::var("SKB_CHAT_EXPAND_DEPTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_EXPAND_DEPTH)
        .min(MAX_GRAPH_EXPAND)
}

/// `SKB_CHAT_TOKEN_BUDGET` (default 4000); unparseable values fall back to
/// the default.
fn token_budget() -> usize {
    std::env::var("SKB_CHAT_TOKEN_BUDGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_TOKEN_BUDGET)
}

/// Build the LLM prompt from hit chunks under a TOTAL char budget. Chars
/// approximate tokens (~4 chars/token for English) — documented MVP
/// approximation; a real tokenizer is deliberately not pulled in here.
fn build_prompt(message: &str, hits: &[SearchHit], budget: usize) -> String {
    let mut excerpts = String::new();
    let mut used = 0usize;
    for (i, hit) in hits.iter().enumerate() {
        let title = hit.title.as_deref().unwrap_or("(untitled)");
        let header = format!("Excerpt {} — {title} ({}):\n", i + 1, hit.document_id);
        if used + header.len() >= budget {
            break;
        }
        let remaining = budget - used - header.len();
        let content = truncate_at_char_boundary(&hit.content, remaining);
        used += header.len() + content.len() + 2; // + blank-line separator
        excerpts.push_str(&header);
        excerpts.push_str(content);
        excerpts.push_str("\n\n");
    }
    format!(
        "You are a knowledge-base assistant. Answer the question using the \
         document excerpts below when they are relevant.\n\n{excerpts}Question: {message}"
    )
}

fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

async fn send_json(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    name: &str,
    value: &serde_json::Value,
) {
    let event = Event::default().event(name).data(value.to_string());
    // A send error means the client disconnected; stop feeding quietly.
    let _ = tx.send(Ok(event)).await;
}

async fn send_error(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    code: &str,
    message: &str,
) {
    let event = Event::default()
        .event("error")
        .data(json!({ "code": code, "message": message }).to_string());
    let _ = tx.send(Ok(event)).await;
}
