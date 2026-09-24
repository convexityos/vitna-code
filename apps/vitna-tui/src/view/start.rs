//! The start state: nothing has run in this folder yet.
//!
//! The window's one line for an empty start, set in the upper third the way
//! Codex places "Let's build": enough room above it to feel like a place and
//! not a form. It shows only when there is nothing to list. With runs, the
//! runs are the page, and a greeting over work already done is decoration.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::{blank, paint};
use crate::app::App;
use crate::link::Link;
use crate::text::wrap;
use crate::theme::{self, glyph};

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let w = area.width as usize;
    // Where the window draws the mark over this line, the terminal sets the
    // name, in the mark's own periwinkle: the traced mark cannot be redrawn in
    // cells without being rebuilt, which its source file forbids.
    let mut lines = vec![
        centred(
            vec![Span::styled("Vitna Code", theme::tone(theme::PERI))],
            w,
        ),
        centred(vec![Span::styled("Let\u{2019}s build", theme::title())], w),
    ];

    // Beneath it, the one thing that stands between this screen and a turn,
    // in the daemon's terms rather than a generic "offline".
    let (dot, first, then): (Option<Style>, String, Option<String>) = match &app.link {
        Link::Probing => (None, String::new(), None),
        Link::Absent { .. } => (
            Some(theme::tone(theme::RUST)),
            "The daemon is not running, so there is nothing to send to yet.".into(),
            Some("Start vitna-coded, then press ctrl+r to look again.".into()),
        ),
        Link::Trouble { what } => (
            Some(theme::tone(theme::RUST)),
            "The daemon did not complete the handshake.".into(),
            Some(format!(
                "{}{}",
                upper_first(what),
                if what.ends_with('.') { "" } else { "." }
            )),
        ),
        Link::Open { .. } => (
            None,
            "vitna-coded answered. It takes no turns over the wire yet.".into(),
            None,
        ),
    };
    if !first.is_empty() {
        lines.push(blank());
        let lead = if dot.is_some() { 2 } else { 0 };
        for (i, part) in wrap(&first, w.saturating_sub(lead)).into_iter().enumerate() {
            let mut spans = Vec::new();
            if let Some(style) = dot {
                spans.push(Span::styled(
                    if i == 0 {
                        format!("{} ", glyph::DONE)
                    } else {
                        "  ".into()
                    },
                    style,
                ));
            }
            spans.push(Span::styled(part, theme::faint()));
            lines.push(centred(spans, w));
        }
        if let Some(then) = then {
            for part in wrap(&then, w) {
                lines.push(centred(vec![Span::styled(part, theme::fainter())], w));
            }
        }
    }

    let above = (f32::from(area.height) * 0.28) as usize;
    let top = above.min((area.height as usize).saturating_sub(lines.len()));
    let body = Rect {
        y: area.y + top as u16,
        height: area.height - top as u16,
        ..area
    };
    paint(frame, body, &lines, 0);
}

fn centred(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let mut out = vec![Span::raw(" ".repeat(width.saturating_sub(used) / 2))];
    out.extend(spans);
    Line::from(out)
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}
