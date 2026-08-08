use anyhow::Result;
use clap::{Parser, Subcommand};
use skb_core::config::Config;
use skb_core::crud::{DeleteDocumentRequest, GetDocumentRequest, ListQuery, OrderBy};
use skb_core::graph::{EntityInfo, GraphQueryRequest, LinkInfo};
use skb_core::ingest::UploadRequest;
use skb_core::search::SearchRequest;
use skb_core::KnowledgeBase;
use std::collections::HashMap;
use std::io::Read;
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
    /// Upload a document
    Upload {
        #[arg(long)]
        path: Option<String>,
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
        #[arg(
            long,
            help = "hybrid|vector|keyword (default: config search.default_mode)"
        )]
        mode: Option<String>,
        #[arg(long, help = "number of hits (default: config search.top_k)")]
        top_k: Option<usize>,
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
            println!("{report}");
        }
        Commands::Upload {
            path,
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

            // Expand a directory path into individual files when --recursive.
            let mut paths: Vec<String> = Vec::new();
            if *recursive {
                if let Some(p) = path {
                    let p = std::path::Path::new(p);
                    if p.is_dir() {
                        for entry in collect_files(p)? {
                            paths.push(entry.display().to_string());
                        }
                    } else {
                        paths.push(p.display().to_string());
                    }
                } else {
                    anyhow::bail!("--recursive requires --path to a directory");
                }
            } else if let Some(p) = path {
                paths.push(p.clone());
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
                let mut content = String::new();
                if *base64 {
                    let mut raw = Vec::new();
                    std::io::stdin().read_to_end(&mut raw)?;
                    content = String::from_utf8(raw)?;
                    let result = kb.upload(build(None, None, Some(content))).await?;
                    output(&result, &fmt)?;
                } else {
                    std::io::stdin().read_to_string(&mut content)?;
                    let result = kb.upload(build(None, Some(content), None)).await?;
                    output(&result, &fmt)?;
                }
            } else if !paths.is_empty() {
                let mut results = Vec::new();
                for p in paths {
                    results.push(kb.upload(build(Some(p), None, None)).await?);
                }
                output(&results, &fmt)?;
            } else {
                let result = kb.upload(build(path.clone(), None, None)).await?;
                output(&result, &fmt)?;
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
                mode: mode.as_deref().map(str::parse).transpose()?,
                top_k: *top_k,
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
            let kb = KnowledgeBase::open(cfg()?).await?;
            let req = skb_core::reindex::ReindexRequest { dry_run: *dry_run };
            let result = kb.reindex(&req).await?;
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
    // Errors (invalid SKB_* env values, unreadable/ malformed config files)
    // must surface instead of silently falling back to defaults.
    Config::load().map_err(|e| anyhow::anyhow!("{e:#}"))
}
