//! Auth (plan todo 7): server-owned `user` table, register/login with
//! Argon2id hashing, JWT (HS256) session cookies, and the request extractor.
//!
//! The JWT secret is read from `SKB_SERVER_JWT_SECRET` at request time: unset
//! means every JWT-requiring path answers 503 `E_CONFIG` while startup and
//! public routes are unaffected.

use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderName, StatusCode};
use axum::Json;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use skb_core::error::{ErrorCode, SkbError};
use skb_core::KnowledgeBase;

use crate::api::AppState;
use crate::dto::auth::{AuthResponse, LoginRequest, RegisterRequest};
use crate::dto::ErrorResponse;
use crate::error::ApiError;

/// Server-owned DDL (schema/002_server.surql). Fixed SQL only —
/// `query_surql` cannot bind parameters, and this file contains no user
/// input. Idempotent: every statement is `IF NOT EXISTS`.
const SERVER_SCHEMA_SQL: &str = include_str!("../schema/002_server.surql");

const JWT_SECRET_ENV: &str = "SKB_SERVER_JWT_SECRET";
const TOKEN_TTL_SECS: usize = 24 * 60 * 60;
const SESSION_COOKIE_PREFIX: &str = "skb_session=";

const USER_BY_EMAIL_SQL: &str =
    "SELECT email, password_hash, role FROM user WHERE email = $email LIMIT 1";
const EMAIL_TAKEN_SQL: &str = "SELECT email FROM user WHERE email = $email LIMIT 1";
const CREATE_USER_SQL: &str = "CREATE user SET email = $email, password_hash = $hash, role = $role";

/// Apply the server-owned schema after `KnowledgeBase::open`. Called by the
/// binary at startup and by the integration-test harness.
pub async fn apply_server_schema(kb: &KnowledgeBase) -> Result<(), SkbError> {
    kb.query_surql(SERVER_SCHEMA_SQL).await.map(|_| ())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: the user's email.
    pub sub: String,
    pub role: String,
    pub exp: usize,
}

/// 503 `E_CONFIG` — the JWT secret is not configured.
fn secret_unconfigured() -> ApiError {
    ApiError::with_status(
        SkbError::new(
            ErrorCode::Config,
            "SKB_SERVER_JWT_SECRET is not set; authenticated endpoints are unavailable",
        ),
        StatusCode::SERVICE_UNAVAILABLE,
    )
}

/// 401 `E_VALIDATION` — generic on purpose (no user enumeration).
fn unauthorized(message: &'static str) -> ApiError {
    ApiError::with_status(
        SkbError::new(ErrorCode::Validation, message),
        StatusCode::UNAUTHORIZED,
    )
}

fn jwt_secret() -> Result<String, ApiError> {
    std::env::var(JWT_SECRET_ENV).map_err(|_| secret_unconfigured())
}

fn issue_token(email: &str, role: &str, secret: &str) -> Result<String, ApiError> {
    let claims = Claims {
        sub: email.to_string(),
        role: role.to_string(),
        exp: jsonwebtoken::get_current_timestamp() as usize + TOKEN_TTL_SECS,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ApiError::from(SkbError::new(ErrorCode::Db, format!("jwt encode: {e}"))))
}

/// Token from `Cookie: skb_session=...` (preferred) or
/// `Authorization: Bearer ...`.
fn request_token(parts: &Parts) -> Option<String> {
    if let Some(cookies) = parts
        .headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        for pair in cookies.split(';') {
            if let Some(token) = pair.trim().strip_prefix(SESSION_COOKIE_PREFIX) {
                return Some(token.to_string());
            }
        }
    }
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// Authenticated user resolved from the session JWT.
pub struct AuthUser {
    pub email: String,
    pub role: String,
}

impl AuthUser {
    /// Blog endpoints are author-only; readers get the same 401 shape as
    /// unauthenticated callers.
    pub fn require_author(self) -> Result<Self, ApiError> {
        if self.role == "author" {
            Ok(self)
        } else {
            Err(unauthorized("author role required"))
        }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let secret = jwt_secret()?;
        let token = request_token(parts).ok_or_else(|| unauthorized("missing session token"))?;
        let data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| unauthorized("invalid or expired session token"))?;
        Ok(Self {
            email: data.claims.sub,
            role: data.claims.role,
        })
    }
}

/// Never-rejecting [`AuthUser`] variant: handlers that only conditionally
/// require auth (blog uploads) receive the extraction result and decide.
pub struct OptionalAuth(pub Result<AuthUser, ApiError>);

impl FromRequestParts<AppState> for OptionalAuth {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(OptionalAuth(
            AuthUser::from_request_parts(parts, state).await,
        ))
    }
}

/// Register a user (Argon2id hash; duplicate email → 409).
#[utoipa::path(
    post,
    path = "/api/auth/register",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "User registered", body = AuthResponse),
        (status = 400, description = "Invalid registration", body = ErrorResponse),
        (status = 409, description = "Email already registered", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
    )
)]
pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<AuthResponse>), ApiError> {
    let email = req.email.trim().to_string();
    let role = req.role.as_deref().unwrap_or("reader").to_string();
    if email.is_empty() || req.password.is_empty() {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            "email and password must not be empty",
        )));
    }
    if role != "reader" && role != "author" {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            "role must be reader or author",
        )));
    }

    let mut r = state
        .kb
        .db()
        .db
        .query(EMAIL_TAKEN_SQL)
        .bind(("email", email.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("register lookup: {e}")))?;
    let taken: Vec<Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("register lookup take: {e}")))?;
    if !taken.is_empty() {
        return Err(ApiError::with_status(
            SkbError::new(ErrorCode::Validation, "email already registered"),
            StatusCode::CONFLICT,
        ));
    }

    // Argon2id v19 defaults; the random salt is generated internally.
    let hash = Argon2::default()
        .hash_password(req.password.as_bytes())
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("password hash: {e}")))?
        .to_string();
    let mut r = state
        .kb
        .db()
        .db
        .query(CREATE_USER_SQL)
        .bind(("email", email.clone()))
        .bind(("hash", hash))
        .bind(("role", role.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("register create: {e}")))?;
    let _created: Vec<Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("register create take: {e}")))?;
    Ok((StatusCode::CREATED, Json(AuthResponse { email, role })))
}

/// Login with email + password; success sets the `skb_session` JWT cookie
/// (HttpOnly, SameSite=Lax, Path=/) valid for 24 hours.
#[utoipa::path(
    post,
    path = "/api/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Session cookie set", body = AuthResponse),
        (status = 401, description = "Invalid credentials", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
        (status = 503, description = "JWT secret not configured", body = ErrorResponse),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<(StatusCode, [(HeaderName, String); 1], Json<AuthResponse>), ApiError> {
    let secret = jwt_secret()?;
    let email = req.email.trim().to_string();
    let invalid = || unauthorized("invalid email or password");

    let mut r = state
        .kb
        .db()
        .db
        .query(USER_BY_EMAIL_SQL)
        .bind(("email", email.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("login lookup: {e}")))?;
    let rows: Vec<Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("login lookup take: {e}")))?;
    let Some(row) = rows.into_iter().next() else {
        return Err(invalid());
    };
    let stored = row
        .get("password_hash")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let role = row
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("reader")
        .to_string();
    let parsed = PasswordHash::new(stored).map_err(|_| invalid())?;
    Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed)
        .map_err(|_| invalid())?;

    let token = issue_token(&email, &role, &secret)?;
    let cookie = format!("{SESSION_COOKIE_PREFIX}{token}; HttpOnly; SameSite=Lax; Path=/");
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(AuthResponse { email, role }),
    ))
}
