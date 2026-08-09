use crate::db::Db;
use crate::error::{ErrorCode, SkbError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::LazyLock;
use surrealdb::types::RecordId;

/// Compile-once regular expressions for entity/section extraction (called per
/// chunk during ingest and reindex, so per-call `Regex::new` would recompile
/// them O(documents × chunks) times).
static LINK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());
static WIKI_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").unwrap());
static TAG_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:^|\s)#([a-zA-Z][\w-]{1,})").unwrap());
// Captures both the level (`#{1,6}`) and the heading name; used by heading
// extraction and section hierarchy so the heading rule lives in one place.
static SECTION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^(#{1,6})\s+(.+)").unwrap());

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
    // Persist trimmed names/kinds so " Rust " and "Rust" resolve to the same
    // entity (validation already rejects blank values after trimming).
    let name = entity.name.trim().to_string();
    let kind = entity.kind.trim().to_string();
    let sql = "INSERT INTO entity (id, name, kind, description) \
               VALUES ($id, $name, $kind, $description) \
               ON DUPLICATE KEY UPDATE description = $description";
    db.db
        .query(sql)
        .bind(("id", entity_record_id(&name)?))
        .bind(("name", name))
        .bind(("kind", kind))
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
    // Trim from/to so they resolve to the same normalized entity IDs used by
    // upsert_entity (" Rust " links to entity:⟨Rust⟩).
    let from = link.from.trim();
    let to = link.to.trim();
    let sql = "RELATE $from->related_to->$to SET relation = $relation, weight = $weight";
    db.db
        .query(sql)
        .bind(("from", entity_record_id(from)?))
        .bind(("to", entity_record_id(to)?))
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

    // Expansion starts from the top hits only, bounding the number of queries
    // and the search latency (spec §6).
    const MAX_EXPAND_ORIGINS: usize = 3;
    for hit in hits.iter().take(MAX_EXPAND_ORIGINS) {
        let origin_score = hit.score.max(0.0);

        // Hop 1: entities mentioned by this chunk.
        let sql = "SELECT ->mentions->entity.name AS e \
                   FROM chunk WHERE document = $document AND idx = $idx";
        // `document_id` is "document:<key>"; strip the table prefix so the
        // bound RecordId matches the record (and the document,idx index).
        let document = hit
            .document_id
            .split_once(':')
            .map(|(table, key)| surrealdb::types::RecordId::new(table, key))
            .unwrap_or_else(|| {
                surrealdb::types::RecordId::new("document", hit.document_id.as_str())
            });
        let mut r = db
            .db
            .query(sql)
            .bind(("document", document))
            .bind(("idx", hit.chunk_idx as i64))
            .await
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand: {e}")))?;
        let rows: Vec<serde_json::Value> = r
            .take(0)
            .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand take: {e}")))?;

        let mut frontier: Vec<(String, f64)> = Vec::new(); // (entity, decay)
        for row in rows.iter() {
            for ename in to_string_vec(&row["e"]) {
                frontier.push((ename.clone(), 1.0_f64));
                let origin = origin_entities
                    .entry(format!("{}/{}", hit.document_id, hit.chunk_idx))
                    .or_default();
                if !origin.contains(&ename) {
                    origin.push(ename);
                }
            }
        }

        // Hops 2..: follow related_to edges with distance decay.
        // Cap the frontier so a densely connected graph cannot blow up the
        // number of queries or the search latency.
        const MAX_FRONTIER: usize = 200;
        let mut visited: HashSet<String> = HashSet::new();
        for hop in 2..=max_expand {
            if frontier.len() >= MAX_FRONTIER {
                break;
            }
            let mut next: Vec<(String, f64)> = Vec::new();
            for (entity, _) in frontier.iter() {
                // Capacity check first: an entity is marked visited only when
                // it will actually be queried.
                if frontier.len() + next.len() >= MAX_FRONTIER {
                    break;
                }
                if !visited.insert(entity.clone()) {
                    continue;
                }
                let decay = 1.0 / hop as f64;
                let esql =
                    "SELECT ->related_to->entity.name AS n FROM entity WHERE name = $name LIMIT 1";
                let mut r = db
                    .db
                    .query(esql)
                    .bind(("name", entity.clone()))
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand hop: {e}")))?;
                let erows: Vec<serde_json::Value> = r
                    .take(0)
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("expand hop take: {e}")))?;
                for erow in erows.iter() {
                    for nname in to_string_vec(&erow["n"]) {
                        // Enforce the cap per name so extend(next) cannot
                        // exceed MAX_FRONTIER.
                        if frontier.len() + next.len() >= MAX_FRONTIER {
                            break;
                        }
                        next.push((nname, decay));
                    }
                }
            }
            frontier.extend(next);
        }

        // Dedup the frontier by entity name, keeping the best (closest hop =
        // highest decay) score, so the chunk query below runs once per entity.
        // Sort by name so iteration (and thus matched_entities order) is
        // deterministic across runs.
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
        let mut frontier: Vec<(String, f64)> = best.into_iter().collect();
        // Decay descending (closer hops first, so they win seen_chunks and
        // matched_entities), entity name ascending as the deterministic
        // tie-break.
        frontier.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        // Chunks mentioning any frontier entity; scores are the origin hit's
        // score decayed by hop distance (spec §6 re-rank).
        for (entity, decay) in frontier {
            let esql = "SELECT content, idx, meta::id(document) AS document, \
                        document.title AS title, document.source AS source \
                        FROM chunk WHERE $name IN ->mentions->entity.name \
                        LIMIT 50";
            let mut r = db
                .db
                .query(esql)
                .bind(("name", entity.clone()))
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
                expanded.push(SearchHit {
                    document_id,
                    chunk_idx,
                    content: erow["content"].as_str().unwrap_or("").to_string(),
                    score: origin_score * decay,
                    title: erow["title"].as_str().map(|s| s.to_string()),
                    source: erow["source"].as_str().map(|s| s.to_string()),
                    highlights: None,
                    matched_entities: Some(vec![entity.clone()]),
                });
            }
        }
    }

    // No count truncation here: `max_expand` is the hop depth and the caller
    // (KnowledgeBase::search) re-sorts the merged list with deterministic
    // tie-breaks and trims to top_k (spec §6), so no intermediate sort needed.

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
    for cap in LINK_RE.captures_iter(content) {
        let link_text = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if link_text.is_empty() {
            continue;
        }
        entities.push(EntityInfo {
            name: link_text.to_string(),
            kind: "reference".into(),
            description: None,
        });
    }

    // WikiLinks: [[target]] or [[target|alias]]
    for cap in WIKI_RE.captures_iter(content) {
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

    // Inline tags: TAG_RE matches `#tag` after start-of-input or whitespace,
    // with a leading letter and at least one word-character or hyphen.
    for cap in TAG_RE.captures_iter(content) {
        let tag = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        entities.push(EntityInfo {
            name: tag.to_string(),
            kind: "tag".into(),
            description: None,
        });
    }

    // Headings: ^#{1,6}\s+(.+)
    for cap in SECTION_RE.captures_iter(content) {
        let heading = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        if heading.chars().count() > 2 {
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
        // A bullet inside a list is always a list entry — even when the item
        // text contains a colon (e.g. "- note: something"), which must not
        // terminate the list via the key/value branch below.
        if in_list && trimmed.starts_with('-') {
            let item = trimmed[1..].trim();
            if !item.is_empty() {
                out.push(item.to_string());
            }
            continue;
        }
        if let Some((k, rest)) = trimmed.split_once(':') {
            let is_target = k.trim() == key;
            let rest = rest.trim();
            if is_target {
                in_list = true;
                if let Some(items) = parse_inline_list(rest) {
                    out.extend(items);
                    in_list = false;
                } else if !rest.is_empty() {
                    // A scalar value (e.g. `tags: rust`) is a single item;
                    // strip surrounding quotes and close the list so later
                    // unrelated bullets are not attached to this key.
                    out.push(rest.trim_matches('"').trim_matches('\'').to_string());
                    in_list = false;
                }
                continue;
            }
            // Any other key ends the current list: the following bullets
            // belong to that key's value, not to the one we are collecting.
            in_list = false;
        } else {
            in_list = false;
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
    // The opening marker must be a full `---` line (followed by a newline),
    // otherwise a horizontal rule would be misread as frontmatter.
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
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
    for cap in SECTION_RE.captures_iter(content) {
        let level = cap.get(1).map(|m| m.as_str().len()).unwrap_or(1) as u32;
        let name = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if name.chars().count() > 2 {
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
            let sql = "INSERT INTO entity (id, name, kind, description) \
                       VALUES ($id, $name, $kind, $description) \
                       ON DUPLICATE KEY UPDATE description = $description";
            // Both endpoints of the part-of edge must exist as entities. Chunk
            // processing may have missed either one (a heading split across a
            // chunk boundary is not re-extracted at document scope), so upsert
            // both the child and its parent before creating the edge.
            for name in [section.name.as_str(), parent.as_str()] {
                tx.query(sql)
                    .bind(("id", entity_record_id(name)?))
                    .bind(("name", name.to_string()))
                    .bind(("kind", "section"))
                    .bind(("description", ""))
                    .await
                    .map_err(|e| SkbError::new(ErrorCode::Db, format!("section upsert: {e}")))?
                    .check()
                    .map_err(|e| {
                        SkbError::new(ErrorCode::Db, format!("section upsert check: {e}"))
                    })?;
            }
            // Remove every part-of edge originating from the child (a section
            // may have moved under a different parent since a previous upload)
            // before creating the current parent edge. Section entities are
            // global (name-keyed), so this cleans up stale edges from any
            // document without touching unrelated relations.
            tx.query(
                "DELETE FROM related_to WHERE relation = 'part-of' AND in = $child; \
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
