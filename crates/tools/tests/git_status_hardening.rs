//! `git_status` is the tool that carried the defect, so it gets its own proof.
//!
//! The helper it now calls has its own tests in `vitna-git-workspaces`. This one
//! asserts the tool actually calls it: a future edit that rebuilds a bare
//! `Command::new("git")` here would pass every test over there.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use vitna_runner::FakeRunner;
use vitna_tools::{GitStatusTool, Tool, ToolContext};

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn git_status_tool_does_not_run_repository_controlled_commands() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not available, skipping");
        return;
    }

    let root = std::env::temp_dir().join(format!("vitna_git_status_tool_{}", std::process::id()));
    let repo = root.join("repo");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&repo).expect("create repo");

    let marker = root.join("MARK_FSMONITOR");
    let marker_arg = marker.to_string_lossy().replace('\\', "/");

    assert!(git_ok(&repo, &["init", "-q", "."]), "git init");
    assert!(git_ok(&repo, &["config", "user.email", "t@example.invalid"]));
    assert!(git_ok(&repo, &["config", "user.name", "vitna test"]));
    fs::write(repo.join("file.txt"), "hello\n").expect("write");
    assert!(git_ok(&repo, &["add", "-A"]));
    assert!(git_ok(&repo, &["commit", "-qm", "init"]));
    assert!(git_ok(
        &repo,
        &[
            "config",
            "core.fsmonitor",
            &format!("echo x > \"{}\" #", marker_arg),
        ]
    ));

    // Control: the repository really can reach a shell through plain git here.
    fs::write(repo.join("file.txt"), "hello\n").expect("restat");
    let _ = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(&repo)
        .output();
    assert!(
        marker.exists(),
        "control failed: core.fsmonitor did not run under plain git, so this test \
         cannot prove the tool blocks it"
    );

    let _ = fs::remove_file(&marker);
    fs::write(repo.join("file.txt"), "hello\n").expect("restat");

    let journal = root.join("fake.journal");
    let ctx = ToolContext {
        workspace_root: repo.clone(),
        runner: Arc::new(Mutex::new(
            FakeRunner::new(&journal).expect("fake runner"),
        )),
    };

    let result = GitStatusTool
        .execute(serde_json::json!({}), &ctx)
        .await
        .expect("git_status succeeds");

    assert!(
        !marker.exists(),
        "git_status executed a repository-controlled command"
    );
    assert!(
        result.output.contains("Branch:"),
        "git_status lost its output: {:?}",
        result.output
    );

    let _ = fs::remove_dir_all(&root);
}
