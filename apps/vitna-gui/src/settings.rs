//! Settings: a centred modal with a nav on the left, laid out after OpenCode's.
//!
//! Every page states something this window can actually know or set: the
//! defaults it sends with a turn, the shortcuts that are bound, the daemon it
//! looked for, the providers whose key is in its environment, the catalog.
//! Nothing here stores a key or promises persistence the daemon does not yet
//! give, and the notes at the foot of each page say so.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Stroke, Vec2};

use crate::app::{App, Mode, Placement, SettingsPage};
use crate::catalog::{Catalog, Choice};
use crate::composer::provider_badge;
use crate::icons;
use crate::link::Link;
use crate::menu::{
    SC_CLOSE, SC_COPY, SC_CUT, SC_PASTE, SC_REDO, SC_SELECT_ALL, SC_SETTINGS, SC_SIDEBAR, SC_UNDO,
    SC_ZOOM_IN, SC_ZOOM_OUT, SC_ZOOM_RESET,
};
use crate::theme;

const NAV_W: f32 = 200.0;

type Icon = fn(&egui::Painter, egui::Pos2, Color32);

impl App {
    pub(crate) fn settings_modal(&mut self, ui: &mut egui::Ui) {
        let Some(page) = self.settings else { return };
        let screen = ui.ctx().content_rect();
        let size = Vec2::new(
            (screen.width() - 60.0).min(880.0),
            (screen.height() - 60.0).min(540.0),
        );
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(
                egui::Frame::default()
                    .fill(theme::GROUND)
                    .stroke(Stroke::new(1.0, theme::HAIR))
                    .corner_radius(CornerRadius::same(12)),
            )
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ui.ctx(), |ui| {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let nav = Rect::from_min_size(rect.min, Vec2::new(NAV_W, rect.height()));
                let pane = Rect::from_min_max(egui::pos2(nav.right(), rect.top()), rect.max);
                ui.painter().vline(nav.right(), nav.y_range(), Stroke::new(1.0, theme::HAIR_2));
                self.settings_nav(ui, nav, page);
                self.settings_page(ui, pane, page);

                let x = Rect::from_center_size(
                    egui::pos2(rect.right() - 22.0, rect.top() + 22.0),
                    Vec2::splat(24.0),
                );
                // interact, not allocate_rect: the latter moves the layout cursor.
                let close = ui.interact(x, ui.id().with("close"), egui::Sense::click());
                if close.hovered() {
                    ui.painter().rect_filled(x, CornerRadius::same(6), theme::FACE);
                }
                icons::close(ui.painter(), x.center(), theme::FAINT);
                close.clicked()
            });
        if modal.should_close() || modal.inner {
            self.settings = None;
        }
    }

    fn settings_nav(&mut self, ui: &mut egui::Ui, rect: Rect, page: SettingsPage) {
        let mut ui = ui.new_child(
            egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(12.0, 16.0))),
        );
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.spacing_mut().item_spacing.y = 2.0;

        nav_group(&mut ui, "Window");
        for (p, icon, label) in [
            (SettingsPage::General, icons::sliders as Icon, "General"),
            (SettingsPage::Shortcuts, icons::keyboard as Icon, "Shortcuts"),
        ] {
            if nav_row(&mut ui, icon, label, page == p).clicked() {
                self.settings = Some(p);
            }
        }
        ui.add_space(12.0);
        nav_group(&mut ui, "Daemon");
        for (p, icon, label) in [
            (SettingsPage::Daemon, icons::server as Icon, "Daemon"),
            (SettingsPage::Providers, icons::grid as Icon, "Providers"),
            (SettingsPage::Models, icons::sparkle as Icon, "Models"),
        ] {
            if nav_row(&mut ui, icon, label, page == p).clicked() {
                self.settings = Some(p);
            }
        }

        // The product and its version sign the nav, the way OpenCode's does.
        ui.painter().text(
            egui::pos2(rect.left() + 20.0, rect.bottom() - 32.0),
            Align2::LEFT_CENTER,
            "Vitna Code",
            theme::sans(12.0),
            theme::FAINT,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 20.0, rect.bottom() - 17.0),
            Align2::LEFT_CENTER,
            format!("v{}", env!("CARGO_PKG_VERSION")),
            theme::mono(11.0),
            theme::FAINTER,
        );
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, rect: Rect, page: SettingsPage) {
        let inner = Rect::from_min_max(rect.min + Vec2::new(28.0, 22.0), rect.max - Vec2::new(28.0, 20.0));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.set_clip_rect(rect);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let title = match page {
            SettingsPage::General => "General",
            SettingsPage::Shortcuts => "Shortcuts",
            SettingsPage::Daemon => "Daemon",
            SettingsPage::Providers => "Providers",
            SettingsPage::Models => "Models",
        };
        ui.label(RichText::new(title).font(theme::display(16.0)).color(theme::INK));
        ui.add_space(12.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            match page {
                SettingsPage::General => self.page_general(ui),
                SettingsPage::Shortcuts => page_shortcuts(ui),
                SettingsPage::Daemon => self.page_daemon(ui),
                SettingsPage::Providers => self.page_providers(ui),
                SettingsPage::Models => self.page_models(ui),
            }
        });
    }

    fn page_general(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        section(ui, "Defaults for a new turn");
        card(ui, |ui| {
            let mode = self.mode;
            let mut pick = None;
            setting_row(ui, "Mode", "Build edits files. Plan proposes and stops.", |ui| {
                // Right-to-left, so the later pill lands leftmost.
                for m in [Mode::Plan, Mode::Build] {
                    if pill(ui, m.label(), mode == m).clicked() {
                        pick = Some(m);
                    }
                }
            });
            if let Some(m) = pick {
                self.mode = m;
            }
            rule(ui);
            let on = self.placement == Placement::Worktree;
            let mut flip = false;
            setting_row(ui, "Worktree", "Run each turn in a fresh worktree, leaving this checkout as it is.", |ui| {
                if toggle(ui, on).clicked() {
                    flip = true;
                }
            });
            if flip {
                self.placement = if on { Placement::Local } else { Placement::Worktree };
            }
            rule(ui);
            let name = self
                .model
                .and_then(|i| self.choices.get(i))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "No model".to_string());
            setting_row(ui, "Model", "The preference sent with each turn. Change it under Models.", |ui| {
                ui.label(RichText::new(name).font(theme::sans(13.0)).color(theme::INK_2));
            });
        });

        ui.add_space(16.0);
        section(ui, "Window");
        card(ui, |ui| {
            let open = self.sidebar_open;
            let mut flip = false;
            let desc = format!("{} toggles it.", ctx.format_shortcut(&SC_SIDEBAR));
            setting_row(ui, "Sidebar", &desc, |ui| {
                if toggle(ui, open).clicked() {
                    flip = true;
                }
            });
            if flip {
                self.sidebar_open = !open;
            }
            rule(ui);
            let z = ctx.zoom_factor();
            let desc = format!(
                "{} and {} to zoom, {} to reset.",
                ctx.format_shortcut(&SC_ZOOM_IN),
                ctx.format_shortcut(&SC_ZOOM_OUT),
                ctx.format_shortcut(&SC_ZOOM_RESET)
            );
            setting_row(ui, "Zoom", &desc, |ui| {
                ui.label(RichText::new(format!("{:.0}%", z * 100.0)).font(theme::mono(12.5)).color(theme::INK_2));
                if (z - 1.0).abs() > 0.01 && text_button(ui, "Reset").clicked() {
                    ctx.set_zoom_factor(1.0);
                }
            });
        });

        ui.add_space(14.0);
        note(ui, "Preferences for this window. The daemon keeps them once it runs; until then they last as long as the window does.");
    }

    fn page_daemon(&mut self, ui: &mut egui::Ui) {
        section(ui, "State");
        let (dot, word, detail) = match &self.link {
            Link::Open { endpoint } => (theme::OK, "Connected", endpoint.clone()),
            Link::Absent { detail, .. } => (theme::RUST, "Not running", detail.clone()),
        };
        card(ui, |ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
            ui.painter().circle_filled(egui::pos2(r.left() + 18.0, r.bottom()), 3.5, dot);
            ui.painter().text(egui::pos2(r.left() + 30.0, r.bottom()), Align2::LEFT_CENTER, word, theme::sans(13.5), theme::INK);
            ui.horizontal(|ui| {
                ui.add_space(30.0);
                ui.add(egui::Label::new(RichText::new(detail).font(theme::sans(12.0)).color(theme::FAINT)).truncate());
            });
            ui.add_space(8.0);
        });

        ui.add_space(16.0);
        section(ui, "Looked for");
        let tried = match &self.link {
            Link::Absent { tried, .. } => tried.clone(),
            Link::Open { endpoint } => vec![endpoint.clone()],
        };
        card(ui, |ui| {
            if tried.is_empty() {
                plain_row(ui, "No endpoint to try on this platform.");
            }
            for (i, t) in tried.iter().enumerate() {
                if i > 0 {
                    rule(ui);
                }
                mono_row(ui, t);
            }
        });
        ui.add_space(8.0);
        if text_button(ui, "Check again").clicked() {
            self.reprobe();
        }

        ui.add_space(14.0);
        note(ui, "The daemon owns sessions, provider keys and receipts. Until it is running this window reads the workspace and states preferences, nothing more.");
    }

    fn page_providers(&mut self, ui: &mut egui::Ui) {
        let Some(catalog) = self.catalog.clone() else {
            note(ui, "The model catalog did not load, so there is nothing to list.");
            return;
        };
        let (ready, rest): (Vec<_>, Vec<_>) =
            catalog.providers.iter().partition(|p| Catalog::provider_ready(p));

        section(ui, "Connected");
        if ready.is_empty() {
            note(ui, "None yet. A provider is connected when the variable it authenticates with is set in this window's environment.");
        } else {
            card(ui, |ui| {
                for (i, p) in ready.iter().enumerate() {
                    if i > 0 {
                        rule(ui);
                    }
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), egui::Sense::hover());
                    provider_badge(ui, egui::pos2(r.left() + 20.0, r.center().y), 14.0, &p.id, &p.name);
                    let name = ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, &p.name, theme::sans(13.5), theme::INK);
                    tag(ui, egui::pos2(name.right() + 10.0, r.center().y), "Environment");
                    ui.painter().text(egui::pos2(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, p.env.join(", "), theme::mono(11.5), theme::FAINTER);
                }
            });
        }

        ui.add_space(16.0);
        section(ui, "Available");
        card(ui, |ui| {
            if rest.is_empty() {
                plain_row(ui, "Every provider in the catalog is connected.");
            }
            for (i, p) in rest.iter().enumerate() {
                if i > 0 {
                    rule(ui);
                }
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 50.0), egui::Sense::hover());
                provider_badge(ui, egui::pos2(r.left() + 20.0, r.top() + 18.0), 14.0, &p.id, &p.name);
                ui.painter().text(egui::pos2(r.left() + 36.0, r.top() + 18.0), Align2::LEFT_CENTER, &p.name, theme::sans(13.5), theme::INK);
                let needs = if p.env.is_empty() {
                    "No variable declared for it.".to_string()
                } else {
                    format!("Needs {} in the environment.", p.env.join(" or "))
                };
                ui.painter().text(egui::pos2(r.left() + 36.0, r.top() + 36.0), Align2::LEFT_CENTER, needs, theme::sans(12.0), theme::FAINT);

                let b = Rect::from_min_size(egui::pos2(r.right() - 14.0 - 84.0, r.center().y - 13.0), Vec2::new(84.0, 26.0));
                let connect = ui.interact(b, ui.id().with(("connect", &p.id)), egui::Sense::hover());
                ui.painter().rect_stroke(b, CornerRadius::same(6), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
                icons::plus(ui.painter(), egui::pos2(b.left() + 16.0, b.center().y), theme::FAINTER);
                ui.painter().text(egui::pos2(b.left() + 30.0, b.center().y), Align2::LEFT_CENTER, "Connect", theme::sans(12.5), theme::FAINTER);
                let var = p.env.first().map(String::as_str).unwrap_or("the variable");
                connect.on_hover_text(format!("Keys are the daemon's to keep, and it is not running. Until then, set {var} and start the window again."));
            }
        });

        ui.add_space(14.0);
        note(ui, "Connected means the key is present in this window's environment. That is a check, not a promise the key works.");
    }

    fn page_models(&mut self, ui: &mut egui::Ui) {
        section(ui, "Catalog");
        let choices = self.choices.clone();
        let mut pick = None;
        card(ui, |ui| {
            let mut last = String::new();
            for (i, c) in choices.iter().enumerate() {
                if c.provider_id != last {
                    last = c.provider_id.clone();
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), egui::Sense::hover());
                    ui.painter().text(egui::pos2(r.left() + 14.0, r.bottom() - 6.0), Align2::LEFT_BOTTOM, &c.provider_name, theme::sans(11.0), theme::FAINTER);
                }
                let on = self.model == Some(i);
                let row = ui.allocate_response(Vec2::new(ui.available_width(), 30.0), egui::Sense::click());
                let r = row.rect;
                if row.hovered() {
                    ui.painter().rect_filled(r.shrink2(Vec2::new(6.0, 0.0)), CornerRadius::same(6), Color32::from_white_alpha(8));
                }
                ui.painter().text(egui::pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, &c.name, theme::sans(13.0), if on { theme::PERI_2 } else { theme::INK_2 });
                let right = if on { r.right() - 34.0 } else { r.right() - 14.0 };
                ui.painter().text(egui::pos2(right, r.center().y), Align2::RIGHT_CENTER, model_meta(c), theme::mono(11.0), theme::FAINTER);
                if on {
                    icons::check(ui.painter(), egui::pos2(r.right() - 18.0, r.center().y), theme::PERI_2);
                }
                if row.clicked() {
                    pick = Some(i);
                }
            }
        });
        if let Some(i) = pick {
            self.model = Some(i);
        }
        ui.add_space(14.0);
        note(ui, "A committed copy of models.dev, narrowed to what the providers crate can call. Choosing one sets a preference; the receipt says what ran.");
    }
}

fn page_shortcuts(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let f = |sc: &egui::KeyboardShortcut| ctx.format_shortcut(sc);
    section(ui, "Window");
    card(ui, |ui| {
        let rows = [
            ("Settings", f(&SC_SETTINGS)),
            ("Toggle sidebar", f(&SC_SIDEBAR)),
            ("Close window", f(&SC_CLOSE)),
            ("Zoom in", f(&SC_ZOOM_IN)),
            ("Zoom out", f(&SC_ZOOM_OUT)),
            ("Reset zoom", f(&SC_ZOOM_RESET)),
        ];
        for (i, (label, keys)) in rows.iter().enumerate() {
            if i > 0 {
                rule(ui);
            }
            shortcut_row(ui, label, keys);
        }
    });
    ui.add_space(16.0);
    section(ui, "Composer");
    card(ui, |ui| {
        let rows = [
            ("Send", "Enter".to_string()),
            ("New line", "Shift+Enter".to_string()),
            ("Undo", f(&SC_UNDO)),
            ("Redo", f(&SC_REDO)),
            ("Cut, copy, paste", format!("{}, {}, {}", f(&SC_CUT), f(&SC_COPY), f(&SC_PASTE))),
            ("Select all", f(&SC_SELECT_ALL)),
        ];
        for (i, (label, keys)) in rows.iter().enumerate() {
            if i > 0 {
                rule(ui);
            }
            shortcut_row(ui, label, keys);
        }
    });
    ui.add_space(14.0);
    note(ui, "Shortcuts are fixed in this version.");
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).font(theme::sans(13.0)).color(theme::INK_2));
    ui.add_space(8.0);
}

/// A bordered surface the rows of a page sit on.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(theme::FIELD)
        .stroke(Stroke::new(1.0, theme::HAIR_2))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(0, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn rule(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline((r.left() + 14.0)..=(r.right() - 14.0), r.center().y, Stroke::new(1.0, theme::HAIR_2));
}

/// A label and its description on the left, a control on the right.
fn setting_row(ui: &mut egui::Ui, label: &str, desc: &str, right: impl FnOnce(&mut egui::Ui)) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(r.left() + 14.0, r.top() + 16.0), Align2::LEFT_CENTER, label, theme::sans(13.5), theme::INK);
    ui.painter().text(egui::pos2(r.left() + 14.0, r.top() + 32.0), Align2::LEFT_CENTER, desc, theme::sans(12.0), theme::FAINT);
    let right_rect = Rect::from_min_max(egui::pos2(r.left() + r.width() * 0.6, r.top()), egui::pos2(r.right() - 14.0, r.bottom()));
    let mut sub = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    sub.spacing_mut().item_spacing.x = 6.0;
    right(&mut sub);
}

fn shortcut_row(ui: &mut egui::Ui, label: &str, keys: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, label, theme::sans(13.0), theme::INK_2);
    let galley = ui.painter().layout_no_wrap(keys.to_string(), theme::mono(11.5), theme::INK_2);
    let cap = Rect::from_min_max(
        egui::pos2(r.right() - 14.0 - galley.size().x - 14.0, r.center().y - 10.0),
        egui::pos2(r.right() - 14.0, r.center().y + 10.0),
    );
    ui.painter().rect_filled(cap, CornerRadius::same(5), theme::FACE);
    ui.painter().rect_stroke(cap, CornerRadius::same(5), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
    ui.painter().galley(egui::pos2(cap.left() + 7.0, cap.center().y - galley.size().y / 2.0), galley, theme::INK_2);
}

fn plain_row(ui: &mut egui::Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, text, theme::sans(12.5), theme::FAINT);
}

fn mono_row(ui: &mut egui::Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), egui::Sense::hover());
    ui.painter().text(egui::pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, text, theme::mono(12.0), theme::INK_2);
}

fn pill(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), theme::sans(12.5), theme::INK_2);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 24.0), egui::Sense::click());
    if on {
        ui.painter().rect_filled(rect, CornerRadius::same(6), theme::FACE_2);
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, if on { theme::PERI } else { theme::HAIR_2 }), egui::StrokeKind::Inside);
    ui.painter().galley(egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, if on { theme::INK } else { theme::FAINT });
    resp
}

fn toggle(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(34.0, 18.0), egui::Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(9), if on { theme::PERI } else { theme::CONTROL });
    let knob = if on { rect.right() - 9.0 } else { rect.left() + 9.0 };
    ui.painter().circle_filled(egui::pos2(knob, rect.center().y), 6.5, if on { theme::CANVAS } else { theme::FAINT });
    resp
}

fn text_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).font(theme::sans(12.5)).color(theme::PERI_2))
            .fill(Color32::TRANSPARENT)
            .frame(false),
    )
}

/// A small tag beside a name, like OpenCode's "Environment".
fn tag(ui: &mut egui::Ui, at: egui::Pos2, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), theme::sans(10.5), theme::FAINT);
    let rect = Rect::from_min_size(egui::pos2(at.x, at.y - 9.0), Vec2::new(galley.size().x + 12.0, 18.0));
    ui.painter().rect_filled(rect, CornerRadius::same(4), theme::FACE);
    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
    ui.painter().galley(egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, theme::FAINT);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).font(theme::prose(12.5)).color(theme::FAINT)).wrap());
}

fn model_meta(c: &Choice) -> String {
    let ctx = c.context_tokens.map(compact_tokens).unwrap_or_else(|| "unknown".to_string());
    match &c.usd_per_mtok {
        Some(p) => format!("{ctx} ctx, ${:.2} in, ${:.2} out", p.input, p.output),
        None => format!("{ctx} ctx"),
    }
}

fn compact_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else {
        format!("{}k", n / 1000)
    }
}

fn nav_group(ui: &mut egui::Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(title).font(theme::sans(12.0)).color(theme::FAINT));
    });
    ui.add_space(2.0);
}

fn nav_row(ui: &mut egui::Ui, icon: Icon, label: &str, active: bool) -> egui::Response {
    let resp = ui.allocate_response(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
    let r = resp.rect;
    if active {
        ui.painter().rect_filled(r, CornerRadius::same(6), theme::FACE);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(8));
    }
    icon(ui.painter(), egui::pos2(r.left() + 14.0, r.center().y), if active { theme::INK_2 } else { theme::FAINT });
    ui.painter().text(
        egui::pos2(r.left() + 30.0, r.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::sans(13.0),
        if active { theme::INK } else { theme::INK_2 },
    );
    resp
}
