//! Recent runs: the content of the start screen's centre.
//!
//! After Cursor's agent home, one row per run: a small card holding how many
//! files the run changed and how it ended, and beside it a title and one line
//! of facts. Everything in a row is read off the receipt on disk except the
//! title, which is the run's prompt when the daemon's log has one and the run
//! id when it does not. A file in the receipts directory that will not parse
//! gets a row of its own, where its timestamp puts it, rather than vanishing.

use std::time::SystemTime;

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

use crate::app::App;
use crate::link::Link;
use crate::runs;
use crate::theme;

const ROW_H: f32 = 58.0;
const CARD_W: f32 = 124.0;
const CARD_H: f32 = 44.0;

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
    pub provider: String,
    pub sku: String,
    pub age: Option<String>,
    pub age_long: Option<String>,
    pub failed_checks: Vec<String>,
    pub written: Option<SystemTime>,
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
        let waiting = match (&self.link, &self.prompts_trouble) {
            (_, Some(e)) => format!("The daemon did not list its prompts: {e}"),
            (Link::Open { .. }, None) => "Waiting for the daemon to list its prompts.".to_string(),
            _ => "Prompts come from the daemon's event log, and it is not connected.".to_string(),
        };
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
                provider: run.receipt.model_selection.provider.clone(),
                sku: run.receipt.model_selection.model_sku.clone(),
                age: runs::short_age(run.written),
                age_long: runs::age(run.written),
                failed_checks: run.report.errors.clone(),
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
                provider: String::new(),
                sku: String::new(),
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
                    .font(theme::prose(12.5))
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
            theme::sans(11.5),
            theme::FAINTER,
        );
        ui.painter().text(
            egui::pos2(h.right() - 6.0, h.center().y),
            egui::Align2::RIGHT_CENTER,
            ".vitna/receipts",
            theme::sans(11.5),
            theme::FAINTER,
        );

        if let Some(t) = &list.trouble {
            ui.label(
                egui::RichText::new(format!("The receipts could not be listed: {t}"))
                    .font(theme::prose(12.5))
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
fn paint_row(ui: &mut egui::Ui, row: &Row) -> Option<usize> {
    let sense = if row.run.is_some() { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    let p = ui.painter().clone();
    let hot = resp.hovered() && row.run.is_some();
    if hot {
        p.rect_filled(rect, CornerRadius::same(10), theme::FACE);
    }

    // The card: what the run changed, over how it ended.
    let card = Rect::from_min_size(
        rect.min + Vec2::new(6.0, (ROW_H - CARD_H) / 2.0),
        Vec2::new(CARD_W, CARD_H),
    );
    p.rect_filled(card, CornerRadius::same(8), theme::GROUND);
    p.rect_stroke(
        card,
        CornerRadius::same(8),
        Stroke::new(1.0, theme::HAIR_2),
        egui::StrokeKind::Inside,
    );
    let files = match row.files {
        Some(0) => "No files".to_string(),
        Some(1) => "1 file".to_string(),
        Some(n) => format!("{n} files"),
        None => "Not a receipt".to_string(),
    };
    p.text(
        card.left_top() + Vec2::new(10.0, 13.0),
        egui::Align2::LEFT_CENTER,
        files,
        theme::sans(12.0),
        theme::INK_2,
    );
    let (word, full, tone) = &row.state;
    let g = theme::line(ui, word, theme::sans(11.5), *tone, CARD_W - 34.0);
    let pill = Rect::from_min_size(
        card.left_top() + Vec2::new(7.0, 23.0),
        Vec2::new(g.size().x + 23.0, 16.0),
    );
    p.rect_filled(pill, CornerRadius::same(8), tone.gamma_multiply(0.16));
    p.circle_filled(egui::pos2(pill.left() + 8.5, pill.center().y), 2.5, *tone);
    p.galley(
        egui::pos2(pill.left() + 15.0, pill.center().y - g.size().y / 2.0),
        g,
        *tone,
    );

    // Beside it: the title, then one line of facts.
    let x = card.right() + 14.0;
    let w = (rect.right() - x - 10.0).max(0.0);
    let (font, ink) = if row.titled {
        (theme::sans(13.5), theme::INK)
    } else {
        (theme::sans(13.0), theme::MUTE)
    };
    let tg = theme::line(ui, &row.title, font, ink, w);
    p.galley(egui::pos2(x, rect.center().y - 10.0 - tg.size().y / 2.0), tg, ink);

    let my = rect.center().y + 10.0;
    let mut cx = x;
    if row.run.is_some() {
        // The model the receipt says served the run, under its provider's mark.
        crate::composer::provider_badge(ui, egui::pos2(cx + 6.0, my), 12.0, &row.provider, &row.provider);
        cx += 18.0;
        let sg = theme::line(ui, &row.sku, theme::sans(12.0), theme::FAINT, w * 0.5);
        let sw = sg.size().x;
        p.galley(egui::pos2(cx, my - sg.size().y / 2.0), sg, theme::FAINT);
        cx += sw + 14.0;
    }
    if let Some(a) = &row.age {
        let ag = theme::line(ui, a, theme::sans(12.0), theme::FAINTER, 60.0);
        let aw = ag.size().x;
        p.galley(egui::pos2(cx, my - ag.size().y / 2.0), ag, theme::FAINTER);
        cx += aw + 14.0;
    }
    if row.run.is_none() {
        // A file that is not a receipt says why, in place of a model.
        let rg = theme::line(ui, full, theme::sans(12.0), theme::FAINT, (rect.right() - cx - 10.0).max(0.0));
        p.galley(egui::pos2(cx, my - rg.size().y / 2.0), rg, theme::FAINT);
    } else if !row.failed_checks.is_empty() {
        // A failed check is the loudest thing a row can say.
        p.circle_filled(egui::pos2(cx + 3.0, my), 3.0, theme::RUST);
        p.text(
            egui::pos2(cx + 11.0, my),
            egui::Align2::LEFT_CENTER,
            "Check failed",
            theme::sans(12.0),
            theme::RUST,
        );
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
