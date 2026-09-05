//! Server-owned auth DTOs (plan todo 7).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Body of `POST /api/auth/register`. The role is decided by the server
/// (`SKB_SERVER_AUTHOR_EMAILS` allowlist); clients cannot request one.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
}

/// Body of `POST /api/auth/login`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Body of successful register/login responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    pub email: String,
    pub role: String,
}
