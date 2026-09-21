//! No source file in this crate reads the process environment.
//!
//! `no_ambient_credentials.rs` proves that the paths it exercises ignore
//! planted keys. This guard covers the paths it does not exercise: a future
//! adapter reading `OPENAI_ORG_ID`, or a helper walking `env::vars()` for
//! anything ending in `_API_KEY`, which no list of variable names would catch.
//! So it refuses every way of reaching the environment, whatever is being
//! read. A crate that resolves credentials has no business reading it at all.
//!
//! It reads source text, so it is a backstop and not a proof: an alias such as
//! `use std::env::var as v` imported through another crate would get past it.

use std::path::{Path, PathBuf};

/// Each spelling that reaches the environment. `env::` also covers
/// `env::var`, `env::vars` and the `_os` forms, and `std::env` covers imports.
const FORBIDDEN: [&str; 4] = ["std::env", "env::", "env!(", "option_env!("];

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
    {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Whether `code` contains `token` as a token of its own, so that `env::`
/// matches `std::env::var` and does not match `dev_env::`.
fn has_token(code: &str, token: &str) -> bool {
    code.match_indices(token).any(|(at, _)| {
        !code[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

#[test]
fn no_source_file_reads_the_environment() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    // Guards against the walk itself silently finding nothing.
    assert!(
        files.iter().any(|f| f.ends_with("credentials.rs")),
        "the walk of {} missed credentials.rs",
        src.display()
    );

    let mut hits = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (index, line) in text.lines().enumerate() {
            // Comments may name the variables this crate refuses to read.
            let code = line.split("//").next().unwrap_or_default();
            for token in FORBIDDEN {
                if has_token(code, token) {
                    hits.push(format!(
                        "{}:{}: {}",
                        file.strip_prefix(&src).unwrap_or(file).display(),
                        index + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "vitna-providers reads the environment, which CONTRIBUTING rule 4 forbids \
         for credentials and nothing else here needs:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_token_match_is_neither_too_loose_nor_too_tight() {
    assert!(has_token("let k = std::env::var(\"X\");", "env::"));
    assert!(has_token("use std::env;", "std::env"));
    assert!(has_token("env::vars().filter(f)", "env::"));
    assert!(has_token("let k = env!(\"X\");", "env!("));
    assert!(has_token("let k = option_env!(\"X\");", "option_env!("));
    assert!(!has_token("let k = option_env!(\"X\");", "env!("));
    assert!(!has_token("dev_env::load()", "env::"));
    assert!(!has_token("my_std::envelope()", "std::env"));
}
