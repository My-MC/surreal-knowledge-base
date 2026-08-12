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
    /// Upload a document
    Upload {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, conflicts_with = "path")]
        url: Option<String>,
        #[arg(long, conflicts_with_all = ["path", "url"])]
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
        #[arg(long, conflicts_with_all = ["path", "url"], requires = "stdin", help = "read base64-encoded content from stdin")]
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
        Ok(code) => std::process::ExitCode::from(code),
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
                "error": code.unwrap_or_else(|| "E_IO".to_string()),
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

async fn run(cli: &Cli) -> Result<u8> {
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
            // A URL input takes precedence: `--url --recursive` (no --path)
            // follows the single-URL upload flow instead of erroring.
            let mut paths: Vec<String> = Vec::new();
            if *recursive && url.is_none() {
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
                // Bound stdin reads by upload.max_file_mb (spec §12.3).
                // Base64 input can be up to 4/3 of the decoded size, so the
                // read cap is scaled for the base64 branch; the byte-length
                // validation below still rejects any input whose DECODED size
                // would exceed max.
                let max = kb.config().upload.max_file_mb.saturating_mul(1024 * 1024);
                let read_cap = if *base64 {
                    max.saturating_mul(4).saturating_div(3).saturating_add(1)
                } else {
                    max.saturating_add(1)
                };
                let mut raw = Vec::new();
                std::io::stdin().take(read_cap).read_to_end(&mut raw)?;
                if raw.len() as u64 > max {
                    anyhow::bail!("stdin exceeds upload.max_file_mb");
                }
                let content = String::from_utf8(raw)?;
                let result = if *base64 {
                    kb.upload(build(None, None, Some(content))).await?
                } else {
                    kb.upload(build(None, Some(content), None)).await?
                };
                output(&result, &fmt)?;
            } else if paths.len() > 1 {
                // Multi-input uploads: successful uploads are committed and
                // returned in `results`, failures are aggregated in `errors`
                // (spec §12.3). A single input keeps the direct UploadResult
                // shape with top-level document_id/status fields.
                let mut results: Vec<serde_json::Value> = Vec::new();
                let mut errors: Vec<serde_json::Value> = Vec::new();
                for p in paths {
                    match kb.upload(build(Some(p.clone()), None, None)).await {
                        Ok(result) => results.push(serde_json::to_value(result)?),
                        Err(e) => errors.push(serde_json::json!({
                            "input": p,
                            "error": skb_core::error::ErrorCode::from_std(&e)
                                .map(|c| c.code_str().to_string())
                                .unwrap_or_else(|| "E_IO".to_string()),
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
                    // without emitting a second error document. Returning the
                    // code (rather than std::process::exit) lets the embedded
                    // SurrealKv connection drop and flush normally.
                    let _ = std::io::stdout().flush();
                    return Ok(1);
                }
            } else if let Some(p) = paths.first() {
                // Single collected input: use the discovered file (recursive
                // expansions with exactly one file included), never the
                // original directory path.
                let result = kb.upload(build(Some(p.clone()), None, None)).await?;
                output(&result, &fmt)?;
            } else if url.is_some() {
                // URL-only upload: no path input; build with an empty path so
                // the URL is preserved and upload proceeds.
                let result = kb.upload(build(None, None, None)).await?;
                output(&result, &fmt)?;
            } else {
                anyhow::bail!("no files to upload");
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
            // open_or_for_reindex retries transient file-lock races from the
            // first failed open and falls back to open_for_reindex on mismatch.
            let config = cfg()?;
            let kb = skb_core::KnowledgeBase::open_or_for_reindex(config).await?;
            let req = skb_core::reindex::ReindexRequest { dry_run: *dry_run };
            // Live \r-based progress only on a terminal; for piped stderr
            // (CI logs) suppress intermediate updates. Track whether any TTY
            // progress was actually emitted to decide on the trailing
            // newline.
            let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
            let emitted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let emitted_cb = emitted.clone();
            let progress = move |done: usize, total: usize| {
                if stderr_tty {
                    emitted_cb.store(true, std::sync::atomic::Ordering::Relaxed);
                    eprint!("\rreindexed {done}/{total}");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
            };
            let result = kb.reindex(&req, Some(&progress)).await?;
            if emitted.load(std::sync::atomic::Ordering::Relaxed) {
                eprintln!();
            } else if !*dry_run {
                eprintln!("reindexed {} documents", result.documents_processed);
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

    Ok(0)
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
    // Map configuration-loading failures (TOML parse, invalid SKB_* numeric
    // values, missing/invalid file) to E_CONFIG so the CLI reports the
    // correct error code and exit status.
    Config::load().map_err(|e| {
        let err = skb_core::error::SkbError::new(
            skb_core::error::ErrorCode::Config,
            format!("failed to load config: {e:#}"),
        );
        anyhow::Error::from(err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn upload_base64_requires_stdin() {
        assert!(Cli::try_parse_from(["skb", "upload", "--base64"]).is_err());
        assert!(Cli::try_parse_from(["skb", "upload", "--base64", "--path", "a.md"]).is_err());
        assert!(
            Cli::try_parse_from(["skb", "upload", "--base64", "--url", "https://x.example/a"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["skb", "upload", "--base64", "--stdin"]).is_ok());
    }

    #[test]
    fn upload_rejects_multiple_input_sources() {
        assert!(Cli::try_parse_from(["skb", "upload", "--path", "a.md", "--stdin"]).is_err());
        assert!(Cli::try_parse_from([
            "skb",
            "upload",
            "--path",
            "a.md",
            "--url",
            "https://x.example/a"
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["skb", "upload", "--url", "https://x.example/a", "--stdin"])
                .is_err()
        );
    }
}
