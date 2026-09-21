//! Merging never reads or writes the primary checkout through a link.
//!
//! Materialization leaves a link that leads out of the checkout out of the
//! agent workspace, so the agent can create a regular file at the same path.
//! Before this, inspecting that change read the link's target into the diff,
//! and merging it wrote through the link.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use vitna_git_broker::{
    ChangeSet, FileChange, GitBroker, MergeQueue, MergeQueueItem, MergeQueueResult, NULL_HASH,
};
use vitna_git_workspaces::{AgentWorkspace, AgentWorkspaceManager};

const SECRET: &str = "TOP SECRET\n";
const LIB: &str = "pub fn f() {}\n";

/// A primary checkout holding `src/lib.rs`, and a directory outside it
/// holding a secret. Removed on drop.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir()
            .join(format!("vitna_primary_links_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("repo/src")).expect("create primary src");
        fs::create_dir_all(base.join("outside")).expect("create outside dir");
        fs::write(base.join("repo/src/lib.rs"), LIB).expect("write primary file");
        fs::write(base.join("outside/secret.txt"), SECRET).expect("write secret");
        Self { base }
    }

    fn repo(&self) -> PathBuf {
        self.base.join("repo")
    }

    fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    fn secret(&self) -> String {
        fs::read_to_string(self.outside().join("secret.txt")).expect("read secret")
    }

    /// An agent workspace materialized from the checkout as it is now.
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

/// Links `link` to the file `target`. False where this machine refuses, such
/// as Windows without the symlink privilege; the test then skips.
#[cfg(unix)]
fn file_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn file_link(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
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

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A change as a changeset records it. The diff plays no part in merging.
fn change(path: &str, preimage_hash: &str, content: &str) -> FileChange {
    FileChange {
        path: path.to_string(),
        preimage_hash: preimage_hash.to_string(),
        postimage_hash: sha(content.as_bytes()),
        diff: String::new(),
    }
}

fn changeset(files: Vec<FileChange>) -> ChangeSet {
    ChangeSet {
        changeset_id: "cs-test".to_string(),
        base_commit_sha: "0".repeat(40),
        files,
        combined_diff: String::new(),
        diff_digest: String::new(),
    }
}

fn queue_item(ws: &AgentWorkspace, changeset: &ChangeSet) -> MergeQueueItem {
    MergeQueueItem {
        queue_id: "q-1".to_string(),
        task_id: "task-1".to_string(),
        agent_workspace_root: ws.agent_workspace_root.clone(),
        changeset: changeset.clone(),
    }
}

/// `validate_preimages` reports exactly `conflicted`, and `apply_changeset`
/// and the merge queue both refuse the changeset as conflicts, writing
/// nothing. Returns the conflict messages.
fn assert_refused(s: &Scratch, ws: &AgentWorkspace, cs: &ChangeSet, conflicted: &[&str]) -> Vec<String> {
    let conflicts = GitBroker::validate_preimages(s.repo(), cs).expect("validate");
    let paths: Vec<&str> = conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, conflicted, "{:?}", conflicts);

    let result = GitBroker::apply_changeset(s.repo(), ws, cs).expect("apply");
    assert!(!result.success, "{:?}", result);
    assert!(result.applied_files.is_empty(), "{:?}", result);
    assert_eq!(result.conflicts, conflicts);

    let mut queue = MergeQueue::new(s.repo());
    queue.enqueue(queue_item(ws, cs));
    match queue.process_next() {
        Some((_, MergeQueueResult::Conflict { conflicts: queued })) => assert_eq!(queued, conflicts),
        other => panic!("the queue must refuse this as a conflict: {:?}", other),
    }
    conflicts.into_iter().map(|c| c.message).collect()
}

#[test]
fn test_primary_link_to_a_file_outside_is_neither_read_nor_written() {
    let s = Scratch::new("file_out");
    if !file_link(&s.outside().join("secret.txt"), &s.repo().join("notes.txt")) {
        eprintln!("skipped: this machine cannot create symbolic links");
        return;
    }
    // Materialization leaves the link out, so the agent can make a regular
    // file in its place.
    let ws = s.workspace();
    assert!(fs::symlink_metadata(ws.agent_workspace_root.join("notes.txt")).is_err());
    fs::write(ws.agent_workspace_root.join("notes.txt"), "agent notes\n").expect("agent write");

    let err = GitBroker::inspect_changes(&ws).expect_err("no preimage may be read through the link");
    assert!(err.contains("notes.txt") && err.contains("outside"), "{}", err);
    assert!(!err.contains("TOP SECRET"), "{}", err);

    // The preimage the old scan recorded was the link target's hash, and a
    // changeset claiming a new file must not get through either.
    for preimage in [sha(SECRET.as_bytes()), NULL_HASH.to_string()] {
        let cs = changeset(vec![change("notes.txt", &preimage, "agent notes\n")]);
        let messages = assert_refused(&s, &ws, &cs, &["notes.txt"]);
        assert!(messages[0].contains("outside"), "{}", messages[0]);
    }
    assert_eq!(s.secret(), SECRET, "the merge wrote through the link");
    assert!(is_link(&s.repo().join("notes.txt")), "the link itself must be left alone");
}

#[test]
fn test_primary_directory_link_to_outside_is_neither_read_nor_written() {
    let s = Scratch::new("dir_out");
    if !dir_link(&s.outside(), &s.repo().join("docs")) {
        eprintln!("skipped: this machine cannot create directory links");
        return;
    }
    let ws = s.workspace();
    assert!(fs::symlink_metadata(ws.agent_workspace_root.join("docs")).is_err());
    fs::create_dir(ws.agent_workspace_root.join("docs")).expect("agent mkdir");
    fs::write(ws.agent_workspace_root.join("docs/secret.txt"), "agent\n").expect("agent write");

    let err = GitBroker::inspect_changes(&ws).expect_err("no preimage may be read through the link");
    assert!(err.contains("docs") && err.contains("outside"), "{}", err);
    assert!(!err.contains("TOP SECRET"), "{}", err);

    let cs = changeset(vec![
        change("docs/secret.txt", &sha(SECRET.as_bytes()), "agent\n"),
        change("docs/new.txt", NULL_HASH, "new\n"),
        change("docs/deeper/new.txt", NULL_HASH, "new\n"),
    ]);
    assert_refused(&s, &ws, &cs, &["docs/secret.txt", "docs/new.txt", "docs/deeper/new.txt"]);

    assert_eq!(s.secret(), SECRET, "the merge wrote through the link");
    assert!(!s.outside().join("new.txt").exists(), "a file was created through the link");
    assert!(!s.outside().join("deeper").exists(), "a directory was created through the link");
}

/// A link that stays inside the checkout is refused as well. The changeset
/// cannot say whether to write through it or replace it, and writing through
/// it changes a file the changeset names differently, possibly one it also
/// changes: here both entries pass a preimage check against the same file,
/// and the second would silently overwrite the first.
#[test]
fn test_link_inside_the_checkout_is_refused_too() {
    let s = Scratch::new("dir_in");
    if !dir_link(&s.repo().join("src"), &s.repo().join("alias")) {
        eprintln!("skipped: this machine cannot create directory links");
        return;
    }
    let ws = s.workspace();
    // Materialization refuses an absolute link, even one that stays inside.
    assert!(fs::symlink_metadata(ws.agent_workspace_root.join("alias")).is_err());
    fs::create_dir(ws.agent_workspace_root.join("alias")).expect("agent mkdir");
    fs::write(ws.agent_workspace_root.join("alias/lib.rs"), "pub fn b() {}\n").expect("agent write");

    let err = GitBroker::inspect_changes(&ws).expect_err("the link is not followed");
    assert!(err.contains("alias") && err.contains("inside"), "{}", err);

    let cs = changeset(vec![
        change("src/lib.rs", &sha(LIB.as_bytes()), "pub fn a() {}\n"),
        change("alias/lib.rs", &sha(LIB.as_bytes()), "pub fn b() {}\n"),
    ]);
    let messages = assert_refused(&s, &ws, &cs, &["alias/lib.rs"]);
    assert!(messages[0].contains("inside"), "{}", messages[0]);
    assert_eq!(fs::read_to_string(s.repo().join("src/lib.rs")).unwrap(), LIB);
}

#[test]
fn test_dangling_primary_link_is_not_created_through() {
    let s = Scratch::new("dangling");
    let planted = s.outside().join("planted.txt");
    if !file_link(&planted, &s.repo().join("planted.txt")) {
        eprintln!("skipped: this machine cannot create symbolic links");
        return;
    }
    let ws = s.workspace();
    fs::write(ws.agent_workspace_root.join("planted.txt"), "agent\n").expect("agent write");

    let err = GitBroker::inspect_changes(&ws).expect_err("the link is not followed");
    assert!(err.contains("planted.txt") && err.contains("does not exist"), "{}", err);

    // A dangling link does not "exist" to `Path::exists`, so the old check
    // took this for a new file, and `fs::copy` created the link's target.
    let cs = changeset(vec![change("planted.txt", NULL_HASH, "agent\n")]);
    assert_refused(&s, &ws, &cs, &["planted.txt"]);
    assert!(!planted.exists(), "the merge created the link's target");
}

#[cfg(unix)]
#[test]
fn test_primary_fifo_is_refused_without_blocking() {
    let s = Scratch::new("fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(s.repo().join("pipe"))
        .status()
        .is_ok_and(|status| status.success());
    if !made {
        eprintln!("skipped: mkfifo is unavailable");
        return;
    }
    let ws = s.workspace();
    fs::write(ws.agent_workspace_root.join("pipe"), "agent\n").expect("agent write");

    let (repo, agent_ws) = (s.repo(), ws.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let cs = changeset(vec![change("pipe", NULL_HASH, "agent\n")]);
        let _ = tx.send((
            GitBroker::inspect_changes(&agent_ws),
            GitBroker::validate_preimages(&repo, &cs),
        ));
    });
    let (inspected, validated) = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("reading the FIFO blocked instead of refusing it");
    let err = inspected.expect_err("a FIFO has no preimage");
    assert!(err.contains("a FIFO"), "{}", err);
    let conflicts = validated.expect("validate");
    assert_eq!(conflicts.len(), 1, "{:?}", conflicts);
    assert!(conflicts[0].message.contains("a FIFO"), "{}", conflicts[0].message);
}

#[test]
fn test_changeset_paths_must_stay_inside_the_checkout() {
    let s = Scratch::new("paths");
    let ws = s.workspace();
    // Where `agent_workspace_root.join("../escape.txt")` lands, so the old
    // copy had a source to take.
    let beside = ws.agent_workspace_root.parent().expect("workspaces dir").join("escape.txt");
    fs::write(&beside, "escaped\n").expect("write beside the workspace");
    let absolute = s.outside().join("absolute.txt");

    for path in ["../escape.txt", absolute.to_str().expect("utf-8 temp path"), ""] {
        let cs = changeset(vec![change(path, NULL_HASH, "escaped\n")]);
        let err = GitBroker::validate_preimages(s.repo(), &cs).expect_err("malformed path");
        assert!(err.contains("inside the primary checkout"), "{}", err);
        GitBroker::apply_changeset(s.repo(), &ws, &cs).expect_err("malformed path");
        let mut queue = MergeQueue::new(s.repo());
        queue.enqueue(queue_item(&ws, &cs));
        assert!(
            matches!(queue.process_next(), Some((_, MergeQueueResult::Error { .. }))),
            "the queue must report a malformed path as an error"
        );
    }
    assert!(!s.base.join("escape.txt").exists(), "the merge wrote above the checkout");
    assert!(!absolute.exists(), "the merge wrote to an absolute path");
}

/// A file where one of a path's directories belongs cannot be merged over.
/// That is a conflict, found before anything is written, rather than an
/// error partway through the merge.
#[test]
fn test_non_directory_on_the_way_is_a_conflict_before_anything_is_written() {
    let s = Scratch::new("blocked");
    fs::write(s.repo().join("foo"), "foo\n").expect("write primary file");
    let ws = s.workspace();
    let root = &ws.agent_workspace_root;
    fs::remove_file(root.join("foo")).expect("agent removes the file");
    fs::create_dir(root.join("foo")).expect("agent mkdir");
    fs::write(root.join("foo/bar.rs"), "bar\n").expect("agent write");
    fs::write(root.join("a.txt"), "a\n").expect("agent write");

    // Nothing is there, so the preimage is absent...
    let cs = GitBroker::inspect_changes(&ws).expect("inspect");
    let blocked = cs.files.iter().find(|f| f.path == "foo/bar.rs").expect("foo/bar.rs changed");
    assert_eq!(blocked.preimage_hash, NULL_HASH);

    // ...but nothing can be created there either.
    let messages = assert_refused(&s, &ws, &cs, &["foo/bar.rs"]);
    assert!(messages[0].contains("not a directory"), "{}", messages[0]);
    assert!(!s.repo().join("a.txt").exists(), "part of the changeset was written");
}

#[test]
fn test_merge_creates_missing_directories_and_keeps_permission_bits() {
    let s = Scratch::new("new_dirs");
    let ws = s.workspace();
    let root = &ws.agent_workspace_root;
    fs::create_dir_all(root.join("a/b")).expect("agent mkdir");
    fs::write(root.join("a/b/c.txt"), "c\n").expect("agent write");
    fs::write(root.join("run.sh"), "#!/bin/sh\n").expect("agent write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755))
            .expect("agent chmod");
    }

    let cs = GitBroker::inspect_changes(&ws).expect("inspect");
    let result = GitBroker::apply_changeset(s.repo(), &ws, &cs).expect("apply");
    assert!(result.success, "{:?}", result);
    assert_eq!(fs::read_to_string(s.repo().join("a/b/c.txt")).unwrap(), "c\n");
    assert_eq!(fs::read_to_string(s.repo().join("run.sh")).unwrap(), "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(s.repo().join("run.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the executable bits were lost: {:o}", mode);
    }
}
