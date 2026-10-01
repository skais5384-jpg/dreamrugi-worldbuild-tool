//! 앱이 worker와 응답의 수명을 소유한다. 이 mutex 안에서는 I/O나 응답 대기를 하지 않는다.
use crate::{
    commands::{
        backend::{self, Binding, Completed, Job, PendingEdit, Receipt, View},
        dto::*,
    },
    data::{
        application::{
            scheduler::TriggerReason,
            shutdown::{ShutdownBlocker, ShutdownReport},
            worker::{
                RequestId, WorkerCategory, WorkerClient, WorkerConfig, WorkerControl, WorkerStatus,
            },
        },
        collaboration_lock::LockService,
    },
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

const MAX_PROJECTS: usize = 8;
const ORDINARY: usize = 32;
const CONTROL: usize = 8;
const VIEWS: usize = 64;
const SESSIONS: usize = 16;

fn new_project_root(parent: &str, name: &str) -> Result<PathBuf, Code> {
    if parent.is_empty()
        || parent.len() > 32768
        || parent.contains('\0')
        || !PathBuf::from(parent).is_absolute()
        || name.is_empty()
        || name != name.trim()
        || name.encode_utf16().count() > 255
        || name.chars().any(|character| {
            character <= '\u{1f}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        || name.ends_with('.')
        || name.ends_with(' ')
        || matches!(name, "." | "..")
    {
        return Err(Code::InvalidInput);
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Code::InvalidInput);
    }
    Ok(PathBuf::from(parent).join(name))
}
mod local;
mod native;
mod retained;
mod ui_close;
mod update;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caller(Id);
enum Pending {
    Local(local::LocalTask),
    Open(Id, bool),
    Worker(RequestId),
}
struct Operation {
    progress: Arc<crate::data::repository::progress::Progress>,
    caller: Caller,
    lane: Lane,
    input: Option<Arc<Work>>,
    project: Option<Id>,
    pending: Option<Pending>,
    result: Option<Completed>,
    rejected: bool,
    edit_input: Option<Arc<Work>>,
    draft: Option<(Arc<Work>, Completed)>,
}
struct Project {
    root: PathBuf,
    collaborative: bool,
    provider: Arc<dyn LockService>,
    svn_provider: Option<Arc<crate::svn::SvnLockService>>,
    caller: Caller,
    control: WorkerControl<PendingEdit, Receipt>,
    client: WorkerClient<PendingEdit, Receipt>,
    views: BTreeMap<Id, Arc<View>>,
    sessions: BTreeMap<Id, Binding>,
    reports: Vec<ShutdownReport>,
    failures: Vec<crate::data::application::worker::RevalidationFailure>,
    closing: bool,
    joined: bool,
    last_observation: String,
    force_active: usize,
    force_results: Vec<ForceResult>,
}
#[derive(Clone)]
pub(crate) struct ForceResult {
    pub(crate) document: String,
    pub(crate) outcome: &'static str,
    pub(crate) error: Option<String>,
}
pub(crate) struct ForcePermit {
    state: Arc<AppState>,
    project: Id,
    pub(crate) provider: Arc<crate::svn::SvnLockService>,
    document: String,
    outcome: Option<Result<(), String>>,
}
impl ForcePermit {
    pub(crate) fn finish(&mut self, outcome: &Result<(), String>) {
        self.outcome = Some(outcome.clone());
    }
}
impl Drop for ForcePermit {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        let app_closed = state.closed;
        if let Some(project) = state.projects.get_mut(&self.project) {
            project.force_active = project.force_active.saturating_sub(1);
            if project.closing || app_closed {
                let result = self
                    .outcome
                    .take()
                    .unwrap_or_else(|| Err("svn_task_failed".into()));
                project.force_results.push(ForceResult {
                    document: self.document.clone(),
                    outcome: match &result {
                        Ok(()) => "verified",
                        Err(reason)
                            if reason == "svn_lock_unverified" || reason == "svn_task_failed" =>
                        {
                            "unknown"
                        }
                        Err(_) => "rejected",
                    },
                    error: result.err(),
                });
            }
            state.generation = state.generation.saturating_add(1);
        }
        let wake = state.wake.clone();
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
    }
}
struct Inner {
    workspace_owners: Arc<std::sync::atomic::AtomicUsize>,
    caller: Caller,
    revoked: bool,
    closed: bool,
    approved: bool,
    exit_sent: bool,
    generation: u64,
    projects: BTreeMap<Id, Project>,
    operations: BTreeMap<Id, Operation>,
    retained: Vec<(Arc<Work>, Completed)>,
    event_error: Option<Code>,
    native: native::Cleanup,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    ui_close: ui_close::Guard,
    update_intent: Option<String>,
    update_hold: bool,
    startup_blocked: bool,
}
pub(crate) struct AppState {
    inner: Mutex<Inner>,
    lock_root: PathBuf,
    recovery: Arc<crate::data::edit_recovery::Owner>,
    settings: Arc<crate::data::project_settings::ProjectSettings>,
    workspace: Arc<Mutex<backend::workspace::Registry>>,
    provider: Arc<dyn LockService>,
    svn: Option<crate::svn::Manager>,
}
impl AppState {
    pub(crate) fn recovery_handoff_status(&self) -> crate::data::edit_recovery::HandoffStatus {
        self.recovery.handoff_status()
    }
    pub(crate) fn run_recovery_handoff(
        &self,
        stage: &'static str,
    ) -> crate::data::edit_recovery::HandoffStatus {
        let _ = self.recovery.handoff_legacy();
        let status = self.recovery.handoff_status();
        if status.state != "not_applicable" && status.state != "absent" {
            let outcome = match status.state {
                "preserved" => "completed",
                "needs_attention" => "warning",
                _ => "error",
            };
            crate::diagnostic_log::record("recovery", stage, "legacy_handoff", outcome, None, None);
        }
        status
    }
    pub(crate) fn begin_force(
        self: &Arc<Self>,
        root: &Path,
        document: String,
    ) -> Result<ForcePermit, &'static str> {
        let root = root.canonicalize().map_err(|_| "svn_project_missing")?;
        let mut state = self.lock();
        if state.closed || state.revoked {
            return Err("svn_close_project_first");
        }
        let (id, project) = state
            .projects
            .iter_mut()
            .find(|(_, project)| {
                project.root.canonicalize().ok().as_deref() == Some(root.as_path())
            })
            .ok_or("svn_close_project_first")?;
        if project.closing || !project.collaborative {
            return Err("svn_close_project_first");
        }
        if project.force_active != 0 {
            return Err("svn_busy");
        }
        let id = *id;
        let provider = project.svn_provider.clone().ok_or("svn_login_required")?;
        if state
            .operations
            .values()
            .any(|operation| operation.project == Some(id) && operation.pending.is_some())
        {
            return Err("svn_busy");
        }
        state
            .projects
            .get_mut(&id)
            .expect("project admitted")
            .force_active += 1;
        state.generation = state.generation.saturating_add(1);
        Ok(ForcePermit {
            state: self.clone(),
            project: id,
            provider,
            document,
            outcome: None,
        })
    }
    pub(crate) fn managed_project_roots(&self) -> Vec<PathBuf> {
        self.lock()
            .projects
            .values()
            .map(|project| project.root.clone())
            .collect()
    }
    pub(crate) fn svn_lock_active(&self) -> bool {
        self.lock().projects.values().any(|project| {
            project
                .svn_provider
                .as_ref()
                .is_some_and(|provider| !provider.held_targets().is_empty())
        })
    }
    pub(crate) fn svn_commit_provider(
        &self,
        root: &Path,
    ) -> Result<Arc<crate::svn::SvnLockService>, &'static str> {
        let root = root.canonicalize().map_err(|_| "svn_project_missing")?;
        let state = self.lock();
        let (id, project) = state
            .projects
            .iter()
            .find(|(_, project)| {
                project.root.canonicalize().ok().as_deref() == Some(root.as_path())
            })
            .ok_or("svn_close_project_first")?;
        if project.closing || !project.collaborative {
            return Err("svn_close_project_first");
        }
        if project.force_active != 0 {
            return Err("svn_busy");
        }
        if state
            .operations
            .values()
            .any(|operation| operation.project == Some(*id) && operation.pending.is_some())
        {
            return Err("svn_busy");
        }
        project.svn_provider.clone().ok_or("svn_login_required")
    }
    pub(crate) fn svn_update_admission(&self, root: &Path) -> Result<(), &'static str> {
        let roots: Vec<_> = self
            .lock()
            .projects
            .iter()
            .map(|(id, project)| (*id, project.root.clone()))
            .collect();
        let matches: Vec<_> = roots
            .into_iter()
            .filter_map(|(id, path)| {
                (path.canonicalize().ok().as_deref() == Some(root)).then_some(id)
            })
            .collect();
        if matches.is_empty() {
            return Err("svn_close_project_first");
        }
        let state = self.lock();
        for id in matches {
            let Some(project) = state.projects.get(&id) else {
                continue;
            };
            if !project.collaborative || project.closing {
                return Err("svn_close_project_first");
            }
            if project.force_active != 0
                || !project.sessions.is_empty()
                || state
                    .operations
                    .values()
                    .any(|operation| operation.project == Some(id) && operation.pending.is_some())
            {
                return Err("svn_busy");
            }
        }
        Ok(())
    }
    pub(crate) fn new(lock_root: PathBuf) -> Self {
        Self::with_provider(lock_root, backend::provider())
    }
    pub(crate) fn with_recovery_root(
        lock_root: PathBuf,
        settings_root: PathBuf,
        recovery_root: PathBuf,
        legacy_recovery_root: Option<PathBuf>,
    ) -> Self {
        let mut state = Self::new(lock_root);
        state.settings = Arc::new(crate::data::project_settings::ProjectSettings::new(
            settings_root,
        ));
        state.recovery = Arc::new(crate::data::edit_recovery::Owner::with_legacy(
            recovery_root,
            legacy_recovery_root,
        ));
        state
    }
    fn with_provider(lock_root: PathBuf, provider: Arc<dyn LockService>) -> Self {
        let workspace = backend::workspace::Registry::default();
        let workspace_owners = workspace.owners.clone();
        Self {
            inner: Mutex::new(Inner {
                workspace_owners,
                caller: Caller(Id::new()),
                revoked: false,
                closed: false,
                approved: false,
                exit_sent: false,
                generation: 0,
                projects: BTreeMap::new(),
                operations: BTreeMap::new(),
                retained: Vec::new(),
                event_error: None,
                native: native::Cleanup::default(),
                wake: None,
                ui_close: ui_close::Guard::default(),
                update_intent: None,
                update_hold: false,
                startup_blocked: false,
            }),
            recovery: Arc::new(crate::data::edit_recovery::Owner::new(
                lock_root.join("edit-recovery"),
            )),
            settings: Arc::new(crate::data::project_settings::ProjectSettings::new(
                lock_root
                    .parent()
                    .unwrap_or(lock_root.as_path())
                    .join("settings"),
            )),
            lock_root,
            provider,
            svn: None,
            workspace: Arc::new(Mutex::new(workspace)),
        }
    }
    pub(crate) fn with_svn_manager(mut self, manager: crate::svn::Manager) -> Self {
        self.svn = Some(manager);
        self
    }
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| {
            let mut state = e.into_inner();
            state.closed = true;
            state.approved = false;
            state.event_error = Some(Code::Unavailable);
            state
        })
    }
    pub(crate) fn caller(&self, webview: &str, window: &str) -> Reply<Caller> {
        let inner = self.lock();
        if webview != "main" || window != "main" || inner.revoked {
            return Err(Code::Forbidden.into());
        }
        Ok(inner.caller)
    }
    pub(crate) fn dispatch(&self, caller: Caller, command: Command) -> Reply<Response> {
        let mut state = self.lock();
        if caller != state.caller || state.revoked {
            return Err(Code::Forbidden.into());
        }
        collect(&mut state);
        let mutating = !matches!(
            &command,
            Command::Operation { .. }
                | Command::RetainedRead { .. }
                | Command::RetainedList {}
                | Command::ProjectStatus { .. }
                | Command::SessionStatus { .. }
                | Command::AppStatus {}
                | Command::UiCloseStatus {}
                | Command::ForegroundStatus {}
        );
        let mut wake_progress = false;
        let response = match command {
            Command::DocumentProgress { operation, cancel } => {
                let op = operation_for(&state, caller, operation)?;
                // 예약 응답 직후, submit보다 먼저 온 취소도 같은 operation에 보존한다.
                // 실행이 수락된 다른 종류의 작업에는 이 신호를 허용하지 않는다.
                if op.input.is_some()
                    && !matches!(
                        op.input.as_deref(),
                        Some(
                            Work::DocumentWorkspace { .. }
                                | Work::ProjectCopy { .. }
                                | Work::BackupCreate { .. }
                                | Work::BackupList { .. }
                                | Work::BackupInspect { .. }
                                | Work::RestoreNew { .. }
                                | Work::RestoreCurrent { .. }
                                | Work::AssetInspect { .. }
                                | Work::AssetTrashMove { .. }
                                | Work::AssetTrashRestore { .. }
                                | Work::AssetTrashPurge { .. }
                                | Work::TemplatePurge { .. }
                                | Work::DiagnosticExport { .. }
                        )
                    )
                {
                    return Err(Code::WrongBinding.into());
                }
                if cancel {
                    op.progress.cancel();
                }
                let (requested, files, phase) = op.progress.snapshot();
                Response::DocumentProgress {
                    requested,
                    files: files.to_string(),
                    phase,
                }
            }
            Command::Reserve { lane } => {
                if state.exit_sent || (state.closed && lane == Lane::Ordinary) {
                    return Err(Code::Closed.into());
                }
                let limit = if lane == Lane::Ordinary {
                    ORDINARY
                } else {
                    CONTROL
                };
                if state
                    .operations
                    .values()
                    .filter(|op| op.lane == lane)
                    .count()
                    >= limit
                {
                    // 비어 있는 예약에는 입력/부작용이 없다. 수락된 작업/결과는 절대로 eviction하지 않는다.
                    let empty = state
                        .operations
                        .iter()
                        .find(|(_, o)| o.lane == lane && o.input.is_none())
                        .map(|(id, _)| *id);
                    if let Some(id) = empty {
                        state.operations.remove(&id);
                    } else {
                        return Err(Code::Full.into());
                    }
                }
                let operation = Id::new();
                state.operations.insert(
                    operation,
                    Operation {
                        progress: Arc::default(),
                        caller,
                        lane,
                        input: None,
                        project: None,
                        pending: None,
                        result: None,
                        rejected: false,
                        edit_input: None,
                        draft: None,
                    },
                );
                Response::Reserved { operation }
            }
            Command::AbandonReservation { operation } => {
                // 빈 ordinary 예약만 제거한다. submit과 같은 잠금 아래에서 판정하므로
                // 수락된 작업의 입력·결과·편집 owner를 취소하는 경로가 될 수 없다.
                let op = operation_for(&state, caller, operation)?;
                if op.lane != Lane::Ordinary
                    || op.input.is_some()
                    || op.pending.is_some()
                    || op.result.is_some()
                {
                    return Err(Code::NotTerminal.into());
                }
                state.operations.remove(&operation);
                wake_progress = true;
                Response::Acknowledged
            }
            Command::Submit { operation, input } => {
                let control = (input.lane() == Lane::Control).then(|| Arc::new(*input.clone()));
                if let Err(error) = self.submit(&mut state, caller, operation, *input) {
                    // 미수락 제어도 같은 예약 ID로 거부 결과를 조회/ack한다. 원 편집 owner는 건드리지 않는다.
                    // 다른 caller/기존 요청/ordinary 예약은 바꾸지 않고 원 submit 오류도 그대로 반환한다.
                    if let Some(request) = control {
                        if let Some(op) = state.operations.get_mut(&operation).filter(|op| {
                            op.caller == caller && op.lane == Lane::Control && op.input.is_none()
                        }) {
                            op.input = Some(request);
                            op.rejected = true;
                            op.result = Some(Completed::new(
                                error.clone(),
                                Ok(ResultDto::Rejected {
                                    error: error.clone(),
                                    input_retained: false,
                                }),
                            ));
                        }
                    }
                    return Err(error);
                }
                Response::Submitted { operation }
            }
            Command::Operation { operation } => {
                let op = operation_for(&state, caller, operation)?;
                let phase = if op.rejected {
                    OperationPhase::Rejected
                } else if op.result.is_some() {
                    OperationPhase::Complete
                } else if op.pending.is_some() {
                    OperationPhase::Pending
                } else {
                    OperationPhase::Reserved
                };
                let result = op
                    .result
                    .as_ref()
                    .map(|r| r.dto.clone())
                    .transpose()?
                    .map(Box::new);
                Response::Operation {
                    operation,
                    state: phase,
                    result,
                    retained: op
                        .draft
                        .as_ref()
                        .and_then(|(_, r)| r.retained)
                        .or_else(|| op.result.as_ref().and_then(|r| r.retained)),
                }
            }
            Command::AcknowledgeTransport { operation } => {
                let op = operation_for(&state, caller, operation)?;
                let result = op.result.as_ref().ok_or(Code::NotTerminal)?;
                if (result.retain_edit || op.draft.is_some()) && state.retained.len() >= ORDINARY {
                    return Err(Code::OwnersRemain.into());
                }
                let mut op = state.operations.remove(&operation).ok_or(Code::UnknownId)?;
                if let Some(result) = op.result.as_mut() {
                    // 명시적으로 선택한 새 source의 terminal 결과를 인수했으므로 이전 결과만 회수한다.
                    // 새 실패 입력/현재 원 결과는 아래에서 그대로 retained owner로 이동한다.
                    result.previous = None;
                }
                if let Some(draft) = op.draft.take() {
                    state.retained.push(draft);
                } else if op.result.as_ref().is_some_and(|r| r.retain_edit) {
                    let input = op
                        .edit_input
                        .take()
                        .or_else(|| op.input.take())
                        .ok_or(Code::Unavailable)?;
                    // 원 typed 입력/결과 전체를 앱 owner로 옮기므로 transport ack가 편집 폐기가 되지 않는다.
                    state
                        .retained
                        .push((input, op.result.take().ok_or(Code::Unavailable)?));
                }
                wake_progress = true;
                Response::Acknowledged
            }
            Command::ProjectStatus { project } => {
                let p = project_for(&state, caller, project)?;
                let snap = p.control.snapshot();
                let initialization_error = snap
                    .admission_error
                    .clone()
                    .map(backend::policy_error)
                    .or_else(|| snap.initialization_error.as_ref().map(initialization_error));
                Response::Project {
                    project,
                    collaborative: p.collaborative,
                    status: format!("{:?}", snap.status),
                    runtime: snap.runtime.map(|s| format!("{:?}", s.state)),
                    error: initialization_error,
                    validation_failures: validation_failures(p),
                    shutdown: shutdown(p),
                }
            }
            Command::SessionStatus { project, session } => {
                let p = project_for(&state, caller, project)?;
                let b = p.sessions.get(&session).ok_or(Code::UnknownId)?;
                let observation = p
                    .control
                    .session_observation(&b.registration)
                    .ok_or(Code::UnknownId)?;
                let snap = observation.snapshot;
                Response::Session {
                    session,
                    state: format!("{:?}", snap.state()),
                    active_dirty: observation.active_dirty,
                    custody: snap.recovery_handoff().map(|s| format!("{s:?}")),
                    recovery_error: b.recovery.dto(),
                    sink_connected: observation.capability
                        == crate::data::application::recovery_handoff::SinkCapability::Connected,
                }
            }
            Command::ReleaseView { project, view } => {
                let p = project_for(&state, caller, project)?;
                if (!p.joined && p.sessions.values().any(|s| s.views.contains(&view)))
                    || state
                        .operations
                        .values()
                        .any(|o| o.project == Some(project) && o.input.is_some())
                {
                    return Err(Code::OwnersRemain.into());
                }
                state
                    .projects
                    .get_mut(&project)
                    .ok_or(Code::UnknownId)?
                    .views
                    .remove(&view)
                    .ok_or(Code::UnknownId)?;
                Response::Acknowledged
            }
            Command::AppShutdown {} => {
                ui_close::request(&mut state);
                wake_progress = true;
                app(&state)
            }
            Command::UiReady {} => ui_close::register(&mut state)?,
            Command::UiCloseStatus {} => ui_close::status(&state),
            Command::UiCloseDecision { attempt, proceed } => {
                let response = ui_close::decide(&mut state, attempt, proceed)?;
                wake_progress = proceed;
                response
            }
            Command::AppStatus {} | Command::ForegroundStatus {} => app(&state),
            Command::AcknowledgeShutdown { project } => {
                project_for(&state, caller, project)?;
                let p = state.projects.get_mut(&project).ok_or(Code::UnknownId)?;
                wake_progress = !p.reports.is_empty() || !p.failures.is_empty();
                p.reports.clear();
                p.failures.clear();
                p.force_results.clear();
                Response::Acknowledged
            }
            Command::ReleaseRound { project } => {
                let p = project_for(&state, caller, project)?;
                p.control
                    .request_release_round()
                    .map_err(|_| Code::ReleaseRejected)?;
                Response::Acknowledged
            }
            Command::RetainedRead { retained } => retained::read(&state, caller, retained)?,
            Command::RetainedList {} => Response::RetainedList {
                entries: retained::list(&state, caller),
            },
            Command::RetryNativeCleanup { generation } => {
                native::retry(&mut state, &generation)?;
                wake_progress = true;
                app(&state)
            }
        };
        if mutating {
            state.generation = state.generation.checked_add(1).ok_or(Code::Unavailable)?;
        }
        // IPC 인수/명시적 재시도 뒤에는 새 native 입력 없이도 기존 cleanup/exit 검사가 필요하다.
        // wake는 허가가 아닌 실행 기회이며, callback 재진입을 위해 앱 잠금을 먼저 푼다.
        let wake = if wake_progress && state.closed {
            state.wake.clone()
        } else {
            None
        };
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
        Ok(response)
    }
    fn submit(&self, state: &mut Inner, caller: Caller, operation: Id, input: Work) -> Reply<()> {
        // 시작 선택 전에는 runtime을 열거나 프로젝트를 쓰는 경로 자체를 허용하지 않는다.
        if state.startup_blocked
            && input.lane() == Lane::Ordinary
            && !backend::workspace::center::is_local(&input)
        {
            return Err(Code::Starting.into());
        }
        let op = operation_for(state, caller, operation)?;
        if let Some(previous) = &op.input {
            return if **previous == input {
                Ok(())
            } else {
                Err(Code::DuplicateConflict.into())
            };
        }
        if op.lane != input.lane() {
            return Err(Code::WrongBinding.into());
        }
        if state.exit_sent || (state.closed && input.lane() == Lane::Ordinary) {
            return Err(Code::Closed.into());
        }
        let request = Arc::new(input);
        state.approved = false;
        let retiring_project = match &*request {
            Work::RetireProject { project } => Some(*project),
            _ => None,
        };
        if retained::handle(state, caller, operation, request.clone())? {
            let retired = state
                .operations
                .get(&operation)
                .and_then(|operation| operation.result.as_ref())
                .is_some_and(|result| matches!(result.dto, Ok(ResultDto::ProjectRetired { .. })));
            if retired {
                let mut workspace = self
                    .workspace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                workspace.release_project(retiring_project.expect("retired project input"));
            }
            return Ok(());
        }
        let input = retained::resume_input(state, caller, &request)?;
        if let Work::RestoreNew { parent, .. } | Work::ProjectCopy { parent, .. } = &*input {
            if crate::svn_guard::working_copy_root(PathBuf::from(parent).as_path()).is_some() {
                return Err(Code::CollaborationReadOnly.into());
            }
        }
        if let Some(project) = input.project() {
            let p = project_for(state, caller, project)?;
            if p.collaborative
                && !crate::svn_guard::permitted_in_collaboration(&input, p.svn_provider.is_some())
            {
                return Err(Code::CollaborationReadOnly.into());
            }
        }
        if backend::workspace::center::is_local(&input) {
            let owned = input.clone();
            let store = self.recovery.clone();
            let registry = self.workspace.clone();
            let wake = state.wake.clone();
            let task = local::LocalTask::start(
                move || backend::workspace::center::run(owned, store, registry),
                wake,
            )
            .map_err(|_| Code::Unavailable)?;
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.pending = Some(Pending::Local(task));
            return Ok(());
        }
        if backend::project_settings::is_local(&input) {
            let owned = input.clone();
            let settings = self.settings.clone();
            let wake = state.wake.clone();
            let task = local::LocalTask::start(
                move || backend::project_settings::run(owned, settings),
                wake,
            )
            .map_err(|_| Code::Unavailable)?;
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.pending = Some(Pending::Local(task));
            return Ok(());
        }
        if backend::project_backup::is_local(&input) {
            let owned = input.clone();
            let wake = state.wake.clone();
            let progress = state
                .operations
                .get(&operation)
                .ok_or(Code::UnknownId)?
                .progress
                .clone();
            let task = local::LocalTask::start(
                move || {
                    let _progress = crate::data::repository::progress::scope(progress);
                    backend::project_backup::run(owned)
                },
                wake,
            )
            .map_err(|_| Code::Unavailable)?;
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.pending = Some(Pending::Local(task));
            return Ok(());
        }
        if matches!(&*input, Work::Open { .. } | Work::CreateProject { .. }) {
            let (project_root, initialize_empty, create_directory) = match &*input {
                Work::Open { root } => {
                    if root.is_empty()
                        || root.len() > 32768
                        || root.contains('\0')
                        || !PathBuf::from(root).is_absolute()
                    {
                        return Err(Code::InvalidInput.into());
                    }
                    (PathBuf::from(root), false, false)
                }
                Work::CreateProject { parent, name } => {
                    if crate::svn_guard::working_copy_root(PathBuf::from(parent).as_path())
                        .is_some()
                    {
                        return Err(Code::CollaborationReadOnly.into());
                    }
                    (new_project_root(parent, name)?, true, true)
                }
                _ => unreachable!(),
            };
            if state.projects.len() >= MAX_PROJECTS {
                return Err(Code::Full.into());
            }
            let project = Id::new();
            let working_copy = crate::svn_guard::working_copy_root(&project_root);
            let collaborative = working_copy.is_some();
            if let Some(working_copy) = working_copy {
                crate::svn_guard::remember(&working_copy).map_err(|_| Code::Unavailable)?;
                crate::svn_guard::remember(&project_root).map_err(|_| Code::Unavailable)?;
            }
            let svn_provider = if collaborative {
                self.svn.as_ref().map(|manager| {
                    Arc::new(crate::svn::SvnLockService::new(
                        manager.clone(),
                        project_root.clone(),
                    ))
                })
            } else {
                None
            };
            let provider: Arc<dyn LockService> = svn_provider
                .as_ref()
                .map(|provider| provider.clone() as Arc<dyn LockService>)
                .unwrap_or_else(|| self.provider.clone());
            // start에는 locator만 넘긴다. canonicalize/acquire/recovery는 project worker에서 한다.
            let admission: Option<Box<dyn FnOnce() -> Result<(), String> + Send>> = if collaborative
            {
                let provider = svn_provider.clone();
                Some(Box::new(move || {
                    provider
                        .ok_or_else(|| "svn_cli_missing".to_owned())?
                        .policy_admit()
                }))
            } else {
                None
            };
            let (control, client) = WorkerControl::start_with_admission(
                WorkerConfig {
                    project_root: project_root.clone(),
                    lock_root: self.lock_root.clone(),
                    initialize_empty,
                    create_directory,
                    capacity: ORDINARY,
                    max_sessions: SESSIONS,
                    interval: Duration::from_secs(30),
                    automatic_revalidation: !collaborative,
                },
                admission,
            )
            .map_err(|_| Code::Unavailable)?;
            if let Some(wake) = &state.wake {
                control.set_app_wake(wake.clone());
            }
            state.projects.insert(
                project,
                Project {
                    collaborative,
                    provider,
                    svn_provider,
                    root: project_root,
                    caller,
                    control,
                    client,
                    views: BTreeMap::new(),
                    sessions: BTreeMap::new(),
                    reports: Vec::new(),
                    failures: Vec::new(),
                    closing: false,
                    joined: false,
                    last_observation: String::new(),
                    force_active: 0,
                    force_results: Vec::new(),
                },
            );
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.project = Some(project);
            op.pending = Some(Pending::Open(project, initialize_empty));
            return Ok(());
        }
        let project = input.project().ok_or(Code::InvalidInput)?;
        let p = project_for(state, caller, project)?;
        if p.closing && input.lane() == Lane::Ordinary {
            return Err(Code::Closed.into());
        }
        if matches!(
            &*input,
            Work::DocumentWorkspace {
                request: crate::commands::document_workspace::Request::Mutate { .. },
                ..
            }
        ) && (p.sessions.values().any(|b| {
            b.views.iter().any(|id| {
                p.views
                    .get(id)
                    .is_some_and(|v| matches!(&**v, backend::View::Document(_)))
            })
        }) || state.operations.values().any(|op| {
            op.project == Some(project)
                && op.result.is_none()
                && op.input.as_ref().is_some_and(|w| {
                    matches!(
                        &**w,
                        Work::BeginSession { .. }
                            | Work::SaveDocument { .. }
                            | Work::SaveComposite { .. }
                            | Work::CreateDocument { .. }
                    )
                })
        })) {
            return Err(Code::OwnersRemain.into());
        }
        let allocated = Id::new();
        let binding = input
            .session()
            .map(|s| p.sessions.get(&s).cloned().ok_or(Code::UnknownId))
            .transpose()?;
        let requested = match &*input {
            Work::BeginTemplateDraft { view, .. } => view.iter().copied().collect(),
            Work::DuplicateTemplate { view, .. }
            | Work::CreateDocument { view, .. }
            | Work::UpdateTemplate { view, .. }
            | Work::TombstoneTemplate { view, .. } => vec![*view],
            Work::BeginSession { views, .. } => views.clone(),
            Work::MaterializeDocument {
                document, template, ..
            }
            | Work::SaveDocument {
                document, template, ..
            }
            | Work::SaveComposite {
                document, template, ..
            } => vec![*document, *template],
            _ => vec![],
        };
        if requested.len() > 2
            || (matches!(&*input, Work::BeginSession { .. }) && requested.is_empty())
        {
            return Err(Code::InvalidInput.into());
        }
        if let Some(b) = &binding {
            if requested.iter().any(|v| !b.views.contains(v)) {
                return Err(Code::WrongBinding.into());
            }
        }
        let views = requested
            .iter()
            .map(|id| {
                p.views
                    .get(id)
                    .cloned()
                    .map(|v| (*id, v))
                    .ok_or_else(|| Code::WrongBinding.into())
            })
            .collect::<Reply<Vec<_>>>()?;
        let pending_views = state
            .operations
            .values()
            .filter(|o| o.project == Some(project) && o.pending.is_some())
            .count();
        if matches!(
            &*input,
            Work::ReadTemplate { .. } | Work::ReadDocument { .. }
        ) && p.views.len() + pending_views >= VIEWS
        {
            return Err(Code::Full.into());
        }
        if matches!(
            &*input,
            Work::BeginSession { .. }
                | Work::BeginTemplateDraft { .. }
                | Work::RecoveryRestore { .. }
                | Work::CreateTemplate { .. }
                | Work::DuplicateTemplate { .. }
                | Work::CreateDocument { .. }
        ) && p.sessions.len() + pending_views >= SESSIONS
        {
            return Err(Code::Full.into());
        }
        if let Work::Close { .. } = &*input {
            if state
                .workspace_owners
                .load(std::sync::atomic::Ordering::Acquire)
                != 0
            {
                return Err(Code::OwnersRemain.into());
            }
            let p = state.projects.get_mut(&project).ok_or(Code::UnknownId)?;
            p.closing = true;
            let original = p.control.request_shutdown();
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.project = Some(project);
            op.result = Some(Completed::new(
                original,
                Ok(ResultDto::Control { error: None }),
            ));
            return Ok(());
        }
        if let Work::SessionControl {
            control: SessionControl::Revalidate,
            ..
        } = &*input
        {
            let key = binding
                .as_ref()
                .and_then(|b| b.key.as_ref())
                .ok_or(Code::WrongBinding)?;
            let snapshot = p.control.snapshot();
            // app이 닫은 admission과 실제 worker 장애를 같은 Stopped category로 섞지 않는다.
            let original = if matches!(snapshot.status, WorkerStatus::Unavailable) {
                Err(WorkerCategory::Unavailable)
            } else if snapshot.initialization_error.is_some() {
                Err(WorkerCategory::InitializationFailed)
            } else if p.closing || state.closed {
                Err(WorkerCategory::Closed)
            } else {
                p.client.trigger(key, TriggerReason::Foreground)
            };
            // 실행 결과의 거부도 terminal DTO다. 조회의 transport 오류로 바꾸지 않는다.
            let dto = Ok(ResultDto::Control {
                error: original.as_ref().err().map(|e| worker_error(*e)),
            });
            let op = state
                .operations
                .get_mut(&operation)
                .ok_or(Code::UnknownId)?;
            op.input = Some(input);
            op.project = Some(project);
            op.result = Some(Completed::new(original, dto));
            return Ok(());
        }
        let progress = operation_for(state, caller, operation)?.progress.clone();
        let job = Job {
            progress,
            input: input.clone(),
            views,
            binding,
            provider: p.provider.clone(),
            collaborative: p.collaborative,
            allocated,
            operation,
            recovery: self.recovery.clone(),
            workspace: self.workspace.clone(),
        };
        let source_owners = job.views.iter().map(|(_, v)| v.clone()).collect();
        let admitted = if input.lane() == Lane::Control {
            p.control.try_control(job, backend::cleanup)
        } else {
            p.client.try_submit(job, backend::run)
        };
        // 정상 admission 검사가 끝난 뒤 같은 app lock에서 원본을 목적 작업 owner로 옮긴다.
        let inherited = if let Work::ResumeRetained { retained, .. } = &*request {
            Some(retained::take(state, caller, *retained)?)
        } else {
            None
        };
        let op = state
            .operations
            .get_mut(&operation)
            .ok_or(Code::UnknownId)?;
        op.input = Some(request);
        op.edit_input = Some(input.clone());
        op.draft = inherited;
        op.project = Some(project);
        match admitted {
            Ok(ticket) => op.pending = Some(Pending::Worker(ticket.id())),
            Err(rejected) => {
                let error = worker_error(rejected.category);
                let mut result = Completed::new(
                    rejected,
                    Ok(ResultDto::Rejected {
                        error,
                        input_retained: true,
                    }),
                );
                result.retain_edit = input.contains_edit();
                result.g6_clearable = backend::g6_intent(&input).is_some();
                result.sources = source_owners;
                retained::finish(operation, caller, op, &mut result);
                op.result = Some(result);
                op.rejected = true;
            }
        }
        Ok(())
    }
    pub(crate) fn request_shutdown(&self) {
        ui_close::request(&mut self.lock());
    }
    pub(crate) fn trigger(&self, reason: TriggerReason) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        let mut failure = None;
        for p in state.projects.values() {
            // Focus and OS resume are idle events, not explicit SVN actions.
            // SVN validates the current token during edit/save/commit instead.
            if p.closing || p.collaborative {
                continue;
            }
            for b in p.sessions.values() {
                if let Some(key) = &b.key {
                    if let Err(e) = p.client.trigger(key, reason) {
                        if !matches!(e, WorkerCategory::Stale | WorkerCategory::Inactive) {
                            failure = Some(worker_error(e).code);
                        }
                    }
                }
            }
        }
        state.event_error = failure;
        state.generation = state.generation.saturating_add(1);
    }
    pub(crate) fn poll(&self) -> bool {
        let mut state = self.lock();
        collect(&mut state);
        native::refresh(&mut state);
        state.approved
    }
    pub(crate) fn take_exit_approval(&self) -> bool {
        let mut state = self.lock();
        native::refresh(&mut state);
        if state.approved && !state.exit_sent && !state.update_hold {
            state.exit_sent = true;
            true
        } else {
            false
        }
    }
    pub(crate) fn defer_exit(&self) {
        let mut state = self.lock();
        if state.approved {
            state.exit_sent = false;
        }
    }
    pub(crate) fn exit_approved(&self) -> bool {
        self.lock().approved
    }
    pub(crate) fn shutdown_wait_reason(&self) -> Option<&'static str> {
        let state = self.lock();
        if !state.closed {
            return None;
        }
        if state
            .workspace_owners
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return Some("workspace_owner");
        }
        if state
            .operations
            .values()
            .any(|operation| operation.input.is_some())
        {
            return Some("operation_owner");
        }
        if !state.retained.is_empty() {
            return Some("retained_input");
        }
        if state.projects.values().any(|project| !project.joined) {
            return Some("project_join");
        }
        if state
            .projects
            .values()
            .any(|project| project.force_active != 0)
        {
            return Some("svn_force_active");
        }
        if state
            .projects
            .values()
            .any(|project| !project.reports.is_empty())
        {
            return Some("project_report");
        }
        if state
            .projects
            .values()
            .any(|project| !project.failures.is_empty())
        {
            return Some("project_failure");
        }
        if state
            .projects
            .values()
            .any(|project| !project.control.shutdown_snapshot().normal_exit_allowed)
        {
            return Some("project_blocker");
        }
        if matches!(
            state.event_error,
            Some(Code::Unavailable | Code::PlatformRegistration)
        ) {
            return Some("event_failure");
        }
        Some(match native::dto(&state).phase {
            NativePhase::Registered => "native_registered",
            NativePhase::Pending => "native_pending",
            NativePhase::Running => "native_running",
            NativePhase::Failed => "native_failed",
            NativePhase::Complete if state.approved => "exit_approved",
            NativePhase::Complete => "exit_not_approved",
        })
    }
    pub(crate) fn event_failure(&self, code: Code) {
        let mut state = self.lock();
        state.event_error = Some(code);
        if code == Code::PlatformRemoval {
            native::failed(&mut state);
        }
    }
    pub(crate) fn revoke(&self) {
        let mut state = self.lock();
        state.revoked = true;
        request_shutdown(&mut state);
    }
    pub(crate) fn generation(&self) -> String {
        self.lock().generation.to_string()
    }
    /// WebView callback처럼 AppState 바깥에서 끝나는 선택 기능도 같은 bounded hint를 쓴다.
    /// 종료가 시작된 뒤의 늦은 callback은 store만 정리하고 event loop를 다시 깨우지 않는다.
    pub(crate) fn support_diagnostics_changed(&self) -> bool {
        let mut state = self.lock();
        if state.closed || state.exit_sent {
            return false;
        }
        state.generation = state.generation.saturating_add(1);
        let wake = state.wake.clone();
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
        true
    }
    pub(crate) fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.lock().wake = Some(wake);
    }
    pub(crate) fn clear_wake(&self) {
        let mut state = self.lock();
        for p in state.projects.values() {
            p.control.clear_app_wake();
        }
        state.wake = None;
    }
    pub(crate) fn join_pending(&self) -> bool {
        self.lock()
            .projects
            .values()
            .any(|p| p.control.shutdown_snapshot().stop_requested && !p.joined)
    }
}
fn operation_for(state: &Inner, caller: Caller, id: Id) -> Reply<&Operation> {
    state
        .operations
        .get(&id)
        .filter(|o| o.caller == caller)
        .ok_or_else(|| Code::UnknownId.into())
}
fn project_for(state: &Inner, caller: Caller, id: Id) -> Reply<&Project> {
    state
        .projects
        .get(&id)
        .filter(|p| p.caller == caller)
        .ok_or_else(|| Code::UnknownId.into())
}
fn worker_error(error: WorkerCategory) -> ErrorDto {
    ErrorDto::new(match error {
        WorkerCategory::Starting => Code::Starting,
        WorkerCategory::InitializationFailed => Code::InitializationFailed,
        WorkerCategory::Full => Code::Full,
        WorkerCategory::Closed => Code::Closed,
        WorkerCategory::OwnersRemain => Code::OwnersRemain,
        _ => Code::Unavailable,
    })
}
fn request_shutdown(state: &mut Inner) {
    if state
        .workspace_owners
        .load(std::sync::atomic::Ordering::Acquire)
        != 0
        || state
            .operations
            .values()
            .any(|o| matches!(o.pending, Some(Pending::Local(_))))
        || state.operations.values().any(|o| {
            o.pending.is_some()
                && o.input.as_ref().is_some_and(|i| {
                    matches!(
                        &**i,
                        Work::BeginTemplateDraft { .. } | Work::RecoveryRestore { .. }
                    )
                })
        })
    {
        state.approved = false;
        state.event_error = Some(Code::OwnersRemain);
        return;
    }
    if !state.closed {
        state.generation = state.generation.saturating_add(1);
    }
    state.closed = true;
    for p in state.projects.values_mut() {
        if !p.closing {
            p.closing = true;
            p.control.request_shutdown();
        }
    }
}
fn shutdown(p: &Project) -> ShutdownDto {
    let s = p.control.shutdown_snapshot();
    let mut blockers = s.blockers.clone();
    if !p.reports.is_empty() && !blockers.contains(&ShutdownBlocker::Results) {
        blockers.push(ShutdownBlocker::Results);
    }
    if !p.force_results.is_empty() && !blockers.contains(&ShutdownBlocker::Results) {
        blockers.push(ShutdownBlocker::Results);
    }
    ShutdownDto {
        closing: p.closing,
        phase: format!("{:?}", s.phase),
        round: s.round.to_string(),
        blockers: blockers.iter().map(|b| format!("{b:?}")).collect(),
        report_pending: s.report_pending || !p.reports.is_empty() || !p.force_results.is_empty(),
        resources_complete: s.resources_complete,
        normal_exit_allowed: s.normal_exit_allowed
            && p.reports.is_empty()
            && p.failures.is_empty()
            && p.force_active == 0
            && p.force_results.is_empty(),
        joined: p.joined,
        force_active: p.force_active != 0,
        force_results: p
            .force_results
            .iter()
            .map(|r| crate::commands::dto::ForceResultDto {
                document: r.document.clone(),
                outcome: r.outcome.to_owned(),
                error: r.error.clone(),
            })
            .collect(),
        next_actions: blockers.iter().map(ToString::to_string).collect(),
        reports: p
            .reports
            .iter()
            .map(|r| ShutdownReportDto {
                round: r.round.to_string(),
                initialization_failed: r.initialization_error.is_some(),
                close_failed: r.close.as_ref().is_some_and(Result::is_err),
                release_failures: r
                    .sessions
                    .iter()
                    .flat_map(|s| s.end.iter().chain(&s.retries))
                    .filter(|r| r.original.is_err())
                    .count()
                    .to_string(),
                coordination_errors: r
                    .coordination_errors
                    .iter()
                    .map(|e| format!("{e:?}"))
                    .collect(),
            })
            .collect(),
    }
}

fn initialization_error(error: &crate::data::project_runtime::RuntimeError) -> ErrorDto {
    use crate::data::project_runtime::{InitializationCleanupOutcome, RuntimeCategory};

    let diagnostic = error.diagnostic();
    let code = if diagnostic.category == RuntimeCategory::ProjectNotEmpty {
        Code::ProjectNotEmpty
    } else {
        Code::InitializationFailed
    };
    let cleanup_outcome = diagnostic
        .initialization_cleanup_outcome
        .map(|outcome| match outcome {
            InitializationCleanupOutcome::Removed => "removed",
            InitializationCleanupOutcome::PreservedExternalEntries => "preserved_external_entries",
            InitializationCleanupOutcome::Failed => "failed",
        })
        .map(str::to_owned);
    ErrorDto {
        code,
        next_action: diagnostic.next_action(),
        diagnostic: Some(ErrorDiagnosticDto {
            stage: format!("{:?}", diagnostic.stage),
            category: format!("{:?}", diagnostic.category),
            outcome: None,
            io_kind: diagnostic.io.map(|io| format!("{:?}", io.kind)),
            os_code: diagnostic.io.and_then(|io| io.os_code),
            cleanup_outcome,
            cleanup_io_kind: diagnostic
                .initialization_cleanup
                .map(|io| format!("{:?}", io.kind)),
            cleanup_os_code: diagnostic.initialization_cleanup.and_then(|io| io.os_code),
        }),
    }
}
fn validation_failures(p: &Project) -> Vec<ValidationDto> {
    use crate::data::edit_session::{EditSessionError, ValidationErrorSource};
    p.failures
        .iter()
        .map(|f| ValidationDto {
            session: p
                .sessions
                .values()
                .find(|b| b.key.as_ref() == Some(&f.key))
                .map(|b| b.id),
            category: format!("{:?}", f.original.category()),
            lock_category: match &f.original {
                EditSessionError::Validation {
                    source: ValidationErrorSource::Lock(e),
                    ..
                } => Some(format!("{:?}", e.category())),
                _ => None,
            },
            preserve_failed: f.preserve.as_ref().is_some_and(Result::is_err),
        })
        .collect()
}
fn app(state: &Inner) -> Response {
    Response::App {
        generation: state.generation.to_string(),
        closing: state.closed,
        projects: state.projects.len().to_string(),
        operations: state
            .operations
            .values()
            .filter(|o| o.input.is_some())
            .count()
            .to_string(),
        retained_edits: state.retained.len().to_string(),
        normal_exit_allowed: state.approved,
        event_error: state.event_error,
        native_cleanup: native::dto(state),
        support_diagnostics: crate::support_diagnostics::snapshot(),
        diagnostics: crate::diagnostic_log::status(),
    }
}
fn collect(state: &mut Inner) {
    let mut acknowledged_inputs = Vec::new();
    for (operation, op) in state.operations.iter_mut() {
        if let Some(Pending::Local(task)) = &op.pending {
            let Some(result) = task.completed() else {
                continue;
            };
            if let Some(Pending::Local(task)) = op.pending.take() {
                op.result = Some(task.retain(result));
                state.generation = state.generation.saturating_add(1);
            }
            continue;
        }
        let Some(p) = op.project.and_then(|id| state.projects.get_mut(&id)) else {
            continue;
        };
        let completed = match &op.pending {
            Some(Pending::Open(project, created)) => {
                let snap = p.control.snapshot();
                if snap.status == WorkerStatus::Starting {
                    None
                } else {
                    let error = snap
                        .admission_error
                        .clone()
                        .map(backend::policy_error)
                        .or_else(|| snap.initialization_error.as_ref().map(initialization_error));
                    let dto = ResultDto::Open {
                        project: *project,
                        status: format!("{:?}", snap.status),
                        runtime: snap.runtime.map(|s| format!("{:?}", s.state)),
                        error,
                        created: *created,
                    };
                    Some(Completed::new(snap, Ok(dto)))
                }
            }
            Some(Pending::Worker(request)) => match p.control.try_take::<Completed>(request) {
                Ok(result) => result,
                Err(e) => {
                    state.event_error = Some(worker_error(e).code);
                    None
                }
            },
            None | Some(Pending::Local(_)) => None,
        };
        if let Some(mut result) = completed {
            retained::finish(*operation, op.caller, op, &mut result);
            if let Some((id, view)) = &result.view {
                p.views.insert(*id, view.clone());
            }
            if let Some(binding) = &result.binding {
                p.sessions.insert(binding.id, binding.clone());
            }
            if let Some(id) = result.removed_binding {
                p.sessions.remove(&id);
            }
            if let Some(id) = result.acknowledged_session {
                if let Some(input) = p.sessions.get_mut(&id).and_then(|b| b.draft_input.take()) {
                    acknowledged_inputs.push(input);
                }
            }
            op.result = Some(result);
            op.pending = None;
            state.generation = state.generation.saturating_add(1);
        }
    }
    for input in acknowledged_inputs {
        // 실제 receipt를 받은 바로 그 P와 같은 입력만 해제한다. 다른 실패 요청은 남는다.
        state
            .retained
            .retain(|(owner, _)| !Arc::ptr_eq(owner, &input));
        for op in state.operations.values_mut() {
            if op
                .input
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &input))
            {
                if let Some(result) = op.result.as_mut() {
                    result.retain_edit = false;
                }
            }
        }
    }
    for p in state.projects.values_mut() {
        for binding in p.sessions.values() {
            if p.failures.len() < SESSIONS {
                if let Some(key) = &binding.key {
                    if let Some(failure) = p.control.take_validation_failure(key) {
                        p.failures.push(failure);
                    }
                }
            }
        }
        if p.closing && !p.joined {
            if p.reports.len() < CONTROL {
                if let Some(report) = p.control.take_shutdown_report() {
                    p.reports.push(report);
                }
            }
            let s = p.control.shutdown_snapshot();
            if !s.stop_requested {
                match request_project_stop(p) {
                    Ok(()) => {}
                    Err(
                        WorkerCategory::OwnersRemain
                        | WorkerCategory::MustCloseAdmission
                        | WorkerCategory::Starting,
                    ) => {}
                    Err(e) => state.event_error = Some(worker_error(e).code),
                }
            }
            if p.control.shutdown_snapshot().stop_requested {
                match p.control.try_join() {
                    Ok(joined) => p.joined = joined,
                    Err(e) => state.event_error = Some(worker_error(e).code),
                }
            }
        }
        // 보고서 공개, refresh, join도 새 관측이다. 상태 조회 자체만으로 알림을 반복하지 않는다.
        let s = p.control.shutdown_snapshot();
        let observation = format!(
            "{:?}/{}/{:?}/{}/{}/{}/{}/{}",
            s.phase,
            s.round,
            s.blockers,
            s.resources_complete,
            s.normal_exit_allowed,
            p.joined,
            p.reports.len(),
            p.failures.len()
        );
        if observation != p.last_observation {
            p.last_observation = observation;
            state.generation = state.generation.saturating_add(1);
        }
    }
}

fn request_project_stop(p: &Project) -> Result<(), WorkerCategory> {
    match p.control.request_stop() {
        Err(WorkerCategory::Unavailable)
            if p.closing
                && p.control.shutdown_snapshot().stop_requested
                && p.control.snapshot().status == WorkerStatus::Stopped =>
        {
            // snapshot 뒤 worker 자동 stop이 먼저 끝날 수 있다. 실제 의도된 정지만 확인하며
            // Unavailable/panic/init 실패는 이 분기로 정상화하지 않는다.
            Ok(())
        }
        original => original,
    }
}

#[cfg(all(test, windows))]
mod tests;
