//! The file layer the tools use, for a host that serves a workspace to a
//! client: the same resolution, the same regular-file checks and the same
//! verified writes, behind a small public surface.
//!
//! `vitna app` (`crates/app-server`, ADR-0006) serves a folder to the browser
//! page. Reusing `workspace_fs` rather than writing a second file layer means
//! one set of containment rules, tested once, decides what a tool and the page
//! can each touch: a path is resolved on the real filesystem and must stay
//! inside the root however many links it crosses, only regular files are
//! opened, a write captures its preimage from the handle it writes through and
//! reads the file back, and nothing larger than [`MAX_FILE_BYTES`] is loaded.
//!
//! Two things are added for a client that is not a tool. A write is
//! compare-and-swap: it names the hash the file must hold now, and a file that
//! holds anything else is left alone. And a file can be removed, under the
//! same comparison, which is what undoing a file the client made needs.

use crate::workspace_fs::{self, read_up_to, Resolved, Workspace};
use std::fs;
use std::path::{Path, PathBuf};

/// The hash a write expects, and a remove records, for a file that does not
/// exist.
pub const ABSENT_PREIMAGE_HASH: &str = workspace_fs::ABSENT_PREIMAGE_HASH;

/// The most bytes read or written in one call.
pub const MAX_FILE_BYTES: u64 = workspace_fs::MAX_FILE_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}

/// One entry of a folder, or what `stat` found at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// A file's size in bytes, when it was asked for. Never set for a folder.
    pub size: Option<u64>,
}

/// What a write did: the hash the file held before, and the hash of what
/// reading it back afterwards returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub preimage_hash: String,
    pub postimage_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    /// Nothing is at the path.
    Missing,
    /// The path or what is there is refused, and the text says why: outside
    /// the root, through a link that leads out, not a regular file, and so on.
    Refused(String),
    /// A file larger than [`MAX_FILE_BYTES`], with its size.
    TooLarge(u64),
    /// The file does not hold what the caller expected, so nothing was
    /// changed. `current` is the hash it holds.
    Changed { current: String },
    /// The operating system failed.
    Failed(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Missing => write!(f, "there is nothing at that path"),
            FileError::Refused(reason) => write!(f, "{reason}"),
            FileError::TooLarge(size) => write!(
                f,
                "the file is {size} bytes, over the {MAX_FILE_BYTES} read or written at once"
            ),
            FileError::Changed { current } => write!(f, "the file changed: it now holds {current}"),
            FileError::Failed(reason) => write!(f, "{reason}"),
        }
    }
}

/// A workspace root, resolved once, served through the tools' file layer.
pub struct WorkspaceFiles {
    ws: Workspace,
}

impl WorkspaceFiles {
    pub fn open(root: &Path) -> Result<Self, String> {
        Ok(Self {
            ws: Workspace::new(root)?,
        })
    }

    fn existing(&self, requested: &str) -> Result<PathBuf, FileError> {
        match self.ws.resolve(requested).map_err(FileError::Refused)? {
            Resolved::Existing(path) => Ok(path),
            Resolved::Missing(_) => Err(FileError::Missing),
        }
    }

    /// Where a path leads once every link and every alias is resolved, as a
    /// path from the root with `/` between names (`.` for the root itself):
    /// the file or folder when it exists, and otherwise the deepest folder
    /// along it that does, followed by the names still missing.
    ///
    /// A host that keeps a client away from some names checks this as well as
    /// the path it was asked for, since a name is not the only way to reach a
    /// file: a link inside the workspace can lead to `.git/config`, and on
    /// Windows the short name `GIT~1` is `.git`.
    pub fn locate(&self, requested: &str) -> Result<String, FileError> {
        match self.ws.resolve(requested).map_err(FileError::Refused)? {
            Resolved::Existing(real) => Ok(self.ws.relative(&real)),
            Resolved::Missing(lexical) => {
                let (real, missing) = self
                    .ws
                    .existing_parent(&lexical, requested)
                    .map_err(FileError::Refused)?;
                let mut located = self.ws.relative(&real);
                let last = lexical.file_name().map(|n| n.to_os_string());
                for name in missing.into_iter().chain(last) {
                    let name = name.to_string_lossy();
                    located = if located == "." {
                        name.into_owned()
                    } else {
                        format!("{located}/{name}")
                    };
                }
                Ok(located)
            }
        }
    }

    /// The real location of an existing folder inside the workspace, for a
    /// command to run in.
    pub fn directory(&self, requested: &str) -> Result<PathBuf, FileError> {
        let path = self.existing(requested)?;
        if path.is_dir() {
            Ok(path)
        } else {
            Err(FileError::Refused(format!("'{requested}' is not a folder")))
        }
    }

    /// What is in a folder, folders first and then files, each by name. A
    /// link is listed as what it leads to, and only when that lies inside the
    /// workspace: one that leads out is left out, since nothing can be read
    /// through it. A name that is not valid Unicode is left out too, since no
    /// client can ask for it back.
    pub fn list(&self, requested: &str, sizes: bool) -> Result<Vec<Entry>, FileError> {
        let dir = self.existing(requested)?;
        let meta = fs::metadata(&dir)
            .map_err(|e| FileError::Failed(format!("Cannot read '{requested}': {e}")))?;
        if !meta.is_dir() {
            return Err(FileError::Refused(format!("'{requested}' is not a folder")));
        }
        let read = fs::read_dir(&dir)
            .map_err(|e| FileError::Failed(format!("Cannot list '{requested}': {e}")))?;
        let mut entries = Vec::new();
        for item in read {
            let item =
                item.map_err(|e| FileError::Failed(format!("Cannot list '{requested}': {e}")))?;
            let Ok(name) = item.file_name().into_string() else {
                continue;
            };
            let Ok(kind) = item.file_type() else { continue };
            let (kind, size) = if kind.is_symlink() {
                match fs::canonicalize(item.path()) {
                    Ok(real) if self.ws.contains(&real) => match fs::metadata(&real) {
                        Ok(target) if target.is_dir() => (EntryKind::Directory, None),
                        Ok(target) if target.is_file() => {
                            (EntryKind::File, sizes.then_some(target.len()))
                        }
                        _ => continue,
                    },
                    _ => continue,
                }
            } else if kind.is_dir() {
                (EntryKind::Directory, None)
            } else if kind.is_file() {
                let size = if sizes {
                    item.metadata().ok().map(|m| m.len())
                } else {
                    None
                };
                (EntryKind::File, size)
            } else {
                continue;
            };
            entries.push(Entry { name, kind, size });
        }
        entries.sort_by(|a, b| {
            (a.kind == EntryKind::File)
                .cmp(&(b.kind == EntryKind::File))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// What is at a path: a folder, or a file and its size.
    pub fn stat(&self, requested: &str) -> Result<Entry, FileError> {
        let path = self.existing(requested)?;
        let meta = fs::metadata(&path)
            .map_err(|e| FileError::Failed(format!("Cannot read '{requested}': {e}")))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if meta.is_dir() {
            Ok(Entry {
                name,
                kind: EntryKind::Directory,
                size: None,
            })
        } else if meta.is_file() {
            Ok(Entry {
                name,
                kind: EntryKind::File,
                size: Some(meta.len()),
            })
        } else {
            Err(FileError::Refused(format!(
                "'{requested}' is neither a file nor a folder"
            )))
        }
    }

    /// A regular file's bytes, whole.
    pub fn read(&self, requested: &str) -> Result<Vec<u8>, FileError> {
        let path = self.existing(requested)?;
        let mut file = self
            .ws
            .open_regular(&path, requested)
            .map_err(FileError::Refused)?;
        let (bytes, longer) = read_up_to(&mut file, MAX_FILE_BYTES)
            .map_err(|e| FileError::Failed(format!("Cannot read '{requested}': {e}")))?;
        if longer {
            let size = file
                .metadata()
                .map(|m| m.len())
                .unwrap_or(MAX_FILE_BYTES + 1);
            return Err(FileError::TooLarge(size));
        }
        Ok(bytes)
    }

    /// Writes `content` only if the file now holds what `expected` says:
    /// its SHA-256, or [`ABSENT_PREIMAGE_HASH`] for a file that must not exist
    /// yet. Anything else leaves the file alone and names what it holds.
    pub fn write(
        &self,
        requested: &str,
        expected: &str,
        content: &[u8],
    ) -> Result<Written, FileError> {
        if content.len() as u64 > MAX_FILE_BYTES {
            return Err(FileError::TooLarge(content.len() as u64));
        }
        let pending = self
            .ws
            .prepare_write(requested)
            .map_err(FileError::Refused)?;
        if pending.preimage_hash != expected {
            return Err(FileError::Changed {
                current: pending.preimage_hash.clone(),
            });
        }
        let preimage_hash = pending.preimage_hash.clone();
        let postimage_hash = self
            .ws
            .commit_write(pending, content)
            .map_err(FileError::Failed)?;
        Ok(Written {
            preimage_hash,
            postimage_hash,
        })
    }

    /// Removes a regular file only if it holds what `expected` says, and
    /// removes the very file that was hashed: one put in its place meanwhile
    /// is left alone. Returns the hash it held.
    pub fn remove(&self, requested: &str, expected: &str) -> Result<String, FileError> {
        let path = self.existing(requested)?;
        let pending = self
            .ws
            .prepare_remove(&path, requested)
            .map_err(FileError::Refused)?;
        if pending.preimage_hash != expected {
            return Err(FileError::Changed {
                current: pending.preimage_hash.clone(),
            });
        }
        let held = pending.preimage_hash.clone();
        self.ws.commit_remove(pending).map_err(FileError::Failed)?;
        Ok(held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_fs::sha256_hex;

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "vitna_workspace_files_{tag}_{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws").join("src")).expect("create workspace");
            fs::write(
                dir.join("ws").join("src").join("greet.js"),
                b"export const hi = 1;\n",
            )
            .expect("write file");
            fs::write(dir.join("ws").join("README.md"), b"# Hi\n").expect("write file");
            fs::write(dir.join("outside.txt"), b"not yours\n").expect("write outside");
            Self(dir)
        }
        fn files(&self) -> WorkspaceFiles {
            WorkspaceFiles::open(&self.0.join("ws")).expect("open workspace")
        }
        fn at(&self, path: &str) -> std::path::PathBuf {
            self.0.join("ws").join(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_folder_lists_folders_first_then_files_with_their_sizes_when_asked() {
        let fx = Fixture::new("list");
        let files = fx.files();
        let listed = files.list("", true).expect("list the top");
        assert_eq!(
            listed,
            vec![
                Entry {
                    name: "src".into(),
                    kind: EntryKind::Directory,
                    size: None
                },
                Entry {
                    name: "README.md".into(),
                    kind: EntryKind::File,
                    size: Some(5)
                },
            ]
        );
        assert_eq!(
            files.list("", false).expect("list")[1].size,
            None,
            "a size was read that was not asked for"
        );
        assert_eq!(files.list("nowhere", true), Err(FileError::Missing));
        assert!(matches!(
            files.list("README.md", true),
            Err(FileError::Refused(_))
        ));
    }

    #[test]
    fn nothing_outside_the_root_is_read_or_listed() {
        let fx = Fixture::new("outside");
        let files = fx.files();
        assert!(matches!(
            files.read("../outside.txt"),
            Err(FileError::Refused(_))
        ));
        assert!(matches!(files.list("..", true), Err(FileError::Refused(_))));
        assert_eq!(
            files.read("src/greet.js").expect("read"),
            b"export const hi = 1;\n"
        );
        assert_eq!(files.stat("src").expect("stat").kind, EntryKind::Directory);
        assert_eq!(files.stat("src/greet.js").expect("stat").size, Some(21));
    }

    #[test]
    fn a_write_happens_only_over_the_bytes_it_expects() {
        let fx = Fixture::new("write");
        let files = fx.files();
        let before = sha256_hex(b"# Hi\n");
        let stale = sha256_hex(b"something else\n");
        assert_eq!(
            files.write("README.md", &stale, b"# Changed\n"),
            Err(FileError::Changed {
                current: before.clone()
            })
        );
        assert_eq!(
            fs::read(fx.at("README.md")).expect("read back"),
            b"# Hi\n",
            "a refused write changed the file"
        );
        let written = files
            .write("README.md", &before, b"# Changed\n")
            .expect("write");
        assert_eq!(
            written,
            Written {
                preimage_hash: before,
                postimage_hash: sha256_hex(b"# Changed\n")
            }
        );
        assert_eq!(
            fs::read(fx.at("README.md")).expect("read back"),
            b"# Changed\n"
        );
    }

    #[test]
    fn a_new_file_is_made_only_where_nothing_is() {
        let fx = Fixture::new("create");
        let files = fx.files();
        let made = files
            .write("notes/NOTES.md", ABSENT_PREIMAGE_HASH, b"# Notes\n")
            .expect("make");
        assert_eq!(made.preimage_hash, ABSENT_PREIMAGE_HASH);
        assert_eq!(
            fs::read(fx.at("notes/NOTES.md")).expect("read back"),
            b"# Notes\n"
        );
        assert_eq!(
            files.write("notes/NOTES.md", ABSENT_PREIMAGE_HASH, b"again\n"),
            Err(FileError::Changed {
                current: sha256_hex(b"# Notes\n")
            }),
            "a file that exists was written as new"
        );
        assert!(matches!(
            files.write("../escape.txt", ABSENT_PREIMAGE_HASH, b"x"),
            Err(FileError::Refused(_))
        ));
        assert!(!fx.0.join("escape.txt").exists());
    }

    #[test]
    fn locate_names_where_a_path_really_leads() {
        let fx = Fixture::new("locate");
        let files = fx.files();
        assert_eq!(files.locate("").expect("the root"), ".");
        assert_eq!(
            files.locate("./src//greet.js").expect("a file"),
            "src/greet.js"
        );
        assert_eq!(
            files.locate("src/new/deeper.txt").expect("a new file"),
            "src/new/deeper.txt"
        );
        assert_eq!(
            files.locate("fresh.txt").expect("a new file at the top"),
            "fresh.txt"
        );
        assert!(matches!(
            files.locate("../outside.txt"),
            Err(FileError::Refused(_))
        ));
        assert!(
            matches!(files.locate("README.md/inside"), Err(FileError::Refused(_))),
            "a file was treated as a folder"
        );
    }

    #[test]
    fn a_command_folder_is_an_existing_folder_inside() {
        let fx = Fixture::new("directory");
        let files = fx.files();
        let src = files.directory("src").expect("a folder");
        assert!(
            src.is_absolute() && src.ends_with("src"),
            "{}",
            src.display()
        );
        assert!(matches!(
            files.directory("README.md"),
            Err(FileError::Refused(_))
        ));
        assert_eq!(files.directory("nowhere"), Err(FileError::Missing));
        assert!(matches!(files.directory(".."), Err(FileError::Refused(_))));
    }

    #[test]
    fn a_file_is_removed_only_while_it_holds_what_was_expected() {
        let fx = Fixture::new("remove");
        let files = fx.files();
        let held = sha256_hex(b"# Hi\n");
        assert_eq!(
            files.remove("README.md", &sha256_hex(b"other")),
            Err(FileError::Changed {
                current: held.clone()
            })
        );
        assert!(
            fx.at("README.md").exists(),
            "a refused remove removed the file"
        );
        assert_eq!(files.remove("README.md", &held), Ok(held));
        assert!(!fx.at("README.md").exists());
        assert_eq!(
            files.remove("README.md", ABSENT_PREIMAGE_HASH),
            Err(FileError::Missing)
        );
        assert!(
            matches!(
                files.remove("src", ABSENT_PREIMAGE_HASH),
                Err(FileError::Refused(_))
            ),
            "a folder was removed"
        );
    }
}
