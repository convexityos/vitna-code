//! The repository facts behind the window's one designed moment.
//!
//! Read by asking git, on a worker thread, because `git status` on a large
//! checkout is not something to do between two frames. Until the answer
//! arrives the window says it is reading; if git is absent or fails, it says
//! that, with the reason. No field here is ever a plausible default: an
//! unknown count stays `None` and prints as a dash.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone, Default)]
pub struct RepoFacts {
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub staged: Option<usize>,
    pub modified: Option<usize>,
    pub untracked: Option<usize>,
    pub commits: Vec<Commit>,
    /// Why some of the above is missing. Printed rather than swallowed.
    pub trouble: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub unix_seconds: i64,
    pub author: String,
    pub subject: String,
}

/// What the window has so far.
pub enum Loading {
    Reading,
    Done(Box<RepoFacts>),
}

pub struct Probe {
    rx: Receiver<RepoFacts>,
    state: Loading,
}

impl Probe {
    /// Starts the read. Returns immediately.
    pub fn start(root: &Path) -> Self {
        let (tx, rx) = mpsc::channel();
        let root: PathBuf = root.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(read(&root));
        });
        Self {
            rx,
            state: Loading::Reading,
        }
    }

    /// Takes the answer if it has arrived. Cheap to call every frame.
    pub fn poll(&mut self) -> &Loading {
        if matches!(self.state, Loading::Reading) {
            if let Ok(facts) = self.rx.try_recv() {
                self.state = Loading::Done(Box::new(facts));
            }
        }
        &self.state
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("git could not be run: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn read(root: &Path) -> RepoFacts {
    let mut facts = RepoFacts::default();

    // Upstream first: without one, ahead and behind are not merely unknown,
    // they are undefined, and the window says so rather than printing zero.
    match git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]) {
        Ok(name) => {
            let name = name.trim().to_string();
            if !name.is_empty() {
                facts.upstream = Some(name);
            }
        }
        Err(e) => {
            if e.contains("no upstream") || e.contains("does not point to a branch") {
                // Not trouble. A branch with no upstream is an ordinary state.
            } else if facts.trouble.is_none() {
                facts.trouble = Some(e);
            }
        }
    }

    if facts.upstream.is_some() {
        if let Ok(counts) = git(root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
            let mut parts = counts.split_whitespace();
            facts.ahead = parts.next().and_then(|s| s.parse().ok());
            facts.behind = parts.next().and_then(|s| s.parse().ok());
        }
    }

    match git(root, &["status", "--porcelain=v1", "--untracked-files=normal"]) {
        Ok(status) => {
            let (mut staged, mut modified, mut untracked) = (0usize, 0usize, 0usize);
            for line in status.lines() {
                if line.len() < 2 {
                    continue;
                }
                let bytes = line.as_bytes();
                let (index, worktree) = (bytes[0] as char, bytes[1] as char);
                if index == '?' && worktree == '?' {
                    untracked += 1;
                    continue;
                }
                if index != ' ' {
                    staged += 1;
                }
                if worktree != ' ' {
                    modified += 1;
                }
            }
            facts.staged = Some(staged);
            facts.modified = Some(modified);
            facts.untracked = Some(untracked);
        }
        Err(e) => {
            if facts.trouble.is_none() {
                facts.trouble = Some(e);
            }
        }
    }

    // Unit separator between fields: a commit subject may contain anything a
    // person can type, including tabs, and this repository's subjects are long
    // sentences with quotes and colons in them.
    if let Ok(log) = git(root, &["log", "-n", "6", "--format=%h%x1f%ct%x1f%an%x1f%s"]) {
        for line in log.lines() {
            let mut f = line.split('\u{1f}');
            let (sha, ts, author, subject) = (f.next(), f.next(), f.next(), f.next());
            if let (Some(sha), Some(ts), Some(author), Some(subject)) = (sha, ts, author, subject) {
                facts.commits.push(Commit {
                    sha: sha.to_string(),
                    unix_seconds: ts.parse().unwrap_or(0),
                    author: author.to_string(),
                    subject: subject.to_string(),
                });
            }
        }
    }

    facts
}
