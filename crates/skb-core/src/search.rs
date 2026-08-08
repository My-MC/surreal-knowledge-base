use crate::config::SearchMode;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchRequest {
    pub query: String,
    pub mode: Option<SearchMode>,
    #[schemars(range(min = 1))]
    pub top_k: Option<usize>,
    #[schemars(range(min = 0, max = 5))]
    pub graph_expand: Option<usize>,
    pub filter: Option<HashMap<String, String>>,
}

impl SearchRequest {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.query.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "query must not be empty",
            ));
        }
        if let Some(top_k) = self.top_k {
            if top_k == 0 {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "top_k must be at least 1",
                ));
            }
            if top_k > usize::MAX / 3 {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "top_k too large: fetch_k = top_k * 3 must not overflow",
                ));
            }
        }
        if let Some(depth) = self.graph_expand {
            if depth > 5 {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "graph_expand must be at most 5",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub document_id: String,
    pub chunk_idx: usize,
    pub content: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub mode: SearchMode,
    pub elapsed_ms: u64,
}

pub async fn search(
    db: &Db,
    embedder: &dyn Embed,
    rrf_k: usize,
    req: SearchRequest,
) -> Result<SearchResponse, SkbError> {
    req.validate()?;
    let mode = req.mode.unwrap_or(SearchMode::Hybrid);
    let top_k = req.top_k.unwrap_or(10);
    let start = std::time::Instant::now();

    let hits = match mode {
        SearchMode::Vector => vector_search(db, embedder, &req.query, top_k).await?,
        SearchMode::Keyword => keyword_search(db, &req.query, top_k).await?,
        SearchMode::Hybrid => hybrid_search(db, embedder, &req.query, top_k, rrf_k).await?,
    };

    let hits = if let Some(filter) = &req.filter {
        apply_filter(db, hits, filter).await?
    } else {
        hits
    };

    Ok(SearchResponse {
        hits,
        mode,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

async fn vector_search(
    db: &Db,
    embedder: &dyn Embed,
    query: &str,
    top_k: usize,
) -> Result<Vec<SearchHit>, SkbError> {
    let query_emb = embedder
        .embed_batch(&[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| SkbError::new(ErrorCode::Embedding, "no embedding"))?;

    let emb_str = serde_json::to_string(&query_emb).unwrap_or_default();
    let sql = format!(
        "SELECT content, idx, meta::id(document) AS document, \
         vector::similarity::cosine(embedding, {emb_str}) AS score \
         FROM chunk WHERE embedding <|{top_k},40|> {emb_str} \
         ORDER BY score DESC LIMIT {top_k}"
    );

    let mut r = db
        .db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("vector: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("vector take: {e}")))?;

    rows_to_hits(&rows)
}

async fn keyword_search(db: &Db, query: &str, top_k: usize) -> Result<Vec<SearchHit>, SkbError> {
    let escaped = query.replace('\'', "''");
    let sql = format!(
        "SELECT content, idx, meta::id(document) AS document, search::score(0) AS score \
         FROM chunk WHERE content @@ '{escaped}' ORDER BY score DESC LIMIT {top_k}"
    );

    let mut r = db
        .db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("keyword: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("keyword take: {e}")))?;

    rows_to_hits(&rows)
}

async fn hybrid_search(
    db: &Db,
    embedder: &dyn Embed,
    query: &str,
    top_k: usize,
    rrf_k: usize,
) -> Result<Vec<SearchHit>, SkbError> {
    let fetch_k = top_k * 3;

    let query_emb = embedder
        .embed_batch(&[query.to_string()])?
        .into_iter()
        .next()
        .ok_or_else(|| SkbError::new(ErrorCode::Embedding, "no embedding"))?;
    let emb_str = serde_json::to_string(&query_emb).unwrap_or_default();

    // Vector results
    let vsql = format!(
        "SELECT content, idx, meta::id(id) AS chunk_id, \
         meta::id(document) AS document, \
         vector::similarity::cosine(embedding, {emb_str}) AS score \
         FROM chunk WHERE embedding <|{fetch_k},40|> {emb_str}"
    );
    let mut r = db
        .db
        .query(&vsql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid vec: {e}")))?;
    let vrows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid vec take: {e}")))?;

    // Keyword results
    let escaped = query.replace('\'', "''");
    let ksql = format!(
        "SELECT content, idx, meta::id(id) AS chunk_id, \
         meta::id(document) AS document, search::score(0) AS score \
         FROM chunk WHERE content @@ '{escaped}' ORDER BY score DESC LIMIT {fetch_k}"
    );
    let mut r = db
        .db
        .query(&ksql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid kw: {e}")))?;
    let krows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid kw take: {e}")))?;

    // RRF merge
    let rrf_k = rrf_k.max(1) as f64;
    let mut scores: HashMap<String, (f64, String, usize, String)> = HashMap::new();

    for (rank, row) in vrows.iter().enumerate() {
        let id = row["id"].as_str().unwrap_or("").to_string();
        let content = row["content"].as_str().unwrap_or("").to_string();
        let idx = row["idx"].as_u64().unwrap_or(0) as usize;
        let doc = row["document"].as_str().unwrap_or("").to_string();
        let rrf = 1.0 / (rrf_k + (rank as f64 + 1.0));
        scores
            .entry(id)
            .and_modify(|e| e.0 += rrf)
            .or_insert((rrf, content, idx, doc));
    }

    for (rank, row) in krows.iter().enumerate() {
        let id = row["id"].as_str().unwrap_or("").to_string();
        let content = row["content"].as_str().unwrap_or("").to_string();
        let idx = row["idx"].as_u64().unwrap_or(0) as usize;
        let doc = row["document"].as_str().unwrap_or("").to_string();
        let rrf = 1.0 / (rrf_k + (rank as f64 + 1.0));
        scores
            .entry(id)
            .and_modify(|e| e.0 += rrf)
            .or_insert((rrf, content, idx, doc));
    }

    let mut sorted: Vec<_> = scores.into_iter().collect();
    sorted.sort_by(|a, b| {
        b.1 .0
            .partial_cmp(&a.1 .0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted.truncate(top_k);

    Ok(sorted
        .into_iter()
        .map(|(_, (score, content, idx, doc))| SearchHit {
            document_id: doc,
            chunk_idx: idx,
            content,
            score,
        })
        .collect())
}

fn rows_to_hits(rows: &[serde_json::Value]) -> Result<Vec<SearchHit>, SkbError> {
    let mut hits = Vec::new();
    for row in rows {
        hits.push(SearchHit {
            document_id: row["document"].as_str().unwrap_or("").to_string(),
            chunk_idx: row["idx"].as_u64().unwrap_or(0) as usize,
            content: row["content"].as_str().unwrap_or("").to_string(),
            score: row["score"].as_f64().unwrap_or(0.0),
        });
    }
    Ok(hits)
}

/// Post-filter hits by matching document fields (title/source/source_type/...).
/// The vector/hybrid paths use a KNN operator that cannot be combined with an
/// arbitrary field condition, so filtering happens after retrieval.
async fn apply_filter(
    db: &Db,
    hits: Vec<SearchHit>,
    filter: &HashMap<String, String>,
) -> Result<Vec<SearchHit>, SkbError> {
    if filter.is_empty() {
        return Ok(hits);
    }

    validate_filter_fields(filter)?;
    if hits.is_empty() {
        return Ok(hits);
    }

    let mut ids: Vec<String> = hits.iter().map(|h| h.document_id.clone()).collect();
    ids.sort();
    ids.dedup();
    let in_list = ids
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",");
    let fields: Vec<&str> = filter.keys().map(String::as_str).collect();
    let select_fields = fields.join(",");

    let sql = format!(
        "SELECT meta::id(id) AS id, {select_fields} \
         FROM document WHERE meta::id(id) IN [{ids}]",
        select_fields = select_fields,
        ids = in_list,
    );
    let mut r = db
        .db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("filter: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("filter take: {e}")))?;

    let rows_by_id: HashMap<&str, &serde_json::Value> = rows
        .iter()
        .filter_map(|row| row["id"].as_str().map(|id| (id, row)))
        .collect();
    let mut keep: Vec<SearchHit> = Vec::new();
    for hit in &hits {
        let Some(row) = rows_by_id.get(hit.document_id.as_str()) else {
            continue;
        };
        let mut ok = true;
        for (k, v) in filter {
            if row.get(k).and_then(|fv| fv.as_str()) != Some(v.as_str()) {
                ok = false;
                break;
            }
        }
        if ok {
            keep.push(hit.clone());
        }
    }
    Ok(keep)
}

fn validate_filter_fields(filter: &HashMap<String, String>) -> Result<(), SkbError> {
    const FILTER_FIELDS: &[&str] = &["title", "source", "source_type", "mime", "sha256"];
    for field in filter.keys() {
        if !FILTER_FIELDS.contains(&field.as_str()) {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!("unsupported search filter field: {field}"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            mode: None,
            top_k: None,
            graph_expand: None,
            filter: None,
        }
    }

    #[test]
    fn validates_unsupported_filter_even_when_hits_are_empty() {
        let filter = HashMap::from([(String::from("unsupported"), String::from("value"))]);
        let result = validate_filter_fields(&filter);
        assert!(matches!(
            result,
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_empty_query() {
        let result = request("  ").validate();
        assert!(matches!(
            result,
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_zero_top_k() {
        let mut req = request("hello");
        req.top_k = Some(0);
        assert!(matches!(
            req.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_graph_expand_beyond_five() {
        let mut req = request("hello");
        req.graph_expand = Some(6);
        assert!(matches!(
            req.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn search_schema_marks_query_required_and_mode_enum() {
        let schema = schemars::schema_for!(SearchRequest);
        let value = serde_json::to_value(&schema).unwrap();
        assert_eq!(value["required"], serde_json::json!(["query"]));
        assert_eq!(
            value["$defs"]["SearchMode"]["enum"],
            serde_json::json!(["hybrid", "vector", "keyword"])
        );
        assert_eq!(value["properties"]["top_k"]["minimum"], 1);
        assert_eq!(value["properties"]["graph_expand"]["maximum"], 5);
    }
}
