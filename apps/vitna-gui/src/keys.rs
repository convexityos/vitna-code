//! Where a provider's API key is kept, said to the person who has to put it
//! there.
//!
//! CONTRIBUTING rule 4, as the owner decided it on 2026-09-21: a key comes
//! from an explicit credential source, the operating system's credential
//! store first, and never from the environment, in any build. On main,
//! `crates/providers` holds that store (`KeyringStore`, #21), one entry per
//! provider under the service `vitna-code`. This module names those entries
//! and the platform's own command that makes one, and does nothing else. The
//! window never opens the store, so it never holds a key, and so it cannot
//! say whether an entry exists; that is the daemon's to report, and no daemon
//! reports it yet.
//!
//! The names here must match `KEYCHAIN_SERVICE` and the platform entries in
//! `crates/providers/src/keyring.rs` on main. This branch's providers crate
//! predates that store, so they are restated here rather than imported; once
//! the window builds against main, import them instead.

/// The service every Vitna Code API key is filed under.
pub const SERVICE: &str = "vitna-code";

/// The credential store on this platform, by the name the platform gives it.
#[cfg(windows)]
pub const STORE: &str = "Windows Credential Manager";
#[cfg(target_os = "macos")]
pub const STORE: &str = "macOS Keychain";
#[cfg(all(unix, not(target_os = "macos")))]
pub const STORE: &str = "Secret Service";
#[cfg(not(any(windows, unix)))]
pub const STORE: &str = "OS credential store";

/// The platform's own command that files a key for `provider`. Each one asks
/// for the key rather than taking it as an argument, which keeps it out of
/// the shell's history; none of them is ever handed a key by this window.
pub fn store_command(provider: &str) -> String {
    #[cfg(windows)]
    {
        format!("cmdkey /generic:{SERVICE}:{provider} /user:{provider} /pass")
    }
    #[cfg(target_os = "macos")]
    {
        format!("security add-generic-password -s {SERVICE} -a {provider} -w")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        format!("secret-tool store --label=\"Vitna Code API key ({provider})\" service {SERVICE} username {provider}")
    }
    #[cfg(not(any(windows, unix)))]
    {
        format!("store the key under service {SERVICE}, account {provider}")
    }
}

/// What one key is, in the words the store's own interface uses, for the
/// sentence under the list: "Each key is {this} in {STORE}".
pub fn entry_form() -> &'static str {
    #[cfg(windows)]
    {
        "a generic credential named for its provider, like vitna-code:anthropic,"
    }
    #[cfg(target_os = "macos")]
    {
        "a generic password under the service vitna-code, with its provider as the account,"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "an item under the service vitna-code, with its provider as the username,"
    }
    #[cfg(not(any(windows, unix)))]
    {
        "an entry under the service vitna-code, one for each provider,"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command names the entry the providers crate reads, and asks for
    /// the key rather than carrying one: the window never has one to carry.
    #[test]
    fn a_store_command_names_the_entry_and_never_carries_a_key() {
        for provider in ["anthropic", "openai"] {
            let c = store_command(provider);
            assert!(c.contains(SERVICE), "{c}");
            assert!(c.contains(provider), "{c}");
            #[cfg(windows)]
            assert!(c.ends_with("/pass"), "cmdkey prompts only when /pass is bare: {c}");
            #[cfg(target_os = "macos")]
            assert!(c.ends_with(" -w"), "security prompts only when -w is last: {c}");
            assert!(!c.to_lowercase().contains("key="), "{c}");
        }
    }

    /// A pin. Changing the service strands every key already stored, and on
    /// main `KEYCHAIN_SERVICE` is this same string, so the two move together
    /// or not at all.
    #[test]
    fn keys_are_filed_under_vitna_code() {
        assert_eq!(SERVICE, "vitna-code");
    }

    /// The window reads nothing from its environment but its own capture
    /// hooks, whose names start `VITNA_GUI_`. It used to read each provider's
    /// key variable by a name held in the catalog, which is a read whose name
    /// is not a literal at all, so any read not naming a `VITNA_GUI_` literal
    /// fails here, however the name is spelled. The patterns are built in
    /// pieces so this test's own source does not match them.
    #[test]
    fn the_window_reads_no_key_from_its_environment() {
        let reads: Vec<String> = ["var(", "var_os(", "vars(", "vars_os("]
            .iter()
            .map(|f| format!("{}::{f}", "env"))
            .collect();
        let hook = format!("\"{}_", "VITNA_GUI");
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src") {
            let path = entry.expect("entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !name.ends_with(".rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            for (i, line) in text.lines().enumerate() {
                for read in &reads {
                    for (at, _) in line.match_indices(read.as_str()) {
                        if !line[at + read.len()..].starts_with(&hook) {
                            offenders.push(format!("{name}:{}: {}", i + 1, line.trim()));
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "environment reads that are not capture hooks: {offenders:#?}");
    }
}
