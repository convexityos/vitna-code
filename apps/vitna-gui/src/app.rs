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
use crate::link::{self, Link};
use crate::repo::{Loading, Probe, RepoFacts};
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
        let catalog = Catalog::load().ok();
        let choices = catalog.as_ref().map(|c| c.choices()).unwrap_or_default();
        let model = catalog.as_ref().and_then(|c| c.default_choice(&choices));
        Self {
            workspace,
            repo,
            link: link::probe(),
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

    pub(crate) fn reprobe(&mut self) {
        self.link = link::probe();
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
        self.stage(ui, main);
        // The strip goes last so its menu sits over everything else.
        self.strip(ui, full);
        self.settings_modal(ui);
    }
}
