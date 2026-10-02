//! Public CLI scenario corpus. All network responses are local protocol fixtures;
//! none are evidence of live model quality or execution on another OS.
use agent_harness::decision::{DecisionRequest, DecisionScope, DecisionSubject};
use agent_harness::types::{Action, DecisionAssessment, ModelReply, TaskSpec};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

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
    String::from_utf8(output.stdout).unwrap().trim().into()
}

struct Fixture {
    directory: tempfile::TempDir,
    repo: PathBuf,
    baseline: String,
    files: Value,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repository");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir(repo.join("restricted")).unwrap();
        std::fs::write(repo.join("src/owned.txt"), "original source\n").unwrap();
        std::fs::write(repo.join("src/other.txt"), "other source\n").unwrap();
        std::fs::write(
            repo.join("restricted/private.txt"),
            "SYNTHETIC_DENIED_READ\n",
        )
        .unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.name", "Harness fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "baseline"]);
        let baseline = git(&repo, &["rev-parse", "HEAD"]);
        let files = Self::snapshot(&repo);
        Self {
            directory,
            repo,
            baseline,
            files,
        }
    }
    fn snapshot(repo: &Path) -> Value {
        let mut result = serde_json::Map::new();
        for path in ["src/owned.txt", "src/other.txt", "restricted/private.txt"] {
            result.insert(
                path.into(),
                json!(blake3::hash(&std::fs::read(repo.join(path)).unwrap())
                    .to_hex()
                    .to_string()),
            );
        }
        Value::Object(result)
    }
    fn task(&self, endpoint: &str) -> Value {
        json!({
            "prompt":"Preserve the original fixture source", "requirements":["Original source remains readable"],
            "grants":{"read":["src/**"],"write":["src/**"],"commands":[]},
            "checks":[{"program":env!("CARGO_BIN_EXE_harness-test-helper"),"args":["echo","ACTUAL_TRUSTED_CHECK"],"timeout_secs":3}],
            "provider":{"kind":"open_ai","model":"local-protocol-fixture","base_url":endpoint,"api_key_env":""},
            "nodes":[{"id":"worker","prompt":"Preserve sources","requirements":[0],"owned_paths":["src/owned.txt"]}],
            "decision_mode":"enforced", "max_repairs":0,"max_steps":3,"concurrency":1,
            "budget":{"max_tokens":100000,"max_output_tokens":2048,"deadline_secs":20}
        })
    }
    fn run(&self, task: &Value) -> Output {
        let path = self.directory.path().join("task.json");
        std::fs::write(&path, serde_json::to_vec_pretty(task).unwrap()).unwrap();
        Command::new(env!("CARGO_BIN_EXE_harness"))
            .arg("--repo")
            .arg(&self.repo)
            .args(["run", "--task"])
            .arg(path)
            .env_remove("HARNESS_FAILPOINT")
            .output()
            .unwrap()
    }
    fn preserved(&self) {
        assert_eq!(git(&self.repo, &["rev-parse", "HEAD"]), self.baseline);
        assert_eq!(Self::snapshot(&self.repo), self.files);
        assert!(git(
            &self.repo,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .is_empty());
    }
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn events(report: &Value, kind: &str) -> Vec<Value> {
    report["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .cloned()
        .collect()
}
fn record(name: &str, evidence: Value) {
    if let Some(root) = std::env::var_os("HARNESS_SCENARIO_ARTIFACT_DIR") {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
}

struct Server {
    endpoint: String,
    stopped: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(mut response: impl FnMut(&Value) -> ModelReply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = stopped.clone();
        let captures = requests.clone();
        let join = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !stop.load(Ordering::SeqCst) {
                assert!(
                    Instant::now() < deadline,
                    "fixture server deadline exceeded"
                );
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets inherit the listener's nonblocking
                        // mode on macOS. Read the fixture request in blocking
                        // mode on every platform, with the bounds below.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        // Concurrent reviewer transports can be cancelled as
                        // soon as one deterministic policy denial wins. A TCP
                        // connection without a complete request is not a call.
                        let Some(body) = request(&mut stream) else {
                            continue;
                        };
                        captures.lock().unwrap().push(body.clone());
                        let reply = response(&body);
                        let payload = serde_json::to_vec(&json!({"choices":[{"message":{"content":serde_json::to_string(&reply).unwrap()},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":10}})).unwrap();
                        let sent=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",payload.len()).and_then(|_|stream.write_all(&payload));
                        if let Err(error) = sent {
                            assert!(
                                matches!(
                                    error.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionReset
                                ),
                                "fixture response failed: {error}"
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self {
            endpoint,
            stopped,
            requests,
            join: Some(join),
        }
    }
    fn captured(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.join.take().unwrap().join().unwrap();
    }
}
fn request(stream: &mut TcpStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).unwrap();
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 256 * 1024, "unbounded HTTP fixture request");
        if let Some(position) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..position]).unwrap();
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= position + 4 + length {
                return Some(
                    serde_json::from_slice(&bytes[position + 4..position + 4 + length]).unwrap(),
                );
            }
        }
    }
}
fn user(body: &Value) -> &str {
    body["messages"][1]["content"].as_str().unwrap()
}
fn approval(request: &DecisionRequest) -> ModelReply {
    request.validate().unwrap();
    let (choice, tools) = match &request.subject {
        DecisionSubject::ModelSelection {
            requested_model, ..
        } => (Some(requested_model.clone()), vec![]),
        DecisionSubject::ToolConfiguration { enabled_tools } => (None, enabled_tools.clone()),
        DecisionSubject::ActionRisk { .. } => (None, vec![]),
    };
    ModelReply {done:true,decision:Some(DecisionAssessment {purpose:request.purpose.as_str().into(),subject_hash:request.subject_hash.clone(),allow:true,abstain:false,reason:"Exact local fixture subject receives unanimous advisory approval; gateway remains authoritative".into(),choice,tools}),..Default::default()}
}
fn pass_review() -> ModelReply {
    serde_json::from_value(json!({"done":true,"verdict":"PASS","summary":"Local source evidence","proofs":[{"requirement":0,"path":"src/owned.txt","line":1,"explanation":"Exact fixture source exists"}]})).unwrap()
}
fn positive(body: &Value) -> ModelReply {
    if let Ok(request) = serde_json::from_str::<DecisionRequest>(user(body)) {
        return approval(&request);
    }
    if user(body).starts_with("Role:") {
        pass_review()
    } else {
        ModelReply {
            done: true,
            ..Default::default()
        }
    }
}

#[test]
fn s02_invalid_cli_contract_corpus_has_specific_diagnostics_and_zero_dispatch() {
    let mut cases = Vec::new();
    let fixture = Fixture::new();
    let server = Server::new(|_| panic!("invalid TaskSpec dispatched a model call"));
    let baseline = fixture.task(&server.endpoint);
    for (name, path, replacement, diagnostic) in [
        ("empty_prompt", "prompt", json!(" "), "prompt"),
        (
            "empty_requirements",
            "requirements",
            json!([]),
            "requirements",
        ),
        ("empty_checks", "checks", json!([]), "check"),
        (
            "unsupported_version",
            "schema_version",
            json!(99),
            "schema_version",
        ),
        (
            "invalid_concurrency",
            "concurrency",
            json!(0),
            "concurrency",
        ),
        ("invalid_steps", "max_steps", json!(0), "max_steps"),
        (
            "unknown_field",
            "unknown_permission",
            json!(true),
            "unknown_permission",
        ),
        (
            "wrong_type",
            "max_steps",
            json!("SYNTHETIC_INVALID_TYPE_CANARY"),
            "invalid field type",
        ),
        (
            "unknown_profile",
            "profile",
            json!("SYNTHETIC_INVALID_ENUM_CANARY"),
            "unknown enum variant",
        ),
    ] {
        let mut task = baseline.clone();
        task[path] = replacement;
        let output = fixture.run(&task);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let evidence = json!({"criteria":["S02.C01","S02.C02"],"task":task,"status":output.status.code(),"stderr":stderr,"stdout":String::from_utf8_lossy(&output.stdout),"requests":server.captured().len(),"head":fixture.baseline,"files":fixture.files});
        record(&format!("S02-{name}"), evidence.clone());
        cases.push(evidence);
        assert!(!output.status.success(), "accepted invalid case {name}");
        assert!(
            server.captured().is_empty(),
            "invalid contract {name} reached model"
        );
        fixture.preserved();
        assert!(
            !fixture.repo.join(".git/harness").exists(),
            "invalid parser contract must not initialize harness state"
        );
        assert!(!stderr.contains("SYNTHETIC_INVALID_TYPE_CANARY"));
        assert!(!stderr.contains("SYNTHETIC_INVALID_ENUM_CANARY"));
        assert!(
            stderr.contains(diagnostic),
            "{name} lacks specific field {diagnostic}: {stderr}"
        );
    }
    for (name, key, value, diagnostic) in [
        ("zero_tokens", "max_tokens", 0, "budget"),
        ("zero_output", "max_output_tokens", 0, "budget"),
        ("zero_deadline", "deadline_secs", 0, "budget"),
        (
            "output_over_root",
            "max_output_tokens",
            100001,
            "max_output_tokens",
        ),
    ] {
        let mut task = baseline.clone();
        task["budget"][key] = json!(value);
        let output = fixture.run(&task);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        record(
            &format!("S02-{name}"),
            json!({"criteria":["S02.C01","S02.C02"],"task":task,"status":output.status.code(),"stderr":stderr,"requests":server.captured().len()}),
        );
        assert!(!output.status.success());
        assert!(stderr.contains(diagnostic), "{name}: {stderr}");
        assert!(server.captured().is_empty());
        fixture.preserved();
    }
    for (name, task, diagnostic) in [
        (
            "missing_requirements",
            {
                let mut task = baseline.clone();
                task.as_object_mut().unwrap().remove("requirements");
                task
            },
            "missing field `requirements`",
        ),
        (
            "unknown_nested_budget",
            {
                let mut task = baseline.clone();
                task["budget"]["unknown_limit"] = json!(true);
                task
            },
            "unknown field `unknown_limit`",
        ),
    ] {
        let output = fixture.run(&task);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        record(
            &format!("S02-{name}"),
            json!({"criteria":["S02.C01","S02.C02"],"task":task,"status":output.status.code(),"stderr":stderr,"requests":server.captured().len()}),
        );
        assert!(!output.status.success());
        assert!(stderr.contains(diagnostic), "{name}: {stderr}");
        assert!(server.captured().is_empty());
        fixture.preserved();
        assert!(!fixture.repo.join(".git/harness").exists());
    }
    record(
        "S02-corpus-summary",
        json!({"passed":true,"criteria":["S02.C01","S02.C02"],"schema_cases":cases.len()+2,"budget_cases":4,"model_calls":0}),
    );
}

#[test]
fn s02_valid_cli_control_is_accepted_with_actual_checks_and_source_receipts() {
    let fixture = Fixture::new();
    let server = Server::new(positive);
    let output = fixture.run(&fixture.task(&server.endpoint));
    let result = report(&output);
    record(
        "S02-positive-control",
        json!({"criteria":["S02.C03"],"report":result,"model_calls":server.captured().len()}),
    );
    assert!(output.status.success(), "{result:#}");
    assert_eq!(result["run"]["state"], "VERIFIED");
    assert_eq!(
        result["checks"][0]["receipt"]["stdout"],
        "ACTUAL_TRUSTED_CHECK\n"
    );
    assert_eq!(result["reviews"].as_array().unwrap().len(), 4);
    fixture.preserved();
}

#[test]
fn s22_explicit_invalid_dags_block_before_builder_and_distinguish_causes() {
    let node = json!({"id":"a","prompt":"Preserve source","requirements":[0],"owned_paths":["src/owned.txt"]});
    let mut cycle_a = node.clone();
    cycle_a["depends_on"] = json!(["b"]);
    let mut cycle_b = node.clone();
    cycle_b["id"] = json!("b");
    cycle_b["depends_on"] = json!(["a"]);
    let mut missing = node.clone();
    missing["depends_on"] = json!(["missing"]);
    let mut uncovered = node.clone();
    uncovered["requirements"] = json!([]);
    let mut parallel = node.clone();
    parallel["id"] = json!("b");
    let cases = [
        ("cycle", json!([cycle_a, cycle_b]), "cycle"),
        ("missing_dependency", json!([missing]), "missing dependency"),
        (
            "uncovered_requirement",
            json!([uncovered]),
            "cover all requirements",
        ),
        (
            "parallel_overlap",
            json!([node.clone(), parallel]),
            "parallel ownership overlap",
        ),
        (
            "duplicate_id",
            json!([node.clone(), node]),
            "duplicate node id",
        ),
    ];
    let mut errors = std::collections::BTreeMap::new();
    for (name, nodes, diagnostic) in cases {
        let fixture = Fixture::new();
        let server = Server::new(|_| panic!("invalid explicit DAG reached model"));
        let mut task = fixture.task(&server.endpoint);
        task["nodes"] = nodes;
        let output = fixture.run(&task);
        let result = report(&output);
        record(
            &format!("S22-{name}"),
            json!({"criteria":["S22.C01","S22.C02"],"report":result,"requests":server.captured().len()}),
        );
        assert!(!output.status.success());
        assert_eq!(result["run"]["state"], "BLOCKED");
        assert!(result["attempts"].as_array().unwrap().is_empty());
        assert!(events(&result, "model.intent").is_empty());
        assert!(events(&result, "action_intent").is_empty());
        assert!(server.captured().is_empty());
        fixture.preserved();
        let error = result["run"]["error"].as_str().unwrap().to_owned();
        assert!(error.contains(diagnostic), "{name}: {error}");
        errors.insert(name, error);
    }
    record(
        "S22-errors",
        json!({"criteria":["S22.C01","S22.C02"],"errors":errors}),
    );
    assert_ne!(
        errors["cycle"], errors["missing_dependency"],
        "cycle and nonexistent dependency need distinct diagnostics"
    );
}

#[test]
fn s22_invalid_model_planner_gets_a_distinct_safe_fallback_revision() {
    let fixture = Fixture::new();
    let invalid = vec![
        json!({"id":"a","prompt":"unsafe cyclic edge a","requirements":[0],"depends_on":["b"],"owned_paths":["src/**"]}),
        json!({"id":"b","prompt":"unsafe cyclic edge b","requirements":[0],"depends_on":["a"],"owned_paths":["src/**"]}),
    ];
    let original = invalid.clone();
    let server = Server::new(move |body| {
        if user(body).starts_with("{\"requirements\"")
            || body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Produce a DAG covering all requirements")
        {
            return serde_json::from_value(json!({"done":true,"plan":original})).unwrap();
        }
        positive(body)
    });
    let mut task = fixture.task(&server.endpoint);
    task["nodes"] = json!([]);
    let output = fixture.run(&task);
    let result = report(&output);
    record(
        "S22-planner-fallback",
        json!({"criteria":["S22.C01","S22.C02","S22.C03"],"task":task,"original_invalid_plan":invalid,"report":result,"model_requests":server.captured()}),
    );
    assert!(output.status.success(),"invalid model plan must safely fall back without changing the trusted TaskSpec: {result:#}");
    assert_eq!(result["run"]["state"], "VERIFIED");
    let rejected = events(&result, "plan.rejected");
    let accepted = events(&result, "plan.accepted");
    assert_eq!(rejected.len(), 1);
    assert_eq!(accepted.len(), 1);
    assert!(rejected[0]["payload"]["error"]
        .as_str()
        .unwrap()
        .contains("cycle"));
    assert_ne!(
        rejected[0]["payload"]["plan_hash"],
        accepted[0]["payload"]["plan_hash"]
    );
    assert_eq!(rejected[0]["payload"]["revision"], 0);
    assert_eq!(accepted[0]["payload"]["revision"], 1);
    assert_eq!(
        accepted[0]["payload"]["supersedes_plan_hash"],
        rejected[0]["payload"]["plan_hash"]
    );
    let nodes: Vec<agent_harness::types::TaskNode> =
        serde_json::from_value(accepted[0]["payload"]["nodes"].clone()).unwrap();
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].depends_on.is_empty());
    assert_eq!(nodes[0].requirements, vec![0]);
    assert_eq!(nodes[0].owned_paths, vec!["src/**"]);
    let spec: TaskSpec = serde_json::from_value(task).unwrap();
    agent_harness::engine::validate_plan(&nodes, &spec).unwrap();
    assert_eq!(
        result["run"]["task"]["nodes"],
        json!([]),
        "caller contract remains unchanged"
    );
    assert_eq!(
        result["run"]["task"]["grants"],
        serde_json::to_value(spec.grants).unwrap()
    );
    assert!(rejected[0]["seq"].as_u64().unwrap() < accepted[0]["seq"].as_u64().unwrap());
    assert!(
        accepted[0]["seq"].as_u64().unwrap()
            < events(&result, "decision.assessment")[0]["seq"]
                .as_u64()
                .unwrap()
    );
    assert_eq!(result["attempts"].as_array().unwrap().len(), 1);
    fixture.preserved();
}

#[test]
fn s22_serial_overlap_control_is_accepted_after_dependency_validation() {
    let fixture = Fixture::new();
    let server = Server::new(positive);
    let mut task = fixture.task(&server.endpoint);
    task["nodes"] = json!([
        {"id":"first","prompt":"Preserve source","requirements":[0],"owned_paths":["src/owned.txt"]},
        {"id":"second","prompt":"Observe first candidate","requirements":[0],"depends_on":["first"],"owned_paths":["src/owned.txt"]}
    ]);
    let output = fixture.run(&task);
    let result = report(&output);
    record(
        "S22-serial-overlap-control",
        json!({"criteria":["S22.C01","S22.C02"],"task":task,"report":result}),
    );
    assert!(output.status.success(), "{result:#}");
    assert_eq!(result["attempts"].as_array().unwrap().len(), 2);
    fixture.preserved();
}

#[test]
fn s03_cli_gateway_denial_corpus_prevents_effects_despite_typed_model_approval() {
    for name in [
        "unknown_command",
        "forbidden_argv",
        "unowned_write",
        "ungranted_write",
        "ungranted_read",
        "reviewer_write",
        "reviewer_command",
    ] {
        let fixture = Fixture::new();
        let marker = fixture.directory.path().join("subprocess-trap.txt");
        let helper = env!("CARGO_BIN_EXE_harness-test-helper");
        let proposal = match name {
            "unknown_command" | "forbidden_argv" | "reviewer_command" => Action::RunCommand {
                program: helper.into(),
                args: vec!["resource-marker".into(), marker.to_string_lossy().into()],
            },
            "unowned_write" => Action::WriteFile {
                path: "src/other.txt".into(),
                content: "FORBIDDEN_EFFECT".into(),
                expected_hash: Some(blake3::hash(b"other source\n").to_hex().to_string()),
            },
            "ungranted_write" => Action::WriteFile {
                path: "restricted/private.txt".into(),
                content: "FORBIDDEN_EFFECT".into(),
                expected_hash: Some(
                    blake3::hash(b"SYNTHETIC_DENIED_READ\n")
                        .to_hex()
                        .to_string(),
                ),
            },
            "ungranted_read" => Action::ReadFile {
                path: "restricted/private.txt".into(),
            },
            "reviewer_write" => Action::WriteFile {
                path: "src/owned.txt".into(),
                content: "FORBIDDEN_EFFECT".into(),
                expected_hash: Some(blake3::hash(b"original source\n").to_hex().to_string()),
            },
            _ => unreachable!(),
        };
        let proposed = proposal.clone();
        let reviewer = name.starts_with("reviewer_");
        let server = Server::new(move |body| {
            if let Ok(request) = serde_json::from_str::<DecisionRequest>(user(body)) {
                assert!(
                    !matches!(request.subject, DecisionSubject::ActionRisk { .. }),
                    "programmatically forbidden action must not reach the advisory risk evaluator"
                );
                return approval(&request);
            }
            if user(body).starts_with("Role:") {
                let mut reply = pass_review();
                if reviewer && user(body).starts_with("Role: security\n") {
                    reply.actions = vec![proposed.clone()];
                }
                reply
            } else {
                ModelReply {done:true,actions:if reviewer {vec![]}else{vec![proposed.clone()]},summary:"Two unanimous advisory approvals and high confidence do not grant this proposal".into(),..Default::default()}
            }
        });
        let mut task = fixture.task(&server.endpoint);
        if name == "forbidden_argv" {
            task["grants"]["commands"] = json!([{"program":helper,"args_prefix":["echo"]}]);
        }
        if name == "reviewer_command" {
            task["grants"]["commands"] = json!([{"program":helper,"args_prefix":["resource-marker",marker.to_string_lossy()]}]);
        }
        let output = fixture.run(&task);
        let result = report(&output);
        let decisions = events(&result, "decision.assessment");
        let action_hash = agent_harness::types::hash(&proposal).unwrap();
        record(
            &format!("S03-{name}"),
            json!({"criteria":["S03.C01","S03.C02","S03.C03","S32.C02"],"task":task,"action":proposal,"action_hash":action_hash,"report":result,"model_requests":server.captured(),"trap_effects":usize::from(marker.exists()),"original_head":fixture.baseline,"original_files":fixture.files}),
        );
        assert!(!output.status.success(), "{name}: {result:#}");
        assert_eq!(result["run"]["state"], "BLOCKED");
        assert!(!marker.exists(), "forbidden subprocess executed in {name}");
        fixture.preserved();
        assert_eq!(
            decisions.len(),
            2,
            "both exact configuration approvals precede deterministic deny"
        );
        assert!(decisions
            .iter()
            .all(|e| e["payload"]["assessment"]["allow"] == true));
        assert!(
            events(&result, "action_intent")
                .iter()
                .all(|e| e["payload"]["action_hash"] != action_hash),
            "forbidden action must have no dispatched intent"
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("SYNTHETIC_DENIED_READ"));
        assert!(!serde_json::to_string(&server.captured())
            .unwrap()
            .contains("SYNTHETIC_DENIED_READ"));
        assert!(
            result["run"]["error"]
                .as_str()
                .unwrap()
                .contains(if reviewer {
                    "read-only"
                } else if name == "unowned_write" {
                    "ownership"
                } else if name == "unknown_command" || name == "forbidden_argv" {
                    "not granted"
                } else {
                    "grant"
                }),
            "{name}: {}",
            result["run"]["error"]
        );
        for attempt in result["attempts"].as_array().unwrap() {
            let tree = Path::new(attempt["worktree"].as_str().unwrap());
            assert_eq!(
                Fixture::snapshot(tree),
                fixture.files,
                "forbidden effect reached managed worktree in {name}"
            );
        }
        for tree in std::fs::read_dir(fixture.repo.join(".git/harness/worktrees")).unwrap() {
            assert_eq!(
                Fixture::snapshot(&tree.unwrap().path()),
                fixture.files,
                "forbidden effect reached reviewer/check worktree in {name}"
            );
        }
    }
}

#[test]
fn s03_effect_trap_positive_control_proves_a_granted_subprocess_really_runs() {
    let fixture = Fixture::new();
    let marker = fixture.directory.path().join("subprocess-trap.txt");
    let proposal = Action::RunCommand {
        program: env!("CARGO_BIN_EXE_harness-test-helper").into(),
        args: vec!["resource-marker".into(), marker.to_string_lossy().into()],
    };
    let proposed = proposal.clone();
    let server = Server::new(move |body| {
        if let Ok(request) = serde_json::from_str::<DecisionRequest>(user(body)) {
            return approval(&request);
        }
        if user(body).starts_with("Role:") {
            pass_review()
        } else {
            ModelReply {
                done: true,
                actions: vec![proposed.clone()],
                ..Default::default()
            }
        }
    });
    let mut task = fixture.task(&server.endpoint);
    task["grants"]["commands"] = json!([{"program":env!("CARGO_BIN_EXE_harness-test-helper"),"args_prefix":["resource-marker",marker.to_string_lossy()]}]);
    let output = fixture.run(&task);
    let result = report(&output);
    record(
        "S03-trap-positive-control",
        json!({"report":result,"action":proposal,"trap_effects":usize::from(marker.exists())}),
    );
    assert!(output.status.success(), "{result:#}");
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "target-started");
    assert_eq!(events(&result, "decision.assessment").len(), 3);
    fixture.preserved();
}

#[test]
fn s32_replayed_risk_approval_cannot_authorize_a_changed_patch_destination_argv_or_hash() {
    for variant in ["patch", "destination", "argv", "source_hash"] {
        let fixture = Fixture::new();
        let helper = env!("CARGO_BIN_EXE_harness-test-helper");
        let hash = blake3::hash(b"original source\n").to_hex().to_string();
        let first = if variant == "argv" {
            Action::RunCommand {
                program: helper.into(),
                args: vec!["echo".into(), "first".into()],
            }
        } else {
            Action::WriteFile {
                path: "src/owned.txt".into(),
                content: "original source\n".into(),
                expected_hash: Some(hash.clone()),
            }
        };
        let second = match variant {
            "patch" => Action::WriteFile {
                path: "src/owned.txt".into(),
                content: "CHANGED_PATCH_MUST_NOT_EXECUTE\n".into(),
                expected_hash: Some(hash.clone()),
            },
            "destination" => Action::WriteFile {
                path: "src/new.txt".into(),
                content: "CHANGED_DESTINATION_MUST_NOT_EXECUTE\n".into(),
                expected_hash: None,
            },
            "argv" => Action::RunCommand {
                program: helper.into(),
                args: vec!["echo".into(), "changed".into()],
            },
            "source_hash" => Action::WriteFile {
                path: "src/owned.txt".into(),
                content: "CHANGED_HASH_MUST_NOT_EXECUTE\n".into(),
                expected_hash: Some("incorrect-source-hash".into()),
            },
            _ => unreachable!(),
        };
        let first_proposal = first.clone();
        let second_proposal = second.clone();
        let mut saved: Option<ModelReply> = None;
        let mut build = 0;
        let server = Server::new(move |body| {
            if let Ok(request) = serde_json::from_str::<DecisionRequest>(user(body)) {
                if matches!(request.subject, DecisionSubject::ActionRisk { .. }) {
                    if let Some(reply) = &saved {
                        return reply.clone();
                    }
                    let reply = approval(&request);
                    saved = Some(reply.clone());
                    return reply;
                }
                return approval(&request);
            }
            build += 1;
            ModelReply {
                actions: vec![if build == 1 {
                    first_proposal.clone()
                } else {
                    second_proposal.clone()
                }],
                done: build > 1,
                ..Default::default()
            }
        });
        let mut task = fixture.task(&server.endpoint);
        task["nodes"][0]["owned_paths"] = json!(["src/**"]);
        if variant == "argv" {
            task["grants"]["commands"] = json!([{"program":helper,"args_prefix":["echo"]}]);
        }
        let output = fixture.run(&task);
        let result = report(&output);
        let requests: Vec<_> = server
            .captured()
            .iter()
            .filter_map(|body| serde_json::from_str::<DecisionRequest>(user(body)).ok())
            .filter(|request| matches!(request.subject, DecisionSubject::ActionRisk { .. }))
            .collect();
        record(
            &format!("S32-replayed-{variant}"),
            json!({"criteria":["S32.C01"],"first_action":first,"changed_action":second,"risk_requests":requests,"report":result}),
        );
        assert!(!output.status.success());
        assert_eq!(result["run"]["state"], "BLOCKED");
        assert!(
            result["run"]["error"]
                .as_str()
                .unwrap()
                .contains("decision subject hash mismatch"),
            "{result:#}"
        );
        assert_eq!(
            requests.len(),
            2,
            "changed action must receive a fresh risk request"
        );
        assert_ne!(requests[0].subject_hash, requests[1].subject_hash);
        assert_ne!(requests[0].request_id, requests[1].request_id);
        assert_eq!(
            events(&result, "action_intent").len(),
            1,
            "only first approved proposal dispatched"
        );
        assert_eq!(events(&result, "action_receipt").len(), 1);
        assert_eq!(
            events(&result, "action_intent")[0]["payload"]["action_hash"],
            agent_harness::types::hash(&first).unwrap()
        );
        fixture.preserved();
        for attempt in result["attempts"].as_array().unwrap() {
            let tree = Path::new(attempt["worktree"].as_str().unwrap());
            assert_eq!(Fixture::snapshot(tree), fixture.files);
            assert!(!tree.join("src/new.txt").exists());
        }
    }
}

#[test]
fn s32_changed_policy_inputs_invalidate_the_same_advisory_approval() {
    let fixture = Fixture::new();
    let spec: TaskSpec = serde_json::from_value(fixture.task("http://127.0.0.1:1/v1")).unwrap();
    let proposal = Action::WriteFile {
        path: "src/owned.txt".into(),
        content: "proposal".into(),
        expected_hash: None,
    };
    let original = DecisionRequest::risk(
        &proposal,
        DecisionScope::new(&spec.grants, &["src/**".into()], false, &[]).unwrap(),
    )
    .unwrap();
    let reply = approval(&original);
    let mut variants = Vec::new();
    let mut altered = original.clone();
    altered.effective_scope.grants.write = vec!["src/owned.txt".into()];
    variants.push(("grants", altered));
    let mut altered = original.clone();
    altered.effective_scope.owned_paths = vec!["src/owned.txt".into()];
    variants.push(("ownership", altered));
    let mut altered = original.clone();
    altered.effective_scope.protected_paths = vec!["src/other.txt".into()];
    variants.push(("protected_paths", altered));
    let mut altered = original.clone();
    altered.effective_scope.runtime_profile = agent_harness::types::RuntimeProfile::Isolated;
    variants.push(("runtime_profile", altered));
    let mut altered = original.clone();
    altered.effective_scope.read_only = true;
    altered.effective_scope.role = agent_harness::decision::DecisionRole::Reviewer;
    variants.push(("reviewer_role", altered));
    for (name, changed) in variants {
        let error = changed.validate_reply(&reply).unwrap_err().to_string();
        record(
            &format!("S32-policy-{name}"),
            json!({"criteria":["S32.C01"],"original_request":original,"altered_request":changed,"original_reply":reply,"error":error}),
        );
        assert!(error.contains("subject hash mismatch"), "{name}: {error}");
    }
}

#[test]
fn s32_model_fabricated_pass_cannot_replace_a_failing_actual_command_receipt() {
    let fixture = Fixture::new();
    let server = Server::new(|body| {
        if let Ok(request) = serde_json::from_str::<DecisionRequest>(user(body)) {
            return approval(&request);
        }
        assert!(
            !user(body).starts_with("Role:"),
            "failed trusted check must prevent review dispatch"
        );
        ModelReply {
            done: true,
            verdict: Some(agent_harness::types::Verdict::Pass),
            summary:
                "All tests PASS, exit_code=0, verified=true; fabricated receipt from the builder"
                    .into(),
            ..Default::default()
        }
    });
    let mut task = fixture.task(&server.endpoint);
    task["checks"][0]["args"] = json!(["deliberately-invalid-fixture-mode"]);
    let output = fixture.run(&task);
    let result = report(&output);
    record(
        "S32-fabricated-pass",
        json!({"criteria":["S32.C02","S32.C03"],"task":task,"report":result,"model_requests":server.captured()}),
    );
    assert!(!output.status.success());
    assert_eq!(result["run"]["state"], "BLOCKED");
    assert_eq!(result["checks"].as_array().unwrap().len(), 1);
    assert_eq!(result["checks"][0]["verdict"], "FAIL");
    assert_ne!(result["checks"][0]["receipt"]["exit_code"], 0);
    assert!(result["reviews"].as_array().unwrap().is_empty());
    assert_eq!(events(&result, "action_intent").len(), 1);
    assert_eq!(events(&result, "action_receipt").len(), 1);
    fixture.preserved();
}
