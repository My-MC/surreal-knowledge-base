use anyhow::Result;
use clap::{Parser, Subcommand};
use skb_core::config::Config;
use skb_core::graph::{EntityInfo, GraphQueryRequest, LinkInfo};
use skb_core::ingest::UploadRequest;
use skb_core::search::SearchRequest;
use skb_core::KnowledgeBase;
use std::io::Read;

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
    },
    /// List documents
    List {
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
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
    Reindex,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "skb=info,warn".into()))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let fmt = cli.format.clone();

    match &cli.command {
        Commands::List { limit, offset } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let docs = kb.list_documents(*limit, *offset).await?;
            output(&docs, &fmt)?;
        }
        Commands::Get { id, chunks } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let doc = kb.get_document(id, *chunks).await?;
            output(&doc, &fmt)?;
        }
        Commands::Delete { id, yes } => {
            if !yes {
                anyhow::bail!("use --yes to confirm deletion of {id}");
            }
            let kb = KnowledgeBase::open(cfg()?).await?;
            let result = kb.delete_document(id).await?;
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
        } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let req = if *stdin {
                let mut content = String::new();
                std::io::stdin().read_to_string(&mut content)?;
                UploadRequest {
                    path: None,
                    url: None,
                    content_base64: None,
                    content: Some(content),
                    title: title.clone(),
                    tags: tags.clone(),
                    metadata: None,
                    force: Some(*force),
                }
            } else {
                UploadRequest {
                    path: path.clone(),
                    url: url.clone(),
                    content: None,
                    content_base64: None,
                    title: title.clone(),
                    tags: tags.clone(),
                    metadata: None,
                    force: Some(*force),
                }
            };
            let result = kb.upload(req).await?;
            output(&result, &fmt)?;
        }
        Commands::Search {
            query,
            mode,
            top_k,
            graph_expand,
        } => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let req = SearchRequest {
                query: query.clone(),
                mode: Some(mode.clone()),
                top_k: Some(*top_k),
                graph_expand: *graph_expand,
                filter: None,
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
        Commands::Reindex => {
            let kb = KnowledgeBase::open(cfg()?).await?;
            let result = kb.reindex().await?;
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
        },
    }

    Ok(())
}

fn cfg() -> Result<Config> {
    Config::load().or_else(|_| Ok(Config::default()))
}
