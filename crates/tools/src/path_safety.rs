use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Component equality, case insensitive only where the filesystem is.
/// Lowercasing everywhere would make `/WORKSPACE` and `/workspace` the same
/// directory on Linux, where they are not, which would itself be an escape.
#[cfg(windows)]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(not(windows))]
fn same_component(a: &OsStr, b: &OsStr) -> bool {
    a == b
}

/// Collapses `.` and `..` lexically. Returns None when `..` walks above the root.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => out.push(Component::Prefix(p)),
            Component::RootDir => out.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(n) => out.push(n),
        }
    }
    Some(out)
}

/// True when `path` is `root` or lies beneath it. Containment is compared
/// COMPONENT BY COMPONENT: a string prefix test accepts /workspace_other as
/// living inside /workspace, which is how a sibling directory used to pass
/// this guard. Both paths must already be normalized or canonical; nothing
/// here touches the filesystem.
pub(crate) fn is_within(path: &Path, root: &Path) -> bool {
    let root_parts: Vec<&OsStr> = root.components().map(|c| c.as_os_str()).collect();
    let path_parts: Vec<&OsStr> = path.components().map(|c| c.as_os_str()).collect();

    path_parts.len() >= root_parts.len()
        && root_parts
            .iter()
            .zip(path_parts.iter())
            .all(|(r, p)| same_component(r, p))
}

/// Resolves a requested relative or absolute path against workspace_root,
/// strictly enforcing that the resolved path does not escape the workspace boundary.
pub fn resolve_workspace_path<P: AsRef<Path>>(workspace_root: P, requested_path: &str) -> Result<PathBuf, String> {
    let root = workspace_root.as_ref();
    let requested = Path::new(requested_path);

    let combined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };

    let normalized = normalize(&combined).ok_or_else(|| {
        format!(
            "Path traversal attempt detected: '{}' escapes workspace root",
            requested_path
        )
    })?;
    let root_normalized = normalize(root).ok_or_else(|| {
        format!("Workspace root is not a usable path: '{}'", root.display())
    })?;

    // This check is lexical. It cannot see links; `workspace_fs` repeats it
    // on the path the operating system actually resolves.
    if !is_within(&normalized, &root_normalized) {
        return Err(format!(
            "Access denied: path '{}' escapes workspace root '{}'",
            requested_path,
            root.display()
        ));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_workspace_paths() {
        let root = Path::new("/workspace");
        let res = resolve_workspace_path(root, "src/main.rs").unwrap();
        assert_eq!(res, Path::new("/workspace/src/main.rs"));

        let res2 = resolve_workspace_path(root, "./subdir/../src/lib.rs").unwrap();
        assert_eq!(res2, Path::new("/workspace/src/lib.rs"));
    }

    #[test]
    fn test_traversal_rejection() {
        let root = Path::new("/workspace");
        assert!(resolve_workspace_path(root, "../../etc/passwd").is_err());
        assert!(resolve_workspace_path(root, "../workspace_other/file").is_err());
        assert!(resolve_workspace_path(root, "/outside/file").is_err());
    }
}
