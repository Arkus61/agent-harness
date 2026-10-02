//! Exercise the actual Linux boundary; never substitute native execution.
#![cfg(target_os = "linux")]

use agent_harness::isolation::{availability, run_isolated};
use agent_harness::types::CommandSpec;
use std::fs;
use tokio_util::sync::CancellationToken;

fn shell(script: &str, timeout_secs: u64) -> CommandSpec {
    CommandSpec {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        timeout_secs,
    }
}

fn available() -> bool {
    let capability = availability();
    if std::env::var("HARNESS_REQUIRE_LINUX_ISOLATION").as_deref() == Ok("1") {
        assert!(
            capability.available,
            "required isolation unavailable: {capability:?}"
        );
    }
    if !capability.available {
        eprintln!("Linux isolation unavailable: {:?}", capability.reason);
    }
    capability.available
}

#[tokio::test]
async fn real_boundary_allows_workspace_but_hides_host_and_credentials() {
    if !available() {
        return;
    }
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("worktree");
    fs::create_dir(&root).unwrap();
    let sentinel = parent.path().join("host-secret.txt");
    fs::write(&sentinel, "host-private-sentinel").unwrap();
    fs::write(root.join(".git"), "gitdir: /outside/repository").unwrap();
    fs::write(root.join(".env"), "MODEL_TOKEN=private-value").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/auth.json"), "private-auth").unwrap();
    std::os::unix::fs::symlink(&sentinel, root.join("outside-link")).unwrap();
    let script = format!(
        "set -eu; test \"$HOME\" = /home/sandbox; test \"$HARNESS_RUNTIME_PROFILE\" = isolated; \
         test ! -e '{}'; test ! -e /etc/passwd; test ! -e /sys; \
         test ! -s .git; test ! -s .env; test ! -s nested/auth.json; \
         test ! -e outside-link; test -z \"${{OPENAI_API_KEY-}}\"; \
         test -z \"${{CODEX_HOME-}}\"; test -z \"${{LD_PRELOAD-}}\"; \
         printf allowed > mutable.txt; ! printf bad > .git; ! printf bad > .env; \
         ! mv .env moved-env; printf confined",
        sentinel.display()
    );
    let result = run_isolated(
        &root,
        &shell(&script, 10),
        CancellationToken::new(),
        8192,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(result.stdout, "confined");
    assert_eq!(
        fs::read_to_string(root.join("mutable.txt")).unwrap(),
        "allowed"
    );
    assert_eq!(
        fs::read_to_string(&sentinel).unwrap(),
        "host-private-sentinel"
    );
    assert_eq!(
        fs::read_to_string(root.join(".env")).unwrap(),
        "MODEL_TOKEN=private-value"
    );
    assert_eq!(
        fs::read_to_string(root.join(".git")).unwrap(),
        "gitdir: /outside/repository"
    );
}

#[tokio::test]
async fn protected_nested_source_and_parent_cannot_be_replaced() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("acceptance")).unwrap();
    fs::write(root.path().join("acceptance/oracle.rs"), "trusted-oracle").unwrap();
    let result = run_isolated(
        root.path(),
        &shell("set -eu; ! printf bad > acceptance/oracle.rs; ! rm acceptance/oracle.rs; ! mv acceptance moved; ! printf new > acceptance/new.rs; printf good > source.rs", 10),
        CancellationToken::new(),
        8192,
        &["acceptance/oracle.rs".into()],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("acceptance/oracle.rs")).unwrap(),
        "trusted-oracle"
    );
    assert!(!root.path().join("acceptance/new.rs").exists());
    assert_eq!(
        fs::read_to_string(root.path().join("source.rs")).unwrap(),
        "good"
    );
}

#[tokio::test]
async fn missing_protected_prefix_freezes_existing_ancestor() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("source.rs"), "source").unwrap();
    let result = run_isolated(
        root.path(),
        &shell(
            "set -eu; ! mkdir acceptance; ! printf bad > source.rs; printf allowed > /tmp/output",
            10,
        ),
        CancellationToken::new(),
        8192,
        &["acceptance/**".into()],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("source.rs")).unwrap(),
        "source"
    );
    assert!(!root.path().join("acceptance").exists());
}

#[tokio::test]
async fn hardlink_and_protected_symlink_mounts_fail_closed() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("oracle"), "trusted").unwrap();
    fs::hard_link(root.path().join("oracle"), root.path().join("alias")).unwrap();
    let error = run_isolated(
        root.path(),
        &shell("printf changed > alias", 10),
        CancellationToken::new(),
        8192,
        &["oracle".into()],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("hardlink"), "{error:#}");
    assert_eq!(
        fs::read_to_string(root.path().join("oracle")).unwrap(),
        "trusted"
    );
    fs::remove_file(root.path().join("alias")).unwrap();
    std::os::unix::fs::symlink("oracle", root.path().join("protected-link")).unwrap();
    let error = run_isolated(
        root.path(),
        &shell("true", 10),
        CancellationToken::new(),
        8192,
        &["protected-link".into()],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("symlink"), "{error:#}");
}

#[tokio::test]
async fn host_loopback_and_nested_namespaces_are_unreachable() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let script = format!(
        "set -eu; ! unshare --user --map-root-user /bin/true; \
         python3 -c 'import socket; s=socket.socket(); s.settimeout(0.5); \
         assert s.connect_ex((\"127.0.0.1\", {port})) != 0; print(\"network-confined\")'"
    );
    let result = run_isolated(
        root.path(),
        &shell(&script, 10),
        CancellationToken::new(),
        8192,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stdout.contains("network-confined"));
}

#[tokio::test]
async fn cargo_offline_check_runs_with_protected_manifest_and_no_host_home() {
    if !available()
        || std::env::var_os("CARGO_HOME").is_none()
        || std::env::var_os("RUSTUP_HOME").is_none()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"isolation-proof\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.path().join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"isolation-proof\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "#[test] fn acceptance() { assert_eq!(2 + 2, 4); }\n",
    )
    .unwrap();
    let spec = CommandSpec {
        program: "cargo".into(),
        args: vec!["test".into(), "--locked".into(), "--offline".into()],
        timeout_secs: 60,
    };
    let result = run_isolated(
        root.path(),
        &spec,
        CancellationToken::new(),
        64 * 1024,
        &[
            "Cargo.toml".into(),
            "Cargo.lock".into(),
            "acceptance/**".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stdout.contains("1 passed"), "{result:?}");
    assert!(!root.path().join("target").exists());
    assert!(!root.path().join("acceptance").exists());
}

#[tokio::test]
async fn timeout_kills_detached_sandbox_descendants() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let result = run_isolated(
        root.path(),
        &shell(
            "setsid sh -c 'while :; do printf x >> heartbeat; sleep 0.02; done' & sleep 30",
            1,
        ),
        CancellationToken::new(),
        8192,
        &[],
    )
    .await
    .unwrap();
    assert!(result.timed_out, "{result:?}");
    let heartbeat = root.path().join("heartbeat");
    let length = fs::metadata(&heartbeat).unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(fs::metadata(&heartbeat).unwrap().len(), length);
}
