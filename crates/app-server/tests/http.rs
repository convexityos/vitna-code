//! `vitna app` over real HTTP: admission, the file API, commands and signed
//! receipts, each asked the way a browser would ask and the way an attacker
//! would. Requests are written by hand so any header can be forged.

use ed25519_dalek::SigningKey;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use vitna_app_server::{AppConfig, AppServer, Launcher};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::VitnaRunReceiptV1;
use vitna_runner::FakeRunner;

const HEAD_COMMIT: &str = "3f786850e387550fdab836ed7e6dc881de23001b";

struct Harness {
    base: PathBuf,
    port: u16,
    launcher: Launcher,
    public_key: String,
}

impl Harness {
    async fn start(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("vitna_app_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let folder = base.join("project");
        for dir in ["src", ".git/refs/heads", "ui/assets"] {
            let at = if dir.starts_with("ui") {
                base.join(dir)
            } else {
                folder.join(dir)
            };
            std::fs::create_dir_all(at).expect("make a folder");
        }
        std::fs::write(folder.join("README.md"), "# Project\n").unwrap();
        std::fs::write(folder.join("src/greet.js"), "export const hi = 1;\n").unwrap();
        std::fs::write(folder.join(".env"), "TOKEN=do-not-read\n").unwrap();
        std::fs::write(folder.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(folder.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            folder.join(".git/refs/heads/main"),
            format!("{HEAD_COMMIT}\n"),
        )
        .unwrap();
        std::fs::write(base.join("outside.txt"), "not yours\n").unwrap();
        std::fs::write(
            base.join("ui/index.html"),
            "<!doctype html><title>Vitna</title>\n",
        )
        .unwrap();
        std::fs::write(base.join("ui/assets/app.js"), "console.log(1);\n").unwrap();

        let runner = FakeRunner::new(base.join("runner.journal")).expect("fake runner");
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let public_key = hex::encode(key.verifying_key().to_bytes());
        let server = AppServer::bind(
            AppConfig {
                folder: folder.clone(),
                port: 0,
                ui: Some(base.join("ui")),
                allow_unsandboxed: false,
            },
            Arc::new(Mutex::new(runner)),
            key,
        )
        .await
        .expect("bind");
        let port = server.port();
        let launcher = server.launcher();
        tokio::spawn(server.serve());
        Self {
            base,
            port,
            launcher,
            public_key,
        }
    }

    fn folder(&self) -> PathBuf {
        self.base.join("project")
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// A session, as the page makes one from the address it was opened at.
    async fn session(&self) -> String {
        let reply = self.exchange(&self.launcher.url()).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        reply.json()["token"].as_str().expect("a token").to_string()
    }

    async fn exchange(&self, url: &str) -> Reply {
        let code = url.split_once("#launch=").expect("a launch code").1;
        let body = serde_json::json!({ "launch": code }).to_string();
        self.api(None, "session", "", body.as_bytes()).await
    }

    async fn api(&self, token: Option<&str>, name: &str, query: &str, body: &[u8]) -> Reply {
        let origin = self.origin();
        let bearer = token.map(|t| format!("Bearer {t}"));
        let mut headers = vec![
            ("Origin", origin.as_str()),
            ("Sec-Fetch-Site", "same-origin"),
        ];
        if let Some(bearer) = bearer.as_deref() {
            headers.push(("Authorization", bearer));
        }
        let target = if query.is_empty() {
            format!("/api/v1/{name}")
        } else {
            format!("/api/v1/{name}?{query}")
        };
        send(self.port, "POST", &target, &headers, body).await
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text()))
    }
    fn error(&self) -> String {
        self.json()["error"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// One HTTP/1.1 request, written by hand. A Host header is added unless one
/// is given.
async fn send(
    port: u16,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let mut request = format!("{method} {target} HTTP/1.1\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("connection"))
    {
        request.push_str("Connection: close\r\n");
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send the head");
    stream.write_all(body).await.expect("send the body");
    let mut raw = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream.read_to_end(&mut raw),
    )
    .await
    .expect("the server left the connection open")
    .expect("read the reply");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a complete head");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("a status");
    let headers = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    Reply {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn a_launch_code_opens_one_session_once() {
    let h = Harness::start("launch").await;
    let url = h.launcher.url();
    assert!(
        url.starts_with(&format!("http://127.0.0.1:{}/#launch=", h.port)),
        "{url}"
    );

    let first = h.exchange(&url).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let hello = first.json();
    let token = hello["token"].as_str().expect("a token");
    assert_eq!(token.len(), 64);
    assert_eq!(hello["folder"], "project");
    assert_eq!(hello["commands"]["state"], "no_process");
    assert_eq!(hello["public_key"], h.public_key.as_str());

    let again = h.exchange(&url).await;
    assert_eq!(
        (again.status, again.error().as_str()),
        (403, "launch_code"),
        "a code worked twice"
    );
    let made_up = h.exchange("x#launch=00").await;
    assert_eq!(made_up.status, 403);

    assert_eq!(h.api(Some(token), "hello", "", b"").await.status, 200);
    assert_eq!(h.api(None, "hello", "", b"").await.status, 401);
    assert_eq!(
        h.api(Some("not-a-token"), "hello", "", b"").await.status,
        401
    );
}

#[tokio::test]
async fn requests_from_anywhere_else_are_refused() {
    let h = Harness::start("admission").await;
    let token = h.session().await;
    let bearer = format!("Bearer {token}");
    let origin = h.origin();
    let port = h.port;

    // DNS rebinding: a page on another name that resolves to 127.0.0.1.
    let host = format!("attacker.example:{port}");
    let rebound = send(
        port,
        "POST",
        "/api/v1/hello",
        &[
            ("Host", &host),
            ("Origin", &format!("http://{host}")),
            ("Authorization", &bearer),
        ],
        b"",
    )
    .await;
    assert_eq!(rebound.status, 421, "{}", rebound.text());
    let page = send(port, "GET", "/", &[("Host", &host)], b"").await;
    assert_eq!(
        page.status, 421,
        "the interface itself was served to another name"
    );

    // Another origin, with a stolen token even.
    let cross = send(
        port,
        "POST",
        "/api/v1/hello",
        &[
            ("Origin", "https://evil.example"),
            ("Authorization", &bearer),
        ],
        b"",
    )
    .await;
    assert_eq!(
        (cross.status, cross.error().as_str()),
        (403, "wrong_origin")
    );
    let no_origin = send(
        port,
        "POST",
        "/api/v1/hello",
        &[("Authorization", &bearer)],
        b"",
    )
    .await;
    assert_eq!(no_origin.status, 403);
    let other_name = send(
        port,
        "POST",
        "/api/v1/hello",
        &[
            ("Origin", &format!("http://localhost:{port}")),
            ("Authorization", &bearer),
        ],
        b"",
    )
    .await;
    assert_eq!(
        other_name.status, 403,
        "localhost is another origin from 127.0.0.1"
    );
    let cross_site = send(
        port,
        "POST",
        "/api/v1/hello",
        &[
            ("Origin", &origin),
            ("Sec-Fetch-Site", "cross-site"),
            ("Authorization", &bearer),
        ],
        b"",
    )
    .await;
    assert_eq!(cross_site.status, 403);

    // The API takes POST only, and sends no CORS header to anyone.
    let get = send(
        port,
        "GET",
        "/api/v1/hello",
        &[("Origin", &origin), ("Authorization", &bearer)],
        b"",
    )
    .await;
    assert_eq!(get.status, 405);
    let preflight = send(
        port,
        "OPTIONS",
        "/api/v1/hello",
        &[
            ("Origin", "https://evil.example"),
            ("Access-Control-Request-Method", "POST"),
        ],
        b"",
    )
    .await;
    assert_ne!(preflight.status, 200);
    assert!(preflight.header("access-control-allow-origin").is_none());
    let ok = h.api(Some(&token), "hello", "", b"").await;
    assert!(ok.header("access-control-allow-origin").is_none());

    // No WebSocket, whatever is asked.
    let upgrade = send(
        port,
        "GET",
        "/",
        &[
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade, close"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ("Sec-WebSocket-Version", "13"),
        ],
        b"",
    )
    .await;
    assert_ne!(upgrade.status, 101);
}

#[tokio::test]
async fn files_are_read_and_written_in_place_only_over_what_was_read() {
    let h = Harness::start("files").await;
    let token = h.session().await;
    let t = Some(token.as_str());

    let listed = h.api(t, "fs/list", "path=", b"").await;
    assert_eq!(listed.status, 200, "{}", listed.text());
    let entries = listed.json()["entries"].as_array().cloned().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [".git", "src", ".env", "README.md"],
        "folders first, then files, each by name"
    );
    assert_eq!(entries[3]["size"], 10);

    let read = h.api(t, "fs/read", "path=README.md", b"").await;
    assert_eq!(read.body, b"# Project\n");
    let held = sha256(b"# Project\n");
    assert_eq!(read.header("x-vitna-sha256"), Some(held.as_str()));

    let stale = h
        .api(
            t,
            "fs/write",
            &format!("path=README.md&expect={}", sha256(b"old")),
            b"# Mine\n",
        )
        .await;
    assert_eq!((stale.status, stale.error().as_str()), (409, "changed"));
    assert_eq!(stale.json()["current"], held.as_str());
    assert_eq!(
        std::fs::read(h.folder().join("README.md")).unwrap(),
        b"# Project\n",
        "a stale write changed the file"
    );

    let written = h
        .api(
            t,
            "fs/write",
            &format!("path=README.md&expect={held}"),
            b"# Mine\n",
        )
        .await;
    assert_eq!(written.status, 200, "{}", written.text());
    assert_eq!(
        written.json()["postimage_hash"],
        sha256(b"# Mine\n").as_str()
    );
    assert_eq!(
        std::fs::read(h.folder().join("README.md")).unwrap(),
        b"# Mine\n"
    );

    let made = h
        .api(
            t,
            "fs/write",
            "path=notes%2FNEW+FILE.md&expect=absent",
            b"new\n",
        )
        .await;
    assert_eq!(made.status, 200, "{}", made.text());
    assert_eq!(
        std::fs::read(h.folder().join("notes").join("NEW FILE.md")).unwrap(),
        b"new\n"
    );
    let twice = h
        .api(
            t,
            "fs/write",
            "path=notes/NEW+FILE.md&expect=absent",
            b"again\n",
        )
        .await;
    assert_eq!(twice.status, 409, "a file that exists was made again");

    let stat = h.api(t, "fs/stat", "path=notes", b"").await;
    assert_eq!(stat.json()["kind"], "directory");
    let removed = h
        .api(
            t,
            "fs/remove",
            &format!("path=notes/NEW+FILE.md&expect={}", sha256(b"new\n")),
            b"",
        )
        .await;
    assert_eq!(removed.status, 200, "{}", removed.text());
    assert!(!h.folder().join("notes").join("NEW FILE.md").exists());
    let missing = h.api(t, "fs/read", "path=notes/NEW+FILE.md", b"").await;
    assert_eq!((missing.status, missing.error().as_str()), (404, "missing"));
    let no_expect = h.api(t, "fs/write", "path=README.md", b"x").await;
    assert_eq!(no_expect.status, 400, "a write that names no preimage");
}

#[tokio::test]
async fn the_server_refuses_what_the_page_refuses_however_it_is_named() {
    let h = Harness::start("guards").await;
    let token = h.session().await;
    let t = Some(token.as_str());

    for (query, status) in [
        ("path=.git/config", 403),
        ("path=.GIT/config", 403),
        ("path=.env", 403),
        ("path=.vitna/receipts/x.json", 403),
        ("path=../outside.txt", 400),
        ("path=/etc/hosts", 400),
        ("path=C:%5CWindows%5Cwin.ini", 400),
        ("path=.git./config", 400),
    ] {
        let reply = h.api(t, "fs/read", query, b"").await;
        assert_eq!(reply.status, status, "{query}: {}", reply.text());
    }
    assert_eq!(h.api(t, "fs/list", "path=.git", b"").await.status, 403);
    assert_eq!(
        h.api(
            t,
            "fs/write",
            "path=.vitna/receipts/forged.json&expect=absent",
            b"{}"
        )
        .await
        .status,
        403
    );
    assert!(
        !h.folder().join(".vitna").exists(),
        "the page wrote where receipts go"
    );
    let hook = h
        .api(
            t,
            "fs/write",
            "path=.git/hooks/pre-commit&expect=absent",
            b"#!/bin/sh\n",
        )
        .await;
    assert_eq!(hook.status, 403);
    assert!(!h.folder().join(".git").join("hooks").exists());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".git/config", h.folder().join("link.txt")).unwrap();
        let through = h.api(t, "fs/read", "path=link.txt", b"").await;
        assert_eq!(
            through.status,
            403,
            "a link reached .git: {}",
            through.text()
        );
        assert!(through.text().contains(".git/config"), "{}", through.text());
    }
}

#[tokio::test]
async fn a_session_s_receipt_is_signed_and_names_what_it_did() {
    let h = Harness::start("receipt").await;
    let token = h.session().await;
    let t = Some(token.as_str());

    let ran = h
        .api(t, "run", "", br#"{"command":"npm test","directory":"src"}"#)
        .await;
    assert_eq!(ran.status, 200, "{}", ran.text());
    let ran = ran.json();
    assert_eq!(ran["exit_code"], 0);
    assert_eq!(ran["sandbox_backend"], "no_process");
    let action_id = ran["action_id"].as_str().unwrap().to_string();
    assert!(action_id.starts_with("fake-act-"), "{action_id}");

    let held = sha256(b"export const hi = 1;\n");
    let written = h
        .api(
            t,
            "fs/write",
            &format!("path=src/greet.js&expect={held}"),
            b"export const hi = 2;\n",
        )
        .await;
    assert_eq!(written.status, 200);
    let refused = h
        .api(t, "run", "", br#"{"command":"ls","directory":".git"}"#)
        .await;
    assert_eq!(refused.status, 403, "a command ran inside .git");

    let made = h
        .api(
            t,
            "receipt",
            "",
            br#"{"provider":"openrouter","model":"some/model","summary":"Changed the greeting."}"#,
        )
        .await;
    assert_eq!(made.status, 200, "{}", made.text());
    let made = made.json();
    let path = made["path"].as_str().unwrap();
    assert!(
        path.starts_with(".vitna/receipts/app-") && path.ends_with(".json"),
        "{path}"
    );

    let on_disk = std::fs::read_to_string(h.folder().join(path)).expect("the receipt file");
    let receipt: VitnaRunReceiptV1 = serde_json::from_str(&on_disk).expect("a receipt");
    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&h.public_key)).expect("verify");
    assert!(
        report.is_valid && report.signature_verified,
        "{:?}",
        report.errors
    );
    assert_eq!(receipt.completion_state, "completed_with_unknowns");
    assert_eq!(receipt.base_commit_sha, HEAD_COMMIT);
    assert_eq!(receipt.model_selection.model_sku, "some/model");
    assert_eq!(receipt.model_selection.routing_reason, "reported_by_page");
    assert_eq!(receipt.changeset.files_modified.len(), 1);
    assert_eq!(receipt.changeset.files_modified[0].path, "src/greet.js");
    assert_eq!(receipt.changeset.files_modified[0].preimage_hash, held);
    assert_eq!(
        receipt.changeset.files_modified[0].postimage_hash,
        sha256(b"export const hi = 2;\n")
    );
    assert_eq!(receipt.runner_execution_statements.len(), 1);
    assert_eq!(receipt.runner_execution_statements[0].action_id, action_id);
    assert_eq!(
        receipt.runner_execution_statements[0].statement_digest,
        ran["statement_digest"].as_str().unwrap()
    );
    assert!(receipt
        .evidence_items
        .iter()
        .any(|e| e.grade == "model_reported"));

    // The page cannot read the receipt folder, nor forge a better ending.
    assert_eq!(
        h.api(t, "fs/read", &format!("path={path}"), b"")
            .await
            .status,
        403
    );
    let bold = h
        .api(
            t,
            "receipt",
            "",
            br#"{"completion":"completed_with_evidence"}"#,
        )
        .await;
    assert_eq!(bold.status, 400);

    // The journal starts again after each receipt.
    let next = h
        .api(t, "receipt", "", br#"{"completion":"cancelled"}"#)
        .await
        .json();
    assert_eq!(
        next["receipt"]["changeset"]["files_modified"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(next["receipt"]["completion_state"], "cancelled");
}

#[tokio::test]
async fn the_interface_is_served_and_cannot_be_framed() {
    let h = Harness::start("ui").await;
    let port = h.port;

    let page = send(port, "GET", "/", &[], b"").await;
    assert_eq!(page.status, 200);
    assert!(page.text().contains("<title>Vitna</title>"));
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    assert_eq!(
        page.header("content-security-policy"),
        Some("frame-ancestors 'none'")
    );
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );

    let script = send(port, "GET", "/assets/app.js", &[], b"").await;
    assert_eq!(
        (script.status, script.header("content-type")),
        (200, Some("text/javascript; charset=utf-8"))
    );
    let route = send(port, "GET", "/code", &[], b"").await;
    assert!(
        route.text().contains("<title>Vitna</title>"),
        "a route of the page gets the page"
    );
    assert_eq!(
        send(port, "GET", "/assets/missing.js", &[], b"")
            .await
            .status,
        404
    );
    assert_eq!(
        send(port, "GET", "/../project/README.md", &[], b"")
            .await
            .status,
        404
    );
    assert_eq!(send(port, "POST", "/", &[], b"").await.status, 405);
    let head = send(port, "HEAD", "/", &[], b"").await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
}
