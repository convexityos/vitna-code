//! Recent runs: the content of the start screen's centre.
//!
//! After Cursor's agent home, one row per run: a status icon, the title, and
//! one quiet line of facts under it. Everything in a row is read off the receipt on disk except the
//! title, which is the run's prompt when the daemon's log has one and the run
//! id when it does not. A file in the receipts directory that will not parse
//! gets a row of its own, where its timestamp puts it, rather than vanishing.

use std::time::SystemTime;

use eframe::egui::{self, Color32, CornerRadius, Sense, Vec2};

use crate::app::App;
use crate::link::Link;
use crate::runs;
use crate::theme;

const ROW_H: f32 = 52.0;

/// A completion state in words: a short one for the card, the full phrase for
/// the hover, and a tone. The phrases are `apps/vitna-desktop`'s, so the two
/// surfaces cannot name one state two ways.
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

/// Everything one row paints, copied off the ledger so that painting holds
/// no borrow on it. Search reads the same rows.
pub(crate) struct Row {
    /// Index into the ledger's runs, or None for a file that would not parse.
    pub run: Option<usize>,
    /// The run id, or the file's name when it is not a receipt.
    pub id: String,
    pub title: String,
    /// True when the title is the prompt, not a stand-in for one.
    pub titled: bool,
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
    pub failed_checks: Vec<String>,
    pub written: Option<SystemTime>,
}

/// The icon a row's standing is drawn with, and its tone. A failed check
/// and a file that is not a receipt outrank whatever the receipt claims.
pub(crate) fn status(row: &Row) -> (fn(&egui::Painter, egui::Pos2, Color32), Color32) {
    if row.run.is_none() || !row.failed_checks.is_empty() {
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
            (_, Some(e)) => format!("The daemon did not list its prompts: {e}"),
            (Link::Open { .. }, None) => "Waiting for the daemon to list its prompts.".to_string(),
            _ => "Prompts come from the daemon's event log, and it is not connected.".to_string(),
        };
        let (key, _) = crate::run_view::device_key(&self.link);
        let ledger = match self.runs.poll() {
            runs::Loading::Done(l) => l,
            runs::Loading::Reading => return None,
        };

        let mut rows = Vec::new();
        for (i, run) in ledger.runs.iter().enumerate() {
            let id = &run.receipt.run_id;
            let (title, titled, title_hint) = match prompts {
                Some(p) => match (p.for_run(id), p.unreadable(id)) {
                    (Some(t), _) => (
                        crate::prompts::title(&t.prompt),
                        true,
                        format!(
                            "{}\n\nThe prompt, from the daemon's event log. The receipt does not carry one.",
                            theme::clip(&t.prompt, 280)
                        ),
                    ),
                    (None, Some(why)) => (
                        id.clone(),
                        false,
                        format!("The daemon could not read this run's prompt back: {why}"),
                    ),
                    (None, None) => (
                        id.clone(),
                        false,
                        "The daemon's event log has no turn for this run, so it goes by its id."
                            .to_string(),
                    ),
                },
                None => (id.clone(), false, waiting.clone()),
            };
            rows.push(Row {
                run: Some(i),
                id: id.clone(),
                title,
                titled,
                title_hint,
                files: Some(run.receipt.changeset.files_modified.len()),
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
                // The signature is a check too, once the daemon has published
                // its key, and a row that fails it does not wear a tick.
                failed_checks: {
                    let mut failed = run.report.errors.clone();
                    match run.signature(key.as_deref()) {
                        runs::Signature::Mismatch => failed.push(
                            "the signature does not match the key the connected daemon signs with".to_string(),
                        ),
                        runs::Signature::Absent => failed.push("the receipt carries no signature".to_string()),
                        runs::Signature::Verified | runs::Signature::Unchecked => {}
                    }
                    failed
                },
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
                titled: false,
                title_hint: u.path.display().to_string(),
                files: None,
                state: ("Unreadable", u.reason.clone(), theme::RUST),
                state_key: String::new(),
                provider: String::new(),
                sku: String::new(),
                model: String::new(),
                age: runs::short_age(u.written),
                age_long: runs::age(u.written),
                failed_checks: Vec::new(),
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

impl App {
    /// The list, under the start screen's heading. Clicking a run opens it.
    pub(crate) fn run_list(&mut self, ui: &mut egui::Ui) {
        let Some(list) = self.run_rows() else {
            return;
        };

        if list.rows.is_empty() && list.trouble.is_none() {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(
                        "No runs here yet. A turn's receipt lands in .vitna/receipts and is listed here.",
                    )
                    .font(theme::prose(theme::FS_UI))
                    .color(theme::FAINTER),
                );
            });
            return;
        }

        // The heading: what this is, how many, and where they are read from.
        let (h, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        let eyebrow = ui.painter().layout_job(theme::eyebrow(ui, "Recent runs"));
        let ew = eyebrow.size().x;
        ui.painter().galley(
            egui::pos2(h.left() + 6.0, h.center().y - eyebrow.size().y / 2.0),
            eyebrow,
            theme::FAINTER,
        );
        ui.painter().text(
            egui::pos2(h.left() + 6.0 + ew + 8.0, h.center().y),
            egui::Align2::LEFT_CENTER,
            list.rows.len().to_string(),
            theme::sans(theme::FS_MICRO),
            theme::FAINTER,
        );
        ui.painter().text(
            egui::pos2(h.right() - 6.0, h.center().y),
            egui::Align2::RIGHT_CENTER,
            ".vitna/receipts",
            theme::sans(theme::FS_MICRO),
            theme::FAINTER,
        );

        if let Some(t) = &list.trouble {
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
                ui.spacing_mut().item_spacing.y = 2.0;
                for row in &list.rows {
                    if let Some(i) = paint_row(ui, row) {
                        open = Some(i);
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
/// A status icon on the title's line, the title, the age at the right, and
/// one quiet line under it: how the run ended, what served it, what it
/// changed. No card and no tinted pill: the state is an icon with its word
/// beside it, which is all a list needs to say it.
fn paint_row(ui: &mut egui::Ui, row: &Row) -> Option<usize> {
    let sense = if row.run.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    let ui: &egui::Ui = ui;
    let p = ui.painter().clone();
    let hot = resp.hovered() && row.run.is_some();
    let t = theme::hover(ui, resp.id, hot);
    if t > 0.0 {
        p.rect_filled(rect, CornerRadius::same(10), theme::FACE.gamma_multiply(t));
    }

    let title_y = rect.center().y - 9.0;
    let meta_y = rect.center().y + 10.0;
    let (icon, tone) = status(row);
    icon(&p, egui::pos2(rect.left() + 20.0, title_y), tone);

    // The age, on the title's line at the right.
    let x = rect.left() + 40.0;
    let mut title_right = rect.right() - 14.0;
    if let Some(a) = &row.age {
        let g = theme::line(ui, a, theme::sans(theme::FS_META), theme::FAINTER, 60.0);
        let w = g.size().x;
        p.galley(egui::pos2(title_right - w, title_y - g.size().y / 2.0), g, theme::FAINTER);
        title_right -= w + 16.0;
    }
    // The hover ground says which row the pointer is on; the ink says only
    // whether the row has a prompt to show or falls back to its id.
    let ink = if row.titled { theme::INK } else { theme::FAINT };
    let tg = theme::line(ui, &row.title, theme::sans(theme::FS_TITLE), ink, (title_right - x).max(0.0));
    p.galley(egui::pos2(x, title_y - tg.size().y / 2.0), tg, ink);

    // One quiet line, laid left to right.
    let mut cx = x;
    let mut put = |text: &str, tone: Color32| {
        let g = theme::line(ui, text, theme::sans(theme::FS_META), tone, (rect.right() - 14.0 - cx).max(0.0));
        let w = g.size().x;
        p.galley(egui::pos2(cx, meta_y - g.size().y / 2.0), g, tone);
        cx += w + 14.0;
    };
    let (word, full, _) = &row.state;
    if row.run.is_none() {
        put(full, theme::FAINT);
    } else {
        if row.failed_checks.is_empty() {
            put(word, theme::FAINT);
        } else {
            put("Check failed", theme::RUST);
        }
        put(&row.model, theme::FAINT);
        let files = match row.files {
            Some(0) => "No files".to_string(),
            Some(1) => "1 file".to_string(),
            Some(n) => format!("{n} files"),
            None => String::new(),
        };
        put(&files, theme::FAINTER);
    }

    let mut hint = row.title_hint.clone();
    hint.push_str(&format!("\n\n{full}"));
    if row.run.is_some() {
        hint.push_str(&format!("\n{} via {}, as the receipt records it", row.sku, row.provider));
    }
    if let Some(a) = &row.age_long {
        hint.push_str(&format!("\nWritten {a}"));
    }
    for e in &row.failed_checks {
        hint.push_str(&format!("\nCheck failed: {e}"));
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
