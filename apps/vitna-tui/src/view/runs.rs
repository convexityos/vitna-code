//! Recent runs: the centre of the start screen, once anything has run here.
//!
//! The window's run list, set in cells: one row per run, a status mark on the
//! title's line, the age at the right, and one quiet line of facts beneath. No
//! box around a row and no tinted pill in it. A chosen row says so with its
//! ground and the periwinkle mark of where the person stands.
//!
//! A receipt carries no prompt, on purpose: it records what was done and what
//! can be checked, not what was said. So a row's title says what the run
//! changed, which the receipt does record, and never the run id, which waits
//! in the run's own view.

use std::time::SystemTime;

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::{blank, eyebrow, grounded, paint, spread};
use crate::app::App;
use crate::catalog;
use crate::ledger::{Entry, Run};
use crate::text::{ago, plural, truncate};
use crate::theme::{self, glyph};
use crate::words;

/// The gutter: a cell of ground, the cursor, a space, the mark, a space.
const GUTTER: usize = 5;

pub fn draw(app: &App, frame: &mut Frame, area: Rect, now: SystemTime) {
    let Some(ledger) = &app.ledger else { return };
    let w = area.width as usize;
    let mut lines = vec![eyebrow("Recent runs", Some(ledger.entries.len())), blank()];

    if let Some(trouble) = &ledger.trouble {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", glyph::DONE), theme::tone(theme::RUST)),
            Span::styled(truncate(trouble, w.saturating_sub(2)), theme::faint()),
        ]));
        lines.push(blank());
    }

    let mut chosen_at = 0;
    for (i, entry) in ledger.entries.iter().enumerate() {
        let chosen = i == app.selected;
        if chosen {
            chosen_at = lines.len();
        }
        let row = Row::of(entry);
        let age = entry
            .written()
            .and_then(|t| ago(t, now))
            .unwrap_or_else(|| "-".into());

        let cursor = if chosen { glyph::CURSOR } else { " " };
        let mut first = vec![
            Span::raw(" "),
            Span::styled(cursor, theme::tone(theme::PERI)),
            Span::raw(" "),
            Span::styled(row.mark, theme::tone(row.tone)),
            Span::raw(" "),
        ];
        let body = w.saturating_sub(GUTTER + 1);
        first.extend(spread(
            vec![Span::styled(
                row.title,
                if row.titled {
                    theme::ink()
                } else {
                    theme::faint()
                },
            )],
            vec![Span::styled(age, theme::fainter())],
            body,
        ));
        let mut second = vec![Span::raw(" ".repeat(GUTTER))];
        second.extend(spread(row.meta, Vec::new(), body));

        if chosen {
            lines.push(grounded(first, w, theme::FACE));
            lines.push(grounded(second, w, theme::FACE));
        } else {
            lines.push(Line::from(first));
            lines.push(Line::from(second));
        }
        lines.push(blank());
    }

    // Keep the chosen row in view: scroll just far enough to show both of its
    // lines, and no further.
    let visible = area.height as usize;
    let scroll = (chosen_at + 2).saturating_sub(visible);
    paint(frame, area, &lines, scroll);
}

struct Row {
    mark: &'static str,
    tone: Color,
    title: String,
    /// False for a stand-in, drawn a step fainter than a real title.
    titled: bool,
    meta: Vec<Span<'static>>,
}

fn sep() -> Span<'static> {
    Span::styled(format!(" {} ", glyph::DOT), theme::fainter())
}

impl Row {
    fn of(entry: &Entry) -> Row {
        match entry {
            // The meta line is quiet all the way along. The mark on the line
            // above carries the colour; a second colour here, on four rows in
            // five, is an amber flood, and the word already says the state.
            Entry::Unreadable(u) => Row {
                mark: glyph::ALERT,
                tone: theme::RUST,
                title: u.file.clone(),
                titled: false,
                meta: vec![
                    Span::styled("Unreadable", theme::faint()),
                    sep(),
                    Span::styled(u.reason.clone(), theme::faint()),
                ],
            },
            Entry::Run(run) => {
                let (state, short) = words::completion(&run.completion);
                let (mark, tone) = mark(run, state.tone);
                let mut meta = vec![Span::styled(short, theme::faint())];
                if !run.failed_checks.is_empty() {
                    meta.push(sep());
                    meta.push(Span::styled(
                        if run.failed_checks.len() == 1 {
                            "a check failed".to_string()
                        } else {
                            format!("{} checks failed", run.failed_checks.len())
                        },
                        theme::faint(),
                    ));
                }
                meta.push(sep());
                meta.push(Span::styled(model(run), theme::faint()));
                meta.push(sep());
                meta.push(Span::styled(
                    if run.changes.is_empty() {
                        "no files".to_string()
                    } else {
                        plural(run.changes.len(), "file", "files")
                    },
                    theme::faint(),
                ));
                Row {
                    mark,
                    tone,
                    title: title(run),
                    titled: true,
                    meta,
                }
            }
        }
    }
}

/// A tick only for a run that completed with evidence and passed every check
/// the verifier can make; a cross for one that failed or was cancelled; an
/// alert for everything else, a failed check included.
pub(super) fn mark(run: &Run, state_tone: Color) -> (&'static str, Color) {
    if !run.failed_checks.is_empty() {
        return (glyph::ALERT, theme::RUST);
    }
    match run.completion.as_str() {
        "completed_with_evidence" => (glyph::CHECK, theme::OK),
        "failed" | "cancelled" => (glyph::CROSS, state_tone),
        _ => (glyph::ALERT, theme::RUST),
    }
}

pub fn model(run: &Run) -> String {
    catalog::model_name(&run.provider, &run.model_sku)
        .map(str::to_string)
        .unwrap_or_else(|| run.model_sku.clone())
}

fn title(run: &Run) -> String {
    match run.changes.as_slice() {
        [] => "Changed no files".to_string(),
        [one] => format!("Changed {}", one.path),
        [first, rest @ ..] => format!("Changed {} and {} more", first.path, rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Change, ChangeKind};
    use crate::text::width;

    fn run(changes: &[&str]) -> Run {
        Run {
            file: ".vitna/receipts/run-1.json".into(),
            written: None,
            run_id: "run-1".into(),
            base_commit: String::new(),
            provider: "anthropic".into(),
            model_sku: "claude-fable-5-1".into(),
            isolation: "guarded".into(),
            completion: "completed_with_evidence".into(),
            evidence: Vec::new(),
            changes: changes
                .iter()
                .map(|p| Change {
                    path: p.to_string(),
                    kind: ChangeKind::New,
                })
                .collect(),
            children: 0,
            signed: true,
            failed_checks: Vec::new(),
        }
    }

    #[test]
    fn a_title_says_what_changed_and_never_the_run_id() {
        assert_eq!(title(&run(&[])), "Changed no files");
        assert_eq!(title(&run(&["src/lib.rs"])), "Changed src/lib.rs");
        assert_eq!(title(&run(&["a", "b", "c"])), "Changed a and 2 more");
        assert!(!title(&run(&["a"])).contains("run-1"));
    }

    #[test]
    fn a_failed_check_takes_the_tick_away() {
        let mut r = run(&["a"]);
        assert_eq!(mark(&r, theme::OK).0, glyph::CHECK);
        r.failed_checks.push("Unknown completion state: x".into());
        assert_eq!(mark(&r, theme::OK).0, glyph::ALERT);
    }

    #[test]
    fn the_model_is_named_when_the_catalog_names_it() {
        let mut r = run(&[]);
        assert_eq!(model(&r), "Claude Fable 5.1");
        r.model_sku = "stub-messages-api".into();
        assert_eq!(model(&r), "stub-messages-api");
        assert!(width(&model(&r)) > 0);
    }
}
