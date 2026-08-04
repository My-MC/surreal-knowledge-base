use anyhow::Result;
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
use serde_json::{json, Value};
use skb_core::config::Config;
use skb_core::graph::{EntityInfo, GraphQueryRequest, LinkInfo};
use skb_core::ingest::UploadRequest;
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
        let tools = vec![
            tool(
                "skb_upload",
                "Upload document (path, url, content, or content_base64)",
                &[
                    ("path", "string"),
                    ("url", "string"),
                    ("content", "string"),
                    ("content_base64", "string"),
                    ("title", "string"),
                    ("tags", "string"),
                    ("metadata", "object"),
                    ("force", "boolean"),
                ],
            ),
            tool(
                "skb_search",
                "Search documents (hybrid, vector, or keyword)",
                &[
                    ("query", "string"),
                    ("mode", "string"),
                    ("top_k", "integer"),
                    ("graph_expand", "integer"),
                    ("filter", "object"),
                ],
            ),
            tool(
                "skb_list_documents",
                "List all documents",
                &[
                    ("limit", "integer"),
                    ("offset", "integer"),
                    ("order", "string"),
                ],
            ),
            tool(
                "skb_get_document",
                "Get document details",
                &[("id", "string"), ("include_chunks", "boolean")],
            ),
            tool(
                "skb_delete_document",
                "Delete a document",
                &[("id", "string")],
            ),
            tool("skb_stats", "Show statistics", &[]),
            tool(
                "skb_graph_query",
                "Query knowledge graph",
                &[
                    ("from", "string"),
                    ("relation", "string"),
                    ("depth", "integer"),
                    ("limit", "integer"),
                ],
            ),
            tool(
                "skb_graph_upsert_entity",
                "Create or update entity",
                &[
                    ("name", "string"),
                    ("kind", "string"),
                    ("description", "string"),
                ],
            ),
            tool(
                "skb_graph_link",
                "Link two entities",
                &[
                    ("from", "string"),
                    ("to", "string"),
                    ("relation", "string"),
                    ("weight", "number"),
                ],
            ),
            tool(
                "skb_reindex",
                "Reindex all documents",
                &[("dry_run", "boolean")],
            ),
        ];
        Ok(ListToolsResult::with_all_items(tools))
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
            .with_description("A single document body and its chunks, by id")];
        Ok(ListResourceTemplatesResult::with_all_items(templates))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        let kb = self.kb.lock().await;
        let uri = request.uri.clone();
        let contents: Vec<ResourceContents> = if uri == "skb://documents" {
            let docs = kb.list_documents(100, 0, None).await.map_err(err_data)?;
            let body = serde_json::to_string_pretty(&docs)
                .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            vec![ResourceContents::text(body, uri)]
        } else if uri == "skb://stats" {
            let stats = kb.stats().await.map_err(err_data)?;
            let body = serde_json::to_string_pretty(&stats)
                .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;
            vec![ResourceContents::text(body, uri)]
        } else if let Some(id) = uri.strip_prefix("skb://documents/") {
            let doc = kb
                .get_document(id, true)
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

fn tool(
    name: &'static str,
    desc: &'static str,
    params: &[(&'static str, &'static str)],
) -> ToolDef {
    let mut props = serde_json::Map::new();
    for (pname, ptype) in params {
        props.insert(pname.to_string(), json!({"type": *ptype}));
    }
    let required: Vec<Value> = if params.len() <= 1 {
        params.iter().map(|(n, _)| json!(n)).collect()
    } else {
        vec![]
    };
    let input_schema = json!({
        "type": "object",
        "properties": props,
        "required": required,
    });
    let input_schema =
        serde_json::from_value::<rmcp::model::JsonObject>(input_schema).unwrap_or_default();
    ToolDef::new(name, desc, input_schema)
}

impl SkbServer {
    async fn handle_tool(&self, req: CallToolRequestParams) -> Result<Value, String> {
        let args = req.arguments.unwrap_or_default();
        let kb = self.kb.lock().await;

        match req.name.as_ref() {
            "skb_upload" => {
                let has_source = [
                    args.get("path"),
                    args.get("url"),
                    args.get("content"),
                    args.get("content_base64"),
                ]
                .iter()
                .any(|v| v.is_some());
                if !has_source {
                    return Err(valid_err(
                        "skb_upload requires one of: path, url, content, content_base64",
                    ));
                }
                let params: UploadRequest =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.upload(params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_search" => {
                if !args.contains_key("query") {
                    return Err(valid_err("skb_search requires 'query'"));
                }
                let params: SearchRequest =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.search(params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_list_documents" => {
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let order = args
                    .get("order")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                kb.list_documents(limit, offset, order)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_get_document" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    return Err(valid_err("skb_get_document requires 'id'"));
                }
                let include_chunks = args
                    .get("include_chunks")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                kb.get_document(id, include_chunks)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_delete_document" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    return Err(valid_err("skb_delete_document requires 'id'"));
                }
                kb.delete_document(id)
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
                let params: GraphQueryRequest =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.graph_query(&params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_graph_upsert_entity" => {
                let params: EntityInfo =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.upsert_entity(&params)
                    .await
                    .map(|_| json!({"status": "ok"}))
                    .map_err(|e| format!("{e}"))
            }
            "skb_graph_link" => {
                let params: LinkInfo =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.link_entities(&params)
                    .await
                    .map(|_| json!({"status": "ok"}))
                    .map_err(|e| format!("{e}"))
            }
            "skb_reindex" => {
                let dry_run = args
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let req = skb_core::reindex::ReindexRequest { dry_run };
                kb.reindex(&req)
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

    let config = Config::load().unwrap_or_default();
    let kb = KnowledgeBase::open(config).await?;
    let server = SkbServer::new(kb);

    tracing::info!("MCP server starting (stdio)");

    let running = serve_server(server, stdio()).await?;
    running.waiting().await?;

    Ok(())
}
