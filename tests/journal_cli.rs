use serde_json::{json, Value};
use std::{path::Path, process::Command};

fn cli(repo: Option<&Path>, args: &[&str]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    if let Some(repo) = repo {
        command.arg("--repo").arg(repo);
    }
    let output = command.args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn replay_and_local_outbox_are_accessible_through_the_real_cli() {
    let parent = tempfile::tempdir().unwrap();
    let repo = parent.path().join("журнал с пробелами");
    let report = cli(None, &["demo", "--dir", repo.to_str().unwrap()]);
    assert_eq!(report["run"]["state"], "FIXTURE_VERIFIED");
    let compiled: Vec<_> = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "context.compiled")
        .collect();
    assert_eq!(
        compiled.len(),
        5,
        "builder and four reviewer context compilations"
    );
    for event in compiled {
        assert_eq!(event["payload"]["evidence"]["claim_status"], "unknown");
        assert!(event["payload"]["cache_key"].as_str().unwrap().len() == 64);
    }
    let id = report["run"]["id"].as_str().unwrap();
    let replay = cli(Some(&repo), &["replay", id]);
    assert_eq!(replay["origin"], "historical_journal");
    assert_eq!(replay["dispatched_effects"], 0);
    assert_eq!(replay["projection_consistent"], true);
    assert_eq!(
        replay["events"].as_array().unwrap().len(),
        report["events"].as_array().unwrap().len()
    );
    let policy = parent.path().join("policy.json");
    std::fs::write(
        &policy,
        serde_json::to_vec(&json!({
            "handler":"journal", "version":1, "event_kinds":["run_created"],
            "max_attempts":3, "lease_ms":10000, "backoff_ms":10,
            "max_backoff_ms":100, "max_chain_depth":2
        }))
        .unwrap(),
    )
    .unwrap();
    let args = [
        "outbox",
        "dispatch",
        "--policy",
        policy.to_str().unwrap(),
        "--limit",
        "1",
    ];
    assert_eq!(cli(Some(&repo), &args)["delivered"], 1);
    assert_eq!(cli(Some(&repo), &args)["claimed"], 0);
    let deliveries = cli(Some(&repo), &["outbox", "list", "journal"]);
    assert_eq!(deliveries.as_array().unwrap().len(), 1);
    assert_eq!(deliveries[0]["state"], "delivered");
    assert_eq!(
        cli(Some(&repo), &["replay", id])["projection_consistent"],
        true
    );
}

#[test]
fn replay_does_not_initialize_a_missing_repository_store() {
    let root = tempfile::tempdir().unwrap();
    let result = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(root.path())
        .status()
        .unwrap();
    assert!(result.success());
    let output = Command::new(env!("CARGO_BIN_EXE_harness"))
        .arg("--repo")
        .arg(root.path())
        .args(["replay", "unknown"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.path().join(".git/harness").exists());
}
