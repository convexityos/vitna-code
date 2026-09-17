//! Hardened execution runner for untrusted repository commands and patch operations.

pub mod fake;
pub mod journal;

pub use fake::{FakeRunner, FaultInjectionMode, RunnerFault};
pub use journal::{ActionJournal, ActionState, JournalEntry};

pub struct Runner;