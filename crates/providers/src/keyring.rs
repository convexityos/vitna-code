use std::collections::HashMap;
use std::env;
use std::sync::Mutex;

/// Local OS keychain and credential store abstraction.
/// Strictly enforces the Vitna honesty contract: all secrets are customer-held
/// and never transmitted to vitna.ai or any third party.
pub struct KeyringStore {
    in_memory_vault: Mutex<HashMap<String, String>>,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self {
            in_memory_vault: Mutex::new(HashMap::new()),
        }
    }

    fn key(service: &str, account: &str) -> String {
        format!("{}:{}", service, account)
    }

    /// Stores a secret in the local vault.
    pub fn set_secret(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        let mut vault = self.in_memory_vault.lock().map_err(|e| e.to_string())?;
        vault.insert(Self::key(service, account), secret.to_string());
        Ok(())
    }

    /// Retrieves a secret from the vault, falling back to local environment variables if unset.
    pub fn get_secret(&self, service: &str, account: &str) -> Result<String, String> {
        // 1. Check in-memory vault
        {
            let vault = self.in_memory_vault.lock().map_err(|e| e.to_string())?;
            if let Some(secret) = vault.get(&Self::key(service, account)) {
                return Ok(secret.clone());
            }
        }

        // 2. Fall back to standard environment variables
        if service == "vitna" {
            match account {
                "anthropic" => {
                    if let Ok(val) = env::var("ANTHROPIC_API_KEY") {
                        return Ok(val);
                    }
                }
                "openai" => {
                    if let Ok(val) = env::var("OPENAI_API_KEY") {
                        return Ok(val);
                    }
                }
                _ => {}
            }
        }

        Err(format!("Secret for service '{}' and account '{}' not found", service, account))
    }

    /// Removes a secret from the vault.
    pub fn delete_secret(&self, service: &str, account: &str) -> Result<(), String> {
        let mut vault = self.in_memory_vault.lock().map_err(|e| e.to_string())?;
        vault.remove(&Self::key(service, account));
        Ok(())
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyring_store_lifecycle() {
        let store = KeyringStore::new();

        // 1. Store secret
        store.set_secret("vitna", "test_key", "secret_val_xyz").expect("set secret");

        // 2. Retrieve secret
        let val = store.get_secret("vitna", "test_key").expect("get secret");
        assert_eq!(val, "secret_val_xyz");

        // 3. Delete secret
        store.delete_secret("vitna", "test_key").expect("delete secret");
        assert!(store.get_secret("vitna", "test_key").is_err());
    }
}
