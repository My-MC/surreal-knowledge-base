//! Observation spike (plan todo 1): two child processes open the SAME
//! embedded SurrealKv path concurrently.
//!
//! This test asserts ONLY that both children terminate with a status code
//! (no hang, no test panic). The observed outcome — exclusive-lock error in
//! the second process vs. concurrent success — is classified, printed, and
//! written to `target/skb-test-spike-result.txt`; it is deliberately NOT
//! asserted, because the conclusion is the spike's deliverable (SPIKE.md).
//!
//! Serial execution required (embedded SurrealKV): run with
//! `cargo test -p skb-server --test spike_multi_process -- --test-threads=1`.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Workspace root: this test lives at `<root>/crates/skb-server/tests`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// DB directory for the spike (repo rule: test DBs only under `./target/`).
fn spike_dir() -> PathBuf {
    workspace_root().join("target/skb-test-spike")
}

fn result_file() -> PathBuf {
    workspace_root().join("target/skb-test-spike-result.txt")
}

fn describe(name: &str, out: &Output) -> String {
    format!(
        "{name}: exit={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).trim_end(),
        String::from_utf8_lossy(&out.stderr).trim_end(),
    )
}

#[test]
fn spike_two_processes_open_same_db_path() {
    let bin = env!("CARGO_BIN_EXE_spike_open");
    let dir = spike_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create spike dir under target/");
    let db_path = dir.join("db");

    // Spawn both children before reaping either so the open attempts overlap.
    let spawn = || {
        Command::new(bin)
            .arg(&db_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn spike_open child")
    };
    let first = spawn();
    let second = spawn();

    let first_out = first.wait_with_output().expect("wait first child");
    let second_out = second.wait_with_output().expect("wait second child");

    // Observation only: classify the outcome, never assert which one occurs.
    let outcome = match (first_out.status.success(), second_out.status.success()) {
        (true, true) => "BOTH_SUCCEEDED_CONCURRENTLY (no cross-process exclusive lock observed)",
        (true, false) | (false, true) => {
            "ONE_PROCESS_FAILED (cross-process exclusive lock observed)"
        }
        (false, false) => "BOTH_FAILED (unexpected; recorded for investigation)",
    };
    let report = format!(
        "multi-process open spike\n  db path: {}\n  {}\n  {}\n  outcome: {outcome}\n",
        db_path.display(),
        describe("process-1", &first_out),
        describe("process-2", &second_out),
    );
    print!("{report}");
    std::fs::write(result_file(), &report).expect("write spike result file");

    // The only assertions: both children terminated with a status code.
    assert!(
        first_out.status.code().is_some(),
        "first child must terminate with a status code"
    );
    assert!(
        second_out.status.code().is_some(),
        "second child must terminate with a status code"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
