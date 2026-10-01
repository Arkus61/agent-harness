//! Git, filesystem gateway, and bounded native process execution.
//!
//! `native-trusted` commands run with the user's filesystem and network rights.
//! A worktree, environment filtering, and a process group are not a sandbox.
//! The filesystem gateway rejects links, but cannot confine arbitrary commands
//! or hostile same-user races. `doctor` therefore never advertises isolation.

use crate::types::{Action, CommandResult, CommandSpec, Grants};
use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

pub struct Repo {
    pub root: PathBuf,
    pub common_dir: PathBuf,
    pub state_dir: PathBuf,
}

pub struct RepoLock {
    file: File,
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

fn basic_git(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(cwd).env_clear();
    copy_runtime_environment_std(&mut command);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_CONFIG_SYSTEM", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "tag.gpgSign=false",
            "-c",
            "diff.external=",
            "-c",
            "core.pager=cat",
            "-c",
            "core.quotePath=false",
            "-c",
            "user.name=Agent Harness",
            "-c",
            "user.email=harness@localhost",
        ]);
    command
        .arg("-c")
        .arg(format!("core.attributesFile={}", null_device()));
    command
        .arg("-c")
        .arg(format!("core.hooksPath={}", null_device()));
    command
}

/// Disable every configured clean/smudge/process driver before Git reads files.
/// Global config is disabled; this also covers drivers imported by local includes.
fn safe_git(cwd: &Path) -> Result<Command> {
    let result = basic_git(cwd)
        .args([
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^(filter\..*\.(clean|smudge|process|required)|diff\..*\.(command|textconv))$",
        ])
        .output()
        .context("read local Git filter configuration")?;
    ensure!(
        result.status.success() || result.status.code() == Some(1),
        "cannot inspect Git filters: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut command = basic_git(cwd);
    for raw in result
        .stdout
        .split(|b| *b == 0)
        .filter(|key| !key.is_empty())
    {
        let key = std::str::from_utf8(raw).context("invalid Git filter key")?;
        let value = if key.ends_with(".required") {
            "false"
        } else {
            ""
        };
        command.arg("-c").arg(format!("{key}={value}"));
    }
    Ok(command)
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = safe_git(cwd)?.args(args).output().context("execute Git")?;
    ensure!(
        output.status.success(),
        "Git {} failed: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)
        .context("Git output is not UTF-8")?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

impl Repo {
    pub fn discover(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .context("repository path does not exist")?;
        let root =
            PathBuf::from(git_output(&path, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        let common_dir = PathBuf::from(git_output(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?)
        .canonicalize()?;
        let state_dir = common_dir.join("harness");
        if state_dir.exists() {
            ensure!(
                !is_link(&fs::symlink_metadata(&state_dir)?),
                "harness state directory cannot be a link"
            );
        }
        fs::create_dir_all(state_dir.join("worktrees"))?;
        ensure!(
            state_dir.canonicalize()?.starts_with(&common_dir),
            "state directory escaped Git common directory"
        );
        Ok(Self {
            root,
            common_dir,
            state_dir,
        })
    }

    pub fn git(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        git_output(cwd, args)
    }

    pub fn head(&self) -> Result<String> {
        self.git(&self.root, &["rev-parse", "--verify", "HEAD^{commit}"])
    }

    pub fn head_at(&self, path: &Path) -> Result<String> {
        self.git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
    }

    pub fn lock(&self) -> Result<RepoLock> {
        let path = self.common_dir.join("harness.lock");
        if path.exists() {
            ensure!(
                !is_link(&fs::symlink_metadata(&path)?),
                "lock cannot be a link"
            );
        }
        let file = nofollow_open(&path, true, true, false)?;
        file.try_lock_exclusive()
            .context("another harness coordinator holds this repository lock")?;
        Ok(RepoLock { file })
    }

    fn commit_id(&self, value: &str) -> Result<String> {
        ensure!(
            !value.is_empty() && !value.starts_with('-'),
            "invalid commit revision"
        );
        self.git(
            &self.root,
            &["rev-parse", "--verify", &format!("{value}^{{commit}}")],
        )
    }

    pub fn create_worktree(&self, path: &Path, sha: &str) -> Result<()> {
        let workspace_root = self.state_dir.join("worktrees").canonicalize()?;
        let relative = path
            .strip_prefix(&workspace_root)
            .context("worktrees must be beneath Git common_dir/harness/worktrees")?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
            "invalid worktree path"
        );
        let path = scoped_path(&workspace_root, relative, true)?;
        ensure!(
            fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "worktree already exists or is inaccessible"
        );
        let sha = self.commit_id(sha)?;
        let mut cmd = safe_git(&self.root)?;
        let output = cmd
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg(&sha)
            .output()?;
        ensure!(
            output.status.success(),
            "create worktree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    pub fn commit(&self, path: &Path, message: &str) -> Result<String> {
        self.git(path, &["add", "-A", "--", "."])?;
        let staged = self.git(
            path,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--no-ext-diff",
                "--no-textconv",
            ],
        )?;
        if !staged.is_empty() {
            self.git(
                path,
                &["commit", "--no-verify", "--no-gpg-sign", "-m", message],
            )?;
        }
        self.git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
    }

    /// Integrate each attempt's own input..output delta, never its whole branch.
    pub fn integrate(
        &self,
        path: &Path,
        base: &str,
        deltas: &[(String, String)],
    ) -> Result<String> {
        let base = self.commit_id(base)?;
        let workspace_root = self.state_dir.join("worktrees").canonicalize()?;
        let relative = path
            .strip_prefix(&workspace_root)
            .context("integration may only reset a managed worktree")?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
            "invalid integration worktree path"
        );
        let managed = scoped_path(&workspace_root, relative, false)?;
        if !managed.exists() {
            self.create_worktree(&managed, &base)?;
        }
        ensure!(
            managed.is_dir(),
            "integration path must be a worktree directory"
        );
        let top_level =
            PathBuf::from(self.git(&managed, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        let common = PathBuf::from(self.git(
            &managed,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?)
        .canonicalize()?;
        ensure!(
            top_level == managed.canonicalize()? && common == self.common_dir,
            "integration path is not an owned worktree of this repository"
        );
        let path = managed.as_path();
        self.git(path, &["reset", "--hard", &base])?;
        self.git(path, &["clean", "-fd", "--"])?;
        let mut seen = HashSet::new();
        for (input, output) in deltas {
            let input = self.commit_id(input)?;
            let output = self.commit_id(output)?;
            if input == output || !seen.insert((input.clone(), output.clone())) {
                continue;
            }
            let ancestry = safe_git(&self.root)?
                .args(["merge-base", "--is-ancestor", &input, &output])
                .output()?;
            ensure!(
                ancestry.status.success(),
                "attempt output must descend from its input commit"
            );
            let patch = safe_git(&self.root)?
                .args([
                    "diff",
                    "--binary",
                    "--full-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    &input,
                    &output,
                    "--",
                ])
                .output()?;
            ensure!(patch.status.success(), "cannot generate attempt delta");
            if patch.stdout.is_empty() {
                continue;
            }
            let mut apply = safe_git(path)?
                .args(["apply", "--index", "--3way", "--whitespace=nowarn", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let mut stdin = apply.stdin.take().context("Git apply stdin unavailable")?;
            // Drain output concurrently with writing: a large diagnostic cannot
            // deadlock the writer against Git's stderr pipe.
            let patch_bytes = patch.stdout;
            let writer = std::thread::spawn(move || stdin.write_all(&patch_bytes));
            let receipt = apply.wait_with_output()?;
            writer
                .join()
                .map_err(|_| anyhow::anyhow!("Git patch writer panicked"))??;
            ensure!(
                receipt.status.success(),
                "integration conflict: {}",
                String::from_utf8_lossy(&receipt.stderr)
            );
            self.commit(path, "harness: integrate attempt delta")?;
        }
        self.git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
    }

    pub fn tree(&self, sha: &str) -> Result<String> {
        let sha = self.commit_id(sha)?;
        self.git(&self.root, &["rev-parse", &format!("{sha}^{{tree}}")])
    }

    pub fn diff(&self, base: &str, sha: &str) -> Result<String> {
        let base = self.commit_id(base)?;
        let sha = self.commit_id(sha)?;
        self.git(
            &self.root,
            &["diff", "--no-ext-diff", "--no-textconv", &base, &sha, "--"],
        )
    }

    /// Publish only to an unoccupied local branch, using compare-and-swap.
    pub fn publish(&self, candidate: &str, target_ref: &str, expected: &str) -> Result<()> {
        ensure!(
            target_ref.starts_with("refs/heads/"),
            "publication target must be refs/heads/..."
        );
        self.git(&self.root, &["check-ref-format", target_ref])?;
        let candidate = self.commit_id(candidate)?;
        let expected = self.commit_id(expected)?;
        let actual = self.git(&self.root, &["rev-parse", "--verify", target_ref])?;
        ensure!(
            actual == expected,
            "publication target changed since verification"
        );
        let worktrees = self.git(&self.root, &["worktree", "list", "--porcelain", "-z"])?;
        ensure!(
            !worktrees
                .split('\0')
                .any(|field| field == format!("branch {target_ref}")),
            "publication target is checked out in a worktree"
        );
        let status = safe_git(&self.root)?
            .args(["merge-base", "--is-ancestor", &expected, &candidate])
            .output()?;
        ensure!(
            status.status.success(),
            "publication must be a fast-forward"
        );
        self.git(
            &self.root,
            &[
                "update-ref",
                "-m",
                "harness: publish verified candidate",
                target_ref,
                &candidate,
                &expected,
            ],
        )?;
        Ok(())
    }
}

fn copy_runtime_environment_std(command: &mut Command) {
    for key in runtime_environment_keys() {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn runtime_environment_keys() -> &'static [&'static str] {
    &[
        "PATH",
        "SYSTEMROOT",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
    ]
}

fn glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        ensure!(!pattern.trim().is_empty(), "empty path grant");
        builder.add(Glob::new(pattern).context("invalid path grant glob")?);
    }
    Ok(builder.build()?)
}

fn protected(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".git"
                | ".harness"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa"
        ) || name == ".env"
            || name.starts_with(".env.")
            || name.ends_with(".pem")
            || name.ends_with(".key")
    })
}

fn relative_path(value: &str) -> Result<PathBuf> {
    ensure!(!value.contains('\0'), "path contains NUL");
    // Backslashes are separators on Windows, not escape syntax. Reject them on
    // Unix too so a grant cannot have different traversal meaning across OSes.
    ensure!(
        !value.contains('\\'),
        "use forward slashes in gateway paths"
    );
    ensure!(
        !value.contains(':'),
        "drive prefixes and alternate data streams are forbidden"
    );
    let path = Path::new(value);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let name = value.to_string_lossy();
                ensure!(
                    !name.ends_with([' ', '.']),
                    "trailing spaces/dots are not portable gateway paths"
                );
                let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
                ensure!(
                    !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                        && !(stem.len() == 4
                            && (stem.starts_with("com") || stem.starts_with("lpt"))
                            && stem.as_bytes()[3].is_ascii_digit()),
                    "Windows device names are forbidden"
                );
                normalized.push(value)
            }
            Component::CurDir => {}
            _ => bail!("gateway paths must be relative without parent traversal"),
        }
    }
    ensure!(!normalized.as_os_str().is_empty(), "file path is empty");
    ensure!(!protected(&normalized), "protected file or directory");
    Ok(normalized)
}

fn match_scope(path: &Path, patterns: &[String]) -> Result<bool> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    Ok(glob_set(patterns)?.is_match(normalized))
}

#[cfg(windows)]
fn is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

#[cfg(not(windows))]
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Refuse every intermediate link, including Windows junctions/reparse points.
fn scoped_path(root: &Path, relative: &Path, create_parents: bool) -> Result<PathBuf> {
    let root = root.canonicalize().context("gateway root does not exist")?;
    ensure!(root.is_dir(), "gateway root must be a directory");
    let components: Vec<_> = relative.components().collect();
    let mut current = root.clone();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure!(
                    !is_link(&metadata),
                    "gateway refuses symlinks and junctions"
                );
                if index + 1 < components.len() {
                    ensure!(metadata.is_dir(), "path parent is not a directory");
                }
                ensure!(
                    current.canonicalize()?.starts_with(&root),
                    "path escaped gateway root"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if index + 1 < components.len() && create_parents {
                    match fs::create_dir(&current) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                    let metadata = fs::symlink_metadata(&current)?;
                    ensure!(
                        !is_link(&metadata) && metadata.is_dir(),
                        "new parent is not a plain directory"
                    );
                    ensure!(
                        current.canonicalize()?.starts_with(&root),
                        "new parent escaped gateway root"
                    );
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
}

fn nofollow_open(path: &Path, read: bool, write: bool, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(read).write(write);
    if create_new {
        options.create_new(true);
    } else if write {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    ensure!(
        !is_link(&file.metadata()?),
        "opened file is a link/reparse point"
    );
    Ok(file)
}

pub fn command_allowed(program: &str, args: &[String], grants: &Grants) -> bool {
    !program.is_empty()
        && !program.contains('\0')
        && !args.iter().any(|arg| arg.contains('\0'))
        && grants
            .commands
            .iter()
            .any(|grant| grant.program == program && args.starts_with(&grant.args_prefix))
}

pub fn validate_action(
    action: &Action,
    grants: &Grants,
    owned_paths: &[String],
    read_only: bool,
) -> Result<()> {
    match action {
        Action::ReadFile { path } => {
            let path = relative_path(path)?;
            ensure!(
                match_scope(&path, &grants.read)?,
                "read is outside granted scope"
            );
        }
        Action::Search { path, .. } => {
            if let Some(path) = path {
                if path != "." {
                    relative_path(path)?;
                }
            }
            ensure!(!grants.read.is_empty(), "search has no read grants");
            glob_set(&grants.read)?;
        }
        Action::WriteFile { path, .. } | Action::EditFile { path, .. } => {
            ensure!(!read_only, "review context is read-only");
            let path = relative_path(path)?;
            ensure!(
                match_scope(&path, &grants.write)?,
                "write is outside granted scope"
            );
            ensure!(
                match_scope(&path, owned_paths)?,
                "write is outside node ownership"
            );
        }
        Action::RunCommand { program, args } => {
            ensure!(
                !read_only,
                "arbitrary commands are unavailable in read-only reviews"
            );
            ensure!(
                command_allowed(program, args, grants),
                "command is not granted"
            );
        }
    }
    Ok(())
}

pub fn read_file(root: &Path, path: &str, grants: &Grants, limit: usize) -> Result<String> {
    ensure!(limit <= 64 * 1024 * 1024, "read limit exceeds 64 MiB");
    let relative = relative_path(path)?;
    ensure!(
        match_scope(&relative, &grants.read)?,
        "read is outside granted scope"
    );
    let target = scoped_path(root, &relative, false)?;
    let file = nofollow_open(&target, true, false, false)?;
    ensure!(
        file.metadata()?.is_file(),
        "only ordinary files can be read"
    );
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    let mut result = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        result.push_str("\n[TRUNCATED: requested file exceeds read limit]");
    }
    Ok(result)
}

/// Hash the complete authorized file, never a truncated model-visible excerpt.
/// Both metadata and streaming byte counts enforce the 64 MiB source bound.
pub fn read_file_hash(root: &Path, path: &str, grants: &Grants) -> Result<String> {
    Ok(read_file_snapshot(root, path, grants, 0)?.1)
}

/// Read one FD once, pairing a bounded excerpt with its complete source hash.
/// Metadata changes during the pass invalidate the snapshot. This does not
/// promise confinement against deliberate hostile same-user mutations.
pub fn read_file_snapshot(
    root: &Path,
    path: &str,
    grants: &Grants,
    limit: usize,
) -> Result<(String, String)> {
    const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
    ensure!(
        limit as u64 <= MAX_SOURCE_BYTES,
        "read limit exceeds 64 MiB"
    );
    let relative = relative_path(path)?;
    ensure!(
        match_scope(&relative, &grants.read)?,
        "read is outside granted scope"
    );
    let target = scoped_path(root, &relative, false)?;
    let mut file = nofollow_open(&target, true, false, false)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "only ordinary files can be hashed");
    ensure!(metadata.len() <= MAX_SOURCE_BYTES, "source exceeds 64 MiB");
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 8192];
    let mut total = 0u64;
    let mut excerpt = Vec::with_capacity(limit.min(metadata.len() as usize));
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(
            total <= MAX_SOURCE_BYTES,
            "source grew beyond 64 MiB while hashing"
        );
        hasher.update(&buffer[..count]);
        let copy_count = count.min(limit.saturating_sub(excerpt.len()));
        excerpt.extend_from_slice(&buffer[..copy_count]);
    }
    let after = file.metadata()?;
    ensure!(
        metadata.len() == after.len()
            && total == after.len()
            && metadata.modified().ok() == after.modified().ok(),
        "source changed during snapshot read"
    );
    let mut text = bounded_utf8(&excerpt, limit);
    if total > limit as u64 {
        text.push_str(
            "\n[TRUNCATED: requested file exceeds read limit; hash covers the full source]",
        );
    }
    Ok((text, hasher.finalize().to_hex().to_string()))
}

/// Apply one exact replacement to the version identified by `expected_hash`.
/// The replacement uses the existing atomic writer's second CAS validation.
pub fn edit_file(
    root: &Path,
    path: &str,
    old: &str,
    new: &str,
    expected_hash: &str,
    grants: &Grants,
) -> Result<String> {
    const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
    ensure!(!old.is_empty(), "edit old text must be nonempty");
    ensure!(
        !expected_hash.is_empty(),
        "edit requires a full source hash"
    );
    let relative = relative_path(path)?;
    ensure!(
        match_scope(&relative, &grants.write)?,
        "write is outside granted scope"
    );
    let target = scoped_path(root, &relative, false)?;
    let file = nofollow_open(&target, true, false, false)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "edit source must be an ordinary file");
    ensure!(
        metadata.len() <= MAX_SOURCE_BYTES as u64,
        "source exceeds 64 MiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_SOURCE_BYTES,
        "source grew beyond 64 MiB while reading"
    );
    ensure!(
        blake3::hash(&bytes).to_hex().as_str() == expected_hash,
        "source hash changed before edit"
    );
    let original = String::from_utf8(bytes).context("edit source is not UTF-8")?;
    let offset = original
        .find(old)
        .context("old text was not found in source")?;
    // Start one character after the first match, rather than after the whole
    // old string, to detect overlapping occurrences such as aa inside aaa.
    let next_start = offset + old.chars().next().unwrap().len_utf8();
    ensure!(
        !original[next_start..].contains(old),
        "old text is ambiguous; expected exactly one match"
    );
    let new_len = original
        .len()
        .checked_sub(old.len())
        .and_then(|length| length.checked_add(new.len()))
        .context("edit size overflow")?;
    ensure!(new_len <= MAX_SOURCE_BYTES, "edited source exceeds 64 MiB");
    let mut edited = String::with_capacity(new_len);
    edited.push_str(&original[..offset]);
    edited.push_str(new);
    edited.push_str(&original[offset + old.len()..]);
    write_file(root, path, &edited, Some(expected_hash), grants)
}

static WRITE_GATE: Mutex<()> = Mutex::new(());

fn file_hash(mut file: File) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn write_file(
    root: &Path,
    path: &str,
    content: &str,
    expected_hash: Option<&str>,
    grants: &Grants,
) -> Result<String> {
    let _guard = WRITE_GATE
        .lock()
        .map_err(|_| anyhow::anyhow!("write gateway lock poisoned"))?;
    let relative = relative_path(path)?;
    ensure!(
        match_scope(&relative, &grants.write)?,
        "write is outside granted scope"
    );
    let target = scoped_path(root, &relative, true)?;
    let original_permissions = if target.exists() {
        let file = nofollow_open(&target, true, false, false)?;
        ensure!(
            file.metadata()?.is_file(),
            "write target is not an ordinary file"
        );
        if let Some(expected) = expected_hash {
            ensure!(
                file_hash(file.try_clone()?)? == expected,
                "source hash changed before write"
            );
        }
        Some(file.metadata()?.permissions())
    } else {
        ensure!(expected_hash.is_none(), "expected source file is missing");
        None
    };
    let parent = target.parent().context("write target has no parent")?;
    let temporary = TempFile(parent.join(format!(".harness-write-{}", uuid::Uuid::new_v4())));
    let mut output = nofollow_open(&temporary.0, false, true, true)?;
    output.write_all(content.as_bytes())?;
    if let Some(permissions) = original_permissions {
        output.set_permissions(permissions)?;
    }
    output.sync_all()?;
    drop(output);
    // Recheck path and CAS immediately before atomic replacement. Native-trusted
    // cannot promise race confinement against arbitrary same-user processes.
    scoped_path(root, &relative, false)?;
    if let Some(expected) = expected_hash {
        ensure!(
            file_hash(nofollow_open(&target, true, false, false)?)? == expected,
            "source hash changed before replacement"
        );
    }
    atomic_replace(&temporary.0, &target)?;
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(blake3::hash(content.as_bytes()).to_hex().to_string())
}

#[cfg(not(windows))]
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub fn search(
    root: &Path,
    query: &str,
    path: Option<&str>,
    grants: &Grants,
    limit: usize,
) -> Result<String> {
    ensure!(!grants.read.is_empty(), "search has no read grants");
    let regex = regex::RegexBuilder::new(query)
        .size_limit(4 * 1024 * 1024)
        .build()
        .context("invalid search regex")?;
    let root = root.canonicalize()?;
    let start = match path {
        Some(path) if path != "." => scoped_path(&root, &relative_path(path)?, false)?,
        _ => root.clone(),
    };
    ensure!(start.exists(), "search path does not exist");
    let read_scope = glob_set(&grants.read)?;
    let mut builder = ignore::WalkBuilder::new(&start);
    builder
        .hidden(false)
        .follow_links(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false);
    let walker = builder.build();
    let mut output = String::new();
    let mut visited = 0usize;
    let mut scanned_bytes = 0usize;
    let began = Instant::now();
    let mut truncated = false;
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(&root)?;
        if protected(relative)
            || !read_scope.is_match(relative.to_string_lossy().replace('\\', "/"))
        {
            continue;
        }
        visited += 1;
        if visited > 20_000
            || scanned_bytes > 64 * 1024 * 1024
            || began.elapsed() > Duration::from_secs(5)
        {
            truncated = true;
            break;
        }
        let target = scoped_path(&root, relative, false)?;
        let file = nofollow_open(&target, true, false, false)?;
        let size = file.metadata()?.len();
        if size > 1024 * 1024 {
            truncated = true;
            continue;
        }
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
        scanned_bytes += bytes.len();
        if bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                let matched = format!("{}:{}:{}\n", relative.display(), index + 1, line);
                if output.len().saturating_add(matched.len()) > limit {
                    truncated = true;
                    break;
                }
                output.push_str(&matched);
            }
        }
        if output.len() >= limit || (truncated && output.len().saturating_add(1) >= limit) {
            break;
        }
    }
    if truncated {
        output.push_str(
            "[SEARCH INCOMPLETE: scope, time, file, or output limit; absence is not proven]\n",
        );
    }
    if output.is_empty() {
        output.push_str("No matches in the searched readable scope.\n");
    }
    Ok(output)
}

#[cfg(unix)]
struct ProcessTree {
    pid: i32,
    armed: AtomicBool,
}

#[cfg(unix)]
impl ProcessTree {
    fn attach(pid: u32) -> Result<Self> {
        Ok(Self {
            pid: pid as i32,
            armed: AtomicBool::new(true),
        })
    }
    fn terminate(&self) {
        if self.armed.swap(false, Ordering::AcqRel) {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(windows)]
struct ProcessTree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    fn attach(pid: u32) -> Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        use windows_sys::Win32::System::Threading::*;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(
                !job.is_null(),
                "CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            );
            let result = Self { job };
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32
                ) != 0,
                "cannot configure process job: {}",
                std::io::Error::last_os_error()
            );
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            ensure!(
                !process.is_null(),
                "cannot open spawned process: {}",
                std::io::Error::last_os_error()
            );
            let assigned = AssignProcessToJobObject(job, process);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            ensure!(assigned != 0, "cannot assign process job: {error}");
            Ok(result)
        }
    }
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessTree;
#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn attach(_: u32) -> Result<Self> {
        bail!("native process tree management is unsupported on this OS")
    }
    fn terminate(&self) {}
}

async fn capture<R: AsyncRead + Unpin>(
    mut reader: R,
    budget: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        let mut previous = budget.load(Ordering::Acquire);
        loop {
            match budget.compare_exchange_weak(
                previous,
                previous.saturating_sub(count),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => previous = current,
            }
        }
        let accepted = count.min(previous);
        output.extend_from_slice(&buffer[..accepted]);
        if accepted < count {
            truncated.store(true, Ordering::Release);
        }
    }
    Ok(output)
}

struct CaptureTask(tokio::task::JoinHandle<std::io::Result<Vec<u8>>>);
impl Drop for CaptureTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn finish_capture(mut task: CaptureTask, truncated: &AtomicBool) -> Result<Vec<u8>> {
    match tokio::time::timeout(Duration::from_secs(1), &mut task.0).await {
        Ok(result) => Ok(result.context("output reader task failed")??),
        Err(_) => {
            task.0.abort();
            let _ = (&mut task.0).await;
            truncated.store(true, Ordering::Release);
            Ok(Vec::new())
        }
    }
}

fn bounded_utf8(bytes: &[u8], limit: usize) -> String {
    let mut value = String::from_utf8_lossy(bytes).into_owned();
    if value.len() > limit {
        let mut boundary = limit;
        while !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    value
}

pub async fn run_command(
    cwd: &Path,
    spec: &CommandSpec,
    cancel: CancellationToken,
    output_limit: usize,
) -> Result<CommandResult> {
    ensure!(
        !spec.program.is_empty()
            && !spec.program.contains('\0')
            && !spec.args.iter().any(|arg| arg.contains('\0')),
        "invalid command argv"
    );
    ensure!(spec.timeout_secs > 0, "command timeout must be positive");
    ensure!(
        output_limit <= 64 * 1024 * 1024,
        "command output limit exceeds 64 MiB"
    );
    let started = Instant::now();
    if cancel.is_cancelled() {
        return Ok(CommandResult {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            truncated: false,
            timed_out: false,
            cancelled: true,
            duration_ms: 0,
        });
    }
    let mut command = tokio::process::Command::new(&spec.program);
    command
        .current_dir(cwd)
        .args(&spec.args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in runtime_environment_keys() {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("HARNESS_RUNTIME_PROFILE", "native-trusted")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
    }
    let mut child = command.spawn().context("spawn native command")?;
    let pid = child.id().context("spawned process has no ID")?;
    let tree = match ProcessTree::attach(pid) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill().await;
            return Err(error);
        }
    };
    let budget = Arc::new(AtomicUsize::new(output_limit));
    let truncated = Arc::new(AtomicBool::new(false));
    let stdout = CaptureTask(tokio::spawn(capture(
        child.stdout.take().context("stdout unavailable")?,
        budget.clone(),
        truncated.clone(),
    )));
    let stderr = CaptureTask(tokio::spawn(capture(
        child.stderr.take().context("stderr unavailable")?,
        budget,
        truncated.clone(),
    )));
    let mut timed_out = false;
    let mut cancelled = false;
    let status = tokio::select! {
        biased;
        _ = cancel.cancelled() => { cancelled = true; None },
        result = child.wait() => Some(result.context("wait for command")?),
        _ = tokio::time::sleep(Duration::from_secs(spec.timeout_secs)) => { timed_out = true; None },
    };
    // Cleanup also after a successful parent exit: background children must not
    // survive and keep writing to a supposedly frozen candidate.
    tree.terminate();
    if status.is_none() {
        let _ = child.kill().await;
    }
    let stdout_bytes = finish_capture(stdout, &truncated).await?;
    let stderr_bytes = finish_capture(stderr, &truncated).await?;
    let stdout = bounded_utf8(&stdout_bytes, output_limit);
    let stderr = bounded_utf8(&stderr_bytes, output_limit.saturating_sub(stdout.len()));
    let lossy_truncated = stdout.len() < String::from_utf8_lossy(&stdout_bytes).len()
        || stderr.len() < String::from_utf8_lossy(&stderr_bytes).len();
    Ok(CommandResult {
        exit_code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        truncated: truncated.load(Ordering::Acquire) || lossy_truncated,
        timed_out,
        cancelled,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

pub fn doctor(repo: Option<&Repo>) -> Value {
    let git = basic_git(
        repo.map(|repo| repo.root.as_path())
            .unwrap_or_else(|| Path::new(".")),
    )
    .arg("--version")
    .output();
    json!({
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "git": { "available": git.as_ref().is_ok_and(|r| r.status.success()), "version": git.ok().map(|r| String::from_utf8_lossy(&r.stdout).trim().to_owned()) },
        "repository": repo.map(|repo| repo.root.to_string_lossy().into_owned()),
        "runtime_profiles": {
            "native-trusted": { "available": cfg!(any(unix, windows)), "filesystem_confinement": false, "network_confinement": false, "credential_file_confinement": false },
            "isolated": { "available": false, "reason": "No implemented, probed filesystem/network isolation backend; automatic downgrade is forbidden" }
        },
        "process_tree": if cfg!(windows) { "JobObject kill-on-close, post-spawn assignment; assignment race is not confinement" } else { "Unix process group; deliberately detached processes can escape native cleanup" },
        "filesystem_gateway": { "scope_globs": true, "refuses_symlinks_and_junctions": true, "hostile_same_user_race_confinement": false },
        "limitations": ["Native commands have user-level filesystem and network rights", "No native process mechanism is advertised as a sandbox", "OS capabilities require execution on the actual platform"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants() -> Grants {
        Grants {
            read: vec!["**".into()],
            write: vec!["src/**".into()],
            commands: vec![],
        }
    }

    #[test]
    fn gateway_denies_protected_paths_and_unowned_writes() {
        for path in [
            "../escape",
            ".git/config",
            ".env",
            "src/.env.production",
            "src/../../outside",
            "src\\..\\escape",
            ".git /config",
            ".env::$DATA",
            "src/NUL.txt",
        ] {
            assert!(
                validate_action(
                    &Action::ReadFile { path: path.into() },
                    &grants(),
                    &[],
                    false
                )
                .is_err(),
                "{path}"
            );
        }
        let action = Action::WriteFile {
            path: "src/other.rs".into(),
            content: "".into(),
            expected_hash: None,
        };
        assert!(validate_action(&action, &grants(), &["src/owned.rs".into()], false).is_err());
        assert!(validate_action(&action, &grants(), &["src/**".into()], true).is_err());
    }

    #[test]
    fn command_grants_compare_exact_argv_prefix() {
        let mut grants = grants();
        grants.commands.push(crate::types::CommandGrant {
            program: "cargo".into(),
            args_prefix: vec!["test".into(), "--locked".into()],
        });
        assert!(command_allowed(
            "cargo",
            &["test".into(), "--locked".into(), "--lib".into()],
            &grants
        ));
        assert!(!command_allowed("cargo", &["test".into()], &grants));
        assert!(!command_allowed(
            "/tmp/cargo",
            &["test".into(), "--locked".into()],
            &grants
        ));
        assert!(!command_allowed(
            "cargo",
            &["test; echo bad".into(), "--locked".into()],
            &grants
        ));
    }

    #[test]
    fn unicode_write_hash_and_compare_and_swap() {
        let root = tempfile::tempdir().unwrap();
        let path = "src/каталог с пробелами/файл.rs";
        let first = write_file(root.path(), path, "hello\r\nмир", None, &grants()).unwrap();
        assert_eq!(
            read_file(root.path(), path, &grants(), 100).unwrap(),
            "hello\r\nмир"
        );
        assert!(write_file(root.path(), path, "bad", Some("outdated"), &grants()).is_err());
        let second = write_file(root.path(), path, "new", Some(&first), &grants()).unwrap();
        assert_ne!(first, second);
        assert_eq!(read_file(root.path(), path, &grants(), 100).unwrap(), "new");
    }

    #[test]
    fn edit_requires_unique_exact_match_and_current_full_hash() {
        let root = tempfile::tempdir().unwrap();
        let path = "src/edited.rs";
        let initial = "// привет\r\nlet value = 1;\r\n";
        let hash = write_file(root.path(), path, initial, None, &grants()).unwrap();
        assert_eq!(read_file_hash(root.path(), path, &grants()).unwrap(), hash);
        let edited_hash = edit_file(
            root.path(),
            path,
            "value = 1",
            "value = 2",
            &hash,
            &grants(),
        )
        .unwrap();
        let expected = "// привет\r\nlet value = 2;\r\n";
        assert_eq!(
            read_file(root.path(), path, &grants(), 100).unwrap(),
            expected
        );
        assert_eq!(
            edited_hash,
            blake3::hash(expected.as_bytes()).to_hex().to_string()
        );
        assert!(edit_file(
            root.path(),
            path,
            "value = 2",
            "value = 3",
            &hash,
            &grants()
        )
        .is_err());
        assert!(edit_file(
            root.path(),
            path,
            "missing",
            "replacement",
            &edited_hash,
            &grants()
        )
        .is_err());
        assert!(edit_file(
            root.path(),
            path,
            "",
            "replacement",
            &edited_hash,
            &grants()
        )
        .is_err());
        assert_eq!(
            read_file(root.path(), path, &grants(), 100).unwrap(),
            expected
        );
    }

    #[test]
    fn edit_rejects_ambiguous_overlapping_text_and_readonly_actions() {
        let root = tempfile::tempdir().unwrap();
        let path = "src/ambiguous.txt";
        let hash = write_file(root.path(), path, "aaa", None, &grants()).unwrap();
        assert!(edit_file(root.path(), path, "aa", "b", &hash, &grants()).is_err());
        assert_eq!(read_file(root.path(), path, &grants(), 100).unwrap(), "aaa");
        let action = Action::EditFile {
            path: path.into(),
            old: "aaa".into(),
            new: "b".into(),
            expected_hash: hash,
        };
        assert!(validate_action(&action, &grants(), &["src/**".into()], true).is_err());
        assert!(validate_action(&action, &grants(), &["src/other.txt".into()], false).is_err());
        assert!(validate_action(&action, &grants(), &["src/**".into()], false).is_ok());
    }

    #[test]
    fn file_hash_covers_bytes_beyond_read_excerpt() {
        let root = tempfile::tempdir().unwrap();
        let path = "src/large.txt";
        let content = format!("{}tail", "x".repeat(20_000));
        let hash = write_file(root.path(), path, &content, None, &grants()).unwrap();
        assert!(read_file(root.path(), path, &grants(), 10)
            .unwrap()
            .contains("TRUNCATED"));
        assert_eq!(read_file_hash(root.path(), path, &grants()).unwrap(), hash);
        let (excerpt, snapshot_hash) =
            read_file_snapshot(root.path(), path, &grants(), 10).unwrap();
        assert_eq!(snapshot_hash, hash);
        assert!(excerpt.starts_with("xxxxxxxxxx"));
        assert!(excerpt.contains("hash covers the full source"));
        assert_ne!(
            hash,
            blake3::hash(&content.as_bytes()[..10]).to_hex().to_string()
        );
        assert!(read_file_hash(root.path(), ".git/config", &grants()).is_err());
    }

    #[test]
    fn source_snapshot_and_edit_enforce_size_and_utf8_bounds() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        let oversized = File::create(root.path().join("src/oversized.txt")).unwrap();
        oversized.set_len(64 * 1024 * 1024 + 1).unwrap();
        assert!(read_file_hash(root.path(), "src/oversized.txt", &grants()).is_err());
        assert!(read_file_snapshot(root.path(), "src/oversized.txt", &grants(), 10).is_err());
        assert!(edit_file(
            root.path(),
            "src/oversized.txt",
            "x",
            "y",
            "unused",
            &grants()
        )
        .is_err());
        fs::write(root.path().join("src/binary.txt"), [0xffu8, b'x']).unwrap();
        let binary_hash = read_file_hash(root.path(), "src/binary.txt", &grants()).unwrap();
        assert!(edit_file(
            root.path(),
            "src/binary.txt",
            "x",
            "y",
            &binary_hash,
            &grants()
        )
        .is_err());
        assert_eq!(
            fs::read(root.path().join("src/binary.txt")).unwrap(),
            [0xffu8, b'x']
        );
    }

    fn fixture_repo() -> (tempfile::TempDir, Repo, String) {
        let directory = tempfile::tempdir().unwrap();
        let receipt = basic_git(directory.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(receipt.status.success());
        fs::write(directory.path().join("base.txt"), "base\n").unwrap();
        let repo = Repo::discover(directory.path()).unwrap();
        let base = repo.commit(directory.path(), "initial").unwrap();
        (directory, repo, base)
    }

    #[test]
    fn integrates_attempt_deltas_without_duplicate_dependency_changes() {
        let (_directory, repo, base) = fixture_repo();
        let a = repo.state_dir.join("worktrees/first");
        repo.create_worktree(&a, &base).unwrap();
        fs::write(a.join("first.txt"), "first\n").unwrap();
        let first = repo.commit(&a, "first").unwrap();
        let b = repo.state_dir.join("worktrees/second");
        repo.create_worktree(&b, &first).unwrap();
        fs::write(b.join("second.txt"), "second\n").unwrap();
        let second = repo.commit(&b, "second").unwrap();
        let merged = repo.state_dir.join("worktrees/merged");
        repo.create_worktree(&merged, &base).unwrap();
        let candidate = repo
            .integrate(
                &merged,
                &base,
                &[
                    (base.clone(), first.clone()),
                    (first.clone(), second),
                    (base.clone(), first),
                ],
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(merged.join("first.txt")).unwrap(),
            "first\n"
        );
        assert_eq!(
            fs::read_to_string(merged.join("second.txt")).unwrap(),
            "second\n"
        );
        assert_eq!(repo.head().unwrap(), base);
        assert_eq!(repo.head_at(&merged).unwrap(), candidate);
    }

    #[test]
    fn integration_creates_new_managed_worktree_and_refuses_user_root() {
        let (_directory, repo, base) = fixture_repo();
        let worktree = repo.state_dir.join("worktrees/new-integration");
        assert!(!worktree.exists());
        assert_eq!(repo.integrate(&worktree, &base, &[]).unwrap(), base);
        fs::write(worktree.join("stale-untracked.txt"), "old candidate").unwrap();
        assert_eq!(repo.integrate(&worktree, &base, &[]).unwrap(), base);
        assert!(!worktree.join("stale-untracked.txt").exists());
        fs::write(repo.root.join("user-uncommitted.txt"), "preserve me").unwrap();
        assert!(repo.integrate(&repo.root, &base, &[]).is_err());
        assert_eq!(
            fs::read_to_string(repo.root.join("user-uncommitted.txt")).unwrap(),
            "preserve me"
        );
    }

    #[test]
    fn publish_requires_unoccupied_ref_and_compare_and_swap() {
        let (_directory, repo, base) = fixture_repo();
        let target = repo.git(&repo.root, &["symbolic-ref", "HEAD"]).unwrap();
        let worktree = repo.state_dir.join("worktrees/candidate");
        repo.create_worktree(&worktree, &base).unwrap();
        fs::write(worktree.join("new.txt"), "new\n").unwrap();
        let candidate = repo.commit(&worktree, "new").unwrap();
        assert!(repo.publish(&candidate, &target, &base).is_err());
        repo.git(&repo.root, &["branch", "release", &base]).unwrap();
        repo.publish(&candidate, "refs/heads/release", &base)
            .unwrap();
        assert!(repo
            .publish(&candidate, "refs/heads/release", &base)
            .is_err());
        assert_eq!(
            repo.git(&repo.root, &["rev-parse", "release"]).unwrap(),
            candidate
        );
    }

    #[test]
    fn git_does_not_run_required_clean_filters() {
        let (_directory, repo, _base) = fixture_repo();
        repo.git(&repo.root, &["config", "filter.hostile.clean", "false"])
            .unwrap();
        repo.git(&repo.root, &["config", "filter.hostile.required", "true"])
            .unwrap();
        fs::write(repo.root.join(".gitattributes"), "*.txt filter=hostile\n").unwrap();
        fs::write(repo.root.join("filtered.txt"), "safe\n").unwrap();
        assert!(repo.commit(&repo.root, "filter is disabled").is_ok());
    }

    #[test]
    fn repository_lock_is_exclusive() {
        let (_directory, repo, _base) = fixture_repo();
        let lock = repo.lock().unwrap();
        assert!(repo.lock().is_err());
        drop(lock);
        assert!(repo.lock().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn git_does_not_run_commit_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, repo, _base) = fixture_repo();
        let hook = repo.common_dir.join("hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 73\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(repo.root.join("hook-test.txt"), "safe\n").unwrap();
        assert!(repo.commit(&repo.root, "hook is disabled").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn gateway_refuses_link_parents() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("src/linked")).unwrap();
        assert!(write_file(root.path(), "src/linked/secret", "x", None, &grants()).is_err());
        assert!(!outside.path().join("secret").exists());
    }

    #[tokio::test]
    async fn already_cancelled_command_does_not_spawn() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = run_command(
            Path::new("."),
            &CommandSpec {
                program: "nonexistent-command-for-test".into(),
                args: vec![],
                timeout_secs: 1,
            },
            cancel,
            1024,
        )
        .await
        .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.exit_code, None);
    }
}
