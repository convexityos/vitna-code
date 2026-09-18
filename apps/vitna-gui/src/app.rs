//! The window.
//!
//! Built to sit beside Claude Code, Codex and OpenCode and hold its own: a wide
//! sidebar that lists work by name, a calm centre that says what it is looking
//! at and stops, and a composer with enough presence to be the thing you reach
//! for. Vitna's palette carries the brand, neutral grounds with one periwinkle.
//!
//! What this surface may claim is still bounded by what it can reach. The
//! daemon owns sessions and runs; this window owns the workspace facts it reads
//! off disk, a model PREFERENCE the daemon will be handed, and nothing more.

use eframe::egui::{self, Rect, Vec2};

use crate::catalog::{Catalog, Choice};
use crate::daemon::{self, Worker};
use crate::link::Link;
use crate::repo::{Loading, Probe, RepoFacts};
use crate::runs;
use crate::theme;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Build,
    Plan,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Build => "Build",
            Mode::Plan => "Plan",
        }
    }
}

/// Where a run executes: on this checkout in place, or in a fresh worktree of
/// it. The daemon enforces this; the window only states the preference, as
/// the worktree checkbox above the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Local,
    Worktree,
}

/// The inspector's tabs. Three, because three is what the receipt can answer
/// for on its own. Cursor's fourth and fifth (Terminal, Secrets) have nothing
/// behind them here, and an events tab needs the daemon to expose the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Receipt,
    Changes,
    Evidence,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Receipt => "Receipt",
            Tab::Changes => "Changes",
            Tab::Evidence => "Evidence",
        }
    }
}

/// The pages of the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    General,
    Shortcuts,
    Daemon,
    Providers,
    Models,
}

impl SettingsPage {
    /// For `VITNA_GUI_OPEN_SETTINGS=<page>`, so a capture can show a page.
    fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "shortcuts" => SettingsPage::Shortcuts,
            "daemon" => SettingsPage::Daemon,
            "providers" => SettingsPage::Providers,
            "models" => SettingsPage::Models,
            _ => SettingsPage::General,
        }
    }
}

/// An Edit-menu action, carried one frame so the composer can take focus
/// before the event is injected.
#[derive(Debug, Clone, Copy)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The composer's widget id, so the Edit menu can hand it focus.
pub fn composer_id() -> egui::Id {
    egui::Id::new("composer")
}

pub struct App {
    pub(crate) workspace: Workspace,
    pub(crate) repo: Probe,
    pub(crate) link: Link,
    /// The connection, on its own thread. The client blocks and a turn blocks
    /// for as long as the model takes, so none of it happens here.
    pub(crate) worker: Worker,
    /// Sessions as the daemon lists them. Empty until it answers, which is
    /// why the sidebar distinguishes "no sessions yet" from "not connected".
    pub(crate) sessions: Vec<vitna_protocol::api::SessionInfo>,
    /// The session a turn will be submitted against, once one exists.
    pub(crate) active_session: Option<String>,
    /// The prompts the daemon's event log holds, which is what names a
    /// session or a run. None until the daemon has answered.
    pub(crate) prompts: Option<crate::prompts::Prompts>,
    /// Why the prompts could not be read, when they could not.
    pub(crate) prompts_trouble: Option<String>,
    /// Set while a turn is in flight, so the composer says so rather than
    /// offering a Send that silently queues.
    pub(crate) turn_running: bool,
    /// The last thing the daemon said, good or bad, for the stage to show.
    pub(crate) last_result: Option<Box<vitna_protocol::api::TurnResultResponse>>,
    pub(crate) last_error: Option<String>,
    /// The receipts on disk. Read and verified by this window, not by the
    /// daemon that wrote them.
    pub(crate) runs: runs::Probe,
    /// Which run the centre is showing, as an index into the ledger. None is
    /// the start state.
    pub(crate) open_run: Option<usize>,
    pub(crate) tab: Tab,
    pub(crate) catalog: Option<Catalog>,
    pub(crate) choices: Vec<Choice>,
    /// Index into `choices`. A preference, handed to the daemon with the turn;
    /// the receipt, not this field, says what actually ran.
    pub(crate) model: Option<usize>,
    pub(crate) draft: String,
    /// The model menu's search text.
    pub(crate) model_query: String,
    /// Set from VITNA_GUI_OPEN_MODEL_MENU at startup; consumed on the first frame.
    pub(crate) open_model_menu_once: bool,
    /// Set from VITNA_GUI_OPEN_MENU at startup; consumed on the first frame.
    pub(crate) open_menu_once: bool,
    pub(crate) mode: Mode,
    pub(crate) placement: Placement,
    /// The open settings page, or None while the modal is closed.
    pub(crate) settings: Option<SettingsPage>,
    pub(crate) sidebar_open: bool,
    pub(crate) pending_edit: Option<EditAction>,
    /// The height the composer used last frame, plus its bottom pad. The
    /// stage hands it that much; measured rather than guessed, since the
    /// field's own margins and the row spacing are egui's to decide.
    pub(crate) composer_h: f32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, workspace: Workspace) -> Self {
        theme::install(&cc.egui_ctx);
        // The SVG loader behind the mark.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let repo = Probe::start(&workspace.path);
        let workspace_for_runs = workspace.path.clone();
        // The worker wakes the UI when a reply lands, since an idle window
        // would otherwise hold the answer until the next mouse move.
        let worker = Worker::spawn(cc.egui_ctx.clone());
        worker.send(daemon::Command::Connect);
        let catalog = Catalog::load().ok();
        let choices = catalog.as_ref().map(|c| c.choices()).unwrap_or_default();
        let model = catalog.as_ref().and_then(|c| c.default_choice(&choices));
        Self {
            workspace,
            repo,
            link: Link::Probing,
            worker,
            sessions: Vec::new(),
            active_session: None,
            prompts: None,
            prompts_trouble: None,
            turn_running: false,
            last_result: None,
            last_error: None,
            runs: runs::Probe::start(&workspace_for_runs),
            // VITNA_GUI_OPEN_RUN=<index> opens a run on the first frame, the
            // way the menu and settings hooks do, so a capture can show this
            // surface before the run list exists to reach it from.
            open_run: std::env::var("VITNA_GUI_OPEN_RUN")
                .ok()
                .map(|v| v.trim().parse::<usize>().unwrap_or(0)),
            tab: Tab::Receipt,
            catalog,
            choices,
            model,
            draft: String::new(),
            model_query: String::new(),
            open_model_menu_once: std::env::var_os("VITNA_GUI_OPEN_MODEL_MENU").is_some(),
            open_menu_once: std::env::var_os("VITNA_GUI_OPEN_MENU").is_some(),
            mode: Mode::Build,
            placement: Placement::Worktree,
            settings: std::env::var("VITNA_GUI_OPEN_SETTINGS")
                .ok()
                .map(|v| SettingsPage::from_name(&v)),
            sidebar_open: true,
            pending_edit: None,
            composer_h: 132.0,
        }
    }

    pub(crate) fn facts(&mut self) -> Option<RepoFacts> {
        match self.repo.poll() {
            Loading::Done(f) => Some((**f).clone()),
            Loading::Reading => None,
        }
    }

    /// Asks again. The answer arrives as an event, so this only states that
    /// the window is looking.
    pub(crate) fn reprobe(&mut self) {
        self.link = Link::Probing;
        self.worker.send(daemon::Command::Connect);
    }

    /// Asks the daemon for a session on this folder. The sidebar's button, the
    /// File menu and search all come here, so none of them can become a
    /// control that looks live and does nothing.
    pub(crate) fn new_session(&mut self) {
        if self.link.is_open() {
            self.worker.send(daemon::Command::CreateSession(
                self.workspace.path.clone(),
            ));
        }
    }

    /// Takes everything the worker has posted since the last frame.
    pub(crate) fn drain_daemon(&mut self) {
        for event in self.worker.drain() {
            match event {
                daemon::Event::Connected { endpoint, health } => {
                    self.link = Link::Open { endpoint, health };
                    self.last_error = None;
                    // The session list is a fact of the daemon, so it is asked
                    // for rather than assumed empty, and so are the prompts
                    // that name its sessions.
                    self.worker.send(daemon::Command::ListSessions);
                    self.worker.send(daemon::Command::ListTurns);
                }
                daemon::Event::Absent { tried, detail } => {
                    self.link = Link::Absent { tried, detail };
                    self.sessions.clear();
                    self.active_session = None;
                    self.turn_running = false;
                }
                daemon::Event::SessionCreated(s) => {
                    self.active_session = Some(s.session_id.clone());
                    self.last_error = None;
                    self.worker.send(daemon::Command::ListSessions);
                }
                daemon::Event::Sessions(list) => {
                    if self.active_session.is_none() {
                        self.active_session = list.first().map(|s| s.session_id.clone());
                    }
                    self.sessions = list;
                }
                daemon::Event::Turns(list) => {
                    self.prompts = Some(crate::prompts::Prompts::from_list(*list));
                    self.prompts_trouble = None;
                }
                daemon::Event::TurnsFailed(reason) => {
                    self.prompts_trouble = Some(reason);
                }
                daemon::Event::TurnFinished(result) => {
                    self.turn_running = false;
                    self.last_error = None;
                    self.last_result = Some(result);
                    self.worker.send(daemon::Command::ListSessions);
                    self.worker.send(daemon::Command::ListTurns);
                    // A receipt was just written, so the ledger is stale. The
                    // new run becomes the open one.
                    self.runs = runs::Probe::start(&self.workspace.path);
                    self.open_run = Some(0);
                }
                daemon::Event::Failed(message) => {
                    self.turn_running = false;
                    self.last_error = Some(message);
                }
            }
        }
    }

    /// "1 of 2 providers ready", from the environment, not from a guess.
    pub(crate) fn providers_ready(&self) -> (usize, usize) {
        match &self.catalog {
            Some(c) => (
                c.providers.iter().filter(|p| Catalog::provider_ready(p)).count(),
                c.providers.len(),
            ),
            None => (0, 0),
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = theme::CANVAS;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_daemon();
        self.shortcuts(ui.ctx());
        self.run_pending_edit(ui.ctx());

        if matches!(self.repo.poll(), Loading::Reading) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }

        let full = ui.max_rect();
        let side_w = if self.sidebar_open { theme::SIDEBAR_W } else { 0.0 };
        let sidebar = Rect::from_min_size(full.min, Vec2::new(side_w, full.height()));
        let main = Rect::from_min_max(egui::pos2(sidebar.right(), full.top()), full.max);

        if self.sidebar_open {
            self.sidebar(ui, sidebar);
        }

        match self.open_run {
            // A run is open: Cursor's split, the run in the centre and the
            // inspector down the right. The composer stays, because a follow
            // up is another turn on the same session rather than a new place.
            Some(_) => {
                let w = crate::inspector::WIDTH.min(main.width() * 0.42);
                let centre =
                    Rect::from_min_max(main.min, egui::pos2(main.right() - w, main.bottom()));
                let panel =
                    Rect::from_min_max(egui::pos2(centre.right(), main.top()), main.max);

                let composer_h = self.composer_h;
                let body = Rect::from_min_max(
                    centre.min,
                    egui::pos2(centre.right(), centre.bottom() - composer_h),
                );
                let composer_rect = Rect::from_min_max(
                    egui::pos2(centre.left(), body.bottom()),
                    centre.max,
                );

                self.run_view(ui, crate::stage::column(body));
                self.composer(ui, composer_rect);
                self.inspector(ui, panel);
            }
            None => self.stage(ui, main),
        }
        // The strip goes last so its menu sits over everything else.
        self.strip(ui, full);
        self.settings_modal(ui);
    }
}
