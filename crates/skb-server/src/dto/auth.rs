//! Server-owned auth DTOs (plan todo 7).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Body of `POST /api/auth/register`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    /// `reader` (default) or `author`.
    pub role: Option<String>,
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
