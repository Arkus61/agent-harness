//! Durable bounded delivery of immutable events. Adapters are trusted code;
//! event metadata carries no execution authority or command interface.
use crate::{storage::Store, types::Event};
use anyhow::{anyhow, ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HandlerPolicy {
    pub handler: String,
    pub version: u32,
    pub event_kinds: Vec<String>,
    pub max_attempts: u32,
    pub lease_ms: u64,
    pub backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub max_chain_depth: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCapabilities {
    pub idempotency_keys: bool,
    pub independent_reconciliation: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Leased,
    InFlight,
    Delivered,
    Unknown,
    DeadLetter,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectReceipt {
    pub event_seq: i64,
    pub handler: String,
    pub policy_hash: String,
    pub idempotency_key: String,
    pub effect_id: String,
}

/// Trusted adapter attestation of independently settled NoEffect evidence.
/// For a non-idempotent backend, evidence must also fence/quiesce the old
/// attempt so it cannot commit later. Snapshot absence alone is insufficient.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NoEffectReceipt {
    pub event_seq: i64,
    pub handler: String,
    pub policy_hash: String,
    pub idempotency_key: String,
    pub attempt_fence: u64,
    pub evidence_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub event: Event,
    pub handler: String,
    pub policy_hash: String,
    pub idempotency_key: String,
    pub state: DeliveryState,
    pub attempts: u32,
    pub fence: u64,
    pub lease_until_ms: Option<u64>,
    pub next_attempt_ms: u64,
    pub chain_depth: u32,
    pub receipt: Option<EffectReceipt>,
    pub failure: Option<String>,
    #[serde(default)]
    pub no_effect_receipt: Option<NoEffectReceipt>,
}

#[derive(Clone, Debug)]
pub enum DeliveryOutcome {
    Delivered(EffectReceipt),
    FailedBeforeEffect { retryable: bool },
    Unknown,
}

#[derive(Clone, Debug)]
pub enum Reconciliation {
    Delivered(EffectReceipt),
    NoEffect(NoEffectReceipt),
    Unknown,
}

pub trait DeliveryAdapter {
    fn identity(&self) -> &str;
    fn capabilities(&self) -> AdapterCapabilities;
    fn deliver(&mut self, delivery: &DeliveryRecord) -> Result<DeliveryOutcome>;
    fn reconcile(&mut self, delivery: &DeliveryRecord) -> Result<Reconciliation>;
}

/// A safe local backend: one SQLite journal entry is the actual effect. Payload
/// fields are hashed for evidence and never interpreted as commands or grants.
pub struct LocalJournalAdapter {
    handler: String,
    journal: Store,
}
impl LocalJournalAdapter {
    pub fn open(root: impl AsRef<std::path::Path>, handler: &str) -> Result<Self> {
        validate_identity(handler)?;
        let journal = Store::open(root)?;
        journal.transaction(|tx| {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS outbox_local_metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1),handler TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS outbox_local_effects(seq INTEGER PRIMARY KEY AUTOINCREMENT,idempotency_key TEXT NOT NULL UNIQUE,event_hash TEXT NOT NULL,policy_hash TEXT NOT NULL,receipt TEXT NOT NULL);")?;
            tx.execute("INSERT OR IGNORE INTO outbox_local_metadata(singleton,handler) VALUES(1,?1)",[handler])?;
            let identity:String=tx.query_row("SELECT handler FROM outbox_local_metadata WHERE singleton=1",[],|r|r.get(0))?;
            ensure!(identity==handler,"local journal belongs to another handler");
            Ok(())
        })?;
        Ok(Self {
            handler: handler.into(),
            journal,
        })
    }
    pub fn effect_count(&self) -> Result<u64> {
        Ok(self.journal.connection()?.query_row(
            "SELECT COUNT(*) FROM outbox_local_effects",
            [],
            |r| r.get(0),
        )?)
    }
    fn existing(tx: &Transaction<'_>, delivery: &DeliveryRecord) -> Result<Option<EffectReceipt>> {
        let existing:Option<(String,String,String)>=tx.query_row("SELECT event_hash,policy_hash,receipt FROM outbox_local_effects WHERE idempotency_key=?1",[&delivery.idempotency_key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((event_hash, policy_hash, body)) = existing else {
            return Ok(None);
        };
        ensure!(
            event_hash == crate::types::hash(&delivery.event)?
                && policy_hash == delivery.policy_hash,
            "idempotency key cannot identify a different effect"
        );
        let receipt = serde_json::from_str(&body)?;
        ensure!(
            receipt_matches(delivery, &receipt),
            "local journal receipt identity mismatch"
        );
        Ok(Some(receipt))
    }
}
impl DeliveryAdapter for LocalJournalAdapter {
    fn identity(&self) -> &str {
        &self.handler
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            idempotency_keys: true,
            independent_reconciliation: true,
        }
    }
    fn deliver(&mut self, delivery: &DeliveryRecord) -> Result<DeliveryOutcome> {
        ensure!(
            delivery.handler == self.handler,
            "local journal handler mismatch"
        );
        self.journal.transaction(|tx| {
            if let Some(receipt)=Self::existing(tx,delivery)? {return Ok(DeliveryOutcome::Delivered(receipt));}
            let event_hash=crate::types::hash(&delivery.event)?;
            // INSERT and the final receipt are one durable FULL-synchronous
            // transaction; dropping a coordinator reply cannot duplicate it.
            tx.execute("INSERT INTO outbox_local_effects(idempotency_key,event_hash,policy_hash,receipt) VALUES(?1,?2,?3,'')",params![delivery.idempotency_key,event_hash,delivery.policy_hash])?;
            let seq=tx.last_insert_rowid();
            let receipt=EffectReceipt {event_seq:delivery.event.seq,handler:delivery.handler.clone(),policy_hash:delivery.policy_hash.clone(),idempotency_key:delivery.idempotency_key.clone(),effect_id:format!("local-journal:{}:{seq}",self.handler)};
            tx.execute("UPDATE outbox_local_effects SET receipt=?2 WHERE seq=?1",params![seq,serde_json::to_string(&receipt)?])?;
            Ok(DeliveryOutcome::Delivered(receipt))
        })
    }
    fn reconcile(&mut self, delivery: &DeliveryRecord) -> Result<Reconciliation> {
        ensure!(
            delivery.handler == self.handler,
            "local journal handler mismatch"
        );
        self.journal.transaction(|tx| {
            Ok(match Self::existing(tx, delivery)? {
                Some(receipt) => Reconciliation::Delivered(receipt),
                None => Reconciliation::NoEffect(NoEffectReceipt {
                    event_seq: delivery.event.seq,
                    handler: delivery.handler.clone(),
                    policy_hash: delivery.policy_hash.clone(),
                    idempotency_key: delivery.idempotency_key.clone(),
                    attempt_fence: delivery.fence,
                    evidence_id: format!("local-idempotency-fence:{}", delivery.idempotency_key),
                }),
            })
        })
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct WorkerSummary {
    pub claimed: u32,
    pub delivered: u32,
    pub pending: u32,
    pub unknown: u32,
    pub dead_letter: u32,
    pub fenced: u32,
}
pub struct OutboxWorker<A: DeliveryAdapter> {
    store: Store,
    adapter: A,
}
impl<A: DeliveryAdapter> OutboxWorker<A> {
    pub fn new(store: Store, policy: HandlerPolicy, adapter: A) -> Result<Self> {
        ensure!(
            policy.handler == adapter.identity(),
            "adapter identity does not match handler policy"
        );
        store.register_outbox_handler(&policy, adapter.capabilities())?;
        Ok(Self { store, adapter })
    }
    /// A tick dispatches at most 100 effects. Adapter errors carry uncertain
    /// effect status; their arbitrary error text is never persisted.
    pub fn tick(&mut self, now_ms: u64, limit: u32) -> Result<WorkerSummary> {
        ensure!(
            limit > 0 && limit <= 100,
            "outbox tick limit must be between 1 and 100"
        );
        let started = std::time::Instant::now();
        let stamp = || {
            now_ms.saturating_add(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
        };
        let mut summary = WorkerSummary::default();
        for _ in 0..limit {
            let Some(delivery) = self.store.claim_outbox(self.adapter.identity(), stamp())? else {
                break;
            };
            summary.claimed += 1;
            if !self.store.begin_outbox_effect(&delivery, stamp())? {
                summary.fenced += 1;
                continue;
            }
            let outcome = self
                .adapter
                .deliver(&delivery)
                .unwrap_or(DeliveryOutcome::Unknown);
            if !self
                .store
                .finish_outbox_delivery(&delivery, outcome, stamp())?
            {
                summary.fenced += 1;
                continue;
            }
            let state = self
                .store
                .get_outbox_delivery(delivery.event.seq, &delivery.handler)?
                .state;
            match state {
                DeliveryState::Delivered => summary.delivered += 1,
                DeliveryState::Pending => summary.pending += 1,
                DeliveryState::Unknown => summary.unknown += 1,
                DeliveryState::DeadLetter => summary.dead_letter += 1,
                _ => {}
            }
        }
        Ok(summary)
    }
    /// Reconciliation queries independent backend evidence; it never performs
    /// the effect. A proven NoEffect allows a later bounded retry.
    pub fn reconcile(&mut self, event_seq: i64, now_ms: u64) -> Result<bool> {
        let delivery = self
            .store
            .get_outbox_delivery(event_seq, self.adapter.identity())?;
        ensure!(
            delivery.state == DeliveryState::Unknown,
            "only unknown deliveries require reconciliation"
        );
        let outcome = self
            .adapter
            .reconcile(&delivery)
            .unwrap_or(Reconciliation::Unknown);
        self.store.reconcile_outbox(&delivery, outcome, now_ms)
    }
}

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS outbox_handlers(handler TEXT PRIMARY KEY, policy TEXT NOT NULL, policy_hash TEXT NOT NULL, capabilities TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS outbox_chains(event_seq INTEGER PRIMARY KEY REFERENCES events(seq), depth INTEGER NOT NULL CHECK(depth>=0));
        CREATE TABLE IF NOT EXISTS outbox_deliveries(event_seq INTEGER NOT NULL REFERENCES outbox(event_seq),handler TEXT NOT NULL REFERENCES outbox_handlers(handler),state TEXT NOT NULL,next_attempt_ms INTEGER NOT NULL,lease_until_ms INTEGER,record TEXT NOT NULL,PRIMARY KEY(event_seq,handler));
        CREATE INDEX IF NOT EXISTS outbox_delivery_ready ON outbox_deliveries(handler,state,next_attempt_ms,event_seq);
        CREATE INDEX IF NOT EXISTS outbox_delivery_expiry ON outbox_deliveries(handler,state,lease_until_ms);")?;
    Ok(())
}

fn validate_identity(identity: &str) -> Result<()> {
    ensure!(
        !identity.is_empty()
            && identity.len() <= 128
            && identity
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "invalid handler identity"
    );
    Ok(())
}
impl HandlerPolicy {
    pub fn validate(&self) -> Result<()> {
        validate_identity(&self.handler)?;
        ensure!(self.version > 0, "handler policy version must be positive");
        ensure!(
            !self.event_kinds.is_empty()
                && self.event_kinds.len() <= 32
                && self.event_kinds.iter().all(|s| !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    || s == "*"),
            "invalid event subscriptions"
        );
        ensure!(
            self.max_attempts > 0 && self.max_attempts <= 100,
            "invalid retry limit"
        );
        ensure!(
            self.lease_ms > 0 && self.lease_ms <= 3_600_000,
            "invalid lease duration"
        );
        ensure!(
            self.backoff_ms <= self.max_backoff_ms && self.max_backoff_ms <= 86_400_000,
            "invalid retry backoff"
        );
        ensure!(self.max_chain_depth <= 16, "invalid hook chain limit");
        Ok(())
    }
    fn accepts(&self, event: &Event) -> bool {
        self.event_kinds
            .iter()
            .any(|kind| kind == "*" || kind == &event.kind)
    }
    fn backoff(&self, attempts: u32) -> u64 {
        self.backoff_ms
            .saturating_mul(1u64 << attempts.saturating_sub(1).min(31))
            .min(self.max_backoff_ms)
    }
}
fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).context("outbox timestamp exceeds SQLite range")
}
fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let payload: String = row.get(3)?;
    let payload = serde_json::from_str(&payload).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Event {
        seq: row.get(0)?,
        run_id: row.get(1)?,
        kind: row.get(2)?,
        payload,
        created_at: row.get(4)?,
    })
}
fn handler(tx: &Transaction<'_>, id: &str) -> Result<(HandlerPolicy, String, AdapterCapabilities)> {
    let row: Option<(String, String, String)> = tx
        .query_row(
            "SELECT policy,policy_hash,capabilities FROM outbox_handlers WHERE handler=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (policy, hash, capabilities) = row.ok_or_else(|| anyhow!("unknown outbox handler"))?;
    Ok((
        serde_json::from_str(&policy)?,
        hash,
        serde_json::from_str(&capabilities)?,
    ))
}
fn state_name(state: DeliveryState) -> &'static str {
    match state {
        DeliveryState::Pending => "pending",
        DeliveryState::Leased => "leased",
        DeliveryState::InFlight => "in_flight",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Unknown => "unknown",
        DeliveryState::DeadLetter => "dead_letter",
    }
}
fn save(tx: &Transaction<'_>, record: &DeliveryRecord) -> Result<()> {
    ensure!(tx.execute("UPDATE outbox_deliveries SET state=?3,next_attempt_ms=?4,lease_until_ms=?5,record=?6 WHERE event_seq=?1 AND handler=?2",params![record.event.seq,record.handler,state_name(record.state),integer(record.next_attempt_ms)?,record.lease_until_ms.map(integer).transpose()?,serde_json::to_string(record)?])? == 1,"unknown outbox delivery");
    Ok(())
}
fn load(tx: &Transaction<'_>, seq: i64, id: &str) -> Result<DeliveryRecord> {
    let body: Option<String> = tx
        .query_row(
            "SELECT record FROM outbox_deliveries WHERE event_seq=?1 AND handler=?2",
            params![seq, id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(
        &body.ok_or_else(|| anyhow!("unknown outbox delivery"))?,
    )?)
}
fn same_identity(a: &DeliveryRecord, b: &DeliveryRecord) -> Result<bool> {
    Ok(a.fence == b.fence
        && a.handler == b.handler
        && a.idempotency_key == b.idempotency_key
        && a.policy_hash == b.policy_hash
        && serde_json::to_vec(&a.event)? == serde_json::to_vec(&b.event)?)
}
fn receipt_matches(record: &DeliveryRecord, receipt: &EffectReceipt) -> bool {
    receipt.event_seq == record.event.seq
        && receipt.handler == record.handler
        && receipt.policy_hash == record.policy_hash
        && receipt.idempotency_key == record.idempotency_key
        && !receipt.effect_id.is_empty()
        && receipt.effect_id.len() <= 256
        && !receipt.effect_id.chars().any(char::is_control)
}
fn insert_delivery(
    tx: &Transaction<'_>,
    event: &Event,
    policy: &HandlerPolicy,
    policy_hash: &str,
    depth: u32,
) -> Result<()> {
    if !policy.accepts(event) {
        return Ok(());
    }
    let key = crate::types::hash(&("harness-outbox-v1", event, policy_hash))?;
    let blocked = depth > policy.max_chain_depth;
    let record = DeliveryRecord {
        event: event.clone(),
        handler: policy.handler.clone(),
        policy_hash: policy_hash.into(),
        idempotency_key: key,
        state: if blocked {
            DeliveryState::DeadLetter
        } else {
            DeliveryState::Pending
        },
        attempts: 0,
        fence: 0,
        lease_until_ms: None,
        next_attempt_ms: 0,
        chain_depth: depth,
        receipt: None,
        failure: blocked.then(|| "cycle_depth_exceeded".into()),
        no_effect_receipt: None,
    };
    tx.execute("INSERT OR IGNORE INTO outbox_deliveries(event_seq,handler,state,next_attempt_ms,lease_until_ms,record) VALUES(?1,?2,?3,0,NULL,?4)",params![event.seq,policy.handler,state_name(record.state),serde_json::to_string(&record)?])?;
    Ok(())
}
pub(crate) fn enqueue_event(tx: &Transaction<'_>, event: &Event, depth: u32) -> Result<()> {
    tx.execute(
        "INSERT INTO outbox_chains(event_seq,depth) VALUES(?1,?2)",
        params![event.seq, depth],
    )?;
    let registrations: Vec<(String, String)> = tx
        .prepare("SELECT policy,policy_hash FROM outbox_handlers")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (policy, policy_hash) in registrations {
        insert_delivery(
            tx,
            event,
            &serde_json::from_str(&policy)?,
            &policy_hash,
            depth,
        )?;
    }
    Ok(())
}
fn retry(record: &mut DeliveryRecord, policy: &HandlerPolicy, now_ms: u64, uncertain: bool) {
    record.lease_until_ms = None;
    if record.attempts >= policy.max_attempts {
        record.state = if uncertain {
            DeliveryState::Unknown
        } else {
            DeliveryState::DeadLetter
        };
        record.failure = Some(
            if uncertain {
                "uncertain_effect_attempt_limit"
            } else {
                "attempt_limit_exceeded"
            }
            .into(),
        );
    } else {
        record.state = DeliveryState::Pending;
        record.next_attempt_ms = now_ms.saturating_add(policy.backoff(record.attempts));
        record.failure = Some(
            if uncertain {
                "uncertain_idempotent_effect"
            } else {
                "retryable_before_effect_failure"
            }
            .into(),
        );
    }
}
fn delivered(
    tx: &Transaction<'_>,
    record: &mut DeliveryRecord,
    receipt: EffectReceipt,
) -> Result<()> {
    record.state = DeliveryState::Delivered;
    record.lease_until_ms = None;
    record.receipt = Some(receipt);
    record.failure = None;
    save(tx, record)?;
    crate::storage::append_event_with_depth(
        tx,
        &record.event.run_id,
        "outbox.delivered",
        json!({"event_seq":record.event.seq,"handler":record.handler,"policy_hash":record.policy_hash,"idempotency_key":record.idempotency_key,"effect_id":record.receipt.as_ref().unwrap().effect_id}),
        record.chain_depth + 1,
    )?;
    Ok(())
}

impl Store {
    pub fn register_outbox_handler(
        &self,
        policy: &HandlerPolicy,
        capabilities: AdapterCapabilities,
    ) -> Result<()> {
        policy.validate()?;
        self.transaction(|tx| {
            let hash=crate::types::hash(&(policy,capabilities))?;
            let existing: Option<String>=tx.query_row("SELECT policy_hash FROM outbox_handlers WHERE handler=?1",[&policy.handler],|r|r.get(0)).optional()?;
            if let Some(existing)=existing { ensure!(existing==hash,"registered handler policy or capabilities cannot change"); return Ok(()); }
            let count:u32=tx.query_row("SELECT COUNT(*) FROM outbox_handlers",[],|r|r.get(0))?;
            ensure!(count<32,"outbox handler limit reached");
            tx.execute("INSERT INTO outbox_handlers(handler,policy,policy_hash,capabilities) VALUES(?1,?2,?3,?4)",params![policy.handler,serde_json::to_string(policy)?,hash,serde_json::to_string(&capabilities)?])?;
            let events:Vec<(Event,u32)>=tx.prepare("SELECT e.seq,e.run_id,e.kind,e.payload,e.created_at,COALESCE(c.depth,0) FROM events e JOIN outbox o ON o.event_seq=e.seq LEFT JOIN outbox_chains c ON c.event_seq=e.seq ORDER BY e.seq")?.query_map([],|r|Ok((event_from_row(r)?,r.get(5)?)))?.collect::<rusqlite::Result<_>>()?;
            for (event,depth) in events {insert_delivery(tx,&event,policy,&hash,depth)?;}
            Ok(())
        })
    }
    pub fn get_outbox_delivery(&self, seq: i64, handler: &str) -> Result<DeliveryRecord> {
        self.transaction(|tx| load(tx, seq, handler))
    }
    pub fn outbox_deliveries(&self, id: &str) -> Result<Vec<DeliveryRecord>> {
        let db = self.connection()?;
        let bodies: Vec<String> = db
            .prepare("SELECT record FROM outbox_deliveries WHERE handler=?1 ORDER BY event_seq")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        bodies
            .into_iter()
            .map(|s| Ok(serde_json::from_str(&s)?))
            .collect()
    }
    pub fn claim_outbox(&self, id: &str, now_ms: u64) -> Result<Option<DeliveryRecord>> {
        integer(now_ms)?;
        self.transaction(|tx| {
            let (policy,_,capabilities)=handler(tx,id)?;
            let expired:Vec<String>=tx.prepare("SELECT record FROM outbox_deliveries WHERE handler=?1 AND state IN ('leased','in_flight') AND lease_until_ms<=?2 ORDER BY event_seq LIMIT 256")?.query_map(params![id,integer(now_ms)?],|r|r.get(0))?.collect::<rusqlite::Result<_>>()?;
            for body in expired {
                let mut record:DeliveryRecord=serde_json::from_str(&body)?;
                let expiry=record.lease_until_ms.unwrap_or(now_ms);
                if record.state==DeliveryState::InFlight && !capabilities.idempotency_keys {record.state=DeliveryState::Unknown;record.lease_until_ms=None;record.failure=Some("lease_expired_uncertain_effect".into());}
                else {let uncertain=record.state==DeliveryState::InFlight; retry(&mut record,&policy,expiry,uncertain);}
                save(tx,&record)?;
            }
            let body:Option<String>=tx.query_row("SELECT record FROM outbox_deliveries WHERE handler=?1 AND state='pending' AND next_attempt_ms<=?2 ORDER BY event_seq LIMIT 1",params![id,integer(now_ms)?],|r|r.get(0)).optional()?;
            let Some(body)=body else{return Ok(None)};
            let mut record:DeliveryRecord=serde_json::from_str(&body)?;
            ensure!(record.attempts<policy.max_attempts,"pending delivery exceeds attempt limit");
            record.attempts+=1;
            record.fence=record.fence.checked_add(1).ok_or_else(||anyhow!("outbox fence overflow"))?;
            record.state=DeliveryState::Leased;
            record.lease_until_ms=Some(now_ms.checked_add(policy.lease_ms).ok_or_else(||anyhow!("outbox lease overflow"))?);
            integer(record.lease_until_ms.unwrap())?;
            save(tx,&record)?;
            Ok(Some(record))
        })
    }
    pub fn begin_outbox_effect(&self, delivery: &DeliveryRecord, now_ms: u64) -> Result<bool> {
        self.transaction(|tx| {
            let mut record = load(tx, delivery.event.seq, &delivery.handler)?;
            if !same_identity(&record, delivery)?
                || record.state != DeliveryState::Leased
                || record.lease_until_ms.is_none_or(|expiry| expiry <= now_ms)
            {
                return Ok(false);
            }
            record.state = DeliveryState::InFlight;
            save(tx, &record)?;
            Ok(true)
        })
    }
    pub fn finish_outbox_delivery(
        &self,
        delivery: &DeliveryRecord,
        outcome: DeliveryOutcome,
        now_ms: u64,
    ) -> Result<bool> {
        self.transaction(|tx| {
            let mut record = load(tx, delivery.event.seq, &delivery.handler)?;
            if !same_identity(&record, delivery)?
                || record.state != DeliveryState::InFlight
                || record.lease_until_ms.is_none_or(|expiry| expiry <= now_ms)
            {
                return Ok(false);
            }
            let (policy, _, capabilities) = handler(tx, &record.handler)?;
            match outcome {
                DeliveryOutcome::Delivered(receipt) if receipt_matches(&record, &receipt) => {
                    delivered(tx, &mut record, receipt)?;
                    return Ok(true);
                }
                DeliveryOutcome::FailedBeforeEffect { retryable: true } => {
                    retry(&mut record, &policy, now_ms, false)
                }
                DeliveryOutcome::FailedBeforeEffect { retryable: false } => {
                    record.state = DeliveryState::DeadLetter;
                    record.lease_until_ms = None;
                    record.failure = Some("permanent_before_effect_failure".into());
                }
                DeliveryOutcome::Unknown if capabilities.idempotency_keys => {
                    retry(&mut record, &policy, now_ms, true)
                }
                DeliveryOutcome::Unknown | DeliveryOutcome::Delivered(_) => {
                    record.state = DeliveryState::Unknown;
                    record.lease_until_ms = None;
                    record.failure = Some("uncertain_or_invalid_effect_receipt".into());
                }
            }
            save(tx, &record)?;
            Ok(true)
        })
    }
    pub fn reconcile_outbox(
        &self,
        delivery: &DeliveryRecord,
        outcome: Reconciliation,
        now_ms: u64,
    ) -> Result<bool> {
        self.transaction(|tx| {
            let mut record = load(tx, delivery.event.seq, &delivery.handler)?;
            let (policy, _, capabilities) = handler(tx, &record.handler)?;
            ensure!(
                capabilities.independent_reconciliation,
                "adapter does not support independent reconciliation"
            );
            if !same_identity(&record, delivery)? || record.state != DeliveryState::Unknown {
                return Ok(false);
            }
            match outcome {
                Reconciliation::Delivered(receipt) => {
                    ensure!(
                        receipt_matches(&record, &receipt),
                        "reconciliation receipt identity mismatch"
                    );
                    delivered(tx, &mut record, receipt)?;
                }
                Reconciliation::NoEffect(proof) => {
                    ensure!(
                        proof.event_seq == record.event.seq
                            && proof.handler == record.handler
                            && proof.policy_hash == record.policy_hash
                            && proof.idempotency_key == record.idempotency_key
                            && proof.attempt_fence == record.fence
                            && !proof.evidence_id.is_empty()
                            && proof.evidence_id.len() <= 256
                            && !proof.evidence_id.chars().any(char::is_control),
                        "NoEffect evidence identity or attempt fence mismatch"
                    );
                    record.no_effect_receipt = Some(proof);
                    retry(&mut record, &policy, now_ms, false);
                    save(tx, &record)?;
                }
                Reconciliation::Unknown => {}
            }
            Ok(true)
        })
    }
}
