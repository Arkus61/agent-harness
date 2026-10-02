//! A sibling cancellation must not replace the policy failure that caused it.
use agent_harness::decision::{DecisionRequest, DecisionSubject};
use agent_harness::execution::Repo;
use agent_harness::types::{Action, DecisionAssessment, ModelReply, RunState, TaskSpec};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Barrier;

async fn request_body(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        assert!(count > 0, "mock received an incomplete request");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() < 256 * 1024, "mock request exceeded test bound");
        let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&bytes[..position]).unwrap();
        let length: usize = headers
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
            return serde_json::from_slice(&bytes[position + 4..position + 4 + length]).unwrap();
        }
    }
}

async fn respond(stream: &mut TcpStream, reply: ModelReply) {
    let body = serde_json::to_vec(&json!({
        "choices": [{"message": {"content": serde_json::to_string(&reply).unwrap()}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 10}
    }))
    .unwrap();
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(&body).await.unwrap();
    stream.shutdown().await.unwrap();
}

#[tokio::test]
async fn parallel_cancellation_preserves_primary_policy_failure_and_node_evidence() {
    let directory = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(directory.path())
        .status()
        .unwrap()
        .success());
    std::fs::create_dir(directory.path().join("src")).unwrap();
    std::fs::write(directory.path().join("src/violating.txt"), "unchanged\n").unwrap();
    std::fs::write(directory.path().join("src/waiting.txt"), "unchanged\n").unwrap();
    let repo = Repo::discover(directory.path()).unwrap();
    let baseline = repo.commit(directory.path(), "baseline").unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let mock = tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        // Two model assessments, two tool assessments, and two builders.
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let barrier = barrier.clone();
            clients.spawn(async move {
                let body = request_body(&mut stream).await;
                let user = body["messages"][1]["content"].as_str().unwrap();
                if let Ok(request) = serde_json::from_str::<DecisionRequest>(user) {
                    request.validate().unwrap();
                    let (choice, tools) = match &request.subject {
                        DecisionSubject::ModelSelection {
                            requested_model, ..
                        } => (Some(requested_model.clone()), vec![]),
                        DecisionSubject::ToolConfiguration { enabled_tools } => {
                            (None, enabled_tools.clone())
                        }
                        DecisionSubject::ActionRisk { .. } => {
                            panic!("a forbidden proposal reached model risk assessment")
                        }
                    };
                    respond(
                        &mut stream,
                        ModelReply {
                            done: true,
                            decision: Some(DecisionAssessment {
                                purpose: request.purpose.as_str().into(),
                                subject_hash: request.subject_hash,
                                allow: true,
                                abstain: false,
                                reason: "Exact supplied model or tool configuration".into(),
                                choice,
                                tools,
                            }),
                            ..Default::default()
                        },
                    )
                    .await;
                    "decision"
                } else {
                    // No sleeps establish ordering: both builder requests must
                    // be in-flight before the sole failing response is released.
                    barrier.wait().await;
                    if user.contains("Node: VIOLATING_WORKER\n") {
                        respond(
                            &mut stream,
                            ModelReply {
                                done: true,
                                actions: vec![Action::WriteFile {
                                    path: ".env".into(),
                                    content: "synthetic forbidden write\n".into(),
                                    expected_hash: None,
                                }],
                                ..Default::default()
                            },
                        )
                        .await;
                        "violating"
                    } else {
                        assert!(user.contains("Node: WAITING_WORKER\n"));
                        // The second worker remains inside model transport until
                        // the first worker's gateway denial cancels its request.
                        let mut byte = [0u8; 1];
                        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
                        "waiting_cancelled"
                    }
                }
            });
        }
        let mut outcomes = Vec::new();
        while let Some(result) = clients.join_next().await {
            outcomes.push(result.unwrap());
        }
        outcomes
    });

    let task: TaskSpec = serde_json::from_value(json!({
        "prompt": "Exercise a policy denial while an independent worker is running",
        "requirements": ["No forbidden file is written", "Sibling work is cancelled"],
        "grants": {"read": ["src/**"], "write": ["src/**"], "commands": []},
        "checks": [{"program": "git", "args": ["diff", "--exit-code"], "timeout_secs": 10}],
        "provider": {"kind": "open_ai", "model": "mock", "base_url": url, "api_key_env": ""},
        "nodes": [
            {"id": "violating", "prompt": "VIOLATING_WORKER", "requirements": [0], "owned_paths": ["src/violating.txt"]},
            {"id": "waiting", "prompt": "WAITING_WORKER", "requirements": [1], "owned_paths": ["src/waiting.txt"]}
        ],
        "decision_mode": "enforced", "concurrency": 2, "max_repairs": 0, "max_steps": 1
    }))
    .unwrap();
    let report = tokio::time::timeout(
        Duration::from_secs(20),
        agent_harness::engine::run(directory.path(), Some(task), None),
    )
    .await
    .expect("parallel failure recovery hung")
    .unwrap();
    let outcomes = tokio::time::timeout(Duration::from_secs(5), mock)
        .await
        .expect("mock did not observe sibling transport cancellation")
        .unwrap();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == "decision")
            .count(),
        4
    );
    assert!(outcomes.contains(&"violating"));
    assert!(outcomes.contains(&"waiting_cancelled"));

    assert_eq!(report.run.state, RunState::Blocked);
    assert_eq!(report.attempts.len(), 2);
    assert!(report.run.candidate_sha.is_none());
    assert!(
        report.run.reserved_tokens > 0,
        "cancelled remote spending remains unknown"
    );
    assert_eq!(repo.head().unwrap(), baseline);
    assert!(!directory.path().join(".env").exists());
    for attempt in &report.attempts {
        assert!(!std::path::Path::new(&attempt.worktree)
            .join(".env")
            .exists());
    }
    assert!(report
        .events
        .iter()
        .all(|event| event.kind != "action_intent"));
    assert!(report
        .events
        .iter()
        .any(|event| event.kind == "action.denied_or_failed"));
    let error = report.run.error.as_deref().unwrap();
    assert!(
        error.contains("protected file or directory"),
        "initiating gateway denial was masked by sibling cancellation: {error}"
    );
    let primary: Vec<_> = report
        .events
        .iter()
        .filter(|event| event.kind == "dag.primary_failure")
        .collect();
    let secondary: Vec<_> = report
        .events
        .iter()
        .filter(|event| event.kind == "dag.secondary_failure")
        .collect();
    assert_eq!(primary.len(), 1);
    assert_eq!(secondary.len(), 1);
    assert_eq!(primary[0].payload["node"], "violating");
    assert_eq!(secondary[0].payload["node"], "waiting");
    assert!(primary[0].payload["error"]
        .as_str()
        .unwrap()
        .contains("protected file or directory"));
    assert!(secondary[0].payload["error"]
        .as_str()
        .unwrap()
        .contains("cancelled"));
    assert!(primary[0].seq < secondary[0].seq);
}
