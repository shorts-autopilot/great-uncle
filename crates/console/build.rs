//! Stamp the binary with the short git hash, when there is one.
//!
//! A release tarball is not a checkout, and `cargo package` builds from a
//! temporary directory: both have no `.git`, so the hash is empty and
//! `--version` simply omits it. Nothing here fails a build.

use std::path::Path;
use std::process::Command;

fn git(manifest: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(manifest)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();

    // The hash must change when a commit moves the branch, not only when a
    // source file does. Watch the resolved git directory so a normal checkout
    // and a linked worktree both rebuild the stamp.
    if let Some(directory) = git(&manifest, &["rev-parse", "--absolute-git-dir"]) {
        for entry in ["HEAD", "refs/heads", "packed-refs"] {
            let path = Path::new(&directory).join(entry);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    let hash = git(&manifest, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=OPENLIGHT_GIT_HASH={hash}");
}
