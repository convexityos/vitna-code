//! The words this terminal prints for protocol and receipt values.
//!
//! They are `apps/vitna-desktop/src/ui/words.ts`'s, which the desktop window
//! also reads its phrases from, so no two surfaces name one state two ways.
//! A value this table does not know is printed as itself and marked, so a new
//! state from a newer daemon reads as unknown rather than as a blank, and it
//! is painted rust, because somebody should look at it.

use ratatui::style::Color;

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub tone: Color,
}

fn word(w: &str, tone: Color) -> Word {
    Word {
        word: w.to_string(),
        tone,
    }
}

/// A run's live state, while it runs.
pub fn run_state(state: &str) -> Word {
    match state {
        "queued" => word("Queued", theme::FAINT),
        "running_model" => word("Thinking", theme::FAINT),
        "waiting_for_approval" => word("Waiting on you", theme::RUST),
        "running_tool" => word("Running a tool", theme::FAINT),
        "waiting_for_child" => word("Waiting on a sub-agent", theme::FAINT),
        "waiting_for_retry" => word("Waiting to retry", theme::RUST),
        "paused" => word("Paused", theme::FAINT),
        "needs_reconciliation" => word("Needs reconciliation", theme::RUST),
        "completed" => word("Completed", theme::OK),
        "failed" => word("Failed", theme::RUST),
        "cancelled" => word("Cancelled", theme::FAINT),
        "orphaned" => word("Orphaned", theme::RUST),
        other => word(&format!("Unknown state \"{other}\""), theme::RUST),
    }
}

/// What a finished run's receipt says it came to: the full phrase for a
/// heading, and the short one the run list uses.
pub fn completion(state: &str) -> (Word, &'static str) {
    match state {
        "completed_with_evidence" => (word("Completed, with evidence", theme::OK), "Completed"),
        "completed_with_unknowns" => (
            word("Completed, with unknowns stated", theme::RUST),
            "With unknowns",
        ),
        "blocked" => (word("Blocked", theme::RUST), "Blocked"),
        "failed" => (word("Failed", theme::RUST), "Failed"),
        "cancelled" => (word("Cancelled", theme::FAINT), "Cancelled"),
        "needs_reconciliation" => (word("Needs reconciliation", theme::RUST), "Reconcile"),
        // The desktop window's phrase for a state its branch added. Named here
        // so a receipt that carries it reads as what it is; the verifier on
        // this branch still refuses it, and that refusal shows as a failed
        // check beside it.
        "stopped_at_round_limit" => (
            word(
                "Stopped at the round limit, before the model concluded",
                theme::RUST,
            ),
            "Round limit",
        ),
        other => (
            word(&format!("Unknown completion \"{other}\""), theme::RUST),
            "Unknown",
        ),
    }
}

pub fn isolation(label: &str) -> String {
    match label {
        "read_only" => "Read only".to_string(),
        "guarded" => "Guarded".to_string(),
        "strong" => "Strong".to_string(),
        "full_access" => "Full access".to_string(),
        other => format!("Unknown label \"{other}\""),
    }
}

/// An evidence grade, and how much weight it carries: captured or reproduced
/// is the strongest on offer, observed by the broker is next, and a claim the
/// model made is shown and marked as the weakest.
pub fn grade(grade: &str) -> Word {
    match grade {
        "sandbox_captured" => word("Captured in the sandbox", theme::OK),
        "independently_reproduced" => word("Independently reproduced", theme::OK),
        "remote_or_hardware_attested" => word("Attested remotely or by hardware", theme::OK),
        "broker_observed" => word("Observed by the broker", theme::PERI_2),
        "model_reported" => word("Model reported", theme::RUST),
        other => word(&format!("Unknown grade \"{other}\""), theme::RUST),
    }
}

pub fn effect(effect: &str) -> String {
    match effect {
        "" => "effect not stated".to_string(),
        "read" => "reads".to_string(),
        "write" => "writes".to_string(),
        "execute" => "runs a program".to_string(),
        "network" => "uses the network".to_string(),
        "secret" => "uses a secret".to_string(),
        other => format!("effect \"{other}\""),
    }
}

pub fn tool_title(name: &str) -> String {
    match name {
        "read_file" => "Read",
        "write_file" => "Write",
        "apply_patch" => "Patch",
        "list_dir" => "List",
        "search_code" => "Search",
        "run_command" => "Run",
        "git_status" => "Git status",
        "browser_verify" => "Open in a browser",
        other => return other.to_string(),
    }
    .to_string()
}

/// A one-line reading of a tool's arguments, for the tools whose arguments
/// have an obvious one.
pub fn argument_summary(name: &str, arguments_json: &str) -> Option<String> {
    let args: serde_json::Value = serde_json::from_str(arguments_json).ok()?;
    let a = args.as_object()?;
    let text = |k: &str| a.get(k).and_then(|v| v.as_str()).map(str::to_string);
    match name {
        "read_file" | "write_file" | "apply_patch" | "list_dir" => text("path"),
        "search_code" => {
            let query = text("query")?;
            Some(match text("path") {
                Some(path) => format!("\"{query}\" in {path}"),
                None => format!("\"{query}\""),
            })
        }
        "run_command" => {
            if let Some(argv) = a.get("argv").and_then(|v| v.as_array()) {
                let parts: Option<Vec<&str>> = argv.iter().map(|p| p.as_str()).collect();
                if let Some(parts) = parts {
                    return Some(parts.join(" "));
                }
            }
            text("command").or_else(|| text("cmd"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_value_is_printed_and_marked_never_blank() {
        let w = run_state("teleporting");
        assert_eq!(w.word, "Unknown state \"teleporting\"");
        assert_eq!(w.tone, theme::RUST);
        assert_eq!(completion("").0.word, "Unknown completion \"\"");
        assert_eq!(isolation("sideways"), "Unknown label \"sideways\"");
    }

    #[test]
    fn the_model_saying_so_is_the_weakest_evidence() {
        assert_eq!(grade("model_reported").tone, theme::RUST);
        assert_eq!(grade("sandbox_captured").tone, theme::OK);
    }

    #[test]
    fn arguments_read_as_one_line_where_they_have_one() {
        assert_eq!(
            argument_summary(
                "run_command",
                r#"{"argv":["cargo","test","-p","vitna-cli"]}"#
            )
            .as_deref(),
            Some("cargo test -p vitna-cli")
        );
        assert_eq!(
            argument_summary("search_code", r#"{"query":"fn main","path":"src"}"#).as_deref(),
            Some("\"fn main\" in src")
        );
        assert_eq!(argument_summary("write_file", "not json"), None);
    }
}
