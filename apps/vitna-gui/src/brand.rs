//! The Vitna mark, as the vector it ships as.
//!
//! `vitna-web` PR #306 established that the mark is a traced SVG and every
//! raster is derived from it at the size it is displayed at, because
//! downscaling a raster softens the edges the trace was measured on. This
//! window keeps that: the mark goes through the SVG loader, which rasterises
//! at the requested size times the display's pixel ratio, so it is as crisp
//! at 18px in the sidebar as at 44px on the stage. The fill in the file is
//! the periwinkle token, so it needs no tint here.

use eframe::egui;

/// The mark, for `egui::Image::new`.
pub fn mark() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/vitna-mark.svg")
}

/// The rasterised icon, for the OS window. Platforms take a bitmap here.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The wordmark, the small cut the site chrome ships (376 x 104, 3.6:1),
/// white ink on transparent. Tint it with the ink token at the call site.
pub fn wordmark() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/vitna-wordmark-sm.png")
}

/// A provider's mark, when a licensed file exists for it. `None` means draw a
/// monogram: the menu never invents a logo.
pub fn provider_mark(provider_id: &str) -> Option<egui::ImageSource<'static>> {
    match provider_id {
        "anthropic" => Some(egui::include_image!("../assets/logos/anthropic.svg")),
        _ => None,
    }
}
