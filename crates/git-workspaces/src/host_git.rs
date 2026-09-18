//! Hardened host-side git invocation.
//!
//! Git reads configuration out of the repository it is pointed at, and several
//! of those keys hold commands git will execute. `git status` alone runs three
//! of them, all confirmed against git 2.55:
//!
//! 1. `core.fsmonitor`, queried through the shell whenever the index is
//!    refreshed.
//! 2. The `post-index-change` hook, run whenever status writes the index back
//!    after refreshing it.
//! 3. `filter.<driver>.clean`, run when an entry is stat dirty and git has to
//!    re-hash the working tree file to decide whether it changed. A file whose
//!    mtime moved while its size did not is enough; a build touching files
//!    produces that state routinely.
//!
//! None of this needs a hook file to be committed, and none of it needs the
//! operator to run anything unusual. Anything able to write `.git/config` gets
//! code execution with the operator's full privileges the next time Vitna looks
//! at the repository, outside any sandbox and with no approval prompt, because
//! `git_status` is a read-only tool that does not ask for one.
//!
//! Every host-side git invocation goes through [`command`]. It is deliberately
//! the only constructor: a bare `Command::new("git")` anywhere in the workspace
//! is the defect this module exists to remove.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Characters a repository-scoped filter driver name may contain for this
/// module to be able to neutralize it.
///
/// Git accepts far more than this in a subsection name, including `=`. A `=`
/// matters because neutralization is applied as `-c filter.<name>.clean=`, and
/// git splits a `-c` argument at its first `=`: a driver named `a=b` would turn
/// that into a write to `filter.a` and leave the real driver armed. The name
/// cannot escape the `filter.` namespace, so this is not a route to setting
/// `core.fsmonitor`, but it is a route to a filter surviving the scrub. A name
/// outside this set therefore fails the whole invocation rather than producing
/// a half-neutralized command.
fn is_neutralizable_driver_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

#[cfg(windows)]
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

/// An empty directory this process owns, used as `core.hooksPath`.
///
/// Pointing `core.hooksPath` at a directory with no hooks in it is what stops
/// `post-index-change` and every other hook a repository can carry. The
/// directory must be outside the workspace, or the sandboxed process it is
/// meant to defend against could simply write a hook into it.
///
/// Emptiness is re-checked on every call and a non-empty directory is a hard
/// error, because the whole value of this path is that there is nothing in it
/// to run.
pub fn empty_hooks_dir() -> Result<PathBuf, String> {
    let base = state_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("vitna").join("empty-hooks");

    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create empty hooks directory {}: {}", dir.display(), e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }

    let mut entries = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read empty hooks directory {}: {}", dir.display(), e))?;
    if let Some(entry) = entries.next() {
        let name = entry
            .map(|e| e.file_name().to_string_lossy().to_string())
            .unwrap_or_else(|_| "<unreadable>".to_string());
        return Err(format!(
            "Refusing to run git: hooks directory {} is not empty (found '{}'). \
             A file there would be executed as a git hook.",
            dir.display(),
            name
        ));
    }

    Ok(dir)
}

/// Repository-scoped filter driver names, as `git config` reports them.
///
/// Only `local` and `worktree` scopes are collected. Global and system scopes
/// are the operator's own configuration and are trusted; neutralizing those
/// would break a working git-lfs install for every repository. Includes are
/// followed, so a `include.path` in `.git/config` pointing at a file inside the
/// working tree is covered.
///
/// Reading configuration executes nothing, so this call only needs the
/// protections that keep it from being the thing that runs a hook.
fn repo_scoped_filter_drivers(repo_dir: &Path, hooks_dir: &Path) -> Result<Vec<String>, String> {
    let output = base_command(repo_dir, hooks_dir)
        .args([
            "config",
            "--show-scope",
            "--name-only",
            "--get-regexp",
            r"^filter\.",
        ])
        .output();

    let output = match output {
        Ok(o) => o,
        // git missing entirely is not this function's problem to report; the
        // caller's own invocation will fail with a better message.
        Err(_) => return Ok(Vec::new()),
    };

    // Exit code 1 with no output is "no matches", which is the common case.
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut drivers: Vec<String> = Vec::new();

    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let scope = parts.next().unwrap_or_default().trim();
        let key = parts.next().unwrap_or_default().trim();
        if scope != "local" && scope != "worktree" {
            continue;
        }
        let Some(rest) = key.strip_prefix("filter.") else {
            continue;
        };
        // `filter.<driver>.<key>`, and <driver> may itself contain dots.
        let Some((driver, _)) = rest.rsplit_once('.') else {
            continue;
        };
        if !is_neutralizable_driver_name(driver) {
            return Err(format!(
                "Refusing to run git: repository configures filter driver '{}', whose name \
                 cannot be safely neutralized on a git command line.",
                driver
            ));
        }
        if !drivers.iter().any(|d| d == driver) {
            drivers.push(driver.to_string());
        }
    }

    Ok(drivers)
}

/// The protections that do not depend on reading the repository first.
fn base_command(repo_dir: &Path, hooks_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_dir);

    // Ignore /etc/gitconfig. Note that on Git for Windows this is also where a
    // git-lfs install writes its filter driver, so an LFS working tree may
    // report spurious modifications from these invocations.
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    // Never block waiting for a credential or passphrase prompt.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    // Inherited pager and external-diff selections are not used either.
    cmd.env_remove("GIT_EXTERNAL_DIFF");
    cmd.env_remove("GIT_PAGER");

    cmd.arg("--no-pager");
    // Do not take the index lock to write back a refreshed index. Besides
    // avoiding a race with the operator's own git, this is what keeps status
    // from reaching the post-index-change hook at all.
    cmd.arg("--no-optional-locks");
    cmd.args(["-c", "core.fsmonitor=false"]);
    cmd.args(["-c", "core.pager=cat"]);
    cmd.args(["-c", "diff.external="]);
    cmd.arg("-c");
    cmd.arg(format!("core.hooksPath={}", hooks_dir.display()));

    cmd
}

/// Builds a `git` command for `repo_dir` with repository-controlled execution
/// keys neutralized.
///
/// Returns an error rather than a weaker command when a protection cannot be
/// established. A caller that cannot harden git must not run git.
///
/// Subcommands that produce diffs must additionally pass `--no-ext-diff` and
/// `--no-textconv`; `diff.external` is cleared here, but `diff.<driver>.textconv`
/// is selected by attributes and is not enumerable the way filter drivers are.
pub fn command<P: AsRef<Path>>(repo_dir: P) -> Result<Command, String> {
    let repo_dir = repo_dir.as_ref();
    let hooks_dir = empty_hooks_dir()?;

    let mut cmd = base_command(repo_dir, &hooks_dir);

    for driver in repo_scoped_filter_drivers(repo_dir, &hooks_dir)? {
        for key in ["clean", "smudge", "process"] {
            cmd.arg("-c");
            cmd.arg(format!("filter.{}.{}=", driver, key));
        }
        // An empty command with `required` still set makes git fail the whole
        // operation rather than skip the filter.
        cmd.arg("-c");
        cmd.arg(format!("filter.{}.required=false", driver));
    }

    Ok(cmd)
}

/// Reports the local git version, or None when git cannot be run.
///
/// `git --version` answers before it ever looks for a repository, so this one
/// is not a way in. It lives here anyway so that "nothing builds git directly"
/// stays a rule with no exceptions for a reader to weigh.
pub fn version() -> Option<String> {
    let output = Command::new("git")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("--no-pager")
        .arg("--version")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_driver_name_validation() {
        assert!(is_neutralizable_driver_name("lfs"));
        assert!(is_neutralizable_driver_name("my.driver"));
        assert!(is_neutralizable_driver_name("a_b-c.1"));
        assert!(!is_neutralizable_driver_name(""));
        // The one that matters: `=` would be parsed as the -c separator.
        assert!(!is_neutralizable_driver_name("a=b"));
        assert!(!is_neutralizable_driver_name("a b"));
        assert!(!is_neutralizable_driver_name("a\nb"));
    }

    #[test]
    fn test_empty_hooks_dir_is_empty() {
        let dir = empty_hooks_dir().expect("hooks dir");
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).expect("read").count(), 0);
    }
}
