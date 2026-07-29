use anyhow::Context;
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
    pub mode: StorageMode,
    pub path: PathBuf,
    pub namespace: String,
    pub database: String,
    pub url: Option<String>,
    pub username: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Hybrid,
    Vector,
    Keyword,
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
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::find_config_path()
            .context("config file not found (skb.toml or ~/.config/skb/config.toml)")?;
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        Ok(config)
    }

    fn find_config_path() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from("./skb.toml"),
            home_dir().join(".config/skb/config.toml"),
        ];
        candidates.into_iter().find(|p| p.exists())
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
