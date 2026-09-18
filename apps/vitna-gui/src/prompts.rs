//! The prompts behind the sessions and runs, as the daemon's event log holds
//! them.
//!
//! A receipt carries no prompt, on purpose, so a title here is never read off
//! a receipt and never remembered from what this window sent. It is what the
//! daemon's `TurnStarted` event recorded, fetched with `ListTurns`. Where the
//! log has nothing, the caller shows the id it does have instead of a title
//! it would have to invent.

use std::collections::HashMap;

use vitna_protocol::api::{TurnInfo, TurnListResponse};

#[derive(Default)]
pub struct Prompts {
    by_run: HashMap<String, TurnInfo>,
    /// The first turn of each session, which is what names it.
    first_by_session: HashMap<String, TurnInfo>,
    /// Runs whose TurnStarted the daemon could not read back, with why.
    unreadable: HashMap<String, String>,
}

impl Prompts {
    pub fn from_list(list: TurnListResponse) -> Self {
        let mut p = Prompts::default();
        // Oldest first on the wire, so the first insert per session is its
        // first turn and a later one must not replace it.
        for turn in list.turns {
            p.first_by_session
                .entry(turn.session_id.clone())
                .or_insert_with(|| turn.clone());
            p.by_run.insert(turn.run_id.clone(), turn);
        }
        for u in list.unreadable {
            p.unreadable.insert(u.run_id, u.reason);
        }
        p
    }

    pub fn for_run(&self, run_id: &str) -> Option<&TurnInfo> {
        self.by_run.get(run_id)
    }

    pub fn for_session(&self, session_id: &str) -> Option<&TurnInfo> {
        self.first_by_session.get(session_id)
    }

    /// How many turns the log holds for a session.
    pub fn turns_in(&self, session_id: &str) -> usize {
        self.by_run.values().filter(|t| t.session_id == session_id).count()
    }

    pub fn unreadable(&self, run_id: &str) -> Option<&str> {
        self.unreadable.get(run_id).map(String::as_str)
    }
}

/// A prompt as a one-line title: its first line with text on it, trimmed.
/// The caller truncates to width; this never shortens a word itself.
pub fn title(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitna_protocol::api::UnreadableTurn;

    fn turn(run: &str, session: &str, prompt: &str, at: u64) -> TurnInfo {
        TurnInfo {
            run_id: run.into(),
            session_id: session.into(),
            prompt: prompt.into(),
            started_at_ms: at,
        }
    }

    #[test]
    fn a_session_is_named_by_its_first_turn() {
        let p = Prompts::from_list(TurnListResponse {
            turns: vec![
                turn("run-1", "sess-a", "tidy the readme", 1),
                turn("run-2", "sess-a", "now the changelog", 2),
            ],
            unreadable: vec![UnreadableTurn {
                run_id: "run-3".into(),
                reason: "the payload is not JSON".into(),
            }],
        });
        assert_eq!(p.for_session("sess-a").map(|t| t.prompt.as_str()), Some("tidy the readme"));
        assert_eq!(p.for_run("run-2").map(|t| t.prompt.as_str()), Some("now the changelog"));
        assert!(p.for_session("sess-b").is_none());
        assert_eq!(p.unreadable("run-3"), Some("the payload is not JSON"));
    }

    #[test]
    fn a_title_is_the_first_line_with_text_on_it() {
        assert_eq!(title("\n  \n  fix the build  \nand then the tests"), "fix the build");
        assert_eq!(title("   "), "");
    }
}
