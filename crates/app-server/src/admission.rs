//! Who may use the server, decided before any handler runs (ADR-0006, 3 and 4).
//!
//! Each check answers one attack ADR-0005 names:
//!
//! - the TCP peer must be this machine, so nothing on the network reaches it;
//! - the `Host` header must name this server exactly, so a page on another
//!   name that resolves to 127.0.0.1 (DNS rebinding) is refused;
//! - an API request must be a `POST` from this server's own origin, which a
//!   browser states in `Origin` and, where it sends one, `Sec-Fetch-Site`;
//! - and it must carry a session token, which only a page that exchanged a
//!   launch code holds. No CORS header is ever sent, so another origin cannot
//!   get past the preflight a custom header forces, and could not read a
//!   reply if it did.

use rand_core::{OsRng, RngCore};
use std::net::IpAddr;
use std::time::Duration;

/// How long a launch code works for, once minted.
pub(crate) const LAUNCH_CODE_TTL: Duration = Duration::from_secs(120);

/// `n` random bytes from the operating system, in hex.
pub(crate) fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// True for a peer on this machine, an IPv4 address mapped into IPv6
/// included.
pub(crate) fn peer_is_local(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// The `Host` header, when it names this server exactly. The server listens
/// on 127.0.0.1 only; `localhost` is accepted because a person may type it.
pub(crate) fn our_host(host: Option<&str>, port: u16) -> Option<String> {
    let host = host?.trim().to_ascii_lowercase();
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    ours.contains(&host).then_some(host)
}

/// Whether a request comes from a page this server served, reached by the
/// name in `host`.
pub(crate) fn same_origin(origin: Option<&str>, fetch_site: Option<&str>, host: &str) -> bool {
    let from_here =
        origin.is_some_and(|o| o.trim().eq_ignore_ascii_case(&format!("http://{host}")));
    // A browser that sends Sec-Fetch-Site says outright where the request
    // came from; one that does not is judged on Origin alone.
    let site_agrees = fetch_site.is_none_or(|site| site.trim().eq_ignore_ascii_case("same-origin"));
    from_here && site_agrees
}

/// The token in `Authorization: Bearer <token>`.
pub(crate) fn bearer(authorization: Option<&str>) -> Option<&str> {
    let token = authorization?.trim().strip_prefix("Bearer ")?.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machine_is_a_peer() {
        assert!(peer_is_local("127.0.0.1".parse().unwrap()));
        assert!(peer_is_local("::1".parse().unwrap()));
        assert!(peer_is_local("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!peer_is_local("192.168.1.20".parse().unwrap()));
        assert!(!peer_is_local("::ffff:10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn the_host_must_name_this_server() {
        assert_eq!(
            our_host(Some("127.0.0.1:7788"), 7788).as_deref(),
            Some("127.0.0.1:7788")
        );
        assert_eq!(
            our_host(Some("LocalHost:7788"), 7788).as_deref(),
            Some("localhost:7788")
        );
        // DNS rebinding: another name that resolves here.
        assert_eq!(our_host(Some("attacker.example:7788"), 7788), None);
        assert_eq!(
            our_host(Some("127.0.0.1"), 7788),
            None,
            "a Host with no port"
        );
        assert_eq!(our_host(Some("127.0.0.1:7789"), 7788), None, "another port");
        assert_eq!(our_host(Some("127.0.0.1.nip.io:7788"), 7788), None);
        assert_eq!(our_host(None, 7788), None);
    }

    #[test]
    fn only_this_origin_may_call_the_api() {
        let host = "127.0.0.1:7788";
        assert!(same_origin(
            Some("http://127.0.0.1:7788"),
            Some("same-origin"),
            host
        ));
        assert!(same_origin(Some("http://127.0.0.1:7788"), None, host));
        assert!(
            !same_origin(None, Some("same-origin"), host),
            "no Origin at all"
        );
        assert!(
            !same_origin(Some("http://localhost:7788"), None, host),
            "the other name is another origin"
        );
        assert!(!same_origin(Some("https://evil.example"), None, host));
        assert!(
            !same_origin(Some("null"), None, host),
            "a sandboxed frame or a file page"
        );
        assert!(!same_origin(
            Some("http://127.0.0.1:7788"),
            Some("cross-site"),
            host
        ));
        assert!(!same_origin(
            Some("http://127.0.0.1:7788"),
            Some("same-site"),
            host
        ));
    }

    #[test]
    fn the_token_is_read_from_a_bearer_header_only() {
        assert_eq!(bearer(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer(Some("Bearer ")), None);
        assert_eq!(bearer(Some("Basic abc")), None);
        assert_eq!(bearer(None), None);
    }

    #[test]
    fn random_codes_are_long_and_do_not_repeat() {
        let a = random_hex(32);
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_hex(32));
    }
}
