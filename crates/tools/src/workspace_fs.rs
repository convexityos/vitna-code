//! Opening workspace files where the operating system actually resolves them.
//!
//! `path_safety::resolve_workspace_path` is lexical: it proves that a
//! requested path *names* something under the workspace root. Opening that
//! name still follows symbolic links, junctions and directories renamed after
//! the check, so a link committed to the repository, or one swapped in by a
//! process the agent started, could hand a tool any file on the machine.
//!
//! Everything here repeats the containment check on the path the operating
//! system resolved, against the workspace root with its own links resolved,
//! and then asks the opened handle where it actually lives, so a rename
//! between the check and the open cannot move the file outside. Only regular
//! files are opened: a FIFO blocks its opener until a writer appears, and a
//! device can produce bytes forever.

use crate::path_safety::{is_within, resolve_workspace_path};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The most bytes a tool loads from one file. A read stops here and says so;
/// a write refuses to replace a larger file, since its preimage could not be
/// held for the diff.
pub(crate) const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The preimage hash recorded for a file that did not exist before a write.
pub(crate) const ABSENT_PREIMAGE_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Reads at most `limit` bytes. The flag is true when there was more.
pub(crate) fn read_up_to<R: Read>(reader: R, limit: u64) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    reader.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    let longer = bytes.len() as u64 > limit;
    if longer {
        bytes.truncate(limit as usize);
    }
    Ok((bytes, longer))
}

/// Where a requested path leads, decided on the real filesystem.
pub(crate) enum Resolved {
    /// Something exists there. This is its real location, every link
    /// resolved, and it lies inside the workspace.
    Existing(PathBuf),
    /// Nothing exists there yet. This is the lexically normalized request;
    /// the directories along it are checked again before anything is made.
    Missing(PathBuf),
}

/// A write that has been checked but not yet made.
pub(crate) struct PendingWrite {
    requested: String,
    target: Target,
    /// What the file held before this write. Empty when it did not exist.
    pub(crate) preimage: Vec<u8>,
    pub(crate) preimage_hash: String,
}

enum Target {
    /// A regular file, opened read-write and verified, not yet truncated.
    Existing { file: File, path: PathBuf },
    /// Nothing there yet. Created at commit, after its parents are re-checked.
    Missing { path: PathBuf },
}

/// A workspace root, resolved to its real location once per tool call.
pub(crate) struct Workspace {
    /// The root as configured. Lexical checks run against it, so an absolute
    /// path spelled the way the root was configured keeps working.
    given: PathBuf,
    /// The root with its own links resolved. Containment is decided here.
    real: PathBuf,
}

impl Workspace {
    pub(crate) fn new(root: &Path) -> Result<Self, String> {
        let real = fs::canonicalize(root).map_err(|e| {
            format!("Workspace root '{}' cannot be resolved: {}", root.display(), e)
        })?;
        Ok(Self {
            given: root.to_path_buf(),
            real,
        })
    }

    /// True when `path`, already resolved, is the workspace root or beneath it.
    pub(crate) fn contains(&self, path: &Path) -> bool {
        is_within(path, &self.real)
    }

    /// A resolved path relative to the workspace root, with `/` separators.
    pub(crate) fn relative(&self, path: &Path) -> String {
        match path.strip_prefix(&self.real) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => path.display().to_string(),
        }
    }

    /// Resolves `requested` lexically, then on the filesystem. An existing
    /// path must resolve inside the workspace, however many links it crosses.
    pub(crate) fn resolve(&self, requested: &str) -> Result<Resolved, String> {
        let lexical = resolve_workspace_path(&self.given, requested)?;
        match fs::canonicalize(&lexical) {
            Ok(real) if self.contains(&real) => Ok(Resolved::Existing(real)),
            Ok(_) => Err(outside(requested)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // A link whose target is missing is still there, and writing
                // through it would create the target wherever it points.
                match fs::symlink_metadata(&lexical) {
                    Ok(meta) if meta.file_type().is_symlink() => Err(format!(
                        "'{}' is a link whose target does not exist",
                        requested
                    )),
                    _ => Ok(Resolved::Missing(lexical)),
                }
            }
            Err(e) => Err(format!("Cannot resolve '{}': {}", requested, e)),
        }
    }

    /// Opens a resolved path for reading. Only a regular file is accepted,
    /// and only if the opened handle still lies inside the workspace.
    pub(crate) fn open_regular(&self, real: &Path, requested: &str) -> Result<File, String> {
        let meta =
            fs::metadata(real).map_err(|e| format!("Cannot read '{}': {}", requested, e))?;
        if !meta.is_file() {
            return Err(not_regular(requested, &meta));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        no_follow_no_block(&mut options);
        let file = options
            .open(real)
            .map_err(|e| format!("Cannot open '{}': {}", requested, e))?;
        self.verify_opened(&file, real, requested)?;
        Ok(file)
    }

    /// Checks a write target and captures its preimage. Nothing is changed
    /// on disk, so a write rejected after this (a preimage conflict, say)
    /// leaves no trace.
    pub(crate) fn prepare_write(&self, requested: &str) -> Result<PendingWrite, String> {
        match self.resolve(requested)? {
            Resolved::Existing(path) => {
                let meta = fs::metadata(&path)
                    .map_err(|e| format!("Cannot read '{}': {}", requested, e))?;
                if !meta.is_file() {
                    return Err(not_regular(requested, &meta));
                }
                // Opened without truncation: nothing changes until the handle
                // has been verified and the caller commits.
                let mut options = OpenOptions::new();
                options.read(true).write(true);
                no_follow_no_block(&mut options);
                let mut file = options
                    .open(&path)
                    .map_err(|e| format!("Cannot open '{}' for writing: {}", requested, e))?;
                self.verify_opened(&file, &path, requested)?;

                let (preimage, longer) = read_up_to(&mut file, MAX_FILE_BYTES)
                    .map_err(|e| format!("Failed to read existing file {}: {}", requested, e))?;
                if longer {
                    return Err(format!(
                        "Refusing to replace '{}': it is larger than {} bytes, the most write_file and apply_patch load",
                        requested, MAX_FILE_BYTES
                    ));
                }
                let preimage_hash = sha256_hex(&preimage);
                Ok(PendingWrite {
                    requested: requested.to_string(),
                    target: Target::Existing { file, path },
                    preimage,
                    preimage_hash,
                })
            }
            Resolved::Missing(path) => {
                self.existing_parent(&path, requested)?;
                Ok(PendingWrite {
                    requested: requested.to_string(),
                    target: Target::Missing { path },
                    preimage: Vec::new(),
                    preimage_hash: ABSENT_PREIMAGE_HASH.to_string(),
                })
            }
        }
    }

    /// Makes a prepared write, flushes it to the device, then reads the file
    /// back. Returns the SHA-256 of the bytes read back, which is the only
    /// postimage this write may claim; a file that does not hold exactly
    /// `content` afterwards is a failed write.
    pub(crate) fn commit_write(
        &self,
        pending: PendingWrite,
        content: &[u8],
    ) -> Result<String, String> {
        let requested = pending.requested;
        let (mut file, path) = match pending.target {
            Target::Existing { mut file, path } => {
                file.set_len(0)
                    .and_then(|_| file.seek(SeekFrom::Start(0)))
                    .map_err(|e| format!("Failed to write file {}: {}", requested, e))?;
                (file, path)
            }
            Target::Missing { path } => self.create_file(&path, &requested)?,
        };
        write_synced(&mut file, content)
            .map_err(|e| format!("Failed to write file {}: {}", requested, e))?;
        drop(file);
        #[cfg(test)]
        tests::AFTER_WRITE.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook(path.as_path());
            }
        });
        self.verify_written(&path, &requested, content)
    }

    /// Reads a just-written file back and compares it with what was written,
    /// byte for byte, line endings included.
    fn verify_written(&self, path: &Path, requested: &str, content: &[u8]) -> Result<String, String> {
        let mut file = self
            .open_regular(path, requested)
            .map_err(|e| unverified(requested, &e))?;
        // One byte past what was written is enough to tell that the file grew.
        let (back, longer) =
            read_up_to(&mut file, content.len() as u64).map_err(|e| unverified(requested, &e))?;
        if !longer && back == content {
            return Ok(sha256_hex(&back));
        }
        let found = if longer {
            format!("more than {} bytes", content.len())
        } else {
            format!("{} bytes (SHA-256 {})", back.len(), sha256_hex(&back))
        };
        Err(format!(
            "Write to '{}' failed verification: {} bytes were written (SHA-256 {}), but reading the file back returned {}. The file does not hold the requested content, so no postimage is recorded.",
            requested,
            content.len(),
            sha256_hex(content),
            found
        ))
    }

    /// Resolves the deepest existing ancestor of `path`, which must be a real
    /// directory inside the workspace, and lists the directories still
    /// missing below it, outermost first.
    fn existing_parent(&self, path: &Path, requested: &str) -> Result<(PathBuf, Vec<OsString>), String> {
        let mut existing = path
            .parent()
            .ok_or_else(|| format!("'{}' has no parent directory", requested))?
            .to_path_buf();
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(&existing) {
                Ok(_) => break,
                Err(e) if e.kind() == io::ErrorKind::NotFound => match existing.file_name() {
                    Some(name) => {
                        missing.push(name.to_os_string());
                        existing.pop();
                    }
                    None => return Err(format!("Cannot resolve '{}': {}", requested, e)),
                },
                Err(e) => return Err(format!("Cannot resolve '{}': {}", requested, e)),
            }
        }
        missing.reverse();

        let real = fs::canonicalize(&existing)
            .map_err(|e| format!("Cannot resolve '{}': {}", requested, e))?;
        if !self.contains(&real) {
            return Err(outside(requested));
        }
        if !real.is_dir() {
            return Err(format!(
                "Cannot write '{}': '{}' is not a directory",
                requested,
                self.relative(&real)
            ));
        }
        Ok((real, missing))
    }

    /// Creates a missing file, first creating and re-checking its parents.
    fn create_file(&self, path: &Path, requested: &str) -> Result<(File, PathBuf), String> {
        let (mut dir, missing) = self.existing_parent(path, requested)?;
        for name in missing {
            dir.push(name);
            match fs::create_dir(&dir) {
                Ok(()) => {}
                // Made by someone else meanwhile: only a real directory will do.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
                        return Err(format!(
                            "Cannot write '{}': '{}' appeared as something other than a directory",
                            requested,
                            self.relative(&dir)
                        ));
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to create parent directory for {}: {}",
                        requested, e
                    ))
                }
            }
        }

        // The directories above were made under a path resolved before the
        // first of them existed. Resolve it again now that it does.
        let parent = fs::canonicalize(&dir)
            .map_err(|e| format!("Cannot resolve '{}': {}", requested, e))?;
        if !self.contains(&parent) {
            return Err(outside(requested));
        }
        let name = path
            .file_name()
            .ok_or_else(|| format!("'{}' does not name a file", requested))?;
        let target = parent.join(name);

        // create_new refuses anything already there, a link included, so a
        // file planted after the check is never written through.
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        no_follow_no_block(&mut options);
        let file = options
            .open(&target)
            .map_err(|e| format!("Failed to create file {}: {}", requested, e))?;
        self.verify_opened(&file, &target, requested)?;
        Ok((file, target))
    }

    /// Checks what a handle actually opened: a regular file, still inside the
    /// workspace. `expected` is the resolved path it was opened by.
    fn verify_opened(&self, file: &File, expected: &Path, requested: &str) -> Result<(), String> {
        let meta = file
            .metadata()
            .map_err(|e| format!("Cannot inspect '{}': {}", requested, e))?;
        if !meta.is_file() {
            return Err(not_regular(requested, &meta));
        }
        match opened_path(file) {
            Ok(Some(actual)) if self.contains(&actual) => Ok(()),
            Ok(Some(_)) => Err(format!(
                "Access denied: '{}' was redirected outside the workspace root while it was opened",
                requested
            )),
            // Nothing on this platform says where a handle lives, so settle
            // for identity: the path that was checked must still name the
            // file that was opened.
            Ok(None) => match fs::symlink_metadata(expected) {
                Ok(now) if same_file(&meta, &now) => Ok(()),
                _ => Err(format!("'{}' changed while it was being opened", requested)),
            },
            Err(e) => Err(format!(
                "Cannot verify where '{}' was opened: {}",
                requested, e
            )),
        }
    }
}

fn outside(requested: &str) -> String {
    format!(
        "Access denied: '{}' resolves outside the workspace root",
        requested
    )
}

fn unverified(requested: &str, detail: &dyn std::fmt::Display) -> String {
    format!(
        "Write to '{}' could not be verified, so no postimage is recorded: {}",
        requested, detail
    )
}

fn not_regular(requested: &str, meta: &fs::Metadata) -> String {
    format!("'{}' is {}, not a regular file", requested, describe(meta))
}

fn describe(meta: &fs::Metadata) -> &'static str {
    let kind = meta.file_type();
    if kind.is_dir() {
        return "a directory";
    }
    if kind.is_symlink() {
        return "a link";
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_fifo() {
            return "a FIFO";
        }
        if kind.is_socket() {
            return "a socket";
        }
        if kind.is_block_device() || kind.is_char_device() {
            return "a device";
        }
    }
    "a special file"
}

/// Writes everything, then flushes it to the device before the handle is
/// dropped: `File`'s drop discards any error from close, and network and
/// FUSE filesystems report some write failures only when flushed.
fn write_synced(file: &mut File, content: &[u8]) -> io::Result<()> {
    file.write_all(content)?;
    file.sync_all()
}

/// O_NOFOLLOW makes a final component swapped for a link after the check
/// fail to open rather than be followed. O_NONBLOCK keeps a FIFO swapped in
/// after the type check from blocking the open; the handle check refuses it.
#[cfg(unix)]
fn no_follow_no_block(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// Windows keeps FIFOs out of the file namespace, and a link swapped in
/// after the check is caught by the handle's final path.
#[cfg(not(unix))]
fn no_follow_no_block(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Windows always answers `opened_path`, so this runs only on platforms that
/// can do neither. Refuse rather than guess.
#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    false
}

/// Where the operating system says an open handle lives. `Ok(None)` means
/// this platform cannot say.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn opened_path(file: &File) -> io::Result<Option<PathBuf>> {
    use std::os::unix::io::AsRawFd;
    // Without /proc there is no answer, and the caller falls back to identity.
    Ok(fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok())
}

#[cfg(target_os = "macos")]
fn opened_path(file: &File) -> io::Result<Option<PathBuf>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;

    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes a NUL-terminated path of at most PATH_MAX
    // bytes, and `buf` is exactly PATH_MAX bytes long.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    let path = CStr::from_bytes_until_nul(&buf).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "F_GETPATH returned no terminator")
    })?;
    Ok(Some(PathBuf::from(OsStr::from_bytes(path.to_bytes()))))
}

#[cfg(windows)]
fn opened_path(file: &File) -> io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    let mut buf = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path too long"))?;
        // SAFETY: the handle is owned by `file`, which outlives the call, and
        // `buf` holds `capacity` UTF-16 units.
        let len = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buf.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < buf.len() {
            buf.truncate(len);
            return Ok(Some(PathBuf::from(OsString::from_wide(&buf))));
        }
        // Too small: `len` is the size needed, terminating NUL included.
        buf.resize(len, 0);
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android", target_os = "macos")))]
fn opened_path(_file: &File) -> io::Result<Option<PathBuf>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    type Hook = Box<dyn Fn(&Path)>;

    thread_local! {
        /// Runs between a commit's write and its read-back, standing in for
        /// whatever rewrites a file behind the tool: a formatter, or a
        /// filesystem that loses part of a write. Each test runs on its own
        /// thread, so a hook set here cannot reach another test.
        pub(super) static AFTER_WRITE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    /// A workspace with a sibling directory outside it, removed on drop.
    struct Scratch {
        base: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("vitna_wsfs_{}_{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(base.join("ws")).expect("create workspace");
            fs::create_dir_all(base.join("outside")).expect("create outside dir");
            fs::write(base.join("outside").join("secret.txt"), "TOP SECRET\n")
                .expect("write secret");
            Self { base }
        }

        fn ws(&self) -> PathBuf {
            self.base.join("ws")
        }

        fn outside(&self) -> PathBuf {
            self.base.join("outside")
        }

        fn workspace(&self) -> Workspace {
            Workspace::new(&self.ws()).expect("resolve workspace root")
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
    fn file_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn file_link(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    /// Links `link` to the directory `target`, which must be absolute. On
    /// Windows without the symlink privilege this makes a junction instead,
    /// which needs none and is the same kind of redirect.
    #[cfg(unix)]
    fn dir_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn dir_link(target: &Path, link: &Path) -> bool {
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
    fn mkfifo(path: &Path) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Runs `f` on its own thread and fails the test, instead of hanging it,
    /// when `f` blocks.
    #[cfg(unix)]
    fn bounded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the call blocked instead of refusing")
    }

    fn read_all(file: &mut File) -> Vec<u8> {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read opened file");
        bytes
    }

    #[test]
    fn test_reads_a_regular_file_inside() {
        let s = Scratch::new("read_inside");
        fs::write(s.ws().join("a.txt"), "hello\n").unwrap();
        let ws = s.workspace();

        let Resolved::Existing(real) = ws.resolve("a.txt").expect("resolve") else {
            panic!("a.txt exists");
        };
        let mut file = ws.open_regular(&real, "a.txt").expect("open");
        assert_eq!(read_all(&mut file), b"hello\n");

        // The root as configured still works for absolute requests, even
        // where it is itself reached through a link (macOS /var -> /private/var).
        let absolute = s.ws().join("a.txt");
        assert!(matches!(
            ws.resolve(absolute.to_str().unwrap()),
            Ok(Resolved::Existing(_))
        ));
        assert!(matches!(ws.resolve("missing.txt"), Ok(Resolved::Missing(_))));
    }

    #[test]
    fn test_opened_handle_reports_where_it_lives() {
        let s = Scratch::new("handle_path");
        let path = s.ws().join("h.txt");
        fs::write(&path, "x").unwrap();
        let file = File::open(&path).unwrap();
        let expected = fs::canonicalize(&path).unwrap();
        if let Some(actual) = opened_path(&file).expect("ask the handle") {
            assert!(
                is_within(&actual, &expected) && is_within(&expected, &actual),
                "handle says {:?}, canonicalize says {:?}",
                actual,
                expected
            );
        }
    }

    #[test]
    fn test_link_to_a_file_outside_is_denied() {
        let s = Scratch::new("file_link_out");
        if !file_link(&s.outside().join("secret.txt"), &s.ws().join("notes.txt")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let ws = s.workspace();
        let err = ws.resolve("notes.txt").err().expect("must be denied");
        assert!(err.contains("outside the workspace"), "{}", err);
        let err = ws.prepare_write("notes.txt").err().expect("must be denied");
        assert!(err.contains("outside the workspace"), "{}", err);
        assert_eq!(
            fs::read_to_string(s.outside().join("secret.txt")).unwrap(),
            "TOP SECRET\n"
        );
    }

    #[test]
    fn test_directory_link_to_outside_is_denied_for_reads_and_writes() {
        let s = Scratch::new("dir_link_out");
        if !dir_link(&s.outside(), &s.ws().join("leak")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let ws = s.workspace();

        assert!(ws.resolve("leak/secret.txt").is_err());
        assert!(ws.prepare_write("leak/secret.txt").is_err());
        assert!(ws.prepare_write("leak/new.txt").is_err());
        assert!(ws.prepare_write("leak/deeper/new.txt").is_err());

        assert!(!s.outside().join("new.txt").exists());
        assert!(!s.outside().join("deeper").exists());
        assert_eq!(
            fs::read_to_string(s.outside().join("secret.txt")).unwrap(),
            "TOP SECRET\n"
        );
    }

    #[test]
    fn test_link_that_stays_inside_is_followed() {
        let s = Scratch::new("dir_link_in");
        fs::create_dir_all(s.ws().join("real_dir")).unwrap();
        fs::write(s.ws().join("real_dir").join("f.txt"), "inside\n").unwrap();
        if !dir_link(&s.ws().join("real_dir"), &s.ws().join("alias")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let ws = s.workspace();
        let Resolved::Existing(real) = ws.resolve("alias/f.txt").expect("resolve") else {
            panic!("alias/f.txt exists");
        };
        assert!(ws.contains(&real));
        assert_eq!(ws.relative(&real), "real_dir/f.txt");
    }

    #[test]
    fn test_dangling_link_is_refused_for_writes() {
        let s = Scratch::new("dangling");
        let target = s.outside().join("planted.txt");
        if !file_link(&target, &s.ws().join("d.txt")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let err = s.workspace().prepare_write("d.txt").err().expect("refused");
        assert!(err.contains("does not exist"), "{}", err);
        assert!(!target.exists(), "nothing may be created through the link");
    }

    #[cfg(unix)]
    #[test]
    fn test_fifo_is_refused_without_blocking() {
        let s = Scratch::new("fifo");
        if !mkfifo(&s.ws().join("pipe")) {
            eprintln!("skipped: mkfifo is unavailable");
            return;
        }
        let root = s.ws();
        let (read_err, write_err) = bounded(move || {
            let ws = Workspace::new(&root).expect("resolve workspace root");
            let Resolved::Existing(real) = ws.resolve("pipe").expect("resolve") else {
                panic!("pipe exists");
            };
            (
                ws.open_regular(&real, "pipe").err(),
                ws.prepare_write("pipe").err(),
            )
        });
        assert!(read_err.expect("read refused").contains("a FIFO"));
        assert!(write_err.expect("write refused").contains("a FIFO"));
    }

    #[test]
    fn test_write_creates_parents_and_hashes_what_was_read_back() {
        let s = Scratch::new("write_new");
        let ws = s.workspace();

        let pending = ws.prepare_write("a/b/new.txt").expect("prepare");
        assert_eq!(pending.preimage_hash, ABSENT_PREIMAGE_HASH);
        let hash = ws.commit_write(pending, b"one\n").expect("commit");
        assert_eq!(hash, sha256_hex(b"one\n"));
        assert_eq!(fs::read(s.ws().join("a/b/new.txt")).unwrap(), b"one\n");

        let pending = ws.prepare_write("a/b/new.txt").expect("prepare again");
        assert_eq!(pending.preimage, b"one\n");
        assert_eq!(pending.preimage_hash, sha256_hex(b"one\n"));
        let hash = ws.commit_write(pending, b"2").expect("shorter rewrite");
        assert_eq!(hash, sha256_hex(b"2"));
        assert_eq!(fs::read(s.ws().join("a/b/new.txt")).unwrap(), b"2");
    }

    #[test]
    fn test_line_endings_are_written_byte_exactly() {
        let s = Scratch::new("endings");
        let ws = s.workspace();
        let cases: [&[u8]; 6] = [
            b"a\r\nb\r\n",
            b"a\nb\r\nc\rd",
            b"no final newline",
            b"\xef\xbb\xbfbom\r\n",
            b"\r\n\r\n",
            b"",
        ];
        for (i, content) in cases.iter().enumerate() {
            let name = format!("case{}.txt", i);
            let pending = ws.prepare_write(&name).expect("prepare");
            let hash = ws.commit_write(pending, content).expect("commit");
            assert_eq!(hash, sha256_hex(content), "case {}", i);
            assert_eq!(fs::read(s.ws().join(&name)).unwrap(), *content, "case {}", i);
        }
    }

    #[test]
    fn test_file_that_differs_after_the_write_is_a_failed_write() {
        let s = Scratch::new("mismatch");
        let ws = s.workspace();
        let path = s.ws().join("f.txt");
        let content = b"fn main() {}\r\n";

        // A formatter rewrote it.
        fs::write(&path, "fn main() {}\n").unwrap();
        let err = ws.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("failed verification"), "{}", err);
        assert!(err.contains(&sha256_hex(content)), "{}", err);

        // A short write.
        fs::write(&path, &content[..4]).unwrap();
        assert!(ws.verify_written(&path, "f.txt", content).is_err());

        // Something appended.
        fs::write(&path, b"fn main() {}\r\n// more\n").unwrap();
        let err = ws.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("more than"), "{}", err);

        // Gone.
        fs::remove_file(&path).unwrap();
        let err = ws.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("could not be verified"), "{}", err);

        fs::write(&path, content).unwrap();
        assert_eq!(
            ws.verify_written(&path, "f.txt", content).unwrap(),
            sha256_hex(content)
        );
    }

    #[test]
    fn test_commit_fails_when_the_file_is_rewritten_before_the_read_back() {
        let s = Scratch::new("rewritten");
        let ws = s.workspace();
        // A formatter that normalizes line endings after the write lands.
        AFTER_WRITE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path: &Path| {
                fs::write(path, "fn main() {}\n").expect("rewrite")
            }))
        });
        let pending = ws.prepare_write("main.rs").expect("prepare");
        let result = ws.commit_write(pending, b"fn main() {}\r\n");
        AFTER_WRITE.with(|hook| *hook.borrow_mut() = None);

        // Hashing what was asked for, instead of what is there, would pass.
        let err = result.unwrap_err();
        assert!(err.contains("failed verification"), "{}", err);
        assert!(err.contains(&sha256_hex(b"fn main() {}\n")), "{}", err);
    }

    #[test]
    fn test_prepared_write_changes_nothing_until_committed() {
        let s = Scratch::new("no_trace");
        let ws = s.workspace();
        fs::write(s.ws().join("keep.txt"), "original").unwrap();

        drop(ws.prepare_write("sub/dir/new.txt").expect("prepare new"));
        assert!(!s.ws().join("sub").exists());

        drop(ws.prepare_write("keep.txt").expect("prepare existing"));
        assert_eq!(fs::read_to_string(s.ws().join("keep.txt")).unwrap(), "original");
    }

    #[test]
    fn test_file_planted_after_the_check_is_not_overwritten() {
        let s = Scratch::new("planted");
        let ws = s.workspace();
        let pending = ws.prepare_write("x.txt").expect("prepare");
        fs::write(s.ws().join("x.txt"), "planted").unwrap();
        assert!(ws.commit_write(pending, b"mine").is_err());
        assert_eq!(fs::read_to_string(s.ws().join("x.txt")).unwrap(), "planted");
    }

    #[test]
    fn test_directory_is_not_a_regular_file() {
        let s = Scratch::new("dir_target");
        fs::create_dir_all(s.ws().join("d")).unwrap();
        let ws = s.workspace();
        let err = ws.prepare_write("d").err().expect("refused");
        assert!(err.contains("a directory"), "{}", err);
    }

    #[test]
    fn test_oversized_file_is_not_replaced() {
        let s = Scratch::new("oversized");
        let file = File::create(s.ws().join("big.bin")).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);
        let err = s.workspace().prepare_write("big.bin").err().expect("refused");
        assert!(err.contains("larger than"), "{}", err);
    }

    #[test]
    fn test_read_up_to_reports_what_it_left_out() {
        assert_eq!(read_up_to(&b"abcdef"[..], 4).unwrap(), (b"abcd".to_vec(), true));
        assert_eq!(read_up_to(&b"abcdef"[..], 6).unwrap(), (b"abcdef".to_vec(), false));
        assert_eq!(read_up_to(&b""[..], 0).unwrap(), (Vec::new(), false));
    }
}
