//! Opening the interface in a window of its own.
//!
//! A Chromium browser (Edge, Chrome, Chromium, Brave) opens an address as an
//! app window with `--app`: no tabs and no address bar, its own entry in the
//! taskbar or dock. Where none is found the address opens in the default
//! browser, as a tab. Either way the browser is the person's own, with the
//! page's saved state in the profile it already uses.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Opens `url` and says how.
pub fn open(url: &str) -> Result<String, String> {
    for (name, program, prefix) in app_browsers() {
        let mut command = Command::new(&program);
        command.args(prefix).arg(format!("--app={url}"));
        if spawn(command).is_ok() {
            return Ok(format!("an app window in {name}"));
        }
    }
    spawn(default_browser(url))
        .map(|()| "your default browser".to_string())
        .map_err(|e| format!("could not open a browser: {e}. Open {url} yourself"))
}

/// Starts a program and leaves it running, reaping it when it exits.
fn spawn(mut command: Command) -> std::io::Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(windows)]
fn app_browsers() -> Vec<(&'static str, PathBuf, Vec<String>)> {
    let mut found = Vec::new();
    let bases = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"];
    let known = [
        ("Microsoft Edge", "Microsoft\\Edge\\Application\\msedge.exe"),
        ("Google Chrome", "Google\\Chrome\\Application\\chrome.exe"),
        (
            "Brave",
            "BraveSoftware\\Brave-Browser\\Application\\brave.exe",
        ),
    ];
    for (name, tail) in known {
        for base in bases {
            if let Some(dir) = std::env::var_os(base) {
                let program = PathBuf::from(dir).join(tail);
                if program.is_file() {
                    found.push((name, program, Vec::new()));
                    break;
                }
            }
        }
    }
    found
}

#[cfg(target_os = "macos")]
fn app_browsers() -> Vec<(&'static str, PathBuf, Vec<String>)> {
    // `open -na` hands the arguments to the running browser rather than
    // starting a second copy of it.
    [
        "Google Chrome",
        "Microsoft Edge",
        "Chromium",
        "Brave Browser",
    ]
    .into_iter()
    .filter(|app| std::path::Path::new(&format!("/Applications/{app}.app")).is_dir())
    .map(|app| {
        (
            app,
            PathBuf::from("/usr/bin/open"),
            vec!["-na".to_string(), app.to_string(), "--args".to_string()],
        )
    })
    .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn app_browsers() -> Vec<(&'static str, PathBuf, Vec<String>)> {
    let names = [
        ("Google Chrome", "google-chrome"),
        ("Google Chrome", "google-chrome-stable"),
        ("Chromium", "chromium"),
        ("Chromium", "chromium-browser"),
        ("Microsoft Edge", "microsoft-edge"),
        ("Microsoft Edge", "microsoft-edge-stable"),
        ("Brave", "brave-browser"),
    ];
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut found = Vec::new();
    for (name, binary) in names {
        if let Some(program) = std::env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|p| p.is_file())
        {
            found.push((name, program, Vec::new()));
        }
    }
    found
}

#[cfg(not(any(unix, windows)))]
fn app_browsers() -> Vec<(&'static str, PathBuf, Vec<String>)> {
    Vec::new()
}

#[cfg(windows)]
fn default_browser(url: &str) -> Command {
    let mut command = Command::new("rundll32.exe");
    command.arg("url.dll,FileProtocolHandler").arg(url);
    command
}

#[cfg(target_os = "macos")]
fn default_browser(url: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command.arg(url);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_browser(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(not(any(unix, windows)))]
fn default_browser(url: &str) -> Command {
    Command::new(url)
}
