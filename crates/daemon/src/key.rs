//! The daemon's signing key: made once, kept owner-only, never replaced
//! behind anyone's back.
//!
//! Every receipt is signed, and a signature is worth something only to a
//! person who can check it against a key they trust. This key used to be
//! generated afresh on every start and kept nowhere, so no receipt the daemon
//! wrote could be checked by anyone, the daemon itself included, a minute
//! later. It now lives in `device-key` beside the event store, and the daemon
//! publishes its public half in `Health`, which is how the window checks a
//! receipt against the key of the daemon it is talking to.
//!
//! What owner-only means here, and what it does not:
//!
//! - Unix: the file is created 0600 in one step (`O_EXCL`, the mode given at
//!   open), and a key file another account could read is refused, the way ssh
//!   refuses one, rather than used.
//! - Windows: the file is created in one step with a protected DACL naming
//!   this account and nobody else, so it never inherits what its folder
//!   grants. A key file whose DACL lets in any account other than this one,
//!   SYSTEM or Administrators is refused.
//! - Neither is encryption at rest. ADR-0003 seals secrets with the platform
//!   store (DPAPI, the Keychain, Secret Service), and this key is not sealed
//!   that way yet, which is where an ssh key without a passphrase stands.
//!
//! A key file that exists and will not parse is never overwritten: a new key
//! would silently make this daemon a different signer, and every receipt it
//! wrote before would stop verifying with nothing to say why. The one
//! exception is an empty file, which a crash between creating it and writing
//! it leaves behind, and which never held a key.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::Path;

use ed25519_dalek::SigningKey;

/// The key's file name, in the event store's directory.
pub const KEY_FILE: &str = "device-key";

/// The file's first line, so a key is never mistaken for anything else and a
/// later format can say it is one.
const HEADER: &str = "vitna-device-key-v1";

/// The signing key for the store in `dir`: read if it exists, made once if it
/// does not.
pub fn load_or_create(dir: &Path) -> Result<SigningKey, String> {
    let path = dir.join(KEY_FILE);
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() == 0 => {
            check_owner_only(&path)?;
            tracing::warn!(key = %path.display(), "the signing key file is empty, so it never held a key; making one");
            std::fs::remove_file(&path)
                .map_err(|e| format!("could not remove the empty signing key file {}: {e}", path.display()))?;
        }
        Ok(_) => return read_key(&path),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(format!("could not read the signing key {}: {e}", path.display())),
    }

    let key = vitna_receipts::generate_signing_key();
    let mut file = match create_owner_only(&path) {
        Ok(file) => file,
        // Another daemon on this store made it first, and theirs is the key.
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return read_key(&path),
        Err(e) => return Err(format!("could not create the signing key {}: {e}", path.display())),
    };
    let body = format!("{HEADER}\n{}\n", hex::encode(key.to_bytes()));
    if let Err(e) = file.write_all(body.as_bytes()).and_then(|_| file.sync_all()) {
        // Nothing was ever signed with it, so taking it back is safe.
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(format!("could not write the signing key {}: {e}", path.display()));
    }
    tracing::info!(key = %path.display(), "made this daemon's signing key");
    Ok(key)
}

fn read_key(path: &Path) -> Result<SigningKey, String> {
    check_owner_only(path)?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read the signing key {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let (Some(HEADER), Some(seed)) = (lines.next(), lines.next()) else {
        return Err(unrecognized(path));
    };
    let seed: [u8; 32] = hex::decode(seed.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| unrecognized(path))?;
    Ok(SigningKey::from_bytes(&seed))
}

fn unrecognized(path: &Path) -> String {
    format!(
        "{} is not a signing key this daemon wrote. It is not replaced automatically, since a new key \
         would make this daemon a different signer; move it aside to have a new one made, and receipts \
         signed with the old key will then no longer verify against this daemon.",
        path.display()
    )
}

/// The accounts an allowing entry in an SDDL DACL names, other than this
/// account (by every name in `user`), SYSTEM and Administrators. `None` for a
/// DACL that is missing altogether, which lets every account in. Parsed here
/// rather than inside the Windows module so every platform's tests read it.
#[cfg_attr(not(windows), allow(dead_code))]
fn foreign_trustees(dacl: &str, user: &[&str]) -> Option<Vec<String>> {
    let body = dacl.strip_prefix("D:")?;
    if body.starts_with("NO_ACCESS_CONTROL") {
        return None;
    }
    let mut trusted = vec!["SY", "BA", "OW", "S-1-5-18", "S-1-5-32-544"];
    trusted.extend_from_slice(user);
    let mut foreign = Vec::new();
    for ace in body.split('(').skip(1) {
        let fields: Vec<&str> = ace.trim_end_matches(')').split(';').collect();
        // Allow entries only: a deny entry takes access away.
        if !matches!(fields.first(), Some(&("A" | "OA" | "XA" | "ZA"))) {
            continue;
        }
        let trustee = fields.get(5).copied().unwrap_or("?");
        if !trusted.contains(&trustee) {
            foreign.push(trustee.to_string());
        }
    }
    Some(foreign)
}

#[cfg(unix)]
fn create_owner_only(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(unix)]
fn check_owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("could not read the permissions on {}: {e}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "the signing key {} can be read by other accounts (mode {:o}); make it this account's alone \
             with chmod 600",
            path.display(),
            mode & 0o777
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn create_owner_only(path: &Path) -> std::io::Result<File> {
    let user = windows_acl::current_user_sid()?;
    windows_acl::create_with_dacl(path, &format!("D:P(A;;FA;;;{user})"))
}

#[cfg(windows)]
fn check_owner_only(path: &Path) -> Result<(), String> {
    let user = windows_acl::current_user_sid().map_err(|e| format!("could not read this account's SID: {e}"))?;
    // SDDL writes some accounts by alias: the built-in Administrator, which a
    // GitHub Windows runner runs as, reads back as `LA`, never as its SID.
    let alias = windows_acl::sddl_name(&user).map_err(|e| format!("could not read this account's SDDL name: {e}"))?;
    let dacl = windows_acl::dacl_sddl(path)
        .map_err(|e| format!("could not read the permissions on {}: {e}", path.display()))?;
    match foreign_trustees(&dacl, &[&user, &alias]) {
        Some(others) if others.is_empty() => Ok(()),
        Some(others) => Err(format!(
            "the signing key {} can be reached by {} as well as this account ({dacl}); make it this \
             account's alone",
            path.display(),
            others.join(", ")
        )),
        None => Err(format!(
            "the signing key {} has no access control list, so every account can read it",
            path.display()
        )),
    }
}

#[cfg(not(any(unix, windows)))]
fn create_owner_only(_: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "this platform has no owner-only file this daemon knows how to make",
    ))
}

#[cfg(not(any(unix, windows)))]
fn check_owner_only(path: &Path) -> Result<(), String> {
    Err(format!("cannot tell whether {} is readable by other accounts on this platform", path.display()))
}

/// The three Windows security calls this needs, each in one place.
#[cfg(windows)]
pub(crate) mod windows_acl {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// Copies out a NUL-terminated string the system allocated, and frees it.
    ///
    /// # Safety
    /// `p` must be a live `LocalAlloc` pointer to a NUL-terminated UTF-16 string.
    unsafe fn take_wide(p: *mut u16) -> String {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        LocalFree(p.cast());
        s
    }

    /// The account this process runs as, as a SID string (`S-1-5-21-...`).
    pub fn current_user_sid() -> io::Result<String> {
        // SAFETY: the token handle is closed on every path; GetTokenInformation
        // writes at most `len` bytes into a buffer of at least `len` bytes,
        // aligned for TOKEN_USER; the SID string is freed by take_wide.
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            let err = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(err);
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut sid: *mut u16 = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(take_wide(sid))
        }
    }

    /// Creates `path`, failing if it exists, with the DACL `sddl` describes
    /// applied as the file is made rather than after, so there is no moment
    /// when it carries what its folder would have given it.
    pub fn create_with_dacl(path: &Path, sddl: &str) -> io::Result<File> {
        // SAFETY: the descriptor is freed on every path after CreateFileW has
        // copied it; the handle is owned by the File it is wrapped in.
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let sddl = wide(OsStr::new(sddl));
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut())
                == 0
            {
                return Err(io::Error::last_os_error());
            }
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let path = wide(path.as_os_str());
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );
            let err = io::Error::last_os_error();
            LocalFree(sd);
            if handle == INVALID_HANDLE_VALUE {
                return Err(err);
            }
            Ok(File::from_raw_handle(handle))
        }
    }

    /// How SDDL writes the account `sid`: the SID string, or the alias Windows
    /// uses for it (`LA` for the built-in Administrator). Read back through the
    /// same conversion a DACL is read with, so the two cannot disagree.
    pub fn sddl_name(sid: &str) -> io::Result<String> {
        // SAFETY: both allocations the system hands back are freed here.
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let sddl = wide(OsStr::new(&format!("D:(A;;FA;;;{sid})")));
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut())
                == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut text: *mut u16 = null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                null_mut(),
            );
            let err = io::Error::last_os_error();
            LocalFree(sd);
            if ok == 0 {
                return Err(err);
            }
            let round_trip = take_wide(text);
            round_trip
                .rsplit(';')
                .next()
                .map(|name| name.trim_end_matches(')').to_string())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| io::Error::other(format!("unexpected SDDL: {round_trip}")))
        }
    }

    /// The file's DACL, in SDDL.
    pub fn dacl_sddl(path: &Path) -> io::Result<String> {
        // SAFETY: both allocations the system hands back are freed here.
        unsafe {
            let path = wide(path.as_os_str());
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let rc = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            );
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc as i32));
            }
            let mut text: *mut u16 = null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                null_mut(),
            );
            let err = io::Error::last_os_error();
            LocalFree(sd);
            if ok == 0 {
                return Err(err);
            }
            Ok(take_wide(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vitna_key_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_key_is_made_once_and_read_back() {
        let dir = fresh_dir("once");
        let first = load_or_create(&dir).expect("made");
        let second = load_or_create(&dir).expect("read back");
        assert_eq!(first.verifying_key(), second.verifying_key(), "a second start is the same signer");
        let text = std::fs::read_to_string(dir.join(KEY_FILE)).expect("key file");
        assert!(text.starts_with(HEADER));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_key_file_is_this_accounts_alone() {
        let dir = fresh_dir("owner");
        load_or_create(&dir).expect("made");
        let path = dir.join(KEY_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            let user = windows_acl::current_user_sid().expect("sid");
            let name = windows_acl::sddl_name(&user).expect("sddl name");
            assert_eq!(windows_acl::dacl_sddl(&path).expect("dacl"), format!("D:P(A;;FA;;;{name})"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_other_accounts_can_read_is_refused() {
        let dir = fresh_dir("shared");
        let path = dir.join(KEY_FILE);
        let body = format!("{HEADER}\n{}\n", hex::encode([7u8; 32]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&path, &body).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
        #[cfg(windows)]
        {
            // The Users group may read it as well as this account.
            let user = windows_acl::current_user_sid().expect("sid");
            let mut file = windows_acl::create_with_dacl(&path, &format!("D:P(A;;FA;;;{user})(A;;FR;;;BU)"))
                .expect("create");
            file.write_all(body.as_bytes()).expect("write");
        }
        let err = load_or_create(&dir).expect_err("refused");
        assert!(err.contains(KEY_FILE), "the refusal names the file: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_key_is_never_replaced() {
        let dir = fresh_dir("garbage");
        let path = dir.join(KEY_FILE);
        create_owner_only(&path).and_then(|mut f| f.write_all(b"not a key\n")).expect("write");
        let err = load_or_create(&dir).expect_err("refused");
        assert!(err.contains("not a signing key"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).expect("still there"), "not a key\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_key_file_never_held_a_key_and_is_made_whole() {
        let dir = fresh_dir("empty");
        drop(create_owner_only(&dir.join(KEY_FILE)).expect("create"));
        let key = load_or_create(&dir).expect("made");
        assert_eq!(load_or_create(&dir).expect("read").verifying_key(), key.verifying_key());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_this_account_system_and_administrators_may_be_named() {
        let user = "S-1-5-21-1-2-3-1001";
        let me = [user];
        assert_eq!(foreign_trustees(&format!("D:P(A;;FA;;;{user})"), &me), Some(vec![]));
        let inherited = format!("D:AI(A;ID;FA;;;SY)(A;ID;FA;;;BA)(A;ID;FA;;;{user})");
        assert_eq!(foreign_trustees(&inherited, &me), Some(vec![]));
        let shared = format!("D:P(A;;FA;;;{user})(A;;FR;;;BU)");
        assert_eq!(foreign_trustees(&shared, &me), Some(vec!["BU".to_string()]));
        // A deny entry only takes access away.
        let denied = format!("D:P(D;;FA;;;WD)(A;;FA;;;{user})");
        assert_eq!(foreign_trustees(&denied, &me), Some(vec![]));
        assert_eq!(foreign_trustees("D:NO_ACCESS_CONTROL", &me), None);
        assert_eq!(foreign_trustees("", &me), None);
        // The built-in Administrator reads back by alias, and is still this
        // account when it is the one running; to anyone else it is foreign.
        let admin = "S-1-5-21-1-2-3-500";
        assert_eq!(foreign_trustees("D:P(A;;FA;;;LA)", &[admin, "LA"]), Some(vec![]));
        assert_eq!(foreign_trustees("D:P(A;;FA;;;LA)", &me), Some(vec!["LA".to_string()]));
    }
}
