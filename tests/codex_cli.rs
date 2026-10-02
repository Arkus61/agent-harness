//! Offline CLI contracts; the helper is an explicit protocol fixture, not a live model.
use serde_json::Value;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    command.args([
        "auth",
        "chatgpt",
        "--check",
        "--codex-program",
        env!("CARGO_BIN_EXE_harness-test-helper"),
    ]);
    // These must not reach the subscription subprocess.
    command.env("OPENAI_API_KEY", "PRIVATE_UNUSED_API_KEY");
    command.env("OPENAI_BASE_URL", "https://private-unused.example");
    command
}

#[test]
fn subscription_cli_checks_reply_and_usage_without_exporting_account_data() {
    let output = command().output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let status: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(status["account_type"], "chatgpt");
    assert_eq!(status["connection_check"]["status"], "PASS");
    assert_eq!(status["connection_check"]["usage"]["complete"], true);
    assert_eq!(status["connection_check"]["usage"]["input_tokens"], 200);
    assert!(!text.contains("PRIVATE"));
}

#[cfg(unix)]
#[test]
fn ctrl_c_on_subscription_cli_cancels_app_server_and_cleans_private_directory() {
    use std::time::{Duration, Instant};
    let temporary = tempfile::tempdir().unwrap();
    let marker = temporary.path().join("started");
    let mut child = command()
        .args(["--model", "fixture-cli-cancel"])
        .env("HARNESS_CODEX_TEST_MARKER", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(5) {
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before mock started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    if !marker.exists() {
        let _ = child.kill();
        panic!("mock app-server did not reach the model call");
    }
    let directory = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        panic!("CLI did not handle Ctrl-C");
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled"));
    assert!(!std::path::Path::new(&directory).exists());
    let heartbeat = marker.with_extension("heartbeat");
    std::thread::sleep(Duration::from_millis(100));
    let first = std::fs::read(&heartbeat).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(first, std::fs::read(&heartbeat).unwrap());
}
