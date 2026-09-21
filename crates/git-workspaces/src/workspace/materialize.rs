//! Copies the primary checkout into an agent workspace without following links.
//!
//! A repository is untrusted input, and it can commit a link to anywhere:
//! `notes -> ~/.ssh` turns a naive copy into a copy of the operator's keys,
//! and `loop -> .` recurses until the stack runs out. Nothing here follows a
//! link. A link is recreated as a link only when its text is relative and
//! stays inside the checkout both as written and once every link along the
//! way is resolved, so the same text leads to the same place inside the copy.
//! Every other link, and every FIFO, socket or device, is left out and
//! recorded.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, DirEntry, OpenOptions};
use std::io;
use std::path::{Component, Path};

/// Names never copied, at any depth: VCS metadata, build output,
/// dependencies, and Vitna's own state, which holds the workspaces.
const SKIPPED_NAMES: [&str; 4] = [".git", ".vitna", "target", "node_modules"];

/// An entry of the primary checkout that was deliberately left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedEntry {
    /// Path relative to the primary checkout, `/`-separated.
    pub path: String,
    /// Why it was left out.
    pub reason: String,
}

/// Copies the tree at `src` into `dst`, an existing empty directory, and
/// returns what was left out. An I/O error aborts the copy: a workspace that
/// looks complete but is not would be worse than none.
pub(super) fn copy_tree(src: &Path, dst: &Path) -> Result<Vec<RefusedEntry>, String> {
    let root = fs::canonicalize(src)
        .map_err(|e| format!("Cannot resolve primary repository {}: {}", src.display(), e))?;
    let mut refused = Vec::new();
    copy_dir(&root, &root, dst, &mut refused)?;
    Ok(refused)
}

fn copy_dir(root: &Path, src: &Path, dst: &Path, refused: &mut Vec<RefusedEntry>) -> Result<(), String> {
    for entry in sorted_entries(src)? {
        let name = entry.file_name();
        if name.to_str().is_some_and(|n| SKIPPED_NAMES.contains(&n)) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);

        // The entry's own type, never its target's: a link is handled as a
        // link and is never descended into.
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Cannot inspect {}: {}", src_path.display(), e))?;
        let refusal = if file_type.is_symlink() {
            copy_link(root, &src_path, &dst_path)
        } else if file_type.is_dir() {
            fs::create_dir(&dst_path)
                .map_err(|e| format!("Cannot create {}: {}", dst_path.display(), e))?;
            copy_dir(root, &src_path, &dst_path, refused)?;
            None
        } else if file_type.is_file() {
            copy_file(&src_path, &dst_path)?
        } else {
            Some("not a regular file, directory or link (a FIFO, socket or device)".to_string())
        };

        if let Some(reason) = refusal {
            let path = relative(root, &src_path);
            tracing::warn!("Not copying '{}' into the agent workspace: {}", path, reason);
            refused.push(RefusedEntry { path, reason });
        }
    }
    Ok(())
}

/// Directory entries in name order, so the copy and its record of refusals
/// come out the same on every run.
fn sorted_entries(dir: &Path) -> Result<Vec<DirEntry>, String> {
    let mut entries = fs::read_dir(dir)
        .and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
        .map_err(|e| format!("Cannot list {}: {}", dir.display(), e))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

/// Recreates a link verbatim when that is safe, and otherwise says why not.
fn copy_link(root: &Path, link: &Path, dst: &Path) -> Option<String> {
    let text = match fs::read_link(link) {
        Ok(text) => text,
        Err(e) => return Some(format!("link could not be read: {}", e)),
    };
    let shown = text.display();

    if text.has_root() || matches!(text.components().next(), Some(Component::Prefix(_))) {
        return Some(format!(
            "link target '{}' is absolute, so a copy would point outside the agent workspace",
            shown
        ));
    }
    if escapes_as_written(root, link, &text) {
        return Some(format!("link target '{}' leaves the repository", shown));
    }
    // As written it stays inside; a chain through other links must too.
    let target = match fs::canonicalize(link) {
        Ok(target) => target,
        Err(e) => return Some(format!("link target '{}' does not resolve: {}", shown, e)),
    };
    if !is_within(&target, root) {
        return Some(format!("link target '{}' resolves outside the repository", shown));
    }

    match make_link(&text, dst, target.is_dir()) {
        Ok(()) => None,
        Err(e) => Some(format!("link to '{}' could not be recreated: {}", shown, e)),
    }
}

/// True when relative link text, followed from the link's own directory,
/// climbs above the root at any point. Coming back down by name does not
/// help: `../repo/file` names a file inside the checkout, but the same text
/// inside the copy names the checkout's file, not the copy's.
fn escapes_as_written(root: &Path, link: &Path, text: &Path) -> bool {
    let mut depth = link
        .parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .map_or(0, |rel| rel.components().count());
    for component in text.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth == 0 => return true,
            Component::ParentDir => depth -= 1,
            Component::Prefix(_) | Component::RootDir => return true,
        }
    }
    false
}

/// Copies a regular file through a handle opened without following links,
/// refusing it if what was opened turns out not to be a regular file.
fn copy_file(src: &Path, dst: &Path) -> Result<Option<String>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow_no_block(&mut options);
    let mut input = options
        .open(src)
        .map_err(|e| format!("Cannot open {}: {}", src.display(), e))?;
    let meta = input
        .metadata()
        .map_err(|e| format!("Cannot inspect {}: {}", src.display(), e))?;
    if !meta.is_file() {
        return Ok(Some(
            "changed into something other than a regular file while it was being copied".to_string(),
        ));
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| format!("Cannot create {}: {}", dst.display(), e))?;
    io::copy(&mut input, &mut output).map_err(|e| format!("Cannot copy {}: {}", src.display(), e))?;
    // Keep the executable bit and the read-only flag, as `fs::copy` did.
    output
        .set_permissions(meta.permissions())
        .map_err(|e| format!("Cannot set permissions on {}: {}", dst.display(), e))?;
    Ok(None)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Component-wise containment, case-insensitive only where the filesystem
/// is. The same rule as `vitna_tools::path_safety`, which this crate cannot
/// depend on.
pub(super) fn is_within(path: &Path, root: &Path) -> bool {
    let mut path_parts = path.components();
    root.components().all(|r| {
        path_parts
            .next()
            .is_some_and(|p| same_component(r.as_os_str(), p.as_os_str()))
    })
}

#[cfg(windows)]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(not(windows))]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a == b
}

#[cfg(unix)]
fn make_link(text: &Path, dst: &Path, _target_is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(text, dst)
}

#[cfg(windows)]
fn make_link(text: &Path, dst: &Path, target_is_dir: bool) -> io::Result<()> {
    if target_is_dir {
        std::os::windows::fs::symlink_dir(text, dst)
    } else {
        std::os::windows::fs::symlink_file(text, dst)
    }
}

#[cfg(not(any(unix, windows)))]
fn make_link(_text: &Path, _dst: &Path, _target_is_dir: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "links are not supported on this platform",
    ))
}

/// O_NOFOLLOW: a file swapped for a link after the directory was listed
/// fails to open instead of being followed. O_NONBLOCK: a FIFO swapped in
/// cannot block the copy; the type check on the handle refuses it.
#[cfg(unix)]
fn no_follow_no_block(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

#[cfg(not(unix))]
fn no_follow_no_block(_options: &mut OpenOptions) {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A primary checkout, an empty copy destination, and a directory
    /// outside both. Removed on drop.
    struct Scratch {
        base: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("vitna_materialize_{}_{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&base);
            for dir in ["repo/src", "copy", "outside"] {
                fs::create_dir_all(base.join(dir)).expect("create scratch dirs");
            }
            fs::write(base.join("repo/README.md"), "readme\n").expect("write readme");
            fs::write(base.join("repo/src/main.rs"), "fn main() {}\n").expect("write main");
            fs::write(base.join("outside/id_rsa"), "PRIVATE KEY\n").expect("write key");
            Self { base }
        }

        fn repo(&self) -> PathBuf {
            self.base.join("repo")
        }

        fn copy(&self) -> PathBuf {
            self.base.join("copy")
        }

        fn outside(&self) -> PathBuf {
            self.base.join("outside")
        }

        fn run(&self) -> Vec<RefusedEntry> {
            copy_tree(&self.repo(), &self.copy()).expect("copy completes")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Links `link` to the directory `target`, absolute or relative to the
    /// link's directory. Windows without the symlink privilege gets a
    /// junction instead, which needs none but is always absolute. False when
    /// neither can be made; the test then skips.
    #[cfg(unix)]
    pub(crate) fn dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    pub(crate) fn dir_link(target: &Path, link: &Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        let parent = link.parent().unwrap_or(Path::new("."));
        let Ok(absolute) = fs::canonicalize(parent.join(target)) else {
            return false;
        };
        // mklink wants a plain drive path, not the verbatim form canonicalize returns.
        let absolute = absolute.to_string_lossy().replace(r"\\?\", "");
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(absolute)
            .output()
            .is_ok_and(|out| out.status.success())
    }

    #[cfg(unix)]
    fn file_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn file_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    fn was_refused(entries: &[RefusedEntry], path: &str) -> bool {
        entries.iter().any(|entry| entry.path == path)
    }

    /// The text of every regular file under `dir`, links not followed.
    fn all_contents(dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("list copy") {
            let entry = entry.expect("read entry");
            let file_type = entry.file_type().expect("entry type");
            if file_type.is_dir() {
                all_contents(&entry.path(), out);
            } else if file_type.is_file() {
                out.push(fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
    }

    #[test]
    fn test_plain_tree_is_copied_with_nothing_refused() {
        let s = Scratch::new("plain");
        let refused = s.run();
        assert!(refused.is_empty(), "{:?}", refused);
        assert_eq!(
            fs::read_to_string(s.copy().join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert_eq!(fs::read_to_string(s.copy().join("README.md")).unwrap(), "readme\n");
    }

    #[test]
    fn test_link_to_outside_is_refused_and_recorded() {
        let s = Scratch::new("outside");
        if !dir_link(&s.outside(), &s.repo().join("notes")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let refused = s.run();
        assert!(was_refused(&refused, "notes"), "{:?}", refused);
        assert!(
            fs::symlink_metadata(s.copy().join("notes")).is_err(),
            "nothing may be created for a refused link"
        );
        let mut contents = Vec::new();
        all_contents(&s.copy(), &mut contents);
        assert!(contents.iter().all(|c| !c.contains("PRIVATE KEY")), "{:?}", contents);
        assert_eq!(fs::read_to_string(s.copy().join("README.md")).unwrap(), "readme\n");
    }

    #[test]
    fn test_relative_link_out_of_the_tree_is_refused() {
        let s = Scratch::new("relative_out");
        if !dir_link(Path::new("../outside"), &s.repo().join("notes")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let refused = s.run();
        assert!(was_refused(&refused, "notes"), "{:?}", refused);
        assert!(fs::symlink_metadata(s.copy().join("notes")).is_err());
    }

    #[test]
    fn test_link_that_leaves_and_comes_back_is_refused() {
        let s = Scratch::new("round_trip");
        if !file_link(Path::new("../repo/README.md"), &s.repo().join("readme-link")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let refused = s.run();
        assert!(
            refused
                .iter()
                .any(|e| e.path == "readme-link" && e.reason.contains("leaves the repository")),
            "{:?}",
            refused
        );
    }

    #[test]
    fn test_link_cycle_terminates_and_is_never_descended() {
        let s = Scratch::new("cycle");
        if !dir_link(Path::new("."), &s.repo().join("loop")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        // Before this fix, this call never returned.
        let refused = s.run();
        match fs::symlink_metadata(s.copy().join("loop")) {
            // Copied: it must still be a link, not a directory holding a copy of the tree.
            Ok(meta) => assert!(meta.file_type().is_symlink(), "loop became a directory"),
            // Refused (a junction is absolute): it must be on record.
            Err(_) => assert!(was_refused(&refused, "loop"), "{:?}", refused),
        }
    }

    #[test]
    fn test_link_that_stays_inside_is_copied_as_a_link() {
        let s = Scratch::new("inside");
        if !dir_link(Path::new("src"), &s.repo().join("docs")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let refused = s.run();
        if was_refused(&refused, "docs") {
            // Windows without the symlink privilege made a junction, which
            // is absolute, and no link could be recreated in the copy anyway.
            assert!(fs::symlink_metadata(s.copy().join("docs")).is_err());
            return;
        }
        let meta = fs::symlink_metadata(s.copy().join("docs")).expect("docs copied");
        assert!(meta.file_type().is_symlink());
        assert_eq!(fs::read_link(s.copy().join("docs")).unwrap(), Path::new("src"));
        // It leads into the copy, not back into the primary checkout.
        assert_eq!(
            fs::canonicalize(s.copy().join("docs")).unwrap(),
            fs::canonicalize(s.copy().join("src")).unwrap()
        );
    }

    #[test]
    fn test_absolute_link_is_refused_even_inside_the_tree() {
        let s = Scratch::new("absolute");
        if !dir_link(&s.repo().join("src"), &s.repo().join("abs")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let refused = s.run();
        assert!(
            refused
                .iter()
                .any(|e| e.path == "abs" && e.reason.contains("absolute")),
            "{:?}",
            refused
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_link_to_a_device_is_refused() {
        let s = Scratch::new("device");
        std::os::unix::fs::symlink("/dev/null", s.repo().join("null")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", s.repo().join("zero")).unwrap();
        let refused = s.run();
        assert!(was_refused(&refused, "null"), "{:?}", refused);
        assert!(was_refused(&refused, "zero"), "{:?}", refused);
        assert!(fs::symlink_metadata(s.copy().join("zero")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_fifo_is_refused_without_blocking() {
        let s = Scratch::new("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(s.repo().join("pipe"))
            .status()
            .is_ok_and(|status| status.success());
        if !made {
            eprintln!("skipped: mkfifo is unavailable");
            return;
        }
        let (repo, copy) = (s.repo(), s.copy());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(copy_tree(&repo, &copy));
        });
        let refused = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the copy blocked on a FIFO")
            .expect("copy completes");
        assert!(was_refused(&refused, "pipe"), "{:?}", refused);
    }

    #[test]
    fn test_escape_is_judged_at_every_step() {
        let root = Path::new("/r");
        assert!(!escapes_as_written(root, Path::new("/r/a/link"), Path::new("../b")));
        assert!(!escapes_as_written(root, Path::new("/r/link"), Path::new("./a/../b")));
        assert!(escapes_as_written(root, Path::new("/r/link"), Path::new("../r/b")));
        assert!(escapes_as_written(root, Path::new("/r/a/link"), Path::new("x/../../../b")));
    }
}
