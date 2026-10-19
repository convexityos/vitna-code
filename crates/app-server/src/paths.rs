//! The page's own path rules, held again here, where they are the boundary.
//!
//! These mirror `resolvePath` and `blocked` in convexityos/vitna
//! `chat/lib/folder.mjs`, so a path the page would refuse is refused by the
//! server too, with the same sentence. The page's copy is a courtesy to the
//! person; this one is what stands between a page and the disk. One name is
//! added: `.vitna`, where this server keeps the receipts it signs.

/// Folders never listed inside, read or written, at any depth, in any case.
const SKIPPED: [&str; 3] = [".git", "node_modules", ".vitna"];

/// The longest path looked up, in UTF-16 units as the page counts, and the
/// deepest.
const MAX_PATH_LENGTH: usize = 1024;
const MAX_DEPTH: usize = 64;

/// A path as the page wrote it, reduced to the names it walks through from the
/// folder's top, or the reason it is refused. `""`, `"."` and `"/"` all name
/// the top, and a backslash is read as a slash.
pub(crate) fn resolve(input: &str) -> Result<Vec<String>, String> {
    let raw = input.trim().replace('\\', "/");
    if raw.encode_utf16().count() > MAX_PATH_LENGTH {
        return Err("the path is too long".to_string());
    }
    if matches!(raw.as_str(), "" | "." | "./" | "/") {
        return Ok(Vec::new());
    }
    let mut chars = raw.chars();
    if let (Some(first), Some(':')) = (chars.next(), chars.next()) {
        if first.is_ascii_alphabetic() {
            return Err(
                "it names a drive, and paths are relative to the top of the folder".to_string(),
            );
        }
    }
    if raw.starts_with('/') {
        return Err(
            "it starts at a root, and paths are relative to the top of the folder".to_string(),
        );
    }
    if raw.starts_with('~') {
        return Err(
            "it starts at a home folder, and paths are relative to the top of the folder"
                .to_string(),
        );
    }
    let segments: Vec<&str> = raw
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if segments.len() > MAX_DEPTH {
        return Err("the path is too deep".to_string());
    }
    for segment in &segments {
        if *segment == ".." {
            return Err("it climbs out of the folder".to_string());
        }
        if segment.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
            return Err("a name in it holds a control character".to_string());
        }
        // Windows drops a trailing dot or space from a name, so `.git.` would
        // reach `.git` there. Such a name is refused everywhere.
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err("a name in it ends in a dot or a space".to_string());
        }
        if segment.contains(':') {
            return Err("a name in it holds a colon".to_string());
        }
    }
    Ok(segments.into_iter().map(str::to_string).collect())
}

/// Files that usually hold a secret. An example file that exists to be
/// committed (`.env.example` and its kin) is not one of them.
fn holds_secret(name: &str) -> bool {
    let name = name.to_lowercase();
    if name == ".env" {
        return true;
    }
    if let Some(rest) = name.strip_prefix(".env.") {
        if !rest.is_empty() && !matches!(rest, "example" | "sample" | "template" | "dist") {
            return true;
        }
    }
    if [".pem", ".key", ".p12", ".pfx"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        return true;
    }
    if matches!(
        name.as_str(),
        "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
    ) {
        return true;
    }
    matches!(name.as_str(), ".npmrc" | ".netrc" | ".pypirc")
}

/// Why a path is kept away from, as the page says it, or `None`.
pub(crate) fn guarded<S: AsRef<str>>(segments: &[S]) -> Option<String> {
    for segment in segments {
        let segment = segment.as_ref();
        if SKIPPED.contains(&segment.to_lowercase().as_str()) {
            return Some(format!("{segment} is never read"));
        }
    }
    let last = segments.last()?.as_ref();
    holds_secret(last).then(|| format!("{last} usually holds secrets, so it is never read"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> Vec<String> {
        resolve(input).unwrap_or_else(|reason| panic!("{input:?} was refused: {reason}"))
    }

    fn refused(input: &str) -> String {
        resolve(input).expect_err(input)
    }

    #[test]
    fn the_top_has_four_spellings() {
        for top in ["", ".", "./", "/", "  "] {
            assert!(ok(top).is_empty(), "{top:?}");
        }
    }

    #[test]
    fn a_path_is_reduced_to_its_names() {
        assert_eq!(ok("src/app.ts"), ["src", "app.ts"]);
        assert_eq!(ok("./src//./app.ts"), ["src", "app.ts"]);
        assert_eq!(ok("src\\lib\\a.rs"), ["src", "lib", "a.rs"]);
        assert_eq!(ok(" docs/ "), ["docs"]);
    }

    #[test]
    fn nothing_leaves_the_folder() {
        assert!(refused("../secrets").contains("climbs"));
        assert!(refused("src/../../x").contains("climbs"));
        assert!(
            refused("src/..").contains("climbs"),
            "a climb that lands back inside is refused too"
        );
        assert!(refused("/etc/passwd").contains("root"));
        assert!(refused("\\\\server\\share").contains("root"));
        assert!(refused("C:\\Users").contains("drive"));
        assert!(refused("c:relative").contains("drive"));
        assert!(refused("~/notes").contains("home"));
    }

    #[test]
    fn names_that_mean_something_else_on_some_disk_are_refused() {
        assert!(refused(".git./config").contains("dot or a space"));
        assert_eq!(ok("notes "), ["notes"], "the whole path is trimmed first");
        assert!(refused("a /b").contains("dot or a space"));
        assert!(refused("file.txt:stream").contains("colon"));
        assert!(refused("a\u{0007}b").contains("control"));
        assert!(refused("a\u{007f}").contains("control"));
    }

    #[test]
    fn length_and_depth_are_bounded() {
        assert!(refused(&"a".repeat(1025)).contains("too long"));
        assert_eq!(ok(&"a".repeat(1024)).len(), 1);
        assert!(refused(&["d"; 65].join("/")).contains("too deep"));
        assert_eq!(ok(&["d"; 64].join("/")).len(), 64);
    }

    #[test]
    fn git_node_modules_and_receipts_are_never_entered() {
        assert_eq!(guarded(&[".git"]).as_deref(), Some(".git is never read"));
        assert!(
            guarded(&["sub", ".GIT", "config"]).is_some(),
            "at any depth, in any case"
        );
        assert!(guarded(&["web", "node_modules", "x", "index.js"]).is_some());
        assert!(guarded(&[".vitna", "receipts", "app-1.json"]).is_some());
        assert!(guarded(&[".github", "workflows", "ci.yml"]).is_none());
        assert!(guarded(&["src", "git.rs"]).is_none());
        assert!(guarded::<&str>(&[]).is_none(), "the top");
    }

    #[test]
    fn files_that_usually_hold_secrets_are_never_read() {
        for secret in [
            ".env",
            ".ENV",
            ".env.local",
            ".env.production",
            "server.pem",
            "tls.KEY",
            "cert.p12",
            "a.pfx",
            "id_rsa",
            "id_ed25519",
            ".npmrc",
            ".netrc",
            ".pypirc",
        ] {
            let reason =
                guarded(&["config", secret]).unwrap_or_else(|| panic!("{secret} was not guarded"));
            assert!(reason.contains("usually holds secrets"), "{reason}");
        }
        for fine in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.dist",
            ".env.",
            "env",
            "id_rsa.pub",
            "keys.md",
            ".npmrc.md",
            "monkey",
        ] {
            assert!(guarded(&[fine]).is_none(), "{fine} was guarded");
        }
        assert!(
            guarded(&[".env", "inside"]).is_none(),
            "only the last name is a file"
        );
    }
}
