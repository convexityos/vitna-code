//! States to draw in tests and previews. Test-only: nothing here is compiled
//! into the binary, and nothing in the binary can produce a turn from it.
//!
//! The runs are shaped like the receipts real runs wrote on this machine (the
//! desktop window's captures), and the turn is the shape the declared events
//! will fill. Neither is anything that happened.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crate::app::{App, Screen};
use crate::ledger::{Change, ChangeKind, Entry, Evidence, Ledger, Run, Unreadable};
use crate::link::Link;
use crate::place::Branch;
use crate::turn::{Approval, DiffLine, Entry as TurnEntry, FileDiff, Phase, Tool};

/// The instant the snapshots are drawn at (`view::snapshot::draw`).
pub fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_200_000)
}

fn ago(secs: u64) -> Option<SystemTime> {
    Some(now() - Duration::from_secs(secs))
}

pub fn app() -> App {
    let mut a = App::new(PathBuf::from("vitna-code"));
    a.branch = Branch::Named("claude/vitna-terminal-redesign".into());
    a.link = Link::Absent {
        why: "nothing is listening".into(),
    };
    a.ledger = Some(Ledger::default());
    a
}

fn run(
    id: &str,
    state: &str,
    sku: &str,
    written: Option<SystemTime>,
    changes: &[(&str, ChangeKind)],
    evidence: &[(&str, &str)],
    failed_checks: &[&str],
) -> Entry {
    Entry::Run(Box::new(Run {
        file: format!(".vitna/receipts/{id}.json"),
        written,
        run_id: id.into(),
        base_commit: "38d891d1cd3807f04a4ddd9499986afb9e4259ce".into(),
        provider: "anthropic".into(),
        model_sku: sku.into(),
        isolation: "guarded".into(),
        completion: state.into(),
        evidence: evidence
            .iter()
            .map(|(g, d)| Evidence {
                grade: g.to_string(),
                description: d.to_string(),
            })
            .collect(),
        changes: changes
            .iter()
            .map(|(p, k)| Change {
                path: p.to_string(),
                kind: k.clone(),
            })
            .collect(),
        children: 0,
        signed: true,
        failed_checks: failed_checks.iter().map(|s| s.to_string()).collect(),
    }))
}

pub fn ledger() -> Ledger {
    Ledger {
        entries: vec![
            run(
                "run-1790196400112",
                "completed_with_evidence",
                "claude-fable-5-1",
                ago(3_600),
                &[
                    ("apps/vitna-cli/src/main.rs", ChangeKind::Modified),
                    ("apps/vitna-cli/tests/sessions_json.rs", ChangeKind::New),
                ],
                &[
                    (
                        "sandbox_captured",
                        "cargo test -p vitna-cli passed: 14 tests, 0 failed.",
                    ),
                    (
                        "model_reported",
                        "vitna sessions --json prints one object per session.",
                    ),
                ],
                &[],
            ),
            run(
                "run-1790109990031",
                "completed_with_unknowns",
                "claude-fable-5-1",
                ago(26 * 3_600),
                &[("crates/store/src/replay.rs", ChangeKind::Modified)],
                &[(
                    "broker_observed",
                    "The replay test reads back every event it wrote.",
                )],
                &[],
            ),
            run(
                "run-1790023590876",
                "failed",
                "claude-haiku-4-5",
                ago(3 * 86_400),
                &[],
                &[],
                &[],
            ),
            run(
                "run-1789754052047",
                "stopped_at_round_limit",
                "stub-messages-api",
                ago(5 * 86_400),
                &[],
                &[],
                &["Unknown completion state: stopped_at_round_limit"],
            ),
            Entry::Unreadable(Unreadable {
                file: "notes.json".into(),
                written: ago(6 * 86_400),
                reason: "is not a vitna-run-receipt-v1: missing field `run_id` at line 3 column 1"
                    .into(),
            }),
        ],
        trouble: None,
    }
}

pub fn with_runs() -> App {
    let mut a = app();
    a.ledger = Some(ledger());
    a.selected = 0;
    a
}

pub fn receipt() -> App {
    let mut a = with_runs();
    a.screen = Screen::Run(0);
    a
}

fn tool(name: &str, target: &str, effect: &str, phase: Phase, status: Option<&str>) -> Tool {
    Tool {
        name: name.into(),
        target: Some(target.into()),
        effect: effect.into(),
        phase,
        status: status.map(str::to_string),
        output: Vec::new(),
        diff: None,
        approval: None,
    }
}

/// A turn stopped at a write, waiting on the person, the approval armed.
pub fn turn_waiting() -> App {
    let mut a = app();
    a.link = Link::Open {
        version: "1.0".into(),
    };
    let mut write = tool(
        "write_file",
        "apps/vitna-cli/src/main.rs",
        "write",
        Phase::Waiting,
        None,
    );
    write.diff = Some(FileDiff {
        path: "apps/vitna-cli/src/main.rs".into(),
        status: "modified".into(),
        additions: 9,
        deletions: 1,
        lines: vec![
            DiffLine::Hunk("@@ -48,7 +48,15 @@ enum Commands {".into()),
            DiffLine::Same("    /// List active and previous sessions".into()),
            DiffLine::Del("    Sessions,".into()),
            DiffLine::Add("    Sessions {".into()),
            DiffLine::Add("        /// One JSON object per session, for scripts.".into()),
            DiffLine::Add("        #[arg(long)]".into()),
            DiffLine::Add("        json: bool,".into()),
            DiffLine::Add("    },".into()),
            DiffLine::Same("    /// Receipt operations".into()),
        ],
    });
    write.approval = Some(Approval {
        description:
            "Write apps/vitna-cli/src/main.rs: 9 lines added, 1 removed, inside the workspace."
                .into(),
        digest: "sha256:9f2c81ab03de5b7741a0c6e2d95f18b4c07e3a6d2f91b8c4e5a07d3361fe2b90".into(),
        runs_as: "vitna-tools write_file 1.0".into(),
        cwd: "C:\\src\\vitna-code".into(),
        mounts: Vec::new(),
        env: Vec::new(),
        cursor: 0,
        opened: Instant::now() - Duration::from_secs(5),
        answer: None,
    });
    a.turn = vec![
        TurnEntry::You("Add a --json flag to vitna sessions so scripts can read the list.".into()),
        TurnEntry::Model(
            "I'll read how the CLI lists sessions, then add the flag and a test that parses its output.".into(),
        ),
        TurnEntry::Tool(Box::new(tool("read_file", "apps/vitna-cli/src/main.rs", "read", Phase::Done, Some("exit 0 \u{b7} 4 ms")))),
        TurnEntry::Tool(Box::new(tool("search_code", "\"fn run_list_sessions\" in apps", "read", Phase::Done, Some("exit 0 \u{b7} 11 ms")))),
        TurnEntry::Tool(Box::new(write)),
    ];
    a
}

/// A turn running its check, with output arriving under it.
pub fn turn_running() -> App {
    let mut a = app();
    a.link = Link::Open {
        version: "1.0".into(),
    };
    let mut test = tool(
        "run_command",
        "cargo test -p vitna-cli",
        "execute",
        Phase::Running,
        None,
    );
    test.output = vec![
        "   Compiling vitna-cli v0.1.0".into(),
        "    Finished `test` profile [unoptimized + debuginfo] target(s) in 41.27s".into(),
        "     Running tests/sessions_json.rs".into(),
        "running 3 tests".into(),
        "test prints_one_object_per_session ... ok".into(),
    ];
    let mut failed = tool(
        "run_command",
        "cargo clippy -p vitna-cli -- -D warnings",
        "execute",
        Phase::Failed,
        Some("exit 101 \u{b7} 8.2 s"),
    );
    failed.output = vec![
        "error: unused import: `std::fs`".into(),
        "  --> apps/vitna-cli/src/main.rs:6:5".into(),
    ];
    a.turn = vec![
        TurnEntry::You("Add a --json flag to vitna sessions so scripts can read the list.".into()),
        TurnEntry::Model("The flag is in. Checking it now.".into()),
        TurnEntry::Tool(Box::new(tool(
            "write_file",
            "apps/vitna-cli/src/main.rs",
            "write",
            Phase::Done,
            Some("exit 0 \u{b7} 2 ms"),
        ))),
        TurnEntry::Tool(Box::new(failed)),
        TurnEntry::Model(
            "Clippy flags an import the change left unused. Removing it, then running the tests."
                .into(),
        ),
        TurnEntry::Tool(Box::new(test)),
    ];
    a
}
