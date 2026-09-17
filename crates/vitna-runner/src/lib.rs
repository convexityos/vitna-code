//! Hardened execution runner for untrusted repository commands and patch operations.

pub mod journal;

pub use journal::{ActionJournal, ActionState, JournalEntry};

pub struct Runner;