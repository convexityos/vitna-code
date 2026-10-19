//! HTML previews of each screen, drawn by the real views into a buffer and
//! written cell for cell, for reviewing the design without a terminal.
//!
//! `cargo test -p vitna-tui -- --ignored write_previews` writes them to the
//! folder named by VITNA_TUI_PREVIEW, or to `vitna-tui-preview` in the temp
//! folder. The page stands in for Windows Terminal's default scheme: its
//! background behind every cell the terminal would paint, its foreground for
//! any cell that sets none, and half blocks filled to the cell exactly, as the
//! terminal draws them, rather than left to a font's idea of the glyph.

use std::fmt::Write as _;
use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::app::App;
use crate::fixtures;
use crate::theme::{self, glyph};

const TERMINAL_BG: &str = "#0c0c0c";
const TERMINAL_FG: &str = "#cccccc";

fn hex(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => default.to_string(),
        other => panic!("the preview draws 24-bit only, and met {other:?}"),
    }
}

fn draw(app: &App, w: u16, h: u16) -> (Buffer, Option<Position>) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
    terminal
        .draw(|f| crate::view::render(app, f, fixtures::now()))
        .expect("draw");
    let cursor = terminal.get_cursor_position().ok();
    (terminal.backend().buffer().clone(), cursor)
}

fn page(title: &str, buf: &Buffer, cursor: Option<Position>) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<section><h2>{title}</h2><div class=\"term\" style=\"width:{w}ch\">",
        w = buf.area.width
    );
    for y in 0..buf.area.height {
        out.push_str("<div class=\"row\">");
        let mut x = 0;
        while x < buf.area.width {
            let cell = &buf[(x, y)];
            let fg = hex(cell.fg, TERMINAL_FG);
            let bg = hex(cell.bg, TERMINAL_BG);
            let bold = cell.modifier.contains(Modifier::BOLD);
            let symbol = cell.symbol();
            let mut style = String::new();
            let text = if symbol == glyph::WELL_TOP {
                let _ = write!(
                    style,
                    "background:linear-gradient(to bottom,{bg} 50%,{fg} 50%);"
                );
                " ".to_string()
            } else if symbol == glyph::WELL_FOOT {
                let _ = write!(
                    style,
                    "background:linear-gradient(to bottom,{fg} 50%,{bg} 50%);"
                );
                " ".to_string()
            } else {
                let _ = write!(style, "color:{fg};background:{bg};");
                html_escape(if symbol.is_empty() { " " } else { symbol })
            };
            if bold {
                style.push_str("font-weight:700;");
            }
            if cursor == Some(Position::new(x, y)) {
                style.push_str("box-shadow:inset 2px 0 0 #e8e8e8;");
            }
            // Every cell exactly one cell wide, as the terminal lays it out,
            // whatever width the font that has the glyph gives it.
            let wide = crate::text::width(symbol) == 2;
            let _ = write!(
                out,
                "<span style=\"{style}width:{}ch;overflow:hidden;text-align:center\">{text}</span>",
                if wide { 2 } else { 1 }
            );
            x += if wide { 2 } else { 1 };
        }
        out.push_str("</div>");
    }
    out.push_str("</div></section>");
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[test]
#[ignore = "writes HTML previews for design review; run with --ignored"]
fn write_previews() {
    let dir = std::env::var_os("VITNA_TUI_PREVIEW")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("vitna-tui-preview"));
    std::fs::create_dir_all(&dir).expect("create preview folder");

    let mut open_start = fixtures::app();
    open_start.link = crate::link::Link::Open {
        version: "1.0".into(),
    };
    let mut typing = fixtures::with_runs();
    typing.draft = "Add a --json flag to vitna sessions".chars().collect();
    typing.cursor = typing.draft.len();
    let mut refused = typing.clone_for_preview();
    refused.notice =
        Some("Not sent. The daemon is not running, so there is nothing to send to.".into());
    let mut help = fixtures::with_runs();
    help.help = true;

    let screens: Vec<(&str, App, u16, u16)> = vec![
        ("Start, no daemon", fixtures::app(), 110, 30),
        ("Start, daemon connected", open_start, 110, 30),
        ("Recent runs", fixtures::with_runs(), 110, 30),
        ("A receipt", fixtures::receipt(), 110, 36),
        ("Typing", typing, 110, 30),
        ("Enter with nothing to send to", refused, 110, 30),
        ("Keys", help, 110, 30),
        (
            "A turn waiting on an approval (fixture)",
            fixtures::turn_waiting(),
            110,
            52,
        ),
        (
            "A turn running its checks (fixture)",
            fixtures::turn_running(),
            110,
            34,
        ),
        ("Recent runs at 80 x 24", fixtures::with_runs(), 80, 24),
        ("A receipt at 80 x 24", fixtures::receipt(), 80, 24),
    ];

    let style = format!(
        "<style>\
         body{{background:#1b1b1d;color:#cfcfcf;font:14px system-ui;margin:24px}}\
         body.one{{margin:0;background:{TERMINAL_BG}}}body.one h2{{display:none}}\
         h2{{font:500 13px system-ui;color:#9a9a9a;margin:28px 0 8px}}\
         .term{{background:{TERMINAL_BG};padding:10px 12px;border-radius:8px;display:inline-block;\
         font:15px/1.2 'Cascadia Mono','Cascadia Code',Consolas,monospace;box-shadow:0 8px 24px #0008}}\
         body.one .term{{border-radius:0;box-shadow:none}}\
         .row{{white-space:pre;height:1.2em}}.row span{{display:inline-block;height:1.2em}}\
         </style>"
    );
    let mut body = String::new();
    for (i, (title, app, w, h)) in screens.iter().enumerate() {
        let (buf, cursor) = draw(app, *w, *h);
        let section = page(title, &buf, cursor);
        // One page per screen as well, for a capture at the terminal's size.
        let one = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>{style}<body class=\"one\">{section}</body>"
        );
        std::fs::write(dir.join(format!("{:02}-{w}x{h}.html", i + 1)), one).expect("write screen");
        body.push_str(&section);
    }
    let html = format!("<!doctype html><meta charset=\"utf-8\"><title>Vitna Code terminal</title>{style}<body>{body}</body>");
    let path = dir.join("index.html");
    std::fs::write(&path, html).expect("write preview");
    let _ = theme::INK;
    println!("wrote {}", path.display());
}

impl App {
    fn clone_for_preview(&self) -> App {
        let mut a = fixtures::with_runs();
        a.draft = self.draft.clone();
        a.cursor = self.cursor;
        a
    }
}
