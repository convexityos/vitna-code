//! Vitna's material, ported from `apps/vitna-desktop/src/styles/tokens.css`,
//! with the three faces it names bundled under `assets/fonts/`.
//!
//! The values are the same hexes rather than an approximation, so the native
//! window and the reference surface cannot drift into two different products.
//! What the tokens say, and this module keeps:
//!
//!   Grounds  blue-black canvas, a charcoal-blue working ground a step up,
//!            faces above that, fields sunk below, and a plate a step BELOW
//!            the ground for the one object that is a record rather than
//!            furniture.
//!   Accents  periwinkle for what the window asks of a person and where they
//!            stand; rust for a state somebody has to act on; green as one dot.
//!   Rule     colour carries state and never decoration, and the word is
//!            always printed beside it.
//!
//! No glass, no glow, no gradient.

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

// Neutral, with a whisper of cool: every ground keeps its blue channel a few
// points over its red, no more, so the periwinkle is the only blue thing on
// screen. The main area and the sidebar sit on one floor, and a visibly
// lighter hairline draws the edges between them, since two near-blacks side
// by side need a line to be two things. The composer sits highest of the
// grounds, with the brightest line, so the thing you type into is the thing
// you see.
pub const CANVAS: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x10);
pub const RAIL: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x10);
pub const FACE: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x24);
pub const GROUND: Color32 = Color32::from_rgb(0x12, 0x13, 0x18);
pub const FACE_2: Color32 = Color32::from_rgb(0x24, 0x25, 0x2d);
pub const FIELD: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x21);
pub const CONTROL: Color32 = Color32::from_rgb(0x24, 0x25, 0x2d);

pub const INK: Color32 = Color32::from_rgb(0xf2, 0xf3, 0xf6);
pub const INK_2: Color32 = Color32::from_rgb(0xdf, 0xe1, 0xe7);
pub const MUTE: Color32 = Color32::from_rgb(0xc6, 0xc9, 0xd2);
pub const FAINT: Color32 = Color32::from_rgb(0xa5, 0xa9, 0xb5);
pub const FAINTER: Color32 = Color32::from_rgb(0x8c, 0x90, 0x9c);

pub const PERI: Color32 = Color32::from_rgb(0x71, 0x88, 0xff);
pub const PERI_2: Color32 = Color32::from_rgb(0x8e, 0xa2, 0xff);
pub const RUST: Color32 = Color32::from_rgb(0xd9, 0xa4, 0x41);
pub const OK: Color32 = Color32::from_rgb(0x2f, 0xbf, 0x71);

/// The light line that delineates things: the sidebar's edge, the composer,
/// the popups. HAIR_2 is its quieter sibling for rules inside a surface and
/// the outline of a chip.
pub const HAIR: Color32 = Color32::from_rgb(0x34, 0x36, 0x3f);
pub const HAIR_2: Color32 = Color32::from_rgb(0x24, 0x26, 0x2e);


/// Widget radii: the field and the button.
pub const R: f32 = 16.0;
pub const R_XS: f32 = 8.0;

/// The type floor. Nothing in this window is set smaller than MARK.
pub const FS_MARK: f32 = 11.0;
pub const FS_META: f32 = 12.0;
pub const FS_SMALL: f32 = 13.5;
pub const FS_BODY: f32 = 15.0;

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
const SPACE_GROTESK: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Variable.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Variable.ttf");

/// Weights. The variable masters default to Regular, which on a dark ground
/// reads thin; these are the cuts the window actually uses.
const WGHT_UI: f32 = 440.0;
const WGHT_DISPLAY: f32 = 540.0;
const WGHT_PROSE: f32 = 370.0;
const WGHT_MONO: f32 = 440.0;

/// Inter is every word in the window except the header: labels, controls,
/// chips, meta lines, sentences and the composer. The owner's rule is that a
/// font change is universal except for the header, so the UI face and the
/// prose face change together. Space Grotesk keeps only the display cut (the
/// "Let's build" headline, the lockup's "Code", page titles), where its
/// character is the point rather than a distraction.
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
    fonts.font_data.insert("SpaceGrotesk".to_owned(), face(SPACE_GROTESK, WGHT_UI));
    fonts.font_data.insert("SpaceGroteskDisplay".to_owned(), face(SPACE_GROTESK, WGHT_DISPLAY));
    fonts.font_data.insert("JetBrainsMono".to_owned(), face(JETBRAINS_MONO, WGHT_MONO));

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
        .insert(FontFamily::Proportional, chain(&["Inter", "SpaceGrotesk"]));
    fonts.families.insert(
        FontFamily::Name("display".into()),
        chain(&["SpaceGroteskDisplay", "SpaceGrotesk"]),
    );
    fonts
        .families
        .insert(FontFamily::Name("prose".into()), chain(&["InterProse", "Inter"]));
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono".to_owned());
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {

        style.text_styles = [
            (TextStyle::Heading, sans(20.0)),
            // Body is what an unstyled label and every hover text draws in, so it
        // is the paragraph face: any sentence nobody styled explicitly is prose.
        (TextStyle::Body, prose(FS_BODY)),
            (TextStyle::Monospace, mono(FS_SMALL)),
            (TextStyle::Button, sans(FS_SMALL)),
            (TextStyle::Small, sans(FS_META)),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = CANVAS;
        v.window_fill = GROUND;
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = FACE;
        v.override_text_color = Some(INK_2);
        v.window_stroke = egui::Stroke::new(1.0, HAIR);
        v.selection.bg_fill = Color32::from_rgb(0x2a, 0x2c, 0x36);
        v.selection.stroke = egui::Stroke::new(1.0, PERI_2);
        v.hyperlink_color = PERI_2;

        v.widgets.noninteractive.bg_fill = GROUND;
        v.widgets.noninteractive.weak_bg_fill = GROUND;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, HAIR_2);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTE);
        v.widgets.noninteractive.corner_radius = (R_XS as u8).into();

        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = FACE;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, HAIR_2);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK_2);
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

        // Shadows exist in exactly one place, between the desk and the device,
        // and that is painted by hand in app.rs. Every popup and window stays
        // flat, because a shadow on a working surface is glow by another name.
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.popup_shadow = egui::epaint::Shadow::NONE;
        v.window_corner_radius = (R as u8).into();

        // A thin, floating scrollbar. The default draws a pale strip down the
        // middle of the console, which reads as a seam in the layout.
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        style.spacing.scroll.floating = true;
        style.spacing.scroll.bar_width = 5.0;

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.interact_size.y = 26.0;
    });
}


/// A named sidebar, not an icon rail. Every terminal this sits beside lists
/// real work by name, and an icon rail is Convexity's answer to twenty
/// destinations rather than to one plus a list of sessions.
pub const SIDEBAR_W: f32 = 244.0;

/// An eyebrow: sans, small, uppercase, wide tracking, tertiary.
///
/// Sans and not mono on purpose. Convexity moved these labels off the mono role
/// because when everything is mono nothing scans, and mono is the value's
/// voice, never the label's.
pub fn eyebrow(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        egui::TextFormat {
            font_id: sans(FS_MARK),
            extra_letter_spacing: 1.1,
            color: FAINTER,
            ..Default::default()
        },
    );
    let _ = ui;
    job
}

