//! The composer: the thing you reach for.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Stroke, Vec2};

use crate::app::{App, Mode, Placement};
use crate::catalog;
use crate::icons;
use crate::stage::column;
use crate::theme;

impl App {
    pub(crate) fn composer(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let col = column(rect);
        let inner = Rect::from_min_max(col.min, egui::pos2(col.right(), rect.bottom() - 24.0));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.set_clip_rect(rect);

        let ready = self.link.is_open() && !self.draft.trim().is_empty();

        egui::Frame::default()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::HAIR))
            .corner_radius(CornerRadius::same(20))
            .inner_margin(egui::Margin { left: 18, right: 14, top: 14, bottom: 12 })
            .show(&mut ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::default())
                        // What you type is a paragraph, and so is the prompt
                        // that stands in for it; both read in the prose face.
                        .font(theme::prose(15.0))
                        .text_color(theme::INK)
                        .hint_text(
                            RichText::new("Ask anything")
                                .font(theme::prose(15.0))
                                .color(theme::FAINTER),
                        ),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    // Attach, on the left, the way all three references place it.
                    let (r, attach) =
                        ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
                    if attach.hovered() {
                        ui.painter().circle_filled(r.center(), 15.0, theme::FACE);
                    }
                    icons::plus(ui.painter(), r.center(), theme::MUTE);
                    attach.on_hover_text("Name a file with @, or attach one. Both need the daemon.");

                    ui.add_space(4.0);
                    self.mode_chip(ui);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (r, send) =
                            ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::click());
                        ui.painter().circle_filled(
                            r.center(),
                            16.0,
                            if ready { theme::PERI } else { theme::FACE },
                        );
                        icons::arrow_up(
                            ui.painter(),
                            r.center(),
                            if ready { theme::CANVAS } else { theme::FAINTER },
                        );
                        if !self.link.is_open() {
                            send.on_hover_text("Nothing to send to: the daemon is not running.");
                        }
                    });
                });
            });

        ui.add_space(10.0);

        // Under the field: where it runs, which model, which branch.
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            for p in Placement::ALL {
                let active = self.placement == p;
                let label = ui.add(
                    egui::Button::new(
                        RichText::new(p.label())
                            .font(theme::sans(13.0))
                            .color(if active { theme::INK } else { theme::FAINTER }),
                    )
                    .fill(Color32::TRANSPARENT)
                    .frame(false),
                );
                if label.clicked() {
                    self.placement = p;
                }
                if active {
                    ui.painter().hline(
                        label.rect.x_range().shrink(4.0),
                        label.rect.bottom() + 1.0,
                        Stroke::new(1.5, theme::PERI),
                    );
                }
                ui.add_space(6.0);
            }

            ui.add_space(10.0);
            self.model_picker(ui);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(head) = &self.workspace.head {
                    ui.label(
                        RichText::new(head.label())
                            .font(theme::mono(12.0))
                            .color(theme::FAINT),
                    );
                    icons::inline(ui, 16.0, theme::FAINTER, icons::branch);
                }
            });
        });
    }

    fn mode_chip(&mut self, ui: &mut egui::Ui) {
        let text = self.mode.label();
        let galley = ui
            .painter()
            .layout_no_wrap(text.to_string(), theme::sans(12.5), theme::INK_2);
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(galley.size().x + 34.0, 26.0),
            egui::Sense::click(),
        );
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(13),
            if response.hovered() { theme::FACE_2 } else { theme::FACE },
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::INK_2,
        );
        icons::chevron_down(ui.painter(), egui::pos2(rect.right() - 12.0, rect.center().y), theme::FAINT);
        if response.clicked() {
            self.mode = match self.mode {
                Mode::Build => Mode::Plan,
                Mode::Plan => Mode::Build,
            };
        }
        response.on_hover_text("Build edits files. Plan proposes and stops.");
    }

    /// A real picker over the pinned catalog. It sets a preference the daemon
    /// is handed with the turn; the receipt says what ran.
    fn model_picker(&mut self, ui: &mut egui::Ui) {
        let selected = self
            .model
            .and_then(|i| self.choices.get(i))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "No model".to_string());

        let sku_hint = self
            .model
            .and_then(|i| self.choices.get(i))
            .map(|c| format!("Preference sent with the turn: {} via {}. The receipt says what ran.", c.sku, c.provider_id))
            .unwrap_or_else(|| "No tool-calling model in the catalog.".to_string());

        let combo = egui::ComboBox::from_id_salt("model")
            .selected_text(RichText::new(selected).font(theme::sans(13.0)).color(theme::INK_2))
            .width(210.0)
            .show_ui(ui, |ui| {
                let mut last_provider = String::new();
                for (i, c) in self.choices.iter().enumerate() {
                    if c.provider_name != last_provider {
                        if !last_provider.is_empty() {
                            ui.add_space(4.0);
                        }
                        ui.label(theme::eyebrow(ui, &c.provider_name));
                        last_provider = c.provider_name.clone();
                    }
                    let mut text = c.name.clone();
                    if let Some(ctx) = catalog::context_label(c.context_tokens) {
                        text.push_str(&format!("   {ctx}"));
                    }
                    if ui
                        .selectable_label(self.model == Some(i), RichText::new(text).font(theme::sans(12.5)))
                        .clicked()
                    {
                        self.model = Some(i);
                    }
                }
            });
        combo.response.on_hover_text(sku_hint);
    }
}
