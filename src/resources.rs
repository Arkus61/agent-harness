//! Explicit inherited process limits. These are not aggregate task quotas.
//!
//! Unix hard/soft rlimits apply to each process independently (NPROC instead
//! counts the real UID's processes). They do not bound a tree's total CPU,
//! memory, or storage. Unsupported requested guarantees always fail closed.
use crate::types::CommandSpec;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_space_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_cpu_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_disk_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProcessLimitCapability {
    pub cpu_seconds: bool,
    pub address_space_bytes: bool,
    pub file_size_bytes: bool,
    pub processes: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResourceCapability {
    pub backend: Option<String>,
    pub per_process: ProcessLimitCapability,
    pub aggregate: bool,
    pub limitations: Vec<String>,
}

/// This reports implemented guarantees, not merely installed container tools.
/// The coordinator never modifies its containing host cgroup or silently
/// chooses a daemon; no aggregate backend has been configured in this runtime.
pub fn availability() -> ResourceCapability {
    let unprivileged = hard_limit_caller_available();
    let probed = if unprivileged {
        probe_hard_limits()
    } else {
        [false; 4]
    };
    ResourceCapability {
        backend: cfg!(unix).then(|| "unix-hard-rlimit".into()),
        per_process: ProcessLimitCapability {
            cpu_seconds: cfg!(unix) && unprivileged && probed[0],
            address_space_bytes: cfg!(target_os = "linux") && unprivileged && probed[1],
            file_size_bytes: cfg!(unix) && unprivileged && probed[2],
            processes: process_limit_available() && probed[3],
        },
        aggregate: false,
        limitations: vec![
            "CPU seconds and virtual address space are per process, inherited by descendants; they are not aggregate CPU or RAM quotas".into(),
            "File size bounds each regular file, not total files, disk use, or private sandbox tmpfs capacity".into(),
            "Linux NPROC counts processes/threads for the real UID, including unrelated commands; privileged callers that can bypass it are rejected".into(),
            "The bubblewrap backend has no configured trusted delegated cgroup or aggregate storage backend; aggregate requests fail closed".into(),
            "Windows process resource limits are unsupported in this implementation; lifecycle JobObjects alone do not provide these guarantees".into(),
            "Root/CAP_SYS_RESOURCE callers that could raise hard limits are rejected; requested Linux limits set no_new_privs before exec; these remain process limits, not host confinement".into(),
        ],
    }
}

/// A cached bounded probe in a disposable child, never in the coordinator.
/// The child uses only async-signal-safe syscalls and never allocates or execs.
/// This proves that lowering limits/no_new_privs is permitted in this process
/// environment; actual dispatch still checks every requested setter.
fn probe_hard_limits() -> [bool; 4] {
    #[cfg(unix)]
    {
        static PROBE: std::sync::OnceLock<[bool; 4]> = std::sync::OnceLock::new();
        *PROBE.get_or_init(|| unsafe {
            let pid = libc::fork();
            if pid < 0 {
                return [false; 4];
            }
            if pid == 0 {
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    libc::_exit(0);
                }
                let mut mask = 0;
                for (index, resource) in [
                    libc::RLIMIT_CPU,
                    libc::RLIMIT_AS,
                    libc::RLIMIT_FSIZE,
                    libc::RLIMIT_NPROC,
                ]
                .into_iter()
                .enumerate()
                {
                    let mut current: libc::rlimit = std::mem::zeroed();
                    if libc::getrlimit(resource, &mut current) == 0 {
                        let value = current.rlim_max.min(1);
                        let lowered = libc::rlimit {
                            rlim_cur: value,
                            rlim_max: value,
                        };
                        if libc::setrlimit(resource, &lowered) == 0 {
                            mask |= 1 << index;
                        }
                    }
                }
                libc::_exit(mask);
            }
            let started = std::time::Instant::now();
            let mut status = 0;
            loop {
                let waited = libc::waitpid(pid, &mut status, libc::WNOHANG);
                if waited == pid {
                    let mask = if libc::WIFEXITED(status) {
                        libc::WEXITSTATUS(status)
                    } else {
                        0
                    };
                    return [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0];
                }
                if waited < 0 {
                    return [false; 4];
                }
                if started.elapsed() >= std::time::Duration::from_millis(100) {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                    return [false; 4];
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    }
    #[cfg(not(unix))]
    {
        [false; 4]
    }
}

#[cfg(target_os = "linux")]
fn linux_capabilities_allow(mask: u64) -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    ["CapEff:", "CapPrm:"].iter().all(|key| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|bits| u64::from_str_radix(bits.trim(), 16).ok())
            })
            .is_some_and(|bits| bits & mask == 0)
    })
}

fn hard_limit_caller_available() -> bool {
    #[cfg(unix)]
    {
        if unsafe { libc::geteuid() } == 0 || unsafe { libc::getuid() } == 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            linux_capabilities_allow(1 << 24)
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn process_limit_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Linux explicitly exempts root and CAP_SYS_ADMIN/CAP_SYS_RESOURCE
        // holders. Unknown capability state is not treated as enforcement.
        if unsafe { libc::geteuid() } == 0 || unsafe { libc::getuid() } == 0 {
            return false;
        }
        linux_capabilities_allow((1 << 21) | (1 << 24))
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

impl ResourceLimits {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("cpu_seconds", self.cpu_seconds),
            ("address_space_bytes", self.address_space_bytes),
            ("file_size_bytes", self.file_size_bytes),
            ("processes", self.processes),
            ("aggregate_cpu_seconds", self.aggregate_cpu_seconds),
            ("aggregate_memory_bytes", self.aggregate_memory_bytes),
            ("aggregate_disk_bytes", self.aggregate_disk_bytes),
        ] {
            if let Some(value) = value {
                ensure!(
                    (1..=i64::MAX as u64).contains(&value),
                    "resource limit {name} must be positive and finite (at most i64::MAX)"
                );
            }
        }
        Ok(())
    }

    pub fn validate_available(&self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.aggregate_cpu_seconds.is_none()
                && self.aggregate_memory_bytes.is_none()
                && self.aggregate_disk_bytes.is_none(),
            "aggregate resource limits unavailable: no trusted aggregate backend is configured; per-process rlimits cannot satisfy this request"
        );
        let capability = availability().per_process;
        for (name, requested, available) in [
            ("cpu_seconds", self.cpu_seconds, capability.cpu_seconds),
            (
                "address_space_bytes",
                self.address_space_bytes,
                capability.address_space_bytes,
            ),
            (
                "file_size_bytes",
                self.file_size_bytes,
                capability.file_size_bytes,
            ),
            ("processes", self.processes, capability.processes),
        ] {
            ensure!(requested.is_none() || available, "resource limit {name} unavailable on this platform or caller; no unbounded fallback");
        }
        Ok(())
    }
}

/// A command can tighten a trusted task default, never replace it with a
/// larger cap or omit it. Absence preserves the original serialized command.
pub fn apply_default_limits(
    spec: &CommandSpec,
    defaults: Option<&ResourceLimits>,
) -> Result<CommandSpec> {
    if let Some(limits) = &spec.resource_limits {
        limits.validate()?;
    }
    if let Some(limits) = defaults {
        limits.validate()?;
    }
    fn lower(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }
    let mut effective = spec.clone();
    effective.resource_limits = match (&spec.resource_limits, defaults) {
        (None, None) => None,
        (Some(explicit), None) => Some(explicit.clone()),
        (None, Some(defaults)) => Some(defaults.clone()),
        (Some(explicit), Some(defaults)) => Some(ResourceLimits {
            cpu_seconds: lower(explicit.cpu_seconds, defaults.cpu_seconds),
            address_space_bytes: lower(explicit.address_space_bytes, defaults.address_space_bytes),
            file_size_bytes: lower(explicit.file_size_bytes, defaults.file_size_bytes),
            processes: lower(explicit.processes, defaults.processes),
            aggregate_cpu_seconds: lower(
                explicit.aggregate_cpu_seconds,
                defaults.aggregate_cpu_seconds,
            ),
            aggregate_memory_bytes: lower(
                explicit.aggregate_memory_bytes,
                defaults.aggregate_memory_bytes,
            ),
            aggregate_disk_bytes: lower(
                explicit.aggregate_disk_bytes,
                defaults.aggregate_disk_bytes,
            ),
        }),
    };
    if let Some(limits) = &effective.resource_limits {
        limits.validate()?;
    }
    Ok(effective)
}

pub(crate) fn configure_target(
    command: &mut std::process::Command,
    limits: Option<&ResourceLimits>,
) -> Result<()> {
    let Some(limits) = limits else {
        return Ok(());
    };
    limits.validate_available()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let requested = [
            (libc::RLIMIT_CPU, limits.cpu_seconds),
            (libc::RLIMIT_AS, limits.address_space_bytes),
            (libc::RLIMIT_FSIZE, limits.file_size_bytes),
            (libc::RLIMIT_NPROC, limits.processes),
        ];
        // Only async-signal-safe syscalls occur in the post-fork closure. The
        // supervisor keeps its own unconstrained cleanup and lease threads.
        unsafe {
            command.pre_exec(move || {
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for (resource, value) in requested {
                    if let Some(value) = value {
                        let mut inherited: libc::rlimit = std::mem::zeroed();
                        if libc::getrlimit(resource, &mut inherited) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        let value = libc::rlim_t::try_from(value)
                            .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
                        let hard = value.min(inherited.rlim_max);
                        let limit = libc::rlimit {
                            rlim_cur: hard,
                            rlim_max: hard,
                        };
                        if libc::setrlimit(resource, &limit) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = command;
    Ok(())
}
