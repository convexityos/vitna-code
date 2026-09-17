use std::path::PathBuf;
use vitna_sandbox::{SandboxConfig, SandboxGuarantee};

#[test]
fn test_linux_bwrap_network_isolation() {
    let workspace = PathBuf::from("/home/user/project");
    let mut config = SandboxConfig::new(&workspace, SandboxGuarantee::Strong);

    // Default configuration has network disabled
    assert!(!config.allow_network);
    let args_no_net = config.generate_linux_bwrap_args();
    assert!(args_no_net.contains(&"--unshare-net".to_string()));
    assert!(args_no_net.contains(&"--unshare-pid".to_string()));
    assert!(args_no_net.contains(&"--unshare-ipc".to_string()));

    // When network is allowed, --unshare-net is omitted
    config.allow_network = true;
    let args_with_net = config.generate_linux_bwrap_args();
    assert!(!args_with_net.contains(&"--unshare-net".to_string()));
}

#[test]
fn test_linux_bwrap_mount_permissions() {
    let workspace = PathBuf::from("/home/user/project");

    // Read-only guarantee creates ro-bind for workspace
    let ro_config = SandboxConfig::new(&workspace, SandboxGuarantee::ReadOnly);
    let ro_args = ro_config.generate_linux_bwrap_args();
    let ro_mount_idx = ro_args
        .windows(3)
        .position(|window| {
            window[0] == "--ro-bind"
                && window[1] == workspace.to_string_lossy()
                && window[2] == workspace.to_string_lossy()
        });
    assert!(ro_mount_idx.is_some(), "Read-only workspace must use --ro-bind");

    // Strong guarantee creates writable bind for workspace
    let rw_config = SandboxConfig::new(&workspace, SandboxGuarantee::Strong);
    let rw_args = rw_config.generate_linux_bwrap_args();
    let rw_mount_idx = rw_args
        .windows(3)
        .position(|window| {
            window[0] == "--bind"
                && window[1] == workspace.to_string_lossy()
                && window[2] == workspace.to_string_lossy()
        });
    assert!(rw_mount_idx.is_some(), "Writable workspace must use --bind");
}

#[test]
fn test_macos_seatbelt_profile_generation() {
    let workspace = PathBuf::from("/Users/user/project");

    // Read-only profile
    let ro_config = SandboxConfig::new(&workspace, SandboxGuarantee::ReadOnly);
    let ro_profile = ro_config.generate_macos_seatbelt_profile();
    assert!(ro_profile.contains("(deny default)"));
    assert!(ro_profile.contains("(deny network*)"));
    assert!(ro_profile.contains(&format!("(allow file-read* (subpath \"{}\"))", workspace.to_string_lossy())));
    assert!(!ro_profile.contains(&format!("(allow file-read* file-write* (subpath \"{}\"))", workspace.to_string_lossy())));

    // Guarded/Strong profile with network enabled
    let mut rw_config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);
    rw_config.allow_network = true;
    let rw_profile = rw_config.generate_macos_seatbelt_profile();
    assert!(rw_profile.contains("(allow network-outbound)"));
    assert!(!rw_profile.contains("(deny network*)"));
    assert!(rw_profile.contains(&format!("(allow file-read* file-write* (subpath \"{}\"))", workspace.to_string_lossy())));
}

#[test]
fn test_windows_appcontainer_deterministic_naming() {
    let workspace_a = PathBuf::from("C:\\Users\\user\\project_a");
    let workspace_b = PathBuf::from("C:\\Users\\user\\project_b");

    let config_a1 = SandboxConfig::new(&workspace_a, SandboxGuarantee::Strong);
    let config_a2 = SandboxConfig::new(&workspace_a, SandboxGuarantee::ReadOnly);
    let config_b = SandboxConfig::new(&workspace_b, SandboxGuarantee::Strong);

    let name_a1 = config_a1.generate_windows_appcontainer_name();
    let name_a2 = config_a2.generate_windows_appcontainer_name();
    let name_b = config_b.generate_windows_appcontainer_name();

    assert!(name_a1.starts_with("VitnaSandbox_"));
    assert_eq!(name_a1.len(), "VitnaSandbox_".len() + 16); // 8 bytes hex encoded = 16 chars

    // Identical paths yield identical AppContainer isolation profiles
    assert_eq!(name_a1, name_a2);

    // Distinct paths yield distinct AppContainer isolation profiles
    assert_ne!(name_a1, name_b);
}
