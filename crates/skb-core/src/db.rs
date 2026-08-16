use crate::config::{Config, StorageMode};
use crate::error::{ErrorCode, SkbError};
use std::path::PathBuf;
use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

pub struct Db {
    pub db: Surreal<surrealdb::engine::local::Db>,
}

/// SQL for upserting a `meta` key/value; shared by the connection handle and
/// the transaction implementation so the behavior cannot diverge.
const SET_META_SQL: &str = "INSERT INTO meta (key, meta_value) VALUES ($key, $val) \
                            ON DUPLICATE KEY UPDATE meta_value = $val";

/// Delete a `meta` row (used to remove transient markers such as
/// `reindex_in_progress` once the operation completes).
pub(crate) async fn delete_meta(
    db: &Surreal<surrealdb::engine::local::Db>,
    key: &str,
) -> Result<(), SkbError> {
    db.query("DELETE FROM meta WHERE key = $key")
        .bind(("key", key))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete_meta: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete_meta check: {e}")))?;
    Ok(())
}

/// Something that can persist a `meta` table key/value. Implemented for both
/// the connection handle and an in-progress transaction so metadata writes can
/// be grouped atomically (e.g. reindex §9-5).
pub(crate) trait MetaStore {
    fn set_meta(
        &self,
        key: &str,
        val: &str,
    ) -> impl std::future::Future<Output = Result<(), SkbError>> + Send;
}

impl MetaStore for Db {
    async fn set_meta(&self, key: &str, val: &str) -> Result<(), SkbError> {
        set_meta_impl(&self.db, key, val).await
    }
}

impl MetaStore for surrealdb::method::Transaction<surrealdb::engine::local::Db> {
    async fn set_meta(&self, key: &str, val: &str) -> Result<(), SkbError> {
        set_meta_tx(self, key, val).await
    }
}

/// Shared upsert implementation for the connection handle; both the inherent
/// `Db::set_meta` and the `MetaStore for Db` impl delegate here so the SQL and
/// error mapping cannot diverge.
async fn set_meta_impl(
    db: &Surreal<surrealdb::engine::local::Db>,
    key: &str,
    val: &str,
) -> Result<(), SkbError> {
    db.query(SET_META_SQL)
        .bind(("key", key))
        .bind(("val", val))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta check: {e}")))?;
    Ok(())
}

/// Shared upsert implementation for the transaction handle (same SQL and
/// error mapping as `set_meta_impl`).
async fn set_meta_tx(
    tx: &surrealdb::method::Transaction<surrealdb::engine::local::Db>,
    key: &str,
    val: &str,
) -> Result<(), SkbError> {
    tx.query(SET_META_SQL)
        .bind(("key", key))
        .bind(("val", val))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta check: {e}")))?;
    Ok(())
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

    /// True when the database has no tables yet (a freshly created data
    /// directory). Used to decide whether stored model metadata can be
    /// compared before the schema is applied (spec §9-5).
    pub async fn is_new_database(&self) -> Result<bool, SkbError> {
        let mut r = self
            .db
            .query("INFO FOR DB")
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("info db: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("info db take: {e}")))?;
        let tables = rows.first().and_then(|v| v["tables"].as_object());
        // Fail closed: if the result shape is unexpected (tables missing),
        // treat the database as existing so model/dimension comparison is
        // never skipped on a populated store (spec §9-5).
        Ok(tables.is_some_and(|t| t.is_empty()))
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
        // chunk_document_idx assumes (document, idx) is unique; a legacy store
        // with duplicates would silently corrupt index lookups (graph
        // expansion, deletes), so abort startup on duplicates. Runs after the
        // schema and index are applied (the index is non-UNIQUE, so applying
        // it first never fails on duplicates; the duplicate check then runs
        // against the applied store). The check is a full-table GROUP BY, so
        // it runs only once per store: success is recorded in `meta` and
        // subsequent startups skip it. Only the exact "ok" value counts as
        // validated; unexpected or corrupted metadata re-runs the check.
        if self.get_meta("dup_check_v1").await?.as_deref() != Some("ok") {
            let dup_sql =
                "SELECT string::concat('document:', meta::id(document)) AS doc_id, idx FROM \
                           (SELECT document, idx, count() AS c FROM chunk \
                            GROUP BY document, idx) WHERE c > 1 LIMIT 10";
            let mut dup = self
                .db
                .query(dup_sql)
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate dup check: {e}")))?;
            let dup_rows: Vec<serde_json::Value> = dup
                .take(0)
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate dup take: {e}")))?;
            if !dup_rows.is_empty() {
                let sample: Vec<String> = dup_rows
                    .iter()
                    .map(|r| {
                        format!(
                            "{} idx={}",
                            r["doc_id"].as_str().unwrap_or("?"),
                            r["idx"].as_u64().unwrap_or(0)
                        )
                    })
                    .collect();
                return Err(SkbError::new(
                    ErrorCode::Db,
                    format!(
                        "chunk table has duplicate (document, idx) rows; resolve before opening: {}",
                        sample.join(", ")
                    ),
                ));
            }
            set_meta_impl(&self.db, "dup_check_v1", "ok")
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate dup mark: {e}")))?;
        }
        Ok(())
    }

    pub async fn get_meta(&self, key: &str) -> Result<Option<String>, SkbError> {
        let mut r = self
            .db
            .query("SELECT meta_value FROM meta WHERE key = $key")
            .bind(("key", key))
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
        set_meta_impl(&self.db, key, val).await
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
