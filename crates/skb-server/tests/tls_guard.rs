//! TLS guard (plan todo 5): the workspace stays rustls-only — `openssl-sys`
//! and `native-tls` must be ABSENT from the dependency graph
//! (CONTRIBUTING.md "TLS"). `cargo tree -i <pkg>` exits non-zero when the
//! package ID specification matches nothing, so non-zero exit = pass.
//! Runs offline against Cargo.lock; `cargo tree` performs no builds and
//! takes no target-dir lock, so it is safe inside `cargo test`.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn inverted_tree_must_fail(package: &str) {
    let output = Command::new("cargo")
        // --offline + --locked: a dependency-resolution or network failure
        // must NOT read as "package absent" and silently pass the guard.
        .args(["tree", "--offline", "--locked", "-i", package])
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|e| panic!("failed to run cargo tree -i {package}: {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "cargo tree -i {package} must exit non-zero (package absent), but it \
         succeeded — TLS guard violated:\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("did not match any packages"),
        "cargo tree -i {package} failed for another reason than package absence:\nstderr: {stderr}"
    );
}

#[test]
fn openssl_sys_is_absent_from_the_dependency_graph() {
    inverted_tree_must_fail("openssl-sys");
}

#[test]
fn native_tls_is_absent_from_the_dependency_graph() {
    inverted_tree_must_fail("native-tls");
}
