use std::path::Path;
use std::process::Command;

fn main() {
    embed_git_commit();
    tauri_build::build()
}

/// Runs git in the crate directory and returns its trimmed stdout, or `None`
/// if git is missing, this is not a checkout (a source tarball), or it failed.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Bakes the commit the app was built from into `ARIA_GIT_COMMIT`, for the
/// About screen and bug reports. `ARIA_GIT_COMMIT` in the environment wins, so
/// a packager building from a tarball can still say what it is; with neither,
/// the variable is empty and the About screen shows no build line.
fn embed_git_commit() {
    println!("cargo:rerun-if-env-changed=ARIA_GIT_COMMIT");

    let commit = std::env::var("ARIA_GIT_COMMIT")
        .ok()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .or_else(|| git(&["rev-parse", "--short=10", "HEAD"]))
        .unwrap_or_default();
    println!("cargo:rustc-env=ARIA_GIT_COMMIT={commit}");

    // tauri_build emits its own rerun-if-changed lines, which switches off
    // Cargo's "rerun on any change" default, so name the git files that move
    // when the commit does. `--git-path` resolves them inside a worktree too,
    // where `.git` is a file rather than a directory.
    let mut watched = vec![git(&["rev-parse", "--git-path", "HEAD"])];
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(git(&["rev-parse", "--git-path", &head_ref]));
    }
    watched.push(git(&["rev-parse", "--git-path", "packed-refs"]));
    for path in watched.into_iter().flatten() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
