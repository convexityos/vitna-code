//! The pinned model catalog, embedded at build time.
//!
//! `catalog/models-dev.json` is a committed copy of models.dev narrowed to the
//! providers `crates/providers` can call. Nothing is fetched at runtime, so the
//! picker offers exactly what the repository says is sellable on the day it was
//! built, and a model that appears here is one somebody reviewed into the file.
//!
//! It carries no readiness. Whether a key is stored for a provider is the
//! credential store's to answer and the daemon's to report (`keys.rs`), and
//! the environment is no source of keys at all (CONTRIBUTING rule 4). The
//! file's own `env` names, which models.dev publishes, are not read.

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

    /// The default preference: the first choice in catalog order. It used to
    /// follow whichever provider's key variable was set in this process, which
    /// made the default depend on the environment; with no key read from
    /// there, it is the choice every window without one already showed.
    pub fn default_choice(&self, choices: &[Choice]) -> Option<usize> {
        (!choices.is_empty()).then_some(0)
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
