//! The top-left strip and the menu behind it: File, Edit, View, Help.
//!
//! Every item either does the thing or is disabled with the reason on hover.
//! A shortcut shown beside an item is one that actually fires: the Edit set is
//! egui's own text editing, zoom is the context's, and the rest are bound in
//! `shortcuts` below. New session carries none, because nothing is bound to it
//! until the daemon can open one.

use eframe::egui::{self, CornerRadius, Key, KeyboardShortcut, Modifiers, Rect, Vec2};

use crate::app::{composer_id, App, EditAction, SettingsPage};
use crate::icons;
use crate::theme;

pub const SC_SETTINGS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Comma);
pub const SC_SEARCH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
pub const SC_CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
pub const SC_SIDEBAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::B);
pub const SC_ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
pub const SC_ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
pub const SC_ZOOM_RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0);
pub const SC_UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub const SC_REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
pub const SC_CUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::X);
pub const SC_COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
pub const SC_PASTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::V);
pub const SC_SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::A);

/// The strip's width: the menu and the sidebar toggle, 28 points each.
pub const STRIP_W: f32 = 58.0;

/// A shortcut as the window prints it: the modifiers by name and then the
/// key's own character where it has one, "Ctrl+," rather than egui's
/// "Ctrl+Comma", which reads as a word to type.
pub(crate) fn keys(ctx: &egui::Context, sc: &KeyboardShortcut) -> String {
    let text = ctx.format_shortcut(sc);
    let (name, symbol) = (sc.logical_key.name(), sc.logical_key.symbol_or_name());
    match text.strip_suffix(name) {
        Some(head) if name != symbol => format!("{head}{symbol}"),
        _ => text,
    }
}

pub const REPO_URL: &str = "https://github.com/convexityos/vitna-code";

impl App {
    /// The window's own shortcuts. Keyboard zoom is egui's and needs nothing here.
    pub(crate) fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_shortcut(&SC_SEARCH)) {
            self.search = match self.search {
                Some(_) => None,
                None => Some(crate::search::Search::new(String::new())),
            };
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SC_SETTINGS)) {
            self.settings = Some(self.settings.unwrap_or(SettingsPage::General));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SC_SIDEBAR)) {
            self.sidebar_open = !self.sidebar_open;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SC_CLOSE)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Escape goes back from a run to the ledger, as the terminal client's
        // does, unless something floating has it first: a menu, search or
        // settings each close on their own Escape.
        if self.open_run.is_some()
            && self.search.is_none()
            && self.settings.is_none()
            && !ctx.any_popup_open()
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.open_run = None;
        }
    }

    /// An Edit-menu action lands here on the frame after the click, once the
    /// composer has focus again, as the event the field would have received
    /// from the keyboard.
    pub(crate) fn run_pending_edit(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_edit.take() else { return };
        if !ctx.memory(|m| m.has_focus(composer_id())) {
            return;
        }
        let key = |key: Key, modifiers: Modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let event = match action {
            EditAction::Undo => key(Key::Z, Modifiers::COMMAND),
            EditAction::Redo => key(Key::Y, Modifiers::COMMAND),
            EditAction::Cut => egui::Event::Cut,
            EditAction::Copy => egui::Event::Copy,
            EditAction::SelectAll => key(Key::A, Modifiers::COMMAND),
            EditAction::Paste => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(text) => egui::Event::Paste(text),
                Err(_) => return,
            },
        };
        ctx.input_mut(|i| i.events.push(event));
    }

    /// Two buttons, the menu and the sidebar toggle, laid rightward from `at`
    /// (its left edge, at the row's centre). With the sidebar hidden, search
    /// has no row to live in, so the strip carries it as a third.
    pub(crate) fn strip(&mut self, ui: &mut egui::Ui, at: egui::Pos2) {
        let w = if self.sidebar_open { STRIP_W } else { STRIP_W + 30.0 };
        let rect = Rect::from_min_size(egui::pos2(at.x, at.y - 14.0), Vec2::new(w, 28.0));
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = 2.0;
        let burger = icon_button(&mut ui, icons::hamburger, "Menu");
        let toggle = icon_button(
            &mut ui,
            icons::sidebar,
            if self.sidebar_open { "Hide the sidebar" } else { "Show the sidebar" },
        );
        if toggle.clicked() {
            self.sidebar_open = !self.sidebar_open;
        }
        if !self.sidebar_open {
            let hint = format!("Search ({})", keys(ui.ctx(), &SC_SEARCH));
            if icon_button(&mut ui, icons::search, &hint).clicked() {
                self.search = Some(crate::search::Search::new(String::new()));
            }
        }
        self.menu(&burger);
    }

    fn menu(&mut self, anchor: &egui::Response) {
        let ctx = anchor.ctx.clone();
        let force_open = std::mem::take(&mut self.open_menu_once);
        egui::Popup::menu(anchor)
            .open_memory(if force_open { Some(egui::SetOpenCommand::Bool(true)) } else { None })
            .align(egui::RectAlign::BOTTOM_START)
            .gap(4.0)
            .show(|ui| {
                ui.set_min_width(150.0);
                ui.menu_button("File", |ui| self.file_menu(ui, &ctx));
                ui.menu_button("Edit", |ui| self.edit_menu(ui, &ctx));
                ui.menu_button("View", |ui| self.view_menu(ui, &ctx));
                ui.menu_button("Help", |ui| self.help_menu(ui));
            });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(220.0);
        let linked = self.link.is_open();
        if ui
            .add_enabled(linked, egui::Button::new("New session"))
            .on_disabled_hover_text("Sessions are opened by the daemon, and it is not running.")
            .clicked()
        {
            self.new_session();
        }
        ui.add_enabled(false, egui::Button::new("Open folder..."))
            .on_disabled_hover_text("Opening another folder is the daemon's to do, and it is not running.");
        ui.separator();
        if ui.add(item("Settings", keys(ctx, &SC_SETTINGS))).clicked() {
            self.settings = Some(SettingsPage::General);
        }
        ui.separator();
        if ui.add(item("Close window", keys(ctx, &SC_CLOSE))).clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(200.0);
        let groups: [&[(&str, KeyboardShortcut, EditAction)]; 3] = [
            &[("Undo", SC_UNDO, EditAction::Undo), ("Redo", SC_REDO, EditAction::Redo)],
            &[
                ("Cut", SC_CUT, EditAction::Cut),
                ("Copy", SC_COPY, EditAction::Copy),
                ("Paste", SC_PASTE, EditAction::Paste),
            ],
            &[("Select all", SC_SELECT_ALL, EditAction::SelectAll)],
        ];
        for (g, group) in groups.iter().enumerate() {
            if g > 0 {
                ui.separator();
            }
            for (label, sc, action) in group.iter() {
                if ui.add(item(label, keys(ctx, sc))).clicked() {
                    self.pending_edit = Some(*action);
                    ctx.memory_mut(|m| m.request_focus(composer_id()));
                }
            }
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(200.0);
        let label = if self.sidebar_open { "Hide sidebar" } else { "Show sidebar" };
        if ui.add(item(label, keys(ctx, &SC_SIDEBAR))).clicked() {
            self.sidebar_open = !self.sidebar_open;
        }
        ui.separator();
        let z = ctx.zoom_factor();
        if ui.add(item("Zoom in", keys(ctx, &SC_ZOOM_IN))).clicked() {
            ctx.set_zoom_factor((z * 1.1).min(2.0));
        }
        if ui.add(item("Zoom out", keys(ctx, &SC_ZOOM_OUT))).clicked() {
            ctx.set_zoom_factor((z / 1.1).max(0.5));
        }
        if ui.add(item("Reset zoom", keys(ctx, &SC_ZOOM_RESET))).clicked() {
            ctx.set_zoom_factor(1.0);
        }
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(200.0);
        if ui.button("Vitna Code on GitHub").clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(REPO_URL));
        }
        if ui.button("Report an issue").clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{REPO_URL}/issues/new")));
        }
        ui.separator();
        if ui.button("About Vitna Code").clicked() {
            self.settings = Some(SettingsPage::General);
        }
    }
}

/// A menu row: a label with its shortcut at the right.
fn item(label: &str, shortcut: String) -> egui::Button<'_> {
    egui::Button::new(label).shortcut_text(shortcut)
}

fn icon_button(
    ui: &mut egui::Ui,
    icon: fn(&egui::Painter, egui::Pos2, egui::Color32),
    hint: &str,
) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(6), theme::FACE);
    }
    icon(ui.painter(), r.center(), theme::FAINT);
    resp.on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key with a character of its own is printed as that character, and
    /// one without keeps its name.
    #[test]
    fn a_shortcut_prints_the_key_itself() {
        let ctx = egui::Context::default();
        let settings = keys(&ctx, &SC_SETTINGS);
        assert!(settings.ends_with(','), "{settings}");
        assert!(!settings.contains("Comma"), "{settings}");
        assert!(keys(&ctx, &SC_SEARCH).ends_with('K'));
    }
}
