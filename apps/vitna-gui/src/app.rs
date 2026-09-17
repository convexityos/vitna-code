//! The window.
//!
//! What this surface may claim is bounded by what it can reach. The daemon owns
//! sessions, runs, policy and model choice; this window owns the workspace facts
//! it reads off disk and nothing else. So with no daemon it shows a real
//! workspace and an honest empty session list, and it does not offer a Send
//! button that cannot send.

use eframe::egui::{self, Align, Layout, RichText};

use crate::link::{self, Link};
use crate::theme;
use crate::workspace::{self, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Build,
    Plan,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Build => "build",
            Mode::Plan => "plan",
        }
    }
}

/// Where a run executes. The daemon enforces this; the window only states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The workspace itself.
    Local,
    /// A disposable clone, which is what `crates/git-workspaces` makes.
    Worktree,
}

impl Placement {
    fn label(self) -> &'static str {
        match self {
            Placement::Local => "local",
            Placement::Worktree => "worktree",
        }
    }
}

pub struct App {
    workspace: Workspace,
    link: Link,
    draft: String,
    mode: Mode,
    placement: Placement,
    mono_face: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, workspace: Workspace) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            workspace,
            link: link::probe(),
            draft: String::new(),
            mode: Mode::Build,
            placement: Placement::Worktree,
            mono_face: theme::mono_face_name(),
        }
    }

    fn reprobe(&mut self) {
        self.link = link::probe();
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
        // Order matters: the side panel claims its edge first, the composer
        // takes the bottom of what is left, and the stage fills the remainder.
        self.rail(ui);
        self.composer(ui);
        self.stage(ui);
    }
}

/// A small label in the mono face, at or above the type floor.
fn meta(text: impl Into<String>, color: egui::Color32) -> RichText {
    RichText::new(text).font(theme::mono(theme::FS_META)).color(color)
}

fn mark(text: impl Into<String>, color: egui::Color32) -> RichText {
    RichText::new(text)
        .font(theme::mono(theme::FS_MARK))
        .color(color)
}

/// A section heading in the rail.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(mark(title.to_uppercase(), theme::FAINTER));
    ui.add_space(4.0);
}

impl App {
    fn rail(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("rail")
            .exact_size(268.0)
            .resizable(false)
            .frame(
                egui::Frame::default()
                    .fill(theme::RAIL)
                    .inner_margin(egui::Margin::symmetric(16, 14)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Vitna Code")
                            .font(theme::sans(theme::FS_SMALL))
                            .color(theme::INK),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(mark(env!("CARGO_PKG_VERSION"), theme::FAINTER));
                    });
                });

                ui.add_space(12.0);

                // A session is the daemon's to create. With no daemon the
                // control is shown disabled with the reason, rather than hidden
                // (which would read as a product without sessions) or enabled
                // (which would fail on click).
                let can_start = self.link.is_open();
                let start = ui.add_enabled(
                    can_start,
                    egui::Button::new(
                        RichText::new("+  New session")
                            .font(theme::sans(theme::FS_SMALL))
                            .color(if can_start { theme::INK } else { theme::FAINT }),
                    )
                    .fill(theme::FACE)
                    .min_size(egui::vec2(ui.available_width(), 32.0)),
                );
                if !can_start {
                    start.on_hover_text("vitna-coded is not running, and it is the daemon that opens a session.");
                }

                section(ui, "workspace");
                ui.label(
                    RichText::new(&self.workspace.name)
                        .font(theme::sans(theme::FS_SMALL))
                        .color(theme::INK_2),
                );
                match &self.workspace.head {
                    Some(head) => {
                        ui.label(meta(format!("git  {}", head.label()), theme::MUTE));
                    }
                    None => {
                        ui.label(meta("not a git repository", theme::FAINT));
                    }
                }

                section(ui, "sessions");
                ui.label(meta("none", theme::FAINT));
                ui.label(
                    RichText::new(
                        "The daemon keeps sessions. This window shows the ones it reports and invents none.",
                    )
                    .font(theme::sans(theme::FS_META))
                    .color(theme::FAINTER),
                );

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.label(mark(format!("drawing with {}", self.mono_face), theme::FAINTER));
                    ui.add_space(6.0);
                    self.link_line(ui);
                });
            });
    }

    /// One dot and the word beside it, which is the tokens' rule for state.
    fn link_line(&mut self, ui: &mut egui::Ui) {
        let (dot, word) = match &self.link {
            Link::Open { .. } => (theme::OK, "connected"),
            Link::Absent { .. } => (theme::RUST, "no daemon"),
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 3.5, dot);
            ui.label(meta(word, theme::MUTE));
        });
    }
}

impl App {
    fn stage(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::no_frame()
            .frame(
                egui::Frame::default()
                    .fill(theme::CANVAS)
                    .inner_margin(egui::Margin::symmetric(40, 34)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("New session")
                        .font(theme::sans(22.0))
                        .color(theme::INK),
                );
                ui.add_space(18.0);

                fact(ui, "folder", &self.workspace.path.to_string_lossy());

                match &self.workspace.head {
                    Some(head) => fact(ui, "branch", &head.label()),
                    None => fact(ui, "branch", "not a git repository"),
                }

                match self.workspace.last_modified.and_then(workspace::ago) {
                    Some(age) => {
                        let note = if self.workspace.scan_was_capped {
                            format!(
                                "{age}  (newest of the first {} entries)",
                                self.workspace.entries_scanned
                            )
                        } else {
                            age
                        };
                        fact(ui, "last modified", &note);
                    }
                    None => fact(ui, "last modified", "unknown"),
                }

                ui.add_space(26.0);

                if let Link::Absent { tried, detail } = self.link.clone() {
                    self.absent_plate(ui, &tried, &detail);
                }
            });
    }

    /// The one object here that is a record rather than furniture, so it sits on
    /// the plate, a step BELOW the ground, per the tokens.
    fn absent_plate(&mut self, ui: &mut egui::Ui, tried: &[String], detail: &str) {
        egui::Frame::default()
            .fill(theme::PLATE)
            .stroke(egui::Stroke::new(1.0, theme::HAIR_2))
            .corner_radius(theme::R_SM)
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_max_width(700.0);
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 3.5, theme::RUST);
                    ui.label(
                        RichText::new("vitna-coded is not running")
                            .font(theme::sans(theme::FS_SMALL))
                            .color(theme::INK_2),
                    );
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "This window reaches the daemon over an owner-only pipe or socket and \
                         never over a network port, so nothing here can be served from a \
                         browser. Until the daemon listens there is no session to show.",
                    )
                    .font(theme::sans(theme::FS_META))
                    .color(theme::FAINT),
                );
                ui.add_space(12.0);

                ui.label(mark("ENDPOINTS TRIED", theme::FAINTER));
                ui.add_space(3.0);
                if tried.is_empty() {
                    ui.label(meta(detail, theme::FAINT));
                } else {
                    for endpoint in tried {
                        ui.label(meta(endpoint, theme::MUTE));
                    }
                    ui.add_space(6.0);
                    ui.label(meta(detail, theme::FAINTER));
                }

                ui.add_space(12.0);
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Look again")
                                .font(theme::sans(theme::FS_META))
                                .color(theme::INK_2),
                        )
                        .fill(theme::CONTROL),
                    )
                    .clicked()
                {
                    self.reprobe();
                }
            });
    }
}

/// A labelled fact: the label in mark, the value in mono, one per line.
fn fact(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(120.0, 20.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(mark(label.to_uppercase(), theme::FAINTER));
            },
        );
        ui.label(
            RichText::new(value)
                .font(theme::mono(theme::FS_SMALL))
                .color(theme::INK_2),
        );
    });
    ui.add_space(6.0);
}

impl App {
    fn composer(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("composer")
            .frame(
                egui::Frame::default()
                    .fill(theme::CANVAS)
                    .inner_margin(egui::Margin { left: 40, right: 40, top: 0, bottom: 22 }),
            )
            .show_separator_line(false)
            .show(ui, |ui| {
                egui::Frame::default()
                    .fill(theme::FIELD)
                    .stroke(egui::Stroke::new(1.0, theme::HAIR_2))
                    .corner_radius(theme::R)
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.draft)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                // The field around it already draws the surface.
                                .frame(egui::Frame::default())
                                .font(theme::mono(theme::FS_SMALL))
                                .hint_text(
                                    RichText::new("Ask Vitna Code to change something.")
                                        .font(theme::mono(theme::FS_SMALL))
                                        .color(theme::FAINTER),
                                ),
                        );

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            self.mode_chip(ui);
                            self.placement_chip(ui);

                            // The daemon's policy picks the model. This window
                            // has no policy engine, so it names the mechanism
                            // rather than printing a model it did not choose.
                            ui.label(meta("model chosen by policy", theme::FAINTER));

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                self.send_button(ui);
                            });
                        });
                    });
            });
    }

    fn mode_chip(&mut self, ui: &mut egui::Ui) {
        let next = match self.mode {
            Mode::Build => Mode::Plan,
            Mode::Plan => Mode::Build,
        };
        if chip(ui, self.mode.label(), theme::PERI).clicked() {
            self.mode = next;
        }
    }

    fn placement_chip(&mut self, ui: &mut egui::Ui) {
        let next = match self.placement {
            Placement::Local => Placement::Worktree,
            Placement::Worktree => Placement::Local,
        };
        // Rust marks the provisional choice: running in the workspace itself is
        // the one that can leave changes behind.
        let tone = match self.placement {
            Placement::Local => theme::RUST,
            Placement::Worktree => theme::MUTE,
        };
        let response = chip(ui, self.placement.label(), tone);
        if response.clicked() {
            self.placement = next;
        }
        response.on_hover_text(match self.placement {
            Placement::Local => "Runs in this workspace. Changes land in the files you have open.",
            Placement::Worktree => "Runs in a disposable clone. The workspace is untouched.",
        });
    }

    fn send_button(&mut self, ui: &mut egui::Ui) {
        let ready = self.link.is_open() && !self.draft.trim().is_empty();
        let response = ui.add_enabled(
            ready,
            egui::Button::new(
                RichText::new("Send")
                    .font(theme::sans(theme::FS_META))
                    .color(if ready { theme::INK } else { theme::FAINTER }),
            )
            .fill(if ready { theme::CONTROL } else { theme::FACE }),
        );
        if !self.link.is_open() {
            response.on_hover_text(
                "Nothing to send to: vitna-coded is not running. A turn is submitted to the daemon, never from this window directly.",
            );
        }
    }
}

/// A composer chip: a face, a hairline, a word. No fill that carries meaning on
/// its own, since the word is always printed beside the colour.
fn chip(ui: &mut egui::Ui, label: &str, tone: egui::Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(theme::mono(theme::FS_META)).color(tone))
            .fill(theme::FACE)
            .corner_radius(theme::R_XS),
    )
}
