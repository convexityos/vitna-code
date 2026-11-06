//! What the page's Routines map prices its agents with (convexityos/vitna,
//! APPS.md section 13): the runtime ledger `vitna app --ledger` names, which
//! the Vitna runtime appends a row to for every model request it answers
//! (`VITNA_LEDGER` there), and the public price history of each sku those
//! rows name, asked of the record the operator named at the terminal
//! (`--record`, vitna.ai unless told otherwise).
//!
//! Read only, and it folds nothing: the page prices the rows with the code
//! `platform/connections/agents-run.mjs` uses there, so the page and the
//! terminal cannot price one request two ways. What leaves this machine is
//! each sku slug the page asks to price, one GET to the record's price
//! history per slug, and nothing else: no row, no token count, no agent, no
//! run. The page names neither the file nor the record; it can ask only for
//! slugs, and only of the origin the operator gave.

use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// The most of the ledger one reply carries: its newest rows. A row is a few
/// hundred bytes, so this is some hundred thousand requests.
pub const MAX_LEDGER_BYTES: u64 = 32 * 1024 * 1024;
/// The most slugs priced in one request. The registry holds a few hundred
/// skus, and one runtime serves a handful of them.
pub const MAX_SLUGS: usize = 64;
/// The most one history may hold. vitna.ai's are a few kilobytes.
pub const MAX_HISTORY_BYTES: usize = 1024 * 1024;
/// How long one history may take, from asking to the last byte.
pub const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);
/// The record asked unless the operator names another, or none.
pub const DEFAULT_RECORD: &str = "https://vitna.ai";

/// Why the ledger was not sent.
#[derive(Debug, PartialEq, Eq)]
pub enum Unread {
    NotAFile,
    NotText,
    Unreadable,
}

/// The newest rows of the ledger, as text, and how many bytes before them
/// were left out. A missing file is an empty ledger: the runtime makes it on
/// its first request. The cut falls after a newline, so no row is sent in
/// part from its start; a last line still being written is the page's to
/// leave for the next read, as `agents-run.mjs` leaves it.
pub fn read(path: &Path) -> Result<Value, Unread> {
    read_capped(path, MAX_LEDGER_BYTES)
}

fn read_capped(path: &Path, max: u64) -> Result<Value, Unread> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "ledger": "", "left_out_bytes": 0 }))
        }
        Err(_) => return Err(Unread::Unreadable),
    };
    if !meta.is_file() {
        return Err(Unread::NotAFile);
    }
    let mut file = std::fs::File::open(path).map_err(|_| Unread::Unreadable)?;
    let mut start = meta.len().saturating_sub(max);
    // Past the cap, the read starts one byte early, so the byte before the
    // cut says whether the cut falls on a row's start.
    let early = u64::from(start > 0);
    file.seek(SeekFrom::Start(start - early))
        .map_err(|_| Unread::Unreadable)?;
    let mut bytes = Vec::new();
    // The runtime may append while this reads, so what is read is bounded
    // here rather than by the size taken before.
    file.take(max + early)
        .read_to_end(&mut bytes)
        .map_err(|_| Unread::Unreadable)?;
    if start > 0 {
        // Drop the byte before the cut and, if the cut fell inside a row,
        // the rest of that row up to and including its newline.
        let cut = if bytes.first() == Some(&b'\n') {
            1
        } else {
            match bytes.iter().position(|&b| b == b'\n') {
                Some(at) => at + 1,
                None => bytes.len(),
            }
        };
        bytes.drain(..cut);
        start += cut as u64 - early;
    }
    let ledger = String::from_utf8(bytes).map_err(|_| Unread::NotText)?;
    Ok(json!({ "ledger": ledger, "left_out_bytes": start }))
}

/// A slug as the record names a sku: a lower-case letter or digit, then up
/// to 127 lower-case letters, digits, dots and hyphens. So it is one path
/// segment and can never name a parent or another route.
pub fn slug(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 128
        && matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '-'))
}

/// The record's origin as the operator gave it, if it is one this server
/// will ask: https anywhere, or http on this machine's loopback, with no
/// path, query, fragment or credentials. Returned without a trailing slash.
pub fn record_origin(given: &str) -> Result<String, String> {
    let refused = || {
        format!(
            "--record {given} is not an origin vitna app will ask: give https://<host>, http on this machine's loopback, or none."
        )
    };
    let url = reqwest::Url::parse(given).map_err(|_| refused())?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !scheme_ok
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(refused());
    }
    Ok(url.origin().ascii_serialization())
}

/// One history as the record answered it: its status and its body, when the
/// body is JSON no larger than `MAX_HISTORY_BYTES`. A record that could not
/// be reached has neither. Redirects are not followed, so an answer is
/// always the named origin's own.
async fn history(client: &reqwest::Client, record: &str, slug: &str) -> Value {
    let url = format!("{record}/api/v1/skus/{slug}/history");
    let Ok(mut answered) = client.get(url).timeout(HISTORY_TIMEOUT).send().await else {
        return json!({ "slug": slug, "status": null, "body": null });
    };
    let status = answered.status().as_u16();
    let mut bytes = Vec::new();
    loop {
        match answered.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_HISTORY_BYTES => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return json!({ "slug": slug, "status": status, "body": null }),
        }
    }
    let body = serde_json::from_slice::<Value>(&bytes).ok();
    json!({ "slug": slug, "status": status, "body": body })
}

/// Every slug's history, in the order asked, each asked at once.
pub async fn histories(client: &reqwest::Client, record: &str, slugs: Vec<String>) -> Value {
    let asked: Vec<_> = slugs
        .into_iter()
        .map(|slug| {
            let client = client.clone();
            let record = record.to_string();
            tokio::spawn(async move { history(&client, &record, &slug).await })
        })
        .collect();
    let mut found = Vec::with_capacity(asked.len());
    for one in asked {
        found.push(one.await.unwrap_or(Value::Null));
    }
    json!({ "histories": found })
}

/// The client every history is asked with: no redirects, no cookies, and a
/// name that says what is asking.
pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("vitna-app/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("could not make the client prices are asked with: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_one_path_segment() {
        for good in [
            "claude-opus-4-8",
            "gpt-4.1-mini",
            "o3",
            "x".repeat(128).as_str(),
        ] {
            assert!(slug(good), "{good}");
        }
        for bad in [
            "",
            "-a",
            ".a",
            "..",
            "a/b",
            "A",
            "a b",
            "a%2fb",
            "a?b",
            "a#b",
            &"x".repeat(129),
        ] {
            assert!(!slug(bad), "{bad}");
        }
    }

    #[test]
    fn a_record_is_an_https_origin_or_loopback_http() {
        assert_eq!(
            record_origin("https://vitna.ai").unwrap(),
            "https://vitna.ai"
        );
        assert_eq!(
            record_origin("https://vitna.ai/").unwrap(),
            "https://vitna.ai"
        );
        assert_eq!(
            record_origin("http://127.0.0.1:4181").unwrap(),
            "http://127.0.0.1:4181"
        );
        assert_eq!(
            record_origin("http://localhost:4181").unwrap(),
            "http://localhost:4181"
        );
        assert_eq!(
            record_origin("http://[::1]:4181").unwrap(),
            "http://[::1]:4181"
        );
        for bad in [
            "http://vitna.ai",
            "https://vitna.ai/api",
            "https://vitna.ai?x=1",
            "https://vitna.ai#x",
            "https://user@vitna.ai",
            "ftp://vitna.ai",
            "vitna.ai",
            "http://10.0.0.1",
        ] {
            assert!(record_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_newest_rows_are_sent_and_a_row_cut_at_the_start_is_dropped() {
        let dir = std::env::temp_dir().join(format!("vitna_ledger_unit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ledger.jsonl");
        assert_eq!(
            read(&path).unwrap(),
            json!({ "ledger": "", "left_out_bytes": 0 })
        );
        std::fs::write(&path, "{\"a\":1}\n{\"a\":2}\n{\"a\":").unwrap();
        assert_eq!(
            read(&path).unwrap(),
            json!({ "ledger": "{\"a\":1}\n{\"a\":2}\n{\"a\":", "left_out_bytes": 0 })
        );
        // Past the cap, the newest rows, from the first whole one: a cut on
        // a row's start keeps that row, and a cut inside one drops it.
        assert_eq!(
            read_capped(&path, 13).unwrap(),
            json!({ "ledger": "{\"a\":2}\n{\"a\":", "left_out_bytes": 8 })
        );
        assert_eq!(
            read_capped(&path, 12).unwrap(),
            json!({ "ledger": "{\"a\":", "left_out_bytes": 16 })
        );
        assert_eq!(read(&dir), Err(Unread::NotAFile));
        std::fs::write(&path, [0xffu8, b'\n']).unwrap();
        assert_eq!(read(&path), Err(Unread::NotText));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
