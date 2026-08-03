use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkbError {
    pub code: ErrorCode,
    pub message: String,
}

impl std::fmt::Display for SkbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code.code_str(), self.message)
    }
}

impl std::error::Error for SkbError {}

impl SkbError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Config,
    Db,
    UnsupportedFormat,
    Io,
    DocumentNotFound,
    Embedding,
    Validation,
    ModelMismatch,
    Tokenize,
}

impl ErrorCode {
    pub fn code_str(self) -> &'static str {
        match self {
            ErrorCode::Config => "E_CONFIG",
            ErrorCode::Db => "E_DB",
            ErrorCode::UnsupportedFormat => "E_UNSUPPORTED_FORMAT",
            ErrorCode::Io => "E_IO",
            ErrorCode::DocumentNotFound => "E_DOCUMENT_NOT_FOUND",
            ErrorCode::Embedding => "E_EMBEDDING",
            ErrorCode::Validation => "E_VALIDATION",
            ErrorCode::ModelMismatch => "E_MODEL_MISMATCH",
            ErrorCode::Tokenize => "E_TOKENIZE",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Config => 2,
            ErrorCode::Db => 3,
            ErrorCode::UnsupportedFormat => 4,
            ErrorCode::Io => 5,
            ErrorCode::DocumentNotFound => 6,
            ErrorCode::Embedding => 7,
            ErrorCode::Validation => 8,
            ErrorCode::ModelMismatch => 9,
            ErrorCode::Tokenize => 10,
        }
    }

    /// Walk an error chain looking for a `SkbError` (works through `anyhow`
    /// context wrappers). Returns `None` for non-`SkbError` failures.
    pub fn from_std(e: &(dyn std::error::Error + 'static)) -> Option<ErrorCode> {
        let mut cur: &(dyn std::error::Error + 'static) = e;
        loop {
            if let Some(sk) = cur.downcast_ref::<SkbError>() {
                return Some(sk.code);
            }
            cur = cur.source()?;
        }
    }
}
