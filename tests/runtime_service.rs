#![cfg(all(
    feature = "runtime-service",
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn python_and_node_clients_pass_live_service_conformance() {
    let output = std::process::Command::new("python3")
        .args([
            "-m",
            "unittest",
            "discover",
            "-s",
            "tests",
            "-p",
            "test_runtime_service.py",
            "-v",
        ])
        .env("E2EM_SERVICE_BIN", env!("CARGO_BIN_EXE_e2emd"))
        .output()
        .expect("Python integration runner");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
