//! `vitna-tui [folder]`: the terminal client, opened on a folder, or on the
//! current one.

use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, SystemTime};

use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::runtime::Runtime;

use vitna_tui::theme::Depth;
use vitna_tui::{ledger, link, place, view, App, Ask, Update};

/// How often the frame is drawn while nothing happens. Drawing an unchanged
/// frame writes nothing to the terminal, since ratatui sends only the cells
/// that differ, so this costs a buffer comparison and keeps anything timed
/// (an approval arming) moving without a timer of its own.
const TICK: Duration = Duration::from_millis(80);

fn main() {
    if let Err(e) = start() {
        // One line naming what failed, on the ordinary screen.
        eprintln!("vitna-tui: {e}");
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let root = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => std::env::current_dir()
            .map_err(|e| format!("the current folder cannot be read: {e}"))?,
    };
    // Absolute without resolving links, and without the `\\?\` prefix
    // canonicalizing adds on Windows, which git refuses as a working folder.
    let root = std::path::absolute(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }

    let depth = Depth::detect(|name| std::env::var(name).ok());
    let runtime = Runtime::new().map_err(|e| format!("the async runtime would not start: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(root.clone());
    look(&runtime, &tx, &root, true);

    let mut terminal =
        enter(&app.folder).map_err(|e| format!("the terminal would not enter full screen: {e}"))?;
    let result = run(&mut terminal, &mut app, &rx, &runtime, &tx, depth);
    leave(&mut terminal);
    result.map_err(|e| format!("drawing failed: {e}"))
}

/// Finds out, off the drawing thread, everything the screen reports: whether
/// a daemon answers, what receipts are on disk, and which branch this is.
fn look(runtime: &Runtime, tx: &Sender<Update>, root: &Path, branch: bool) {
    let t = tx.clone();
    runtime.spawn(async move {
        let _ = t.send(Update::Link(link::probe().await));
    });
    let (t, r) = (tx.clone(), root.to_path_buf());
    runtime.spawn_blocking(move || {
        let _ = t.send(Update::Ledger(ledger::read(&r)));
    });
    if branch {
        let (t, r) = (tx.clone(), root.to_path_buf());
        runtime.spawn_blocking(move || {
            let _ = t.send(Update::Branch(place::read_branch(&r)));
        });
    }
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &Receiver<Update>,
    runtime: &Runtime,
    tx: &Sender<Update>,
    depth: Depth,
) -> io::Result<()> {
    while !app.quit {
        while let Ok(update) = rx.try_recv() {
            app.update(update);
        }
        terminal.draw(|frame| {
            view::render(app, frame, SystemTime::now());
            depth.apply(frame.buffer_mut());
        })?;
        if event::poll(TICK)? {
            // Everything already waiting is read before the next frame, so a
            // paste that arrives as a burst of keys costs one draw, not one
            // per key.
            let mut batch = vec![event::read()?];
            while event::poll(Duration::ZERO)? {
                batch.push(event::read()?);
            }
            if app.events(&batch) == Ask::Refresh {
                look(runtime, tx, &app.root, false);
            }
        }
    }
    Ok(())
}

fn enter(folder: &str) -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    // A panic must hand the terminal back as it found it, or the shell it
    // returns to is left in raw mode with no cursor.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            SetCursorStyle::DefaultUserShape
        );
        previous(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        SetCursorStyle::BlinkingBar,
        SetTitle(format!("Vitna Code \u{b7} {folder}"))
    )?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape
    );
    let _ = terminal.show_cursor();
}
