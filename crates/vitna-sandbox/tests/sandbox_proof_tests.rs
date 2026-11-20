//! What the generated invocations claim, checked without running them.
//!
//! These are string-level assertions and they prove only that the arguments say
//! what they should. Whether a process is actually confined is asserted by
//! `vitna-runner`'s `sandbox_enforcement.rs`, which runs a command and tries to
//! escape. Keeping the distinction visible matters: the previous version of
//! this file was the entire evidence behind "Platform Sandbox Enforcement:
//! Verified", and nothing in it ever started a process.

use std::fs;
use std::path::PathBuf;
use vitna_sandbox::{plan, policy_path, PlanError, SandboxConfig, SandboxGuarantee};

fn temp_workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitna_sbx_{}_{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".git").join("hooks")).expect("create .git/hooks");
    fs::write(dir.join(".git").join("config"), "[core]\n").expect("write config");
    dir
}

#[test]
fn test_linux_bwrap_network_isolation() {
    let workspace = PathBuf::from("/home/user/project");
    let mut config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);

    assert!(!config.allow_network);
    let args_no_net = config.generate_linux_bwrap_args();
    assert!(args_no_net.contains(&"--unshare-net".to_string()));
    assert!(args_no_net.contains(&"--unshare-pid".to_string()));
    assert!(args_no_net.contains(&"--unshare-ipc".to_string()));
    // Its own session, so the child cannot inject into the operator's terminal.
    assert!(args_no_net.contains(&"--new-session".to_string()));

    config.allow_network = true;
    let args_with_net = config.generate_linux_bwrap_args();
    assert!(!args_with_net.contains(&"--unshare-net".to_string()));
}

#[test]
fn test_linux_bwrap_mount_permissions() {
    let workspace = PathBuf::from("/home/user/project");

    let ro_config = SandboxConfig::new(&workspace, SandboxGuarantee::ReadOnly);
    let ro_args = ro_config.generate_linux_bwrap_args();
    assert!(
        ro_args.windows(3).any(|w| w[0] == "--ro-bind"
            && w[1] == workspace.to_string_lossy()
            && w[2] == workspace.to_string_lossy()),
        "read-only workspace must use --ro-bind"
    );

    let rw_config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);
    let rw_args = rw_config.generate_linux_bwrap_args();
    assert!(
        rw_args.windows(3).any(|w| w[0] == "--bind"
            && w[1] == workspace.to_string_lossy()
            && w[2] == workspace.to_string_lossy()),
        "writable workspace must use --bind"
    );
}

#[test]
fn test_linux_bwrap_protects_git_metadata() {
    let workspace = temp_workspace("bwrap_git");
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);
    let args = config.generate_linux_bwrap_args();

    let git_config = workspace.join(".git").join("config").to_string_lossy().to_string();
    let hooks = workspace.join(".git").join("hooks").to_string_lossy().to_string();
    let git_dir = workspace.join(".git").to_string_lossy().to_string();

    for protected in [&git_config, &hooks] {
        assert!(
            args.windows(3)
                .any(|w| w[0] == "--ro-bind" && w[1] == *protected && w[2] == *protected),
            "{} must be bound read-only, otherwise a sandboxed process writes itself a \
             git hook that runs on the host later",
            protected
        );
    }

    // `.git` itself has to be a mount point, or the directory is renamed and
    // rebuilt without any of the read-only pieces inside it.
    assert!(
        args.windows(3)
            .any(|w| w[0] == "--bind" && w[1] == git_dir && w[2] == git_dir),
        "the .git directory must be pinned as its own mount point"
    );

    // The workspace bind has to come before the protections layered on top.
    let ws = workspace.to_string_lossy().to_string();
    let ws_at = args.iter().position(|a| *a == ws).expect("workspace bound");
    let cfg_at = args
        .iter()
        .position(|a| *a == git_config)
        .expect("git config bound");
    assert!(
        ws_at < cfg_at,
        "workspace mount must be applied before the read-only overlays inside it"
    );

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn test_macos_seatbelt_profile_generation() {
    let workspace = temp_workspace("seatbelt");

    let ro_config = SandboxConfig::new(&workspace, SandboxGuarantee::ReadOnly);
    let ro_profile = ro_config.generate_macos_seatbelt_profile();
    assert!(ro_profile.contains("(deny default)"));
    assert!(ro_profile.contains("(deny network*)"));
    assert!(
        !ro_profile.contains("file-write*"),
        "a read-only guarantee must grant no write at all"
    );
    // A profile that denies fork and exec cannot run a command at all.
    assert!(ro_profile.contains("(allow process-fork)"));
    assert!(ro_profile.contains("(allow process-exec)"));

    let mut rw_config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);
    rw_config.allow_network = true;
    let rw_profile = rw_config.generate_macos_seatbelt_profile();
    assert!(rw_profile.contains("(allow network-outbound)"));
    assert!(!rw_profile.contains("(deny network*)"));
    // Compared against the resolved form, because that is what Seatbelt
    // matches and what the profile is therefore written against. On macOS a
    // temp dir reached through /var/folders really lives under /private.
    let resolved_ws = policy_path(&workspace);
    let resolved_config = policy_path(&workspace.join(".git").join("config"));
    assert!(rw_profile.contains(&format!("(subpath \"{}\")", resolved_ws.to_string_lossy())));
    assert!(
        rw_profile.contains(&format!(
            "(require-not (subpath \"{}\"))",
            resolved_config.to_string_lossy()
        )),
        "git config must be carved out of the writable subtree: {}",
        rw_profile
    );

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn test_windows_appcontainer_deterministic_naming() {
    let workspace_a = PathBuf::from("C:\\Users\\user\\project_a");
    let workspace_b = PathBuf::from("C:\\Users\\user\\project_b");

    let config_a1 = SandboxConfig::new(&workspace_a, SandboxGuarantee::Guarded);
    let config_a2 = SandboxConfig::new(&workspace_a, SandboxGuarantee::ReadOnly);
    let config_b = SandboxConfig::new(&workspace_b, SandboxGuarantee::Guarded);

    let name_a1 = config_a1.generate_windows_appcontainer_name();
    let name_a2 = config_a2.generate_windows_appcontainer_name();
    let name_b = config_b.generate_windows_appcontainer_name();

    assert!(name_a1.starts_with("VitnaSandbox_"));
    assert_eq!(name_a1.len(), "VitnaSandbox_".len() + 16);
    assert_eq!(name_a1, name_a2);
    assert_ne!(name_a1, name_b);
}

#[test]
fn strong_isolation_is_refused_rather_than_downgraded() {
    let workspace = temp_workspace("strong");
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Strong);

    // Without an explicit pinned OCI image, strong isolation fails closed
    // rather than guessing an image or downgrading to guarded.
    match plan(&config, "echo hello", &workspace) {
        Err(PlanError::Unavailable { reason }) => {
            assert!(
                reason.contains("OCI container image"),
                "unexpected refusal reason: {reason}"
            );
        }
        other => panic!(
            "strong isolation without an explicit image must fail closed, got {:?}",
            other.map(|p| p.backend)
        ),
    }

    // Even with an image, strong isolation either plans OciContainer or returns
    // Unavailable, never Bubblewrap, Seatbelt, or AppContainer.
    let with_image = config.with_container_image("alpine:3.20");
    match plan(&with_image, "echo hello", &workspace) {
        Ok(p) => assert_eq!(p.backend, vitna_sandbox::SandboxBackend::OciContainer),
        Err(PlanError::Unavailable { reason }) => {
            assert!(!reason.is_empty(), "unavailable strong backend must say why");
        }
        Err(other) => panic!("unexpected error for strong isolation: {:?}", other),
    }

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn test_oci_container_strong_args_enforce_isolation_and_protect_git() {
    let workspace = temp_workspace("oci_strong");
    fs::create_dir_all(workspace.join("sub")).unwrap();
    fs::create_dir_all(workspace.join(".vitna")).unwrap();

    let mut config =
        SandboxConfig::new(&workspace, SandboxGuarantee::Strong).with_container_image("alpine:3.20");
    let args = config
        .generate_oci_container_args("podman", "alpine:3.20", "echo ok", &workspace.join("sub"))
        .expect("valid oci args");

    assert_eq!(args[0], "podman");
    assert_eq!(args[1], "run");
    assert!(args.contains(&"--rm".to_string()));
    assert!(args.contains(&"--init".to_string()));
    assert!(args.contains(&"--cap-drop=ALL".to_string()));
    assert!(args.contains(&"--security-opt=no-new-privileges:true".to_string()));
    assert!(args.contains(&"--read-only".to_string()));
    assert!(args.contains(&"--network=none".to_string()));
    assert!(args.iter().any(|a| a.starts_with("--memory=")));
    assert!(args.iter().any(|a| a.starts_with("--pids-limit=")));
    assert!(args.windows(2).any(|w| w[0] == "--workdir" && w[1] == "/workspace/sub"));
    assert!(args
        .iter()
        .any(|a| a.contains("dst=/workspace/.git/config,readonly")));
    assert!(args
        .iter()
        .any(|a| a.contains("dst=/workspace/.git/hooks,readonly")));
    assert!(args
        .iter()
        .any(|a| a.contains("dst=/workspace/.vitna,readonly")));

    // Allowing network removes --network=none.
    config.allow_network = true;
    let net_args = config
        .generate_oci_container_args("docker", "alpine:3.20", "echo ok", &workspace)
        .expect("valid oci args with net");
    assert!(!net_args.contains(&"--network=none".to_string()));

    // Flag-injection image names are refused.
    assert!(config
        .generate_oci_container_args("podman", "--privileged", "echo ok", &workspace)
        .is_err());
    assert!(config
        .generate_oci_container_args("podman", "alpine latest", "echo ok", &workspace)
        .is_err());

    let _ = fs::remove_dir_all(&workspace);
}

#[test]
fn full_access_is_never_planned_as_a_sandbox() {
    let workspace = temp_workspace("full");
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::FullAccess);

    assert!(
        matches!(
            plan(&config, "echo hello", &workspace),
            Err(PlanError::NotImplemented { .. })
        ),
        "full_access is host execution; it must never come back as a plan that a \
         caller could report as sandboxed"
    );

    let _ = fs::remove_dir_all(&workspace);
}

/// On a platform with no implemented backend the answer is an error naming the
/// reason, never a plan. Windows is that platform today.
#[test]
fn guarded_either_plans_or_says_why_not() {
    let workspace = temp_workspace("guarded");
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);

    match plan(&config, "echo hello", &workspace) {
        Ok(p) => {
            assert!(!p.program.is_empty());
            assert!(
                p.args.iter().any(|a| a.contains("echo hello")),
                "the planned invocation must actually carry the command"
            );
        }
        Err(PlanError::Unavailable { reason }) => {
            assert!(!reason.is_empty(), "an unavailable backend must say why");
        }
        Err(other) => panic!("guarded should plan or be unavailable, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&workspace);
}
