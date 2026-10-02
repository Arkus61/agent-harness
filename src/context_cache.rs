//! Process-local bounded context cache. Sources remain untrusted evidence.
use crate::types::{ContextBundle, Grants};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const CACHE_VERSION: &str = "bounded-context-v1";
const REDACTION_POLICY_VERSION: &str = "sensitive-paths-v1-explicit-secrets-v1";

pub struct ContextRequest<'a> {
    pub root: &'a Path,
    pub project_id: &'a str,
    pub role: &'a str,
    pub prompt: &'a str,
    pub requirements: &'a [String],
    pub grants: &'a Grants,
    pub byte_limit: usize,
    pub secrets: &'a [String],
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub bypasses: u64,
    pub source_revalidations: u64,
    pub entries: usize,
    pub retained_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceExcerpt {
    pub path: String,
    pub full_file_hash: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub complete: bool,
    pub redacted: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePacket {
    pub sources: Vec<SourceExcerpt>,
    pub omissions: Vec<String>,
    pub claim_status: ClaimStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Current,
    StaleSource,
    NotFoundWithinScope,
    AccessDenied,
    InvalidSource,
    InvalidRange,
    ReadLimitExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceExpansion {
    pub status: EvidenceStatus,
    pub source: Option<SourceExcerpt>,
    pub claim_status: ClaimStatus,
}

pub struct ExpansionRequest<'a> {
    pub root: &'a Path,
    pub path: &'a str,
    pub expected_hash: &'a str,
    pub grants: &'a Grants,
    pub start_byte: usize,
    pub end_byte: usize,
    pub byte_limit: usize,
    pub secrets: &'a [String],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedContext {
    pub bundle: ContextBundle,
    pub evidence: EvidencePacket,
    pub cache_key: String,
    pub cache_hit: bool,
}

pub struct ContextCache {
    max_entries: usize,
    max_bytes: usize,
    clock: u64,
    entries: BTreeMap<String, Entry>,
    stats: ContextCacheStats,
}

struct Entry {
    // Store only serialized, redacted packets: byte accounting is exact. Map
    // and LRU metadata add bounded overhead proportional to max_entries.
    payload: Vec<u8>,
    last_used: u64,
}

#[derive(Serialize, Deserialize)]
struct Payload {
    bundle: ContextBundle,
    evidence: EvidencePacket,
}

impl ContextCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> Result<Self> {
        ensure!(
            max_entries > 0 && max_bytes > 0,
            "context cache bounds must be positive"
        );
        Ok(Self {
            max_entries,
            max_bytes,
            clock: 0,
            entries: BTreeMap::new(),
            stats: ContextCacheStats::default(),
        })
    }

    pub fn compile(&mut self, request: ContextRequest<'_>) -> Result<CachedContext> {
        ensure!(
            !request.project_id.is_empty() && !request.role.is_empty(),
            "context project and role must be explicit"
        );
        ensure!(
            request.byte_limit > 0,
            "context byte limit must be positive"
        );
        // Always collect under *current* grants before even consulting a cache
        // entry. The digest includes file bytes, not merely a Git revision or
        // timestamps, and discovery makes new relevant files invalidate it.
        let snapshot = crate::context::collect(
            request.root,
            request.prompt,
            request.requirements,
            request.grants,
        )?;
        self.stats.source_revalidations = self
            .stats
            .source_revalidations
            .saturating_add(snapshot.revalidations as u64);
        let mut read_scope = request
            .grants
            .read
            .iter()
            .map(|pattern| pattern.replace('\\', "/"))
            .collect::<Vec<_>>();
        read_scope.sort();
        read_scope.dedup();
        let mut secrets = request
            .secrets
            .iter()
            .filter(|value| !value.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        secrets.sort();
        secrets.dedup();
        let key = crate::types::hash(&(
            CACHE_VERSION,
            env!("CARGO_PKG_VERSION"),
            REDACTION_POLICY_VERSION,
            snapshot
                .root
                .to_str()
                .context("context root path is not UTF-8")?,
            request.project_id,
            request.role,
            &read_scope,
            request.prompt,
            request.requirements,
            request.byte_limit,
            crate::types::hash(&secrets)?,
            &snapshot.fingerprint,
        ))?;
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.clock;
            let payload: Payload = serde_json::from_slice(&entry.payload)?;
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(CachedContext {
                bundle: payload.bundle,
                evidence: payload.evidence,
                cache_key: key,
                cache_hit: true,
            });
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        let (bundle, evidence) = crate::context::render(
            &snapshot,
            request.prompt,
            request.requirements,
            request.byte_limit,
            request.secrets,
        )?;
        let payload = Payload { bundle, evidence };
        let bytes = serde_json::to_vec(&payload)?;
        if bytes.len() > self.max_bytes {
            self.stats.bypasses = self.stats.bypasses.saturating_add(1);
        } else {
            while self.entries.len() >= self.max_entries
                || self.stats.retained_bytes.saturating_add(bytes.len()) > self.max_bytes
            {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| key.clone());
                let Some(oldest) = oldest else {
                    break;
                };
                let removed = self
                    .entries
                    .remove(&oldest)
                    .expect("selected existing cache entry");
                self.stats.retained_bytes -= removed.payload.len();
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
            self.stats.retained_bytes += bytes.len();
            self.entries.insert(
                key.clone(),
                Entry {
                    payload: bytes,
                    last_used: self.clock,
                },
            );
            self.stats.entries = self.entries.len();
        }
        Ok(CachedContext {
            bundle: payload.bundle,
            evidence: payload.evidence,
            cache_key: key,
            cache_hit: false,
        })
    }

    pub fn stats(&self) -> ContextCacheStats {
        self.stats.clone()
    }
}

pub fn expand_source(request: ExpansionRequest<'_>) -> Result<SourceExpansion> {
    ensure!(
        request.byte_limit > 0,
        "source expansion byte limit must be positive"
    );
    let unknown = |status| SourceExpansion {
        status,
        source: None,
        claim_status: ClaimStatus::Unknown,
    };
    let Ok(relative) = crate::policy::relative_path(request.path) else {
        return Ok(unknown(EvidenceStatus::AccessDenied));
    };
    let path = relative
        .to_str()
        .context("source path is not UTF-8")?
        .replace('\\', "/");
    let scope = crate::context::read_scope(request.grants)?;
    // Denials intentionally precede existence checks: excluded files cannot be
    // probed through a not-found response and old grants cannot expand evidence.
    if !scope.is_match(&path) {
        return Ok(unknown(EvidenceStatus::AccessDenied));
    }
    let root = request
        .root
        .canonicalize()
        .context("context root is unavailable")?;
    ensure!(root.is_dir(), "context root is not a directory");
    match root.join(&relative).symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(unknown(EvidenceStatus::NotFoundWithinScope))
        }
        Err(_) => return Ok(unknown(EvidenceStatus::InvalidSource)),
        Ok(metadata) if metadata.len() > crate::context::MAX_SOURCE_BYTES as u64 => {
            return Ok(unknown(EvidenceStatus::ReadLimitExceeded))
        }
        _ => {}
    }
    let (contents, hash) = match crate::context::read_source(&root, &path) {
        Ok(value) => value,
        Err(_) => return Ok(unknown(EvidenceStatus::InvalidSource)),
    };
    if request.expected_hash.is_empty() || request.expected_hash != hash {
        return Ok(unknown(EvidenceStatus::StaleSource));
    }
    if request.start_byte > request.end_byte
        || request.end_byte > contents.len()
        || !contents.is_char_boundary(request.start_byte)
        || !contents.is_char_boundary(request.end_byte)
        || request.end_byte - request.start_byte > request.byte_limit
        || crate::context::range_cuts_secret(
            &contents,
            request.start_byte,
            request.end_byte,
            request.secrets,
        )
    {
        return Ok(unknown(EvidenceStatus::InvalidRange));
    }
    let raw = &contents[request.start_byte..request.end_byte];
    let text = crate::context::redact(raw, request.secrets);
    if text.len() > request.byte_limit {
        return Ok(unknown(EvidenceStatus::InvalidRange));
    }
    Ok(SourceExpansion {
        status: EvidenceStatus::Current,
        source: Some(SourceExcerpt {
            path,
            full_file_hash: hash,
            start_byte: request.start_byte,
            end_byte: request.end_byte,
            complete: request.start_byte == 0 && request.end_byte == contents.len(),
            redacted: raw != text,
            text,
        }),
        // Matching bytes establish freshness, never semantic entailment.
        claim_status: ClaimStatus::Unknown,
    })
}
