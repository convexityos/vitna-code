//! A credential store that lives in process memory, for tests only.
//!
//! Compiled for this crate's own tests and under the `test-support` feature,
//! which other crates enable from `[dev-dependencies]`. No shipped binary
//! enables it: every key it holds is gone when the process exits, so it can
//! stand in for a keychain in a test and cannot stand in for one in a product.

use crate::credentials::{ApiKey, CredentialError, CredentialSource, CredentialStore, ProviderId};
use std::collections::HashMap;
use std::sync::Mutex;

/// API keys in a `HashMap`, lost when the process exits. Reads nothing from
/// the environment and nothing from the operating system's credential store.
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    keys: Mutex<HashMap<ProviderId, ApiKey>>,
}

impl MemoryCredentialStore {
    /// What [`CredentialSource::name`] reports for this store.
    pub const NAME: &'static str = "in-memory test store";

    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn keys(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<ProviderId, ApiKey>>, CredentialError> {
        self.keys.lock().map_err(|_| CredentialError::StoreFailed {
            store: Self::NAME.to_string(),
            detail: "a thread panicked while holding the store's lock".to_string(),
        })
    }
}

impl CredentialSource for MemoryCredentialStore {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn api_key(&self, provider: ProviderId) -> Result<Option<ApiKey>, CredentialError> {
        Ok(self.keys()?.get(&provider).cloned())
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn set_api_key(&self, provider: ProviderId, key: &ApiKey) -> Result<(), CredentialError> {
        self.keys()?.insert(provider, key.clone());
        Ok(())
    }

    fn delete_api_key(&self, provider: ProviderId) -> Result<bool, CredentialError> {
        Ok(self.keys()?.remove(&provider).is_some())
    }
}
