//! Fixtures shared by the file tools' tests.

use crate::ToolContext;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use vitna_runner::FakeRunner;

/// A workspace, a directory beside it that is outside it, and a runner
/// journal kept out of the workspace so no search ever sees it. Removed on
/// drop. `name` must be unique across the crate's tests, which run in
/// parallel in one process.
pub(crate) struct Scratch {
    base: PathBuf,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        let base = std::env::temp_dir()
            .join(format!("vitna_tools_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("ws")).expect("create workspace");
        fs::create_dir_all(base.join("outside")).expect("create outside dir");
        fs::write(base.join("outside").join("secret.txt"), "TOP SECRET\n")
            .expect("write outside file");
        Self { base }
    }

    pub(crate) fn ws(&self) -> PathBuf {
        self.base.join("ws")
    }

    pub(crate) fn outside(&self) -> PathBuf {
        self.base.join("outside")
    }

    pub(crate) fn ctx(&self) -> ToolContext {
        let runner = FakeRunner::new(self.base.join("test.journal")).expect("open fake runner");
        ToolContext {
            workspace_root: self.ws(),
            runner: Arc::new(Mutex::new(runner)),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Links `link` to the file `target`. False where this machine refuses,
/// such as Windows without the symlink privilege; the test then skips.
#[cfg(unix)]
pub(crate) fn file_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
pub(crate) fn file_link(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

/// Links `link` to the absolute directory `target`. Windows without the
/// symlink privilege gets a junction, which needs none and redirects the
/// same way.
#[cfg(unix)]
pub(crate) fn dir_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
pub(crate) fn dir_link(target: &Path, link: &Path) -> bool {
    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return true;
    }
    std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[cfg(unix)]
pub(crate) fn mkfifo(path: &Path) -> bool {
    std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

/// Runs a tool on its own thread and fails the test, instead of hanging it,
/// when the call blocks.
#[cfg(unix)]
pub(crate) fn execute_bounded(
    tool: impl crate::Tool + 'static,
    args: serde_json::Value,
    ctx: ToolContext,
) -> Result<crate::ToolResult, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let _ = tx.send(runtime.block_on(tool.execute(args, &ctx)));
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("the tool blocked instead of refusing")
}
