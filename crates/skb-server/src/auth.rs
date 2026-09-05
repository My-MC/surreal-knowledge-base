//! Auth (plan todo 7): server-owned `user` table, register/login with
//! Argon2id hashing, JWT (HS256) session cookies, logout with a jti
//! revocation list, and the request extractor.
//!
//! The JWT secret is read from `SKB_SERVER_JWT_SECRET` at request time: unset
//! or shorter than 32 characters means every JWT-requiring path answers 503
//! `E_CONFIG` while startup and public routes are unaffected.

use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
    Argon2,
};
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderName, StatusCode};
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
/// Minimum accepted JWT secret length (bytes). Short secrets fall to
/// brute-force and forged-token attacks (HS256), so a configured-but-weak
/// secret is treated the same as an unset one: 503 `E_CONFIG`.
const JWT_SECRET_MIN_LEN: usize = 32;
const TOKEN_TTL_SECS: usize = 24 * 60 * 60;
const SESSION_COOKIE_PREFIX: &str = "skb_session=";
/// Mirrors the login cookie's attributes so the browser drops the session
/// cookie instead of keeping a value-less one around.
const CLEARED_SESSION_COOKIE: &str = "skb_session=; Max-Age=0; HttpOnly; SameSite=Lax; Path=/";

const USER_BY_EMAIL_SQL: &str =
    "SELECT email, password_hash, role FROM user WHERE email = $email LIMIT 1";
const EMAIL_TAKEN_SQL: &str = "SELECT email FROM user WHERE email = $email LIMIT 1";
const CREATE_USER_SQL: &str = "CREATE user SET email = $email, password_hash = $hash, role = $role";
const REVOKE_SESSION_SQL: &str = "CREATE revoked_session SET jti = $jti, exp = $exp";
const SESSION_REVOKED_SQL: &str = "SELECT id FROM revoked_session WHERE jti = $jti LIMIT 1";
const PURGE_EXPIRED_SQL: &str = "DELETE FROM revoked_session WHERE exp < $now";

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
    /// Unique token id. Recorded in `revoked_session` by logout so a stolen
    /// cookie is rejected for the rest of its 24h lifetime (CWE-613).
    pub jti: String,
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

/// 409 `E_VALIDATION` — the email lost the unique-index race (or was
/// pre-emptively found in the store).
fn duplicate_email() -> ApiError {
    ApiError::with_status(
        SkbError::new(ErrorCode::Validation, "email already registered"),
        StatusCode::CONFLICT,
    )
}

/// The user-email unique index violation (CWE fix for concurrent
/// registrations losing the race) maps to the documented 409, not 500. The
/// violation can surface on the CREATE query itself or on result extraction.
fn register_create_error(context: &'static str, e: impl std::fmt::Display) -> ApiError {
    if e.to_string().contains("user_email_unique") {
        duplicate_email()
    } else {
        ApiError::from(SkbError::new(ErrorCode::Db, format!("{context}: {e}")))
    }
}

/// Author invites (`SKB_SERVER_AUTHOR_INVITES`): comma-separated
/// `email:token` pairs. Registration is public and always mints `reader`
/// unless the request presents the token configured for an exact email —
/// registration never verifies email ownership, so any coarser grant (client
/// roles, domain or suffix forms, or a bare allowlist) would let an
/// unverified caller self-grant `author` (CWE-269). Entries with an empty
/// token are ignored: an operator cannot mint a tokenless invite by
/// accident.
fn author_invites() -> Vec<(String, String)> {
    std::env::var("SKB_SERVER_AUTHOR_INVITES")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter_map(|entry| entry.split_once(':'))
                .filter(|(email, token)| !email.is_empty() && !token.is_empty())
                .map(|(email, token)| (email.to_string(), token.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Constant-time invite comparison: token bytes never reach a timing oracle
/// on the HTTP path (a length mismatch short-circuits first — only the
/// token's length leaks, which is not the secret).
fn invite_matches(provided: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    provided.len() == expected.len() && bool::from(provided.as_bytes().ct_eq(expected.as_bytes()))
}

fn jwt_secret() -> Result<String, ApiError> {
    let secret = std::env::var(JWT_SECRET_ENV).map_err(|_| secret_unconfigured())?;
    if secret.len() < JWT_SECRET_MIN_LEN {
        return Err(ApiError::with_status(
            SkbError::new(
                ErrorCode::Config,
                format!(
                    "{JWT_SECRET_ENV} must be at least {JWT_SECRET_MIN_LEN} characters to resist brute-forced HS256 tokens"
                ),
            ),
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    }
    Ok(secret)
}

fn issue_token(email: &str, role: &str, secret: &str) -> Result<String, ApiError> {
    let claims = Claims {
        sub: email.to_string(),
        role: role.to_string(),
        jti: uuid::Uuid::new_v4().to_string(),
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
    request_token_from_headers(&parts.headers)
}

fn request_token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        for pair in cookies.split(';') {
            if let Some(token) = pair.trim().strip_prefix(SESSION_COOKIE_PREFIX) {
                return Some(token.to_string());
            }
        }
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// True when the token's `jti` was recorded by an earlier logout, so the
/// remaining cookie lifetime is worthless. Expired rows are purged by the
/// logout handler; an expired token is rejected by the JWT `exp` check before
/// this lookup anyway.
async fn session_revoked(kb: &KnowledgeBase, jti: &str) -> Result<bool, ApiError> {
    let mut r = kb
        .db()
        .db
        .query(SESSION_REVOKED_SQL)
        .bind(("jti", jti.to_string()))
        .await
        .map_err(|e| ApiError::from(SkbError::new(ErrorCode::Db, format!("revoke lookup: {e}"))))?;
    let rows: Vec<Value> = r.take(0).map_err(|e| {
        ApiError::from(SkbError::new(
            ErrorCode::Db,
            format!("revoke lookup take: {e}"),
        ))
    })?;
    Ok(!rows.is_empty())
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
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let secret = jwt_secret()?;
        let token = request_token(parts).ok_or_else(|| unauthorized("missing session token"))?;
        let data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| unauthorized("invalid or expired session token"))?;
        if session_revoked(&state.kb, &data.claims.jti).await? {
            return Err(unauthorized("session revoked"));
        }
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

/// Register a user (Argon2id hash; duplicate email → 409). The role is
/// server-decided and public registration always mints `reader` unless the
/// request presents the invite token listed for the email in
/// `SKB_SERVER_AUTHOR_INVITES` — never privileges from unauthenticated
/// client input (CWE-269).
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
    if email.is_empty() || req.password.is_empty() {
        return Err(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            "email and password must not be empty",
        )));
    }
    let role = match author_invites().iter().find(|(listed, _)| listed == &email) {
        Some((_, expected)) if invite_matches(req.invite.as_deref().unwrap_or(""), expected) => {
            "author"
        }
        _ => "reader",
    };

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
        return Err(duplicate_email());
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
        .bind(("role", role))
        .await
        .map_err(|e| {
            // A concurrent registration can pass the pre-lookup and then lose
            // the unique-index race inside CREATE; surface that as 409
            // instead of the default 500.
            register_create_error("register create", e)
        })?;
    let _created: Vec<Value> = r
        .take(0)
        .map_err(|e| register_create_error("register create take", e))?;
    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            email,
            role: role.to_string(),
        }),
    ))
}

/// Login with email + password; success sets the `skb_session` JWT cookie
/// (Secure, HttpOnly, SameSite=Lax, Path=/) valid for 24 hours.
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
    let cookie = format!("{SESSION_COOKIE_PREFIX}{token}; Secure; HttpOnly; SameSite=Lax; Path=/");
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(AuthResponse { email, role }),
    ))
}

/// Logout: record the session's `jti` in the revocation list (the cookie's
/// remaining lifetime becomes worthless even if it was stolen) and expire the
/// cookie. Re-logging-out the same token is a no-op (non-unique revocation
/// rows).
#[utoipa::path(
    post,
    path = "/api/auth/logout",
    responses(
        (status = 204, description = "Session revoked and cookie cleared"),
        (status = 401, description = "Missing or invalid session", body = ErrorResponse),
        (status = 500, description = "Server fault", body = ErrorResponse),
        (status = 503, description = "JWT secret not configured or too weak", body = ErrorResponse),
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, [(HeaderName, String); 1]), ApiError> {
    let secret = jwt_secret()?;
    let token = request_token_from_headers(&headers)
        .ok_or_else(|| unauthorized("missing session token"))?;
    let data = decode::<Claims>(
        &token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| unauthorized("invalid or expired session token"))?;

    let mut r = state
        .kb
        .db()
        .db
        .query(REVOKE_SESSION_SQL)
        .bind(("jti", data.claims.jti))
        .bind(("exp", data.claims.exp as i64))
        .await
        .map_err(|e| ApiError::from(SkbError::new(ErrorCode::Db, format!("logout revoke: {e}"))))?;
    let _revoked: Vec<Value> = r.take(0).map_err(|e| {
        ApiError::from(SkbError::new(
            ErrorCode::Db,
            format!("logout revoke take: {e}"),
        ))
    })?;

    let now = jsonwebtoken::get_current_timestamp() as i64;
    let mut r = state
        .kb
        .db()
        .db
        .query(PURGE_EXPIRED_SQL)
        .bind(("now", now))
        .await
        .map_err(|e| ApiError::from(SkbError::new(ErrorCode::Db, format!("logout purge: {e}"))))?;
    let _purged: Vec<Value> = r.take(0).map_err(|e| {
        ApiError::from(SkbError::new(
            ErrorCode::Db,
            format!("logout purge take: {e}"),
        ))
    })?;

    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, CLEARED_SESSION_COOKIE.to_string())],
    ))
}
