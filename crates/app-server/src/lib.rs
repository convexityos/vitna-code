//! `vitna app`: the Vitna Code interface, served from this machine (ADR-0006).
//!
//! One folder, one port on this machine's loopback, one process. The page it
//! serves at `http://localhost:<port>` is the page app.vitna.ai serves, built
//! for the runner, and it still calls the model itself on the person's own
//! key. This server gives it what only the machine can: the folder's files in
//! place, commands inside the OS sandbox, and receipts signed with the device
//! key.
//!
//! Nothing here is reachable from the daemon's IPC endpoints, and nothing
//! binds unless someone runs `vitna app`.

mod admission;
mod api;
mod journal;
mod paths;
mod ui;
pub mod window;

use ed25519_dalek::SigningKey;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::net::TcpListener;
use vitna_runner::Runner;
use vitna_tools::workspace_files::WorkspaceFiles;

/// The port `vitna app` uses unless told otherwise. It stays the same from
/// one launch to the next on purpose: the page keeps the person's key and
/// conversations in its origin's storage, and another port is another origin.
pub const DEFAULT_PORT: u16 = 7788;

pub struct AppConfig {
    /// The folder the page works in.
    pub folder: PathBuf,
    /// The port on this machine's loopback, or 0 for any free one (tests).
    pub port: u16,
    /// The built interface. Without it the server says how to build one.
    pub ui: Option<PathBuf>,
    /// The operator's decision, made at the terminal, to run commands with no
    /// OS sandbox where the machine has none.
    pub allow_unsandboxed: bool,
}

/// How commands are run, decided once when the server starts and told to the
/// page before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Commands {
    /// Inside the OS sandbox this machine has.
    Sandboxed {
        backend: String,
        enforcement: String,
    },
    /// With no sandbox, because the operator started the server with
    /// `--allow-unsandboxed` on a machine that has none.
    Unsandboxed { reason: String },
    /// Not at all: no sandbox, and no approval to run without one.
    Refused { reason: String },
    /// By a runner that starts no process, which only tests use.
    NoProcess,
}

impl Commands {
    fn decide(runner: &dyn Runner, root: &Path, allow_unsandboxed: bool) -> Self {
        if !runner.spawns_processes() {
            return Commands::NoProcess;
        }
        match vitna_runner::sandbox_status(root, false) {
            Ok((backend, enforcement)) => Commands::Sandboxed {
                backend,
                enforcement,
            },
            Err(reason) if allow_unsandboxed => Commands::Unsandboxed { reason },
            Err(reason) => Commands::Refused { reason },
        }
    }
}

/// One window's session: the token it holds, and what it has done since its
/// last receipt.
struct Session {
    id: String,
    journal: journal::Journal,
}

struct State {
    root: PathBuf,
    folder_name: String,
    files: WorkspaceFiles,
    port: u16,
    ui: Option<PathBuf>,
    runner: Arc<dyn Runner>,
    signing_key: SigningKey,
    public_key: String,
    allow_unsandboxed: bool,
    commands: Commands,
    /// One-time codes, each with the instant it stops working.
    launch_codes: Mutex<HashMap<String, Instant>>,
    /// Session tokens, each with its session.
    sessions: Mutex<HashMap<String, Session>>,
    /// One command at a time, so a page cannot start a crowd of them.
    command_slot: tokio::sync::Semaphore,
}

impl State {
    fn launch_url(&self) -> String {
        let code = admission::random_hex(32);
        let expires = Instant::now() + admission::LAUNCH_CODE_TTL;
        let mut codes = self.launch_codes.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        codes.retain(|_, until| *until > now);
        codes.insert(code.clone(), expires);
        format!("{}/#launch={code}", origin(self.port))
    }
}

fn origin(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// A handle that mints launch addresses while the server runs.
#[derive(Clone)]
pub struct Launcher {
    state: Arc<State>,
}

impl Launcher {
    /// A fresh one-time address for one window: valid for two minutes and a
    /// single exchange.
    pub fn url(&self) -> String {
        self.state.launch_url()
    }
}

pub struct AppServer {
    listeners: Vec<TcpListener>,
    state: Arc<State>,
}

impl AppServer {
    /// Resolves the folder and binds the loopback. A port already in use is an
    /// error rather than a reason to take another, since another port is
    /// another origin, with none of the page's saved state.
    pub async fn bind(
        config: AppConfig,
        runner: Arc<dyn Runner>,
        signing_key: SigningKey,
    ) -> Result<Self, String> {
        let root = std::fs::canonicalize(&config.folder)
            .map_err(|e| format!("cannot open the folder {}: {e}", config.folder.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a folder", config.folder.display()));
        }
        let files = WorkspaceFiles::open(&root)?;
        let ui = match config.ui {
            Some(dir) => Some(
                std::fs::canonicalize(&dir)
                    .map_err(|e| format!("cannot open the interface at {}: {e}", dir.display()))?,
            ),
            None => None,
        };
        let (listeners, port) = bind_loopback(config.port).await?;
        let commands = Commands::decide(runner.as_ref(), &root, config.allow_unsandboxed);
        let folder_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let public_key = hex::encode(signing_key.verifying_key().to_bytes());
        let state = State {
            root,
            folder_name,
            files,
            port,
            ui,
            runner,
            signing_key,
            public_key,
            allow_unsandboxed: config.allow_unsandboxed,
            commands,
            launch_codes: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            command_slot: tokio::sync::Semaphore::new(1),
        };
        Ok(Self {
            listeners,
            state: Arc::new(state),
        })
    }

    pub fn port(&self) -> u16 {
        self.state.port
    }

    /// `http://localhost:<port>`, the origin the page is opened at.
    pub fn origin(&self) -> String {
        origin(self.state.port)
    }

    /// The addresses it listens on: 127.0.0.1, and [::1] where this machine
    /// has an IPv6 loopback.
    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.listeners
            .iter()
            .filter_map(|l| l.local_addr().ok())
            .collect()
    }

    pub fn folder(&self) -> &Path {
        &self.state.root
    }

    pub fn commands(&self) -> &Commands {
        &self.state.commands
    }

    /// The public half of the key receipts are signed with, in hex.
    pub fn public_key(&self) -> &str {
        &self.state.public_key
    }

    pub fn launcher(&self) -> Launcher {
        Launcher {
            state: self.state.clone(),
        }
    }

    /// Answers connections until the process stops.
    pub async fn serve(self) -> Result<(), String> {
        let mut listeners = self.listeners.into_iter();
        let first = listeners.next().ok_or("nothing to listen on")?;
        for more in listeners {
            tokio::spawn(accept(more, self.state.clone()));
        }
        accept(first, self.state).await;
        Ok(())
    }
}

async fn accept(listener: TcpListener, state: Arc<State>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            // A connection that failed before it was accepted is that
            // connection's problem, not the server's.
            Err(_) => continue,
        };
        let state = state.clone();
        tokio::spawn(async move {
            let service = hyper::service::service_fn(move |request| {
                let state = state.clone();
                async move { Ok::<_, Infallible>(api::handle(&state, peer, request).await) }
            });
            // HTTP/1.1 only, and no upgrades: a WebSocket handshake gets an
            // ordinary reply and nothing more.
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await;
        });
    }
}

fn in_use(address: &str, port: u16) -> String {
    format!(
        "port {port} on {address} is in use. Stop whatever holds it, or pass --port; a page opened \
         on another port starts without its saved key and conversations"
    )
}

/// Binds 127.0.0.1 and, where this machine has one, the IPv6 loopback, on one
/// port. The page is opened at `localhost`, which a browser may look up as
/// [::1] before 127.0.0.1, so a server holding only 127.0.0.1 would let any
/// other process that took [::1] on the same port answer to the page's own
/// origin, and read the launch code out of its address. Holding both leaves
/// that name nobody else's. Port 0 (tests) takes a free port for both.
async fn bind_loopback(port: u16) -> Result<(Vec<TcpListener>, u16), String> {
    for _ in 0..8 {
        let v4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::AddrInUse => in_use("127.0.0.1", port),
                _ => format!("cannot listen on 127.0.0.1:{port}: {e}"),
            })?;
        let bound = v4
            .local_addr()
            .map_err(|e| format!("cannot read the port it listens on: {e}"))?
            .port();
        match TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, bound))).await {
            Ok(v6) => return Ok((vec![v4, v6], bound)),
            // A free port for 127.0.0.1 that something holds on [::1]: try
            // another, where any port will do.
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && port == 0 => continue,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(in_use("[::1]", port))
            }
            // No IPv6 loopback on this machine, so nothing can answer to
            // localhost there either.
            Err(_) => return Ok((vec![v4], bound)),
        }
    }
    Err("no port was free on both 127.0.0.1 and [::1]".to_string())
}
