//! A turn: one column with a glyph gutter down its left, every entry hanging
//! off it, the way the web client set the transcript and the way a terminal
//! reads. The person's words sit on a ground; the model's are plain prose; a
//! tool call is one line, its output tucked under it behind a hook, and an
//! approval it waits on sits between the two, in the flow.

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::{blank, grounded, paint, spread};
use crate::app::App;
use crate::text::{group_digest, more, truncate, wrap};
use crate::theme::{self, glyph};
use crate::turn::{Approval, DiffLine, Entry, FileDiff, Phase, Tool, CHOICES};
use crate::words;

/// Output lines shown under a tool before the rest collapse into a count.
const OUTPUT_LINES: usize = 3;
/// Diff lines shown in an approval before the rest collapse into a count.
const DIFF_LINES: usize = 12;

pub fn draw(app: &App, frame: &mut Frame, area: Rect) {
    let lines = lines(&app.turn, area.width as usize, Instant::now());
    // A running turn follows its tail, the way a terminal does, unless the
    // person has scrolled back up through it.
    let tail = lines.len().saturating_sub(area.height as usize);
    app.scroll_room.set(tail);
    paint(frame, area, &lines, tail.saturating_sub(app.scroll));
}

pub fn lines(entries: &[Entry], w: usize, now: Instant) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        // A run of tool calls reads as one block, the way a terminal prints
        // them; a row of room separates everything else.
        let tools_in_a_row =
            i > 0 && matches!(entry, Entry::Tool(_)) && matches!(entries[i - 1], Entry::Tool(_));
        if i > 0 && !tools_in_a_row {
            out.push(blank());
        }
        match entry {
            Entry::You(text) => you(&mut out, text, w),
            Entry::Model(text) => {
                for part in wrap(text, w.saturating_sub(2)) {
                    out.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(part, theme::ink()),
                    ]));
                }
            }
            Entry::Tool(tool) => tool_row(&mut out, tool, w, now),
            Entry::Note { tone, text } => {
                for (j, part) in wrap(text, w.saturating_sub(2)).into_iter().enumerate() {
                    let lead = if j == 0 {
                        Span::styled(format!("{} ", glyph::DOT), theme::tone(*tone))
                    } else {
                        Span::raw("  ")
                    };
                    out.push(Line::from(vec![lead, Span::styled(part, theme::faint())]));
                }
            }
        }
    }
    out
}

/// What the person sent, on a ground with a row of room above and below, so
/// it reads as the turn's heading rather than as one more line of output.
fn you(out: &mut Vec<Line<'static>>, text: &str, w: usize) {
    let inner = w.saturating_sub(5);
    out.push(grounded(Vec::new(), w, theme::GROUND));
    for (i, part) in wrap(text, inner).into_iter().enumerate() {
        let lead = if i == 0 {
            Span::styled(
                format!(" {} ", glyph::YOU),
                theme::tone(theme::PERI).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("   ")
        };
        out.push(grounded(
            vec![lead, Span::styled(part, theme::ink())],
            w,
            theme::GROUND,
        ));
    }
    out.push(grounded(Vec::new(), w, theme::GROUND));
}

fn phase_mark(phase: Phase) -> (&'static str, Style) {
    match phase {
        Phase::Proposed | Phase::Skipped => (glyph::PENDING, theme::fainter()),
        Phase::Waiting => (glyph::PENDING, theme::tone(theme::RUST)),
        Phase::Running => (glyph::RUNNING, theme::tone(theme::PERI)),
        Phase::Done => (glyph::DONE, theme::faint()),
        Phase::Failed => (glyph::DONE, theme::tone(theme::RUST)),
    }
}

fn phase_status(tool: &Tool) -> (String, Style) {
    match tool.phase {
        Phase::Proposed => ("proposed".into(), theme::fainter()),
        Phase::Waiting => ("waiting on you".into(), theme::tone(theme::RUST)),
        Phase::Running => ("running".into(), theme::faint()),
        Phase::Skipped => ("did not run".into(), theme::fainter()),
        Phase::Done => (
            tool.status.clone().unwrap_or_else(|| "done".into()),
            theme::faint(),
        ),
        Phase::Failed => (
            tool.status.clone().unwrap_or_else(|| "failed".into()),
            theme::tone(theme::RUST),
        ),
    }
}

/// One tool call, one line: mark, verb, target, what it does beyond reading,
/// and its status at the right.
fn tool_row(out: &mut Vec<Line<'static>>, tool: &Tool, w: usize, now: Instant) {
    let (mark, mark_style) = phase_mark(tool.phase);
    let (status, status_style) = phase_status(tool);
    let mut left = vec![
        Span::styled(format!("{mark} "), mark_style),
        Span::styled(words::tool_title(&tool.name), theme::title()),
        Span::raw(" "),
        match &tool.target {
            Some(t) => Span::styled(t.clone(), theme::ink()),
            None => Span::styled(tool.name.clone(), theme::fainter()),
        },
    ];
    if !tool.effect.is_empty() && tool.effect != "read" {
        left.push(Span::styled(
            format!("  {}", words::effect(&tool.effect)),
            theme::fainter(),
        ));
    }
    out.push(Line::from(spread(
        left,
        vec![Span::styled(status, status_style)],
        w,
    )));

    if let Some(approval) = &tool.approval {
        approval_box(out, approval, tool.diff.as_ref(), w, now);
    }

    let kept: Vec<&String> = tool
        .output
        .iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if !kept.is_empty() {
        for (i, line) in kept.iter().take(OUTPUT_LINES).enumerate() {
            let lead = if i == 0 {
                format!("  {} ", glyph::UNDER)
            } else {
                "    ".to_string()
            };
            out.push(Line::from(vec![
                Span::styled(lead, theme::fainter()),
                Span::styled(truncate(line, w.saturating_sub(4)), theme::faint()),
            ]));
        }
        if kept.len() > OUTPUT_LINES {
            out.push(Line::from(Span::styled(
                format!("    {}", more(kept.len() - OUTPUT_LINES, "line", "lines")),
                theme::fainter(),
            )));
        }
    }
}

/// An exact action waiting for a person, as a box in the flow with numbered
/// choices, the way a terminal asks. Raised on its own ground, the one thing
/// in a turn that stands off the page, since it is the one thing waiting on
/// somebody. The digest is printed in full because the digest is what gets
/// approved; the description says what it covers, and the diff shows the
/// change when the action is a write.
fn approval_box(
    out: &mut Vec<Line<'static>>,
    a: &Approval,
    diff: Option<&FileDiff>,
    w: usize,
    now: Instant,
) {
    let indent = 2;
    let bw = w.saturating_sub(indent);
    let pad = 2;
    let inner = bw.saturating_sub(pad * 2);
    let g = theme::GROUND;
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let push = |rows: &mut Vec<Vec<Span<'static>>>, spans: Vec<Span<'static>>| rows.push(spans);

    push(&mut rows, Vec::new());
    push(
        &mut rows,
        vec![
            Span::styled(format!("{} ", glyph::DONE), theme::tone(theme::RUST)),
            Span::styled(
                "Approval needed",
                theme::tone(theme::RUST).add_modifier(Modifier::BOLD),
            ),
        ],
    );
    for part in wrap(
        if a.description.is_empty() {
            "The daemon did not describe this action."
        } else {
            &a.description
        },
        inner,
    ) {
        push(&mut rows, vec![Span::styled(part, theme::ink())]);
    }
    push(&mut rows, Vec::new());

    let label_w = 9;
    let fact = |label: &str, value: String, empty: &str| -> Vec<Vec<Span<'static>>> {
        let (text, style) = if value.is_empty() {
            (empty.to_string(), theme::fainter())
        } else {
            (value, theme::ink())
        };
        wrap(&text, inner.saturating_sub(label_w))
            .into_iter()
            .enumerate()
            .map(|(i, part)| {
                let lead = if i == 0 {
                    format!("{label:<label_w$}")
                } else {
                    " ".repeat(label_w)
                };
                vec![
                    Span::styled(lead, theme::fainter()),
                    Span::styled(part, style),
                ]
            })
            .collect()
    };
    let digest = if a.digest.is_empty() {
        String::new()
    } else {
        group_digest(&a.digest)
    };
    rows.extend(fact(
        "digest",
        digest,
        "none sent. Approving would bind to nothing.",
    ));
    rows.extend(fact("runs as", a.runs_as.clone(), "not stated"));
    rows.extend(fact("cwd", a.cwd.clone(), "not stated"));
    rows.extend(fact("mounts", a.mounts.join("  "), "none"));
    rows.extend(fact("env", a.env.join(", "), "none passed"));

    if let Some(d) = diff {
        push(&mut rows, Vec::new());
        push(
            &mut rows,
            vec![
                Span::styled(truncate(&d.path, inner.saturating_sub(24)), theme::ink()),
                Span::styled(format!("  {}  ", d.status), theme::fainter()),
                Span::styled(format!("+{}", d.additions), theme::tone(theme::OK)),
                Span::styled(
                    format!(" {}{}", glyph::MINUS, d.deletions),
                    theme::tone(theme::DEL),
                ),
            ],
        );
        for line in d.lines.iter().take(DIFF_LINES) {
            let (sign, sign_style, text_style, band, text) = diff_parts(line);
            let body = truncate(text, inner.saturating_sub(2));
            let used = 2 + crate::text::width(&body);
            let mut spans = vec![
                Span::styled(format!("{sign:<2}"), sign_style),
                Span::styled(body, text_style),
            ];
            if let Some(bg) = band {
                for s in spans.iter_mut() {
                    s.style = s.style.bg(bg);
                }
                spans.push(Span::styled(
                    " ".repeat(inner.saturating_sub(used)),
                    Style::new().bg(bg),
                ));
            }
            push(&mut rows, spans);
        }
        if d.lines.len() > DIFF_LINES {
            push(
                &mut rows,
                vec![Span::styled(
                    more(d.lines.len() - DIFF_LINES, "line", "lines"),
                    theme::fainter(),
                )],
            );
        }
    }

    push(&mut rows, Vec::new());
    let armed = a.armed(now);
    for (i, choice) in CHOICES.iter().enumerate() {
        let here = i == a.cursor;
        let label_style = if !armed {
            theme::fainter()
        } else if here {
            theme::title()
        } else {
            theme::ink()
        };
        let mut spans = vec![
            Span::styled(
                if here {
                    format!("{} ", glyph::CURSOR)
                } else {
                    "  ".into()
                },
                theme::tone(theme::PERI),
            ),
            Span::styled(format!("{}. ", choice.key), theme::fainter()),
            Span::styled(choice.label, label_style),
        ];
        if let Some(h) = choice.hint {
            spans.push(Span::styled(format!("  {h}"), theme::fainter()));
        }
        push(&mut rows, spans);
    }
    if let Some(meaning) = CHOICES[a.cursor].meaning {
        for part in wrap(meaning, inner) {
            push(
                &mut rows,
                vec![Span::styled(part, theme::tone(theme::RUST))],
            );
        }
    }
    push(
        &mut rows,
        vec![Span::styled(
            if armed {
                "\u{2191}\u{2193} move \u{b7} enter choose \u{b7} y approve once \u{b7} n reject \u{b7} 1-5 pick"
            } else {
                "Answers open in a moment, so a key typed for the composer cannot answer this."
            },
            theme::fainter(),
        )],
    );
    push(&mut rows, Vec::new());

    for spans in rows {
        let mut line = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(" ".repeat(pad), Style::new().bg(g)),
        ];
        line.extend(spans);
        let mut l = grounded(line, w, g);
        // The indent is the page, not the box.
        if let Some(first) = l.spans.first_mut() {
            first.style = Style::new();
        }
        out.push(l);
    }
}

/// A diff line's sign, the sign's style, the text's style, its band, and its
/// text. Removed lines keep the red sign and a fainter text, so the eye lands
/// on what the change adds.
fn diff_parts(line: &DiffLine) -> (&'static str, Style, Style, Option<Color>, &str) {
    match line {
        DiffLine::Hunk(t) => ("", theme::fainter(), theme::fainter(), None, t),
        DiffLine::Add(t) => (
            "+",
            theme::tone(theme::OK),
            theme::ink(),
            Some(theme::ADD_BAND),
            t,
        ),
        DiffLine::Del(t) => (
            glyph::MINUS,
            theme::tone(theme::DEL),
            theme::faint(),
            Some(theme::DEL_BAND),
            t,
        ),
        DiffLine::Same(t) => ("", theme::fainter(), theme::faint(), None, t),
    }
}
