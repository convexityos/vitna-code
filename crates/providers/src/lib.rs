//! Provider adapters with normalized event streaming and credential isolation.

pub mod fake;

pub use fake::{FakeProvider, FakeProviderConfig, StreamItem};

pub struct ProviderRegistry;