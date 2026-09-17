//! Main entrypoint for Vitna TUI

use vitna_tui::TerminalApp;

fn main() {
    let app = TerminalApp::default();
    println!("Vitna TUI initialized. Running: {}", app.is_running);
}
