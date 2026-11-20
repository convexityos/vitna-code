//! One run, as its receipt records it.
//!
//! A signed JSON document is not a review surface (CURSOR-CHANGELOG-LESSONS
//! 6), so this is one: the run's standing in words, what it changed, the
//! evidence behind it graded by how much weight each item carries, and, check
//! by check, what this terminal could and could not establish about the file.
//! That last part is the product's own claim made visible: a receipt says what
//! is known and what is not, and so does this view.
//!
//! The identifiers the receipt is built on (the run id, the base commit, the
//! file) are here too, once, at the foot. This is the view they belong in.

use std::time::SystemTime;

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::{blank, eyebrow, paint, rule};
use crate::app::App;
use crate::ledger::{Entry, Run};
use crate::text::{ago, plural, wrap};
use crate::theme::{self, glyph};
use crate::words;

pub fn draw(app: &App, index: usize, frame: &mut Frame, area: Rect, now: SystemTime) {
    let Some(entry) = app.ledger.as_ref().and_then(|l| l.entries.get(index)) else {
        return;
    };
    let w = area.width as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} Runs", glyph::BACK), theme::faint()),
            Span::styled("  esc", theme::fainter()),
        ]),
        blank(),
    ];
    match entry {
        Entry::Run(run) => standing(&mut lines, run, w, now),
        Entry::Unreadable(u) => {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", glyph::ALERT), theme::tone(theme::RUST)),
                Span::styled("Not a receipt this terminal can read", theme::title()),
            ]));
            for part in wrap(&format!("{} {}.", u.file, u.reason), w.saturating_sub(2)) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(part, theme::faint()),
                ]));
            }
        }
    }
    app.scroll_room
        .set(lines.len().saturating_sub(area.height as usize));
    paint(frame, area, &lines, app.scroll);
}

fn standing(lines: &mut Vec<Line<'static>>, run: &Run, w: usize, now: SystemTime) {
    let (state, _) = words::completion(&run.completion);
    let (mark, tone) = super::runs::mark(run, state.tone);
    for (i, part) in wrap(&state.word, w.saturating_sub(2))
        .into_iter()
        .enumerate()
    {
        let lead = if i == 0 {
            format!("{mark} ")
        } else {
            "  ".into()
        };
        lines.push(Line::from(vec![
            Span::styled(lead, theme::tone(tone)),
            Span::styled(part, theme::title()),
        ]));
    }

    let mut facts = vec![
        if run.changes.is_empty() {
            "No files changed".to_string()
        } else {
            format!("{} changed", plural(run.changes.len(), "file", "files"))
        },
        super::runs::model(run),
        words::isolation(&run.isolation),
    ];
    if let Some(ref pd) = run.policy_digest {
        let short_pd: String = pd.chars().take(12).collect();
        facts.push(format!("policy {short_pd}"));
    }
    if run.children > 0 {
        facts.push(format!(
            "with {}",
            plural(run.children, "sub-agent receipt", "sub-agent receipts")
        ));
    }
    if let Some(age) = run.written.and_then(|t| ago(t, now)) {
        facts.push(format!("written {age}"));
    }
    for part in wrap(
        &facts.join(&format!(" {} ", glyph::DOT)),
        w.saturating_sub(2),
    ) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(part, theme::faint()),
        ]));
    }

    // What changed. The receipt holds a hash before and after each file, not
    // the lines, so this says new, modified or deleted, and nothing it cannot.
    lines.push(blank());
    lines.push(eyebrow("Changed", Some(run.changes.len())));
    lines.push(rule(w));
    if run.changes.is_empty() {
        lines.push(Line::from(Span::styled("  None.", theme::fainter())));
    }
    let kind_w = run
        .changes
        .iter()
        .map(|c| c.kind.word().len())
        .max()
        .unwrap_or(0)
        + 3;
    for change in &run.changes {
        let kind = format!("  {:<width$}", change.kind.word(), width = kind_w - 2);
        for (i, part) in wrap(&change.path, w.saturating_sub(kind_w))
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 {
                Span::styled(kind.clone(), theme::fainter())
            } else {
                Span::raw(" ".repeat(kind_w))
            };
            lines.push(Line::from(vec![lead, Span::styled(part, theme::ink())]));
        }
    }

    // The evidence, strongest first as the receipt lists it, each item marked
    // with how much weight its grade carries. A claim the model made is shown
    // and marked as the weakest kind.
    lines.push(blank());
    lines.push(eyebrow("Evidence", Some(run.evidence.len())));
    lines.push(rule(w));
    if run.evidence.is_empty() {
        lines.push(Line::from(Span::styled(
            "  None recorded.",
            theme::fainter(),
        )));
    }
    for item in &run.evidence {
        let grade = words::grade(&item.grade);
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", glyph::DONE), theme::tone(grade.tone)),
            Span::styled(grade.word, theme::faint()),
        ]));
        for part in wrap(&item.description, w.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(part, theme::ink()),
            ]));
        }
    }

    if !run.runner_statements.is_empty() {
        lines.push(blank());
        lines.push(eyebrow("Runner", Some(run.runner_statements.len())));
        lines.push(rule(w));
        for st in &run.runner_statements {
            let backend = st.sandbox_backend.as_deref().unwrap_or("unspecified");
            let enforcement = st.sandbox_enforcement.as_deref().unwrap_or("unspecified");
            let summary = format!("{} ({}/{})", st.action_id, backend, enforcement);
            for part in wrap(&summary, w.saturating_sub(2)) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(part, theme::faint()),
                ]));
            }
        }
    }

    if !run.egress_calls.is_empty() {
        lines.push(blank());
        lines.push(eyebrow("Egress", Some(run.egress_calls.len())));
        lines.push(rule(w));
        for eg in &run.egress_calls {
            let short_sha: String = eg.response_sha256.chars().take(12).collect();
            let summary = format!(
                "{} {} -> {} ({}B, {}ms, sha256 {})",
                eg.method, eg.redacted_url, eg.status_code, eg.bytes_received, eg.latency_ms, short_sha
            );
            for part in wrap(&summary, w.saturating_sub(2)) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(part, theme::ink()),
                ]));
            }
        }
    }

    if let Some(mp) = &run.mandate_provenance {
        lines.push(blank());
        lines.push(eyebrow("Mandate (am/1)", None));
        lines.push(rule(w));
        let short_chain: String = mp.chain_hash.chars().take(16).collect();
        let summary = format!(
            "root {} {} spent {} {} ({} receipts) {} chain {}",
            mp.root_jti,
            glyph::DOT,
            mp.total_spent,
            mp.currency,
            mp.spend_count,
            glyph::DOT,
            short_chain
        );
        for part in wrap(&summary, w.saturating_sub(2)) {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(part, theme::ink()),
            ]));
        }
        if !mp.path.is_empty() {
            let path_line = format!("path {} {} ledger {}", mp.path.join(" -> "), glyph::DOT, mp.ledger);
            for part in wrap(&path_line, w.saturating_sub(2)) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(part, theme::faint()),
                ]));
            }
        }
    }

    // What this terminal could establish, check by check.
    lines.push(blank());
    lines.push(eyebrow("Checks", None));
    lines.push(rule(w));
    if run.failed_checks.is_empty() {
        check(
            lines,
            w,
            glyph::CHECK,
            theme::OK,
            "Every check the verifier makes without a key passed.",
        );
    }
    for failed in &run.failed_checks {
        check(lines, w, glyph::CROSS, theme::RUST, &format!("{failed}."));
    }
    if run.signature_verified == Some(true) {
        let short_pk = run
            .device_public_key
            .as_deref()
            .map(|k| k.chars().take(12).collect::<String>())
            .unwrap_or_default();
        check(
            lines,
            w,
            glyph::CHECK,
            theme::OK,
            &format!(
                "Signature verified against embedded device key {short_pk}. Confirm trust with `vitna verify --key`."
            ),
        );
    } else if run.signature_verified == Some(false) {
        check(
            lines,
            w,
            glyph::CROSS,
            theme::RUST,
            "Signature check failed against the embedded device key. The receipt was modified after signing or carries a bad signature.",
        );
    } else if run.signed {
        check(
            lines,
            w,
            glyph::PENDING,
            theme::FAINTER,
            "Signature not checked. No embedded device public key is present in this receipt; check it with `vitna verify --key`.",
        );
    } else {
        check(
            lines,
            w,
            glyph::CROSS,
            theme::RUST,
            "No signature. The receipt carries none, which is a fault.",
        );
    }

    lines.push(blank());
    let short = |s: &str| s.chars().take(7).collect::<String>();
    let foot = if run.base_commit.is_empty() {
        format!("{} {} {}", run.run_id, glyph::DOT, run.file)
    } else {
        format!(
            "{} {} base {} {} {}",
            run.run_id,
            glyph::DOT,
            short(&run.base_commit),
            glyph::DOT,
            run.file
        )
    };
    for part in wrap(&foot, w) {
        lines.push(Line::from(Span::styled(part, theme::fainter())));
    }
}

fn check(
    lines: &mut Vec<Line<'static>>,
    w: usize,
    mark: &'static str,
    tone: ratatui::style::Color,
    text: &str,
) {
    for (i, part) in wrap(text, w.saturating_sub(2)).into_iter().enumerate() {
        let lead = if i == 0 {
            Span::styled(format!("{mark} "), theme::tone(tone))
        } else {
            Span::raw("  ")
        };
        let ink = if tone == theme::RUST {
            theme::ink()
        } else {
            theme::faint()
        };
        lines.push(Line::from(vec![lead, Span::styled(part, ink)]));
    }
}
