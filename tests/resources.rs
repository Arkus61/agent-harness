//! Resource fixtures never allocate more than 64 MiB, write more than 16 KiB,
//! create more than one child, or consume more than four seconds of CPU.
use agent_harness::execution::run_command;
use agent_harness::types::CommandSpec;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn command(mode: &str, arguments: &[&str], limits: Value) -> CommandSpec {
    let parsed = serde_json::from_value(json!({
        "program": env!("CARGO_BIN_EXE_harness-test-helper"),
        "args": std::iter::once(mode).chain(arguments.iter().copied()).collect::<Vec<_>>(),
        "timeout_secs": 10,
        "resource_limits": limits
    }));
    assert!(
        parsed.is_ok(),
        "CommandSpec must accept explicit resource limits: {parsed:?}"
    );
    parsed.unwrap()
}

#[test]
fn existing_commands_keep_their_serialized_contract_when_limits_are_absent() {
    let original = json!({"program":"old-command","args":[],"timeout_secs":3});
    let spec: CommandSpec = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(spec).unwrap(), original);
}

#[test]
fn explicit_resource_limits_are_preserved_in_the_command_contract() {
    let spec = command("echo", &[], json!({"cpu_seconds":1}));
    assert_eq!(
        serde_json::to_value(spec).unwrap()["resource_limits"]["cpu_seconds"],
        1
    );
}

#[tokio::test]
async fn aggregate_requests_fail_before_the_target_is_dispatched() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-exist.txt");
    for limits in [
        json!({"aggregate_cpu_seconds":1}),
        json!({"aggregate_memory_bytes":1024*1024}),
        json!({"aggregate_disk_bytes":4096}),
    ] {
        let spec = command("resource-marker", &[marker.to_str().unwrap()], limits);
        let result = run_command(root.path(), &spec, CancellationToken::new(), 4096).await;
        assert!(
            result.is_err(),
            "unsupported aggregate limits must fail closed"
        );
        assert!(!marker.exists(), "denied target must never start");
    }
}

#[tokio::test]
async fn zero_limits_are_rejected_before_target_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-exist.txt");
    for limits in [
        json!({"cpu_seconds":0}),
        json!({"address_space_bytes":0}),
        json!({"file_size_bytes":0}),
        json!({"processes":0}),
        json!({"cpu_seconds":u64::MAX}),
    ] {
        let spec = command("resource-marker", &[marker.to_str().unwrap()], limits);
        assert!(
            run_command(root.path(), &spec, CancellationToken::new(), 4096)
                .await
                .is_err()
        );
        assert!(!marker.exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cpu_limit_stops_a_bounded_spin_without_waiting_for_wall_timeout() {
    let root = tempfile::tempdir().unwrap();
    let spec = command("resource-cpu", &[], json!({"cpu_seconds":1}));
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert!(result.stdout.contains("CPU_STARTED"), "{result:?}");
    assert!(!result.stdout.contains("CPU_FINISHED"), "{result:?}");
    assert_ne!(result.exit_code, Some(0));
    assert!(
        !result.timed_out && !result.cancelled,
        "CPU limit must be kernel enforced"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
async fn address_space_limit_denies_a_bounded_fallible_allocation() {
    let root = tempfile::tempdir().unwrap();
    let spec = command(
        "resource-memory",
        &[],
        json!({"address_space_bytes":32*1024*1024}),
    );
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(42), "{result:?}");
    assert!(result.stdout.contains("MEMORY_DENIED"), "{result:?}");
    assert!(!result.timed_out && !result.cancelled);
}

#[cfg(unix)]
#[tokio::test]
async fn file_size_limit_bounds_one_regular_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bounded-file.bin");
    let spec = command(
        "resource-file",
        &[path.to_str().unwrap()],
        json!({"file_size_bytes":4096}),
    );
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert_ne!(result.exit_code, Some(0), "{result:?}");
    assert!(std::fs::metadata(path).unwrap().len() <= 4096);
    assert!(!result.timed_out && !result.cancelled);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn uid_process_limit_denies_a_single_fork_or_fails_closed_when_privileged() {
    let root = tempfile::tempdir().unwrap();
    let spec = command("resource-process", &[], json!({"processes":1}));
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096).await;
    if unsafe { libc::geteuid() } == 0 {
        assert!(
            result.is_err(),
            "Linux root bypasses RLIMIT_NPROC and must fail closed"
        );
    } else {
        let result = result.unwrap();
        assert_eq!(result.exit_code, Some(42), "{result:?}");
        assert!(result.stdout.contains("PROCESS_DENIED"), "{result:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn hard_limits_are_inherited_and_cannot_be_raised_by_the_target() {
    let root = tempfile::tempdir().unwrap();
    let spec = command(
        "resource-inherit",
        &[],
        json!({"cpu_seconds":5,"file_size_bytes":8192}),
    );
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    let value: Value = serde_json::from_str(result.stdout.trim()).unwrap();
    assert_eq!(value["cpu"], json!({"soft":5,"hard":5,"raised":false}));
    assert_eq!(
        value["file"],
        json!({"soft":8192,"hard":8192,"raised":false})
    );
}

#[test]
fn task_defaults_do_not_launder_invalid_explicit_resource_values() {
    let spec = command("echo", &[], json!({"cpu_seconds":u64::MAX}));
    let defaults = serde_json::from_value(json!({"cpu_seconds":1})).unwrap();
    assert!(agent_harness::resources::apply_default_limits(&spec, Some(&defaults)).is_err());
}

#[test]
fn task_defaults_intersect_every_explicit_and_aggregate_constraint() {
    let spec = command(
        "echo",
        &[],
        json!({"cpu_seconds":10,"address_space_bytes":100,"file_size_bytes":50,"processes":8,"aggregate_memory_bytes":1000}),
    );
    let defaults = serde_json::from_value(json!({"cpu_seconds":2,"address_space_bytes":200,"file_size_bytes":20,"processes":4,"aggregate_cpu_seconds":1,"aggregate_memory_bytes":2000,"aggregate_disk_bytes":1000})).unwrap();
    let effective = agent_harness::resources::apply_default_limits(&spec, Some(&defaults)).unwrap();
    assert_eq!(
        serde_json::to_value(effective).unwrap()["resource_limits"],
        json!({"cpu_seconds":2,"address_space_bytes":100,"file_size_bytes":20,"processes":4,"aggregate_cpu_seconds":1,"aggregate_memory_bytes":1000,"aggregate_disk_bytes":1000})
    );
}

#[test]
fn decision_hash_binds_resource_policy_and_detects_stale_approval() {
    use agent_harness::decision::{DecisionRequest, DecisionScope};
    use agent_harness::types::{Action, Grants};
    let limits = serde_json::from_value(json!({"cpu_seconds":2})).unwrap();
    let grants: Grants =
        serde_json::from_value(json!({"commands":[{"program":"fixture","args_prefix":[]}]}))
            .unwrap();
    let scope = DecisionScope::new(&grants, &[], false, &[])
        .unwrap()
        .with_resource_limits(Some(&limits))
        .unwrap();
    let mut request = DecisionRequest::risk(
        &Action::RunCommand {
            program: "fixture".into(),
            args: vec![],
        },
        scope,
    )
    .unwrap();
    request.validate().unwrap();
    request
        .effective_scope
        .command_resource_limits
        .as_mut()
        .unwrap()
        .cpu_seconds = Some(3);
    assert!(
        request.validate().is_err(),
        "changing execution caps must invalidate the approval binding"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_explicit_check_cannot_widen_the_default_file_size_cap() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("effective-cap.bin");
    let spec = command(
        "resource-file",
        &[path.to_str().unwrap()],
        json!({"file_size_bytes":8192}),
    );
    let defaults = serde_json::from_value(json!({"file_size_bytes":2048})).unwrap();
    let effective = agent_harness::resources::apply_default_limits(&spec, Some(&defaults)).unwrap();
    let result = run_command(root.path(), &effective, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert_ne!(result.exit_code, Some(0));
    assert!(std::fs::metadata(path).unwrap().len() <= 2048);
}

#[test]
fn unsupported_task_defaults_block_before_attempts_models_or_actions() {
    let parent = tempfile::tempdir().unwrap();
    let repo = parent.path().join("fixture");
    let demo = std::process::Command::new(env!("CARGO_BIN_EXE_harness"))
        .args(["demo", "--dir"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(
        demo.status.success(),
        "{}",
        String::from_utf8_lossy(&demo.stderr)
    );
    let task_path = repo.join("task.json");
    let mut task: Value = serde_json::from_slice(&std::fs::read(&task_path).unwrap()).unwrap();
    task["command_resource_limits"] = json!({"aggregate_memory_bytes":1024*1024});
    task["provider"]["scripts"] = json!({});
    std::fs::write(&task_path, serde_json::to_vec(&task).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_harness"))
        .arg("--repo")
        .arg(&repo)
        .args(["run", "--task"])
        .arg(&task_path)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["run"]["state"], "BLOCKED", "{report}");
    assert!(report["run"]["error"]
        .as_str()
        .unwrap()
        .contains("aggregate resource limits unavailable"));
    assert!(report["attempts"].as_array().unwrap().is_empty());
    assert!(report["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["kind"] != "model_intent" && event["kind"] != "action_intent"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_targets_inherit_the_same_hard_limits() {
    let capability = agent_harness::isolation::availability();
    if !capability.available {
        assert_ne!(
            std::env::var("HARNESS_REQUIRE_LINUX_ISOLATION").as_deref(),
            Ok("1"),
            "{capability:?}"
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("resource-helper");
    std::fs::copy(env!("CARGO_BIN_EXE_harness-test-helper"), &helper).unwrap();
    let mut spec = command(
        "resource-inherit",
        &[],
        json!({"cpu_seconds":5,"file_size_bytes":8192}),
    );
    spec.program = helper.to_string_lossy().into();
    let result = agent_harness::isolation::run_isolated(
        root.path(),
        &spec,
        CancellationToken::new(),
        4096,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    let value: Value = serde_json::from_str(result.stdout.trim()).unwrap();
    assert_eq!(value["cpu"], json!({"soft":5,"hard":5,"raised":false}));
    assert_eq!(
        value["file"],
        json!({"soft":8192,"hard":8192,"raised":false})
    );
}

#[tokio::test]
async fn bounded_memory_and_file_controls_execute_without_resource_limits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("control.bin");
    for (mode, args, expected) in [
        ("resource-memory", vec![], "MEMORY_ALLOCATED"),
        (
            "resource-file",
            vec![path.to_str().unwrap()],
            "FILE_WRITTEN",
        ),
    ] {
        let mut spec = command(mode, &args, json!({}));
        spec.resource_limits = None;
        let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(result.stdout.contains(expected), "{result:?}");
    }
    assert_eq!(std::fs::metadata(path).unwrap().len(), 16 * 1024);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn requested_hard_limits_disable_exec_privilege_gains_in_the_target() {
    let root = tempfile::tempdir().unwrap();
    let spec = command("resource-privilege", &[], json!({"cpu_seconds":5}));
    let result = run_command(root.path(), &spec, CancellationToken::new(), 4096)
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(result.stdout.trim(), "NO_NEW_PRIVS=1");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_uid_and_capabilities_do_not_bypass_the_inherited_nproc_limit() {
    let capability = agent_harness::isolation::availability();
    if !capability.available {
        assert_ne!(
            std::env::var("HARNESS_REQUIRE_LINUX_ISOLATION").as_deref(),
            Ok("1"),
            "{capability:?}"
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("resource-helper");
    std::fs::copy(env!("CARGO_BIN_EXE_harness-test-helper"), &helper).unwrap();
    let mut spec = command("resource-isolated-process", &[], json!({"processes":4096}));
    spec.program = helper.to_string_lossy().into();
    let result = agent_harness::isolation::run_isolated(
        root.path(),
        &spec,
        CancellationToken::new(),
        4096,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    let value: Value = serde_json::from_str(result.stdout.trim()).unwrap();
    assert_eq!(value["uid"], unsafe { libc::geteuid() });
    assert_eq!(value["effective_capabilities"], "0000000000000000");
    assert_eq!(value["inherited_soft"], 4096);
    assert_eq!(value["inherited_hard"], 4096);
    assert_eq!(value["raise_denied"], true);
    assert_eq!(value["fork_denied_after_self_tightening_to_one"], true);
}
