//! The composer: the thing you reach for.
//!
//! Its base is the window's, which is Claude Code's: a line above the field
//! stating where a turn would run, the field, and a base row beneath it in the
//! open rather than inside the box. The field is a lit well, drawn from half
//! blocks so it stands half a row off the lines around it, and it is the one
//! lit surface on screen, because the thing you type into is the thing you
//! should see. With no colour to draw it in, it falls back to two rules.

use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::spread;
use crate::app::{App, Screen};
use crate::link::Link;
use crate::place::Branch;
use crate::text::{cursor_in, rows, truncate, Row};
use crate::theme::{self, glyph};

/// The field grows with what is typed, up to this many rows, then scrolls.
const MAX_ROWS: usize = 6;
/// Cells inside the well: two of room on the left, and on the right a space,
/// the return cap and two of room, reserved whether or not the cap shows so
/// the first keystroke does not move the text.
const PAD_LEFT: usize = 2;
const PAD_RIGHT: usize = 4;

const KEYS: [(&str, &str); 8] = [
    ("enter", "send, or open the chosen run"),
    ("alt+enter", "a new line"),
    ("\u{2191} \u{2193}", "choose a run, or scroll one"),
    ("esc", "back to the runs"),
    ("ctrl+r", "look for the daemon again"),
    ("ctrl+u", "clear to the start of the line"),
    ("ctrl+c", "clear what is typed, or quit"),
    ("?", "close this"),
];

fn text_width(width: u16) -> usize {
    (width as usize).saturating_sub(PAD_LEFT + PAD_RIGHT).max(1)
}

fn layout(app: &App, width: u16) -> Vec<Row> {
    rows(&app.draft, text_width(width))
}

fn help_rows(app: &App, width: u16) -> usize {
    if !app.help {
        return 0;
    }
    if width >= 80 {
        KEYS.len() / 2
    } else {
        KEYS.len()
    }
}

pub fn height(app: &App, width: u16) -> u16 {
    let field = layout(app, width).len().clamp(1, MAX_ROWS);
    // The bar, the well's two caps and its rows, and the base row (or the
    // keys, which take its place).
    let base = help_rows(app, width).max(1);
    (1 + 2 + field + base) as u16
}

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let w = area.width as usize;
    let mut y = area.y;
    let buf = frame.buffer_mut();

    // Where the turn would run, as facts git can give without a network.
    let mut place = vec![Span::styled(app.folder.clone(), theme::ink())];
    match &app.branch {
        Branch::Named(b) => {
            place.push(Span::styled(" on ", theme::fainter()));
            place.push(Span::styled(b.clone(), theme::faint()));
        }
        Branch::Detached => place.push(Span::styled(" on a detached HEAD", theme::faint())),
        Branch::Reading | Branch::Unknown => {}
    }
    buf.set_line(
        area.x,
        y,
        &Line::from(spread(place, Vec::new(), w)),
        area.width,
    );
    y += 1;

    let rows = layout(app, area.width);
    let (cursor_row, cursor_col) = cursor_in(&app.draft, &rows, app.cursor);
    let shown = rows.len().clamp(1, MAX_ROWS);
    let first = (cursor_row + 1).saturating_sub(MAX_ROWS);

    let well = Style::new().fg(theme::FIELD);
    buf.set_string(area.x, y, glyph::WELL_TOP.repeat(w), well);
    y += 1;
    let tw = text_width(area.width);
    for i in 0..shown {
        let mut spans = vec![Span::raw(" ".repeat(PAD_LEFT))];
        if app.draft.is_empty() {
            if i == 0 {
                spans.push(Span::styled(truncate("Ask anything", tw), theme::fainter()));
            }
        } else if let Some(r) = rows.get(first + i) {
            let text: String = app.draft[r.start..r.end]
                .iter()
                .filter(|c| **c != '\n')
                .collect();
            spans.push(Span::styled(text, theme::ink()));
        }
        buf.set_line(
            area.x,
            y,
            &super::grounded(spans, w, theme::FIELD),
            area.width,
        );
        if app.draft.is_empty() && i == 0 {
            // The return cap, at the right of the first row, while there is
            // nothing typed: the resting state says which key acts.
            let x = area.x + (w.saturating_sub(PAD_RIGHT) + 1) as u16;
            buf.set_string(
                x,
                y,
                glyph::RETURN,
                Style::new().fg(theme::FAINTER).bg(theme::FIELD),
            );
        }
        y += 1;
    }
    buf.set_string(area.x, y, glyph::WELL_FOOT.repeat(w), well);
    y += 1;

    if app.help {
        keys(frame, area.x, y, area.width);
    } else {
        let line = Line::from(spread(base_left(app, w), base_right(&app.link), w));
        frame.buffer_mut().set_line(area.x, y, &line, area.width);
    }

    // The field keeps the cursor even while the keys are open: the next key
    // closes them and lands in the field.
    let cx = area.x + (PAD_LEFT + cursor_col) as u16;
    let cy = area.y + 2 + (cursor_row - first) as u16;
    frame.set_cursor_position(Position::new(cx.min(area.right().saturating_sub(1)), cy));
}

/// Beneath the field, on the left: why the last Enter did not send, until the
/// next key, and otherwise the keys that matter on this screen.
fn base_left(app: &App, w: usize) -> Vec<Span<'static>> {
    // No dot of its own: the reason is already the state on the right, and a
    // second amber dot on one line says the same thing twice.
    if let Some(notice) = &app.notice {
        return vec![Span::styled(
            truncate(notice, w.saturating_sub(24)),
            theme::faint(),
        )];
    }
    let hint = if !app.draft.is_empty() {
        "alt+enter for a new line"
    } else {
        match app.screen {
            Screen::Run(_) => "esc back \u{b7} \u{2191}\u{2193} scroll \u{b7} ? keys",
            Screen::Home if app.ledger.as_ref().is_some_and(|l| !l.entries.is_empty()) => {
                "\u{2191}\u{2193} choose \u{b7} enter open \u{b7} ? keys"
            }
            Screen::Home => "? keys",
        }
    };
    vec![Span::styled(hint, theme::fainter())]
}

/// On the right: whether there is a daemon, as a dot and its word.
fn base_right(link: &Link) -> Vec<Span<'static>> {
    let (dot, word) = match link {
        Link::Probing => (None, "looking for the daemon"),
        Link::Absent { .. } => (Some(theme::RUST), "no daemon"),
        Link::Trouble { .. } => (Some(theme::RUST), "daemon did not answer"),
        Link::Open { .. } => (Some(theme::OK), "daemon connected"),
    };
    let mut out = Vec::new();
    if let Some(tone) = dot {
        out.push(Span::styled(format!("{} ", glyph::DONE), theme::tone(tone)));
    }
    out.push(Span::styled(
        word,
        if dot.is_some() {
            theme::faint()
        } else {
            theme::fainter()
        },
    ));
    out
}

fn keys(frame: &mut Frame, x: u16, y: u16, width: u16) {
    let columns = if width >= 80 { 2 } else { 1 };
    let per = KEYS.len() / columns;
    let col_w = width as usize / columns;
    for (i, (key, what)) in KEYS.iter().enumerate() {
        let (c, r) = (i / per, i % per);
        let line = Line::from(vec![
            Span::styled(format!("{key:<11}"), theme::faint()),
            Span::styled(truncate(what, col_w.saturating_sub(13)), theme::fainter()),
        ]);
        frame
            .buffer_mut()
            .set_line(x + (c * col_w) as u16, y + r as u16, &line, col_w as u16);
    }
}
