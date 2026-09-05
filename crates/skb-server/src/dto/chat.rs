//! Server-owned chat DTOs (plan todo 6).

use serde::Deserialize;
use utoipa::ToSchema;

/// Body of `POST /api/chat/stream`. SPECIFICATION.md has no SSE request
/// section yet (todo 9 adds it), so the shape is `{"message": string}`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ChatStreamRequest {
    pub message: String,
}
