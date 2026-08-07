use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use crate::tokenize::Tokenize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub heading: Option<String>,
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
        validate_document_id(&self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeleteDocumentRequest {
    pub id: String,
}

impl DeleteDocumentRequest {
    pub fn validate(&self) -> Result<(), SkbError> {
        validate_document_id(&self.id)
    }
}

/// Validate that `id` is a `document:<key>` record id and reject inputs that
/// could alter the query when interpolated (the query itself is parameterized
/// as a second layer of defense).
fn validate_document_id(id: &str) -> Result<(), SkbError> {
    if id.trim().is_empty() {
        return Err(SkbError::new(ErrorCode::Validation, "id must not be empty"));
    }
    let (table, key) = id.split_once(':').ok_or_else(|| {
        SkbError::new(
            ErrorCode::Validation,
            format!("id must be a document record id (document:<key>), got '{id}'"),
        )
    })?;
    if table != "document" {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!("id must reference the document table, got '{table}'"),
        ));
    }
    if key.is_empty() {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!("id must not be empty: '{id}'"),
        ));
    }
    if key
        .chars()
        .any(|c| matches!(c, '\'' | '"' | ';' | '`' | '\\' | '\n' | '\r'))
    {
        return Err(SkbError::new(
            ErrorCode::Validation,
            format!("invalid document id: '{id}'"),
        ));
    }
    Ok(())
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

    // Per-document chunk counts in one grouped query (spec §9-6).
    let mut r = db
        .db
        .query(
            "SELECT string::concat('document:', meta::id(document)) AS document, \
             count() AS c FROM chunk GROUP BY document",
        )
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("list chunks: {e}")))?;
    let count_rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("list chunks take: {e}")))?;
    let counts: HashMap<String, usize> = count_rows
        .iter()
        .filter_map(|row| {
            let doc = row["document"].as_str()?;
            let count = row["c"].as_u64()? as usize;
            Some((doc.to_string(), count))
        })
        .collect();

    Ok(rows
        .iter()
        .map(|row| {
            let id = val_str(row, "id");
            DocumentSummary {
                chunk_count: counts.get(&id).copied().unwrap_or(0),
                id: id.clone(),
                title: val_str(row, "title"),
                source: val_str(row, "source"),
                sha256: val_str(row, "sha256"),
                created_at: val_str(row, "created_at"),
            }
        })
        .collect())
}

pub async fn get_document(db: &Db, req: &GetDocumentRequest) -> Result<DocumentDetail, SkbError> {
    req.validate()?;
    let record_id = document_record_id(&req.id)?;
    let query = "SELECT title, source, source_type, sha256, content, created_at FROM $id";
    let mut r = db
        .db
        .query(query)
        .bind(("id", record_id.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("get: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("get take: {e}")))?;

    if rows.is_empty() {
        return Err(SkbError::new(
            ErrorCode::DocumentNotFound,
            format!("not found: {}", req.id),
        ));
    }
    let row = &rows[0];

    let chunks = if req.include_chunks.unwrap_or(false) {
        let cq = "SELECT idx, content, token_count, heading FROM chunk WHERE document = $id ORDER BY idx";
        let mut r = db
            .db
            .query(cq)
            .bind(("id", record_id.clone()))
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
                    heading: c["heading"].as_str().map(|s| s.to_string()),
                })
                .collect(),
        )
    } else {
        None
    };

    Ok(DocumentDetail {
        id: req.id.clone(),
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
    let record_id = document_record_id(&req.id)?;

    // A missing document is an explicit error (spec §9-6).
    let mut r = db
        .db
        .query("SELECT id FROM $id LIMIT 1")
        .bind(("id", record_id.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete lookup: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete lookup take: {e}")))?;
    if rows.is_empty() {
        return Err(SkbError::new(
            ErrorCode::DocumentNotFound,
            format!("not found: {}", req.id),
        ));
    }

    // Count the chunks that will be removed (spec §9-6).
    let mut r = db
        .db
        .query("SELECT count() AS c FROM chunk WHERE document = $id GROUP ALL")
        .bind(("id", record_id.clone()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete count: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete count take: {e}")))?;
    let chunks_deleted = rows.first().and_then(|v| v["c"].as_u64()).unwrap_or(0) as usize;

    let query = "DELETE FROM mentions WHERE in.document = $id; DELETE FROM chunk WHERE document = $id; DELETE $id;";
    db.db
        .query(query)
        .bind(("id", record_id))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("delete check: {e}")))?;

    Ok(DeleteResult {
        document_id: req.id.clone(),
        chunks_deleted,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DoctorReport {
    /// SurrealDB connectivity check.
    pub db_connected: bool,
    pub embedding_dimension: usize,
    pub tokenizer_vocab: usize,
    pub model: String,
    pub schema_version: String,
    /// Environment/connectivity problems detected (empty when healthy).
    pub errors: Vec<String>,
}

impl DoctorReport {
    pub fn is_healthy(&self) -> bool {
        self.errors.is_empty()
    }
}

pub async fn doctor(
    db: &Db,
    embedder: &dyn Embed,
    tokenizer: &dyn Tokenize,
) -> Result<DoctorReport, SkbError> {
    let mut report = DoctorReport {
        db_connected: false,
        embedding_dimension: embedder.dimension(),
        tokenizer_vocab: tokenizer.vocab_size(),
        model: db.get_meta("embedding_model").await?.unwrap_or_default(),
        schema_version: db.get_meta("schema_version").await?.unwrap_or_default(),
        errors: Vec::new(),
    };
    match db.db.query("RETURN 1").await {
        Ok(_) => report.db_connected = true,
        Err(e) => report.errors.push(format!("SurrealDB connection: {e}")),
    }
    if report.embedding_dimension == 0 {
        report
            .errors
            .push("embedding dimension is 0 (model not loaded?)".to_string());
    }
    if report.tokenizer_vocab == 0 {
        report
            .errors
            .push("tokenizer vocab is 0 (tokenizer not loaded?)".to_string());
    }
    if report.model.is_empty() {
        report
            .errors
            .push("embedding model is not recorded in meta".to_string());
    }
    Ok(report)
}

fn val_str(row: &serde_json::Value, key: &str) -> String {
    row[key].as_str().unwrap_or("").to_string()
}

fn val_u64(row: &serde_json::Value, key: &str) -> u64 {
    row[key].as_u64().unwrap_or(0)
}

/// Convert a validated document id string into a typed `RecordId` for query
/// parameter binding (never interpolated into SurrealQL).
fn document_record_id(id: &str) -> Result<surrealdb::types::RecordId, SkbError> {
    let (table, key) = id
        .split_once(':')
        .expect("validated document id must contain ':'");
    Ok(surrealdb::types::RecordId::new(table, key))
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
    fn document_ids_require_document_table() {
        for id in ["abc", "foo:bar", "document:", "entity:abc"] {
            let result = GetDocumentRequest {
                id: id.into(),
                include_chunks: None,
            }
            .validate();
            assert!(
                matches!(
                    result,
                    Err(SkbError {
                        code: ErrorCode::Validation,
                        ..
                    })
                ),
                "expected '{id}' to be rejected"
            );
        }
    }

    #[test]
    fn document_ids_reject_injection_characters() {
        let malicious = "document:abc'; DELETE FROM document; --";
        let result = DeleteDocumentRequest {
            id: malicious.into(),
        }
        .validate();
        assert!(matches!(
            result,
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn document_ids_accept_normal_ids() {
        GetDocumentRequest {
            id: "document:01jhfabc123".into(),
            include_chunks: None,
        }
        .validate()
        .unwrap();
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
