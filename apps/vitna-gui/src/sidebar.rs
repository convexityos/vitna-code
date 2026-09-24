//! The sidebar: work, listed by name, on the desk.
//!
//! It sits on the window's floor with no ground or edge of its own; the
//! panel's own edge is what divides them, the way the owner's new design
//! stands a panel on its rail. Text first, every row a word with its Lucide
//! icon, and the key that reaches it written quietly at the right when one
//! is bound. At the foot, a readout rather than a light: whether the daemon
//! is there, and the key it signs receipts with, which is what every
//! signature check on the page is made against.

use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Vec2};

use crate::app::{App, SettingsPage};
use crate::icons;
use crate::link::Link;
use crate::theme;

type Icon = fn(&egui::Painter, egui::Pos2, Color32);

const ROW_H: f32 = 30.0;
/// The foot's height: Settings and the two lines of the readout.
const FOOT_H: f32 = 96.0;

impl App {
    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        // The lockup, on the line the panel's bar sits on, so the two read as
        // one line across the window. The menu strip takes the row's right end.
        let lock = Rect::from_min_size(
            egui::pos2(rect.left() + 18.0, rect.top() + theme::PANEL_INSET),
            Vec2::new(rect.width() - 18.0, theme::BAR_H),
        );
        lockup(ui, lock);
        self.foot(ui, rect);

        // The rows, clipped above the foot so a long list of sessions runs
        // under nothing.
        let list = Rect::from_min_max(
            egui::pos2(rect.left() + 10.0, lock.bottom() + 10.0),
            egui::pos2(rect.right() - 12.0, rect.bottom() - FOOT_H),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        ui.set_clip_rect(list.expand2(Vec2::new(10.0, 0.0)));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.spacing_mut().item_spacing.y = 2.0;

        let linked = self.link.is_open();
        let new_session = nav(&mut ui, icons::plus, "New session", None, linked, false);
        if linked {
            if new_session.clicked() {
                // The daemon owns sessions. This asks for one against the
                // folder the window is looking at, and the reply arrives as
                // an event rather than a return value.
                self.new_session();
            }
        } else {
            new_session.on_hover_text("Sessions are opened by the daemon, and it is not running.");
        }
        let search_key = crate::menu::keys(ui.ctx(), &crate::menu::SC_SEARCH);
        if nav(&mut ui, icons::search, "Search", Some(&search_key), true, self.search.is_some()).clicked() {
            self.search = Some(crate::search::Search::new(String::new()));
        }

        ui.add_space(18.0);
        let (g, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
        theme::label(ui.painter(), egui::pos2(g.left() + 10.0, g.center().y), Align2::LEFT_CENTER, "Sessions");

        // The workspace is the group's head, the way every reference groups
        // threads under the folder they belong to.
        let (f, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), egui::Sense::hover());
        icons::folder(ui.painter(), egui::pos2(f.left() + 18.0, f.center().y), theme::FAINT);
        let name = theme::line(&ui, &self.workspace.name, theme::sans(theme::FS_UI), theme::INK, f.width() - 40.0);
        ui.painter().galley(egui::pos2(f.left() + 34.0, f.center().y - name.size().y / 2.0), name, theme::INK);

        if self.sessions.is_empty() {
            // Two different nothings. A window that has not asked must not
            // report an empty list as a finding.
            let line = match &self.link {
                Link::Open { .. } => "No sessions yet",
                Link::Probing => "Checking",
                Link::Absent { .. } => "Not connected",
            };
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), egui::Sense::hover());
            ui.painter().text(egui::pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, line, theme::sans(theme::FS_UI), theme::FAINTER);
        } else {
            self.session_rows(&mut ui);
        }
    }

    fn session_rows(&mut self, ui: &mut egui::Ui) {
        let active = self.active_session.clone();
        let mut picked: Option<String> = None;
        for session in &self.sessions {
            let is_active = active.as_deref() == Some(session.session_id.as_str());
            let row = ui.allocate_response(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
            let r = row.rect;
            // The chosen session is the brightest fill in the sidebar and a
            // hovered one a step under it, so where you stand is never a
            // guess between two near-blacks.
            let t = theme::hover(ui, row.id, row.hovered());
            if is_active {
                ui.painter().rect_filled(r, CornerRadius::same(7), theme::FACE_2);
            } else if t > 0.0 {
                ui.painter().rect_filled(r, CornerRadius::same(7), theme::FACE.gamma_multiply(t));
            }
            // Named by the prompt its first turn opened with, as the daemon's
            // log recorded it. A session with no turn has none and says so;
            // until the daemon has listed its prompts it goes by when it was
            // opened, never by its id, which waits on the hover.
            let first = self.prompts.as_ref().and_then(|p| p.for_session(&session.session_id));
            let id = &session.session_id;
            let opened = std::time::UNIX_EPOCH + std::time::Duration::from_millis(session.created_at_ms);
            let (text, tone, hint) = match (first, &self.prompts) {
                (Some(t), _) => (
                    crate::prompts::title(&t.prompt),
                    theme::INK,
                    format!("{}\n\n{id}", theme::clip(&t.prompt, 280)),
                ),
                (None, Some(_)) => (
                    "New session".to_string(),
                    if is_active { theme::INK } else { theme::FAINT },
                    format!("No turn has run in this session yet.\n{id}"),
                ),
                (None, None) => (
                    crate::runs::age(Some(opened)).map(|a| format!("Opened {a}")).unwrap_or_else(|| "A session".to_string()),
                    theme::FAINT,
                    match &self.prompts_trouble {
                        Some(e) => format!("The daemon did not list its prompts: {e}\n{id}"),
                        None => format!("Waiting for the daemon to list its prompts.\n{id}"),
                    },
                ),
            };
            let age = crate::runs::short_age(Some(opened)).unwrap_or_default();
            let age_w = if age.is_empty() {
                0.0
            } else {
                ui.painter()
                    .text(egui::pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, &age, theme::sans(theme::FS_META), theme::FAINTER)
                    .width()
                    + 10.0
            };
            let galley = theme::line(ui, &text, theme::sans(theme::FS_UI), tone, r.width() - 44.0 - age_w);
            ui.painter().galley(egui::pos2(r.left() + 34.0, r.center().y - galley.size().y / 2.0), galley, tone);
            let row = row.on_hover_text(hint);
            if row.clicked() {
                picked = Some(session.session_id.clone());
            }
        }
        if let Some(id) = picked {
            self.active_session = Some(id);
        }
    }

    /// Settings, then the readout.
    fn foot(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let settings_key = crate::menu::keys(ui.ctx(), &crate::menu::SC_SETTINGS);
        let at = Rect::from_min_size(egui::pos2(rect.left() + 10.0, rect.bottom() - 84.0), Vec2::new(rect.width() - 22.0, ROW_H));
        let mut foot = ui.new_child(egui::UiBuilder::new().max_rect(at));
        if nav(&mut foot, icons::sliders, "Settings", Some(&settings_key), true, self.settings.is_some()).clicked() {
            self.settings = Some(SettingsPage::General);
        }

        // A readout, not a light. The dot appears only when something is
        // wrong: a green one on a healthy window is colour meaning nothing.
        let (state, detail, trouble) = match &self.link {
            Link::Open { health, .. } => (
                "Daemon connected".to_string(),
                match &health.device_public_key {
                    Some(k) => format!("signs with {}", theme::digest(k)),
                    None => "publishes no signing key".to_string(),
                },
                false,
            ),
            Link::Absent { .. } => ("Daemon not running".to_string(), "receipts are read from disk".to_string(), true),
            Link::Probing => ("Looking for the daemon".to_string(), String::new(), false),
        };
        let x = rect.left() + 20.0;
        let y1 = rect.bottom() - 38.0;
        let y2 = rect.bottom() - 20.0;
        let p = ui.painter();
        let mut tx = x;
        if trouble {
            p.circle_filled(egui::pos2(x + 3.0, y1), 3.0, theme::RUST);
            tx += 13.0;
        }
        p.text(egui::pos2(tx, y1), Align2::LEFT_CENTER, state, theme::sans(theme::FS_META), theme::FAINT);
        p.text(egui::pos2(x, y2), Align2::LEFT_CENTER, detail, theme::sans(theme::FS_META), theme::FAINTER);
        p.text(
            egui::pos2(rect.right() - 16.0, y2),
            Align2::RIGHT_CENTER,
            env!("CARGO_PKG_VERSION"),
            theme::sans(theme::FS_META),
            theme::FAINTER,
        );
        let readout = Rect::from_min_max(egui::pos2(x - 4.0, y1 - 10.0), egui::pos2(rect.right() - 12.0, y2 + 10.0));
        let hint = match &self.link {
            Link::Open { endpoint, health } => match &health.device_public_key {
                Some(k) => format!("vitna-coded, on {endpoint}.\nIt signs receipts with {k}, and every signature on this page is checked against that key."),
                None => format!("vitna-coded, on {endpoint}.\nIt does not say which key it signs with, so no signature here can be checked."),
            },
            Link::Absent { detail, .. } => format!("{detail}\n\nWithout the daemon there is nothing to send to, and no key to check a signature with. The receipts on disk are still read and checked."),
            Link::Probing => "Asking each declared endpoint in turn.".to_string(),
        };
        ui.interact(readout, ui.id().with("readout"), egui::Sense::hover()).on_hover_text(hint);
    }
}

/// The lockup: the mark, the wordmark at the height the site chrome uses, and
/// the product word in the display cut. The wordmark is white ink on
/// transparent, so it takes the ink token as a tint.
fn lockup(ui: &egui::Ui, row: Rect) {
    let cy = row.center().y;
    let mark = Rect::from_center_size(egui::pos2(row.left() + 9.0, cy), Vec2::splat(18.0));
    egui::Image::new(crate::brand::mark()).paint_at(ui, mark);
    let word_w = 20.0 * 376.0 / 104.0;
    let word = Rect::from_min_size(egui::pos2(mark.right() + 8.0, cy - 10.0), Vec2::new(word_w, 20.0));
    egui::Image::new(crate::brand::wordmark()).tint(theme::INK).paint_at(ui, word);
    ui.painter().text(
        egui::pos2(word.right() + 7.0, cy + 1.0),
        Align2::LEFT_CENTER,
        "Code",
        theme::display(theme::FS_TITLE),
        theme::INK,
    );
}

/// A rail row: its icon, its word, and the key that reaches it at the right.
/// A disabled row keeps its place in the quiet ink and says why on hover.
fn nav(ui: &mut egui::Ui, icon: Icon, label: &str, key: Option<&str>, enabled: bool, active: bool) -> egui::Response {
    let resp = ui.allocate_response(Vec2::new(ui.available_width(), ROW_H), egui::Sense::click());
    let r = resp.rect;
    let t = theme::hover(ui, resp.id, enabled && resp.hovered());
    if active {
        ui.painter().rect_filled(r, CornerRadius::same(7), theme::FACE_2);
    } else if t > 0.0 {
        ui.painter().rect_filled(r, CornerRadius::same(7), theme::FACE.gamma_multiply(t));
    }
    let tone = if enabled { theme::INK } else { theme::FAINTER };
    icon(ui.painter(), egui::pos2(r.left() + 18.0, r.center().y), if enabled { theme::FAINT } else { theme::FAINTER });
    ui.painter().text(egui::pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, label, theme::sans(theme::FS_UI), tone);
    if let Some(k) = key {
        ui.painter().text(egui::pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, k, theme::sans(theme::FS_META), theme::FAINTER);
    }
    resp
}
