#![cfg(all(feature = "runtime-service", windows))]
use e2em_runtime::runtime::service::{proof, read_frame, write_frame};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
};

async fn connect(pipe: &str) -> NamedPipeClient {
    for _ in 0..500 {
        if let Ok(client) = ClientOptions::new().open(pipe) {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("named pipe did not become ready");
}
async fn read(client: &mut NamedPipeClient) -> Value {
    serde_json::from_slice(
        &tokio::time::timeout(Duration::from_secs(5), read_frame(client))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}
async fn authenticate(pipe: &str, secret: &str) -> NamedPipeClient {
    let mut client = connect(pipe).await;
    let nonce = "c".repeat(64);
    write_frame(&mut client, &json!({"principal":"app", "nonce":nonce}))
        .await
        .unwrap();
    let challenge = read(&mut client).await;
    assert_eq!(challenge["provider"], "windows-test");
    let server = challenge["nonce"].as_str().unwrap();
    assert_eq!(
        challenge["proof"],
        proof(secret, "server", "windows-test", "app", &nonce, server)
    );
    let response = proof(secret, "client", "windows-test", "app", &nonce, server);
    write_frame(&mut client, &json!({"proof":response}))
        .await
        .unwrap();
    assert_eq!(read(&mut client).await["authenticated"], true);
    client
}
struct Fixture {
    directory: PathBuf,
    grants: PathBuf,
    pipe: String,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("e2em-pipe-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let grants = directory.join("grants.json");
        let sid = e2em_platform::current_user_sid().unwrap();
        e2em_platform::write_private_file(&grants, &serde_json::to_vec(&json!({"provider":"windows-test", "grants":[{"principal":"app", "sid":sid, "secret":"a".repeat(64)}]})).unwrap()).unwrap();
        let pipe = format!(r"\\.\pipe\e2em-test-{}", std::process::id());
        Self {
            directory,
            grants,
            pipe,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn named_pipe_authentication_fixtures_revocation_and_restart() {
    let fixture = Fixture::new();
    let pipe = fixture.pipe.clone();
    let grants = fixture.grants.clone();
    let service = tokio::spawn(async move {
        e2em_runtime::runtime::service::serve(&pipe, &grants, Duration::from_millis(50)).await
    });
    let secret = "a".repeat(64);
    let mut client = authenticate(&fixture.pipe, &secret).await;
    let mut second = authenticate(&fixture.pipe, &secret).await;
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("conformance/assessments.json")).unwrap();
    for (index, case) in cases.iter().enumerate() {
        write_frame(&mut client, &json!({"call_id":format!("case-{index}"), "api_version":"0.1", "operation":{"op":"assess","request":case["request"]}})).await.unwrap();
        let response = read(&mut client).await;
        let assessment = &response["reply"]["assessment"];
        assert_eq!(assessment["status"], case["expected"]["status"]);
        assert_eq!(assessment["action"], case["expected"]["action"]);
        let spans: Vec<Value> = assessment["findings"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|f| f["spans"].as_array().unwrap())
            .map(|s| json!([s["start"], s["end"]]))
            .collect();
        assert_eq!(json!(spans), case["expected"]["spans"]);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    write_frame(
        &mut second,
        &json!({"call_id":"caps", "api_version":"0.1", "operation":{"op":"capabilities"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        read(&mut second).await["reply"]["capabilities"]["runtime_state"],
        "unloaded"
    );
    let mut forged = connect(&fixture.pipe).await;
    write_frame(
        &mut forged,
        &json!({"principal":"app", "nonce":"c".repeat(64)}),
    )
    .await
    .unwrap();
    read(&mut forged).await;
    write_frame(&mut forged, &json!({"proof":"0".repeat(64)}))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut forged))
            .await
            .unwrap()
            .is_err()
    );
    let mut oversized = connect(&fixture.pipe).await;
    oversized.write_u32(131073).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut oversized))
            .await
            .unwrap()
            .is_err()
    );
    let mut malformed = connect(&fixture.pipe).await;
    malformed.write_u32(1).await.unwrap();
    malformed.write_all(b"{").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut malformed))
            .await
            .unwrap()
            .is_err()
    );
    std::fs::remove_file(&fixture.grants).unwrap();
    e2em_platform::write_private_file(
        &fixture.grants,
        br#"{"provider":"windows-test","grants":[]}"#,
    )
    .unwrap();
    write_frame(
        &mut client,
        &json!({"call_id":"revoked", "api_version":"0.1", "operation":{"op":"capabilities"}}),
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut client))
            .await
            .unwrap()
            .is_err()
    );
    drop(client);
    drop(second);
    drop(forged);
    drop(oversized);
    drop(malformed);
    service.abort();
    let _ = service.await;
    assert!(
        e2em_platform::create_user_pipe(&fixture.pipe, true).is_ok(),
        "shutdown releases namespace for restart"
    );
}

#[tokio::test]
async fn first_instance_rejects_namespace_takeover() {
    let name = format!(r"\\.\pipe\e2em-exclusive-{}", std::process::id());
    let _server = e2em_platform::create_user_pipe(&name, true).unwrap();
    assert!(e2em_platform::create_user_pipe(&name, true).is_err());
    assert!(e2em_platform::create_user_pipe(&name, false).is_ok());
}

#[test]
fn windows_registry_rejects_foreign_sid_legacy_uid_duplicates_and_large_files() {
    let path = std::env::temp_dir().join(format!("e2em-registry-{}.json", std::process::id()));
    let sid = e2em_platform::current_user_sid().unwrap();
    for grants in [
        json!([{"principal":"app","sid":"S-1-5-18","secret":"a".repeat(64)}]),
        json!([{"principal":"app","uid":0,"secret":"a".repeat(64)}]),
        json!([{"principal":"app","sid":sid,"secret":"a".repeat(64)}, {"principal":"app","sid":sid,"secret":"b".repeat(64)}]),
    ] {
        e2em_platform::write_private_file(
            &path,
            &serde_json::to_vec(&json!({"provider":"test","grants":grants})).unwrap(),
        )
        .unwrap();
        assert!(e2em_runtime::runtime::service::read_grants(&path).is_err());
        assert!(e2em_platform::write_private_file(&path, b"overwrite").is_err());
        std::fs::remove_file(&path).unwrap();
    }
    e2em_platform::write_private_file(&path, &vec![b' '; 65_537]).unwrap();
    assert!(e2em_platform::read_private_file(&path).is_err());
    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn pipe_names_reject_remote_and_unrelated_names() {
    for name in [
        r"\\remote\pipe\e2em-app",
        r"\\.\pipe\other",
        r"\\.\pipe\e2em-",
        r"\\.\pipe\e2em-app/path",
    ] {
        assert!(e2em_platform::create_user_pipe(name, true).is_err());
    }
}
