//! Icons drawn from primitives.
//!
//! No icon font, because a face the window has not verified renders as tofu,
//! and the first rail proved it. Each icon is a few strokes at a given centre
//! and size, and every one is legible at 14px in the sidebar and 16px beside a
//! heading.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

fn s(tone: Color32) -> Stroke {
    Stroke::new(1.5, tone)
}

pub fn folder(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    let r = Rect::from_center_size(c + Vec2::new(0.0, 1.0), Vec2::new(14.0, 10.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
    // The tab.
    p.line_segment([Pos2::new(r.left(), r.top()), Pos2::new(r.left() + 5.0, r.top() - 2.5)], st);
    p.line_segment([Pos2::new(r.left() + 5.0, r.top() - 2.5), Pos2::new(r.left() + 8.0, r.top())], st);
}

/// A git branch: a trunk with one fork.
pub fn branch(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.circle_stroke(c + Vec2::new(-4.0, -5.0), 2.2, st);
    p.circle_stroke(c + Vec2::new(-4.0, 5.0), 2.2, st);
    p.circle_stroke(c + Vec2::new(4.0, -4.0), 2.2, st);
    p.line_segment([c + Vec2::new(-4.0, -2.8), c + Vec2::new(-4.0, 2.8)], st);
    p.line_segment([c + Vec2::new(4.0, -1.8), c + Vec2::new(-3.0, 4.0)], st);
}

pub fn clock(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.circle_stroke(c, 6.5, st);
    p.line_segment([c, c + Vec2::new(0.0, -4.0)], st);
    p.line_segment([c, c + Vec2::new(3.0, 1.5)], st);
}

/// A diff: plus over minus.
pub fn changes(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.line_segment([c + Vec2::new(-6.0, -3.5), c + Vec2::new(-0.5, -3.5)], st);
    p.line_segment([c + Vec2::new(-3.25, -6.25), c + Vec2::new(-3.25, -0.75)], st);
    p.line_segment([c + Vec2::new(0.5, 4.0), c + Vec2::new(6.0, 4.0)], st);
}

pub fn plus(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], st);
    p.line_segment([c + Vec2::new(0.0, -5.0), c + Vec2::new(0.0, 5.0)], st);
}

/// Send: an arrow pointing up, the way every one of the references draws it.
pub fn arrow_up(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = Stroke::new(1.8, tone);
    p.line_segment([c + Vec2::new(0.0, 5.0), c + Vec2::new(0.0, -5.0)], st);
    p.line_segment([c + Vec2::new(-4.5, -0.5), c + Vec2::new(0.0, -5.0)], st);
    p.line_segment([c + Vec2::new(4.5, -0.5), c + Vec2::new(0.0, -5.0)], st);
}

pub fn chevron_down(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.line_segment([c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.0)], st);
    p.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(4.0, -2.0)], st);
}

/// The product mark: a facet, filled.
pub fn mark(p: &egui::Painter, c: Pos2, r: f32, tone: Color32) {
    p.add(egui::Shape::convex_polygon(
        vec![
            c + Vec2::new(0.0, -r),
            c + Vec2::new(r * 0.86, 0.0),
            c + Vec2::new(0.0, r),
            c + Vec2::new(-r * 0.86, 0.0),
        ],
        tone,
        Stroke::NONE,
    ));
}

/// Allocates a square and draws an icon in it, so an icon can sit inline in a
/// horizontal row beside text.
pub fn inline(ui: &mut egui::Ui, size: f32, tone: Color32, draw: fn(&egui::Painter, Pos2, Color32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    draw(ui.painter(), rect.center(), tone);
}
