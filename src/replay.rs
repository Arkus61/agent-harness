//! Historical journal inspection. No providers, commands or event adapters are invoked.
use crate::{
    context,
    storage::Store,
    types::{hash, Event, RunRecord, RunState},
};
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize)]
pub struct RecordedEvent {
    #[serde(flatten)]
    pub event: Event,
    pub generation: u64,
}

#[derive(Debug, Serialize)]
pub struct ReplaySnapshot {
    pub origin: String,
    pub dispatched_effects: u64,
    pub run: Value,
    pub events: Vec<RecordedEvent>,
    pub journal_hash: String,
    pub projection_consistent: bool,
    pub limitations: Vec<String>,
}

pub fn snapshot(store: &Store, run_id: &str) -> Result<ReplaySnapshot> {
    ensure!(
        store.is_read_only(),
        "historical replay requires a read-only store"
    );
    let mut db = store.connection()?;
    // One read transaction binds the persisted run and journal to the same snapshot.
    let tx = db.transaction()?;
    let body: String = tx.query_row("SELECT record FROM runs WHERE id=?1", [run_id], |r| {
        r.get(0)
    })?;
    let run: RunRecord = serde_json::from_str(&body)?;
    let mut stmt = tx.prepare(
        "SELECT seq,kind,payload,created_at,generation FROM events WHERE run_id=?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([run_id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, kind, payload, created_at, generation) = row?;
        ensure!(seq > 0 && generation > 0, "invalid recorded event identity");
        events.push(RecordedEvent {
            event: Event {
                seq,
                run_id: run_id.into(),
                kind,
                payload: serde_json::from_str(&payload)?,
                created_at,
            },
            generation: generation as u64,
        });
    }
    drop(stmt);
    tx.commit()?;
    let journal_hash = hash(&json!({"run":run,"events":events}))?;
    let mut state = RunState::Received;
    let mut generation = 1;
    let mut candidate: Option<String> = None;
    let mut consistent = events.first().is_some_and(|record| {
        record.event.kind == "run_created"
            && record.event.payload["task_hash"] == run.task_hash
            && record.event.payload["base_sha"] == run.base_sha
    }) && hash(&run.task)? == run.task_hash;
    for record in &events {
        let payload = &record.event.payload;
        match record.event.kind.as_str() {
            "state_changed" => {
                consistent &= payload["from"] == serde_json::to_value(state)?;
                state = serde_json::from_value(payload["to"].clone())?;
            }
            "cancelled" => state = RunState::Cancelled,
            "generation_advanced" => generation = payload["generation"].as_u64().unwrap_or(0),
            "candidate_set" => candidate = payload["sha"].as_str().map(str::to_owned),
            _ => {}
        }
        consistent &= record.generation == generation;
    }
    consistent &=
        state == run.state && generation == run.generation && candidate == run.candidate_sha;
    for record in &mut events {
        record.event.payload = context::redact_value(&record.event.payload, &run.task.secrets);
    }
    Ok(ReplaySnapshot {
        origin: "historical_journal".into(), dispatched_effects: 0,
        run: context::redact_value(&serde_json::to_value(&run)?, &run.task.secrets), events,
        journal_hash, projection_consistent: consistent,
        limitations: vec![
            "Historical evidence is not a new verification of the candidate or model quality".into(),
            "Checks state/candidate/generation projections; does not replay effects or reconstruct all usage, attempts and reviews".into(),
            "Journal hash identifies this snapshot; no authenticated event chain or proof against coordinated database tampering".into(),
            "SQLite read-only WAL access can use shared-memory sidecars; no database writes, migration or CAS publication are permitted".into(),
        ],
    })
}
