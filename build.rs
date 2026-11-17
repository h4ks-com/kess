//! Records the commit kess is built from, so `kess --version` names it.

use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    if let Some(commit) = commit() {
        println!("cargo::rustc-env=KESS_COMMIT={commit}");
    }
}

/// The commit checked out in this folder, when it is a git checkout. A
/// source download has none, and we never ask git about a parent folder,
/// which would name some other repository's commit.
fn commit() -> Option<String> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR")?;
    let git = Path::new(&manifest).join(".git");
    if !git.exists() {
        return None;
    }
    let head = git.join("HEAD");
    println!("cargo::rerun-if-changed={}", head.display());
    if let Some(branch) = std::fs::read_to_string(&head).ok().and_then(|head| {
        head.strip_prefix("ref: ")
            .map(|branch| branch.trim().to_owned())
    }) {
        println!("cargo::rerun-if-changed={}", git.join(branch).display());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&manifest)
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()?;
    let commit = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !commit.is_empty()).then_some(commit)
}
