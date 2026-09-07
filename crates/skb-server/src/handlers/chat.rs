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

/// Fixed prompt scaffolding (module level so [`token_budget`] can enforce
/// the budget floor it implies).
const INSTRUCTION: &str = "You are a knowledge-base assistant. Answer the question using the document excerpts below when they are relevant.\n\n";
const QUESTION_LABEL: &str = "Question: ";

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
    // Every long wait races the client disconnect channel: once `tx.closed()`
    // wins, the pipeline (and its upstream LLM connection, via LlmStream
    // drop) must stop instead of running to completion unobserved.
    let search_req = CoreSearchRequest {
        query: message.clone(),
        mode: None,
        top_k: Some(CHAT_TOP_K),
        graph_expand: Some(expand_depth()),
        filter: None,
    };
    let search = tokio::select! {
        _ = tx.closed() => return,
        result = state.kb.search(search_req) => result,
    };
    let hits: Vec<SearchHit> = match search {
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
    let connect = tokio::select! {
        _ = tx.closed() => return,
        result = client.stream_chat(&prompt) => result,
    };
    let mut stream = match connect {
        Ok(stream) => stream,
        Err(e) => {
            send_error(&tx, e.code(), &e.to_string()).await;
            return;
        }
    };
    loop {
        let fragment = tokio::select! {
            _ = tx.closed() => return,
            fragment = stream.next_fragment() => fragment,
        };
        match fragment {
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
/// the default. Values below the fixed-string floor are lifted to it — the
/// scaffolding alone occupies that much, so a smaller configuration would
/// otherwise make [`build_prompt`]'s output exceed the configured cap.
fn token_budget() -> usize {
    std::env::var("SKB_CHAT_TOKEN_BUDGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.max(INSTRUCTION.len() + QUESTION_LABEL.len()))
        .unwrap_or(DEFAULT_TOKEN_BUDGET)
}

/// Build the LLM prompt under a TOTAL char budget that covers the fixed
/// instruction, the question, and the excerpts — including each excerpt's
/// blank-line separator, so the output never exceeds `budget` (precondition:
/// `budget` ≥ the fixed-string floor, which [`token_budget`] enforces).
/// Chars approximate tokens (~4 chars/token for English) — documented MVP
/// approximation; a real tokenizer is deliberately not pulled in here. The
/// question keeps its priority: the excerpts share whatever budget remains
/// after the (possibly truncated) question.
fn build_prompt(message: &str, hits: &[SearchHit], budget: usize) -> String {
    let reserved = INSTRUCTION.len() + QUESTION_LABEL.len();
    let message_budget = budget.saturating_sub(reserved);
    let message = truncate_at_char_boundary(message, message_budget);

    let excerpt_budget = budget
        .saturating_sub(reserved)
        .saturating_sub(message.len());
    let mut excerpts = String::new();
    let mut used = 0usize;
    for (i, hit) in hits.iter().enumerate() {
        let title = hit.title.as_deref().unwrap_or("(untitled)");
        let header = format!("Excerpt {} — {title} ({}):\n", i + 1, hit.document_id);
        // The blank-line separator after this excerpt must fit too: content
        // capped at the raw remainder alone would push the total 2 bytes over.
        if used + header.len() + 2 > excerpt_budget {
            break;
        }
        let remaining = excerpt_budget - used - header.len() - 2;
        let content = truncate_at_char_boundary(&hit.content, remaining);
        used += header.len() + content.len() + 2;
        excerpts.push_str(&header);
        excerpts.push_str(content);
        excerpts.push_str("\n\n");
    }
    format!("{INSTRUCTION}{excerpts}{QUESTION_LABEL}{message}")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Restores the previous `SKB_CHAT_TOKEN_BUDGET` value on drop (the
    /// tests/auth_blog.rs EnvGuard pattern; the workspace runs with
    /// --test-threads=1).
    struct EnvGuard(Option<String>);

    impl EnvGuard {
        fn set(value: &str) -> Self {
            let old = std::env::var("SKB_CHAT_TOKEN_BUDGET").ok();
            std::env::set_var("SKB_CHAT_TOKEN_BUDGET", value);
            Self(old)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("SKB_CHAT_TOKEN_BUDGET", value),
                None => std::env::remove_var("SKB_CHAT_TOKEN_BUDGET"),
            }
        }
    }

    fn hit(document_id: &str, title: &str, content: &str) -> SearchHit {
        SearchHit {
            document_id: document_id.to_string(),
            chunk_idx: 0,
            content: content.to_string(),
            score: 1.0,
            title: Some(title.to_string()),
            source: None,
            highlights: None,
            matched_entities: None,
        }
    }

    /// Given: SKB_CHAT_TOKEN_BUDGET below the fixed-string floor.
    /// When:  resolving the token budget.
    /// Then:  the floor wins — the scaffolding alone occupies it, so a
    ///        smaller configuration could not be honored anyway.
    #[test]
    fn tiny_budget_is_lifted_to_the_fixed_string_floor() {
        let _env = EnvGuard::set("10");
        assert_eq!(token_budget(), INSTRUCTION.len() + QUESTION_LABEL.len());
    }

    /// Given: several excerpts whose content far exceeds any budget.
    /// When:  building prompts across budget sizes (floor, small surplus,
    ///        typical).
    /// Then:  every prompt is within its total budget — fixed strings,
    ///        message, headers, excerpt content, and separators included.
    #[test]
    fn prompt_never_exceeds_the_total_budget() {
        let hits: Vec<SearchHit> = (0..6)
            .map(|i| {
                hit(
                    &format!("document:d{i}"),
                    &format!("Title {i}"),
                    &"x".repeat(500),
                )
            })
            .collect();
        let floor = INSTRUCTION.len() + QUESTION_LABEL.len();
        for budget in [floor, floor + 40, 800, DEFAULT_TOKEN_BUDGET] {
            let prompt = build_prompt("total budget question", &hits, budget);
            assert!(
                prompt.len() <= budget,
                "budget {budget}: prompt is {} bytes",
                prompt.len()
            );
        }
    }

    /// Given: one excerpt whose content exactly fills the space left after
    ///        its header.
    /// When:  building the prompt.
    /// Then:  the blank-line separator is part of the cap — the total stays
    ///        at the budget instead of running 2 bytes over.
    #[test]
    fn excerpt_filling_the_remaining_budget_leaves_room_for_the_separator() {
        let budget = INSTRUCTION.len() + QUESTION_LABEL.len() + 100;
        let header_len = "Excerpt 1 — t (document:t1):\n".len();
        let content = "a".repeat(100 - header_len);
        let prompt = build_prompt("", &[hit("document:t1", "t", &content)], budget);
        assert!(
            prompt.len() <= budget,
            "exact-fill case: prompt is {} bytes over budget {budget}",
            prompt.len()
        );
    }
}
