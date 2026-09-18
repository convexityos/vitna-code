use std::fs;
use std::path::Path;
use vitna_git_broker::GitBroker;
use vitna_git_workspaces::AgentWorkspaceManager;

/// Links `link` to the absolute directory `target`. Windows without the
/// symlink privilege gets a junction, which needs none.
#[cfg(unix)]
fn dir_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn dir_link(target: &Path, link: &Path) -> bool {
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return true;
    }
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// A link cycle in the agent workspace, and a link the agent made to a
/// directory outside it, must neither loop the scan nor put files from
/// outside the workspace into the changeset.
#[test]
fn test_inspect_changes_does_not_follow_links() {
    let base = std::env::temp_dir().join(format!("vitna_broker_links_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let primary = base.join("repo");
    let outside = base.join("outside");
    fs::create_dir_all(primary.join("src")).expect("create primary src");
    fs::create_dir_all(&outside).expect("create outside dir");
    fs::write(primary.join("src/lib.rs"), "pub fn f() {}\n").expect("write primary file");
    fs::write(outside.join("id_rsa"), "PRIVATE KEY\n").expect("write key");

    let ws = AgentWorkspaceManager::create_isolated_workspace(&primary, "run-links")
        .expect("create workspace");
    let root = ws.agent_workspace_root.clone();
    let linked = dir_link(&root, &root.join("loop")) && dir_link(&outside, &root.join("keys"));
    if !linked {
        eprintln!("skipped: this machine cannot create directory links");
        AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup ws");
        let _ = fs::remove_dir_all(&base);
        return;
    }
    fs::write(root.join("src/lib.rs"), "pub fn g() {}\n").expect("agent edit");

    let changeset = GitBroker::inspect_changes(&ws).expect("the scan terminates");
    let paths: Vec<&str> = changeset.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["src/lib.rs"]);
    assert!(!changeset.combined_diff.contains("PRIVATE KEY"));

    AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup ws");
    let _ = fs::remove_dir_all(&base);
}
