// Contract tests: verify CLI and core API produce compatible JSON output

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn skb_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/skb");
    path
}

fn test_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/contract-test");
    path
}

fn setup_config() {
    let dir = test_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let db_path = dir.join("db");
    let config = format!(
        r#"[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "{}"
"#,
        db_path.display()
    );
    std::fs::write(dir.join("skb.toml"), config).unwrap();
}

fn run_skb(args: &[&str], stdin_data: Option<&str>) -> Value {
    let dir = test_dir();
    let mut cmd = Command::new(skb_binary());
    cmd.args(args)
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = if let Some(data) = stdin_data {
        use std::io::Write;
        cmd.stdin(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn skb");
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data.as_bytes()).unwrap();
        }
        child.wait_with_output().expect("failed to wait on skb")
    } else {
        cmd.output().expect("failed to run skb binary")
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        panic!(
            "skb {} failed (exit {}):\nstdout: {stdout}\nstderr: {stderr}",
            args.join(" "),
            output.status
        );
    }

    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "skb {} produced non-JSON output:\n{stdout}\nError: {e}",
            args.join(" ")
        )
    })
}

fn core_search(query: &str, mode: &str) -> Value {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = test_dir();
    let mut config = skb_core::config::Config::default();
    config.embedding.onnx_path = "mock".to_string();
    config.embedding.dimension = 8;
    config.storage.path = dir.join("db");
    rt.block_on(async {
        let kb = skb_core::KnowledgeBase::open(config).await.unwrap();
        let req = skb_core::search::SearchRequest {
            query: query.into(),
            mode: Some(mode.into()),
            top_k: Some(5),
            graph_expand: None,
            filter: None,
        };
        let resp = kb.search(req).await.unwrap();
        serde_json::to_value(resp).unwrap()
    })
}

fn core_list() -> Value {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = test_dir();
    let mut config = skb_core::config::Config::default();
    config.embedding.onnx_path = "mock".to_string();
    config.embedding.dimension = 8;
    config.storage.path = dir.join("db");
    rt.block_on(async {
        let kb = skb_core::KnowledgeBase::open(config).await.unwrap();
        let docs = kb.list_documents(10, 0, None).await.unwrap();
        serde_json::to_value(docs).unwrap()
    })
}

fn core_stats() -> Value {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = test_dir();
    let mut config = skb_core::config::Config::default();
    config.embedding.onnx_path = "mock".to_string();
    config.embedding.dimension = 8;
    config.storage.path = dir.join("db");
    rt.block_on(async {
        let kb = skb_core::KnowledgeBase::open(config).await.unwrap();
        let stats = kb.stats().await.unwrap();
        serde_json::to_value(stats).unwrap()
    })
}

const TEST_DATA: &str = "SurrealDB supports vector search with HNSW and full-text with BM25.";

#[test]
fn contract_upload() {
    setup_config();
    let cli_val = run_skb(
        &["upload", "--stdin", "--title", "contract-test"],
        Some(TEST_DATA),
    );
    assert_eq!(cli_val["status"], "created");
    assert_eq!(cli_val["title"], "contract-test");
    assert!(cli_val["chunks"].as_u64().unwrap() > 0);
}

#[test]
fn contract_search() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "search-test"],
        Some(TEST_DATA),
    );

    let cli_val = run_skb(&["search", "vector", "--mode", "vector"], None);
    let core_val = core_search("vector", "vector");

    assert!(cli_val["hits"].is_array());
    assert!(core_val["hits"].is_array());
    assert_eq!(cli_val["mode"], core_val["mode"]);
}

#[test]
fn contract_list() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "list-test"],
        Some(TEST_DATA),
    );

    let cli_val = run_skb(&["list"], None);
    let core_val = core_list();

    assert!(!cli_val.as_array().unwrap().is_empty());
    assert!(!core_val.as_array().unwrap().is_empty());
}

#[test]
fn contract_stats() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "stats-test"],
        Some(TEST_DATA),
    );

    let cli_val = run_skb(&["stats"], None);
    let core_val = core_stats();

    assert!(cli_val["document_count"].as_u64().unwrap() >= 1);
    assert_eq!(cli_val["document_count"], core_val["document_count"]);
    assert_eq!(cli_val["chunk_count"], core_val["chunk_count"]);
    assert_eq!(
        cli_val["embedding_dimension"],
        core_val["embedding_dimension"]
    );
}

#[test]
fn contract_pipeline() {
    setup_config();

    let up = run_skb(
        &["upload", "--stdin", "--title", "pipeline-test"],
        Some(TEST_DATA),
    );
    assert_eq!(up["status"], "created");
    assert!(up["chunks"].as_u64().unwrap() > 0);

    let search = run_skb(&["search", "SurrealDB", "--mode", "hybrid"], None);
    assert!(!search["hits"].as_array().unwrap().is_empty());

    let list = run_skb(&["list"], None);
    let doc_id = list[0]["id"].as_str().unwrap();

    let get = run_skb(&["get", doc_id], None);
    assert_eq!(get["title"], "pipeline-test");

    let stats = run_skb(&["stats"], None);
    assert!(stats["document_count"].as_u64().unwrap() >= 1);

    let del = run_skb(&["delete", doc_id, "--yes"], None);
    assert_eq!(del["document_id"], doc_id);
}
