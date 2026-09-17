//! The composer: the thing you reach for.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Stroke, Vec2};

use crate::app::{App, Mode, Placement};
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

    /// The model menu, shaped after OpenCode's: a search field, providers as
    /// muted headers, plain rows with a check on the chosen one, a hover card
    /// beside the row with the model's facts, and a footer. It sets a
    /// preference the daemon is handed with the turn; the receipt says what ran.
    fn model_picker(&mut self, ui: &mut egui::Ui) {
        let selected = self.model.and_then(|i| self.choices.get(i)).cloned();
        let (label, pid, pname) = match &selected {
            Some(c) => (c.name.clone(), c.provider_id.clone(), c.provider_name.clone()),
            None => ("No model".to_owned(), String::new(), String::new()),
        };
        let galley = ui.painter().layout_no_wrap(label, theme::sans(13.0), theme::INK_2);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(galley.size().x + 60.0, 28.0), egui::Sense::click());
        ui.painter().rect_filled(rect, CornerRadius::same(8), if response.hovered() { theme::FACE_2 } else { theme::FACE });
        ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
        provider_badge(ui, egui::pos2(rect.left() + 18.0, rect.center().y), 14.0, &pid, &pname);
        ui.painter().galley(egui::pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0), galley, theme::INK_2);
        icons::chevron_down(ui.painter(), egui::pos2(rect.right() - 13.0, rect.center().y), theme::FAINT);
        let hint = match &selected {
            Some(c) => format!("Preference sent with the turn: {} via {}. The receipt says what ran.", c.sku, c.provider_id),
            None => "No tool-calling model in the catalog.".to_owned(),
        };
        let response = response.on_hover_text(hint);

        let force_open = std::mem::take(&mut self.open_model_menu_once);
        egui::Popup::menu(&response)
            .open_memory(if force_open { Some(egui::SetOpenCommand::Bool(true)) } else { None })
            .align(egui::RectAlign::TOP_START)
            .gap(8.0)
            .width(300.0)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| self.model_menu(ui));
    }

    fn model_menu(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.horizontal(|ui| {
            ui.add_space(4.0);
            icons::inline(ui, 16.0, theme::FAINTER, icons::search);
            ui.add(
                egui::TextEdit::singleline(&mut self.model_query)
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::default())
                    .font(theme::prose(12.5))
                    .hint_text(RichText::new("Search models").font(theme::prose(12.5)).color(theme::FAINTER)),
            );
        });
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().hline(rule.x_range(), rule.center().y, Stroke::new(1.0, theme::HAIR_2));
        ui.add_space(4.0);

        let query = self.model_query.trim().to_lowercase();
        let choices = self.choices.clone();
        let mut hovered: Option<(usize, egui::Rect)> = None;

        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            let mut last_provider = String::new();
            for (i, c) in choices.iter().enumerate() {
                if !query.is_empty()
                    && !c.name.to_lowercase().contains(&query)
                    && !c.provider_name.to_lowercase().contains(&query)
                {
                    continue;
                }
                if c.provider_id != last_provider {
                    last_provider = c.provider_id.clone();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&c.provider_name).font(theme::sans(11.0)).color(theme::FAINTER));
                    });
                    ui.add_space(2.0);
                }
                let is_selected = self.model == Some(i);
                let row = ui.allocate_response(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
                let r = row.rect;
                if row.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(10));
                    hovered = Some((i, r));
                }
                ui.painter().text(
                    egui::pos2(r.left() + 8.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    &c.name,
                    theme::sans(13.0),
                    if is_selected { theme::PERI_2 } else { theme::INK_2 },
                );
                if is_selected {
                    icons::check(ui.painter(), egui::pos2(r.right() - 14.0, r.center().y), theme::PERI_2);
                }
                if row.clicked() {
                    self.model = Some(i);
                }
            }
        });

        ui.add_space(4.0);
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().hline(rule.x_range(), rule.center().y, Stroke::new(1.0, theme::HAIR_2));
        let manage = ui.allocate_response(Vec2::new(ui.available_width(), 30.0), egui::Sense::click());
        icons::sliders(ui.painter(), egui::pos2(manage.rect.left() + 16.0, manage.rect.center().y), theme::FAINT);
        ui.painter().text(egui::pos2(manage.rect.left() + 32.0, manage.rect.center().y), egui::Align2::LEFT_CENTER, "Manage models", theme::sans(13.0), theme::FAINT);
        manage.on_hover_text("Providers and their keys are the daemon's to manage, and it is not running.");

        if let Some((i, r)) = hovered {
            if let Some(c) = choices.get(i) {
                egui::Area::new(egui::Id::new("model-detail"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(r.right() + 12.0, r.top() - 6.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::default()
                            .fill(theme::FACE)
                            .stroke(Stroke::new(1.0, theme::HAIR))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(200.0);
                                kv(ui, "Model", &c.name);
                                kv(ui, "Provider", &c.provider_name);
                                kv(ui, "Reasoning", if c.reasoning { "Allows reasoning" } else { "No" });
                                kv(ui, "Context", &c.context_tokens.map(thousands).unwrap_or_else(|| "unknown".into()));
                                if let Some(pr) = &c.usd_per_mtok {
                                    kv(ui, "Price / Mtok", &format!("${:.2} in, ${:.2} out", pr.input, pr.output));
                                }
                            });
                    });
            }
        }
    }
}

/// One line of the hover card: a quiet key, a right-aligned value.
fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(r.left(), r.center().y), egui::Align2::LEFT_CENTER, key, theme::sans(11.0), theme::FAINTER);
    ui.painter().text(egui::pos2(r.right(), r.center().y), egui::Align2::RIGHT_CENTER, value, theme::sans(11.0), theme::INK_2);
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A provider's mark at `size`, centred on `c`: the licensed SVG when one
/// exists, otherwise a monogram badge. The menu never invents a logo.
fn provider_badge(ui: &mut egui::Ui, c: egui::Pos2, size: f32, provider_id: &str, provider_name: &str) {
    let rect = egui::Rect::from_center_size(c, Vec2::splat(size));
    match crate::brand::provider_mark(provider_id) {
        Some(src) => {
            ui.put(rect, egui::Image::new(src).fit_to_exact_size(Vec2::splat(size)).tint(theme::INK));
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same(4), theme::CONTROL);
            let initial = provider_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, theme::display(size * 0.62), theme::INK);
        }
    }
}
