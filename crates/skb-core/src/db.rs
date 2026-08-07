use crate::config::{Config, StorageMode};
use crate::error::{ErrorCode, SkbError};
use std::path::PathBuf;
use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

pub struct Db {
    pub db: Surreal<surrealdb::engine::local::Db>,
}

impl Db {
    pub async fn open(config: &Config) -> Result<Self, SkbError> {
        let path = shellexpand_path(&config.storage.path);

        match config.storage.mode {
            StorageMode::Embedded => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| SkbError::new(ErrorCode::Db, format!("create db dir: {e}")))?;
                }
                let db = Surreal::new::<SurrealKv>(path.display().to_string())
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("open SurrealKv: {e}")))?;

                db.use_ns(&config.storage.namespace)
                    .use_db(&config.storage.database)
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("use ns/db: {e}")))?;

                Ok(Self { db })
            }
            StorageMode::Remote => Err(SkbError::new(
                ErrorCode::Db,
                "Remote mode not yet implemented. Use embedded mode (default).",
            )),
        }
    }

    pub async fn migrate(&self, embedding_dim: usize) -> Result<(), SkbError> {
        let schema = include_str!("../../../schema/001_init.surql");
        let schema = schema.replace("{DIM}", &embedding_dim.to_string());
        self.db
            .query(&schema)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate check: {e}")))?;
        Ok(())
    }

    pub async fn get_meta(&self, key: &str) -> Result<Option<String>, SkbError> {
        let query = format!("SELECT meta_value FROM meta WHERE key = '{key}'");
        let mut r = self
            .db
            .query(&query)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("get_meta: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("get_meta take: {e}")))?;
        Ok(rows
            .first()
            .and_then(|v| v["meta_value"].as_str().map(|s| s.to_string())))
    }

    pub async fn set_meta(&self, key: &str, val: &str) -> Result<(), SkbError> {
        let query = format!(
            "INSERT INTO meta (key, meta_value) VALUES ('{key}', '{val}') \
             ON DUPLICATE KEY UPDATE meta_value = '{val}'"
        );
        self.db
            .query(&query)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta check: {e}")))?;
        Ok(())
    }
}

fn shellexpand_path(p: &std::path::Path) -> PathBuf {
    let s = p.display().to_string();
    if s.starts_with("~/") || s == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            if let Some(rest) = s.strip_prefix("~/") {
                return PathBuf::from(home).join(rest);
            } else {
                return PathBuf::from(home);
            }
        }
    }
    p.to_path_buf()
}
