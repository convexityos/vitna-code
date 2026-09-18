//! The window's connection to `vitna-coded`, on a thread of its own.
//!
//! The client is blocking, and a turn blocks for as long as the model and its
//! tools take, so none of it may happen on the thread that paints. This owns
//! one worker thread holding the connection, takes commands over a channel and
//! posts events back; the UI drains them each frame and never waits.
//!
//! Only one command runs at a time, deliberately. A second turn submitted while
//! the first is still running would queue behind it, and the window says a turn
//! is running rather than offering a control that silently waits.

use eframe::egui;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use vitna_protocol::api::{HealthResponse, SessionInfo, SubmitTurnRequest, TurnResultResponse};
use vitna_protocol::client::Client;

pub enum Command {
    /// Try every declared endpoint and report what was found.
    Connect,
    CreateSession(std::path::PathBuf),
    ListSessions,
    SubmitTurn(Box<SubmitTurnRequest>),
}

pub enum Event {
    Connected {
        endpoint: String,
        health: Box<HealthResponse>,
    },
    /// Carries what was tried and why each refused, so the window states a
    /// checkable reading rather than a shrug.
    Absent {
        tried: Vec<String>,
        detail: String,
    },
    SessionCreated(Box<SessionInfo>),
    Sessions(Vec<SessionInfo>),
    TurnFinished(Box<TurnResultResponse>),
    /// A call the daemon refused, with the daemon's own words.
    Failed(String),
}

pub struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
}

impl Worker {
    pub fn spawn(repaint: egui::Context) -> Self {
        let (tx, commands) = std::sync::mpsc::channel::<Command>();
        let (events, rx) = std::sync::mpsc::channel::<Event>();

        std::thread::Builder::new()
            .name("vitna-daemon-link".to_string())
            .spawn(move || {
                let mut client: Option<Client> = None;

                while let Ok(command) = commands.recv() {
                    let event = run(&mut client, command);
                    if events.send(event).is_err() {
                        return;
                    }
                    // The UI is idle between frames, so it has to be woken or
                    // the reply sits in the channel until the next mouse move.
                    repaint.request_repaint();
                }
            })
            .expect("the daemon link thread starts");

        Self { tx, rx }
    }

    pub fn send(&self, command: Command) {
        // A closed channel means the worker thread is gone. Nothing to do
        // about it here; the next poll reports the link as absent.
        let _ = self.tx.send(command);
    }

    /// Everything that arrived since the last frame.
    pub fn drain(&self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return out,
            }
        }
    }
}

fn run(client: &mut Option<Client>, command: Command) -> Event {
    if matches!(command, Command::Connect) || client.is_none() {
        match Client::connect() {
            Ok(c) => *client = Some(c),
            Err(detail) => {
                *client = None;
                return Event::Absent {
                    tried: vitna_protocol::endpoint::candidates(),
                    detail,
                };
            }
        }
    }

    let c = match client.as_mut() {
        Some(c) => c,
        None => {
            return Event::Absent {
                tried: vitna_protocol::endpoint::candidates(),
                detail: "no connection".to_string(),
            }
        }
    };

    let endpoint = c.endpoint().to_string();

    match command {
        Command::Connect => match c.health() {
            Ok(health) => Event::Connected {
                endpoint,
                health: Box::new(health),
            },
            Err(detail) => {
                // Connected but unanswering is not connected. Drop it so the
                // next command reconnects rather than reusing a dead socket.
                *client = None;
                Event::Absent {
                    tried: vitna_protocol::endpoint::candidates(),
                    detail,
                }
            }
        },
        Command::CreateSession(path) => match c.create_session(path) {
            Ok(s) => Event::SessionCreated(Box::new(s)),
            Err(e) => Event::Failed(e),
        },
        Command::ListSessions => match c.list_sessions() {
            Ok(s) => Event::Sessions(s),
            Err(e) => Event::Failed(e),
        },
        Command::SubmitTurn(req) => match c.submit_turn(&req) {
            Ok(r) => Event::TurnFinished(Box::new(r)),
            Err(e) => Event::Failed(e),
        },
    }
}
