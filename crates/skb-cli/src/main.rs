use anyhow::Result;
use clap::{Parser, Subcommand};
use skb_core::config::Config;
use skb_core::crud::{DeleteDocumentRequest, GetDocumentRequest, ListQuery, OrderBy};
use skb_core::graph::{EntityInfo, GraphQueryRequest, LinkInfo};
use skb_core::ingest::UploadRequest;
use skb_core::search::SearchRequest;
use skb_core::KnowledgeBase;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "skb", version, about = "Surreal Knowledge Base CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, default_value = "json")]
    format: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload documents (multiple paths, glob patterns, --recursive for dirs)
    Upload {
        #[arg(help = "files or glob patterns to upload")]
        paths: Vec<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long)]
        force: bool,
        #[arg(long, help = "JSON object of metadata")]
        metadata: Option<String>,
        #[arg(long, help = "upload all files under a directory path")]
        recursive: bool,
        #[arg(long, help = "read base64-encoded content from stdin")]
        base64: bool,
    },
    /// Search documents
    Search {
        query: String,
        #[arg(long, default_value = "hybrid")]
        mode: String,
        #[arg(long, default_value = "10")]
        top_k: usize,
        #[arg(long)]
        graph_expand: Option<usize>,
        #[arg(long, value_delimiter = ',', help = "filter KEY=VALUE (repeatable)")]
        filter: Vec<String>,
    },
    /// List documents
    List {
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, help = "created_desc|created_asc|title_asc|title_desc")]
        order: Option<String>,
    },
    /// Get a document by ID
    Get {
        id: String,
        #[arg(long)]
        chunks: bool,
    },
    /// Delete a document by ID
    Delete {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Show statistics
    Stats,
    /// Graph operations
    Graph {
        #[command(subcommand)]
        cmd: GraphCmd,
    },
    /// Reindex all documents
    Reindex {
        #[arg(long, help = "report what a reindex would do without mutating")]
        dry_run: bool,
    },
    /// Execute raw SurrealQL (advanced; not available via MCP)
    Query { surql: String },
    /// Configuration management
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Run diagnostics
    Doctor,
}

#[derive(Subcommand)]
enum GraphCmd {
    /// Query graph from a node
    Query {
        from: String,
        #[arg(long)]
        relation: Option<String>,
        #[arg(long, default_value = "1")]
        depth: usize,
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Add or update an entity
    Entity {
        name: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Link two entities
    Link {
        from: String,
        to: String,
        #[arg(long)]
        relation: String,
        #[arg(long)]
        weight: Option<f64>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Create default config
    Init,
    /// Show current config
    Show,
    /// Set a config value by dotted key (e.g. storage.path, search.top_k)
    Set { key: String, value: String },
}

fn output(val: &impl serde::Serialize, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(val)?),
        "table" => {
            if let Ok(t) = serde_json::to_string(val) {
                println!("{t}");
            }
        }
        f => anyhow::bail!("unknown format: {f}"),
    }
    Ok(())
}

/// Recursively collect files under `dir` (std-only, no new deps).
fn collect_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        for entry in std::fs::read_dir(&cur)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn main() -> std::process::ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: failed to start tokio runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    rt.block_on(async_main())
}

async fn async_main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "skb=info,warn".into()))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let fmt = cli.format.clone();

    match run(&cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            emit_error(&fmt, &e);
            std::process::ExitCode::from(exit_code_of(&e))
        }
    }
}

/// Print a machine-parseable error (JSON when --format json), then pick the exit
/// code: a `SkbError` declares one via `ErrorCode::exit_code`, anything else is 1.
fn emit_error(fmt: &str, e: &anyhow::Error) {
    if fmt == "json" {
        let code =
            skb_core::error::ErrorCode::from_std(e.as_ref()).map(|c| c.code_str().to_string());
        let msg = format!("{e:#}");
        println!(
            "{}",
            serde_json::json!({
                "error": code.unwrap_or_else(|| "E_INTERNAL".to_string()),
                "message": msg,
            })
        );
    } else {
        eprintln!("Error: {e:#}");
    }
}

fn exit_code_of(e: &anyhow::Error) -> u8 {
    skb_core::error::ErrorCode::from_std(e.as_ref())
        .map(|c| c.exit_code() as u8)
        .unwrap_or(1)
}

async fn run(cli: &Cli) -> Result<()> {
    let fmt = cli.format.clone();
    match &cli.command {
        Commands::List {
            limit,
            offset,
            order,
        } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let order = order.as_deref().map(OrderBy::from_str).transpose()?;
            let docs = kb
                .list_documents(&ListQuery {
                    limit: Some(*limit),
                    offset: Some(*offset),
                    order,
                })
                .await?;
            output(&docs, &fmt)?;
        }
        Commands::Get { id, chunks } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let doc = kb
                .get_document(&GetDocumentRequest {
                    id: id.clone(),
                    include_chunks: Some(*chunks),
                })
                .await?;
            output(&doc, &fmt)?;
        }
        Commands::Delete { id, yes } => {
            if !yes {
                anyhow::bail!("use --yes to confirm deletion of {id}");
            }
            let kb = KnowledgeBase::open(cfg()?).await?;
            let result = kb
                .delete_document(&DeleteDocumentRequest { id: id.clone() })
                .await?;
            output(&result, &fmt)?;
        }
        Commands::Stats => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let stats = kb.stats().await?;
            output(&stats, &fmt)?;
        }
        Commands::Doctor => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let report = kb.doctor().await?;
            if fmt == "json" {
                output(&report, &fmt)?;
            } else {
                println!("=== SKB Doctor ===");
                println!(
                    "DB connection: {}",
                    if report.db_connected {
                        "[OK]"
                    } else {
                        "[FAIL]"
                    }
                );
                println!("Embedding dim: {}", report.embedding_dimension);
                println!("Tokenizer vocab: {}", report.tokenizer_vocab);
                println!("Model: {}", report.model);
                println!("Schema ver: {}", report.schema_version);
                for error in &report.errors {
                    println!("[ERROR] {error}");
                }
                if report.is_healthy() {
                    println!("Status: healthy");
                } else {
                    println!("Status: {} problem(s) found", report.errors.len());
                }
            }
        }
        Commands::Query { surql } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let result = kb.query_surql(surql).await?;
            output(&result, &fmt)?;
        }
        Commands::Upload {
            paths,
            url,
            stdin,
            title,
            tags,
            force,
            metadata,
            recursive,
            base64,
        } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let meta: HashMap<String, String> = metadata
                .as_ref()
                .map(|s| serde_json::from_str::<HashMap<String, String>>(s))
                .transpose()?
                .unwrap_or_default();

            // Expand positional paths: glob patterns, and directories when
            // --recursive (spec §12.2: 複数・glob・--recursive).
            let mut expanded: Vec<String> = Vec::new();
            for pattern in paths {
                if pattern.contains(['*', '?', '[']) {
                    let entries = glob::glob(pattern)
                        .map_err(|e| anyhow::anyhow!("invalid glob '{pattern}': {e}"))?;
                    let mut matched = false;
                    for entry in entries {
                        let path = entry.map_err(|e| anyhow::anyhow!("glob '{pattern}': {e}"))?;
                        matched = true;
                        if *recursive && path.is_dir() {
                            for file in collect_files(&path)? {
                                expanded.push(file.display().to_string());
                            }
                        } else if path.is_file() {
                            expanded.push(path.display().to_string());
                        }
                    }
                    if !matched {
                        anyhow::bail!("no files match '{pattern}'");
                    }
                } else {
                    let path = std::path::Path::new(pattern);
                    if *recursive && path.is_dir() {
                        for file in collect_files(path)? {
                            expanded.push(file.display().to_string());
                        }
                    } else {
                        expanded.push(pattern.clone());
                    }
                }
            }

            let build = |p: Option<String>, c: Option<String>, b64: Option<String>| UploadRequest {
                path: p,
                url: url.clone(),
                content: c,
                content_base64: b64,
                title: title.clone(),
                tags: tags.clone(),
                metadata: Some(meta.clone()),
                force: Some(*force),
            };

            if *stdin {
                // Bound stdin reads by upload.max_file_mb (spec §12.3).
                let max = kb.config().upload.max_file_mb.saturating_mul(1024 * 1024);
                let read_cap = max.saturating_add(1);
                if *base64 {
                    let mut raw = Vec::new();
                    std::io::stdin().take(read_cap).read_to_end(&mut raw)?;
                    if raw.len() as u64 > max {
                        anyhow::bail!("stdin exceeds upload.max_file_mb");
                    }
                    let content = String::from_utf8(raw)?;
                    let result = kb.upload(build(None, None, Some(content))).await?;
                    output(&result, &fmt)?;
                } else {
                    let mut content = String::new();
                    std::io::stdin()
                        .take(read_cap)
                        .read_to_string(&mut content)?;
                    if content.len() as u64 > max {
                        anyhow::bail!("stdin exceeds upload.max_file_mb");
                    }
                    let result = kb.upload(build(None, Some(content), None)).await?;
                    output(&result, &fmt)?;
                }
            } else if expanded.len() > 1 {
                // Multi-input uploads: successful uploads are committed and
                // returned in `results`, failures are aggregated in `errors`
                // (spec §12.3). A single input keeps the direct UploadResult
                // shape with top-level document_id/status fields.
                let mut results: Vec<serde_json::Value> = Vec::new();
                let mut errors: Vec<serde_json::Value> = Vec::new();
                for p in expanded {
                    match kb.upload(build(Some(p.clone()), None, None)).await {
                        Ok(result) => results.push(serde_json::to_value(result)?),
                        Err(e) => errors.push(serde_json::json!({
                            "input": p,
                            "error": skb_core::error::ErrorCode::from_std(&e)
                                .map(|c| c.code_str().to_string())
                                .unwrap_or_else(|| "E_INTERNAL".to_string()),
                            "message": format!("{e:#}"),
                        })),
                    }
                }
                // Multi-input uploads always report {results, errors}; any
                // failure makes the command exit non-zero so callers can
                // detect partial failure (spec §12.3).
                output(
                    &serde_json::json!({ "results": results, "errors": errors }),
                    &fmt,
                )?;
                if !errors.is_empty() {
                    // The JSON payload is already on stdout; exit non-zero
                    // without emitting a second error document.
                    let _ = std::io::stdout().flush();
                    std::process::exit(1);
                }
            } else if url.is_some() {
                // Single URL upload keeps the direct UploadResult shape.
                let result = kb.upload(build(None, None, None)).await?;
                output(&result, &fmt)?;
            } else if expanded.len() == 1 {
                // Single input keeps the direct UploadResult shape.
                let p = expanded.into_iter().next().expect("len == 1");
                let result = kb.upload(build(Some(p), None, None)).await?;
                output(&result, &fmt)?;
            } else {
                anyhow::bail!("no input: provide paths, --url, or --stdin");
            }
        }
        Commands::Search {
            query,
            mode,
            top_k,
            graph_expand,
            filter,
        } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let filter: HashMap<String, String> = filter
                .iter()
                .map(|kv| {
                    kv.split_once('=').map_or_else(
                        || anyhow::bail!("invalid filter '{kv}'; expected KEY=VALUE"),
                        |(k, v)| {
                            if k.is_empty() {
                                anyhow::bail!("invalid filter '{kv}'; key must not be empty")
                            }
                            Ok((k.to_string(), v.to_string()))
                        },
                    )
                })
                .collect::<Result<_, _>>()?;
            let req = SearchRequest {
                query: query.clone(),
                mode: Some(mode.parse()?),
                top_k: Some(*top_k),
                graph_expand: *graph_expand,
                filter: if filter.is_empty() {
                    None
                } else {
                    Some(filter)
                },
            };
            let resp = kb.search(req).await?;
            output(&resp, &fmt)?;
        }
        Commands::Graph { cmd } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            match cmd {
                GraphCmd::Query {
                    from,
                    relation,
                    depth,
                    limit,
                } => {
                    let req = GraphQueryRequest {
                        from: from.clone(),
                        relation: relation.clone(),
                        depth: Some(*depth),
                        limit: Some(*limit),
                    };
                    let result = kb.graph_query(&req).await?;
                    output(&result, &fmt)?;
                }
                GraphCmd::Entity {
                    name,
                    kind,
                    description,
                } => {
                    kb.upsert_entity(&EntityInfo {
                        name: name.clone(),
                        kind: kind.clone(),
                        description: description.clone(),
                    })
                    .await?;
                    println!("Entity '{name}' upserted");
                }
                GraphCmd::Link {
                    from,
                    to,
                    relation,
                    weight,
                } => {
                    kb.link_entities(&LinkInfo {
                        from: from.clone(),
                        to: to.clone(),
                        relation: relation.clone(),
                        weight: *weight,
                    })
                    .await?;
                    println!("Linked '{from}' ->[{relation}]-> '{to}'");
                }
            }
        }
        Commands::Reindex { dry_run } => {
            // A model/dimension/tokenizer mismatch blocks normal open; reindex
            // is the management path out of that state (spec §9-5).
            let config = cfg()?;
            let kb = match KnowledgeBase::open(config.clone()).await {
                Ok(kb) => kb,
                Err(e) if e.code == skb_core::error::ErrorCode::ModelMismatch => {
                    KnowledgeBase::open_for_reindex(config).await?
                }
                Err(e) => return Err(e.into()),
            };
            let req = skb_core::reindex::ReindexRequest { dry_run: *dry_run };
            let progress = |done: usize, total: usize| {
                eprint!("\rreindexed {done}/{total}");
                let _ = std::io::Write::flush(&mut std::io::stderr());
            };
            let result = kb.reindex(&req, Some(&progress)).await?;
            if !*dry_run {
                eprintln!();
            }
            output(&result, &fmt)?;
        }
        Commands::Config { cmd } => match cmd {
            ConfigCmd::Init => {
                let c = Config::default();
                let s = toml::to_string_pretty(&c)?;
                std::fs::write("./skb.toml", s)?;
                println!("Created ./skb.toml with default settings");
            }
            ConfigCmd::Show => {
                let c = cfg()?;
                output(&c, &fmt)?;
            }
            ConfigCmd::Set { key, value } => set_config(key, value)?,
        },
    }

    Ok(())
}

/// `skb config set storage.path './db'`: write a dotted key into the writable
/// config file (./skb.toml or ~/.config/skb/config.toml), preserving other keys.
fn set_config(key: &str, value: &str) -> Result<()> {
    let path = Config::writable_config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(anyhow::anyhow!("read config {}: {e}", path.display())),
    };
    let mut root = if content.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| anyhow::anyhow!("parse config {}: {e}", path.display()))?
    };

    let parts: Vec<&str> = key.trim().split('.').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        anyhow::bail!("invalid key: {key}");
    }

    // Walk (or create) nested tables for all but the last segment.
    let mut cur: &mut dyn toml_edit::TableLike = root.as_table_mut();
    for seg in &parts[..parts.len() - 1] {
        let item = cur
            .entry(seg)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        cur = item
            .as_table_like_mut()
            .ok_or_else(|| anyhow::anyhow!("path segment '{seg}' is not a table"))?;
    }
    let last = *parts.last().unwrap();
    let decor = cur
        .get(last)
        .and_then(|item| item.as_value().map(|value| value.decor().clone()));
    let mut replacement = parse_scalar_item(value);
    if let (Some(decor), Some(value)) = (decor, replacement.as_value_mut()) {
        *value.decor_mut() = decor;
    }
    cur.insert(last, replacement);

    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&path, root.to_string())?;
    println!(
        "Set {key} = {value} in {} (restart to reload; reindex if it changes embedding/chunking).",
        path.display()
    );
    Ok(())
}

fn parse_scalar_item(raw: &str) -> toml_edit::Item {
    if let Ok(b) = raw.parse::<bool>() {
        return toml_edit::value(b);
    }
    if let Ok(i) = raw.parse::<i64>() {
        return toml_edit::value(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return toml_edit::value(f);
    }
    toml_edit::value(raw)
}

fn cfg() -> Result<Config> {
    Config::load().or_else(|_| Ok(Config::default()))
}
