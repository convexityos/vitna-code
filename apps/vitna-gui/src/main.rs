//! Vitna Code, the desktop window.
//!
//! A native application rather than a page in a browser, because ADR-0005 puts
//! `vitna-coded` on an owner-only Unix socket or Windows named pipe and never on
//! loopback TCP. A web page cannot open either transport, by design, so the
//! surface that talks to the daemon has to be a process that can.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bar;
mod brand;
mod catalog;
mod chrome;
mod composer;
mod daemon;
mod icons;
mod inspector;
mod link;
mod menu;
mod prompts;
mod repo;
mod run_list;
mod run_view;
mod runs;
mod search;
mod settings;
mod sidebar;
mod stage;
mod theme;
mod workspace;

use workspace::Workspace;

fn main() -> eframe::Result<()> {
    // The workspace is the directory the window was opened on: an argument when
    // one is given, the current directory otherwise, which is how `vitna` starts
    // in a directory too.
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let workspace = Workspace::open(root);
    let title = format!("Vitna Code  {}", workspace.name);

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 780.0])
        .with_min_inner_size([880.0, 560.0])
        .with_title(title);
    // The OS takes a bitmap for the window icon; it is the upstream raster of
    // the same mark, not a second drawing.
    if let Ok(icon) = eframe::icon_data::from_png_bytes(brand::ICON_PNG) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "vitna-code",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, workspace)))),
    )
}
