// Plain `git`, for the window's tests and nothing else.
//
// Tests are the one place a bare `git` belongs: they build fixture
// repositories the way a person would, and plain git is the control that
// proves a repository-controlled command really runs before a test asserts
// that the window's own git does not run it. The window's tests live in
// `#[cfg(test)]` modules under `src/`, where
// `crates/git-workspaces/tests/no_bare_git_invocations.rs` cannot tell them
// from shipped code, since it exempts test files by their path. So the plain
// git they need lives here, under `tests/`, and they `include!` it. This
// file is not a test target of its own: cargo only builds `tests/*.rs`.

/// Runs plain git in `dir` as a fixed test identity that signs nothing.
#[allow(dead_code)]
fn plain_git(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git runs")
}

/// Plain git with `input` on its standard input.
#[allow(dead_code)]
fn plain_git_input(dir: &std::path::Path, args: &[&str], input: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git runs");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("stdin written");
    child.wait_with_output().expect("git finishes")
}
