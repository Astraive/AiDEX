//! `ContextService`: the daemon-owned index manager (one background indexing job
//! per workspace), search, project rules, memory, and pack assembly.
//!
//! Index jobs are reported as `task.*` events (`task.created` with role
//! [`INDEX_TASK_ROLE`], then `task.started` and `task.completed`,
//! `task.cancelled`, or `task.failed`) scoped to the workspace; live progress is
//! read with [`ContextService::status`]. A job stays `running` until its terminal
//! event has been recorded, so whoever sees it finished also finds that event
//! (or the job's `event_error`).

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread::{self, JoinHandle},
    time::Duration,
};

use aidex_core::{CancelToken, EntityId, Timestamp};
use aidex_drive::DrivePaths;
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_workspace::ConfinedRoot;
use serde::{Deserialize, Serialize};

use crate::{
    ContextError,
    index::{
        BuildOutcome, Index, IndexOptions, IndexProgress, IndexReport, IndexSummary, IndexTarget,
        Query, SearchHit,
    },
    memory::{FileMemoryStore, MemoryStore},
    pack::{ContextPack, PackRequest, PackSources, assemble},
    rules::ProjectRules,
    tokens::{CharEstimator, TokenEstimator},
};

/// Prefix of index job ids (`idx_<ULID>`), used as the `task_id` of its events.
pub const INDEX_JOB_PREFIX: &str = "idx";
/// `role` of the `task.created` event of an index job.
pub const INDEX_TASK_ROLE: &str = "context.index";

/// Lifecycle of an index job.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// The latest index job of a workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexJobStatus {
    pub job_id: EntityId,
    pub workspace_id: EntityId,
    pub state: JobState,
    pub progress: IndexProgress,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub report: Option<IndexReport>,
    pub error: Option<String>,
    /// Set when a `task.*` event for this job could not be recorded.
    pub event_error: Option<String>,
}

/// An index and its latest job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexStatus {
    pub workspace_id: EntityId,
    pub index: IndexSummary,
    pub job: Option<IndexJobStatus>,
}

/// Result of [`ContextService::start_index`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexJobStarted {
    pub job_id: EntityId,
    pub workspace_id: EntityId,
    /// A job was already running; its id is returned and nothing new started.
    pub already_running: bool,
}

struct Job {
    status: Mutex<IndexJobStatus>,
    /// Notified when `status` leaves [`JobState::Running`].
    finished: Condvar,
    cancel: CancelToken,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Job {
    fn status(&self) -> MutexGuard<'_, IndexJobStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Context services for every workspace on one drive. `Send + Sync`; the daemon
/// holds it in an `Arc`. Dropping it cancels and joins running index jobs.
pub struct ContextService {
    paths: DrivePaths,
    options: IndexOptions,
    events: Arc<dyn EventSink>,
    memory: Arc<dyn MemoryStore>,
    estimator: Arc<dyn TokenEstimator>,
    indexes: Mutex<HashMap<EntityId, Arc<Index>>>,
    jobs: Mutex<HashMap<EntityId, Arc<Job>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ContextService {
    /// Context services rooted at the drive's `private_index/` and `private_memory/`.
    pub fn open(paths: &DrivePaths, events: Arc<dyn EventSink>) -> Result<Self, ContextError> {
        paths.ensure_dir(&paths.index())?;
        let memory = FileMemoryStore::open(paths)?;
        Ok(Self {
            paths: paths.clone(),
            options: IndexOptions::default(),
            events,
            memory: Arc::new(memory),
            estimator: Arc::new(CharEstimator),
            indexes: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn with_options(mut self, options: IndexOptions) -> Self {
        self.options = options;
        self
    }

    #[must_use]
    pub fn with_memory_store(mut self, store: Arc<dyn MemoryStore>) -> Self {
        self.memory = store;
        self
    }

    #[must_use]
    pub fn with_estimator(mut self, estimator: Arc<dyn TokenEstimator>) -> Self {
        self.estimator = estimator;
        self
    }

    pub fn options(&self) -> &IndexOptions {
        &self.options
    }

    pub fn memory(&self) -> &Arc<dyn MemoryStore> {
        &self.memory
    }

    pub fn estimator(&self) -> &Arc<dyn TokenEstimator> {
        &self.estimator
    }

    /// The (cached) index of `workspace`, opened with corruption recovery.
    pub fn index(&self, workspace: &EntityId) -> Result<Arc<Index>, ContextError> {
        let mut indexes = lock(&self.indexes);
        if let Some(index) = indexes.get(workspace) {
            return Ok(Arc::clone(index));
        }
        let index = Arc::new(Index::open_on_drive(&self.paths, workspace)?);
        indexes.insert(workspace.clone(), Arc::clone(&index));
        Ok(index)
    }

    /// Start (or resume) indexing `target` in the background. Only one job runs
    /// per workspace; a second request returns the running job.
    pub fn start_index(&self, target: IndexTarget) -> Result<IndexJobStarted, ContextError> {
        let workspace = target.workspace_id.clone();
        let mut jobs = lock(&self.jobs);
        if let Some(job) = jobs.get(&workspace) {
            let status = job.status();
            if status.state == JobState::Running {
                return Ok(IndexJobStarted {
                    job_id: status.job_id.clone(),
                    workspace_id: workspace,
                    already_running: true,
                });
            }
        }
        let index = self.index(&workspace)?;
        if index.is_building() {
            return Err(ContextError::IndexBusy(workspace));
        }
        let job_id = EntityId::generate(INDEX_JOB_PREFIX)
            .map_err(|error| ContextError::Index(error.to_string()))?;
        let job = Arc::new(Job {
            status: Mutex::new(IndexJobStatus {
                job_id: job_id.clone(),
                workspace_id: workspace.clone(),
                state: JobState::Running,
                progress: IndexProgress::default(),
                started_at: Timestamp::now(),
                finished_at: None,
                report: None,
                error: None,
                event_error: None,
            }),
            finished: Condvar::new(),
            cancel: CancelToken::new(),
            handle: Mutex::new(None),
        });
        emit(
            &*self.events,
            &job,
            EventKind::TaskCreated {
                task_id: job_id.clone(),
                parent_task_id: None,
                role: INDEX_TASK_ROLE.into(),
            },
        );
        let worker = Arc::clone(&job);
        let events = Arc::clone(&self.events);
        let options = self.options.clone();
        let spawned = thread::Builder::new()
            .name(format!("aidex-index-{workspace}"))
            .spawn(move || run_job(&worker, &index, &target, &options, &*events));
        match spawned {
            Ok(handle) => {
                *lock(&job.handle) = Some(handle);
                jobs.insert(workspace.clone(), job);
                Ok(IndexJobStarted {
                    job_id,
                    workspace_id: workspace,
                    already_running: false,
                })
            }
            Err(error) => {
                let message = format!("cannot start the index job: {error}");
                emit(
                    &*self.events,
                    &job,
                    EventKind::TaskFailed {
                        task_id: job_id,
                        reason: message.clone(),
                    },
                );
                finish(&job, JobState::Failed, None, Some(message.clone()));
                jobs.insert(workspace, job);
                Err(ContextError::Index(message))
            }
        }
    }

    /// Request cancellation of the running job; `false` when none is running.
    pub fn cancel_index(&self, workspace: &EntityId) -> bool {
        let jobs = lock(&self.jobs);
        match jobs.get(workspace) {
            Some(job) if job.status().state == JobState::Running => {
                job.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Wait up to `timeout` for the workspace's job to finish (its terminal
    /// `task.*` event recorded); returns its status.
    pub fn wait_index(&self, workspace: &EntityId, timeout: Duration) -> Option<IndexJobStatus> {
        let job = lock(&self.jobs).get(workspace).map(Arc::clone)?;
        let (status, _) = job
            .finished
            .wait_timeout_while(job.status(), timeout, |status| {
                status.state == JobState::Running
            })
            .unwrap_or_else(PoisonError::into_inner);
        Some(status.clone())
    }

    /// Run a build on the calling thread (for the harness and tests).
    pub fn build_now(
        &self,
        target: &IndexTarget,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<IndexReport, ContextError> {
        let index = self.index(&target.workspace_id)?;
        index.build(target, &self.options, cancel, progress)
    }

    /// The index summary and the latest job of `workspace`.
    pub fn status(&self, workspace: &EntityId) -> Result<IndexStatus, ContextError> {
        let index = self.index(workspace)?;
        let job = lock(&self.jobs)
            .get(workspace)
            .map(|job| job.status().clone());
        Ok(IndexStatus {
            workspace_id: workspace.clone(),
            index: index.summary()?,
            job,
        })
    }

    pub fn search(
        &self,
        workspace: &EntityId,
        query: &Query,
    ) -> Result<Vec<SearchHit>, ContextError> {
        self.index(workspace)?.search(query)
    }

    /// Project rules and instruction files of a workspace root.
    pub fn rules(&self, root: &ConfinedRoot) -> Result<ProjectRules, ContextError> {
        ProjectRules::load(root)
    }

    /// Assemble a budgeted context pack for `target` (see [`assemble`]).
    pub fn assemble(
        &self,
        target: &IndexTarget,
        request: &PackRequest,
    ) -> Result<ContextPack, ContextError> {
        if request.workspace_id != target.workspace_id {
            return Err(ContextError::InvalidRequest(
                "the pack request names a different workspace than the target".into(),
            ));
        }
        let index = self.index(&target.workspace_id)?;
        let rules = if request.include.rules {
            Some(ProjectRules::load(&target.root)?)
        } else {
            None
        };
        let sources = PackSources {
            index: Some(&index),
            root: Some(&target.root),
            rules: rules.as_ref(),
            memory: Some(&*self.memory),
            estimator: &*self.estimator,
        };
        assemble(&sources, request)
    }

    /// Cancel every running job and wait for the workers to stop.
    pub fn shutdown(&self) {
        let jobs: Vec<Arc<Job>> = lock(&self.jobs).values().map(Arc::clone).collect();
        for job in &jobs {
            job.cancel.cancel();
        }
        for job in jobs {
            if let Some(handle) = lock(&job.handle).take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for ContextService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for ContextService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextService")
            .field("index_dir", &self.paths.index())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

fn emit(events: &dyn EventSink, job: &Job, event: EventKind) {
    let workspace = job.status().workspace_id.clone();
    if let Err(error) = events.emit(EventScope::workspace(workspace), event) {
        job.status().event_error = Some(error.to_string());
    }
}

/// Publish the job's terminal state and wake [`ContextService::wait_index`].
/// Called only after the terminal `task.*` event was emitted: until then the job
/// reads as running, so no observer sees it finished without that event (or its
/// `event_error`), and no new job for the workspace can start in between.
fn finish(job: &Job, state: JobState, report: Option<IndexReport>, error: Option<String>) {
    {
        let mut status = job.status();
        status.state = state;
        status.report = report;
        status.error = error;
        status.finished_at = Some(Timestamp::now());
    }
    job.finished.notify_all();
}

fn run_job(
    job: &Job,
    index: &Index,
    target: &IndexTarget,
    options: &IndexOptions,
    events: &dyn EventSink,
) {
    let task_id = job.status().job_id.clone();
    emit(
        events,
        job,
        EventKind::TaskStarted {
            task_id: task_id.clone(),
        },
    );
    let result = index.build(target, options, &job.cancel, &mut |progress| {
        job.status().progress = progress.clone()
    });
    match result {
        Ok(report) => {
            let cancelled = report.outcome == BuildOutcome::Cancelled;
            let (state, event) = if cancelled {
                (JobState::Cancelled, EventKind::TaskCancelled { task_id })
            } else {
                (JobState::Completed, EventKind::TaskCompleted { task_id })
            };
            emit(events, job, event);
            finish(job, state, Some(report), None);
        }
        Err(error) => {
            let message = error.to_string();
            emit(
                events,
                job,
                EventKind::TaskFailed {
                    task_id,
                    reason: message.clone(),
                },
            );
            finish(job, JobState::Failed, None, Some(message));
        }
    }
}
