//! Model names from the pinned catalog, embedded at build time.
//!
//! `catalog/models-dev.json` is the repository's reviewed copy of models.dev.
//! A receipt records a sku; this turns a sku the catalog lists into the name a
//! person knows ("Claude Fable 5.1"), and leaves any other sku exactly as the
//! receipt wrote it, since a guessed name would be a claim nobody made.

use std::sync::OnceLock;

use serde::Deserialize;

const CATALOG: &str = include_str!("../../../catalog/models-dev.json");

#[derive(Deserialize)]
struct Catalog {
    providers: Vec<Provider>,
}

#[derive(Deserialize)]
struct Provider {
    id: String,
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    sku: String,
    name: String,
}

fn catalog() -> &'static [(String, String, String)] {
    static NAMES: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        // A catalog that will not parse names nothing, and every sku shows as
        // written. That is a build defect a test below catches, not a state
        // to paper over at runtime.
        let Ok(c) = serde_json::from_str::<Catalog>(CATALOG) else {
            return Vec::new();
        };
        c.providers
            .into_iter()
            .flat_map(|p| {
                let id = p.id;
                p.models
                    .into_iter()
                    .map(move |m| (id.clone(), m.sku, m.name))
            })
            .collect()
    })
}

/// The catalog's name for `sku` under `provider`, when it lists one.
pub fn model_name(provider: &str, sku: &str) -> Option<&'static str> {
    catalog()
        .iter()
        .find(|(p, s, _)| p == provider && s == sku)
        .map(|(_, _, name)| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_catalog_parses_and_names_a_pinned_model() {
        assert!(
            !catalog().is_empty(),
            "catalog/models-dev.json did not parse"
        );
        assert_eq!(
            model_name("anthropic", "claude-fable-5-1"),
            Some("Claude Fable 5.1")
        );
    }

    #[test]
    fn a_sku_it_does_not_list_gets_no_invented_name() {
        assert_eq!(model_name("anthropic", "stub-messages-api"), None);
        // Right sku, wrong provider: not the same model.
        assert_eq!(model_name("openai", "claude-fable-5-1"), None);
    }
}
