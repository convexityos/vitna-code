//! The system file's working-tree keys reach hardened git, and nothing else in
//! it does.
//!
//! `host_git` hides the system config, which is where Git for Windows keeps
//! `core.autocrlf=true`, and without that key git reads a CRLF checkout as if
//! every line of an edited file had changed. This test points the system and
//! global config at fixture files for its own process, which is why it is the
//! only test in this binary: the variables are process-wide.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use vitna_git_workspaces::host_git;

fn git(repo: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git runs")
}

fn stdout(out: Output) -> String {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const NUMSTAT: [&str; 5] = ["diff", "--numstat", "--no-ext-diff", "--no-textconv", "HEAD"];

#[test]
fn system_working_tree_keys_carry_over_and_nothing_else_does() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not available, skipping");
        return;
    }
    let root = std::env::temp_dir().join(format!("vitna_host_git_system_{}", std::process::id()));
    let repo = root.join("repo");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&repo).expect("create repo");
    let marker = root.join("MARK_SYSTEM_FILTER");
    let marker_arg = marker.to_string_lossy().replace(char::from(92u8), "/");

    // The system file these gits read holds the key Git for Windows ships,
    // and a filter, which has to stay hidden. The global file is empty, so
    // nothing on the machine running this can answer for either.
    let system = root.join("system.gitconfig");
    let global = root.join("global.gitconfig");
    fs::write(&global, "").expect("write global config");
    // Written by git, which quotes the value: in a config file `;` begins a
    // comment, so the filter written by hand loses its `cat`.
    let filter = format!("echo x > \"{marker_arg}\"; cat");
    for (key, value) in [("core.autocrlf", "true"), ("filter.sx.clean", filter.as_str())] {
        let file = system.to_string_lossy().to_string();
        assert!(git(&root, &["config", "--file", &file, key, value]).status.success(), "write {key}");
    }
    std::env::set_var("GIT_CONFIG_SYSTEM", &system);
    std::env::set_var("GIT_CONFIG_GLOBAL", &global);
    std::env::remove_var("GIT_CONFIG_NOSYSTEM");

    assert!(git(&repo, &["init", "-q", "."]).status.success(), "git init");
    assert!(git(&repo, &["config", "user.email", "t@example.invalid"]).status.success());
    assert!(git(&repo, &["config", "user.name", "vitna test"]).status.success());
    fs::write(repo.join("f.txt"), "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n").expect("write");
    assert!(git(&repo, &["add", "f.txt"]).status.success());
    assert!(git(&repo, &["commit", "-qm", "base"]).status.success());

    // Checked out again, so autocrlf writes it with CRLF, as a Windows
    // checkout is, then one line edited with its line endings kept.
    fs::remove_file(repo.join("f.txt")).expect("remove");
    assert!(git(&repo, &["checkout", "-q", "--", "f.txt"]).status.success());
    let text = fs::read_to_string(repo.join("f.txt")).expect("read");
    assert!(text.contains("\r\n"), "autocrlf from the system file did not apply to the checkout");
    fs::write(repo.join("f.txt"), text.replace("l4", "L4")).expect("edit");
    let info = repo.join(".git").join("info");
    fs::create_dir_all(&info).expect("info dir");
    fs::write(info.join("attributes"), "* filter=sx\n").expect("attributes");

    // Controls: the operator's git counts one line and runs the system
    // filter; with the system file hidden and nothing carried over, all
    // eight lines read as changed.
    let _ = fs::remove_file(&marker);
    assert_eq!(stdout(git(&repo, &NUMSTAT)), "1\t1\tf.txt");
    assert!(marker.exists(), "control: plain git did not run the system filter");
    let hidden = Command::new("git")
        .current_dir(&repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(NUMSTAT)
        .output()
        .expect("git runs");
    assert_eq!(stdout(hidden), "8\t8\tf.txt", "control: hiding the system file changed nothing");

    // Hardened: the count the operator's git gives, and no system filter.
    let _ = fs::remove_file(&marker);
    let hardened = host_git::command(&repo).expect("hardened command").args(NUMSTAT).output();
    assert_eq!(stdout(hardened.expect("git runs")), "1\t1\tf.txt");
    assert!(!marker.exists(), "a filter from the system file ran under hardened git");

    // A scope above the system file still wins: set locally, the carried
    // value gives way, and both gits agree again.
    assert!(git(&repo, &["config", "core.autocrlf", "false"]).status.success());
    let plain = stdout(git(&repo, &NUMSTAT));
    let hardened = host_git::command(&repo).expect("hardened command").args(NUMSTAT).output();
    assert_eq!(stdout(hardened.expect("git runs")), plain);
    assert_eq!(plain, "8\t8\tf.txt");

    std::env::remove_var("GIT_CONFIG_SYSTEM");
    std::env::remove_var("GIT_CONFIG_GLOBAL");
    let _ = fs::remove_dir_all(&root);
}
