use crate::error::{ErrorCode, SkbError};
use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub storage: StorageConfig,
    pub embedding: EmbeddingConfig,
    pub chunking: ChunkingConfig,
    pub search: SearchConfig,
    pub upload: UploadConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Embedded storage is currently the only supported runtime mode.
    pub mode: StorageMode,
    pub path: PathBuf,
    pub namespace: String,
    pub database: String,
    /// Reserved for a future remote storage backend; currently ignored.
    pub url: Option<String>,
    /// Reserved for a future remote storage backend; currently ignored.
    pub username: Option<String>,
    /// Reserved for a future remote storage backend; currently ignored.
    pub password: Option<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mode: StorageMode::Embedded,
            path: home_dir().join(".local/share/skb/db"),
            namespace: "skb".into(),
            database: "knowledge".into(),
            url: None,
            username: None,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageMode {
    Embedded,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub model: String,
    pub onnx_path: String,
    pub tokenizer: String,
    pub dimension: usize,
    pub max_input_tokens: usize,
    /// Reserved for provider-specific execution selection; currently ignored.
    pub device: String,
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "BAAI/bge-m3".into(),
            onnx_path: "auto".into(),
            tokenizer: "auto".into(),
            dimension: 0,
            max_input_tokens: 0,
            device: "cpu".into(),
            batch_size: 32,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChunkingConfig {
    pub max_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            overlap_tokens: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub default_mode: SearchMode,
    pub top_k: usize,
    pub rrf_k: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_mode: SearchMode::Hybrid,
            top_k: 10,
            rrf_k: 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Hybrid,
    Vector,
    Keyword,
}

impl SearchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SearchMode::Hybrid => "hybrid",
            SearchMode::Vector => "vector",
            SearchMode::Keyword => "keyword",
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = crate::error::SkbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hybrid" => Ok(SearchMode::Hybrid),
            "vector" => Ok(SearchMode::Vector),
            "keyword" => Ok(SearchMode::Keyword),
            other => Err(crate::error::SkbError::new(
                crate::error::ErrorCode::Validation,
                format!("unknown mode: {other}"),
            )),
        }
    }
}

impl std::str::FromStr for StorageMode {
    type Err = crate::error::SkbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "embedded" => Ok(StorageMode::Embedded),
            "remote" => Ok(StorageMode::Remote),
            other => Err(crate::error::SkbError::new(
                crate::error::ErrorCode::Validation,
                format!("unknown storage mode: {other}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadConfig {
    pub max_file_mb: u64,
    pub allowed_dirs: Vec<PathBuf>,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_file_mb: 100,
            allowed_dirs: vec![],
        }
    }
}

impl Config {
    /// Load configuration with the precedence:
    /// environment variables (`SKB_*`) > `./skb.toml` > `~/.config/skb/config.toml`.
    /// When no config file exists, defaults are used (environment overrides still apply).
    pub fn load() -> anyhow::Result<Self> {
        let mut config = match Self::find_config_path() {
            Some(path) => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read config: {}", path.display()))?;
                toml::from_str(&content)
                    .with_context(|| format!("failed to parse config: {}", path.display()))?
            }
            None => Config::default(),
        };
        config.apply_env_overrides()?;
        Ok(config)
    }

    /// Overlay `SKB_*` environment variables on top of the file-based config.
    /// Variable names follow the dotted key with underscores, e.g.
    /// `SKB_STORAGE_PATH`, `SKB_EMBEDDING_MODEL`, `SKB_CHUNKING_MAX_TOKENS`.
    pub fn apply_env_overrides(&mut self) -> anyhow::Result<()> {
        if let Some(v) = env_opt("SKB_STORAGE_PATH")? {
            self.storage.path = PathBuf::from(v);
        }
        if let Some(v) = env_opt("SKB_STORAGE_MODE")? {
            self.storage.mode = v
                .parse::<StorageMode>()
                .with_context(|| format!("SKB_STORAGE_MODE invalid, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_STORAGE_URL")? {
            self.storage.url = Some(v);
        }
        if let Some(v) = env_opt("SKB_STORAGE_USERNAME")? {
            self.storage.username = Some(v);
        }
        if let Some(v) = env_opt("SKB_STORAGE_PASSWORD")? {
            self.storage.password = Some(v);
        }
        if let Some(v) = env_opt("SKB_STORAGE_NAMESPACE")? {
            self.storage.namespace = v;
        }
        if let Some(v) = env_opt("SKB_STORAGE_DATABASE")? {
            self.storage.database = v;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_MODEL")? {
            self.embedding.model = v;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_ONNX_PATH")? {
            self.embedding.onnx_path = v;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_TOKENIZER")? {
            self.embedding.tokenizer = v;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_DIMENSION")? {
            self.embedding.dimension = v
                .parse()
                .with_context(|| format!("SKB_EMBEDDING_DIMENSION must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_MAX_INPUT_TOKENS")? {
            self.embedding.max_input_tokens = v.parse().with_context(|| {
                format!("SKB_EMBEDDING_MAX_INPUT_TOKENS must be a number, got '{v}'")
            })?;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_DEVICE")? {
            self.embedding.device = v;
        }
        if let Some(v) = env_opt("SKB_EMBEDDING_BATCH_SIZE")? {
            self.embedding.batch_size = v
                .parse()
                .with_context(|| format!("SKB_EMBEDDING_BATCH_SIZE must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_CHUNKING_MAX_TOKENS")? {
            self.chunking.max_tokens = v
                .parse()
                .with_context(|| format!("SKB_CHUNKING_MAX_TOKENS must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_CHUNKING_OVERLAP_TOKENS")? {
            self.chunking.overlap_tokens = v.parse().with_context(|| {
                format!("SKB_CHUNKING_OVERLAP_TOKENS must be a number, got '{v}'")
            })?;
        }
        if let Some(v) = env_opt("SKB_SEARCH_DEFAULT_MODE")? {
            self.search.default_mode = v
                .parse::<SearchMode>()
                .with_context(|| format!("SKB_SEARCH_DEFAULT_MODE invalid, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_SEARCH_TOP_K")? {
            self.search.top_k = v
                .parse()
                .with_context(|| format!("SKB_SEARCH_TOP_K must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_SEARCH_RRF_K")? {
            self.search.rrf_k = v
                .parse()
                .with_context(|| format!("SKB_SEARCH_RRF_K must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_UPLOAD_MAX_FILE_MB")? {
            self.upload.max_file_mb = v
                .parse()
                .with_context(|| format!("SKB_UPLOAD_MAX_FILE_MB must be a number, got '{v}'"))?;
        }
        if let Some(v) = env_opt("SKB_UPLOAD_ALLOWED_DIRS")? {
            let dirs: Vec<PathBuf> = v
                .split(',')
                .map(|part| PathBuf::from(part.trim()))
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
            if dirs.is_empty() {
                // Present but all entries empty (empty string, commas,
                // whitespace) would silently disable the allowed-directories
                // restriction; reject the configuration.
                anyhow::bail!(
                    "SKB_UPLOAD_ALLOWED_DIRS must list at least one directory, got '{v}'"
                );
            }
            self.upload.allowed_dirs = dirs;
        }
        Ok(())
    }

    /// Validate static config rules. Dynamic values (`dimension`, `max_input_tokens`)
    /// must be resolved against the model first via
    /// [`Config::resolve_embedding_settings`].
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.embedding.dimension == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "embedding.dimension must be resolved before validation",
            ));
        }
        if self.embedding.max_input_tokens == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "embedding.max_input_tokens must be resolved before validation",
            ));
        }
        if self.embedding.batch_size == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "embedding.batch_size must be at least 1",
            ));
        }
        if self.chunking.max_tokens == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "chunking.max_tokens must be at least 1",
            ));
        }
        // overlap_tokens == 0 (no overlap) is a valid configuration; only
        // overlap >= max_tokens is rejected below.
        if self.chunking.overlap_tokens >= self.chunking.max_tokens {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!(
                    "chunking.overlap_tokens ({}) must be less than chunking.max_tokens ({})",
                    self.chunking.overlap_tokens, self.chunking.max_tokens
                ),
            ));
        }
        if self.chunking.max_tokens > self.embedding.max_input_tokens {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!(
                    "chunking.max_tokens ({}) must not exceed embedding.max_input_tokens ({})",
                    self.chunking.max_tokens, self.embedding.max_input_tokens
                ),
            ));
        }
        if self.search.top_k == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "search.top_k must be at least 1",
            ));
        }
        if self.search.top_k > crate::search::MAX_TOP_K {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!(
                    "search.top_k ({}) must be at most {}",
                    self.search.top_k,
                    crate::search::MAX_TOP_K
                ),
            ));
        }
        if self.search.rrf_k == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "search.rrf_k must be at least 1",
            ));
        }
        if self.upload.max_file_mb == 0 {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "upload.max_file_mb must be at least 1",
            ));
        }
        Ok(())
    }

    /// Resolve `embedding.dimension` / `embedding.max_input_tokens` from the
    /// model's detected values and validate the result. Explicit config values
    /// that disagree with the model produce `E_VALIDATION`.
    ///
    /// Returns a normalized copy with detected values filled in (when the
    /// config value is 0 = auto-detect).
    pub fn resolve_embedding_settings(
        &self,
        detected_dimension: usize,
        detected_max_input_tokens: usize,
    ) -> Result<Config, SkbError> {
        let mut resolved = self.clone();
        let cfg_dim = self.embedding.dimension;
        let cfg_max = self.embedding.max_input_tokens;
        if cfg_dim != 0 && cfg_dim != detected_dimension {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!(
                    "embedding.dimension ({cfg_dim}) does not match model dimension ({detected_dimension})"
                ),
            ));
        }
        if cfg_max != 0 && cfg_max != detected_max_input_tokens {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!(
                    "embedding.max_input_tokens ({cfg_max}) does not match model max input ({detected_max_input_tokens})"
                ),
            ));
        }
        if cfg_dim == 0 {
            resolved.embedding.dimension = detected_dimension;
        }
        if cfg_max == 0 {
            resolved.embedding.max_input_tokens = detected_max_input_tokens;
        }
        resolved.validate()?;
        Ok(resolved)
    }

    fn find_config_path() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from("./skb.toml"),
            home_dir().join(".config/skb/config.toml"),
        ];
        candidates.into_iter().find(|p| p.exists())
    }

    /// Path to the config file that `set` should write to: the first existing
    /// config (project then user), else the project `./skb.toml`.
    pub fn writable_config_path() -> PathBuf {
        Self::find_config_path().unwrap_or_else(|| PathBuf::from("./skb.toml"))
    }
}

fn env_opt(key: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(anyhow::anyhow!("{key} must be unicode")),
    }
}

fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home);
    }
    if cfg!(target_os = "windows") {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(profile);
        }
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_default() -> Config {
        Config::default()
            .resolve_embedding_settings(
                crate::embed::MOCK_EMBEDDER_DIMENSION,
                crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS,
            )
            .unwrap()
    }

    #[test]
    fn validate_accepts_defaults() {
        resolved_default().validate().unwrap();
    }

    #[test]
    fn validate_accepts_zero_overlap() {
        let mut c = resolved_default();
        c.chunking.overlap_tokens = 0;
        // No-overlap chunking is valid (spec allows 0 <= overlap < max).
        c.validate().unwrap();
    }

    #[test]
    fn validate_rejects_overlap_gte_max() {
        let mut c = resolved_default();
        c.chunking.overlap_tokens = 512;
        c.chunking.max_tokens = 512;
        assert!(matches!(
            c.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn validate_rejects_max_tokens_above_model_input() {
        let mut c = resolved_default();
        c.chunking.max_tokens = crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS + 1;
        assert!(matches!(
            c.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn resolve_rejects_explicit_dimension_mismatch() {
        let mut c = Config::default();
        // Explicit value deliberately disagrees with the detected dimension.
        c.embedding.dimension = crate::embed::MOCK_EMBEDDER_DIMENSION + 1;
        assert!(matches!(
            c.resolve_embedding_settings(
                crate::embed::MOCK_EMBEDDER_DIMENSION,
                crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS
            ),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn resolve_rejects_explicit_max_input_mismatch() {
        let mut c = Config::default();
        // Explicit value deliberately disagrees with the detected max input.
        c.embedding.max_input_tokens = crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS / 2;
        assert!(matches!(
            c.resolve_embedding_settings(
                crate::embed::MOCK_EMBEDDER_DIMENSION,
                crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS
            ),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn resolve_fills_detected_values() {
        let c = Config::default()
            .resolve_embedding_settings(
                crate::embed::MOCK_EMBEDDER_DIMENSION,
                crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS,
            )
            .unwrap();
        assert_eq!(c.embedding.dimension, 8);
        assert_eq!(
            c.embedding.max_input_tokens,
            crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS
        );
    }

    #[test]
    fn resolve_accepts_matching_explicit_values() {
        let mut c = Config::default();
        c.embedding.dimension = crate::embed::MOCK_EMBEDDER_DIMENSION;
        c.embedding.max_input_tokens = crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS;
        let resolved = c
            .resolve_embedding_settings(
                crate::embed::MOCK_EMBEDDER_DIMENSION,
                crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS,
            )
            .unwrap();
        assert_eq!(
            resolved.embedding.dimension,
            crate::embed::MOCK_EMBEDDER_DIMENSION
        );
        assert_eq!(
            resolved.embedding.max_input_tokens,
            crate::embed::MOCK_EMBEDDER_MAX_INPUT_TOKENS
        );
    }

    struct EnvGuard(Vec<(&'static str, Option<String>)>);

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            EnvGuard(vec![(key, old)])
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, old) in &self.0 {
                match old {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    // Env mutation must not race between tests in this module.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn env_overrides_apply_with_precedence() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _model = EnvGuard::set("SKB_EMBEDDING_MODEL", "env-model");
        let _tokens = EnvGuard::set("SKB_CHUNKING_MAX_TOKENS", "256");
        let _top_k = EnvGuard::set("SKB_SEARCH_TOP_K", "42");
        let _dirs = EnvGuard::set("SKB_UPLOAD_ALLOWED_DIRS", "/a,/b");

        let mut config = Config::default();
        config.apply_env_overrides().unwrap();
        assert_eq!(config.embedding.model, "env-model");
        assert_eq!(config.chunking.max_tokens, 256);
        assert_eq!(config.search.top_k, 42);
        assert_eq!(
            config.upload.allowed_dirs,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn env_overrides_reject_invalid_numbers() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _tokens = EnvGuard::set("SKB_CHUNKING_MAX_TOKENS", "not-a-number");
        let mut config = Config::default();
        assert!(config.apply_env_overrides().is_err());
    }

    #[test]
    fn load_works_without_config_file_when_env_set() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _model = EnvGuard::set("SKB_EMBEDDING_MODEL", "env-only-model");
        // Run Config::load() from an isolated cwd with no config file: it must
        // fall back to defaults and apply the environment override.
        let original = std::env::current_dir().unwrap();
        let isolated =
            std::path::PathBuf::from(format!("./target/skb-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&isolated).unwrap();
        std::env::set_current_dir(&isolated).unwrap();
        let _cwd_guard = CwdGuard::new(original);
        let config = Config::load().unwrap();
        let _ = std::fs::remove_dir_all(&isolated);
        assert_eq!(config.embedding.model, "env-only-model");
    }

    /// Restores the original current directory on drop, including on panic.
    struct CwdGuard {
        original: std::path::PathBuf,
    }

    impl CwdGuard {
        fn new(original: std::path::PathBuf) -> Self {
            Self { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }
}
