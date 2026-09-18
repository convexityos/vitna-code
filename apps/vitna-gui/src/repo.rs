//! The repository facts the composer's bar states, read by asking git.
//!
//! On a worker thread, because `git status` on a large checkout is not
//! something to do between two frames, and again every few seconds, with the
//! last answer kept on screen while the next is read so the bar stays current
//! without flickering. If git is absent or fails, the bar says so with the
//! reason. No field here is ever a plausible default: an unknown count stays
//! `None` and prints as a dash.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

/// How often the facts are read again while the window is open.
pub const REFRESH: Duration = Duration::from_secs(8);

/// Whether this branch would merge into the one it is based on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    /// `git merge-tree` wrote a tree with no conflicts.
    Clean,
    /// It did not; these are the conflicted paths.
    Conflicts(Vec<String>),
    /// HEAD has no commit the base lacks, so there is nothing to merge.
    UpToDate,
    /// The check was not run, for this reason.
    NotRun(String),
}

#[derive(Debug, Clone, Default)]
pub struct RepoFacts {
    /// The repository's name, off the origin remote's URL, and the URL.
    pub repo_name: Option<String>,
    pub origin_url: Option<String>,
    /// HEAD as a short hash, with its subject, commit time and `git describe`.
    pub head: Option<String>,
    pub subject: Option<String>,
    pub committed: Option<SystemTime>,
    pub describe: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub staged: Option<usize>,
    pub modified: Option<usize>,
    pub untracked: Option<usize>,
    /// Lines added and removed in tracked files against HEAD, staged or not.
    pub added: Option<u64>,
    pub removed: Option<u64>,
    /// The branch a merge would go into, as a remote-tracking ref, and how
    /// this branch stands against it.
    pub base: Option<String>,
    pub merge: Option<Merge>,
    pub base_ahead: Option<u32>,
    pub base_behind: Option<u32>,
    /// When this checkout last fetched, which is how old `base` is.
    pub fetched: Option<SystemTime>,
    /// The newest mtime in a bounded walk of the folder, and the walk's reach.
    pub touched: Option<SystemTime>,
    pub entries_scanned: usize,
    pub scan_was_capped: bool,
    /// Why some of the above is missing. Printed rather than swallowed.
    pub trouble: Option<String>,
}

/// What the window has so far.
pub enum Loading {
    Reading,
    Done(Box<RepoFacts>),
}

pub struct Probe {
    root: PathBuf,
    rx: Option<Receiver<RepoFacts>>,
    state: Loading,
    asked: Instant,
}

impl Probe {
    /// Starts the first read. Returns immediately.
    pub fn start(root: &Path) -> Self {
        let mut p = Self {
            root: root.to_path_buf(),
            rx: None,
            state: Loading::Reading,
            asked: Instant::now(),
        };
        p.refresh();
        p
    }

    /// Reads again, unless a read is already under way. The last answer stays
    /// in place until the new one lands.
    pub fn refresh(&mut self) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read(&root));
        });
        self.rx = Some(rx);
        self.asked = Instant::now();
    }

    /// How long ago the current read, or the last one, was asked for.
    pub fn since_asked(&self) -> Duration {
        self.asked.elapsed()
    }

    pub fn in_flight(&self) -> bool {
        self.rx.is_some()
    }

    /// Takes the answer if it has arrived. Cheap to call every frame.
    pub fn poll(&mut self) -> &Loading {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(facts) => {
                    self.state = Loading::Done(Box::new(facts));
                    self.rx = None;
                }
                // The reader died without answering. Let the next refresh try.
                Err(TryRecvError::Disconnected) => self.rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        &self.state
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let (code, out, err) = git_raw(root, args)?;
    if code != 0 {
        return Err(err);
    }
    Ok(out)
}

/// Runs git and returns its exit code with both streams, for the commands
/// (merge-tree, a config lookup) whose non-zero exit is an answer rather than
/// a failure. `Err` means git did not run at all.
///
/// Built only by `host_git`, never as a bare `git`: a repository's own config
/// names commands git will execute (`core.fsmonitor`, hooks, filter drivers),
/// and this runs every few seconds, outside any sandbox, on whatever folder
/// the window has open. `host_git` also starts git without a console window,
/// which a windowed program otherwise gets once per spawn.
fn git_raw(root: &Path, args: &[&str]) -> Result<(i32, String, String), String> {
    let mut command = vitna_git_workspaces::host_git::command(root)?;
    let output = command
        .args(args)
        .output()
        .map_err(|e| format!("git could not be run: {e}"))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn read(root: &Path) -> RepoFacts {
    let mut facts = RepoFacts::default();
    let (touched, scanned, capped) = crate::workspace::newest_mtime(root);
    facts.touched = touched;
    facts.entries_scanned = scanned;
    facts.scan_was_capped = capped;

    match git_raw(root, &["rev-parse", "--git-dir"]) {
        Ok((0, _, _)) => {}
        // Outside a repository there is nothing more to ask, and that is a
        // state rather than a fault: the bar says "no repository" off the
        // workspace.
        Ok(_) => return facts,
        // git is missing, or would not be run here safely. That is a fault,
        // and the bar names it rather than reading as a folder with no
        // repository in it.
        Err(e) => {
            facts.trouble = Some(e);
            return facts;
        }
    }

    if let Ok(url) = git(root, &["config", "--get", "remote.origin.url"]) {
        let url = url.trim().to_string();
        facts.repo_name = repo_name(&url);
        facts.origin_url = Some(url).filter(|u| !u.is_empty());
    }

    // The hash, the commit time and the subject in one call, tab separated.
    if let Ok(line) = git(root, LOG) {
        let mut parts = line.trim_end().splitn(3, '\t');
        facts.head = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
        facts.committed = parts
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .map(|t| SystemTime::UNIX_EPOCH + Duration::from_secs(t));
        facts.subject = parts.next().map(str::to_string);
    }
    facts.describe = git(root, &["describe", "--tags", "--always"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Upstream first: without one, ahead and behind are not merely unknown,
    // they are undefined, and the window says so rather than printing zero.
    match git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]) {
        Ok(name) => facts.upstream = Some(name.trim().to_string()).filter(|n| !n.is_empty()),
        Err(e) => {
            if !(e.contains("no upstream") || e.contains("does not point to a branch")) {
                facts.trouble.get_or_insert(e);
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

    match git(root, STATUS) {
        Ok(status) => {
            let (mut staged, mut modified, mut untracked) = (0usize, 0usize, 0usize);
            for line in status.lines() {
                let bytes = line.as_bytes();
                if bytes.len() < 2 {
                    continue;
                }
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
            facts.trouble.get_or_insert(e);
        }
    }

    // Lines against HEAD, staged or not. A binary file reports "-" and adds
    // no lines, which is the honest count for it.
    match git(root, DIFF) {
        Ok(out) => {
            let (mut added, mut removed) = (0u64, 0u64);
            for line in out.lines() {
                let mut cols = line.split('\t');
                added += cols.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                removed += cols.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            }
            facts.added = Some(added);
            facts.removed = Some(removed);
        }
        Err(e) => {
            facts.trouble.get_or_insert(e);
        }
    }

    // Where a merge would go, and whether it would go cleanly.
    if let Some(base) = base_branch(root) {
        if let Ok(counts) = git(root, &["rev-list", "--left-right", "--count", &format!("{base}...HEAD")]) {
            let mut parts = counts.split_whitespace();
            facts.base_behind = parts.next().and_then(|s| s.parse().ok());
            facts.base_ahead = parts.next().and_then(|s| s.parse().ok());
        }
        facts.merge = match facts.base_ahead {
            Some(0) => Some(Merge::UpToDate),
            Some(_) => merge_state(root, &base),
            None => None,
        };
        facts.base = Some(base);
    }

    facts.fetched = git(root, &["rev-parse", "--git-path", "FETCH_HEAD"])
        .ok()
        .and_then(|p| {
            let p = PathBuf::from(p.trim());
            let p = if p.is_relative() { root.join(p) } else { p };
            std::fs::metadata(p).ok()?.modified().ok()
        });

    facts
}

/// `git log` for HEAD's hash, time and subject. `--no-show-signature` because
/// `log.showSignature` in the repository's config makes log check a signed
/// commit by running `gpg.program`, which the same config names, and nothing
/// in `host_git` covers that. Measured on git 2.55: plain `git log` ran it.
const LOG: &[&str] = &["log", "-1", "--no-show-signature", "--format=%h%x09%ct%x09%s"];

/// `git status`, leaving submodule working trees alone: status would otherwise
/// run in each one, under that submodule's own config.
const STATUS: &[&str] = &["status", "--porcelain=v1", "--untracked-files=normal", "--ignore-submodules=dirty"];

/// `git diff` in lines against HEAD. `host_git` asks every diff-producing
/// command for `--no-ext-diff` and `--no-textconv`: both run programs chosen
/// by attributes, which the repository writes.
const DIFF: &[&str] = &[
    "diff",
    "--numstat",
    "--no-ext-diff",
    "--no-textconv",
    "--ignore-submodules=dirty",
    "HEAD",
];

/// Merge drivers the repository's own config declares (`merge.<name>.driver`
/// in local or worktree scope), by name. Global config is the operator's own,
/// and is trusted the way `host_git` trusts global filter drivers.
fn repo_merge_drivers(root: &Path) -> Result<Vec<String>, String> {
    let (code, out, err) = git_raw(
        root,
        &["config", "--show-scope", "--name-only", "--get-regexp", r"^merge\..*\.driver$"],
    )?;
    match code {
        0 => Ok(out
            .lines()
            .filter_map(|line| {
                let (scope, key) = line.split_once('\t')?;
                if scope != "local" && scope != "worktree" {
                    return None;
                }
                key.strip_prefix("merge.")?.strip_suffix(".driver").map(str::to_string)
            })
            .collect()),
        // No key matched.
        1 => Ok(Vec::new()),
        _ => Err(err),
    }
}

/// Asks git whether HEAD merges into `base`, without touching the checkout.
fn merge_state(root: &Path, base: &str) -> Option<Merge> {
    // A merge driver is a command merge-tree runs for each path the
    // repository's attributes route to it, so one the repository names is
    // never run. Nor is there a harmless value to put in its place: an emptied
    // driver fails, and every path it covers then reads as a conflict that may
    // not exist. So the check is skipped, and the bar says why.
    match repo_merge_drivers(root) {
        Ok(names) if names.is_empty() => {}
        Ok(names) => {
            return Some(Merge::NotRun(format!(
                "This repository's own config names a merge driver ({}), a command git merge-tree \
                 would run. The window does not run commands a repository names.",
                names.join(", ")
            )))
        }
        Err(e) => {
            return Some(Merge::NotRun(format!(
                "The repository's merge drivers could not be read, so merge-tree was not run: {e}"
            )))
        }
    }
    let (code, out, _) = git_raw(
        root,
        &["merge-tree", "--write-tree", "--name-only", "--no-messages", base, "HEAD"],
    )
    .ok()?;
    match code {
        0 => Some(Merge::Clean),
        // Exit 1 is the answer "conflicts": the tree's id, then one line per
        // conflicted path.
        1 => Some(Merge::Conflicts(
            out.lines()
                .skip(1)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        )),
        // Anything else is git failing to answer, say one too old for
        // --write-tree, which is no verdict either way.
        _ => None,
    }
}

/// The branch this one would merge into: what origin says its default is, or
/// failing that a conventional name, if one exists here.
fn base_branch(root: &Path) -> Option<String> {
    if let Ok(s) = git(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]) {
        let s = s.trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    ["origin/main", "origin/master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{b}")]).is_ok())
        .map(str::to_string)
}

/// A repository's name off its remote URL: the last path segment, without
/// `.git`. Takes https, scp-style `git@host:owner/name`, and local paths.
pub fn repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    // 92 is a backslash, which separates a Windows path.
    let name = trimmed
        .rsplit(|c: char| c == '/' || c == ':' || c == char::from(92u8))
        .next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("../tests/support/plain_git.rs");

    #[test]
    fn a_repository_is_named_off_its_remote() {
        let name = |u: &str| repo_name(u);
        assert_eq!(name("https://github.com/convexityos/vitna-code.git").as_deref(), Some("vitna-code"));
        assert_eq!(name("git@github.com:convexityos/vitna-code.git").as_deref(), Some("vitna-code"));
        assert_eq!(name("https://example.com/a/b/").as_deref(), Some("b"));
        let windows = format!("C:{0}src{0}thing", char::from(92u8));
        assert_eq!(name(&windows).as_deref(), Some("thing"));
        assert_eq!(name(""), None);
    }

    /// merge-tree's two answers, from a throwaway repository: a branch that
    /// merges cleanly, then the same branch once the base edits its line.
    #[test]
    fn the_merge_check_reads_clean_and_conflicted() {
        let dir = std::env::temp_dir().join(format!("vitna_gui_merge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let run = |args: &[&str]| {
            let out = plain_git(&dir, args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").expect("write");
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "base"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&["checkout", "-q", "-b", "work"]);
        std::fs::write(dir.join("b.txt"), "two\n").expect("write");
        run(&["add", "b.txt"]);
        run(&["commit", "-q", "-m", "work"]);

        assert_eq!(base_branch(&dir).as_deref(), Some("origin/main"));
        assert_eq!(merge_state(&dir, "origin/main"), Some(Merge::Clean));

        run(&["checkout", "-q", "main"]);
        std::fs::write(dir.join("a.txt"), "base\n").expect("write");
        run(&["commit", "-q", "-am", "base edit"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&["checkout", "-q", "work"]);
        std::fs::write(dir.join("a.txt"), "work\n").expect("write");
        run(&["commit", "-q", "-am", "work edit"]);

        let facts = read(&dir);
        assert_eq!(facts.base.as_deref(), Some("origin/main"));
        assert_eq!(facts.base_ahead, Some(2));
        assert_eq!(facts.base_behind, Some(1));
        assert_eq!(facts.merge, Some(Merge::Conflicts(vec!["a.txt".to_string()])));
        assert_eq!(facts.repo_name, None, "no origin, so no name to read");
        assert_eq!(facts.added, Some(0), "a clean checkout adds no lines");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The flags that stop what `host_git` does not, pinned by name: the diff
    /// ones have no behaviour to observe on git 2.55, where `--numstat` ran
    /// neither a textconv nor an external diff even without them.
    #[test]
    fn the_window_asks_git_for_its_own_protections() {
        assert!(LOG.contains(&"--no-show-signature"));
        for flag in ["--no-ext-diff", "--no-textconv", "--ignore-submodules=dirty"] {
            assert!(DIFF.contains(&flag), "diff is missing {flag}");
        }
        assert!(STATUS.contains(&"--ignore-submodules=dirty"));
    }

    /// Everything `read` asks git, against a repository whose own config
    /// names five commands: an fsmonitor, a clean filter and an index hook,
    /// which `host_git` stops, and a gpg program behind `log.showSignature`
    /// and a merge driver, which the window stops itself. Each is first shown
    /// to run under plain git, because a marker that stays absent proves
    /// nothing on a git build that never had the vector.
    #[test]
    fn the_window_runs_nothing_the_repository_names() {
        let root = std::env::temp_dir().join(format!("vitna_gui_hostile_{}", std::process::id()));
        let repo = root.join("repo");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&repo).expect("temp dir");
        let run = |args: &[&str]| {
            let out = plain_git(&repo, args);
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // Git runs these through sh, even on Windows, so forward slashes.
        let slashed = |p: &Path| p.to_string_lossy().replace(char::from(92u8), "/");
        let mark = |name: &str| slashed(&root.join(name));
        let executable = |path: &Path, body: String| {
            std::fs::write(path, body).expect("write script");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            }
        };

        // Two branches that conflict on a.txt, with origin/main as the base.
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").expect("write");
        std::fs::write(repo.join("b.txt"), "two\n").expect("write");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "base"]);
        run(&["checkout", "-q", "-b", "work"]);
        std::fs::write(repo.join("a.txt"), "work\n").expect("write");
        run(&["commit", "-q", "-am", "work"]);
        run(&["checkout", "-q", "main"]);
        std::fs::write(repo.join("a.txt"), "main\n").expect("write");
        run(&["commit", "-q", "-am", "main"]);
        run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&["checkout", "-q", "work"]);

        // HEAD becomes a commit with a signature header, which is all log
        // needs to go looking for gpg. The signature itself is not real.
        let tree = run(&["rev-parse", "HEAD^{tree}"]);
        let parent = run(&["rev-parse", "HEAD"]);
        let body = format!(
            "tree {tree}\nparent {parent}\nauthor t <t@t> 1700000000 +0000\n\
             committer t <t@t> 1700000000 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n \
             -----END PGP SIGNATURE-----\n\nsigned work\n"
        );
        let signed = plain_git_input(&repo, &["hash-object", "-t", "commit", "-w", "--stdin"], &body);
        assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
        let signed = String::from_utf8_lossy(&signed.stdout).trim().to_string();
        run(&["update-ref", "refs/heads/work", &signed]);

        // Armed after every commit, so each blob holds the raw bytes and a
        // neutralized filter still compares equal.
        let gpg = root.join("fake-gpg");
        executable(&gpg, format!("#!/bin/sh\necho x > \"{}\"\nexit 1\n", mark("MARK_GPG")));
        let hooks = repo.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).expect("hooks dir");
        executable(&hooks.join("post-index-change"), format!("#!/bin/sh\necho x > \"{}\"\n", mark("MARK_HOOK")));
        run(&["config", "core.fsmonitor", &format!("echo x > \"{}\" #", mark("MARK_FSMONITOR"))]);
        run(&["config", "filter.eek.clean", &format!("echo x > \"{}\"; cat", mark("MARK_FILTER"))]);
        run(&["config", "log.showSignature", "true"]);
        run(&["config", "gpg.program", &slashed(&gpg)]);
        run(&["config", "merge.vx.driver", &format!("echo x > \"{}\"; exit 1", mark("MARK_MERGE"))]);
        let info = repo.join(".git").join("info");
        std::fs::create_dir_all(&info).expect("info dir");
        std::fs::write(info.join("attributes"), "* filter=eek merge=vx\n").expect("attributes");

        const MARKS: [&str; 5] = ["MARK_FSMONITOR", "MARK_FILTER", "MARK_HOOK", "MARK_GPG", "MARK_MERGE"];
        let fired = |name: &str| root.join(name).exists();
        let reset = || {
            for m in MARKS {
                let _ = std::fs::remove_file(root.join(m));
            }
            // Identical bytes: mtime moves and size does not, so git hashes
            // the file again, and hashing is what reaches the clean filter.
            std::fs::write(repo.join("b.txt"), "two\n").expect("restat");
        };

        // Control: plain git, the way the window used to run it.
        reset();
        plain_git(&repo, &["status", "--porcelain=v1"]);
        plain_git(&repo, &["log", "-1", "--format=%h%x09%ct%x09%s"]);
        plain_git(&repo, &["merge-tree", "--write-tree", "--name-only", "--no-messages", "origin/main", "HEAD"]);
        let live: Vec<&str> = MARKS.into_iter().filter(|m| fired(m)).collect();
        eprintln!("control: vectors live on this git build = {live:?}");
        assert!(live.contains(&"MARK_GPG"), "plain git log ran no gpg program: the control proves nothing");
        assert!(live.contains(&"MARK_MERGE"), "plain merge-tree ran no merge driver: the control proves nothing");

        // The window: everything read() asks.
        reset();
        let facts = read(&repo);
        for m in &live {
            assert!(!fired(m), "{m} ran under the window's git");
        }
        assert_eq!(facts.trouble, None);
        assert_eq!(facts.subject.as_deref(), Some("signed work"), "log still answers");
        assert_eq!(facts.modified, Some(0), "a neutralized filter still compares equal");
        match &facts.merge {
            Some(Merge::NotRun(why)) => assert!(why.contains("vx"), "the reason names the driver: {why}"),
            other => panic!("the merge check ran, or gave no reason: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}
