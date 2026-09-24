//! The chain of checks: what this window did to a receipt before believing it.
//!
//! Vitna's record is one "a stranger can check", and vitna.ai draws that as a
//! proof path, a line of nodes from the archived document to the signed root,
//! each node a check with its answer. A receipt here has the same shape in
//! three links, and they are the only three this window runs:
//!
//!   Read     the file parsed as a `vitna-run-receipt-v1`.
//!   Checked  every check that needs no key held. That is
//!            `vitna-receipt-verify`: the schema, the isolation label, the
//!            completion state, and a before and after hash for each file.
//!   Signed   the device signature held against the key the connected daemon
//!            publishes in `Health`.
//!
//! It is drawn small on every row of the ledger and large on an open run, so
//! it is one grammar in two places rather than two ornaments, the way the
//! Convexity draft's range rail is one mark on the tape and on every company.
//! A node's colour is its answer and never decoration: the faint ink for a
//! check that held, a hollow ring for one that was not run (never a fault,
//! and never painted as one), rust for one that failed, and green for exactly
//! one thing, a signature verified against the daemon's own key. That is the
//! only confirmed read this window makes, and Vitna keeps green for a
//! confirmed read and nothing else.

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};

use crate::runs::{Run, Signature};
use crate::theme;

/// One link's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// Run, and it held.
    Held,
    /// Run against the daemon's own key, and it held: the one confirmed read.
    Confirmed,
    /// Not run, for the reason the step states. Not a fault.
    Unchecked,
    /// Run, and it did not hold.
    Failed,
}

impl Node {
    fn tone(self) -> Color32 {
        match self {
            Node::Held => theme::FAINT,
            Node::Confirmed => theme::OK,
            Node::Unchecked => theme::FAINTER,
            Node::Failed => theme::RUST,
        }
    }
}

/// One link: its name, its answer, and the sentence that says what happened.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: &'static str,
    pub node: Node,
    pub said: String,
}

pub type Chain = [Step; 3];

/// The chain for a receipt that parsed. `key` is the connected daemon's
/// signing key, and `no_key_because` says why there is none when there is not.
pub fn for_run(run: &Run, key: Option<&str>, no_key_because: &str) -> Chain {
    let file = run
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| run.path.display().to_string());
    let files = run.receipt.changeset.files_modified.len();
    let checked = if run.report.errors.is_empty() {
        Step {
            name: "Checked",
            node: Node::Held,
            said: match files {
                0 => "Every check that needs no key held: the schema, the isolation label and the completion state. The run changed no files, so there were no hashes to check.".to_string(),
                1 => "Every check that needs no key held: the schema, the isolation label, the completion state, and a before and after hash for the one changed file.".to_string(),
                n => format!("Every check that needs no key held: the schema, the isolation label, the completion state, and a before and after hash for each of the {n} changed files."),
            },
        }
    } else {
        Step { name: "Checked", node: Node::Failed, said: run.report.errors.join(" ") }
    };
    let short_key = key.map(theme::digest).unwrap_or_default();
    let signed = match run.signature(key) {
        Signature::Verified => Step {
            name: "Signed",
            node: Node::Confirmed,
            said: format!("Verified against the key the connected daemon signs with, {short_key}."),
        },
        Signature::Unchecked => Step {
            name: "Signed",
            node: Node::Unchecked,
            said: format!("A signature is present and was not checked: {no_key_because}. That is not a fault in the receipt."),
        },
        Signature::Mismatch => Step {
            name: "Signed",
            node: Node::Failed,
            said: format!(
                "It does not verify with the key the connected daemon signs with, {short_key}. Another key signed it, or it changed after it was signed; a receipt names no key, so it cannot say which."
            ),
        },
        Signature::Absent => Step {
            name: "Signed",
            node: Node::Failed,
            said: "The receipt carries no signature at all, which is a fault.".to_string(),
        },
    };
    [
        Step {
            name: "Read",
            node: Node::Held,
            said: format!("Parsed from {file} as a vitna-run-receipt-v1."),
        },
        checked,
        signed,
    ]
}

/// The chain for a file in the receipts directory that is not a receipt.
pub fn for_unreadable(reason: &str) -> Chain {
    [
        Step { name: "Read", node: Node::Failed, said: format!("It did not parse as a receipt: {reason}") },
        Step { name: "Checked", node: Node::Unchecked, said: "Nothing to check, having no receipt to check.".to_string() },
        Step { name: "Signed", node: Node::Unchecked, said: "Nothing to check, having no receipt to check.".to_string() },
    ]
}

/// Whether any link failed, which is what a row calls "a check failed".
pub fn any_failed(chain: &Chain) -> bool {
    chain.iter().any(|s| s.node == Node::Failed)
}

/// The chain as lines of words, for a hover.
pub fn hint(chain: &Chain) -> String {
    chain
        .iter()
        .map(|s| format!("{}: {}", s.name, s.said))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Node spacing and size in the small rail: far enough apart that the
/// hairline between two nodes reads as a link rather than as a gap.
const RAIL_GAP: f32 = 17.0;
const RAIL_R: f32 = 3.0;

/// How wide the small rail is.
pub const RAIL_W: f32 = RAIL_GAP * 2.0 + RAIL_R * 2.0;

/// The small rail, its first node's centre at `start`: three dots on a
/// hairline, read left to right as read, checked, signed.
pub fn rail(p: &egui::Painter, start: Pos2, chain: &Chain) {
    let at = |i: usize| start + Vec2::new(RAIL_GAP * i as f32, 0.0);
    for i in 0..chain.len() - 1 {
        p.line_segment(
            [at(i) + Vec2::new(RAIL_R + 1.5, 0.0), at(i + 1) - Vec2::new(RAIL_R + 1.5, 0.0)],
            Stroke::new(1.0, theme::HAIR),
        );
    }
    for (i, step) in chain.iter().enumerate() {
        dot(p, at(i), RAIL_R, step.node);
    }
}

fn dot(p: &egui::Painter, c: Pos2, r: f32, node: Node) {
    match node {
        Node::Unchecked => {
            p.circle_stroke(c, r - 0.4, Stroke::new(1.2, node.tone()));
        }
        _ => {
            p.circle_filled(c, r, node.tone());
        }
    }
}

/// The large path, for an open run: each link on a line of its own, its node
/// on a vertical hairline, its name and then what happened.
pub fn path(ui: &mut egui::Ui, chain: &Chain) {
    const LANE: f32 = 22.0;
    const R: f32 = 4.5;
    let mut centres: Vec<Pos2> = Vec::new();
    for (i, step) in chain.iter().enumerate() {
        if i > 0 {
            ui.add_space(12.0);
        }
        ui.horizontal_top(|ui| {
            let (lane, _) = ui.allocate_exact_size(Vec2::new(LANE, 18.0), egui::Sense::hover());
            let c = egui::pos2(lane.left() + R + 1.0, lane.center().y);
            dot(ui.painter(), c, R, step.node);
            centres.push(c);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(
                    RichText::new(step.name)
                        .font(theme::display(theme::FS_UI))
                        .color(if step.node == Node::Failed { theme::RUST } else { theme::INK }),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&step.said).font(theme::prose(theme::FS_UI)).color(theme::FAINT),
                    )
                    .wrap(),
                );
            });
        });
    }
    let p = ui.painter();
    for pair in centres.windows(2) {
        p.line_segment(
            [pair[0] + Vec2::new(0.0, R + 4.0), pair[1] - Vec2::new(0.0, R + 4.0)],
            Stroke::new(1.0, theme::HAIR),
        );
    }
}

/// The rect a rail at `start` covers, for hit testing a hover.
pub fn rail_rect(start: Pos2) -> Rect {
    Rect::from_min_max(
        start - Vec2::new(RAIL_R + 3.0, 9.0),
        start + Vec2::new(RAIL_W - RAIL_R + 3.0, 9.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use vitna_receipts::{ChangeSetRecord, FileModificationRecord, ModelSelectionRecord, VitnaRunReceiptV1};

    fn run(files: usize, key: &ed25519_dalek::SigningKey) -> Run {
        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-1".to_string(),
            session_id: "sess-1".to_string(),
            workspace_fingerprint: "ws".to_string(),
            base_commit_sha: "0".repeat(40),
            model_selection: ModelSelectionRecord {
                provider: "anthropic".to_string(),
                model_sku: "m".to_string(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "root".to_string(),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![],
            changeset: ChangeSetRecord {
                files_modified: (0..files)
                    .map(|i| FileModificationRecord {
                        path: format!("f{i}.rs"),
                        preimage_hash: "a".repeat(64),
                        postimage_hash: "b".repeat(64),
                    })
                    .collect(),
                diff_digest: "d".to_string(),
            },
            runner_execution_statements: vec![],
            child_receipt_roots: vec![],
            device_signature: String::new(),
        };
        receipt.sign(key).expect("signs");
        let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&receipt, None).expect("report");
        Run {
            path: PathBuf::from(".vitna/receipts/run-1.json"),
            receipt: Box::new(receipt),
            report,
            written: None,
            checked: RefCell::new(None),
        }
    }

    /// Without a key the last link is a ring, and a ring is not a failure:
    /// a window that painted "not checked" as a fault would be accusing every
    /// honest receipt.
    #[test]
    fn no_key_leaves_the_signature_unchecked_and_nothing_failed() {
        let key = vitna_receipts::generate_signing_key();
        let chain = for_run(&run(2, &key), None, "no daemon is connected");
        assert_eq!(chain.each_ref().map(|s| s.node), [Node::Held, Node::Held, Node::Unchecked]);
        assert!(!any_failed(&chain));
        assert!(chain[2].said.contains("no daemon is connected"));
    }

    /// Green is earned by one thing only: the daemon's own key.
    #[test]
    fn only_a_verified_signature_is_confirmed() {
        let key = vitna_receipts::generate_signing_key();
        let other = vitna_receipts::generate_signing_key();
        let hex_of = |k: &ed25519_dalek::SigningKey| hex::encode(k.verifying_key().to_bytes());
        let r = run(1, &key);
        assert_eq!(for_run(&r, Some(&hex_of(&key)), "").map(|s| s.node)[2], Node::Confirmed);
        assert_eq!(for_run(&r, Some(&hex_of(&other)), "").map(|s| s.node)[2], Node::Failed);
        for chain in [for_run(&r, Some(&hex_of(&other)), ""), for_run(&r, None, "x")] {
            assert!(chain.iter().take(2).all(|s| s.node != Node::Confirmed));
        }
    }

    #[test]
    fn a_file_that_is_not_a_receipt_fails_at_the_first_link() {
        let chain = for_unreadable("missing field `run_id`");
        assert_eq!(chain.each_ref().map(|s| s.node), [Node::Failed, Node::Unchecked, Node::Unchecked]);
        assert!(any_failed(&chain));
    }
}
