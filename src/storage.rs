//! Durable local state. Every mutable projection and its event/outbox entry are
//! committed together; a persisted intent must precede an external effect.
use crate::types::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}
struct Inner {
    root: PathBuf,
    db: Mutex<Connection>,
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}
fn decode<T: DeserializeOwned>(value: String) -> Result<T> {
    Ok(serde_json::from_str(&value)?)
}
fn run_tx(tx: &Transaction<'_>, id: &str) -> Result<RunRecord> {
    let body: Option<String> = tx
        .query_row("SELECT record FROM runs WHERE id=?1", [id], |r| r.get(0))
        .optional()?;
    decode(body.ok_or_else(|| anyhow!("unknown run {id}"))?)
}
fn save_run(tx: &Transaction<'_>, run: &mut RunRecord) -> Result<()> {
    run.updated_at = now();
    ensure!(
        tx.execute(
            "UPDATE runs SET record=?2 WHERE id=?1",
            params![run.id, encode(run)?]
        )? == 1,
        "unknown run"
    );
    Ok(())
}
fn append_event(tx: &Transaction<'_>, run_id: &str, kind: &str, payload: Value) -> Result<Event> {
    let created_at = now();
    let generation = run_tx(tx, run_id)?.generation;
    tx.execute(
        "INSERT INTO events(run_id,kind,payload,created_at,generation) VALUES(?1,?2,?3,?4,?5)",
        params![
            run_id,
            kind,
            encode(&payload)?,
            created_at,
            sqlite_u64(generation)?
        ],
    )?;
    let seq = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO outbox(event_seq,run_id,state) VALUES(?1,?2,'pending')",
        params![seq, run_id],
    )?;
    Ok(Event {
        seq,
        run_id: run_id.into(),
        kind: kind.into(),
        payload,
        created_at,
    })
}
fn mutable_run(run: &RunRecord) -> Result<()> {
    ensure!(
        !run.cancelled && !run.state.terminal(),
        "run {} is terminal ({:?})",
        run.id,
        run.state
    );
    Ok(())
}
fn immutable_run(run: &RunRecord) -> bool {
    matches!(run.state, RunState::Verified | RunState::FixtureVerified)
}
fn transition(from: RunState, to: RunState, resume_permitted: bool) -> bool {
    use RunState::*;
    if from == to {
        return true;
    }
    if matches!(from, Verified | FixtureVerified | Cancelled) {
        return false;
    }
    if to == Planning && resume_permitted {
        return true;
    }
    if matches!(from, Blocked | Failed | BudgetExhausted) {
        return to == Planning && resume_permitted;
    }
    if matches!(to, Blocked | Failed | Cancelled | BudgetExhausted) {
        return true;
    }
    matches!(
        (from, to),
        (Received, Planning)
            | (Planning, Executing)
            | (Executing, Integrating)
            | (Executing, Repairing)
            | (Integrating, Verifying)
            | (Integrating, Repairing)
            | (Verifying, Verified)
            | (Verifying, FixtureVerified)
            | (Verifying, Repairing)
            | (Repairing, Planning)
            | (Repairing, Executing)
            | (Repairing, Integrating)
    )
}
fn sum_usage(usage: &Usage) -> Result<u64> {
    usage
        .input_tokens
        .checked_add(usage.output_tokens)
        .ok_or_else(|| anyhow!("usage overflow"))
}
fn sqlite_u64(value: u64) -> Result<i64> {
    i64::try_from(value).context("value exceeds SQLite integer range")
}

#[cfg(windows)]
fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
fn plain_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !is_link(&metadata),
        "state directory is not a plain directory: {}",
        path.display()
    );
    Ok(())
}
/// Only the supplied directory is tightened. Existing ancestors are untouched.
fn private_directory(path: &Path, recursive: bool) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    plain_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn private_file(path: &Path, create: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "state file is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl Store {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        private_directory(root.as_ref(), true).context("create private state directory")?;
        let root = fs::canonicalize(root.as_ref()).context("resolve state directory")?;
        ensure!(
            root.to_str().is_some(),
            "state directory must have a UTF-8 path"
        );
        let objects = root.join("objects");
        private_directory(&objects, false)?;
        // Tighten existing CAS prefix directories when opening an older store.
        for entry in fs::read_dir(&objects)? {
            let entry = entry?;
            let metadata = entry.path().symlink_metadata()?;
            ensure!(!is_link(&metadata), "CAS prefix cannot be a link");
            if metadata.is_dir() {
                private_directory(&entry.path(), false)?;
            }
        }
        #[cfg(unix)]
        {
            fs::File::open(&root)?.sync_all()?;
        }
        let db_path = root.join("state.sqlite3");
        private_file(&db_path, true)?;
        for name in ["state.sqlite3-wal", "state.sqlite3-shm"] {
            let path = root.join(name);
            match path.symlink_metadata() {
                Ok(_) => private_file(&path, false)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut db = Connection::open_with_flags(
            db_path,
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_secs(15))?;
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        ensure!(
            mode.eq_ignore_ascii_case("wal"),
            "SQLite WAL is unavailable on this filesystem"
        );
        db.pragma_update(None, "synchronous", "FULL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(version <= 2, "state schema is newer than this harness");
        tx.execute_batch("CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY,record TEXT NOT NULL,resume_permitted INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL REFERENCES runs(id),kind TEXT NOT NULL,payload TEXT NOT NULL,created_at TEXT NOT NULL,generation INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS events_run ON events(run_id,seq);
            CREATE TABLE IF NOT EXISTS outbox(event_seq INTEGER PRIMARY KEY REFERENCES events(seq),run_id TEXT NOT NULL REFERENCES runs(id),state TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS actions(id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id),generation INTEGER NOT NULL,record TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS actions_run ON actions(run_id);
            CREATE TABLE IF NOT EXISTS attempts(id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id),record TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS attempts_run ON attempts(run_id);
            CREATE TABLE IF NOT EXISTS reservations(run_id TEXT NOT NULL REFERENCES runs(id),call_id TEXT NOT NULL,amount INTEGER NOT NULL CHECK(amount>=0),status TEXT NOT NULL,usage TEXT,PRIMARY KEY(run_id,call_id));
            CREATE TABLE IF NOT EXISTS checks(run_id TEXT NOT NULL REFERENCES runs(id),ordinal INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(run_id,ordinal));
            CREATE TABLE IF NOT EXISTS reviews(run_id TEXT NOT NULL REFERENCES runs(id),ordinal INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(run_id,ordinal));
            CREATE TABLE IF NOT EXISTS hook_inbox(run_id TEXT NOT NULL REFERENCES runs(id),event_seq INTEGER NOT NULL REFERENCES events(seq),handler TEXT NOT NULL,generation INTEGER NOT NULL,PRIMARY KEY(run_id,event_seq,handler,generation));")?;
        if version == 1 {
            tx.execute_batch(
                "ALTER TABLE events ADD COLUMN generation INTEGER NOT NULL DEFAULT 1;",
            )?;
        }
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                db: Mutex::new(db),
            }),
        })
    }
    pub fn root(&self) -> &Path {
        &self.inner.root
    }
    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.inner
            .db
            .lock()
            .map_err(|_| anyhow!("state connection mutex poisoned"))
    }
    fn transaction<T>(&self, op: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut db = self.connection()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = op(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn create_run(
        &self,
        id: &str,
        repo: &Path,
        task: &TaskSpec,
        base: &str,
    ) -> Result<RunRecord> {
        task.validate()?;
        ensure!(
            !id.is_empty() && !base.is_empty(),
            "run ID and base SHA are required"
        );
        let repo = repo
            .to_str()
            .ok_or_else(|| anyhow!("repository must have a UTF-8 path"))?
            .to_owned();
        let stamp = now();
        let run = RunRecord {
            id: id.into(),
            repo,
            task: task.clone(),
            task_hash: hash(task)?,
            base_sha: base.into(),
            state: RunState::Received,
            generation: 1,
            spent_tokens: 0,
            reserved_tokens: 0,
            created_at: stamp.clone(),
            updated_at: stamp,
            cancelled: false,
            candidate_sha: None,
            error: None,
        };
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO runs(id,record) VALUES(?1,?2)",
                params![run.id, encode(&run)?],
            )?;
            append_event(
                tx,
                id,
                "run_created",
                json!({"task_hash":run.task_hash,"base_sha":base,"generation":1}),
            )?;
            Ok(run)
        })
    }
    pub fn get_run(&self, id: &str) -> Result<RunRecord> {
        let db = self.connection()?;
        let body: Option<String> = db
            .query_row("SELECT record FROM runs WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        decode(body.ok_or_else(|| anyhow!("unknown run {id}"))?)
    }
    pub fn list_runs(&self) -> Result<Vec<RunRecord>> {
        let db = self.connection()?;
        let mut stmt = db.prepare("SELECT record FROM runs ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| decode(r?)).collect()
    }
    pub fn set_state(&self, id: &str, state: RunState, error: Option<&str>) -> Result<()> {
        self.transaction(|tx| {
            let mut run = run_tx(tx, id)?;
            let permission: bool =
                tx.query_row("SELECT resume_permitted FROM runs WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
            ensure!(
                transition(run.state, state, permission),
                "illegal state transition {:?} -> {:?}",
                run.state,
                state
            );
            ensure!(
                !run.cancelled || state == RunState::Cancelled,
                "cancelled run cannot resume"
            );
            if immutable_run(&run) {
                ensure!(
                    error == run.error.as_deref(),
                    "verified result is immutable"
                );
                return Ok(());
            }
            let before = run.state;
            run.state = state;
            run.error = error.map(str::to_owned);
            if state == RunState::Cancelled {
                run.cancelled = true;
            }
            save_run(tx, &mut run)?;
            if state == RunState::Planning {
                tx.execute("UPDATE runs SET resume_permitted=0 WHERE id=?1", [id])?;
            }
            append_event(
                tx,
                id,
                "state_changed",
                json!({"from":before,"to":state,"error":error,"generation":run.generation}),
            )?;
            Ok(())
        })
    }
    pub fn advance_generation(&self, id: &str) -> Result<u64> {
        self.transaction(|tx| {
            let mut run = run_tx(tx, id)?;
            ensure!(
                !immutable_run(&run) && !run.cancelled,
                "verified/cancelled run cannot advance generation"
            );
            run.generation = run
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("generation overflow"))?;
            sqlite_u64(run.generation)?;
            save_run(tx, &mut run)?;
            tx.execute(
                "UPDATE runs SET resume_permitted=1 WHERE id=?1",
                params![id],
            )?;
            append_event(
                tx,
                id,
                "generation_advanced",
                json!({"generation":run.generation}),
            )?;
            Ok(run.generation)
        })
    }
    pub fn set_candidate(&self, id: &str, sha: &str) -> Result<()> {
        ensure!(!sha.is_empty(), "candidate SHA is empty");
        self.transaction(|tx| {
            let mut run = run_tx(tx, id)?;
            mutable_run(&run)?;
            run.candidate_sha = Some(sha.into());
            save_run(tx, &mut run)?;
            append_event(
                tx,
                id,
                "candidate_set",
                json!({"sha":sha,"generation":run.generation}),
            )?;
            Ok(())
        })
    }
    pub fn event(&self, run_id: &str, kind: &str, payload: Value) -> Result<Event> {
        ensure!(!kind.is_empty(), "event kind is empty");
        self.transaction(|tx| {
            run_tx(tx, run_id)?;
            append_event(tx, run_id, kind, payload)
        })
    }
    pub fn events(&self, run_id: &str) -> Result<Vec<Event>> {
        let db = self.connection()?;
        let mut stmt = db.prepare(
            "SELECT seq,kind,payload,created_at FROM events WHERE run_id=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([run_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (seq, kind, payload, created_at) = row?;
            Ok(Event {
                seq,
                run_id: run_id.into(),
                kind,
                payload: decode(payload)?,
                created_at,
            })
        })
        .collect()
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        self.transaction(|tx| {
            let mut run = run_tx(tx, id)?;
            ensure!(!immutable_run(&run), "verified result is immutable");
            if run.cancelled {
                return Ok(());
            }
            let before = run.state;
            run.cancelled = true;
            run.state = RunState::Cancelled;
            save_run(tx, &mut run)?;
            append_event(
                tx,
                id,
                "cancelled",
                json!({"from":before,"generation":run.generation}),
            )?;
            Ok(())
        })
    }
    pub fn reserve(&self, run_id: &str, call_id: &str, amount: u64) -> Result<()> {
        ensure!(!call_id.is_empty(), "call ID is empty");
        let amount_sql = sqlite_u64(amount)?;
        self.transaction(|tx| {
            let mut run=run_tx(tx,run_id)?;
            let old:Option<i64>=tx.query_row("SELECT amount FROM reservations WHERE run_id=?1 AND call_id=?2",params![run_id,call_id],|r|r.get(0)).optional()?;
            if let Some(old)=old { ensure!(old==amount_sql,"call ID already reserved with another amount");return Ok(()); }
            mutable_run(&run)?;
            let reserved=run.reserved_tokens.checked_add(amount).ok_or_else(||anyhow!("budget overflow"))?;
            let total=run.spent_tokens.checked_add(reserved).ok_or_else(||anyhow!("budget overflow"))?;
            ensure!(total<=run.task.budget.max_tokens,"token budget exhausted: {total} > {}",run.task.budget.max_tokens);
            tx.execute("INSERT INTO reservations(run_id,call_id,amount,status) VALUES(?1,?2,?3,'reserved')",params![run_id,call_id,amount_sql])?;
            run.reserved_tokens=reserved;save_run(tx,&mut run)?;
            append_event(tx,run_id,"budget_reserved",json!({"call_id":call_id,"amount":amount,"generation":run.generation}))?;Ok(())
        })
    }
    pub fn settle(&self, run_id: &str, call_id: &str, usage: &Usage) -> Result<()> {
        let actual = sum_usage(usage)?;
        let exceeded=self.transaction(|tx| {
            let mut run=run_tx(tx,run_id)?;
            let entry:Option<(i64,String,Option<String>)>=tx.query_row("SELECT amount,status,usage FROM reservations WHERE run_id=?1 AND call_id=?2",params![run_id,call_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let (amount,status,prior)=entry.ok_or_else(||anyhow!("unknown reservation {call_id}"))?;
            let encoded=encode(usage)?;
            if status=="settled" { ensure!(prior.as_deref()==Some(encoded.as_str()),"settled usage is immutable");return Ok(false); }
            if !usage.complete && prior.as_deref()==Some(encoded.as_str()) {return Ok(false);}
            let held=u64::try_from(amount)?;
            run.reserved_tokens=run.reserved_tokens.checked_sub(held).ok_or_else(||anyhow!("budget projection corrupted"))?;
            if usage.complete {
                run.spent_tokens=run.spent_tokens.checked_add(actual).ok_or_else(||anyhow!("budget overflow"))?;
                tx.execute("UPDATE reservations SET status='settled',usage=?3 WHERE run_id=?1 AND call_id=?2",params![run_id,call_id,encoded])?;
            } else {
                let remaining=held.max(actual);sqlite_u64(remaining)?;
                run.reserved_tokens=run.reserved_tokens.checked_add(remaining).ok_or_else(||anyhow!("budget overflow"))?;
                tx.execute("UPDATE reservations SET amount=?3,status='unknown',usage=?4 WHERE run_id=?1 AND call_id=?2",params![run_id,call_id,sqlite_u64(remaining)?,encoded])?;
            }
            let total=run.spent_tokens.checked_add(run.reserved_tokens).ok_or_else(||anyhow!("budget overflow"))?;
            let exceeded=total>run.task.budget.max_tokens;
            if exceeded && !run.state.terminal() {run.state=RunState::BudgetExhausted;run.error=Some("actual provider usage exceeded token budget".into());}
            save_run(tx,&mut run)?;
            append_event(tx,run_id,if usage.complete{"budget_settled"}else{"budget_usage_unknown"},json!({"call_id":call_id,"usage":usage,"spent_tokens":run.spent_tokens,"reserved_tokens":run.reserved_tokens,"budget_exceeded":exceeded}))?;
            Ok(exceeded)
        })?;
        ensure!(
            !exceeded,
            "actual usage exceeded root token budget (accounting persisted)"
        );
        Ok(())
    }
    pub fn intent(&self, run_id: &str, attempt: &str, action: &Action) -> Result<ActionRecord> {
        self.transaction(|tx| {
            let run=run_tx(tx,run_id)?;mutable_run(&run)?;
            let known_attempt:Option<String>=tx.query_row("SELECT record FROM attempts WHERE id=?1",[attempt],|r|r.get(0)).optional()?;
            if let Some(known_attempt)=known_attempt {let owner:AttemptRecord=decode(known_attempt)?;ensure!(owner.run_id==run_id && owner.generation==run.generation,"action belongs to a stale or foreign attempt");}
            let record=ActionRecord{id:id(),run_id:run_id.into(),attempt:attempt.into(),action_hash:hash(action)?,action:action.clone(),status:"intent".into(),result:None};
            tx.execute("INSERT INTO actions(id,run_id,generation,record) VALUES(?1,?2,?3,?4)",params![record.id,run_id,sqlite_u64(run.generation)?,encode(&record)?])?;
            append_event(tx,run_id,"action_intent",json!({"action_id":record.id,"attempt":attempt,"action_hash":record.action_hash,"generation":run.generation}))?;Ok(record)
        })
    }
    pub fn receipt(&self, action_id: &str, result: Value) -> Result<()> {
        self.transaction(|tx| {
            let body: Option<String> = tx
                .query_row("SELECT record FROM actions WHERE id=?1", [action_id], |r| {
                    r.get(0)
                })
                .optional()?;
            let mut action: ActionRecord =
                decode(body.ok_or_else(|| anyhow!("unknown action {action_id}"))?)?;
            if action.status == "completed" {
                ensure!(
                    action.result.as_ref() == Some(&result),
                    "completed receipt is immutable"
                );
                return Ok(());
            }
            ensure!(
                matches!(action.status.as_str(), "intent" | "unknown"),
                "action was already reconciled as {}",
                action.status
            );
            action.status = "completed".into();
            action.result = Some(result);
            tx.execute(
                "UPDATE actions SET record=?2 WHERE id=?1",
                params![action_id, encode(&action)?],
            )?;
            append_event(
                tx,
                &action.run_id,
                "action_receipt",
                json!({"action_id":action_id,"result_hash":hash(&action.result)?}),
            )?;
            Ok(())
        })
    }
    pub fn reconcile_action(&self, action_id: &str, status: &str) -> Result<()> {
        ensure!(
            matches!(status, "unknown" | "not_executed" | "failed"),
            "completed reconciliation requires a receipt"
        );
        self.transaction(|tx| {
            let body: Option<String> = tx
                .query_row("SELECT record FROM actions WHERE id=?1", [action_id], |r| {
                    r.get(0)
                })
                .optional()?;
            let mut action: ActionRecord =
                decode(body.ok_or_else(|| anyhow!("unknown action {action_id}"))?)?;
            if action.status == status {
                return Ok(());
            }
            ensure!(
                matches!(action.status.as_str(), "intent" | "unknown"),
                "final action cannot be reconciled again"
            );
            action.status = status.into();
            tx.execute(
                "UPDATE actions SET record=?2 WHERE id=?1",
                params![action_id, encode(&action)?],
            )?;
            append_event(
                tx,
                &action.run_id,
                "action_reconciled",
                json!({"action_id":action_id,"status":status}),
            )?;
            Ok(())
        })
    }
    pub fn pending_actions(&self, run_id: &str) -> Result<Vec<ActionRecord>> {
        Ok(self
            .actions(run_id)?
            .into_iter()
            .filter(|a| matches!(a.status.as_str(), "intent" | "unknown"))
            .collect())
    }
    /// Includes completed effects, so recovery can distinguish a known effect
    /// from an action that is safe to start for the first time.
    pub fn actions(&self, run_id: &str) -> Result<Vec<ActionRecord>> {
        let db = self.connection()?;
        let mut stmt = db.prepare("SELECT record FROM actions WHERE run_id=?1 ORDER BY rowid")?;
        let rows = stmt.query_map([run_id], |r| r.get::<_, String>(0))?;
        let all: Vec<ActionRecord> = rows.map(|r| decode(r?)).collect::<Result<_>>()?;
        Ok(all)
    }
    pub fn put_attempt(&self, attempt: &AttemptRecord) -> Result<()> {
        self.transaction(|tx| {
            let run=run_tx(tx,&attempt.run_id)?;mutable_run(&run)?;
            ensure!(attempt.generation==run.generation,"stale attempt generation");
            let old:Option<String>=tx.query_row("SELECT record FROM attempts WHERE id=?1",[&attempt.id],|r|r.get(0)).optional()?;
            let body=encode(attempt)?;
            if let Some(old)=old {ensure!(old==body,"attempt ID already exists");return Ok(());}
            tx.execute("INSERT INTO attempts(id,run_id,record) VALUES(?1,?2,?3)",params![attempt.id,attempt.run_id,body])?;
            append_event(tx,&attempt.run_id,"attempt_created",json!({"attempt_id":attempt.id,"node_id":attempt.node_id,"generation":attempt.generation,"input_sha":attempt.input_sha}))?;Ok(())
        })
    }
    pub fn finish_attempt(&self, id: &str, generation: u64, output_sha: &str) -> Result<()> {
        ensure!(!output_sha.is_empty(), "output SHA is empty");
        self.transaction(|tx| {
            let body: Option<String> = tx
                .query_row("SELECT record FROM attempts WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .optional()?;
            let mut attempt: AttemptRecord =
                decode(body.ok_or_else(|| anyhow!("unknown attempt {id}"))?)?;
            let run = run_tx(tx, &attempt.run_id)?;
            mutable_run(&run)?;
            ensure!(
                generation == run.generation && generation == attempt.generation,
                "stale attempt result"
            );
            if attempt.status == "completed" {
                ensure!(
                    attempt.output_sha.as_deref() == Some(output_sha),
                    "completed output is immutable"
                );
                return Ok(());
            }
            attempt.output_sha = Some(output_sha.into());
            attempt.status = "completed".into();
            tx.execute(
                "UPDATE attempts SET record=?2 WHERE id=?1",
                params![id, encode(&attempt)?],
            )?;
            append_event(
                tx,
                &attempt.run_id,
                "attempt_completed",
                json!({"attempt_id":id,"generation":generation,"output_sha":output_sha}),
            )?;
            Ok(())
        })
    }
    pub fn attempts(&self, run_id: &str) -> Result<Vec<AttemptRecord>> {
        self.records("attempts", run_id)
    }
    fn records<T: DeserializeOwned>(&self, table: &str, run_id: &str) -> Result<Vec<T>> {
        // `table` is selected only by this module, never user input.
        let db = self.connection()?;
        let mut stmt = db.prepare(&format!(
            "SELECT record FROM {table} WHERE run_id=?1 ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([run_id], |r| r.get::<_, String>(0))?;
        rows.map(|r| decode(r?)).collect()
    }
    fn save_results<T: Serialize>(&self, run_id: &str, table: &str, results: &[T]) -> Result<()> {
        self.transaction(|tx| {let run=run_tx(tx,run_id)?;mutable_run(&run)?;let start:i64=tx.query_row(&format!("SELECT COALESCE(MAX(ordinal)+1,0) FROM {table} WHERE run_id=?1"),[run_id],|r|r.get(0))?;for (index,result) in results.iter().enumerate(){tx.execute(&format!("INSERT INTO {table}(run_id,ordinal,record) VALUES(?1,?2,?3)"),params![run_id,start+i64::try_from(index)?,encode(result)?])?;}append_event(tx,run_id,&format!("{table}_saved"),json!({"count":results.len(),"generation":run.generation,"results_hash":hash(&results)?}))?;Ok(())})
    }
    pub fn save_checks(&self, run_id: &str, checks: &[CheckResult]) -> Result<()> {
        self.save_results(run_id, "checks", checks)
    }
    pub fn checks(&self, run_id: &str) -> Result<Vec<CheckResult>> {
        self.records("checks", run_id)
    }
    pub fn save_reviews(&self, run_id: &str, reviews: &[ReviewResult]) -> Result<()> {
        self.save_results(run_id, "reviews", reviews)
    }
    pub fn reviews(&self, run_id: &str) -> Result<Vec<ReviewResult>> {
        self.records("reviews", run_id)
    }
    pub fn claim_hook(
        &self,
        run_id: &str,
        event_seq: i64,
        handler: &str,
        generation: u64,
    ) -> Result<bool> {
        ensure!(!handler.is_empty(), "hook handler is empty");
        self.transaction(|tx| {let run=run_tx(tx,run_id)?;if generation!=run.generation || run.cancelled {return Ok(false);}let event_generation:Option<i64>=tx.query_row("SELECT generation FROM events WHERE seq=?1 AND run_id=?2",params![event_seq,run_id],|r|r.get(0)).optional()?;let event_generation=event_generation.ok_or_else(||anyhow!("hook event does not belong to run"))?;if event_generation!=sqlite_u64(generation)?{return Ok(false);}let added=tx.execute("INSERT OR IGNORE INTO hook_inbox(run_id,event_seq,handler,generation) VALUES(?1,?2,?3,?4)",params![run_id,event_seq,handler,sqlite_u64(generation)?])?;if added==1{append_event(tx,run_id,"hook_claimed",json!({"event_seq":event_seq,"handler":handler,"generation":generation}))?;}Ok(added==1)})
    }
    pub fn put_object(&self, bytes: &[u8]) -> Result<String> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let target = self.object_path(&hash)?;
        let parent = target.parent().expect("object parent");
        plain_directory(self.root())?;
        plain_directory(&self.root().join("objects"))?;
        private_directory(parent, false)?;
        #[cfg(unix)]
        {
            fs::File::open(self.root().join("objects"))?.sync_all()?;
        }
        if target.exists() {
            self.get_object(&hash)?;
            return Ok(hash);
        }
        let staged = parent.join(format!(".{}.tmp", id()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&staged)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            match fs::hard_link(&staged, &target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.get_object(&hash)?;
                }
                Err(error) => {
                    bail!("atomic object publication failed: {error}");
                }
            }
            #[cfg(unix)]
            {
                fs::File::open(parent)?.sync_all()?;
            }
            Ok(())
        })();
        let cleanup = fs::remove_file(&staged);
        result?;
        cleanup?;
        Ok(hash)
    }
    fn object_path(&self, hash: &str) -> Result<PathBuf> {
        ensure!(
            hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_hexdigit())
                && hash == hash.to_ascii_lowercase(),
            "invalid BLAKE3 object hash"
        );
        Ok(self.root().join("objects").join(&hash[..2]).join(hash))
    }
    pub fn get_object(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.object_path(hash)?;
        plain_directory(self.root())?;
        plain_directory(&self.root().join("objects"))?;
        plain_directory(path.parent().expect("object parent"))?;
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() && !is_link(&metadata),
            "object is not a regular file"
        );
        let bytes = fs::read(path)?;
        ensure!(
            blake3::hash(&bytes).to_hex().as_str() == hash,
            "object hash mismatch: {hash}"
        );
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    fn task() -> TaskSpec {
        serde_json::from_value(json!({"prompt":"change a file","requirements":["correct result"],"grants":{"write":["src/**"]},"checks":[{"program":"helper"}],"provider":{},"budget":{"max_tokens":100,"max_output_tokens":50,"deadline_secs":30}})).unwrap()
    }
    fn fixture() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .create_run("run", dir.path(), &task(), "base")
            .unwrap();
        (dir, store)
    }
    #[test]
    fn projections_events_and_outbox_survive_reopen() {
        let (dir, store) = fixture();
        store.set_state("run", RunState::Planning, None).unwrap();
        store.set_state("run", RunState::Executing, None).unwrap();
        let before = store.events("run").unwrap();
        drop(store);
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(reopened.get_run("run").unwrap().state, RunState::Executing);
        assert_eq!(reopened.events("run").unwrap().len(), before.len());
        let db = reopened.connection().unwrap();
        let events: i64 = db
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let outbox: i64 = db
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, outbox);
        assert_eq!(
            db.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn event_failure_rolls_back_projection_and_outbox() {
        let (_dir, store) = fixture();
        store.connection().unwrap().execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON events WHEN NEW.kind='state_changed' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
        let before = store.events("run").unwrap().len();
        assert!(store.set_state("run", RunState::Planning, None).is_err());
        assert_eq!(store.get_run("run").unwrap().state, RunState::Received);
        assert_eq!(store.events("run").unwrap().len(), before);
        let outbox: i64 = store
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(outbox as usize, before);
    }
    #[test]
    fn parallel_reservations_never_exceed_root_budget() {
        let (dir, store) = fixture();
        let path = dir.path().to_path_buf();
        let jobs: Vec<_> = (0..20)
            .map(|i| {
                let path = path.clone();
                thread::spawn(move || {
                    Store::open(path)
                        .unwrap()
                        .reserve("run", &format!("call-{i}"), 10)
                        .is_ok()
                })
            })
            .collect();
        let success = jobs
            .into_iter()
            .map(|job| job.join().unwrap())
            .filter(|success| *success)
            .count();
        assert_eq!(success, 10);
        assert_eq!(store.get_run("run").unwrap().reserved_tokens, 100);
    }
    #[test]
    fn unknown_usage_retains_reservation_and_final_settlement_is_idempotent() {
        let (dir, store) = fixture();
        store.reserve("run", "call", 40).unwrap();
        store
            .settle(
                "run",
                "call",
                &Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    complete: false,
                },
            )
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.get_run("run").unwrap().reserved_tokens, 40);
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            complete: true,
        };
        store.settle("run", "call", &usage).unwrap();
        store.settle("run", "call", &usage).unwrap();
        let run = store.get_run("run").unwrap();
        assert_eq!((run.spent_tokens, run.reserved_tokens), (15, 0));
        assert!(store
            .settle(
                "run",
                "call",
                &Usage {
                    input_tokens: 1,
                    output_tokens: 0,
                    complete: true
                }
            )
            .is_err());
    }
    #[test]
    fn actual_budget_overshoot_is_persisted_even_when_settle_fails() {
        let (_dir, store) = fixture();
        store.reserve("run", "call", 50).unwrap();
        assert!(store
            .settle(
                "run",
                "call",
                &Usage {
                    input_tokens: 90,
                    output_tokens: 20,
                    complete: true
                }
            )
            .is_err());
        let run = store.get_run("run").unwrap();
        assert_eq!(run.spent_tokens, 110);
        assert_eq!(run.state, RunState::BudgetExhausted);
    }
    #[test]
    fn action_effect_without_receipt_remains_pending_after_crash() {
        let (dir, store) = fixture();
        let intent = store
            .intent(
                "run",
                "attempt",
                &Action::RunCommand {
                    program: "helper".into(),
                    args: vec![],
                },
            )
            .unwrap();
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        let pending = store.pending_actions("run").unwrap();
        assert_eq!(pending[0].id, intent.id);
        store.reconcile_action(&intent.id, "unknown").unwrap();
        assert_eq!(store.pending_actions("run").unwrap().len(), 1);
        store
            .receipt(&intent.id, json!({"external_effect":"observed"}))
            .unwrap();
        assert!(store.pending_actions("run").unwrap().is_empty());
        assert!(store
            .receipt(&intent.id, json!({"external_effect":"different"}))
            .is_err());
        let actions = store.actions("run").unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].status, "completed");
        assert_eq!(actions[0].id, intent.id);
    }
    #[test]
    fn stale_and_cancelled_attempts_cannot_publish_output() {
        let (_dir, store) = fixture();
        let attempt = AttemptRecord {
            id: "attempt".into(),
            run_id: "run".into(),
            node_id: "node".into(),
            generation: 1,
            input_sha: "base".into(),
            output_sha: None,
            worktree: "tree".into(),
            status: "running".into(),
        };
        store.put_attempt(&attempt).unwrap();
        store.advance_generation("run").unwrap();
        assert!(store.finish_attempt("attempt", 1, "output").is_err());
        let mut next = attempt.clone();
        next.id = "next".into();
        next.generation = 2;
        store.put_attempt(&next).unwrap();
        store.cancel("run").unwrap();
        assert!(store.finish_attempt("next", 2, "output").is_err());
        assert!(store.advance_generation("run").is_err());
    }
    #[test]
    fn hook_inbox_deduplicates_and_rejects_old_generation() {
        let (_dir, store) = fixture();
        let event = store.event("run", "failed_check", json!({})).unwrap();
        assert!(store.claim_hook("run", event.seq, "repair", 1).unwrap());
        assert!(!store.claim_hook("run", event.seq, "repair", 1).unwrap());
        store.advance_generation("run").unwrap();
        assert!(!store.claim_hook("run", event.seq, "repair", 1).unwrap());
        assert!(!store.claim_hook("run", event.seq, "repair", 2).unwrap());
        let next = store.event("run", "new_failure", json!({})).unwrap();
        assert!(store.claim_hook("run", next.seq, "repair", 2).unwrap());
    }
    #[test]
    fn state_transitions_fail_closed_and_verified_is_immutable() {
        let (_dir, store) = fixture();
        assert!(store.set_state("run", RunState::Verified, None).is_err());
        store
            .set_state("run", RunState::Blocked, Some("unavailable"))
            .unwrap();
        assert!(store.set_state("run", RunState::Planning, None).is_err());
        store.advance_generation("run").unwrap();
        store.set_state("run", RunState::Planning, None).unwrap();
        for state in [
            RunState::Executing,
            RunState::Integrating,
            RunState::Verifying,
            RunState::Verified,
        ] {
            store.set_state("run", state, None).unwrap();
        }
        assert!(store.set_candidate("run", "new").is_err());
        assert!(store
            .set_state("run", RunState::Failed, Some("oops"))
            .is_err());
        assert!(store.cancel("run").is_err());
    }
    #[test]
    fn cas_is_concurrent_atomic_and_checks_corruption() {
        let (_dir, store) = fixture();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                thread::spawn(move || store.put_object(b"same immutable payload").unwrap())
            })
            .collect();
        let hashes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(hashes.iter().all(|h| h == &hashes[0]));
        assert_eq!(
            store.get_object(&hashes[0]).unwrap(),
            b"same immutable payload"
        );
        fs::write(store.object_path(&hashes[0]).unwrap(), b"tampered").unwrap();
        assert!(store.get_object(&hashes[0]).is_err());
        assert!(store.get_object("../../secret").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn private_permissions_do_not_change_existing_parent() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let state = parent.path().join("state");
        let store = Store::open(&state).unwrap();
        let hash = store.put_object(b"private source evidence").unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(parent.path()), 0o755);
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&state.join("objects")), 0o700);
        assert_eq!(mode(&state.join("objects").join(&hash[..2])), 0o700);
        assert_eq!(mode(&state.join("state.sqlite3")), 0o600);
        assert_eq!(mode(&store.object_path(&hash).unwrap()), 0o600);
        for name in ["state.sqlite3-wal", "state.sqlite3-shm"] {
            let path = state.join(name);
            if path.exists() {
                assert_eq!(mode(&path), 0o600);
            }
        }
        drop(store);
        fs::set_permissions(&state, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(
            state.join("state.sqlite3"),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        let _reopened = Store::open(&state).unwrap();
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&state.join("state.sqlite3")), 0o600);
        assert_eq!(mode(parent.path()), 0o755);
    }
    #[cfg(unix)]
    #[test]
    fn linked_state_and_cas_directories_are_rejected_without_touching_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let parent = tempfile::tempdir().unwrap();
        let foreign = parent.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::set_permissions(&foreign, fs::Permissions::from_mode(0o755)).unwrap();
        let linked = parent.path().join("linked-state");
        symlink(&foreign, &linked).unwrap();
        assert!(Store::open(&linked).is_err());
        assert!(!foreign.join("objects").exists());
        let state = parent.path().join("state");
        fs::create_dir(&state).unwrap();
        symlink(&foreign, state.join("objects")).unwrap();
        assert!(Store::open(&state).is_err());
        assert_eq!(
            fs::metadata(&foreign).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let clean = parent.path().join("clean");
        let store = Store::open(&clean).unwrap();
        let hash = blake3::hash(b"payload").to_hex().to_string();
        symlink(&foreign, clean.join("objects").join(&hash[..2])).unwrap();
        assert!(store.put_object(b"payload").is_err());
        assert!(!foreign.join(&hash).exists());
        assert!(Store::open(&clean).is_err());
        assert_eq!(
            fs::metadata(&foreign).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
