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

/// Deterministic record id for an entity, derived from its name.
/// Using the name as the id keeps `RELATE`.=references and graph traversal
/// unambiguous (entity identity is the name). Characters that would break the
/// `⟨…⟩` quoted id literal are stripped.
pub(crate) fn entity_rid(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '⟨' | '⟩' | '\\' | '\''))
        .collect();
    format!("entity:⟨{cleaned}⟩")
}

/// Insert-or-update an entity, addressed by its deterministic id (`entity:⟨name⟩`).
pub async fn upsert_entity(db: &Db, entity: &EntityInfo) -> Result<(), SkbError> {
    let name = entity.name.replace('\'', "\\'");
    let kind = entity.kind.replace('\'', "\\'");
    let desc = entity
        .description
        .as_deref()
        .unwrap_or("")
        .replace('\'', "\\'");
    let rid = entity_rid(&entity.name);
    let sql = format!(
        "INSERT INTO entity (id, name, kind, description) \
         VALUES ({rid}, '{name}', '{kind}', '{desc}') \
         ON DUPLICATE KEY UPDATE description = '{desc}'",
    );
    db.db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity: {e}")))?;
    Ok(())
}

pub async fn link(db: &Db, link: &LinkInfo) -> Result<(), SkbError> {
    let weight = link.weight.unwrap_or(1.0);
    let from = entity_rid(&link.from);
    let to = entity_rid(&link.to);
    let sql = format!(
        "RELATE {from}->related_to->{to} \
         SET relation = '{}', weight = {}",
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
    let to = entity_rid(entity_name);
    let sql = format!("RELATE {chunk_id}->mentions->{to}");
    db.db
        .query(&sql)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link chunk: {e}")))?;
    Ok(())
}

/// Extract entities from a chunk, upsert each into the `entity` table, and relay
/// a `chunk ->mentions-> entity` edge so graph expansion can find related chunks.
/// Returns the number of entities linked.
pub async fn index_chunk_entities(
    db: &Db,
    chunk_id: &str,
    chunk_content: &str,
) -> Result<usize, SkbError> {
    let entities = extract_entities(chunk_content);
    let mut linked = 0;
    for entity in entities.iter() {
        upsert_entity(db, entity).await?;
        link_chunk_to_entity(db, chunk_id, &entity.name).await?;
        linked += 1;
    }
    Ok(linked)
}

pub async fn graph_query(db: &Db, req: &GraphQueryRequest) -> Result<GraphQueryResult, SkbError> {
    let depth = req.depth.unwrap_or(1).min(5);
    let limit = req.limit.unwrap_or(50);
    let from = req.from.replace('\'', "\\'");

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Resolve the starting node: a full record id (`entity:…` / `document:…`)
    // is used as-is; otherwise it is treated as an entity name.
    let mut current = if from.starts_with("entity:") || from.starts_with("document:") {
        from.clone()
    } else {
        entity_rid(&from)
    };

    // Start from the specified node
    // Get the chain: from -> related_to -> ... -> N hops
    for d in 0..depth {
        let sql = format!(
            "SELECT meta::id(id) AS id, name, kind, \
             ->related_to->entity.name AS next_name, \
             ->related_to->entity.kind AS next_kind \
             FROM {current} LIMIT 1",
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

            // Normalize `next_name`/`next_kind` to Vec<String> (single or array).
            let next_names: Vec<String> = to_string_vec(&row["next_name"]);
            let next_kinds: Vec<String> = to_string_vec(&row["next_kind"]);

            for (i, nname) in next_names.into_iter().take(limit).enumerate() {
                let nkind = next_kinds.get(i).cloned().unwrap_or_default();
                let rel = &req.relation.as_deref().unwrap_or("related").to_string();

                nodes.push(GraphNode {
                    id: nname.clone(),
                    name: nname.clone(),
                    kind: nkind,
                    depth: d + 1,
                });
                edges.push(GraphEdge {
                    from: current.clone(),
                    to: nname.clone(),
                    relation: rel.clone(),
                });

                // Only follow the first result for simplicity
                if d + 1 < depth && i == 0 {
                    current = entity_rid(&nname);
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
             FROM chunk WHERE idx = {} AND meta::id(document) = '{}'",
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
            for ename in to_string_vec(&row["e"]) {
                // Find other chunks mentioning this entity
                let esql = format!(
                    "SELECT content, idx, meta::id(document) AS document, 1.0 AS score \
                     FROM chunk WHERE '{}' IN ->mentions->entity.name \
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
                    let document_id = erow["document"].as_str().unwrap_or("").to_string();
                    let chunk_idx = erow["idx"].as_u64().unwrap_or(0) as usize;
                    // Skip chunks already present in the original hits.
                    if hits
                        .iter()
                        .any(|h| h.document_id == document_id && h.chunk_idx == chunk_idx)
                    {
                        continue;
                    }
                    expanded.push(crate::search::SearchHit {
                        document_id,
                        chunk_idx,
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

/// Normalize a SurrealQL value into a list of strings (a single value, a string
/// array, or an empty value all become `Vec<String>`).
fn to_string_vec(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
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
