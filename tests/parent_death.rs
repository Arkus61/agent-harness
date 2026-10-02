//! Abrupt coordinator death must stop ordinary native descendants.
#![cfg(unix)]
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Running {
    coordinator: Child,
    heartbeat: PathBuf,
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(text) = std::fs::read_to_string(&self.heartbeat) {
            if let Some(pid) = text.lines().next().and_then(|v| v.parse::<i32>().ok()) {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        let _ = self.coordinator.kill();
        let _ = self.coordinator.wait();
    }
}

fn wait_for_heartbeat(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
        assert!(Instant::now() < deadline, "command was not dispatched");
        assert!(
            child.try_wait().unwrap().is_none(),
            "coordinator exited early"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn sigkill_coordinator_stops_grandchild_heartbeat() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let output = Command::new(env!("CARGO_BIN_EXE_harness"))
        .args(["demo", "--dir"])
        .arg(&repo)
        .env_remove("HARNESS_FAILPOINT")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let task_path = repo.join("task.json");
    let mut task: Value = serde_json::from_slice(&std::fs::read(&task_path).unwrap()).unwrap();
    let heartbeat = temp.path().join("grandchild-heartbeat.txt");
    let helper = env!("CARGO_BIN_EXE_harness-test-helper");
    task["decision_mode"] = json!("shadow");
    task["grants"]["commands"] =
        json!([{ "program":helper,"args_prefix":["spawn",heartbeat.to_str().unwrap()] }]);
    task["provider"]["scripts"]["decision:tools"][0]["decision"]["tools"] = json!([
        "read_file",
        "search",
        "write_file",
        "edit_file",
        "run_command"
    ]);
    task["provider"]["scripts"]["builder:fix"] = json!([{ "actions":[{"type":"run_command","program":helper,"args":["spawn",heartbeat.to_str().unwrap()]}],"done":true }]);
    std::fs::write(&task_path, serde_json::to_vec_pretty(&task).unwrap()).unwrap();
    let coordinator = Command::new(env!("CARGO_BIN_EXE_harness"))
        .arg("--repo")
        .arg(&repo)
        .args(["run", "--task"])
        .arg(&task_path)
        .env_remove("HARNESS_FAILPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut running = Running {
        coordinator,
        heartbeat: heartbeat.clone(),
    };
    wait_for_heartbeat(&heartbeat, &mut running.coordinator);
    running.coordinator.kill().unwrap();
    running.coordinator.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let before = std::fs::metadata(&heartbeat).unwrap().len();
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(
        std::fs::metadata(&heartbeat).unwrap().len(),
        before,
        "grandchild continued after coordinator SIGKILL"
    );
}

#[tokio::test]
async fn successful_command_cleanup_stops_background_descendant_and_preserves_exit_code() {
    let temp = tempfile::tempdir().unwrap();
    let heartbeat = temp.path().join("background-heartbeat.txt");
    let spec = agent_harness::types::CommandSpec {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            "\"$1\" heartbeat \"$2\" 25 >/dev/null 2>&1 & while [ ! -s \"$2\" ]; do sleep 0.01; done; exit 7".into(),
            "fixture".into(),
            env!("CARGO_BIN_EXE_harness-test-helper").into(),
            heartbeat.to_string_lossy().into(),
        ],
        timeout_secs: 10,
    };
    let result = agent_harness::execution::run_command(
        temp.path(),
        &spec,
        tokio_util::sync::CancellationToken::new(),
        4096,
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(7));
    assert!(!result.cancelled && !result.timed_out);
    let before = std::fs::metadata(&heartbeat).unwrap().len();
    assert!(before > 0);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(std::fs::metadata(&heartbeat).unwrap().len(), before);
}

#[test]
fn internal_supervisor_refuses_invalid_or_abandoned_headers_before_dispatch() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let heartbeat = temp.path().join("must-not-exist.txt");
    let valid = json!({"protocol":1,"program":env!("CARGO_BIN_EXE_harness-test-helper"),
        "args":["heartbeat",heartbeat.to_str().unwrap(),"25"],"forward_stdin":false});
    let payloads = [
        b"{\"protocol\":1}".to_vec(),
        vec![b'x'; 1024 * 1024 + 1],
        format!("{valid}\n").into_bytes(),
    ];
    for payload in payloads {
        let mut child = Command::new(env!("CARGO_BIN_EXE_harness"))
            .arg("--__harness-native-supervisor")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(&payload).unwrap();
        drop(input);
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(125));
        assert!(
            !heartbeat.exists(),
            "invalid or abandoned request was executed"
        );
    }
}
