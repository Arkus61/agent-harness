use agent_harness::execution::{
    command_allowed, read_file, run_command, validate_action, write_file,
};
use agent_harness::types::{Action, CommandGrant, CommandSpec, Grants, TaskSpec};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn task() -> TaskSpec {
    serde_json::from_value(json!({
        "prompt": "Make the requested change",
        "requirements": ["The command exits successfully"],
        "grants": {"read":["**"],"write":["src/**"],"commands":[]},
        "checks": [{"program":"fixture-check","args":[],"timeout_secs":1}],
        "provider": {"kind":"scripted","scripts":{}}
    }))
    .unwrap()
}

#[test]
fn invalid_specs_are_rejected_before_execution() {
    let valid = task();
    valid.validate().unwrap();
    let mut invalid = valid.clone();
    invalid.prompt = " ".into();
    assert!(invalid.validate().is_err());
    invalid = valid.clone();
    invalid.requirements.clear();
    assert!(invalid.validate().is_err());
    invalid = valid.clone();
    invalid.checks.clear();
    assert!(invalid.validate().is_err());
    invalid = valid.clone();
    invalid.budget.max_output_tokens = invalid.budget.max_tokens + 1;
    assert!(invalid.validate().is_err());
    let mut unknown_field = serde_json::to_value(valid).unwrap();
    unknown_field["unrecognized_permission"] = json!(true);
    assert!(serde_json::from_value::<TaskSpec>(unknown_field).is_err());
}

#[test]
fn gateway_denies_commands_and_unowned_writes() {
    let grants = Grants {
        read: vec!["**".into()],
        write: vec!["src/**".into()],
        commands: vec![CommandGrant {
            program: "git".into(),
            args_prefix: vec!["status".into()],
        }],
    };
    assert!(command_allowed("git", &["status".into()], &grants));
    assert!(!command_allowed("git", &["push".into()], &grants));
    assert!(!command_allowed("sh", &["-c".into()], &grants));
    let denied = Action::RunCommand {
        program: "sh".into(),
        args: vec!["-c".into(), "write-outside-scope".into()],
    };
    assert!(validate_action(&denied, &grants, &["src/**".into()], false).is_err());
    let write = Action::WriteFile {
        path: "src/other.rs".into(),
        content: "replacement".into(),
        expected_hash: None,
    };
    assert!(validate_action(&write, &grants, &["src/owned.rs".into()], false).is_err());
    assert!(validate_action(&write, &grants, &["src/**".into()], true).is_err());
}

#[test]
fn unicode_paths_hashes_and_traversal_are_checked() {
    let root = tempfile::tempdir().unwrap();
    let grants = Grants {
        read: vec!["**".into()],
        write: vec!["**".into()],
        commands: vec![],
    };
    let path = "каталог с пробелами/данные.txt";
    let content = "Unicode ✓\r\n";
    let hash = write_file(root.path(), path, content, None, &grants).unwrap();
    assert_eq!(hash, blake3::hash(content.as_bytes()).to_hex().to_string());
    assert_eq!(
        read_file(root.path(), path, &grants, 1024).unwrap(),
        content
    );
    assert!(write_file(root.path(), path, "changed", Some("wrong-hash"), &grants).is_err());
    assert_eq!(
        read_file(root.path(), path, &grants, 1024).unwrap(),
        content
    );
    assert!(read_file(root.path(), "../outside.txt", &grants, 1024).is_err());
    assert!(write_file(root.path(), "../outside.txt", "bad", None, &grants).is_err());
}

fn fixture(command: &str, arguments: &[&str], timeout_secs: u64) -> CommandSpec {
    CommandSpec {
        program: env!("CARGO_BIN_EXE_harness-test-helper").into(),
        args: std::iter::once(command.to_owned())
            .chain(arguments.iter().map(|v| (*v).to_owned()))
            .collect(),
        timeout_secs,
        resource_limits: None,
    }
}

#[tokio::test]
async fn subprocess_stdout_is_bounded_and_timeout_is_reported() {
    let root = tempfile::tempdir().unwrap();
    let output = run_command(
        root.path(),
        &fixture("stdoutflood", &["1048576"], 10),
        CancellationToken::new(),
        4096,
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, Some(0));
    assert!(output.truncated);
    assert!(output.stdout.len() <= 4096);
    assert!(!output.timed_out);
    let timed_out = run_command(
        root.path(),
        &fixture("sleep", &["60000"], 1),
        CancellationToken::new(),
        4096,
    )
    .await
    .unwrap();
    assert!(timed_out.timed_out);
    assert_ne!(timed_out.exit_code, Some(0));
}

#[tokio::test]
async fn cancellation_stops_descendant_heartbeat() {
    let root = tempfile::tempdir().unwrap();
    let heartbeat = root.path().join("heartbeat.txt");
    let spec = fixture("spawn", &[heartbeat.to_str().unwrap()], 30);
    let token = CancellationToken::new();
    let stop = token.clone();
    let path = heartbeat.clone();
    let canceller = tokio::spawn(async move {
        for _ in 0..200 {
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        stop.cancel();
    });
    let result = run_command(root.path(), &spec, token, 4096).await.unwrap();
    canceller.await.unwrap();
    assert!(result.cancelled);
    let before = std::fs::metadata(&heartbeat).unwrap().len();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let after = std::fs::metadata(&heartbeat).unwrap().len();
    assert_eq!(after, before, "descendant must stop after cancellation");
}

#[tokio::test]
async fn evaluation_reports_executed_fixture_assertions() {
    let report = agent_harness::evaluation::run_suite().await.unwrap();
    assert_eq!(report["verification_mode"], "fixture");
    let criteria = report["criteria"].as_array().unwrap();
    assert!(criteria.len() >= 20);
    assert_eq!(report["summary"]["criteria"], criteria.len());
    let passed = criteria.iter().filter(|v| v["verdict"] == "PASS").count();
    assert_eq!(report["summary"]["passed"], passed);
    assert_eq!(report["summary"]["failed"], criteria.len() - passed);
    assert_eq!(report["passed"], true, "{report:#}");
}
