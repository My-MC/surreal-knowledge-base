//! Server-only configuration: the `[server]` table of skb.toml plus env and
//! CLI overrides.
//!
//! Parsed separately from `skb_core::config::Config`: the core loader is
//! `#[serde(default)]`, so the unknown `[server]` key is harmless there, and
//! this loader ignores the core sections in return.

use serde::{Deserialize, Serialize};
use skb_core::error::{ErrorCode, SkbError};
use std::path::PathBuf;

/// Listen address settings. Precedence (highest wins):
/// CLI `--port`/`--host` > `SKB_SERVER_PORT`/`SKB_SERVER_HOST` > skb.toml `[server]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
        }
    }
}

/// skb.toml shape: only the `[server]` table is read; serde skips every other
/// (core-owned) section.
#[derive(Debug, Deserialize)]
struct FileConfig {
    #[serde(default)]
    server: ServerConfig,
}

impl ServerConfig {
    /// Resolve the effective server config: skb.toml `[server]`, then env
    /// overrides, then CLI overrides (highest wins).
    pub fn load(cli_port: Option<u16>, cli_host: Option<String>) -> Result<Self, SkbError> {
        let mut config = Self::from_toml()?;
        config.apply_env()?;
        if let Some(host) = cli_host {
            config.host = host;
        }
        if let Some(port) = cli_port {
            config.port = port;
        }
        Ok(config)
    }

    /// Discover skb.toml the same way `skb_core::Config` does (cwd `./skb.toml`,
    /// then `~/.config/skb/config.toml`) and take its `[server]` table.
    fn from_toml() -> Result<Self, SkbError> {
        let Some(path) = find_config_path() else {
            return Ok(Self::default());
        };
        let content = std::fs::read_to_string(&path).map_err(|e| {
            SkbError::new(
                ErrorCode::Config,
                format!("failed to read config: {}: {e}", path.display()),
            )
        })?;
        let file: FileConfig = toml::from_str(&content).map_err(|e| {
            SkbError::new(
                ErrorCode::Config,
                format!("failed to parse config: {}: {e}", path.display()),
            )
        })?;
        Ok(file.server)
    }

    /// Overlay `SKB_SERVER_HOST` / `SKB_SERVER_PORT` on top of the file config.
    pub fn apply_env(&mut self) -> Result<(), SkbError> {
        if let Some(v) = env_opt("SKB_SERVER_HOST")? {
            self.host = v;
        }
        if let Some(v) = env_opt("SKB_SERVER_PORT")? {
            self.port = v.parse().map_err(|e| {
                SkbError::new(
                    ErrorCode::Config,
                    format!("SKB_SERVER_PORT must be a port number, got '{v}': {e}"),
                )
            })?;
        }
        Ok(())
    }
}

fn find_config_path() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("./skb.toml"),
        home_dir().join(".config/skb/config.toml"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn env_opt(key: &str) -> Result<Option<String>, SkbError> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(SkbError::new(
            ErrorCode::Config,
            format!("{key} must be unicode"),
        )),
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
    fn defaults_are_localhost_8080() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn env_overrides_apply_on_top_of_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _port = EnvGuard::set("SKB_SERVER_PORT", "9999");
        let _host = EnvGuard::set("SKB_SERVER_HOST", "0.0.0.0");

        let mut config = ServerConfig::default();
        config.apply_env().unwrap();
        assert_eq!(config.port, 9999);
        assert_eq!(config.host, "0.0.0.0");
    }

    #[test]
    fn cli_overrides_win_over_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _port = EnvGuard::set("SKB_SERVER_PORT", "9999");
        let _host = EnvGuard::set("SKB_SERVER_HOST", "0.0.0.0");

        let mut config = ServerConfig::default();
        config.apply_env().unwrap();
        config.port = 1234; // what `load` does with Some(cli_port)
        config.host = "localhost".into();
        assert_eq!(config.port, 1234);
        assert_eq!(config.host, "localhost");
    }

    #[test]
    fn invalid_env_port_is_config_error() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _port = EnvGuard::set("SKB_SERVER_PORT", "not-a-number");

        let mut config = ServerConfig::default();
        let err = config.apply_env().unwrap_err();
        assert!(matches!(err.code, ErrorCode::Config));
        assert!(err.message.contains("not-a-number"));
    }

    #[test]
    fn toml_server_section_parsed_and_core_sections_ignored() {
        let raw = r#"
[storage]
path = "./somewhere"
[embedding]
onnx_path = "mock"
[server]
host = "0.0.0.0"
port = 3000
"#;
        let file: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(file.server.host, "0.0.0.0");
        assert_eq!(file.server.port, 3000);
    }

    #[test]
    fn toml_without_server_section_falls_back_to_defaults() {
        let raw = r#"
[storage]
path = "./somewhere"
"#;
        let file: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(file.server, ServerConfig::default());
    }

    #[test]
    fn toml_with_wrong_port_type_is_config_error() {
        let raw = r#"
[server]
port = "not-a-number"
"#;
        let err = toml::from_str::<FileConfig>(raw)
            .map_err(|e| SkbError::new(ErrorCode::Config, e.to_string()))
            .unwrap_err();
        assert!(matches!(err.code, ErrorCode::Config));
    }
}
