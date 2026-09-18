# Vitna Code Tier 1 Platform Guarantee Matrix

- Document Version: 1.0.0
- Date: 2026-09-16
- Status: Phase 0A Specification
- Authority: ADR-0001, ADR-0002

Vitna Code targets Tier 1 release status across Linux, macOS, and Windows. This matrix specifies the real-hardware OS enforcement mechanisms, isolation capabilities, and empirical proof plans for each platform.

---

## 1. Isolation Levels and Honesty Definitions

- **Read-only**: No arbitrary process execution. Repository files may be read and analyzed in-memory.
- **Guarded**: Native OS-level sandbox enforcement. Filesystem access restricted to the designated agent workspace clone; network egress blocked or policy-filtered; minimal sanitized environment.
- **Strong**: Ephemeral container or hardware-isolated VM (Hyper-V / Apple Hypervisor / KVM). Kernel-level boundary separating host from untrusted child processes.
- **Full Access**: Host-level execution without OS sandboxing, enabled only upon explicit user override. **Never described or labeled as sandboxed in the UI or receipts**.

---

## 2. Platform Guarantee Matrix

| Security Dimension | Linux (x86_64 & ARM64) | macOS (x86_64 & Apple Silicon) | Windows 11 (x64 & ARM64) |
|---|---|---|---|
| **Guarded Engine** | Bubblewrap (`bwrap`). Implemented. Landlock and seccomp are not used. | `sandbox-exec` Seatbelt profile. Implemented. | **Not implemented.** No AppContainer launcher exists; `generate_windows_appcontainer_name` returns an identifier, not a container. Job Objects are used for process-tree termination only. Commands are refused unless unsandboxed execution is explicitly approved. |
| **Strong Engine** | Rootless Podman container / MicroVM (Firecracker/KVM) | Lightweight VM (`Virtualization.framework`) | Disposable locked-down WSL2 / Hyper-V VM (no host drive interop) |
| **Filesystem Isolation** | Mount namespaces (`CLONE_NEWNS`); read-only system mounts; tmpfs for transient state | Seatbelt sandbox profile restricting writes exclusively to agent clone path | AppContainer SID filesystem ACLs denying access to host user profile and system drives |
| **Process Tree Control** | PID namespaces (`CLONE_NEWPID`); cgroups v2 memory and CPU limits | `posix_spawn` with resource limits; process group tree termination | Win32 Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` |
| **Network Egress** | Network namespaces (`CLONE_NEWNET`) with loopback-only or veth filtering | Sandbox profile `network-outbound` denial rules | Windows Filtering Platform (WFP) AppContainer network capability omission |
| **PTY Handling** | POSIX pseudo-terminals (`openpty`) with sane termios | BSD pseudo-terminals (`openpty`) with window size propagation | Windows ConPTY (`CreatePseudoConsole`) with UTF-8 code page enforcement |
| **Host Secrets Isolation** | Cleared environment; `XDG_RUNTIME_DIR` isolated; no `/proc` leak | Stripped environment; no access to login keychain without prompt | Stripped environment; AppContainer denies access to User DPAPI keys |
| **Local IPC Security** | Unix domain socket with `0600` permissions and `SO_PEERCRED` check | Unix domain socket with `0600` permissions and `LOCAL_PEERCRED` check | Windows Named Pipe with security descriptor limiting access to User SID |

---

## 3. Platform Proof Plans and Test Harnesses

Every isolation guarantee must be empirically validated in automated CI test suites:

### Linux Proof Plan (`crates/vitna-sandbox/tests/linux_proof.rs`)
1. **Filesystem Escape Test**: Attempt to open `/etc/shadow`, `~/.ssh/id_rsa`, and parent directories from within the sandbox. Expect `EACCES` or `ENOENT`.
2. **Network Isolation Test**: Attempt to resolve `google.com` or connect to `1.1.1.1:53` when network is disabled. Expect `ENETUNREACH` or immediate socket error.
3. **Fork Bomb Containment**: Spawn a recursive fork script. Verify that cgroup `pids.max` halts spawning without affecting host responsiveness.
4. **Namespace Teardown**: Verify that killing the runner leader cleanly unmounts all namespace mounts without leaving dangling mount points.

### macOS Proof Plan (`crates/vitna-sandbox/tests/macos_proof.rs`)
1. **File Quarantine Test**: Attempt writing outside designated workspace clone path. Verify that operation returns `EPERM`.
2. **Keychain Access Test**: Attempt to invoke `/usr/bin/security dump-keychain`. Verify that command fails and prompt does not reach user.
3. **Process Hierarchy Cleanup**: Spawn long-running background tasks. Send `SIGINT` to runner parent and assert that entire process tree is terminated within 500ms.

### Implementation status of these proof plans

The files named below do not exist. What does exist is
`crates/vitna-runner/tests/sandbox_enforcement.rs`, which runs real commands
through the runner and asserts: a command is refused where no backend exists,
a sandboxed command cannot write outside the workspace, a sandboxed command
cannot write `.git/config`, the workspace itself stays writable, and a timed
out command's process tree does not outlive its action. Fork bombs, network
isolation, and namespace teardown are not covered by any test.

### Windows 11 Proof Plan (`crates/vitna-sandbox/tests/windows_proof.rs`)
1. **AppContainer Traversal Test**: Attempt to read `C:\Users\<User>\AppData\Roaming` and `C:\Windows\System32\config\SAM`. Verify `ERROR_ACCESS_DENIED`.
2. **Job Object Termination Test**: Spawn child processes that detach from console (`CREATE_NEW_PROCESS_GROUP`). Close the Job Object handle and verify all descendants are killed.
3. **Named Pipe ACL Test**: Attempt to connect to daemon pipe from a simulated secondary Windows SID. Verify `ERROR_ACCESS_DENIED`.
4. **WSL2 Interoperability Guard**: In Strong mode, verify that `/mnt/c` is unmounted and Windows interoperability (`/proc/sys/fs/binfmt_misc/WSLInterop`) is disabled.
