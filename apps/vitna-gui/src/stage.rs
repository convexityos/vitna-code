//! The centre: what the window is looking at, stated once, then the composer.

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Vec2};

use crate::app::App;
use crate::icons;
use crate::link::Link;
use crate::theme;
use crate::workspace::{self, Head};

/// The centred column every surface in the main area shares.
pub(crate) const COLUMN: f32 = 720.0;

pub(crate) fn column(rect: Rect) -> Rect {
    let w = COLUMN.min(rect.width() - 64.0);
    Rect::from_center_size(rect.center(), Vec2::new(w, rect.height()))
}

impl App {
    pub(crate) fn stage(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let composer_h = 150.0;
        let body = Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - composer_h));
        let composer_rect = Rect::from_min_max(egui::pos2(rect.left(), body.bottom()), rect.max);

        let col = column(body);
        let mut ui2 = ui.new_child(egui::UiBuilder::new().max_rect(col));
        ui2.set_clip_rect(body);
        ui2.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

        // The start state sits in the upper third, the way Codex centres
        // "Let's build": enough space above it to feel like a place, not a form.
        ui2.add_space((body.height() * 0.24).max(40.0));

        ui2.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
            icons::mark(ui.painter(), r.center(), 15.0, theme::PERI);
            ui.add_space(14.0);
            ui.label(RichText::new("Let's build").font(theme::sans(28.0)).color(theme::INK));
            ui.add_space(14.0);
            self.workspace_pill(ui);
        });

        ui2.add_space(34.0);
        self.facts_line(&mut ui2);

        if let Link::Absent { .. } = &self.link {
            ui2.add_space(22.0);
            ui2.vertical_centered(|ui| self.absent_line(ui));
        }

        self.composer(ui, composer_rect);
    }

    /// The folder this window is open on, as a pill with a chevron. The
    /// references all put the workspace here, under the heading.
    fn workspace_pill(&mut self, ui: &mut egui::Ui) {
        let text = self.workspace.name.clone();
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), theme::sans(13.0), theme::INK_2);
        let size = Vec2::new(galley.size().x + 54.0, 30.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(15),
            if response.hovered() { theme::FACE_2 } else { theme::FACE },
        );
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(15),
            egui::Stroke::new(1.0, theme::HAIR_2),
            egui::StrokeKind::Inside,
        );
        icons::folder(ui.painter(), egui::pos2(rect.left() + 17.0, rect.center().y), theme::FAINT);
        ui.painter().galley(
            egui::pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::INK_2,
        );
        icons::chevron_down(ui.painter(), egui::pos2(rect.right() - 15.0, rect.center().y), theme::FAINT);
        response.on_hover_text(
            "Opening another folder is the daemon's to do, and it is not running.",
        );
    }

    /// One quiet line of facts with icons: branch, changes, last touched.
    fn facts_line(&mut self, ui: &mut egui::Ui) {
        let facts = self.facts();

        let (head, head_tone) = match &self.workspace.head {
            Some(Head::Branch(b)) => (b.clone(), theme::MUTE),
            Some(h) => (h.label(), theme::RUST),
            None => ("no repository".to_string(), theme::FAINT),
        };

        let changes = match &facts {
            None => "reading changes".to_string(),
            Some(f) => {
                let s = f.staged.unwrap_or(0);
                let m = f.modified.unwrap_or(0);
                let u = f.untracked.unwrap_or(0);
                match s + m + u {
                    0 => "clean".to_string(),
                    _ => {
                        let mut parts = Vec::new();
                        if s > 0 { parts.push(format!("{s} staged")); }
                        if m > 0 { parts.push(format!("{m} modified")); }
                        if u > 0 { parts.push(format!("{u} untracked")); }
                        parts.join(", ")
                    }
                }
            }
        };

        let touched = match self.workspace.last_modified.and_then(workspace::ago) {
            Some(when) if self.workspace.scan_was_capped => {
                format!("{when} (first {} entries)", self.workspace.entries_scanned)
            }
            Some(when) => when,
            None => "unknown".to_string(),
        };

        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.add_space((ui.available_width() - 560.0).max(0.0) / 2.0);
            fact(ui, icons::branch, &head, head_tone);
            ui.add_space(22.0);
            fact(ui, icons::changes, &changes, theme::MUTE);
            ui.add_space(22.0);
            fact(ui, icons::clock, &touched, theme::MUTE);
        });

        if let Some(t) = facts.as_ref().and_then(|f| f.trouble.clone()) {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("git: {t}")).font(theme::mono(11.5)).color(theme::RUST));
            });
        }
    }

    fn absent_line(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (d, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
            ui.painter().circle_filled(d.center(), 3.0, theme::RUST);
            ui.label(
                RichText::new("The daemon is not running, so there is nothing to send to yet.")
                    .font(theme::sans(12.5))
                    .color(theme::FAINT),
            );
            let again = ui.add(
                egui::Button::new(RichText::new("Check again").font(theme::sans(12.5)).color(theme::PERI_2))
                    .fill(Color32::TRANSPARENT)
                    .frame(false),
            );
            if again.clicked() {
                self.reprobe();
            }
        });
    }
}

fn fact(ui: &mut egui::Ui, icon: fn(&egui::Painter, egui::Pos2, Color32), text: &str, tone: Color32) {
    icons::inline(ui, 16.0, theme::FAINT, icon);
    ui.add_space(2.0);
    ui.add(egui::Label::new(RichText::new(text).font(theme::sans(13.0)).color(tone)).truncate());
}
