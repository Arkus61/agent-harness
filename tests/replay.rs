use agent_harness::{replay, storage::Store, types::TaskSpec};
use serde_json::json;

#[test]
fn absent_read_only_store_is_not_created() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("absent");
    assert!(Store::open_read_only(&absent).is_err());
    assert!(!absent.exists());
}

#[test]
fn historical_snapshot_is_deterministic_redacted_and_cannot_write_or_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let task: TaskSpec = serde_json::from_value(json!({
        "prompt":"preserve source", "requirements":["preserve"],
        "grants":{"read":["src/**"],"write":["src/**"]},
        "checks":[{"program":"never-dispatch-this"}], "provider":{},
        "secrets":["REPLAY_SYNTHETIC_SECRET"]
    }))
    .unwrap();
    store.create_run("run", root.path(), &task, "base").unwrap();
    store
        .event("run", "observed", json!({"data":"REPLAY_SYNTHETIC_SECRET"}))
        .unwrap();
    let cas = store.put_object(b"immutable evidence").unwrap();
    drop(store);
    let before = std::fs::read(root.path().join("state.sqlite3")).unwrap();
    let reader = Store::open_read_only(root.path()).unwrap();
    let first = replay::snapshot(&reader, "run").unwrap();
    let second = replay::snapshot(&reader, "run").unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
    let text = serde_json::to_string(&first).unwrap();
    assert!(!text.contains("REPLAY_SYNTHETIC_SECRET"));
    assert_eq!(first.origin, "historical_journal");
    assert_eq!(first.dispatched_effects, 0);
    assert_eq!(first.events.len(), 2);
    assert!(first.projection_consistent);
    assert!(reader.event("run", "must-not-write", json!({})).is_err());
    assert!(reader.put_object(b"must-not-create-CAS").is_err());
    assert_eq!(reader.get_object(&cas).unwrap(), b"immutable evidence");
    drop(reader);
    assert_eq!(
        std::fs::read(root.path().join("state.sqlite3")).unwrap(),
        before
    );
}

#[test]
fn newer_schema_is_rejected_without_migration() {
    let root = tempfile::tempdir().unwrap();
    drop(Store::open(root.path()).unwrap());
    let db = rusqlite::Connection::open(root.path().join("state.sqlite3")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let before = std::fs::read(root.path().join("state.sqlite3")).unwrap();
    assert!(Store::open_read_only(root.path()).is_err());
    assert_eq!(
        std::fs::read(root.path().join("state.sqlite3")).unwrap(),
        before
    );
}

#[test]
fn replay_reads_live_wal_and_detects_projection_tampering() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let task: TaskSpec = serde_json::from_value(json!({
        "prompt":"preserve", "requirements":["preserve"],
        "grants":{"read":["src/**"],"write":["src/**"]},
        "checks":[{"program":"never-dispatch"}], "provider":{}
    }))
    .unwrap();
    store.create_run("run", root.path(), &task, "base").unwrap();
    let reader = Store::open_read_only(root.path()).unwrap();
    let first = replay::snapshot(&reader, "run").unwrap();
    store.cancel("run").unwrap();
    let cancelled = replay::snapshot(&reader, "run").unwrap();
    assert!(cancelled.projection_consistent);
    assert_ne!(first.journal_hash, cancelled.journal_hash);
    let db = rusqlite::Connection::open(root.path().join("state.sqlite3")).unwrap();
    db.execute(
        "UPDATE events SET kind='observed' WHERE run_id='run' AND kind='cancelled'",
        [],
    )
    .unwrap();
    assert!(
        !replay::snapshot(&reader, "run")
            .unwrap()
            .projection_consistent
    );
}
