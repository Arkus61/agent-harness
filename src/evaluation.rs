//! Deterministic component assertions. These are fixture evidence, never release gates.
use crate::execution::{command_allowed, read_file, validate_action, write_file};
use crate::storage::Store;
use crate::types::{Action, AttemptRecord, CommandGrant, Grants, RunState, TaskSpec, Usage};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Workspace(PathBuf);

impl Workspace {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("harness-eval-{}", crate::types::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn record(results: &mut Vec<Value>, id: &str, description: &str, pass: bool, evidence: Value) {
    results.push(json!({
        "criterion_id":id,
        "scenario_id":id.split('.').next().unwrap_or(id),
        "description":description,
        "verdict":if pass {"PASS"} else {"FAIL"},
        "evidence":evidence,
    }));
}

fn fixture_task() -> Result<TaskSpec> {
    Ok(serde_json::from_value(json!({
        "prompt":"Exercise deterministic harness contracts",
        "requirements":["The protected command check passes"],
        "grants":{"read":["**"],"write":["src/**"],"commands":[]},
        "checks":[{"program":"fixture-check","args":[],"timeout_secs":1}],
        "provider":{"kind":"scripted","scripts":{}},
        "budget":{"max_tokens":100,"max_output_tokens":20,"deadline_secs":60}
    }))?)
}

pub async fn run_suite() -> Result<Value> {
    let workspace = Workspace::create()?;
    let mut results = Vec::new();
    let valid = fixture_task()?;
    record(
        &mut results,
        "S02.valid-spec",
        "A complete fixture TaskSpec validates",
        valid.validate().is_ok(),
        json!({"schema_version":valid.schema_version}),
    );
    let mut invalid = valid.clone();
    invalid.prompt = " ".into();
    let validation = invalid.validate();
    record(
        &mut results,
        "S02.empty-prompt",
        "An empty prompt is rejected",
        validation.is_err(),
        json!({"error":validation.err().map(|e|e.to_string())}),
    );
    invalid = valid.clone();
    invalid.checks.clear();
    let validation = invalid.validate();
    record(
        &mut results,
        "S02.missing-check",
        "A task without protected checks is rejected",
        validation.is_err(),
        json!({"error":validation.err().map(|e|e.to_string())}),
    );
    let mut malformed = serde_json::to_value(&valid)?;
    malformed["unrecognized_permission"] = json!(true);
    let parsed = serde_json::from_value::<TaskSpec>(malformed);
    record(
        &mut results,
        "S02.unknown-field",
        "Unknown task fields are rejected instead of silently accepted",
        parsed.is_err(),
        json!({"error":parsed.err().map(|e|e.to_string())}),
    );
    let grants = Grants {
        read: vec!["**".into()],
        write: vec!["src/**".into()],
        commands: vec![CommandGrant {
            program: "git".into(),
            args_prefix: vec!["status".into()],
        }],
    };
    record(
        &mut results,
        "S03.command-grant",
        "An exact granted command is allowed but an ungranted command is denied",
        command_allowed("git", &["status".into()], &grants)
            && !command_allowed("git", &["push".into()], &grants)
            && !command_allowed("sh", &["-c".into()], &grants),
        json!({"granted":"git status","denied":["git push","sh -c"]}),
    );
    let denied = Action::RunCommand {
        program: "sh".into(),
        args: vec!["-c".into(), "mutate-outside-scope".into()],
    };
    let validation = validate_action(&denied, &grants, &["src/**".into()], false);
    record(
        &mut results,
        "S03.action-validation",
        "Action validation rejects a command outside deterministic grants",
        validation.is_err(),
        json!({"error":validation.err().map(|e|e.to_string())}),
    );
    let write = Action::WriteFile {
        path: "src/not-owned.rs".into(),
        content: "replacement".into(),
        expected_hash: None,
    };
    record(
        &mut results,
        "S03.owner-and-reviewer",
        "Unowned and reviewer writes are rejected",
        validate_action(&write, &grants, &["src/owned.rs".into()], false).is_err()
            && validate_action(&write, &grants, &["src/**".into()], true).is_err(),
        json!({"action_path":"src/not-owned.rs","owner":"src/owned.rs","reviewer_read_only":true}),
    );
    let file_grants = Grants {
        read: vec!["**".into()],
        write: vec!["**".into()],
        commands: vec![],
    };
    let path = "каталог с пробелами/данные.txt";
    let contents = "Unicode ✓\r\n";
    let written = write_file(workspace.path(), path, contents, None, &file_grants)?;
    let read = read_file(workspace.path(), path, &file_grants, 1024)?;
    record(
        &mut results,
        "S10.unicode-roundtrip",
        "Unicode, spaces and CRLF survive an exact scoped file roundtrip",
        read == contents && written == blake3::hash(contents.as_bytes()).to_hex().to_string(),
        json!({"path":path,"expected_bytes":contents.len(),"observed_bytes":read.len(),"content_hash":written}),
    );
    let stale_write = write_file(
        workspace.path(),
        path,
        "changed",
        Some("wrong-hash"),
        &file_grants,
    );
    let after = read_file(workspace.path(), path, &file_grants, 1024)?;
    record(
        &mut results,
        "S10.stale-source",
        "A stale source hash blocks mutation and preserves the original bytes",
        stale_write.is_err() && after == contents,
        json!({"error":stale_write.err().map(|e|e.to_string()),"preserved":after==contents}),
    );
    record(
        &mut results,
        "S10.traversal",
        "Scoped file APIs deny parent traversal",
        read_file(workspace.path(), "../outside.txt", &file_grants, 1024).is_err()
            && write_file(
                workspace.path(),
                "../outside.txt",
                "bad",
                None,
                &file_grants,
            )
            .is_err(),
        json!({"denied_path":"../outside.txt"}),
    );
    let state_dir = workspace.path().join("state");
    let store = Store::open(&state_dir)?;
    let run = store.create_run("fixture-run", workspace.path(), &valid, "fixture-base")?;
    store.set_state(&run.id, RunState::Planning, None)?;
    store.set_state(&run.id, RunState::Executing, None)?;
    let before_events = store.events(&run.id)?;
    drop(store);
    let store = Store::open(&state_dir)?;
    let reopened = store.get_run(&run.id)?;
    let reopened_events = store.events(&run.id)?;
    record(
        &mut results,
        "S04.store-reopen",
        "Run projection and event history survive closing and reopening SQLite",
        reopened.state == RunState::Executing
            && reopened.task_hash == run.task_hash
            && crate::types::hash(&before_events)? == crate::types::hash(&reopened_events)?,
        json!({"state":reopened.state,"events_before":before_events.len(),"events_after":reopened_events.len()}),
    );
    let action = store.intent(&run.id, "fixture-attempt", &denied)?;
    store.reconcile_action(&action.id, "unknown")?;
    record(
        &mut results,
        "S05.pending-intent",
        "An unknown command intent stays pending until reconciled; no completion is fabricated",
        store
            .pending_actions(&run.id)?
            .iter()
            .any(|a| a.id == action.id && a.status == "unknown")
            && store.reconcile_action(&action.id, "completed").is_err(),
        json!({"action_id":action.id,"status":"unknown","executed":false}),
    );
    store.reserve(&run.id, "settled-call", 30)?;
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 5,
        complete: true,
    };
    store.settle(&run.id, "settled-call", &usage)?;
    store.settle(&run.id, "settled-call", &usage)?;
    store.reserve(&run.id, "unknown-call", 70)?;
    store.reserve(&run.id, "unknown-call", 70)?;
    let accounted = store.get_run(&run.id)?;
    record(
        &mut results,
        "S09.idempotent-accounting",
        "Repeated reservation and settlement do not double-count the same call",
        accounted.spent_tokens == 15 && accounted.reserved_tokens == 70,
        json!({"spent":accounted.spent_tokens,"reserved":accounted.reserved_tokens,"expected_spent":15,"expected_reserved":70}),
    );
    let excess = store.reserve(&run.id, "excess-call", 16);
    record(
        &mut results,
        "S09.root-limit",
        "Concurrent-call headroom is checked against spent plus reserved root tokens",
        excess.is_err() && store.get_run(&run.id)?.reserved_tokens == 70,
        json!({"error":excess.err().map(|e|e.to_string()),"root_limit":100}),
    );
    store.settle(
        &run.id,
        "unknown-call",
        &Usage {
            input_tokens: 0,
            output_tokens: 0,
            complete: false,
        },
    )?;
    let attempt = AttemptRecord {
        id: "fenced-attempt".into(),
        run_id: run.id.clone(),
        node_id: "fixture-node".into(),
        generation: 1,
        input_sha: "fixture-base".into(),
        output_sha: None,
        worktree: workspace.path().to_string_lossy().into_owned(),
        status: "running".into(),
    };
    store.put_attempt(&attempt)?;
    let generation = store.advance_generation(&run.id)?;
    record(
        &mut results,
        "S11.stale-generation",
        "An old-generation attempt cannot publish an output after recovery",
        generation == 2
            && store
                .finish_attempt(&attempt.id, 1, "stale-output")
                .is_err(),
        json!({"attempt_generation":1,"run_generation":generation}),
    );
    drop(store);
    let store = Store::open(&state_dir)?;
    let recovered = store.get_run(&run.id)?;
    record(
        &mut results,
        "S09.recovery-keeps-budget",
        "Unknown usage and budget accounting survive generation advance and store reopen",
        recovered.spent_tokens == 15
            && recovered.reserved_tokens == 70
            && recovered.generation == 2,
        json!({"spent":recovered.spent_tokens,"reserved":recovered.reserved_tokens,"generation":recovered.generation}),
    );
    let event_seq = store
        .event(&run.id, "fixture.hook-trigger", json!({"generation":2}))?
        .seq;
    let claimed = store.claim_hook(&run.id, event_seq, "fixture-hook", 2)?;
    let duplicate = store.claim_hook(&run.id, event_seq, "fixture-hook", 2)?;
    let stale = store.claim_hook(&run.id, event_seq, "fixture-hook", 1)?;
    record(
        &mut results,
        "S19.hook-idempotency",
        "Duplicate and stale-generation hook claims do not fire a second handler",
        claimed && !duplicate && !stale,
        json!({"first":claimed,"duplicate":duplicate,"stale":stale}),
    );
    let object = store.put_object(b"fixture evidence")?;
    let bytes = store.get_object(&object)?;
    record(
        &mut results,
        "S12.cas-roundtrip",
        "Content-addressed evidence is readable under its exact BLAKE3 hash",
        bytes == b"fixture evidence" && object == blake3::hash(&bytes).to_hex().to_string(),
        json!({"hash":object,"bytes":bytes.len()}),
    );
    let missing = store.get_object(&"0".repeat(64));
    record(
        &mut results,
        "S12.missing-evidence",
        "Missing evidence is rejected instead of becoming an empty receipt",
        missing.is_err(),
        json!({"error":missing.err().map(|e|e.to_string())}),
    );
    let object_path = store
        .root()
        .join("objects")
        .join(&object[..2])
        .join(&object);
    std::fs::write(object_path, b"corrupted evidence")?;
    let corrupted = store.get_object(&object);
    record(
        &mut results,
        "S12.corrupt-evidence",
        "Evidence with bytes inconsistent with its hash is rejected",
        corrupted.is_err(),
        json!({"error":corrupted.err().map(|e|e.to_string())}),
    );
    let passed = results.iter().filter(|v| v["verdict"] == "PASS").count();
    let failed = results.len() - passed;
    Ok(json!({
        "schema_version":1,
        "suite":"deterministic-component-contracts",
        "verification_mode":"fixture",
        "platform":std::env::consts::OS,
        "generated_at":crate::types::now(),
        "summary":{"criteria":results.len(),"passed":passed,"failed":failed},
        "passed":failed==0,
        "criteria":results,
        "limitations":[
            "These component facets do not establish full S01-S42 scenario coverage.",
            "No production model quality, power-loss durability or kernel sandbox support is certified.",
            "Process timeout and descendant cancellation are exercised by cargo test --test contracts.",
            "Fixture evidence cannot authorize production VERIFIED or merge."
        ]
    }))
}
