//! A vitna-runtime lane, reached on this machine.
//!
//! `vitna-runtime` executes a signed routing policy on the operator's own
//! keys and speaks the OpenAI Chat Completions dialect. Pointing the daemon at
//! one of its lanes means every model call Vitna Code makes is routed and
//! ledgered by the runtime, rather than by a second client holding provider
//! keys of its own.
//!
//! Loopback only. A runtime listening on loopback asks for no key, and one
//! listening anywhere else refuses every request without
//! `VITNA_INBOUND_KEY`. The daemon has no place to keep that key yet:
//! CONTRIBUTING's fourth rule forbids reading it from the environment, and the
//! keychain holds provider keys by `ProviderId`, which names no runtime. So a
//! remote address is refused when the route is formed, rather than failing on
//! the first turn with a 401 that reads like a bad provider key.

use async_trait::async_trait;

use crate::credentials::ProviderCredentials;
use crate::openai::OpenAIProvider;
use crate::{Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent};

/// The name a receipt records for a turn served through a lane.
pub const RUNTIME_PROVIDER_NAME: &str = "vitna-runtime";

/// Where the runtime answers, and which lane the daemon asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLane {
    /// The Chat Completions endpoint, always on a loopback host.
    pub endpoint: String,
    /// What goes in the request's `model` field: a lane alias such as
    /// `vitna/code`, or a model name the runtime passes through.
    pub lane: String,
}

impl RuntimeLane {
    /// `url` may be the runtime's origin (`http://127.0.0.1:8787`), its `/v1`
    /// base, or the full `/v1/chat/completions` path; all three name the same
    /// endpoint. Refuses a host that is not loopback, a scheme that is not
    /// http or https, and an empty lane.
    pub fn new(url: &str, lane: &str) -> Result<RuntimeLane, String> {
        let lane = lane.trim();
        if lane.is_empty() {
            return Err("a runtime lane needs a name, such as vitna/code".to_string());
        }
        let parsed = reqwest::Url::parse(url.trim())
            .map_err(|e| format!("{url} is not a URL the runtime could be reached at: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("{url} is not an http or https address"));
        }
        // `host_str` brackets an IPv6 literal; the address parser does not.
        let loopback = match parsed.host_str() {
            Some(host) => {
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                match bare.parse::<std::net::IpAddr>() {
                    Ok(ip) => ip.is_loopback(),
                    Err(_) => bare.eq_ignore_ascii_case("localhost"),
                }
            }
            None => false,
        };
        if !loopback {
            return Err(format!(
                "{url} is not on this machine. The daemon reaches a runtime on loopback only, \
                 since one listening anywhere else needs VITNA_INBOUND_KEY and the daemon has \
                 nowhere to keep it"
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(format!(
                "{url} carries a query or fragment the runtime does not read"
            ));
        }

        let path = parsed.path().trim_end_matches('/');
        let path = if path.ends_with("/chat/completions") {
            path.to_string()
        } else if path.ends_with("/v1") {
            format!("{path}/chat/completions")
        } else {
            format!("{path}/v1/chat/completions")
        };
        let mut endpoint = parsed.clone();
        endpoint.set_path(&path);
        Ok(RuntimeLane {
            endpoint: endpoint.to_string(),
            lane: lane.to_string(),
        })
    }
}

/// The OpenAI-dialect adapter, sent to a lane and named for it.
///
/// A separate type rather than an `OpenAIProvider` with a base URL, because
/// `name()` is what the receipt records as the provider, and a turn the
/// runtime served must not be signed as one OpenAI did.
pub struct RuntimeLaneProvider {
    lane: RuntimeLane,
    inner: OpenAIProvider,
}

impl RuntimeLaneProvider {
    pub fn new(lane: RuntimeLane) -> Self {
        let inner = OpenAIProvider::new(ProviderCredentials {
            provider: RUNTIME_PROVIDER_NAME.to_string(),
            // None: see the module comment. The adapter sends no
            // Authorization header when the key is empty.
            api_key: String::new(),
            base_url: Some(lane.endpoint.clone()),
        });
        Self { lane, inner }
    }

    pub fn lane(&self) -> &RuntimeLane {
        &self.lane
    }
}

#[async_trait]
impl Provider for RuntimeLaneProvider {
    fn name(&self) -> &str {
        RUNTIME_PROVIDER_NAME
    }

    fn parse_sse_event(
        &self,
        event_type: &str,
        data: &str,
    ) -> Result<Vec<ProviderStreamEvent>, String> {
        self.inner.parse_sse_event(event_type, data)
    }

    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.inner
            .complete(req)
            .await
            .map_err(|e| format!("the runtime at {} answered: {e}", self.lane.endpoint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_spellings_of_one_runtime_name_one_endpoint() {
        for url in [
            "http://127.0.0.1:8787",
            "http://127.0.0.1:8787/",
            "http://127.0.0.1:8787/v1",
            "http://127.0.0.1:8787/v1/",
            "http://127.0.0.1:8787/v1/chat/completions",
        ] {
            let lane = RuntimeLane::new(url, "vitna/code").expect(url);
            assert_eq!(
                lane.endpoint, "http://127.0.0.1:8787/v1/chat/completions",
                "{url}"
            );
            assert_eq!(lane.lane, "vitna/code");
        }
        assert_eq!(
            RuntimeLane::new("http://localhost:9000", "vitna/code")
                .unwrap()
                .endpoint,
            "http://localhost:9000/v1/chat/completions"
        );
        assert_eq!(
            RuntimeLane::new("http://[::1]:8787/v1", "vitna/code")
                .unwrap()
                .endpoint,
            "http://[::1]:8787/v1/chat/completions"
        );
    }

    #[test]
    fn a_runtime_off_this_machine_is_refused_when_the_route_is_formed() {
        for url in [
            "http://10.0.0.5:8787",
            "https://runtime.example.com",
            "http://0.0.0.0:8787",
            "http://localhost.example.com:8787",
        ] {
            let err = RuntimeLane::new(url, "vitna/code").expect_err(url);
            assert!(err.contains("not on this machine"), "{url}: {err}");
        }
    }

    #[test]
    fn a_route_that_cannot_be_read_is_refused_rather_than_guessed() {
        assert!(RuntimeLane::new("127.0.0.1:8787", "vitna/code").is_err());
        assert!(RuntimeLane::new("ftp://127.0.0.1:8787", "vitna/code").is_err());
        assert!(RuntimeLane::new("http://127.0.0.1:8787?lane=x", "vitna/code").is_err());
        assert!(RuntimeLane::new("http://127.0.0.1:8787", "  ").is_err());
    }

    /// The receipt names the provider from `name()`, so this is the claim a
    /// signed run makes about who served it.
    #[test]
    fn a_lane_is_named_for_the_runtime_and_not_for_openai() {
        let provider = RuntimeLaneProvider::new(
            RuntimeLane::new("http://127.0.0.1:8787", "vitna/code").unwrap(),
        );
        assert_eq!(provider.name(), RUNTIME_PROVIDER_NAME);
        assert_eq!(
            provider.inner.default_endpoint(),
            "http://127.0.0.1:8787/v1/chat/completions"
        );
        assert!(provider.inner.credentials.api_key.is_empty());
    }
}
