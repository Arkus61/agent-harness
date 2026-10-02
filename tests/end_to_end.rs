//! Public CLI acceptance tests. Every review is a declared fixture; publication must reject it.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cli(repo: Option<&Path>, args: &[&str], failpoint: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_harness"));
    command.env_remove("HARNESS_FAILPOINT");
    if let Some(repo) = repo {
        command.arg("--repo").arg(repo);
    }
    if let Some(failpoint) = failpoint {
        command.env("HARNESS_FAILPOINT", failpoint);
    }
    command.args(args).output().expect("launch harness CLI")
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid CLI JSON: {error}; status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn successful(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "status={}; stdout={}; stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    json_output(output)
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn demo(parent: &Path, failpoint: Option<&str>) -> (PathBuf, Output) {
    let repo = parent.join("пример с пробелами ✓");
    let output = cli(None, &["demo", "--dir", repo.to_str().unwrap()], failpoint);
    (repo, output)
}

fn assert_fixture_verified(report: &Value) {
    assert_eq!(report["run"]["state"], "FIXTURE_VERIFIED", "{report:#}");
    let decisions: Vec<_> = report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "decision.assessment")
        .collect();
    assert!(!decisions.is_empty());
    let mut request_ids = std::collections::BTreeSet::new();
    for event in decisions {
        let payload = &event["payload"];
        let request: agent_harness::decision::DecisionRequest =
            serde_json::from_value(payload["request"].clone()).unwrap();
        request.validate().unwrap();
        assert!(request_ids.insert(request.request_id.clone()));
        assert_eq!(payload["input_hash"], request.subject_hash);
        assert_eq!(payload["assessment"]["subject_hash"], request.subject_hash);
        assert_eq!(payload["fixture"], true);
    }
    let reviews = report["reviews"].as_array().unwrap();
    assert_eq!(reviews.len(), 4);
    let roles: BTreeMap<_, _> = reviews
        .iter()
        .map(|review| (review["role"].as_str().unwrap(), review))
        .collect();
    assert_eq!(roles.len(), 4);
    for role in ["requirements", "code", "tests", "security"] {
        let review = roles[role];
        assert_eq!(review["verdict"], "PASS");
        assert_eq!(review["fixture"], true);
        assert!(review["context_hash"]
            .as_str()
            .is_some_and(|h| !h.is_empty()));
    }
    let checks = report["checks"].as_array().unwrap();
    assert!(!checks.is_empty());
    for check in checks {
        assert_eq!(check["verdict"], "PASS");
        assert_eq!(check["receipt"]["exit_code"], 0);
        assert_eq!(check["receipt"]["timed_out"], false);
        assert_eq!(check["receipt"]["cancelled"], false);
        for review in reviews {
            assert_eq!(review["candidate_hash"], check["candidate_hash"]);
        }
    }
}

#[test]
fn demo_has_independent_fixture_reviews_preserves_head_and_cannot_publish() {
    let parent = tempfile::tempdir().unwrap();
    let (repo, output) = demo(parent.path(), None);
    let report = successful(&output);
    assert_fixture_verified(&report);
    let run_id = report["run"]["id"].as_str().unwrap();
    let base = report["run"]["base_sha"].as_str().unwrap();
    let candidate = report["run"]["candidate_sha"].as_str().unwrap();
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base);
    assert_ne!(candidate, base);
    let original = std::fs::read_to_string(repo.join("src/lib.rs")).unwrap();
    assert!(original.contains("value.min(low).max(high)"));
    let candidate_file = git(&repo, &["show", &format!("{candidate}:src/lib.rs")]);
    assert!(candidate_file.contains("value.max(low).min(high)"));

    let status = successful(&cli(Some(&repo), &["status", run_id], None));
    assert_eq!(status["state"], "FIXTURE_VERIFIED");
    let inspected = successful(&cli(Some(&repo), &["inspect", run_id], None));
    assert_eq!(inspected["run"]["candidate_sha"], candidate);
    let output_path = parent.path().join("отчёт.json");
    let write_report = cli(
        Some(&repo),
        &["report", run_id, "--output", output_path.to_str().unwrap()],
        None,
    );
    assert!(write_report.status.success());
    let saved: Value = serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(saved["run"]["id"], run_id);

    let rejected = cli(
        Some(&repo),
        &[
            "merge",
            run_id,
            "--target",
            "refs/heads/fixture-target",
            "--expected",
            base,
        ],
        None,
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("production VERIFIED"));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base);
}

#[test]
fn cli_eval_counts_only_executed_assertions() {
    let report = successful(&cli(None, &["eval"], None));
    assert_eq!(report["verification_mode"], "fixture");
    assert_eq!(report["passed"], true);
    let criteria = report["criteria"].as_array().unwrap();
    assert_eq!(report["summary"]["criteria"], criteria.len());
    assert_eq!(report["summary"]["passed"], criteria.len());
    assert_eq!(report["summary"]["failed"], 0);
    assert!(criteria
        .iter()
        .all(|criterion| criterion["verdict"] == "PASS"));
    let ids: std::collections::BTreeSet<_> = criteria
        .iter()
        .map(|criterion| criterion["criterion_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), criteria.len(), "criteria must have unique IDs");
}

#[test]
fn isolated_profile_uses_probed_backend_or_fails_closed_before_actions() {
    let parent = tempfile::tempdir().unwrap();
    let (repo, output) = demo(parent.path(), None);
    successful(&output);
    let path = repo.join("task.json");
    let mut task: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    task["profile"] = json!("isolated");
    std::fs::write(&path, serde_json::to_vec_pretty(&task).unwrap()).unwrap();
    let output = cli(
        Some(&repo),
        &["run", "--task", path.to_str().unwrap()],
        None,
    );
    let report = json_output(&output);
    if agent_harness::isolation::availability().available {
        assert!(output.status.success(), "{report:#}");
        assert_fixture_verified(&report);
        for event in report["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "decision.assessment")
        {
            assert_eq!(
                event["payload"]["request"]["effective_scope"]["runtime_profile"],
                "isolated"
            );
        }
        return;
    }
    assert!(!output.status.success());
    assert_eq!(report["run"]["state"], "BLOCKED");
    assert!(report["attempts"].as_array().unwrap().is_empty());
    assert!(report["events"].as_array().unwrap().iter().all(|event| {
        !matches!(
            event["kind"].as_str(),
            Some("action_intent" | "model.intent")
        )
    }));
    assert!(report["run"]["error"]
        .as_str()
        .unwrap()
        .contains("isolated"));
}

#[test]
fn durable_pending_intent_requires_reconciliation_before_resume() {
    let parent = tempfile::tempdir().unwrap();
    let (repo, output) = demo(parent.path(), Some("after_intent"));
    assert!(!output.status.success());
    let blocked = json_output(&output);
    assert_eq!(blocked["run"]["state"], "BLOCKED");
    let run_id = blocked["run"]["id"].as_str().unwrap();
    let base = blocked["run"]["base_sha"].as_str().unwrap();
    let spent_before = blocked["run"]["spent_tokens"].as_u64().unwrap();
    assert!(spent_before > 0);
    assert_eq!(blocked["run"]["reserved_tokens"], 0);
    let intents: Vec<_> = blocked["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "action_intent")
        .collect();
    assert_eq!(intents.len(), 1);
    let action = intents[0]["payload"]["action_id"].as_str().unwrap();
    assert!(blocked["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["kind"] != "action_receipt"));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base);
    let unsafe_resume = cli(Some(&repo), &["resume", run_id], None);
    assert!(!unsafe_resume.status.success());
    assert!(String::from_utf8_lossy(&unsafe_resume.stderr).contains("NEEDS_RECONCILIATION"));
    let inspected = successful(&cli(Some(&repo), &["inspect", run_id], None));
    assert_eq!(inspected["run"]["generation"], 1);
    assert_eq!(inspected["run"]["spent_tokens"], spent_before);
    successful(&cli(
        Some(&repo),
        &[
            "reconcile",
            run_id,
            "--action",
            action,
            "--status",
            "not_executed",
            "--evidence",
            "Named failpoint stopped dispatch before mutation",
        ],
        None,
    ));
    let resumed = successful(&cli(Some(&repo), &["resume", run_id], None));
    assert_fixture_verified(&resumed);
    assert_eq!(resumed["run"]["generation"], 2);
    assert!(resumed["run"]["spent_tokens"].as_u64().unwrap() > spent_before);
    assert_eq!(resumed["run"]["task"]["budget"]["max_tokens"], 100_000);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), base);
}

#[test]
fn dag_integrates_two_independent_builders_before_dependent_builder() {
    let parent = tempfile::tempdir().unwrap();
    let (repo, output) = demo(parent.path(), None);
    successful(&output);
    let path = repo.join("task.json");
    let mut task: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    task["prompt"] = json!("Implement independent left/right values and their combined sum");
    task["requirements"] = json!([
        "left() returns one",
        "right() returns two",
        "combined() returns three"
    ]);
    task["nodes"] = json!([
        {"id":"left","prompt":"Add src/left.rs","requirements":[0],"depends_on":[],"owned_paths":["src/left.rs"]},
        {"id":"right","prompt":"Add src/right.rs","requirements":[1],"depends_on":[],"owned_paths":["src/right.rs"]},
        {"id":"combined","prompt":"Combine both modules in src/lib.rs","requirements":[2],"depends_on":["left","right"],"owned_paths":["src/lib.rs"]}
    ]);
    let helper = env!("CARGO_BIN_EXE_harness-test-helper");
    task["grants"]["commands"] = json!([{"program":helper,"args_prefix":["sleep","1500"]}]);
    task["provider"]["scripts"]["decision:tools"][0]["decision"]["tools"] = json!([
        "read_file",
        "search",
        "write_file",
        "edit_file",
        "run_command"
    ]);
    let pause = json!({"type":"run_command","program":helper,"args":["sleep","1500"]});
    let original = std::fs::read(repo.join("src/lib.rs")).unwrap();
    let scripts = task["provider"]["scripts"].as_object_mut().unwrap();
    scripts.remove("builder:fix");
    scripts.insert("builder:left".into(), json!([{"actions":[pause.clone(),{"type":"write_file","path":"src/left.rs","content":"pub fn left() -> i32 { 1 }\n"}],"done":true}]));
    scripts.insert("builder:right".into(), json!([{"actions":[pause,{"type":"write_file","path":"src/right.rs","content":"pub fn right() -> i32 { 2 }\n"}],"done":true}]));
    scripts.insert("builder:combined".into(), json!([{"actions":[{"type":"write_file","path":"src/lib.rs","expected_hash":blake3::hash(&original).to_hex().to_string(),"content":"mod left; mod right;\npub fn combined() -> i32 { left::left() + right::right() }\n#[cfg(test)] mod tests { #[test] fn total() { assert_eq!(super::combined(),3); } }\n"}],"done":true}]));
    for purpose in ["model", "tools", "risk"] {
        let key = format!("decision:{purpose}");
        let reply = scripts[&key][0].clone();
        let count = if purpose == "risk" { 5 } else { 3 };
        scripts.insert(key, json!(vec![reply; count]));
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&task).unwrap()).unwrap();
    let report = successful(&cli(
        Some(&repo),
        &["run", "--task", path.to_str().unwrap()],
        None,
    ));
    assert_fixture_verified(&report);
    let attempts: BTreeMap<_, _> = report["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attempt| (attempt["node_id"].as_str().unwrap(), attempt))
        .collect();
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        attempts["left"]["input_sha"],
        attempts["right"]["input_sha"]
    );
    let combined_input = attempts["combined"]["input_sha"].as_str().unwrap();
    assert!(git(&repo, &["show", &format!("{combined_input}:src/left.rs")]).contains("{ 1 }"));
    assert!(git(&repo, &["show", &format!("{combined_input}:src/right.rs")]).contains("{ 2 }"));
    let candidate = report["run"]["candidate_sha"].as_str().unwrap();
    assert!(git(&repo, &["show", &format!("{candidate}:src/lib.rs")]).contains("pub fn combined"));
    assert_eq!(
        git(&repo, &["rev-parse", "HEAD"]),
        report["run"]["base_sha"].as_str().unwrap()
    );
    let command_hash = agent_harness::types::hash(&agent_harness::types::Action::RunCommand {
        program: helper.into(),
        args: vec!["sleep".into(), "1500".into()],
    })
    .unwrap();
    let events = report["events"].as_array().unwrap();
    let command_intents: Vec<_> = events
        .iter()
        .filter(|event| {
            event["kind"] == "action_intent" && event["payload"]["action_hash"] == command_hash
        })
        .collect();
    assert_eq!(command_intents.len(), 2);
    let last_dispatch = command_intents
        .iter()
        .map(|event| event["seq"].as_i64().unwrap())
        .max()
        .unwrap();
    let ids: Vec<_> = command_intents
        .iter()
        .map(|event| event["payload"]["action_id"].as_str().unwrap())
        .collect();
    let first_completion = events
        .iter()
        .filter(|event| {
            event["kind"] == "action_receipt"
                && ids.contains(&event["payload"]["action_id"].as_str().unwrap_or(""))
        })
        .map(|event| event["seq"].as_i64().unwrap())
        .min()
        .unwrap();
    assert!(
        last_dispatch < first_completion,
        "independent commands must overlap before either completes"
    );
}

#[tokio::test]
async fn aborting_runtime_future_stops_descendant_processes() {
    let parent = tempfile::tempdir().unwrap();
    let heartbeat = parent.path().join("heartbeat.txt");
    let spec = agent_harness::types::CommandSpec {
        program: env!("CARGO_BIN_EXE_harness-test-helper").into(),
        args: vec!["spawn".into(), heartbeat.to_str().unwrap().into()],
        timeout_secs: 30,
        resource_limits: None,
    };
    let cwd = parent.path().to_path_buf();
    let command = tokio::spawn(async move {
        agent_harness::execution::run_command(
            &cwd,
            &spec,
            tokio_util::sync::CancellationToken::new(),
            4096,
        )
        .await
    });
    for _ in 0..200 {
        if std::fs::metadata(&heartbeat).is_ok_and(|file| file.len() > 0) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(std::fs::metadata(&heartbeat).unwrap().len() > 0);
    command.abort();
    assert!(command.await.unwrap_err().is_cancelled());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let before = std::fs::metadata(&heartbeat).unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(std::fs::metadata(&heartbeat).unwrap().len(), before);
}

#[cfg(unix)]
#[test]
fn sigkill_coordinator_preserves_command_intent_and_resume_holds() {
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    // The trusted supervisor survives coordinator loss and stops ordinary
    // descendants. Native execution still is not hostile-process confinement.
    struct RunningCommand {
        coordinator: Child,
        heartbeat: PathBuf,
    }
    impl Drop for RunningCommand {
        fn drop(&mut self) {
            if let Ok(text) = std::fs::read_to_string(&self.heartbeat) {
                if let Some(pid) = text
                    .lines()
                    .next()
                    .and_then(|line| line.parse::<i32>().ok())
                {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            let _ = self.coordinator.kill();
            let _ = self.coordinator.wait();
        }
    }

    let parent = tempfile::tempdir().unwrap();
    let (repo, output) = demo(parent.path(), None);
    successful(&output);
    let task_path = repo.join("task.json");
    let mut task: Value = serde_json::from_slice(&std::fs::read(&task_path).unwrap()).unwrap();
    let heartbeat = parent.path().join("command-heartbeat.txt");
    let helper = env!("CARGO_BIN_EXE_harness-test-helper");
    task["prompt"] = json!("Exercise durable intent while a long fixture command is running");
    task["requirements"] = json!(["The command is journaled before dispatch"]);
    task["nodes"][0]["requirements"] = json!([0]);
    task["grants"]["commands"] =
        json!([{"program":helper,"args_prefix":["heartbeat",heartbeat.to_str().unwrap()]}]);
    task["provider"]["scripts"]["decision:tools"][0]["decision"]["tools"] = json!([
        "read_file",
        "search",
        "write_file",
        "edit_file",
        "run_command"
    ]);
    task["provider"]["scripts"]["builder:fix"] = json!([{
        "actions":[{"type":"run_command","program":helper,"args":["heartbeat",heartbeat.to_str().unwrap(),"25"]}],
        "done":true
    }]);
    std::fs::write(&task_path, serde_json::to_vec_pretty(&task).unwrap()).unwrap();
    let coordinator = Command::new(env!("CARGO_BIN_EXE_harness"))
        .arg("--repo")
        .arg(&repo)
        .args(["run", "--task"])
        .arg(&task_path)
        .env_remove("HARNESS_FAILPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = RunningCommand {
        coordinator,
        heartbeat: heartbeat.clone(),
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::fs::metadata(&heartbeat).is_ok_and(|file| file.len() > 0) {
        assert!(
            Instant::now() < deadline,
            "fixture command was not dispatched"
        );
        assert!(
            running.coordinator.try_wait().unwrap().is_none(),
            "coordinator exited before command dispatch"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    running.coordinator.kill().unwrap();
    let killed = running.coordinator.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(killed.signal(), Some(libc::SIGKILL));
    std::thread::sleep(Duration::from_millis(250));
    let before = std::fs::metadata(&heartbeat).unwrap().len();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(std::fs::metadata(&heartbeat).unwrap().len(), before);
    let all_runs = successful(&cli(Some(&repo), &["status"], None));
    let recovered = all_runs
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["state"] == "EXECUTING")
        .expect("interrupted run remains durable");
    let run_id = recovered["id"].as_str().unwrap();
    let inspected = successful(&cli(Some(&repo), &["inspect", run_id], None));
    let intents = inspected["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "action_intent")
        .count();
    assert_eq!(intents, 1);
    assert!(inspected["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["kind"] != "action_receipt"));
    let held = cli(Some(&repo), &["resume", run_id], None);
    assert!(!held.status.success());
    assert!(String::from_utf8_lossy(&held.stderr).contains("NEEDS_RECONCILIATION"));
    let after = successful(&cli(Some(&repo), &["inspect", run_id], None));
    assert_eq!(after["run"]["generation"], 1);
    assert_eq!(after["run"]["spent_tokens"], recovered["spent_tokens"]);
    assert_eq!(
        after["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"] == "action_intent")
            .count(),
        1
    );
}
