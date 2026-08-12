// Golden contract tests: the same JSON request must produce semantically
// identical responses through the MCP handler (stdio) and the CLI (spec
// §11.2-3). Each side runs against its own database with identical setup;
// random document ids are normalized away before comparison.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

fn mcp_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/skb-mcp");
    path
}

fn cli_binary() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../target/debug/skb");
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
    rx: std::sync::mpsc::Receiver<Value>,
    next_id: u64,
}

impl McpClient {
    fn spawn(dir: &std::path::Path) -> McpClient {
        let mut child = Command::new(mcp_binary())
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn skb-mcp (build with: cargo build -p skb-mcp)");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        // A dedicated reader thread pushes every stdout line into an mpsc
        // channel; the main thread consumes with recv_timeout so both a
        // silent child hang and unrelated-message floods are detected within
        // the deadline (the child is killed before failing).
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let mut stdout = stdout;
            let mut line = String::new();
            loop {
                line.clear();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                            if tx.send(msg).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        McpClient {
            child,
            stdin,
            rx,
            next_id: 1,
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
        // Bounded wait via recv_timeout: an absolute deadline is fixed once
        // before the loop; each recv_timeout receives only the REMAINING
        // duration. The reader thread keeps pushing unrelated messages; if
        // the overall deadline expires without the expected response, kill
        // the child and fail the test instead of blocking indefinitely.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                let _ = self.child.kill();
                panic!("timed out waiting for id {id}");
            }
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    if msg["id"] == json!(id) {
                        return msg;
                    }
                }
                Err(_) => {
                    let _ = self.child.kill();
                    panic!("timed out waiting for id {id}");
                }
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        // A notification omits the id and never waits for a response.
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
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
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn setup_side(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = format!(
        r#"search = {{ rrf_k = 10 }}

[embedding]
onnx_path = "mock"
dimension = 8

[storage]
path = "{}"
"#,
        dir.join("db").display()
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
    let _ = args;
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
            for v in map.values_mut() {
                normalize(v);
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
    let mut mcp_stats = mcp.call_tool("skb_stats", json!({})).unwrap();
    let mut cli_stats = run_cli(&cli_dir, &["stats"], None).unwrap();
    normalize(&mut mcp_stats);
    normalize(&mut cli_stats);
    assert_eq!(mcp_stats, cli_stats, "stats: {mcp_stats} vs {cli_stats}");

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

    // Release the MCP child process and its file handles before removing the
    // test directories.
    drop(mcp);
    let _ = std::fs::remove_dir_all(&root);
}
