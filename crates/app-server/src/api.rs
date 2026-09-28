//! Every request, admitted or refused, and the API behind the admission.
//!
//! The API is `POST /api/v1/<name>`, with its parameters in the query string
//! and a body only when there is content: a file's bytes for a write, JSON for
//! a command or a receipt. Every refusal is JSON, `{"error", "message"}`, and
//! its message is a sentence the page can show as it is.

// A refusal is returned early as the finished reply, once per request at
// most, so there is nothing to gain from boxing it to shrink the Result.
#![allow(clippy::result_large_err)]

use crate::admission;
use crate::journal::{self, Effect, Ran};
use crate::paths;
use crate::ui::{self, Reply};
use crate::{Commands, Session, State};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::{header, Method, Request, Response, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use vitna_runner::{CommandRequest, SANDBOX_UNAVAILABLE};
use vitna_tools::workspace_files::{EntryKind, FileError, ABSENT_PREIMAGE_HASH, MAX_FILE_BYTES};

/// The most a JSON body may hold.
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// The most of each output stream sent back to the page. The runner keeps it
/// all, and the statement digest covers all of it.
const OUTPUT_CAP: usize = 256 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_COMMAND_CHARS: usize = 8 * 1024;
const MAX_REPORTED_CHARS: usize = 200;
const MAX_SUMMARY_CHARS: usize = 4000;

fn with_api_headers(mut response: Reply) -> Reply {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        header::HeaderValue::from_static("same-origin"),
    );
    response
}

fn json_reply(status: StatusCode, value: Value) -> Reply {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    with_api_headers(response)
}

fn refusal(status: StatusCode, error: &str, message: impl Into<String>) -> Reply {
    json_reply(status, json!({ "error": error, "message": message.into() }))
}

fn file_refusal(error: FileError) -> Reply {
    match error {
        FileError::Missing => refusal(
            StatusCode::NOT_FOUND,
            "missing",
            "There is nothing at that path.",
        ),
        FileError::Refused(reason) => refusal(StatusCode::FORBIDDEN, "refused", reason),
        FileError::TooLarge(size) => refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!("The file is {size} bytes, over the {MAX_FILE_BYTES} read or written at once."),
        ),
        FileError::Changed { current } => json_reply(
            StatusCode::CONFLICT,
            json!({
                "error": "changed",
                "message": "The file changed on disk since it was read, so it was left alone.",
                "current": current,
            }),
        ),
        FileError::Failed(reason) => refusal(StatusCode::INTERNAL_SERVER_ERROR, "failed", reason),
    }
}

/// The first value of each query parameter.
fn query(request: &Request<Incoming>) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for (key, value) in form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()) {
        params
            .entry(key.into_owned())
            .or_insert_with(|| value.into_owned());
    }
    params
}

async fn body(request: Request<Incoming>, limit: usize) -> Result<Bytes, Reply> {
    match Limited::new(request.into_body(), limit).collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => Err(refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            format!("The request is over the {limit} bytes accepted."),
        )),
        Err(_) => Err(refusal(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "The request body could not be read.",
        )),
    }
}

async fn json_body<T: for<'de> Deserialize<'de>>(request: Request<Incoming>) -> Result<T, Reply> {
    let bytes = body(request, MAX_JSON_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(|e| {
        refusal(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("The request is not what was expected: {e}"),
        )
    })
}

fn header_text(request: &Request<Incoming>, name: header::HeaderName) -> Option<&str> {
    request.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Answers one request. Admission runs first, in the order ADR-0006 gives.
pub(crate) async fn handle(state: &State, peer: SocketAddr, request: Request<Incoming>) -> Reply {
    if !admission::peer_is_local(peer.ip()) {
        return refusal(
            StatusCode::FORBIDDEN,
            "not_local",
            "Only this machine may connect.",
        );
    }
    let host = header_text(&request, header::HOST)
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    let Some(host) = admission::our_host(host, state.port) else {
        return refusal(
            StatusCode::MISDIRECTED_REQUEST,
            "wrong_host",
            "This server answers only to 127.0.0.1 and localhost.",
        );
    };
    let path = request.uri().path().to_string();
    let Some(name) = path.strip_prefix("/api/v1/").map(str::to_string) else {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return ui::not_allowed();
        }
        return ui::serve(
            state.ui.as_deref(),
            &state.folder_name,
            request.method(),
            &path,
        );
    };
    if request.method() != Method::POST {
        let mut response = refusal(
            StatusCode::METHOD_NOT_ALLOWED,
            "method",
            "The API takes POST only.",
        );
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("POST"));
        return response;
    }
    let origin = header_text(&request, header::ORIGIN);
    let fetch_site = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok());
    if !admission::same_origin(origin, fetch_site, &host) {
        return refusal(
            StatusCode::FORBIDDEN,
            "wrong_origin",
            "Only the page this server serves may call it.",
        );
    }
    if name == "session" {
        return start_session(state, request).await;
    }
    let token = admission::bearer(header_text(&request, header::AUTHORIZATION)).map(str::to_string);
    let Some(session_id) = token.as_deref().and_then(|t| session_id(state, t)) else {
        return refusal(
            StatusCode::UNAUTHORIZED,
            "no_session",
            "This window has no session. Press Enter in the terminal running vitna app for a new window.",
        );
    };
    let token = token.unwrap_or_default();
    match name.as_str() {
        "hello" => json_reply(StatusCode::OK, hello(state)),
        "fs/list" => list(state, &request),
        "fs/stat" => stat(state, &request),
        "fs/read" => read(state, &request),
        "fs/write" => write(state, &token, request).await,
        "fs/remove" => remove(state, &token, &request),
        "run" => run(state, &token, request).await,
        "receipt" => make_receipt(state, &token, &session_id, request).await,
        _ => refusal(
            StatusCode::NOT_FOUND,
            "no_route",
            format!("There is no /api/v1/{name}."),
        ),
    }
}

fn session_id(state: &State, token: &str) -> Option<String> {
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    sessions.get(token).map(|s| s.id.clone())
}

fn record(state: &State, token: &str, effect: Effect) {
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(session) = sessions.get_mut(token) {
        session.journal.record(effect);
    }
}

fn hello(state: &State) -> Value {
    let commands = match &state.commands {
        Commands::Sandboxed {
            backend,
            enforcement,
        } => {
            json!({ "state": "sandboxed", "backend": backend, "enforcement": enforcement })
        }
        Commands::Unsandboxed { reason } => json!({ "state": "unsandboxed", "reason": reason }),
        Commands::Refused { reason } => json!({ "state": "refused", "reason": reason }),
        Commands::NoProcess => json!({ "state": "no_process" }),
    };
    json!({
        "folder": state.folder_name,
        "commands": commands,
        "public_key": state.public_key,
        "max_file_bytes": MAX_FILE_BYTES,
    })
}

#[derive(Deserialize)]
struct SessionRequest {
    launch: String,
}

/// Exchanges a launch code, once and within its two minutes, for a session.
async fn start_session(state: &State, request: Request<Incoming>) -> Reply {
    let asked: SessionRequest = match json_body(request).await {
        Ok(asked) => asked,
        Err(reply) => return reply,
    };
    let valid = {
        let mut codes = state.launch_codes.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let valid = codes
            .remove(asked.launch.trim())
            .is_some_and(|until| until > now);
        codes.retain(|_, until| *until > now);
        valid
    };
    if !valid {
        return refusal(
            StatusCode::FORBIDDEN,
            "launch_code",
            "That window's launch code was used already or has expired. Press Enter in the terminal running vitna app for a new window.",
        );
    }
    let token = admission::random_hex(32);
    let id = format!("app-session-{}", admission::random_hex(8));
    state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            token.clone(),
            Session {
                id,
                journal: journal::Journal::default(),
            },
        );
    let mut reply = hello(state);
    reply["token"] = Value::String(token);
    json_reply(StatusCode::OK, reply)
}

/// A path the page may use, as the tools' file layer should be asked for it:
/// refused if the page's own rules refuse it, and refused again if it leads,
/// once links and aliases are resolved, somewhere those rules keep it from.
fn allowed(state: &State, raw: &str) -> Result<String, Reply> {
    let segments = paths::resolve(raw).map_err(|reason| {
        refusal(
            StatusCode::BAD_REQUEST,
            "bad_path",
            format!("That path is refused: {reason}."),
        )
    })?;
    if let Some(reason) = paths::guarded(&segments) {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "guarded",
            format!("{reason}."),
        ));
    }
    let path = segments.join("/");
    let located = state.files.locate(&path).map_err(file_refusal)?;
    let real: Vec<&str> = located
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if let Some(reason) = paths::guarded(&real) {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "guarded",
            format!("That path leads to {located}, and {reason}."),
        ));
    }
    Ok(path)
}

fn path_param(state: &State, params: &HashMap<String, String>) -> Result<String, Reply> {
    allowed(state, params.get("path").map(String::as_str).unwrap_or(""))
}

fn entry_json(name: &str, kind: EntryKind, size: Option<u64>) -> Value {
    match kind {
        EntryKind::Directory => json!({ "name": name, "kind": "directory" }),
        EntryKind::File => json!({ "name": name, "kind": "file", "size": size }),
    }
}

fn list(state: &State, request: &Request<Incoming>) -> Reply {
    let params = query(request);
    let path = match path_param(state, &params) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    let sizes = params.get("sizes").map(String::as_str) != Some("0");
    match state.files.list(&path, sizes) {
        Ok(entries) => {
            let entries: Vec<Value> = entries
                .iter()
                .map(|e| entry_json(&e.name, e.kind, e.size))
                .collect();
            json_reply(StatusCode::OK, json!({ "entries": entries }))
        }
        Err(e) => file_refusal(e),
    }
}

fn stat(state: &State, request: &Request<Incoming>) -> Reply {
    let path = match path_param(state, &query(request)) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    match state.files.stat(&path) {
        Ok(entry) => json_reply(
            StatusCode::OK,
            entry_json(&entry.name, entry.kind, entry.size),
        ),
        Err(e) => file_refusal(e),
    }
}

fn read(state: &State, request: &Request<Incoming>) -> Reply {
    let path = match path_param(state, &query(request)) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    match state.files.read(&path) {
        Ok(bytes) => {
            let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes));
            let mut response = Response::new(Full::new(Bytes::from(bytes)));
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(value) = header::HeaderValue::from_str(&hash) {
                headers.insert("x-vitna-sha256", value);
            }
            with_api_headers(response)
        }
        Err(e) => file_refusal(e),
    }
}

/// `absent`, or a SHA-256 in hex.
fn expected_hash(params: &HashMap<String, String>) -> Result<String, Reply> {
    match params.get("expect").map(String::as_str) {
        Some("absent") => Ok(ABSENT_PREIMAGE_HASH.to_string()),
        Some(hash) if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(hash.to_ascii_lowercase()),
        _ => Err(refusal(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "A change names what the file holds now: expect=<its SHA-256>, or expect=absent for a new file.",
        )),
    }
}

async fn write(state: &State, token: &str, request: Request<Incoming>) -> Reply {
    let params = query(&request);
    let (path, expected) = match (path_param(state, &params), expected_hash(&params)) {
        (Ok(path), Ok(expected)) => (path, expected),
        (Err(reply), _) | (_, Err(reply)) => return reply,
    };
    if path.is_empty() {
        return refusal(StatusCode::BAD_REQUEST, "bad_path", "A write names a file.");
    }
    let content = match body(request, MAX_FILE_BYTES as usize).await {
        Ok(content) => content,
        Err(reply) => return reply,
    };
    match state.files.write(&path, &expected, &content) {
        Ok(written) => {
            record(
                state,
                token,
                Effect::Write {
                    path: path.clone(),
                    preimage_hash: written.preimage_hash.clone(),
                    postimage_hash: written.postimage_hash.clone(),
                },
            );
            json_reply(
                StatusCode::OK,
                json!({ "preimage_hash": written.preimage_hash, "postimage_hash": written.postimage_hash }),
            )
        }
        Err(e) => file_refusal(e),
    }
}

fn remove(state: &State, token: &str, request: &Request<Incoming>) -> Reply {
    let params = query(request);
    let (path, expected) = match (path_param(state, &params), expected_hash(&params)) {
        (Ok(path), Ok(expected)) => (path, expected),
        (Err(reply), _) | (_, Err(reply)) => return reply,
    };
    match state.files.remove(&path, &expected) {
        Ok(held) => {
            record(
                state,
                token,
                Effect::Remove {
                    path,
                    preimage_hash: held.clone(),
                },
            );
            json_reply(StatusCode::OK, json!({ "removed": held }))
        }
        Err(e) => file_refusal(e),
    }
}

#[derive(Deserialize)]
struct RunRequest {
    command: String,
    #[serde(default)]
    directory: String,
    timeout_ms: Option<u64>,
}

/// The end of `text`, at most `cap` bytes of it, and whether anything went.
fn tail(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut start = text.len() - cap;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

async fn run(state: &State, token: &str, request: Request<Incoming>) -> Reply {
    let asked: RunRequest = match json_body(request).await {
        Ok(asked) => asked,
        Err(reply) => return reply,
    };
    let command = asked.command.trim().to_string();
    if command.is_empty() || command.chars().count() > MAX_COMMAND_CHARS || command.contains('\0') {
        return refusal(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "A command is one line of text, at most 8192 characters.",
        );
    }
    let directory = match allowed(state, &asked.directory) {
        Ok(directory) => directory,
        Err(reply) => return reply,
    };
    let working_dir = match state.files.directory(&directory) {
        Ok(dir) => dir,
        Err(e) => return file_refusal(e),
    };
    let not_run = |reason: String| Effect::Command {
        command: command.clone(),
        directory: directory.clone(),
        ran: None,
        not_run: Some(reason),
    };
    if let Commands::Refused { reason } = &state.commands {
        record(state, token, not_run(format!("refused: {reason}")));
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_sandbox",
            format!(
                "Commands are not run here: this machine has no sandbox ({reason}), and vitna app was not started with --allow-unsandboxed."
            ),
        );
    }
    let timeout_ms = asked
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1_000, MAX_TIMEOUT_MS);
    let Ok(_slot) = state.command_slot.acquire().await else {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "stopping",
            "The server is stopping.",
        );
    };
    let mut command_request = CommandRequest::new(&command, &working_dir, &state.root, timeout_ms);
    command_request.allow_unsandboxed = state.allow_unsandboxed;
    command_request.allow_network = false;
    match state.runner.run_command(command_request).await {
        Ok(output) => {
            record(
                state,
                token,
                Effect::Command {
                    command: command.clone(),
                    directory: directory.clone(),
                    ran: Some(Ran {
                        action_id: output.action_id.clone(),
                        exit_code: output.exit_code,
                        statement_digest: output.statement_digest.clone(),
                        sandbox_backend: output.sandbox_backend.clone(),
                        sandbox_enforcement: output.sandbox_enforcement.clone(),
                        duration_ms: output.duration_ms,
                    }),
                    not_run: None,
                },
            );
            let (stdout, stdout_cut) = tail(&output.stdout, OUTPUT_CAP);
            let (stderr, stderr_cut) = tail(&output.stderr, OUTPUT_CAP);
            json_reply(
                StatusCode::OK,
                json!({
                    "exit_code": output.exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                    "stdout_bytes": output.stdout.len(),
                    "stderr_bytes": output.stderr.len(),
                    "stdout_truncated": stdout_cut,
                    "stderr_truncated": stderr_cut,
                    "duration_ms": output.duration_ms,
                    "sandbox_backend": output.sandbox_backend,
                    "sandbox_enforcement": output.sandbox_enforcement,
                    "action_id": output.action_id,
                    "statement_digest": output.statement_digest,
                }),
            )
        }
        Err(reason) => {
            record(state, token, not_run(reason.clone()));
            if reason.starts_with(SANDBOX_UNAVAILABLE) {
                refusal(StatusCode::SERVICE_UNAVAILABLE, "no_sandbox", reason)
            } else {
                refusal(StatusCode::INTERNAL_SERVER_ERROR, "not_run", reason)
            }
        }
    }
}

#[derive(Deserialize)]
struct ReceiptRequest {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    completion: Option<String>,
}

/// A value the page reported, kept short and printable.
fn reported(value: &str) -> String {
    let clean: String = value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REPORTED_CHARS)
        .collect();
    if clean.is_empty() {
        "unreported".to_string()
    } else {
        clean
    }
}

/// The commit the folder's checkout is at, read from its `.git` through the
/// same file layer: `HEAD` itself, or the branch it names, loose or packed.
fn base_commit(state: &State) -> Option<String> {
    let head = String::from_utf8(state.files.read(".git/HEAD").ok()?).ok()?;
    if let Some(id) = journal::commit_id(&head) {
        return Some(id);
    }
    let reference = head.trim().strip_prefix("ref: ")?.trim().to_string();
    let plain = reference.starts_with("refs/")
        && reference
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !reference.contains('\\');
    if !plain {
        return None;
    }
    if let Ok(bytes) = state.files.read(&format!(".git/{reference}")) {
        return journal::commit_id(&String::from_utf8(bytes).ok()?);
    }
    let packed = String::from_utf8(state.files.read(".git/packed-refs").ok()?).ok()?;
    packed.lines().find_map(|line| {
        let (id, name) = line.split_once(' ')?;
        (name.trim() == reference)
            .then(|| journal::commit_id(id))
            .flatten()
    })
}

async fn make_receipt(
    state: &State,
    token: &str,
    session_id: &str,
    request: Request<Incoming>,
) -> Reply {
    let asked: ReceiptRequest = match json_body(request).await {
        Ok(asked) => asked,
        Err(reply) => return reply,
    };
    // The page can say a turn stopped short. It cannot say the work is done:
    // nothing here can judge that.
    let completion = match asked.completion.as_deref() {
        None | Some("completed_with_unknowns") => "completed_with_unknowns",
        Some("cancelled") => "cancelled",
        Some("failed") => "failed",
        Some("blocked") => "blocked",
        Some(other) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("A receipt from the page ends completed_with_unknowns, cancelled, failed or blocked, not {other}."),
            )
        }
    };
    let effects = {
        let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
        match sessions.get_mut(token) {
            Some(session) => session.journal.take(),
            None => Vec::new(),
        }
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let run_id = format!("app-{now_ms}-{}", admission::random_hex(4));
    let details = journal::Details {
        run_id: run_id.clone(),
        session_id,
        root: &state.root,
        base_commit: base_commit(state),
        provider: reported(&asked.provider),
        model: reported(&asked.model),
        completion: completion.to_string(),
        summary: asked
            .summary
            .map(|s| s.chars().take(MAX_SUMMARY_CHARS).collect()),
    };
    let signed = journal::receipt(&effects, details, &state.signing_key).and_then(|receipt| {
        let text = serde_json::to_string_pretty(&receipt).map_err(|e| e.to_string())?;
        let path = format!(".vitna/receipts/{run_id}.json");
        state
            .files
            .write(&path, ABSENT_PREIMAGE_HASH, text.as_bytes())
            .map_err(|e| format!("could not write {path}: {e}"))?;
        Ok((receipt, path))
    });
    match signed {
        Ok((receipt, path)) => json_reply(
            StatusCode::OK,
            json!({
                "run_id": run_id,
                "path": path,
                "public_key": state.public_key,
                "receipt": receipt,
            }),
        ),
        Err(reason) => {
            // Nothing was lost: the next receipt covers these effects too.
            let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(session) = sessions.get_mut(token) {
                session.journal.restore(effects);
            }
            refusal(StatusCode::INTERNAL_SERVER_ERROR, "failed", reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_keeps_its_end_and_never_splits_a_character() {
        assert_eq!(tail("short", 10), ("short", false));
        assert_eq!(tail("0123456789", 4), ("6789", true));
        // "é" is two bytes; cutting inside it moves the cut forward.
        let (kept, cut) = tail("aébc", 3);
        assert!(cut);
        assert_eq!(kept, "bc");
    }

    #[test]
    fn reported_values_are_short_and_printable() {
        assert_eq!(reported("  anthropic/claude\n "), "anthropic/claude");
        assert_eq!(reported(""), "unreported");
        assert_eq!(reported("a\u{0007}b"), "ab");
        assert_eq!(reported(&"x".repeat(500)).len(), MAX_REPORTED_CHARS);
    }
}
