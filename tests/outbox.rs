use agent_harness::outbox::{
    AdapterCapabilities, DeliveryAdapter, DeliveryOutcome, DeliveryState, EffectReceipt,
    HandlerPolicy, LocalJournalAdapter, NoEffectReceipt, OutboxWorker, Reconciliation,
};
use agent_harness::storage::Store;
use agent_harness::types::TaskSpec;
use rusqlite::Connection;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let task: TaskSpec = serde_json::from_value(json!({
        "prompt":"change a file", "requirements":["correct result"],
        "grants":{"read":["src/**"],"write":["src/**"]},
        "checks":[{"program":"helper"}], "provider":{}
    }))
    .unwrap();
    store.create_run("run", root.path(), &task, "base").unwrap();
    (root, store)
}

fn policy() -> HandlerPolicy {
    HandlerPolicy {
        handler: "journal".into(),
        version: 1,
        event_kinds: vec!["run_created".into()],
        max_attempts: 3,
        lease_ms: 100,
        backoff_ms: 10,
        max_backoff_ms: 40,
        max_chain_depth: 2,
    }
}
fn capabilities(idempotent: bool) -> AdapterCapabilities {
    AdapterCapabilities {
        idempotency_keys: idempotent,
        independent_reconciliation: true,
    }
}
fn receipt(delivery: &agent_harness::outbox::DeliveryRecord) -> EffectReceipt {
    EffectReceipt {
        event_seq: delivery.event.seq,
        handler: delivery.handler.clone(),
        policy_hash: delivery.policy_hash.clone(),
        idempotency_key: delivery.idempotency_key.clone(),
        effect_id: "durable-effect".into(),
    }
}

fn no_effect(delivery: &agent_harness::outbox::DeliveryRecord) -> NoEffectReceipt {
    NoEffectReceipt {
        event_seq: delivery.event.seq,
        handler: delivery.handler.clone(),
        policy_hash: delivery.policy_hash.clone(),
        idempotency_key: delivery.idempotency_key.clone(),
        attempt_fence: delivery.fence,
        evidence_id: "independent-backend-cancellation-proof".into(),
    }
}

#[test]
fn registrations_backfill_old_events_and_freeze_authority_policy() {
    let (root, store) = fixture();
    store
        .register_outbox_handler(&policy(), capabilities(true))
        .unwrap();
    assert_eq!(store.outbox_deliveries("journal").unwrap().len(), 1);
    assert!(store
        .register_outbox_handler(&policy(), capabilities(false))
        .is_err());
    let mut changed = policy();
    changed.max_attempts += 1;
    assert!(store
        .register_outbox_handler(&changed, capabilities(true))
        .is_err());
    assert!(serde_json::from_value::<HandlerPolicy>(json!({
        "handler":"evil","version":1,"event_kinds":["*"],"max_attempts":3,"lease_ms":100,
        "backoff_ms":10,"max_backoff_ms":40,"max_chain_depth":2,"grants":{"write":["**"]}
    }))
    .is_err());
    drop(store);
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(reopened.outbox_deliveries("journal").unwrap().len(), 1);
}

#[test]
fn expired_lease_is_fenced_and_stable_idempotency_key_survives_reopen() {
    let (root, store) = fixture();
    store
        .register_outbox_handler(&policy(), capabilities(true))
        .unwrap();
    let first = store.claim_outbox("journal", 1000).unwrap().unwrap();
    assert!(store.begin_outbox_effect(&first, 1001).unwrap());
    assert!(store.claim_outbox("journal", 1099).unwrap().is_none());
    drop(store);
    let store = Store::open(root.path()).unwrap();
    let next = store.claim_outbox("journal", 1120).unwrap().unwrap();
    assert_eq!(first.idempotency_key, next.idempotency_key);
    assert_eq!(next.attempts, 2);
    assert!(next.fence > first.fence);
    assert!(!store
        .finish_outbox_delivery(&first, DeliveryOutcome::Delivered(receipt(&first)), 1121)
        .unwrap());
    assert!(store.begin_outbox_effect(&next, 1121).unwrap());
    assert!(store
        .finish_outbox_delivery(&next, DeliveryOutcome::Delivered(receipt(&next)), 1122)
        .unwrap());
    assert_eq!(
        store.outbox_deliveries("journal").unwrap()[0].state,
        DeliveryState::Delivered
    );
    assert!(!store
        .finish_outbox_delivery(&next, DeliveryOutcome::Delivered(receipt(&next)), 1123)
        .unwrap());
}

#[test]
fn non_idempotent_uncertain_effect_is_unknown_and_never_repeated_automatically() {
    let (_root, store) = fixture();
    store
        .register_outbox_handler(&policy(), capabilities(false))
        .unwrap();
    let first = store.claim_outbox("journal", 1000).unwrap().unwrap();
    store.begin_outbox_effect(&first, 1001).unwrap();
    assert!(store.claim_outbox("journal", 1200).unwrap().is_none());
    let unknown = store.outbox_deliveries("journal").unwrap().remove(0);
    assert_eq!(unknown.state, DeliveryState::Unknown);
    assert_eq!(unknown.attempts, 1);
    assert!(store.claim_outbox("journal", 100000).unwrap().is_none());
    assert!(store
        .reconcile_outbox(
            &unknown,
            Reconciliation::NoEffect(no_effect(&unknown)),
            100001
        )
        .unwrap());
    assert!(store.claim_outbox("journal", 100002).unwrap().is_none());
    assert!(store.claim_outbox("journal", 100020).unwrap().is_some());
}

#[test]
fn retry_backoff_and_attempt_limit_are_durable_and_receipts_cannot_be_substituted() {
    let (_root, store) = fixture();
    store
        .register_outbox_handler(&policy(), capabilities(true))
        .unwrap();
    for (attempt, time) in [(1, 1000), (2, 1012), (3, 1034)] {
        let claimed = store.claim_outbox("journal", time).unwrap().unwrap();
        assert_eq!(claimed.attempts, attempt);
        store.begin_outbox_effect(&claimed, time).unwrap();
        store
            .finish_outbox_delivery(
                &claimed,
                DeliveryOutcome::FailedBeforeEffect { retryable: true },
                time + 1,
            )
            .unwrap();
        assert!(store.claim_outbox("journal", time + 2).unwrap().is_none());
    }
    assert_eq!(
        store.outbox_deliveries("journal").unwrap()[0].state,
        DeliveryState::DeadLetter
    );
    assert!(store.claim_outbox("journal", 100000).unwrap().is_none());

    let mut second = policy();
    second.handler = "second".into();
    store
        .register_outbox_handler(&second, capabilities(true))
        .unwrap();
    let claimed = store.claim_outbox("second", 1000).unwrap().unwrap();
    store.begin_outbox_effect(&claimed, 1000).unwrap();
    let mut wrong = receipt(&claimed);
    wrong.idempotency_key = "other-effect".into();
    store
        .finish_outbox_delivery(&claimed, DeliveryOutcome::Delivered(wrong), 1001)
        .unwrap();
    assert_eq!(
        store.outbox_deliveries("second").unwrap()[0].state,
        DeliveryState::Unknown
    );
}

#[test]
fn delivery_events_are_atomic_and_hook_cycles_stop_at_depth_limit() {
    let (_root, store) = fixture();
    let mut all = policy();
    all.event_kinds = vec!["*".into()];
    store
        .register_outbox_handler(&all, capabilities(true))
        .unwrap();
    for depth in 0..=2 {
        let delivery = store
            .claim_outbox("journal", 1000 + u64::from(depth))
            .unwrap()
            .unwrap();
        assert_eq!(delivery.chain_depth, depth);
        store
            .begin_outbox_effect(&delivery, 1000 + u64::from(depth))
            .unwrap();
        store
            .finish_outbox_delivery(
                &delivery,
                DeliveryOutcome::Delivered(receipt(&delivery)),
                1000 + u64::from(depth),
            )
            .unwrap();
    }
    assert!(store.claim_outbox("journal", 1100).unwrap().is_none());
    let rows = store.outbox_deliveries("journal").unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3].state, DeliveryState::DeadLetter);
    assert_eq!(rows[3].failure.as_deref(), Some("cycle_depth_exceeded"));
    let db = Connection::open(store.root().join("state.sqlite3")).unwrap();
    let events: i64 = db
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let outbox: i64 = db
        .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, outbox);
}

#[test]
fn real_local_journal_effect_is_not_duplicated_after_reply_is_lost_and_worker_reopens() {
    let (root, store) = fixture();
    let journal_root = root.path().join("journal-backend");
    let mut adapter = LocalJournalAdapter::open(&journal_root, "journal").unwrap();
    store
        .register_outbox_handler(&policy(), adapter.capabilities())
        .unwrap();
    let delivery = store.claim_outbox("journal", 1000).unwrap().unwrap();
    store.begin_outbox_effect(&delivery, 1001).unwrap();
    let outcome = adapter.deliver(&delivery).unwrap();
    assert!(matches!(outcome, DeliveryOutcome::Delivered(_)));
    assert_eq!(adapter.effect_count().unwrap(), 1);
    // Crash boundary: the backend transaction commits, but its receipt never
    // reaches the coordinator. Both coordinator and backend are then reopened.
    drop(outcome);
    drop(adapter);
    drop(store);
    let store = Store::open(root.path()).unwrap();
    let adapter = LocalJournalAdapter::open(&journal_root, "journal").unwrap();
    let mut worker = OutboxWorker::new(store.clone(), policy(), adapter).unwrap();
    let summary = worker.tick(1120, 10).unwrap();
    assert_eq!(summary.claimed, 1);
    assert_eq!(summary.delivered, 1);
    let adapter = LocalJournalAdapter::open(&journal_root, "journal").unwrap();
    assert_eq!(adapter.effect_count().unwrap(), 1);
    assert_eq!(store.outbox_deliveries("journal").unwrap()[0].attempts, 2);
    assert_eq!(worker.tick(2000, 10).unwrap().claimed, 0);
}

#[test]
fn worker_tick_is_bounded_and_adapter_identity_cannot_change_policy() {
    let (root, store) = fixture();
    let adapter =
        LocalJournalAdapter::open(root.path().join("journal-backend"), "journal").unwrap();
    let mut all = policy();
    all.event_kinds = vec!["*".into()];
    all.max_chain_depth = 16;
    let mut worker = OutboxWorker::new(store, all, adapter).unwrap();
    assert_eq!(worker.tick(1000, 2).unwrap().claimed, 2);
    assert!(worker.tick(1000, 0).is_err());
    assert!(worker.tick(1000, 101).is_err());
    let (_root, store) = fixture();
    let wrong = LocalJournalAdapter::open(root.path().join("wrong-backend"), "other").unwrap();
    assert!(OutboxWorker::new(store, policy(), wrong).is_err());
}

#[test]
fn local_backend_independent_reconciliation_returns_only_matching_durable_effect() {
    let (root, store) = fixture();
    let mut adapter = LocalJournalAdapter::open(root.path().join("backend"), "journal").unwrap();
    store
        .register_outbox_handler(&policy(), adapter.capabilities())
        .unwrap();
    let record = store.claim_outbox("journal", 1000).unwrap().unwrap();
    assert!(matches!(
        adapter.reconcile(&record).unwrap(),
        Reconciliation::NoEffect(_)
    ));
    adapter.deliver(&record).unwrap();
    match adapter.reconcile(&record).unwrap() {
        Reconciliation::Delivered(receipt) => {
            assert_eq!(receipt.idempotency_key, record.idempotency_key)
        }
        other => panic!("expected durable backend evidence, got {other:?}"),
    }
    let mut changed = record;
    changed.event.payload = json!({"grants":{"write":["**"]},"command":"touch /outside"});
    assert!(adapter.deliver(&changed).is_err());
    assert!(adapter.reconcile(&changed).is_err());
    assert_eq!(adapter.effect_count().unwrap(), 1);
}

#[test]
fn outbox_schema_migrates_old_events_without_losing_atomic_entries() {
    let (root, store) = fixture();
    drop(store);
    let db = Connection::open(root.path().join("state.sqlite3")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, 3,
        "durable delivery leases and receipts require schema v3"
    );
    let events: i64 = db
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let outbox: i64 = db
        .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events, outbox);
    let event_bytes: String = db
        .query_row("SELECT payload FROM events WHERE seq=1", [], |r| r.get(0))
        .unwrap();
    db.execute_batch("DROP TABLE outbox_deliveries; DROP TABLE outbox_handlers; DROP TABLE outbox_chains; PRAGMA user_version=2;").unwrap();
    drop(db);
    let reopened = Store::open(root.path()).unwrap();
    let db = Connection::open(reopened.root().join("state.sqlite3")).unwrap();
    let after: String = db
        .query_row("SELECT payload FROM events WHERE seq=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(event_bytes, after);
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    reopened
        .register_outbox_handler(&policy(), capabilities(true))
        .unwrap();
    assert_eq!(reopened.outbox_deliveries("journal").unwrap().len(), 1);
}

#[test]
fn failed_delivery_enqueue_rolls_back_projection_event_and_original_outbox_together() {
    let (_root, store) = fixture();
    let mut all = policy();
    all.event_kinds = vec!["*".into()];
    store
        .register_outbox_handler(&all, capabilities(true))
        .unwrap();
    let db = Connection::open(store.root().join("state.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_delivery BEFORE INSERT ON outbox_deliveries BEGIN SELECT RAISE(ABORT,'delivery failure'); END;").unwrap();
    let before = store.events("run").unwrap().len();
    assert!(store
        .set_state("run", agent_harness::types::RunState::Planning, None)
        .is_err());
    assert_eq!(
        store.get_run("run").unwrap().state,
        agent_harness::types::RunState::Received
    );
    assert_eq!(store.events("run").unwrap().len(), before);
    assert_eq!(store.outbox_deliveries("journal").unwrap().len(), 1);
    let original: i64 = db
        .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(original, before as i64);
}

#[test]
fn competing_real_sqlite_workers_commit_one_backend_effect_and_one_receipt() {
    let (root, store) = fixture();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let jobs: Vec<_> = (0..8)
        .map(|_| {
            let root = root.path().to_owned();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&root).unwrap();
                let adapter = LocalJournalAdapter::open(root.join("backend"), "journal").unwrap();
                let mut worker = OutboxWorker::new(store, policy(), adapter).unwrap();
                barrier.wait();
                worker.tick(1000, 1).unwrap().delivered
            })
        })
        .collect();
    let delivered: u32 = jobs.into_iter().map(|job| job.join().unwrap()).sum();
    assert_eq!(delivered, 1);
    assert_eq!(
        LocalJournalAdapter::open(root.path().join("backend"), "journal")
            .unwrap()
            .effect_count()
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .events("run")
            .unwrap()
            .iter()
            .filter(|e| e.kind == "outbox.delivered")
            .count(),
        1
    );
}

struct LostReplyWithoutReplay {
    inner: LocalJournalAdapter,
}
impl DeliveryAdapter for LostReplyWithoutReplay {
    fn identity(&self) -> &str {
        self.inner.identity()
    }
    fn capabilities(&self) -> AdapterCapabilities {
        capabilities(false)
    }
    fn deliver(
        &mut self,
        record: &agent_harness::outbox::DeliveryRecord,
    ) -> anyhow::Result<DeliveryOutcome> {
        self.inner.deliver(record)?;
        anyhow::bail!("SECRET-ERROR-CANARY: reply dropped after actual backend commit")
    }
    fn reconcile(
        &mut self,
        record: &agent_harness::outbox::DeliveryRecord,
    ) -> anyhow::Result<Reconciliation> {
        self.inner.reconcile(record)
    }
}

#[test]
fn uncertain_actual_effect_requires_independent_evidence_and_error_text_is_redacted() {
    let (root, store) = fixture();
    let backend = root.path().join("backend");
    let adapter = LostReplyWithoutReplay {
        inner: LocalJournalAdapter::open(&backend, "journal").unwrap(),
    };
    let mut worker = OutboxWorker::new(store.clone(), policy(), adapter).unwrap();
    assert_eq!(worker.tick(1000, 1).unwrap().unknown, 1);
    assert_eq!(
        LocalJournalAdapter::open(&backend, "journal")
            .unwrap()
            .effect_count()
            .unwrap(),
        1
    );
    assert_eq!(worker.tick(2000, 1).unwrap().claimed, 0);
    let record = store.outbox_deliveries("journal").unwrap().remove(0);
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("SECRET-ERROR-CANARY"));
    assert!(worker.reconcile(record.event.seq, 2001).unwrap());
    assert_eq!(
        store.outbox_deliveries("journal").unwrap()[0].state,
        DeliveryState::Delivered
    );
    assert_eq!(
        LocalJournalAdapter::open(&backend, "journal")
            .unwrap()
            .effect_count()
            .unwrap(),
        1
    );
}

#[test]
fn no_effect_evidence_is_bound_to_exact_identity_and_expired_attempt_fence() {
    let (_root, store) = fixture();
    store
        .register_outbox_handler(&policy(), capabilities(false))
        .unwrap();
    let first = store.claim_outbox("journal", 1000).unwrap().unwrap();
    store.begin_outbox_effect(&first, 1000).unwrap();
    store.claim_outbox("journal", 1200).unwrap();
    let unknown = store.outbox_deliveries("journal").unwrap().remove(0);
    let mut wrong = no_effect(&unknown);
    wrong.attempt_fence += 1;
    assert!(store
        .reconcile_outbox(&unknown, Reconciliation::NoEffect(wrong), 1201)
        .is_err());
    assert_eq!(
        store.outbox_deliveries("journal").unwrap()[0].state,
        DeliveryState::Unknown
    );
    let mut wrong = no_effect(&unknown);
    wrong.idempotency_key = "another-effect".into();
    assert!(store
        .reconcile_outbox(&unknown, Reconciliation::NoEffect(wrong), 1202)
        .is_err());
    let proof = no_effect(&unknown);
    assert!(store
        .reconcile_outbox(&unknown, Reconciliation::NoEffect(proof.clone()), 1203)
        .unwrap());
    let settled = store.outbox_deliveries("journal").unwrap().remove(0);
    assert_eq!(settled.no_effect_receipt, Some(proof));
}
