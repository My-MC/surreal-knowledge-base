//! Mock OpenAI-compatible LLM server (plan todo 6): answers
//! `POST /v1/chat/completions` with a fixed SSE stream, for E2E runs that
//! point `SKB_LLM_BASE_URL` at it. Tracing goes to stderr; with `--port 0`
//! stdout carries exactly one machine-parsed line `MOCK_LLM_PORT=<n>` (same
//! protocol as skb-server's `SKB_SERVER_PORT`).

use axum::response::sse::{Event, Sse};
use axum::routing::post;
use axum::Router;
use clap::Parser;
use serde_json::json;
use std::convert::Infallible;
use tokio_stream::Stream;

#[derive(Parser)]
#[command(
    name = "mock_llm",
    about = "Mock OpenAI-compatible streaming LLM server"
)]
struct Cli {
    /// Listen port (0 = pick an ephemeral port and print `MOCK_LLM_PORT=<n>`)
    #[arg(long, default_value_t = 0)]
    port: u16,
}

const RESPONSE_FRAGMENTS: &[&str] = &[
    "Based on the knowledge base excerpts, ",
    "this is a mock answer streamed ",
    "from mock_llm for end-to-end testing.",
];

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "mock_llm=info,warn".into()))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let app = Router::new().route("/v1/chat/completions", post(chat_completions));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", cli.port))
        .await
        .expect("bind mock_llm port");
    let bound_port = listener.local_addr().expect("local_addr").port();
    if cli.port == 0 {
        println!("MOCK_LLM_PORT={bound_port}");
    }
    tracing::info!(port = bound_port, "mock_llm listening");

    axum::serve(listener, app)
        .await
        .expect("mock_llm server failed");
}

async fn chat_completions() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let deltas = RESPONSE_FRAGMENTS.iter().map(|fragment| {
        Ok(Event::default()
            .data(json!({"choices": [{"delta": {"content": fragment}}]}).to_string()))
    });
    let done = std::iter::once(Ok(Event::default().data("[DONE]")));
    Sse::new(tokio_stream::iter(deltas.chain(done)))
}
