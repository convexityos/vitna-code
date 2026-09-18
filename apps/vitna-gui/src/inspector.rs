//! The panel beside the run.
//!
//! Cursor puts five tabs here: Setup, Secrets, Git, Desktop, Terminal. This has
//! three, and the count is the point. A Secrets tab would be a lie, because the
//! daemon reads credentials out of its environment and cannot store one;
//! Terminal and Desktop have nothing behind them at all. What is left is what
//! the receipt can answer for by itself, which is also the only part of this
//! product a stranger could check.

use eframe::egui::{self, CornerRadius, Rect, RichText, Stroke};

use crate::app::{App, Tab};
use crate::runs::Signature;
use crate::theme;

pub(crate) const WIDTH: f32 = 380.0;

/// Everything the panel draws, lifted off the run in one go so no closure
/// below holds a borrow on the ledger.
struct Facts {
    valid: bool,
    errors: Vec<String>,
    signature: Signature,
    path: String,
    json: String,
    checks: Vec<(String, String)>,
    files: Vec<(String, String, String)>,
    diff_digest: String,
    evidence: Vec<(String, String, Option<String>)>,
}

impl App {
    pub(crate) fn inspector(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, theme::GROUND);
        ui.painter()
            .vline(rect.left(), rect.y_range(), Stroke::new(1.0, theme::HAIR));

        let inner = Rect::from_min_max(
            egui::pos2(rect.left() + 16.0, rect.top() + crate::menu::STRIP_H),
            egui::pos2(rect.right() - 16.0, rect.bottom() - 12.0),
        );
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        ui.set_clip_rect(rect);
        // Leave the scrollbar its lane, or a 64 character hash reaches under it.
        ui.set_max_width(inner.width() - 12.0);

        crate::run_view::tab_strip(&mut ui, &mut self.tab);
        ui.add_space(12.0);

        let tab = self.tab;
        let (key, _) = crate::run_view::device_key(&self.link);
        let Some(run) = self.current_run() else {
            return;
        };

        let f = Facts {
            valid: run.report.is_valid,
            errors: run.report.errors.clone(),
            signature: run.signature(key.as_deref()),
            path: run.path.display().to_string(),
            json: serde_json::to_string_pretty(&*run.receipt)
                .unwrap_or_else(|e| format!("the receipt would not re-serialize: {e}")),
            checks: vec![
                ("schema".into(), run.receipt.schema_version.clone()),
                ("run".into(), run.receipt.run_id.clone()),
                ("session".into(), run.receipt.session_id.clone()),
                ("chain root".into(), run.receipt.event_hash_chain_root.clone()),
                ("workspace".into(), run.receipt.workspace_fingerprint.clone()),
                (
                    "statements".into(),
                    run.receipt.runner_execution_statements.len().to_string(),
                ),
            ],
            files: run
                .receipt
                .changeset
                .files_modified
                .iter()
                .map(|x| (x.path.clone(), x.preimage_hash.clone(), x.postimage_hash.clone()))
                .collect(),
            diff_digest: run.receipt.changeset.diff_digest.clone(),
            evidence: run
                .receipt
                .evidence_items
                .iter()
                .map(|e| (e.grade.clone(), e.description.clone(), e.artifact_digest.clone()))
                .collect(),
        };

        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| match tab {
                Tab::Receipt => receipt_tab(ui, &f),
                Tab::Changes => changes_tab(ui, &f),
                Tab::Evidence => evidence_tab(ui, &f),
            });
    }
}

fn receipt_tab(ui: &mut egui::Ui, f: &Facts) {
    // The signature is a check this window runs too, whenever the daemon has
    // published its key, so the verdict answers for it as well.
    let signature_fault = match f.signature {
        Signature::Mismatch => Some("The signature does not match the key the connected daemon signs with."),
        Signature::Absent => Some("The receipt carries no signature."),
        Signature::Verified | Signature::Unchecked => None,
    };
    let (tone, word) = match (f.valid && signature_fault.is_none(), f.signature) {
        (false, _) => (theme::RUST, "A check failed."),
        (true, Signature::Verified) => (theme::OK, "Every check passed, the signature included."),
        (true, _) => (
            theme::OK,
            "Every check this window can run passed. The signature was not checked, having no key to be checked with.",
        ),
    };
    ui.add(egui::Label::new(RichText::new(word).font(theme::prose(theme::FS_UI)).color(tone)).wrap());
    for e in f.errors.iter().map(String::as_str).chain(signature_fault) {
        ui.add(
            egui::Label::new(RichText::new(e).font(theme::prose(theme::FS_META)).color(theme::RUST)).wrap(),
        );
    }

    ui.add_space(14.0);
    for (k, v) in &f.checks {
        field(ui, k, v);
    }
    ui.add_space(10.0);
    field(ui, "file", &f.path);

    ui.add_space(16.0);
    ui.label(
        RichText::new("The receipt")
            .font(theme::sans(theme::FS_META))
            .color(theme::FAINT),
    );
    ui.add_space(6.0);
    // The thing itself. Everything above is derived from it, so it has to be
    // readable here or the rest is just this window asserting.
    ui.add(
        egui::Label::new(
            RichText::new(&f.json)
                .font(theme::mono(theme::FS_MICRO))
                .color(theme::FAINT),
        )
        .wrap(),
    );
    ui.add_space(20.0);
}

fn changes_tab(ui: &mut egui::Ui, f: &Facts) {
    if f.files.is_empty() {
        ui.label(
            RichText::new("This run changed no files.")
                .font(theme::prose(theme::FS_UI))
                .color(theme::FAINTER),
        );
    }
    for (p, pre, post) in &f.files {
        ui.add(
            egui::Label::new(RichText::new(p).font(theme::mono(theme::FS_META)).color(theme::INK))
                .wrap(),
        );
        ui.add_space(3.0);
        // Content hashes, not commits. Naming them this way stops anyone
        // reading them as revisions they could check out.
        field(ui, "before", &short(pre));
        field(ui, "after", &short(post));
        ui.add_space(10.0);
    }

    ui.add_space(6.0);
    field(ui, "diff digest", &short(&f.diff_digest));
    ui.add_space(10.0);
    ui.add(
        egui::Label::new(
            RichText::new(
                "The receipt carries a digest of the diff, not the diff. That proves a diff was not altered; it cannot show you one.",
            )
            .font(theme::prose(theme::FS_MICRO))
            .color(theme::FAINTER),
        )
        .wrap(),
    );
}

fn evidence_tab(ui: &mut egui::Ui, f: &Facts) {
    if f.evidence.is_empty() {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "No evidence was recorded. A run with no verification command produces none.",
                )
                .font(theme::prose(theme::FS_UI))
                .color(theme::FAINTER),
            )
            .wrap(),
        );
    }
    for (grade, description, digest) in &f.evidence {
        ui.label(
            RichText::new(grade.replace('_', " "))
                .font(theme::mono(theme::FS_MICRO))
                .color(theme::INK),
        );
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(
                RichText::new(description)
                    .font(theme::prose(theme::FS_META))
                    .color(theme::FAINT),
            )
            .wrap(),
        );
        if let Some(d) = digest {
            ui.add_space(3.0);
            field(ui, "artifact", &short(d));
        }
        ui.add_space(14.0);
    }
}

fn short(hash: &str) -> String {
    hash.chars().take(16).collect()
}

fn field(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(key)
                .font(theme::sans(theme::FS_MICRO))
                .color(theme::FAINTER),
        );
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .font(theme::mono(theme::FS_MICRO))
                    .color(theme::FAINT),
            )
            .truncate(),
        );
    });
    ui.add_space(3.0);
}
