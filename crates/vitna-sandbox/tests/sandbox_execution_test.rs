use std::path::PathBuf;
use vitna_sandbox::{SandboxConfig, SandboxExecutor, SandboxGuarantee};

#[tokio::test]
async fn test_sandbox_executor_basic_execution() {
    let workspace = std::env::temp_dir();
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);

    let cmd = if cfg!(windows) {
        "echo vitna_sandbox_test"
    } else {
        "echo vitna_sandbox_test"
    };

    let res = SandboxExecutor::execute(&config, cmd, &workspace, 5000)
        .await
        .expect("execution succeeds");

    assert_eq!(res.exit_code, 0);
    assert!(res.stdout.contains("vitna_sandbox_test"));
    assert_eq!(res.statement_digest.len(), 64);
}

#[tokio::test]
async fn test_sandbox_executor_environment_sanitization() {
    // Set a sensitive variable in the parent process
    std::env::set_var("VITNA_SECRET_KEY_EXPOSURE_TEST", "super_secret_token_123");

    let workspace = std::env::temp_dir();
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);

    let check_cmd = if cfg!(windows) {
        "set VITNA_SECRET_KEY_EXPOSURE_TEST"
    } else {
        "echo $VITNA_SECRET_KEY_EXPOSURE_TEST"
    };

    let res = SandboxExecutor::execute(&config, check_cmd, &workspace, 5000)
        .await
        .expect("execute");

    // In sanitized sandbox environment, the secret variable is cleared
    assert!(!res.stdout.contains("super_secret_token_123"));

    std::env::remove_var("VITNA_SECRET_KEY_EXPOSURE_TEST");
}

#[tokio::test]
async fn test_sandbox_executor_timeout_enforcement() {
    let workspace = std::env::temp_dir();
    let config = SandboxConfig::new(&workspace, SandboxGuarantee::Guarded);

    let sleep_cmd = if cfg!(windows) {
        "powershell -Command Start-Sleep -Seconds 5"
    } else {
        "sleep 5"
    };

    // Set timeout to 100ms
    let res = SandboxExecutor::execute(&config, sleep_cmd, &workspace, 100).await;

    assert!(res.is_err(), "Must fail with timeout error");
    let err_msg = res.unwrap_err();
    assert!(err_msg.contains("timed out after 100 ms"));
}
