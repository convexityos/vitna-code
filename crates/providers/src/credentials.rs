//! Provider credentials, resolved only through an explicit [`CredentialSource`].
//!
//! CONTRIBUTING's fourth rule is "Never scan the environment for API keys", and
//! nothing in this crate reads an environment variable. `ANTHROPIC_API_KEY`,
//! `OPENAI_API_KEY` and their `*_BASE_URL` companions therefore change nothing
//! a resolver returns. `tests/no_ambient_credentials.rs` sets all four and
//! checks that none of them is picked up, and
//! `tests/providers_read_no_environment.rs` fails if any source file in this
//! crate starts reading the environment at all.
//!
//! The production source is [`crate::KeyringStore`], the operating system's
//! credential store. Anything else that should supply keys (a customer
//! credential helper, workload identity) belongs here as a further
//! implementation of [`CredentialSource`]; none exists yet.
//! `MemoryCredentialStore` is a test double, compiled only for tests and
//! under the `test-support` feature.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// A model provider Vitna can hold a credential for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Anthropic,
    OpenAi,
}

impl ProviderId {
    /// Every provider, in the order a readiness report lists them.
    pub const ALL: [ProviderId; 2] = [ProviderId::Anthropic, ProviderId::OpenAi];

    /// The name the adapters report from `Provider::name`, and the account a
    /// [`crate::KeyringStore`] files the key under.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Anthropic => "anthropic",
            ProviderId::OpenAi => "openai",
        }
    }

    /// The provider with this name, if there is one.
    pub fn from_name(name: &str) -> Option<ProviderId> {
        ProviderId::ALL.into_iter().find(|p| p.as_str() == name)
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A provider API key. `Debug` never prints it, and [`ApiKey::expose`] is the
/// only way to read it, so a key reaches a log only through a call that names
/// what it is doing.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Accepts `raw` with its surrounding whitespace removed, because a key
    /// pasted or piped into a keychain tool often arrives with a trailing
    /// newline. Refuses a value that is empty after that, or that holds
    /// anything but printable ASCII: no provider issues such a key, and an
    /// HTTP header refuses one, so it could only fail later and less clearly.
    /// A non-breaking space copied from a web page is the usual culprit.
    pub fn parse(raw: &str) -> Result<ApiKey, InvalidApiKey> {
        let key = raw.trim();
        if key.is_empty() {
            return Err(InvalidApiKey("it is empty"));
        }
        if !key.chars().all(|c| c.is_ascii_graphic()) {
            return Err(InvalidApiKey(
                "it contains a space, a control character, or a character outside printable ASCII",
            ));
        }
        Ok(ApiKey(key.to_string()))
    }

    /// The key itself, for the adapter that sends it to its provider.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Why a value was refused as an API key. Carries the reason only, never the
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidApiKey(&'static str);

impl InvalidApiKey {
    pub fn reason(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for InvalidApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a usable API key: {}", self.0)
    }
}

impl std::error::Error for InvalidApiKey {}

/// Why a credential could not be resolved, stored or removed. No variant
/// carries a key, and none is built from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// The store is working and holds no key for this provider. This is the
    /// `AuthenticationRequired` failure of AUTHORITY_INVENTORY section 6: the
    /// request that needed the key fails closed.
    AuthenticationRequired { provider: ProviderId, store: String },
    /// The store holds something for this provider that cannot be an API key.
    InvalidKey {
        provider: ProviderId,
        store: String,
        reason: String,
    },
    /// The store could not be used at all: no backend on this platform, a
    /// locked keychain, no Secret Service on the session bus, access refused.
    /// There is no fallback to try instead.
    StoreFailed { store: String, detail: String },
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CredentialError::AuthenticationRequired { provider, store } => {
                write!(
                    f,
                    "authentication required: {store} holds no {provider} API key"
                )
            }
            CredentialError::InvalidKey {
                provider,
                store,
                reason,
            } => write!(
                f,
                "{store} holds a {provider} entry that is not a usable API key: {reason}"
            ),
            CredentialError::StoreFailed { store, detail } => {
                write!(f, "{store} could not be used: {detail}")
            }
        }
    }
}

impl std::error::Error for CredentialError {}

/// Somewhere API keys can be read from. The one production implementation is
/// [`crate::KeyringStore`].
///
/// Every call may block on the operating system (a keychain unlock, a D-Bus
/// round trip), so async code should make it from
/// `tokio::task::spawn_blocking`.
pub trait CredentialSource: Send + Sync {
    /// The store's name, for people: "Windows Credential Manager". It is what
    /// a readiness report shows as a provider's credential source and what
    /// errors name, so it reads as a label, and it is never secret.
    fn name(&self) -> &str;

    /// The key stored for `provider`, or `None` when the store is working and
    /// holds none. Never falls back to anywhere else.
    fn api_key(&self, provider: ProviderId) -> Result<Option<ApiKey>, CredentialError>;
}

/// A [`CredentialSource`] that can also be written. A credential helper or
/// workload identity would be a source only.
pub trait CredentialStore: CredentialSource {
    /// Stores `key` for `provider`, replacing any key already there.
    fn set_api_key(&self, provider: ProviderId, key: &ApiKey) -> Result<(), CredentialError>;

    /// Removes the key for `provider`. `Ok(false)` means none was stored.
    fn delete_api_key(&self, provider: ProviderId) -> Result<bool, CredentialError>;
}

/// What an adapter is built with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCredentials {
    pub provider: String,
    pub api_key: String,
    /// Where requests go instead of the provider's public endpoint. A resolver
    /// always leaves this `None`: an override comes only from configuration
    /// the caller passes explicitly, never from `ANTHROPIC_BASE_URL` or
    /// `OPENAI_BASE_URL`, because whatever sets it decides where the key is
    /// sent.
    pub base_url: Option<String>,
}

impl fmt::Debug for ProviderCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderCredentials")
            .field("provider", &self.provider)
            .field("api_key", &CredentialResolver::mask_key(&self.api_key))
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Resolves provider credentials from one explicit [`CredentialSource`], and
/// from nowhere else.
#[derive(Clone)]
pub struct CredentialResolver {
    source: Arc<dyn CredentialSource>,
}

impl CredentialResolver {
    pub fn new(source: Arc<dyn CredentialSource>) -> Self {
        Self { source }
    }

    /// The name of the store this resolver reads, for display.
    pub fn source_name(&self) -> &str {
        self.source.name()
    }

    /// The credentials for `provider`. A key missing from the source is
    /// [`CredentialError::AuthenticationRequired`], and nothing else is asked.
    pub fn resolve(&self, provider: ProviderId) -> Result<ProviderCredentials, CredentialError> {
        let key = self.stored_key(provider)?;
        Ok(ProviderCredentials {
            provider: provider.as_str().to_string(),
            api_key: key.expose().to_string(),
            base_url: None,
        })
    }

    /// Whether `provider` could be resolved right now, without handing the
    /// key to the caller. The key is still read, because the platform stores
    /// offer no cheaper way to find out that a usable one is there.
    pub fn check(&self, provider: ProviderId) -> Result<(), CredentialError> {
        self.stored_key(provider).map(|_| ())
    }

    fn stored_key(&self, provider: ProviderId) -> Result<ApiKey, CredentialError> {
        self.source
            .api_key(provider)?
            .ok_or_else(|| CredentialError::AuthenticationRequired {
                provider,
                store: self.source.name().to_string(),
            })
    }

    /// Masks an API key for display in logs or receipts without exposing the secret.
    ///
    /// Counts characters rather than bytes: `ProviderCredentials`'s `Debug`
    /// calls this, and byte slicing panicked on a key with a multi-byte
    /// character near either end, which would turn logging an error into a
    /// crash.
    pub fn mask_key(key: &str) -> String {
        let chars: Vec<char> = key.chars().collect();
        if chars.len() <= 8 {
            return "********".to_string();
        }
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{}...{}", prefix, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryCredentialStore;

    fn key(raw: &str) -> ApiKey {
        ApiKey::parse(raw).expect("a valid test key")
    }

    #[test]
    fn test_mask_key() {
        let key = "sk-ant-api03-abcdef1234567890";
        let masked = CredentialResolver::mask_key(key);
        assert!(masked.starts_with("sk-a..."));
        assert!(masked.ends_with("7890"));
        assert!(!masked.contains("abcdef"));
    }

    #[test]
    fn a_missing_key_is_authentication_required_naming_the_store() {
        let resolver = CredentialResolver::new(Arc::new(MemoryCredentialStore::new()));
        for provider in ProviderId::ALL {
            assert_eq!(
                resolver.resolve(provider),
                Err(CredentialError::AuthenticationRequired {
                    provider,
                    store: MemoryCredentialStore::NAME.to_string(),
                })
            );
            assert_eq!(
                resolver.check(provider),
                Err(CredentialError::AuthenticationRequired {
                    provider,
                    store: MemoryCredentialStore::NAME.to_string(),
                })
            );
        }
    }

    #[test]
    fn a_stored_key_resolves_for_its_own_provider_only() {
        let store = Arc::new(MemoryCredentialStore::new());
        store
            .set_api_key(ProviderId::Anthropic, &key("sk-ant-stored-0001"))
            .unwrap();
        let resolver = CredentialResolver::new(store);

        let creds = resolver.resolve(ProviderId::Anthropic).unwrap();
        assert_eq!(creds.provider, "anthropic");
        assert_eq!(creds.api_key, "sk-ant-stored-0001");
        assert_eq!(creds.base_url, None);
        assert_eq!(resolver.check(ProviderId::Anthropic), Ok(()));

        assert!(matches!(
            resolver.resolve(ProviderId::OpenAi),
            Err(CredentialError::AuthenticationRequired {
                provider: ProviderId::OpenAi,
                ..
            })
        ));
    }

    #[test]
    fn a_deleted_key_stops_resolving() {
        let store = Arc::new(MemoryCredentialStore::new());
        store
            .set_api_key(ProviderId::OpenAi, &key("sk-proj-stored-0002"))
            .unwrap();
        assert_eq!(store.delete_api_key(ProviderId::OpenAi), Ok(true));
        assert_eq!(store.delete_api_key(ProviderId::OpenAi), Ok(false));

        let resolver = CredentialResolver::new(store);
        assert!(matches!(
            resolver.resolve(ProviderId::OpenAi),
            Err(CredentialError::AuthenticationRequired { .. })
        ));
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        assert_eq!(key("  sk-ant-abc123\r\n").expose(), "sk-ant-abc123");
    }

    #[test]
    fn parse_refuses_what_cannot_be_a_key() {
        for raw in [
            "",
            " \n\t ",
            "sk-ant abc",
            "sk-ant-\nabc",
            "sk-ant-\u{7}abc",
            "sk-ant-\u{a0}abc",
            "sk-ant-\u{e9}abc",
        ] {
            assert!(ApiKey::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn mask_key_counts_characters_not_bytes() {
        // Byte slicing put index 4 inside the second character here and panicked.
        let key = "a\u{e9}\u{e9}bcdefghij\u{e9}";
        assert_eq!(
            CredentialResolver::mask_key(key),
            "a\u{e9}\u{e9}b...hij\u{e9}"
        );
        let creds = ProviderCredentials {
            provider: "openai".to_string(),
            api_key: key.to_string(),
            base_url: None,
        };
        assert!(format!("{creds:?}").contains("..."));
    }

    #[test]
    fn debug_output_never_carries_the_key() {
        let secret = "sk-ant-api03-SECRETSECRETSECRET-tail";
        assert_eq!(format!("{:?}", key(secret)), "ApiKey(<redacted>)");

        let creds = ProviderCredentials {
            provider: "anthropic".to_string(),
            api_key: secret.to_string(),
            base_url: None,
        };
        let shown = format!("{creds:?}");
        assert!(!shown.contains("SECRETSECRETSECRET"), "{shown}");
        assert!(shown.contains("sk-a...tail"), "{shown}");
    }

    #[test]
    fn errors_name_the_provider_and_store_and_carry_no_key() {
        let err = CredentialError::AuthenticationRequired {
            provider: ProviderId::OpenAi,
            store: "Windows Credential Manager".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "authentication required: Windows Credential Manager holds no openai API key"
        );
    }

    #[test]
    fn provider_names_round_trip() {
        for provider in ProviderId::ALL {
            assert_eq!(ProviderId::from_name(provider.as_str()), Some(provider));
        }
        assert_eq!(ProviderId::from_name("not-a-provider"), None);
        assert_eq!(ProviderId::from_name("Anthropic"), None);
        assert_eq!(ProviderId::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderId::OpenAi.to_string(), "openai");
    }
}
