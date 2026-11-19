//! Native Windows AppContainer sandbox launcher for `vitna-runner`.
//!
//! Launches untrusted repository commands inside a real Win32 AppContainer
//! security boundary (`CreateAppContainerProfile`, `SECURITY_CAPABILITIES`,
//! `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`, `CreateProcessW`), granting
//! workspace-only read/write ACLs, denying write/delete access on protected
//! `.git/*` and `.vitna` subpaths, stripping network capability unless
//! explicitly requested, and assigning the suspended child to a
//! `KILL_ON_JOB_CLOSE` Win32 Job Object before resuming execution.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, SetHandleInformation, ERROR_SUCCESS, HANDLE,
    HANDLE_FLAG_INHERIT, HLOCAL, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
    DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, REVOKE_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    FreeSid, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_EXECUTE,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, OPEN_EXISTING, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    STARTUPINFOW,
};

use crate::tree::Teardown;

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)` (0x800700B7).
const HRESULT_ALREADY_EXISTS: i32 = -2147024713; // 0x800700B7u32 as i32
/// `SE_GROUP_ENABLED` flag for `SID_AND_ATTRIBUTES.Attributes`.
const SE_GROUP_ENABLED_ATTR: u32 = 4;
/// Standard Win32 Internet Client AppContainer capability SID.
const INTERNET_CLIENT_CAPABILITY_SID: &str = "S-1-15-3-1";

pub struct AppContainerOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub teardown: Option<Teardown>,
}

fn wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn wide_str(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Wrapper for a Win32 handle moved to a background pipe-reader thread.
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

impl Drop for SendHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

struct OwnedAppSid(PSID);

impl Drop for OwnedAppSid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FreeSid(self.0);
            }
        }
    }
}

struct OwnedLocalSid(PSID);

impl Drop for OwnedLocalSid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0 as HLOCAL);
            }
        }
    }
}

struct OwnedAttrList {
    _backing: Vec<u8>,
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl OwnedAttrList {
    fn new(attribute_count: u32) -> Result<Self, String> {
        let mut size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &mut size);
        }
        if size == 0 {
            return Err(format!(
                "InitializeProcThreadAttributeList sizing failed (error {})",
                unsafe { GetLastError() }
            ));
        }
        let mut backing = vec![0u8; size];
        let ptr = backing.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        let ok = unsafe { InitializeProcThreadAttributeList(ptr, attribute_count, 0, &mut size) };
        if ok == 0 {
            return Err(format!(
                "InitializeProcThreadAttributeList failed (error {})",
                unsafe { GetLastError() }
            ));
        }
        Ok(Self {
            _backing: backing,
            ptr,
        })
    }
}

impl Drop for OwnedAttrList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.ptr);
        }
    }
}

/// Ensures the named AppContainer profile exists and returns its SID.
fn ensure_appcontainer_sid(profile_name: &str) -> Result<OwnedAppSid, String> {
    let w_name = wide_str(profile_name);
    let w_display = wide_str("Vitna Code Sandbox");
    let w_desc = wide_str("Vitna Code guarded workspace sandbox");
    let mut sid: PSID = null_mut();

    let hr = unsafe {
        CreateAppContainerProfile(
            w_name.as_ptr(),
            w_display.as_ptr(),
            w_desc.as_ptr(),
            null(),
            0,
            &mut sid,
        )
    };

    if hr == 0 && !sid.is_null() {
        return Ok(OwnedAppSid(sid));
    }

    if hr == HRESULT_ALREADY_EXISTS {
        let derive_hr =
            unsafe { DeriveAppContainerSidFromAppContainerName(w_name.as_ptr(), &mut sid) };
        if derive_hr == 0 && !sid.is_null() {
            return Ok(OwnedAppSid(sid));
        }
        return Err(format!(
            "DeriveAppContainerSidFromAppContainerName failed (HRESULT 0x{:08x})",
            derive_hr as u32
        ));
    }

    Err(format!(
        "CreateAppContainerProfile failed (HRESULT 0x{:08x})",
        hr as u32
    ))
}

/// Modifies the DACL on `path` for `sid` with the given `access_mode`
/// (`GRANT_ACCESS`, `DENY_ACCESS`, or `REVOKE_ACCESS`).
fn update_path_dacl(
    path: &Path,
    sid: PSID,
    permissions: u32,
    access_mode: i32,
    inheritance: u32,
) -> Result<(), String> {
    let w_path = wide_null(path.as_os_str());
    let mut old_dacl: *mut ACL = null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();

    let get_rc = unsafe {
        GetNamedSecurityInfoW(
            w_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut old_dacl,
            null_mut(),
            &mut sd,
        )
    };
    if get_rc != ERROR_SUCCESS {
        return Err(format!(
            "GetNamedSecurityInfoW failed on {} (error {get_rc})",
            path.display()
        ));
    }

    let ea = EXPLICIT_ACCESS_W {
        grfAccessPermissions: permissions,
        grfAccessMode: access_mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid as *mut u16,
        },
    };

    let mut new_dacl: *mut ACL = null_mut();
    let set_entries_rc = unsafe { SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl) };
    if set_entries_rc != ERROR_SUCCESS {
        if !sd.is_null() {
            unsafe {
                LocalFree(sd as HLOCAL);
            }
        }
        return Err(format!(
            "SetEntriesInAclW failed on {} (error {set_entries_rc})",
            path.display()
        ));
    }

    let set_info_rc = unsafe {
        SetNamedSecurityInfoW(
            w_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            new_dacl,
            null_mut(),
        )
    };

    if !new_dacl.is_null() {
        unsafe {
            LocalFree(new_dacl as HLOCAL);
        }
    }
    if !sd.is_null() {
        unsafe {
            LocalFree(sd as HLOCAL);
        }
    }

    if set_info_rc != ERROR_SUCCESS {
        return Err(format!(
            "SetNamedSecurityInfoW failed on {} (error {set_info_rc})",
            path.display()
        ));
    }
    Ok(())
}

/// RAII guard that grants workspace access and applies protected-subpath deny
/// ACEs for the AppContainer SID, revoking those ACEs when the action finishes.
struct AclGuard {
    sid: PSID,
    touched_paths: Vec<PathBuf>,
}

impl AclGuard {
    fn apply(
        sid: PSID,
        workspace_root: &Path,
        protected: &[PathBuf],
        writable: bool,
    ) -> Result<Self, String> {
        let mut guard = Self {
            sid,
            touched_paths: Vec::new(),
        };

        let ws_perms = if writable {
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE
        } else {
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
        };
        update_path_dacl(
            workspace_root,
            sid,
            ws_perms,
            GRANT_ACCESS,
            SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        )?;
        guard.touched_paths.push(workspace_root.to_path_buf());

        let write_deny_mask = FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER;

        // Prevent renaming or deleting `.git` itself to bypass protected subpaths.
        let git_dir = workspace_root.join(".git");
        if git_dir.is_dir() {
            update_path_dacl(&git_dir, sid, DELETE, DENY_ACCESS, NO_INHERITANCE)?;
            guard.touched_paths.push(git_dir);
        }

        for path in protected {
            if !path.exists() {
                continue;
            }
            let inherit = if path.is_dir() {
                SUB_CONTAINERS_AND_OBJECTS_INHERIT
            } else {
                NO_INHERITANCE
            };
            update_path_dacl(path, sid, write_deny_mask, DENY_ACCESS, inherit)?;
            guard.touched_paths.push(path.clone());
        }

        Ok(guard)
    }
}

impl Drop for AclGuard {
    fn drop(&mut self) {
        // Revoke in reverse order (protected subpaths first, then workspace root).
        for path in self.touched_paths.iter().rev() {
            if path.exists() {
                let _ = update_path_dacl(path, self.sid, 0, REVOKE_ACCESS, NO_INHERITANCE);
            }
        }
    }
}

fn create_inheritable_pipe() -> Result<(SendHandle, OwnedHandle), String> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let mut read_h: HANDLE = null_mut();
    let mut write_h: HANDLE = null_mut();
    let ok = unsafe { CreatePipe(&mut read_h, &mut write_h, &sa, 0) };
    if ok == 0 {
        return Err(format!("CreatePipe failed (error {})", unsafe {
            GetLastError()
        }));
    }
    // Keep only the child's write end inheritable so EOF fires promptly when
    // the child and its descendants exit.
    let no_inherit = unsafe { SetHandleInformation(read_h, HANDLE_FLAG_INHERIT, 0) };
    if no_inherit == 0 {
        unsafe {
            CloseHandle(read_h);
            CloseHandle(write_h);
        }
        return Err(format!("SetHandleInformation failed (error {})", unsafe {
            GetLastError()
        }));
    }
    let write_owned = OwnedHandle::new(write_h)
        .ok_or_else(|| "CreatePipe returned invalid write handle".to_string())?;
    Ok((SendHandle(read_h), write_owned))
}

fn open_nul_stdin() -> Result<OwnedHandle, String> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let w_nul = wide_str("NUL");
    let handle = unsafe {
        CreateFileW(
            w_nul.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    OwnedHandle::new(handle).ok_or_else(|| {
        format!("CreateFileW(NUL) failed (error {})", unsafe {
            GetLastError()
        })
    })
}

fn build_unicode_env_block(env: &[(String, String)]) -> Vec<u16> {
    let mut sorted = env.to_vec();
    sorted.sort_by_key(|a| a.0.to_ascii_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in sorted {
        block.extend(OsStr::new(&k).encode_wide());
        block.push(b'=' as u16);
        block.extend(OsStr::new(&v).encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn drain_pipe(handle: SendHandle) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let mut bytes_read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                handle.0,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut bytes_read,
                null_mut(),
            )
        };
        if ok == 0 || bytes_read == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..bytes_read as usize]);
    }
    out
}

fn resolve_cmd_exe() -> PathBuf {
    if let Ok(root) = std::env::var("SystemRoot") {
        let candidate = PathBuf::from(root).join("System32").join("cmd.exe");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(r"C:\Windows\System32\cmd.exe")
}

#[allow(clippy::too_many_arguments)]
pub fn run_in_appcontainer(
    profile_name: &str,
    workspace_root: &Path,
    working_dir: &Path,
    protected: &[PathBuf],
    command: &str,
    env: &[(String, String)],
    writable: bool,
    allow_network: bool,
    timeout: Duration,
) -> Result<AppContainerOutput, String> {
    let app_sid = ensure_appcontainer_sid(profile_name)?;
    let _acl_guard = AclGuard::apply(app_sid.0, workspace_root, protected, writable)?;

    let mut _net_cap_sid: Option<OwnedLocalSid> = None;
    let mut capabilities: Vec<SID_AND_ATTRIBUTES> = Vec::new();
    if allow_network {
        let w_cap = wide_str(INTERNET_CLIENT_CAPABILITY_SID);
        let mut cap_sid: PSID = null_mut();
        let ok = unsafe { ConvertStringSidToSidW(w_cap.as_ptr(), &mut cap_sid) };
        if ok == 0 || cap_sid.is_null() {
            return Err(format!(
                "ConvertStringSidToSidW failed for internetClient capability (error {})",
                unsafe { GetLastError() }
            ));
        }
        capabilities.push(SID_AND_ATTRIBUTES {
            Sid: cap_sid,
            Attributes: SE_GROUP_ENABLED_ATTR,
        });
        _net_cap_sid = Some(OwnedLocalSid(cap_sid));
    }

    let mut sec_caps = SECURITY_CAPABILITIES {
        AppContainerSid: app_sid.0,
        Capabilities: if capabilities.is_empty() {
            null_mut()
        } else {
            capabilities.as_mut_ptr()
        },
        CapabilityCount: capabilities.len() as u32,
        Reserved: 0,
    };

    let attr_list = OwnedAttrList::new(1)?;
    let attr_ok = unsafe {
        UpdateProcThreadAttribute(
            attr_list.ptr,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &mut sec_caps as *mut SECURITY_CAPABILITIES as *const core::ffi::c_void,
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            null_mut(),
            null(),
        )
    };
    if attr_ok == 0 {
        return Err(format!(
            "UpdateProcThreadAttribute(SECURITY_CAPABILITIES) failed (error {})",
            unsafe { GetLastError() }
        ));
    }

    let (stdout_read, stdout_write) = create_inheritable_pipe()?;
    let (stderr_read, stderr_write) = create_inheritable_pipe()?;
    let stdin_nul = open_nul_stdin()?;

    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = stdin_nul.raw();
    si.StartupInfo.hStdOutput = stdout_write.raw();
    si.StartupInfo.hStdError = stderr_write.raw();
    si.lpAttributeList = attr_list.ptr;

    let cmd_exe = resolve_cmd_exe();
    let w_app = wide_null(cmd_exe.as_os_str());
    let mut w_cmdline = wide_str(&format!("cmd.exe /C {command}"));
    let w_cwd = wide_null(working_dir.as_os_str());

    let mut effective_env = env.to_vec();
    let ws_str = workspace_root.to_string_lossy().to_string();
    effective_env.push(("TEMP".to_string(), ws_str.clone()));
    effective_env.push(("TMP".to_string(), ws_str));
    let env_block = build_unicode_env_block(&effective_env);

    // Create Job Object before resuming the suspended process so zero
    // descendants can escape process-tree teardown.
    let job = unsafe { CreateJobObjectW(null(), null()) };
    let job_owned = OwnedHandle::new(job);
    let mut job_ready = false;
    if let Some(ref j) = job_owned {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                j.raw(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        job_ready = ok != 0;
    }

    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let flags = EXTENDED_STARTUPINFO_PRESENT
        | CREATE_UNICODE_ENVIRONMENT
        | CREATE_NO_WINDOW
        | CREATE_SUSPENDED;
    let created = unsafe {
        CreateProcessW(
            w_app.as_ptr(),
            w_cmdline.as_mut_ptr(),
            null(),
            null(),
            1, // inherit stdout/stderr/stdin handles
            flags,
            env_block.as_ptr() as *const core::ffi::c_void,
            w_cwd.as_ptr(),
            &si.StartupInfo as *const STARTUPINFOW,
            &mut pi,
        )
    };

    // Parent drops write ends and stdin handle immediately whether spawn
    // succeeded or failed so pipe readers never block forever.
    drop(stdout_write);
    drop(stderr_write);
    drop(stdin_nul);

    if created == 0 {
        return Err(format!(
            "CreateProcessW inside AppContainer failed (error {})",
            unsafe { GetLastError() }
        ));
    }

    let proc_handle = OwnedHandle::new(pi.hProcess)
        .ok_or_else(|| "CreateProcessW returned null process handle".to_string())?;
    let thread_handle = OwnedHandle::new(pi.hThread)
        .ok_or_else(|| "CreateProcessW returned null thread handle".to_string())?;

    let job_assigned = if job_ready {
        if let Some(ref j) = job_owned {
            unsafe { AssignProcessToJobObject(j.raw(), proc_handle.raw()) != 0 }
        } else {
            false
        }
    } else {
        false
    };

    // Resume the child now that it is inside both the AppContainer and the Job Object.
    let resume_rc = unsafe { ResumeThread(thread_handle.raw()) };
    if resume_rc == u32::MAX {
        unsafe {
            TerminateProcess(proc_handle.raw(), 1);
        }
        return Err(format!("ResumeThread failed (error {})", unsafe {
            GetLastError()
        }));
    }

    let stdout_reader = std::thread::spawn(move || drain_pipe(stdout_read));
    let stderr_reader = std::thread::spawn(move || drain_pipe(stderr_read));

    let wait_ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
    let wait_rc = unsafe { WaitForSingleObject(proc_handle.raw(), wait_ms) };

    let (timed_out, teardown, exit_code) = if wait_rc == WAIT_OBJECT_0 {
        let mut code: u32 = 1;
        unsafe {
            GetExitCodeProcess(proc_handle.raw(), &mut code);
        }
        (false, None, code as i32)
    } else if wait_rc == WAIT_TIMEOUT {
        let td = if job_assigned {
            if let Some(ref j) = job_owned {
                if unsafe { TerminateJobObject(j.raw(), 1) } != 0 {
                    Teardown::Complete
                } else {
                    Teardown::Failed
                }
            } else {
                Teardown::Failed
            }
        } else if unsafe { TerminateProcess(proc_handle.raw(), 1) } != 0 {
            Teardown::Partial
        } else {
            Teardown::Failed
        };
        unsafe {
            WaitForSingleObject(proc_handle.raw(), 2_000);
        }
        (true, Some(td), -1)
    } else {
        let td = if job_assigned {
            if let Some(ref j) = job_owned {
                let _ = unsafe { TerminateJobObject(j.raw(), 1) };
                Teardown::Complete
            } else {
                Teardown::Failed
            }
        } else {
            let _ = unsafe { TerminateProcess(proc_handle.raw(), 1) };
            Teardown::Partial
        };
        return Err(format!(
            "WaitForSingleObject failed (rc {wait_rc}, teardown {})",
            td.as_str()
        ));
    };

    // Closing the job object (if still alive) ensures any lingering background
    // descendants are terminated before joining pipe readers.
    drop(job_owned);

    let stdout_bytes = stdout_reader.join().unwrap_or_default();
    let stderr_bytes = stderr_reader.join().unwrap_or_default();

    Ok(AppContainerOutput {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        timed_out,
        teardown,
    })
}
