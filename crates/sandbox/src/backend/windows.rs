//! Windows backend: Job Objects (via the safe `win32job` API), a cleared environment,
//! and a sampling watchdog for resource limits the safe API cannot set.
//!
//! Filesystem confinement and network denial need restricted tokens, AppContainer, or
//! WFP filters, none of which are reachable without unsafe Win32 calls, so they are
//! reported as not enforced and the maximum level is Brokered.

use std::{
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::Path,
    process::{Child, Command},
    sync::{Mutex, PoisonError},
};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use win32job::{ExtendedLimitInfo, Job};

use super::WATCHDOG_INTERVAL;
use crate::{
    BackendCapabilities, BackendKind, Capability, Enforcement, EnforcementPlan, HelperStatus,
    Mount, Requirement, SandboxError, helper::protocol::HelperPlan,
};

/// `CREATE_NO_WINDOW`: console children get a hidden console instead of a window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const NO_CONFINEMENT: &str = "requires a restricted token or AppContainer; the safe Win32 bindings available to this crate cannot create them";
const NO_NETWORK: &str = "requires AppContainer network capabilities or WFP filters; not implemented without unsafe Win32 calls";

pub(crate) fn probe(helper: HelperStatus) -> BackendCapabilities {
    let interval = u64::try_from(WATCHDOG_INTERVAL.as_millis()).unwrap_or(u64::MAX);
    let watchdog = |mechanism: &str| Enforcement::Watchdog {
        mechanism: mechanism.into(),
        interval_ms: interval,
    };
    let capabilities = vec![
        Capability::new(
            Requirement::ProcessTreeTermination,
            Enforcement::enforced(
                "Job Object with KILL_ON_JOB_CLOSE; the helper joins the job before it starts the target, so every descendant is a member",
            ),
        ),
        Capability::new(
            Requirement::ProcessCountLimit,
            watchdog("job membership sampled; the whole job is terminated when the count exceeds the limit"),
        )
        .limited("process creation is not refused: win32job does not expose JOB_OBJECT_LIMIT_ACTIVE_PROCESS")
        .limited("the helper and its console host (conhost.exe) are not counted"),
        Capability::new(
            Requirement::MemoryLimit,
            watchdog("per-process private commit sampled; the whole job is terminated on violation"),
        )
        .limited("allocations are not refused: win32job does not expose JOB_OBJECT_LIMIT_PROCESS_MEMORY"),
        Capability::new(
            Requirement::CpuTimeLimit,
            watchdog("per-process user+kernel CPU time sampled; the whole job is terminated on violation"),
        )
        .limited("win32job does not expose JOB_OBJECT_LIMIT_PROCESS_TIME"),
        Capability::new(
            Requirement::WallTimeout,
            Enforcement::enforced("parent deadline; the whole job is terminated"),
        ),
        Capability::new(
            Requirement::EnvIsolation,
            Enforcement::enforced("cleared environment block plus allowlist"),
        )
        .limited("level 1 does not contain hostile code: a same-user process can open other processes of that user, including the daemon"),
        Capability::new(
            Requirement::FilesystemReadConfinement,
            Enforcement::not_enforced(NO_CONFINEMENT),
        ),
        Capability::new(
            Requirement::FilesystemWriteConfinement,
            Enforcement::not_enforced(NO_CONFINEMENT),
        ),
        Capability::new(Requirement::NetworkDeny, Enforcement::not_enforced(NO_NETWORK)),
        Capability::new(
            Requirement::NetworkAllowlist,
            Enforcement::not_enforced("no enforcing egress proxy is implemented"),
        ),
        Capability::new(
            Requirement::EphemeralEnvironment,
            Enforcement::not_enforced("no container, VM, or overlay backend is implemented"),
        ),
    ];
    BackendCapabilities::build(
        BackendKind::WindowsJobObject,
        helper,
        capabilities,
        vec!["console children run with CREATE_NO_WINDOW".into()],
    )
}

pub(crate) fn request_enforcement(
    capabilities: &BackendCapabilities,
    requirement: Requirement,
    _mounts: &[Mount],
) -> Enforcement {
    capabilities.enforcement(requirement)
}

pub(crate) fn helper_plan(
    _plan: &EnforcementPlan,
    base: HelperPlan,
) -> Result<HelperPlan, SandboxError> {
    // Every Windows control is applied by the parent (job, watchdog, deadline).
    Ok(base)
}

pub(crate) fn check_program(_plan: &EnforcementPlan, _program: &Path) -> Result<(), SandboxError> {
    Ok(())
}

pub(crate) fn configure(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Create the job and assign the helper. The helper is still blocked reading its plan,
/// so nothing it starts can predate the assignment.
///
/// Breakaway is never allowed: every process a member starts joins the job and ends
/// with the tree. A program the user goes on using (a browser a CLI opens for a sign-in)
/// must therefore not be started inside a tree; the official runtime login asks the CLI
/// for its authorization page and the client opens it outside.
pub(crate) fn contain(child: &Child) -> Result<TreeHandle, SandboxError> {
    let mut info = ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = Job::create_with_limit_info(&info)
        .map_err(|error| SandboxError::Spawn(format!("creating job object: {error}")))?;
    job.assign_process(child.as_raw_handle() as isize)
        .map_err(|error| SandboxError::Spawn(format!("assigning helper to job object: {error}")))?;
    Ok(TreeHandle {
        job: Mutex::new(Some(job)),
        root_pid: child.id(),
    })
}

/// The Job Object holding the tree. Closing the only handle kills every member.
pub(crate) struct TreeHandle {
    job: Mutex<Option<Job>>,
    root_pid: u32,
}

impl TreeHandle {
    /// Terminate every job member by closing the job (KILL_ON_JOB_CLOSE).
    pub(crate) fn kill(&self) -> Result<(), SandboxError> {
        let job = self
            .job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(job);
        Ok(())
    }

    pub(crate) fn members(&self) -> Result<Vec<u32>, SandboxError> {
        let guard = self.job.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(job) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let pids = job
            .query_process_id_list()
            .map_err(|error| SandboxError::Io(format!("querying job members: {error}")))?;
        Ok(pids
            .into_iter()
            .filter_map(|pid| u32::try_from(pid).ok())
            .collect())
    }

    fn root_pid(&self) -> u32 {
        self.root_pid
    }
}

/// Samples job members and reports the first violated limit.
pub(crate) struct Watchdog {
    max_processes: Option<usize>,
    max_memory_bytes: Option<u64>,
    max_cpu_ms: Option<u64>,
    system: System,
    /// The helper and its console host, identified on the first sample.
    infrastructure: Option<Vec<u32>>,
}

impl Watchdog {
    /// A watchdog for the plan's watchdog-enforced limits, if any.
    pub(crate) fn for_plan(plan: &EnforcementPlan) -> Option<Self> {
        let active = |requirement| {
            plan.control(requirement)
                .is_some_and(|control| matches!(control.enforcement, Enforcement::Watchdog { .. }))
        };
        let watchdog = Self {
            max_processes: plan
                .limits
                .max_processes
                .filter(|_| active(Requirement::ProcessCountLimit))
                .and_then(|max| usize::try_from(max).ok()),
            max_memory_bytes: plan
                .limits
                .max_memory_bytes
                .filter(|_| active(Requirement::MemoryLimit)),
            max_cpu_ms: plan
                .limits
                .max_cpu_time
                .filter(|_| active(Requirement::CpuTimeLimit))
                .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
            system: System::new(),
            infrastructure: None,
        };
        (watchdog.max_processes.is_some()
            || watchdog.max_memory_bytes.is_some()
            || watchdog.max_cpu_ms.is_some())
        .then_some(watchdog)
    }

    /// The helper plus the `conhost.exe` it got from `CREATE_NO_WINDOW`. Both exist
    /// before the helper confirms its handshake, so one lookup suffices.
    fn infrastructure(&mut self, tree: &TreeHandle, members: &[u32]) -> Vec<u32> {
        if let Some(known) = &self.infrastructure {
            return known.clone();
        }
        let root = tree.root_pid();
        let pids: Vec<Pid> = members.iter().map(|pid| Pid::from_u32(*pid)).collect();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );
        let console_host = pids.iter().find(|pid| {
            self.system.process(**pid).is_some_and(|process| {
                process.parent() == Some(Pid::from_u32(root))
                    && process.name().eq_ignore_ascii_case("conhost.exe")
            })
        });
        let mut known = vec![root];
        known.extend(console_host.map(|pid| pid.as_u32()));
        self.infrastructure = Some(known.clone());
        known
    }

    pub(crate) fn check(&mut self, tree: &TreeHandle) -> Option<Requirement> {
        let all = tree.members().ok()?;
        let infrastructure = self.infrastructure(tree, &all);
        let members: Vec<u32> = all
            .into_iter()
            .filter(|pid| !infrastructure.contains(pid))
            .collect();
        if self.max_processes.is_some_and(|max| members.len() > max) {
            return Some(Requirement::ProcessCountLimit);
        }
        if self.max_memory_bytes.is_none() && self.max_cpu_ms.is_none() {
            return None;
        }
        let pids: Vec<Pid> = members.iter().map(|pid| Pid::from_u32(*pid)).collect();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing().with_memory().with_cpu(),
        );
        for pid in &pids {
            let Some(process) = self.system.process(*pid) else {
                continue;
            };
            if self
                .max_memory_bytes
                .is_some_and(|max| process.virtual_memory() > max)
            {
                return Some(Requirement::MemoryLimit);
            }
            if self
                .max_cpu_ms
                .is_some_and(|max| process.accumulated_cpu_time() > max)
            {
                return Some(Requirement::CpuTimeLimit);
            }
        }
        None
    }
}
