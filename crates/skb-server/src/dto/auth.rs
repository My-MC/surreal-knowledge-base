//! Server-owned auth DTOs (plan todo 7).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Body of `POST /api/auth/register`. The role is decided by the server:
/// registration is public and always mints `reader` unless the request
/// presents the invite token configured for the email
/// (`SKB_SERVER_AUTHOR_INVITES`); clients cannot request a role directly.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    /// Author invite token for this email. Required only when the operator
    /// listed the email in `SKB_SERVER_AUTHOR_INVITES`; omit it (or a wrong
    /// token) registers a plain reader.
    #[serde(default)]
    pub invite: Option<String>,
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
