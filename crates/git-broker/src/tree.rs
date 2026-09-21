//! The primary checkout and agent workspaces, read and written without
//! following links.
//!
//! A changeset names files by relative path, and the broker resolves those
//! paths in two trees: the agent workspace it merges from and the primary
//! checkout it merges into. Both can hold links. A repository can commit
//! `notes.txt -> ~/.bashrc` or `docs -> /etc`, and an agent can make any link
//! it likes in its own workspace. Opening such a path the ordinary way puts
//! the link's target into a changeset's diff, or merges an agent's bytes into
//! whatever file the link names.
//!
//! So a path is walked one name at a time from the tree's root, whose own
//! links are resolved once, up front. Every directory on the way must be a
//! real directory, and the path itself a regular file or nothing. A link
//! anywhere on the way is refused, including one that stays inside the tree:
//! a changeset describes regular files only, so it cannot say whether a merge
//! should write through such a link or replace it, and writing through it
//! changes a file other than the one the changeset names, possibly one the
//! same changeset also changes, with both preimage checks passing.
//! Directories are created one level at a time, never through a link. Files
//! are opened without following a final link or blocking on a FIFO, and the
//! opened handle must then report exactly the path that was walked.
//!
//! The platform pieces at the bottom (`no_follow_no_block`, `opened_path`,
//! `same_file`, `describe`, `is_within`) repeat `vitna_tools::workspace_fs`
//! and `vitna_tools::path_safety`, which this crate cannot depend on.

use crate::broker::NULL_HASH;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What a tree holds at a changeset path.
pub(crate) enum Entry {
    /// Nothing. Every directory on the way that does exist is a real one.
    Absent,
    /// Nothing, and nothing can be created there, because something other
    /// than a directory stands where a directory is needed. The reason says
    /// what.
    Blocked(String),
    /// A regular file, reached through real directories only.
    File { bytes: Vec<u8>, permissions: Permissions },
    /// Something the broker neither reads nor writes: a link at the path or
    /// at a directory above it, or a directory, FIFO, socket or device where
    /// a file belongs. The reason says which.
    Refused(String),
}

/// A directory tree whose paths are resolved without following links.
pub(crate) struct Tree {
    /// How messages refer to the tree, such as "the primary checkout".
    name: &'static str,
    /// The root, its own links resolved once. Nothing below it is ever
    /// resolved through a link.
    root: PathBuf,
}

impl Tree {
    pub(crate) fn new(root: &Path, name: &'static str) -> Result<Self, String> {
        let real = fs::canonicalize(root)
            .map_err(|e| format!("Cannot resolve {} at '{}': {}", name, root.display(), e))?;
        if !real.is_dir() {
            return Err(format!(
                "Cannot use '{}' as {}: it is not a directory",
                root.display(),
                name
            ));
        }
        Ok(Self { name, root: real })
    }

    /// What the tree holds at `rel`. No link is followed, and a regular file
    /// is read through a handle checked to be that very file.
    pub(crate) fn entry(&self, rel: &str) -> Result<Entry, String> {
        let names = self.names(rel)?;
        let mut path = self.root.clone();
        for (depth, name) in names.iter().enumerate() {
            path.push(name);
            let Some(meta) = self.lstat(&path, rel)? else {
                return Ok(Entry::Absent);
            };
            if meta.file_type().is_symlink() {
                return Ok(Entry::Refused(self.link(&path, &names[..=depth])));
            }
            let last = depth + 1 == names.len();
            if !last && !meta.is_dir() {
                return Ok(Entry::Blocked(format!(
                    "'{}' is {}, not a directory",
                    shown(&names[..=depth]),
                    describe(&meta)
                )));
            }
            if last && !meta.is_file() {
                return Ok(Entry::Refused(format!(
                    "'{}' is {}, not a regular file",
                    rel,
                    describe(&meta)
                )));
            }
        }

        let mut options = OpenOptions::new();
        options.read(true);
        let mut file = open_no_follow(&mut options, &path)
            .map_err(|e| format!("Cannot open '{}' in {}: {}", rel, self.name, e))?;
        let meta = self.verify_opened(&file, &path, rel)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read '{}' in {}: {}", rel, self.name, e))?;
        Ok(Entry::File {
            bytes,
            permissions: meta.permissions(),
        })
    }

    /// Makes `rel` hold exactly `content`, provided it still holds the
    /// preimage a changeset recorded for it: the regular file hashing to
    /// `preimage_hash`, or nothing when that is `NULL_HASH`. The check is
    /// made through the handle about to be written, so the file replaced is
    /// the file whose hash was checked. Missing directories are created one
    /// level at a time, never through a link. The write is flushed and read
    /// back, and a file that does not then hold exactly `content` is an
    /// error.
    pub(crate) fn write(
        &self,
        rel: &str,
        preimage_hash: &str,
        content: &[u8],
        permissions: &Permissions,
    ) -> Result<(), String> {
        let names = self.names(rel)?;
        let (file_name, dirs) = names.split_last().expect("names() refuses an empty path");
        let mut path = self.root.clone();
        for (depth, name) in dirs.iter().enumerate() {
            path.push(name);
            let dir = &names[..=depth];
            match self.lstat(&path, rel)? {
                Some(meta) if meta.file_type().is_symlink() => {
                    return Err(self.cannot_write(rel, &self.link(&path, dir)));
                }
                Some(meta) if meta.is_dir() => {}
                Some(meta) => {
                    let reason = format!("'{}' is {}, not a directory", shown(dir), describe(&meta));
                    return Err(self.cannot_write(rel, &reason));
                }
                None => self.create_dir(&path, rel, dir)?,
            }
        }
        path.push(file_name);
        match self.lstat(&path, rel)? {
            Some(meta) if meta.file_type().is_symlink() => {
                return Err(self.cannot_write(rel, &self.link(&path, &names)));
            }
            Some(meta) if !meta.is_file() => {
                let reason = format!("it is {}, not a regular file", describe(&meta));
                return Err(self.cannot_write(rel, &reason));
            }
            _ => {}
        }

        let mut file = if preimage_hash == NULL_HASH {
            // create_new refuses anything already there, a link included, so
            // nothing that appeared since the preimage check is written
            // through or over.
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            let file = open_no_follow(&mut options, &path).map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    self.cannot_write(rel, "something appeared there after it was found absent")
                } else {
                    format!("Cannot create '{}' in {}: {}", rel, self.name, e)
                }
            })?;
            self.verify_opened(&file, &path, rel)?;
            file
        } else {
            let mut options = OpenOptions::new();
            options.read(true).write(true);
            let mut file = open_no_follow(&mut options, &path)
                .map_err(|e| format!("Cannot open '{}' in {} for writing: {}", rel, self.name, e))?;
            self.verify_opened(&file, &path, rel)?;
            let mut current = Vec::new();
            file.read_to_end(&mut current)
                .map_err(|e| format!("Cannot read '{}' in {}: {}", rel, self.name, e))?;
            let found = sha256_hex(&current);
            if found != preimage_hash {
                let reason = format!(
                    "it changed after its preimage was checked (expected SHA-256 {}, found {})",
                    preimage_hash, found
                );
                return Err(self.cannot_write(rel, &reason));
            }
            file.set_len(0)
                .and_then(|()| file.seek(SeekFrom::Start(0)))
                .map_err(|e| format!("Failed to write '{}' in {}: {}", rel, self.name, e))?;
            file
        };

        // Flushed before the handle is dropped: `File`'s drop discards any
        // error from close, and some filesystems report write failures only
        // when flushed.
        file.write_all(content)
            .and_then(|()| file.set_permissions(permissions.clone()))
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("Failed to write '{}' in {}: {}", rel, self.name, e))?;
        drop(file);
        self.verify_written(&path, rel, content)
    }

    /// Reads a just-written file back through a freshly checked handle and
    /// compares it with what was written, byte for byte.
    fn verify_written(&self, path: &Path, rel: &str, content: &[u8]) -> Result<(), String> {
        let unverified = |detail: &dyn std::fmt::Display| {
            format!(
                "Write to '{}' in {} could not be verified: {}",
                rel, self.name, detail
            )
        };
        let mut options = OpenOptions::new();
        options.read(true);
        let file = open_no_follow(&mut options, path).map_err(|e| unverified(&e))?;
        self.verify_opened(&file, path, rel).map_err(|e| unverified(&e))?;
        // One byte past what was written is enough to tell that the file grew.
        let mut back = Vec::new();
        file.take(content.len() as u64 + 1)
            .read_to_end(&mut back)
            .map_err(|e| unverified(&e))?;
        if back == content {
            return Ok(());
        }
        let found = if back.len() > content.len() {
            format!("more than {} bytes", content.len())
        } else {
            format!("{} bytes (SHA-256 {})", back.len(), sha256_hex(&back))
        };
        Err(format!(
            "Write to '{}' in {} failed verification: {} bytes were written (SHA-256 {}), but reading the file back returned {}",
            rel,
            self.name,
            content.len(),
            sha256_hex(content),
            found
        ))
    }

    /// Splits a changeset path into the names it walks. It must be relative
    /// and name every directory on the way down, the only form
    /// `inspect_changes` produces: a root, a drive, `.` or `..` is refused.
    fn names<'a>(&self, rel: &'a str) -> Result<Vec<&'a OsStr>, String> {
        let mut names = Vec::new();
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(name) => names.push(name),
                _ => return Err(format!("'{}' is not a path inside {}", rel, self.name)),
            }
        }
        if names.is_empty() {
            return Err(format!("'{}' does not name a file inside {}", rel, self.name));
        }
        Ok(names)
    }

    /// The entry's own metadata, links not followed. `None` when nothing is
    /// there.
    fn lstat(&self, path: &Path, rel: &str) -> Result<Option<fs::Metadata>, String> {
        match fs::symlink_metadata(path) {
            Ok(meta) => Ok(Some(meta)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Cannot inspect '{}' in {}: {}", rel, self.name, e)),
        }
    }

    /// Creates one missing directory. Something else made at that name in
    /// the meantime is accepted only if it is a real directory.
    fn create_dir(&self, path: &Path, rel: &str, dir: &[&OsStr]) -> Result<(), String> {
        match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match self.lstat(path, rel)? {
                Some(meta) if meta.is_dir() => Ok(()),
                _ => {
                    let reason = format!("'{}' appeared as something other than a directory", shown(dir));
                    Err(self.cannot_write(rel, &reason))
                }
            },
            Err(e) => Err(format!(
                "Cannot create directory '{}' in {}: {}",
                shown(dir),
                self.name,
                e
            )),
        }
    }

    /// Checks what a handle opened: a regular file at exactly the path the
    /// walk reached. Anything else means a link or a rename intervened
    /// between the walk and the open.
    fn verify_opened(&self, file: &File, expected: &Path, rel: &str) -> Result<fs::Metadata, String> {
        let meta = file
            .metadata()
            .map_err(|e| format!("Cannot inspect '{}' in {}: {}", rel, self.name, e))?;
        if !meta.is_file() {
            return Err(format!(
                "'{}' in {} is {}, not a regular file",
                rel,
                self.name,
                describe(&meta)
            ));
        }
        let verified = match opened_path(file) {
            Ok(Some(actual)) => same_path(&actual, expected),
            // Nothing on this platform says where a handle lives. Settle for
            // identity, with every directory on the way still a real one.
            Ok(None) => {
                fs::symlink_metadata(expected).is_ok_and(|now| same_file(&meta, &now))
                    && self.directories_are_real(expected)
            }
            Err(e) => {
                return Err(format!(
                    "Cannot verify where '{}' in {} was opened: {}",
                    rel, self.name, e
                ))
            }
        };
        if verified {
            Ok(meta)
        } else {
            Err(format!(
                "'{}' in {} was moved, or reached through a link, while it was being opened",
                rel, self.name
            ))
        }
    }

    /// True when every directory between the root and `path` is a real
    /// directory, not a link.
    fn directories_are_real(&self, path: &Path) -> bool {
        let Ok(below) = path.strip_prefix(&self.root) else {
            return false;
        };
        let mut dirs: Vec<Component> = below.components().collect();
        dirs.pop();
        let mut dir = self.root.clone();
        dirs.into_iter().all(|name| {
            dir.push(name);
            fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
        })
    }

    /// Why the link at `link`, reached by `names`, is refused. Where a link
    /// inside the tree leads is named; where one outside leads is not.
    fn link(&self, link: &Path, names: &[&OsStr]) -> String {
        let shown = shown(names);
        match fs::canonicalize(link) {
            Ok(target) if is_within(&target, &self.root) => format!(
                "'{}' is a link to '{}' inside {}, and links are not followed, even inside it",
                shown,
                self.relative(&target),
                self.name
            ),
            Ok(_) => format!(
                "'{}' is a link that leads outside {}, and links are not followed",
                shown, self.name
            ),
            Err(_) => format!(
                "'{}' is a link whose target does not exist, and links are not followed",
                shown
            ),
        }
    }

    fn cannot_write(&self, rel: &str, reason: &str) -> String {
        format!("Cannot write '{}' in {}: {}", rel, self.name, reason)
    }

    /// A resolved path relative to the root, with `/` separators.
    fn relative(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => path.display().to_string(),
        }
    }
}

fn shown(names: &[&OsStr]) -> String {
    let names: Vec<_> = names.iter().map(|name| name.to_string_lossy()).collect();
    names.join("/")
}

fn open_no_follow(options: &mut OpenOptions, path: &Path) -> io::Result<File> {
    no_follow_no_block(options);
    options.open(path)
}

/// True when `a` and `b` name the same path, component by component.
fn same_path(a: &Path, b: &Path) -> bool {
    a.components().count() == b.components().count() && is_within(a, b)
}

/// Component-wise containment, case-insensitive only where the filesystem
/// is. The same rule as `vitna_tools::path_safety::is_within`.
fn is_within(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|r| {
        parts
            .next()
            .is_some_and(|p| same_component(r.as_os_str(), p.as_os_str()))
    })
}

#[cfg(windows)]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(not(windows))]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a == b
}

fn describe(meta: &fs::Metadata) -> &'static str {
    let kind = meta.file_type();
    if kind.is_dir() {
        return "a directory";
    }
    if kind.is_symlink() {
        return "a link";
    }
    if kind.is_file() {
        return "a regular file";
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

/// O_NOFOLLOW makes a final component swapped for a link after the walk
/// fail to open rather than be followed. O_NONBLOCK keeps a FIFO swapped in
/// after the walk from blocking the open; the handle check refuses it.
#[cfg(unix)]
fn no_follow_no_block(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// Windows keeps FIFOs out of the file namespace, and a link swapped in
/// after the walk is caught by the handle's final path.
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
    use std::ffi::CStr;
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
    use std::ffi::OsString;
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

    /// A tree, and a directory outside it. Removed on drop.
    struct Scratch {
        base: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir()
                .join(format!("vitna_broker_tree_{}_{}", name, std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(base.join("tree")).expect("create tree");
            fs::create_dir_all(base.join("outside")).expect("create outside dir");
            Self { base }
        }

        fn root(&self) -> PathBuf {
            self.base.join("tree")
        }

        fn outside(&self) -> PathBuf {
            self.base.join("outside")
        }

        fn tree(&self) -> Tree {
            Tree::new(&self.root(), "the test tree").expect("resolve tree root")
        }

        /// Permissions of an ordinary new file, for writes.
        fn plain(&self) -> Permissions {
            let path = self.base.join("plain.txt");
            fs::write(&path, "").expect("write plain file");
            fs::metadata(&path).expect("stat plain file").permissions()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    /// Links `link` to the absolute directory `target`. Windows without the
    /// symlink privilege gets a junction, which needs none.
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

    fn file_bytes(entry: Entry) -> Vec<u8> {
        match entry {
            Entry::File { bytes, .. } => bytes,
            Entry::Absent => panic!("expected a file, found nothing"),
            Entry::Blocked(reason) | Entry::Refused(reason) => panic!("expected a file: {}", reason),
        }
    }

    fn refusal(entry: Entry) -> String {
        match entry {
            Entry::Refused(reason) => reason,
            Entry::Absent => panic!("expected a refusal, found nothing"),
            Entry::Blocked(reason) => panic!("expected a refusal, found a blocked path: {}", reason),
            Entry::File { .. } => panic!("expected a refusal, found a file"),
        }
    }

    #[test]
    fn test_paths_that_leave_the_tree_are_malformed() {
        let s = Scratch::new("names");
        let tree = s.tree();
        let absolute = s.outside().join("x.txt");
        for rel in ["../x.txt", "a/../../x.txt", "", ".", "./a.txt", absolute.to_str().unwrap()] {
            assert!(tree.entry(rel).is_err(), "'{}' must be refused", rel);
            assert!(tree.write(rel, NULL_HASH, b"x", &s.plain()).is_err(), "'{}'", rel);
        }
        assert!(!absolute.exists());
        assert_eq!(
            tree.names("a/b.rs").unwrap(),
            vec![OsStr::new("a"), OsStr::new("b.rs")]
        );
    }

    /// The handle check compares the path the walk built with the path the
    /// platform reports for the opened handle. If the two forms ever
    /// disagreed, every read would be refused.
    #[test]
    fn test_reads_a_regular_file_through_real_directories() {
        let s = Scratch::new("read");
        fs::create_dir_all(s.root().join("a/b")).unwrap();
        fs::write(s.root().join("a/b/f.txt"), "hello\r\n").unwrap();
        let tree = s.tree();
        assert_eq!(file_bytes(tree.entry("a/b/f.txt").unwrap()), b"hello\r\n");
        assert!(matches!(tree.entry("a/b/missing.txt").unwrap(), Entry::Absent));
        assert!(matches!(tree.entry("a/missing/f.txt").unwrap(), Entry::Absent));
    }

    #[test]
    fn test_what_is_not_a_regular_file_is_classified_without_being_opened() {
        let s = Scratch::new("classify");
        fs::create_dir_all(s.root().join("dir")).unwrap();
        fs::write(s.root().join("file"), "x").unwrap();
        let tree = s.tree();
        assert!(refusal(tree.entry("dir").unwrap()).contains("a directory"));
        match tree.entry("file/below.txt").unwrap() {
            Entry::Blocked(reason) => assert!(reason.contains("not a directory"), "{}", reason),
            _ => panic!("a file where a directory belongs blocks the path"),
        }
    }

    #[test]
    fn test_links_are_refused_whether_they_lead_out_or_stay_in() {
        let s = Scratch::new("links");
        fs::create_dir_all(s.root().join("real")).unwrap();
        fs::write(s.root().join("real/f.txt"), "inside\n").unwrap();
        fs::write(s.outside().join("f.txt"), "outside\n").unwrap();
        if !dir_link(&s.outside(), &s.root().join("out"))
            || !dir_link(&s.root().join("real"), &s.root().join("in"))
        {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let tree = s.tree();

        let reason = refusal(tree.entry("out/f.txt").unwrap());
        assert!(reason.contains("outside"), "{}", reason);
        assert!(!reason.contains(&s.outside().display().to_string()), "{}", reason);

        let reason = refusal(tree.entry("in/f.txt").unwrap());
        assert!(reason.contains("'real' inside"), "{}", reason);
        // The link itself, not only paths through it.
        assert!(refusal(tree.entry("in").unwrap()).contains("inside"));
    }

    /// The walk refuses links before anything is opened, so this stands in
    /// for a link swapped in between the walk and the open: the handle must
    /// be refused for living somewhere other than the path that was walked.
    #[test]
    fn test_handle_opened_through_a_link_is_refused() {
        let s = Scratch::new("handle_link");
        fs::write(s.outside().join("f.txt"), "outside\n").unwrap();
        fs::write(s.root().join("f.txt"), "inside\n").unwrap();
        if !dir_link(&s.outside(), &s.root().join("leak")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let tree = s.tree();

        let walked = tree.root.join("leak").join("f.txt");
        let file = File::open(&walked).expect("open through the link");
        let err = tree.verify_opened(&file, &walked, "leak/f.txt").unwrap_err();
        assert!(err.contains("reached through a link"), "{}", err);

        let walked = tree.root.join("f.txt");
        let file = File::open(&walked).expect("open directly");
        tree.verify_opened(&file, &walked, "f.txt").expect("the walked path itself");
    }

    #[test]
    fn test_write_never_goes_through_a_directory_link() {
        let s = Scratch::new("write_link");
        if !dir_link(&s.outside(), &s.root().join("leak")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let tree = s.tree();
        for rel in ["leak/new.txt", "leak/deeper/new.txt", "leak"] {
            for preimage in [NULL_HASH.to_string(), sha256_hex(b"")] {
                let err = tree.write(rel, &preimage, b"x", &s.plain()).unwrap_err();
                assert!(err.contains("link"), "{}", err);
            }
        }
        assert!(
            fs::read_dir(s.outside()).unwrap().next().is_none(),
            "something was created through the link"
        );
    }

    #[test]
    fn test_write_creates_directories_and_replaces_only_the_checked_preimage() {
        let s = Scratch::new("write");
        let tree = s.tree();
        let path = s.root().join("a/b/new.txt");

        tree.write("a/b/new.txt", NULL_HASH, b"one\n", &s.plain()).expect("create");
        assert_eq!(fs::read(&path).unwrap(), b"one\n");

        let err = tree.write("a/b/new.txt", NULL_HASH, b"two\n", &s.plain()).unwrap_err();
        assert!(err.contains("appeared"), "{}", err);
        let stale = sha256_hex(b"stale");
        let err = tree.write("a/b/new.txt", &stale, b"two\n", &s.plain()).unwrap_err();
        assert!(err.contains("changed after its preimage was checked"), "{}", err);
        assert_eq!(fs::read(&path).unwrap(), b"one\n", "a refused write changed the file");

        let current = sha256_hex(b"one\n");
        tree.write("a/b/new.txt", &current, b"2", &s.plain()).expect("shorter rewrite");
        assert_eq!(fs::read(&path).unwrap(), b"2");
    }

    #[test]
    fn test_file_that_differs_after_the_write_is_a_failed_write() {
        let s = Scratch::new("verify");
        let tree = s.tree();
        let path = tree.root.join("f.txt");
        let content = b"fn main() {}\r\n";

        // A formatter rewrote it.
        fs::write(&path, "fn main() {}\n").unwrap();
        let err = tree.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("failed verification"), "{}", err);
        assert!(err.contains(&sha256_hex(content)), "{}", err);

        // A short write.
        fs::write(&path, &content[..4]).unwrap();
        assert!(tree.verify_written(&path, "f.txt", content).is_err());

        // Something appended.
        fs::write(&path, b"fn main() {}\r\n// more\n").unwrap();
        let err = tree.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("more than"), "{}", err);

        // Gone.
        fs::remove_file(&path).unwrap();
        let err = tree.verify_written(&path, "f.txt", content).unwrap_err();
        assert!(err.contains("could not be verified"), "{}", err);

        fs::write(&path, content).unwrap();
        tree.verify_written(&path, "f.txt", content).expect("identical bytes verify");
    }
}
