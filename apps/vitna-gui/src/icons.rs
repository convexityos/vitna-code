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

/// Allocates a square and draws an icon in it, so an icon can sit inline in a
/// horizontal row beside text.
pub fn inline(ui: &mut egui::Ui, size: f32, tone: Color32, draw: fn(&egui::Painter, Pos2, Color32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    draw(ui.painter(), rect.center(), tone);
}

pub fn search(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.circle_stroke(c + Vec2::new(-1.5, -1.5), 5.0, st);
    p.line_segment([c + Vec2::new(2.2, 2.2), c + Vec2::new(6.0, 6.0)], st);
}

pub fn check(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = Stroke::new(1.8, tone);
    p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(-1.5, 3.5)], st);
    p.line_segment([c + Vec2::new(-1.5, 3.5), c + Vec2::new(5.5, -3.5)], st);
}

/// Three sliders, the settings mark every reference uses.
pub fn sliders(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    for (dy, knob) in [(-4.5, -2.0), (0.0, 3.0), (4.5, -1.0)] {
        p.line_segment([c + Vec2::new(-6.0, dy), c + Vec2::new(6.0, dy)], st);
        p.circle_filled(c + Vec2::new(knob, dy), 1.8, tone);
    }
}

/// A screen on a stand: where the turn runs.
pub fn monitor(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    let r = Rect::from_center_size(c + Vec2::new(0.0, -1.5), Vec2::new(13.0, 9.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
    p.line_segment([c + Vec2::new(0.0, 3.0), c + Vec2::new(0.0, 5.5)], st);
    p.line_segment([c + Vec2::new(-4.0, 5.5), c + Vec2::new(4.0, 5.5)], st);
}

/// The return key: down, then left, with a head.
pub fn enter(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = Stroke::new(1.3, tone);
    p.line_segment([c + Vec2::new(4.0, -3.5), c + Vec2::new(4.0, 1.0)], st);
    p.line_segment([c + Vec2::new(4.0, 1.0), c + Vec2::new(-3.5, 1.0)], st);
    p.line_segment([c + Vec2::new(-3.5, 1.0), c + Vec2::new(-1.0, -1.5)], st);
    p.line_segment([c + Vec2::new(-3.5, 1.0), c + Vec2::new(-1.0, 3.5)], st);
}

/// Three lines: the menu.
pub fn hamburger(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    for dy in [-4.5, 0.0, 4.5] {
        p.line_segment([c + Vec2::new(-6.0, dy), c + Vec2::new(6.0, dy)], st);
    }
}

/// A window with its sidebar: the toggle.
pub fn sidebar(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    let r = Rect::from_center_size(c, Vec2::new(14.0, 11.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
    p.line_segment([Pos2::new(r.left() + 5.0, r.top()), Pos2::new(r.left() + 5.0, r.bottom())], st);
}

/// A key cap row: shortcuts.
pub fn keyboard(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    let r = Rect::from_center_size(c, Vec2::new(14.0, 9.0));
    p.rect_stroke(r, 2.0, st, egui::StrokeKind::Middle);
    for dx in [-4.0, 0.0, 4.0] {
        p.circle_filled(c + Vec2::new(dx, -1.5), 0.9, tone);
    }
    p.line_segment([c + Vec2::new(-3.0, 2.0), c + Vec2::new(3.0, 2.0)], st);
}

/// Two stacked units: the daemon.
pub fn server(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    for dy in [-3.0, 3.0] {
        let r = Rect::from_center_size(c + Vec2::new(0.0, dy), Vec2::new(14.0, 5.0));
        p.rect_stroke(r, 1.5, st, egui::StrokeKind::Middle);
        p.circle_filled(c + Vec2::new(-4.5, dy), 0.9, tone);
    }
}

/// Four cells: providers.
pub fn grid(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    for (dx, dy) in [(-3.5, -3.5), (3.5, -3.5), (-3.5, 3.5), (3.5, 3.5)] {
        let r = Rect::from_center_size(c + Vec2::new(dx, dy), Vec2::splat(5.0));
        p.rect_stroke(r, 1.0, st, egui::StrokeKind::Middle);
    }
}

/// A four-point star: models.
pub fn sparkle(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.line_segment([c + Vec2::new(0.0, -6.5), c + Vec2::new(0.0, 6.5)], st);
    p.line_segment([c + Vec2::new(-6.5, 0.0), c + Vec2::new(6.5, 0.0)], st);
    p.line_segment([c + Vec2::new(-3.0, -3.0), c + Vec2::new(3.0, 3.0)], Stroke::new(1.0, tone));
    p.line_segment([c + Vec2::new(3.0, -3.0), c + Vec2::new(-3.0, 3.0)], Stroke::new(1.0, tone));
}

/// A cross: close.
pub fn close(p: &egui::Painter, c: Pos2, tone: Color32) {
    let st = s(tone);
    p.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], st);
    p.line_segment([c + Vec2::new(4.5, -4.5), c + Vec2::new(-4.5, 4.5)], st);
}
