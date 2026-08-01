use anyhow::Result;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool as ToolDef,
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
                ],
            ),
            tool(
                "skb_list_documents",
                "List all documents",
                &[("limit", "integer"), ("offset", "integer")],
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
            tool("skb_reindex", "Reindex all documents", &[]),
        ];
        Ok(ListToolsResult::with_all_items(tools))
    }

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
                let params: UploadRequest =
                    serde_json::from_value(Value::Object(args)).map_err(|e| format!("{e}"))?;
                kb.upload(params)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_search" => {
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
                kb.list_documents(limit, offset)
                    .await
                    .map(|r| serde_json::to_value(r).unwrap_or_default())
                    .map_err(|e| format!("{e}"))
            }
            "skb_get_document" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
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
            "skb_reindex" => kb
                .reindex()
                .await
                .map(|r| serde_json::to_value(r).unwrap_or_default())
                .map_err(|e| format!("{e}")),
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
