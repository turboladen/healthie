//! Embeds the git commit into `GIT_COMMIT`, served at `GET /api/health` so a
//! deploy can assert the binary it just uploaded is the one answering.
//!
//! Precedence: `$GIT_COMMIT` (verbatim — `just deploy` resolves the SHA once and
//! uses the same string for the build and the assertion, so they agree by
//! construction), else ask git, else `"unknown"` — a `git archive` tarball or a
//! git-less host must still build.

use std::process::Command;

fn main() {
    // Emitting any rerun directive opts out of cargo's default "rerun on any
    // package file change", so every input has to be declared explicitly.
    println!("cargo:rerun-if-env-changed=GIT_COMMIT");
    emit_rerun_triggers();

    let commit = std::env::var("GIT_COMMIT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_COMMIT={commit}");
}

/// Declare the git files that determine the resolved HEAD commit.
///
/// `logs/HEAD` (the reflog) is the load-bearing one — the single file git writes
/// on every commit, checkout and reset. Both obvious alternatives ship a stale
/// hash silently: HEAD alone is a symref a commit never touches, and HEAD plus
/// the resolved loose ref misses a repo whose refs are packed (fresh clone, or
/// post-`gc`), where that file doesn't exist yet when triggers are computed.
fn emit_rerun_triggers() {
    for relative in ["logs/HEAD", "HEAD", "packed-refs"] {
        rerun_if_git_path_exists(relative);
    }
}

/// Emit a rerun trigger for `relative` resolved against the real gitdir.
///
/// Asking git rather than assuming `.git/<relative>` is what makes this work in a
/// worktree, where `.git` is a *file* and the literal path doesn't exist.
fn rerun_if_git_path_exists(relative: &str) {
    if let Some(path) = git_output(&["rev-parse", "--git-path", relative]) {
        let path = path.trim();
        if !path.is_empty() && std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// Abbreviated HEAD, `-dirty` when the tree has uncommitted changes.
///
/// `--short=8` and `git status --porcelain` both match what `just deploy` uses.
/// The `-dirty` marker is best-effort: cleanliness has no file to watch, so it
/// goes stale in either direction whenever this script doesn't rerun. Nothing on
/// the deploy path relies on it — `just deploy` refuses a dirty tree and passes
/// `GIT_COMMIT` in verbatim.
fn git_commit() -> Option<String> {
    let hash = git_output(&["rev-parse", "--short=8", "HEAD"])?;
    let hash = hash.trim();
    if hash.is_empty() {
        return None;
    }
    let dirty = git_output(&["status", "--porcelain"]).is_some_and(|out| !out.trim().is_empty());
    Some(if dirty {
        format!("{hash}-dirty")
    } else {
        hash.to_string()
    })
}

/// Run git, returning stdout or `None` on any failure. Callers trim.
fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
