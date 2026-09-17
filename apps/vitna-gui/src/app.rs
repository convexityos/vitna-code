//! The window.
//!
//! Framed the way Convexity frames its terminal: a fixed icon rail on a
//! near-black desk, and the working area as a rounded device floating off it.
//! That shell is what makes the product read as one instrument rather than a
//! page, and it is carried here in Vitna's palette rather than Convexity's warm
//! one, so the accent is periwinkle and the desk is blue-black.
//!
//! The archetype is CONSOLE: a main stream with a docked panel on the right,
//! which is the shape a coding session has. Both are laid out now, with honest
//! empty states, so the window does not need relaying out when a daemon exists.
//!
//! What this surface may claim is bounded by what it can reach. The daemon owns
//! sessions, runs, policy and model choice. This window owns the workspace facts
//! it reads off disk, and says "unknown" about the rest.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Stroke, Vec2};

use crate::link::{self, Link};
use crate::repo::{Loading, Probe, RepoFacts};
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
    Local,
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

/// The rail's destinations. Only Session is reachable today; the rest are the
/// shape of the product and are drawn disabled rather than omitted, so the rail
/// does not silently change size when the daemon arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Session,
    Changes,
    Receipts,
    Providers,
}

impl Dest {
    const ALL: [Dest; 4] = [Dest::Session, Dest::Changes, Dest::Receipts, Dest::Providers];

    fn caption(self) -> &'static str {
        match self {
            Dest::Session => "Session",
            Dest::Changes => "Changes",
            Dest::Receipts => "Receipts",
            Dest::Providers => "Models",
        }
    }

    /// Drawn from primitives rather than from a font. The first version used
    /// box-drawing and arrow marks, and the bundled face has no coverage for
    /// them, so every one rendered as a tofu box.
    fn draw_glyph(self, painter: &egui::Painter, c: egui::Pos2, tone: Color32) {
        let s = Stroke::new(1.4, tone);
        match self {
            // A prompt caret.
            Dest::Session => {
                painter.line_segment([c + Vec2::new(-4.0, -4.0), c + Vec2::new(1.0, 0.0)], s);
                painter.line_segment([c + Vec2::new(1.0, 0.0), c + Vec2::new(-4.0, 4.0)], s);
                painter.line_segment([c + Vec2::new(3.0, 4.5), c + Vec2::new(7.0, 4.5)], s);
            }
            // A plus over a minus: what a diff is.
            Dest::Changes => {
                painter.line_segment([c + Vec2::new(-6.0, -3.0), c + Vec2::new(0.0, -3.0)], s);
                painter.line_segment([c + Vec2::new(-3.0, -6.0), c + Vec2::new(-3.0, 0.0)], s);
                painter.line_segment([c + Vec2::new(1.0, 4.0), c + Vec2::new(7.0, 4.0)], s);
            }
            // A stamped record: a sheet with two ruled lines.
            Dest::Receipts => {
                painter.rect_stroke(
                    Rect::from_center_size(c, Vec2::new(11.0, 13.0)),
                    CornerRadius::same(2),
                    s,
                    egui::StrokeKind::Inside,
                );
                painter.line_segment([c + Vec2::new(-3.0, -2.0), c + Vec2::new(3.0, -2.0)], s);
                painter.line_segment([c + Vec2::new(-3.0, 2.0), c + Vec2::new(1.0, 2.0)], s);
            }
            // A facet: one choice among several.
            Dest::Providers => {
                let r = 6.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        c + Vec2::new(0.0, -r),
                        c + Vec2::new(r, 0.0),
                        c + Vec2::new(0.0, r),
                        c + Vec2::new(-r, 0.0),
                    ],
                    Color32::TRANSPARENT,
                    s,
                ));
            }
        }
    }
}

pub struct App {
    workspace: Workspace,
    repo: Probe,
    link: Link,
    draft: String,
    mode: Mode,
    placement: Placement,
    dest: Dest,
    mono_face: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, workspace: Workspace) -> Self {
        theme::install(&cc.egui_ctx);
        let repo = Probe::start(&workspace.path);
        Self {
            workspace,
            repo,
            link: link::probe(),
            draft: String::new(),
            mode: Mode::Build,
            placement: Placement::Worktree,
            dest: Dest::Session,
            mono_face: theme::mono_face_name(),
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = theme::DESK;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The repository read runs on a worker thread; ask for a frame while it
        // is still out so the answer appears without an input event.
        if matches!(self.repo.poll(), Loading::Reading) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
        }

        let full = ui.max_rect();
        let rail_rect = Rect::from_min_size(full.min, Vec2::new(theme::RAIL_W, full.height()));
        let device_rect = Rect::from_min_max(
            egui::pos2(full.min.x + theme::RAIL_W + 6.0, full.min.y + theme::FRAME),
            egui::pos2(full.max.x - theme::FRAME, full.max.y - theme::FRAME),
        );

        self.paint_device_frame(ui, device_rect);
        self.rail(ui, rail_rect);
        self.device(ui, device_rect);
    }
}

impl App {
    /// The desk-to-device step: a deep soft shadow, a hairline bezel and a
    /// single line of top light. This is the only shadow in the window.
    fn paint_device_frame(&self, ui: &egui::Ui, rect: Rect) {
        let radius = CornerRadius::same(theme::DEVICE_RADIUS as u8);
        let painter = ui.painter();

        let shadow = egui::epaint::Shadow {
            offset: [0, 26],
            blur: 60,
            spread: 0,
            color: Color32::from_black_alpha(190),
        };
        painter.add(shadow.as_shape(rect, radius));

        painter.rect_filled(rect, radius, theme::DEVICE);
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, theme::HAIR_2),
            egui::StrokeKind::Inside,
        );
        // One line of light along the top edge, the way a screen catches a room.
        painter.hline(
            (rect.left() + 18.0)..=(rect.right() - 18.0),
            rect.top() + 0.5,
            Stroke::new(1.0, Color32::from_white_alpha(14)),
        );
    }

    /// Fixed chrome: the product mark, then the destinations. The active item is
    /// periwinkle, which in Vitna's palette is "where you stand".
    fn rail(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::RAIL);
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        ui.set_clip_rect(rect);

        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            // The mark: a periwinkle glyph, the one place the accent is chrome.
            ui.label(
                RichText::new("\u{25c6}")
                    .font(theme::sans(17.0))
                    .color(theme::PERI),
            );
            ui.add_space(14.0);

            for dest in Dest::ALL {
                let reachable = dest == Dest::Session || self.link.is_open();
                let active = dest == self.dest;
                let (bg, fg) = match (active, reachable) {
                    (true, _) => (theme::FACE, theme::PERI_2),
                    (false, true) => (Color32::TRANSPARENT, theme::FAINT),
                    (false, false) => (Color32::TRANSPARENT, theme::FAINTER),
                };

                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(theme::RAIL_W - 12.0, 44.0),
                    if reachable {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );

                if response.hovered() && reachable && !active {
                    ui.painter().rect_filled(
                        Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.top() + 16.0),
                            Vec2::new(42.0, 31.0),
                        ),
                        CornerRadius::same(11),
                        Color32::from_white_alpha(12),
                    );
                }
                if active {
                    ui.painter().rect_filled(
                        Rect::from_center_size(
                            egui::pos2(rect.center().x, rect.top() + 16.0),
                            Vec2::new(42.0, 31.0),
                        ),
                        CornerRadius::same(11),
                        bg,
                    );
                }

                dest.draw_glyph(
                    ui.painter(),
                    egui::pos2(rect.center().x, rect.top() + 16.0),
                    fg,
                );
                ui.painter().text(
                    egui::pos2(rect.center().x, rect.top() + 36.0),
                    egui::Align2::CENTER_CENTER,
                    dest.caption(),
                    theme::sans(9.0),
                    fg,
                );

                if response.clicked() && reachable {
                    self.dest = dest;
                }
                if !reachable {
                    response.on_hover_text(
                        "The daemon is not running, and it is the daemon that has changes, receipts and providers.",
                    );
                }
            }
        });

        // The link state lives at the foot of the rail: one dot, and the word
        // beside it, which is this palette's rule for state.
        let (dot, word) = match &self.link {
            Link::Open { .. } => (theme::OK, "linked"),
            Link::Absent { .. } => (theme::RUST, "no daemon"),
        };
        let foot = Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - 40.0),
            egui::pos2(rect.right(), rect.bottom()),
        );
        ui.painter()
            .circle_filled(egui::pos2(foot.center().x, foot.top() + 6.0), 3.0, dot);
        ui.painter().text(
            egui::pos2(foot.center().x, foot.top() + 20.0),
            egui::Align2::CENTER_CENTER,
            word,
            theme::sans(8.5),
            theme::FAINT,
        );
    }
}

impl App {
    fn device(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        ui.set_clip_rect(rect);

        let header = Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), rect.top() + theme::HEADER_H),
        );
        self.header(&mut ui, header);

        let body = Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);

        // CONSOLE: a main stream, and a panel docked to its right.
        let panel_w = (body.width() * 0.34).clamp(260.0, 380.0);
        let stream = Rect::from_min_max(
            body.min,
            egui::pos2(body.right() - panel_w, body.bottom()),
        );
        let panel = Rect::from_min_max(egui::pos2(stream.right(), body.top()), body.max);

        ui.painter().vline(
            stream.right(),
            body.y_range(),
            Stroke::new(1.0, theme::HAIR_2),
        );

        self.stream(&mut ui, stream);
        self.changes_panel(&mut ui, panel);
    }

    /// Breadcrumb on the left, state on the right, a hairline underneath.
    fn header(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(
            rect,
            CornerRadius {
                nw: theme::DEVICE_RADIUS as u8,
                ne: theme::DEVICE_RADIUS as u8,
                sw: 0,
                se: 0,
            },
            theme::BAND,
        );
        ui.painter()
            .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::HAIR_2));

        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(Vec2::new(18.0, 12.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );

        ui.label(
            RichText::new(&self.workspace.name)
                .font(theme::sans(theme::FS_SMALL))
                .color(theme::INK)
                .strong(),
        );
        ui.label(
            RichText::new("/")
                .font(theme::sans(theme::FS_SMALL))
                .color(theme::FAINTER),
        );
        let head = match &self.workspace.head {
            Some(h) => h.label(),
            None => "no repository".to_string(),
        };
        ui.label(
            RichText::new(head)
                .font(theme::mono(theme::FS_META))
                .color(theme::MUTE),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (word, tone) = match self.mode {
                Mode::Build => ("build", theme::PERI_2),
                Mode::Plan => ("plan", theme::MUTE),
            };
            pill(ui, word, tone);
            pill(ui, self.placement.label(), theme::MUTE);
        });
    }
}

/// A chip: mono, small, tabular, a face and a hairline. Chips repeat, so they
/// are texture, and texture stays quiet.
fn pill(ui: &mut egui::Ui, text: &str, tone: Color32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        theme::mono(theme::FS_MARK),
        tone,
    );
    let size = Vec2::new(galley.size().x + 14.0, 19.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), theme::FACE);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(1.0, theme::HAIR_2),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
        galley,
        tone,
    );
}

impl App {
    fn stream(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let composer_h = 104.0;
        let body = Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), rect.bottom() - composer_h),
        );
        let composer_rect =
            Rect::from_min_max(egui::pos2(rect.left(), body.bottom()), rect.max);

        let mut body_ui = ui.new_child(
            egui::UiBuilder::new().max_rect(body.shrink2(Vec2::new(24.0, 18.0))),
        );
        body_ui.set_clip_rect(body);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| {
                self.instrument(ui);
                ui.add_space(20.0);
                if let Link::Absent { tried, detail } = self.link.clone() {
                    self.absent_plate(ui, &tried, &detail);
                }
            });

        self.composer(ui, composer_rect);
    }

    /// The window's one designed moment: the repository, read as an instrument.
    ///
    /// Every figure here is measured, and the ones that cannot be measured print
    /// a dash. The source line under it says where they came from, because a
    /// number without provenance is decoration.
    fn instrument(&mut self, ui: &mut egui::Ui) {
        let facts: Option<RepoFacts> = match self.repo.poll() {
            Loading::Done(f) => Some((**f).clone()),
            Loading::Reading => None,
        };

        theme::section_header(ui, "Workspace", Some(&self.workspace.name));

        // One row, fixed height, explicit column widths. The first version let
        // the stats flow, so the eyebrow wrapped in half and the last column
        // fell off the right edge entirely.
        const STATS: f32 = 326.0;
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 50.0),
            Layout::left_to_right(Align::Min),
            |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

                let head_w = (ui.available_width() - STATS).max(180.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(head_w, 50.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.label(theme::eyebrow(ui, "head"));
                        ui.add_space(1.0);
                        let (head, tone) = match &self.workspace.head {
                            Some(workspace::Head::Branch(b)) => (b.clone(), theme::INK),
                            Some(h) => (h.label(), theme::RUST),
                            None => ("not a repository".to_string(), theme::FAINT),
                        };
                        ui.add(
                            egui::Label::new(
                                RichText::new(head).font(theme::mono(22.0)).color(tone),
                            )
                            .truncate(),
                        );
                    },
                );

                let num = |v: Option<u32>| v.map(|n| n.to_string());
                let cnt = |v: Option<usize>| v.map(|n| n.to_string());
                let (ahead, behind, staged, modified, untracked) = match &facts {
                    Some(f) => (
                        num(f.ahead),
                        num(f.behind),
                        cnt(f.staged),
                        cnt(f.modified),
                        cnt(f.untracked),
                    ),
                    None => (None, None, None, None, None),
                };

                // Periwinkle for work of yours that is ahead, rust for work you
                // are behind by, and plain ink for a count that is merely a
                // count. A zero is never coloured.
                let live = |v: &Option<String>, tone: Color32| {
                    if v.as_deref().is_some_and(|s| s != "0") {
                        tone
                    } else {
                        theme::MUTE
                    }
                };

                for (label, value, tone, w) in [
                    ("ahead", &ahead, theme::PERI_2, 54.0),
                    ("behind", &behind, theme::RUST, 58.0),
                    ("staged", &staged, theme::INK_2, 58.0),
                    ("modified", &modified, theme::INK_2, 72.0),
                    ("untracked", &untracked, theme::INK_2, 84.0),
                ] {
                    theme::stat(
                        ui,
                        label,
                        value.as_deref().unwrap_or(theme::UNKNOWN),
                        live(value, tone),
                        18.0,
                        w,
                    );
                }
            },
        );

        ui.add_space(6.0);
        let upstream = facts
            .as_ref()
            .and_then(|f| f.upstream.clone())
            .unwrap_or_else(|| "no upstream".to_string());
        let touched = match self.workspace.last_modified.and_then(workspace::ago) {
            Some(when) if self.workspace.scan_was_capped => format!(
                "touched {when} (newest of {} entries)",
                self.workspace.entries_scanned
            ),
            Some(when) => format!("touched {when}"),
            None => "touch time unknown".to_string(),
        };
        ui.label(
            RichText::new(format!(
                "{}  \u{00b7}  tracking {}  \u{00b7}  {}",
                self.workspace.path.to_string_lossy(),
                upstream,
                touched
            ))
            .font(theme::mono(theme::FS_MARK))
            .color(theme::FAINTER),
        );

        if let Some(trouble) = facts.as_ref().and_then(|f| f.trouble.clone()) {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("git reported: {trouble}"))
                    .font(theme::mono(theme::FS_MARK))
                    .color(theme::RUST),
            );
        }

        ui.add_space(18.0);
        self.commits(ui, facts.as_ref());
    }
}

impl App {
    /// The commit table. Hairline rows, mono sha and age, sans subject, and the
    /// table stays a table: nothing here is a card.
    fn commits(&mut self, ui: &mut egui::Ui, facts: Option<&RepoFacts>) {
        let count = facts.map(|f| f.commits.len()).unwrap_or(0);
        theme::section_header(
            ui,
            "Recent history",
            Some(&match facts {
                None => "reading".to_string(),
                Some(_) if count == 1 => "1 commit".to_string(),
                Some(_) => format!("{count} commits"),
            }),
        );

        let Some(facts) = facts else {
            ui.label(
                RichText::new("Reading the repository.")
                    .font(theme::sans(theme::FS_META))
                    .color(theme::FAINT),
            );
            return;
        };

        if facts.commits.is_empty() {
            ui.label(
                RichText::new("git returned no commits for this directory.")
                    .font(theme::sans(theme::FS_META))
                    .color(theme::FAINT),
            );
            return;
        }

        for (i, commit) in facts.commits.iter().enumerate() {
            if i > 0 {
                let (rule, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().hline(
                    rule.x_range(),
                    rule.center().y,
                    Stroke::new(1.0, theme::HAIR_3),
                );
            }
            ui.horizontal(|ui| {
                ui.add_space(1.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(72.0, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(&commit.sha)
                                .font(theme::mono(theme::FS_META))
                                .color(theme::PERI_2),
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(96.0, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(age(commit.unix_seconds))
                                .font(theme::mono(theme::FS_META))
                                .color(theme::FAINTER),
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(110.0, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(&commit.author)
                                .font(theme::sans(theme::FS_META))
                                .color(theme::FAINT),
                        );
                    },
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&commit.subject)
                            .font(theme::sans(theme::FS_META))
                            .color(theme::INK_2),
                    )
                    .truncate(),
                );
            });
            ui.add_space(5.0);
        }
    }

    /// The daemon's absence, on the plate: a step BELOW the working ground,
    /// because it is a record of a state rather than furniture.
    fn absent_plate(&mut self, ui: &mut egui::Ui, tried: &[String], detail: &str) {
        egui::Frame::default()
            .fill(theme::PLATE)
            .stroke(Stroke::new(1.0, theme::HAIR_2))
            .corner_radius(CornerRadius::same(theme::R_SM as u8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_max_width(640.0);
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, theme::RUST);
                    ui.label(
                        RichText::new("vitna-coded is not running")
                            .font(theme::sans(theme::FS_SMALL))
                            .color(theme::INK_2),
                    );
                });
                ui.add_space(7.0);
                ui.label(
                    RichText::new(
                        "The daemon is reached over an owner-only pipe or socket and never over a \
                         network port, so nothing here can be served from a browser.",
                    )
                    .font(theme::sans(theme::FS_META))
                    .color(theme::FAINT),
                );
                ui.add_space(11.0);
                ui.label(theme::eyebrow(ui, "endpoints tried"));
                ui.add_space(3.0);
                if tried.is_empty() {
                    ui.label(
                        RichText::new(detail)
                            .font(theme::mono(theme::FS_MARK))
                            .color(theme::FAINT),
                    );
                } else {
                    for endpoint in tried {
                        ui.label(
                            RichText::new(endpoint)
                                .font(theme::mono(theme::FS_MARK))
                                .color(theme::MUTE),
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(detail)
                            .font(theme::mono(theme::FS_MARK))
                            .color(theme::FAINTER),
                    );
                }
                ui.add_space(11.0);
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
                    self.link = link::probe();
                }
            });
    }
}

/// A short age for a commit timestamp, in the mono voice.
fn age(unix_seconds: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = now - unix_seconds;
    if secs < 0 {
        return theme::UNKNOWN.to_string();
    }
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

impl App {
    /// The docked panel. It is laid out now and empty now, because changes come
    /// from a run and there has been none.
    fn changes_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let mut ui = ui.new_child(
            egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(18.0, 18.0))),
        );
        ui.set_clip_rect(rect);

        theme::section_header(&mut ui, "Changes", Some("0 files"));
        ui.label(
            RichText::new(
                "A run's changes are listed here with a preimage and postimage hash per file, \
                 which is what a receipt binds to.",
            )
            .font(theme::sans(theme::FS_META))
            .color(theme::FAINT),
        );

        ui.add_space(22.0);
        theme::section_header(&mut ui, "Authority", None);

        let rows: [(&str, String, Color32); 4] = [
            ("isolation", self.placement.label().to_string(), theme::MUTE),
            ("mode", self.mode.label().to_string(), theme::MUTE),
            (
                "model",
                "chosen by policy".to_string(),
                theme::FAINT,
            ),
            (
                "daemon",
                match &self.link {
                    Link::Open { endpoint } => endpoint.clone(),
                    Link::Absent { .. } => "not running".to_string(),
                },
                match &self.link {
                    Link::Open { .. } => theme::OK,
                    Link::Absent { .. } => theme::RUST,
                },
            ),
        ];

        for (label, value, tone) in rows {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(78.0, 18.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(theme::eyebrow(ui, label));
                    },
                );
                ui.label(
                    RichText::new(value)
                        .font(theme::mono(theme::FS_MARK))
                        .color(tone),
                );
            });
            ui.add_space(6.0);
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let mut ui = ui.new_child(
            egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(24.0, 14.0))),
        );
        ui.set_clip_rect(rect);

        egui::Frame::default()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::HAIR_2))
            .corner_radius(CornerRadius::same(theme::R_SM as u8))
            .inner_margin(egui::Margin::symmetric(14, 11))
            .show(&mut ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(1)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::default())
                        .font(theme::mono(theme::FS_SMALL))
                        .hint_text(
                            RichText::new("Ask Vitna Code to change something.")
                                .font(theme::mono(theme::FS_SMALL))
                                .color(theme::FAINTER),
                        ),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if chip(ui, self.mode.label(), theme::PERI_2).clicked() {
                        self.mode = match self.mode {
                            Mode::Build => Mode::Plan,
                            Mode::Plan => Mode::Build,
                        };
                    }
                    // Rust marks the provisional choice: running in the
                    // workspace itself is the one that leaves changes behind.
                    let tone = match self.placement {
                        Placement::Local => theme::RUST,
                        Placement::Worktree => theme::MUTE,
                    };
                    let placement = chip(ui, self.placement.label(), tone);
                    if placement.clicked() {
                        self.placement = match self.placement {
                            Placement::Local => Placement::Worktree,
                            Placement::Worktree => Placement::Local,
                        };
                    }
                    placement.on_hover_text(match self.placement {
                        Placement::Local => {
                            "Runs in this workspace. Changes land in the files you have open."
                        }
                        Placement::Worktree => {
                            "Runs in a disposable clone. The workspace is untouched."
                        }
                    });

                    ui.label(
                        RichText::new("model chosen by policy")
                            .font(theme::mono(theme::FS_MARK))
                            .color(theme::FAINTER),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ready = self.link.is_open() && !self.draft.trim().is_empty();
                        let send = ui.add_enabled(
                            ready,
                            egui::Button::new(
                                RichText::new("Send")
                                    .font(theme::sans(theme::FS_META))
                                    .color(if ready { theme::INK } else { theme::FAINTER }),
                            )
                            .fill(if ready { theme::CONTROL } else { theme::FACE }),
                        );
                        if !self.link.is_open() {
                            send.on_hover_text(
                                "Nothing to send to: vitna-coded is not running. A turn is submitted to the daemon, never from this window.",
                            );
                        }
                        ui.label(
                            RichText::new(format!("drawing with {}", self.mono_face))
                                .font(theme::mono(theme::FS_MARK))
                                .color(theme::FAINTER),
                        );
                    });
                });
            });
    }
}

/// A composer chip. Chips repeat, so they are texture, and texture stays quiet.
fn chip(ui: &mut egui::Ui, label: &str, tone: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .font(theme::mono(theme::FS_META))
                .color(tone),
        )
        .fill(theme::FACE)
        .corner_radius(CornerRadius::same(theme::R_XS as u8)),
    )
}
