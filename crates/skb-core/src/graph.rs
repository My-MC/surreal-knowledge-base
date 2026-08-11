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
) -> Result<Vec<String>, SkbError> {
    let entities = extract_entities(chunk_content);
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
    }
    Ok(entities.into_iter().map(|e| e.name).collect())
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

/// Expand search results by following the graph (spec §6): each hit's chunk
/// mentions entities (hop 1); `related_to` edges extend the frontier for
/// `max_expand - 1` further hops; chunks mentioning any frontier entity are
/// returned with a hop-decayed score and the connecting entity recorded in
/// `matched_entities`.
///
/// Returns the expanded hits plus, for each original hit, the entities its
/// chunk mentions (keyed `"<document_id>/<chunk_idx>"`).
pub async fn expand_search_hits(
    db: &Db,
    hits: &[crate::search::SearchHit],
    max_expand: usize,
) -> Result<
    (
        Vec<crate::search::SearchHit>,
        std::collections::HashMap<String, Vec<String>>,
    ),
    SkbError,
> {
    use crate::search::SearchHit;
    if max_expand == 0 || hits.is_empty() {
        return Ok((vec![], std::collections::HashMap::new()));
    }

    let mut expanded = Vec::new();
    let mut origin_entities: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut seen_chunks: std::collections::HashSet<(String, usize)> = hits
        .iter()
        .map(|h| (h.document_id.clone(), h.chunk_idx))
        .collect();

    // Direct-hit entity metadata is recorded for EVERY hit, while graph
    // expansion is bounded to the top EXPAND_ORIGIN_LIMIT hits so dense
    // result sets stay cheap.
    const EXPAND_ORIGIN_LIMIT: usize = 3;
    const FRONTIER_MAX: usize = 100;

    // Hop 1 for all hits in ONE batched query, using direct RecordId
    // comparison (no string-based document matching). document_id in search
    // results is the raw key; rebuild the `document:<key>` record id for the
    // chunk's document link. The (document, idx) pair filter is applied in
    // Rust so an IN x IN cross product cannot match wrong pairs.
    let wanted: std::collections::HashSet<(String, usize)> = hits
        .iter()
        .map(|h| (h.document_id.clone(), h.chunk_idx))
        .collect();
    let unique_docs: Vec<RecordId> = hits
        .iter()
        .map(|h| format!("document:{}", h.document_id))
        .filter_map(|s| RecordId::parse_simple(&s).ok())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let sql = "SELECT meta::id(document) AS document, idx, ->mentions->entity.name AS e \
               FROM chunk WHERE document IN $docs";
    let mut r = db
        .db
        .query(sql)
        .bind(("docs", unique_docs))
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand: {e}")))?;
    let rows: Vec<serde_json::Value> = r
        .take(0)
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand take: {e}")))?;

    // Bucket entities by hit (document, idx); the bucket key matches the
    // origin_entities key format used by the caller.
    let mut by_hit: std::collections::HashMap<(String, usize), Vec<String>> =
        std::collections::HashMap::new();
    for row in rows.iter() {
        let doc = row["document"].as_str().unwrap_or("").to_string();
        let idx = row["idx"].as_u64().unwrap_or(0) as usize;
        if !wanted.contains(&(doc.clone(), idx)) {
            continue;
        }
        for ename in to_string_vec(&row["e"]) {
            by_hit.entry((doc.clone(), idx)).or_default().push(ename);
        }
    }

    for (hit_idx, hit) in hits.iter().enumerate() {
        let origin_score = hit.score.max(0.0);
        let do_expand = hit_idx < EXPAND_ORIGIN_LIMIT && max_expand > 0;

        let mut frontier: Vec<(String, f64)> = Vec::new(); // (entity, decay)
        let hit_entities = by_hit
            .remove(&(hit.document_id.clone(), hit.chunk_idx))
            .unwrap_or_default();
        for ename in hit_entities {
            // Decay below 1.0 so expanded results can never tie direct
            // hits in the re-rank (spec §6).
            frontier.push((ename.clone(), 0.95_f64));
            origin_entities
                .entry(format!("{}/{}", hit.document_id, hit.chunk_idx))
                .or_default()
                .push(ename);
        }

        if !do_expand {
            continue;
        }

        // Cap each hop's frontier so a dense graph cannot issue unbounded
        // related_to queries (request-level bound on query fan-out).
        if frontier.len() > FRONTIER_MAX {
            frontier.truncate(FRONTIER_MAX);
        }

        // Hops 2..: follow related_to edges with distance decay. Each hop is
        // one batched query (IN $names) instead of one query per entity.
        let mut visited: HashSet<String> = HashSet::new();
        for hop in 2..=max_expand {
            let mut next: Vec<(String, f64)> = Vec::new();
            let hop_names: Vec<String> = frontier
                .iter()
                .filter(|(e, _)| visited.insert(e.clone()))
                .map(|(e, _)| e.clone())
                .collect();
            if hop_names.is_empty() {
                continue;
            }
            let decay = 1.0 / hop as f64;
            let esql = "SELECT name, ->related_to->entity.name AS n \
                        FROM entity WHERE name IN $names";
            let mut r = db
                .db
                .query(esql)
                .bind(("names", hop_names))
                .await
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand hop: {e}")))?;
            let erows: Vec<serde_json::Value> = r
                .take(0)
                .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand hop take: {e}")))?;
            for erow in erows.iter() {
                for nname in to_string_vec(&erow["n"]) {
                    next.push((nname, decay));
                }
            }
            // Cap the frontier at the end of each hop so a dense graph
            // cannot grow it unboundedly across hops (request-level bound).
            // Sort by decay descending first so truncation keeps the closest
            // (highest-decay) entities, independent of insertion/HashMap
            // order.
            frontier.extend(next);
            if frontier.len() > FRONTIER_MAX {
                frontier.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                frontier.truncate(FRONTIER_MAX);
            }
        }

        // Dedup the frontier by entity name, keeping the best (closest hop =
        // highest decay) score, so the chunk query below runs once per entity.
        let mut best: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (entity, decay) in frontier {
            best.entry(entity)
                .and_modify(|d| {
                    if decay > *d {
                        *d = decay;
                    }
                })
                .or_insert(decay);
        }
        // Same request-level fan-out bound: the chunk-query loop below must
        // not exceed the cap either. Sort by decay descending so truncation
        // keeps the closest entities regardless of HashMap iteration order.
        let mut frontier: Vec<(String, f64)> = best.into_iter().collect();
        frontier.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        frontier.truncate(FRONTIER_MAX);

        // Chunks mentioning any frontier entity; scores are the origin hit's
        // score decayed by hop distance (spec §6 re-rank). All frontier
        // entities are resolved in one batched query to avoid N+1 round trips.
        // The predicate runs through the mentions edge on entity (selective:
        // candidate chunks are filtered before LIMIT, not scanned and cut).
        let names: Vec<String> = frontier.iter().map(|(e, _)| e.clone()).collect();
        let decay_map: std::collections::HashMap<String, f64> = frontier.into_iter().collect();
        let esql = "SELECT content, idx, meta::id(document) AS document, \
                    document.title AS title, document.source AS source, \
                    ->mentions->entity.name AS e \
                    FROM chunk WHERE $names CONTAINSANY ->mentions->entity.name \
                    LIMIT 200";
        let mut r = db
            .db
            .query(esql)
            .bind(("names", names))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand2: {e}")))?;
        let erows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand2 take: {e}")))?;

        for erow in erows.iter() {
            let document_id = erow["document"].as_str().unwrap_or("").to_string();
            let chunk_idx = erow["idx"].as_u64().unwrap_or(0) as usize;
            if !seen_chunks.insert((document_id.clone(), chunk_idx)) {
                continue;
            }
            let matched = to_string_vec(&erow["e"]);
            // Missing-decay fallback stays below 1.0 so an expanded result
            // can never tie a direct hit in the re-rank (spec §6).
            let (decay, entity) = matched
                .iter()
                .filter_map(|e| decay_map.get(e).map(|d| (d, e.clone())))
                .max_by(|a, b| a.0.partial_cmp(b.0).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or((&0.95, matched.into_iter().next().unwrap_or_default()));
            expanded.push(SearchHit {
                document_id,
                chunk_idx,
                content: erow["content"].as_str().unwrap_or("").to_string(),
                score: origin_score * *decay,
                title: erow["title"].as_str().map(|s| s.to_string()),
                source: erow["source"].as_str().map(|s| s.to_string()),
                highlights: None,
                matched_entities: Some(vec![entity]),
            });
        }
    }

    // No count truncation here: `max_expand` is the hop depth and the caller
    // (KnowledgeBase::search) trims the merged list to top_k (spec §6).
    expanded.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok((expanded, origin_entities))
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

/// Extracts entities from document text. The default implementation is
/// rule-based; an LLM-backed extractor can replace it later (spec §5.3).
pub trait EntityExtractor: Send + Sync {
    fn extract(&self, content: &str) -> Vec<EntityInfo>;
}

/// Default rule-based extractor (spec §5.3): Markdown links, WikiLinks,
/// frontmatter tags/aliases, inline tags and headings.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleBasedExtractor;

impl EntityExtractor for RuleBasedExtractor {
    fn extract(&self, content: &str) -> Vec<EntityInfo> {
        extract_entities(content)
    }
}

/// Extract entities from document text using rule-based extraction:
/// - Markdown links: [text](link)
/// - WikiLinks: [[target]] / [[target|alias]]
/// - Tags in YAML frontmatter: tags: [a, b, c]
/// - Aliases in YAML frontmatter: aliases: [x, y]
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

    // WikiLinks: [[target]] or [[target|alias]]
    let wiki_re = regex::Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").unwrap();
    for cap in wiki_re.captures_iter(content) {
        let name = cap
            .get(2)
            .map(|m| m.as_str())
            .filter(|a| !a.trim().is_empty())
            .or_else(|| cap.get(1).map(|m| m.as_str()))
            .unwrap_or("")
            .trim();
        if !name.is_empty() {
            entities.push(EntityInfo {
                name: name.to_string(),
                kind: "reference".into(),
                description: None,
            });
        }
    }

    // YAML frontmatter: tags and aliases.
    for tag in frontmatter_list(content, "tags") {
        entities.push(EntityInfo {
            name: tag,
            kind: "tag".into(),
            description: None,
        });
    }
    for alias in frontmatter_list(content, "aliases") {
        entities.push(EntityInfo {
            name: alias,
            kind: "alias".into(),
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

    dedup_entities(entities)
}

/// Read a list value (`[a, b]` or `- item` lines) for `key` from a YAML
/// frontmatter block at the top of the document.
fn frontmatter_list(content: &str, key: &str) -> Vec<String> {
    let Some(body) = frontmatter_body(content) else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut in_list = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("---") {
            break;
        }
        if let Some((k, rest)) = trimmed.split_once(':') {
            let is_target = k.trim() == key;
            let rest = rest.trim();
            if is_target {
                in_list = true;
                if let Some(items) = parse_inline_list(rest) {
                    out.extend(items);
                    in_list = false;
                }
                continue;
            }
            // Any other key ends the current list: the following bullets
            // belong to that key's value, not to the one we are collecting.
            in_list = false;
        } else if in_list {
            if let Some(item) = trimmed.strip_prefix("-") {
                let item = item.trim();
                if !item.is_empty() {
                    out.push(item.to_string());
                }
            } else {
                in_list = false;
            }
        }
    }
    out
}

fn parse_inline_list(rest: &str) -> Option<Vec<String>> {
    let rest = rest.trim();
    if !rest.starts_with('[') || !rest.ends_with(']') {
        return None;
    }
    Some(
        rest[1..rest.len() - 1]
            .split(',')
            .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|item| !item.is_empty())
            .collect(),
    )
}

/// The text between the leading `---` markers, if present.
fn frontmatter_body(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn dedup_entities(entities: Vec<EntityInfo>) -> Vec<EntityInfo> {
    let mut seen = std::collections::HashSet::new();
    entities
        .into_iter()
        .filter(|e| seen.insert((e.name.clone(), e.kind.clone())))
        .collect()
}

/// Sections with their markdown level, in document order.
pub struct Section {
    pub name: String,
    pub level: u32,
}

/// Extract heading sections with levels for hierarchy linking.
pub fn extract_sections(content: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let heading_re = regex::Regex::new(r"(?m)^(#{1,6})\s+(.+)").unwrap();
    for cap in heading_re.captures_iter(content) {
        let level = cap.get(1).map(|m| m.as_str().len()).unwrap_or(1) as u32;
        let name = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if name.len() > 2 {
            sections.push(Section {
                name: name.to_string(),
                level,
            });
        }
    }
    sections
}

/// Link heading sections into a `related_to(relation = "part-of")` hierarchy:
/// each section becomes part of the nearest preceding section with a lower
/// level (spec §5.3). Entities are upserted as `kind = "section"`.
pub(crate) async fn link_section_hierarchy(
    tx: &surrealdb::method::Transaction<surrealdb::engine::local::Db>,
    content: &str,
) -> Result<(), SkbError> {
    let mut stack: Vec<(String, u32)> = Vec::new();
    for section in extract_sections(content) {
        while let Some((_, level)) = stack.last() {
            if *level < section.level {
                break;
            }
            stack.pop();
        }
        if let Some((parent, _)) = stack.last() {
            let upsert_sql = "INSERT INTO entity (id, name, kind, description) \
                              VALUES ($id, $name, $kind, $description) \
                              ON DUPLICATE KEY UPDATE description = $description";
            // Both endpoints of the part-of edge are sections; upsert the
            // parent (ancestor) and the child (current section) so the child
            // exists as an entity even when it is a leaf with no descendants.
            for (name, id) in [
                (parent.clone(), entity_record_id(parent)?),
                (section.name.clone(), entity_record_id(&section.name)?),
            ] {
                tx.query(upsert_sql)
                    .bind(("id", id))
                    .bind(("name", name))
                    .bind(("kind", "section"))
                    .bind(("description", ""))
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("section upsert: {e}")))?
                    .check()
                    .map_err(|e| {
                        SkbError::new(ErrorCode::Db, format!("section upsert check: {e}"))
                    })?;
            }
            // Idempotent edge: `RELATE` always creates a new edge, so remove
            // an existing part-of edge between the same pair first. Section
            // entities are global (shared across documents), so the dedup is
            // per pair — repeated uploads never accumulate duplicates.
            // Direction: the child section is part of its ancestor, so the
            // edge points child -> part-of -> parent.
            tx.query(
                "DELETE FROM related_to WHERE relation = 'part-of' \
                 AND in = $child AND out = $parent; \
                 RELATE $child->related_to->$parent SET relation = 'part-of', weight = 1.0",
            )
            .bind(("child", entity_record_id(&section.name)?))
            .bind(("parent", entity_record_id(parent)?))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("section link: {e}")))?
            .check()
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("section link check: {e}")))?;
        }
        stack.push((section.name, section.level));
    }
    Ok(())
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

    #[test]
    fn extracts_markdown_links_and_tags() {
        let entities = extract_entities("# Doc\n\nSee [HNSW](https://h.w) and #graph");
        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"HNSW"));
        assert!(names.contains(&"graph"));
        assert!(entities
            .iter()
            .any(|e| e.kind == "section" && e.name == "Doc"));
    }

    #[test]
    fn extracts_wikilinks_with_alias() {
        let entities = extract_entities("Read [[SurrealDB]] and [[Vector Search|vect]]");
        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"SurrealDB"));
        assert!(names.contains(&"vect"));
        assert!(
            entities.iter().all(|e| e.kind == "reference"),
            "wiki links must be references"
        );
    }

    #[test]
    fn extracts_frontmatter_tags_and_aliases() {
        let content = "---\ntags: [rust, database]\naliases:\n  - surrealkb\n---\nbody";
        let entities = extract_entities(content);
        let tags: Vec<&str> = entities
            .iter()
            .filter(|e| e.kind == "tag")
            .map(|e| e.name.as_str())
            .collect();
        let aliases: Vec<&str> = entities
            .iter()
            .filter(|e| e.kind == "alias")
            .map(|e| e.name.as_str())
            .collect();
        assert!(tags.contains(&"rust"));
        assert!(tags.contains(&"database"));
        assert!(aliases.contains(&"surrealkb"));
    }

    #[test]
    fn frontmatter_list_stops_at_other_keys() {
        // A bare key (`aliases:`) must end the tags list: the aliases bullets
        // must not leak into the tags result (regression).
        let content = "---\ntags:\n  - a\naliases:\n  - b\n  - c\ndescription: x\n---\nbody";
        assert_eq!(frontmatter_list(content, "tags"), vec!["a"]);
        assert_eq!(frontmatter_list(content, "aliases"), vec!["b", "c"]);
    }

    #[test]
    fn extract_dedups_entities() {
        let entities = extract_entities("[[A]] [A](x) #tag #tag");
        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names.iter().filter(|n| **n == "A").count(), 1);
        assert_eq!(names.iter().filter(|n| **n == "tag").count(), 1);
    }

    #[test]
    fn extracts_section_levels() {
        let sections = extract_sections("# Alpha\n## Beta\n### Gamma\n## Delta\n");
        let pairs: Vec<(String, u32)> = sections.into_iter().map(|s| (s.name, s.level)).collect();
        assert_eq!(
            pairs,
            vec![
                ("Alpha".to_string(), 1),
                ("Beta".to_string(), 2),
                ("Gamma".to_string(), 3),
                ("Delta".to_string(), 2),
            ]
        );
    }

    #[test]
    fn rule_based_extractor_implements_trait() {
        let extractor = RuleBasedExtractor;
        let entities = extractor.extract("[[WikiLink]]");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "WikiLink");
    }

    #[test]
    fn rejects_invalid_link_weight() {
        for weight in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(matches!(
                LinkInfo {
                    from: "a".into(),
                    to: "b".into(),
                    relation: "r".into(),
                    weight: Some(weight),
                }
                .validate(),
                Err(SkbError {
                    code: ErrorCode::Validation,
                    ..
                })
            ));
        }
        // Valid non-negative finite weights are accepted.
        for weight in [0.0, 1.0, 2.5] {
            assert!(LinkInfo {
                from: "a".into(),
                to: "b".into(),
                relation: "r".into(),
                weight: Some(weight),
            }
            .validate()
            .is_ok());
        }
    }
}
