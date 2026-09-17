//! Vitna's material, ported from `apps/vitna-desktop/src/styles/tokens.css`.
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

pub const CANVAS: Color32 = Color32::from_rgb(0x0a, 0x0d, 0x18);
pub const RAIL: Color32 = Color32::from_rgb(0x0b, 0x10, 0x20);
pub const GROUND: Color32 = Color32::from_rgb(0x10, 0x15, 0x24);
pub const FACE: Color32 = Color32::from_rgb(0x17, 0x1d, 0x31);
pub const FACE_2: Color32 = Color32::from_rgb(0x1c, 0x23, 0x39);
pub const FIELD: Color32 = Color32::from_rgb(0x0c, 0x10, 0x20);
pub const CONTROL: Color32 = Color32::from_rgb(0x23, 0x2b, 0x45);
pub const PLATE: Color32 = Color32::from_rgb(0x09, 0x0d, 0x1a);

pub const INK: Color32 = Color32::from_rgb(0xf3, 0xf6, 0xfd);
pub const INK_2: Color32 = Color32::from_rgb(0xdf, 0xe5, 0xf4);
pub const MUTE: Color32 = Color32::from_rgb(0xb4, 0xbc, 0xd0);
pub const FAINT: Color32 = Color32::from_rgb(0x96, 0x9f, 0xb8);
pub const FAINTER: Color32 = Color32::from_rgb(0x7e, 0x88, 0xa1);

pub const PERI: Color32 = Color32::from_rgb(0x71, 0x88, 0xff);
pub const PERI_2: Color32 = Color32::from_rgb(0x8e, 0xa2, 0xff);
pub const RUST: Color32 = Color32::from_rgb(0xd9, 0xa4, 0x41);
pub const OK: Color32 = Color32::from_rgb(0x2f, 0xbf, 0x71);

/// Hairlines are the tokens' alpha over the canvas, resolved once here rather
/// than blended per widget.
pub const HAIR: Color32 = Color32::from_rgb(0x25, 0x2a, 0x38);
pub const HAIR_2: Color32 = Color32::from_rgb(0x1a, 0x1f, 0x2c);
/// The faintest rule, for rows inside a table.
pub const HAIR_3: Color32 = Color32::from_rgb(0x14, 0x18, 0x24);

/// Radii, from the tokens' 8 to 20 range.
pub const R: f32 = 16.0;
pub const R_SM: f32 = 12.0;
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

/// A monospace face from the host, when one of the preferred faces is
/// installed.
///
/// The tokens name JetBrains Mono, Space Grotesk and Manrope. None of the three
/// is in this repository, so this window does not claim them: it asks the host
/// for the closest face it actually has, falls back to egui's bundled default,
/// and reports which one it drew with. Bundling the named faces is a licensing
/// decision somebody has to make, not something to paper over with a lookalike.
fn host_mono() -> Option<(String, Vec<u8>)> {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            r"C:\Windows\Fonts\JetBrainsMono-Regular.ttf",
            r"C:\Windows\Fonts\CascadiaMono.ttf",
            r"C:\Windows\Fonts\consola.ttf",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/Library/Fonts/JetBrainsMono-Regular.ttf",
            "/System/Library/Fonts/SFNSMono.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ]
    };

    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let name = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "host-mono".to_string());
            return Some((name, bytes));
        }
    }
    None
}

/// The face this window is actually drawing with. Names what was found rather
/// than what the tokens asked for.
pub fn mono_face_name() -> String {
    match host_mono() {
        Some((name, _)) => name,
        None => "egui default".to_string(),
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((name, bytes)) = host_mono() {
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, name);
    }
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {

        style.text_styles = [
            (TextStyle::Heading, sans(20.0)),
            (TextStyle::Body, sans(FS_BODY)),
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
        v.selection.bg_fill = Color32::from_rgb(0x2a, 0x33, 0x5e);
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

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.interact_size.y = 26.0;
    });
}

// ---------------------------------------------------------------------------
// The shell: a computer in a display.
//
// Convexity frames its terminal as a rounded "device" floating on a near-black
// desk beside a fixed icon rail, which is what makes the product read as one
// instrument rather than a page. The geometry below is that shell
// (`frontend/mockups/assets/shell.css`) in Vitna's palette: the desk is a step
// BELOW the canvas rather than a warm black, and the one accent is periwinkle
// rather than amber.
// ---------------------------------------------------------------------------

/// A step below the canvas, so the device has something to sit on.
pub const DESK: Color32 = Color32::from_rgb(0x05, 0x07, 0x0d);
/// The device surface is the canvas proper.
pub const DEVICE: Color32 = CANVAS;
/// The header band inside the device, one step up from it.
pub const BAND: Color32 = Color32::from_rgb(0x0e, 0x13, 0x22);

pub const RAIL_W: f32 = 76.0;
pub const FRAME: f32 = 14.0;
pub const DEVICE_RADIUS: f32 = 18.0;
pub const HEADER_H: f32 = 46.0;

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

/// A section header: a short title, then a hairline running to the edge, then an
/// optional mono meta label. The rule is what makes a dense surface read as
/// sections rather than as a list of paragraphs.
pub fn section_header(ui: &mut egui::Ui, title: &str, meta: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .font(sans(FS_SMALL))
                .color(INK_2)
                .strong(),
        );
        let meta_w = meta.map(|m| m.len() as f32 * 6.5 + 10.0).unwrap_or(0.0);
        let rule_w = (ui.available_width() - meta_w - 10.0).max(0.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(rule_w, 1.0), egui::Sense::hover());
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            egui::Stroke::new(1.0, HAIR_2),
        );
        if let Some(m) = meta {
            ui.label(egui::RichText::new(m).font(mono(FS_MARK)).color(FAINTER));
        }
    });
    ui.add_space(8.0);
}

/// A value and the quiet label above it. The scale contrast between the two is
/// what makes density read as deliberate.
pub fn stat(ui: &mut egui::Ui, label: &str, value: &str, tone: Color32, size: f32) {
    ui.vertical(|ui| {
        ui.label(eyebrow(ui, label));
        ui.add_space(1.0);
        ui.label(egui::RichText::new(value).font(mono(size)).color(tone));
    });
}

/// A dash, for a number this window does not know. Never a zero.
pub const UNKNOWN: &str = "--";
