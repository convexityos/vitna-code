//! The line over the composer's field: where the turn will run, stated as
//! facts.
//!
//! It was a bordered bar of chips and two tinted pills over a bordered field,
//! two boxes stacked, and the pills were the tell the owner named first. It
//! is a line of words now, the way the terminal client and Claude Code both
//! write it: the repository on its branch at the left, and at the right the
//! uncommitted change, whether the branch merges into the one it is based on,
//! when a file here last changed, and the worktree switch. Colour is spent
//! only where it is a state: the plus and minus of the change, the merge's
//! mark, and a conflict. Every fact here is one git gives without a network,
//! and each says where it came from on its hover. A pull request number and
//! CI state are GitHub's to say, and this window does not ask GitHub, so
//! neither appears.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

use crate::app::{App, Placement};
use crate::icons;
use crate::repo::{Loading, Merge, RepoFacts};
use crate::theme;
use crate::workspace::Head;

const LINE_H: f32 = 24.0;
const GAP: f32 = 16.0;

/// A Lucide icon's drawing function, and the tone it is drawn in.
type Mark = (fn(&egui::Painter, egui::Pos2, Color32), Color32);

/// Text laid at `x`, cut to `max_w`, with its source on hover. Returns the
/// rect it took.
#[allow(clippy::too_many_arguments)]
fn text(ui: &egui::Ui, id: &str, x: f32, cy: f32, s: &str, tone: Color32, max_w: f32, hint: &str) -> Rect {
    let g = theme::line(ui, s, theme::sans(theme::FS_UI), tone, max_w.max(0.0));
    let r = Rect::from_min_size(egui::pos2(x, cy - g.size().y / 2.0), g.size());
    ui.painter().galley(r.min, g, tone);
    if !hint.is_empty() {
        ui.interact(r.expand2(Vec2::new(3.0, 4.0)), ui.id().with(("bar", id)), Sense::hover())
            .on_hover_text(hint);
    }
    r
}

/// A fact for the line's right side: an optional leading mark, runs of text
/// each in its tone, and where it came from on the hover. Measured before it
/// is laid, so the line can drop the least of them when it runs short.
struct Fact {
    id: &'static str,
    icon: Option<Mark>,
    parts: Vec<(String, Color32)>,
    hint: String,
}

impl Fact {
    fn galleys(&self, ui: &egui::Ui) -> Vec<std::sync::Arc<egui::Galley>> {
        self.parts
            .iter()
            .map(|(t, c)| theme::line(ui, t, theme::sans(theme::FS_UI), *c, 300.0))
            .collect()
    }

    fn width(&self, ui: &egui::Ui) -> f32 {
        let g = self.galleys(ui);
        (if self.icon.is_some() { 20.0 } else { 0.0 })
            + g.iter().map(|g| g.size().x).sum::<f32>()
            + 6.0 * g.len().saturating_sub(1) as f32
    }

    /// Lays the fact leftward from `right`. Returns the new right edge.
    fn lay(&self, ui: &egui::Ui, right: f32, cy: f32) -> f32 {
        let left = right - self.width(ui);
        if let Some((draw, tone)) = self.icon {
            draw(ui.painter(), egui::pos2(left + 7.0, cy), tone);
        }
        let mut x = left + if self.icon.is_some() { 20.0 } else { 0.0 };
        for (g, (_, c)) in self.galleys(ui).into_iter().zip(&self.parts) {
            let gw = g.size().x;
            ui.painter().galley(egui::pos2(x, cy - g.size().y / 2.0), g, *c);
            x += gw + 6.0;
        }
        let r = Rect::from_min_max(egui::pos2(left - 3.0, cy - LINE_H / 2.0), egui::pos2(right + 3.0, cy + LINE_H / 2.0));
        ui.interact(r, ui.id().with(("bar", self.id)), Sense::hover()).on_hover_text(&self.hint);
        left - GAP
    }
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
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), LINE_H), Sense::hover());
        let cy = rect.center().y;

        // The repository, "on" and some of the branch are what the line is
        // for, so they are measured first and the facts at the right give way
        // to them, the least of them first: when a file last changed, then
        // the merge, then the change itself. The worktree switch always stays.
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
        let repo_w = theme::line(ui, &repo, theme::sans(theme::FS_UI), theme::INK, 220.0).size().x;
        let head_text = self.workspace.head.as_ref().map(|h| h.label());
        let head_w = head_text
            .as_ref()
            .map(|h| theme::line(ui, h, theme::sans(theme::FS_UI), theme::FAINT, 2000.0).size().x)
            .unwrap_or(140.0);
        let left_min = 2.0 + repo_w + 30.0 + head_w.min(150.0) + GAP;

        let mut right = self.worktree_toggle(ui, rect.right() - 2.0, cy);
        let mut shown: Vec<Fact> = [changes(facts.as_ref()), merge(facts.as_ref()), timing(facts.as_ref())]
            .into_iter()
            .flatten()
            .collect();
        let room = right - rect.left();
        while !shown.is_empty()
            && shown.iter().map(|f| f.width(ui) + GAP).sum::<f32>() > room - left_min
        {
            shown.pop();
        }
        // Laid right to left in the order they read: the change, the merge,
        // the time, then the switch at the end.
        for f in shown.iter().rev() {
            right = f.lay(ui, right, cy);
        }

        // Left to right in what is left: the repository, "on", the branch,
        // and the commit, which goes before the branch is cut.
        let limit = right;
        let mut x = rect.left() + 2.0;
        let r = text(ui, "repo", x, cy, &repo, theme::INK, (limit - x).min(220.0), &repo_hint);
        x = r.right();

        let (head, tone) = match (&self.workspace.head, head_text) {
            (Some(Head::Branch(b)), _) => (b.clone(), theme::FAINT),
            (Some(_), Some(label)) => (label, theme::RUST),
            _ => {
                // Not a repository: there is no branch to be "on".
                text(
                    ui,
                    "norepo",
                    x + 12.0,
                    cy,
                    "not a git repository",
                    theme::FAINTER,
                    limit - x - 12.0,
                    "No .git here, so there is no branch, commit or change to state.",
                );
                return;
            }
        };
        let mut branch_hint = "The branch this window is on. Checking out another is the daemon's to do.".to_string();
        if let Some(f) = &facts {
            match (&f.upstream, f.ahead, f.behind) {
                (Some(u), Some(a), Some(b)) => branch_hint.push_str(&format!("\nTracks {u}: {a} ahead, {b} behind, as of the last fetch.")),
                (Some(u), _, _) => branch_hint.push_str(&format!("\nTracks {u}.")),
                (None, _, _) => branch_hint.push_str("\nNo upstream is set for it."),
            }
        }
        if limit - x < 60.0 {
            return;
        }
        let on = text(ui, "on", x + 6.0, cy, "on", theme::FAINTER, 40.0, "");
        x = on.right() + 6.0;

        let commit = facts.as_ref().and_then(|f| f.head.clone());
        let commit_w = commit
            .as_ref()
            .map(|c| theme::line(ui, c, theme::sans(theme::FS_UI), theme::FAINTER, 300.0).size().x + GAP)
            .unwrap_or(0.0);
        let branch_full = theme::line(ui, &head, theme::sans(theme::FS_UI), tone, 2000.0).size().x;
        let room = limit - x;
        let (branch_w, show_commit) = if branch_full + GAP + commit_w <= room {
            (branch_full, commit.is_some())
        } else if room - commit_w >= 100.0 {
            (room - commit_w - GAP, commit.is_some())
        } else {
            (room.min(branch_full), false)
        };
        // A branch is shown whole or cut to at least 40 points, and "main" is
        // shorter than that, so the floor is the smaller of the two.
        if branch_w >= branch_full.min(40.0) {
            let b = text(ui, "branch", x, cy, &head, tone, branch_w + 1.0, &branch_hint);
            x = b.right() + GAP;
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
            let cr = text(ui, "commit", x, cy, c, theme::FAINTER, commit_w, &hint);
            x = cr.right() + GAP;
        }
        if let Some(t) = facts.as_ref().and_then(|f| f.trouble.clone()) {
            if limit - x > 70.0 {
                ui.painter().circle_filled(egui::pos2(x + 3.0, cy), 3.0, theme::RUST);
                text(ui, "trouble", x + 11.0, cy, "git error", theme::RUST, limit - x - 11.0, &t);
            }
        }
    }

    /// The worktree checkbox. Placement is the daemon's to enforce; this only
    /// states the preference.
    fn worktree_toggle(&mut self, ui: &egui::Ui, right: f32, cy: f32) -> f32 {
        let on = self.placement == Placement::Worktree;
        let galley = theme::line(ui, "worktree", theme::sans(theme::FS_UI), theme::FAINT, 200.0);
        let rect = Rect::from_min_max(
            egui::pos2(right - galley.size().x - 26.0, cy - 11.0),
            egui::pos2(right, cy + 11.0),
        );
        let resp = ui.interact(rect, ui.id().with("bar-worktree"), Sense::click());
        let t = theme::hover(ui, resp.id, resp.hovered());
        if t > 0.0 {
            ui.painter().rect_filled(rect.expand2(Vec2::new(4.0, 0.0)), CornerRadius::same(6), theme::FACE.gamma_multiply(t));
        }
        let b = Rect::from_center_size(egui::pos2(rect.left() + 7.0, cy), Vec2::splat(12.0));
        if on {
            ui.painter().rect_filled(b, CornerRadius::same(3), theme::PERI);
            let st = Stroke::new(1.6, theme::CANVAS);
            let c = b.center();
            ui.painter().line_segment([c + Vec2::new(-3.0, 0.0), c + Vec2::new(-1.0, 2.2)], st);
            ui.painter().line_segment([c + Vec2::new(-1.0, 2.2), c + Vec2::new(3.2, -2.4)], st);
        } else {
            ui.painter().rect_stroke(b, CornerRadius::same(3), Stroke::new(1.2, theme::FAINT), egui::StrokeKind::Inside);
        }
        ui.painter().galley(egui::pos2(rect.left() + 20.0, cy - galley.size().y / 2.0), galley, theme::FAINT);
        if resp.clicked() {
            self.placement = if on { Placement::Local } else { Placement::Worktree };
        }
        resp.on_hover_text(if on {
            "The turn runs in a fresh worktree of this repo; this checkout stays as it is."
        } else {
            "The turn runs on this checkout, in place."
        });
        rect.left() - GAP - 4.0
    }
}

/// When a file here last changed, off the probe's bounded walk.
fn timing(facts: Option<&RepoFacts>) -> Option<Fact> {
    let f = facts?;
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
    Some(Fact { id: "time", icon: Some((icons::clock, theme::FAINTER)), parts: vec![(text, theme::FAINTER)], hint })
}

/// Whether the branch merges into its base, as of the last fetch.
fn merge(facts: Option<&RepoFacts>) -> Option<Fact> {
    let f = facts?;
    // No base branch to merge into is not a verdict, so nothing is shown.
    let base = f.base.as_ref()?;
    let short = short_base(base);
    let (words, mark, ink) = match (&f.merge, f.base_behind) {
        (Some(Merge::Clean), _) => (format!("merges into {short}"), theme::OK, theme::FAINT),
        (Some(Merge::Conflicts(files)), _) => {
            let n = files.len().max(1);
            (format!("{n} conflict{} with {short}", if n == 1 { "" } else { "s" }), theme::RUST, theme::RUST)
        }
        (Some(Merge::UpToDate), Some(b)) if b > 0 => (format!("{b} behind {short}"), theme::FAINTER, theme::FAINT),
        (Some(Merge::UpToDate), _) => (format!("even with {short}"), theme::FAINTER, theme::FAINT),
        (Some(Merge::NotRun(_)), _) => (format!("merge into {short} not checked"), theme::FAINTER, theme::FAINTER),
        (None, _) => (format!("merge into {short} unknown"), theme::FAINTER, theme::FAINTER),
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
    Some(Fact { id: "merge", icon: Some((icons::merge, mark)), parts: vec![(words, ink)], hint })
}

/// The uncommitted change, in lines against HEAD, and the untracked files.
fn changes(facts: Option<&RepoFacts>) -> Option<Fact> {
    let Some(f) = facts else {
        return Some(Fact {
            id: "changes",
            icon: None,
            parts: vec![("reading git".to_string(), theme::FAINTER)],
            hint: "git is being asked about this folder.".to_string(),
        });
    };
    let (a, r) = (f.added?, f.removed?);
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
        parts.push(("no changes".to_string(), theme::FAINTER));
    }
    let hint = format!(
        "Uncommitted, against HEAD: {a} lines added and {r} removed ({} files modified, {} staged).\n{u} untracked file{}, whose lines git does not count until they are added.",
        f.modified.unwrap_or(0),
        f.staged.unwrap_or(0),
        if u == 1 { "" } else { "s" },
    );
    Some(Fact { id: "changes", icon: None, parts, hint })
}
