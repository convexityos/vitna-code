//! Vitna TUI
//!
//! Terminal client for Vitna Code adhering to the Calm Terminal visual law:
//! - Dark only
//! - Dense but legible
//! - Zero faux chat bubbles, glassmorphism, gradients, or ambient glow
//! - Amber budget: maximum one attention signal per view

pub struct TerminalApp {
    pub is_running: bool,
}

impl Default for TerminalApp {
    fn default() -> Self {
        Self { is_running: true }
    }
}
