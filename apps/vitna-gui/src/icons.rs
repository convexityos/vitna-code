//! Icons: Lucide's, bundled.
//!
//! These were drawn from line primitives, and a hand-drawn set is the first
//! thing that makes an interface read as a mock: every glyph a slightly
//! different weight on a slightly different grid. These are Lucide's own
//! (ISC), one 24px grid and one stroke, bundled under `assets/icons/` with the
//! licence and their provenance, stroked white so the window can tint them to
//! any tone. The same SVG loader that draws the mark rasterises each at the
//! size it is drawn times the screen's scale, so they stay crisp at any zoom.

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

/// The size an icon is drawn at, in points, unless a call site asks otherwise.
pub const SIZE: f32 = 16.0;

macro_rules! icons {
    ($($name:ident, $uri:ident => $file:literal;)*) => {
        $(
            pub const $uri: &str = concat!("bytes://icons/", $file, ".svg");
            pub fn $name(p: &egui::Painter, c: Pos2, tone: Color32) {
                at(p, c, tone, $uri, SIZE);
            }
        )*
        const ALL: &[(&str, &[u8])] = &[
            $(($uri, include_bytes!(concat!("../assets/icons/", $file, ".svg"))),)*
        ];
    };
}

icons! {
    folder, FOLDER => "folder";
    merge, MERGE => "git-merge";
    bubble, BUBBLE => "message-square";
    clock, CLOCK => "clock";
    plus, PLUS => "plus";
    arrow_up, ARROW_UP => "arrow-up";
    search, SEARCH => "search";
    check, CHECK => "check";
    sliders, SLIDERS => "sliders-horizontal";
    enter, ENTER => "corner-down-left";
    hamburger, HAMBURGER => "menu";
    sidebar, SIDEBAR => "panel-left";
    keyboard, KEYBOARD => "keyboard";
    server, SERVER => "server";
    grid, GRID => "layout-grid";
    sparkle, SPARKLE => "sparkles";
    close, CLOSE => "x";
    chevron_left, CHEVRON_LEFT => "chevron-left";
    done, DONE => "circle-check";
    failed, FAILED => "circle-x";
    alert, ALERT => "circle-alert";
}

/// Hands every icon's bytes to the image loaders, once, at startup.
pub fn install(ctx: &egui::Context) {
    for (uri, bytes) in ALL {
        ctx.include_bytes(*uri, *bytes);
    }
}

/// Draws the icon at `uri` centred on `c`, `size` points square, in `tone`.
pub fn at(p: &egui::Painter, c: Pos2, tone: Color32, uri: &str, size: f32) {
    let px = (size * p.ctx().pixels_per_point()).round().max(1.0) as u32;
    let hint = egui::SizeHint::Size {
        width: px,
        height: px,
        maintain_aspect_ratio: true,
    };
    match p.ctx().try_load_texture(uri, egui::TextureOptions::LINEAR, hint) {
        Ok(egui::load::TexturePoll::Ready { texture }) => {
            let rect = Rect::from_center_size(c, Vec2::splat(size));
            let uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
            p.image(texture.id, rect, uv, tone);
        }
        Ok(egui::load::TexturePoll::Pending { .. }) => p.ctx().request_repaint(),
        // A bundled file that will not load is a build defect, and the test
        // below is what catches it; at run time it draws nothing rather than
        // a glyph that means something else.
        Err(_) => {}
    }
}

pub fn inline(ui: &mut egui::Ui, size: f32, tone: Color32, draw: fn(&egui::Painter, Pos2, Color32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    draw(ui.painter(), rect.center(), tone);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every bundled icon parses and rasterises, and is stroked white so a
    /// tint can colour it: a black one would multiply every tone to black.
    #[test]
    fn every_bundled_icon_loads_and_takes_a_tint() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        install(&ctx);
        for (uri, bytes) in ALL {
            let text = std::str::from_utf8(bytes).expect("svg is text");
            assert!(text.contains("stroke=\"#ffffff\""), "{uri} is not stroked white");
            assert!(!text.contains("currentColor"), "{uri} still strokes currentColor");
            let hint = egui::SizeHint::Size { width: 32, height: 32, maintain_aspect_ratio: true };
            let poll = ctx.try_load_texture(uri, egui::TextureOptions::LINEAR, hint);
            assert!(
                matches!(poll, Ok(egui::load::TexturePoll::Ready { .. })),
                "{uri} did not load: {:?}",
                poll.err()
            );
        }
    }
}
