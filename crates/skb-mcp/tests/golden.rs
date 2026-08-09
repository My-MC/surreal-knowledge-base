// Golden contract tests: the same JSON request must produce semantically
// identical responses through the MCP handler (stdio) and the CLI (spec
// §11.2-3). Each side runs against its own database with identical setup;
// random document ids are normalized away before comparison.
//
// Run via: cargo test --workspace -- --test-threads=1
// (spawns the real target/debug/skb and target/debug/skb-mcp binaries; do not
// run standalone.)

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

fn mcp_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/skb-mcp");
    assert!(
        path.exists(),
        "missing target/debug/skb-mcp; run: cargo test --workspace -- --test-threads=1"
    );
    path
}

fn cli_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/skb");
    assert!(
        path.exists(),
        "missing target/debug/skb; run: cargo test --workspace -- --test-threads=1"
    );
    path
}

fn golden_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/golden-test");
    path
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl McpClient {
    fn spawn(dir: &std::path::Path) -> McpClient {
        let mut child = Command::new(mcp_binary())
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is inherited so a startup crash is visible in CI.
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to spawn skb-mcp (build with: cargo build -p skb-mcp)");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        // Watchdog: if the server stays alive without replying, read_response
        // would block forever; abort the test process after 60s instead of
        // hanging CI (matches the 30s smoke limit with margin). It is
        // cancelled when the client is dropped so multiple sequential clients
        // cannot trip the first watchdog.
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let watch = alive.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60));
            if watch.load(std::sync::atomic::Ordering::SeqCst) {
                eprintln!("golden test watchdog: aborting after 60s (no response from skb-mcp)");
                std::process::abort();
            }
        });
        McpClient {
            child,
            stdin,
            stdout,
            next_id: 1,
            alive,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        self.read_response(id)
    }

    fn read_response(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("failed to read skb-mcp stdout");
            if read == 0 {
                panic!("skb-mcp exited before responding to request {id} (early exit)");
            }
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn initialize(&mut self) {
        let resp = self.send(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "golden-test", "version": "0.0.1"}
            }),
        );
        assert!(resp["result"].is_object(), "initialize failed: {resp}");
        self.notify("notifications/initialized", json!({}));
    }

    /// Call a tool and return the parsed JSON payload of its text content.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, Value> {
        let resp = self.send("tools/call", json!({"name": name, "arguments": arguments}));
        if resp["result"]["isError"] == json!(true) {
            let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
            return Err(json!(text));
        }
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.alive.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn setup_side(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("db").display().to_string().replace('\\', "/");
    let config = format!(
        r#"search = {{ rrf_k = 10 }}

[embedding]
onnx_path = "mock"
dimension = {}

[storage]
path = "{db_path}"
"#,
        skb_core::embed::MOCK_EMBEDDER_DIMENSION,
    );
    std::fs::write(dir.join("skb.toml"), config).unwrap();
    dir
}

fn run_cli(dir: &std::path::Path, args: &[&str], stdin_data: Option<&str>) -> Result<Value, Value> {
    let mut cmd = Command::new(cli_binary());
    cmd.args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = if let Some(data) = stdin_data {
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn skb");
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data.as_bytes()).unwrap();
        }
        child.wait_with_output().expect("failed to wait on skb")
    } else {
        cmd.output().expect("failed to run skb binary")
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        serde_json::from_str(&stdout).map_err(|e| json!(format!("non-JSON output: {stdout}: {e}")))
    } else {
        let parsed: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|_| json!({"error": "?", "message": stdout.to_string()}));
        Err(parsed)
    }
}

/// Drop fields that legitimately differ between runs (random record ids,
/// timing).
fn normalize(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("document_id");
            map.remove("id");
            map.remove("elapsed_ms");
            map.remove("created_at");
            map.remove("updated_at");
            for (k, v) in map.iter_mut() {
                // `hits` carries ranking order that must survive comparison;
                // `highlights` / `matched_entities` inside each hit are also
                // contractually ordered, so their arrays are normalized
                // without sorting.
                if k == "hits" {
                    for hit in v.as_array_mut().into_iter().flatten() {
                        normalize_object_preserving_arrays(hit);
                    }
                } else {
                    normalize(v);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalize(item);
            }
            items.sort_by_key(|item| serde_json::to_string(item).unwrap_or_default());
        }
        _ => {}
    }
}

/// Like `normalize`, but preserves the order of array fields inside an object
/// (used for hits, where highlights/matched_entities ordering is significant).
fn normalize_object_preserving_arrays(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("document_id");
            map.remove("id");
            map.remove("elapsed_ms");
            map.remove("created_at");
            map.remove("updated_at");
            for (k, v) in map.iter_mut() {
                if k == "highlights" || k == "matched_entities" {
                    for item in v.as_array_mut().into_iter().flatten() {
                        normalize(item);
                    }
                } else {
                    normalize(v);
                }
            }
        }
        other => normalize(other),
    }
}

const TEST_DOC: &str = "SurrealDB supports vector search with HNSW and full-text with BM25.";

#[test]
fn golden_upload_search_list_stats_get_delete() {
    let root = golden_root();
    let mcp_dir = setup_side(&root, "mcp");
    let cli_dir = setup_side(&root, "cli");

    let mut mcp = McpClient::spawn(&mcp_dir);
    mcp.initialize();

    // upload
    let mcp_upload = mcp
        .call_tool(
            "skb_upload",
            json!({"content": TEST_DOC, "title": "golden-doc", "tags": ["golden"]}),
        )
        .unwrap();
    let cli_upload = run_cli(
        &cli_dir,
        &[
            "upload",
            "--stdin",
            "--title",
            "golden-doc",
            "--tags",
            "golden",
        ],
        Some(TEST_DOC),
    )
    .unwrap();
    let mut a = mcp_upload.clone();
    let mut b = cli_upload.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "upload: MCP {mcp_upload} vs CLI {cli_upload}");

    // search (hybrid)
    let mcp_search = mcp
        .call_tool("skb_search", json!({"query": "HNSW BM25", "top_k": 5}))
        .unwrap();
    let cli_search = run_cli(&cli_dir, &["search", "HNSW BM25", "--top-k", "5"], None).unwrap();
    let mut a = mcp_search.clone();
    let mut b = cli_search.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "search: MCP {mcp_search} vs CLI {cli_search}");

    // list
    let mcp_list = mcp.call_tool("skb_list_documents", json!({})).unwrap();
    let cli_list = run_cli(&cli_dir, &["list"], None).unwrap();
    let mut a = mcp_list.clone();
    let mut b = cli_list.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "list: MCP {mcp_list} vs CLI {cli_list}");
    assert_eq!(a[0]["chunk_count"], 1, "chunk_count must be populated");

    // stats
    let mcp_stats = mcp.call_tool("skb_stats", json!({})).unwrap();
    let cli_stats = run_cli(&cli_dir, &["stats"], None).unwrap();
    let mut a = mcp_stats.clone();
    let mut b = cli_stats.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "stats: {mcp_stats} vs {cli_stats}");

    // get
    let mcp_doc_id = mcp_upload["document_id"].as_str().unwrap();
    let cli_doc_id = cli_upload["document_id"].as_str().unwrap();
    let mcp_get = mcp
        .call_tool(
            "skb_get_document",
            json!({"id": mcp_doc_id, "include_chunks": true}),
        )
        .unwrap();
    let cli_get = run_cli(&cli_dir, &["get", cli_doc_id, "--chunks"], None).unwrap();
    let mut a = mcp_get.clone();
    let mut b = cli_get.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "get: {mcp_get} vs {cli_get}");

    // delete
    let mcp_del = mcp
        .call_tool("skb_delete_document", json!({"id": mcp_doc_id}))
        .unwrap();
    let cli_del = run_cli(&cli_dir, &["delete", cli_doc_id, "--yes"], None).unwrap();
    let mut a = mcp_del.clone();
    let mut b = cli_del.clone();
    normalize(&mut a);
    normalize(&mut b);
    assert_eq!(a, b, "delete: {mcp_del} vs {cli_del}");

    // delete again: both sides must report E_DOCUMENT_NOT_FOUND
    let mcp_err = mcp
        .call_tool("skb_delete_document", json!({"id": mcp_doc_id}))
        .unwrap_err();
    let cli_err = run_cli(&cli_dir, &["delete", cli_doc_id, "--yes"], None).unwrap_err();
    assert!(
        mcp_err
            .as_str()
            .unwrap_or("")
            .contains("E_DOCUMENT_NOT_FOUND"),
        "MCP error: {mcp_err}"
    );
    assert_eq!(
        cli_err["error"], "E_DOCUMENT_NOT_FOUND",
        "CLI error: {cli_err}"
    );

    // Terminate the MCP server so its SurrealKV file handles are released
    // before removing the data directory (Windows cannot delete open handles).
    drop(mcp);
    let _ = std::fs::remove_dir_all(&root);
}
