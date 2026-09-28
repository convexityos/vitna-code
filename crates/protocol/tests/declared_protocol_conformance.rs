//! Holds `vitna_protocol::type_url` equal to what the `.proto` files declare,
//! and to the one other implementation of this wire in the repository.
//!
//! The `.proto` files under `protocol/vitna/protocol/v1/` are the declaration.
//! Nothing generates code from them today (the framing is serde JSON, which
//! `crate::ProtocolEnvelope` says out loud), so without this test they are
//! prose: a name can be added to the daemon, or dropped from it, and the
//! declaration goes on saying something else with every check green.
//!
//! That is not hypothetical. The `native-desktop-app` branch spelled `Health`,
//! `HealthResult`, `ListSessions`, `SessionList`, `TurnResult`, `ListTurns`,
//! `TurnList` and `Error` on the wire, none of them declared anywhere, while
//! implementing none of `SteerRun`, `PauseRun`, `ResumeRun`, `CancelRun`,
//! `ApproveAction`, `RejectAction`, `SpawnChild`, `ApplyChangeSet`,
//! `AttachTerminal` or `SubscribeEvents`, all of which are declared. Both
//! directions are failures, and they fail differently: an undeclared name
//! cannot be implemented by a second client, and a declared name nobody
//! implements is a promise the protocol does not keep.
//!
//! What this test cannot check: whether a payload's FIELDS match the message
//! that declares them. It compares names only. A `SubmitTurn` whose Rust
//! struct has lost a field passes here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/protocol.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/protocol sits two levels below the repository root")
        .to_path_buf()
}

fn proto(file: &str) -> String {
    let path = repo_root().join("protocol/vitna/protocol/v1").join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read the declaration at {}: {e}", path.display()))
}

/// The `message X {` names a `.proto` file declares, in file order.
fn declared_messages(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("message "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(|name| name.trim_end_matches('{').to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

fn names(urls: &[&str]) -> BTreeSet<String> {
    urls.iter()
        .map(|u| {
            vitna_protocol::type_url::message_name(u)
                .unwrap_or_else(|| panic!("{u} does not carry the declared prefix"))
                .to_string()
        })
        .collect()
}

fn set(items: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    items.into_iter().collect()
}

/// Reads `apps/vitna-desktop/src/protocol/types.ts`, the only other
/// implementation of this wire.
fn desktop_types_ts() -> String {
    let path = repo_root().join("apps/vitna-desktop/src/protocol/types.ts");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn commands_match_commands_proto_exactly() {
    let declared = set(declared_messages(&proto("commands.proto")));
    let implemented = names(vitna_protocol::type_url::COMMANDS);

    assert_eq!(
        declared,
        implemented,
        "commands.proto and type_url::COMMANDS disagree.\n  \
         declared but not implemented: {:?}\n  \
         implemented but not declared: {:?}",
        declared.difference(&implemented).collect::<Vec<_>>(),
        implemented.difference(&declared).collect::<Vec<_>>(),
    );
}

#[test]
fn events_match_events_proto_exactly() {
    let declared = set(declared_messages(&proto("events.proto")));
    let implemented = names(vitna_protocol::type_url::EVENTS);

    assert_eq!(
        declared,
        implemented,
        "events.proto and type_url::EVENTS disagree.\n  \
         declared but not implemented: {:?}\n  \
         implemented but not declared: {:?}",
        declared.difference(&implemented).collect::<Vec<_>>(),
        implemented.difference(&declared).collect::<Vec<_>>(),
    );
}

/// `ProtocolEnvelope` is the frame, not something framed, so it is the one
/// declared message with no `type_url`. Everything else in `envelope.proto`
/// travels as a payload like any command.
#[test]
fn envelope_payloads_are_every_envelope_message_but_the_envelope() {
    let declared: BTreeSet<String> = declared_messages(&proto("envelope.proto"))
        .into_iter()
        .filter(|name| name != "ProtocolEnvelope")
        .collect();
    let implemented = names(vitna_protocol::type_url::ENVELOPE_PAYLOADS);

    assert_eq!(
        declared,
        implemented,
        "envelope.proto and type_url::ENVELOPE_PAYLOADS disagree.\n  \
         declared but not implemented: {:?}\n  \
         implemented but not declared: {:?}",
        declared.difference(&implemented).collect::<Vec<_>>(),
        implemented.difference(&declared).collect::<Vec<_>>(),
    );

    assert!(
        !implemented.contains("ProtocolEnvelope"),
        "the envelope must not name itself in its own type_url"
    );
}

/// The prefix is the one string both implementations of this wire must spell
/// identically. They are written in different languages, so nothing but a
/// test like this compares them.
#[test]
fn the_typescript_client_spells_the_same_prefix() {
    let source = desktop_types_ts();

    let line = source
        .lines()
        .find(|l| l.contains("TYPE_URL_PREFIX") && l.contains('='))
        .expect("types.ts no longer declares TYPE_URL_PREFIX; this test needs rewriting");

    let quoted = line
        .split(['\'', '"'])
        .nth(1)
        .expect("TYPE_URL_PREFIX is not assigned a quoted literal");

    assert_eq!(
        quoted,
        vitna_protocol::type_url::PREFIX,
        "the desktop client and this crate disagree about the type_url prefix, \
         so neither can read a frame the other sends"
    );
}

/// The TypeScript client keys its command registry by declared message name,
/// including `HandshakeRequest`, which `envelope.proto` declares but which
/// travels as a command like any other.
///
/// This is a name comparison across two languages and nothing more. It does
/// not prove either side can serialize what the other sends.
#[test]
fn the_typescript_client_implements_the_same_command_set() {
    let source = desktop_types_ts();

    let body = source
        .split_once("export interface CommandBodies {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .expect("types.ts no longer declares CommandBodies; this test needs rewriting");

    let typescript: BTreeSet<String> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//") && !line.starts_with('*'))
        .filter_map(|line| line.split_once(':'))
        .map(|(key, _)| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .collect();

    let mut rust = names(vitna_protocol::type_url::COMMANDS);
    rust.insert("HandshakeRequest".to_string());

    assert_eq!(
        rust,
        typescript,
        "the desktop client and this crate implement different command sets.\n  \
         in Rust only: {:?}\n  \
         in TypeScript only: {:?}",
        rust.difference(&typescript).collect::<Vec<_>>(),
        typescript.difference(&rust).collect::<Vec<_>>(),
    );
}

/// Mutation check on the parser itself: a helper that silently found nothing
/// would make every test above pass by comparing two empty sets.
#[test]
fn the_declaration_parser_actually_finds_messages() {
    assert!(
        declared_messages(&proto("commands.proto")).len() >= 12,
        "commands.proto parsed to fewer messages than it declares; the parser is broken"
    );
    assert!(
        declared_messages(&proto("events.proto")).len() >= 14,
        "events.proto parsed to fewer messages than it declares; the parser is broken"
    );
    assert_eq!(
        declared_messages("message Alpha {\n  string a = 1;\n}\nmessage Beta {"),
        vec!["Alpha".to_string(), "Beta".to_string()],
    );
    assert!(declared_messages("// message Commented {").is_empty());
}

// ---- fields ----------------------------------------------------------------
//
// Everything above compares message NAMES. These compare the fields inside
// them, which the name check said out loud it could not see: a `SubmitTurn`
// that had lost a field passed it. The framing is serde JSON, so a Rust
// struct's serialized keys ARE the wire form, and a rename here is a rename on
// the wire.

/// The field names a `.proto` message declares, in the order it declares them.
fn declared_fields(source: &str, message: &str) -> Vec<String> {
    let start = source
        .find(&format!("message {message} {{"))
        .unwrap_or_else(|| panic!("{message} is not declared"));
    let body_start = start + source[start..].find('{').expect("a message has a body");
    let body_end = body_start
        + source[body_start..]
            .find('}')
            .expect("a message body closes");

    source[body_start + 1..body_end]
        .lines()
        .map(|line| line.split("//").next().unwrap_or("").trim())
        .filter(|line| line.ends_with(';') && line.contains('='))
        .filter_map(|line| {
            // `repeated string bound_mounts = 8;` -> `bound_mounts`
            let before_eq = line.split('=').next()?.trim();
            before_eq.split_whitespace().last().map(str::to_string)
        })
        .collect()
}

/// The keys a value actually serializes to.
fn serialized_fields<T: serde::Serialize>(value: &T) -> Vec<String> {
    match serde_json::to_value(value).expect("serialize") {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("expected an object, got {other}"),
    }
}

fn assert_fields_match<T: serde::Serialize + Default>(proto_file: &str, message: &str) {
    assert_fields_of(proto_file, message, &T::default());
}

fn assert_fields_of<T: serde::Serialize>(proto_file: &str, message: &str, value: &T) {
    let declared: BTreeSet<String> = declared_fields(&proto(proto_file), message)
        .into_iter()
        .collect();
    let implemented: BTreeSet<String> = serialized_fields(value).into_iter().collect();

    assert_eq!(
        declared,
        implemented,
        "{message} does not match its declaration.\n  \
         declared but not serialized: {:?}\n  \
         serialized but not declared: {:?}",
        declared.difference(&implemented).collect::<Vec<_>>(),
        implemented.difference(&declared).collect::<Vec<_>>(),
    );
}

#[test]
fn every_implemented_message_carries_exactly_its_declared_fields() {
    use vitna_protocol::messages::*;

    assert_fields_match::<HandshakeRequest>("envelope.proto", "HandshakeRequest");
    assert_fields_match::<HandshakeResponse>("envelope.proto", "HandshakeResponse");
    assert_fields_match::<ErrorResponse>("envelope.proto", "ErrorResponse");
    assert_fields_match::<SubscribeEvents>("commands.proto", "SubscribeEvents");
    assert_fields_match::<ApprovalRequested>("events.proto", "ApprovalRequested");
    assert_fields_match::<ToolStarted>("events.proto", "ToolStarted");
    assert_fields_match::<ToolFinished>("events.proto", "ToolFinished");
}

/// The frame itself, which carries every other message.
/// The frame itself, which carries every other message. Built rather than
/// defaulted, because `ProtocolEnvelope` has no `Default` and a public type
/// should not grow one to suit a test.
#[test]
fn the_envelope_carries_exactly_its_declared_fields() {
    let envelope = vitna_protocol::ProtocolEnvelope::new(
        vitna_protocol::type_url::event::DIAGNOSTIC,
        "sess",
        "run",
        1,
        "key",
        Vec::new(),
    );
    assert_fields_of("envelope.proto", "ProtocolEnvelope", &envelope);
}

/// Mutation check on the parser: a helper that found no fields would make
/// every comparison above pass by comparing two empty sets.
#[test]
fn the_field_parser_actually_finds_fields() {
    let tool_started = declared_fields(&proto("events.proto"), "ToolStarted");
    assert_eq!(tool_started, vec!["tool_call_id", "started_at_ms"]);

    // A `repeated` field, and one followed by a trailing comment.
    let approval = declared_fields(&proto("events.proto"), "ApprovalRequested");
    assert!(approval.contains(&"environment_names".to_string()));
    let finished = declared_fields(&proto("events.proto"), "ToolFinished");
    assert!(
        finished.contains(&"status".to_string()),
        "a field whose line ends in a comment must still be found: {finished:?}"
    );
}
