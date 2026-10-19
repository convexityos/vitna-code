//! A turn, as this terminal draws one: the person's words, the model's, and
//! each tool call with what it printed and what it asked.
//!
//! Nothing fills this yet, and the terminal says so rather than pretending.
//! vitna-coded answers the handshake and streams a session's events, and it
//! serves no `SubmitTurn`, so there is no way to start a turn over the wire.
//! The client this replaces invented one: a canned reply, a diff of a file
//! nobody wrote, and a receipt path for a receipt nobody signed. The shapes
//! below follow the declared events (`MessageDelta`, `ToolProposed`,
//! `ApprovalRequested`, `ToolStarted`, `ToolOutput`, `ToolFinished`,
//! `DiffChanged`) so the wiring can fill them without a redesign. Until it
//! does, only the tests draw them, from fixtures that never ship.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub enum Entry {
    /// What the person sent.
    You(String),
    /// What the model said, as prose.
    Model(String),
    Tool(Box<Tool>),
    /// Something this terminal says itself, never the model or the daemon.
    Note {
        tone: Color,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Proposed,
    Waiting,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub target: Option<String>,
    pub effect: String,
    pub phase: Phase,
    /// "exit 0 · 12 ms", once the tool has finished.
    pub status: Option<String>,
    pub output: Vec<String>,
    pub diff: Option<FileDiff>,
    pub approval: Option<Approval>,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone)]
pub enum DiffLine {
    Hunk(String),
    Add(String),
    Del(String),
    Same(String),
}

/// How long an approval ignores keys after it appears. A key typed for the
/// composer a moment before the box drew must not answer it: Claude Code
/// shipped exactly that as a bug, the first keypress captured as approval
/// before the dialog drew (CLAUDE-CODE-RELEASES-LESSONS 4).
pub const ARMING: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    ApproveOnce,
    ApproveSession,
    ApproveProject,
    Reject,
    RejectWithReason,
}

pub struct Choice {
    pub key: char,
    pub label: &'static str,
    pub hint: Option<char>,
    /// What the choice means beyond its label, for the ones that grant more
    /// than the action on the screen.
    pub meaning: Option<&'static str>,
    pub answer: Answer,
}

/// The web client's five choices, in its order and its words.
pub const CHOICES: [Choice; 5] = [
    Choice {
        key: '1',
        label: "Approve once",
        hint: Some('y'),
        meaning: None,
        answer: Answer::ApproveOnce,
    },
    Choice {
        key: '2',
        label: "Approve for this session",
        hint: None,
        meaning: Some("This action runs again for the rest of the session without asking."),
        answer: Answer::ApproveSession,
    },
    Choice {
        key: '3',
        label: "Approve for this project",
        hint: None,
        meaning: Some("This action runs in every later session in this project without asking."),
        answer: Answer::ApproveProject,
    },
    Choice {
        key: '4',
        label: "Reject",
        hint: Some('n'),
        meaning: None,
        answer: Answer::Reject,
    },
    Choice {
        key: '5',
        label: "Reject with a reason",
        hint: None,
        meaning: None,
        answer: Answer::RejectWithReason,
    },
];

/// An exact action waiting for a person. The digest is printed in full,
/// because the digest is what gets approved.
#[derive(Debug, Clone)]
pub struct Approval {
    pub description: String,
    pub digest: String,
    pub runs_as: String,
    pub cwd: String,
    pub mounts: Vec<String>,
    pub env: Vec<String>,
    pub cursor: usize,
    pub opened: Instant,
    pub answer: Option<Answer>,
}

impl Approval {
    pub fn armed(&self, now: Instant) -> bool {
        now.duration_since(self.opened) >= ARMING
    }

    /// One key, while the box has focus. Returns the answer it chose, if any.
    ///
    /// Before the box is armed every key is dropped, a held key never repeats
    /// into a choice, and once an answer is given no later key changes it.
    pub fn key(&mut self, key: KeyEvent, now: Instant) -> Option<Answer> {
        if self.answer.is_some() || !self.armed(now) || key.kind != KeyEventKind::Press {
            return None;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let chosen = match key.code {
            KeyCode::Char(c) => {
                if let Some(i) = CHOICES.iter().position(|o| o.key == c) {
                    self.cursor = i;
                    Some(CHOICES[i].answer)
                } else {
                    match c {
                        'y' | 'Y' => Some(Answer::ApproveOnce),
                        'n' | 'N' => Some(Answer::Reject),
                        _ => None,
                    }
                }
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1) % CHOICES.len();
                None
            }
            KeyCode::Up => {
                self.cursor = (self.cursor + CHOICES.len() - 1) % CHOICES.len();
                None
            }
            KeyCode::Enter => Some(CHOICES[self.cursor].answer),
            _ => None,
        };
        if chosen.is_some() {
            self.answer = chosen;
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(opened: Instant) -> Approval {
        Approval {
            description: "Write 12 lines to src/lib.rs".into(),
            digest: "ab".repeat(32),
            runs_as: String::new(),
            cwd: String::new(),
            mounts: Vec::new(),
            env: Vec::new(),
            cursor: 0,
            opened,
            answer: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn a_key_typed_before_the_box_is_armed_answers_nothing() {
        let opened = Instant::now();
        let mut a = approval(opened);
        assert_eq!(
            a.key(
                press(KeyCode::Char('y')),
                opened + Duration::from_millis(50)
            ),
            None
        );
        assert_eq!(
            a.key(press(KeyCode::Enter), opened + Duration::from_millis(200)),
            None
        );
        assert_eq!(a.answer, None, "a typed-ahead key answered the approval");
        assert_eq!(
            a.key(press(KeyCode::Char('y')), opened + ARMING),
            Some(Answer::ApproveOnce)
        );
    }

    #[test]
    fn a_held_key_does_not_repeat_into_a_choice() {
        let opened = Instant::now();
        let mut a = approval(opened);
        let mut held = press(KeyCode::Char('y'));
        held.kind = KeyEventKind::Repeat;
        assert_eq!(a.key(held, opened + ARMING * 2), None);
    }

    #[test]
    fn an_answer_once_given_is_final() {
        let opened = Instant::now();
        let later = opened + ARMING * 2;
        let mut a = approval(opened);
        assert_eq!(
            a.key(press(KeyCode::Char('n')), later),
            Some(Answer::Reject)
        );
        assert_eq!(a.key(press(KeyCode::Char('y')), later), None);
        assert_eq!(a.answer, Some(Answer::Reject));
    }

    #[test]
    fn the_arrows_move_and_enter_chooses_where_the_cursor_is() {
        let opened = Instant::now();
        let later = opened + ARMING * 2;
        let mut a = approval(opened);
        a.key(press(KeyCode::Up), later);
        assert_eq!(a.cursor, CHOICES.len() - 1);
        assert_eq!(
            a.key(press(KeyCode::Enter), later),
            Some(Answer::RejectWithReason)
        );
    }
}
