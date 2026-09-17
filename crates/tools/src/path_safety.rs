use std::path::{Component, Path, PathBuf};

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

    let mut normalized = PathBuf::new();
    for component in combined.components() {
        match component {
            Component::Prefix(p) => normalized.push(Component::Prefix(p)),
            Component::RootDir => normalized.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!(
                        "Path traversal attempt detected: '{}' escapes workspace root",
                        requested_path
                    ));
                }
            }
            Component::Normal(n) => normalized.push(n),
        }
    }

    let root_str = root.to_string_lossy().to_lowercase().replace('\\', "/");
    let norm_str = normalized.to_string_lossy().to_lowercase().replace('\\', "/");

    if !norm_str.starts_with(&root_str) {
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
