//! The start screen: the folder's runs, headed by one sentence about them.
//!
//! With runs to list, the page is the ledger, the way the owner's new design
//! gives a feed the whole centre: a headline that says what the last run did
//! (never a greeting, which spends the best pixels on the screen saying
//! nothing a reader came for), one quiet line under it saying where the
//! ledger was read from and who checked it, and then the rows. With nothing
//! to list, the mark and "Let's build" hold the upper third, the way Codex
//! centres its start, and the terminal client says the same words there.

use eframe::egui::{self, Color32, Rect, RichText, Vec2};

use crate::app::App;
use crate::icons;
use crate::parts;
use crate::theme;

/// The centred column every surface in the panel shares.
pub(crate) const COLUMN: f32 = 800.0;

/// The air between what scrolls and the composer, so a page scrolled under
/// the composer ends a clear line above it rather than touching its words.
pub(crate) const CLEAR: f32 = 14.0;

pub(crate) fn column(rect: Rect) -> Rect {
    let w = COLUMN.min(rect.width() - 64.0);
    Rect::from_center_size(rect.center(), Vec2::new(w, rect.height()))
}

impl App {
    pub(crate) fn stage(&mut self, ui: &mut egui::Ui, rect: Rect) {
        // As tall as the composer measured itself last frame; it grows with
        // the text and asks for a repaint when it does.
        let composer_h = self.composer_h;
        let body = Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - composer_h - CLEAR));
        let composer_rect = Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - composer_h), rect.max);

        let col = column(body);
        let mut ui2 = ui.new_child(egui::UiBuilder::new().max_rect(col));
        ui2.set_clip_rect(body);
        ui2.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui2.spacing_mut().item_spacing.y = 4.0;

        let listing = self.ledger_has_entries();
        if listing {
            ui2.add_space(28.0);
            self.masthead(&mut ui2);
            self.turn_state(&mut ui2);
            ui2.add_space(18.0);
            self.run_list(&mut ui2);
        } else {
            ui2.add_space((body.height() * 0.22).max(24.0));
            ui2.vertical_centered(|ui| {
                ui.add(egui::Image::new(crate::brand::mark()).fit_to_exact_size(Vec2::splat(40.0)));
                ui.add_space(6.0);
                ui.label(RichText::new("Let\u{2019}s build").font(theme::display(theme::FS_HERO)).color(theme::INK));
                ui.add_space(6.0);
                if self.runs_read() {
                    ui.label(
                        RichText::new(format!(
                            "Nothing has run in {} yet. A run's receipt lands in .vitna/receipts, and is listed here.",
                            self.workspace.name
                        ))
                        .font(theme::prose(theme::FS_UI))
                        .color(theme::FAINT),
                    );
                }
            });
            self.turn_state(&mut ui2);
        }

        self.composer(ui, composer_rect);
    }

    /// The headline and the provenance under it.
    fn masthead(&mut self, ui: &mut egui::Ui) {
        let Some(list) = self.run_rows() else { return };
        let (key, _) = crate::run_view::device_key(&self.link);
        if let Some(h) = crate::run_list::headline(&list.rows) {
            ui.add(
                egui::Label::new(RichText::new(h).font(theme::display(theme::FS_HEAD)).color(theme::INK))
                    .wrap(),
            );
        }
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), egui::Sense::hover());
        let provenance = crate::run_list::provenance(&list.rows, key.as_deref());
        let line = parts::facts(ui, egui::pos2(r.left(), r.center().y), &provenance, r.width());
        ui.interact(line, ui.id().with("provenance"), egui::Sense::hover()).on_hover_text(
            "Read straight off disk and checked here, rather than by asking the daemon whether its own work was sound. The history survives the daemon being stopped, and the check does not depend on the thing being checked.",
        );
    }
}

fn fact(ui: &mut egui::Ui, icon: fn(&egui::Painter, egui::Pos2, Color32), text: &str) {
    icons::inline(ui, 16.0, theme::FAINTER, icon);
    ui.add(egui::Label::new(RichText::new(text).font(theme::sans(theme::FS_META)).color(theme::FAINT)).truncate());
}

impl App {
    /// What the daemon is doing, or what it last said.
    ///
    /// Three states, and none of them is allowed to stand in for another: a
    /// turn in flight, a turn that failed with the daemon's own words, and a
    /// finished turn summarised from the receipt it wrote. A window with
    /// nothing to report draws nothing.
    fn turn_state(&mut self, ui: &mut egui::Ui) {
        if self.turn_running {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let (d, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(d.center(), 3.0, theme::PERI);
                ui.label(
                    RichText::new("Running. The daemon has the turn.")
                        .font(theme::prose(theme::FS_UI))
                        .color(theme::FAINT),
                );
            });
            return;
        }

        if let Some(message) = self.last_error.clone() {
            ui.add_space(14.0);
            ui.horizontal_top(|ui| {
                let (d, _) = ui.allocate_exact_size(Vec2::new(8.0, 18.0), egui::Sense::hover());
                ui.painter().circle_filled(d.center(), 3.0, theme::RUST);
                // The daemon's own sentence, not a rewording of it. A refusal
                // that names the missing key is more use than a tidier one
                // that does not.
                ui.add(
                    egui::Label::new(RichText::new(message).font(theme::prose(theme::FS_UI)).color(theme::FAINT))
                        .wrap(),
                );
            });
            return;
        }

        let Some(result) = self.last_result.clone() else {
            return;
        };

        // The last turn's answer, set on the panel rather than boxed: the
        // model's words, then the facts off the receipt it wrote.
        ui.add_space(18.0);
        if !result.text.is_empty() {
            ui.add(
                egui::Label::new(RichText::new(&result.text).font(theme::prose(theme::FS_TITLE)).color(theme::INK))
                    .wrap(),
            );
            ui.add_space(8.0);
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // What ran, off the receipt. The model line is the point of the
            // whole product, so it is stated plainly.
            fact(ui, icons::sparkle, &format!("{} ({})", result.model_sku, result.provider));
            ui.add_space(10.0);
            fact(ui, icons::done, &result.completion_state.replace('_', " "));
            ui.add_space(10.0);
            let tokens = match (result.prompt_tokens, result.completion_tokens) {
                (Some(p), Some(c)) => format!("{p} in, {c} out"),
                // The provider reported nothing. Zero is a number somebody
                // would believe.
                _ => "-".to_string(),
            };
            fact(ui, icons::clock, &tokens);
        });
    }
}

impl App {
    /// Whether the receipts directory gave the list anything to show,
    /// including a file that would not parse or a directory that would not
    /// open, both of which are rows or lines rather than silence.
    fn ledger_has_entries(&mut self) -> bool {
        matches!(
            self.runs.poll(),
            crate::runs::Loading::Done(l)
                if !l.runs.is_empty() || !l.unreadable.is_empty() || l.trouble.is_some()
        )
    }

    /// Whether the receipts directory has been read at all, so an empty start
    /// does not claim "nothing has run" before it has looked.
    fn runs_read(&mut self) -> bool {
        matches!(self.runs.poll(), crate::runs::Loading::Done(_))
    }
}
