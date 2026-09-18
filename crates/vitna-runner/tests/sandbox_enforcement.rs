//! What the runner actually does, proved by running commands.
//!
//! The sandbox crate's own tests assert on generated arguments and never start
//! a process. That was the entire evidence behind "Strong Sandbox Execution
//! Engine: Verified" while nothing in the product called the crate at all. So
//! these tests go through `ProcessRunner`, the thing `run_command` uses, and
//! judge it by what survives.
//!
//! Set `VITNA_REQUIRE_SANDBOX_BACKEND=<name>` to turn "no backend here, so this
//! case cannot be exercised" from a skip into a failure. CI sets it on the
//! platforms that are supposed to have one, because a test suite that quietly
//! skips its subject is how the original claim survived.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use vitna_runner::{
    sandbox_status, ActionJournal, ActionState, CommandRequest, JournalEntry, ProcessRunner,
    Runner, BACKEND_NONE, SANDBOX_UNAVAILABLE,
};

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    journal: PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!("vitna_sbx_run_{}_{}", std::process::id(), name));
    let workspace = root.join("ws");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(workspace.join(".git")).expect("create workspace");
    fs::write(workspace.join(".git").join("config"), "[core]\n").expect("write git config");
    Fixture {
        journal: root.join("actions.journal"),
        root,
        workspace,
    }
}

/// The last record the runner appended, read back off disk.
fn last_entry(fx: &Fixture) -> JournalEntry {
    ActionJournal::read_log(&fx.journal)
        .expect("read journal")
        .pop()
        .expect("an entry was written")
}

fn runner(fx: &Fixture) -> ProcessRunner {
    ProcessRunner::open_or_create(&fx.journal).expect("open journal")
}

/// What CI insists must be exercised here, if anything.
fn required_backend() -> Option<String> {
    std::env::var("VITNA_REQUIRE_SANDBOX_BACKEND")
        .ok()
        .filter(|v| !v.is_empty())
}

fn backend_or_skip(workspace: &Path, case: &str) -> Option<String> {
    match sandbox_status(workspace, false) {
        Ok((backend, _)) => {
            if let Some(required) = required_backend() {
                assert_eq!(
                    backend, required,
                    "CI requires the {} backend here and got {}",
                    required, backend
                );
            }
            Some(backend)
        }
        Err(reason) => {
            if let Some(required) = required_backend() {
                panic!(
                    "CI requires the {} backend for '{}' and none is available: {}",
                    required, case, reason
                );
            }
            eprintln!("skipping '{}': no sandbox backend here ({})", case, reason);
            None
        }
    }
}

/// A command that leaves a mark from a process that survives its parent.
///
/// The mark has to be written by something a kill aimed at the direct child
/// would miss, or the test cannot tell a process-tree teardown from a plain
/// kill of the shell. Measured: with the marker written by the shell's own
/// command line, disabling the job object still produced no marker, because
/// killing the shell is enough to stop it. On Windows the writer is therefore a
/// detached `start /b` grandchild, which only the job object catches. On Unix
/// it is a backgrounded subshell, which only signalling the process group
/// catches.
///
/// The outer command then waits far longer than the timeout, so the timeout is
/// what ends the action.
fn sleep_then_mark(fx: &Fixture, marker: &Path) -> String {
    let m = marker.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        let script = fx.root.join("grandchild.bat");
        fs::write(
            &script,
            format!("@echo off\r\nping -n 6 127.0.0.1 >nul\r\necho done> \"{}\"\r\n", m),
        )
        .expect("write helper script");
        // The inner `cmd /c` is a grandchild. Confirmed by hand: killing only
        // the outer cmd.exe leaves it running and the marker still appears.
        format!("cmd /c \"{}\"", script.to_string_lossy().replace('\\', "/"))
    } else {
        format!("sh -c 'sleep 5; echo done > \"{}\"'", m)
    }
}

fn write_outside(marker: &Path) -> String {
    let m = marker.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        format!("echo escaped> \"{}\"", m)
    } else {
        format!("echo escaped > \"{}\"", m)
    }
}

/// The defect: `run_command` reached a bare shell without ever consulting the
/// sandbox crate. With no backend available it must now refuse, and refusing
/// means the command does not run.
#[tokio::test]
async fn unsandboxed_execution_is_refused_without_explicit_approval() {
    let fx = fixture("refuse");
    let marker = fx.root.join("RAN_ANYWAY");

    let request = CommandRequest::new(
        write_outside(&marker),
        &fx.workspace,
        &fx.workspace,
        30_000,
    );
    assert!(
        !request.allow_unsandboxed,
        "the default has to be refusal, not permission"
    );

    let result = runner(&fx).run_command(request).await;

    match sandbox_status(&fx.workspace, false) {
        Err(_) => {
            let err = result.expect_err("a command with no sandbox must be refused");
            assert!(
                err.contains(SANDBOX_UNAVAILABLE),
                "the refusal must say it was about the sandbox: {}",
                err
            );
            assert!(
                !marker.exists(),
                "refused means it did not run; the command still executed"
            );
        }
        Ok(_) => {
            // A backend exists, so there was nothing to refuse.
            assert!(result.is_ok(), "sandboxed command should have run");
        }
    }

    let _ = fs::remove_dir_all(&fx.root);
}

/// The refusal is recorded, not just returned.
#[tokio::test]
async fn a_refused_action_is_journalled_as_refused() {
    let fx = fixture("journal_refuse");
    if sandbox_status(&fx.workspace, false).is_ok() {
        eprintln!("skipping: a backend is available, so nothing is refused here");
        return;
    }

    let r = runner(&fx);
    let _ = r
        .run_command(CommandRequest::new(
            "echo hello",
            &fx.workspace,
            &fx.workspace,
            30_000,
        ))
        .await;

    let entry = last_entry(&fx);

    assert_eq!(
        entry.state,
        ActionState::Refused,
        "a command that never ran must not be left looking started"
    );
    assert_eq!(entry.termination.as_deref(), Some("sandbox_unavailable"));

    let _ = fs::remove_dir_all(&fx.root);
}

/// Defect 2: the timeout returned an error and left the process running.
#[tokio::test]
async fn a_timed_out_command_does_not_outlive_the_action() {
    let fx = fixture("timeout");
    let marker = fx.root.join("SURVIVED");

    let mut request = CommandRequest::new(
        sleep_then_mark(&fx, &marker),
        &fx.workspace,
        &fx.workspace,
        800,
    );
    // Explicitly approved, because this test is about the teardown and has to
    // run a process even where no backend exists.
    request.allow_unsandboxed = true;

    let r = runner(&fx);
    let err = r
        .run_command(request)
        .await
        .expect_err("the command must report the timeout");
    assert!(err.contains("timed out"), "unexpected error: {}", err);

    // Outlive the command's own sleep. If the tree was not taken down, the
    // marker lands during this wait.
    tokio::time::sleep(Duration::from_secs(8)).await;

    assert!(
        !marker.exists(),
        "the command kept running after its action was abandoned and wrote {}",
        marker.display()
    );

    let entry = last_entry(&fx);
    assert_eq!(
        entry.state,
        ActionState::NeedsReconciliation,
        "an interrupted command's effect is unknown, so it needs reconciliation"
    );
    assert_eq!(entry.termination.as_deref(), Some("timeout"));
    assert!(
        entry.teardown.is_some(),
        "the journal must record how far the teardown reached"
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// `VITNA_SANDBOX=1` used to be set on a child that nothing was containing.
#[tokio::test]
async fn the_sandbox_variable_is_absent_when_nothing_sandboxed_the_child() {
    let fx = fixture("envvar");
    if sandbox_status(&fx.workspace, false).is_ok() {
        eprintln!("skipping: a backend is available, so the child is genuinely sandboxed");
        return;
    }

    let command = if cfg!(windows) {
        "echo [%VITNA_SANDBOX%]"
    } else {
        "echo \"[$VITNA_SANDBOX]\""
    };

    let mut request =
        CommandRequest::new(command, &fx.workspace, &fx.workspace, 30_000);
    request.allow_unsandboxed = true;

    let out = runner(&fx)
        .run_command(request)
        .await
        .expect("approved unsandboxed command runs");

    assert_eq!(out.sandbox_backend, BACKEND_NONE);
    assert_eq!(out.sandbox_enforcement, BACKEND_NONE);
    assert!(
        !out.stdout.contains("[1]"),
        "the child was told it was sandboxed when it was not: {:?}",
        out.stdout
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// Where a backend exists, a command must not be able to write outside the
/// workspace. This is the assertion the whole crate is for.
#[tokio::test]
async fn a_sandboxed_command_cannot_write_outside_the_workspace() {
    let fx = fixture("confine");
    let Some(backend) = backend_or_skip(&fx.workspace, "write outside the workspace") else {
        let _ = fs::remove_dir_all(&fx.root);
        return;
    };

    let outside = fx.root.join("ESCAPED");
    let out = runner(&fx)
        .run_command(CommandRequest::new(
            write_outside(&outside),
            &fx.workspace,
            &fx.workspace,
            30_000,
        ))
        .await
        .expect("the command itself should run, even if its write fails");

    assert_eq!(out.sandbox_backend, backend);
    assert!(
        !outside.exists(),
        "{} let a command write outside the workspace to {}",
        backend,
        outside.display()
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// The workspace stays writable, or the sandbox is useless rather than safe.
#[tokio::test]
async fn a_sandboxed_command_can_still_work_in_the_workspace() {
    let fx = fixture("inside");
    let Some(_backend) = backend_or_skip(&fx.workspace, "write inside the workspace") else {
        let _ = fs::remove_dir_all(&fx.root);
        return;
    };

    let inside = fx.workspace.join("made.txt");
    let out = runner(&fx)
        .run_command(CommandRequest::new(
            write_outside(&inside),
            &fx.workspace,
            &fx.workspace,
            30_000,
        ))
        .await
        .expect("command runs");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        inside.exists(),
        "a sandbox that cannot write its own workspace is broken, not strict"
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// The read-only overlay, tested by trying to defeat it.
#[tokio::test]
async fn a_sandboxed_command_cannot_write_git_config() {
    let fx = fixture("gitconfig");
    let Some(backend) = backend_or_skip(&fx.workspace, "write .git/config") else {
        let _ = fs::remove_dir_all(&fx.root);
        return;
    };

    let config = fx.workspace.join(".git").join("config");
    let command = if cfg!(windows) {
        format!(
            "echo [core]>> \"{}\"",
            config.to_string_lossy().replace('\\', "/")
        )
    } else {
        format!(
            "echo '[core]' >> \"{}\"",
            config.to_string_lossy()
        )
    };

    let _ = runner(&fx)
        .run_command(CommandRequest::new(
            command,
            &fx.workspace,
            &fx.workspace,
            30_000,
        ))
        .await;

    let contents = fs::read_to_string(&config).expect("config still readable");
    assert_eq!(
        contents, "[core]\n",
        "{} let a command edit .git/config, which is code execution on the host \
         the next time anyone runs git here",
        backend
    );

    let _ = fs::remove_dir_all(&fx.root);
}

/// Whatever ran, the journal has to say what confined it.
#[tokio::test]
async fn the_journal_records_the_isolation_that_applied() {
    let fx = fixture("journal_iso");

    let mut request = CommandRequest::new("echo hello", &fx.workspace, &fx.workspace, 30_000);
    request.allow_unsandboxed = true;

    let r = runner(&fx);
    let out = r.run_command(request).await.expect("command runs");

    let entry = last_entry(&fx);

    assert_eq!(entry.sandbox_backend.as_deref(), Some(out.sandbox_backend.as_str()));
    assert_eq!(
        entry.sandbox_enforcement.as_deref(),
        Some(out.sandbox_enforcement.as_str())
    );

    let _ = fs::remove_dir_all(&fx.root);
}
