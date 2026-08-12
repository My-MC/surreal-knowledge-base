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
/// be grouped atomically (e.g. reindex §5.4).
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
        self.query(SET_META_SQL)
            .bind(("key", key))
            .bind(("val", val))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("set_meta check: {e}")))?;
        Ok(())
    }
}

/// Shared upsert implementation for the connection handle; both the inherent
/// `Db::set_meta` and the `MetaStore` trait impl delegate here so removing or
/// renaming the inherent method cannot turn the trait call into infinite
/// recursion.
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
    /// directory). Used to decide whether stored metadata can be compared
    /// before the schema is applied.
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
        // treat the database as existing so metadata comparison is never
        // skipped on a populated store.
        Ok(tables.is_some_and(|t| t.is_empty()))
    }

    pub async fn migrate(&self, embedding_dim: usize) -> Result<(), SkbError> {
        // When the stored embedding dimension differs from the target, the
        // existing embedding field ASSERT and HNSW index are redefined to the
        // new dimension (a dimension change through reindex); otherwise the
        // IF NOT EXISTS definitions below leave them untouched. Only a
        // genuinely missing meta table (fresh store) maps to None; all other
        // Db failures propagate so a transient read error is not mistaken for
        // a new database.
        let stored_dim = match self.get_meta("embedding_dimension").await {
            Ok(v) => v.and_then(|s| s.parse::<usize>().ok()),
            Err(e)
                if e.code == ErrorCode::Db
                    && e.to_string().to_lowercase().contains("meta")
                    && (e.to_string().contains("does not exist")
                        || e.to_string().contains("not found")) =>
            {
                None // fresh store: meta table does not exist yet
            }
            Err(e) => return Err(e),
        };
        if stored_dim.is_some_and(|d| d != embedding_dim) {
            // Remove the index and the field ASSERT first (an UPDATE on a
            // field with an ASSERT is validated against existing rows), then
            // wipe the stored vectors, then redefine the field for the new
            // dimension.
            let wipe = "REMOVE INDEX IF EXISTS chunk_embedding_hnsw ON chunk; \
                        REMOVE FIELD IF EXISTS embedding ON chunk; \
                        UPDATE chunk UNSET embedding;";
            self.db
                .query(wipe)
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate wipe: {e}")))?
                .check()
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate wipe check: {e}")))?;
            let redefine = format!(
                "DEFINE FIELD embedding ON chunk TYPE array<float> \
                     ASSERT array::len($value) = {embedding_dim};"
            );
            self.db
                .query(&redefine)
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("migrate redefine: {e}")))?
                .check()
                .map_err(|e| {
                    SkbError::new(ErrorCode::Db, format!("migrate redefine check: {e}"))
                })?;
        }
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
        let query = "SELECT meta_value FROM meta WHERE key = $key";
        let mut r = self
            .db
            .query(query)
            .bind(("key", key.to_string()))
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
