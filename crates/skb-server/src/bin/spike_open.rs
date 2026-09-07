//! Multi-process DB open spike: opens the embedded SurrealKv store at the
//! path given as argv[1], performs one trivial SurrealQL write, prints `OK`.
//!
//! When `SKB_SPIKE_READY_FILE` and `SKB_SPIKE_GO_FILE` are set, the process
//! writes the ready file and blocks until the go file appears — that lets
//! the test hold BOTH children in front of `Db::open` so the open attempts
//! truly overlap instead of serializing via scheduler luck.
//!
//! On any failure the error is printed to stderr and the process exits with
//! the matching `ErrorCode::exit_code` (e.g. `E_DB` -> 3), so the
//! multi-process test can classify the observed outcome.

use skb_core::config::Config;
use skb_core::db::Db;
use skb_core::error::{ErrorCode, SkbError};
use std::time::{Duration, Instant};

const SPIKE_WRITE_SQL: &str = "CREATE spike_test SET note = 'multi-process open spike'";
const GO_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(e.code.exit_code());
    }
}

async fn run() -> Result<(), SkbError> {
    let db_path = std::env::args()
        .nth(1)
        .ok_or_else(|| SkbError::new(ErrorCode::Config, "usage: spike_open <db-path>"))?;

    let mut config = Config::default();
    config.storage.path = db_path.into();

    hold_at_barrier()?;

    let db = Db::open(&config).await?;
    let r = db
        .db
        .query(SPIKE_WRITE_SQL)
        .await
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("spike write: {e}")))?;
    r.check()
        .map_err(|e| SkbError::new(ErrorCode::Db, format!("spike write check: {e}")))?;

    println!("OK");
    Ok(())
}

/// With the barrier env set: touch the ready file, then poll-wait for the go
/// file so both children are parked in front of `Db::open` simultaneously.
/// Failures surface through the normal `error:` + exit-code path.
fn hold_at_barrier() -> Result<(), SkbError> {
    let (Some(ready), Some(go)) = (
        std::env::var("SKB_SPIKE_READY_FILE").ok(),
        std::env::var("SKB_SPIKE_GO_FILE").ok(),
    ) else {
        return Ok(());
    };
    std::fs::write(&ready, "ready")
        .map_err(|e| SkbError::new(ErrorCode::Io, format!("spike ready file: {e}")))?;
    let deadline = Instant::now() + GO_WAIT_TIMEOUT;
    while !std::path::Path::new(&go).exists() {
        if Instant::now() > deadline {
            return Err(SkbError::new(
                ErrorCode::Config,
                format!("spike go file {go} never appeared"),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
