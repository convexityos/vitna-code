//! Vitna Code's terminal client.
//!
//! It wears the desktop window's material (`theme`), prints the web client's
//! words for every state (`words`), and says only what it can establish: the
//! folder and its branch, whether a daemon answers the declared handshake, and
//! the runs this folder has receipts for, each read through the offline
//! verifier. It starts no turns, because nothing on the wire takes one yet,
//! and it does not pretend to: the client it replaces answered every prompt
//! with a canned reply, a diff and a receipt it made up.

pub mod app;
pub mod catalog;
pub mod ledger;
pub mod link;
pub mod place;
pub mod text;
pub mod theme;
pub mod turn;
pub mod view;
pub mod words;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod preview;

pub use app::{App, Ask, Screen, Update};
