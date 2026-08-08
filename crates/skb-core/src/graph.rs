use crate::db::Db;
use crate::error::{ErrorCode, SkbError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use surrealdb::types::RecordId;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityInfo {
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
}

impl EntityInfo {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.name.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "name must not be empty",
            ));
        }
        if self.kind.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "kind must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinkInfo {
    pub from: String,
    pub to: String,
    pub relation: String,
    #[schemars(range(min = 0.0))]
    pub weight: Option<f64>,
}

impl LinkInfo {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.from.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "from must not be empty",
            ));
        }
        if self.to.trim().is_empty() {
            return Err(SkbError::new(ErrorCode::Validation, "to must not be empty"));
        }
        if self.relation.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "relation must not be empty",
            ));
        }
        if let Some(weight) = self.weight {
            if !weight.is_finite() || weight < 0.0 {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "weight must be a finite non-negative number",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GraphQueryRequest {
    pub from: String,
    pub relation: Option<String>,
    #[schemars(range(min = 1, max = 5))]
    pub depth: Option<usize>,
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
}

impl GraphQueryRequest {
    pub fn validate(&self) -> Result<(), SkbError> {
        if self.from.trim().is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                "from must not be empty",
            ));
        }
        if let Some(depth) = self.depth {
            if !(1..=5).contains(&depth) {
                return Err(SkbError::new(
                    ErrorCode::Validation,
                    "depth must be between 1 and 5",
                ));
            }
        }
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
pub struct GraphQueryResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    format!("entity:⟨{}⟩", clean_entity_name(name))
}

fn clean_entity_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '⟨' | '⟩' | '\\' | '\''))
        .collect()
}

fn entity_record_id(name: &str) -> Result<RecordId, SkbError> {
    let cleaned = clean_entity_name(name);
    if cleaned.is_empty() {
        return Err(SkbError::new(
            ErrorCode::Validation,
            "entity name must not be empty",
        ));
    }
    Ok(RecordId::new("entity", cleaned))
}

/// Insert-or-update an entity, addressed by its deterministic id (`entity:⟨name⟩`).
pub async fn upsert_entity(db: &Db, entity: &EntityInfo) -> Result<(), SkbError> {
    entity.validate()?;
    let sql = "INSERT INTO entity (id, name, kind, description) \
               VALUES ($id, $name, $kind, $description) \
               ON DUPLICATE KEY UPDATE description = $description";
    db.db
        .query(sql)
        .bind(("id", entity_record_id(&entity.name)?))
        .bind(("name", entity.name.clone()))
        .bind(("kind", entity.kind.clone()))
        .bind((
            "description",
            entity.description.clone().unwrap_or_default(),
        ))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity check: {e}")))?;
    Ok(())
}

/// Create a typed `related_to` edge between two entities.
pub async fn link(db: &Db, link: &LinkInfo) -> Result<(), SkbError> {
    link.validate()?;
    let weight = link.weight.unwrap_or(1.0);
    let sql = "RELATE $from->related_to->$to SET relation = $relation, weight = $weight";
    db.db
        .query(sql)
        .bind(("from", entity_record_id(&link.from)?))
        .bind(("to", entity_record_id(&link.to)?))
        .bind(("relation", link.relation.clone()))
        .bind(("weight", weight))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link check: {e}")))?;
    Ok(())
}

/// Create a `mentions` edge from a chunk to an entity.
pub async fn link_chunk_to_entity(
    db: &Db,
    chunk_id: &str,
    entity_name: &str,
) -> Result<(), SkbError> {
    let sql = "RELATE $chunk->mentions->$entity";
    db.db
        .query(sql)
        .bind((
            "chunk",
            RecordId::parse_simple(chunk_id).map_err(|e| {
                SkbError::new(ErrorCode::Validation, format!("invalid chunk id: {e}"))
            })?,
        ))
        .bind(("entity", entity_record_id(entity_name)?))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("link chunk: {e}")))?;
    Ok(())
}

/// Extract entities from a chunk, upsert each into the `entity` table, and relate
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

/// Index a chunk's entities and mentions edges inside an existing transaction.
pub(crate) async fn index_chunk_entities_in_transaction(
    tx: &surrealdb::method::Transaction<surrealdb::engine::local::Db>,
    chunk_id: &str,
    chunk_content: &str,
) -> Result<usize, SkbError> {
    let entities = extract_entities(chunk_content);
    let mut linked = 0;
    for entity in entities.iter() {
        tx.query(
            "INSERT INTO entity (id, name, kind, description) \
             VALUES ($id, $name, $kind, $description) \
             ON DUPLICATE KEY UPDATE description = $description",
        )
        .bind(("id", entity_record_id(&entity.name)?))
        .bind(("name", entity.name.clone()))
        .bind(("kind", entity.kind.clone()))
        .bind((
            "description",
            entity.description.clone().unwrap_or_default(),
        ))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity: {e}")))?
        .check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("upsert entity check: {e}")))?;

        tx.query("RELATE $chunk->mentions->$entity")
            .bind((
                "chunk",
                RecordId::parse_simple(chunk_id).map_err(|e| {
                    SkbError::new(ErrorCode::Validation, format!("invalid chunk id: {e}"))
                })?,
            ))
            .bind(("entity", entity_record_id(&entity.name)?))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("link chunk: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("link chunk check: {e}")))?;
        linked += 1;
    }
    Ok(linked)
}

/// Traverse entity relations, optionally starting from a document record.
pub async fn graph_query(db: &Db, req: &GraphQueryRequest) -> Result<GraphQueryResult, SkbError> {
    req.validate()?;
    let depth = req.depth.unwrap_or(1).min(5);
    let limit = req.limit.unwrap_or(50);
    let from = req.from.clone();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    // Resolve the starting node: a full record id (`entity:...` / `document:...`)
    // is used as-is; otherwise it is treated as an entity name.
    let mut current = if from.contains(':') {
        from.clone()
    } else {
        entity_rid(&from)
    };
    let mut emitted = HashSet::new();

    let mut start_depth = 0;
    if current.starts_with("document:") {
        let mut doc_query = db
            .db
            .query("SELECT meta::id(id) AS id, title AS name FROM $current")
            .bind(("current", start_record_id(&current)?))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("graph document: {e}")))?;
        let doc_rows: Vec<serde_json::Value> = doc_query
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("graph document take: {e}")))?;
        let Some(doc) = doc_rows.first() else {
            return Err(SkbError::new(ErrorCode::DocumentNotFound, from));
        };
        nodes.push(GraphNode {
            id: current.clone(),
            name: doc["name"].as_str().unwrap_or("").to_string(),
            kind: "document".into(),
            depth: 0,
        });
        let document_id = current.clone();

        let mut chunk_query = db
            .db
            .query(
                "SELECT ->mentions->entity.name AS next_name, \
                 ->mentions->entity.kind AS next_kind FROM chunk WHERE document = $current",
            )
            .bind(("current", start_record_id(&current)?))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("graph document chunks: {e}")))?;
        let chunk_rows: Vec<serde_json::Value> = chunk_query.take(0).map_err(|e| {
            SkbError::new(ErrorCode::Db, format!("graph document chunks take: {e}"))
        })?;
        let relation = req.relation.as_deref().unwrap_or("mentions").to_string();
        'chunks: for row in chunk_rows {
            let names = to_string_vec(&row["next_name"]);
            let kinds = to_string_vec(&row["next_kind"]);
            for (i, name) in names.into_iter().enumerate() {
                if emitted.len() >= limit {
                    break 'chunks;
                }
                let id = entity_rid(&name);
                if !emitted.insert(id.clone()) {
                    continue;
                }
                nodes.push(GraphNode {
                    id: id.clone(),
                    name,
                    kind: kinds.get(i).cloned().unwrap_or_default(),
                    depth: 1,
                });
                edges.push(GraphEdge {
                    from: document_id.clone(),
                    to: id.clone(),
                    relation: relation.clone(),
                });
                if start_depth == 0 {
                    current = id;
                }
                start_depth = 1;
            }
        }
        if start_depth == 0 || depth <= 1 {
            return Ok(GraphQueryResult { nodes, edges });
        }
    }

    // Start from the specified node and follow related_to edges.
    for d in start_depth..depth {
        let relation_filter = if req.relation.is_some() {
            "[WHERE relation = $relation]"
        } else {
            ""
        };
        let sql = format!(
            "SELECT meta::id(id) AS id, name, kind, \
             ->related_to{relation_filter}->entity.name AS next_name, \
             ->related_to{relation_filter}->entity.kind AS next_kind \
             FROM entity WHERE id = $current OR name = $current_name LIMIT 1",
            relation_filter = relation_filter,
        );
        let current_name = current
            .strip_prefix("entity:")
            .map(|name| name.trim_start_matches('⟨').trim_end_matches('⟩'))
            .unwrap_or(&current);
        let mut query = db
            .db
            .query(&sql)
            .bind(("current", start_record_id(&current)?))
            .bind(("current_name", current_name.to_string()));
        if let Some(relation) = req.relation.as_deref() {
            query = query.bind(("relation", relation));
        }
        let mut r = query
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

            for (i, nname) in next_names.into_iter().enumerate() {
                if emitted.len() >= limit {
                    break;
                }
                let nkind = next_kinds.get(i).cloned().unwrap_or_default();
                let rel = req.relation.as_deref().unwrap_or("related").to_string();
                let id = entity_rid(&nname);
                if !emitted.insert(id.clone()) {
                    continue;
                }

                nodes.push(GraphNode {
                    id: id.clone(),
                    name: nname.clone(),
                    kind: nkind,
                    depth: d + 1,
                });
                edges.push(GraphEdge {
                    from: current.clone(),
                    to: id.clone(),
                    relation: rel,
                });

                // Only follow the first result for simplicity
                if d + 1 < depth && i == 0 {
                    current = id;
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

/// Parse and validate a graph start identifier before binding it to SurrealQL.
fn start_record_id(value: &str) -> Result<RecordId, SkbError> {
    if let Some((table, key)) = value.split_once(':') {
        if !matches!(table, "entity" | "document") || key.is_empty() {
            return Err(SkbError::new(
                ErrorCode::Validation,
                format!("invalid graph record id: {value}"),
            ));
        }
        return Ok(RecordId::new(table, key));
    }

    let key = value
        .strip_prefix('⟨')
        .and_then(|key| key.strip_suffix('⟩'))
        .unwrap_or(value);
    entity_record_id(key)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_request(from: &str) -> GraphQueryRequest {
        GraphQueryRequest {
            from: from.into(),
            relation: None,
            depth: None,
            limit: None,
        }
    }

    #[test]
    fn rejects_empty_from() {
        assert!(matches!(
            graph_request("").validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
        assert!(matches!(
            graph_request("  ").validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_out_of_range_depth() {
        for depth in [0usize, 6] {
            let mut req = graph_request("A");
            req.depth = Some(depth);
            assert!(matches!(
                req.validate(),
                Err(SkbError {
                    code: ErrorCode::Validation,
                    ..
                })
            ));
        }
    }

    #[test]
    fn rejects_zero_limit() {
        let mut req = graph_request("A");
        req.limit = Some(0);
        assert!(matches!(
            req.validate(),
            Err(SkbError {
                code: ErrorCode::Validation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_empty_entity_name_and_kind() {
        for (name, kind) in [("", "k"), ("n", "")] {
            assert!(matches!(
                EntityInfo {
                    name: name.into(),
                    kind: kind.into(),
                    description: None,
                }
                .validate(),
                Err(SkbError {
                    code: ErrorCode::Validation,
                    ..
                })
            ));
        }
    }

    #[test]
    fn rejects_empty_link_parts() {
        for (from, to, relation) in [("", "b", "r"), ("a", "", "r"), ("a", "b", "")] {
            assert!(matches!(
                LinkInfo {
                    from: from.into(),
                    to: to.into(),
                    relation: relation.into(),
                    weight: None,
                }
                .validate(),
                Err(SkbError {
                    code: ErrorCode::Validation,
                    ..
                })
            ));
        }
    }
}
