//! The sidebar: work, listed by name.

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Stroke, Vec2};

use crate::app::App;
use crate::icons;
use crate::link::Link;
use crate::theme;

impl App {
    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, theme::RAIL);
        ui.painter()
            .vline(rect.right(), rect.y_range(), Stroke::new(1.0, theme::HAIR));

        let mut ui = ui.new_child(
            // Below the menu strip, which app.rs draws over the top-left corner.
            egui::UiBuilder::new().max_rect(Rect::from_min_max(
                egui::pos2(rect.left() + 12.0, rect.top() + crate::menu::STRIP_H),
                egui::pos2(rect.right() - 12.0, rect.bottom() - 12.0),
            )),
        );
        ui.set_clip_rect(rect);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);

        // Mark and name.
        ui.horizontal(|ui| {
            // The lockup: symbol, then the wordmark at the height the site chrome
            // uses, then the product word in the display cut. The wordmark is
            // white ink on transparent, so it takes the ink token as a tint.
            ui.add(egui::Image::new(crate::brand::mark()).fit_to_exact_size(Vec2::splat(18.0)));
            ui.add(
                egui::Image::new(crate::brand::wordmark())
                    .fit_to_exact_size(Vec2::new(20.0 * 376.0 / 104.0, 20.0))
                    .tint(theme::INK),
            );
            ui.label(
                RichText::new("Code")
                    .font(theme::display(14.5))
                    .color(theme::INK),
            );
        });
        ui.add_space(12.0);

        let linked = self.link.is_open();
        let new_session =
            ui.allocate_response(Vec2::new(ui.available_width(), 30.0), egui::Sense::click());
        let r = new_session.rect;
        ui.painter().rect_filled(
            r,
            CornerRadius::same(8),
            if linked && new_session.hovered() { theme::FACE_2 } else { theme::FACE },
        );
        ui.painter().rect_stroke(
            r,
            CornerRadius::same(8),
            Stroke::new(1.0, theme::HAIR_2),
            egui::StrokeKind::Inside,
        );
        let tone = if linked { theme::INK } else { theme::MUTE };
        icons::plus(ui.painter(), egui::pos2(r.left() + 14.0, r.center().y), tone);
        ui.painter().text(
            egui::pos2(r.left() + 30.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            "New session",
            theme::sans(13.5),
            tone,
        );
        if linked {
            if new_session.clicked() {
                // The daemon owns sessions. This asks for one against the
                // folder the window is looking at, and the reply arrives as an
                // event rather than a return value.
                self.worker.send(crate::daemon::Command::CreateSession(
                    self.workspace.path.clone(),
                ));
            }
        } else {
            new_session
                .on_hover_text("Sessions are opened by the daemon, and it is not running.");
        }

        ui.add_space(4.0);
        let (ready, total) = self.providers_ready();
        row(
            &mut ui,
            "Model providers",
            Some(&format!("{ready} of {total} ready")),
            true,
            false,
        );

        ui.add_space(16.0);
        group(&mut ui, "Sessions");

        // The workspace is the group header, the way every reference groups
        // threads under the folder they belong to.
        ui.horizontal(|ui| {
            icons::inline(ui, 16.0, theme::FAINT, icons::folder);
            ui.label(
                RichText::new(&self.workspace.name)
                    .font(theme::sans(13.0))
                    .color(theme::INK_2),
            );
        });
        ui.add_space(2.0);
        if self.sessions.is_empty() {
            // Two different nothings. A window that has not asked must not
            // report an empty list as a finding.
            let line = match &self.link {
                Link::Open { .. } => "No sessions yet",
                Link::Probing => "Checking",
                Link::Absent { .. } => "Not connected",
            };
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.label(
                    RichText::new(line)
                        .font(theme::sans(12.5))
                        .color(theme::FAINTER),
                );
            });
        } else {
            let active = self.active_session.clone();
            let mut picked: Option<String> = None;
            for session in &self.sessions {
                let is_active = active.as_deref() == Some(session.session_id.as_str());
                let row = ui.allocate_response(
                    Vec2::new(ui.available_width(), 28.0),
                    egui::Sense::click(),
                );
                let r = row.rect;
                if is_active || row.hovered() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(7),
                        if is_active { theme::FACE } else { theme::FACE_2 },
                    );
                }
                ui.painter().text(
                    egui::pos2(r.left() + 22.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    &session.session_id,
                    theme::sans(12.5),
                    if is_active { theme::INK_2 } else { theme::MUTE },
                );
                if row.clicked() {
                    picked = Some(session.session_id.clone());
                }
            }
            if let Some(id) = picked {
                self.active_session = Some(id);
            }
        }

        // Foot: link state.
        let (dot, word) = match &self.link {
            Link::Open { .. } => (theme::OK, "Daemon connected"),
            Link::Absent { .. } => (theme::RUST, "Daemon not running"),
            Link::Probing => (theme::FAINT, "Checking for the daemon"),
        };
        let foot = egui::pos2(rect.left() + 18.0, rect.bottom() - 20.0);
        ui.painter().circle_filled(foot, 3.0, dot);
        ui.painter().text(
            foot + Vec2::new(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            word,
            theme::sans(12.0),
            theme::FAINT,
        );
        ui.painter().text(
            egui::pos2(rect.right() - 14.0, rect.bottom() - 20.0),
            egui::Align2::RIGHT_CENTER,
            env!("CARGO_PKG_VERSION"),
            theme::mono(11.0),
            theme::FAINTER,
        );
    }
}

/// A sidebar row: a label, an optional trailing meta, hover ground.
fn row(ui: &mut egui::Ui, label: &str, meta: Option<&str>, enabled: bool, active: bool) -> egui::Response {
    let response = ui.allocate_response(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
    let r = response.rect;
    if active || (enabled && response.hovered()) {
        ui.painter().rect_filled(
            r,
            CornerRadius::same(7),
            if active { theme::FACE } else { Color32::from_white_alpha(9) },
        );
    }
    ui.painter().text(
        egui::pos2(r.left() + 10.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        theme::sans(13.0),
        if enabled { theme::INK_2 } else { theme::FAINTER },
    );
    if let Some(m) = meta {
        ui.painter().text(
            egui::pos2(r.right() - 10.0, r.center().y),
            egui::Align2::RIGHT_CENTER,
            m,
            theme::sans(11.5),
            theme::FAINTER,
        );
    }
    response
}

fn group(ui: &mut egui::Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(theme::eyebrow(ui, title));
    });
    ui.add_space(4.0);
}
