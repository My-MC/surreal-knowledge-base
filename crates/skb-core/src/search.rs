use crate::config::SearchMode;
use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Practical upper bound for search `top_k`: results are materialized in
/// memory, and hybrid search fetches `top_k * 3` candidates. Used by request
/// and config validation; the JSON Schema below carries the same values as
/// literals (schemars attributes cannot reference constants), and the schema
/// tests assert they stay in sync.
pub const MAX_TOP_K: usize = 1000;

/// Upper bound for graph expansion depth in one search request.
pub const MAX_GRAPH_EXPAND: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchRequest {
    pub query: String,
    pub mode: Option<SearchMode>,
    // Literal mirrors of MAX_TOP_K / MAX_GRAPH_EXPAND; kept in sync by tests.
    #[schemars(range(min = 1, max = 1000))]
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
            if top_k > MAX_TOP_K {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    format!("top_k must be at most {MAX_TOP_K}"),
                ));
            }
        }
        if let Some(depth) = self.graph_expand {
            if depth > MAX_GRAPH_EXPAND {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    format!("graph_expand must be at most {MAX_GRAPH_EXPAND}"),
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
    /// Document title; always present for persisted chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Document source (path / url / inline); always present for persisted chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Query terms found in the chunk, for both keyword and hybrid modes
    /// (only terms actually present in the hit's content are kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<String>>,
    /// Entities that led to this hit via graph expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_entities: Option<Vec<String>>,
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
         document.title AS title, document.source AS source, \
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

    rows_to_hits(&rows, None)
}

async fn keyword_search(db: &Db, query: &str, top_k: usize) -> Result<Vec<SearchHit>, SkbError> {
    let sql = format!(
        "SELECT content, idx, meta::id(document) AS document, \
         document.title AS title, document.source AS source, search::score(0) AS score \
         FROM chunk WHERE content @0@ $q ORDER BY score DESC LIMIT {top_k}"
    );

    let mut r = db
        .db
        .query(&sql)
        .bind(("q", query.to_string()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("keyword: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("keyword take: {e}")))?;

    let highlights = match_terms(query);
    rows_to_hits(&rows, Some(&highlights))
}

async fn hybrid_search(
    db: &Db,
    embedder: &dyn Embed,
    query: &str,
    top_k: usize,
    rrf_k: usize,
) -> Result<Vec<SearchHit>, SkbError> {
    let fetch_k = top_k
        .checked_mul(3)
        .ok_or_else(|| SkbError::new(ErrorCode::Validation, "top_k too large"))?;

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
         document.title AS title, document.source AS source, \
         vector::similarity::cosine(embedding, {emb_str}) AS score \
         FROM chunk WHERE embedding <|{fetch_k},40|> {emb_str} \
         ORDER BY score DESC"
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
    let ksql = format!(
        "SELECT content, idx, meta::id(id) AS chunk_id, \
         meta::id(document) AS document, \
         document.title AS title, document.source AS source, search::score(0) AS score \
         FROM chunk WHERE content @0@ $q ORDER BY score DESC LIMIT {fetch_k}"
    );
    let mut r = db
        .db
        .query(&ksql)
        .bind(("q", query.to_string()))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid kw: {e}")))?;
    let krows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("hybrid kw take: {e}")))?;

    // RRF merge
    struct RankedHit {
        score: f64,
        content: String,
        idx: usize,
        document: String,
        title: Option<String>,
        source: Option<String>,
    }
    let rrf_k = rrf_k.max(1) as f64;
    let mut scores: HashMap<String, RankedHit> = HashMap::new();

    // Vector and keyword result sets contribute to the same RRF scores with
    // identical row handling; one accumulator keeps the two loops in sync.
    let mut accumulate = |rows: &[serde_json::Value]| {
        for (rank, row) in rows.iter().enumerate() {
            let Some(id) = row["chunk_id"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let rrf = 1.0 / (rrf_k + (rank as f64 + 1.0));
            scores
                .entry(id.to_string())
                .and_modify(|e| e.score += rrf)
                .or_insert_with(|| RankedHit {
                    score: rrf,
                    content: row["content"].as_str().unwrap_or("").to_string(),
                    idx: row["idx"].as_u64().unwrap_or(0) as usize,
                    document: row["document"].as_str().unwrap_or("").to_string(),
                    title: row["title"].as_str().map(|s| s.to_string()),
                    source: row["source"].as_str().map(|s| s.to_string()),
                });
        }
    };
    // `highlights` is independent of score accumulation; compute it before
    // both accumulations so the dependency is explicit.
    let highlights = match_terms(query);
    accumulate(&vrows);
    accumulate(&krows);

    let mut sorted: Vec<_> = scores.into_iter().collect();
    sorted.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Deterministic tie-break on a stable identifier so boundary
            // results are reproducible across runs and platforms.
            .then_with(|| a.0.cmp(&b.0))
    });
    sorted.truncate(top_k);

    Ok(sorted
        .into_iter()
        .map(|(_, hit)| SearchHit {
            document_id: hit.document,
            chunk_idx: hit.idx,
            content: hit.content.clone(),
            score: hit.score,
            title: hit.title,
            source: hit.source,
            highlights: present_terms(&hit.content, &highlights),
            matched_entities: None,
        })
        .collect())
}

/// Terms from `terms` that actually occur as words in `content`; `None` when
/// none do. Words are split with the same delimiter rule as `match_terms`, so
/// "go" does not match "google".
fn present_terms(content: &str, terms: &[String]) -> Option<Vec<String>> {
    let lower = content.to_lowercase();
    let words: std::collections::HashSet<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|w| !w.is_empty())
        .collect();
    let present: Vec<String> = terms
        .iter()
        .filter(|t| words.contains(t.as_str()))
        .cloned()
        .collect();
    (!present.is_empty()).then_some(present)
}

fn rows_to_hits(
    rows: &[serde_json::Value],
    highlights: Option<&Vec<String>>,
) -> Result<Vec<SearchHit>, SkbError> {
    let mut hits = Vec::new();
    for row in rows {
        let content = row["content"].as_str().unwrap_or("").to_string();
        // Only terms actually present in this hit's content are highlighted;
        // a keyword row whose content lacks the query terms reports None.
        let hit_highlights = highlights.and_then(|terms| present_terms(&content, terms));
        hits.push(SearchHit {
            document_id: row["document"].as_str().unwrap_or("").to_string(),
            chunk_idx: row["idx"].as_u64().unwrap_or(0) as usize,
            content,
            score: row["score"].as_f64().unwrap_or(0.0),
            title: row["title"].as_str().map(|s| s.to_string()),
            source: row["source"].as_str().map(|s| s.to_string()),
            highlights: hit_highlights,
            matched_entities: None,
        });
    }
    Ok(hits)
}

/// The query terms that a keyword search can highlight: whitespace/punctuation
/// separated words of at least two characters (unicode-aware).
fn match_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect();
    terms.sort();
    terms.dedup();
    terms
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
    fn rejects_top_k_above_max() {
        let mut req = request("hello");
        req.top_k = Some(MAX_TOP_K + 1);
        assert!(matches!(
            req.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
        req.top_k = Some(MAX_TOP_K);
        assert!(req.validate().is_ok());
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
        assert_eq!(
            value["properties"]["top_k"]["maximum"], MAX_TOP_K as u64,
            "schema top_k maximum must track MAX_TOP_K"
        );
        assert_eq!(
            value["properties"]["graph_expand"]["maximum"], MAX_GRAPH_EXPAND as u64,
            "schema graph_expand maximum must track MAX_GRAPH_EXPAND"
        );
    }
}
