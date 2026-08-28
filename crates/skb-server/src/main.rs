//! skb-server binary: loads config, opens the knowledge base and serves the
//! HTTP API. Tracing goes to stderr; stdout carries only the machine-parsed
//! `SKB_SERVER_PORT=<n>` line when port 0 selects an ephemeral port.

use clap::Parser;
use skb_core::config::Config;
use skb_core::error::{ErrorCode, SkbError};
use skb_core::KnowledgeBase;
use skb_server::{build_router, AppState, ServerConfig};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "skb-server",
    version,
    about = "Surreal Knowledge Base HTTP API server"
)]
struct Cli {
    /// Listen port (0 = pick an ephemeral port and print `SKB_SERVER_PORT=<n>`)
    #[arg(long)]
    port: Option<u16>,

    /// Listen host
    #[arg(long)]
    host: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "skb_server=info,warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(e.code.exit_code());
    }
}

async fn run(cli: Cli) -> Result<(), SkbError> {
    let server_cfg = ServerConfig::load(cli.port, cli.host)?;
    let core_cfg = core_config()?;
    let kb = Arc::new(KnowledgeBase::open(core_cfg).await?);
    let state = AppState { kb, server_cfg };

    let host = state.server_cfg.host.clone();
    let port = state.server_cfg.port;
    let addr = format!("{host}:{port}");
    // Bind BEFORE printing so the announced port is already accepting.
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("bind {addr}: {e}")))?;
    let bound_port = listener
        .local_addr()
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("local_addr: {e}")))?
        .port();
    if port == 0 {
        println!("SKB_SERVER_PORT={bound_port}");
    }
    tracing::info!(host = %host, port = bound_port, "skb-server listening");

    let router = build_router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("serve: {e}")))?;
    tracing::info!("skb-server stopped");
    Ok(())
}

/// Load the core config (storage/embedding/...), mapping anyhow's context
/// chain onto the matching `ErrorCode` (defaults to `E_CONFIG`).
fn core_config() -> Result<Config, SkbError> {
    Config::load().map_err(|e| {
        SkbError::new(
            ErrorCode::from_std(e.as_ref()).unwrap_or(ErrorCode::Config),
            format!("failed to load config: {e}"),
        )
    })
}

/// Resolve on ctrl-c (all platforms) or SIGTERM (unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
