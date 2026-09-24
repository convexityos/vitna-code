//! Drawing. One centred column, the way the window and the web client both
//! set their main area: a stage that fills it, and the composer at its foot.

mod composer;
mod receipt;
mod runs;
mod start;
mod turn;

use std::time::SystemTime;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::app::{App, Screen};
use crate::text::truncate;
use crate::theme;

/// The column's widest, in cells: the window's 800px column at a terminal
/// font's width. Wider than this, a line is harder to read, not easier.
const COLUMN_MAX: u16 = 96;

/// Smaller than this, the composer and a line of the stage no longer fit
/// together, so the terminal says so instead of drawing a broken frame.
const MIN_W: u16 = 40;
const MIN_H: u16 = 12;

pub fn column(area: Rect) -> Rect {
    let w = area.width.saturating_sub(4).min(COLUMN_MAX);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y,
        width: w,
        height: area.height,
    }
}

pub fn render(app: &App, frame: &mut Frame, now: SystemTime) {
    let area = frame.area();
    if area.width < MIN_W || area.height < MIN_H {
        let msg = format!("Vitna Code needs {MIN_W} \u{d7} {MIN_H} cells");
        let line = Line::from(Span::styled(
            truncate(&msg, area.width as usize),
            theme::faint(),
        ));
        let y = area.y + area.height / 2;
        frame.buffer_mut().set_line(area.x, y, &line, area.width);
        return;
    }

    let col = column(area);
    let composer_h = composer::height(app, col.width);
    // One row of room above the stage and under the composer, so neither sits
    // on the terminal's edge.
    let top = area.y + 1;
    let foot = area.bottom().saturating_sub(1);
    let composer_top = foot.saturating_sub(composer_h);
    let stage = Rect {
        x: col.x,
        y: top,
        width: col.width,
        height: composer_top.saturating_sub(top + 1),
    };
    let composer_area = Rect {
        x: col.x,
        y: composer_top,
        width: col.width,
        height: composer_h,
    };

    if !app.turn.is_empty() {
        turn::draw(app, frame, stage);
    } else {
        match app.screen {
            Screen::Run(i) => receipt::draw(app, i, frame, stage, now),
            Screen::Home => match &app.ledger {
                // Still reading: draw nothing rather than a start screen that
                // turns into a list a frame later.
                None => {}
                Some(l) if l.entries.is_empty() && l.trouble.is_none() => {
                    start::draw(app, frame, stage)
                }
                Some(_) => runs::draw(app, frame, stage, now),
            },
        }
    }
    composer::draw(app, frame, composer_area);
}

/// Paints lines into `area` from row `scroll`, clamped so the last line never
/// leaves the bottom early.
fn paint(frame: &mut Frame, area: Rect, lines: &[Line<'static>], scroll: usize) {
    let visible = area.height as usize;
    let scroll = scroll.min(lines.len().saturating_sub(visible));
    for (i, line) in lines.iter().skip(scroll).take(visible).enumerate() {
        frame
            .buffer_mut()
            .set_line(area.x, area.y + i as u16, line, area.width);
    }
}

/// A line with `bg` behind all of it, to `width`: how a ground is drawn. A
/// span that brings its own ground, a diff line's band, keeps it.
fn grounded(spans: Vec<Span<'static>>, width: usize, bg: Color) -> Line<'static> {
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let mut spans: Vec<Span<'static>> = spans
        .into_iter()
        .map(|mut s| {
            if s.style.bg.is_none() {
                s.style = s.style.bg(bg);
            }
            s
        })
        .collect();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), Style::new().bg(bg)));
    }
    Line::from(spans)
}

/// Left spans and right spans on one line of `width`, the left cut short
/// before it can run under the right.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let right_w: usize = right.iter().map(|s| s.width()).sum();
    let room = width.saturating_sub(right_w + if right_w > 0 { 2 } else { 0 });
    let mut out = Vec::new();
    let mut used = 0;
    for s in left {
        if used >= room {
            break;
        }
        let w = s.width();
        if used + w > room {
            let cut = truncate(&s.content, room - used);
            used += crate::text::width(&cut);
            out.push(Span::styled(cut, s.style));
            break;
        }
        used += w;
        out.push(s);
    }
    if right_w > 0 && used + right_w <= width {
        out.push(Span::raw(" ".repeat(width - used - right_w)));
        out.extend(right);
    }
    out
}

/// An eyebrow: a small label over a section, uppercase and quiet, with the
/// count beside it when there is one. The count is the count, never a guess.
fn eyebrow(label: &str, count: Option<usize>) -> Line<'static> {
    let mut spans = vec![Span::styled(label.to_uppercase(), theme::fainter())];
    if let Some(n) = count {
        spans.push(Span::styled(format!("  {n}"), theme::fainter()));
    }
    Line::from(spans)
}

/// The quiet rule under a section's heading.
fn rule(width: usize) -> Line<'static> {
    Line::from(Span::styled(
        "\u{2500}".repeat(width),
        theme::tone(theme::HAIR_2),
    ))
}

fn blank() -> Line<'static> {
    Line::default()
}

#[cfg(test)]
pub(crate) mod snapshot {
    //! Draws a state into a buffer, for the tests and the previews.
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    use crate::app::App;

    pub fn draw(app: &App, w: u16, h: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_200_000);
        terminal.draw(|f| super::render(app, f, now)).expect("draw");
        terminal.backend().buffer().clone()
    }

    /// The buffer's text, one line per row, trailing spaces trimmed.
    pub fn text(buf: &Buffer) -> String {
        let area = buf.area;
        let mut out = String::new();
        for y in 0..area.height {
            let mut line = String::new();
            for x in 0..area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::snapshot::{draw, text};
    use crate::app::App;
    use crate::fixtures;
    use crate::theme::{self, Depth};

    fn every_screen() -> Vec<(&'static str, App)> {
        vec![
            ("start", fixtures::app()),
            ("runs", fixtures::with_runs()),
            ("receipt", fixtures::receipt()),
            ("waiting", fixtures::turn_waiting()),
            ("running", fixtures::turn_running()),
        ]
    }

    #[test]
    fn the_hero_is_only_for_an_empty_start() {
        assert!(text(&draw(&fixtures::app(), 100, 30)).contains("Let\u{2019}s build"));
        assert!(!text(&draw(&fixtures::with_runs(), 100, 30)).contains("Let\u{2019}s build"));
    }

    /// Ids are the receipt's vocabulary. The list names a run by what it
    /// changed; the run's own view carries its id, once, at the foot.
    #[test]
    fn the_run_list_prints_no_run_id_and_the_receipt_view_does() {
        let list = text(&draw(&fixtures::with_runs(), 100, 30));
        assert!(
            !list.contains("run-179"),
            "a run id reached the list:\n{list}"
        );
        assert!(
            list.contains("Changed apps/vitna-cli/src/main.rs and 1 more"),
            "{list}"
        );
        let receipt = text(&draw(&fixtures::receipt(), 100, 40));
        assert!(receipt.contains("run-1790196400112"), "{receipt}");
        assert!(receipt.contains("Signature not checked"), "{receipt}");
    }

    #[test]
    fn every_screen_keeps_the_composer_at_eighty_by_twenty_four() {
        for (name, app) in every_screen() {
            let t = text(&draw(&app, 80, 24));
            assert!(
                t.contains("Ask anything"),
                "{name}: the composer is gone at 80x24:\n{t}"
            );
            assert!(
                t.contains("vitna-code on claude/vitna-terminal-redesign"),
                "{name}: the bar is gone:\n{t}"
            );
        }
    }

    #[test]
    fn scrolling_a_receipt_stops_at_its_end() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = fixtures::receipt();
        draw(&app, 80, 24);
        let room = app.scroll_room.get();
        assert!(room > 0, "the receipt fits 80x24, so this proves nothing");
        for _ in 0..100 {
            app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert_eq!(app.scroll, room, "a held arrow counted on past the end");
        let t = text(&draw(&app, 80, 24));
        assert!(
            t.contains("run-1790196400112"),
            "the foot is not reachable:\n{t}"
        );
    }

    #[test]
    fn a_terminal_too_small_is_told_so() {
        let t = text(&draw(&fixtures::with_runs(), 30, 8));
        assert!(t.contains("needs 40"), "{t}");
    }

    /// Every colour on screen is a rung of the palette, or the terminal's own.
    /// A literal colour in a view is how five near-identical greys crept into
    /// the window before it was cut to three.
    #[test]
    fn nothing_is_drawn_in_a_colour_off_the_palette() {
        let palette = [
            Color::Reset,
            theme::INK,
            theme::FAINT,
            theme::FAINTER,
            theme::PERI,
            theme::PERI_2,
            theme::RUST,
            theme::OK,
            theme::DEL,
            theme::GROUND,
            theme::FIELD,
            theme::FACE,
            theme::HAIR_2,
            theme::ADD_BAND,
            theme::DEL_BAND,
        ];
        for (name, app) in every_screen() {
            let buf = draw(&app, 100, 50);
            for cell in &buf.content {
                assert!(
                    palette.contains(&cell.fg),
                    "{name}: {:?} is not a palette ink",
                    cell.fg
                );
                assert!(
                    palette.contains(&cell.bg),
                    "{name}: {:?} is not a palette ground",
                    cell.bg
                );
            }
        }
    }

    #[test]
    fn no_colour_is_written_as_a_literal_outside_the_theme() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Built at runtime, so this file does not match its own needles.
        let needles = [
            format!("Color::{}(", "Rgb"),
            format!("Color::{}(", "Indexed"),
        ];
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                if !name.ends_with(".rs") || name == "theme.rs" || name == "preview.rs" {
                    continue;
                }
                let body = std::fs::read_to_string(&path).expect("read");
                for (i, line) in body.lines().enumerate() {
                    if needles.iter().any(|n| line.contains(n.as_str())) {
                        offenders.push(format!("{name}:{}", i + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "colours written as literals, not off the palette: {offenders:?}"
        );
    }

    #[test]
    fn the_approval_prints_the_digest_it_binds_to_in_full() {
        let t = text(&draw(&fixtures::turn_waiting(), 110, 60));
        assert!(
            t.contains(
                "9f2c 81ab 03de 5b77 41a0 c6e2 d95f 18b4 c07e 3a6d 2f91 b8c4 e5a0 7d33 61fe 2b90"
            ),
            "{t}"
        );
        assert!(t.contains("Approval needed"), "{t}");
        assert!(t.contains("waiting on you"), "{t}");
    }

    /// With every colour gone, each state still reads, because its word is
    /// printed beside its colour, and the well turns into two rules instead of
    /// two bright bars.
    #[test]
    fn with_no_colour_every_state_still_reads() {
        let mut buf = draw(&fixtures::with_runs(), 100, 30);
        Depth::None.apply(&mut buf);
        let t = text(&buf);
        for word in [
            "Completed",
            "With unknowns",
            "Failed",
            "a check failed",
            "Unreadable",
            "no daemon",
        ] {
            assert!(t.contains(word), "{word:?} is missing with no colour:\n{t}");
        }
        assert!(
            !t.contains(theme::glyph::WELL_TOP),
            "the well's cap printed as a bar:\n{t}"
        );
        assert!(buf
            .content
            .iter()
            .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    }
}
