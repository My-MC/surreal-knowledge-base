use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use skb_core::config::Config;
use skb_core::embed::{Embed, MockEmbedder};
use skb_core::ingest::UploadRequest;
use skb_core::search::SearchRequest;
use skb_core::tokenize::{Tokenize, TokenizersImpl};
use skb_core::KnowledgeBase;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[cfg(feature = "ort")]
use skb_core::config::EmbeddingConfig;
#[cfg(feature = "ort")]
use std::sync::Arc;

const EN_PARAGRAPH: &str =
    "SurrealDB is a multi-model database that combines documents, graphs, and key-value stores. \
     It supports SurrealQL for querying, BM25 for full-text search, and HNSW for vector \
     similarity search. The embedded mode uses SurrealKV storage with ACID transactions.\n";

const JA_PARAGRAPH: &str =
    "SurrealDBはドキュメント、グラフ、キーバリューストアを統合したマルチモデルデータベースです。\
     SurrealQLクエリ言語、BM25全文検索、HNSWベクトル類似検索をサポートします。\
     エンベデッドモードではSurrealKVストレージエンジンとACIDトランザクションを使用します。\n";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn bench_db_path(prefix: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    workspace_root()
        .join("target")
        .join(format!("skb-bench-{prefix}-{n}"))
}

fn generate_text(target_bytes: usize) -> String {
    let mut s = String::with_capacity(target_bytes);
    let mut i = 0;
    while s.len() < target_bytes {
        s.push_str(&format!("[{i}] {EN_PARAGRAPH}{JA_PARAGRAPH}"));
        i += 1;
    }
    s.truncate(target_bytes);
    s
}

fn bench_tokenizer() -> TokenizersImpl {
    let client = hf_hub::HFClientSync::new().expect("hf-hub client");
    let repo = client.model("BAAI", "bge-m3");
    let path = repo
        .download_file()
        .filename("tokenizer.json")
        .send()
        .expect("download bge-m3 tokenizer.json");
    TokenizersImpl::from_path(&path).expect("load tokenizer")
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
}

fn bench_tokenize(c: &mut Criterion) {
    let tokenizer = bench_tokenizer();
    let text_1mb = generate_text(1024 * 1024);
    let text_10mb = generate_text(10 * 1024 * 1024);

    let mut group = c.benchmark_group("tokenize");
    group.measurement_time(Duration::from_secs(10));

    group.throughput(Throughput::Bytes(text_1mb.len() as u64));
    group.bench_function("encode_1mb", |b| {
        b.iter(|| tokenizer.encode(&text_1mb).unwrap());
    });

    group.throughput(Throughput::Bytes(text_10mb.len() as u64));
    group.sample_size(15);
    group.bench_function("encode_10mb", |b| {
        b.iter(|| tokenizer.encode(&text_10mb).unwrap());
    });

    group.throughput(Throughput::Bytes(text_1mb.len() as u64));
    group.bench_function("chunk_1mb_512_64", |b| {
        b.iter(|| tokenizer.chunk(&text_1mb, 512, 64).unwrap());
    });

    group.throughput(Throughput::Bytes(text_10mb.len() as u64));
    group.sample_size(10);
    group.bench_function("chunk_10mb_512_64", |b| {
        b.iter(|| tokenizer.chunk(&text_10mb, 512, 64).unwrap());
    });

    group.finish();
}

fn bench_embed(c: &mut Criterion) {
    let mock = MockEmbedder { dimension: 1024 };
    let texts: Vec<String> = (0..32)
        .map(|i| generate_text(2000).replace("[0]", &format!("[batch_{i}]")))
        .collect();

    let mut group = c.benchmark_group("embed");

    group.throughput(Throughput::Elements(32));
    group.bench_function("mock_batch32", |b| {
        b.iter(|| mock.embed_batch(&texts).unwrap());
    });

    #[cfg(feature = "ort")]
    {
        let tokenizer = bench_tokenizer();
        let tokenizer: Arc<dyn Tokenize> = Arc::new(tokenizer);
        let emb_config = EmbeddingConfig {
            model: "BAAI/bge-m3".into(),
            onnx_path: "auto".into(),
            tokenizer: "auto".into(),
            dimension: 0,
            max_input_tokens: 0,
            device: "cpu".into(),
            batch_size: 32,
        };
        let ort = skb_core::embed::ort_embedder::OrtEmbedder::load(&emb_config, tokenizer).ok();
        if let Some(ort) = ort {
            let ort_texts: Vec<String> = (0..32)
                .map(|i| {
                    // ~500 tokens of mixed EN/JA
                    EN_PARAGRAPH.repeat(3) + &JA_PARAGRAPH.repeat(3) + &format!(" batch_{i} ")
                })
                .collect();

            group.throughput(Throughput::Elements(32));
            group.sample_size(20);
            group.bench_function("ort_bge_m3_batch32", |b| {
                b.iter(|| ort.embed_batch(&ort_texts).unwrap());
            });
        } else {
            eprintln!("ort: failed to load OrtEmbedder (model download failed?), skipping");
        }
    }

    group.finish();
}

fn bench_search(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let chunk_count = std::env::var("SKB_BENCH_CHUNKS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1_000);

    let db_path = bench_db_path("search");
    let _ = std::fs::remove_dir_all(&db_path);

    let kb = rt.block_on(async {
        let mut c = Config::default();
        c.embedding.onnx_path = "mock".to_string();
        c.embedding.dimension = 8;
        c.storage.path = db_path.clone();
        KnowledgeBase::open(c).await.unwrap()
    });

    eprintln!(
        "search bench: seeding {} chunks into {}",
        chunk_count,
        db_path.display()
    );
    let mut uploaded = 0;
    while uploaded < chunk_count {
        let batch = 200.min(chunk_count - uploaded);
        for i in 0..batch {
            let idx = uploaded + i;
            let content = format!(
                "[doc_{idx}]\n\
                 SurrealDB multi-model database with embedded SurrealKV. HNSW vector search and \
                 BM25 full-text ranking combined via RRF. Knowledge graph with entity extraction.\n\
                 ベクトル検索HNSWインデックス BM25全文検索キーワードマッチ RRF再ランキング。\
                 マルチモデルデータベースSurrealDB エンベデッドSurrealKV 知識グラフ。\n\
                 ACID transactions. Schema-full and schema-less tables. Record links and graph edges.\n\
                 ACIDトランザクション スキーマ管理 レコードリンクとグラフエッジ。",
            );
            rt.block_on(kb.upload(UploadRequest {
                path: None,
                url: None,
                content: Some(content),
                content_base64: None,
                title: Some(format!("bench-{idx}")),
                tags: Some(vec!["bench".into()]),
                metadata: None,
                force: None,
            }))
            .unwrap();
        }
        uploaded += batch;
        if uploaded % 1000 == 0 {
            eprintln!("  seeded {} / {}", uploaded, chunk_count);
        }
    }
    eprintln!("search bench: seed complete ({} docs)", uploaded);

    let mut group = c.benchmark_group("search");
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(10));

    for mode in &["vector", "keyword", "hybrid"] {
        let req = SearchRequest {
            query: "database vector search".into(),
            mode: Some(mode.to_string()),
            top_k: Some(10),
            graph_expand: None,
            filter: None,
        };
        group.bench_with_input(
            BenchmarkId::new(*mode, format!("{}chunks", chunk_count)),
            &req,
            |b, req| {
                b.iter(|| {
                    rt.block_on(kb.search(req.clone())).unwrap();
                });
            },
        );
    }

    group.finish();
    drop(kb);
    cleanup(&db_path);
}

fn bench_mcp_startup(c: &mut Criterion) {
    let bin = {
        let debug = workspace_root().join("target/debug/skb-mcp");
        let release = workspace_root().join("target/release/skb-mcp");
        if debug.exists() {
            debug
        } else if release.exists() {
            release
        } else {
            eprintln!(
                "skb-mcp binary not found at {:?} or {:?}, skipping mcp_startup bench.\n\
                 Build with: cargo build -p skb-mcp",
                debug, release
            );
            return;
        }
    };

    let work_dir = bench_db_path("mcp");
    let db_dir = work_dir.join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(
        work_dir.join("skb.toml"),
        format!(
            "[storage]\npath = \"{}\"\n\n[embedding]\nonnx_path = \"mock\"\ndimension = 8\n",
            db_dir.display()
        ),
    )
    .unwrap();

    let init_req = format!(
        "{}\n",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "bench", "version": "1.0"}
            }
        })
    );

    eprintln!(
        "mcp_startup bench: spawning {} from {}",
        bin.display(),
        work_dir.display()
    );

    let mut group = c.benchmark_group("mcp_startup");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));
    group.bench_function("cold_start", |b| {
        b.iter(|| {
            let mut child = Command::new(&bin)
                .current_dir(&work_dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn skb-mcp");

            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(init_req.as_bytes()).unwrap();
            drop(stdin);

            let stdout = child.stdout.take().expect("stdout");
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read init response");

            assert!(
                line.contains("protocolVersion"),
                "unexpected init response: {}",
                line
            );

            child.kill().ok();
            let _ = child.wait();
        });
    });

    group.finish();
    cleanup(&work_dir);
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(10));
    targets = bench_tokenize, bench_embed, bench_search, bench_mcp_startup
}
criterion_main!(benches);
