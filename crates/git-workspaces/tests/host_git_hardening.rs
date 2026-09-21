//! Proof that host-side git does not run what the repository tells it to.
//!
//! Every assertion here comes in a pair. The control runs plain `git`, the way
//! `git_status` and `determine_base_commit` used to, and requires the marker to
//! appear: without it a "no marker" result proves nothing, since it is also
//! what a git version that never had the vector would produce. The hardened run
//! then requires the same marker to stay absent.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use vitna_git_workspaces::host_git;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn marker(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Shell-safe, forward-slashed form. Git runs these through `sh` even on
    /// Windows, where a backslash would be an escape.
    fn marker_arg(&self, name: &str) -> String {
        self.marker(name).to_string_lossy().replace('\\', "/")
    }

    fn clear_markers(&self) {
        for name in ["MARK_FSMONITOR", "MARK_HOOK", "MARK_FILTER"] {
            let _ = fs::remove_file(self.marker(name));
        }
    }

    fn fired(&self, name: &str) -> bool {
        self.marker(name).exists()
    }

    /// Rewrites identical bytes, which moves mtime while leaving size alone.
    /// That is the state that makes git re-hash the file, and re-hashing is
    /// what reaches the clean filter.
    fn restat_tracked_file(&self) {
        fs::write(self.repo.join("file.txt"), "hello\n").expect("rewrite tracked file");
    }
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn setup(case: &str) -> Option<Fixture> {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not available, skipping");
        return None;
    }

    let root = std::env::temp_dir().join(format!("vitna_host_git_{}_{}", std::process::id(), case));
    let repo = root.join("repo");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&repo).expect("create repo dir");

    let fx = Fixture { root, repo };

    assert!(git_ok(&fx.repo, &["init", "-q", "."]), "git init");
    assert!(git_ok(&fx.repo, &["config", "user.email", "t@example.invalid"]));
    assert!(git_ok(&fx.repo, &["config", "user.name", "vitna test"]));

    fs::write(fx.repo.join("file.txt"), "hello\n").expect("write file");
    fs::write(fx.repo.join(".gitattributes"), "* filter=eek\n").expect("write attributes");
    assert!(git_ok(&fx.repo, &["add", "-A"]));
    assert!(git_ok(&fx.repo, &["commit", "-qm", "init"]), "git commit");

    // Armed only after the commit, so the stored blob is the raw bytes and a
    // neutralized filter still compares equal. A repository that reports
    // spurious modifications once hardened would be its own kind of defect.
    assert!(git_ok(
        &fx.repo,
        &[
            "config",
            "core.fsmonitor",
            &format!("echo x > \"{}\" #", fx.marker_arg("MARK_FSMONITOR")),
        ]
    ));
    assert!(git_ok(
        &fx.repo,
        &[
            "config",
            "filter.eek.clean",
            &format!("echo x > \"{}\"; cat", fx.marker_arg("MARK_FILTER")),
        ]
    ));

    let hooks = fx.repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).expect("create hooks dir");
    let hook = hooks.join("post-index-change");
    fs::write(
        &hook,
        format!("#!/bin/sh\necho x > \"{}\"\n", fx.marker_arg("MARK_HOOK")),
    )
    .expect("write hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("chmod hook");
    }

    Some(fx)
}

#[test]
fn hardened_git_does_not_run_repository_controlled_commands() {
    let Some(fx) = setup("neutralize") else { return };

    // Control: plain git, exactly what the two callers used to build.
    fx.clear_markers();
    fx.restat_tracked_file();
    let _ = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(&fx.repo)
        .output()
        .expect("plain git status");

    let control = [
        ("MARK_FSMONITOR", fx.fired("MARK_FSMONITOR")),
        ("MARK_HOOK", fx.fired("MARK_HOOK")),
        ("MARK_FILTER", fx.fired("MARK_FILTER")),
    ];
    let live: Vec<&str> = control
        .iter()
        .filter(|(_, fired)| *fired)
        .map(|(n, _)| *n)
        .collect();
    assert!(
        !live.is_empty(),
        "control proved nothing: plain git ran none of the three vectors, so this \
         git build cannot demonstrate the hardening. Vectors: {:?}",
        control
    );
    // Printed so a CI log records which vectors this git build actually has.
    eprintln!("control: vectors live on this git build = {:?}", live);

    // Hardened: the same repository, through the only constructor callers use.
    fx.clear_markers();
    fx.restat_tracked_file();
    let output = host_git::command(&fx.repo)
        .expect("hardened command")
        .args(["status", "--porcelain=v1", "--ignore-submodules=dirty"])
        .output()
        .expect("hardened git status");

    assert!(
        output.status.success(),
        "hardened git status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for name in live {
        assert!(
            !fx.fired(name),
            "{} executed under hardened git: the repository still reaches a shell",
            name
        );
    }

    // Hardening must not turn a clean tree dirty. The blob was written without
    // the filter, so a neutralized filter has to compare equal.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("file.txt"),
        "hardened status reported a spurious modification: {:?}",
        stdout
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// Two of the protections cannot be told apart by behaviour.
///
/// `core.hooksPath` pointing at an empty directory and `--no-optional-locks`
/// each block the `post-index-change` hook on their own: the first leaves no
/// hook to find, the second never writes the index that would trigger one.
/// Removing either alone therefore keeps the behavioural test green, which
/// means that test cannot pin them. Measured by deleting each in turn: with
/// only one gone, the hook still did not run.
///
/// So they are pinned structurally instead. This asserts presence, not effect,
/// and is deliberately paired with the behavioural test above rather than
/// replacing it.
#[test]
fn every_protection_is_present_on_the_built_command() {
    let Some(fx) = setup("structure") else { return };

    let cmd = host_git::command(&fx.repo).expect("hardened command");

    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    for expected in [
        "--no-pager",
        "--no-optional-locks",
        "core.fsmonitor=false",
        "core.pager=cat",
        "diff.external=",
    ] {
        assert!(
            args.iter().any(|a| a == expected),
            "missing protection {:?} in {:?}",
            expected,
            args
        );
    }
    assert!(
        args.iter().any(|a| a.starts_with("core.hooksPath=")),
        "missing core.hooksPath override in {:?}",
        args
    );

    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|v| v.to_string_lossy().to_string()),
            )
        })
        .collect();
    assert!(
        envs.contains(&("GIT_CONFIG_NOSYSTEM".to_string(), Some("1".to_string()))),
        "missing GIT_CONFIG_NOSYSTEM in {:?}",
        envs
    );
    // Removal is recorded as a None value.
    for removed in ["GIT_EXTERNAL_DIFF", "GIT_PAGER"] {
        assert!(
            envs.contains(&(removed.to_string(), None)),
            "{} is not cleared in {:?}",
            removed,
            envs
        );
    }

    let _ = fs::remove_dir_all(&fx.root);
}

#[test]
fn hardened_git_reports_real_changes() {
    let Some(fx) = setup("realchanges") else { return };

    fs::write(fx.repo.join("file.txt"), "changed\n").expect("modify");
    let output = host_git::command(&fx.repo)
        .expect("hardened command")
        .args(["status", "--porcelain=v1", "--ignore-submodules=dirty"])
        .output()
        .expect("hardened git status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("file.txt"),
        "hardened status missed a real modification: {:?}",
        stdout
    );

    let _ = fs::remove_dir_all(&fx.root);
}

#[test]
fn unneutralizable_filter_driver_name_fails_closed() {
    let Some(fx) = setup("failclosed") else { return };

    // `=` would be swallowed by git's own `-c name=value` split, leaving the
    // real driver armed. Refusing is the only safe answer.
    assert!(git_ok(
        &fx.repo,
        &["config", "filter.a=b.clean", "echo owned"]
    ));

    let result = host_git::command(&fx.repo);
    assert!(
        result.is_err(),
        "a filter driver that cannot be neutralized must fail the invocation"
    );

    let _ = fs::remove_dir_all(&fx.root);
}
