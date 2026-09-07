//! Observation spike (plan todo 1): two child processes open the SAME
//! embedded SurrealKv path concurrently.
//!
//! Both children are held at a ready/go file barrier in front of `Db::open`
//! (see `spike_open`), so the open attempts truly overlap instead of
//! serializing via scheduler luck. Every child wait carries a deadline that
//! kills the child and fails with its collected output — a blocked DB open
//! must surface as a test failure, not hang the suite.
//!
//! This test asserts ONLY that both children terminate with a status code
//! (no hang, no test panic). The observed outcome — exclusive-lock error in
//! the second process vs. concurrent success — is classified, printed, and
//! written to `target/skb-test-spike-result.txt`; it is deliberately NOT
//! asserted, because the conclusion is the spike's deliverable (SPIKE.md).
//!
//! Serial execution required (embedded SurrealKV): run with
//! `cargo test -p skb-server --test spike_multi_process -- --test-threads=1`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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

/// Deadline-bounded `wait_with_output`: kills the child on expiry and fails
/// with the collected output so a blocked DB open cannot hang the suite.
fn wait_with_deadline(mut child: std::process::Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("poll spike child") {
            Some(_status) => {
                // Reap the exit status, stdout and stderr.
                return child.wait_with_output().expect("reap spike child");
            }
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let output = child.wait_with_output().expect("reap killed spike child");
                panic!(
                    "spike child did not terminate within {timeout:?} — killed.\nstdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout).trim_end(),
                    String::from_utf8_lossy(&output.stderr).trim_end(),
                );
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Block until both ready files exist (children are parked in front of
/// `Db::open`), then create the go file so both opens race.
fn release_barrier(ready_files: &[PathBuf], go_file: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready_files.iter().all(|p| p.exists()) {
        if Instant::now() > deadline {
            panic!("spike children never reported ready");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(go_file, "go").expect("write spike go file");
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

    let ready_first = dir.join("ready-1");
    let ready_second = dir.join("ready-2");
    let go_file = dir.join("go");
    let _ = std::fs::remove_file(&go_file);

    // Spawn both children before reaping either; the ready/go barrier parks
    // both in front of Db::open so the attempts overlap for real.
    let spawn = |ready: &Path| {
        Command::new(bin)
            .arg(&db_path)
            .env("SKB_SPIKE_READY_FILE", ready)
            .env("SKB_SPIKE_GO_FILE", &go_file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn spike_open child")
    };
    let first = spawn(&ready_first);
    let second = spawn(&ready_second);
    release_barrier(&[ready_first, ready_second], &go_file);

    let first_out = wait_with_deadline(first, Duration::from_secs(60));
    let second_out = wait_with_deadline(second, Duration::from_secs(60));

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
