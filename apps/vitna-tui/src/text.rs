//! Text, measured in cells and made safe to put in them.

use std::time::SystemTime;

use ratatui::text::Span;

/// How many cells a string occupies, measured the way ratatui lays it out.
pub fn width(s: &str) -> usize {
    Span::raw(s).width()
}

fn char_width(c: char) -> usize {
    let mut b = [0u8; 4];
    width(c.encode_utf8(&mut b))
}

/// Cuts a line to `max` cells, ending it with an ellipsis when anything was
/// cut, rather than letting it run on under whatever sits beside it.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// Wraps prose to `max` cells, breaking between words, and inside one only
/// when the word alone is wider than a line. Each explicit line break is kept.
/// Empty text is one empty line, so a caller never has to special-case it.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for word in paragraph.split_whitespace() {
            let w = width(word);
            if used > 0 && used + 1 + w <= max {
                line.push(' ');
                line.push_str(word);
                used += 1 + w;
                continue;
            }
            if used > 0 {
                out.push(std::mem::take(&mut line));
                used = 0;
            }
            if w <= max {
                line.push_str(word);
                used = w;
                continue;
            }
            // A word wider than the line: a path or a digest. Cut it at the
            // edge, since no break inside it would be any better.
            for c in word.chars() {
                let cw = char_width(c);
                if used + cw > max {
                    out.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(c);
                used += cw;
            }
        }
        out.push(line);
    }
    out
}

/// One row of the composer's text, as char indices `[start, end)` into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
}

/// Lays typed text out in rows of `max` cells, keeping every character where
/// it is, spaces included, so the cursor can be found in the result. Breaks
/// after the last space that fits, or at the edge inside a word too long for
/// a row. A space that meets the edge hangs there, invisible, and the next row
/// starts after it.
pub fn rows(chars: &[char], max: usize) -> Vec<Row> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let mut last_break: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push(Row { start, end: i });
            start = i + 1;
            used = 0;
            last_break = None;
            i += 1;
            continue;
        }
        let w = char_width(c);
        if used + w > max {
            if c == ' ' {
                out.push(Row { start, end: i + 1 });
                start = i + 1;
                used = 0;
                last_break = None;
                i += 1;
                continue;
            }
            if let Some(b) = last_break.filter(|b| *b > start) {
                out.push(Row { start, end: b });
                start = b;
            } else if i > start {
                out.push(Row { start, end: i });
                start = i;
            }
            used = chars[start..i].iter().map(|c| char_width(*c)).sum();
            last_break = None;
        }
        used += w;
        if c == ' ' {
            last_break = Some(i + 1);
        }
        i += 1;
    }
    out.push(Row {
        start,
        end: chars.len(),
    });
    out
}

/// Where the cursor at char index `at` sits: its row, and its column in cells.
/// A cursor on a wrap point belongs to the row that starts there, so it shows
/// before the next character rather than past the end of the last one.
pub fn cursor_in(chars: &[char], rows: &[Row], at: usize) -> (usize, usize) {
    let row = rows.iter().rposition(|r| r.start <= at).unwrap_or(0);
    let r = rows[row];
    let end = at.min(r.end).max(r.start);
    (
        row,
        chars[r.start..end].iter().map(|c| char_width(*c)).sum(),
    )
}

/// Makes every character a terminal would act on, or that hides or reorders
/// text, visible instead.
///
/// Anything that reaches the screen from a file, from git or from the daemon
/// goes through this first. A receipt is a file in the repository and the
/// repository is untrusted input: a path holding ESC could drive the terminal
/// this is drawn in, and a right-to-left override could make one path read as
/// another. C0 controls print as their Control Pictures (ESC as U+241B), the
/// way `apps/vitna-desktop`'s `showControls` prints them; invisible and
/// reordering characters print as `<U+XXXX>`, since they have no picture.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let code = c as u32;
        match c {
            _ if code < 0x20 => out.push(char::from_u32(0x2400 + code).unwrap_or('\u{fffd}')),
            '\u{7f}' => out.push('\u{2421}'),
            '\u{80}'..='\u{9f}' => out.push('\u{fffd}'),
            '\u{ad}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}' => out.push_str(&format!("<U+{code:04X}>")),
            _ => out.push(c),
        }
    }
    out
}

/// Text that may run to several lines, each line made safe. Carriage returns
/// are dropped rather than pictured, since a CRLF file is ordinary.
pub fn clean_lines(s: &str) -> Vec<String> {
    s.replace("\r\n", "\n").split('\n').map(clean).collect()
}

/// How long ago, in the window's words. None when the time is unknown or in
/// the future, which the caller prints as a dash rather than as "just now".
pub fn ago(when: SystemTime, now: SystemTime) -> Option<String> {
    let secs = now.duration_since(when).ok()?.as_secs();
    Some(match secs {
        0..=9 => "just now".to_string(),
        10..=59 => format!("{secs} seconds ago"),
        60..=119 => "1 minute ago".to_string(),
        120..=3599 => format!("{} minutes ago", secs / 60),
        3600..=7199 => "1 hour ago".to_string(),
        7200..=86_399 => format!("{} hours ago", secs / 3600),
        86_400..=172_799 => "1 day ago".to_string(),
        _ => format!("{} days ago", secs / 86_400),
    })
}

/// A digest in fixed groups, so it can be read aloud and compared by eye.
pub fn group_digest(digest: &str) -> String {
    let bare = digest.strip_prefix("sha256:").unwrap_or(digest);
    bare.chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|g| g.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// "1 more line", "4 more lines": what a collapsed list still holds.
pub fn more(n: usize, one: &str, many: &str) -> String {
    format!("{n} more {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_wraps_between_words_and_keeps_its_breaks() {
        assert_eq!(
            wrap("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap("", 10), vec![""]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn truncation_says_it_cut() {
        assert_eq!(truncate("abcdef", 6), "abcdef");
        assert_eq!(truncate("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn typed_rows_keep_every_character_and_find_the_cursor() {
        let text: Vec<char> = "hello world again".chars().collect();
        let r = rows(&text, 11);
        let joined: String = r
            .iter()
            .map(|r| text[r.start..r.end].iter().collect::<String>())
            .collect();
        assert_eq!(
            joined, "hello world again",
            "a character went missing in the layout"
        );
        assert_eq!(r.len(), 2);
        // The cursor at the very end sits after "again" on the second row.
        let (row, col) = cursor_in(&text, &r, text.len());
        assert_eq!((row, col), (1, 5));
        // At the wrap point it starts the second row, not ends the first.
        let (row, col) = cursor_in(&text, &r, r[1].start);
        assert_eq!((row, col), (1, 0));
    }

    #[test]
    fn a_newline_starts_a_row_and_is_not_drawn() {
        let text: Vec<char> = "ab\ncd".chars().collect();
        let r = rows(&text, 10);
        assert_eq!(r, vec![Row { start: 0, end: 2 }, Row { start: 3, end: 5 }]);
        assert_eq!(cursor_in(&text, &r, 3), (1, 0));
        assert_eq!(cursor_in(&text, &r, 2), (0, 2));
    }

    #[test]
    fn nothing_reaches_the_terminal_that_could_drive_it() {
        // A title-setting OSC, a clear screen, a bell, and a right-to-left
        // override that would make "txt.exe" read as "exe.txt".
        let hostile = "\u{1b}]0;ADMINISTRATOR\u{7}\u{1b}[2J report\u{202e}exe.txt";
        let shown = clean(hostile);
        assert!(
            !shown.chars().any(|c| c.is_control()),
            "a control character survived: {shown:?}"
        );
        assert!(!shown.contains('\u{202e}'), "the override survived");
        assert!(shown.contains('\u{241b}'), "ESC should show as its picture");
        assert!(shown.contains("<U+202E>"), "the override should be named");
    }

    #[test]
    fn ages_read_as_the_window_words_them() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let at = |s| now - std::time::Duration::from_secs(s);
        assert_eq!(ago(at(3), now).as_deref(), Some("just now"));
        assert_eq!(ago(at(90), now).as_deref(), Some("1 minute ago"));
        assert_eq!(ago(at(3 * 86_400), now).as_deref(), Some("3 days ago"));
        assert_eq!(
            ago(now + std::time::Duration::from_secs(5), now),
            None,
            "a future time is unknown, not new"
        );
    }

    #[test]
    fn digests_group_by_four() {
        assert_eq!(group_digest("sha256:0123456789ab"), "0123 4567 89ab");
    }
}
