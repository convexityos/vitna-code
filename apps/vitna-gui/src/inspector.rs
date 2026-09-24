//! The receipt, beside the run that was read from it.
//!
//! The centre is the reading; this column is the record it was made from. It
//! used to carry three tabs, Receipt, Changes and Evidence, and two of them
//! said again what the centre says, so it is one column now: the receipt's
//! fields as a ledger, then the receipt itself. The fields are the receipt's
//! own vocabulary (run ids, digests, the signature), which is why they live
//! here and not on the page: a name reads, an id identifies. Everything the
//! page says is derived from what is printed at the foot of this column, so
//! it has to be readable here, or the rest is only this window asserting.

use eframe::egui::{self, CornerRadius, Rect, RichText, Stroke, Vec2};

use crate::app::App;
use crate::theme;

pub(crate) const WIDTH: f32 = 380.0;

/// One field: its name, what the column prints, the whole of it for the hover
/// and the copy, and a note when the bare value would say more than it can.
struct Field {
    key: &'static str,
    shown: String,
    whole: String,
    note: Option<&'static str>,
}

impl App {
    pub(crate) fn inspector(&mut self, ui: &mut egui::Ui, rect: Rect) {
        // A column of the panel rather than a second ground: its own hairline,
        // the panel's floor.
        ui.painter().vline(rect.left(), rect.y_range(), Stroke::new(1.0, theme::HAIR_2));

        let inner = Rect::from_min_max(
            egui::pos2(rect.left() + 20.0, rect.top() + 20.0),
            egui::pos2(rect.right() - 14.0, rect.bottom() - 12.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.set_clip_rect(rect);
        // Leave the scrollbar its lane, or a long value reaches under it.
        ui.set_max_width(inner.width() - 10.0);

        let Some(run) = self.current_run() else {
            return;
        };
        let r = &run.receipt;
        let whole = |s: &str| s.to_string();
        let fields = vec![
            Field { key: "schema", shown: r.schema_version.clone(), whole: whole(&r.schema_version), note: None },
            Field { key: "run", shown: r.run_id.clone(), whole: whole(&r.run_id), note: None },
            Field { key: "session", shown: r.session_id.clone(), whole: whole(&r.session_id), note: None },
            Field {
                key: "model",
                shown: format!("{} via {}", r.model_selection.model_sku, r.model_selection.provider),
                whole: format!("{} via {}", r.model_selection.model_sku, r.model_selection.provider),
                note: None,
            },
            Field { key: "routing", shown: r.model_selection.routing_reason.clone(), whole: whole(&r.model_selection.routing_reason), note: None },
            Field {
                key: "isolation",
                shown: r.isolation_label.clone(),
                whole: whole(&r.isolation_label),
                note: Some("As the receipt records it. This window does not check how a run was isolated."),
            },
            Field {
                key: "base",
                shown: r.base_commit_sha.chars().take(12).collect(),
                whole: whole(&r.base_commit_sha),
                note: None,
            },
            Field {
                key: "chain root",
                shown: theme::digest(&r.event_hash_chain_root),
                whole: whole(&r.event_hash_chain_root),
                note: Some("The root of the run's event hash chain, as recorded. This window holds no events to recompute it from."),
            },
            Field {
                key: "workspace",
                shown: theme::digest(&r.workspace_fingerprint),
                whole: whole(&r.workspace_fingerprint),
                note: None,
            },
            Field {
                key: "diff",
                shown: theme::digest(&r.changeset.diff_digest),
                whole: whole(&r.changeset.diff_digest),
                note: Some("A digest of the diff, not the diff. It proves a diff was not altered; it cannot show you one."),
            },
            Field {
                key: "statements",
                shown: r.runner_execution_statements.len().to_string(),
                whole: r.runner_execution_statements.len().to_string(),
                note: None,
            },
            Field {
                key: "sub-runs",
                shown: r.child_receipt_roots.len().to_string(),
                whole: r.child_receipt_roots.join("\n"),
                note: None,
            },
            Field {
                key: "signature",
                shown: if r.device_signature.is_empty() { "none".to_string() } else { theme::digest(&r.device_signature) },
                whole: whole(&r.device_signature),
                note: None,
            },
        ];
        let file = run.path.display().to_string();
        let name = run
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file.clone());
        let json = serde_json::to_string_pretty(&*run.receipt)
            .unwrap_or_else(|e| format!("the receipt would not re-serialize: {e}"));

        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("The receipt").font(theme::display(theme::FS_TITLE)).color(theme::INK));
                ui.add_space(3.0);
                ui.label(RichText::new("As written to disk, and read back here").font(theme::sans(theme::FS_META)).color(theme::FAINTER));
                ui.add_space(14.0);

                for f in &fields {
                    field(ui, f);
                }
                field(ui, &Field { key: "file", shown: file.clone(), whole: file.clone(), note: None });

                ui.add_space(20.0);
                plate(ui, &name, &json);
                ui.add_space(20.0);
            });
    }
}

/// One line of the ledger: the name in the quiet ink, the value beside it,
/// and the whole value with any note on the hover.
fn field(ui: &mut egui::Ui, f: &Field) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), egui::Sense::hover());
    let p = ui.painter();
    p.text(egui::pos2(r.left(), r.center().y), egui::Align2::LEFT_CENTER, f.key, theme::sans(theme::FS_META), theme::FAINTER);
    let x = r.left() + 92.0;
    let g = theme::line(ui, &f.shown, theme::sans(theme::FS_META), theme::FAINT, (r.right() - x).max(0.0));
    p.galley(egui::pos2(x, r.center().y - g.size().y / 2.0), g, theme::FAINT);
    let mut hint = f.whole.clone();
    if let Some(n) = f.note {
        hint.push_str("\n\n");
        hint.push_str(n);
    }
    if !hint.is_empty() {
        resp.on_hover_text(hint);
    }
}

/// The receipt itself, on a plate sunk into the panel: its file's name and a
/// copy over the top, then the text. Keys are set a step quieter than their
/// values so the eye runs down the values; no colour is spent on syntax,
/// since colour here carries state and nothing else.
fn plate(ui: &mut egui::Ui, name: &str, json: &str) {
    egui::Frame::default()
        .fill(theme::RAIL)
        .stroke(Stroke::new(1.0, theme::HAIR_2))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin { left: 12, right: 10, top: 8, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).font(theme::sans(theme::FS_META)).color(theme::FAINT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let copy = ui.add(
                        egui::Button::new(RichText::new("Copy").font(theme::sans(theme::FS_META)).color(theme::PERI_2))
                            .fill(egui::Color32::TRANSPARENT)
                            .frame(false),
                    );
                    if copy.clicked() {
                        ui.ctx().copy_text(json.to_string());
                    }
                    copy.on_hover_text("Copies the receipt as this window re-serialized it from the file.");
                });
            });
            ui.add_space(6.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, theme::HAIR_2));
            ui.add_space(8.0);
            ui.add(egui::Label::new(json_job(json)).wrap());
        });
}

/// The receipt's text as a layout job: each key a step quieter than the value
/// it names, and the brackets quieter still.
fn json_job(json: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let fmt = |c| egui::TextFormat { font_id: theme::sans(theme::FS_META), color: c, ..Default::default() };
    for line in json.lines() {
        let body = line.trim_start();
        let indent = &line[..line.len() - body.len()];
        job.append(indent, 0.0, fmt(theme::FAINTER));
        match body.split_once("\": ") {
            Some((key, value)) if body.starts_with('"') => {
                job.append(&format!("{key}\":"), 0.0, fmt(theme::FAINTER));
                job.append(&format!(" {value}"), 0.0, fmt(theme::FAINT));
            }
            _ => {
                let bracket = body.chars().all(|c| "{}[],".contains(c));
                job.append(body, 0.0, fmt(if bracket { theme::FAINTER } else { theme::FAINT }));
            }
        }
        job.append("\n", 0.0, fmt(theme::FAINTER));
    }
    job
}
