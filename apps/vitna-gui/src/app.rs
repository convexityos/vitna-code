//! The window.
//!
//! Built to sit beside Claude Code, Codex and OpenCode and hold its own: a wide
//! sidebar that lists work by name, a calm centre that says what it is looking
//! at and stops, and a composer with enough presence to be the thing you reach
//! for. Vitna's palette carries the brand, blue-black with one periwinkle.
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

/// Where a run executes. The daemon enforces this; the window only states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Local,
    Worktree,
}

impl Placement {
    pub const ALL: [Placement; 2] = [Placement::Local, Placement::Worktree];

    pub fn label(self) -> &'static str {
        match self {
            Placement::Local => "Local",
            Placement::Worktree => "Worktree",
        }
    }
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
    pub(crate) mode: Mode,
    pub(crate) placement: Placement,
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
            mode: Mode::Build,
            placement: Placement::Worktree,
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
        if matches!(self.repo.poll(), Loading::Reading) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
        }

        let full = ui.max_rect();
        let sidebar = Rect::from_min_size(full.min, Vec2::new(theme::SIDEBAR_W, full.height()));
        let main = Rect::from_min_max(egui::pos2(sidebar.right(), full.top()), full.max);

        self.sidebar(ui, sidebar);
        self.stage(ui, main);
    }
}
