//! What the terminal knows, and what each key does to it.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;
use vitna_protocol::messages::{
    run_state, tool_status, ApprovalRequested, Diagnostic, DiffChanged, ErrorResponse,
    MessageDelta, RunStateChanged, ToolFinished, ToolOutput, ToolProposed, ToolStarted,
};
use vitna_protocol::{type_url, ProtocolEnvelope};

use crate::ledger::Ledger;
use crate::link::{Link, OutboundCmd};
use crate::place::{self, Branch};
use crate::text::clean;
use crate::{theme, turn};

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
    SessionReady(String),
    Wire(ProtocolEnvelope),
    Branch(Branch),
    Ledger(Ledger),
}

/// Rows or runs one Page Up or Page Down moves.
const PAGE: isize = 10;

/// Maximum lines retained per tool output block in the terminal view.
const MAX_TOOL_OUTPUT_LINES: usize = 200;

/// Work the loop should start, because a key or wire transition asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Nothing,
    /// Look for the daemon again and re-read the receipts.
    Refresh,
    /// Re-read the receipts directory after a live run finishes.
    RefreshLedger,
}

pub struct App {
    pub root: PathBuf,
    pub folder: String,
    pub branch: Branch,
    pub link: Link,
    /// The daemon session id once `SessionCreated` and `SubscribeEvents` complete.
    pub session_id: Option<String>,
    /// The most recently observed run id in this session.
    pub active_run_id: Option<String>,
    /// True while a turn is in flight on the daemon.
    pub run_active: bool,
    /// Channel to send outbound protocol commands to the daemon connection task.
    pub cmd_tx: Option<UnboundedSender<OutboundCmd>>,
    /// None until the receipts directory has been read once.
    pub ledger: Option<Ledger>,
    /// The turn on screen, populated strictly from live daemon events.
    pub turn: Vec<turn::Entry>,
    /// Tracks the active streaming model message_id so token deltas append inline.
    pub last_model_message_id: Option<String>,
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
    /// Why the last Enter did not send, or daemon refusal feedback, until the next keystroke.
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
            session_id: None,
            active_run_id: None,
            run_active: false,
            cmd_tx: None,
            ledger: None,
            turn: Vec::new(),
            last_model_message_id: None,
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
        let _ = self.apply_update(update);
    }

    pub fn apply_update(&mut self, update: Update) -> Ask {
        match update {
            Update::Link(link) => {
                if !link.is_open() {
                    self.session_id = None;
                    self.run_active = false;
                }
                self.link = link;
                Ask::Nothing
            }
            Update::SessionReady(id) => {
                self.session_id = Some(id);
                Ask::Nothing
            }
            Update::Wire(env) => {
                if self.fold_wire(env) {
                    Ask::RefreshLedger
                } else {
                    Ask::Nothing
                }
            }
            Update::Branch(branch) => {
                self.branch = branch;
                Ask::Nothing
            }
            Update::Ledger(ledger) => {
                let n = ledger.entries.len();
                self.selected = self.selected.min(n.saturating_sub(1));
                if let Screen::Run(i) = self.screen {
                    if i >= n {
                        self.screen = Screen::Home;
                    }
                }
                self.ledger = Some(ledger);
                Ask::Nothing
            }
        }
    }

    /// Folds one declared protocol frame from the daemon into the visible turn.
    /// Returns `true` when a run reaches a terminal state so the caller can
    /// re-read `.vitna/receipts/`.
    fn fold_wire(&mut self, env: ProtocolEnvelope) -> bool {
        if !env.run_id.is_empty() {
            self.active_run_id = Some(env.run_id.clone());
        }
        match env.type_url.as_str() {
            type_url::event::MESSAGE_DELTA => {
                if let Ok(delta) = env.payload_as::<MessageDelta>() {
                    let text = clean(&delta.delta_text);
                    if text.is_empty() {
                        return false;
                    }
                    if delta.role == "user" {
                        self.last_model_message_id = None;
                        self.turn.push(turn::Entry::You(text));
                    } else if delta.role == "model" {
                        let same_message = !delta.message_id.is_empty()
                            && self.last_model_message_id.as_deref()
                                == Some(delta.message_id.as_str());
                        if let Some(turn::Entry::Model(existing)) = self.turn.last_mut() {
                            if !same_message && !existing.is_empty() && !existing.ends_with('\n') {
                                existing.push('\n');
                            }
                            existing.push_str(&text);
                        } else {
                            self.turn.push(turn::Entry::Model(text));
                        }
                        self.last_model_message_id = if delta.message_id.is_empty() {
                            None
                        } else {
                            Some(delta.message_id)
                        };
                    } else {
                        self.last_model_message_id = None;
                        self.turn.push(turn::Entry::Note {
                            tone: theme::FAINT,
                            text,
                        });
                    }
                }
                false
            }
            type_url::event::TOOL_PROPOSED => {
                self.last_model_message_id = None;
                if let Ok(p) = env.payload_as::<ToolProposed>() {
                    let target = extract_target(&p.raw_arguments_json).map(|t| clean(&t));
                    self.turn.push(turn::Entry::Tool(Box::new(turn::Tool {
                        call_id: p.tool_call_id,
                        name: clean(&p.tool_name),
                        target,
                        effect: clean(&p.effect_class),
                        phase: turn::Phase::Proposed,
                        status: None,
                        output: Vec::new(),
                        diff: None,
                        approval: None,
                    })));
                }
                false
            }
            type_url::event::APPROVAL_REQUESTED => {
                if let Ok(req) = env.payload_as::<ApprovalRequested>() {
                    let approval = turn::Approval {
                        approval_id: req.approval_id,
                        description: clean(&req.description),
                        digest: clean(&req.action_digest),
                        runs_as: clean(&req.executable_identity),
                        cwd: clean(&req.canonical_cwd),
                        mounts: req.bound_mounts.iter().map(|s| clean(s)).collect(),
                        env: req.environment_names.iter().map(|s| clean(s)).collect(),
                        cursor: 0,
                        opened: Instant::now(),
                        answer: None,
                    };
                    if let Some(tool) = self.find_tool_mut(&req.tool_call_id) {
                        tool.phase = turn::Phase::Waiting;
                        tool.approval = Some(approval);
                    } else {
                        self.turn.push(turn::Entry::Tool(Box::new(turn::Tool {
                            call_id: req.tool_call_id,
                            name: "action".to_string(),
                            target: None,
                            effect: "write".to_string(),
                            phase: turn::Phase::Waiting,
                            status: None,
                            output: Vec::new(),
                            diff: None,
                            approval: Some(approval),
                        })));
                    }
                    self.scroll = 0;
                }
                false
            }
            type_url::event::TOOL_STARTED => {
                if let Ok(started) = env.payload_as::<ToolStarted>() {
                    if let Some(tool) = self.find_tool_mut(&started.tool_call_id) {
                        tool.phase = turn::Phase::Running;
                        tool.approval = None;
                    }
                }
                false
            }
            type_url::event::TOOL_OUTPUT => {
                if let Ok(out) = env.payload_as::<ToolOutput>() {
                    let text = String::from_utf8_lossy(&out.chunk);
                    if let Some(tool) = self.find_tool_mut(&out.tool_call_id) {
                        for line in text.lines() {
                            if tool.output.len() >= MAX_TOOL_OUTPUT_LINES {
                                break;
                            }
                            tool.output.push(clean(line));
                        }
                    }
                }
                false
            }
            type_url::event::TOOL_FINISHED => {
                if let Ok(fin) = env.payload_as::<ToolFinished>() {
                    if let Some(tool) = self.find_tool_mut(&fin.tool_call_id) {
                        tool.approval = None;
                        tool.phase = match fin.status.as_str() {
                            tool_status::COMPLETED => turn::Phase::Done,
                            tool_status::CANCELLED => turn::Phase::Skipped,
                            _ => turn::Phase::Failed,
                        };
                        tool.status = Some(format!(
                            "exit {} \u{b7} {} ms",
                            fin.exit_code, fin.duration_ms
                        ));
                    }
                }
                false
            }
            type_url::event::DIFF_CHANGED => {
                if let Ok(diff) = env.payload_as::<DiffChanged>() {
                    let parsed = parse_file_diff(&diff);
                    if let Some(tool) = self.last_tool_mut() {
                        tool.diff = Some(parsed);
                    }
                }
                false
            }
            type_url::event::RUN_STATE_CHANGED => {
                if let Ok(state) = env.payload_as::<RunStateChanged>() {
                    match state.new_state.as_str() {
                        run_state::COMPLETED | run_state::FAILED | run_state::CANCELLED => {
                            self.run_active = false;
                            // Clear any dangling approval prompt if the run ended.
                            for entry in &mut self.turn {
                                if let turn::Entry::Tool(t) = entry {
                                    t.approval = None;
                                }
                            }
                            if state.new_state == run_state::FAILED
                                || state.new_state == run_state::CANCELLED
                            {
                                let reason = clean(&state.reason);
                                let text = if reason.is_empty() {
                                    format!("Run {}.", state.new_state)
                                } else {
                                    format!("Run {}: {}", state.new_state, reason)
                                };
                                self.turn.push(turn::Entry::Note {
                                    tone: theme::RUST,
                                    text,
                                });
                            }
                            return true;
                        }
                        run_state::PAUSED => {
                            let reason = clean(&state.reason);
                            self.notice = Some(if reason.is_empty() {
                                "Run paused.".to_string()
                            } else {
                                format!("Run paused: {reason}")
                            });
                        }
                        _ => {
                            self.run_active = true;
                        }
                    }
                }
                false
            }
            type_url::event::DIAGNOSTIC => {
                if let Ok(diag) = env.payload_as::<Diagnostic>() {
                    let tone = if diag.level == "error" {
                        theme::RUST
                    } else {
                        theme::FAINT
                    };
                    self.turn.push(turn::Entry::Note {
                        tone,
                        text: clean(&diag.message),
                    });
                }
                false
            }
            type_url::envelope::ERROR_RESPONSE => {
                if let Ok(err) = env.payload_as::<ErrorResponse>() {
                    self.notice = Some(clean(&err.message));
                    if !err.action_id.is_empty() {
                        // A refusal on an approval (e.g., wider scope rejected)
                        // leaves the request waiting on the daemon, so re-open
                        // the approval choices for another answer.
                        for entry in self.turn.iter_mut().rev() {
                            if let turn::Entry::Tool(t) = entry {
                                if let Some(a) = t.approval.as_mut() {
                                    if a.approval_id == err.action_id {
                                        a.answer = None;
                                        break;
                                    }
                                }
                            }
                        }
                    } else if self.active_run_id.is_none() {
                        self.run_active = false;
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn find_tool_mut(&mut self, call_id: &str) -> Option<&mut turn::Tool> {
        let idx = if !call_id.is_empty() {
            self.turn
                .iter()
                .rposition(|e| matches!(e, turn::Entry::Tool(t) if t.call_id == call_id))
        } else {
            None
        }
        .or_else(|| {
            self.turn
                .iter()
                .rposition(|e| matches!(e, turn::Entry::Tool(_)))
        })?;
        match &mut self.turn[idx] {
            turn::Entry::Tool(tool) => Some(tool),
            _ => None,
        }
    }

    fn last_tool_mut(&mut self) -> Option<&mut turn::Tool> {
        for entry in self.turn.iter_mut().rev() {
            if let turn::Entry::Tool(tool) = entry {
                return Some(tool);
            }
        }
        None
    }

    fn pending_approval_mut(&mut self) -> Option<&mut turn::Approval> {
        for entry in self.turn.iter_mut().rev() {
            if let turn::Entry::Tool(tool) = entry {
                if let Some(a) = tool.approval.as_mut() {
                    if a.answer.is_none() {
                        return Some(a);
                    }
                }
            }
        }
        None
    }

    fn runs(&self) -> usize {
        self.ledger.as_ref().map_or(0, |l| l.entries.len())
    }

    pub fn key(&mut self, key: KeyEvent) -> Ask {
        self.key_at(key, Instant::now())
    }

    /// Processes a single key event at `now` (injectable for deterministic tests
    /// of approval arming).
    pub fn key_at(&mut self, key: KeyEvent, now: Instant) -> Ask {
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

        // Ctrl+C always works even while an approval box is waiting.
        if matches!(key.code, KeyCode::Char('c')) && command {
            if !self.draft.is_empty() {
                self.clear_draft();
            } else if self.run_active {
                if let (Some(run_id), Some(tx)) = (&self.active_run_id, &self.cmd_tx) {
                    let _ = tx.send(OutboundCmd::CancelRun {
                        run_id: run_id.clone(),
                        reason: "cancelled from vitna-tui".to_string(),
                    });
                    self.notice = Some("Cancelling run...".to_string());
                } else {
                    self.quit = true;
                }
            } else {
                self.quit = true;
            }
            return Ask::Nothing;
        }

        // When an unanswered approval is open, route keys to it first so
        // `y`, `n`, `1-5`, arrows, and Enter answer the approval rather than
        // typing into the composer.
        let approval_cmd = if let Some(approval) = self.pending_approval_mut() {
            let chosen = approval.key(key, now);
            let cmd = chosen.map(|ans| {
                let id = approval.approval_id.clone();
                let digest = approval.digest.clone();
                match ans {
                    turn::Answer::ApproveOnce => OutboundCmd::ApproveAction {
                        approval_id: id,
                        action_digest: digest,
                        scope: "once".to_string(),
                    },
                    turn::Answer::ApproveSession => OutboundCmd::ApproveAction {
                        approval_id: id,
                        action_digest: digest,
                        scope: "session".to_string(),
                    },
                    turn::Answer::ApproveProject => OutboundCmd::ApproveAction {
                        approval_id: id,
                        action_digest: digest,
                        scope: "project".to_string(),
                    },
                    turn::Answer::Reject | turn::Answer::RejectWithReason => {
                        OutboundCmd::RejectAction {
                            approval_id: id,
                            action_digest: digest,
                            reason: String::new(),
                        }
                    }
                }
            });
            Some(cmd)
        } else {
            None
        };

        if let Some(maybe_cmd) = approval_cmd {
            if let Some(cmd) = maybe_cmd {
                if let Some(tx) = &self.cmd_tx {
                    let _ = tx.send(cmd);
                }
            }
            return Ask::Nothing;
        }

        match key.code {
            KeyCode::Char('d') if command => {
                if self.draft.is_empty() {
                    self.quit = true;
                }
            }
            KeyCode::Char('r') if command => {
                self.notice = None;
                self.link = Link::Probing;
                self.session_id = None;
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
                } else if self.screen == Screen::Home && self.runs() > 0 && self.turn.is_empty() {
                    self.screen = Screen::Run(self.selected);
                    self.scroll = 0;
                }
            }
            KeyCode::Esc => {
                if self.notice.is_some() {
                    self.notice = None;
                } else if !self.turn.is_empty() && !self.run_active {
                    self.turn.clear();
                    self.scroll = 0;
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

    /// Enter, with something typed. When connected with an open daemon session,
    /// dispatches `SubmitTurn` over the wire. Otherwise keeps the draft intact
    /// and states why it was not sent.
    fn submit(&mut self) {
        if matches!(self.link, Link::Open { .. }) && self.session_id.is_some() {
            if self.run_active {
                self.notice = Some("Not sent. A turn is already running.".to_string());
                return;
            }
            if let Some(tx) = &self.cmd_tx {
                let prompt: String = self.draft.iter().collect();
                if tx
                    .send(OutboundCmd::SubmitTurn {
                        prompt: prompt.trim().to_string(),
                    })
                    .is_ok()
                {
                    self.draft.clear();
                    self.cursor = 0;
                    self.scroll = 0;
                    self.run_active = true;
                    self.active_run_id = None;
                    self.notice = None;
                    return;
                }
            }
        }

        self.notice = Some(
            match &self.link {
                Link::Open { .. } => {
                    "Not sent. Daemon answered, waiting for the session to open."
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

fn extract_target(raw_args: &[u8]) -> Option<String> {
    let val: serde_json::Value = serde_json::from_slice(raw_args).ok()?;
    for key in ["path", "command", "pattern", "query"] {
        if let Some(s) = val.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn parse_file_diff(diff: &DiffChanged) -> turn::FileDiff {
    let path = diff
        .modified_files
        .first()
        .cloned()
        .unwrap_or_else(|| "workspace".to_string());
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut lines = Vec::new();
    for line in diff.diff_text.lines() {
        let cleaned = clean(line);
        if cleaned.starts_with("@@") {
            lines.push(turn::DiffLine::Hunk(cleaned));
        } else if let Some(rest) = cleaned.strip_prefix('+') {
            if !cleaned.starts_with("+++") {
                additions += 1;
                lines.push(turn::DiffLine::Add(rest.to_string()));
            }
        } else if let Some(rest) = cleaned.strip_prefix('-') {
            if !cleaned.starts_with("---") {
                deletions += 1;
                lines.push(turn::DiffLine::Del(rest.to_string()));
            }
        } else if let Some(rest) = cleaned.strip_prefix(' ') {
            lines.push(turn::DiffLine::Same(rest.to_string()));
        }
    }
    turn::FileDiff {
        path: clean(&path),
        status: "modified".to_string(),
        additions,
        deletions,
        lines,
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

    #[test]
    fn streaming_model_deltas_concatenate_inline_and_separate_new_messages() {
        let mut a = app();
        let make_delta = |seq: u64, msg_id: &str, role: &str, text: &str| {
            ProtocolEnvelope::carrying(
                type_url::event::MESSAGE_DELTA,
                "sess-1",
                "run-1",
                seq,
                "",
                &MessageDelta {
                    message_id: msg_id.to_string(),
                    role: role.to_string(),
                    delta_text: text.to_string(),
                },
            )
            .expect("delta envelope")
        };

        // Chunks sharing message_id concatenate inline without spurious newlines.
        a.update(Update::Wire(make_delta(1, "msg-round-1", "model", "Hello")));
        a.update(Update::Wire(make_delta(2, "msg-round-1", "model", ", ")));
        a.update(Update::Wire(make_delta(3, "msg-round-1", "model", "world!")));

        assert_eq!(a.turn.len(), 1);
        match &a.turn[0] {
            turn::Entry::Model(text) => assert_eq!(text, "Hello, world!"),
            other => panic!("expected Model entry, got {other:?}"),
        }

        // A new message_id separates onto a new line within the model block.
        a.update(Update::Wire(make_delta(4, "msg-round-2", "model", "Next")));
        a.update(Update::Wire(make_delta(5, "msg-round-2", "model", " line")));
        assert_eq!(a.turn.len(), 1);
        match &a.turn[0] {
            turn::Entry::Model(text) => assert_eq!(text, "Hello, world!\nNext line"),
            other => panic!("expected Model entry, got {other:?}"),
        }

        // An intervening user turn resets message tracking and opens a fresh Model entry.
        a.update(Update::Wire(make_delta(6, "user-2", "user", "follow-up")));
        a.update(Update::Wire(make_delta(7, "msg-round-3", "model", "Done")));
        assert_eq!(a.turn.len(), 3);
        match &a.turn[2] {
            turn::Entry::Model(text) => assert_eq!(text, "Done"),
            other => panic!("expected fresh Model entry, got {other:?}"),
        }
    }
}

