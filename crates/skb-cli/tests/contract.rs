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
        r#"search = {{ rrf_k = 10 }}

[embedding]
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
            mode: Some(mode.parse().unwrap()),
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
        let docs = kb
            .list_documents(&skb_core::crud::ListQuery {
                limit: Some(10),
                offset: Some(0),
                order: None,
            })
            .await
            .unwrap();
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
fn contract_config_set_updates_existing_config() {
    setup_config();
    let output = Command::new(skb_binary())
        .args(["config", "set", "search.rrf_k", "42"])
        .current_dir(test_dir())
        .output()
        .expect("failed to run skb config set");
    assert!(output.status.success());

    let config = std::fs::read_to_string(test_dir().join("skb.toml")).unwrap();
    assert!(config.contains("search = { rrf_k = 42 }"));
    let shown = run_skb(&["config", "show"], None);
    assert_eq!(shown["search"]["rrf_k"], 42);
}

#[test]
fn contract_config_env_override() {
    setup_config();
    let output = Command::new(skb_binary())
        .args(["config", "show"])
        .env("SKB_SEARCH_TOP_K", "42")
        .current_dir(test_dir())
        .output()
        .expect("failed to run skb config show");
    assert!(output.status.success());
    let val: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(val["search"]["top_k"], 42);
}

#[test]
fn contract_upload_partial_failure() {
    setup_config();
    let dir = test_dir();
    let docs_dir = dir.join("docs");
    std::fs::create_dir_all(&docs_dir).unwrap();
    std::fs::write(docs_dir.join("good.md"), "# Good\n\ncontent").unwrap();
    // PNG magic bytes: not UTF-8, not a supported format.
    std::fs::write(
        docs_dir.join("blob.bin"),
        [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a],
    )
    .unwrap();

    // Partial failure must exit non-zero while still reporting the committed
    // results and the aggregated errors on stdout.
    let output = Command::new(skb_binary())
        .args(["upload", docs_dir.to_str().unwrap(), "--recursive"])
        .current_dir(test_dir())
        .output()
        .expect("failed to run skb upload");
    assert_ne!(
        output.status.code(),
        Some(0),
        "partial failure must exit non-zero"
    );
    let val: Value = serde_json::from_slice(&output.stdout).unwrap();

    let results = val["results"].as_array().unwrap();
    let errors = val["errors"].as_array().unwrap();
    assert_eq!(results.len(), 1, "good file must be committed");
    assert_eq!(errors.len(), 1, "bad file must be reported");
    assert_eq!(results[0]["status"], "created");
    assert_eq!(errors[0]["error"], "E_UNSUPPORTED_FORMAT");
    assert_eq!(
        errors[0]["input"],
        docs_dir.join("blob.bin").display().to_string()
    );
}

#[test]
fn contract_search_response_fields() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "fields-test"],
        Some("highlighted query words with zzzkw token"),
    );

    let search = run_skb(&["search", "zzzkw", "--mode", "keyword"], None);
    let hit = &search["hits"][0];
    assert_eq!(hit["title"], "fields-test");
    assert!(hit["source"].is_string());
    let hl = hit["highlights"].as_array().unwrap();
    assert!(hl.iter().any(|v| v == "zzzkw"));

    let vec = run_skb(&["search", "zzzkw", "--mode", "vector"], None);
    assert!(vec["hits"][0]["title"].is_string());
    assert!(vec["hits"][0]["highlights"].is_null());
}

#[test]
fn contract_list_chunk_count_and_delete_counts() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "counts-test"],
        Some(TEST_DATA),
    );

    let list = run_skb(&["list"], None);
    assert_eq!(list[0]["title"], "counts-test");
    assert!(
        list[0]["chunk_count"].as_u64().unwrap() >= 1,
        "chunk_count must be populated"
    );
    let doc_id = list[0]["id"].as_str().unwrap().to_string();

    let del = run_skb(&["delete", &doc_id, "--yes"], None);
    assert_eq!(del["document_id"], doc_id);
    assert!(
        del["chunks_deleted"].as_u64().unwrap() >= 1,
        "chunks_deleted must be populated"
    );

    // Deleting again: E_DOCUMENT_NOT_FOUND with exit code 6.
    let output = Command::new(skb_binary())
        .args(["delete", &doc_id, "--yes"])
        .current_dir(test_dir())
        .output()
        .expect("failed to run skb delete");
    assert_eq!(output.status.code(), Some(6));
    let val: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(val["error"], "E_DOCUMENT_NOT_FOUND");
}

#[test]
fn contract_query_command() {
    setup_config();
    run_skb(
        &["upload", "--stdin", "--title", "query-test"],
        Some(TEST_DATA),
    );

    let val = run_skb(
        &["query", "SELECT count() AS c FROM document GROUP ALL"],
        None,
    );
    let statements = val["statements"].as_array().unwrap();
    assert_eq!(statements.len(), 1);
    assert!(statements[0][0]["c"].as_u64().unwrap() >= 1);
}

#[test]
fn contract_doctor_json() {
    setup_config();
    let val = run_skb(&["doctor"], None);
    assert_eq!(val["db_connected"], true);
    assert_eq!(val["embedding_dimension"], 8);
    assert!(val["errors"].as_array().unwrap().is_empty());
}

#[test]
fn contract_upload_glob_and_multiple_paths() {
    setup_config();
    let dir = test_dir();
    let docs = dir.join("globdocs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("a.md"), "# A\n\ncontent a").unwrap();
    std::fs::write(docs.join("b.md"), "# B\n\ncontent b").unwrap();
    std::fs::write(docs.join("c.txt"), "plain c").unwrap();

    let pattern = format!("{}/*.md", docs.display());
    let val = run_skb(&["upload", &pattern], None);
    assert_eq!(
        val.as_array().unwrap().len(),
        2,
        "glob must match both md files"
    );

    let val = run_skb(&["upload", docs.join("c.txt").to_str().unwrap()], None);
    assert_eq!(
        val.as_array().unwrap().len(),
        1,
        "multiple/positional paths"
    );

    let list = run_skb(&["list"], None);
    assert_eq!(list.as_array().unwrap().len(), 3);
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
