use anyhow::{Context, Result};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    Prompt, PromptArgument, PromptMessage, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ResourceTemplate, Role, ServerCapabilities,
    ServerInfo, Tool as ToolDef,
};
use rmcp::service::serve_server;
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::io::stdio;
use schemars::JsonSchema;
use serde_json::{json, Value};
use skb_core::config::Config;
use skb_core::crud::{DeleteDocumentRequest, GetDocumentRequest, ListQuery};
use skb_core::graph::{EntityInfo, GraphQueryRequest, LinkInfo};
use skb_core::ingest::UploadRequest;
use skb_core::reindex::ReindexRequest;
use skb_core::search::SearchRequest;
use skb_core::KnowledgeBase;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SkbServer {
    kb: Arc<Mutex<KnowledgeBase>>,
}

impl SkbServer {
    pub fn new(kb: KnowledgeBase) -> Self {
        Self {
            kb: Arc::new(Mutex::new(kb)),
        }
    }
}

fn text_content(val: &impl serde::Serialize) -> ContentBlock {
    ContentBlock::text(serde_json::to_string_pretty(val).unwrap_or_else(|e| format!("{e}")))
}

impl ServerHandler for SkbServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("surreal-knowledge-base", "0.1.0")
                .with_title("Surreal Knowledge Base"),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        Ok(ListToolsResult::with_all_items(all_tools()?))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let items = vec![
            Resource::new("skb://documents", "documents")
                .with_description("List of documents in the knowledge base"),
            Resource::new("skb://stats", "stats").with_description("Knowledge base statistics"),
        ];
        Ok(ListResourcesResult::with_all_items(items))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        let templates = vec![ResourceTemplate::new("skb://documents/{id}", "document")
            .with_description(
                "A single document body and its chunks, by id (document:<key>, e.g. skb://documents/document:abc)",
            )];
        Ok(ListResourceTemplatesResult::with_all_items(templates))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        let uri = request.uri.clone();
        let contents: Vec<ResourceContents> = if uri == "skb://documents" {
            // Bound the response so a large store cannot exhaust memory or
            // produce an unbounded payload; report whether the cap was hit.
            // The snapshot pages internally, so the server lock is held for a
            // single call rather than across repeated async fetches; it is
            // released before serialization below.
            const MAX_DOCUMENTS_RESOURCE: usize = 10_000;
            let (docs, truncated) = {
                let kb = self.kb.lock().await;
                kb.document_snapshot(MAX_DOCUMENTS_RESOURCE)
                    .await
                    .map_err(err_data)?
            };
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "documents": docs,
                "truncated": truncated,
                "limit": MAX_DOCUMENTS_RESOURCE,
            }))
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            vec![ResourceContents::text(body, uri)]
        } else if uri == "skb://stats" {
            let kb = self.kb.lock().await;
            let stats = kb.stats().await.map_err(err_data)?;
            let body = serde_json::to_string_pretty(&stats)
                .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            vec![ResourceContents::text(body, uri)]
        } else if let Some(id) = uri.strip_prefix("skb://documents/") {
            let kb = self.kb.lock().await;
            let doc = kb
                .get_document(&GetDocumentRequest {
                    id: id.to_string(),
                    include_chunks: Some(true),
                })
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            let body = serde_json::to_string_pretty(&doc)
                .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            vec![ResourceContents::text(body, uri)]
        } else {
            return Err(rmcp::ErrorData::resource_not_found(uri, None));
        };
        Ok(ReadResourceResponse::from(ReadResourceResult::new(
            contents,
        )))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let prompts = vec![Prompt::new(
            "skb-answer",
            Some("Answer a question grounded in the knowledge base via skb_search."),
            Some(vec![PromptArgument::new("question")
                .with_description("The question to answer")
                .with_required(false)]),
        )];
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<GetPromptResponse, rmcp::ErrorData> {
        let question = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("question"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let user = if question.is_empty() {
            "Answer using the local knowledge base.".to_string()
        } else {
            question
        };
        let messages = vec![PromptMessage::new_text(
            Role::User,
            format!(
                "You are an assistant that answers questions strictly using the user's local \
                 knowledge base. Call the skb_search tool, then cite each result's document_id and \
                 chunk_idx.\n\nQuestion: {user}"
            ),
        )];
        Ok(GetPromptResponse::from(GetPromptResult::new(messages)))
    }

    // ── Tools ──
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let result = self.handle_tool(request).await;
        match result {
            Ok(val) => Ok(CallToolResult::success(vec![text_content(&val)]).into()),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)]).into()),
        }
    }
}

fn err_data(e: skb_core::error::SkbError) -> rmcp::ErrorData {
    match e.code {
        skb_core::error::ErrorCode::Validation => {
            rmcp::ErrorData::invalid_params(e.to_string(), None)
        }
        skb_core::error::ErrorCode::DocumentNotFound => {
            rmcp::ErrorData::resource_not_found(e.to_string(), None)
        }
        _ => rmcp::ErrorData::internal_error(e.to_string(), None),
    }
}

fn valid_err(msg: &str) -> String {
    skb_core::error::SkbError::new(skb_core::error::ErrorCode::Validation, msg.to_string())
        .to_string()
}
/// Build a tool definition whose input schema is generated from a shared
/// `skb-core` DTO (the same type the CLI serializes). Hand-written schemas are
/// intentionally avoided so CLI and MCP can never drift apart. A conversion
/// failure is a programmer error and is surfaced as an MCP protocol error.
fn tool_def(
    name: &'static str,
    desc: &'static str,
    schema: schemars::Schema,
) -> Result<ToolDef, rmcp::ErrorData> {
    let value = schema.to_value();
    let input_schema = serde_json::from_value::<rmcp::model::JsonObject>(value).map_err(|e| {
        rmcp::ErrorData::internal_error(
            format!("internal error: generated schema for '{name}' is not an object: {e}"),
            None,
        )
    })?;
    Ok(ToolDef::new(name, desc, input_schema))
}

fn all_tools() -> Result<Vec<ToolDef>, rmcp::ErrorData> {
    Ok(vec![
        tool_def(
            "skb_upload",
            "Upload a document. Exactly one of path, url, content, content_base64 is required",
            schemars::schema_for!(UploadRequest),
        )?,
        tool_def(
            "skb_search",
            "Search documents (hybrid, vector, or keyword)",
            schemars::schema_for!(SearchRequest),
        )?,
        tool_def(
            "skb_list_documents",
            "List all documents",
            schemars::schema_for!(ListQuery),
        )?,
        tool_def(
            "skb_get_document",
            "Get document details",
            schemars::schema_for!(GetDocumentRequest),
        )?,
        tool_def(
            "skb_delete_document",
            "Delete a document",
            schemars::schema_for!(DeleteDocumentRequest),
        )?,
        tool_def(
            "skb_stats",
            "Show statistics",
            schemars::schema_for!(NoParams),
        )?,
        tool_def(
            "skb_graph_query",
            "Query knowledge graph",
            schemars::schema_for!(GraphQueryRequest),
        )?,
        tool_def(
            "skb_graph_upsert_entity",
            "Create or update entity",
            schemars::schema_for!(EntityInfo),
        )?,
        tool_def(
            "skb_graph_link",
            "Link two entities",
            schemars::schema_for!(LinkInfo),
        )?,
        tool_def(
            "skb_reindex",
            "Reindex all documents",
            schemars::schema_for!(ReindexRequest),
        )?,
    ])
}

/// Degenerate request type for tools without parameters (`skb_stats`).
#[derive(JsonSchema)]
struct NoParams {}

impl SkbServer {
    async fn handle_tool(&self, req: CallToolRequestParams) -> Result<Value, String> {
        let args = req.arguments.unwrap_or_default();
        let kb = self.kb.lock().await;

        match req.name.as_ref() {
            "skb_upload" => {
                let params: UploadRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid upload parameters: {e}")))?;
                kb.upload(params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_search" => {
                let params: SearchRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid search parameters: {e}")))?;
                kb.search(params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_list_documents" => {
                let params: ListQuery = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid list parameters: {e}")))?;
                kb.list_documents(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_get_document" => {
                let params: GetDocumentRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid get parameters: {e}")))?;
                kb.get_document(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_delete_document" => {
                let params: DeleteDocumentRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid delete parameters: {e}")))?;
                kb.delete_document(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_stats" => kb
                .stats()
                .await
                .map(|r| serde_json::to_value(r).unwrap_or_default())
                .map_err(|e| format!("{e}")),
            "skb_graph_query" => {
                let params: GraphQueryRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid graph query parameters: {e}")))?;
                kb.graph_query(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_graph_upsert_entity" => {
                let params: EntityInfo = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid entity parameters: {e}")))?;
                kb.upsert_entity(&params)
                    .await
                    .map(|_| json!({"status": "ok"}))
                    .map_err(|e| format!("{e}"))
            }
            "skb_graph_link" => {
                let params: LinkInfo = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid link parameters: {e}")))?;
                kb.link_entities(&params)
                    .await
                    .map(|_| json!({"status": "ok"}))
                    .map_err(|e| format!("{e}"))
            }
            "skb_reindex" => {
                let params: ReindexRequest = serde_json::from_value(Value::Object(args))
                    .map_err(|e| valid_err(&format!("invalid reindex parameters: {e}")))?;
                kb.reindex(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            name => Err(format!("unknown tool: {name}")),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "skb_mcp=info".into()))
        .with_writer(std::io::stderr)
        .init();

    let config = Config::load().context("failed to load config")?;
    let kb = KnowledgeBase::open(config).await?;
    let server = SkbServer::new(kb);

    tracing::info!("MCP server starting (stdio)");

    let running = serve_server(server, stdio()).await?;
    running.waiting().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_schema(name: &str) -> Value {
        let tools = all_tools().unwrap();
        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("tool {name} not found"));
        serde_json::to_value(&tool.input_schema).unwrap()
    }

    #[test]
    fn every_tool_schema_is_a_valid_object() {
        for tool in all_tools().unwrap() {
            let value = serde_json::to_value(&tool.input_schema).unwrap();
            assert_eq!(value["type"], json!("object"), "tool: {}", tool.name);
            assert!(
                value["properties"].is_object() || value["properties"].is_null(),
                "tool {} must have object properties",
                tool.name
            );
            assert!(
                value["required"].is_array() || value["required"].is_null(),
                "tool: {}",
                tool.name
            );
        }
    }

    #[test]
    fn upload_tool_has_one_of_with_four_sources() {
        let schema = tool_schema("skb_upload");
        let one_of = schema["oneOf"].as_array().expect("oneOf missing");
        assert_eq!(one_of.len(), 4);
        assert!(one_of.iter().any(|e| e["required"] == json!(["path"])));
        assert!(one_of.iter().any(|e| e["required"] == json!(["url"])));
        assert!(one_of.iter().any(|e| e["required"] == json!(["content"])));
        assert!(one_of
            .iter()
            .any(|e| e["required"] == json!(["content_base64"])));
    }

    #[test]
    fn search_tool_requires_query_and_enumerates_mode() {
        let schema = tool_schema("skb_search");
        assert_eq!(schema["required"], json!(["query"]));
        assert_eq!(
            schema["$defs"]["SearchMode"]["enum"],
            json!(["hybrid", "vector", "keyword"])
        );
        assert_eq!(schema["properties"]["top_k"]["minimum"], 1);
    }

    #[test]
    fn graph_query_tool_requires_from_with_depth_range() {
        let schema = tool_schema("skb_graph_query");
        assert_eq!(schema["required"], json!(["from"]));
        assert_eq!(schema["properties"]["depth"]["minimum"], 1);
        assert_eq!(schema["properties"]["depth"]["maximum"], 5);
    }

    #[test]
    fn get_and_delete_tools_require_id() {
        assert_eq!(tool_schema("skb_get_document")["required"], json!(["id"]));
        assert_eq!(
            tool_schema("skb_delete_document")["required"],
            json!(["id"])
        );
    }

    #[test]
    fn list_tool_has_no_required_fields() {
        let schema = tool_schema("skb_list_documents");
        assert!(
            schema["required"].is_null() || schema["required"] == json!([]),
            "list must have no required fields"
        );
    }

    #[test]
    fn reindex_tool_has_optional_dry_run() {
        let schema = tool_schema("skb_reindex");
        assert!(
            schema["required"].is_null() || schema["required"] == json!([]),
            "reindex must have no required fields"
        );
        assert_eq!(schema["properties"]["dry_run"]["type"], json!("boolean"));
    }

    #[test]
    fn reindex_request_defaults_dry_run_to_false() {
        let params: ReindexRequest = serde_json::from_value(json!({})).unwrap();
        assert!(!params.dry_run);
    }

    #[test]
    fn reindex_request_preserves_explicit_dry_run() {
        let params: ReindexRequest = serde_json::from_value(json!({"dry_run": true})).unwrap();
        assert!(params.dry_run);
    }
}
