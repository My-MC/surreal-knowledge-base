# SPIKE: Multi-process DB open & locking strategy (plan todo 1)

Date: 2026-08-28 · Branch: `feat/skb-server-web` · Crate: `skb-server` (skeleton)

## What was probed

`spike_open` (bin in this crate) opens the embedded SurrealKv store via
`skb_core::db::Db::open` at the path given as argv[1], executes one trivial
write (`CREATE spike_test ...`), prints `OK`, exits 0. Any failure prints
`error: [E_...] ...` to stderr and exits with the matching `ErrorCode::exit_code`.

## Observed behavior

### 1. Sequential reopen (same path, one process at a time) — SUCCEEDS

`cargo run -p skb-server --bin spike_open -- target/skb-test-spike/db` twice in a
row: both runs print `OK`, exit 0. The SurrealKv file lock is released when the
owning process exits (in-process reopen right after `drop` may transiently fail
while the connection router task shuts the datastore down — see the
`settle_db_lock` note in `skb-core` lib tests — but a fresh process always wins
the lock once the previous owner is gone).

### 2. Two processes opening the SAME path concurrently — EXCLUSIVE LOCK, loser fails fast

`tests/spike_multi_process.rs` spawns two `spike_open` children against the same
db path before reaping either. Observed (both orderings occur — it is a race):

```
process-1: exit=Some(0) stdout=OK
process-2: exit=Some(3) stderr=error: [E_DB] open SurrealKv: There was a problem with the datastore: Other error: Database at <abs-path>/LOCK is already locked by another process
```

Exactly one process wins; the loser errors **immediately at open time** (no
blocking, no retry, no corruption) with `E_DB`, exit code 3. The lock is a lock
file (`<db-path>/LOCK`) held by SurrealKv.

### 3. Path component occupied by a regular file — E_DB at dir creation

With `target/skb-test-spike-collision` pre-created as a regular file, opening
`target/skb-test-spike-collision/anything` fails:

```
error: [E_DB] create db dir: File exists (os error 17)   # exit=3
```

### 4. Missing parent directories — AUTO-CREATED (quirk)

`Db::open` (`crates/skb-core/src/db.rs:95-97`) runs `std::fs::create_dir_all`
on the path's **parent** before SurrealKv opens the store. Opening
`target/skb-test-spike-new/sub/db` with none of those directories existing
succeeds (`OK`, exit 0) — the whole parent chain is created silently. Only a
path component occupied by a regular **file** fails (case 3). Implication: a
typo'd `storage.path` in `skb.toml` does not fail at startup; it materializes a
fresh store at the wrong location.

## Chosen locking strategy (conclusion for skb-server)

1. **The server process is the single DB owner.** `skb-server` opens the
   embedded SurrealKv store once at startup and holds it for the process
   lifetime. All HTTP handlers go through that one `KnowledgeBase` instance;
   the server never re-opens the path.
2. **CLI (`skb`) and MCP (`skb-mcp`) must not open the same `storage.path`
   while the server is running.** SurrealKv's cross-process lock makes any
   concurrent open fail immediately (case 2). Document this in the server
   startup output / docs; there is no safe read-only concurrent mode.
3. **Conflict behavior = fail fast.** A second opener gets `E_DB`
   ("...LOCK is already locked by another process") at open time. In
   `skb-server` this surfaces as a startup failure (non-zero exit, `E_DB`
   message on stderr). If a request path ever triggers a DB open error at
   runtime (e.g. after an external process stole/released the lock), the
   `E_DB`-family error is mapped to **HTTP 500** — it is a server-side
   storage fault, not a client error. No retry loop: the operator resolves
   the ownership conflict.
4. **Path-occupied-by-file and missing-parent quirks are accepted as-is**
   (cases 3-4): `E_DB` at startup for the former; silent auto-create for the
   latter. No skb-core changes (guardrails forbid them).

## Verification

```
cargo run -p skb-server --bin spike_open -- target/skb-test-spike/db   # 2x -> OK / exit 0
cargo test -p skb-server --test spike_multi_process -- --test-threads=1  # green
cargo clippy -p skb-server -- -D warnings                               # zero warnings
```

Evidence: `target/evidence/01/{spike-run-1,spike-run-2,multi-process-test,failure,missing-parent}.log`
