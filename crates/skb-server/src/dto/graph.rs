//! Server-owned graph DTOs (plan todo 4): the graph-query passthrough, the
//! search-expansion envelope, and document backlinks.

use crate::dto::search::SearchHit;
use serde::{Deserialize, Serialize};
use skb_core::graph::GraphEdge as CoreGraphEdge;
use skb_core::graph::GraphNode as CoreGraphNode;
use skb_core::graph::GraphQueryRequest as CoreGraphQueryRequest;
use skb_core::graph::GraphQueryResult as CoreGraphQueryResult;
use std::collections::HashMap;
use utoipa::ToSchema;

/// Body of `POST /api/graph/query`: a transparent passthrough. `from` is an
/// entity name or a full record id (`entity:<key>` / `document:<key>`);
/// `depth` (1-5) and `limit` bounds are core-validated.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct GraphQueryRequest {
    pub from: String,
    pub relation: Option<String>,
    /// Hop depth 1-5 (core-validated; out-of-range → 400 E_VALIDATION).
    pub depth: Option<usize>,
    pub limit: Option<usize>,
}

impl From<GraphQueryRequest> for CoreGraphQueryRequest {
    fn from(dto: GraphQueryRequest) -> Self {
        Self {
            from: dto.from,
            relation: dto.relation,
            depth: dto.depth,
            limit: dto.limit,
        }
    }
}

/// Node of the `POST /api/graph/query` result graph.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub depth: usize,
}

impl From<CoreGraphNode> for GraphNode {
    fn from(node: CoreGraphNode) -> Self {
        Self {
            id: node.id,
            name: node.name,
            kind: node.kind,
            depth: node.depth,
        }
    }
}

/// Edge of the `POST /api/graph/query` result graph.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
}

impl From<CoreGraphEdge> for GraphEdge {
    fn from(edge: CoreGraphEdge) -> Self {
        Self {
            from: edge.from,
            to: edge.to,
            relation: edge.relation,
        }
    }
}

/// Body of the `POST /api/graph/query` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GraphQueryResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl From<CoreGraphQueryResult> for GraphQueryResult {
    fn from(result: CoreGraphQueryResult) -> Self {
        Self {
            nodes: result.nodes.into_iter().map(Into::into).collect(),
            edges: result.edges.into_iter().map(Into::into).collect(),
        }
    }
}

/// Body of `POST /api/search/expand`: search hits plus the expansion hop
/// depth consumed by core's `expand_search_hits`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ExpandRequest {
    pub hits: Vec<SearchHit>,
    pub max_expand: usize,
}

/// Body of the `POST /api/search/expand` response. `entity_origins` maps the
/// `"<document_id>/<chunk_idx>"` key of each ORIGINAL hit to the entities its
/// chunk mentions.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExpandResponse {
    pub hits: Vec<SearchHit>,
    pub entity_origins: HashMap<String, Vec<String>>,
}

/// One backlinking document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BacklinkDocument {
    pub id: String,
    pub title: String,
}

/// Body of the `GET /api/documents/{id}/backlinks` response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BacklinksResponse {
    pub documents: Vec<BacklinkDocument>,
}
