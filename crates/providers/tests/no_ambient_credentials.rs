//! CONTRIBUTING rule 4: "Never scan the environment for API keys."
//!
//! Plants every environment variable the old resolver and the old
//! `KeyringStore` read, then checks that no production path hands any of them
//! back. It is ONE test function on purpose: it changes the process
//! environment, and a second test in this binary could be reading the
//! environment on another thread while it does.

use std::sync::Arc;
use vitna_providers::{
    ApiKey, CredentialError, CredentialResolver, CredentialSource, CredentialStore, KeyringStore,
    ProviderCredentials, ProviderId,
};

const PLANTED: [(&str, &str); 4] = [
    ("ANTHROPIC_API_KEY", "sk-ant-PLANTED-IN-THE-ENVIRONMENT"),
    ("OPENAI_API_KEY", "sk-proj-PLANTED-IN-THE-ENVIRONMENT"),
    ("ANTHROPIC_BASE_URL", "https://planted.invalid/anthropic"),
    ("OPENAI_BASE_URL", "https://planted.invalid/openai"),
];

/// A working source holding at most one key, standing in for any store.
struct Holds(Option<(ProviderId, ApiKey)>);

impl CredentialSource for Holds {
    fn name(&self) -> &str {
        "a test source"
    }

    fn api_key(&self, provider: ProviderId) -> Result<Option<ApiKey>, CredentialError> {
        Ok(self
            .0
            .as_ref()
            .filter(|(held, _)| *held == provider)
            .map(|(_, key)| key.clone()))
    }
}

/// Removes whatever the keychain half stored, including when it fails part way.
struct Scrub(Arc<KeyringStore>);

impl Drop for Scrub {
    fn drop(&mut self) {
        for provider in ProviderId::ALL {
            let _ = self.0.delete_api_key(provider);
        }
    }
}

fn assert_nothing_planted(creds: &ProviderCredentials) {
    for (name, value) in PLANTED {
        assert_ne!(creds.api_key, value, "the API key came from {name}");
        assert_ne!(
            creds.base_url.as_deref(),
            Some(value),
            "the base URL came from {name}"
        );
    }
}

fn assert_authentication_required(
    resolver: &CredentialResolver,
    provider: ProviderId,
    store: &str,
) {
    let expected = Err(CredentialError::AuthenticationRequired {
        provider,
        store: store.to_string(),
    });
    assert_eq!(resolver.resolve(provider), expected, "resolve({provider})");
    assert_eq!(
        resolver.check(provider),
        expected.map(|_: ProviderCredentials| ()),
        "check({provider})"
    );
}

#[test]
fn no_production_path_reads_a_credential_from_the_environment() {
    for (name, value) in PLANTED {
        std::env::set_var(name, value);
    }
    // The plant took, so every pass below means "ignored" and never "unset".
    for (name, value) in PLANTED {
        assert_eq!(std::env::var(name).as_deref(), Ok(value));
    }

    // 1. A resolver over a source that holds nothing refuses every provider,
    //    with a planted key sitting in the environment for each of them.
    let resolver = CredentialResolver::new(Arc::new(Holds(None)));
    for provider in ProviderId::ALL {
        assert_authentication_required(&resolver, provider, "a test source");
    }

    // 2. A key the source does hold is the key that resolves, with no base
    //    URL, while the environment offers a different key and a base URL.
    let held = ApiKey::parse("sk-ant-FROM-THE-SOURCE").unwrap();
    let resolver = CredentialResolver::new(Arc::new(Holds(Some((ProviderId::Anthropic, held)))));
    let creds = resolver.resolve(ProviderId::Anthropic).unwrap();
    assert_eq!(creds.api_key, "sk-ant-FROM-THE-SOURCE");
    assert_eq!(creds.base_url, None);
    assert_nothing_planted(&creds);
    assert_authentication_required(&resolver, ProviderId::OpenAi, "a test source");

    // 3. The production store: the operating system's credential store, under
    //    a service name nobody's real keys are filed under.
    let service = format!(
        "vitna-code-test-no-ambient-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let store = match KeyringStore::open_with_service(&service) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            // Windows and macOS always give a signed-in user a store, so there
            // an error is a failure. Elsewhere the store can be absent (Linux
            // with no Secret Service, as on CI runners), and what matters is
            // that the absence is an error, not a quiet fallback to the
            // planted keys.
            if cfg!(any(target_os = "windows", target_os = "macos")) {
                panic!("the platform credential store should open here: {err}");
            }
            assert!(
                matches!(err, CredentialError::StoreFailed { .. }),
                "an unavailable store should fail as StoreFailed, not {err:?}"
            );
            // The repository's convention, so `grep skipped:` finds the part
            // that did not run; the assertions above it did run.
            eprintln!(
                "skipped: the keychain round trip, because {err}. The store's absence \
                 was checked to be an error rather than an environment fallback."
            );
            return;
        }
    };
    let _scrub = Scrub(Arc::clone(&store));
    let name = store.name().to_string();

    for provider in ProviderId::ALL {
        assert_eq!(store.api_key(provider), Ok(None), "{provider} in {name}");
    }
    let resolver = CredentialResolver::new(store.clone());
    for provider in ProviderId::ALL {
        assert_authentication_required(&resolver, provider, &name);
    }

    // A key stored in the keychain is the one that resolves, not the planted
    // one, and the other provider stays unresolved beside its planted key.
    let stored = ApiKey::parse("sk-ant-FROM-THE-KEYCHAIN").unwrap();
    store.set_api_key(ProviderId::Anthropic, &stored).unwrap();
    let creds = resolver.resolve(ProviderId::Anthropic).unwrap();
    assert_eq!(creds.api_key, "sk-ant-FROM-THE-KEYCHAIN");
    assert_eq!(creds.base_url, None);
    assert_nothing_planted(&creds);
    assert_authentication_required(&resolver, ProviderId::OpenAi, &name);
}
