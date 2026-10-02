use agent_harness::context_cache::{
    expand_source, ClaimStatus, ContextCache, ContextRequest, EvidenceStatus, ExpansionRequest,
};
use agent_harness::types::Grants;
use std::fs;
use std::path::Path;

fn grants(pattern: &str) -> Grants {
    Grants {
        read: vec![pattern.into()],
        ..Default::default()
    }
}

fn compile(
    cache: &mut ContextCache,
    root: &Path,
    role: &str,
    scope: &Grants,
) -> agent_harness::context_cache::CachedContext {
    cache
        .compile(ContextRequest {
            root,
            project_id: "project",
            role,
            prompt: "main",
            requirements: &["Keep behavior".into()],
            grants: scope,
            byte_limit: 8192,
            secrets: &[],
        })
        .unwrap()
}

#[test]
fn identical_inputs_hit_and_dirty_bytes_invalidate_even_without_git_commit() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "fn main() { old(); }").unwrap();
    let mut cache = ContextCache::new(4, 64 * 1024).unwrap();
    let scope = grants("**");
    let first = compile(&mut cache, root.path(), "builder", &scope);
    let second = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(first.bundle.hash, second.bundle.hash);
    fs::write(root.path().join("main.rs"), "fn main() { new(); }").unwrap();
    let changed = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!changed.cache_hit);
    assert_ne!(first.cache_key, changed.cache_key);
    assert!(changed.bundle.text.contains("new()"));
    assert!(!changed.bundle.text.contains("old()"));
    assert_eq!(cache.stats().hits, 1);
    assert_eq!(cache.stats().misses, 2);
    assert!(cache.stats().source_revalidations >= 3);
}

#[test]
fn cached_source_cannot_cross_role_project_or_revoked_scope() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "scoped-private-source").unwrap();
    fs::write(other.path().join("main.rs"), "other-project-source").unwrap();
    let mut cache = ContextCache::new(8, 64 * 1024).unwrap();
    let builder = compile(&mut cache, root.path(), "builder", &grants("**"));
    let reviewer = compile(&mut cache, root.path(), "reviewer", &grants("**"));
    assert!(!reviewer.cache_hit);
    assert_ne!(builder.cache_key, reviewer.cache_key);
    let revoked = compile(&mut cache, root.path(), "builder", &Grants::default());
    assert!(!revoked.bundle.text.contains("scoped-private-source"));
    assert!(revoked.bundle.sources.is_empty());
    let alternate = compile(&mut cache, other.path(), "builder", &grants("**"));
    assert!(!alternate.cache_hit);
    assert!(!alternate.bundle.text.contains("scoped-private-source"));
}

#[test]
fn exact_source_expansion_rejects_stale_hash_and_never_proves_summary_entailment() {
    let root = tempfile::tempdir().unwrap();
    let text = "pub fn limit() -> u32 { 5 }";
    fs::write(root.path().join("main.rs"), text).unwrap();
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    let scope = grants("**");
    let request = || ExpansionRequest {
        root: root.path(),
        path: "main.rs",
        expected_hash: &hash,
        grants: &scope,
        start_byte: 0,
        end_byte: text.len(),
        byte_limit: 1024,
        secrets: &[],
    };
    let current = expand_source(request()).unwrap();
    assert_eq!(current.status, EvidenceStatus::Current);
    assert_eq!(current.claim_status, ClaimStatus::Unknown);
    assert_eq!(current.source.unwrap().text, text);
    fs::write(root.path().join("main.rs"), "pub fn limit() -> u32 { 9 }").unwrap();
    let stale = expand_source(request()).unwrap();
    assert_eq!(stale.status, EvidenceStatus::StaleSource);
    assert!(stale.source.is_none());
}

#[test]
fn secrets_are_redacted_before_retention_even_at_excerpt_boundary() {
    let root = tempfile::tempdir().unwrap();
    let secret = "THE-VERY-LONG-SECRET-MATERIAL";
    let prefix = "public-before-secret ".repeat(5);
    fs::write(
        root.path().join("main.rs"),
        format!("{prefix}{secret} public-after"),
    )
    .unwrap();
    fs::write(root.path().join(".env"), "NEVER-CACHED-PROTECTED-CANARY").unwrap();
    let mut cache = ContextCache::new(4, 64 * 1024).unwrap();
    let scope = grants("**");
    let first = compile(&mut cache, root.path(), "builder", &scope);
    let marker = "TEXT_FROM_BYTE_0\n";
    let source_start = first.bundle.text.find(marker).unwrap() + marker.len();
    let secret_start = source_start + prefix.len();
    let packet = cache
        .compile(ContextRequest {
            root: root.path(),
            project_id: "project",
            role: "builder",
            prompt: "main",
            requirements: &["Keep behavior".into()],
            grants: &scope,
            byte_limit: secret_start + 8,
            secrets: &[secret.into()],
        })
        .unwrap();
    assert!(
        !packet.bundle.text.contains("THE-VERY"),
        "partial secret must not survive truncation"
    );
    assert!(!packet.bundle.text.contains("PROTECTED-CANARY"));
    assert!(!serde_json::to_string(&packet.evidence)
        .unwrap()
        .contains("THE-VERY"));
    assert!(packet.bundle.text.len() <= secret_start + 8);
}

#[test]
fn retention_evicts_lru_and_oversized_packets_bypass_without_growth() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "source").unwrap();
    let scope = grants("**");
    let mut cache = ContextCache::new(2, 64 * 1024).unwrap();
    compile(&mut cache, root.path(), "a", &scope);
    compile(&mut cache, root.path(), "b", &scope);
    assert!(compile(&mut cache, root.path(), "a", &scope).cache_hit);
    compile(&mut cache, root.path(), "c", &scope);
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().evictions, 1);
    assert!(compile(&mut cache, root.path(), "a", &scope).cache_hit);
    assert!(!compile(&mut cache, root.path(), "b", &scope).cache_hit);
    let mut tiny = ContextCache::new(2, 1).unwrap();
    compile(&mut tiny, root.path(), "builder", &scope);
    compile(&mut tiny, root.path(), "builder", &scope);
    assert_eq!(tiny.stats().retained_bytes, 0);
    assert_eq!(tiny.stats().entries, 0);
    assert_eq!(tiny.stats().bypasses, 2);
}

#[test]
fn file_creation_deletion_and_sensitive_rename_invalidate_discovery() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("other.txt"), "other").unwrap();
    let scope = grants("**");
    let mut cache = ContextCache::new(8, 64 * 1024).unwrap();
    let first = compile(&mut cache, root.path(), "builder", &scope);
    fs::write(root.path().join("main.rs"), "new-relevant-source").unwrap();
    let added = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!added.cache_hit);
    assert_ne!(added.cache_key, first.cache_key);
    assert!(added.bundle.text.contains("new-relevant-source"));
    fs::rename(
        root.path().join("main.rs"),
        root.path().join("credentials.json"),
    )
    .unwrap();
    let hidden = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!hidden.bundle.text.contains("new-relevant-source"));
    assert!(!hidden.bundle.sources.contains_key("credentials.json"));
    fs::remove_file(root.path().join("other.txt")).unwrap();
    let removed = compile(&mut cache, root.path(), "builder", &scope);
    assert!(removed.bundle.sources.is_empty());
}

#[test]
fn expansion_denies_revoked_access_and_distinguishes_scoped_not_found() {
    let root = tempfile::tempdir().unwrap();
    let text = "approved-source";
    fs::write(root.path().join("main.rs"), text).unwrap();
    fs::write(root.path().join(".env"), text).unwrap();
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    let scope = grants("**");
    let read = |path, grants| {
        expand_source(ExpansionRequest {
            root: root.path(),
            path,
            expected_hash: &hash,
            grants,
            start_byte: 0,
            end_byte: text.len(),
            byte_limit: 1024,
            secrets: &[],
        })
        .unwrap()
    };
    let revoked = Grants::default();
    assert_eq!(
        read("main.rs", &revoked).status,
        EvidenceStatus::AccessDenied
    );
    assert_eq!(read(".env", &scope).status, EvidenceStatus::AccessDenied);
    assert_eq!(
        read("missing.rs", &scope).status,
        EvidenceStatus::NotFoundWithinScope
    );
    assert_eq!(
        read("../outside.rs", &scope).status,
        EvidenceStatus::AccessDenied
    );
}

#[test]
fn context_in_ten_thousand_file_repo_remains_bounded_and_exact_sources_expand() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("distractors")).unwrap();
    for i in 0..10_000 {
        fs::write(
            root.path().join(format!("distractors/{i:05}.txt")),
            "distractor",
        )
        .unwrap();
    }
    fs::write(root.path().join("main.rs"), "necessary-main-source").unwrap();
    fs::write(root.path().join("extra.rs"), "necessary-secondary-source").unwrap();
    let scope = grants("**");
    let mut cache = ContextCache::new(2, 64 * 1024).unwrap();
    let packet = cache
        .compile(ContextRequest {
            root: root.path(),
            project_id: "project",
            role: "builder",
            prompt: "main",
            requirements: &["extra.rs".into()],
            grants: &scope,
            byte_limit: 1024,
            secrets: &[],
        })
        .unwrap();
    assert!(packet.bundle.text.len() <= 1024);
    assert!(packet.bundle.sources.contains_key("main.rs"));
    assert!(packet.bundle.sources.contains_key("extra.rs"));
    assert!(!packet.bundle.omissions.is_empty());
    let source = packet
        .evidence
        .sources
        .iter()
        .find(|source| source.path == "extra.rs")
        .unwrap();
    let expansion = expand_source(ExpansionRequest {
        root: root.path(),
        path: "extra.rs",
        expected_hash: &source.full_file_hash,
        grants: &scope,
        start_byte: 0,
        end_byte: "necessary-secondary-source".len(),
        byte_limit: 1024,
        secrets: &[],
    })
    .unwrap();
    assert_eq!(expansion.status, EvidenceStatus::Current);
    assert_eq!(expansion.claim_status, ClaimStatus::Unknown);
    assert!(cache.stats().retained_bytes <= 64 * 1024);
}

#[test]
fn normalized_scope_order_does_not_miss_but_changed_context_inputs_do() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "bounded source").unwrap();
    let mut cache = ContextCache::new(16, 64 * 1024).unwrap();
    let first_scope = Grants {
        read: vec!["src/**".into(), "README.md".into()],
        ..Default::default()
    };
    let equivalent_scope = Grants {
        read: vec!["README.md".into(), "src\\**".into(), "src/**".into()],
        ..Default::default()
    };
    let first = compile(&mut cache, root.path(), "builder", &first_scope);
    let equivalent = compile(&mut cache, root.path(), "builder", &equivalent_scope);
    assert!(equivalent.cache_hit);
    assert_eq!(first.cache_key, equivalent.cache_key);
    let requirements = ["Keep behavior".into()];
    let mut request = ContextRequest {
        root: root.path(),
        project_id: "another-project",
        role: "builder",
        prompt: "main",
        requirements: &requirements,
        grants: &first_scope,
        byte_limit: 8192,
        secrets: &[],
    };
    assert!(!cache.compile(request).unwrap().cache_hit);
    request = ContextRequest {
        root: root.path(),
        project_id: "project",
        role: "builder",
        prompt: "new prompt",
        requirements: &requirements,
        grants: &first_scope,
        byte_limit: 8192,
        secrets: &[],
    };
    assert!(!cache.compile(request).unwrap().cache_hit);
    request = ContextRequest {
        root: root.path(),
        project_id: "project",
        role: "builder",
        prompt: "main",
        requirements: &requirements,
        grants: &first_scope,
        byte_limit: 4096,
        secrets: &[],
    };
    assert!(!cache.compile(request).unwrap().cache_hit);
    let policy_secrets = ["bounded".into()];
    request = ContextRequest {
        root: root.path(),
        project_id: "project",
        role: "builder",
        prompt: "main",
        requirements: &requirements,
        grants: &first_scope,
        byte_limit: 8192,
        secrets: &policy_secrets,
    };
    let redacted = cache.compile(request).unwrap();
    assert!(!redacted.cache_hit);
    assert!(!redacted.bundle.text.contains("bounded source"));
    assert!(redacted.bundle.text.contains("[REDACTED] source"));
    let changed_requirements = ["Different behavior".into()];
    request = ContextRequest {
        root: root.path(),
        project_id: "project",
        role: "builder",
        prompt: "main",
        requirements: &changed_requirements,
        grants: &first_scope,
        byte_limit: 8192,
        secrets: &[],
    };
    assert!(!cache.compile(request).unwrap().cache_hit);
}

#[test]
fn exact_expansion_obeys_utf8_and_byte_ranges_and_redacts_source() {
    let root = tempfile::tempdir().unwrap();
    let text = "🦀 source SECRET";
    fs::write(root.path().join("main.rs"), text).unwrap();
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    let scope = grants("**");
    let request = |start, end, limit| ExpansionRequest {
        root: root.path(),
        path: "./main.rs",
        expected_hash: &hash,
        grants: &scope,
        start_byte: start,
        end_byte: end,
        byte_limit: limit,
        secrets: &[],
    };
    assert_eq!(
        expand_source(request(1, 4, 100)).unwrap().status,
        EvidenceStatus::InvalidRange
    );
    assert_eq!(
        expand_source(request(0, 4, 3)).unwrap().status,
        EvidenceStatus::InvalidRange
    );
    let partial = expand_source(request(0, 4, 4)).unwrap().source.unwrap();
    assert_eq!(partial.text, "🦀");
    assert_eq!(partial.path, "main.rs");
    assert!(!partial.complete);
    let secrets = ["SECRET".into()];
    let mut redaction_request = request(0, text.len(), 1024);
    redaction_request.secrets = &secrets;
    let redacted = expand_source(redaction_request).unwrap().source.unwrap();
    assert!(redacted.redacted);
    assert_eq!(redacted.text, "🦀 source [REDACTED]");
}

#[cfg(unix)]
#[test]
fn cache_hit_rejects_source_replaced_by_external_symlink() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "initial-source").unwrap();
    fs::write(
        outside.path().join("secret.txt"),
        "OUTSIDE-SENSITIVE-CANARY",
    )
    .unwrap();
    let scope = grants("**");
    let mut cache = ContextCache::new(4, 64 * 1024).unwrap();
    compile(&mut cache, root.path(), "builder", &scope);
    fs::remove_file(root.path().join("main.rs")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        root.path().join("main.rs"),
    )
    .unwrap();
    let replacement = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!replacement.cache_hit);
    assert!(!replacement.bundle.text.contains("initial-source"));
    assert!(!replacement.bundle.text.contains("OUTSIDE-SENSITIVE-CANARY"));
    let status = expand_source(ExpansionRequest {
        root: root.path(),
        path: "main.rs",
        expected_hash: "old",
        grants: &scope,
        start_byte: 0,
        end_byte: 1,
        byte_limit: 1024,
        secrets: &[],
    })
    .unwrap()
    .status;
    assert_eq!(status, EvidenceStatus::InvalidSource);
}

#[test]
fn source_becoming_oversized_or_binary_cannot_reuse_earlier_cache() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.rs");
    fs::write(&path, "earlier-source").unwrap();
    let scope = grants("**");
    let mut cache = ContextCache::new(4, 64 * 1024).unwrap();
    compile(&mut cache, root.path(), "builder", &scope);
    fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let huge = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!huge.cache_hit);
    assert!(huge.bundle.sources.is_empty());
    assert!(huge
        .bundle
        .omissions
        .iter()
        .any(|text| text.contains("read limit")));
    let expansion = expand_source(ExpansionRequest {
        root: root.path(),
        path: "main.rs",
        expected_hash: "old",
        grants: &scope,
        start_byte: 0,
        end_byte: 1,
        byte_limit: 1024,
        secrets: &[],
    })
    .unwrap();
    assert_eq!(expansion.status, EvidenceStatus::ReadLimitExceeded);
    fs::write(&path, b"binary\0source").unwrap();
    let binary = compile(&mut cache, root.path(), "builder", &scope);
    assert!(!binary.cache_hit);
    assert!(binary.bundle.sources.is_empty());
    assert!(!binary.bundle.text.contains("earlier-source"));
}

#[test]
fn exact_expansion_never_exposes_partial_known_secret_through_a_byte_range() {
    let root = tempfile::tempdir().unwrap();
    let text = "prefix SUPER-SECRET-CREDENTIAL suffix";
    let secret = "SUPER-SECRET-CREDENTIAL";
    let secrets = [secret.into()];
    fs::write(root.path().join("main.rs"), text).unwrap();
    let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
    let scope = grants("**");
    for (start, end) in [(0, 12), (12, text.len())] {
        let result = expand_source(ExpansionRequest {
            root: root.path(),
            path: "main.rs",
            expected_hash: &hash,
            grants: &scope,
            start_byte: start,
            end_byte: end,
            byte_limit: 1024,
            secrets: &secrets,
        })
        .unwrap();
        assert_eq!(result.status, EvidenceStatus::InvalidRange);
        assert!(result.source.is_none());
    }
}

#[test]
fn byte_capacity_evicts_before_entry_capacity_is_reached() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "capacity-bound-source").unwrap();
    let scope = grants("**");
    let mut probe = ContextCache::new(16, 64 * 1024).unwrap();
    compile(&mut probe, root.path(), "a", &scope);
    let one_packet_bytes = probe.stats().retained_bytes;
    let mut cache = ContextCache::new(16, one_packet_bytes + 16).unwrap();
    compile(&mut cache, root.path(), "a", &scope);
    compile(&mut cache, root.path(), "b", &scope);
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(cache.stats().evictions, 1);
    assert!(cache.stats().retained_bytes <= one_packet_bytes + 16);
    assert!(compile(&mut cache, root.path(), "b", &scope).cache_hit);
    assert!(!compile(&mut cache, root.path(), "a", &scope).cache_hit);
}

#[test]
fn invalid_cache_configuration_and_unscoped_context_requests_are_rejected() {
    assert!(ContextCache::new(0, 100).is_err());
    assert!(ContextCache::new(1, 0).is_err());
    let root = tempfile::tempdir().unwrap();
    let mut cache = ContextCache::new(1, 4096).unwrap();
    let scope = grants("**");
    let request = ContextRequest {
        root: root.path(),
        project_id: "",
        role: "builder",
        prompt: "task",
        requirements: &[],
        grants: &scope,
        byte_limit: 1024,
        secrets: &[],
    };
    assert!(cache.compile(request).is_err());
    assert_eq!(cache.stats().entries, 0);
    let request = ContextRequest {
        root: root.path(),
        project_id: "project",
        role: "",
        prompt: "task",
        requirements: &[],
        grants: &scope,
        byte_limit: 1024,
        secrets: &[],
    };
    assert!(cache.compile(request).is_err());
    assert_eq!(cache.stats().entries, 0);
}
