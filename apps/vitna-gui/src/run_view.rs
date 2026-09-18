//! One run, as its receipt records it, and the inspector beside it.
//!
//! The shape is Cursor's: the run in the centre, a tabbed inspector on the
//! right. What fills them is not. Cursor's centre is a chat log, and a Vitna
//! receipt carries no prompt, no timestamps and no transcript, on purpose: it
//! records what was DONE and what can be checked, not what was said. So the
//! centre states the run's standing and its evidence, and a summary line
//! appears only for a run this window watched, because only then is there a
//! sentence to show. A historical run says so rather than inventing one.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

use crate::app::{App, Tab};
use crate::runs::{self, Run, Signature};
use crate::theme;

/// The colour and words for the signature's three states. "Not checked" is not
/// a failure and must never be painted as one.
fn signature_pill(sig: Signature) -> (Color32, &'static str, &'static str) {
    match sig {
        Signature::Verified => (
            theme::OK,
            "Signature verified",
            "The device signature was checked against a public key and held.",
        ),
        Signature::Unchecked => (
            theme::FAINT,
            "Signature not checked",
            "A signature is present. No public key was supplied, so it was not checked. That is not a fault in the receipt.",
        ),
        Signature::Absent => (
            theme::RUST,
            "No signature",
            "The receipt carries no device signature at all, which is a fault.",
        ),
    }
}

fn grade_tone(grade: &str) -> Color32 {
    match grade {
        // Captured by the sandbox or reproduced, so the strongest on offer.
        "sandbox_captured" | "independently_reproduced" | "remote_or_hardware_attested" => theme::OK,
        "broker_observed" => theme::PERI_2,
        // The model said so. Worth showing, and worth marking as weaker.
        _ => theme::RUST,
    }
}

/// One fact on the meta line. It never breaks inside itself, so the row wraps
/// BETWEEN facts rather than leaving "5" stranded at the end of a line above
/// "minutes ago".
fn meta(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(RichText::new(text).font(theme::sans(12.5)).color(theme::FAINT))
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

/// A section heading with its count, which is the count and never a guess.
fn section(ui: &mut egui::Ui, title: &str, n: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::sans(12.0)).color(theme::FAINT));
        ui.add_space(8.0);
        ui.label(
            RichText::new(n.to_string())
                .font(theme::mono(11.5))
                .color(theme::FAINTER),
        );
    });
    ui.add_space(6.0);
    let r = ui.available_rect_before_wrap();
    ui.painter()
        .hline(r.x_range(), r.top(), Stroke::new(1.0, theme::HAIR_2));
    ui.add_space(10.0);
}

/// A tab strip, borrowed straight from Cursor's inspector.
pub(crate) fn tab_strip(ui: &mut egui::Ui, current: &mut Tab) {
    ui.horizontal(|ui| {
        for tab in [Tab::Receipt, Tab::Changes, Tab::Evidence] {
            let selected = *current == tab;
            let text = RichText::new(tab.label())
                .font(theme::sans(12.5))
                .color(if selected { theme::INK } else { theme::FAINT });
            let r = ui.add(
                egui::Button::new(text)
                    .fill(if selected { theme::FACE } else { Color32::TRANSPARENT })
                    .corner_radius(CornerRadius::same(7))
                    .frame(selected),
            );
            if r.clicked() {
                *current = tab;
            }
        }
    });
}

impl App {
    /// The run the centre is showing, if the ledger has arrived and the index
    /// still points at something.
    pub(crate) fn current_run(&mut self) -> Option<&Run> {
        let i = self.open_run?;
        match self.runs.poll() {
            runs::Loading::Done(ledger) => ledger.runs.get(i),
            runs::Loading::Reading => None,
        }
    }

    pub(crate) fn run_view(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        // The summary belongs to the run only if this window watched it. A
        // receipt read off disk has no sentence attached to it.
        let open_id = self.current_run().map(|r| r.receipt.run_id.clone());
        let summary = self
            .last_result
            .as_ref()
            .filter(|r| Some(&r.run_id) == open_id.as_ref())
            .map(|r| r.text.clone())
            .filter(|t| !t.is_empty());

        let Some(run) = self.current_run() else {
            return;
        };

        let run_id = run.receipt.run_id.clone();
        let provider = run.receipt.model_selection.provider.clone();
        let sku = run.receipt.model_selection.model_sku.clone();
        let state = run.receipt.completion_state.replace('_', " ");
        let isolation = run.receipt.isolation_label.clone();
        let age = runs::age(run.written);
        let (tone, word, why) = signature_pill(run.signature());
        let evidence: Vec<(String, String)> = run
            .receipt
            .evidence_items
            .iter()
            .map(|e| (e.grade.clone(), e.description.clone()))
            .collect();
        let files: Vec<String> = run
            .receipt
            .changeset
            .files_modified
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let base = run.receipt.base_commit_sha.clone();
        let errors = run.report.errors.clone();

        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        ui.set_clip_rect(rect);

        let mut leaving = false;
        egui::ScrollArea::vertical()
            .id_salt("run_view")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.add_space(20.0);
                let back = ui.horizontal(|ui| {
                    crate::icons::inline(ui, 14.0, theme::FAINT, crate::icons::chevron_left);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Back")
                            .font(theme::sans(12.5))
                            .color(theme::FAINT),
                    );
                });
                if back
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    leaving = true;
                }

                ui.add_space(14.0);
                ui.label(
                    RichText::new(&run_id)
                        .font(theme::display(19.0))
                        .color(theme::INK),
                );
                ui.add_space(8.0);

                // Spacing rather than separators. A painted dot is its own
                // widget, so a wrap can strand one at the end of a line with
                // nothing after it; space cannot.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    meta(ui, &format!("{sku} ({provider})"));
                    meta(ui, &state);
                    meta(ui, &isolation);
                    if let Some(a) = &age {
                        meta(ui, a);
                    }
                });

                ui.add_space(12.0);
                let pill = ui.horizontal(|ui| {
                    let (d, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                    ui.painter().circle_filled(d.center(), 3.0, tone);
                    ui.label(RichText::new(word).font(theme::sans(12.5)).color(tone));
                });
                pill.response.on_hover_text(why);

                // A failed check is the loudest thing on this surface.
                for e in &errors {
                    ui.add_space(6.0);
                    ui.horizontal_top(|ui| {
                        let (d, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(d.center(), 3.0, theme::RUST);
                        ui.add(
                            egui::Label::new(
                                RichText::new(e).font(theme::prose(12.5)).color(theme::RUST),
                            )
                            .wrap(),
                        );
                    });
                }

                ui.add_space(18.0);
                self.run_body(ui, summary, &evidence, &files, &base);
            });

        if leaving {
            self.open_run = None;
        }
    }
}

impl App {
    /// The sections under the head. Split out because the head reads the run
    /// and these take what it copied, so nothing borrows across the closure.
    fn run_body(
        &self,
        ui: &mut egui::Ui,
        summary: Option<String>,
        evidence: &[(String, String)],
        files: &[String],
        base: &str,
    ) {
        match summary {
            Some(text) => {
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(theme::prose(14.0))
                            .color(theme::INK_2),
                    )
                    .wrap(),
                );
            }
            None => {
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "A receipt records what was done, not what was said, so there is no transcript here. What it does carry is below.",
                        )
                        .font(theme::prose(12.5))
                        .color(theme::FAINTER),
                    )
                    .wrap(),
                );
            }
        }

        ui.add_space(24.0);
        section(ui, "Evidence", evidence.len());
        if evidence.is_empty() {
            ui.label(
                RichText::new("None recorded.")
                    .font(theme::prose(12.5))
                    .color(theme::FAINTER),
            );
        }
        for (grade, description) in evidence {
            ui.horizontal_top(|ui| {
                ui.label(
                    RichText::new(grade.replace('_', " "))
                        .font(theme::mono(11.0))
                        .color(grade_tone(grade)),
                );
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(description)
                            .font(theme::prose(12.5))
                            .color(theme::MUTE),
                    )
                    .wrap(),
                );
            });
            ui.add_space(6.0);
        }

        ui.add_space(18.0);
        section(ui, "Changes", files.len());
        if files.is_empty() {
            ui.label(
                RichText::new("This run changed no files.")
                    .font(theme::prose(12.5))
                    .color(theme::FAINTER),
            );
        }
        for path in files {
            ui.label(
                RichText::new(path)
                    .font(theme::mono(12.0))
                    .color(theme::MUTE),
            );
            ui.add_space(3.0);
        }

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            meta(ui, "base commit");
            ui.add_space(8.0);
            ui.label(
                RichText::new(&base[..base.len().min(12)])
                    .font(theme::mono(11.5))
                    .color(theme::FAINT),
            );
        });
        ui.add_space(28.0);
    }
}
