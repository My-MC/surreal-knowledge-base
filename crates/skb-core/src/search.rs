use crate::db::Db;
use crate::embed::Embed;
use crate::error::{ErrorCode, SkbError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub mode: Option<String>,
    pub top_k: Option<usize>,
    pub graph_expand: Option<usize>,
    pub filter: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub document_id: String,
    pub chunk_idx: usize,
    pub content: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub mode: String,
    pub elapsed_ms: u64,
}

pub async fn search(
    db: &Db,
    embedder: &dyn Embed,
    req: SearchRequest,
) -> Result<SearchResponse, SkbError> {
    let mode = req.mode.as_deref().unwrap_or("hybrid");
    let top_k = req.top_k.unwrap_or(10);
    let start = std::time::Instant::now();

    let hits = match mode {
        "vector" => vector_search(db, embedder, &req.query, top_k).await?,
        "keyword" => keyword_search(db, &req.query, top_k).await?,
        "hybrid" => hybrid_search(db, embedder, &req.query, top_k).await?,
        _ => {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!("unknown mode: {mode}"),
            ))
        }
    };

    Ok(SearchResponse {
        hits,
        mode: mode.to_string(),
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
    let rrf_k: f64 = 60.0;
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
