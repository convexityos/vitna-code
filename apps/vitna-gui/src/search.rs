//! Search: one field over what the window can actually reach.
//!
//! After the Claude desktop app's palette: Ctrl+K, a field, filters, sections
//! and the keys in a footer. What it searches is only what is real here: the
//! runs on disk, the sessions the daemon holds, named by their prompts, and
//! the window's own actions. Claude's Artifacts, Chats, Projects, Tasks and
//! Scheduled filters have nothing behind them in Vitna, so they are not offered.

use eframe::egui::{self, Align2, Color32, CornerRadius, Key, Modifiers, Sense, Stroke, Vec2};

use crate::app::{App, SettingsPage};
use crate::icons;
use crate::theme;

type Icon = fn(&egui::Painter, egui::Pos2, Color32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Runs,
    Sessions,
    Actions,
}

impl Filter {
    const ORDER: [Filter; 4] = [Filter::All, Filter::Runs, Filter::Sessions, Filter::Actions];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Runs => "Runs",
            Filter::Sessions => "Sessions",
            Filter::Actions => "Actions",
        }
    }

    fn step(self, by: i32) -> Filter {
        let i = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0) as i32;
        Self::ORDER[(i + by).rem_euclid(Self::ORDER.len() as i32) as usize]
    }
}

/// The palette while it is open.
pub struct Search {
    pub query: String,
    pub filter: Filter,
    pub selected: usize,
    /// Set when it opens, so the field has the keyboard on its first frame.
    pub focus: bool,
}

impl Search {
    pub fn new(query: String) -> Self {
        Self { query, filter: Filter::All, selected: 0, focus: true }
    }
}

#[derive(Clone)]
enum Target {
    Run(usize),
    Session(String),
    Action(Action),
    Nothing,
}

#[derive(Clone, Copy)]
enum Action {
    NewSession,
    StartScreen,
    Settings(SettingsPage),
    ToggleSidebar,
    Reconnect,
}

struct Item {
    section: &'static str,
    icon: Option<Icon>,
    /// The icon's tone: a run's standing, or quiet ink for the rest.
    tone: Color32,
    title: String,
    /// Right-aligned facts, in order, each in its own tone.
    meta: Vec<(String, Color32)>,
    /// Text the query matches that the row does not print.
    keys: String,
    enabled: bool,
    hint: Option<String>,
    target: Target,
}

impl Item {
    fn matches(&self, q: &str) -> bool {
        q.is_empty()
            || self.title.to_lowercase().contains(q)
            || self.keys.to_lowercase().contains(q)
            || self.meta.iter().any(|(m, _)| m.to_lowercase().contains(q))
    }
}

/// How many of each kind the palette shows before anything is typed.
const BROWSE: usize = 5;

impl App {
    fn search_items(&mut self, ctx: &egui::Context, filter: Filter, query: &str) -> Vec<Item> {
        let q = query.trim().to_lowercase();
        let browsing = q.is_empty() && filter == Filter::All;
        let cap = if browsing { BROWSE } else { usize::MAX };
        let mut out = Vec::new();

        if matches!(filter, Filter::All | Filter::Runs) {
            if let Some(list) = self.run_rows() {
                let mut n = 0;
                for row in list.rows {
                    let files = match row.files {
                        Some(0) => "No files".to_string(),
                        Some(1) => "1 file".to_string(),
                        Some(k) => format!("{k} files"),
                        None => "not a receipt".to_string(),
                    };
                    let (icon, tone) = crate::run_list::status(&row);
                    // The word the ledger gives the row: a failed check,
                    // the signature's included, outranks what the receipt
                    // claims about itself, and only trouble takes a colour.
                    let state = row.standing();
                    let item = Item {
                        section: "Runs",
                        icon: Some(icon),
                        tone,
                        title: row.title.clone(),
                        meta: vec![
                            (files, theme::FAINT),
                            state,
                            (row.age.clone().unwrap_or_else(|| "-".into()), theme::FAINTER),
                        ],
                        keys: format!("{} {} {} {}", row.id, row.sku, row.provider, row.state.1),
                        enabled: row.run.is_some(),
                        hint: row.run.is_none().then(|| format!("Not a receipt: {}", row.state.1)),
                        target: row.run.map(Target::Run).unwrap_or(Target::Nothing),
                    };
                    if n < cap && item.matches(&q) {
                        out.push(item);
                        n += 1;
                    }
                }
            }
        }

        if matches!(filter, Filter::All | Filter::Sessions) {
            let mut n = 0;
            for s in &self.sessions {
                let first = self.prompts.as_ref().and_then(|p| p.for_session(&s.session_id));
                let opened = std::time::UNIX_EPOCH + std::time::Duration::from_millis(s.created_at_ms);
                // Never the id, which the query still matches on.
                let title = match (first, &self.prompts) {
                    (Some(t), _) => crate::prompts::title(&t.prompt),
                    (None, Some(_)) => "New session".to_string(),
                    (None, None) => crate::runs::age(Some(opened))
                        .map(|a| format!("Session opened {a}"))
                        .unwrap_or_else(|| "A session".to_string()),
                };
                let turns = match self.prompts.as_ref().map(|p| p.turns_in(&s.session_id)) {
                    Some(1) => "1 turn".to_string(),
                    Some(k) => format!("{k} turns"),
                    None => "-".to_string(),
                };
                let item = Item {
                    section: "Sessions",
                    icon: Some(icons::bubble),
                    tone: theme::FAINT,
                    title,
                    meta: vec![
                        (turns, theme::FAINT),
                        (crate::runs::short_age(Some(opened)).unwrap_or_else(|| "-".into()), theme::FAINTER),
                    ],
                    keys: format!("{} {}", s.session_id, first.map(|t| t.prompt.as_str()).unwrap_or("")),
                    enabled: true,
                    hint: None,
                    target: Target::Session(s.session_id.clone()),
                };
                if n < cap && item.matches(&q) {
                    out.push(item);
                    n += 1;
                }
            }
        }

        if matches!(filter, Filter::All | Filter::Actions) {
            for item in self.actions(ctx) {
                if item.matches(&q) {
                    out.push(item);
                }
            }
        }
        out
    }
}

impl App {
    /// The window's own actions. Each one does the thing it names, or is
    /// listed disabled with the reason, never offered and then ignored.
    fn actions(&self, ctx: &egui::Context) -> Vec<Item> {
        let linked = self.link.is_open();
        let key = |sc: &egui::KeyboardShortcut| crate::menu::keys(ctx, sc);
        let act = |icon: Icon, title: &str, shortcut: String, keys: &str, target: Action| Item {
            section: "Actions",
            icon: Some(icon),
            tone: theme::FAINT,
            title: title.to_string(),
            meta: if shortcut.is_empty() { vec![] } else { vec![(shortcut, theme::FAINTER)] },
            keys: keys.to_string(),
            enabled: true,
            hint: None,
            target: Target::Action(target),
        };
        let mut out = vec![Item {
            enabled: linked,
            hint: (!linked).then(|| "Sessions are opened by the daemon, and it is not running.".to_string()),
            ..act(icons::plus, "New session", String::new(), "create start open", Action::NewSession)
        }];
        if self.open_run.is_some() {
            out.push(act(icons::chevron_left, "Back to the runs", String::new(), "home start list", Action::StartScreen));
        }
        out.extend([
            act(icons::sliders, "Settings", key(&crate::menu::SC_SETTINGS), "preferences general about", Action::Settings(SettingsPage::General)),
            act(icons::grid, "Model providers", String::new(), "keys anthropic openai connect", Action::Settings(SettingsPage::Providers)),
            act(icons::sparkle, "Models", String::new(), "catalog model pick", Action::Settings(SettingsPage::Models)),
            act(icons::server, "Daemon", String::new(), "vitna-coded pipe endpoint health", Action::Settings(SettingsPage::Daemon)),
            act(icons::keyboard, "Keyboard shortcuts", String::new(), "keys bindings", Action::Settings(SettingsPage::Shortcuts)),
            act(
                icons::sidebar,
                if self.sidebar_open { "Hide sidebar" } else { "Show sidebar" },
                key(&crate::menu::SC_SIDEBAR),
                "toggle panel",
                Action::ToggleSidebar,
            ),
            act(icons::server, "Check for the daemon again", String::new(), "reconnect retry connect", Action::Reconnect),
        ]);
        out
    }

    fn run_target(&mut self, target: Target) {
        match target {
            Target::Run(i) => self.open_run = Some(i),
            Target::Session(id) => {
                self.active_session = Some(id);
                self.open_run = None;
            }
            Target::Action(a) => match a {
                Action::NewSession => self.new_session(),
                Action::StartScreen => self.open_run = None,
                Action::Settings(page) => self.settings = Some(page),
                Action::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
                Action::Reconnect => self.reprobe(),
            },
            Target::Nothing => {}
        }
    }
}

/// The next selectable row from `from`, stepping by `by`, or `from` if none.
fn step_selection(items: &[Item], from: usize, by: i32) -> usize {
    let n = items.len() as i32;
    if n == 0 {
        return 0;
    }
    let mut i = from as i32;
    for _ in 0..n {
        i = (i + by).rem_euclid(n);
        if items[i as usize].enabled {
            return i as usize;
        }
    }
    from
}

fn first_enabled(items: &[Item]) -> usize {
    items.iter().position(|i| i.enabled).unwrap_or(0)
}

impl App {
    pub(crate) fn search_palette(&mut self, ui: &mut egui::Ui) {
        let Some(mut s) = self.search.take() else { return };
        let ctx = ui.ctx().clone();

        // Keys the field would otherwise take, consumed before it is drawn:
        // the arrows move, Enter opens, Tab and Shift+Tab change the filter.
        let (up, down, enter, tab, back) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::SHIFT, Key::Tab),
            )
        });
        if tab || back {
            s.filter = s.filter.step(if back { -1 } else { 1 });
            s.selected = usize::MAX;
        }

        let items = self.search_items(&ctx, s.filter, &s.query);
        if !items.get(s.selected).is_some_and(|i| i.enabled) {
            s.selected = first_enabled(&items);
        }
        if down {
            s.selected = step_selection(&items, s.selected, 1);
        }
        if up {
            s.selected = step_selection(&items, s.selected, -1);
        }
        let keyed = up || down;
        let mut chosen: Option<usize> = enter.then_some(s.selected);

        let screen = ctx.content_rect();
        let w = (screen.width() - 80.0).min(620.0);
        let list_h = (screen.height() * 0.5).clamp(160.0, 380.0);
        let id = egui::Id::new("search");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).anchor(
                Align2::CENTER_TOP,
                Vec2::new(0.0, (screen.height() * 0.12).max(40.0)),
            ))
            .frame(
                egui::Frame::default()
                    .fill(theme::GROUND)
                    .stroke(Stroke::new(1.0, theme::HAIR))
                    .corner_radius(CornerRadius::same(14))
                    .shadow(theme::LIFT),
            )
            .backdrop_color(Color32::from_black_alpha(150))
            .show(&ctx, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                // The field, with the close button at its end.
                let (edit, close) = ui
                    .horizontal(|ui| {
                        ui.set_height(50.0);
                        ui.add_space(14.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        icons::search(ui.painter(), r.center(), theme::FAINT);
                        ui.add_space(10.0);
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut s.query)
                                .id(egui::Id::new("search-field"))
                                .frame(egui::Frame::default())
                                .lock_focus(true)
                                .font(theme::prose(theme::FS_BODY))
                                .text_color(theme::INK)
                                .desired_width(w - 14.0 - 18.0 - 10.0 - 52.0)
                                .hint_text(
                                    egui::RichText::new("Search runs, sessions and actions")
                                        .font(theme::prose(theme::FS_BODY))
                                        .color(theme::FAINTER),
                                ),
                        );
                        let (x, close) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
                        if close.hovered() {
                            ui.painter().rect_filled(x, CornerRadius::same(6), theme::FACE);
                        }
                        icons::close(ui.painter(), x.center(), theme::FAINT);
                        (edit, close.clicked())
                    })
                    .inner;
                if s.focus {
                    edit.request_focus();
                    s.focus = false;
                }
                if edit.changed() {
                    s.selected = usize::MAX;
                }

                // The filters, which name only kinds that exist here.
                ui.horizontal(|ui| {
                    ui.set_height(32.0);
                    ui.add_space(12.0);
                    for f in Filter::ORDER {
                        let on = s.filter == f;
                        let tone = if on { theme::INK } else { theme::FAINT };
                        let g = theme::line(ui, f.label(), theme::sans(theme::FS_UI), tone, 200.0);
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 20.0, 24.0), Sense::click());
                        if on {
                            ui.painter().rect_filled(r, CornerRadius::same(7), theme::FACE);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(7), Color32::from_white_alpha(8));
                        }
                        ui.painter().galley(egui::pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, tone);
                        if resp.clicked() {
                            s.filter = f;
                            s.selected = usize::MAX;
                        }
                        ui.add_space(2.0);
                    }
                });
                rule(ui);

                egui::ScrollArea::vertical()
                    .id_salt("search-results")
                    .max_height(list_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        if items.is_empty() {
                            let q = s.query.trim();
                            let line = if q.is_empty() {
                                "Nothing to show here yet.".to_string()
                            } else {
                                format!("Nothing here matches \"{q}\".")
                            };
                            ui.horizontal(|ui| {
                                ui.set_height(40.0);
                                ui.add_space(22.0);
                                ui.label(egui::RichText::new(line).font(theme::prose(theme::FS_UI)).color(theme::FAINTER));
                            });
                        }
                        let mut section = "";
                        for (i, item) in items.iter().enumerate() {
                            if item.section != section {
                                section = item.section;
                                header(ui, section);
                            }
                            if row(ui, &ctx, item, i, keyed, &mut s.selected) {
                                chosen = Some(i);
                            }
                        }
                        ui.add_space(6.0);
                    });

                rule(ui);
                ui.horizontal(|ui| {
                    ui.set_height(34.0);
                    ui.add_space(14.0);
                    for (k, what) in [("Esc", "Close"), ("Enter", "Open"), ("Tab", "Filter")] {
                        keycap(ui, k);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(what).font(theme::sans(theme::FS_META)).color(theme::FAINT));
                        ui.add_space(16.0);
                    }
                });
                close
            });

        if let Some(item) = chosen.and_then(|i| items.get(i)).filter(|i| i.enabled) {
            self.run_target(item.target.clone());
            return;
        }
        if !(modal.should_close() || modal.inner) {
            self.search = Some(s);
        }
    }
}

/// A section's name over its rows, in words rather than capitals.
fn header(ui: &mut egui::Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    theme::label(ui.painter(), egui::pos2(r.left() + 22.0, r.center().y + 2.0), Align2::LEFT_CENTER, title);
}

/// One result. Returns true when it was clicked.
fn row(ui: &mut egui::Ui, ctx: &egui::Context, item: &Item, i: usize, keyed: bool, selected: &mut usize) -> bool {
    let sense = if item.enabled { Sense::click() } else { Sense::hover() };
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), sense);
    let r2 = r.shrink2(Vec2::new(8.0, 1.0));
    // The pointer takes the selection only when it moves, so a mouse resting
    // over the list does not fight the arrow keys.
    if item.enabled && resp.hovered() && ctx.input(|inp| inp.pointer.delta() != Vec2::ZERO) {
        *selected = i;
    }
    let on = *selected == i;
    if on {
        ui.painter().rect_filled(r2, CornerRadius::same(8), theme::FACE);
        if keyed {
            ui.scroll_to_rect(r, None);
        }
    }
    let mark = egui::pos2(r2.left() + 16.0, r2.center().y);
    if let Some(icon) = item.icon {
        icon(ui.painter(), mark, if item.enabled { item.tone } else { theme::FAINTER });
    }
    // The facts, laid right to left.
    let mut right = r2.right() - 12.0;
    for (text, tone) in item.meta.iter().rev() {
        let g = theme::line(ui, text, theme::sans(theme::FS_META), *tone, 160.0);
        let w = g.size().x;
        ui.painter().galley(egui::pos2(right - w, r2.center().y - g.size().y / 2.0), g, *tone);
        right -= w + 12.0;
    }
    // The highlighted row says so with its ground; the ink only separates
    // what can run from what cannot.
    let ink = if item.enabled { theme::INK } else { theme::FAINTER };
    let x = r2.left() + 36.0;
    let tg = theme::line(ui, &item.title, theme::sans(theme::FS_TITLE), ink, (right - x - 8.0).max(0.0));
    ui.painter().galley(egui::pos2(x, r2.center().y - tg.size().y / 2.0), tg, ink);
    let clicked = item.enabled && resp.clicked();
    if let Some(h) = &item.hint {
        let _ = resp.on_hover_text(h);
    }
    clicked
}

fn keycap(ui: &mut egui::Ui, key: &str) {
    let g = theme::line(ui, key, theme::sans(theme::FS_MICRO), theme::FAINT, 100.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 12.0, 18.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(5), theme::FACE);
    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
    ui.painter().galley(egui::pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, theme::FAINT);
}

fn rule(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, theme::HAIR_2));
}
