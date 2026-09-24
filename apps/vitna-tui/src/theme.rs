//! The terminal's material: the desktop window's palette, set in cells.
//!
//! Every hex below is the desktop window's (`apps/vitna-gui/src/theme.rs`,
//! PR #5), re-stepped there rung by rung on the owner's word, so the two
//! clients read as one product. Copy a rung from there, never from
//! `apps/vitna-desktop`'s stylesheet, which the window left behind.
//!
//!   Floor    the terminal's own background. Nothing here paints a floor of
//!            its own, for the reason the window recoloured its title bar:
//!            a second black inside the terminal's padding is a seam, and a
//!            seam makes an app read as a picture of one set inside a frame.
//!   Inks     three, and warm: a white, and two greys that are that white
//!            dimmed. A chosen row says so with its ground, not a fourth ink.
//!   Grounds  only for what is raised: the composer's well, the approval, a
//!            chosen row, the person's own words.
//!   Accents  periwinkle for what the terminal asks of a person and where they
//!            stand, and for the name on the start screen, where the window
//!            draws its periwinkle mark; rust for a state somebody has to act
//!            on; green as one dot.
//!   Rule     colour carries state and never decoration, and the word is
//!            always printed beside it, which is why the screen still reads
//!            with NO_COLOR set and every colour gone.
//!
//! No glass, no glow, no gradient, and no shade characters faking any of them.
//! No box around every region either: the old client drew five bordered panes,
//! the terminal version of the carded rows the window dropped.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};

// Three inks, and no more: what you read, what explains it, and what you can
// ignore. INK is the owner's #efeeeb; the greys keep the lightness they had as
// cool greys and take INK's exact tint, so a meta line never reads bluer than
// the title above it. A test below holds them to one tint.
pub const INK: Color = Color::Rgb(0xef, 0xee, 0xeb);
pub const FAINT: Color = Color::Rgb(0xaa, 0xa9, 0xa6);
pub const FAINTER: Color = Color::Rgb(0x91, 0x90, 0x8d);

pub const PERI: Color = Color::Rgb(0x71, 0x88, 0xff);
pub const PERI_2: Color = Color::Rgb(0x8e, 0xa2, 0xff);
pub const RUST: Color = Color::Rgb(0xd9, 0xa4, 0x41);
pub const OK: Color = Color::Rgb(0x2f, 0xbf, 0x71);
/// A diff's minus sign, and nothing else. Lines removed read red in every tool
/// this sits beside, and a second meaning for it would be a lie.
pub const DEL: Color = Color::Rgb(0xe5, 0x53, 0x4b);

// Grounds, cool where the inks are warm, each a step over the one it sits on.
// GROUND carries the approval and the person's own words; FIELD is the
// composer, the lit well; FACE is a chosen row, the brightest fill, so the
// place you stand reads as a thing and not as a smudge.
pub const GROUND: Color = Color::Rgb(0x17, 0x18, 0x1e);
pub const FIELD: Color = Color::Rgb(0x1f, 0x20, 0x27);
pub const FACE: Color = Color::Rgb(0x23, 0x25, 0x2d);

/// The quiet rule inside a surface: under a section's heading.
pub const HAIR_2: Color = Color::Rgb(0x30, 0x32, 0x3c);

/// A diff line's band: the sign's colour laid thinly over the dark, so an
/// added or removed line reads at a glance without its text changing colour.
pub const ADD_BAND: Color = Color::Rgb(0x11, 0x26, 0x1e);
pub const DEL_BAND: Color = Color::Rgb(0x2a, 0x17, 0x18);

/// Glyphs. Each state is one of these AND its word; none stands alone.
pub mod glyph {
    /// The person's line.
    pub const YOU: &str = "\u{203a}";
    /// Output tucked under the tool that produced it.
    pub const UNDER: &str = "\u{23bf}";
    pub const DONE: &str = "\u{25cf}";
    pub const RUNNING: &str = "\u{25d0}";
    pub const PENDING: &str = "\u{25cb}";
    pub const CHECK: &str = "\u{2713}";
    pub const CROSS: &str = "\u{2717}";
    pub const ALERT: &str = "!";
    /// Where a person stands in a list of choices.
    pub const CURSOR: &str = "\u{276f}";
    pub const RETURN: &str = "\u{21b5}";
    pub const BACK: &str = "\u{2039}";
    /// A removal count's sign, as the window prints it: U+2212, not a hyphen.
    pub const MINUS: &str = "\u{2212}";
    pub const DOT: &str = "\u{00b7}";
    /// The composer's well is drawn from half blocks, so it stands half a row
    /// off the text above and below it rather than a whole one.
    pub const WELL_TOP: &str = "\u{2584}";
    pub const WELL_FOOT: &str = "\u{2580}";
}

pub fn ink() -> Style {
    Style::new().fg(INK)
}

pub fn faint() -> Style {
    Style::new().fg(FAINT)
}

pub fn fainter() -> Style {
    Style::new().fg(FAINTER)
}

/// The display role: a title, the hero, a state named in full. Terminals have
/// one size, so the window's heavier display cut becomes bold here.
pub fn title() -> Style {
    Style::new().fg(INK).add_modifier(Modifier::BOLD)
}

pub fn tone(color: Color) -> Style {
    Style::new().fg(color)
}

/// How much colour the terminal can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 24-bit, so every rung above lands exactly.
    True,
    /// The xterm 256-colour palette; each rung takes its nearest entry.
    Indexed,
    /// NO_COLOR is set: no colour at all, and the words carry every state.
    None,
}

impl Depth {
    /// Reads what the terminal says about itself. `var` is the environment,
    /// passed in so a test can state one; none of these names is a secret.
    ///
    /// Windows is taken as 24-bit because the console host has drawn it since
    /// Windows 10 1703 and Windows Terminal always has. Apple's Terminal is the
    /// one common terminal that misreads a 24-bit sequence rather than rounding
    /// it, so it and anything unrecognised get the 256-colour palette, which
    /// every terminal this ships to reads.
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Depth {
        if var("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Depth::None;
        }
        if matches!(var("COLORTERM").as_deref(), Some("truecolor" | "24bit")) {
            return Depth::True;
        }
        if cfg!(windows) {
            return Depth::True;
        }
        match var("TERM_PROGRAM").as_deref() {
            Some("iTerm.app" | "WezTerm" | "vscode" | "ghostty") => Depth::True,
            _ => Depth::Indexed,
        }
    }

    /// Rewrites a drawn frame for this depth, so the views draw in 24-bit and
    /// never branch on it. The composer's well is the one exception, and it is
    /// handled here: its caps are half blocks drawn in the field's colour, so
    /// with no colour they would print as bright bars, and they become rules.
    pub fn apply(self, buf: &mut Buffer) {
        if self == Depth::True {
            return;
        }
        for cell in buf.content.iter_mut() {
            if self == Depth::None
                && (cell.symbol() == glyph::WELL_TOP || cell.symbol() == glyph::WELL_FOOT)
            {
                cell.set_symbol("\u{2500}");
            }
            cell.fg = self.map(cell.fg);
            cell.bg = self.map(cell.bg);
        }
    }

    fn map(self, color: Color) -> Color {
        match (self, color) {
            (Depth::None, _) => Color::Reset,
            (Depth::Indexed, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
            (_, other) => other,
        }
    }
}

/// The xterm 256-colour entry nearest an RGB colour: the better of the nearest
/// point on the 6x6x6 cube and the nearest step on the 24-step grey ramp.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).abs())
            .map(|(i, _)| i)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let mean = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let step = if mean < 8 {
        0
    } else {
        ((mean - 8) / 10).min(23)
    };
    let grey_value = (8 + step * 10) as u8;
    let grey_index = 232 + step as usize;

    let distance = |(cr, cg, cb): (u8, u8, u8)| -> i32 {
        let d = |a: u8, b: u8| i32::from(a) - i32::from(b);
        d(cr, r).pow(2) + d(cg, g).pow(2) + d(cb, b).pow(2)
    };
    if distance((grey_value, grey_value, grey_value)) < distance(cube) {
        grey_index as u8
    } else {
        cube_index as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("{other:?} is not a 24-bit rung"),
        }
    }

    /// The greys are the white dimmed, so they carry its tint exactly. A white
    /// changed on its own leaves every meta line a different colour from the
    /// title above it; a hex named for one rung re-steps the other two. The
    /// same test holds the window's three inks.
    #[test]
    fn the_three_inks_are_one_white_at_three_lightnesses() {
        let tint = |c: Color| {
            let (r, g, b) = rgb(c);
            (i16::from(r) - i16::from(g), i16::from(b) - i16::from(g))
        };
        assert_eq!(tint(FAINT), tint(INK), "FAINT does not carry INK's tint");
        assert_eq!(
            tint(FAINTER),
            tint(INK),
            "FAINTER does not carry INK's tint"
        );
        let (_, ink_g, _) = rgb(INK);
        let (_, faint_g, _) = rgb(FAINT);
        let (_, fainter_g, _) = rgb(FAINTER);
        assert!(
            ink_g > faint_g && faint_g > fainter_g,
            "the inks no longer step down"
        );
    }

    /// Every ground keeps its blue a few points over its red, so periwinkle is
    /// the only blue thing on screen, and each steps up from the one below.
    #[test]
    fn the_grounds_step_up_and_stay_neutral() {
        let mut last = 0u8;
        for g in [GROUND, FIELD, FACE] {
            let (r, gg, b) = rgb(g);
            assert!(
                b >= r && b - r <= 10,
                "{g:?} is tinted, not neutral with a whisper of cool"
            );
            assert!(
                gg > last,
                "{g:?} does not sit a step over the ground below it"
            );
            last = gg;
        }
    }

    #[test]
    fn no_color_takes_every_colour_away_and_leaves_the_rest() {
        let depth = Depth::detect(|k| (k == "NO_COLOR").then(|| "1".to_string()));
        assert_eq!(depth, Depth::None);
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 2, 1));
        buf[(0, 0)].set_style(Style::new().fg(RUST).bg(FIELD).add_modifier(Modifier::BOLD));
        depth.apply(&mut buf);
        assert_eq!(buf[(0, 0)].fg, Color::Reset);
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
        assert!(
            buf[(0, 0)].modifier.contains(Modifier::BOLD),
            "emphasis is not colour"
        );
    }

    #[test]
    fn an_empty_no_color_is_not_a_request() {
        // The convention: NO_COLOR set to the empty string means nothing.
        let depth = Depth::detect(|k| match k {
            "NO_COLOR" => Some(String::new()),
            "COLORTERM" => Some("truecolor".to_string()),
            _ => None,
        });
        assert_eq!(depth, Depth::True);
    }

    #[test]
    fn apple_terminal_gets_the_palette_it_can_read() {
        if cfg!(windows) {
            return;
        }
        let depth = Depth::detect(|k| (k == "TERM_PROGRAM").then(|| "Apple_Terminal".to_string()));
        assert_eq!(depth, Depth::Indexed);
    }

    #[test]
    fn the_palette_rounds_to_its_nearest_entries() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        // INK is a warm near-white: it should land on the grey ramp's top, or
        // the cube's white, never on a tinted entry.
        let ink = nearest_256(0xef, 0xee, 0xeb);
        assert!(ink == 231 || ink >= 250, "INK rounded to entry {ink}");
        // The grounds are near-black and neutral, so they fall on the ramp.
        let field = nearest_256(0x1f, 0x20, 0x27);
        assert!(
            (232..=236).contains(&field),
            "FIELD rounded to entry {field}"
        );
    }
}
