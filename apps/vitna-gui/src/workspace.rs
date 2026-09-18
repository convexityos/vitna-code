//! What this window knows about the directory it is open on, read from disk.
//!
//! Every field here is a fact with a source. Nothing is inferred, and a fact
//! this module cannot establish is `None` rather than a plausible default, so
//! the window can say "not a git repository" instead of printing a branch
//! nobody is on.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub path: PathBuf,
    pub name: String,
    /// The checked out branch. `None` when this is not a git repository, and
    /// `Detached` when HEAD points at a commit rather than a branch.
    pub head: Option<Head>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    Detached(String),
}

impl Head {
    pub fn label(&self) -> String {
        match self {
            Head::Branch(b) => b.clone(),
            Head::Detached(sha) => {
                let short: String = sha.chars().take(12).collect();
                format!("detached at {short}")
            }
        }
    }
}

/// How many directory entries the modified-time walk looks at before it stops.
/// A workspace can be enormous, and the repository probe runs this every few
/// seconds on its own thread.
const SCAN_CAP: usize = 4000;

/// Directories the walk never descends into. They are build output and vendored
/// code: their mtimes answer "when did a tool last run", not "when did someone
/// last change this workspace".
const SKIP: &[&str] = &[".git", "node_modules", "dist", ".venv", "__pycache__"];

/// Cargo's output directory is `target` by default and anything at all under
/// CARGO_TARGET_DIR; a `target-mingw` beside `target` is common enough that
/// matching the name exactly let the walk descend into a build tree and hit
/// its cap, which the window then reported as a partial reading.
fn is_skipped(name: &str) -> bool {
    SKIP.contains(&name) || name.starts_with("target")
}

impl Workspace {
    /// Whether a session rooted at `root` belongs to this folder. The daemon
    /// holds the sessions of every folder any window has opened, and one from
    /// another folder chosen here would run the next turn over there. Compared
    /// as the file system resolves them, so the direction of a slash or the
    /// case of a drive letter does not make one folder two; a path that no
    /// longer resolves is compared as written.
    pub fn holds(&self, root: &Path) -> bool {
        match (std::fs::canonicalize(&self.path), std::fs::canonicalize(root)) {
            (Ok(mine), Ok(theirs)) => mine == theirs,
            _ => self.path == root,
        }
    }

    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        Self {
            head: read_head(&path),
            name,
            path,
        }
    }
}

/// Reads HEAD without shelling out to git.
///
/// Handles the worktree case, where `.git` is a FILE holding `gitdir: <path>`
/// rather than a directory. This repository is usually worked in worktrees, so
/// a directory-only version would report "not a git repository" for most of the
/// checkouts anyone actually opens.
fn read_head(root: &Path) -> Option<Head> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        let contents = std::fs::read_to_string(&dot_git).ok()?;
        let rest = contents.trim().strip_prefix("gitdir:")?.trim();
        let candidate = PathBuf::from(rest);
        if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        }
    } else {
        return None;
    };

    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => Some(Head::Branch(branch.to_string())),
        None => Some(Head::Detached(head.to_string())),
    }
}

/// The newest mtime under `root`, over a bounded walk.
///
/// Returns what it found, how many entries it looked at, and whether it hit the
/// cap, because a newest-change reading from a capped walk is a weaker claim
/// than one over the whole tree and the window prints which it had.
pub(crate) fn newest_mtime(root: &Path) -> (Option<SystemTime>, usize, bool) {
    let mut newest: Option<SystemTime> = None;
    let mut scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if scanned >= SCAN_CAP {
                return (newest, scanned, true);
            }
            scanned += 1;

            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            if file_type.is_dir() {
                if is_skipped(name.as_ref()) {
                    continue;
                }
                stack.push(entry.path());
                continue;
            }

            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                newest = Some(match newest {
                    Some(current) if current >= modified => current,
                    _ => modified,
                });
            }
        }
    }

    (newest, scanned, false)
}

/// An age for a past instant, or None when the clock cannot place it.
///
/// A time in the future returns None rather than a negative age: that means the
/// clock moved or the file is stamped ahead, and neither is an age to print.
pub fn ago(when: SystemTime) -> Option<String> {
    let elapsed = SystemTime::now().duration_since(when).ok()?;
    let secs = elapsed.as_secs();
    Some(match secs {
        0 => "just now".to_string(),
        1 => "1 second ago".to_string(),
        2..=59 => format!("{secs} seconds ago"),
        60..=119 => "1 minute ago".to_string(),
        120..=3599 => format!("{} minutes ago", secs / 60),
        3600..=7199 => "1 hour ago".to_string(),
        7200..=86_399 => format!("{} hours ago", secs / 3600),
        86_400..=172_799 => "1 day ago".to_string(),
        _ => format!("{} days ago", secs / 86_400),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vitna_gui_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// A session belongs to this window only when it is rooted in this very
    /// folder, however the path to it is spelled.
    #[test]
    fn a_session_belongs_to_the_folder_it_is_rooted_in() {
        let here = scratch("holds_here");
        let there = scratch("holds_there");
        let ws = Workspace::open(&here);
        assert!(ws.holds(&here));
        let slashed = PathBuf::from(here.to_string_lossy().replace(char::from(92u8), "/"));
        assert!(ws.holds(&slashed), "one folder, whichever way its slashes lean");
        assert!(!ws.holds(&there), "another folder's session is not this one's");
        assert!(!ws.holds(&here.join("inside")), "nor is a folder inside it");
        let _ = std::fs::remove_dir_all(&here);
        let _ = std::fs::remove_dir_all(&there);
    }

    #[test]
    fn ago_reads_past_instants_and_refuses_future_ones() {
        let now = SystemTime::now();
        assert_eq!(ago(now).as_deref(), Some("just now"));
        assert_eq!(
            ago(now - std::time::Duration::from_secs(300)).as_deref(),
            Some("5 minutes ago")
        );
        // A stamp in the future is not an age, so it is not printed as one.
        assert_eq!(ago(now + std::time::Duration::from_secs(600)), None);
    }

    #[test]
    fn head_is_none_outside_a_repository() {
        let dir = scratch("nogit");
        assert_eq!(read_head(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_reads_a_branch_and_a_detached_sha() {
        let dir = scratch("head");
        let git = dir.join(".git");
        std::fs::create_dir_all(&git).expect("create .git");

        std::fs::write(git.join("HEAD"), "ref: refs/heads/claude/native\n").expect("write HEAD");
        assert_eq!(
            read_head(&dir),
            Some(Head::Branch("claude/native".to_string()))
        );

        std::fs::write(git.join("HEAD"), "9f83223ed6277302d33bbc93664759\n").expect("write HEAD");
        match read_head(&dir) {
            Some(Head::Detached(sha)) => assert!(sha.starts_with("9f83223")),
            other => panic!("expected a detached head, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_follows_a_worktree_gitdir_file() {
        let base = scratch("worktree");
        let real_git = base.join("real-git");
        let tree = base.join("tree");
        std::fs::create_dir_all(&real_git).expect("create gitdir");
        std::fs::create_dir_all(&tree).expect("create worktree");
        std::fs::write(real_git.join("HEAD"), "ref: refs/heads/wt\n").expect("write HEAD");
        std::fs::write(
            tree.join(".git"),
            format!("gitdir: {}\n", real_git.display()),
        )
        .expect("write .git file");

        assert_eq!(read_head(&tree), Some(Head::Branch("wt".to_string())));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_walk_skips_build_output() {
        let dir = scratch("skip");
        std::fs::create_dir_all(dir.join("target")).expect("create target");
        std::fs::write(dir.join("target").join("huge.bin"), b"x").expect("write in target");
        // A custom CARGO_TARGET_DIR beside it is build output too.
        std::fs::create_dir_all(dir.join("target-mingw")).expect("create target-mingw");
        std::fs::write(dir.join("target-mingw").join("huge.bin"), b"x").expect("write in target-mingw");
        std::fs::write(dir.join("src.rs"), b"fn main() {}").expect("write source");

        let (_, scanned, capped) = newest_mtime(&dir);
        assert!(!capped);
        // Each build directory is counted as one entry and never descended
        // into, so their contents are not scanned: two dirs plus src.rs.
        assert_eq!(scanned, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
