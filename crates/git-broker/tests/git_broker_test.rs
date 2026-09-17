use std::fs;
use vitna_git_broker::{GitBroker, NULL_HASH};
use vitna_git_workspaces::AgentWorkspaceManager;

#[test]
fn test_git_broker_multi_file_workflow() {
    let temp_dir = std::env::temp_dir().join(format!("vitna_broker_integration_{}", std::process::id()));
    fs::create_dir_all(temp_dir.join("src")).expect("create primary src");
    fs::write(temp_dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("write Cargo.toml");

    // Step 1: Create isolated workspace
    let ws = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-multi-01")
        .expect("create workspace");

    // Step 2: In agent workspace, create a new file and modify Cargo.toml
    fs::write(
        ws.agent_workspace_root.join("src/service.rs"),
        "pub struct Service;\n",
    )
    .expect("write service.rs");

    fs::write(
        ws.agent_workspace_root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.2.0\"\n",
    )
    .expect("modify Cargo.toml");

    // Step 3: Inspect changes via GitBroker
    let changeset = GitBroker::inspect_changes(&ws).expect("inspect changes");
    assert_eq!(changeset.files.len(), 2);

    let service_change = changeset
        .files
        .iter()
        .find(|f| f.path == "src/service.rs")
        .expect("service.rs change exists");
    assert_eq!(service_change.preimage_hash, NULL_HASH);

    let cargo_change = changeset
        .files
        .iter()
        .find(|f| f.path == "Cargo.toml")
        .expect("Cargo.toml change exists");
    assert_ne!(cargo_change.preimage_hash, NULL_HASH);

    // Step 4: Validate preimages against primary
    let conflicts = GitBroker::validate_preimages(&temp_dir, &changeset).expect("validate preimages");
    assert!(conflicts.is_empty(), "No concurrent edits; conflicts must be empty");

    // Step 5: Apply changeset to primary
    let merge_res = GitBroker::apply_changeset(&temp_dir, &ws, &changeset).expect("apply changeset");
    assert!(merge_res.success);
    assert_eq!(merge_res.applied_files.len(), 2);

    // Step 6: Verify primary state
    assert!(temp_dir.join("src/service.rs").exists());
    let updated_cargo = fs::read_to_string(temp_dir.join("Cargo.toml")).expect("read cargo");
    assert!(updated_cargo.contains("version = \"0.2.0\""));

    // Cleanup
    AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup ws");
    let _ = fs::remove_dir_all(&temp_dir);
}
