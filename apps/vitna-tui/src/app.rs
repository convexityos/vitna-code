//! What the terminal knows, and what each key does to it.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;
use vitna_protocol::messages::{
    run_state, tool_status, AgentCompleted, AgentSpawned, ApprovalRequested, Diagnostic,
    DiffChanged, ErrorResponse, MessageDelta, RunStateChanged, ToolFinished, ToolOutput,
    ToolProposed, ToolStarted, UsageUpdated,
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
    SessionMode(String),
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
    /// Active session mode (`"build"` or `"inspect"`).
    pub session_mode: String,
    /// The most recently observed run id in this session.
    pub active_run_id: Option<String>,
    /// True while a turn is in flight on the daemon.
    pub run_active: bool,
    /// Latest token and cost accounting reported by the daemon for the active run.
    pub usage: Option<UsageUpdated>,
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
            session_mode: "build".to_string(),
            active_run_id: None,
            run_active: false,
            usage: None,
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
            Update::SessionMode(mode) => {
                self.session_mode = mode;
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
            type_url::event::AGENT_SPAWNED => {
                if let Ok(spawned) = env.payload_as::<AgentSpawned>() {
                    self.last_model_message_id = None;
                    let goal = clean(&spawned.goal);
                    let child = clean(&spawned.child_run_id);
                    let text = if goal.is_empty() {
                        format!("Child agent {child} spawned.")
                    } else {
                        format!("Child agent {child} spawned: {goal}")
                    };
                    self.turn.push(turn::Entry::Note {
                        tone: theme::FAINT,
                        text,
                    });
                }
                false
            }
            type_url::event::AGENT_COMPLETED => {
                if let Ok(done) = env.payload_as::<AgentCompleted>() {
                    self.last_model_message_id = None;
                    let child = clean(&done.child_run_id);
                    let state = clean(&done.completion_state);
                    let digest = clean(&done.result_artifact_digest);
                    let tone = if state.contains("fail") || state.contains("cancel") {
                        theme::RUST
                    } else {
                        theme::FAINT
                    };
                    let text = if digest.is_empty() {
                        format!("Child agent {child} {state}.")
                    } else {
                        format!("Child agent {child} {state} (digest: {digest}).")
                    };
                    self.turn.push(turn::Entry::Note { tone, text });
                    return true;
                }
                false
            }
            type_url::event::USAGE_UPDATED => {
                if let Ok(usage) = env.payload_as::<UsageUpdated>() {
                    self.usage = Some(usage);
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
                    return self.submit();
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
    /// dispatches slash commands (`/mode`, `/steer`, `/pause`, `/resume`,
    /// `/cancel`, `/spawn`, `/apply`) or `SubmitTurn` (with `@path` workspace
    /// file attachments) over the wire. Otherwise keeps the draft intact and
    /// states why it was not sent.
    fn submit(&mut self) -> Ask {
        let raw: String = self.draft.iter().collect();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ask::Nothing;
        }

        // `/receipt [run_id]` works offline too because signed receipts live on
        // disk in `.vitna/receipts/`.
        if trimmed.starts_with('/') && !trimmed.starts_with("//") {
            let mut parts = trimmed[1..].splitn(2, char::is_whitespace);
            let cmd_name = parts.next().unwrap_or("").to_ascii_lowercase();
            let arg = parts.next().unwrap_or("").trim();
            if cmd_name == "receipt" {
                let target_idx = if let Some(ledger) = &self.ledger {
                    if ledger.entries.is_empty() {
                        None
                    } else if arg.is_empty() {
                        Some(0)
                    } else {
                        ledger.entries.iter().position(|entry| match entry {
                            crate::ledger::Entry::Run(r) => {
                                r.run_id == arg || r.run_id.starts_with(arg) || r.file == arg
                            }
                            crate::ledger::Entry::Unreadable(u) => u.file == arg,
                        })
                    }
                } else {
                    None
                };

                self.draft.clear();
                self.cursor = 0;
                if let Some(idx) = target_idx {
                    self.turn.clear();
                    self.screen = Screen::Run(idx);
                    self.selected = idx;
                    self.scroll = 0;
                    self.notice = None;
                    return Ask::Nothing;
                }
                self.notice = Some(if self.ledger.is_none() {
                    "Loading receipts from disk...".to_string()
                } else if arg.is_empty() {
                    "No receipts recorded in .vitna/receipts yet.".to_string()
                } else {
                    format!("No receipt matching '{arg}' found in .vitna/receipts.")
                });
                return if self.ledger.is_none() {
                    Ask::RefreshLedger
                } else {
                    Ask::Nothing
                };
            }
            if cmd_name == "export" {
                let level = if arg.is_empty() {
                    "standard".to_string()
                } else {
                    arg.to_ascii_lowercase()
                };
                if !matches!(level.as_str(), "none" | "standard" | "strict") {
                    self.notice = Some(
                        "Usage: /export [none|standard|strict] (defaults to standard)."
                            .to_string(),
                    );
                    return Ask::Nothing;
                }
                let file_name = self.ledger.as_ref().and_then(|ledger| {
                    let idx = match self.screen {
                        Screen::Run(i) => i,
                        Screen::Home => self.selected,
                    };
                    ledger.entries.get(idx).map(|entry| match entry {
                        crate::ledger::Entry::Run(r) => r.file.clone(),
                        crate::ledger::Entry::Unreadable(u) => u.file.clone(),
                    })
                });
                self.draft.clear();
                self.cursor = 0;
                let Some(file_name) = file_name else {
                    self.notice = Some("No receipt selected to export.".to_string());
                    return Ask::Nothing;
                };
                match crate::ledger::export_receipt(&self.root, &file_name, &level) {
                    Ok(rel_path) => {
                        self.notice = Some(format!("Exported {file_name} ({level}) -> {rel_path}"));
                    }
                    Err(e) => {
                        self.notice = Some(format!("Export failed: {}", clean(&e)));
                    }
                }
                return Ask::Nothing;
            }
        }

        if matches!(self.link, Link::Open { .. }) && self.session_id.is_some() {
            if trimmed.starts_with('/') && !trimmed.starts_with("//") {
                let mut parts = trimmed[1..].splitn(2, char::is_whitespace);
                let cmd_name = parts.next().unwrap_or("").to_ascii_lowercase();
                let arg = parts.next().unwrap_or("").trim();

                if cmd_name == "mode" {
                    if self.run_active {
                        self.notice = Some(
                            "Wait for the active run to finish or /cancel before switching modes."
                                .to_string(),
                        );
                        return Ask::Nothing;
                    }
                    let target = arg.to_ascii_lowercase();
                    if !matches!(target.as_str(), "inspect" | "build") {
                        self.notice = Some(format!(
                            "Current mode: {}. Use /mode inspect or /mode build.",
                            self.session_mode
                        ));
                        return Ask::Nothing;
                    }
                    self.session_mode = target.clone();
                    self.session_id = None;
                    self.active_run_id = None;
                    self.turn.clear();
                    self.scroll = 0;
                    self.draft.clear();
                    self.cursor = 0;
                    self.notice = Some(if target == "inspect" {
                        "Switched to inspect (read-only) mode. Reconnecting session...".to_string()
                    } else {
                        "Switched to build mode. Reconnecting session...".to_string()
                    });
                    return Ask::Refresh;
                }

                let outbound = match cmd_name.as_str() {
                    "steer" => {
                        if !self.run_active {
                            self.notice = Some("No active run to steer.".to_string());
                            return Ask::Nothing;
                        }
                        if arg.is_empty() {
                            self.notice = Some("Write feedback after /steer.".to_string());
                            return Ask::Nothing;
                        }
                        let Some(run_id) = self.active_run_id.clone() else {
                            self.notice = Some("Waiting for run id before steering.".to_string());
                            return Ask::Nothing;
                        };
                        OutboundCmd::SteerRun {
                            run_id,
                            feedback: arg.to_string(),
                        }
                    }
                    "pause" => {
                        if !self.run_active {
                            self.notice = Some("No active run to pause.".to_string());
                            return Ask::Nothing;
                        }
                        let Some(run_id) = self.active_run_id.clone() else {
                            self.notice = Some("Waiting for run id before pausing.".to_string());
                            return Ask::Nothing;
                        };
                        OutboundCmd::PauseRun {
                            run_id,
                            reason: if arg.is_empty() {
                                "paused from vitna-tui".to_string()
                            } else {
                                arg.to_string()
                            },
                        }
                    }
                    "resume" => {
                        if !self.run_active {
                            self.notice = Some("No active run to resume.".to_string());
                            return Ask::Nothing;
                        }
                        let Some(run_id) = self.active_run_id.clone() else {
                            self.notice = Some("Waiting for run id before resuming.".to_string());
                            return Ask::Nothing;
                        };
                        OutboundCmd::ResumeRun { run_id }
                    }
                    "cancel" => {
                        if !self.run_active {
                            self.notice = Some("No active run to cancel.".to_string());
                            return Ask::Nothing;
                        }
                        let Some(run_id) = self.active_run_id.clone() else {
                            self.notice = Some("Waiting for run id before cancelling.".to_string());
                            return Ask::Nothing;
                        };
                        OutboundCmd::CancelRun {
                            run_id,
                            reason: if arg.is_empty() {
                                "cancelled from vitna-tui".to_string()
                            } else {
                                arg.to_string()
                            },
                        }
                    }
                    "spawn" => {
                        if arg.is_empty() {
                            self.notice = Some("Write a goal after /spawn.".to_string());
                            return Ask::Nothing;
                        }
                        let Some(parent_run_id) = self.active_run_id.clone() else {
                            self.notice =
                                Some("Run a parent turn first before /spawn.".to_string());
                            return Ask::Nothing;
                        };
                        OutboundCmd::SpawnChild {
                            parent_run_id,
                            goal: arg.to_string(),
                        }
                    }
                    "apply" => {
                        let mut tokens = arg.split_whitespace();
                        let Some(change_set_id) = tokens.next() else {
                            self.notice =
                                Some("Write a changeset id after /apply.".to_string());
                            return Ask::Nothing;
                        };
                        let expected_base_commit = tokens.next().unwrap_or("").to_string();
                        OutboundCmd::ApplyChangeSet {
                            change_set_id: change_set_id.to_string(),
                            target_checkout_path: self.root.to_string_lossy().into_owned(),
                            expected_base_commit,
                        }
                    }
                    other => {
                        self.notice = Some(format!(
                            "Unknown command /{other}. Commands: /mode, /receipt, /export, /steer, /pause, /resume, /cancel, /spawn, /apply."
                        ));
                        return Ask::Nothing;
                    }
                };

                if let Some(tx) = &self.cmd_tx {
                    if tx.send(outbound).is_ok() {
                        self.draft.clear();
                        self.cursor = 0;
                        self.notice = None;
                        return Ask::Nothing;
                    }
                }
            } else {
                if self.run_active {
                    self.notice = Some(
                        "Not sent. A turn is already running. Use /steer, /pause, /resume, or /cancel."
                            .to_string(),
                    );
                    return Ask::Nothing;
                }
                if let Some(tx) = &self.cmd_tx {
                    let prompt = if let Some(rest) = trimmed.strip_prefix("//") {
                        format!("/{rest}")
                    } else {
                        trimmed.to_string()
                    };
                    let attached_file_paths = parse_attachments(&prompt);
                    if tx
                        .send(OutboundCmd::SubmitTurn {
                            prompt,
                            attached_file_paths,
                        })
                        .is_ok()
                    {
                        self.draft.clear();
                        self.cursor = 0;
                        self.scroll = 0;
                        self.run_active = true;
                        self.active_run_id = None;
                        self.notice = None;
                        return Ask::Nothing;
                    }
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
        Ask::Nothing
    }
}

/// Extracts deduplicated `@path` workspace file attachments from a prompt,
/// matching the desktop client's `composerInput.ts` rules.
pub fn parse_attachments(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in prompt.split_whitespace() {
        let Some(rest) = token.strip_prefix('@') else {
            continue;
        };
        if rest.is_empty() || rest.contains('@') {
            continue;
        }
        let cleaned = rest.trim_end_matches([')', ',', '.', ';', ':', '!', '?']);
        if !cleaned.is_empty() && !out.iter().any(|existing| existing == cleaned) {
            out.push(cleaned.to_string());
        }
    }
    out
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

    #[test]
    fn parse_attachments_and_slash_commands_dispatch_expected_protocol_commands() {
        assert_eq!(
            parse_attachments("inspect @src/lib.rs, @README.md! and @src/lib.rs"),
            vec!["src/lib.rs".to_string(), "README.md".to_string()]
        );
        assert!(parse_attachments("contact user@example.com").is_empty());

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut a = app();
        a.link = Link::Open {
            version: "1.0".into(),
        };
        a.session_id = Some("sess-1".into());
        a.cmd_tx = Some(tx);

        // Plain prompt with @path attaches file and starts run.
        typed(&mut a, "check @README.md.");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::SubmitTurn {
                prompt: "check @README.md.".into(),
                attached_file_paths: vec!["README.md".into()],
            }
        );

        // Active run slash commands: /steer, /pause, /resume, /cancel.
        a.active_run_id = Some("run-42".into());
        typed(&mut a, "/steer keep tests fast");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::SteerRun {
                run_id: "run-42".into(),
                feedback: "keep tests fast".into(),
            }
        );

        typed(&mut a, "/pause checking diff");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::PauseRun {
                run_id: "run-42".into(),
                reason: "checking diff".into(),
            }
        );

        typed(&mut a, "/resume");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::ResumeRun {
                run_id: "run-42".into(),
            }
        );

        typed(&mut a, "/spawn audit tests");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::SpawnChild {
                parent_run_id: "run-42".into(),
                goal: "audit tests".into(),
            }
        );

        typed(&mut a, "/apply cs-99 abcdef");
        a.key(press(KeyCode::Enter));
        assert_eq!(
            rx.try_recv().unwrap(),
            OutboundCmd::ApplyChangeSet {
                change_set_id: "cs-99".into(),
                target_checkout_path: a.root.to_string_lossy().into_owned(),
                expected_base_commit: "abcdef".into(),
            }
        );

        // Unknown slash command keeps draft intact and reports valid commands.
        typed(&mut a, "/bogus");
        a.key(press(KeyCode::Enter));
        assert_eq!(a.draft.iter().collect::<String>(), "/bogus");
        assert!(a.notice.as_deref().is_some_and(|n| n.contains("Unknown command")));
    }

    #[test]
    fn child_agent_and_usage_events_update_app_state() {
        let mut a = app();
        let spawned = ProtocolEnvelope::carrying(
            type_url::event::AGENT_SPAWNED,
            "sess-1",
            "run-1",
            1,
            "",
            &AgentSpawned {
                child_run_id: "child-1".into(),
                parent_run_id: "run-1".into(),
                goal: "write unit tests".into(),
                agent_workspace_path: "/tmp/ws-child".into(),
            },
        )
        .unwrap();
        assert_eq!(a.apply_update(Update::Wire(spawned)), Ask::Nothing);

        let completed = ProtocolEnvelope::carrying(
            type_url::event::AGENT_COMPLETED,
            "sess-1",
            "run-1",
            2,
            "",
            &AgentCompleted {
                child_run_id: "child-1".into(),
                completion_state: "completed".into(),
                result_artifact_digest: "deadbeef".into(),
            },
        )
        .unwrap();
        assert_eq!(a.apply_update(Update::Wire(completed)), Ask::RefreshLedger);
        assert_eq!(a.turn.len(), 2);

        let usage = ProtocolEnvelope::carrying(
            type_url::event::USAGE_UPDATED,
            "sess-1",
            "run-1",
            3,
            "",
            &UsageUpdated {
                run_id: "run-1".into(),
                provider: "anthropic".into(),
                model_sku: "claude-sonnet-4-5".into(),
                prompt_tokens: 120,
                completion_tokens: 45,
                cached_tokens: 10,
                cost_known: false,
                cost_usd: 0.0,
            },
        )
        .unwrap();
        assert_eq!(a.apply_update(Update::Wire(usage)), Ask::Nothing);
        assert_eq!(a.usage.as_ref().map(|u| u.prompt_tokens), Some(120));
    }

    #[test]
    fn receipt_slash_command_opens_receipt_even_offline() {
        let dir = std::env::temp_dir().join(format!(
            "vitna_tui_receipt_cmd_{}",
            std::process::id()
        ));
        let receipts = dir.join(".vitna").join("receipts");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&receipts).unwrap();
        let make_receipt = |run_id: &str| {
            serde_json::json!({
                "schema_version": "vitna-run-receipt-v1",
                "run_id": run_id,
                "session_id": "sess-1",
                "workspace_fingerprint": "fp",
                "base_commit_sha": "0000000000000000000000000000000000000000",
                "model_selection": { "provider": "fake", "model_sku": "m", "routing_reason": "r" },
                "event_hash_chain_root": "root",
                "isolation_label": "guarded",
                "completion_state": "completed_with_evidence",
                "evidence_items": [],
                "changeset": { "files_modified": [], "diff_digest": "d" },
                "runner_execution_statements": [],
                "child_receipt_roots": [],
                "device_signature": "00"
            })
            .to_string()
        };
        std::fs::write(receipts.join("run-alpha.json"), make_receipt("run-alpha")).unwrap();
        std::fs::write(receipts.join("run-beta.json"), make_receipt("run-beta")).unwrap();

        let mut a = App::new(dir.clone());
        a.update(Update::Ledger(crate::ledger::read(&dir)));
        // Still disconnected (Link::Probing, no daemon): `/receipt` must still work!
        for c in "/receipt".chars() {
            a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Ask::Nothing);
        assert_eq!(a.screen, Screen::Run(0));
        assert!(a.draft.is_empty());

        // Jump by run_id prefix to the other receipt.
        let target_id = match &a.ledger.as_ref().unwrap().entries[1] {
            crate::ledger::Entry::Run(r) => r.run_id.clone(),
            crate::ledger::Entry::Unreadable(u) => u.file.clone(),
        };
        for c in format!("/receipt {target_id}").chars() {
            a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Ask::Nothing);
        assert_eq!(a.screen, Screen::Run(1));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_slash_command_redacts_and_strips_signature_offline() {
        let dir = std::env::temp_dir().join(format!(
            "vitna_tui_export_cmd_{}",
            std::process::id()
        ));
        let receipts = dir.join(".vitna").join("receipts");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&receipts).unwrap();

        let key = vitna_receipts::generate_signing_key();
        let mut receipt = vitna_receipts::VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-export-1".to_string(),
            session_id: "sess-export".to_string(),
            workspace_fingerprint: "secret-workspace-fp".to_string(),
            base_commit_sha: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            model_selection: vitna_receipts::ModelSelectionRecord {
                provider: "fake".to_string(),
                model_sku: "m".to_string(),
                routing_reason: "r".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "root".to_string(),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![vitna_receipts::EvidenceItemRecord {
                evidence_id: "ev-1".to_string(),
                grade: "runner_verified".to_string(),
                description: "secret test output".to_string(),
                artifact_digest: Some("abc123".to_string()),
            }],
            changeset: vitna_receipts::ChangeSetRecord {
                files_modified: vec![vitna_receipts::FileModificationRecord {
                    path: "private/secret/app.rs".to_string(),
                    preimage_hash: "0".repeat(64),
                    postimage_hash: "1".repeat(64),
                }],
                diff_digest: "d".to_string(),
            },
            runner_execution_statements: vec![],
            egress_calls: vec![],
            child_receipt_roots: vec![],
            device_public_key: Some(hex::encode(key.verifying_key().to_bytes())),
            mandate_provenance: None,
            device_signature: String::new(),
        };
        receipt.sign(&key).unwrap();
        std::fs::write(
            receipts.join("run-export-1.json"),
            serde_json::to_string_pretty(&receipt).unwrap(),
        )
        .unwrap();

        let mut a = App::new(dir.clone());
        a.update(Update::Ledger(crate::ledger::read(&dir)));

        // Standard export (default when no arg given) redacts paths and strips signature.
        for c in "/export".chars() {
            a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Ask::Nothing);
        let std_path = dir.join(".vitna/exports/run-export-1-standard.json");
        let std_doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&std_path).unwrap()).unwrap();
        assert_eq!(std_doc["redaction"], "standard");
        assert_eq!(std_doc["signature_stripped"], true);
        assert_eq!(std_doc["receipt"]["device_signature"], "");
        assert_eq!(
            std_doc["receipt"]["changeset"]["files_modified"][0]["path"],
            "<redacted>/app.rs"
        );

        // Strict export also strips file basenames and evidence descriptions.
        for c in "/export strict".chars() {
            a.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Ask::Nothing);
        let strict_path = dir.join(".vitna/exports/run-export-1-strict.json");
        let strict_doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&strict_path).unwrap()).unwrap();
        assert_eq!(strict_doc["redaction"], "strict");
        assert_eq!(
            strict_doc["receipt"]["changeset"]["files_modified"][0]["path"],
            "redacted-file-0"
        );
        assert_eq!(
            strict_doc["receipt"]["evidence_items"][0]["description"],
            "[REDACTED]"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}


