//! The operating system's credential store, as a [`CredentialStore`].

use crate::credentials::{ApiKey, CredentialError, CredentialSource, CredentialStore, ProviderId};
use keyring_core::{Entry, Error as StoreError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The service every Vitna Code API key is filed under.
pub const KEYCHAIN_SERVICE: &str = "vitna-code";

/// API keys held in the operating system's own credential store, one entry per
/// provider:
///
/// - Windows: a generic credential in the signed-in user's Credential Manager,
///   target `vitna-code:<provider>`, written with local-machine persistence so
///   it does not roam to other machines with the user's profile.
/// - macOS: a generic password in the user's login keychain, service
///   `vitna-code`, account `<provider>`.
/// - Linux and the BSDs: an item in the Secret Service's default collection
///   (GNOME Keyring, KeePassXC, or KWallet's Secret Service interface), with
///   attributes `service=vitna-code` and `username=<provider>`. With no Secret
///   Service on the session bus, [`KeyringStore::open`] fails.
///
/// `<provider>` is `anthropic` or `openai` ([`ProviderId::as_str`]). Those are
/// the entries the platform's own tools create, so a key can be stored before
/// any Vitna command exists to do it:
///
/// ```text
/// cmdkey /generic:vitna-code:anthropic /user:anthropic /pass
/// security add-generic-password -s vitna-code -a anthropic -w
/// secret-tool store --label="Vitna Code API key (anthropic)" service vitna-code username anthropic
/// ```
///
/// Each prompts for the key rather than taking it as an argument, which keeps
/// it out of shell history.
///
/// What this store does not do: it never falls back to anything when the
/// platform store has no key or cannot be reached (no environment variable,
/// no file, no memory copy), and it keeps nothing in memory between calls. It
/// does not decide where a key is sent; the adapter it is handed to sends it to
/// that provider, which is what the key is for. Every call blocks on the
/// operating system (a keychain unlock, a D-Bus round trip), so async code
/// should make it from `tokio::task::spawn_blocking`. Calls on one
/// `KeyringStore` run one at a time, because the Windows store documents that
/// concurrent operations on one entry can complete out of order.
pub struct KeyringStore {
    store: Arc<keyring_core::CredentialStore>,
    service: String,
    serial: Mutex<()>,
}

impl KeyringStore {
    /// Opens this platform's credential store, filing keys under
    /// [`KEYCHAIN_SERVICE`].
    pub fn open() -> Result<KeyringStore, CredentialError> {
        Self::open_with_service(KEYCHAIN_SERVICE)
    }

    /// Opens this platform's credential store, filing keys under `service`
    /// instead. Tests use it so they never touch an operator's real keys.
    pub fn open_with_service(service: &str) -> Result<KeyringStore, CredentialError> {
        if service.is_empty() {
            return Err(failed("the service name is empty".to_string()));
        }
        let store = platform::open().map_err(|e| failed(describe(&e)))?;
        Ok(KeyringStore {
            store,
            service: service.to_string(),
            serial: Mutex::new(()),
        })
    }

    /// The service this store files keys under.
    pub fn service(&self) -> &str {
        &self.service
    }

    fn entry(&self, provider: ProviderId) -> Result<Entry, CredentialError> {
        platform::entry(&*self.store, &self.service, provider).map_err(|e| failed(describe(&e)))
    }

    /// Held for the length of each call. It guards no data, so a thread that
    /// panicked while holding it leaves nothing inconsistent behind.
    fn one_at_a_time(&self) -> MutexGuard<'_, ()> {
        self.serial.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl CredentialSource for KeyringStore {
    fn name(&self) -> &str {
        platform::NAME
    }

    fn api_key(&self, provider: ProviderId) -> Result<Option<ApiKey>, CredentialError> {
        let _serial = self.one_at_a_time();
        match self.entry(provider)?.get_password() {
            Ok(raw) => ApiKey::parse(&raw)
                .map(Some)
                .map_err(|e| invalid(provider, e.reason().to_string())),
            Err(StoreError::NoEntry) => Ok(None),
            // The attached bytes are the stored secret, so they are dropped
            // here unread rather than described.
            Err(StoreError::BadEncoding(_)) => {
                Err(invalid(provider, "it is not valid text".to_string()))
            }
            Err(StoreError::BadDataFormat(_, _)) => Err(invalid(
                provider,
                "it is stored in a form this store cannot read".to_string(),
            )),
            Err(e) => Err(failed(describe(&e))),
        }
    }
}

impl CredentialStore for KeyringStore {
    fn set_api_key(&self, provider: ProviderId, key: &ApiKey) -> Result<(), CredentialError> {
        let _serial = self.one_at_a_time();
        self.entry(provider)?
            .set_password(key.expose())
            .map_err(|e| failed(describe(&e)))
    }

    fn delete_api_key(&self, provider: ProviderId) -> Result<bool, CredentialError> {
        let _serial = self.one_at_a_time();
        match self.entry(provider)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(StoreError::NoEntry) => Ok(false),
            Err(e) => Err(failed(describe(&e))),
        }
    }
}

fn failed(detail: String) -> CredentialError {
    CredentialError::StoreFailed {
        store: platform::NAME.to_string(),
        detail,
    }
}

fn invalid(provider: ProviderId, reason: String) -> CredentialError {
    CredentialError::InvalidKey {
        provider,
        store: platform::NAME.to_string(),
        reason,
    }
}

/// A store error in words. Never `{:?}`: the `Debug` form of `BadEncoding` and
/// `BadDataFormat` prints the stored bytes, which are the secret. Their
/// `Display` forms print none, but they are spelled out here anyway, so the
/// guarantee does not rest on a dependency's formatting.
fn describe(err: &StoreError) -> String {
    match err {
        StoreError::BadEncoding(_) => "a stored value is not valid text".to_string(),
        StoreError::BadDataFormat(_, _) => {
            "a stored value is in a form this store cannot read".to_string()
        }
        StoreError::Ambiguous(entries) => format!(
            "{} entries match, so which one to use is ambiguous",
            entries.len()
        ),
        other => other.to_string(),
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::ProviderId;
    use keyring_core::{CredentialStore, Entry, Result};
    use std::collections::HashMap;
    use std::sync::Arc;

    pub(super) const NAME: &str = "Windows Credential Manager";

    pub(super) fn open() -> Result<Arc<CredentialStore>> {
        Ok(windows_native_keyring_store::Store::new()?)
    }

    pub(super) fn entry(
        store: &CredentialStore,
        service: &str,
        provider: ProviderId,
    ) -> Result<Entry> {
        let target = format!("{service}:{}", provider.as_str());
        let modifiers = HashMap::from([("target", target.as_str()), ("persistence", "Local")]);
        store.build(service, provider.as_str(), Some(&modifiers))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ProviderId;
    use keyring_core::{CredentialStore, Entry, Result};
    use std::sync::Arc;

    pub(super) const NAME: &str = "macOS Keychain";

    pub(super) fn open() -> Result<Arc<CredentialStore>> {
        Ok(apple_native_keyring_store::keychain::Store::new()?)
    }

    pub(super) fn entry(
        store: &CredentialStore,
        service: &str,
        provider: ProviderId,
    ) -> Result<Entry> {
        store.build(service, provider.as_str(), None)
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
mod platform {
    use super::ProviderId;
    use keyring_core::{CredentialStore, Entry, Result};
    use std::collections::HashMap;
    use std::sync::Arc;

    pub(super) const NAME: &str = "Secret Service";

    pub(super) fn open() -> Result<Arc<CredentialStore>> {
        Ok(zbus_secret_service_keyring_store::Store::new()?)
    }

    pub(super) fn entry(
        store: &CredentialStore,
        service: &str,
        provider: ProviderId,
    ) -> Result<Entry> {
        let label = format!("Vitna Code API key ({})", provider.as_str());
        let modifiers = HashMap::from([("label", label.as_str())]);
        store.build(service, provider.as_str(), Some(&modifiers))
    }
}

#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
)))]
mod platform {
    use super::ProviderId;
    use keyring_core::{CredentialStore, Entry, Error, Result};
    use std::sync::Arc;

    pub(super) const NAME: &str = "OS credential store";

    pub(super) fn open() -> Result<Arc<CredentialStore>> {
        Err(Error::NotSupportedByStore(
            "Vitna Code has no credential store backend for this platform".to_string(),
        ))
    }

    pub(super) fn entry(_: &CredentialStore, _: &str, _: ProviderId) -> Result<Entry> {
        unreachable!("open() fails on this platform, so no KeyringStore exists to build an entry")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A service name no operator's keys live under, so these tests read and
    /// write only entries they created.
    fn scratch_service(test: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("vitna-code-test-{test}-{}-{nanos}", std::process::id())
    }

    /// Removes whatever a test stored, including when it fails part way.
    struct Scrub<'a>(&'a KeyringStore);

    impl Drop for Scrub<'_> {
        fn drop(&mut self) {
            for provider in ProviderId::ALL {
                let _ = self.0.delete_api_key(provider);
            }
        }
    }

    #[test]
    fn an_empty_service_is_refused() {
        assert!(matches!(
            KeyringStore::open_with_service(""),
            Err(CredentialError::StoreFailed { .. })
        ));
    }

    // Windows and macOS give every signed-in user a credential store, so
    // there this test runs and must pass. Linux has one only where a Secret
    // Service is running, which CI runners do not start; there it is ignored,
    // visibly, and `cargo test -- --ignored` runs it on a desktop session.
    #[test]
    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos")),
        ignore = "needs a Secret Service on the session bus"
    )]
    fn a_key_round_trips_through_the_operating_system_store() {
        let store = KeyringStore::open_with_service(&scratch_service("round-trip"))
            .expect("the platform credential store should open");
        let _scrub = Scrub(&store);

        assert_eq!(store.api_key(ProviderId::Anthropic), Ok(None));

        let key = ApiKey::parse("sk-ant-keyring-round-trip-0001").unwrap();
        store.set_api_key(ProviderId::Anthropic, &key).unwrap();
        assert_eq!(store.api_key(ProviderId::Anthropic), Ok(Some(key)));
        // Another provider's entry is a different entry.
        assert_eq!(store.api_key(ProviderId::OpenAi), Ok(None));

        let replacement = ApiKey::parse("sk-ant-keyring-round-trip-0002").unwrap();
        store
            .set_api_key(ProviderId::Anthropic, &replacement)
            .unwrap();
        assert_eq!(store.api_key(ProviderId::Anthropic), Ok(Some(replacement)));

        assert_eq!(store.delete_api_key(ProviderId::Anthropic), Ok(true));
        assert_eq!(store.api_key(ProviderId::Anthropic), Ok(None));
        assert_eq!(store.delete_api_key(ProviderId::Anthropic), Ok(false));
    }

    #[test]
    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos")),
        ignore = "needs a Secret Service on the session bus"
    )]
    fn two_services_never_see_each_others_keys() {
        let first = KeyringStore::open_with_service(&scratch_service("isolation-a")).unwrap();
        let second = KeyringStore::open_with_service(&scratch_service("isolation-b")).unwrap();
        let _scrub_first = Scrub(&first);
        let _scrub_second = Scrub(&second);

        let key = ApiKey::parse("sk-proj-keyring-isolation-0001").unwrap();
        first.set_api_key(ProviderId::OpenAi, &key).unwrap();
        assert_eq!(first.api_key(ProviderId::OpenAi), Ok(Some(key)));
        assert_eq!(second.api_key(ProviderId::OpenAi), Ok(None));
    }
}
