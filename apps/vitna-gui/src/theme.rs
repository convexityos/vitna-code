//! Vitna's material. It began as a port of
//! `apps/vitna-desktop/src/styles/tokens.css` and has since been re-stepped
//! here, rung by rung on the owner's word, so the two no longer match: this
//! file is the window's own, and copying the stylesheet's hexes back would
//! undo those decisions. Inter, bundled under `assets/fonts/`, sets all text.
//!
//!   Grounds  neutral with a whisper of cool, the side you work in a step
//!            lighter than the rail, and what is raised a step over that.
//!   Inks     three, and warm: a white, and two greys that are that white
//!            dimmed.
//!   Accents  periwinkle for what the window asks of a person and where they
//!            stand; rust for a state somebody has to act on; green as one dot.
//!   Rule     colour carries state and never decoration, and the word is
//!            always printed beside it.
//!   Frame    the sidebar sits on the desk, and the working side is a panel
//!            standing on it: inset from the window's edges, one hairline,
//!            and the one shadow a working surface casts. Borrowed from the
//!            owner's new design for Callsider and Convexity (2026-09-22),
//!            whose rail and panel are this shape.
//!
//! No glass, no glow, no gradient.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Rect, Stroke, TextStyle};

// Neutral, with a whisper of cool: every ground keeps its blue channel a few
// points over its red, no more, so the periwinkle is the only blue thing on
// screen. The sidebar and the title bar sit on the floor (RAIL, the desk), and
// the working side is a panel one small step up from it (CANVAS) with the
// desk showing round its edges, so the side you work in is the lit one and
// its own hairline is the only edge between them. The inspector is a column
// of that panel rather than a second ground. GROUND, for the popups and the
// modals, keeps the same step over CANVAS that it used to keep over the
// floor, so what floats over the panel still reads as raised. The composer field is a lit well with
// the brightest line of all, so the thing you type into is the thing you
// see; the controls (buttons, chips, the chosen session) carry the brightest
// FILLS, a step over the field, so each one reads as a thing to press rather
// than a smudge on the floor.
pub const CANVAS: Color32 = Color32::from_rgb(0x10, 0x11, 0x15);
pub const RAIL: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x10);
pub const GROUND: Color32 = Color32::from_rgb(0x17, 0x18, 0x1e);
pub const FIELD: Color32 = Color32::from_rgb(0x1f, 0x20, 0x27);
pub const FACE: Color32 = Color32::from_rgb(0x23, 0x25, 0x2d);
pub const FACE_2: Color32 = Color32::from_rgb(0x2d, 0x2f, 0x38);
pub const CONTROL: Color32 = Color32::from_rgb(0x2d, 0x2f, 0x38);

// Three inks, and no more: what you read, what explains it, and what you can
// ignore. There were five, two of them thirteen levels apart, which at 12 and
// 13 pixels no eye can tell apart; they read as noise, not as hierarchy. A
// chosen row, a pressed control or an open page says so with its ground, not
// with a fourth ink.
//
// The whites are warm where the grounds are cool: INK is the owner's #efeeeb
// (2026-09-18), and the greys under it are that white dimmed rather than
// greys of their own. Each keeps the lightness it had as a cool grey, so its
// contrast on every ground holds to a hundredth, and takes INK's exact tint,
// one point of red over the green and three of blue under it. Warmer and they
// turn khaki; left cool, a meta line reads bluer than the title above it.
pub const INK: Color32 = Color32::from_rgb(0xef, 0xee, 0xeb);
pub const FAINT: Color32 = Color32::from_rgb(0xaa, 0xa9, 0xa6);
pub const FAINTER: Color32 = Color32::from_rgb(0x91, 0x90, 0x8d);

pub const PERI: Color32 = Color32::from_rgb(0x71, 0x88, 0xff);
pub const PERI_2: Color32 = Color32::from_rgb(0x8e, 0xa2, 0xff);
pub const RUST: Color32 = Color32::from_rgb(0xd9, 0xa4, 0x41);
pub const OK: Color32 = Color32::from_rgb(0x2f, 0xbf, 0x71);
/// A diff's minus sign, and nothing else. Lines removed read red in every
/// tool this sits beside, and a second meaning for it would be a lie.
pub const DEL: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);

/// The light line that delineates things: the sidebar's edge, the composer,
/// the popups. HAIR_2 is its quieter sibling for rules inside a surface and
/// the outline of a chip.
pub const HAIR: Color32 = Color32::from_rgb(0x3c, 0x3f, 0x4a);
pub const HAIR_2: Color32 = Color32::from_rgb(0x30, 0x32, 0x3c);


/// The one shadow in the window, cast by what floats: search, the settings
/// modal, menus and popovers. Dark rather than lit, so a layer reads as above
/// the page without glowing; the page itself stays flat.
pub const LIFT: egui::epaint::Shadow = egui::epaint::Shadow {
    offset: [0, 12],
    blur: 32,
    spread: 0,
    color: Color32::from_black_alpha(150),
};

/// How far a hover has faded in, from 0 to 1 over 120ms, so a ground eases
/// in and out under the pointer rather than snapping.
pub fn hover(ui: &egui::Ui, id: egui::Id, hovered: bool) -> f32 {
    ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12)
}

/// Widget radii: the field and the button.
pub const R: f32 = 16.0;
pub const R_XS: f32 = 8.0;

/// How far the panel stands in from the window's edges, where the desk shows.
pub const PANEL_INSET: f32 = 8.0;
/// The panel's corners: under the field's 16, over a row's 8, so the three
/// read as one family at three sizes rather than as three shapes.
pub const R_PANEL: u8 = 12;
/// The panel's bar, which says where you are as a trail of the pages above
/// this one, and the height the sidebar's lockup shares so the two read as
/// one line across the window.
pub const BAR_H: f32 = 44.0;

/// Paints the working panel: its one shadow, its ground and its hairline.
///
/// The shadow is the new design's, `0 30px 60px -40px`: it falls well below
/// the panel and is pulled in on every side, so it deepens the desk under the
/// panel's foot without ringing it. egui's shadow has no negative spread, so
/// it is cast from the panel's rect pulled in by that much, which is the same
/// shape. Nothing else on a working surface casts one.
pub fn panel(p: &egui::Painter, rect: Rect) {
    let shadow = egui::epaint::Shadow {
        offset: [0, 30],
        blur: 60,
        spread: 0,
        color: Color32::from_black_alpha(200),
    };
    p.add(shadow.as_shape(rect.shrink(40.0), CornerRadius::same(R_PANEL)));
    p.rect(
        rect,
        CornerRadius::same(R_PANEL),
        CANVAS,
        Stroke::new(1.0, HAIR_2),
        egui::StrokeKind::Inside,
    );
}

/// The grain's tile, in texels. Drawn one texel to one screen pixel, so it
/// never scales into blotches.
const GRAIN_N: usize = 128;

/// Vitna's grain: a tile of fine white noise laid over the panel, the native
/// counterpart of the `feTurbulence` tile vitna.ai lays over each of its
/// surfaces, for the reason its guardrails give: "a smooth surface is what
/// reads as generated". It is kept faint enough that it is felt rather than
/// seen, never more than a twentieth of white on any texel, and it comes from
/// a fixed seed, so every window and every capture draws the same one.
pub fn grain_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let mut state: u32 = 0x9e37_79b9;
    let pixels = (0..GRAIN_N * GRAIN_N)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let alpha = ((state >> 24) * 12 / 255) as u8;
            Color32::from_white_alpha(alpha)
        })
        .collect();
    ctx.load_texture(
        "vitna-grain",
        egui::ColorImage::new([GRAIN_N, GRAIN_N], pixels),
        egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        },
    )
}

/// Lays the grain over `rect`, inside the panel's corners.
pub fn grain(p: &egui::Painter, rect: Rect, texture: egui::TextureId) {
    let ppp = p.ctx().pixels_per_point();
    let n = GRAIN_N as f32;
    let uv = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(rect.width() * ppp / n, rect.height() * ppp / n));
    p.add(
        egui::epaint::RectShape::filled(rect, CornerRadius::same(R_PANEL), Color32::WHITE).with_texture(texture, uv),
    );
}

/// A hash as the Vitna record prints one short: the first eight characters and
/// the last four, so two digests that share a prefix still read as two. The
/// whole of it waits on the hover, the inspector and the copy.
pub fn digest(hash: &str) -> String {
    let n = hash.chars().count();
    if n <= 14 {
        return hash.to_string();
    }
    let head: String = hash.chars().take(8).collect();
    let tail: String = hash.chars().skip(n - 4).collect();
    format!("{head}\u{2026}{tail}")
}

/// The type scale: seven sizes, and nothing between them. Fourteen had
/// accumulated as literals, six of them between 11 and 13.5, which reads as
/// carelessness rather than as a choice; a test now refuses a literal size
/// anywhere outside this file. Nothing is set smaller than MICRO.
///
/// MICRO is eyebrows, key caps and the smallest meta; META the quiet line
/// under a title; UI every label, control and sentence in a panel; TITLE a
/// row's title and the sidebar's names; BODY what a person types and reads at
/// length; HEAD a page's heading; HERO the one line on an empty start screen.
pub const FS_MICRO: f32 = 11.0;
pub const FS_META: f32 = 12.0;
pub const FS_UI: f32 = 13.0;
pub const FS_TITLE: f32 = 14.0;
pub const FS_BODY: f32 = 15.0;
pub const FS_HEAD: f32 = 20.0;
pub const FS_HERO: f32 = 28.0;

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// The faces the design names, bundled as the OFL variable TrueType files under
/// `assets/fonts/`, each pinned to a weight on its `wght` axis at registration
/// so every label in the window draws at that weight. Bundled rather than
/// probed from the host, so the window draws the same face on every machine.
/// The OFL permits exactly this: bundling and redistribution with software, as
/// long as the fonts are not sold on their own and the licence travels with
/// them, which is why each file has its OFL.txt beside it.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");

/// Weights. The variable masters default to Regular, which on a dark ground
/// reads thin; these are the cuts the window actually uses.
const WGHT_UI: f32 = 440.0;
const WGHT_DISPLAY: f32 = 540.0;
const WGHT_PROSE: f32 = 370.0;

/// Inter is all of the text in the window, on the owner's instruction
/// (2026-09-17): labels, controls, chips, meta lines, sentences, the composer,
/// the headline and the code-like text (paths, hashes, the receipt JSON). The
/// roles survive as WEIGHTS on Inter's `wght` axis rather than as separate
/// faces: 540 for the display cut, 440 for the interface, 370 for sentences.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

/// Sentences: the one or two lines of explanation a view carries, hover text
/// and what is typed into the composer. The same face as the UI, one step
/// lighter on its weight axis, which a variable face can do and a static one
/// cannot.
pub fn prose(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("prose".into()))
}

fn face(bytes: &'static [u8], weight: f32) -> std::sync::Arc<egui::FontData> {
    std::sync::Arc::new(egui::FontData::from_static(bytes).tweak(egui::FontTweak {
        coords: egui::epaint::text::VariationCoords::new([(*b"wght", weight)]),
        ..Default::default()
    }))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // One file, registered twice at two weights. Both entries borrow the same
    // static bytes, so the binary carries the face once.
    fonts.font_data.insert("Inter".to_owned(), face(INTER, WGHT_UI));
    fonts.font_data.insert("InterProse".to_owned(), face(INTER, WGHT_PROSE));
    fonts.font_data.insert("InterDisplay".to_owned(), face(INTER, WGHT_DISPLAY));

    // Each family leads with the design's face and keeps egui's defaults
    // behind it, so a glyph the face lacks (emoji, a stray symbol) still draws
    // rather than boxing.
    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let chain = |lead: &[&str]| -> Vec<String> {
        lead.iter()
            .map(|s| (*s).to_owned())
            .chain(defaults.iter().cloned())
            .collect()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, chain(&["Inter"]));
    fonts
        .families
        .insert(FontFamily::Name("display".into()), chain(&["InterDisplay", "Inter"]));
    fonts
        .families
        .insert(FontFamily::Name("prose".into()), chain(&["InterProse", "Inter"]));
    // Code-like text too. Inter is proportional, so hashes and the receipt
    // JSON lose column alignment; egui's own monospace face stays behind it
    // only as a fallback for glyphs Inter lacks.
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "Inter".to_owned());
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {

        style.text_styles = [
            (TextStyle::Heading, sans(FS_HEAD)),
            // Body is what an unstyled label and every hover text draws in, so it
        // is the paragraph face: any sentence nobody styled explicitly is prose.
        (TextStyle::Body, prose(FS_BODY)),
            (TextStyle::Monospace, mono(FS_UI)),
            (TextStyle::Button, sans(FS_UI)),
            (TextStyle::Small, sans(FS_META)),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = CANVAS;
        v.window_fill = GROUND;
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = FACE;
        v.override_text_color = Some(INK);
        v.window_stroke = egui::Stroke::new(1.0, HAIR);
        v.selection.bg_fill = Color32::from_rgb(0x2a, 0x2c, 0x36);
        v.selection.stroke = egui::Stroke::new(1.0, PERI_2);
        v.hyperlink_color = PERI_2;

        v.widgets.noninteractive.bg_fill = GROUND;
        v.widgets.noninteractive.weak_bg_fill = GROUND;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, HAIR_2);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, FAINT);
        v.widgets.noninteractive.corner_radius = (R_XS as u8).into();

        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = FACE;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, HAIR_2);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK);
        v.widgets.inactive.corner_radius = (R_XS as u8).into();

        v.widgets.hovered.bg_fill = FACE_2;
        v.widgets.hovered.weak_bg_fill = FACE_2;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, HAIR);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, INK);
        v.widgets.hovered.corner_radius = (R_XS as u8).into();

        v.widgets.active.bg_fill = CONTROL;
        v.widgets.active.weak_bg_fill = CONTROL;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, PERI);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, INK);
        v.widgets.active.corner_radius = (R_XS as u8).into();

        v.widgets.open.bg_fill = FACE;
        v.widgets.open.weak_bg_fill = FACE;
        v.widgets.open.bg_stroke = egui::Stroke::new(1.0, HAIR);
        v.widgets.open.fg_stroke = egui::Stroke::new(1.0, INK);

        // What floats casts the one shadow; working surfaces stay flat.
        v.window_shadow = LIFT;
        v.popup_shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(130),
        };
        v.window_corner_radius = (R as u8).into();

        // A thin scrollbar that is there only while the pointer is over what
        // it scrolls. The default draws a pale strip down the middle of the
        // console, and even a thin one at rest drew a full-height line beside
        // the run and the receipt, which reads as a seam in the layout.
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.scroll.bar_width = 5.0;

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.interact_size.y = 26.0;
    });
}


/// One line of text cut to `max_w` with an ellipsis, rather than wrapped or
/// run on under whatever sits beside it. For painter-drawn rows, where egui's
/// own label truncation is not in play.
pub fn line(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(0.0));
    ui.painter().layout_job(job)
}

/// Long text cut for a hover card, so a pasted essay of a prompt does not
/// become a tooltip taller than the window.
pub fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('\u{2026}');
    }
    out
}

/// A named sidebar, not an icon rail. Every terminal this sits beside lists
/// real work by name, and an icon rail is Convexity's answer to twenty
/// destinations rather than to one plus a list of sessions.
pub const SIDEBAR_W: f32 = 244.0;

/// A group's name over its rows, or a column's over its cells: words set as
/// words, one size under the rows, in the quietest ink.
///
/// These were uppercase with wide tracking. That label is the one every
/// generated dashboard wears, and the owner's new design (2026-09-22) and
/// Vitna's own guardrails ("Uppercase labels used as decoration") both
/// refuse it, so a group is named in sentence case, the way a person would
/// write it.
pub fn label(p: &egui::Painter, at: egui::Pos2, align: egui::Align2, text: &str) -> Rect {
    p.text(at, align, text, sans(FS_META), FAINTER)
}


#[cfg(test)]
mod ink_tests {
    use super::{FAINT, FAINTER, INK};

    /// The greys are the white dimmed, so they carry its tint exactly. A white
    /// changed on its own leaves every meta line a different colour from the
    /// title above it; a hex named for one rung re-steps the other two.
    #[test]
    fn the_three_inks_are_one_white_at_three_lightnesses() {
        let tint = |c: eframe::egui::Color32| {
            (i16::from(c.r()) - i16::from(c.g()), i16::from(c.b()) - i16::from(c.g()))
        };
        assert_eq!(tint(FAINT), tint(INK), "FAINT does not carry INK's tint; re-step it with INK");
        assert_eq!(tint(FAINTER), tint(INK), "FAINTER does not carry INK's tint; re-step it with INK");
        assert!(INK.g() > FAINT.g() && FAINT.g() > FAINTER.g(), "the inks no longer step down");
    }
}

#[cfg(test)]
mod digest_tests {
    use super::digest;

    /// Eight and four, so two digests that share a prefix still read as two;
    /// anything short enough to print whole is printed whole.
    #[test]
    fn a_digest_keeps_its_head_and_its_tail() {
        let h = "0c86bbcdf76d4e44a9059a687a41fcd1ea01197c7fc5413ada1453a9a2139b0e";
        assert_eq!(digest(h), "0c86bbcd\u{2026}9b0e");
        assert_eq!(digest("e3f660422b08"), "e3f660422b08");
        assert_eq!(digest(""), "");
    }
}

#[cfg(test)]
mod scale_tests {
    /// Every size in the window comes off the scale above. A literal is how
    /// fourteen sizes accumulated, six of them between 11 and 13.5, and it is
    /// the one edit nobody would notice in review.
    #[test]
    fn no_font_size_is_written_outside_the_scale() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src") {
            let path = entry.expect("entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !name.ends_with(".rs") || name == "theme.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            for (i, line) in text.lines().enumerate() {
                for face in ["theme::sans(", "theme::prose(", "theme::display(", "theme::mono("] {
                    for (at, _) in line.match_indices(face) {
                        let rest = &line[at + face.len()..];
                        if rest.starts_with(|c: char| c.is_ascii_digit()) {
                            offenders.push(format!("{name}:{}", i + 1));
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "sizes written as literals, not off the scale: {offenders:?}");
    }
}
