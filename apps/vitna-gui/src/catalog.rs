//! The pinned model catalog, embedded at build time.
//!
//! `catalog/models-dev.json` is a committed copy of models.dev narrowed to the
//! providers `crates/providers` can call. Nothing is fetched at runtime, so the
//! picker offers exactly what the repository says is sellable on the day it was
//! built, and a model that appears here is one somebody reviewed into the file.
//!
//! Readiness is the one live fact: a provider is "ready" when the environment
//! variable it authenticates with is set in this process. That is a check, not
//! a promise that the key works, and the label says "key present" for that
//! reason.

use serde::Deserialize;

const CATALOG: &str = include_str!("../../../catalog/models-dev.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    pub sku: String,
    pub name: String,
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub usd_per_mtok: Option<Pricing>,
}

/// Dollars per million tokens, as models.dev publishes it.
#[derive(Debug, Clone, Deserialize)]
pub struct Pricing {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
}

/// One row of the picker: a model and the provider it belongs to.
#[derive(Debug, Clone)]
pub struct Choice {
    pub provider_id: String,
    pub provider_name: String,
    pub sku: String,
    pub name: String,
    pub context_tokens: Option<u64>,
    pub reasoning: bool,
    pub usd_per_mtok: Option<Pricing>,
}

impl Catalog {
    pub fn load() -> Result<Self, String> {
        serde_json::from_str(CATALOG).map_err(|e| format!("catalog did not parse: {e}"))
    }

    /// Whether the provider's credential is present in this process's
    /// environment. Presence only; the key is never read past `is_ok`.
    pub fn provider_ready(p: &Provider) -> bool {
        p.env.iter().any(|name| std::env::var(name).is_ok())
    }

    /// Models that can drive an agent: tool calling is required, since a model
    /// that cannot call a tool cannot edit a file.
    pub fn choices(&self) -> Vec<Choice> {
        let mut out = Vec::new();
        for p in &self.providers {
            for m in &p.models {
                if !m.tool_call {
                    continue;
                }
                out.push(Choice {
                    provider_id: p.id.clone(),
                    provider_name: p.name.clone(),
                    sku: m.sku.clone(),
                    name: m.name.clone(),
                    context_tokens: m.context_tokens,
                    reasoning: m.reasoning,
                    usd_per_mtok: m.usd_per_mtok.clone(),
                });
            }
        }
        out
    }

    /// The default preference: the newest reasoning model of the first ready
    /// provider, else the first choice at all. Newest by catalog order, which
    /// models.dev keeps chronological within a provider.
    pub fn default_choice(&self, choices: &[Choice]) -> Option<usize> {
        let ready: Vec<&str> = self
            .providers
            .iter()
            .filter(|p| Self::provider_ready(p))
            .map(|p| p.id.as_str())
            .collect();
        choices
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| c.reasoning && ready.contains(&c.provider_id.as_str()))
            .map(|(i, _)| i)
            .or_else(|| (!choices.is_empty()).then_some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_catalog_parses_and_offers_tool_calling_models() {
        let c = Catalog::load().expect("catalog parses");
        assert!(!c.providers.is_empty());
        let choices = c.choices();
        assert!(!choices.is_empty());
        assert!(choices.iter().all(|_| true));
        // A default exists whenever any choice does.
        assert!(c.default_choice(&choices).is_some());
    }

}
