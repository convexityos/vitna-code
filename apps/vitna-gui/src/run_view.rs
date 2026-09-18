//! One run, as its receipt records it, and the inspector beside it.
//!
//! The shape is Cursor's: the run in the centre, a tabbed inspector on the
//! right. What fills them is not. Cursor's centre is a chat log, and a Vitna
//! receipt carries no prompt, no timestamps and no transcript, on purpose: it
//! records what was DONE and what can be checked, not what was said. So the
//! centre states the run's standing and its evidence, and a summary line
//! appears only for a run this window watched, because only then is there a
//! sentence to show. A historical run says so rather than inventing one. The
//! one thing said that does appear is the prompt, when the daemon's event log
//! has it, and it is labelled with where it came from.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};

use crate::app::{App, Tab};
use crate::link::Link;
use crate::runs::{self, Run, Signature};
use crate::theme;

/// The key the connected daemon signs with, or why there is none to check
/// with. The daemon is the only source: it publishes the key in `Health`.
pub(crate) fn device_key(link: &Link) -> (Option<String>, &'static str) {
    match link {
        Link::Open { health, .. } => match &health.device_public_key {
            Some(key) => (Some(key.clone()), ""),
            None => (None, "the connected daemon does not say which key it signs with"),
        },
        Link::Probing => (None, "the window has not reached the daemon yet, and the daemon publishes the key"),
        Link::Absent { .. } => (None, "no daemon is connected, and the daemon publishes the key"),
    }
}

/// The colour and words for the signature's four states. "Not checked" is not
/// a failure and must never be painted as one; "does not match" is a finding,
/// and says what it cannot tell.
fn signature_pill(sig: Signature, key: Option<&str>, no_key_because: &str) -> (Color32, &'static str, String) {
    // Enough of the key to tell two apart on a hover, and no more on screen.
    let short = key.map(|k| k.get(..16).unwrap_or(k)).unwrap_or_default();
    match sig {
        Signature::Verified => (
            theme::OK,
            "Signature verified",
            format!(
                "Checked against the key the connected daemon signs with (key {short}\u{2026}), and it \
                 held: this is the receipt exactly as that daemon signed it."
            ),
        ),
        Signature::Unchecked => (
            theme::FAINT,
            "Signature not checked",
            format!(
                "A signature is present, and there is no key to check it with: {no_key_because}. That is \
                 not a fault in the receipt."
            ),
        ),
        Signature::Mismatch => (
            theme::RUST,
            "Signature does not match",
            format!(
                "It does not verify with the key the connected daemon signs with (key {short}\u{2026}). \
                 Either another key signed it (until this build the daemon made a new key every time it \
                 started, and kept none of them), or the receipt changed after it was signed. A receipt \
                 does not name its key, so it cannot say which."
            ),
        ),
        Signature::Absent => (
            theme::RUST,
            "No signature",
            "The receipt carries no device signature at all, which is a fault.".to_string(),
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
        egui::Label::new(RichText::new(text).font(theme::sans(theme::FS_UI)).color(theme::FAINT))
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

/// A section heading with its count, which is the count and never a guess.
fn section(ui: &mut egui::Ui, title: &str, n: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::sans(theme::FS_META)).color(theme::FAINT));
        ui.add_space(8.0);
        ui.label(
            RichText::new(n.to_string())
                .font(theme::mono(theme::FS_MICRO))
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
                .font(theme::sans(theme::FS_UI))
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
        // The prompt, when the daemon's log has one; the receipt never does.
        let prompt = open_id
            .as_ref()
            .and_then(|id| self.prompts.as_ref()?.for_run(id))
            .map(|t| t.prompt.clone());
        let (key, no_key_because) = device_key(&self.link);

        let Some(run) = self.current_run() else {
            return;
        };

        let run_id = run.receipt.run_id.clone();
        let provider = run.receipt.model_selection.provider.clone();
        let sku = run.receipt.model_selection.model_sku.clone();
        let state = run.receipt.completion_state.replace('_', " ");
        let isolation = run.receipt.isolation_label.clone();
        let age = runs::age(run.written);
        let (tone, word, why) = signature_pill(run.signature(key.as_deref()), key.as_deref(), no_key_because);
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
                            .font(theme::sans(theme::FS_UI))
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
                // Headed by what was asked, as the list names it, and by the
                // run id when the daemon's log has no prompt for it.
                let heading = match &prompt {
                    Some(p) => crate::prompts::title(p),
                    None => run_id.clone(),
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(heading)
                            .font(theme::display(theme::FS_HEAD))
                            .color(theme::INK),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);

                // Spacing rather than separators. A painted dot is its own
                // widget, so a wrap can strand one at the end of a line with
                // nothing after it; space cannot.
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    if prompt.is_some() {
                        meta(ui, &run_id);
                    }
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
                    ui.label(RichText::new(word).font(theme::sans(theme::FS_UI)).color(tone));
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
                                RichText::new(e).font(theme::prose(theme::FS_UI)).color(theme::RUST),
                            )
                            .wrap(),
                        );
                    });
                }

                ui.add_space(18.0);
                self.run_body(ui, prompt.as_deref(), summary, &evidence, &files, &base);
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
        prompt: Option<&str>,
        summary: Option<String>,
        evidence: &[(String, String)],
        files: &[String],
        base: &str,
    ) {
        // The heading already says a one-line prompt; the whole of it is
        // shown only when there is more of it than the heading holds.
        if let Some(p) = prompt.filter(|p| p.trim() != crate::prompts::title(p)) {
            ui.label(RichText::new("Asked").font(theme::sans(theme::FS_META)).color(theme::FAINT));
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(RichText::new(p).font(theme::prose(theme::FS_TITLE)).color(theme::INK))
                    .wrap(),
            );
            ui.add_space(14.0);
        }
        let source = "The prompt comes from the daemon's event log. A receipt records what was done, not what was said, so it carries neither the prompt nor a transcript. What it does carry is below.";
        match summary {
            Some(text) => {
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(theme::prose(theme::FS_TITLE))
                            .color(theme::INK),
                    )
                    .wrap(),
                );
                if prompt.is_some() {
                    ui.add_space(6.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new("The prompt comes from the daemon's event log; the receipt does not carry it.")
                                .font(theme::prose(theme::FS_MICRO))
                                .color(theme::FAINTER),
                        )
                        .wrap(),
                    );
                }
            }
            None => {
                ui.add(
                    egui::Label::new(
                        RichText::new(if prompt.is_some() {
                            source
                        } else {
                            "A receipt records what was done, not what was said, so there is no transcript here. What it does carry is below."
                        })
                        .font(theme::prose(theme::FS_UI))
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
                    .font(theme::prose(theme::FS_UI))
                    .color(theme::FAINTER),
            );
        }
        for (grade, description) in evidence {
            ui.horizontal_top(|ui| {
                ui.label(
                    RichText::new(grade.replace('_', " "))
                        .font(theme::mono(theme::FS_MICRO))
                        .color(grade_tone(grade)),
                );
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(description)
                            .font(theme::prose(theme::FS_UI))
                            .color(theme::FAINT),
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
                    .font(theme::prose(theme::FS_UI))
                    .color(theme::FAINTER),
            );
        }
        for path in files {
            ui.label(
                RichText::new(path)
                    .font(theme::mono(theme::FS_META))
                    .color(theme::FAINT),
            );
            ui.add_space(3.0);
        }

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            meta(ui, "base commit");
            ui.add_space(8.0);
            ui.label(
                RichText::new(&base[..base.len().min(12)])
                    .font(theme::mono(theme::FS_MICRO))
                    .color(theme::FAINT),
            );
        });
        ui.add_space(28.0);
    }
}
