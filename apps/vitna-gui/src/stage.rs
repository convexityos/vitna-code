//! The centre: what the window is looking at, stated once, then the composer.

use eframe::egui::{self, Color32, Rect, RichText, Vec2};

use crate::app::App;
use crate::icons;
use crate::link::Link;
use crate::theme;
use crate::workspace;

/// The centred column every surface in the main area shares.
pub(crate) const COLUMN: f32 = 800.0;

pub(crate) fn column(rect: Rect) -> Rect {
    let w = COLUMN.min(rect.width() - 64.0);
    Rect::from_center_size(rect.center(), Vec2::new(w, rect.height()))
}

impl App {
    pub(crate) fn stage(&mut self, ui: &mut egui::Ui, rect: Rect) {
        // As tall as the composer measured itself last frame; it grows with
        // the text and asks for a repaint when it does.
        let composer_h = self.composer_h;
        let body = Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - composer_h));
        let composer_rect = Rect::from_min_max(egui::pos2(rect.left(), body.bottom()), rect.max);

        let col = column(body);
        let mut ui2 = ui.new_child(egui::UiBuilder::new().max_rect(col));
        ui2.set_clip_rect(body);
        ui2.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

        // The start state sits in the upper third, the way Codex centres
        // "Let's build": enough space above it to feel like a place, not a form.
        ui2.add_space((body.height() * 0.22).max(32.0));

        ui2.vertical_centered(|ui| {
            ui.add(egui::Image::new(crate::brand::mark()).fit_to_exact_size(Vec2::splat(40.0)));
            ui.add_space(10.0);
            ui.label(RichText::new("Let's build").font(theme::display(28.0)).color(theme::INK));
        });

        ui2.add_space(22.0);
        self.facts_line(&mut ui2);

        if let Link::Absent { .. } = &self.link {
            ui2.add_space(14.0);
            ui2.vertical_centered(|ui| self.absent_line(ui));
        }

        self.composer(ui, composer_rect);
    }

    /// One quiet line of facts with icons: changes, last touched. The branch
    /// is a chip above the composer now, so it is not repeated here.
    fn facts_line(&mut self, ui: &mut egui::Ui) {
        let facts = self.facts();

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

        let f = theme::sans(13.0);
        let w = |ui: &egui::Ui, s: &str| {
            ui.painter().layout_no_wrap(s.to_string(), f.clone(), theme::MUTE).size().x
        };
        // Per fact: icon, two spacings and its text; 22px between facts.
        let total = 2.0 * 34.0 + w(ui, &changes) + w(ui, &touched) + 22.0;
        let pad = ((ui.available_width() - total) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
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
                    .font(theme::prose(12.5))
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
