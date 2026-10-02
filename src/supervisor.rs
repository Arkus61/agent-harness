//! A separate, trusted process keeps native command cleanup alive after owner death.
//!
//! The owner's pipe is a lifetime lease. This is process lifecycle supervision,
//! not filesystem, network, or hostile-process confinement.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

const ENTRY: &str = "--__harness-native-supervisor";
const HEADER_LIMIT: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u8,
    program: String,
    args: Vec<String>,
    forward_stdin: bool,
}

fn executable() -> Result<PathBuf> {
    let current = std::env::current_exe().context("resolve supervisor executable")?;
    let name = current.file_name().and_then(|v| v.to_str()).unwrap_or("");
    if name == "harness" || name == "harness.exe" {
        return Ok(current);
    }
    // Cargo integration and unit tests run under target/<profile>/deps. The
    // package's CLI is built alongside that directory. Never search cwd/PATH
    // for a supervisor: that could select repository-controlled code.
    if current
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|v| v == "deps")
    {
        let sibling = current
            .parent()
            .and_then(|p| p.parent())
            .context("resolve Cargo supervisor directory")?
            .join(if cfg!(windows) {
                "harness.exe"
            } else {
                "harness"
            });
        ensure!(
            sibling.is_file(),
            "trusted supervisor executable is unavailable"
        );
        return Ok(sibling);
    }
    bail!("native execution requires the harness supervisor executable")
}

/// Prepare the same packaged binary as a supervisor. Environment and cwd are
/// deliberately configured by the caller and inherited by the actual target.
pub fn prepare(
    program: &str,
    args: &[String],
    forward_stdin: bool,
) -> Result<(tokio::process::Command, Vec<u8>)> {
    ensure!(
        !program.is_empty() && !program.contains('\0') && !args.iter().any(|v| v.contains('\0')),
        "invalid supervised command argv"
    );
    let request = Request {
        protocol: 1,
        program: program.into(),
        args: args.into(),
        forward_stdin,
    };
    let mut payload = serde_json::to_vec(&request)?;
    ensure!(
        payload.len() < HEADER_LIMIT,
        "supervisor request exceeds 1 MiB"
    );
    payload.push(b'\n');
    let mut command = tokio::process::Command::new(executable()?);
    command
        .arg(ENTRY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Closing stdin permits the supervisor to clean the real process group.
    // Killing the supervisor first would strand its child on Unix.
    command.kill_on_drop(false);
    #[cfg(unix)]
    command.process_group(0);
    Ok((command, payload))
}

/// Windows holds the job in the coordinator. Assignment precedes the command
/// header, so actual target execution starts only after job membership exists.
pub struct LifetimeGuard {
    #[cfg(windows)]
    _job: ProcessTree,
}

pub async fn dispatch(
    mut command: tokio::process::Command,
    payload: &[u8],
) -> Result<(Child, ChildStdin, LifetimeGuard)> {
    command.kill_on_drop(false);
    let mut child = command
        .spawn()
        .context("spawn trusted command supervisor")?;
    let mut input = child
        .stdin
        .take()
        .context("supervisor lifetime pipe unavailable")?;
    #[cfg(windows)]
    let job = match ProcessTree::attach(child.id().context("supervisor has no process ID")?) {
        Ok(job) => job,
        Err(error) => {
            drop(input);
            let _ = child.kill().await;
            return Err(error);
        }
    };
    let guard = LifetimeGuard {
        #[cfg(windows)]
        _job: job,
    };
    input
        .write_all(payload)
        .await
        .context("send supervisor command header")?;
    input.flush().await?;
    Ok((child, input, guard))
}

/// Called before CLI parsing/runtime creation. The internal entry consumes
/// only its private stdin header and has the same user rights as the owner.
pub fn entry() -> Option<Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(ENTRY)) {
        return None;
    }
    Some((|| {
        ensure!(args.next().is_none(), "unexpected supervisor arguments");
        run()
    })())
}

fn run() -> Result<()> {
    ensure!(
        cfg!(any(unix, windows)),
        "native supervision is unsupported on this platform"
    );
    let mut input = BufReader::new(std::io::stdin());
    let mut header = Vec::new();
    let count = input
        .by_ref()
        .take(HEADER_LIMIT as u64 + 1)
        .read_until(b'\n', &mut header)?;
    ensure!(
        count > 0 && count <= HEADER_LIMIT && header.last() == Some(&b'\n'),
        "incomplete or oversized supervisor header"
    );
    let request: Request = serde_json::from_slice(&header)?;
    ensure!(request.protocol == 1, "unsupported supervisor protocol");
    ensure!(
        !request.program.is_empty()
            && !request.program.contains('\0')
            && !request.args.iter().any(|v| v.contains('\0')),
        "invalid supervisor argv"
    );
    // The pipe can already be closed if the owner died during startup. Refuse
    // dispatch in that case instead of executing an abandoned request.
    ensure!(!lease_closed(), "owner disappeared before command dispatch");
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .stdin(if request.forward_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
    }
    let mut child = command.spawn().context("spawn supervised target")?;
    let tree = match ProcessTree::attach(child.id()) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let owner_lost = Arc::new(AtomicBool::new(false));
    let reader_lost = owner_lost.clone();
    let target_input = child.stdin.take();
    std::thread::spawn(move || {
        let mut target_input = target_input;
        let mut buffer = [0u8; 8192];
        loop {
            match input.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if let Some(writer) = &mut target_input {
                        if writer.write_all(&buffer[..count]).is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        reader_lost.store(true, Ordering::Release);
    });
    let status = loop {
        if owner_lost.load(Ordering::Acquire) || lease_closed() {
            tree.terminate();
            break child.wait().context("reap target after owner loss")?;
        }
        if let Some(status) = child.try_wait().context("poll supervised target")? {
            // Cleanup descendants even after successful target exit. They must
            // not continue mutating an accepted candidate or holding outputs.
            tree.terminate();
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(tree);
    exit_with(status)
}

#[cfg(unix)]
fn lease_closed() -> bool {
    let mut poll = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // POLLHUP is observed even when unread data remains in a blocked forwarding
    // pipe, so a child that stops consuming stdin cannot disable owner cleanup.
    let result = unsafe { libc::poll(&mut poll, 1, 0) };
    result < 0
        || (result > 0 && poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0)
}
#[cfg(not(unix))]
fn lease_closed() -> bool {
    false
}

fn exit_with(status: ExitStatus) -> Result<()> {
    if let Some(code) = status.code() {
        std::process::exit(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            std::process::exit(128 + signal);
        }
    }
    std::process::exit(125)
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
                "cannot open supervised process: {}",
                std::io::Error::last_os_error()
            );
            let assigned = AssignProcessToJobObject(job, process);
            let error = std::io::Error::last_os_error();
            CloseHandle(process);
            ensure!(assigned != 0, "cannot assign supervised job: {error}");
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
        bail!("native process management is unsupported")
    }
    fn terminate(&self) {}
}
