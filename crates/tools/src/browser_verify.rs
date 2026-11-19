//! Digesting a document this process actually read.
//!
//! This tool used to answer for `http://` and `https://` targets by writing a
//! short page that announced the app had loaded, hashing it, and reporting
//! the hash as captured evidence. No request was made. So an `expected_text`
//! matching that sentence passed for any URL, including one that resolves to
//! nothing, and any other expected text failed however the real page
//! rendered. A comment called it "offline/hermetic test mode", but there was
//! no mode to leave. The sentence itself is quoted only in this file's tests,
//! where one of them checks that the code above them holds no such page.
//!
//! Remote targets are refused here rather than fetched, because fetching one
//! honestly needs authority this tool does not have. Every network call in
//! this workspace is either a model provider call, which the receipt
//! discloses as `model_call`, or a subprocess under `vitna-sandbox`, which
//! denies the network unless `allow_network` was granted. A fetch from inside
//! this tool would be neither: it would run in the host process, outside any
//! sandbox, and the receipt has nowhere to record that it happened. The
//! inventory promises an `egress_call` event for exactly this, and nothing
//! implements one. Refusing keeps the gap visible; fetching would trade
//! invented evidence for undisclosed egress, which is the same contract
//! broken more quietly.
//!
//! What is left is honest: a regular file inside the workspace, read through
//! the containment in `workspace_fs`, digested as the bytes on disk. Those
//! bytes are not a rendered DOM, and the report says so, because this tool's
//! output is shaped to land in a receipt as captured evidence.

use crate::egress::{EgressBroker, EgressPolicy, NETWORK_DESTINATION_DENIED};
use crate::workspace_fs::{read_up_to, sha256_hex, Resolved, Workspace, MAX_FILE_BYTES};
use crate::{remote_url_scheme, Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;

pub struct BrowserVerifyTool;

/// The scheme of a remote target, when `target` names one.
fn remote_scheme(target: &str) -> Option<&str> {
    remote_url_scheme(target)
}

/// True when `haystack` contains `needle`. Compared as bytes, so the answer
/// is about what the file holds rather than about a lossy rendering of it.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[async_trait]
impl Tool for BrowserVerifyTool {
    fn name(&self) -> &str {
        "browser_verify"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "browser_verify".to_string(),
            description: "Digest a workspace document or an operator-allowlisted HTTP/HTTPS URL through the egress broker and check it for expected text. Local files are read on disk with no renderer; remote URLs require explicit egress allowlist + exact-action approval and report raw brokered HTTP response bytes (no JS renderer runs).".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "url_or_path": {
                        "type": "string",
                        "description": "Path to a document within the workspace, or an HTTP/HTTPS URL on an operator-allowlisted host."
                    },
                    "expected_text": {
                        "type": "string",
                        "description": "Optional substring expected in the document's bytes."
                    }
                },
                "required": ["url_or_path"]
            }),
            // Local workspace file verification is read-only. When invoked with
            // a remote URL, OrchestrationEngine gates execution behind
            // exact-action approval and EgressBroker enforces the host allowlist.
            is_mutating: false,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let target = args
            .get("url_or_path")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "Missing required 'url_or_path' parameter".to_string())?;

        let expected_text = args.get("expected_text").and_then(|e| e.as_str());

        // Remote targets are refused fail-closed when no egress allowlist is
        // granted, and otherwise fetched strictly through `EgressBroker` with
        // full `EgressCallRecord` emission.
        if let Some(scheme) = remote_scheme(target) {
            if ctx.egress_allowlist.is_empty() {
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: browser_verify cannot retrieve '{}': no egress \
                     allowlist is configured so it performs no network I/O, has no '{}' content \
                     to digest, and reports none. Pass --allow-egress <host> (with exact-action \
                     approval) or save the document into the workspace and verify that file.",
                    target, scheme
                ));
            }

            let policy = EgressPolicy::with_allowlist(ctx.egress_allowlist.clone());
            let fetched = EgressBroker::fetch(&policy, "GET", target).await?;
            let status_ok = (200..400).contains(&fetched.status_code);
            let text_matched = match expected_text {
                Some(exp) => contains_bytes(&fetched.body, exp.as_bytes()),
                None => true,
            };
            let success = status_ok && text_matched;

            let mut output = format!(
                "Brokered HTTP Verification Report for '{}'\n",
                fetched.record.redacted_url
            );
            output.push_str(&format!("HTTP Status: {}\n", fetched.status_code));
            output.push_str(&format!("SHA-256: {}\n", fetched.record.response_sha256));
            output.push_str(&format!("Content Length: {} bytes\n", fetched.body.len()));
            output.push_str(
                "Captured: brokered HTTP response bytes via EgressBroker. No JS browser renderer ran, so this is the raw HTTP response payload and not a rendered DOM.\n",
            );

            if !status_ok {
                output.push_str(&format!(
                    "HTTP Assertion: FAIL (non-success status {})\n",
                    fetched.status_code
                ));
            }
            if let Some(exp) = expected_text {
                if text_matched {
                    output.push_str(&format!(
                        "Verification Assertion: PASS (found expected text '{}')\n",
                        exp
                    ));
                } else {
                    output.push_str(&format!(
                        "Verification Assertion: FAIL (missing expected text '{}')\n",
                        exp
                    ));
                }
            }

            return Ok(ToolResult {
                call_id: String::new(),
                tool_name: "browser_verify".to_string(),
                success,
                output,
                preimage_hash: None,
                postimage_hash: Some(fetched.record.response_sha256.clone()),
                diff: None,
                exit_code: None,
                sandbox_backend: None,
                sandbox_enforcement: None,
                egress_calls: vec![fetched.record],
            });
        }

        // Local workspace file path: resolved on the real filesystem, links
        // included, and only a regular file is opened.
        let workspace = Workspace::new(&ctx.workspace_root)?;
        let real_path = match workspace.resolve(target)? {
            Resolved::Existing(real_path) => real_path,
            Resolved::Missing(_) => return Err(format!("Document does not exist: {}", target)),
        };
        let relative = workspace.relative(&real_path);
        let mut file = workspace.open_regular(&real_path, target)?;

        let (content, longer) = read_up_to(&mut file, MAX_FILE_BYTES)
            .map_err(|e| format!("Failed to read document {}: {}", target, e))?;

        // A digest over part of a document would read as the document's, and
        // an assertion over part of it could miss text further in. Neither is
        // worth reporting, so nothing is.
        if longer {
            return Err(format!(
                "'{}' is larger than the {} bytes browser_verify reads, so neither its digest \
                 nor an assertion over it would describe the whole document.",
                target, MAX_FILE_BYTES
            ));
        }

        let content_hash = sha256_hex(&content);

        let text_matched = match expected_text {
            Some(exp) => contains_bytes(&content, exp.as_bytes()),
            None => true,
        };

        let mut output = format!("Document Verification Report for '{}'\n", relative);
        output.push_str(&format!("SHA-256: {}\n", content_hash));
        output.push_str(&format!("Content Length: {} bytes\n", content.len()));
        output.push_str("Captured: bytes on disk, read by this process. No renderer ran, so this is the document as stored and not a rendered DOM.\n");

        if let Some(exp) = expected_text {
            if text_matched {
                output.push_str(&format!("Verification Assertion: PASS (found expected text '{}')\n", exp));
            } else {
                output.push_str(&format!("Verification Assertion: FAIL (missing expected text '{}')\n", exp));
            }
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "browser_verify".to_string(),
            success: text_matched,
            output,
            preimage_hash: None,
            postimage_hash: Some(content_hash),
            diff: None,
            // No process ran. An exit code here would read in the receipt as
            // one that did.
            exit_code: None,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dir_link, file_link, Scratch};
    use std::fs;

    const SYNTHETIC: &str = "App Loaded";

    async fn verify(s: &Scratch, args: serde_json::Value) -> Result<ToolResult, String> {
        BrowserVerifyTool.execute(args, &s.ctx()).await
    }

    /// The regression this tool shipped with: it answered for a URL nothing
    /// served, and the answer was PASS over a string it had just written.
    /// Any implementation may refuse or may really fetch; none may report a
    /// match against bytes no source produced. Port 9 is discard, so nothing
    /// answers HTTP there on any machine.
    #[tokio::test]
    async fn test_expected_text_cannot_pass_for_a_url_nothing_served() {
        let s = Scratch::new("bv_nothing_served");
        let res = verify(
            &s,
            json!({ "url_or_path": "http://127.0.0.1:9/", "expected_text": SYNTHETIC }),
        )
        .await;

        match res {
            Err(message) => assert!(
                !message.contains(SYNTHETIC),
                "a refusal must not quote content it invented: {}",
                message
            ),
            Ok(result) => {
                assert!(
                    !result.success,
                    "nothing served this URL, so nothing can have matched: {}",
                    result.output
                );
                assert!(
                    !result.output.contains("PASS"),
                    "reported a passing assertion over content nothing served: {}",
                    result.output
                );
            }
        }
    }

    /// The other half of the same defect: a digest was emitted for a URL, and
    /// it was the digest of a string this process wrote. A hash may only
    /// describe bytes that came from somewhere.
    #[tokio::test]
    async fn test_a_url_yields_no_digest_of_invented_content() {
        let s = Scratch::new("bv_no_invented_digest");
        let res = verify(&s, json!({ "url_or_path": "https://example.invalid/app" })).await;

        match res {
            Err(message) => {
                assert!(
                    message.contains("no network I/O"),
                    "the refusal must name why it cannot answer: {}",
                    message
                );
                assert!(
                    !message.contains("SHA-256") && !message.contains("Digest"),
                    "a refusal must carry no digest: {}",
                    message
                );
            }
            Ok(result) => assert!(
                result.postimage_hash.is_none(),
                "a digest was reported for a URL, so it must be of bytes something served"
            ),
        }
    }

    /// Absent `expected_text` the old code reported success unconditionally,
    /// which is the same fabrication with nothing to assert against.
    #[tokio::test]
    async fn test_a_url_does_not_succeed_without_an_assertion() {
        let s = Scratch::new("bv_url_bare");
        let res = verify(&s, json!({ "url_or_path": "http://127.0.0.1:9/" })).await;
        if let Ok(result) = res {
            assert!(!result.success, "nothing served this URL: {}", result.output);
        }
    }

    /// The scheme test is about reaching off this machine, not about the two
    /// spellings the old branch happened to check.
    #[tokio::test]
    async fn test_every_remote_scheme_is_refused() {
        let s = Scratch::new("bv_schemes");
        for target in [
            "http://localhost:3000/",
            "https://example.invalid/",
            "file:///etc/passwd",
            "ftp://example.invalid/app.html",
            "HTTP://EXAMPLE.INVALID/",
        ] {
            let res = verify(&s, json!({ "url_or_path": target, "expected_text": SYNTHETIC })).await;
            let message = res.expect_err(&format!("'{}' must be refused", target));
            assert!(
                !message.contains(SYNTHETIC),
                "'{}' produced invented content: {}",
                target,
                message
            );
        }
    }

    /// A drive letter is a path. `C:` must not read as a URL scheme, or the
    /// refusal would swallow an absolute Windows path and call it egress.
    #[test]
    fn test_a_drive_letter_is_not_a_scheme() {
        assert_eq!(remote_scheme("http://x/"), Some("http"));
        assert_eq!(remote_scheme("ftp://x/"), Some("ftp"));
        assert_eq!(remote_scheme("C://weird/path"), None);
        assert_eq!(remote_scheme("c:/src/index.html"), None);
        assert_eq!(remote_scheme("src/index.html"), None);
        assert_eq!(remote_scheme("./a:b//c"), None);
    }

    #[tokio::test]
    async fn test_reads_a_real_file_and_digests_its_actual_bytes() {
        let s = Scratch::new("bv_local_read");
        let body = "<html><body><h1>Dashboard</h1></body></html>";
        fs::write(s.ws().join("index.html"), body).unwrap();

        let res = verify(
            &s,
            json!({ "url_or_path": "index.html", "expected_text": "Dashboard" }),
        )
        .await
        .expect("read");

        assert!(res.success);
        assert!(res.output.contains("PASS"), "{}", res.output);
        // The digest is of what is on disk, not of anything this tool composed.
        let on_disk = sha256_hex(body.as_bytes());
        assert_eq!(res.postimage_hash.as_deref(), Some(on_disk.as_str()));
        assert!(res.output.contains(&on_disk), "{}", res.output);
        assert!(res.exit_code.is_none(), "no process ran, so no exit code");
    }

    #[tokio::test]
    async fn test_missing_expected_text_fails_against_a_real_file() {
        let s = Scratch::new("bv_local_fail");
        fs::write(s.ws().join("index.html"), "<html>nothing here</html>").unwrap();

        let res = verify(
            &s,
            json!({ "url_or_path": "index.html", "expected_text": SYNTHETIC }),
        )
        .await
        .expect("read");

        assert!(!res.success);
        assert!(res.output.contains("FAIL"), "{}", res.output);
        // The digest still describes the file, which is the point of it.
        assert_eq!(
            res.postimage_hash,
            Some(sha256_hex(b"<html>nothing here</html>"))
        );
    }

    #[tokio::test]
    async fn test_a_missing_document_is_not_invented() {
        let s = Scratch::new("bv_local_missing");
        let err = verify(
            &s,
            json!({ "url_or_path": "nope.html", "expected_text": SYNTHETIC }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("does not exist"), "{}", err);
        assert!(!err.contains(SYNTHETIC), "{}", err);
    }

    #[tokio::test]
    async fn test_traversal_out_of_the_workspace_is_denied() {
        let s = Scratch::new("bv_traversal");
        let err = verify(&s, json!({ "url_or_path": "../outside/secret.txt" }))
            .await
            .unwrap_err();
        assert!(err.contains("escapes workspace root"), "{}", err);
        assert!(!err.contains("TOP SECRET"), "{}", err);
    }

    #[tokio::test]
    async fn test_an_absolute_path_out_of_the_workspace_is_denied() {
        let s = Scratch::new("bv_absolute");
        let outside = s.outside().join("secret.txt");
        let err = verify(&s, json!({ "url_or_path": outside.to_string_lossy() }))
            .await
            .unwrap_err();
        assert!(err.contains("escapes workspace root"), "{}", err);
        assert!(!err.contains("TOP SECRET"), "{}", err);
    }

    #[tokio::test]
    async fn test_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("bv_link_out");
        if !file_link(&s.outside().join("secret.txt"), &s.ws().join("page.html")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let err = verify(&s, json!({ "url_or_path": "page.html" })).await.unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
        assert!(!err.contains("TOP SECRET"), "{}", err);
    }

    #[tokio::test]
    async fn test_directory_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("bv_dir_link_out");
        if !dir_link(&s.outside(), &s.ws().join("pages")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let err = verify(&s, json!({ "url_or_path": "pages/secret.txt" }))
            .await
            .unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
    }

    #[tokio::test]
    async fn test_a_directory_is_refused() {
        let s = Scratch::new("bv_directory");
        fs::create_dir_all(s.ws().join("site")).unwrap();
        let err = verify(&s, json!({ "url_or_path": "site" })).await.unwrap_err();
        assert!(err.contains("not a regular file"), "{}", err);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_fifo_is_refused_without_blocking() {
        let s = Scratch::new("bv_fifo");
        if !crate::test_support::mkfifo(&s.ws().join("pipe")) {
            eprintln!("skipped: mkfifo is unavailable");
            return;
        }
        let err = crate::test_support::execute_bounded(
            BrowserVerifyTool,
            json!({ "url_or_path": "pipe" }),
            s.ctx(),
        )
        .unwrap_err();
        assert!(err.contains("a FIFO"), "{}", err);
    }

    /// The tool must hold no copy of a document to fall back on. A sweep of
    /// its own source is the only check that stays true for code nobody has
    /// written yet, which is what the "hermetic test mode" comment cost.
    #[test]
    fn test_the_source_carries_no_stand_in_document() {
        let source = include_str!("browser_verify.rs");
        let body = source
            .split("#[cfg(test)]")
            .next()
            .expect("source has a non-test half");
        assert!(
            !body.contains("<html"),
            "browser_verify must not carry markup to answer with"
        );
        assert!(
            !body.contains(SYNTHETIC),
            "browser_verify must not carry a stand-in document"
        );
    }

    async fn serve_once(status_line: &str, body: &str) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().unwrap().port();
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://127.0.0.1:{port}/index?token=secret123"), handle)
    }

    #[tokio::test]
    async fn test_allowlisted_loopback_http_verifies_real_bytes_and_emits_egress_record() {
        let s = Scratch::new("bv_loopback_ok");
        let page = "<html><body><h1>Live Preview Ready</h1></body></html>";
        let (url, server) = serve_once("200 OK", page).await;

        let mut ctx = s.ctx();
        ctx.egress_allowlist = vec!["127.0.0.1".to_string()];

        let res = BrowserVerifyTool
            .execute(
                json!({ "url_or_path": url, "expected_text": "Live Preview Ready" }),
                &ctx,
            )
            .await
            .expect("allowlisted loopback verify succeeds");
        server.await.unwrap();

        assert!(res.success, "output: {}", res.output);
        assert!(res.output.contains("PASS"));
        assert!(res.output.contains("No JS browser renderer ran"));
        let expected_hash = sha256_hex(page.as_bytes());
        assert_eq!(res.postimage_hash.as_deref(), Some(expected_hash.as_str()));
        assert_eq!(res.egress_calls.len(), 1);
        assert_eq!(res.egress_calls[0].target_domain, "127.0.0.1");
        assert_eq!(res.egress_calls[0].status_code, 200);
        assert_eq!(res.egress_calls[0].response_sha256, expected_hash);
        assert!(
            !res.egress_calls[0].redacted_url.contains("secret123"),
            "query string must be stripped from redacted_url"
        );
    }

    #[tokio::test]
    async fn test_allowlisted_loopback_http_fails_on_500_or_missing_text() {
        let s = Scratch::new("bv_loopback_500");
        let page = "<html><body><h1>Internal Error</h1></body></html>";
        let (url, server) = serve_once("500 Internal Server Error", page).await;

        let mut ctx = s.ctx();
        ctx.egress_allowlist = vec!["127.0.0.1".to_string()];

        let res = BrowserVerifyTool
            .execute(
                json!({ "url_or_path": url, "expected_text": "Internal Error" }),
                &ctx,
            )
            .await
            .expect("returns report for 500");
        server.await.unwrap();

        assert!(!res.success, "HTTP 500 must fail verification");
        assert_eq!(res.egress_calls.len(), 1);
        assert_eq!(res.egress_calls[0].status_code, 500);
    }

    #[tokio::test]
    async fn test_non_allowlisted_host_is_denied_even_when_other_host_is_allowlisted() {
        let s = Scratch::new("bv_other_host_denied");
        let mut ctx = s.ctx();
        ctx.egress_allowlist = vec!["127.0.0.1".to_string()];

        let err = BrowserVerifyTool
            .execute(
                json!({ "url_or_path": "https://example.invalid/app" }),
                &ctx,
            )
            .await
            .expect_err("non-allowlisted host must fail closed");
        assert!(err.contains(NETWORK_DESTINATION_DENIED), "err={err}");
    }
}
