//! The few shapes every page of the panel is built from, so no page grows its
//! own: the trail at the head of the panel, a section's head, and the rule.
//!
//! They come from the owner's new design (the Callsider and Convexity drafts,
//! 2026-09-22): sections are separated by air and a hairline rather than by
//! boxes, each head is a name and a count, and the bar over a page is a trail
//! of the pages above it, every step but the last a way back.

use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, Vec2};

use crate::theme;

/// The trail in the panel's bar, laid from `left`. Every crumb but the last is
/// a way back; returns the index of the one clicked.
pub fn crumbs(ui: &egui::Ui, bar: Rect, left: f32, trail: &[String]) -> Option<usize> {
    let p = ui.painter();
    let cy = bar.center().y;
    let mut x = left;
    let mut clicked = None;
    for (i, crumb) in trail.iter().enumerate() {
        let last = i + 1 == trail.len();
        if i > 0 {
            let slash = p.text(egui::pos2(x, cy), Align2::LEFT_CENTER, "/", theme::sans(theme::FS_UI), theme::FAINTER);
            x = slash.right() + 8.0;
        }
        let room = (bar.right() - 20.0 - x).max(0.0);
        let tone = if last { theme::INK } else { theme::FAINT };
        let g = theme::line(ui, crumb, theme::sans(theme::FS_UI), tone, room);
        let r = Rect::from_min_size(egui::pos2(x, cy - g.size().y / 2.0), g.size());
        if last {
            p.galley(r.min, g, tone);
        } else {
            let resp = ui.interact(r.expand2(Vec2::new(4.0, 6.0)), ui.id().with(("crumb", i)), Sense::click());
            let t = theme::hover(ui, resp.id, resp.hovered());
            p.galley(r.min, g, if t > 0.5 { theme::INK } else { tone });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(i);
            }
        }
        x = r.right() + 8.0;
    }
    clicked
}

/// A section's head: its name in the display cut, its count beside it in the
/// quiet ink, and a hairline under both. The count is the count and never a
/// guess, so a section with nothing in it says 0 rather than hiding.
pub fn section(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(title).font(theme::display(theme::FS_TITLE)).color(theme::INK));
        if let Some(n) = count {
            ui.label(RichText::new(n.to_string()).font(theme::sans(theme::FS_META)).color(theme::FAINTER));
        }
    });
    ui.add_space(8.0);
    rule(ui, theme::HAIR_2);
    ui.add_space(6.0);
}

/// A hairline across the available width, in `tone`.
pub fn rule(ui: &mut egui::Ui, tone: Color32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, tone));
}

/// The ground a hovered row takes, eased in, across `rect`.
pub fn hover_ground(ui: &egui::Ui, rect: Rect, id: egui::Id, hot: bool) {
    let t = theme::hover(ui, id, hot);
    if t > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(8), theme::FACE.gamma_multiply(t));
    }
}

/// Facts on one line, joined by a middle dot, each in its own tone. The line
/// is cut to `max_w` with an ellipsis rather than wrapped under its neighbour.
pub fn facts(ui: &egui::Ui, at: egui::Pos2, parts: &[(String, Color32)], max_w: f32) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    for (i, (text, tone)) in parts.iter().filter(|(t, _)| !t.is_empty()).enumerate() {
        if i > 0 {
            job.append(
                "  \u{b7}  ",
                0.0,
                egui::TextFormat { font_id: theme::sans(theme::FS_META), color: theme::FAINTER, ..Default::default() },
            );
        }
        job.append(text, 0.0, egui::TextFormat { font_id: theme::sans(theme::FS_META), color: *tone, ..Default::default() });
    }
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(0.0));
    let g = ui.painter().layout_job(job);
    let r = Rect::from_min_size(egui::pos2(at.x, at.y - g.size().y / 2.0), g.size());
    ui.painter().galley(r.min, g, theme::FAINT);
    r
}
