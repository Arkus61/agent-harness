use agent_harness::codex::{account_status, complete, reply_schema, validate_input};
use agent_harness::types::Action;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn executable() -> &'static str {
    env!("CARGO_BIN_EXE_harness-test-helper")
}

#[tokio::test]
async fn subscription_status_excludes_account_and_configuration_data() {
    let status = account_status(executable(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(status["authenticated"], true);
    assert_eq!(status["account_type"], "chatgpt");
    assert_eq!(status["plan_type"], "plus");
    assert_eq!(status["default_model"], "fixture-default");
    assert_eq!(status["models"].as_array().unwrap().len(), 2);
    assert!(!status.to_string().contains("PRIVATE"));
    assert_eq!(status["hard_output_limit"], false);
}

#[tokio::test]
async fn fresh_tool_free_thread_buffers_notifications_and_charges_total_usage() {
    let response = complete(
        executable(),
        "fixture-default",
        "Respond as JSON",
        "task",
        1,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.reply.done);
    assert_eq!(response.reply.summary, "mock completion");
    assert!(response.usage.complete);
    assert_eq!(response.usage.input_tokens, 200);
    // Greater than soft requested output cap: return accurate usage for ledger settlement.
    assert_eq!(response.usage.output_tokens, 17);
}

#[tokio::test]
async fn empty_model_selects_the_authenticated_catalog_default() {
    let response = complete(
        executable(),
        "",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.reply.done);
    assert!(response.usage.complete);
}

#[tokio::test]
async fn tool_free_inference_accepts_gateway_proposals_as_json_data() {
    // The portable mock verifies thread developer instructions and every turn's
    // read-only/no-environment policy before returning this JSON proposal.
    let response = complete(
        executable(),
        "fixture-proposal",
        "Propose an authorized write through the external HarnessToolGateway.",
        "The gateway grants write to src/lib.rs; your inference process has no tools.",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!response.reply.done);
    assert!(matches!(
        response.reply.actions.as_slice(),
        [Action::WriteFile { path, content, expected_hash: None }]
            if path == "src/lib.rs" && content == "gateway proposal only"
    ));
    assert!(response.usage.complete);
    // Actual app-server tool operations continue to fail closed under the same
    // developer instructions; a proposal does not enable transport tools.
    assert!(complete(
        executable(),
        "fixture-tool",
        "Propose an authorized write through the external HarnessToolGateway.",
        "The gateway grants write to src/lib.rs; your inference process has no tools.",
        100,
        CancellationToken::new(),
    )
    .await
    .is_err());
}

#[tokio::test]
async fn gateway_risk_assessment_preserves_required_subject_binding() {
    let subject_hash = "98d86c72a271a24a68dad24a4b3cc80d42dce7f61398d3f2e4e2945d864c37730";
    let input = serde_json::json!({
        "purpose":"risk",
        "subject_hash":subject_hash,
        "subject":{"kind":"gateway_action","action":{"type":"write_file","path":"src/lib.rs","content":"authorized proposal","expected_hash":null}}
    })
    .to_string();
    let response = complete(
        executable(),
        "fixture-decision",
        "Assess the exact gateway subject and echo its subject_hash.",
        &input,
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let assessment = response.reply.decision.unwrap();
    assert_eq!(assessment.purpose, "risk");
    assert_eq!(assessment.subject_hash, subject_hash);
    assert!(assessment.allow && !assessment.abstain);
    assert!(response.reply.actions.is_empty());
    assert!(complete(
        executable(),
        "fixture-decision-missing-hash",
        "Assess the exact gateway subject and echo its subject_hash.",
        &input,
        100,
        CancellationToken::new(),
    )
    .await
    .is_err());
}

#[tokio::test]
async fn missing_usage_is_unknown_instead_of_free() {
    let response = complete(
        executable(),
        "fixture-no-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!response.usage.complete);
}

#[tokio::test]
async fn usage_after_turn_completion_is_reconciled() {
    let response = complete(
        executable(),
        "fixture-after-completed-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.usage.complete);
    assert_eq!(
        (response.usage.input_tokens, response.usage.output_tokens),
        (400, 40)
    );
}

#[tokio::test]
async fn interim_usage_is_replaced_by_final_cumulative_usage() {
    let response = complete(
        executable(),
        "fixture-interim-then-final-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.usage.complete);
    assert_eq!(
        (response.usage.input_tokens, response.usage.output_tokens),
        (400, 40)
    );
}

#[tokio::test]
async fn final_message_after_completed_is_retained() {
    let response = complete(
        executable(),
        "fixture-after-completed-final",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(response.reply.done);
    assert_eq!(response.reply.summary, "mock completion");
    assert!(response.usage.complete);
}

#[tokio::test]
async fn drain_captures_usage_after_read_fence_and_invalid_usage_keeps_hold() {
    let response = complete(
        executable(),
        "fixture-post-fence-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        (response.usage.input_tokens, response.usage.output_tokens),
        (400, 40)
    );
    assert!(response.usage.complete);
    let response = complete(
        executable(),
        "fixture-invalid-final-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!response.usage.complete);
    assert!(complete(
        executable(),
        "fixture-decreasing-usage",
        "JSON",
        "task",
        100,
        CancellationToken::new()
    )
    .await
    .is_err());
}

#[test]
fn shared_validator_rejects_plain_and_json_encoded_oversized_prompts() {
    validate_input("JSON", "task", 100).unwrap();
    assert!(validate_input("JSON", "task", 0).is_err());
    assert!(validate_input("JSON", &"a".repeat(2 * 1024 * 1024), 100).is_err());
    assert!(validate_input(&"\u{0001}".repeat(900_000), "task", 100).is_err());
    assert!(validate_input("JSON", &"\u{0001}".repeat(900_000), 100).is_err());
}

#[tokio::test]
async fn protocol_tools_server_requests_and_wrong_ids_fail_closed() {
    for model in [
        "fixture-tool",
        "fixture-error",
        "fixture-server-request",
        "fixture-wrong-id",
        "fixture-two-finals",
        "fixture-oversized",
        "fixture-invalid-json",
        "fixture-invalid-severity",
    ] {
        let error = complete(
            executable(),
            model,
            "JSON",
            "task",
            100,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(
            !error.to_string().contains("PRIVATE"),
            "raw body exposed for {model}"
        );
    }
}

#[tokio::test]
async fn cancelling_inference_stops_process_and_removes_private_directory() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("started");
    let marker_text = marker.to_string_lossy().to_string();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let completion = tokio::spawn(async move {
        complete(
            executable(),
            "fixture-cancel",
            "JSON",
            &marker_text,
            100,
            token,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let private = std::fs::read_to_string(&marker).unwrap();
    cancel.cancel();
    assert!(completion
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    let heartbeat = marker.with_extension("heartbeat");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let first = std::fs::read(&heartbeat).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(first, std::fs::read(&heartbeat).unwrap());
    assert!(!std::path::Path::new(&private).exists());
}

#[test]
fn strict_reply_schema_covers_all_harness_actions() {
    let schema = reply_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"].as_array().unwrap().len(), 8);
    assert_eq!(
        schema["properties"]["actions"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let decision = &schema["properties"]["decision"]["anyOf"][0];
    assert_eq!(decision["properties"]["subject_hash"]["type"], "string");
    assert!(decision["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "subject_hash"));
}

/// Explicit manual probe: one bounded subscription call, no gateway execution,
/// no repository context, and no API-key fallback. Never runs in ordinary tests.
#[tokio::test]
#[ignore = "requires authorized real ChatGPT subscription inference"]
async fn live_subscription_gateway_risk_contract_smoke() {
    use agent_harness::decision::{DecisionRequest, DecisionScope, DECISION_SYSTEM};
    use agent_harness::types::Grants;
    let scope = DecisionScope::new(
        &Grants {
            read: vec!["src/smoke.txt".into()],
            write: vec!["src/smoke.txt".into()],
            commands: vec![],
        },
        &["src/smoke.txt".into()],
        false,
        &["Cargo.toml".into(), "Cargo.lock".into()],
    )
    .unwrap();
    let request = DecisionRequest::risk(
        &Action::WriteFile {
            path: "src/smoke.txt".into(),
            content: "JSON proposal only; this smoke does not execute gateway actions.\n".into(),
            expected_hash: None,
        },
        scope,
    )
    .unwrap();
    let response = complete(
        "codex",
        "gpt-6.1-sol",
        DECISION_SYSTEM,
        &serde_json::to_string(&request).unwrap(),
        1024,
        CancellationToken::new(),
    )
    .await;
    let (passed, evidence) = match response {
        Ok(response) => {
            let validation = request.validate_reply(&response.reply);
            let passed = validation.is_ok()
                && response.usage.complete
                && response
                    .reply
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.allow && !decision.abstain);
            (
                passed,
                serde_json::json!({
                    "status":if passed {"PASS"} else {"HOLD"},
                    "dispatches":1,
                    "model":"gpt-6.1-sol",
                    "executor":"harness_tool_gateway",
                    "gateway_executed":false,
                    "inference_sandbox":"readOnly",
                    "request":request,
                    "reply":response.reply,
                    "usage":response.usage,
                    "contract_valid":validation.is_ok(),
                    "contract_error":validation.err().map(|error| error.to_string())
                }),
            )
        }
        Err(error) => (
            false,
            serde_json::json!({
                "status":"ERROR",
                "dispatches":1,
                "model":"gpt-6.1-sol",
                "gateway_executed":false,
                "request":request,
                "usage_complete":false,
                "error":error.to_string()
            }),
        ),
    };
    let directory = std::path::Path::new("artifacts/contract-validation/subscription");
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("live-risk-contract-smoke.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    assert!(
        passed,
        "live risk contract smoke failed; see sanitized evidence"
    );
}
