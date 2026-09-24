//! One run, as its receipt records it, and the receipt itself beside it.
//!
//! The centre is the reading: how the run ended, what it was, what it
//! changed, what evidence it left, and the chain of checks this window ran
//! before believing any of it. The inspector down the right is the record
//! the reading was made from. Cursor's centre is a chat log, and a Vitna
//! receipt carries no prompt, no timestamps and no transcript, on purpose: it
//! records what was DONE and what can be checked, not what was said. So a
//! summary appears only for a run this window watched, because only then is
//! there a sentence to show, and the prompt only when the daemon's event log
//! has it. The machine's names for the run (its id, its session, its base
//! commit, its file) are at the foot, where they can be found and copied
//! without heading the page.

use eframe::egui::{self, Color32, RichText, Vec2};

use crate::app::App;
use crate::checks;
use crate::link::Link;
use crate::parts;
use crate::runs::{self, Run};
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

/// The words `apps/vitna-desktop` prints for an evidence grade, and the tone
/// its dot takes: captured or reproduced is the strongest on offer, observed
/// by the broker the next, and the model's own word the weakest.
pub(crate) fn grade(grade: &str) -> (String, Color32) {
    match grade {
        "sandbox_captured" => ("Captured in the sandbox".into(), theme::OK),
        "independently_reproduced" => ("Independently reproduced".into(), theme::OK),
        "remote_or_hardware_attested" => ("Attested remotely or by hardware".into(), theme::OK),
        "broker_observed" => ("Observed by the broker".into(), theme::PERI_2),
        "model_reported" => ("Model reported".into(), theme::RUST),
        other => (format!("Unknown grade \"{other}\""), theme::RUST),
    }
}

/// The words for an isolation label, as `apps/vitna-desktop` prints them.
pub(crate) fn isolation(label: &str) -> String {
    match label {
        "read_only" => "Read only".into(),
        "guarded" => "Guarded".into(),
        "strong" => "Strong".into(),
        "full_access" => "Full access".into(),
        other => format!("Unknown label \"{other}\""),
    }
}

/// What happened to a file, from the hashes the receipt recorded for it: a
/// before of all zeros is a file that did not exist, an after of all zeros
/// one that no longer does.
pub(crate) fn change_kind(pre: &str, post: &str) -> &'static str {
    let zero = |h: &str| !h.is_empty() && h.chars().all(|c| c == '0');
    if pre.is_empty() || post.is_empty() {
        "hash missing"
    } else if zero(pre) {
        "new"
    } else if zero(post) {
        "deleted"
    } else {
        "modified"
    }
}

/// Everything the page draws, lifted off the run in one go so no closure
/// below holds a borrow on the ledger.
struct Page {
    phrase: String,
    icon: fn(&egui::Painter, egui::Pos2, Color32),
    tone: Color32,
    title: String,
    facts: Vec<(String, Option<String>)>,
    chain: checks::Chain,
    changes: Vec<(&'static str, String, String, String)>,
    evidence: Vec<(String, String, Option<String>)>,
    foot: String,
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

    /// The open run's title, for the panel's trail: its prompt when the
    /// daemon's log has one, and what it changed when it does not.
    pub(crate) fn open_run_title(&mut self) -> Option<String> {
        let run = self.current_run()?;
        let id = run.receipt.run_id.clone();
        let paths: Vec<String> = run.receipt.changeset.files_modified.iter().map(|f| f.path.clone()).collect();
        let prompt = self.prompts.as_ref().and_then(|p| p.for_run(&id)).map(|t| crate::prompts::title(&t.prompt));
        Some(prompt.unwrap_or_else(|| {
            crate::run_list::changed_title(&paths.iter().map(String::as_str).collect::<Vec<_>>())
        }))
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
        let model_name = {
            let choices = self.choices.clone();
            move |sku: &str, provider: &str| {
                choices
                    .iter()
                    .find(|c| c.sku == sku && c.provider_id == provider)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| sku.to_string())
            }
        };
        let title = self.open_run_title();

        let Some(run) = self.current_run() else {
            return;
        };
        let r = &run.receipt;
        let chain = checks::for_run(run, key.as_deref(), no_key_because);
        let failed = checks::any_failed(&chain);
        let (_, phrase, _) = crate::run_list::completion(&r.completion_state);
        let (icon, tone) = if failed {
            (crate::icons::alert as fn(&egui::Painter, egui::Pos2, Color32), theme::RUST)
        } else {
            match r.completion_state.as_str() {
                "completed_with_evidence" => (crate::icons::done as fn(&egui::Painter, egui::Pos2, Color32), theme::OK),
                "failed" => (crate::icons::failed as fn(&egui::Painter, egui::Pos2, Color32), theme::RUST),
                "cancelled" => (crate::icons::failed as fn(&egui::Painter, egui::Pos2, Color32), theme::FAINT),
                _ => (crate::icons::alert as fn(&egui::Painter, egui::Pos2, Color32), theme::RUST),
            }
        };
        let files = r.changeset.files_modified.len();
        let page = Page {
            phrase: if failed { "A check failed".to_string() } else { phrase },
            icon,
            tone,
            title: title.unwrap_or_default(),
            facts: vec![
                (
                    match files {
                        0 => "No files changed".to_string(),
                        1 => "1 file changed".to_string(),
                        n => format!("{n} files changed"),
                    },
                    None,
                ),
                (
                    model_name(&r.model_selection.model_sku, &r.model_selection.provider),
                    Some(format!(
                        "{} via {}, routed by {}, as the receipt records it.",
                        r.model_selection.model_sku, r.model_selection.provider, r.model_selection.routing_reason
                    )),
                ),
                (
                    isolation(&r.isolation_label),
                    Some("The isolation label, as the receipt records it. This window does not check how a run was isolated.".to_string()),
                ),
                (
                    runs::age(run.written).map(|a| format!("written {a}")).unwrap_or_default(),
                    Some("When the receipt's file was written. A receipt carries no timestamp of its own.".to_string()),
                ),
            ],
            chain,
            changes: r
                .changeset
                .files_modified
                .iter()
                .map(|f| (change_kind(&f.preimage_hash, &f.postimage_hash), f.path.clone(), f.preimage_hash.clone(), f.postimage_hash.clone()))
                .collect(),
            evidence: r
                .evidence_items
                .iter()
                .map(|e| (e.grade.clone(), e.description.clone(), e.artifact_digest.clone()))
                .collect(),
            foot: format!(
                "{}  \u{b7}  session {}  \u{b7}  base {}  \u{b7}  {}",
                r.run_id,
                r.session_id,
                r.base_commit_sha.chars().take(7).collect::<String>(),
                run.path.display()
            ),
        };

        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        ui.set_clip_rect(rect);

        egui::ScrollArea::vertical()
            .id_salt("run_view")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                ui.add_space(24.0);
                head(ui, &page);
                ui.add_space(18.0);
                self.run_body(ui, prompt.as_deref(), summary, &page);
            });
    }
}

/// How the run stands, what it was, and the facts that frame it.
fn head(ui: &mut egui::Ui, page: &Page) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        crate::icons::inline(ui, 16.0, page.tone, page.icon);
        ui.label(RichText::new(&page.phrase).font(theme::sans(theme::FS_UI)).color(theme::INK));
    });
    ui.add_space(8.0);
    ui.add(egui::Label::new(RichText::new(&page.title).font(theme::display(theme::FS_HEAD)).color(theme::INK)).wrap());
    ui.add_space(8.0);
    let facts: Vec<&(String, Option<String>)> = page.facts.iter().filter(|(t, _)| !t.is_empty()).collect();
    let sep = "  \u{b7}  ";
    let joined = facts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(sep);
    let whole = theme::line(ui, &joined, theme::sans(theme::FS_UI), theme::FAINT, f32::INFINITY).size().x;
    if whole <= ui.available_width() {
        // One line, each fact with its own hover.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (i, (text, hint)) in facts.iter().enumerate() {
                if i > 0 {
                    ui.label(RichText::new("\u{b7}").font(theme::sans(theme::FS_UI)).color(theme::FAINTER));
                }
                let l = ui.add(
                    egui::Label::new(RichText::new(text).font(theme::sans(theme::FS_UI)).color(theme::FAINT))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                if let Some(h) = hint {
                    l.on_hover_text(h);
                }
            }
        });
    } else {
        // Too narrow for one line: the facts wrap as a sentence would, between
        // words, and their notes share one hover rather than a dot being
        // stranded at the end of a line.
        let notes = facts.iter().filter_map(|(_, h)| h.as_deref()).collect::<Vec<_>>().join("\n\n");
        ui.add(egui::Label::new(RichText::new(joined).font(theme::sans(theme::FS_UI)).color(theme::FAINT)).wrap())
            .on_hover_text(notes);
    }
}

impl App {
    /// The sections under the head. Split out because the head reads the run
    /// and these take what it copied, so nothing borrows across the closure.
    fn run_body(&self, ui: &mut egui::Ui, prompt: Option<&str>, summary: Option<String>, page: &Page) {
        // The heading already says a one-line prompt; the whole of it is
        // shown only when there is more of it than the heading holds.
        if let Some(p) = prompt.filter(|p| p.trim() != crate::prompts::title(p)) {
            ui.label(RichText::new("Asked").font(theme::sans(theme::FS_META)).color(theme::FAINTER));
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(p).font(theme::prose(theme::FS_TITLE)).color(theme::INK)).wrap());
            ui.add_space(14.0);
        }
        match summary {
            Some(text) => {
                ui.add(egui::Label::new(RichText::new(text).font(theme::prose(theme::FS_TITLE)).color(theme::INK)).wrap());
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        RichText::new("The model's last words, as this window received them. The receipt records what was done, not what was said.")
                            .font(theme::prose(theme::FS_META))
                            .color(theme::FAINTER),
                    )
                    .wrap(),
                );
            }
            None => {
                ui.add(
                    egui::Label::new(
                        RichText::new(if prompt.is_some() {
                            "The prompt is from the daemon's event log. A receipt records what was done, not what was said."
                        } else {
                            "A receipt records what was done, not what was said, so there is no transcript here."
                        })
                        .font(theme::prose(theme::FS_UI))
                        .color(theme::FAINTER),
                    )
                    .wrap(),
                );
            }
        }

        // The chain first: what this window checked before believing the
        // rest of the page, which is the whole of what a receipt is for.
        ui.add_space(26.0);
        parts::section(ui, "Checks", None);
        ui.add_space(4.0);
        checks::path(ui, &page.chain);

        ui.add_space(26.0);
        parts::section(ui, "Changed", Some(page.changes.len()));
        if page.changes.is_empty() {
            quiet(ui, "This run changed no files.");
        }
        for (kind, path, pre, post) in &page.changes {
            let row = ui.horizontal(|ui| {
                let (k, _) = ui.allocate_exact_size(Vec2::new(64.0, 22.0), egui::Sense::hover());
                ui.painter().text(
                    egui::pos2(k.left(), k.center().y),
                    egui::Align2::LEFT_CENTER,
                    kind,
                    theme::sans(theme::FS_META),
                    if *kind == "hash missing" { theme::RUST } else { theme::FAINTER },
                );
                ui.add(egui::Label::new(RichText::new(path).font(theme::sans(theme::FS_UI)).color(theme::INK)).truncate());
            });
            // Content hashes, not commits. Naming them this way stops anyone
            // reading them as revisions they could check out.
            row.response.on_hover_text(format!(
                "{path}\nbefore {}\nafter {}\nContent hashes as the receipt records them, not commits.",
                if pre.is_empty() { "missing".to_string() } else { theme::digest(pre) },
                if post.is_empty() { "missing".to_string() } else { theme::digest(post) },
            ));
        }

        ui.add_space(24.0);
        parts::section(ui, "Evidence", Some(page.evidence.len()));
        if page.evidence.is_empty() {
            quiet(ui, "None recorded. A run with no verification command produces none.");
        }
        for (g, description, digest) in &page.evidence {
            let (word, tone) = grade(g);
            let block = ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (d, _) = ui.allocate_exact_size(Vec2::new(8.0, 18.0), egui::Sense::hover());
                    ui.painter().circle_filled(d.center(), 3.0, tone);
                    ui.label(RichText::new(word).font(theme::sans(theme::FS_META)).color(theme::FAINT));
                });
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.add(egui::Label::new(RichText::new(description).font(theme::prose(theme::FS_UI)).color(theme::INK)).wrap());
                });
            });
            if let Some(d) = digest {
                block.response.on_hover_text(format!("artifact {}", theme::digest(d)));
            }
            ui.add_space(8.0);
        }


        ui.add_space(28.0);
        parts::rule(ui, theme::HAIR_2);
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(&page.foot).font(theme::sans(theme::FS_META)).color(theme::FAINTER)).wrap());
        ui.add_space(24.0);
    }
}

fn quiet(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).font(theme::prose(theme::FS_UI)).color(theme::FAINTER)).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_named_from_the_hashes_the_receipt_recorded() {
        let zero = "0".repeat(64);
        let h = "a".repeat(64);
        assert_eq!(change_kind(&zero, &h), "new");
        assert_eq!(change_kind(&h, &zero), "deleted");
        assert_eq!(change_kind(&h, &h), "modified");
        assert_eq!(change_kind("", &h), "hash missing");
    }
}
