use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::tokenize::Tokenize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub source: String,
    pub sha256: String,
    pub chunk_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocumentDetail {
    pub id: String,
    pub title: String,
    pub source: String,
    pub source_type: String,
    pub sha256: String,
    pub content: String,
    pub chunks: Option<Vec<ChunkInfo>>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChunkInfo {
    pub idx: usize,
    pub content: String,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteResult {
    pub document_id: String,
    pub chunks_deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Stats {
    pub document_count: usize,
    pub chunk_count: usize,
    pub total_tokens: usize,
    pub embedding_model: String,
    pub embedding_dimension: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderBy {
    CreatedDesc,
    CreatedAsc,
    TitleAsc,
    TitleDesc,
}

impl std::str::FromStr for OrderBy {
    type Err = SkbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created_desc" => Ok(OrderBy::CreatedDesc),
            "created_asc" => Ok(OrderBy::CreatedAsc),
            "title_asc" => Ok(OrderBy::TitleAsc),
            "title_desc" => Ok(OrderBy::TitleDesc),
            _ => Err(SkbError::new(
                ErrorCode::Validation,
                "order must be created_desc, created_asc, title_asc, or title_desc",
            )),
        }
    }
}

impl OrderBy {
    fn to_surql(self) -> &'static str {
        match self {
            OrderBy::CreatedDesc => "created_at DESC",
            OrderBy::CreatedAsc => "created_at ASC",
            OrderBy::TitleAsc => "title ASC",
            OrderBy::TitleDesc => "title DESC",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ListQuery {
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub order: Option<OrderBy>,
}

impl ListQuery {
    pub fn validate(&self) -> Result<(), SkbError> {
        if let Some(limit) = self.limit {
            if limit == 0 {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "limit must be at least 1",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetDocumentRequest {
    pub id: String,
    pub include_chunks: Option<bool>,
}

impl GetDocumentRequest {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.id.trim().is_empty() {
            return Err(SkbError::new(ErrorCode::Validation, "id must not be empty"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteDocumentRequest {
    pub id: String,
}

impl DeleteDocumentRequest {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.id.trim().is_empty() {
            return Err(SkbError::new(ErrorCode::Validation, "id must not be empty"));
        }
        Ok(())
    }
}

pub async fn list_documents(db: &Db, q: &ListQuery) -> Result<Vec<DocumentSummary>, SkbError> {
    q.validate()?;
    let limit = q.limit.unwrap_or(50);
    let offset = q.offset.unwrap_or(0);
    let order_by = q.order.map_or("created_at DESC", OrderBy::to_surql);
    let query = format!(
        "SELECT string::concat('document:', meta::id(id)) AS id, \
         title, source, sha256, created_at \
         FROM document ORDER BY {order_by} LIMIT {limit} START {offset}"
    );
    let mut r = db
        .db
        .query(&query)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("list: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("list take: {e}")))?;

    Ok(rows
        .iter()
        .map(|row| DocumentSummary {
            id: val_str(row, "id"),
            title: val_str(row, "title"),
            source: val_str(row, "source"),
            sha256: val_str(row, "sha256"),
            chunk_count: 0,
            created_at: val_str(row, "created_at"),
        })
        .collect())
}

pub async fn get_document(db: &Db, req: &GetDocumentRequest) -> Result<DocumentDetail, SkbError> {
    req.validate()?;
    let id = &req.id;
    let query = format!("SELECT title, source, source_type, sha256, content, created_at FROM {id}");
    let mut r = db
        .db
        .query(&query)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("get: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("get take: {e}")))?;

    if rows.is_empty() {
        return Err(SkbError::new(
            ErrorCode::DocumentNotFound,
            format!("not found: {id}"),
        ));
    }
    let row = &rows[0];

    let chunks = if req.include_chunks.unwrap_or(false) {
        let cq = format!(
            "SELECT idx, content, token_count FROM chunk WHERE document = {id} ORDER BY idx"
        );
        let mut r = db
            .db
            .query(&cq)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("get chunks: {e}")))?;
        let crows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("get chunks take: {e}")))?;
        Some(
            crows
                .iter()
                .map(|c| ChunkInfo {
                    idx: val_u64(c, "idx") as usize,
                    content: val_str(c, "content"),
                    token_count: val_u64(c, "token_count") as usize,
                })
                .collect(),
        )
    } else {
        None
    };

    Ok(DocumentDetail {
        id: id.to_string(),
        title: val_str(row, "title"),
        source: val_str(row, "source"),
        source_type: val_str(row, "source_type"),
        sha256: val_str(row, "sha256"),
        content: val_str(row, "content"),
        chunks,
        created_at: val_str(row, "created_at"),
    })
}

pub async fn delete_document(
    db: &Db,
    req: &DeleteDocumentRequest,
) -> Result<DeleteResult, SkbError> {
    req.validate()?;
    let id = &req.id;
    let query = format!("DELETE FROM chunk WHERE document = {id}; DELETE FROM {id};");
    db.db
        .query(&query)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete: {e}")))?;

    Ok(DeleteResult {
        document_id: id.to_string(),
        chunks_deleted: 0,
    })
}

pub async fn stats(db: &Db, embedder: &dyn Embed) -> Result<Stats, SkbError> {
    let mut r = db
        .db
        .query("SELECT count() AS c FROM document GROUP ALL")
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats take: {e}")))?;
    let document_count = rows.first().and_then(|v| v["c"].as_u64()).unwrap_or(0) as usize;

    let mut r = db
        .db
        .query("SELECT count() AS c FROM chunk GROUP ALL")
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats chunk: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats chunk take: {e}")))?;
    let chunk_count = rows.first().and_then(|v| v["c"].as_u64()).unwrap_or(0) as usize;

    let mut r = db
        .db
        .query("SELECT math::sum(token_count) AS t FROM chunk GROUP ALL")
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats tokens: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("stats tokens take: {e}")))?;
    let total_tokens = rows.first().and_then(|v| v["t"].as_u64()).unwrap_or(0) as usize;

    let model = db.get_meta("embedding_model").await?.unwrap_or_default();

    Ok(Stats {
        document_count,
        chunk_count,
        total_tokens,
        embedding_model: model,
        embedding_dimension: embedder.dimension(),
    })
}

pub async fn doctor(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
) -> Result<String, SkbError> {
    let mut lines = vec!["=== SKB Doctor ===".to_string(), String::new()];
    match db.db.query("SELECT 1").await {
        Ok(_) => lines.push("[OK] SurrealDB connection".into()),
        Err(e) => lines.push(format!("[FAIL] DB: {e}")),
    }
    lines.push(format!("[INFO] Embedding dim: {}", embedder.dimension()));
    lines.push(format!(
        "[INFO] Tokenizer vocab: {}",
        tokenizer.vocab_size()
    ));
    let m = db.get_meta("embedding_model").await?;
    lines.push(format!("[INFO] Model: {}", m.unwrap_or_default()));
    let v = db.get_meta("schema_version").await?;
    lines.push(format!("[INFO] Schema ver: {}", v.unwrap_or_default()));
    Ok(lines.join("\n"))
}

fn val_str(row: &serde_json::Value, key: &str) -> String {
    row[key].as_str().unwrap_or("").to_string()
}

fn val_u64(row: &serde_json::Value, key: &str) -> u64 {
    row[key].as_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn order_by_parses_known_values() {
        assert_eq!(
            OrderBy::from_str("created_desc").unwrap(),
            OrderBy::CreatedDesc
        );
        assert_eq!(
            OrderBy::from_str("created_asc").unwrap(),
            OrderBy::CreatedAsc
        );
        assert_eq!(OrderBy::from_str("title_asc").unwrap(), OrderBy::TitleAsc);
        assert_eq!(OrderBy::from_str("title_desc").unwrap(), OrderBy::TitleDesc);
    }

    #[test]
    fn order_by_rejects_unknown_values() {
        assert!(matches!(
            OrderBy::from_str("bogus"),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn list_query_rejects_zero_limit() {
        let q = ListQuery {
            limit: Some(0),
            ..Default::default()
        };
        assert!(matches!(
            q.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn document_requests_reject_empty_id() {
        for result in [
            GetDocumentRequest {
                id: String::new(),
                include_chunks: None,
            }
            .validate(),
            DeleteDocumentRequest { id: "  ".into() }.validate(),
        ] {
            assert!(matches!(
                result,
                Err(SkbError {
                    code: ErrorCode::Validation,
                    ..
                })
            ));
        }
    }

    #[test]
    fn list_query_schema_marks_no_required_and_limit_min() {
        let schema = schemars::schema_for!(ListQuery);
        let value = serde_json::to_value(&schema).unwrap();
        assert!(
            value["required"].is_null() || value["required"] == serde_json::json!([]),
            "no field may be required in ListQuery"
        );
        assert_eq!(value["properties"]["limit"]["minimum"], 1);
    }
}
