//! What the window knows about `vitna-coded`.
//!
//! ADR-0005 puts the daemon on an owner-only Unix domain socket or a Windows
//! named pipe and never on loopback TCP, so that no web page can reach it.
//! That decision is the reason this window is a native application rather than
//! a browser tab: the transport is one a page structurally cannot open.
//!
//! The endpoint list used to live here as well as in the daemon, which is a
//! drift waiting to happen, and an invisible one: a daemon binding where the
//! window does not look is indistinguishable, from here, from no daemon at
//! all. Both sides read `vitna_protocol::endpoint` now.
//!
//! This module is state, not behaviour. The connecting is done by
//! [`crate::daemon::Worker`] on its own thread, because the client blocks and
//! a turn blocks for as long as the model takes.

use vitna_protocol::api::HealthResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// The first probe has not answered yet. Distinct from `Absent` on
    /// purpose: "not running" is a finding, and the window has not made it
    /// yet.
    Probing,
    /// Nothing is listening. Carries the endpoints that were tried and why
    /// each refused, so the reading is checkable rather than a shrug.
    Absent { tried: Vec<String>, detail: String },
    /// A daemon answered a health call.
    Open {
        endpoint: String,
        health: Box<HealthResponse>,
    },
}

impl Link {
    pub fn is_open(&self) -> bool {
        matches!(self, Link::Open { .. })
    }

    /// The endpoints this window would try, for the settings page to list.
    pub fn candidates() -> Vec<String> {
        vitna_protocol::endpoint::candidates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_endpoint_the_window_tries_is_tcp() {
        // The ADR's whole point, asserted from the consuming side as well as
        // from the declaring one: a loopback port would be reachable from any
        // page in any browser on this machine.
        for endpoint in Link::candidates() {
            assert!(!endpoint.contains("127.0.0.1"), "got {endpoint}");
            assert!(!endpoint.contains("localhost"), "got {endpoint}");
        }
    }

    #[test]
    fn a_window_that_has_not_looked_yet_does_not_claim_the_daemon_is_absent() {
        // Probing must not read as open, and must not read as a finding.
        let l = Link::Probing;
        assert!(!l.is_open());
        assert!(!matches!(l, Link::Absent { .. }));
    }
}
