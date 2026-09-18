//! The window's frame, painted in the window's own material.
//!
//! The title bar belongs to Windows, and by default it is Windows' own grey, a
//! shade off the app's floor, so the window read as a picture of an app set in
//! a frame. On Windows 11 the compositor takes a caption colour, a caption text
//! colour and a border colour per window, so the frame is asked to wear the
//! floor, the ink and the hairline instead. Snap, resize and the native buttons
//! are untouched, which a hand-drawn title bar would have to rebuild. Windows 10
//! ignores these attributes, and on every other system this does nothing.

#[cfg(windows)]
pub fn match_frame(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE,
    };

    use crate::theme;

    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win) = handle.as_raw() else {
        return;
    };
    let hwnd = win.hwnd.get() as windows_sys::Win32::Foundation::HWND;

    // COLORREF is 0x00BBGGRR.
    let colorref = |c: eframe::egui::Color32| u32::from(c.r()) | u32::from(c.g()) << 8 | u32::from(c.b()) << 16;
    let set = |attribute: DWMWINDOWATTRIBUTE, value: u32| {
        // SAFETY: `hwnd` is this process's own live top-level window, handed
        // over by eframe for exactly this, and each of these attributes takes
        // one 4-byte value. A refusal (Windows 10, say) is ignored on purpose:
        // the frame then keeps the system's colours, which is the old look.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    // The floor, as the sidebar is: chrome, above the lighter working side.
    set(DWMWA_CAPTION_COLOR, colorref(theme::RAIL));
    set(DWMWA_TEXT_COLOR, colorref(theme::FAINT));
    set(DWMWA_BORDER_COLOR, colorref(theme::HAIR));
}

#[cfg(not(windows))]
pub fn match_frame(_cc: &eframe::CreationContext<'_>) {}
