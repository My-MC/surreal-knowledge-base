//! HTTP error mapping: `SkbError` → status code + `{"code","message"}` body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use skb_core::error::{ErrorCode, SkbError};

/// `SkbError` with an HTTP status. The default status is derived from the
/// error code; handlers can override it explicitly (e.g. 503 for a degraded
/// dependency).
#[derive(Debug)]
pub struct ApiError {
    error: SkbError,
    status: StatusCode,
}

impl ApiError {
    pub fn new(error: impl Into<SkbError>) -> Self {
        let error = error.into();
        let status = status_for(error.code);
        Self { error, status }
    }

    /// Explicit status override; the body still carries the error code.
    pub fn with_status(error: impl Into<SkbError>, status: StatusCode) -> Self {
        Self {
            error: error.into(),
            status,
        }
    }
}

fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::Validation => StatusCode::BAD_REQUEST,
        ErrorCode::DocumentNotFound => StatusCode::NOT_FOUND,
        ErrorCode::UnsupportedFormat => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ErrorCode::Db
        | ErrorCode::Io
        | ErrorCode::Config
        | ErrorCode::Embedding
        | ErrorCode::Tokenize
        | ErrorCode::ModelMismatch => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl From<SkbError> for ApiError {
    fn from(error: SkbError) -> Self {
        Self::new(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "code": self.error.code.code_str(),
            "message": self.error.message,
        });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn response_of(api_error: ApiError) -> Response {
        api_error.into_response()
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn validation_maps_to_400() {
        let response = response_of(ApiError::new(SkbError::new(
            ErrorCode::Validation,
            "bad input",
        )))
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn document_not_found_maps_to_404() {
        let response = response_of(ApiError::new(SkbError::new(
            ErrorCode::DocumentNotFound,
            "missing",
        )))
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unsupported_format_maps_to_415() {
        let response = response_of(ApiError::new(SkbError::new(
            ErrorCode::UnsupportedFormat,
            "format",
        )))
        .await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn server_fault_codes_map_to_500() {
        for code in [
            ErrorCode::Db,
            ErrorCode::Io,
            ErrorCode::Config,
            ErrorCode::Embedding,
            ErrorCode::Tokenize,
            ErrorCode::ModelMismatch,
        ] {
            let response = response_of(ApiError::new(SkbError::new(code, "fault"))).await;
            assert_eq!(
                response.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{code:?}"
            );
        }
    }

    #[tokio::test]
    async fn body_carries_code_and_message() {
        let response = response_of(ApiError::new(SkbError::new(
            ErrorCode::DocumentNotFound,
            "doc 42 missing",
        )))
        .await;
        let json = body_json(response).await;
        assert_eq!(json["code"], "E_DOCUMENT_NOT_FOUND");
        assert_eq!(json["message"], "doc 42 missing");
    }

    #[tokio::test]
    async fn explicit_status_overrides_the_default() {
        let response = response_of(ApiError::with_status(
            SkbError::new(ErrorCode::Db, "storage unavailable"),
            StatusCode::SERVICE_UNAVAILABLE,
        ))
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json = body_json(response).await;
        assert_eq!(json["code"], "E_DB");
    }
}
