//! skb-server: HTTP API layer over [`skb_core::KnowledgeBase`].
//!
//! The binary (`src/main.rs`) is a thin wrapper; config resolution, the
//! router and error mapping live here so integration tests can reuse
//! [`build_router`] / [`AppState`] in-process.

pub mod api;
pub mod config;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod llm;

pub use api::{build_router, ApiDoc, AppState};
pub use config::ServerConfig;
pub use error::ApiError;
