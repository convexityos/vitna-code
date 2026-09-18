//! Terminating the whole process tree, not just the child we spawned.
//!
//! `tokio::time::timeout` around `Child::output()` only stops awaiting. It does
//! not signal the child, and tokio does not kill on drop unless asked. So a
//! command that timed out kept running: the journal recorded a start with no
//! finish while the process went on writing to the workspace.
//!
//! Killing the direct child is not enough either. `sh -c "..."` is the parent of
//! whatever it started, and those grandchildren outlive a kill aimed at the
//! shell.

/// Whether a teardown reached the whole tree or only as far as it could see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// Every descendant was terminated by a mechanism that cannot leak.
    Complete,
    /// The direct child was terminated; descendants may survive. Reported as
    /// such rather than as a clean kill.
    Partial,
    Failed,
}

impl Teardown {
    pub fn as_str(&self) -> &'static str {
        match self {
            Teardown::Complete => "process_tree_terminated",
            Teardown::Partial => "child_terminated_descendants_unknown",
            Teardown::Failed => "termination_failed",
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::Teardown;

    /// Put the child in its own process group so the whole group can be
    /// signalled at once.
    pub fn configure(cmd: &mut tokio::process::Command) {
        cmd.process_group(0);
    }

    pub struct Guard;

    pub fn attach(_child: &tokio::process::Child) -> Guard {
        Guard
    }

    /// Signal the child's process group.
    ///
    /// Under bubblewrap this is complete: `--unshare-pid --die-with-parent`
    /// puts every descendant in a PID namespace whose init is the child, so
    /// killing it takes the namespace with it. Without a PID namespace a
    /// descendant that called `setsid` has left the group and survives, which
    /// is why the caller says which case it is in.
    pub fn kill_tree(
        child: &mut tokio::process::Child,
        _guard: &Guard,
        namespaced: bool,
    ) -> Teardown {
        let Some(pid) = child.id() else {
            return Teardown::Failed;
        };

        // A negative pid signals the whole process group.
        let rc = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        let group_killed = rc == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);

        // Backstop, in case the group signal did not reach the child itself.
        let _ = child.start_kill();

        if !group_killed {
            return Teardown::Failed;
        }
        if namespaced {
            Teardown::Complete
        } else {
            Teardown::Partial
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::Teardown;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub fn configure(_cmd: &mut tokio::process::Command) {}

    /// A job object holding the child and everything it spawns.
    pub struct Guard {
        job: HANDLE,
        assigned: bool,
    }

    impl Guard {
        /// True when a job is actually holding the process, which is what makes
        /// a teardown complete rather than best effort.
        pub fn holds_job(&self) -> bool {
            !self.job.is_null() && self.assigned
        }
    }

    // A job handle is a kernel object handle, valid process-wide and usable
    // from any thread. The raw pointer is what makes the compiler doubt it, and
    // without this the runner's future is not Send.
    unsafe impl Send for Guard {}
    unsafe impl Sync for Guard {}

    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.job.is_null() {
                // KILL_ON_JOB_CLOSE means closing the last handle terminates
                // whatever is still inside, so a command that exited cleanly but
                // left a background process behind does not outlive the action.
                unsafe { CloseHandle(self.job) };
            }
        }
    }

    /// Create a job and put the child in it.
    ///
    /// The child is already running by the time it is assigned, so a process it
    /// spawns in that window can escape the job. Closing it properly needs
    /// CREATE_SUSPENDED and a resume, which std's `Child` does not expose a
    /// thread handle for. The window is microseconds and it is named here
    /// rather than assumed away.
    pub fn attach(child: &tokio::process::Child) -> Guard {
        let empty = Guard {
            job: std::ptr::null_mut(),
            assigned: false,
        };

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return empty;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                CloseHandle(job);
                return empty;
            }

            let assigned = match child.raw_handle() {
                Some(handle) => AssignProcessToJobObject(job, handle as HANDLE) != 0,
                None => false,
            };
            if !assigned {
                CloseHandle(job);
                return empty;
            }

            Guard { job, assigned }
        }
    }

    pub fn kill_tree(
        child: &mut tokio::process::Child,
        guard: &Guard,
        _namespaced: bool,
    ) -> Teardown {
        if guard.holds_job() {
            if unsafe { TerminateJobObject(guard.job, 1) } != 0 {
                return Teardown::Complete;
            }
            return Teardown::Failed;
        }

        // No job: the direct child is all that can be reached.
        match child.start_kill() {
            Ok(()) => Teardown::Partial,
            Err(_) => Teardown::Failed,
        }
    }
}

pub use imp::{attach, configure, kill_tree, Guard};
