//! Multi-process DB open spike: opens the embedded SurrealKv store at the
//! path given as argv[1], performs one trivial SurrealQL write, prints `OK`.
//!
//! On any failure the error is printed to stderr and the process exits with
//! the matching `ErrorCode::exit_code` (e.g. `E_DB` -> 3), so the
//! multi-process test can classify the observed outcome.

use skb_core::config::Config;
use skb_core::db::Db;
use skb_core::error::{ErrorCode, SkbError};

const SPIKE_WRITE_SQL: &str = "CREATE spike_test SET note = 'multi-process open spike'";

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
