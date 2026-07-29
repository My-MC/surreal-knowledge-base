use crate::db::Db;
use crate::error::{ErrorCode, SkbError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityInfo {
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkInfo {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryRequest {
    pub from: String,
    pub relation: Option<String>,
    pub depth: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphQueryResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
}

pub async fn upsert_entity(db: &Db, entity: &EntityInfo) -> Result<(), SkbError> {
    let desc = entity.description.as_deref().unwrap_or("");
    let sql = format!(
        "INSERT INTO entity (name, kind, description) \
         VALUES ('{}', '{}', '{}') \
         ON DUPLICATE KEY UPDATE description = '{}'",
        entity.name.replace('\'', "\\'"),
        entity.kind.replace('\'', "\\'"),
        desc.replace('\'', "\\'"),
        desc.replace('\'', "\\'"),
    );
    db.db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity: {e}")))?;
    Ok(())
}

pub async fn link(db: &Db, link: &LinkInfo) -> Result<(), SkbError> {
    let weight = link.weight.unwrap_or(1.0);
    let sql = format!(
        "RELATE entity:{}->related_to->entity:{} \
         SET relation = '{}', weight = {}",
        link.from.replace('\'', "\\'"),
        link.to.replace('\'', "\\'"),
        link.relation.replace('\'', "\\'"),
        weight,
    );
    db.db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link: {e}")))?;
    Ok(())
}

pub async fn link_chunk_to_entity(
    db: &Db,
    chunk_id: &str,
    entity_name: &str,
) -> Result<(), SkbError> {
    let sql = format!(
        "RELATE {chunk_id}->mentions->entity:{}",
        entity_name.replace('\'', "\\'"),
    );
    db.db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link chunk: {e}")))?;
    Ok(())
}

pub async fn graph_query(db: &Db, req: &GraphQueryRequest) -> Result<GraphQueryResult, SkbError> {
    let depth = req.depth.unwrap_or(1).min(5);
    let limit = req.limit.unwrap_or(50);
    let from = req.from.replace('\'', "\\'");

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Start from the specified node
    let rel_match = req.relation.as_deref().map(|r| r.replace('\'', "\\'"));

    // Get the chain: from -> related_to -> ... -> N hops
    let mut current = from.clone();
    for d in 0..depth {
        let filter = if let Some(ref rel) = rel_match {
            format!("WHERE relation = '{}'", rel)
        } else {
            String::new()
        };

        let sql = format!(
            "SELECT meta::id(id) AS id, name, kind, \
             ->related_to->entity.{filter} AS next \
             FROM entity:{} LIMIT 1",
            current
        );
        let mut r = db
            .db
            .query(&sql)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("graph query: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("graph take: {e}")))?;

        if let Some(row) = rows.first() {
            if d == 0 {
                nodes.push(GraphNode {
                    id: row["id"].as_str().unwrap_or("").to_string(),
                    name: row["name"].as_str().unwrap_or("").to_string(),
                    kind: row["kind"].as_str().unwrap_or("").to_string(),
                    depth: 0,
                });
            }

            if let Some(next_arr) = row["next"].as_array() {
                for next in next_arr.iter().take(limit) {
                    let nid = next["id"].as_str().unwrap_or("").to_string();
                    let nname = next["name"].as_str().unwrap_or("").to_string();
                    let nkind = next["kind"].as_str().unwrap_or("").to_string();
                    let rel = &req.relation.as_deref().unwrap_or("related").to_string();

                    nodes.push(GraphNode {
                        id: nid.clone(),
                        name: nname.clone(),
                        kind: nkind.clone(),
                        depth: d + 1,
                    });
                    edges.push(GraphEdge {
                        from: current.clone(),
                        to: nid.clone(),
                        relation: rel.clone(),
                    });

                    if d + 1 < depth {
                        // Only follow the first result for simplicity
                        current = nid;
                    }
                }
            }
        } else {
            break;
        }

        if d + 1 >= depth {
            break;
        }
    }

    Ok(GraphQueryResult { nodes, edges })
}

/// Expand search results by following entity mentions.
/// For each search hit, find entities mentioned by its chunk,
/// then find other chunks that mention the same entities.
pub async fn expand_search_hits(
    db: &Db,
    hits: &[crate::search::SearchHit],
    max_expand: usize,
) -> Result<Vec<crate::search::SearchHit>, SkbError> {
    if max_expand == 0 || hits.is_empty() {
        return Ok(vec![]);
    }

    let mut expanded = Vec::new();

    for hit in hits.iter().take(3) {
        let _chunk_id = format!("chunk:{}", hit.chunk_idx);
        // Find entities mentioned by this chunk via document context
        // Since chunk IDs are auto-generated, we need to find actual chunks
        let sql = format!(
            "SELECT ->mentions->entity.name AS e, ->mentions->entity.kind AS k \
             FROM chunk WHERE idx = {} AND document = '{}'",
            hit.chunk_idx,
            hit.document_id.replace('\'', "\\'")
        );
        let mut r = db
            .db
            .query(&sql)
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand take: {e}")))?;

        for row in rows.iter() {
            if let Some(ename) = row["e"].as_str() {
                // Find other chunks mentioning this entity
                let esql = format!(
                    "SELECT content, idx, meta::id(document) AS document, 1.0 AS score \
                     FROM chunk WHERE ->mentions->entity.name = '{}' \
                     LIMIT {max_expand}",
                    ename.replace('\'', "\\'")
                );
                let mut r = db
                    .db
                    .query(&esql)
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand2: {e}")))?;
                let erows: Vec<serde_json::Value> = r
                    .take(0)
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand2 take: {e}")))?;

                for erow in erows.iter() {
                    expanded.push(crate::search::SearchHit {
                        document_id: erow["document"].as_str().unwrap_or("").to_string(),
                        chunk_idx: erow["idx"].as_u64().unwrap_or(0) as usize,
                        content: erow["content"].as_str().unwrap_or("").to_string(),
                        score: erow["score"].as_f64().unwrap_or(0.1),
                    });
                }
            }
        }
    }

    expanded.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    expanded.truncate(max_expand);

    Ok(expanded)
}

/// Extract entities from document text using rule-based extraction:
/// - Markdown links: [text](link)
/// - Tags in YAML frontmatter: tags: [a, b, c]
/// - Inline tags: #tag
/// - Headings: ## Section Name
pub fn extract_entities(content: &str) -> Vec<EntityInfo> {
    let mut entities = Vec::new();

    // Markdown links: [text](link)
    let link_re = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
    for cap in link_re.captures_iter(content) {
        let link_text = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        entities.push(EntityInfo {
            name: link_text.to_string(),
            kind: "reference".into(),
            description: None,
        });
    }

    // Inline tags: #tag (preceded by space, start-of-line, or punct)
    // Uses word boundary: \b#tag matches when # is at word boundary
    let tag_re = regex::Regex::new(r"(?:^|\s)#([a-zA-Z][\w-]{1,})").unwrap();
    for cap in tag_re.captures_iter(content) {
        let tag = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        entities.push(EntityInfo {
            name: tag.to_string(),
            kind: "tag".into(),
            description: None,
        });
    }

    // Headings: ^#{1,6}\s+(.+)
    let heading_re = regex::Regex::new(r"(?m)^#{1,6}\s+(.+)").unwrap();
    for cap in heading_re.captures_iter(content) {
        let heading = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if heading.len() > 2 {
            entities.push(EntityInfo {
                name: heading.trim().to_string(),
                kind: "section".into(),
                description: None,
            });
        }
    }

    entities
}
