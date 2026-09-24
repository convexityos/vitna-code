//! The composer: the thing you reach for.
//!
//! Its base is shaped after Claude Code's: a bar above the field stating the
//! repository the turn will run in (`bar.rs`), a return-key cap in the field,
//! and a base row with attach and the mode on the left, the model and send on
//! the right.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Rect, RichText, Stroke, Vec2};

use crate::app::{App, Mode};
use crate::icons;
use crate::stage::column;
use crate::theme;

impl App {
    pub(crate) fn composer(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let col = column(rect);
        let inner = Rect::from_min_max(col.min, egui::pos2(col.right(), rect.bottom()));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.set_clip_rect(rect);
        // The rows sit at the gaps stated here, not at egui's default
        // spacing added on top of them.
        ui.spacing_mut().item_spacing.y = 0.0;

        // Why Send cannot send, said beside it rather than over the page.
        if let crate::link::Link::Absent { .. } = &self.link {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (d, _) = ui.allocate_exact_size(Vec2::new(8.0, 20.0), egui::Sense::hover());
                ui.painter().circle_filled(d.center(), 3.0, theme::RUST);
                ui.label(
                    RichText::new("The daemon is not running, so there is nothing to send to yet.")
                        .font(theme::prose(theme::FS_UI))
                        .color(theme::FAINT),
                );
                let again = ui.add(
                    egui::Button::new(RichText::new("Check again").font(theme::sans(theme::FS_UI)).color(theme::PERI_2))
                        .fill(Color32::TRANSPARENT)
                        .frame(false),
                );
                if again.clicked() {
                    self.reprobe();
                }
            });
            ui.add_space(10.0);
        }

        self.repo_bar(&mut ui);
        ui.add_space(6.0);

        // Four conditions, and each one has its own hover text below, because
        // a disabled control that will not say why is the thing this window
        // exists not to ship.
        let ready = self.link.is_open()
            && self.active_session.is_some()
            && !self.turn_running
            && !self.draft.trim().is_empty();

        egui::Frame::default()
            .fill(theme::FIELD)
            .stroke(Stroke::new(1.0, theme::HAIR))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin { left: 14, right: 8, top: 8, bottom: 8 })
            .show(&mut ui, |ui| {
                ui.horizontal_top(|ui| {
                    // The cap's width is reserved whether or not it shows, so
                    // the first keystroke does not move the text.
                    let width = ui.available_width() - 40.0;
                    let empty = self.draft.is_empty();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.draft)
                            .id(crate::app::composer_id())
                            .desired_rows(1)
                            .desired_width(width)
                            .frame(egui::Frame::default())
                            // What you type is a paragraph, and so is the prompt
                            // that stands in for it; both read in the prose face.
                            .font(theme::prose(theme::FS_BODY))
                            .text_color(theme::INK)
                            // Enter is for sending, as the cap says; a new line
                            // is Shift+Enter.
                            .return_key(Some(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                egui::Key::Enter,
                            )))
                            .hint_text(
                                RichText::new("Ask anything")
                                    .font(theme::prose(theme::FS_BODY))
                                    .color(theme::FAINTER),
                            ),
                    );
                    if empty {
                        ui.add_space(8.0);
                        keycap(ui);
                    }
                    // The cap says Enter sends, so it has to. return_key is
                    // set to Shift+Enter, so a bare Enter never reaches the
                    // field and arrives here as a plain key press.
                    let focused = ui.ctx().memory(|m| m.has_focus(crate::app::composer_id()));
                    let entered = ui.input(|i| {
                        i.key_pressed(egui::Key::Enter) && !i.modifiers.shift
                    });
                    if focused && entered && ready {
                        self.submit();
                    }
                });
            });

        // Beneath the field, in the open, the way Claude Code lays its base
        // row on the canvas rather than inside the box.
        ui.add_space(6.0);
        let row = ui.horizontal(|ui| {
            // Attach, on the left, the way all three references place it.
            let (r, attach) =
                ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
            if attach.hovered() {
                ui.painter().circle_filled(r.center(), 13.0, theme::FACE);
            }
            icons::plus(ui.painter(), r.center(), theme::INK);
            attach.on_hover_text("Name a file with @, or attach one. Both need the daemon.");

            ui.add_space(2.0);
            self.mode_picker(ui);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (r, send) =
                    ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
                if ready {
                    ui.painter().circle_filled(r.center(), 12.0, theme::PERI);
                    icons::arrow_up(ui.painter(), r.center(), theme::CANVAS);
                } else {
                    // Nothing to send yet: an empty ring, the resting
                    // state Claude Code draws.
                    ui.painter().circle_stroke(
                        r.center(),
                        8.0,
                        Stroke::new(1.5, theme::FAINT),
                    );
                }
                let hint = if !self.link.is_open() {
                    "Nothing to send to: the daemon is not running."
                } else if self.turn_running {
                    "A turn is running. Wait for it to finish."
                } else if self.active_session.is_none() {
                    "Open a session first, so the run has somewhere to belong."
                } else if self.draft.trim().is_empty() {
                    "Enter sends once there is something to send."
                } else {
                    "Send (Enter)"
                };
                let sent = send.clicked();
                send.on_hover_text(hint);
                if ready && sent {
                    self.submit();
                }

                ui.add_space(2.0);
                self.model_picker(ui);
            });
        });

        // What this frame used, for the stage to hand back next frame, with
        // a stated room under the base row so it never sits on the window's
        // bottom edge.
        let want = row.response.rect.bottom() - inner.top() + 10.0;
        if (want - self.composer_h).abs() > 0.5 {
            self.composer_h = want;
            ui.ctx().request_repaint();
        }
    }

    /// The mode, as a plain word that opens a two-row menu.
    fn mode_picker(&mut self, ui: &mut egui::Ui) {
        let galley = ui.painter().layout_no_wrap(self.mode.label().to_string(), theme::sans(theme::FS_UI), theme::INK);
        let (rect, response) = ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 24.0), egui::Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(7), theme::FACE);
        }
        ui.painter().galley(egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, theme::INK);
        let response = response.on_hover_text("Build edits files. Plan proposes and stops.");

        egui::Popup::menu(&response)
            .align(egui::RectAlign::TOP_START)
            .gap(8.0)
            .width(240.0)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (m, what) in [
                    (Mode::Build, "Edits files and runs commands."),
                    (Mode::Plan, "Proposes, then stops for you."),
                ] {
                    let row = ui.allocate_response(Vec2::new(ui.available_width(), 40.0), egui::Sense::click());
                    let r = row.rect;
                    if row.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(10));
                    }
                    let on = self.mode == m;
                    ui.painter().text(egui::pos2(r.left() + 10.0, r.top() + 13.0), egui::Align2::LEFT_CENTER, m.label(), theme::sans(theme::FS_UI), if on { theme::PERI_2 } else { theme::INK });
                    ui.painter().text(egui::pos2(r.left() + 10.0, r.top() + 28.0), egui::Align2::LEFT_CENTER, what, theme::sans(theme::FS_MICRO), theme::FAINTER);
                    if on {
                        icons::check(ui.painter(), egui::pos2(r.right() - 14.0, r.center().y), theme::PERI_2);
                    }
                    if row.clicked() {
                        self.mode = m;
                    }
                }
            });
    }

    /// The model menu, shaped after OpenCode's: a search field, providers as
    /// muted headers, plain rows with a check on the chosen one, a hover card
    /// beside the row with the model's facts, and a footer. It sets a
    /// preference the daemon is handed with the turn; the receipt says what ran.
    /// Its trigger is the model's name in plain text, as Claude Code writes it.
    fn model_picker(&mut self, ui: &mut egui::Ui) {
        let selected = self.model.and_then(|i| self.choices.get(i)).cloned();
        let (label, pid, pname) = match &selected {
            Some(c) => (c.name.clone(), c.provider_id.clone(), c.provider_name.clone()),
            None => ("No model".to_owned(), String::new(), String::new()),
        };
        let galley = ui.painter().layout_no_wrap(label, theme::sans(theme::FS_UI), theme::INK);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(galley.size().x + 40.0, 24.0), egui::Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(7), theme::FACE);
        }
        provider_badge(ui, egui::pos2(rect.left() + 16.0, rect.center().y), 14.0, &pid, &pname);
        ui.painter().galley(egui::pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0), galley, theme::INK);
        let hint = match &selected {
            Some(c) => format!("Preference sent with the turn: {} via {}. The receipt says what ran.", c.sku, c.provider_id),
            None => "No tool-calling model in the catalog.".to_owned(),
        };
        let response = response.on_hover_text(hint);

        let force_open = std::mem::take(&mut self.open_model_menu_once);
        egui::Popup::menu(&response)
            .open_memory(if force_open { Some(egui::SetOpenCommand::Bool(true)) } else { None })
            .align(egui::RectAlign::TOP_END)
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
            .font(theme::prose(theme::FS_UI))
            .hint_text(RichText::new("Search models").font(theme::prose(theme::FS_UI)).color(theme::FAINTER)),
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
                        ui.label(RichText::new(&c.provider_name).font(theme::sans(theme::FS_MICRO)).color(theme::FAINTER));
                    });
                    ui.add_space(2.0);
                }
                let is_selected = self.model == Some(i);
                let row = ui.allocate_response(Vec2::new(ui.available_width(), 26.0), egui::Sense::click());
                let r = row.rect;
                if row.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(10));
                    hovered = Some((i, r));
                }
                ui.painter().text(
                    egui::pos2(r.left() + 8.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    &c.name,
                    theme::sans(theme::FS_UI),
                    if is_selected { theme::PERI_2 } else { theme::INK },
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
        ui.painter().text(egui::pos2(manage.rect.left() + 32.0, manage.rect.center().y), egui::Align2::LEFT_CENTER, "Manage models", theme::sans(theme::FS_UI), theme::FAINT);
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
    ui.painter().text(egui::pos2(r.left(), r.center().y), egui::Align2::LEFT_CENTER, key, theme::sans(theme::FS_MICRO), theme::FAINTER);
    ui.painter().text(egui::pos2(r.right(), r.center().y), egui::Align2::RIGHT_CENTER, value, theme::sans(theme::FS_MICRO), theme::INK);
}

pub(crate) fn thousands(n: u64) -> String {
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
pub(crate) fn provider_badge(ui: &mut egui::Ui, c: egui::Pos2, size: f32, provider_id: &str, provider_name: &str) {
    let rect = egui::Rect::from_center_size(c, Vec2::splat(size));
    match crate::brand::provider_mark(provider_id) {
        Some(src) => {
            // paint_at, not put: put allocates the rect and drags the layout
            // cursor to the badge's bottom, which folded the settings rows.
            egui::Image::new(src).tint(theme::INK).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same(4), theme::CONTROL);
            let initial = provider_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, theme::display(size * 0.74), theme::INK);
        }
    }
}

/// The return-key cap at the end of the empty field.
fn keycap(ui: &mut egui::Ui) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(24.0, 18.0), egui::Sense::hover());
    let r = r.translate(Vec2::new(0.0, 1.0));
    ui.painter().rect_filled(r, CornerRadius::same(5), theme::FACE);
    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
    icons::enter(ui.painter(), r.center(), theme::FAINT);
    resp.on_hover_text("Enter sends. Shift+Enter starts a new line.");
}

impl crate::app::App {
    /// Hands the draft to the daemon.
    ///
    /// The model is a preference, and the receipt reports what actually ran,
    /// which is why the sku travels as a request field rather than as a claim.
    /// `auto_approve` follows the mode: Build acts, Plan only looks, and the
    /// daemon's approval gate is what enforces that rather than this window.
    pub(crate) fn submit(&mut self) {
        let Some(session_id) = self.active_session.clone() else {
            return;
        };
        let prompt = std::mem::take(&mut self.draft);
        let model_sku = self
            .model
            .and_then(|i| self.choices.get(i))
            .map(|c| c.sku.clone());
        let provider = self
            .model
            .and_then(|i| self.choices.get(i))
            .map(|c| c.provider_id.clone());

        self.turn_running = true;
        self.last_error = None;
        self.last_result = None;
        self.worker
            .send(crate::daemon::Command::SubmitTurn(Box::new(
                vitna_protocol::api::SubmitTurnRequest {
                    session_id,
                    prompt,
                    provider,
                    model_sku,
                    auto_approve: matches!(self.mode, crate::app::Mode::Build),
                    verification_command: None,
                },
            )));
    }
}
