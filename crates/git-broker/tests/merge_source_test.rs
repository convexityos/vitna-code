//! A merge writes exactly the bytes its changeset attests, read from the
//! agent workspace without following links.
//!
//! The merge used to `fs::copy` each path out of the workspace as it stood
//! at merge time. Whatever changed after `inspect_changes`, reviewed or not,
//! went into the primary checkout, and a directory swapped for a link (to
//! `~/.ssh`, say) copied the link's target in.

use std::fs;
use std::path::{Path, PathBuf};
use vitna_git_broker::{GitBroker, MergeQueue, MergeQueueItem, MergeQueueResult};
use vitna_git_workspaces::{AgentWorkspace, AgentWorkspaceManager};

const SECRET: &str = "TOP SECRET\n";

/// A primary checkout, and a directory outside it. Removed on drop.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir()
            .join(format!("vitna_merge_source_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("repo/src")).expect("create primary src");
        fs::create_dir_all(base.join("outside")).expect("create outside dir");
        fs::write(base.join("repo/src/lib.rs"), "pub fn f() {}\n").expect("write primary file");
        Self { base }
    }

    fn repo(&self) -> PathBuf {
        self.base.join("repo")
    }

    fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    fn workspace(&self) -> AgentWorkspace {
        AgentWorkspaceManager::create_isolated_workspace(self.repo(), "run-1")
            .expect("create workspace")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

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

/// Inspects the workspace, lets `change` alter it, then asserts that
/// `apply_changeset` and the merge queue both refuse the stale changeset
/// with an error containing `expected`.
fn merge_after(s: &Scratch, ws: &AgentWorkspace, change: impl FnOnce(), expected: &str) {
    let mut cs = GitBroker::inspect_changes(ws).expect("inspect");
    assert!(!cs.files.is_empty());
    // Path order, whatever order the directory listing gave, so a test can
    // put the file it alters last.
    cs.files.sort_by(|a, b| a.path.cmp(&b.path));
    change();

    let err = GitBroker::apply_changeset(s.repo(), ws, &cs).expect_err("a stale changeset must not merge");
    assert!(err.contains(expected), "{}", err);

    let mut queue = MergeQueue::new(s.repo());
    queue.enqueue(MergeQueueItem {
        queue_id: "q-1".to_string(),
        task_id: "task-1".to_string(),
        agent_workspace_root: ws.agent_workspace_root.clone(),
        changeset: cs,
    });
    match queue.process_next() {
        Some((_, MergeQueueResult::Error { message })) => {
            assert!(message.contains(expected), "{}", message)
        }
        other => panic!("the queue must refuse a stale changeset as an error: {:?}", other),
    }
}

/// Nothing is written, not even the file that still matches, because every
/// source is checked before the first write.
#[test]
fn test_file_changed_after_inspection_is_not_merged() {
    let s = Scratch::new("edited");
    let ws = s.workspace();
    let root = ws.agent_workspace_root.clone();
    fs::write(root.join("a.txt"), "reviewed a\n").expect("agent write");
    fs::write(root.join("b.txt"), "reviewed b\n").expect("agent write");

    merge_after(
        &s,
        &ws,
        || fs::write(root.join("b.txt"), "never reviewed\n").expect("agent rewrite"),
        "no longer holds the content this changeset attests",
    );
    assert!(!s.repo().join("a.txt").exists(), "part of a stale changeset was merged");
    assert!(!s.repo().join("b.txt").exists(), "unreviewed content was merged");
}

#[test]
fn test_file_removed_after_inspection_is_not_merged() {
    let s = Scratch::new("removed");
    let ws = s.workspace();
    let root = ws.agent_workspace_root.clone();
    fs::write(root.join("a.txt"), "a\n").expect("agent write");

    merge_after(
        &s,
        &ws,
        || fs::remove_file(root.join("a.txt")).expect("agent remove"),
        "no longer in the agent workspace",
    );
    assert!(!s.repo().join("a.txt").exists());
}

/// A directory swapped for a link after inspection is refused even when the
/// file behind the link holds the very bytes the changeset attests, so the
/// link check stands on its own, and with a secret behind it.
#[test]
fn test_directory_swapped_for_a_link_after_inspection_is_not_merged() {
    for (name, behind_the_link) in [("same_bytes", "guide\n"), ("secret", SECRET)] {
        let s = Scratch::new(name);
        if !dir_link(&s.outside(), &s.base.join("probe")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let ws = s.workspace();
        let root = ws.agent_workspace_root.clone();
        fs::create_dir(root.join("docs")).expect("agent mkdir");
        fs::write(root.join("docs/guide.md"), "guide\n").expect("agent write");
        fs::write(s.outside().join("guide.md"), behind_the_link).expect("write outside");

        merge_after(
            &s,
            &ws,
            || {
                fs::remove_dir_all(root.join("docs")).expect("agent removes docs");
                assert!(dir_link(&s.outside(), &root.join("docs")), "agent links docs");
            },
            "link",
        );
        assert!(!s.repo().join("docs").exists(), "the merge copied through the link");
    }
}
