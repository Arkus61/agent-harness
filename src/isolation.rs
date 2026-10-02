//! Probed Linux filesystem, credential-file, PID, and network isolation.
//!
//! The trusted host coordinator supplies an immutable runtime configuration;
//! hostile concurrent processes with the same host UID are outside this threat
//! model. This boundary does not grant a command finer read/write ownership
//! than its mounted workspace. The gateway and post-command diff enforce that
//! policy separately. Unsupported systems fail closed without native fallback.

use crate::types::{CommandResult, CommandSpec};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize)]
pub struct IsolationCapability {
    pub available: bool,
    pub backend: Option<String>,
    pub filesystem_confinement: bool,
    pub network_confinement: bool,
    pub credential_file_confinement: bool,
    pub reason: Option<String>,
    pub limitations: Vec<String>,
}

fn unsupported(reason: impl Into<String>) -> IsolationCapability {
    IsolationCapability {
        available: false,
        backend: None,
        filesystem_confinement: false,
        network_confinement: false,
        credential_file_confinement: false,
        reason: Some(reason.into()),
        limitations: vec!["Automatic downgrade to native-trusted is forbidden".into()],
    }
}

/// Run the actual namespace probe once per coordinator, with a three-second
/// bound. Availability is never inferred solely from an installed executable.
pub fn availability() -> IsolationCapability {
    static CAPABILITY: OnceLock<IsolationCapability> = OnceLock::new();
    CAPABILITY
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                match linux::probe() {
                    Ok(()) => IsolationCapability {
                        available: true,
                        backend: Some("linux-bubblewrap".into()),
                        filesystem_confinement: true,
                        network_confinement: true,
                        credential_file_confinement: true,
                        reason: None,
                        limitations: vec![
                            "Only Linux has an implemented and probed isolation backend".into(),
                            "Trusted system/toolchain mounts are read-only; the workspace remains a command resource, not an internal syscall ownership policy".into(),
                            "Absent protected paths freeze their nearest existing ancestor; commands can have less write access than gateway edits".into(),
                            "Separate malicious same-UID host races and kernel vulnerabilities are outside the boundary".into(),
                            "Timeout, output limits and process cleanup do not establish CPU/memory/disk quotas".into(),
                        ],
                    },
                    Err(error) => unsupported(format!("Linux isolation probe failed: {error:#}")),
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                unsupported("No implemented isolation backend for this operating system")
            }
        })
        .clone()
}

pub async fn run_isolated(
    cwd: &Path,
    spec: &CommandSpec,
    cancel: CancellationToken,
    output_limit: usize,
    protected_patterns: &[String],
) -> Result<CommandResult> {
    let capability = availability();
    ensure!(
        capability.available,
        "isolated execution unavailable: {}",
        capability
            .reason
            .as_deref()
            .unwrap_or("capability probe failed")
    );
    #[cfg(target_os = "linux")]
    {
        let (root, wrapped) = linux::prepare(cwd, spec, protected_patterns)?;
        crate::execution::run_command(&root, &wrapped, cancel, output_limit).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cwd, spec, cancel, output_limit, protected_patterns);
        anyhow::bail!("isolated execution is unsupported on this operating system")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::{bail, Context};
    use std::fs::{self, Metadata};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const BWRAP: &str = "/usr/bin/bwrap";
    const SCAN_LIMIT: usize = 100_000;

    fn checked_path(path: &Path, directory: bool, system: bool) -> Result<PathBuf> {
        ensure!(
            path.is_absolute(),
            "isolation mount source must be absolute"
        );
        let canonical = path
            .canonicalize()
            .context("resolve isolation mount source")?;
        ensure!(
            canonical == path,
            "isolation mount source cannot traverse symlinks"
        );
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "isolation mount source cannot be a symlink"
        );
        ensure!(
            if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            },
            "invalid isolation mount source type"
        );
        let uid = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == 0 || (!system && metadata.uid() == uid),
            "untrusted isolation mount owner"
        );
        ensure!(
            metadata.mode() & 0o022 == 0,
            "isolation mount source is group/world writable"
        );
        Ok(canonical)
    }

    fn source(value: &Path) -> Result<String> {
        value
            .to_str()
            .map(str::to_owned)
            .context("isolation mount paths must be UTF-8")
    }

    fn pair(args: &mut Vec<String>, flag: &str, first: &str, second: &str) {
        args.extend([flag.into(), first.into(), second.into()]);
    }

    fn runtime_args() -> Result<Vec<String>> {
        checked_path(Path::new(BWRAP), false, true)?;
        let mut args: Vec<String> = [
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for path in [
            "/usr/bin",
            "/usr/lib",
            "/usr/lib64",
            "/usr/libexec",
            "/usr/include",
        ] {
            if Path::new(path).exists() {
                checked_path(Path::new(path), true, true)?;
                pair(&mut args, "--ro-bind", path, path);
            }
        }
        for (target, link) in [
            ("usr/bin", "/bin"),
            ("usr/lib", "/lib"),
            ("usr/lib64", "/lib64"),
        ] {
            pair(&mut args, "--symlink", target, link);
        }
        for path in ["/etc", "/tmp", "/home", "/opt"] {
            args.extend(["--tmpfs".into(), path.into()]);
        }
        // Debian's cc/ld entries resolve through alternatives. Recreate only
        // vetted symlinks into the already mounted trusted runtime; do not bind
        // any host /etc directory or expose its other configuration.
        let alternatives = Path::new("/etc/alternatives");
        if alternatives.exists() {
            checked_path(alternatives, true, true)?;
            args.extend(["--dir".into(), "/etc/alternatives".into()]);
            for entry in fs::read_dir(alternatives)? {
                let path = entry?.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.uid() != 0 || !metadata.file_type().is_symlink() {
                    continue;
                }
                if let Ok(target) = path.canonicalize() {
                    if target.is_file()
                        && ["/usr/bin", "/usr/lib", "/usr/lib64", "/usr/libexec"]
                            .iter()
                            .any(|prefix| target.starts_with(prefix))
                    {
                        checked_path(&target, false, true)?;
                        pair(&mut args, "--symlink", &source(&target)?, &source(&path)?);
                    }
                }
            }
        }
        args.extend([
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--dir".into(),
            "/home/sandbox".into(),
        ]);
        pair(&mut args, "--setenv", "HOME", "/home/sandbox");
        pair(
            &mut args,
            "--setenv",
            "PATH",
            "/opt/harness/cargo/bin:/usr/bin:/bin",
        );
        pair(&mut args, "--setenv", "TMPDIR", "/tmp");
        pair(&mut args, "--setenv", "LANG", "C.UTF-8");
        pair(&mut args, "--setenv", "HARNESS_RUNTIME_PROFILE", "isolated");
        pair(&mut args, "--setenv", "GIT_CONFIG_NOSYSTEM", "1");
        pair(&mut args, "--setenv", "GIT_CONFIG_GLOBAL", "/dev/null");
        pair(&mut args, "--setenv", "GIT_TERMINAL_PROMPT", "0");
        // A private target avoids writing build products into an immutable
        // acceptance tree. Cache sharing can be added through a separate grant.
        pair(
            &mut args,
            "--setenv",
            "CARGO_TARGET_DIR",
            "/tmp/cargo-target",
        );
        Ok(args)
    }

    pub(super) fn probe() -> Result<()> {
        let mut args = runtime_args()?;
        args.extend([
            "--chdir".into(),
            "/tmp".into(),
            "--".into(),
            "/usr/bin/true".into(),
        ]);
        let mut child = Command::new(BWRAP)
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn isolation probe")?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "namespace probe exited with {status}");
                return Ok(());
            }
            if started.elapsed() >= Duration::from_secs(3) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("namespace probe timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn configured_home(key: &str, suffix: &str) -> Option<PathBuf> {
        std::env::var_os(key)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(suffix)))
    }

    fn toolchain_args(
        args: &mut Vec<String>,
        cargo: Option<PathBuf>,
        rustup: Option<PathBuf>,
    ) -> Result<()> {
        args.extend([
            "--dir".into(),
            "/opt/harness".into(),
            "--dir".into(),
            "/opt/harness/cargo".into(),
            "--dir".into(),
            "/opt/harness/rustup".into(),
        ]);
        pair(args, "--setenv", "CARGO_HOME", "/opt/harness/cargo");
        pair(args, "--setenv", "RUSTUP_HOME", "/opt/harness/rustup");
        if let Some(home) = cargo.as_ref().filter(|path| path.exists()) {
            checked_path(home, true, false)?;
            for name in ["bin", "registry"] {
                let path = home.join(name);
                if path.exists() {
                    checked_path(&path, true, false)?;
                    pair(
                        args,
                        "--ro-bind",
                        &source(&path)?,
                        &format!("/opt/harness/cargo/{name}"),
                    );
                    mask_sensitive(
                        args,
                        &path,
                        &Path::new("/opt/harness/cargo").join(name),
                        &scan(&path)?,
                    )?;
                }
            }
        }
        if let Some(home) = rustup.as_ref().filter(|path| path.exists()) {
            checked_path(home, true, false)?;
            for (name, directory) in [("toolchains", true), ("settings.toml", false)] {
                let path = home.join(name);
                if path.exists() {
                    checked_path(&path, directory, false)?;
                    pair(
                        args,
                        "--ro-bind",
                        &source(&path)?,
                        &format!("/opt/harness/rustup/{name}"),
                    );
                    if directory {
                        mask_sensitive(
                            args,
                            &path,
                            &Path::new("/opt/harness/rustup").join(name),
                            &scan(&path)?,
                        )?;
                    }
                }
            }
        }
        if let Ok(toolchain) = std::env::var("RUSTUP_TOOLCHAIN") {
            ensure!(
                !toolchain.is_empty()
                    && toolchain
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch)),
                "untrusted RUSTUP_TOOLCHAIN value"
            );
            pair(args, "--setenv", "RUSTUP_TOOLCHAIN", &toolchain);
        }
        Ok(())
    }

    struct Entry {
        path: PathBuf,
        metadata: Metadata,
    }

    fn scan(root: &Path) -> Result<Vec<Entry>> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(parent) = pending.pop() {
            for item in fs::read_dir(&parent).context("inspect workspace isolation boundary")? {
                let path = item?.path();
                let metadata = fs::symlink_metadata(&path)?;
                ensure!(
                    found.len() < SCAN_LIMIT,
                    "workspace isolation scan exceeds 100000 entries"
                );
                if metadata.is_file() {
                    ensure!(
                        metadata.nlink() <= 1,
                        "workspace hardlinks are forbidden in isolated execution"
                    );
                }
                // Special nodes can expose host devices through a workspace.
                ensure!(
                    metadata.is_file() || metadata.is_dir() || metadata.file_type().is_symlink(),
                    "special files are forbidden in isolated workspaces"
                );
                let relative = path.strip_prefix(root)?;
                let sensitive = crate::policy::is_sensitive_path(relative);
                if sensitive {
                    ensure!(
                        !metadata.file_type().is_symlink(),
                        "sensitive workspace paths cannot be symlinks"
                    );
                } else if metadata.is_dir() {
                    pending.push(path.clone());
                }
                found.push(Entry { path, metadata });
            }
        }
        Ok(found)
    }

    fn mask_sensitive(
        args: &mut Vec<String>,
        source_root: &Path,
        target_root: &Path,
        entries: &[Entry],
    ) -> Result<()> {
        for entry in entries.iter().filter(|entry| {
            crate::policy::is_sensitive_path(
                entry
                    .path
                    .strip_prefix(source_root)
                    .unwrap_or(Path::new("")),
            )
        }) {
            let target = target_root.join(entry.path.strip_prefix(source_root)?);
            if entry.metadata.is_dir() {
                args.extend([
                    "--size".into(),
                    "4096".into(),
                    "--tmpfs".into(),
                    source(&target)?,
                    "--remount-ro".into(),
                    source(&target)?,
                ]);
            } else {
                pair(args, "--ro-bind", "/dev/null", &source(&target)?);
            }
        }
        Ok(())
    }

    fn protected_ancestors(
        root: &Path,
        patterns: &[String],
        entries: &[Entry],
    ) -> Result<Vec<PathBuf>> {
        let mut frozen = Vec::new();
        for pattern in patterns {
            ensure!(
                !pattern.is_empty() && !pattern.contains(['\\', '\0', ':']),
                "invalid protected isolation path"
            );
            let pattern_path = Path::new(pattern);
            ensure!(
                pattern_path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
                "protected isolation paths must be relative without traversal"
            );
            let matcher = globset::Glob::new(pattern)?.compile_matcher();
            for entry in entries {
                if matcher.is_match(entry.path.strip_prefix(root)?) {
                    ensure!(
                        !entry.metadata.file_type().is_symlink(),
                        "protected paths cannot be symlinks"
                    );
                }
            }
            let mut prefix = PathBuf::new();
            for component in pattern_path.components() {
                let text = component.as_os_str().to_string_lossy();
                if text.contains(['*', '?', '[', '{']) {
                    break;
                }
                prefix.push(component.as_os_str());
            }
            let desired = root.join(&prefix);
            let mut existing = desired.clone();
            while fs::symlink_metadata(&existing)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                ensure!(
                    existing.pop() && existing.starts_with(root),
                    "protected path escaped workspace"
                );
            }
            // A bind-file alone would allow its containing directory to be
            // renamed and recreated. Freeze the containing directory instead.
            if fs::symlink_metadata(&existing)?.is_file() {
                existing = existing
                    .parent()
                    .context("protected path has no parent")?
                    .to_path_buf();
            }
            ensure!(
                existing.starts_with(root),
                "protected ancestor escaped workspace"
            );
            ensure!(
                existing.canonicalize()? == existing && fs::symlink_metadata(&existing)?.is_dir(),
                "protected ancestor cannot traverse a symlink"
            );
            frozen.push(existing);
        }
        frozen.sort_by_key(|path| path.components().count());
        let mut minimal: Vec<PathBuf> = Vec::new();
        for path in frozen {
            if !minimal.iter().any(|parent| path.starts_with(parent)) {
                minimal.push(path);
            }
        }
        Ok(minimal)
    }

    pub(super) fn prepare(
        cwd: &Path,
        spec: &CommandSpec,
        protected_patterns: &[String],
    ) -> Result<(PathBuf, CommandSpec)> {
        ensure!(
            !spec.program.is_empty()
                && !spec.program.contains('\0')
                && !spec.args.iter().any(|arg| arg.contains('\0')),
            "invalid command argv"
        );
        ensure!(spec.timeout_secs > 0, "command timeout must be positive");
        let root = checked_path(cwd, true, false)?;
        ensure!(
            root != Path::new("/"),
            "host root cannot be an isolated workspace"
        );
        let entries = scan(&root)?;
        let frozen = protected_ancestors(&root, protected_patterns, &entries)?;
        let mut args = runtime_args()?;
        let cargo = configured_home("CARGO_HOME", ".cargo");
        let rustup = configured_home("RUSTUP_HOME", ".rustup");
        toolchain_args(&mut args, cargo.clone(), rustup.clone())?;
        pair(&mut args, "--bind", &source(&root)?, "/workspace");
        for path in frozen {
            let target = Path::new("/workspace").join(path.strip_prefix(&root)?);
            pair(&mut args, "--ro-bind", &source(&path)?, &source(&target)?);
        }
        mask_sensitive(&mut args, &root, Path::new("/workspace"), &entries)?;
        let mut program = spec.program.clone();
        if Path::new(&program).is_absolute() {
            let original = PathBuf::from(&program);
            let mappings = [
                (Some(root.clone()), "/workspace"),
                (cargo, "/opt/harness/cargo"),
                (rustup, "/opt/harness/rustup"),
            ];
            let mut mapped = false;
            for (prefix, target) in mappings {
                if let Some(prefix) = prefix {
                    if let Ok(relative) = original.strip_prefix(prefix) {
                        program = source(&Path::new(target).join(relative))?;
                        mapped = true;
                        break;
                    }
                }
            }
            ensure!(
                mapped
                    || ["/usr/bin", "/usr/lib", "/bin", "/lib"]
                        .iter()
                        .any(|prefix| original.starts_with(prefix)),
                "command executable is outside trusted sandbox mounts"
            );
        }
        args.extend(["--chdir".into(), "/workspace".into(), "--".into(), program]);
        args.extend(spec.args.clone());
        Ok((
            root,
            CommandSpec {
                program: BWRAP.into(),
                args,
                timeout_secs: spec.timeout_secs,
                resource_limits: spec.resource_limits.clone(),
            },
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn legacy_registry_git_config_and_runtime_credential_names_are_hidden() {
            if !availability().available {
                return;
            }
            let root = tempfile::tempdir().unwrap();
            let cargo = root.path().join("cargo");
            let registry = cargo.join("registry/index/private/.git");
            fs::create_dir_all(&registry).unwrap();
            fs::write(
                registry.join("config"),
                "[remote]\nurl = https://credential-canary@example.invalid\n",
            )
            .unwrap();
            fs::write(
                cargo.join("credentials.toml"),
                "top-level-credential-canary",
            )
            .unwrap();
            let cache_auth = cargo.join("registry/auth.json");
            fs::write(&cache_auth, "runtime-auth-canary").unwrap();
            let mut args = runtime_args().unwrap();
            toolchain_args(&mut args, Some(cargo), None).unwrap();
            args.extend(["--chdir".into(), "/tmp".into(), "--".into(), "sh".into(), "-c".into(), "set -eu; test ! -e /opt/harness/cargo/credentials.toml; test ! -e /opt/harness/cargo/registry/index/private/.git/config; test ! -s /opt/harness/cargo/registry/auth.json; ! printf bad > /opt/harness/cargo/registry/auth.json; printf runtime-confined".into()]);
            let wrapped = CommandSpec {
                program: BWRAP.into(),
                args,
                timeout_secs: 10,
                resource_limits: None,
            };
            let result = crate::execution::run_command(
                root.path(),
                &wrapped,
                CancellationToken::new(),
                8192,
            )
            .await
            .unwrap();
            assert_eq!(result.exit_code, Some(0), "{result:?}");
            assert_eq!(result.stdout, "runtime-confined");
            assert_eq!(
                fs::read_to_string(cache_auth).unwrap(),
                "runtime-auth-canary"
            );
        }
    }
}
