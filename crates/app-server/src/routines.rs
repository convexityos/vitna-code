//! The routines the page's Routines place reads (convexityos/vitna, APPS.md
//! section 13): the definitions in the folder `vitna app --routines` names,
//! and for each one the receipts and the lock that the routine runner
//! (`platform/connections/run.mjs` there) keeps under its `stateDirectory`.
//!
//! Read only. Nothing here writes, runs, locks or unlocks anything, so it can
//! be read while the runner's `watch` is running. It folds nothing either:
//! the page folds what it is sent into rows with the same code as
//! `run.mjs status`, so the page and the terminal cannot disagree. The page
//! names no path. It asks for the routines and is sent what the person's own
//! definitions name, and nothing else.

use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The most definitions read from the folder, by name; the rest are counted.
pub const MAX_DEFINITIONS: usize = 64;
/// The most one definition may hold. A prompt is at most 32 KiB, and a fixed
/// call's arguments at most 16 KiB.
pub const MAX_DEFINITION_BYTES: u64 = 256 * 1024;
/// The most one routine's receipts may hold. The runner refuses to start a
/// run once they pass 10 MiB, and a run adds a few rows after that check.
pub const MAX_RECEIPT_BYTES: u64 = 12 * 1024 * 1024;
/// The most one reply carries, every routine's files together.
pub const MAX_REPLY_BYTES: u64 = 48 * 1024 * 1024;

/// A routine id as the runner takes one: a lower-case letter, then up to 63
/// lower-case letters, digits and hyphens. So it can never name a parent
/// folder or another one.
fn routine_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

fn problem(error: &str, message: &str) -> Value {
    json!({ "error": error, "message": message })
}

/// Why a file was not sent, in the reply's own words.
enum Unread {
    Missing,
    NotAFile,
    TooLarge,
    ReplyFull,
    NotText,
    Unreadable,
}

/// A file's text, if it is a file no larger than `max` and fits in what is
/// left of the reply. What it reads is charged to `budget`.
fn text(path: &Path, max: u64, budget: &mut u64) -> Result<String, Unread> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(_) => return Err(Unread::Unreadable),
    };
    if !meta.is_file() {
        return Err(Unread::NotAFile);
    }
    if meta.len() > max {
        return Err(Unread::TooLarge);
    }
    if meta.len() > *budget {
        return Err(Unread::ReplyFull);
    }
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|_| Unread::Unreadable)?;
    // The runner may append while this reads, so the size is checked again
    // on what was read rather than taken from before.
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Unread::Unreadable)?;
    let read = bytes.len() as u64;
    if read > max {
        return Err(Unread::TooLarge);
    }
    if read > *budget {
        return Err(Unread::ReplyFull);
    }
    let text = String::from_utf8(bytes).map_err(|_| Unread::NotText)?;
    *budget -= read;
    Ok(text)
}

/// When the runner took the routine's lock, in milliseconds since the epoch,
/// or null when none is held.
fn locked_at(lock: &Path) -> Result<Value, ()> {
    match std::fs::metadata(lock) {
        Ok(meta) => {
            let at = meta.modified().map_err(|_| ())?;
            let ms = at.duration_since(UNIX_EPOCH).map_err(|_| ())?.as_millis();
            Ok(json!(ms as u64))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(_) => Err(()),
    }
}

fn one(file: &str, path: &Path, budget: &mut u64) -> Value {
    let mut row = json!({
        "file": file,
        "definition": null,
        "receipts": null,
        "locked_at_ms": null,
        "problem": null,
    });
    let definition = match text(path, MAX_DEFINITION_BYTES, budget) {
        Ok(text) => text,
        Err(unread) => {
            row["problem"] = match unread {
                Unread::TooLarge => problem(
                    "too_large",
                    "This definition is larger than 256 KiB, so it was not read.",
                ),
                Unread::ReplyFull => problem(
                    "reply_full",
                    "The routines before it filled the reply, so it was not read.",
                ),
                Unread::NotText => problem("not_text", "This file is not UTF-8 text."),
                _ => problem("unreadable", "vitna app could not read this file."),
            };
            return row;
        }
    };
    row["definition"] = Value::String(definition.clone());
    let Ok(parsed) = serde_json::from_str::<Value>(&definition) else {
        row["problem"] = problem("not_json", "This file is not JSON.");
        return row;
    };
    let id = parsed
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| routine_id(id));
    let state = parsed
        .get("stateDirectory")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|state| state.is_absolute());
    let (Some(id), Some(state)) = (id, state) else {
        row["problem"] = problem(
            "no_state",
            "This definition names no routine id and absolute stateDirectory, so it has no receipts to read.",
        );
        return row;
    };
    let folder = state.join(id);
    match text(&folder.join("receipts.jsonl"), MAX_RECEIPT_BYTES, budget) {
        Ok(receipts) => row["receipts"] = Value::String(receipts),
        // A routine that has never run has no receipts yet: no rows.
        Err(Unread::Missing) => row["receipts"] = Value::String(String::new()),
        Err(unread) => {
            row["problem"] = match unread {
                Unread::TooLarge => problem(
                    "receipts_too_large",
                    "Its receipts are larger than 12 MiB, so they were not read.",
                ),
                Unread::ReplyFull => problem(
                    "reply_full",
                    "The routines before it filled the reply, so its receipts were not read.",
                ),
                Unread::NotText => problem("receipts_not_text", "Its receipts are not UTF-8 text."),
                _ => problem(
                    "receipts_unreadable",
                    "vitna app could not read its receipts.",
                ),
            };
            return row;
        }
    }
    match locked_at(&folder.join("lock")) {
        Ok(at) => row["locked_at_ms"] = at,
        Err(()) => {
            row["receipts"] = Value::Null;
            row["problem"] = problem(
                "lock_unreadable",
                "vitna app could not tell whether its lock is held.",
            );
        }
    }
    row
}

/// Every definition in `dir` (each `*.json` file in it, by name), with its
/// receipts and lock: `{ routines: [...], left_out }`, where `left_out` counts
/// the definitions past [`MAX_DEFINITIONS`].
pub fn read(dir: &Path) -> Result<Value, ()> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|_| ())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let path = entry.path();
            // A folder named like a definition is not one.
            (name.ends_with(".json") && std::fs::metadata(&path).is_ok_and(|m| m.is_file()))
                .then_some((name, path))
        })
        .collect();
    files.sort();
    let left_out = files.len().saturating_sub(MAX_DEFINITIONS);
    let mut budget = MAX_REPLY_BYTES;
    let routines: Vec<Value> = files
        .iter()
        .take(MAX_DEFINITIONS)
        .map(|(name, path)| one(name, path, &mut budget))
        .collect();
    Ok(json!({ "routines": routines, "left_out": left_out }))
}

#[cfg(test)]
mod tests {
    use super::{routine_id, text, Unread};

    #[test]
    fn a_file_is_read_only_within_what_is_left_of_the_reply_and_charged_to_it() {
        let dir =
            std::env::temp_dir().join(format!("vitna_routines_budget_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("receipts.jsonl");
        std::fs::write(&file, "0123456789").unwrap();
        let mut budget = 9;
        assert!(matches!(
            text(&file, 100, &mut budget),
            Err(Unread::ReplyFull)
        ));
        assert_eq!(budget, 9, "nothing was charged for a file not sent");
        assert!(matches!(text(&file, 9, &mut 100), Err(Unread::TooLarge)));
        let mut budget = 15;
        assert_eq!(
            text(&file, 100, &mut budget).ok().as_deref(),
            Some("0123456789")
        );
        assert_eq!(budget, 5);
        std::fs::write(&file, [0xff, 0xfe]).unwrap();
        assert!(matches!(text(&file, 100, &mut 100), Err(Unread::NotText)));
        assert!(matches!(
            text(&dir.join("absent"), 100, &mut 100),
            Err(Unread::Missing)
        ));
        assert!(matches!(text(&dir, 100, &mut 100), Err(Unread::NotAFile)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_routine_id_is_the_runners_and_never_names_another_folder() {
        for good in [
            "a",
            "brief",
            "daily-brief-2",
            &format!("a{}", "b".repeat(63)),
        ] {
            assert!(routine_id(good), "{good}");
        }
        for bad in [
            "",
            "Brief",
            "2brief",
            "-brief",
            "../x",
            "a/b",
            "a\\b",
            "a.b",
            "a b",
            &format!("a{}", "b".repeat(64)),
        ] {
            assert!(!routine_id(bad), "{bad}");
        }
    }
}
