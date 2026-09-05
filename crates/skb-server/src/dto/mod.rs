//! Server-owned OpenAPI DTOs, one module per API area.

pub mod auth;
pub mod chat;
pub mod documents;
pub mod graph;
pub mod search;

pub use documents::ErrorResponse;
