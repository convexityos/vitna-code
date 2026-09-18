//! Nothing in the shipped tree may invoke `git` without the hardening.
//!
//! The two callers that carried this defect were each one line of
//! `Command::new("git")`. Fixing those two does not stop a third from being
//! written, and a new one would look entirely ordinary in review. This sweeps
//! the source instead of trusting that.
//!
//! Test files are exempt: they run plain `git` deliberately, as the control that
//! proves a vector is live before asserting the hardening blocks it.

use std::fs;
use std::path::{Path, PathBuf};

const NEEDLE: &str = "Command::new(\"git\")";

/// The one place allowed to build a git command.
const ALLOWED: &str = "host_git.rs";

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // Build output and vendored code are not ours to police.
            if name == "target" || name == "node_modules" || name == ".git" {
                continue;
            }
            collect_rs(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_crate_builds_a_bare_git_command() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf();

    let mut files = Vec::new();
    for top in ["crates", "apps"] {
        collect_rs(&repo_root.join(top), &mut files);
    }
    assert!(
        files.len() > 10,
        "source sweep found only {} files, so it is not looking where it thinks it is: {}",
        files.len(),
        repo_root.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == ALLOWED {
            continue;
        }
        let normalized = file.to_string_lossy().replace('\\', "/");
        if normalized.contains("/tests/") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains(NEEDLE) {
                offenders.push(format!("{}:{}", normalized, i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these build git directly instead of going through host_git::command, so the \
         repository's own configuration decides what they execute: {:?}",
        offenders
    );
}
