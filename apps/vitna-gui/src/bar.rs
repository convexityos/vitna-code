//! The bar over the composer's field: the repository, stated as facts.
//!
//! After Claude Code's pull-request bar, which names the repository, the
//! branch, the change and its checks in one line above where you type. Every
//! fact here is one git gives without a network: the repository and branch,
//! the commit HEAD is at, the uncommitted change in lines, whether the branch
//! merges cleanly into the one it is based on as of the last fetch, and when
//! a file in this folder last changed. A pull request number and CI state are
//! GitHub's to say, and this window does not ask GitHub, so neither appears.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

use crate::app::{App, Placement};
use crate::icons;
use crate::repo::{Loading, Merge, RepoFacts};
use crate::theme;
use crate::workspace::Head;

const BAR_H: f32 = 30.0;
const GAP: f32 = 14.0;
type Icon = fn(&egui::Painter, egui::Pos2, Color32);

/// One fact in the bar, painted at `x` from the left, with its source on hover.
/// Returns where the next one may start.
#[allow(clippy::too_many_arguments)]
fn fact(ui: &egui::Ui, id: &str, x: f32, cy: f32, icon: Option<Icon>, text: &str, tone: Color32, max_w: f32, hint: &str) -> f32 {
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let g = theme::line(ui, text, theme::sans(12.5), tone, (max_w - icon_w).max(0.0));
    let w = icon_w + g.size().x;
    if let Some(draw) = icon {
        draw(ui.painter(), egui::pos2(x + 7.0, cy), theme::FAINT);
    }
    ui.painter().galley(egui::pos2(x + icon_w, cy - g.size().y / 2.0), g, tone);
    let r = Rect::from_min_size(egui::pos2(x - 4.0, cy - BAR_H / 2.0 + 3.0), Vec2::new(w + 8.0, BAR_H - 6.0));
    ui.interact(r, ui.id().with(("bar", id)), Sense::hover()).on_hover_text(hint);
    x + w + GAP
}

/// The same, laid leftward from `right`. Returns the new right edge.
#[allow(clippy::too_many_arguments)]
fn fact_right(ui: &egui::Ui, id: &str, right: f32, cy: f32, icon: Option<Icon>, text: &str, tone: Color32, hint: &str) -> f32 {
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let w = icon_w + theme::line(ui, text, theme::sans(12.5), tone, 400.0).size().x;
    fact(ui, id, right - w, cy, icon, text, tone, w + 1.0, hint);
    right - w - GAP
}

/// A pill with a tinted ground: the change and the merge, the two facts a
/// person acts on.
#[allow(clippy::too_many_arguments)]
fn pill_right(ui: &egui::Ui, id: &str, right: f32, cy: f32, mark: Option<(Icon, Color32)>, parts: &[(String, Color32)], ground: Color32, hint: &str) -> f32 {
    let galleys: Vec<_> = parts
        .iter()
        .map(|(t, c)| theme::line(ui, t, theme::sans(12.0), *c, 300.0))
        .collect();
    let mark_w = if mark.is_some() { 18.0 } else { 0.0 };
    let inner: f32 = mark_w
        + galleys.iter().map(|g| g.size().x).sum::<f32>()
        + 6.0 * (galleys.len().saturating_sub(1)) as f32;
    let r = Rect::from_min_max(egui::pos2(right - inner - 16.0, cy - 11.0), egui::pos2(right, cy + 11.0));
    ui.painter().rect_filled(r, CornerRadius::same(11), ground);
    if let Some((icon, tone)) = mark {
        icon(ui.painter(), egui::pos2(r.left() + 15.0, cy), tone);
    }
    let mut x = r.left() + 8.0 + mark_w;
    for (g, (_, c)) in galleys.into_iter().zip(parts) {
        let w = g.size().x;
        ui.painter().galley(egui::pos2(x, cy - g.size().y / 2.0), g, *c);
        x += w + 6.0;
    }
    ui.interact(r, ui.id().with(("bar", id)), Sense::hover()).on_hover_text(hint);
    r.left() - GAP + 4.0
}

fn ago_long(t: Option<std::time::SystemTime>) -> String {
    t.and_then(crate::workspace::ago).unwrap_or_else(|| "at an unknown time".to_string())
}

fn short_base(base: &str) -> &str {
    base.split_once('/').map(|(_, b)| b).unwrap_or(base)
}

impl App {
    pub(crate) fn repo_bar(&mut self, ui: &mut egui::Ui) {
        let facts: Option<RepoFacts> = match self.repo.poll() {
            Loading::Done(f) => Some((**f).clone()),
            Loading::Reading => None,
        };
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), BAR_H), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(9), theme::GROUND);
        ui.painter().rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, theme::HAIR_2), egui::StrokeKind::Inside);
        let cy = rect.center().y;

        // Right to left: the placement, the time, the merge, the change.
        let mut right = self.worktree_toggle(ui, rect.right() - 8.0, cy);
        right = timing(ui, facts.as_ref(), right, cy);
        right = merge(ui, facts.as_ref(), right, cy);
        right = changes(ui, facts.as_ref(), right, cy);

        // Left to right in what is left: repository, branch, commit, and any
        // trouble git reported. The commit goes before the branch is cut.
        let limit = right;
        let mut x = rect.left() + 12.0;
        let folder = self.workspace.name.clone();
        let (repo, repo_hint) = match facts.as_ref().and_then(|f| f.repo_name.clone()) {
            Some(name) => {
                let origin = facts.as_ref().and_then(|f| f.origin_url.clone()).unwrap_or_default();
                let mut h = format!("Repository {name}, named off its origin remote, {origin}.");
                if name != folder {
                    h.push_str(&format!("\nThis folder is {folder}."));
                }
                (name, h)
            }
            None => (folder.clone(), format!("The folder {folder}. No origin remote names a repository here.")),
        };
        x = fact(ui, "repo", x, cy, Some(icons::folder), &repo, theme::INK_2, (limit - x).min(180.0), &repo_hint);

        let (head, tone) = match &self.workspace.head {
            Some(Head::Branch(b)) => (b.clone(), theme::INK_2),
            Some(h) => (h.label(), theme::RUST),
            None => ("no repository".to_string(), theme::FAINT),
        };
        let mut branch_hint = "The branch this window is on. Checking out another is the daemon's to do.".to_string();
        if let Some(f) = &facts {
            match (&f.upstream, f.ahead, f.behind) {
                (Some(u), Some(a), Some(b)) => branch_hint.push_str(&format!("\nTracks {u}: {a} ahead, {b} behind, as of the last fetch.")),
                (Some(u), _, _) => branch_hint.push_str(&format!("\nTracks {u}.")),
                (None, _, _) => branch_hint.push_str("\nNo upstream is set for it."),
            }
        }
        let commit = facts.as_ref().and_then(|f| f.head.clone());
        let commit_w = commit
            .as_ref()
            .map(|c| 20.0 + theme::line(ui, c, theme::sans(12.5), theme::FAINT, 300.0).size().x + GAP)
            .unwrap_or(0.0);
        let branch_full = 20.0 + theme::line(ui, &head, theme::sans(12.5), tone, 2000.0).size().x;
        let room = limit - x;
        let (branch_w, show_commit) = if branch_full + GAP + commit_w <= room {
            (branch_full, commit.is_some())
        } else if room - commit_w >= 100.0 {
            (room - commit_w, commit.is_some())
        } else {
            (room.min(branch_full), false)
        };
        if branch_w >= 40.0 {
            x = fact(ui, "branch", x, cy, Some(icons::branch), &head, tone, branch_w + 1.0, &branch_hint);
        }
        if let (true, Some(c), Some(f)) = (show_commit, &commit, &facts) {
            let mut hint = format!("HEAD is {c}");
            if let Some(s) = &f.subject {
                hint.push_str(&format!(": {s}"));
            }
            if let Some(d) = f.describe.as_ref().filter(|d| *d != c) {
                hint.push_str(&format!("\ngit describe: {d}"));
            }
            hint.push_str(&format!("\nCommitted {}.", ago_long(f.committed)));
            x = fact(ui, "commit", x, cy, Some(icons::commit), c, theme::FAINT, commit_w, &hint);
        }
        if let Some(t) = facts.as_ref().and_then(|f| f.trouble.clone()) {
            if limit - x > 70.0 {
                ui.painter().circle_filled(egui::pos2(x + 3.0, cy), 3.0, theme::RUST);
                fact(ui, "trouble", x + 11.0, cy, None, "git error", theme::RUST, limit - x - 11.0, &t);
            }
        }
    }

    /// The worktree checkbox. Placement is the daemon's to enforce; this only
    /// states the preference.
    fn worktree_toggle(&mut self, ui: &egui::Ui, right: f32, cy: f32) -> f32 {
        let on = self.placement == Placement::Worktree;
        let galley = theme::line(ui, "worktree", theme::sans(12.5), theme::INK_2, 200.0);
        let rect = Rect::from_min_max(
            egui::pos2(right - galley.size().x - 26.0, cy - 11.0),
            egui::pos2(right, cy + 11.0),
        );
        let resp = ui.interact(rect, ui.id().with("bar-worktree"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), theme::FACE);
        }
        let b = Rect::from_center_size(egui::pos2(rect.left() + 11.0, cy), Vec2::splat(12.0));
        if on {
            ui.painter().rect_filled(b, CornerRadius::same(3), theme::PERI);
            let st = Stroke::new(1.6, theme::CANVAS);
            let c = b.center();
            ui.painter().line_segment([c + Vec2::new(-3.0, 0.0), c + Vec2::new(-1.0, 2.2)], st);
            ui.painter().line_segment([c + Vec2::new(-1.0, 2.2), c + Vec2::new(3.2, -2.4)], st);
        } else {
            ui.painter().rect_stroke(b, CornerRadius::same(3), Stroke::new(1.2, theme::FAINT), egui::StrokeKind::Inside);
        }
        ui.painter().galley(egui::pos2(rect.left() + 22.0, cy - galley.size().y / 2.0), galley, theme::INK_2);
        if resp.clicked() {
            self.placement = if on { Placement::Local } else { Placement::Worktree };
        }
        resp.on_hover_text(if on {
            "The turn runs in a fresh worktree of this repo; this checkout stays as it is."
        } else {
            "The turn runs on this checkout, in place."
        });
        rect.left() - GAP
    }
}

/// When a file here last changed, off the probe's bounded walk.
fn timing(ui: &egui::Ui, facts: Option<&RepoFacts>, right: f32, cy: f32) -> f32 {
    let Some(f) = facts else {
        return right;
    };
    let (text, hint) = match f.touched {
        Some(t) => {
            let mut h = format!("A file in this folder last changed {}.", ago_long(Some(t)));
            if f.scan_was_capped {
                h.push_str(&format!(
                    "\nThat is the newest of the first {} entries; the walk stops there.",
                    f.entries_scanned
                ));
            }
            (crate::runs::short_age(Some(t)).unwrap_or_else(|| "-".to_string()), h)
        }
        None => ("-".to_string(), "No file here has a change time this window could read.".to_string()),
    };
    fact_right(ui, "time", right, cy, Some(icons::clock), &text, theme::FAINT, &hint)
}

/// Whether the branch merges into its base, as of the last fetch.
fn merge(ui: &egui::Ui, facts: Option<&RepoFacts>, right: f32, cy: f32) -> f32 {
    let Some(f) = facts else {
        return right;
    };
    // No base branch to merge into is not a verdict, so nothing is shown.
    let Some(base) = &f.base else {
        return right;
    };
    let short = short_base(base);
    let (text, tone) = match (&f.merge, f.base_behind) {
        (Some(Merge::Clean), _) => (format!("merges into {short}"), theme::OK),
        (Some(Merge::Conflicts(files)), _) => {
            let n = files.len().max(1);
            (format!("{n} conflict{} with {short}", if n == 1 { "" } else { "s" }), theme::RUST)
        }
        (Some(Merge::UpToDate), Some(b)) if b > 0 => (format!("{b} behind {short}"), theme::FAINT),
        (Some(Merge::UpToDate), _) => (format!("even with {short}"), theme::FAINT),
        (Some(Merge::NotRun(_)), _) => (format!("merge into {short} not checked"), theme::FAINTER),
        (None, _) => (format!("merge into {short} unknown"), theme::FAINTER),
    };
    let mut hint = match &f.merge {
        Some(Merge::NotRun(why)) => format!("Not checked against {base}. {why}"),
        _ => format!(
            "Checked with git merge-tree against {base}, as of the last fetch, {}.",
            ago_long(f.fetched)
        ),
    };
    if let (Some(a), Some(b)) = (f.base_ahead, f.base_behind) {
        hint.push_str(&format!("\nThis branch is {a} ahead of it and {b} behind."));
    }
    match &f.merge {
        Some(Merge::Conflicts(files)) => {
            for p in files.iter().take(12) {
                hint.push_str(&format!("\nconflict: {p}"));
            }
        }
        None => hint.push_str("\ngit could not answer the merge check."),
        _ => {}
    }
    pill_right(ui, "merge", right, cy, Some((icons::merge, tone)), &[(text, tone)], tone.gamma_multiply(0.14), &hint)
}

/// The uncommitted change, in lines against HEAD, and the untracked files.
fn changes(ui: &egui::Ui, facts: Option<&RepoFacts>, right: f32, cy: f32) -> f32 {
    let Some(f) = facts else {
        return fact_right(ui, "changes", right, cy, None, "reading git", theme::FAINTER, "git is being asked about this folder.");
    };
    let (Some(a), Some(r)) = (f.added, f.removed) else {
        return right;
    };
    let u = f.untracked.unwrap_or(0);
    let mut parts: Vec<(String, Color32)> = Vec::new();
    if a > 0 || r > 0 {
        parts.push((format!("+{}", crate::composer::thousands(a)), theme::OK));
        parts.push((format!("\u{2212}{}", crate::composer::thousands(r)), theme::DEL));
    }
    if u > 0 {
        parts.push((format!("{u} new"), theme::FAINT));
    }
    if parts.is_empty() {
        parts.push(("no changes".to_string(), theme::FAINT));
    }
    let hint = format!(
        "Uncommitted, against HEAD: {a} lines added and {r} removed ({} files modified, {} staged).\n{u} untracked file{}, whose lines git does not count until they are added.",
        f.modified.unwrap_or(0),
        f.staged.unwrap_or(0),
        if u == 1 { "" } else { "s" },
    );
    pill_right(ui, "changes", right, cy, None, &parts, theme::FACE, &hint)
}
