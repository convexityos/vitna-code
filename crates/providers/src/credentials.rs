use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCredentials {
    pub provider: String,
    pub api_key: String,
    pub base_url: Option<String>,
}

pub struct CredentialResolver;

impl CredentialResolver {
    /// Resolves credentials for Anthropic from local environment variable ANTHROPIC_API_KEY.
    pub fn resolve_anthropic() -> Result<ProviderCredentials, String> {
        let key = env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "Missing required ANTHROPIC_API_KEY in local environment".to_string())?;

        let base_url = env::var("ANTHROPIC_BASE_URL").ok();

        Ok(ProviderCredentials {
            provider: "anthropic".to_string(),
            api_key: key,
            base_url,
        })
    }

    /// Resolves credentials for OpenAI from local environment variable OPENAI_API_KEY.
    pub fn resolve_openai() -> Result<ProviderCredentials, String> {
        let key = env::var("OPENAI_API_KEY")
            .map_err(|_| "Missing required OPENAI_API_KEY in local environment".to_string())?;

        let base_url = env::var("OPENAI_BASE_URL").ok();

        Ok(ProviderCredentials {
            provider: "openai".to_string(),
            api_key: key,
            base_url,
        })
    }

    /// Masks an API key for display in logs or receipts without exposing the secret.
    pub fn mask_key(key: &str) -> String {
        if key.len() <= 8 {
            return "********".to_string();
        }
        let prefix = &key[..4];
        let suffix = &key[key.len() - 4..];
        format!("{}...{}", prefix, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_key() {
        let key = "sk-ant-api03-abcdef1234567890";
        let masked = CredentialResolver::mask_key(key);
        assert!(masked.starts_with("sk-a..."));
        assert!(masked.ends_with("7890"));
        assert!(!masked.contains("abcdef"));
    }
}
