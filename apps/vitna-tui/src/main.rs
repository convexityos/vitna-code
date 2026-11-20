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
    let mut folder_arg: Option<PathBuf> = None;
    let mut initial_mode = "build".to_string();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--mode" {
            let value = args
                .next()
                .ok_or_else(|| "--mode requires a value (inspect or build)".to_string())?;
            let mode_str = value.to_string_lossy();
            match mode_str.as_ref() {
                "inspect" | "build" => initial_mode = mode_str.into_owned(),
                other => {
                    return Err(format!(
                        "unsupported --mode '{other}': expected inspect or build"
                    ));
                }
            }
        } else if folder_arg.is_none() {
            folder_arg = Some(PathBuf::from(arg));
        } else {
            return Err(format!(
                "unexpected argument '{}': usage is `vitna-tui [--mode inspect|build] [folder]`",
                arg.to_string_lossy()
            ));
        }
    }

    let root = match folder_arg {
        Some(arg) => arg,
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
    app.session_mode = initial_mode;
    let mut link_task = Some(connect_daemon(&runtime, &tx, &mut app));
    read_workspace(&runtime, &tx, &root, true);

    let mut terminal =
        enter(&app.folder).map_err(|e| format!("the terminal would not enter full screen: {e}"))?;
    let result = run(
        &mut terminal,
        &mut app,
        &rx,
        &runtime,
        &tx,
        &mut link_task,
        depth,
    );
    if let Some(handle) = link_task.take() {
        handle.abort();
    }
    leave(&mut terminal);
    result.map_err(|e| format!("drawing failed: {e}"))
}

fn connect_daemon(
    runtime: &Runtime,
    tx: &Sender<Update>,
    app: &mut App,
) -> tokio::task::JoinHandle<()> {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    app.cmd_tx = Some(cmd_tx);
    let t = tx.clone();
    let root = app.root.clone();
    let mode = app.session_mode.clone();
    runtime.spawn(async move {
        link::connect_session_with_mode(root, mode, t, cmd_rx).await;
    })
}

/// Finds out, off the drawing thread, what receipts are on disk and which
/// branch this is.
fn read_workspace(runtime: &Runtime, tx: &Sender<Update>, root: &Path, branch: bool) {
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
    link_task: &mut Option<tokio::task::JoinHandle<()>>,
    depth: Depth,
) -> io::Result<()> {
    while !app.quit {
        while let Ok(update) = rx.try_recv() {
            if app.apply_update(update) == Ask::RefreshLedger {
                read_workspace(runtime, tx, &app.root, false);
            }
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
                if let Some(handle) = link_task.take() {
                    handle.abort();
                }
                *link_task = Some(connect_daemon(runtime, tx, app));
                read_workspace(runtime, tx, &app.root, false);
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
