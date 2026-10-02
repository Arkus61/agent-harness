//! Adversarial fixtures use synthetic canaries, never host credentials.
use agent_harness::types::Grants;
use agent_harness::{context, execution};

fn broad_grants() -> Grants {
    Grants {
        read: vec!["**".into()],
        write: vec!["**".into()],
        commands: vec![],
    }
}

#[test]
fn managed_worktree_ancestors_do_not_remove_readable_sources() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent
        .path()
        .join(".git/harness/worktrees/synthetic-worktree");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn readable() -> bool { true }",
    )
    .unwrap();
    std::fs::write(root.join(".npmrc"), "SYNTHETIC_NPM_SECRET").unwrap();
    let bundle = context::compile(&root, "Inspect sources", &[], &broad_grants(), 32_000).unwrap();
    assert!(
        bundle.sources.contains_key("src/lib.rs"),
        "managed sources were omitted: {:?}",
        bundle.omissions
    );
    assert!(bundle.text.contains("pub fn readable()"));
    assert!(!bundle.text.contains("SYNTHETIC_NPM_SECRET"));
}

#[test]
fn automatic_context_cannot_upload_gateway_protected_credentials() {
    let cases = [
        ".npmrc",
        ".netrc",
        ".pypirc",
        ".azure/accessTokens.json",
        "id_ecdsa",
        ".SSH/config",
        ".GIT/config",
        ".HArNeSS/runtime.sqlite",
    ];
    let mut exposed = Vec::new();
    for (index, path) in cases.iter().enumerate() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let canary = format!("SYNTHETIC_PROTECTED_CREDENTIAL_CANARY_{index}");
        std::fs::write(&target, &canary).unwrap();
        assert!(
            execution::read_file(root.path(), path, &broad_grants(), 1024).is_err(),
            "fixture {path} must already be protected by explicit gateway reads"
        );
        let bundle = context::compile(
            root.path(),
            "Inspect this repository",
            &["Sources are reviewed".into()],
            &broad_grants(),
            32_000,
        )
        .unwrap();
        if bundle.text.contains(&canary) || bundle.sources.contains_key(*path) {
            exposed.push(*path);
        }
    }
    assert!(
        exposed.is_empty(),
        "automatic context exposed paths explicitly denied by the gateway: {exposed:?}"
    );
}

#[test]
fn explicit_gateway_cannot_read_sensitive_context_exclusions() {
    let cases = [
        "certificate.p12",
        "certificate.PFX",
        ".gnupg/private_material",
        "secrets",
        "secrets.toml",
    ];
    let mut exposed = Vec::new();
    for path in cases {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "SYNTHETIC_SECRET").unwrap();
        if execution::read_file(root.path(), path, &broad_grants(), 1024).is_ok() {
            exposed.push(path);
        }
    }
    assert!(
        exposed.is_empty(),
        "explicit read bypassed sensitive files excluded from automatic context: {exposed:?}"
    );
}
