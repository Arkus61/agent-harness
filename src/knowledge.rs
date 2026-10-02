//! Project-scoped, evidence-backed local memory. A repeated claim is never a proof.
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{path::Path, str::FromStr, sync::Mutex, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Decision,
    Episode,
    Failure,
    Procedure,
    Constraint,
    SecurityAssumption,
    Preference,
}
impl MemoryKind {
    fn name(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Episode => "episode",
            Self::Failure => "failure",
            Self::Procedure => "procedure",
            Self::Constraint => "constraint",
            Self::SecurityAssumption => "security_assumption",
            Self::Preference => "preference",
        }
    }
}
impl FromStr for MemoryKind {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "fact" => Ok(Self::Fact),
            "decision" => Ok(Self::Decision),
            "episode" | "episodic" => Ok(Self::Episode),
            "failure" => Ok(Self::Failure),
            "procedure" | "procedural" => Ok(Self::Procedure),
            "constraint" => Ok(Self::Constraint),
            "security_assumption" | "security" => Ok(Self::SecurityAssumption),
            "preference" => Ok(Self::Preference),
            _ => bail!("unknown memory kind: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Candidate,
    Active,
    Contested,
    Stale,
    Superseded,
    Archived,
    Rejected,
}
impl MemoryStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Contested => "contested",
            Self::Stale => "stale",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
            Self::Rejected => "rejected",
        }
    }
}
impl FromStr for MemoryStatus {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "active" => Ok(Self::Active),
            "contested" => Ok(Self::Contested),
            "stale" => Ok(Self::Stale),
            "superseded" => Ok(Self::Superseded),
            "archived" => Ok(Self::Archived),
            "rejected" => Ok(Self::Rejected),
            _ => bail!("unknown memory status: {value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub project: String,
    pub kind: MemoryKind,
    pub topic: String,
    pub claim: String,
    pub evidence: String,
    pub source_hash: String,
    pub status: MemoryStatus,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Knowledge {
    connection: Mutex<Connection>,
    fts: bool,
}

impl Knowledge {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).context("open local knowledge database")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS memory (
                id TEXT PRIMARY KEY, project TEXT NOT NULL, kind TEXT NOT NULL,
                topic TEXT NOT NULL, claim TEXT NOT NULL, evidence TEXT NOT NULL,
                source_hash TEXT NOT NULL, status TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS memory_scope ON memory(project,source_hash,status);
            CREATE INDEX IF NOT EXISTS memory_topic ON memory(project,kind,topic);",
        )?;
        let fts = conn.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
                claim, evidence, content='memory', content_rowid='rowid');
            CREATE TRIGGER IF NOT EXISTS memory_fts_insert AFTER INSERT ON memory BEGIN
                INSERT INTO memory_fts(rowid,claim,evidence) VALUES(new.rowid,new.claim,new.evidence);
            END;
            CREATE TRIGGER IF NOT EXISTS memory_fts_delete AFTER DELETE ON memory BEGIN
                INSERT INTO memory_fts(memory_fts,rowid,claim,evidence)
                VALUES('delete',old.rowid,old.claim,old.evidence);
            END;
            CREATE TRIGGER IF NOT EXISTS memory_fts_update AFTER UPDATE OF claim,evidence ON memory BEGIN
                INSERT INTO memory_fts(memory_fts,rowid,claim,evidence)
                VALUES('delete',old.rowid,old.claim,old.evidence);
                INSERT INTO memory_fts(rowid,claim,evidence) VALUES(new.rowid,new.claim,new.evidence);
            END;") .is_ok();
        if fts {
            conn.execute("INSERT INTO memory_fts(memory_fts) VALUES('rebuild')", [])?;
        }
        Ok(Self {
            connection: Mutex::new(conn),
            fts,
        })
    }

    /// Insert a candidate. `key: value` or `key=value` declares a conservative conflict topic.
    /// For arbitrary prose use insert_scoped with an explicit topic to detect competing claims.
    pub fn insert(
        &self,
        project: &str,
        kind: &str,
        claim: &str,
        evidence: &str,
        source_hash: &str,
    ) -> Result<String> {
        let topic = inferred_topic(claim);
        self.insert_scoped(project, kind, &topic, claim, evidence, source_hash)
    }

    pub fn insert_scoped(
        &self,
        project: &str,
        kind: &str,
        topic: &str,
        claim: &str,
        evidence: &str,
        source_hash: &str,
    ) -> Result<String> {
        ensure!(!project.trim().is_empty(), "project is required");
        ensure!(
            !claim.trim().is_empty() && !topic.trim().is_empty(),
            "claim and topic are required"
        );
        ensure!(!source_hash.trim().is_empty(), "source hash is required");
        ensure!(
            claim.len() <= 64 * 1024 && evidence.len() <= 256 * 1024,
            "memory record too large"
        );
        let kind = MemoryKind::from_str(kind)?;
        let topic = normalized(topic);
        let id = crate::types::hash(&(project, kind, &topic, claim, evidence, source_hash))?;
        let now = crate::types::now();
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("knowledge lock poisoned"))?;
        let tx = conn.transaction()?;
        tx.execute("INSERT OR IGNORE INTO memory(id,project,kind,topic,claim,evidence,source_hash,status,created_at,updated_at)
            VALUES(?1,?2,?3,?4,?5,?6,?7,'candidate',?8,?8)",
            params![id, project, kind.name(), topic, claim, evidence, source_hash, now])?;
        let conflicts: i64 = tx.query_row(
            "SELECT count(*) FROM memory WHERE project=?1 AND kind=?2
            AND topic=?3 AND claim!=?4 AND status IN ('candidate','active','contested')",
            params![project, kind.name(), topic, claim],
            |row| row.get(0),
        )?;
        if conflicts > 0 {
            tx.execute("UPDATE memory SET status='contested',updated_at=?4
                WHERE project=?1 AND kind=?2 AND topic=?3 AND status IN ('candidate','active','contested')",
                params![project, kind.name(), topic, now])?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// The caller supplies a verified source fingerprint. This only checks proof prerequisites;
    /// it does not infer that the evidence text entails the claim.
    pub fn promote(&self, id: &str, expected_source_hash: &str) -> Result<()> {
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("knowledge lock poisoned"))?;
        let tx = conn.transaction()?;
        let record = get_record(&tx, id)?;
        ensure!(
            record.source_hash == expected_source_hash,
            "source hash changed; memory cannot be promoted"
        );
        ensure!(
            !record.evidence.trim().is_empty(),
            "promotion requires evidence"
        );
        ensure!(
            matches!(
                record.status,
                MemoryStatus::Candidate | MemoryStatus::Contested | MemoryStatus::Active
            ),
            "memory status cannot be promoted"
        );
        let conflicts: i64 = tx.query_row(
            "SELECT count(*) FROM memory WHERE project=?1 AND kind=?2
            AND topic=?3 AND claim!=?4 AND status IN ('candidate','active','contested')",
            params![
                record.project,
                record.kind.name(),
                record.topic,
                record.claim
            ],
            |row| row.get(0),
        )?;
        ensure!(
            conflicts == 0,
            "unresolved competing claims; reject or supersede conflict explicitly"
        );
        tx.execute(
            "UPDATE memory SET status='active',updated_at=?2 WHERE id=?1",
            params![id, crate::types::now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Active can only be reached through promote. This API makes explicit conflict resolution possible.
    pub fn set_status(&self, id: &str, status: MemoryStatus) -> Result<()> {
        ensure!(
            status != MemoryStatus::Active,
            "use promote to activate a record"
        );
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("knowledge lock poisoned"))?;
        let changed = conn.execute(
            "UPDATE memory SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status.name(), crate::types::now()],
        )?;
        ensure!(changed == 1, "memory not found: {id}");
        Ok(())
    }

    /// Return only active claims for this exact project and applicable source fingerprint.
    pub fn retrieve(
        &self,
        project: &str,
        query: &str,
        current_source_hash: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        ensure!(
            !project.is_empty() && !current_source_hash.is_empty(),
            "project and source hash are required"
        );
        ensure!(limit <= 200, "memory limit exceeds 200");
        if limit == 0 {
            return Ok(vec![]);
        }
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("knowledge lock poisoned"))?;
        let terms = query_terms(query);
        let rows = if self.fts && !terms.is_empty() {
            let fts_query = terms
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut stmt = conn.prepare("SELECT m.id,m.project,m.kind,m.topic,m.claim,m.evidence,m.source_hash,m.status,m.created_at,m.updated_at
                FROM memory m JOIN memory_fts f ON f.rowid=m.rowid
                WHERE memory_fts MATCH ?1 AND m.project=?2 AND m.source_hash=?3 AND m.status='active'
                ORDER BY bm25(memory_fts),m.id LIMIT ?4")?;
            let collected = stmt
                .query_map(
                    params![fts_query, project, current_source_hash, limit as i64],
                    row_record,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            collected
        } else {
            let mut stmt = conn.prepare("SELECT id,project,kind,topic,claim,evidence,source_hash,status,created_at,updated_at
                FROM memory WHERE project=?1 AND source_hash=?2 AND status='active' ORDER BY created_at DESC,id")?;
            let mut result = Vec::new();
            let rows = stmt.query_map(params![project, current_source_hash], row_record)?;
            for record in rows {
                let record = record?;
                let text = format!("{} {}", record.claim, record.evidence).to_lowercase();
                if terms.is_empty() || terms.iter().any(|term| text.contains(term)) {
                    result.push(record);
                }
                if result.len() >= limit {
                    break;
                }
            }
            result
        };
        Ok(rows)
    }

    pub fn list(&self, project: &str) -> Result<Vec<MemoryRecord>> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("knowledge lock poisoned"))?;
        let mut stmt = conn.prepare(
            "SELECT id,project,kind,topic,claim,evidence,source_hash,status,created_at,updated_at
            FROM memory WHERE project=?1 ORDER BY created_at,id",
        )?;
        let rows = stmt
            .query_map([project], row_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let kind: String = row.get(2)?;
    let status: String = row.get(7)?;
    let parse_error = |index, error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error.into())
    };
    Ok(MemoryRecord {
        id: row.get(0)?,
        project: row.get(1)?,
        kind: MemoryKind::from_str(&kind).map_err(|e| parse_error(2, e))?,
        topic: row.get(3)?,
        claim: row.get(4)?,
        evidence: row.get(5)?,
        source_hash: row.get(6)?,
        status: MemoryStatus::from_str(&status).map_err(|e| parse_error(7, e))?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}
fn get_record(conn: &Connection, id: &str) -> Result<MemoryRecord> {
    conn.query_row("SELECT id,project,kind,topic,claim,evidence,source_hash,status,created_at,updated_at FROM memory WHERE id=?1", [id], row_record)
        .with_context(|| format!("memory not found: {id}"))
}
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn inferred_topic(claim: &str) -> String {
    if let Some((key, _)) = claim.split_once([':', '=']) {
        if (2..=100).contains(&key.trim().len()) && !key.contains(['.', '\n']) {
            return normalized(key);
        }
    }
    normalized(claim)
}
fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(24)
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_scope_and_source_are_required() {
        let dir = tempfile::tempdir().unwrap();
        let db = Knowledge::open(&dir.path().join("memory.db")).unwrap();
        let id = db
            .insert("p", "fact", "runtime: Rust", "Cargo.toml", "a")
            .unwrap();
        assert!(db.retrieve("p", "Rust", "a", 10).unwrap().is_empty());
        assert!(db.promote(&id, "b").is_err());
        db.promote(&id, "a").unwrap();
        assert_eq!(db.retrieve("p", "Rust", "a", 10).unwrap().len(), 1);
        assert!(db.retrieve("other", "Rust", "a", 10).unwrap().is_empty());
        assert!(db.retrieve("p", "Rust", "b", 10).unwrap().is_empty());
        let no_proof = db.insert("p", "fact", "unrelated", "", "a").unwrap();
        assert!(db.promote(&no_proof, "a").is_err());
        assert!(db.set_status(&no_proof, MemoryStatus::Active).is_err());
    }
    #[test]
    fn conflicts_stay_contested_until_explicit_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let db = Knowledge::open(&dir.path().join("memory.db")).unwrap();
        let first = db
            .insert("p", "fact", "runtime: Rust", "manifest", "a")
            .unwrap();
        db.promote(&first, "a").unwrap();
        let second = db
            .insert("p", "fact", "runtime: Python", "claim", "b")
            .unwrap();
        assert!(db
            .list("p")
            .unwrap()
            .iter()
            .all(|r| r.status == MemoryStatus::Contested));
        assert!(db.promote(&second, "b").is_err());
        assert!(db.retrieve("p", "runtime", "a", 10).unwrap().is_empty());
        db.set_status(&first, MemoryStatus::Superseded).unwrap();
        db.promote(&second, "b").unwrap();
        assert_eq!(db.retrieve("p", "Python", "b", 10).unwrap()[0].id, second);
    }
    #[test]
    fn duplicate_is_idempotent_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let id;
        {
            let db = Knowledge::open(&path).unwrap();
            id = db
                .insert("p", "failure", "bad boundary", "test line 8", "tree")
                .unwrap();
            for _ in 0..20 {
                assert_eq!(
                    db.insert("p", "failure", "bad boundary", "test line 8", "tree")
                        .unwrap(),
                    id
                );
            }
            assert_eq!(db.list("p").unwrap().len(), 1);
            assert_eq!(db.list("p").unwrap()[0].status, MemoryStatus::Candidate);
            db.promote(&id, "tree").unwrap();
        }
        let db = Knowledge::open(&path).unwrap();
        assert_eq!(
            db.retrieve("p", "boundary OR (\"oops\")", "tree", 10)
                .unwrap()[0]
                .id,
            id
        );
        assert!(db.retrieve("p", "", "tree", 201).is_err());
    }
    #[test]
    fn fallback_search_keeps_filters_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Knowledge::open(&dir.path().join("memory.db")).unwrap();
        db.fts = false;
        for claim in ["Unicode parser", "unicode decoder", "network client"] {
            let id = db.insert("p", "fact", claim, "test", "tree").unwrap();
            db.promote(&id, "tree").unwrap();
        }
        assert_eq!(db.retrieve("p", "unicode", "tree", 1).unwrap().len(), 1);
        assert_eq!(db.retrieve("p", "", "tree", 10).unwrap().len(), 3);
    }
}
