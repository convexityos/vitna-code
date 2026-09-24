//! The runs in this folder: the start screen's ledger.
//!
//! One row per receipt on a column grid the head and every row share, ruled
//! rather than carded, the way the owner's new design lays a feed: the run and
//! what became of it, the model that served it, the chain of checks this
//! window ran on it, and when it was written. Everything in a row is read off
//! the receipt on disk, except the title when the daemon's log holds the
//! prompt. Without one, a run is named by what it changed, which the receipt
//! does carry, and never by its id: `run-1789700058624` is the receipt's
//! vocabulary rather than a reader's, and it waits on the hover and in the
//! inspector. A file in the receipts directory that will not parse keeps a
//! row of its own, where its timestamp puts it, rather than vanishing.

use std::time::SystemTime;

use eframe::egui::{self, Align2, Color32, Sense, Stroke, Vec2};

use crate::app::App;
use crate::checks::{self, Chain};
use crate::link::Link;
use crate::parts;
use crate::runs;
use crate::theme;

const ROW_H: f32 = 54.0;

/// A completion state in words: a short one for the row, the full phrase for
/// the head of a run, and a tone. The phrases are `apps/vitna-desktop`'s, so
/// the surfaces cannot name one state two ways.
pub(crate) fn completion(state: &str) -> (&'static str, String, Color32) {
    match state {
        "completed_with_evidence" => ("Completed", "Completed, with evidence".into(), theme::OK),
        "completed_with_unknowns" => {
            ("With unknowns", "Completed, with unknowns stated".into(), theme::RUST)
        }
        "blocked" => ("Blocked", "Blocked".into(), theme::RUST),
        "failed" => ("Failed", "Failed".into(), theme::RUST),
        "cancelled" => ("Cancelled", "Cancelled".into(), theme::FAINT),
        "needs_reconciliation" => ("Reconcile", "Needs reconciliation".into(), theme::RUST),
        "stopped_at_round_limit" => (
            "Round limit",
            "Stopped at the round limit, before the model concluded".into(),
            theme::RUST,
        ),
        other => ("Unknown", format!("Unknown completion state: {other}"), theme::RUST),
    }
}

/// How a completion state finishes a sentence about a run.
fn outcome(state: &str) -> String {
    match state {
        "completed_with_evidence" => "completed with evidence".into(),
        "completed_with_unknowns" => "completed with unknowns stated".into(),
        "blocked" => "was blocked".into(),
        "failed" => "failed".into(),
        "cancelled" => "was cancelled".into(),
        "needs_reconciliation" => "needs reconciliation".into(),
        "stopped_at_round_limit" => "stopped at the round limit".into(),
        other => format!("ended in a state this window does not know, {other}"),
    }
}

/// A run named by what it changed: the receipt's own changeset, stated.
pub(crate) fn changed_title(paths: &[&str]) -> String {
    match paths {
        [] => "Changed no files".to_string(),
        [one] => format!("Changed {one}"),
        [first, rest @ ..] => format!("Changed {first} and {} more", rest.len()),
    }
}

/// Everything one row paints, copied off the ledger so that painting holds
/// no borrow on it. Search reads the same rows.
pub(crate) struct Row {
    /// Index into the ledger's runs, or None for a file that would not parse.
    pub run: Option<usize>,
    /// The run id, or the file's name when it is not a receipt.
    pub id: String,
    pub title: String,
    pub title_hint: String,
    /// Files the run changed; None for a file that is not a receipt.
    pub files: Option<usize>,
    pub state: (&'static str, String, Color32),
    /// The receipt's completion state as written, for the status icon.
    pub state_key: String,
    pub provider: String,
    pub sku: String,
    /// The catalog's name for the sku, or the sku itself when the pinned
    /// catalog does not list it.
    pub model: String,
    pub age: Option<String>,
    pub age_long: Option<String>,
    /// The three checks this window ran, and their answers.
    pub chain: Chain,
    pub written: Option<SystemTime>,
}

impl Row {
    pub(crate) fn failed(&self) -> bool {
        checks::any_failed(&self.chain)
    }

    /// The words for how the run stands: a failed check, the signature's
    /// included, outranks whatever the receipt claims about itself.
    pub(crate) fn standing(&self) -> (String, Color32) {
        if self.run.is_none() {
            ("Not a receipt".to_string(), theme::RUST)
        } else if self.failed() {
            ("A check failed".to_string(), theme::RUST)
        } else {
            (self.state.0.to_string(), theme::FAINT)
        }
    }
}

/// The icon a row's standing is drawn with, and its tone. A failed check
/// and a file that is not a receipt outrank whatever the receipt claims.
pub(crate) fn status(row: &Row) -> (fn(&egui::Painter, egui::Pos2, Color32), Color32) {
    if row.run.is_none() || row.failed() {
        return (crate::icons::alert, theme::RUST);
    }
    match row.state_key.as_str() {
        "completed_with_evidence" => (crate::icons::done, theme::OK),
        "failed" => (crate::icons::failed, theme::RUST),
        "cancelled" => (crate::icons::failed, theme::FAINT),
        _ => (crate::icons::alert, theme::RUST),
    }
}

pub(crate) struct Rows {
    pub rows: Vec<Row>,
    /// Why the receipts directory could not be listed, if it could not.
    pub trouble: Option<String>,
}

impl App {
    /// The ledger as rows, newest first, or None while it is still being read.
    pub(crate) fn run_rows(&mut self) -> Option<Rows> {
        let prompts = self.prompts.as_ref();
        let choices = &self.choices;
        let waiting = match (&self.link, &self.prompts_trouble) {
            (_, Some(e)) => format!("The daemon did not list its prompts ({e}), so it is named by what it changed."),
            (Link::Open { .. }, None) => {
                "The daemon has not listed its prompts yet, so it is named by what it changed.".to_string()
            }
            _ => "Prompts come from the daemon's event log, and no daemon is connected, so it is named by what it changed."
                .to_string(),
        };
        let (key, no_key_because) = crate::run_view::device_key(&self.link);
        let ledger = match self.runs.poll() {
            runs::Loading::Done(l) => l,
            runs::Loading::Reading => return None,
        };

        let mut rows = Vec::new();
        for (i, run) in ledger.runs.iter().enumerate() {
            let id = &run.receipt.run_id;
            let paths: Vec<&str> = run.receipt.changeset.files_modified.iter().map(|f| f.path.as_str()).collect();
            let (title, why) = match prompts {
                Some(p) => match (p.for_run(id), p.unreadable(id)) {
                    (Some(t), _) => (
                        crate::prompts::title(&t.prompt),
                        format!(
                            "{}\n\nThe prompt, from the daemon's event log. The receipt does not carry one.",
                            theme::clip(&t.prompt, 280)
                        ),
                    ),
                    (None, Some(why)) => (
                        changed_title(&paths),
                        format!("The daemon could not read this run's prompt back ({why}), so it is named by what it changed."),
                    ),
                    (None, None) => (
                        changed_title(&paths),
                        "The daemon's event log has no turn for this run, so it is named by what it changed.".to_string(),
                    ),
                },
                None => (changed_title(&paths), waiting.clone()),
            };
            rows.push(Row {
                run: Some(i),
                id: id.clone(),
                title,
                title_hint: format!("{why}\n\n{id}, in session {}", run.receipt.session_id),
                files: Some(paths.len()),
                state: completion(&run.receipt.completion_state),
                state_key: run.receipt.completion_state.clone(),
                provider: run.receipt.model_selection.provider.clone(),
                sku: run.receipt.model_selection.model_sku.clone(),
                model: {
                    let m = &run.receipt.model_selection;
                    choices
                        .iter()
                        .find(|c| c.sku == m.model_sku && c.provider_id == m.provider)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| m.model_sku.clone())
                },
                age: runs::short_age(run.written),
                age_long: runs::age(run.written),
                chain: checks::for_run(run, key.as_deref(), no_key_because),
                written: run.written,
            });
        }
        for u in &ledger.unreadable {
            let name = u
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| u.path.display().to_string());
            rows.push(Row {
                run: None,
                id: name.clone(),
                title: name,
                title_hint: u.path.display().to_string(),
                files: None,
                state: ("Unreadable", u.reason.clone(), theme::RUST),
                state_key: String::new(),
                provider: String::new(),
                sku: String::new(),
                model: String::new(),
                age: runs::short_age(u.written),
                age_long: runs::age(u.written),
                chain: checks::for_unreadable(&u.reason),
                written: u.written,
            });
        }
        // A broken file sits where its time puts it, not at the bottom.
        rows.sort_by(|a, b| runs::newest_first(a.written, b.written));

        Some(Rows {
            rows,
            trouble: ledger.trouble.clone(),
        })
    }
}

/// The start screen's headline: one sentence a reader came for, rather than
/// a greeting. The ladder puts a failed check first, because a receipt that
/// does not hold is the most important thing in the directory, and otherwise
/// says what the newest run did and how it ended. None when there is nothing
/// to say it about.
pub(crate) fn headline(rows: &[Row]) -> Option<String> {
    let first = rows.first()?;
    let failing = rows.iter().filter(|r| r.failed()).count();
    let n = rows.len();
    if failing > 0 {
        return Some(match (failing, n) {
            (1, 1) => "The receipt here failed a check.".to_string(),
            (1, _) => format!("One of the {n} receipts here failed a check."),
            (f, _) if f == n => format!("All {n} receipts here failed a check."),
            (f, _) => format!("{f} of the {n} receipts here failed a check."),
        });
    }
    let changed = match first.files {
        Some(0) | None => "changed no files".to_string(),
        Some(1) => "changed one file".to_string(),
        Some(k) => format!("changed {k} files"),
    };
    let when = first.age_long.as_ref().map(|a| format!(", {a}")).unwrap_or_default();
    Some(format!("The last run here {changed} and {}{when}.", outcome(&first.state_key)))
}

/// The line under the headline that says where the ledger came from and who
/// checked it, the way the Vitna record prints its provenance under a title.
pub(crate) fn provenance(rows: &[Row], key: Option<&str>) -> Vec<(String, Color32)> {
    let n = rows.len();
    let mut out = vec![
        (
            format!("{n} receipt{} in .vitna/receipts", if n == 1 { "" } else { "s" }),
            theme::FAINT,
        ),
        ("checked by this window, not the daemon".to_string(), theme::FAINTER),
    ];
    out.push(match key {
        Some(k) => (format!("signatures checked against {}", theme::digest(k)), theme::FAINTER),
        None => ("signatures wait on the daemon's key".to_string(), theme::FAINTER),
    });
    out
}

/// Where each column starts, off the ledger's width. The model column is the
/// first to go when the window narrows, since the row's own hover still
/// names it.
struct Cols {
    title: f32,
    title_right: f32,
    model: Option<f32>,
    checks: f32,
    age_right: f32,
}

impl Cols {
    fn for_rect(left: f32, right: f32) -> Self {
        let age_right = right - 14.0;
        let checks = age_right - 52.0 - 28.0 - 56.0;
        let model = (right - left >= 600.0).then_some(checks - 28.0 - 160.0);
        Cols {
            title: left + 40.0,
            title_right: model.unwrap_or(checks) - 24.0,
            model,
            checks,
            age_right,
        }
    }
}

impl App {
    /// The ledger, under the start screen's headline. Clicking a run opens it.
    pub(crate) fn run_list(&mut self, ui: &mut egui::Ui) {
        let Some(list) = self.run_rows() else {
            return;
        };
        if list.rows.is_empty() && list.trouble.is_none() {
            return;
        }

        // The head: each column named once, over a rule that runs the width.
        let (h, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
        let cols = Cols::for_rect(h.left(), h.right());
        let y = h.center().y;
        let p = ui.painter();
        theme::label(p, egui::pos2(cols.title, y), Align2::LEFT_CENTER, "Run");
        if let Some(m) = cols.model {
            theme::label(p, egui::pos2(m, y), Align2::LEFT_CENTER, "Model");
        }
        theme::label(p, egui::pos2(cols.checks, y), Align2::LEFT_CENTER, "Checks");
        theme::label(p, egui::pos2(cols.age_right, y), Align2::RIGHT_CENTER, "Written");
        parts::rule(ui, theme::HAIR);

        if let Some(t) = &list.trouble {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("The receipts could not be listed: {t}"))
                    .font(theme::prose(theme::FS_UI))
                    .color(theme::RUST),
            );
        }

        let mut open: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("run_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let n = list.rows.len();
                for (i, row) in list.rows.iter().enumerate() {
                    if let Some(k) = paint_row(ui, row, i + 1 < n) {
                        open = Some(k);
                    }
                }
            });
        if let Some(i) = open {
            self.open_run = Some(i);
        }
    }
}

/// One row. Returns the run's index when it was clicked.
///
/// The status icon and the title on the first line, with the model, the chain
/// and the age level with it in their columns; under the title, one quiet
/// line of how the run stands and what it touched. No card and no tinted
/// pill: the ground under the pointer says which row it is on, and a
/// hairline between rows is all the ledger needs to be read as one.
fn paint_row(ui: &mut egui::Ui, row: &Row, ruled: bool) -> Option<usize> {
    let sense = if row.run.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    let ui: &egui::Ui = ui;
    let hot = resp.hovered() && row.run.is_some();
    parts::hover_ground(ui, rect.shrink2(Vec2::new(0.0, 2.0)), resp.id, hot);
    if ruled {
        ui.painter()
            .hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::HAIR_2));
    }
    let p = ui.painter().clone();
    let cols = Cols::for_rect(rect.left(), rect.right());
    let title_y = rect.center().y - 9.0;
    let meta_y = rect.center().y + 10.0;

    let (icon, tone) = status(row);
    icon(&p, egui::pos2(rect.left() + 18.0, title_y), tone);

    let tg = theme::line(ui, &row.title, theme::sans(theme::FS_TITLE), theme::INK, (cols.title_right - cols.title).max(0.0));
    let title_rect = egui::Rect::from_min_size(egui::pos2(cols.title, title_y - tg.size().y / 2.0), tg.size());
    p.galley(title_rect.min, tg, theme::INK);

    let (standing, standing_tone) = row.standing();
    let files = match row.files {
        Some(0) => "no files".to_string(),
        Some(1) => "1 file".to_string(),
        Some(n) => format!("{n} files"),
        None => String::new(),
    };
    let detail = if row.run.is_none() { row.state.1.clone() } else { files };
    parts::facts(
        ui,
        egui::pos2(cols.title, meta_y),
        &[(standing, standing_tone), (detail, theme::FAINTER)],
        cols.title_right - cols.title,
    );

    if let Some(m) = cols.model {
        let g = theme::line(ui, &row.model, theme::sans(theme::FS_UI), theme::FAINT, cols.checks - m - 28.0);
        p.galley(egui::pos2(m, title_y - g.size().y / 2.0), g, theme::FAINT);
    }

    let rail_at = egui::pos2(cols.checks + 4.0, title_y);
    checks::rail(&p, rail_at, &row.chain);

    if let Some(a) = &row.age {
        p.text(egui::pos2(cols.age_right, title_y), Align2::RIGHT_CENTER, a, theme::sans(theme::FS_META), theme::FAINTER);
    }

    // The chain says each of its links in words on its own hover; the rest
    // of the row says where its title came from and what it records.
    let rail_hover = ui.interact(checks::rail_rect(rail_at), resp.id.with("chain"), Sense::hover());
    rail_hover.on_hover_text(checks::hint(&row.chain));

    let mut hint = row.title_hint.clone();
    hint.push_str(&format!("\n\n{}", row.state.1));
    if row.run.is_some() {
        hint.push_str(&format!("\n{} via {}, as the receipt records it", row.sku, row.provider));
    }
    if let Some(a) = &row.age_long {
        hint.push_str(&format!("\nWritten {a}"));
    }
    let resp = resp.on_hover_text(hint);
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        row.run
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_named_by_what_it_changed() {
        assert_eq!(changed_title(&[]), "Changed no files");
        assert_eq!(changed_title(&["src/main.rs"]), "Changed src/main.rs");
        assert_eq!(changed_title(&["a.md", "b.md", "c.md"]), "Changed a.md and 2 more");
    }

    fn row(state: &str, files: usize, age: &str, chain: Chain) -> Row {
        Row {
            run: Some(0),
            id: "run-1789700058624".into(),
            title: changed_title(&[]),
            title_hint: String::new(),
            files: Some(files),
            state: completion(state),
            state_key: state.into(),
            provider: "anthropic".into(),
            sku: "m".into(),
            model: "m".into(),
            age: Some("5d".into()),
            age_long: Some(age.into()),
            chain,
            written: None,
        }
    }

    fn held() -> Chain {
        checks::for_unreadable("x").map(|mut s| {
            s.node = checks::Node::Held;
            s
        })
    }

    /// The headline is a sentence about the work, never a greeting and never
    /// an id, and a failed check outranks the newest run.
    #[test]
    fn the_headline_leads_with_a_failed_check_and_otherwise_the_last_run() {
        let ok = row("stopped_at_round_limit", 0, "5 days ago", held());
        assert_eq!(
            headline(&[ok]).as_deref(),
            Some("The last run here changed no files and stopped at the round limit, 5 days ago.")
        );
        let two = row("completed_with_evidence", 2, "1 hour ago", held());
        assert_eq!(
            headline(&[two]).as_deref(),
            Some("The last run here changed 2 files and completed with evidence, 1 hour ago.")
        );
        let bad = row("completed_with_evidence", 1, "now", checks::for_unreadable("x"));
        let fine = row("completed_with_evidence", 1, "now", held());
        assert_eq!(
            headline(&[fine, bad]).as_deref(),
            Some("One of the 2 receipts here failed a check.")
        );
        assert_eq!(headline(&[]), None);
        let h = headline(&[row("failed", 3, "now", held())]).unwrap();
        assert!(!h.contains("run-"), "{h}");
    }
}
