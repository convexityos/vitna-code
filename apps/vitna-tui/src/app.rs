//! What the terminal knows, and what each key does to it.

use std::cell::Cell;
use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::ledger::Ledger;
use crate::link::Link;
use crate::place::{self, Branch};
use crate::turn;

/// What the terminal is looking at, above the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The runs this folder has receipts for, or the start state when none.
    Home,
    /// One run's receipt, by its index in the ledger.
    Run(usize),
}

/// Something found out off the drawing thread.
#[derive(Debug)]
pub enum Update {
    Link(Link),
    Branch(Branch),
    Ledger(Ledger),
}

/// Rows or runs one Page Up or Page Down moves.
const PAGE: isize = 10;

/// Work the loop should start, because a key asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Nothing,
    /// Look for the daemon again and re-read the receipts.
    Refresh,
}

pub struct App {
    pub root: PathBuf,
    pub folder: String,
    pub branch: Branch,
    pub link: Link,
    /// None until the receipts directory has been read once.
    pub ledger: Option<Ledger>,
    /// The turn on screen. Empty: nothing on the wire can start one yet.
    pub turn: Vec<turn::Entry>,
    pub draft: Vec<char>,
    /// The cursor, as a char index into `draft`.
    pub cursor: usize,
    pub screen: Screen,
    /// The chosen row in the run list.
    pub selected: usize,
    /// Rows scrolled, in whatever the stage is showing.
    pub scroll: usize,
    /// How far the stage could scroll, as the last frame measured it, so a
    /// held arrow stops at the end instead of counting on past it.
    pub scroll_room: Cell<usize>,
    /// Why the last Enter did not send, until the next keystroke.
    pub notice: Option<String>,
    pub help: bool,
    pub quit: bool,
}

impl App {
    pub fn new(root: PathBuf) -> App {
        App {
            folder: place::folder_name(&root),
            root,
            branch: Branch::Reading,
            link: Link::Probing,
            ledger: None,
            turn: Vec::new(),
            draft: Vec::new(),
            cursor: 0,
            screen: Screen::Home,
            selected: 0,
            scroll: 0,
            scroll_room: Cell::new(0),
            notice: None,
            help: false,
            quit: false,
        }
    }

    pub fn update(&mut self, update: Update) {
        match update {
            Update::Link(link) => self.link = link,
            Update::Branch(branch) => self.branch = branch,
            Update::Ledger(ledger) => {
                let n = ledger.entries.len();
                self.selected = self.selected.min(n.saturating_sub(1));
                if let Screen::Run(i) = self.screen {
                    if i >= n {
                        self.screen = Screen::Home;
                    }
                }
                self.ledger = Some(ledger);
            }
        }
    }

    fn runs(&self) -> usize {
        self.ledger.as_ref().map_or(0, |l| l.entries.len())
    }

    pub fn key(&mut self, key: KeyEvent) -> Ask {
        // Windows reports a release for every press. Only presses and the
        // repeats of a held key act, or every character would type twice.
        if key.kind == KeyEventKind::Release {
            return Ask::Nothing;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // AltGr arrives on Windows as Ctrl and Alt together, and it is how
        // much of the world types @, { and the euro sign. So a shortcut is
        // Ctrl without Alt, and a character with both is a character.
        let command = ctrl && !alt;

        if self.help {
            self.help = false;
            if key.code == KeyCode::Esc || key.code == KeyCode::Char('?') {
                return Ask::Nothing;
            }
        }

        match key.code {
            KeyCode::Char('c') if command => {
                if self.draft.is_empty() {
                    self.quit = true;
                } else {
                    self.clear_draft();
                }
            }
            KeyCode::Char('d') if command => {
                if self.draft.is_empty() {
                    self.quit = true;
                }
            }
            KeyCode::Char('r') if command => {
                self.notice = None;
                self.link = Link::Probing;
                return Ask::Refresh;
            }
            KeyCode::Char('u') if command => {
                let start = self.line_start();
                self.draft.drain(start..self.cursor);
                self.cursor = start;
                self.notice = None;
            }
            // A new line. Shift+Enter where the terminal reports it, and
            // Alt+Enter and Ctrl+J everywhere, since many terminals send a
            // plain Enter for Shift+Enter.
            KeyCode::Enter if shift || alt => self.insert('\n'),
            KeyCode::Char('j') if command => self.insert('\n'),
            KeyCode::Enter => {
                if !self.draft.iter().all(|c| c.is_whitespace()) {
                    self.submit();
                } else if self.screen == Screen::Home && self.runs() > 0 {
                    self.screen = Screen::Run(self.selected);
                    self.scroll = 0;
                }
            }
            KeyCode::Esc => {
                if self.notice.is_some() {
                    self.notice = None;
                } else if let Screen::Run(i) = self.screen {
                    self.screen = Screen::Home;
                    self.selected = i;
                    self.scroll = 0;
                }
            }
            KeyCode::Char('?') if self.draft.is_empty() => self.help = true,
            KeyCode::Char(c) if !command => self.insert(c),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.draft.remove(self.cursor);
                }
                self.notice = None;
            }
            KeyCode::Delete => {
                if self.cursor < self.draft.len() {
                    self.draft.remove(self.cursor);
                }
                self.notice = None;
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.draft.len()),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => {
                self.cursor = self.draft[self.cursor..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map_or(self.draft.len(), |i| self.cursor + i);
            }
            KeyCode::Down if self.draft.is_empty() => self.step(1),
            KeyCode::Up if self.draft.is_empty() => self.step(-1),
            KeyCode::PageDown => self.step(PAGE),
            KeyCode::PageUp => self.step(-PAGE),
            _ => {}
        }
        Ask::Nothing
    }

    /// Everything that was waiting when the loop looked, in order.
    ///
    /// On Windows a paste arrives as keys, its line breaks as Enter presses,
    /// so an Enter with more keys pressed behind it in the same batch is a
    /// line break in pasted text and not a person asking to send. A person's
    /// own Enter arrives alone, or with nothing behind it but its release.
    pub fn events(&mut self, batch: &[Event]) -> Ask {
        let pressed = |e: &Event| matches!(e, Event::Key(k) if k.kind == KeyEventKind::Press);
        let mut ask = Ask::Nothing;
        for (i, event) in batch.iter().enumerate() {
            match event {
                Event::Key(key)
                    if key.code == KeyCode::Enter
                        && key.kind == KeyEventKind::Press
                        && key.modifiers.is_empty()
                        && batch[i + 1..].iter().any(pressed) =>
                {
                    self.insert('\n')
                }
                Event::Key(key) => {
                    if self.key(*key) == Ask::Refresh {
                        ask = Ask::Refresh;
                    }
                }
                Event::Paste(text) => self.paste(text),
                _ => {}
            }
            if self.quit {
                break;
            }
        }
        ask
    }

    /// Moves through whatever the stage shows: the chosen run in the list, or
    /// the scroll of a receipt or a turn, stopping at either end.
    fn step(&mut self, by: isize) {
        let room = self.scroll_room.get();
        if !self.turn.is_empty() {
            // A turn scrolls back from its tail, so moving up goes further
            // from it.
            self.scroll = self.scroll.saturating_add_signed(-by).min(room);
            return;
        }
        match self.screen {
            Screen::Home => {
                let n = self.runs();
                if n > 0 {
                    self.selected = self.selected.saturating_add_signed(by).min(n - 1);
                }
            }
            Screen::Run(_) => self.scroll = self.scroll.saturating_add_signed(by).min(room),
        }
    }

    /// Pasted text, where the terminal delivers a paste whole. Its line breaks
    /// stay line breaks rather than arriving as Enters that try to send.
    pub fn paste(&mut self, text: &str) {
        for c in text.replace("\r\n", "\n").chars() {
            if c == '\n' || !c.is_control() {
                self.insert(c);
            }
        }
    }

    /// Where the line the cursor is on begins, as a char index.
    fn line_start(&self) -> usize {
        self.draft[..self.cursor]
            .iter()
            .rposition(|c| *c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn insert(&mut self, c: char) {
        self.draft.insert(self.cursor, c);
        self.cursor += 1;
        self.notice = None;
    }

    fn clear_draft(&mut self) {
        self.draft.clear();
        self.cursor = 0;
        self.notice = None;
    }

    /// Enter, with something typed. Nothing can take a turn yet, so the draft
    /// stays where it is and the terminal says why, in words that match what
    /// it actually found. A disabled send that will not say why is the thing
    /// this product exists not to ship.
    fn submit(&mut self) {
        self.notice = Some(
            match &self.link {
                Link::Open { .. } => {
                    "Not sent. vitna-coded answered, and it takes no turns over the wire yet."
                }
                Link::Absent { .. } => {
                    "Not sent. The daemon is not running, so there is nothing to send to."
                }
                Link::Trouble { .. } => "Not sent. The daemon did not complete the handshake.",
                Link::Probing => "Not sent. Still looking for the daemon.",
            }
            .to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(press(KeyCode::Char(c)));
        }
    }

    fn app() -> App {
        App::new(PathBuf::from("vitna-code"))
    }

    /// The old client answered every prompt with a reply, a diff and a
    /// receipt it made up. Enter now adds nothing to the screen that did not
    /// come from somewhere, and loses nothing the person typed.
    #[test]
    fn enter_with_nothing_to_send_to_invents_nothing_and_keeps_the_draft() {
        for link in [
            Link::Absent {
                why: "no pipe".into(),
            },
            Link::Open {
                version: "1.0".into(),
            },
            Link::Probing,
            Link::Trouble {
                what: "silent".into(),
            },
        ] {
            let mut a = app();
            a.link = link;
            typed(&mut a, "add a --json flag");
            a.key(press(KeyCode::Enter));
            assert!(a.turn.is_empty(), "Enter drew a turn nothing produced");
            assert_eq!(
                a.draft.iter().collect::<String>(),
                "add a --json flag",
                "the draft was lost"
            );
            assert!(a
                .notice
                .as_deref()
                .is_some_and(|n| n.starts_with("Not sent")));
        }
    }

    #[test]
    fn a_release_does_not_type_a_second_character() {
        let mut a = app();
        let mut release = press(KeyCode::Char('x'));
        release.kind = KeyEventKind::Release;
        a.key(press(KeyCode::Char('x')));
        a.key(release);
        assert_eq!(a.draft, vec!['x']);
    }

    #[test]
    fn ctrl_c_clears_a_draft_before_it_quits() {
        let mut a = app();
        typed(&mut a, "half a thought");
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.draft.is_empty());
        assert!(!a.quit);
        a.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.quit);
    }

    #[test]
    fn a_new_line_does_not_send() {
        let mut a = app();
        typed(&mut a, "one");
        a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut a, "two");
        assert_eq!(a.draft.iter().collect::<String>(), "one\ntwo");
        assert!(a.notice.is_none());
    }

    #[test]
    fn a_pasted_line_break_is_a_line_break_and_a_typed_enter_still_tries_to_send() {
        let key = |code, kind| {
            let mut k = KeyEvent::new(code, KeyModifiers::NONE);
            k.kind = kind;
            Event::Key(k)
        };
        let mut a = app();
        let burst = [
            key(KeyCode::Char('a'), KeyEventKind::Press),
            key(KeyCode::Enter, KeyEventKind::Press),
            key(KeyCode::Enter, KeyEventKind::Release),
            key(KeyCode::Char('b'), KeyEventKind::Press),
        ];
        a.events(&burst);
        assert_eq!(a.draft.iter().collect::<String>(), "a\nb");
        assert!(a.notice.is_none(), "a pasted line break tried to send");

        a.events(&[
            key(KeyCode::Enter, KeyEventKind::Press),
            key(KeyCode::Enter, KeyEventKind::Release),
        ]);
        assert!(
            a.notice.is_some(),
            "an Enter with only its release behind it did not try to send"
        );
        assert_eq!(a.draft.iter().collect::<String>(), "a\nb");
    }

    /// AltGr+Q is @ on a German keyboard, and Windows reports it as Ctrl+Alt+@.
    #[test]
    fn a_character_typed_with_altgr_is_typed() {
        let mut a = app();
        a.key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        a.key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(a.draft.iter().collect::<String>(), "@c");
        assert!(!a.quit, "AltGr with c is a character, not Ctrl+C");
    }

    #[test]
    fn ctrl_u_clears_only_the_line_the_cursor_is_on() {
        let mut a = app();
        typed(&mut a, "keep");
        a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut a, "drop");
        a.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(a.draft.iter().collect::<String>(), "keep\n");
    }

    #[test]
    fn a_question_mark_is_text_once_something_is_typed() {
        let mut a = app();
        a.key(press(KeyCode::Char('?')));
        assert!(a.help);
        a.key(press(KeyCode::Esc));
        typed(&mut a, "why?");
        assert!(!a.help);
        assert_eq!(a.draft.iter().collect::<String>(), "why?");
    }
}
