#![cfg(all(feature = "runtime-service", unix))]
#[test]
fn offline_background_update_lifecycle() {
    let output = std::process::Command::new("python3")
        .args([
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests",
            "-p",
            "test_runtime_update.py",
            "-v",
        ])
        .env("E2EM_SERVICE_BIN", env!("CARGO_BIN_EXE_e2emd"))
        .output()
        .expect("Python update lifecycle runner");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
