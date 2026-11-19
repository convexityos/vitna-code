//! Fail-closed broker for `net:http_fetch` egress (`docs/AUTHORITY_INVENTORY.md` section 5).
//!
//! Every tool-initiated HTTP request passes through [`EgressBroker`], which:
//! - Denies all remote destinations by default unless explicitly listed in
//!   [`EgressPolicy::allowlist`].
//! - Rejects non-HTTP(S) schemes, URLs carrying userinfo, and blanket wildcards.
//! - Resolves DNS up front, blocks link-local / cloud metadata / private IP
//!   ranges (except explicitly allowlisted loopback/literal targets), and pins
//!   the validated socket address to prevent DNS rebinding.
//! - Re-validates every redirect hop against the same policy (never blindly
//!   following redirects across domains or into internal networks).
//! - Streams response bytes with a strict size cap, refusing oversized bodies
//!   rather than hashing a prefix.
//! - Emits a structured [`EgressCallRecord`] with redacted URL (no query string
//!   or fragment), request/response SHA-256 digests, status code, byte counts,
//!   and latency so receipts record every external fetch cryptographically.

use reqwest::header::LOCATION;
use reqwest::{Method, Url};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};
use vitna_receipts::EgressCallRecord;

pub const NETWORK_DESTINATION_DENIED: &str = "NetworkDestinationDenied";
pub const DEFAULT_MAX_EGRESS_BYTES: usize = 4 * 1024 * 1024; // 4 MiB
pub const DEFAULT_EGRESS_TIMEOUT_MS: u64 = 15_000;
pub const MAX_EGRESS_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressPolicy {
    pub allowlist: Vec<String>,
    pub max_response_bytes: usize,
    pub timeout_ms: u64,
}

impl Default for EgressPolicy {
    fn default() -> Self {
        Self {
            allowlist: Vec::new(),
            max_response_bytes: DEFAULT_MAX_EGRESS_BYTES,
            timeout_ms: DEFAULT_EGRESS_TIMEOUT_MS,
        }
    }
}

impl EgressPolicy {
    pub fn with_allowlist(allowlist: Vec<String>) -> Self {
        Self {
            allowlist,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrokeredHttpResponse {
    pub status_code: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub record: EgressCallRecord,
}

/// Normalizes a hostname or bracketed IPv6 literal for case-insensitive comparison.
fn normalize_host(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('.');
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    unbracketed.to_ascii_lowercase()
}

/// Validates a single operator-supplied `--allow-egress` entry.
///
/// Rejects empty entries, schemes/paths/ports, and blanket wildcards (`*`, `*.*`).
pub fn validate_allowlist_entry(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("egress allowlist entry must be non-empty".to_string());
    }
    if trimmed.contains("://")
        || trimmed.contains('/')
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.contains('@')
        || trimmed.chars().any(|c| c.is_whitespace())
    {
        return Err(format!(
            "invalid egress allowlist host '{trimmed}': specify a bare hostname, IP literal, or '*.domain.tld'"
        ));
    }
    let normalized = normalize_host(trimmed);
    if normalized == "*" || normalized == "*.*" || normalized == "*." {
        return Err(format!(
            "refusing blanket egress allowlist wildcard '{trimmed}': name specific domains or hosts"
        ));
    }
    if let Some(suffix) = normalized.strip_prefix("*.") {
        if suffix.is_empty() || suffix.contains('*') || suffix.contains(':') || !suffix.contains('.')
        {
            return Err(format!(
                "invalid wildcard egress allowlist host '{trimmed}': expected '*.domain.tld'"
            ));
        }
        return Ok(format!("*.{suffix}"));
    }
    if normalized.contains('*') {
        return Err(format!(
            "invalid wildcard egress allowlist host '{trimmed}': wildcard is only allowed as leading '*.'"
        ));
    }
    if normalized.contains(':') && normalized.parse::<IpAddr>().is_err() {
        return Err(format!(
            "invalid egress allowlist host '{trimmed}': ports are not permitted; specify a bare hostname or IP literal"
        ));
    }
    Ok(normalized)
}

/// Checks whether `host` is permitted by `allowlist`.
pub fn host_matches_allowlist(host: &str, allowlist: &[String]) -> bool {
    let target = normalize_host(host);
    if target.is_empty() {
        return false;
    }
    for entry in allowlist {
        let Ok(pattern) = validate_allowlist_entry(entry) else {
            continue;
        };
        if let Some(suffix) = pattern.strip_prefix("*.") {
            // Subdomain-only match: `sub.example.com` matches `*.example.com`,
            // while `evil-example.com` and bare `example.com` do not.
            if target.len() > suffix.len() + 1
                && target.ends_with(suffix)
                && target.as_bytes()[target.len() - suffix.len() - 1] == b'.'
            {
                return true;
            }
        } else if target == pattern {
            return true;
        }
    }
    false
}

/// Strips query string, fragment, and userinfo from a URL before logging or
/// recording in a receipt.
pub fn redact_url(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let port_part = match url.port() {
        Some(p) => format!(":{p}"),
        None => String::new(),
    };
    format!("{}://{}{}{}", url.scheme(), host, port_part, url.path())
}

fn is_link_local_or_metadata_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 169.254.0.0/16 (link-local + AWS/GCP/Azure IMDS 169.254.169.254)
    (octets[0] == 169 && octets[1] == 254)
        // 0.0.0.0/8 (unspecified / "this network")
        || octets[0] == 0
        // 100.64.0.0/10 (CGNAT / cloud internal metadata aliases)
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        || ip.is_multicast()
        || ip.is_broadcast()
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

fn is_link_local_or_special_v6(ip: Ipv6Addr) -> bool {
    let seg0 = ip.segments()[0];
    ip.is_unspecified()
        || ip.is_multicast()
        // fe80::/10 link-local
        || (seg0 & 0xffc0) == 0xfe80
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    // fc00::/7 Unique Local Address
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn validate_resolved_ip(
    ip: IpAddr,
    host: &str,
    allowlist: &[String],
) -> Result<(), String> {
    let normalized_host = normalize_host(host);
    let ip_text = ip.to_string();
    let allowlist_names_exact_ip = allowlist.iter().any(|entry| {
        validate_allowlist_entry(entry)
            .map(|p| p == ip_text)
            .unwrap_or(false)
    });
    let allowlist_names_loopback = allowlist.iter().any(|entry| {
        validate_allowlist_entry(entry)
            .map(|p| p == "localhost" || p == "127.0.0.1" || p == "::1")
            .unwrap_or(false)
    });

    match ip {
        IpAddr::V4(v4) => {
            if is_link_local_or_metadata_v4(v4) {
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved IP {v4} for '{host}' is in a blocked link-local/metadata range"
                ));
            }
            if v4.is_loopback() {
                if allowlist_names_loopback
                    && (normalized_host == "localhost" || normalized_host == ip_text)
                {
                    return Ok(());
                }
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved loopback IP {v4} for '{host}' is not explicitly allowlisted"
                ));
            }
            if is_private_v4(v4) {
                if allowlist_names_exact_ip && normalized_host == ip_text {
                    return Ok(());
                }
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved private IP {v4} for '{host}' is blocked unless that literal IP is explicitly allowlisted"
                ));
            }
            Ok(())
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return validate_resolved_ip(IpAddr::V4(mapped), host, allowlist);
            }
            if is_link_local_or_special_v6(v6) {
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved IPv6 address {v6} for '{host}' is in a blocked link-local/special range"
                ));
            }
            if v6.is_loopback() {
                if allowlist_names_loopback
                    && (normalized_host == "localhost" || normalized_host == ip_text)
                {
                    return Ok(());
                }
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved loopback IPv6 address {v6} for '{host}' is not explicitly allowlisted"
                ));
            }
            if is_private_v6(v6) {
                if allowlist_names_exact_ip && normalized_host == ip_text {
                    return Ok(());
                }
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: resolved private IPv6 address {v6} for '{host}' is blocked unless that literal IP is explicitly allowlisted"
                ));
            }
            Ok(())
        }
    }
}

pub struct EgressBroker;

impl EgressBroker {
    /// Executes an allowlisted, DNS-pinned, redirect-validated, byte-capped
    /// `GET` or `HEAD` HTTP request and returns both the response bytes and
    /// the cryptographic [`EgressCallRecord`].
    pub async fn fetch(
        policy: &EgressPolicy,
        method_str: &str,
        raw_url: &str,
    ) -> Result<BrokeredHttpResponse, String> {
        let method_upper = method_str.trim().to_ascii_uppercase();
        let method = match method_upper.as_str() {
            "GET" => Method::GET,
            "HEAD" => Method::HEAD,
            other => {
                return Err(format!(
                    "{NETWORK_DESTINATION_DENIED}: unsupported HTTP method '{other}'; only GET and HEAD are permitted"
                ));
            }
        };

        if policy.allowlist.is_empty() {
            return Err(format!(
                "{NETWORK_DESTINATION_DENIED}: egress allowlist is empty; no remote HTTP destination is permitted for '{raw_url}'"
            ));
        }

        let mut current_url = parse_and_check_url(raw_url, &policy.allowlist)?;
        let started = Instant::now();
        let request_sha256 = hex::encode(Sha256::digest([]));

        for hop in 0..=MAX_EGRESS_REDIRECTS {
            let host = current_url
                .host_str()
                .ok_or_else(|| format!("{NETWORK_DESTINATION_DENIED}: URL has no host"))?
                .to_string();
            let port = current_url
                .port_or_known_default()
                .ok_or_else(|| format!("{NETWORK_DESTINATION_DENIED}: URL has unknown port"))?;

            let pinned_addr = resolve_and_validate(&host, port, &policy.allowlist).await?;

            // Build a single-hop client pinned to the validated socket address
            // with redirects disabled and ambient proxies ignored.
            let mut builder = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_millis(policy.timeout_ms))
                .user_agent("vitna-code-egress-broker/0.1");

            // `reqwest::ClientBuilder::resolve` pins domain names to the
            // exact validated IP; IP literals already dial directly.
            let unbracketed_host = normalize_host(&host);
            if unbracketed_host.parse::<IpAddr>().is_err() {
                builder = builder.resolve(&unbracketed_host, pinned_addr);
            }

            let client = builder
                .build()
                .map_err(|e| format!("Failed to construct HTTP egress client: {e}"))?;

            let mut response = client
                .request(method.clone(), current_url.clone())
                .send()
                .await
                .map_err(|e| {
                    format!(
                        "HTTP egress request to {} failed: {e}",
                        redact_url(&current_url)
                    )
                })?;

            let status = response.status();
            if status.is_redirection() {
                if hop == MAX_EGRESS_REDIRECTS {
                    return Err(format!(
                        "{NETWORK_DESTINATION_DENIED}: exceeded maximum of {MAX_EGRESS_REDIRECTS} HTTP redirects"
                    ));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        format!(
                            "HTTP redirect ({}) missing valid Location header",
                            status.as_u16()
                        )
                    })?;
                let next_url = current_url
                    .join(location)
                    .map_err(|e| format!("Invalid redirect Location '{location}': {e}"))?;
                current_url = parse_and_check_url(next_url.as_str(), &policy.allowlist)?;
                continue;
            }

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("Failed reading HTTP response body: {e}"))?
            {
                if body.len().saturating_add(chunk.len()) > policy.max_response_bytes {
                    return Err(format!(
                        "HTTP response from {} exceeded maximum allowed size of {} bytes; refusing truncated digest",
                        redact_url(&current_url),
                        policy.max_response_bytes
                    ));
                }
                body.extend_from_slice(&chunk);
            }

            let response_sha256 = hex::encode(Sha256::digest(&body));
            let latency_ms = started.elapsed().as_millis() as u64;
            let redacted = redact_url(&current_url);
            let record = EgressCallRecord {
                target_domain: unbracketed_host,
                method: method_upper,
                redacted_url: redacted,
                request_sha256,
                status_code: status.as_u16(),
                response_sha256,
                bytes_sent: 0,
                bytes_received: body.len() as u64,
                latency_ms,
            };

            return Ok(BrokeredHttpResponse {
                status_code: status.as_u16(),
                content_type,
                body,
                record,
            });
        }

        Err(format!(
            "{NETWORK_DESTINATION_DENIED}: redirect loop terminated without response"
        ))
    }
}

fn parse_and_check_url(raw_url: &str, allowlist: &[String]) -> Result<Url, String> {
    let url = Url::parse(raw_url)
        .map_err(|e| format!("{NETWORK_DESTINATION_DENIED}: invalid URL '{raw_url}': {e}"))?;

    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "{NETWORK_DESTINATION_DENIED}: scheme '{other}' is not permitted; only http and https are allowed"
            ));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "{NETWORK_DESTINATION_DENIED}: URLs containing embedded credentials/userinfo are prohibited"
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| format!("{NETWORK_DESTINATION_DENIED}: URL is missing a host"))?;

    if !host_matches_allowlist(host, allowlist) {
        return Err(format!(
            "{NETWORK_DESTINATION_DENIED}: host '{}' is not in the active egress allowlist",
            normalize_host(host)
        ));
    }

    Ok(url)
}

async fn resolve_and_validate(
    host: &str,
    port: u16,
    allowlist: &[String],
) -> Result<SocketAddr, String> {
    let unbracketed = normalize_host(host);
    if let Ok(ip) = unbracketed.parse::<IpAddr>() {
        validate_resolved_ip(ip, host, allowlist)?;
        return Ok(SocketAddr::new(ip, port));
    }

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((unbracketed.as_str(), port))
        .await
        .map_err(|e| format!("DNS resolution failed for '{unbracketed}': {e}"))?
        .collect();

    if addrs.is_empty() {
        return Err(format!(
            "{NETWORK_DESTINATION_DENIED}: DNS resolution returned no addresses for '{unbracketed}'"
        ));
    }

    // Every resolved IP must satisfy policy so dual-homed records cannot smuggle
    // a metadata or private address alongside a public one.
    for addr in &addrs {
        validate_resolved_ip(addr.ip(), host, allowlist)?;
    }

    Ok(addrs[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_http_once(response: String) -> (String, tokio::task::JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://127.0.0.1:{}/test?secret=should_redact", addr.port());
        let handle = tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return false;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf).await;
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
            true
        });
        (url, handle)
    }

    #[test]
    fn allowlist_validation_and_matching() {
        assert!(validate_allowlist_entry("*").is_err());
        assert!(validate_allowlist_entry("*.*").is_err());
        assert!(validate_allowlist_entry("https://example.com").is_err());
        assert!(validate_allowlist_entry("example.com/path").is_err());
        assert_eq!(
            validate_allowlist_entry("*.Docs.RS").unwrap(),
            "*.docs.rs"
        );

        let list = vec!["127.0.0.1".to_string(), "*.docs.rs".to_string()];
        assert!(host_matches_allowlist("127.0.0.1", &list));
        assert!(host_matches_allowlist("sub.docs.rs", &list));
        assert!(host_matches_allowlist("a.b.DOCS.rs", &list));
        assert!(!host_matches_allowlist("docs.rs", &list));
        assert!(!host_matches_allowlist("evil-docs.rs", &list));
        assert!(!host_matches_allowlist("example.com", &list));
    }

    #[test]
    fn metadata_and_private_ips_are_blocked() {
        let public_only = vec!["example.com".to_string()];
        assert!(validate_resolved_ip(
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            "169.254.169.254",
            &["169.254.169.254".to_string()]
        )
        .is_err());
        assert!(validate_resolved_ip(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            "example.com",
            &public_only
        )
        .is_err());
        assert!(validate_resolved_ip(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "example.com",
            &public_only
        )
        .is_err());
        assert!(validate_resolved_ip(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            "127.0.0.1",
            &["127.0.0.1".to_string()]
        )
        .is_ok());
    }

    #[tokio::test]
    async fn unallowlisted_destination_is_refused_with_zero_egress() {
        let (url, server) = spawn_http_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string(),
        )
        .await;
        let err = EgressBroker::fetch(&EgressPolicy::default(), "GET", &url)
            .await
            .expect_err("empty allowlist must deny");
        assert!(err.contains(NETWORK_DESTINATION_DENIED), "err={err}");
        server.abort();
    }

    #[tokio::test]
    async fn allowlisted_loopback_fetch_records_redacted_url_and_sha256() {
        let body = "hello from loopback";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let (url, server) = spawn_http_once(resp).await;
        let policy = EgressPolicy::with_allowlist(vec!["127.0.0.1".to_string()]);
        let fetched = EgressBroker::fetch(&policy, "GET", &url)
            .await
            .expect("allowlisted loopback succeeds");

        assert!(server.await.unwrap());
        assert_eq!(fetched.status_code, 200);
        assert_eq!(fetched.body, body.as_bytes());
        assert_eq!(fetched.record.target_domain, "127.0.0.1");
        assert_eq!(fetched.record.method, "GET");
        assert!(
            !fetched.record.redacted_url.contains("should_redact"),
            "query string must be stripped from redacted_url: {}",
            fetched.record.redacted_url
        );
        assert_eq!(
            fetched.record.response_sha256,
            hex::encode(Sha256::digest(body.as_bytes()))
        );
        assert_eq!(fetched.record.bytes_received, body.len() as u64);
    }

    #[tokio::test]
    async fn redirect_to_unallowlisted_host_is_blocked() {
        let resp = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let (url, server) = spawn_http_once(resp).await;
        let policy = EgressPolicy::with_allowlist(vec!["127.0.0.1".to_string()]);
        let err = EgressBroker::fetch(&policy, "GET", &url)
            .await
            .expect_err("redirect to metadata host must be blocked");
        assert!(server.await.unwrap());
        assert!(err.contains(NETWORK_DESTINATION_DENIED), "err={err}");
    }

    #[tokio::test]
    async fn oversized_response_is_refused_without_truncated_digest() {
        let big = "x".repeat(2048);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            big.len(),
            big
        );
        let (url, _server) = spawn_http_once(resp).await;
        let mut policy = EgressPolicy::with_allowlist(vec!["127.0.0.1".to_string()]);
        policy.max_response_bytes = 512;
        let err = EgressBroker::fetch(&policy, "GET", &url)
            .await
            .expect_err("oversized body must be refused");
        assert!(err.contains("exceeded maximum allowed size"), "err={err}");
    }
}
