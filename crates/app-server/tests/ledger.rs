//! `vitna app --ledger`: the runtime ledger the page's Routines map prices,
//! and the price histories it asks the record for, over real HTTP. The
//! record is a fake on this machine's loopback that writes down every request
//! it is sent, so the tests can say exactly what left the machine.

use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use vitna_app_server::{AppConfig, AppServer, Launcher};
use vitna_runner::FakeRunner;

struct Harness {
    base: PathBuf,
    port: u16,
    launcher: Launcher,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn base_for(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("vitna_ledger_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("project")).unwrap();
    std::fs::create_dir_all(base.join("runtime")).unwrap();
    base
}

async fn bind(
    base: &Path,
    ledger: Option<PathBuf>,
    record: Option<String>,
) -> Result<AppServer, String> {
    let runner = FakeRunner::new(base.join("runner.journal")).expect("fake runner");
    AppServer::bind(
        AppConfig {
            folder: base.join("project"),
            port: 0,
            ui: None,
            allow_unsandboxed: false,
            routines: None,
            ledger,
            record,
        },
        Arc::new(Mutex::new(runner)),
        SigningKey::from_bytes(&[9u8; 32]),
    )
    .await
}

impl Harness {
    async fn start(tag: &str, ledger: bool, record: Option<String>) -> Self {
        let base = base_for(tag);
        let at = ledger.then(|| base.join("runtime").join("ledger.jsonl"));
        let server = bind(&base, at, record).await.expect("bind");
        let port = server.port();
        let launcher = server.launcher();
        tokio::spawn(server.serve());
        Self {
            base,
            port,
            launcher,
        }
    }

    fn ledger(&self) -> PathBuf {
        self.base.join("runtime").join("ledger.jsonl")
    }

    async fn session(&self) -> String {
        let code = self
            .launcher
            .url()
            .split_once("#launch=")
            .unwrap()
            .1
            .to_string();
        let reply = self
            .api(
                None,
                "session",
                json!({ "launch": code }).to_string().as_bytes(),
            )
            .await;
        assert_eq!(reply.0, 200, "{}", reply.1);
        reply.json()["token"].as_str().unwrap().to_string()
    }

    async fn api(&self, token: Option<&str>, name: &str, body: &[u8]) -> Reply {
        let mut head = format!(
            "POST /api/v1/{name} HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://localhost:{port}\r\nSec-Fetch-Site: same-origin\r\nConnection: close\r\nContent-Length: {len}\r\n",
            port = self.port,
            len = body.len()
        );
        if let Some(token) = token {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        head.push_str("\r\n");
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut raw = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            stream.read_to_end(&mut raw),
        )
        .await
        .expect("the server left the connection open")
        .unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = String::from_utf8_lossy(&raw[..split])
            .split(' ')
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap();
        Reply(
            status,
            String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
        )
    }
}

struct Reply(u16, String);

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.1).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.1))
    }
    fn refused(&self) -> (u16, String) {
        (
            self.0,
            self.json()["error"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }
}

/// A record on this machine's loopback. It answers a history for the slugs
/// it knows, `unknown_sku` for the rest, a redirect for `moved`, and a body
/// past the cap for `huge`, and writes down every request's head.
async fn fake_record() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => raw.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&raw).into_owned();
                log.lock().unwrap().push(head.clone());
                let target = head.split(' ').nth(1).unwrap_or_default().to_string();
                let slug = target
                    .strip_prefix("/api/v1/skus/")
                    .and_then(|t| t.strip_suffix("/history"))
                    .unwrap_or_default()
                    .to_string();
                let (status, extra, body) = match slug.as_str() {
                    "claude-opus-4-8" => (
                        "200 OK",
                        "",
                        json!({ "sku": { "slug": slug }, "periods": [{ "from": "2026-01-01T00:00:00Z", "input": 5 }] }).to_string(),
                    ),
                    "moved" => ("302 Found", "Location: http://127.0.0.1:9/elsewhere\r\n", String::new()),
                    "huge" => ("200 OK", "", format!("\"{}\"", "x".repeat(1024 * 1024 + 1))),
                    _ => (
                        "404 Not Found",
                        "",
                        json!({ "error": { "code": "unknown_sku" } }).to_string(),
                    ),
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (origin, seen)
}

#[tokio::test]
async fn the_ledger_is_read_as_it_stands_and_never_written() {
    let (record, _) = fake_record().await;
    let h = Harness::start("read", true, Some(record.clone())).await;
    let token = h.session().await;
    let hello = h.api(Some(&token), "hello", b"").await.json();
    assert_eq!(
        (hello["ledger"].as_str(), hello["record"].as_str()),
        (Some("ledger.jsonl"), Some(record.as_str()))
    );

    // The runtime has not made it yet.
    assert_eq!(
        h.api(Some(&token), "ledger", b"").await.json(),
        json!({ "ledger": "", "left_out_bytes": 0 })
    );

    let rows = "{\"agentId\":\"issue-brief\",\"sku\":\"anthropic/claude-opus-4-8\"}\n{\"agentId\":\"harness:opencode\"";
    std::fs::write(h.ledger(), rows).unwrap();
    let read = h.api(Some(&token), "ledger", b"").await.json();
    assert_eq!(
        read,
        json!({ "ledger": rows, "left_out_bytes": 0 }),
        "the half-written row is the page's to leave"
    );
    assert_eq!(
        std::fs::read_to_string(h.ledger()).unwrap(),
        rows,
        "reading the ledger changed it"
    );

    assert_eq!(
        h.api(None, "ledger", b"").await.0,
        401,
        "a window with no session reads nothing"
    );
}

#[tokio::test]
async fn prices_are_asked_of_the_named_record_by_slug_alone() {
    let (record, seen) = fake_record().await;
    let h = Harness::start("prices", true, Some(record.clone())).await;
    let token = h.session().await;
    let asked = json!({ "slugs": ["claude-opus-4-8", "nope", "claude-opus-4-8", "moved", "huge"] })
        .to_string();
    let reply = h.api(Some(&token), "prices", asked.as_bytes()).await;
    assert_eq!(reply.0, 200, "{}", reply.1);
    let found = reply.json()["histories"].as_array().cloned().unwrap();
    let short: Vec<_> = found
        .iter()
        .map(|one| {
            (
                one["slug"].as_str().unwrap().to_string(),
                one["status"].as_u64(),
                one["body"].is_null(),
            )
        })
        .collect();
    assert_eq!(
        short,
        vec![
            ("claude-opus-4-8".to_string(), Some(200), false),
            ("nope".to_string(), Some(404), false),
            // Not followed: an answer is always the named origin's own.
            ("moved".to_string(), Some(302), true),
            // Past the cap, the body is not passed on.
            ("huge".to_string(), Some(200), true),
        ]
    );
    assert_eq!(found[0]["body"]["periods"][0]["input"], 5);
    assert_eq!(found[1]["body"]["error"]["code"], "unknown_sku");

    // What left the machine: one GET per slug, asked once, with no body and
    // nothing that names the person.
    let heads = seen.lock().unwrap().clone();
    let mut targets: Vec<_> = heads
        .iter()
        .map(|head| head.lines().next().unwrap().to_string())
        .collect();
    targets.sort();
    assert_eq!(
        targets,
        vec![
            "GET /api/v1/skus/claude-opus-4-8/history HTTP/1.1",
            "GET /api/v1/skus/huge/history HTTP/1.1",
            "GET /api/v1/skus/moved/history HTTP/1.1",
            "GET /api/v1/skus/nope/history HTTP/1.1",
        ]
    );
    for head in &heads {
        let lower = head.to_ascii_lowercase();
        for header in [
            "cookie:",
            "authorization:",
            "origin:",
            "referer:",
            "content-length: 1",
        ] {
            assert!(!lower.contains(header), "{header} was sent: {head}");
        }
    }
}

#[tokio::test]
async fn a_slug_that_is_not_one_asks_nothing_and_the_routes_need_their_flags() {
    let (record, seen) = fake_record().await;
    let h = Harness::start("refused", true, Some(record)).await;
    let token = h.session().await;
    for slugs in [
        json!(["../x"]),
        json!(["Claude"]),
        json!(["a/b"]),
        json!([""]),
    ] {
        let reply = h
            .api(
                Some(&token),
                "prices",
                json!({ "slugs": slugs }).to_string().as_bytes(),
            )
            .await;
        assert_eq!(reply.refused(), (400, "bad_slug".to_string()), "{slugs}");
    }
    let many: Vec<String> = (0..65).map(|n| format!("s{n}")).collect();
    let reply = h
        .api(
            Some(&token),
            "prices",
            json!({ "slugs": many }).to_string().as_bytes(),
        )
        .await;
    assert_eq!(reply.refused(), (400, "too_many_slugs".to_string()));
    let reply = h
        .api(
            Some(&token),
            "prices",
            json!({ "slugs": [], "url": "https://elsewhere" })
                .to_string()
                .as_bytes(),
        )
        .await;
    assert_eq!(reply.0, 400, "the page cannot name an origin");
    assert!(
        seen.lock().unwrap().is_empty(),
        "a refused request asked the record"
    );

    let unpriced = Harness::start("unpriced", true, None).await;
    let token = unpriced.session().await;
    assert!(unpriced.api(Some(&token), "hello", b"").await.json()["record"].is_null());
    let reply = unpriced
        .api(Some(&token), "prices", br#"{"slugs":["claude-opus-4-8"]}"#)
        .await;
    assert_eq!(reply.refused(), (404, "no_record".to_string()));
    assert_eq!(unpriced.api(Some(&token), "ledger", b"").await.0, 200);

    let plain = Harness::start("plain", false, None).await;
    let token = plain.session().await;
    let hello = plain.api(Some(&token), "hello", b"").await.json();
    assert!(hello["ledger"].is_null() && hello["record"].is_null());
    for route in ["ledger", "prices"] {
        let reply = plain.api(Some(&token), route, br#"{"slugs":[]}"#).await;
        assert_eq!(reply.refused(), (404, "no_ledger".to_string()), "{route}");
    }
}

#[tokio::test]
async fn the_server_refuses_a_record_it_would_not_ask_and_a_ledger_with_no_folder() {
    let base = base_for("bind");
    let ledger = Some(base.join("runtime").join("ledger.jsonl"));
    for record in [
        "http://example.com",
        "https://vitna.ai/api",
        "file:///etc/passwd",
    ] {
        assert!(
            bind(&base, ledger.clone(), Some(record.into()))
                .await
                .is_err(),
            "{record}"
        );
    }
    assert!(
        bind(&base, None, Some("https://vitna.ai".into()))
            .await
            .is_err(),
        "a record with no ledger"
    );
    assert!(
        bind(&base, Some(base.join("missing").join("ledger.jsonl")), None)
            .await
            .is_err()
    );
    assert!(
        bind(&base, Some(base.join("runtime")), None).await.is_err(),
        "a folder is not a ledger"
    );
    assert!(bind(&base, ledger, Some("https://vitna.ai/".into()))
        .await
        .is_ok());
    let _ = std::fs::remove_dir_all(&base);
}
