//! worker 실행 어댑터. DTO와 독립적인 원 결과 타입을 AppState에 넘긴다.
mod asset_maintenance;
mod asset_open;
mod document_edit;
mod document_replace;
mod document_search;
mod document_workspace;
mod transition;
#[cfg(test)]
pub(crate) use document_workspace::install_issue_snapshot_hook;
pub(crate) mod project_backup;
pub(crate) mod project_settings;
pub(crate) mod recovery;
mod recovery_document;
pub(crate) mod recovery_merge;
pub(crate) mod workspace;
use super::{convert, dto::*, projection};
use crate::data::{
    application::{
        composite,
        diagnostics::{ApplicationDiagnostic, DiskState},
        documents::{self, persistence},
        templates,
        worker::{CleanupContext, Registration, SessionKey, WorkerContext},
    },
    artifact::{self, DocumentArtifact, TemplateArtifact},
    collaboration_lock::{LockService, NoLockService},
    repository::{ArtifactRepository, ArtifactSourceId, LoadedArtifact},
};
use std::{any::Any, sync::Arc};

/// 운영 생성자는 M3 adapter가 맡는다. frontend나 bool로 receipt를 구성할 수 없다.
pub(crate) struct Receipt {
    _proof: Box<dyn Any + Send>,
}
impl Receipt {
    fn from_store(proof: crate::data::edit_recovery::Proof) -> Self {
        Self {
            _proof: Box::new(proof),
        }
    }
}
#[cfg(test)]
impl Receipt {
    pub(crate) fn from_test_sink(proof: Box<dyn Any + Send>) -> Self {
        Self { _proof: proof }
    }
}
pub(crate) struct PendingEdit {
    pub(crate) input: Arc<Work>,
    // 원본 source와 표시 snapshot도 P의 수명에 묶인다. frontend handle만 남겨 복구를 추측하지 않는다.
    #[allow(dead_code, reason = "M3 인계 때까지 backend source owner를 보관한다")]
    views: Vec<Arc<View>>,
    recovery: recovery::PendingRecovery,
}
pub(crate) type Context = WorkerContext<PendingEdit, Receipt>;
pub(crate) enum View {
    Template(LoadedArtifact<TemplateArtifact>),
    Document(LoadedArtifact<DocumentArtifact>),
}
impl View {
    pub(crate) fn target(&self) -> ArtifactSourceId {
        match self {
            Self::Template(v) => v.source().id(),
            Self::Document(v) => v.source().id(),
        }
    }
    fn template(&self) -> Reply<templates::TemplateSource> {
        match self {
            Self::Template(v) => Ok(templates::TemplateSource {
                id: v.artifact().template_id(),
                token: v.source().clone(),
                expected_revision: v.artifact().revision(),
            }),
            _ => Err(Code::WrongBinding.into()),
        }
    }
    fn document(&self) -> Reply<persistence::DocumentSource> {
        match self {
            Self::Document(v) => Ok(persistence::DocumentSource {
                id: v.artifact().document_id(),
                token: v.source().clone(),
            }),
            _ => Err(Code::WrongBinding.into()),
        }
    }
}
#[derive(Clone)]
pub(crate) struct Binding {
    pub(crate) id: Id,
    pub(crate) registration: Registration,
    pub(crate) key: Option<SessionKey>,
    pub(crate) views: Vec<Id>,
    pub(crate) draft_input: Option<Arc<Work>>,
    pub(crate) recovery: Arc<recovery::Observation>,
}
#[derive(Clone)]
pub(crate) struct Job {
    pub(crate) input: Arc<Work>,
    pub(crate) views: Vec<(Id, Arc<View>)>,
    pub(crate) binding: Option<Binding>,
    pub(crate) provider: Arc<dyn LockService>,
    pub(crate) collaborative: bool,
    pub(crate) allocated: Id,
    pub(crate) operation: Id,
    pub(crate) progress: Arc<crate::data::repository::progress::Progress>,
    pub(crate) recovery: Arc<crate::data::edit_recovery::Owner>,
    pub(crate) workspace: Arc<std::sync::Mutex<workspace::Registry>>,
}
impl Job {
    fn view(&self, id: Id) -> Reply<&View> {
        self.views
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, v)| &**v)
            .ok_or_else(|| Code::WrongBinding.into())
    }
    fn source(&self, id: Id, revision: &str) -> Reply<templates::TemplateSource> {
        let source = self.view(id)?.template()?;
        if source.expected_revision != convert::revision(revision)? {
            return Err(Code::WrongBinding.into());
        }
        Ok(source)
    }
    fn key(&self) -> Reply<&SessionKey> {
        self.binding
            .as_ref()
            .and_then(|b| b.key.as_ref())
            .ok_or_else(|| Code::SessionRejected.into())
    }
}
pub(crate) struct Completed {
    // 이 owner는 응답 future로 이동하지 않는다. 재직렬화 실패 때도 그대로 보관한다.
    #[allow(
        dead_code,
        reason = "소비하지 않은 원 결과를 transport 인수 또는 앱 편집 owner 수명까지 보관한다"
    )]
    pub(crate) original: Box<dyn Any + Send>,
    pub(crate) dto: Reply<ResultDto>,
    pub(crate) view: Option<(Id, Arc<View>)>,
    pub(crate) binding: Option<Binding>,
    pub(crate) removed_binding: Option<Id>,
    pub(crate) acknowledged_session: Option<Id>,
    pub(crate) retain_edit: bool,
    pub(crate) retained: Option<RetainedRef>,
    pub(crate) retained_caller: Option<Id>,
    pub(crate) sources: Vec<Arc<View>>,
    pub(crate) g6_clearable: bool,
    pub(crate) previous: Option<Box<(Arc<Work>, Completed)>>,
}

fn diagnostic_kind(input: &Work) -> Option<(&'static str, &'static str)> {
    match input {
        Work::Open { .. } => Some(("project", "open")),
        Work::CreateProject { .. } => Some(("project", "create")),
        Work::Close { .. } | Work::RetireProject { .. } => Some(("project", "close")),
        Work::ProjectCopy { .. } => Some(("project_backup", "copy")),
        Work::BackupCreate { .. } => Some(("project_backup", "create")),
        Work::BackupList { .. } | Work::BackupInspect { .. } => Some(("project_backup", "inspect")),
        Work::BackupDelete { .. } => Some(("project_backup", "delete")),
        Work::BackupDeletedList { .. } => Some(("project_backup", "inspect")),
        Work::BackupDeletedRestore { .. } => Some(("project_backup", "restore")),
        Work::BackupDeletedPurge { .. } => Some(("project_backup", "delete")),
        Work::RestoreNew { .. } => Some(("project_restore", "new")),
        Work::RestoreCurrent { .. } => Some(("project_restore", "current")),
        Work::Recover { .. }
        | Work::RecoveryRestore { .. }
        | Work::RecoveryRevalidate { .. }
        | Work::RecoveryDiscard { .. } => Some(("recovery", "operation")),
        Work::DiagnosticExport { .. } => Some(("diagnostics", "export")),
        Work::AssetInspect { .. } => Some(("asset_maintenance", "inspect")),
        Work::AssetTrashMove { .. } => Some(("asset_maintenance", "trash_move")),
        Work::AssetRename { .. } => Some(("asset_maintenance", "rename")),
        Work::AssetTrashRestore { .. } => Some(("asset_maintenance", "trash_restore")),
        Work::AssetTrashPurge { .. } => Some(("asset_maintenance", "trash_purge")),
        Work::TemplatePurge { .. } => Some(("asset_maintenance", "template_purge")),
        Work::TemplateRestore { .. } => Some(("asset_maintenance", "template_restore")),
        _ if input.contains_edit() => Some(("editor", "write")),
        _ => None,
    }
}

fn diagnostic_outcome(result: &Result<ResultDto, ErrorDto>) -> &'static str {
    match result {
        Err(_) | Ok(ResultDto::Rejected { .. }) => "rejected",
        Ok(ResultDto::Open { error: Some(_), .. })
        | Ok(ResultDto::Write { error: Some(_), .. }) => "failed",
        Ok(ResultDto::TemplateDraft { status }) if status.error.is_some() => "failed",
        Ok(ResultDto::DocumentWorkspace { value }) => match &**value {
            super::document_workspace::Response::Draft {
                problem, outcome, ..
            }
            | super::document_workspace::Response::Editing {
                problem, outcome, ..
            } => document_edit_diagnostic_outcome(problem.as_deref(), outcome.as_deref()),
            _ => "complete",
        },
        Ok(ResultDto::ProjectData {
            recovery_required: true,
            ..
        }) => "recovery_required",
        Ok(ResultDto::ProjectData {
            outcome:
                Some(
                    "quarantined_unverified"
                    | "restore_unverified"
                    | "restored_receipt_cleanup_required"
                    | "deleted_receipt_cleanup_required",
                ),
            ..
        }) => "recovery_required",
        Ok(ResultDto::ProjectData {
            outcome: Some("partially_deleted"),
            ..
        }) => "partial",
        Ok(ResultDto::AssetMaintenance {
            cleanup_required, ..
        }) if !cleanup_required.is_empty() => "recovery_required",
        Ok(ResultDto::AssetMaintenance { partial: true, .. }) => "partial",
        Ok(ResultDto::AssetMaintenance { inspection, .. }) if !inspection.complete => "incomplete",
        _ => "complete",
    }
}

fn document_edit_diagnostic_outcome(
    problem: Option<&str>,
    outcome: Option<&ResultDto>,
) -> &'static str {
    if problem.is_some() {
        return "failed";
    }
    match outcome {
        Some(ResultDto::Rejected { .. }) => "rejected",
        Some(ResultDto::Write { disk, error, .. }) => {
            document_write_diagnostic_outcome(*disk, error.as_ref())
        }
        _ => "complete",
    }
}

fn document_write_diagnostic_outcome(disk: DiskDto, error: Option<&ErrorDto>) -> &'static str {
    if disk == DiskDto::Uncertain {
        "recovery_required"
    } else if error.is_none() && matches!(disk, DiskDto::Committed | DiskDto::NoWrite) {
        "complete"
    } else {
        "failed"
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn rejected_template_draft_is_not_logged_as_a_completed_write() {
        let mut status = crate::commands::workspace::DraftStatus {
            comparison: None,
            remaining_input: false,
            owner: Id::new(),
            draft_id: "draft".into(),
            project_fingerprint: "fixture".into(),
            artifact: "template".into(),
            generation: "1".into(),
            saved_generation: None,
            base_revision: "0".into(),
            source_digest: None,
            snapshot: Id::new(),
            phase: "dirty",
            receipt: None,
            error: Some(ErrorDto::new(Code::SaveRejected)),
            problems: vec![],
            identities: Default::default(),
            outcome: None,
        };
        assert_eq!(
            diagnostic_outcome(&Ok(ResultDto::TemplateDraft {
                status: Box::new(status.clone()),
            })),
            "failed"
        );
        status.error = None;
        status.phase = "saved";
        assert_eq!(
            diagnostic_outcome(&Ok(ResultDto::TemplateDraft {
                status: Box::new(status),
            })),
            "complete"
        );
    }

    #[test]
    fn document_draft_failure_is_not_logged_as_a_completed_write() {
        assert_eq!(
            document_edit_diagnostic_outcome(Some("SaveRejected"), None),
            "failed"
        );
        assert_eq!(
            document_edit_diagnostic_outcome(
                None,
                Some(&ResultDto::Rejected {
                    error: Code::SaveRejected.into(),
                    input_retained: true,
                })
            ),
            "rejected"
        );
        assert_eq!(document_edit_diagnostic_outcome(None, None), "complete");
        assert_eq!(
            document_write_diagnostic_outcome(DiskDto::NoWrite, None),
            "complete"
        );
        assert_eq!(
            document_write_diagnostic_outcome(DiskDto::NotApplied, None),
            "failed"
        );
        assert_eq!(
            document_write_diagnostic_outcome(DiskDto::Uncertain, None),
            "recovery_required"
        );
    }
}
impl Completed {
    pub(crate) fn new<T: Send + 'static>(original: T, dto: Reply<ResultDto>) -> Self {
        Self {
            original: Box::new(original),
            dto,
            view: None,
            binding: None,
            removed_binding: None,
            acknowledged_session: None,
            retain_edit: false,
            retained: None,
            retained_caller: None,
            sources: Vec::new(),
            g6_clearable: false,
            previous: None,
        }
    }
    fn reject<T: Send + 'static>(original: T, error: ErrorDto) -> Self {
        Self::new(
            original,
            Ok(ResultDto::Rejected {
                error,
                input_retained: true,
            }),
        )
    }
}
fn timestamp() -> Reply<String> {
    time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(|_| Code::Unavailable.into())
}
fn creation_baseline(
    ctx: &mut Context,
    id: ArtifactSourceId,
    bytes: Result<Vec<u8>, artifact::ArtifactCodecError>,
    time: &str,
) -> std::io::Result<u64> {
    let bytes = bytes.map_err(std::io::Error::other)?;
    let expected = crate::data::edit_recovery::model::digest(&bytes);
    ctx.read(|ready| {
        let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
        crate::data::repository::versions::confirm_source(&repository, id, time, Some(&expected))
    })
    .map_err(std::io::Error::other)?
}
fn write(
    session: Id,
    artifact: Option<String>,
    d: ApplicationDiagnostic,
    changed: Option<bool>,
    warnings: Vec<WarningDto>,
) -> ResultDto {
    let disk = match d.disk {
        DiskState::NotAttempted => DiskDto::NotAttempted,
        DiskState::NoWrite => DiskDto::NoWrite,
        DiskState::NotApplied => DiskDto::NotApplied,
        DiskState::RolledBack => DiskDto::RolledBack,
        DiskState::Committed => DiskDto::Committed,
        DiskState::Uncertain => DiskDto::Uncertain,
    };
    let diagnostic = DiagnosticDto {
        stage: format!("{:?}", d.stage),
        category: d.category.map(|c| format!("{c:?}")),
        session_state: format!("{:?}", d.session_state),
        lock_category: d
            .permit
            .as_ref()
            .and_then(|p| p.lock_category)
            .map(|c| format!("{c:?}")),
        next_action: d.next_action(),
        operation_id: None,
        observed_at_utc: None,
        failures: d
            .artifact_write
            .as_ref()
            .into_iter()
            .flat_map(|v| &v.failures)
            .chain(
                d.artifact_commit
                    .as_ref()
                    .into_iter()
                    .flat_map(|v| &v.failures),
            )
            .map(WriteFailureDto::from)
            .collect(),
    };
    ResultDto::Write {
        session,
        artifact,
        disk,
        changed,
        warnings,
        cleanup_failed: d.artifact_commit.as_ref().is_some_and(|v| v.cleanup_failed)
            || d.artifact_write
                .as_ref()
                .is_some_and(|v| v.cleanup.is_some()),
        recovery_required: d.recovery_required,
        error: d.category.map(|_| Code::SaveRejected.into()),
        diagnostic,
        deletion: None,
    }
}
fn deletion_summary(execution: &templates::TemplateMutationExecution) -> Option<DeletionSummary> {
    use crate::data::artifact::template_mutation::TemplateMutationErrorCategory;
    let error = execution.rejection()?;
    match error.domain_cause() {
        Some(templates::TemplateUseCaseError::Mutation(error))
            if error.category() == TemplateMutationErrorCategory::TemplateHasDocuments =>
        {
            Some(DeletionSummary::TemplateHasDocuments {
                count: error.reference_count()?,
                truncated: error.reference_count_truncated(),
            })
        }
        Some(
            templates::TemplateUseCaseError::SourceMismatch
            | templates::TemplateUseCaseError::RevisionMismatch,
        ) => Some(DeletionSummary::SourceChanged),
        _ if error.repository_cause().is_some() => Some(DeletionSummary::ReferenceCheckFailed),
        _ => None,
    }
}
fn begin(
    ctx: &mut Context,
    job: &Job,
    targets: Vec<crate::data::project_relative_path::ProjectRelativePath>,
) -> Reply<(
    Binding,
    crate::data::application::worker::SessionRegistration,
)> {
    let registration = ctx
        .begin_session(job.provider.clone(), targets)
        .map_err(|_| Code::SessionRejected)?;
    let binding = Binding {
        id: job.allocated,
        registration: registration.registration.clone(),
        key: registration.key.clone(),
        views: job.views.iter().map(|(id, _)| *id).collect(),
        draft_input: None,
        recovery: Arc::new(recovery::Observation::default()),
    };
    // A failed acquisition has no session identity. Its absence is not a
    // recovery Store failure and must not manufacture a second diagnosis.
    if registration.key.is_some() {
        recovery::connect(&mut ctx.session_control(), job, &binding);
    }
    Ok((binding, registration))
}
pub(crate) fn run(ctx: &mut Context, job: Job) -> Completed {
    let _progress = crate::data::repository::progress::scope(job.progress.clone());
    let mut result = match execute(ctx, &job) {
        Ok(result) => result,
        Err(error) => Completed::reject(job.input.clone(), error),
    };
    if matches!(&*job.input, Work::TemplateDraft { .. }) && result.binding.is_none() {
        if let Some(binding) = &job.binding {
            let mut updated = binding.clone();
            updated.key = ctx.session_key(&binding.registration);
            result.binding = Some(updated);
        }
    }
    result.retain_edit = job.input.contains_edit()
        && !matches!(&result.dto, Ok(ResultDto::TemplateDraft { .. }))
        && !matches!(
            &result.dto,
            Ok(ResultDto::Write {
                disk: DiskDto::Committed | DiskDto::NoWrite,
                ..
            })
        );
    if let Err(error) = &result.dto {
        result.dto = Ok(ResultDto::Rejected {
            error: error.clone(),
            input_retained: true,
        });
    }
    if result.retain_edit {
        result.sources = job.views.iter().map(|(_, v)| v.clone()).collect();
        result.g6_clearable = g6_intent(&job.input).is_some()
            && matches!(
                &result.dto,
                Ok(ResultDto::Rejected { .. })
                    | Ok(ResultDto::Write {
                        disk: DiskDto::NotAttempted | DiskDto::NotApplied | DiskDto::RolledBack,
                        cleanup_failed: false,
                        recovery_required: false,
                        ..
                    })
            );
    }
    if let Some((feature, stage)) = diagnostic_kind(&job.input) {
        crate::diagnostic_log::record(
            feature,
            stage,
            "operation",
            diagnostic_outcome(&result.dto),
            Some(String::from(job.operation)),
            None,
        );
    }
    result
}
pub(crate) fn g6_intent(input: &Work) -> Option<G6Intent> {
    match input {
        Work::CreateTemplate {
            name, presentation, ..
        } => Some(G6Intent::Create {
            name: name.clone(),
            presentation: presentation.clone(),
        }),
        Work::DuplicateTemplate { .. } => Some(G6Intent::Duplicate {}),
        Work::UpdateTemplate { revision, edit, .. } => Some(G6Intent::Update {
            revision: revision.clone(),
            edit: edit.clone(),
        }),
        Work::TombstoneTemplate { revision, .. } => Some(G6Intent::Tombstone {
            revision: revision.clone(),
        }),
        _ => None,
    }
}
pub(crate) fn policy_error(reason: String) -> ErrorDto {
    let mut error = ErrorDto::new(Code::CollaborationPolicyRejected);
    error.next_action = "팀에서 요구하는 앱 버전과 연결 상태를 확인하세요. 보관 입력은 유지되며 홈에서 앱을 업데이트할 수 있습니다";
    error.diagnostic = Some(ErrorDiagnosticDto {
        stage: "compatibility".into(),
        category: reason,
        outcome: None,
        io_kind: None,
        os_code: None,
        cleanup_outcome: None,
        cleanup_io_kind: None,
        cleanup_os_code: None,
    });
    error
}
fn policy_deposit_input(work: &Work) -> Option<Work> {
    use super::document_workspace::Request;
    match work {
        Work::DocumentWorkspace { project, request } => {
            let request = match request {
                Request::EditDraft {
                    owner,
                    generation,
                    body,
                    ..
                } => Request::EditDeposit {
                    owner: *owner,
                    generation: generation.clone(),
                    body: body.clone(),
                },
                Request::Draft {
                    owner,
                    generation,
                    body,
                    ..
                } => Request::Deposit {
                    owner: *owner,
                    generation: generation.clone(),
                    body: body.clone(),
                },
                _ => return None,
            };
            Some(Work::DocumentWorkspace {
                project: *project,
                request,
            })
        }
        Work::TemplateDraft {
            project,
            session,
            generation,
            body,
            ..
        } => Some(Work::TemplateDraft {
            project: *project,
            session: *session,
            generation: generation.clone(),
            body: body.clone(),
            action: super::workspace::DraftAction::Deposit,
        }),
        _ => None,
    }
}
fn execute(ctx: &mut Context, job: &Job) -> Reply<Completed> {
    if job.collaborative && crate::svn_guard::policy_required(&job.input) {
        if let Err(reason) = job.provider.authorize_project_operation() {
            let mut error = policy_error(reason);
            // Freeze the submitted generation through the existing external
            // recovery sink; never turn rejection into a canonical write.
            if let Some(input) = policy_deposit_input(&job.input) {
                let mut deposit_job = job.clone();
                deposit_job.input = Arc::new(input);
                let mut result = match &*deposit_job.input {
                    Work::DocumentWorkspace { project, request } => {
                        document_workspace::execute(ctx, &deposit_job, *project, request)
                    }
                    _ => workspace::control(&mut ctx.session_control(), &deposit_job),
                };
                let deposited = result.as_ref().is_ok_and(|completed| match &completed.dto {
                    Ok(ResultDto::TemplateDraft { status }) => {
                        status.receipt.is_some() && status.error.is_none()
                    }
                    Ok(ResultDto::DocumentWorkspace { value }) => matches!(
                        &**value,
                        super::document_workspace::Response::Draft {
                            deposited: true,
                            ..
                        } | super::document_workspace::Response::Editing {
                            deposited: true,
                            ..
                        }
                    ),
                    _ => false,
                });
                error.next_action = if deposited {
                    "입력을 복구 센터에 보관했습니다. 홈으로 돌아가 앱을 업데이트한 뒤 보관 입력을 다시 여세요"
                } else {
                    "입력을 외부에 보관하지 못했습니다. 현재 창과 입력을 유지하고 보관을 재시도하세요"
                };
                // The current native draft owner already retains this exact
                // generation. Return its real receipt/status instead of
                // manufacturing a second unhandled retained-input owner.
                if let Ok(completed) = &mut result {
                    match &mut completed.dto {
                        Ok(ResultDto::TemplateDraft { status }) => {
                            status.error = Some(error.clone());
                            return result;
                        }
                        Ok(ResultDto::DocumentWorkspace { value }) => match &mut **value {
                            super::document_workspace::Response::Draft { outcome, .. }
                            | super::document_workspace::Response::Editing { outcome, .. } => {
                                *outcome = Some(Box::new(ResultDto::Rejected {
                                    error: error.clone(),
                                    input_retained: true,
                                }));
                                return result;
                            }
                            _ => (),
                        },
                        _ => (),
                    }
                }
            }
            if matches!(
                &*job.input,
                Work::SaveDocument { .. } | Work::SaveComposite { .. }
            ) {
                let mut bound = false;
                let preserved = (|| -> Reply<()> {
                    let mut session = ctx.session(job.key()?).map_err(|_| Code::SessionRejected)?;
                    let snapshot = session.snapshot();
                    let payload = PendingEdit::new(
                        job,
                        snapshot.project_fingerprint().ok_or(Code::WrongBinding)?,
                    )?;
                    session
                        .bind_draft(payload)
                        .map_err(|_| Code::OwnersRemain)?;
                    bound = true;
                    drop(session);
                    ctx.session_control()
                        .deposit_whole_draft(
                            &job.binding.as_ref().ok_or(Code::WrongBinding)?.registration,
                        )
                        .map_err(|_| Code::SessionRejected)?
                        .map_err(|e| handoff_error(&e))?
                        .ok_or(Code::NoReceipt)?;
                    Ok(())
                })();
                error.next_action = if preserved.is_ok() {
                    "입력을 복구 센터에 보관했습니다. 홈으로 돌아가 앱을 업데이트한 뒤 보관 입력을 다시 여세요"
                } else {
                    "입력을 외부에 보관하지 못했습니다. 현재 창과 입력을 유지하고 보관을 재시도하세요"
                };
                if bound {
                    let mut result = Completed::reject(job.input.clone(), error);
                    let mut binding = job.binding.clone().ok_or(Code::WrongBinding)?;
                    binding.draft_input = Some(job.input.clone());
                    result.acknowledged_session = preserved.is_ok().then_some(binding.id);
                    result.binding = Some(binding);
                    return Ok(result);
                }
            }
            return Err(error);
        }
    }
    // Startup/list refresh is the automatic transition boundary, before ordinary
    // items become editable. Active owners keep their current input untouched.
    if matches!(
        &*job.input,
        Work::ListTemplates { .. }
            | Work::CreateTemplate { .. }
            | Work::ReadTemplate { .. }
            | Work::ReadDocument { .. }
            | Work::BeginTemplateDraft { .. }
            | Work::DocumentWorkspace {
                request: crate::commands::document_workspace::Request::List { .. }
                    | crate::commands::document_workspace::Request::Read { .. }
                    | crate::commands::document_workspace::Request::EditBegin { .. }
                    | crate::commands::document_workspace::Request::VersionRestore { .. }
                    | crate::commands::document_workspace::Request::VersionsList { .. }
                    | crate::commands::document_workspace::Request::VersionPreview { .. }
                    | crate::commands::document_workspace::Request::Begin { .. },
                ..
            }
    ) {
        if let Some(failure) = transition::run(ctx, job)? {
            return Ok(failure);
        }
        if let Some(failure) = transition::policy(ctx, job)? {
            return Ok(failure);
        }
    }
    match &*job.input {
        Work::DocumentWorkspace { project, request } => {
            document_workspace::execute(ctx, job, *project, request)
        }
        Work::RecoveryRestore { .. }
        | Work::BeginTemplateDraft { .. }
        | Work::TemplateDraft { .. }
        | Work::TemplateDraftContent { .. }
        | Work::RefreshTemplateDraft { .. }
        | Work::ResumeTemplateDraft { .. }
        | Work::ReleaseTemplateDraft { .. } => workspace::execute(ctx, job),
        Work::ProjectCopy { .. }
        | Work::BackupCreate { .. }
        | Work::BackupList { .. }
        | Work::BackupDelete { .. }
        | Work::BackupDeletedList { .. }
        | Work::BackupDeletedRestore { .. }
        | Work::BackupDeletedPurge { .. }
        | Work::RestoreCurrent { .. } => project_backup::execute(ctx, job),
        Work::AssetInspect { .. }
        | Work::AssetTrashMove { .. }
        | Work::AssetRename { .. }
        | Work::AssetTrashRestore { .. }
        | Work::AssetTrashPurge { .. }
        | Work::TemplatePurge { .. }
        | Work::DiagnosticExport { .. } => asset_maintenance::execute(ctx, job),
        Work::TemplateRestore { template, .. } => {
            let id = convert::id(template)?;
            let source = match ctx.read(|ready| {
                let loaded = ArtifactRepository::new(ready)?.load_template(id)?;
                Ok::<_, crate::data::repository::RepositoryError>(templates::TemplateSource {
                    id,
                    token: loaded.source().clone(),
                    expected_revision: loaded.artifact().revision(),
                })
            }) {
                Ok(Ok(source)) => source,
                Ok(Err(error)) => {
                    return Ok(Completed::reject(error, Code::RepositoryRejected.into()))
                }
                Err(error) => return Ok(Completed::reject(error, Code::RuntimeRejected.into())),
            };
            let target = ArtifactSourceId::Template(id)
                .path()
                .map_err(|_| Code::InvalidInput)?;
            let (binding, registration) = begin(ctx, job, vec![target])?;
            let Some(key) = binding.key.as_ref() else {
                let mut result =
                    Completed::reject((source, registration), Code::SessionRejected.into());
                result.binding = Some(binding);
                return Ok(result);
            };
            let input = templates::RestoreTemplateInput {
                source,
                timestamp_utc: timestamp()?,
            };
            let original = ctx
                .session(key)
                .map_err(|_| Code::SessionRejected)?
                .restore_template(&input);
            let dto = original
                .as_ref()
                .map(|result| {
                    write(
                        binding.id,
                        Some(input.source.id.to_string()),
                        result.execution.diagnostic(),
                        Some(result.candidate().is_some()),
                        vec![],
                    )
                })
                .map_err(|_| Code::SaveRejected.into());
            let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
            let mut completed = Completed::new((input, original, cleanup), dto);
            if !released {
                completed.binding = Some(binding);
            }
            Ok(completed)
        }
        Work::RetainedHandoff { .. }
        | Work::AbandonRetained { .. }
        | Work::ResumeRetained { .. }
        | Work::RetireProject { .. } => Err(Code::InvalidInput.into()),
        Work::BackupInspect { .. } | Work::RestoreNew { .. } => Err(Code::InvalidInput.into()),
        Work::ListTemplates { .. } => {
            let result = match ctx.read(|ready| ArtifactRepository::new(ready)?.scan_templates()) {
                Ok(r) => r,
                Err(e) => return Ok(Completed::reject(e, Code::RuntimeRejected.into())),
            };
            match result {
                Ok(scan) => {
                    let ids = ctx
                        .read(|ready| {
                            let repository =
                                ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                            crate::data::repository::versions::targets(&repository)
                        })
                        .map_err(|_| Code::RuntimeRejected)?
                        .map_err(|_| Code::RepositoryRejected)?;
                    if ids.iter().any(|id| {
                        matches!(id, ArtifactSourceId::Template(_))
                            && !scan
                                .records()
                                .iter()
                                .any(|record| record.source().id() == *id)
                    }) {
                        let display = ctx
                            .read(|ready| {
                                let repository = ArtifactRepository::new(ready)
                                    .map_err(std::io::Error::other)?;
                                repository.display_templates()
                            })
                            .map_err(|_| Code::RuntimeRejected)?
                            .map_err(|_| Code::RepositoryRejected)?;
                        return Ok(Completed::new(
                            scan,
                            Ok(ResultDto::Templates {
                                templates: display
                                    .iter()
                                    .map(|template| TemplateSummary {
                                        id: template.id.clone(),
                                        name: template.name.clone(),
                                        revision: template.revision.clone(),
                                        lifecycle: template.lifecycle.clone(),
                                        glossary_excluded: template.glossary_excluded,
                                    })
                                    .collect(),
                            }),
                        ));
                    }
                    let rows = scan
                        .records()
                        .iter()
                        .map(|r| TemplateSummary {
                            name: r.name().to_owned(),
                            id: match r.source().id() {
                                ArtifactSourceId::Template(id) => id.to_string(),
                                ArtifactSourceId::Document(id) => id.to_string(),
                                ArtifactSourceId::DocumentLayout => "document-layout".into(),
                            },
                            revision: match r.source().revision() {
                                crate::data::repository::SourceRevision::Layout(r) => r.to_string(),
                                crate::data::repository::SourceRevision::Template(r)
                                | crate::data::repository::SourceRevision::DocumentTemplate(r) => {
                                    r.get().to_string()
                                }
                            },
                            lifecycle: format!("{:?}", r.lifecycle()),
                            glossary_excluded: r.glossary_excluded(),
                        })
                        .collect();
                    Ok(Completed::new(
                        scan,
                        Ok(ResultDto::Templates { templates: rows }),
                    ))
                }
                Err(e) => {
                    let fallback = ctx.read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        repository.display_templates()
                    });
                    match fallback {
                        Ok(Ok(rows)) if !rows.is_empty() => Ok(Completed::new(
                            e,
                            Ok(ResultDto::Templates {
                                templates: rows
                                    .iter()
                                    .map(|template| TemplateSummary {
                                        id: template.id.clone(),
                                        name: template.name.clone(),
                                        revision: template.revision.clone(),
                                        lifecycle: template.lifecycle.clone(),
                                        glossary_excluded: template.glossary_excluded,
                                    })
                                    .collect(),
                            }),
                        )),
                        original => Ok(Completed::reject(
                            (e, original),
                            Code::RepositoryRejected.into(),
                        )),
                    }
                }
            }
        }
        Work::ReadTemplate { template, .. } => {
            let id = convert::id(template)?;
            let result = match ctx.read(|ready| ArtifactRepository::new(ready)?.load_template(id)) {
                Ok(r) => r,
                Err(e) => return Ok(Completed::reject(e, Code::RuntimeRejected.into())),
            };
            match result {
                Ok(loaded) => {
                    let dto = projection::template(loaded.artifact()).map(|content| {
                        ResultDto::Template {
                            view: job.allocated,
                            content,
                        }
                    });
                    let view = Arc::new(View::Template(loaded));
                    let mut result = Completed::new(view.clone(), dto);
                    result.view = Some((job.allocated, view));
                    Ok(result)
                }
                Err(e) => Ok(Completed::reject(e, Code::RepositoryRejected.into())),
            }
        }
        Work::ReadDocument { document, .. } => {
            let id = convert::id(document)?;
            let result = match ctx.read(|ready| ArtifactRepository::new(ready)?.load_document(id)) {
                Ok(r) => r,
                Err(e) => return Ok(Completed::reject(e, Code::RuntimeRejected.into())),
            };
            match result {
                Ok(loaded) => {
                    let dto = projection::document(loaded.artifact()).map(|content| {
                        ResultDto::Document {
                            view: job.allocated,
                            content,
                        }
                    });
                    let view = Arc::new(View::Document(loaded));
                    let mut result = Completed::new(view.clone(), dto);
                    result.view = Some((job.allocated, view));
                    Ok(result)
                }
                Err(e) => Ok(Completed::reject(e, Code::RepositoryRejected.into())),
            }
        }
        Work::BeginSession { purpose, .. } => {
            let templates = job
                .views
                .iter()
                .filter(|(_, v)| matches!(&**v, View::Template(_)))
                .count();
            let documents = job.views.len() - templates;
            if (*purpose == EditPurpose::Template && (templates != 1 || documents != 0))
                || (*purpose != EditPurpose::Template && (templates != 1 || documents != 1))
            {
                return Err(Code::WrongBinding.into());
            }
            let targets = job
                .views
                .iter()
                .filter(|(_, v)| {
                    *purpose != EditPurpose::Document || matches!(&**v, View::Document(_))
                })
                .map(|(_, v)| v.target().path().map_err(|_| Code::InvalidInput.into()))
                .collect::<Reply<Vec<_>>>()?;
            let (binding, original) = begin(ctx, job, targets)?;
            let state = ctx
                .session_snapshot(&binding.registration)
                .map_err(|_| Code::SessionRejected)?;
            let dto = ResultDto::Session {
                session: binding.id,
                state: format!("{:?}", state.state()),
                error: original
                    .original
                    .as_ref()
                    .err()
                    .map(|_| Code::SessionRejected.into()),
            };
            let mut result = Completed::new(original, Ok(dto));
            result.binding = Some(binding);
            Ok(result)
        }
        Work::CreateTemplate { .. } | Work::DuplicateTemplate { .. } => {
            let timestamp_utc = timestamp()?;
            let prepared = match &*job.input {
                Work::CreateTemplate {
                    name, presentation, ..
                } => ctx.prepare_template(&templates::CreateTemplateInput {
                    name: name.clone(),
                    presentation_token: presentation.clone(),
                    timestamp_utc,
                }),
                Work::DuplicateTemplate { view, .. } => {
                    ctx.prepare_duplicate(&templates::DuplicateTemplateInput {
                        source: job.view(*view)?.template()?,
                        timestamp_utc,
                    })
                }
                _ => return Err(Code::InvalidInput.into()),
            };
            let mut prepared = match prepared {
                Ok(p) => p,
                Err(e) => return Ok(Completed::reject(e, Code::PreparationRejected.into())),
            };
            let (binding, registration) = begin(ctx, job, prepared.session_targets().to_vec())?;
            let Some(key) = binding.key.as_ref() else {
                let mut result =
                    Completed::reject((prepared, registration), Code::SessionRejected.into());
                result.binding = Some(binding);
                return Ok(result);
            };
            let mut session = ctx.session(key).map_err(|_| Code::SessionRejected)?;
            let execution = if matches!(&*job.input, Work::DuplicateTemplate { .. }) {
                session.duplicate_template(&mut prepared)
            } else {
                session.create_template(&mut prepared)
            };
            drop(session);
            let baseline = if execution.diagnostic().disk == DiskState::Committed {
                Some(creation_baseline(
                    ctx,
                    ArtifactSourceId::Template(prepared.template_id()),
                    artifact::encode_template(prepared.candidate()),
                    &timestamp()?,
                ))
            } else {
                None
            };
            let warnings = if baseline.as_ref().is_some_and(|result| result.is_err()) {
                vec![WarningDto {
                    category: "content_version_unavailable".into(),
                    field: None,
                }]
            } else {
                vec![]
            };
            let dto = write(
                binding.id,
                Some(prepared.template_id().to_string()),
                execution.diagnostic(),
                Some(true),
                warnings,
            );
            let mut result = Completed::new((prepared, registration, execution, baseline), Ok(dto));
            result.binding = Some(binding);
            Ok(result)
        }
        Work::CreateDocument { view, name, .. } => {
            let prepared = ctx.prepare_document(&documents::CreateDocumentInput {
                source: job.view(*view)?.template()?,
                name: name.clone(),
                timestamp_utc: timestamp()?,
            });
            let mut prepared = match prepared {
                Ok(p) => p,
                Err(e) => return Ok(Completed::reject(e, Code::PreparationRejected.into())),
            };
            let (binding, registration) = begin(ctx, job, prepared.session_targets().to_vec())?;
            let Some(key) = binding.key.as_ref() else {
                let mut result =
                    Completed::reject((prepared, registration), Code::SessionRejected.into());
                result.binding = Some(binding);
                return Ok(result);
            };
            let execution = ctx
                .session(key)
                .map_err(|_| Code::SessionRejected)?
                .create_document(&mut prepared);
            let baseline = if execution.diagnostic().disk == DiskState::Committed {
                Some(creation_baseline(
                    ctx,
                    ArtifactSourceId::Document(prepared.document_id()),
                    artifact::encode_document(prepared.candidate()),
                    &timestamp()?,
                ))
            } else {
                None
            };
            let warnings = if baseline.as_ref().is_some_and(|result| result.is_err()) {
                vec![WarningDto {
                    category: "content_version_unavailable".into(),
                    field: None,
                }]
            } else {
                vec![]
            };
            let dto = write(
                binding.id,
                Some(prepared.document_id().to_string()),
                execution.diagnostic(),
                Some(true),
                warnings,
            );
            let mut result = Completed::new((prepared, registration, execution, baseline), Ok(dto));
            result.binding = Some(binding);
            Ok(result)
        }
        Work::UpdateTemplate {
            view,
            revision,
            edit,
            session,
            ..
        } => {
            let input = templates::UpdateTemplateInput {
                source: job.source(*view, revision)?,
                timestamp_utc: timestamp()?,
                intent: convert::intent(edit)?,
            };
            let original = ctx
                .session(job.key()?)
                .map_err(|_| Code::SessionRejected)?
                .update_template(&input);
            let dto = original
                .as_ref()
                .map(|r| {
                    write(
                        *session,
                        Some(input.source.id.to_string()),
                        r.execution.diagnostic(),
                        Some(r.candidate().is_some()),
                        vec![],
                    )
                })
                .map_err(|_| Code::SaveRejected.into());
            Ok(Completed::new((input, original), dto))
        }
        Work::TombstoneTemplate {
            view,
            revision,
            session,
            ..
        } => {
            let input = templates::TombstoneTemplateInput {
                source: job.source(*view, revision)?,
                timestamp_utc: timestamp()?,
            };
            let original = ctx
                .session(job.key()?)
                .map_err(|_| Code::SessionRejected)?
                .tombstone_template(&input);
            let dto = original
                .as_ref()
                .map(|r| {
                    let mut result = write(
                        *session,
                        Some(input.source.id.to_string()),
                        r.execution.diagnostic(),
                        Some(r.candidate().is_some()),
                        vec![],
                    );
                    if let ResultDto::Write { deletion, .. } = &mut result {
                        *deletion = deletion_summary(r);
                    }
                    result
                })
                .map_err(|_| Code::SaveRejected.into());
            Ok(Completed::new((input, original), dto))
        }
        Work::MaterializeDocument {
            document,
            template,
            revision,
            session,
            ..
        } => {
            let input = persistence::MaterializeDocumentInput {
                document: job.view(*document)?.document()?,
                template: job.source(*template, revision)?,
                timestamp_utc: timestamp()?,
            };
            let original = ctx
                .session(job.key()?)
                .map_err(|_| Code::SessionRejected)?
                .materialize_document(&input);
            let dto = original
                .as_ref()
                .map(|r| {
                    write(
                        *session,
                        Some(input.document.id.to_string()),
                        r.execution.diagnostic(),
                        r.outcome().map(|o| {
                            o.kind() == artifact::DocumentMaterializationOutcomeKind::Changed
                        }),
                        r.outcome()
                            .map_or_else(Vec::new, |o| projection::warnings(o.warnings())),
                    )
                })
                .map_err(|_| Code::SaveRejected.into());
            Ok(Completed::new((input, original), dto))
        }
        Work::SaveDocument {
            document,
            template,
            revision,
            edits,
            ..
        } => {
            let input = persistence::SaveDocumentInput {
                document: job.view(*document)?.document()?,
                template: job.source(*template, revision)?,
                edits: convert::edits(edits)?,
                timestamp_utc: timestamp()?,
            };
            let mut session = ctx.session(job.key()?).map_err(|_| Code::SessionRejected)?;
            let snapshot = session.snapshot();
            let payload = PendingEdit::new(
                job,
                snapshot.project_fingerprint().ok_or(Code::WrongBinding)?,
            )?;
            let attempt = payload.recovery.attempt.clone();
            if let Err(rejected) = session.bind_draft(payload) {
                return Ok(Completed::reject(rejected, Code::OwnersRemain.into()));
            }
            let original = session.save_dirty_document(&input);
            let diagnostic = original
                .as_ref()
                .ok()
                .and_then(|r| r.original.as_ref().ok())
                .map(|r| r.execution.diagnostic());
            if attempt
                .set(recovery::attempt(job.operation, diagnostic))
                .is_err()
            {
                return Err(Code::RecoveryRejected.into());
            }
            // 실제 G10 거부 원인을 분류하며 성공한 운영 sink를 가정하지 않는다.
            let dto = match &original {
                Ok(saved) => match &saved.original {
                    Ok(r) => Ok(dirty(
                        write(
                            job.binding.as_ref().ok_or(Code::WrongBinding)?.id,
                            Some(input.document.id.to_string()),
                            r.execution.diagnostic(),
                            r.outcome()
                                .map(|o| o.kind() == artifact::DocumentSaveOutcomeKind::Changed),
                            r.outcome()
                                .map_or_else(Vec::new, |o| projection::warnings(o.warnings())),
                        ),
                        &saved.custody,
                    )),
                    Err(_) => Ok(dirty(
                        ResultDto::Rejected {
                            error: Code::SaveRejected.into(),
                            input_retained: true,
                        },
                        &saved.custody,
                    )),
                },
                Err(error) => Err(handoff_error(error)),
            };
            let mut result = Completed::new((input, original), dto);
            let mut binding = job.binding.clone().ok_or(Code::WrongBinding)?;
            binding.draft_input = Some(job.input.clone());
            result.binding = Some(binding);
            Ok(result)
        }
        Work::SaveComposite {
            document,
            template,
            revision,
            edit,
            edits,
            ..
        } => {
            let input = composite::CompositeSaveInput {
                document: job.view(*document)?.document()?,
                template: job.source(*template, revision)?,
                intent: convert::intent(edit)?,
                edits: convert::edits(edits)?,
                timestamp_utc: timestamp()?,
            };
            let mut session = ctx.session(job.key()?).map_err(|_| Code::SessionRejected)?;
            let snapshot = session.snapshot();
            let payload = PendingEdit::new(
                job,
                snapshot.project_fingerprint().ok_or(Code::WrongBinding)?,
            )?;
            let attempt = payload.recovery.attempt.clone();
            if let Err(rejected) = session.bind_draft(payload) {
                return Ok(Completed::reject(rejected, Code::OwnersRemain.into()));
            }
            let original = session.save_dirty_composite(&input);
            let diagnostic = original
                .as_ref()
                .ok()
                .map(|r| r.original.execution.diagnostic());
            if attempt
                .set(recovery::attempt(job.operation, diagnostic))
                .is_err()
            {
                return Err(Code::RecoveryRejected.into());
            }
            let dto = match &original {
                Ok(saved) => {
                    let result = &saved.original;
                    let outcome = write(
                        job.binding.as_ref().ok_or(Code::WrongBinding)?.id,
                        None,
                        result.execution.diagnostic(),
                        None,
                        result
                            .document_outcome()
                            .map_or_else(Vec::new, |o| projection::warnings(o.warnings())),
                    );
                    Ok(dirty(
                        ResultDto::Composite {
                            outcome: Box::new(outcome),
                            template_changed: result.template_outcome().map(|o| {
                                matches!(
                                    o,
                                    artifact::template_mutation::TemplateMutationOutcome::Changed(
                                        _
                                    )
                                )
                            }),
                            document_changed: result
                                .document_outcome()
                                .map(|o| o.kind() == artifact::DocumentSaveOutcomeKind::Changed),
                        },
                        &saved.custody,
                    ))
                }
                Err(crate::data::application::worker::CompositeDispatchError::Handoff(e)) => {
                    Err(handoff_error(e))
                }
                Err(_) => Err(Code::SaveRejected.into()),
            };
            let mut result = Completed::new((input, original), dto);
            let mut binding = job.binding.clone().ok_or(Code::WrongBinding)?;
            binding.draft_input = Some(job.input.clone());
            result.binding = Some(binding);
            Ok(result)
        }
        Work::SessionControl { .. } => Ok(control(&mut ctx.session_control(), job)),
        Work::Recover { .. } => {
            let original = ctx.recover();
            let dto = original
                .as_ref()
                .map(|_| ResultDto::Control { error: None })
                .map_err(|_| Code::RecoveryRejected.into());
            Ok(Completed::new(original, dto))
        }
        _ => Err(Code::InvalidInput.into()),
    }
}
pub(crate) fn cleanup(ctx: &mut CleanupContext<'_, PendingEdit, Receipt>, job: Job) -> Completed {
    let mut result = control(ctx, &job);
    if let Err(error) = &result.dto {
        result.dto = Ok(ResultDto::Rejected {
            error: error.clone(),
            input_retained: true,
        });
    }
    result
}
fn control(ctx: &mut CleanupContext<'_, PendingEdit, Receipt>, job: &Job) -> Completed {
    if matches!(
        &*job.input,
        Work::TemplateDraft { .. } | Work::ReleaseTemplateDraft { .. }
    ) {
        return workspace::control(ctx, job)
            .unwrap_or_else(|e| Completed::reject(job.input.clone(), e));
    }
    if matches!(&*job.input, Work::Recover { .. }) {
        let original = ctx.recover_runtime();
        let dto = original
            .as_ref()
            .map(|_| ResultDto::Control { error: None })
            .map_err(|_| Code::RecoveryRejected.into());
        return Completed::new(original, dto);
    }
    let Some(binding) = &job.binding else {
        return Completed::reject(job.input.clone(), Code::WrongBinding.into());
    };
    let Work::SessionControl { control, .. } = &*job.input else {
        return Completed::reject(job.input.clone(), Code::InvalidInput.into());
    };
    macro_rules! result {
        ($original:expr,$code:expr) => {{
            let original = $original;
            let error = match &original {
                Ok(Ok(_)) => None,
                _ => Some(ErrorDto::new($code)),
            };
            Completed::new(original, Ok(ResultDto::Control { error }))
        }};
    }
    match control {
        SessionControl::End => {
            // G12 R1과 같이 active 원본의 처리보다 end가 앞서지 않게 한다.
            match ctx.observe_session(&binding.registration) {
                Ok((_, true)) => {
                    return Completed::reject(job.input.clone(), Code::OwnersRemain.into())
                }
                Err(e) => return Completed::reject(e, Code::SessionRejected.into()),
                _ => {}
            }
            let original = ctx.end_session_observed(&binding.registration);
            let removal = if original.as_ref().is_ok_and(|r| r.original.is_ok()) {
                Some(ctx.remove_session(&binding.registration))
            } else {
                None
            };
            let removed = matches!(&removal, Some(Ok(())));
            let error = if !original.as_ref().is_ok_and(|r| r.original.is_ok()) {
                Some(Code::ReleaseRejected.into())
            } else if !removed {
                Some(Code::OwnersRemain.into())
            } else {
                None
            };
            let mut result = Completed::new((original, removal), Ok(ResultDto::Control { error }));
            result.removed_binding = removed.then_some(binding.id);
            result
        }
        SessionControl::RetryRelease => {
            let original = ctx.retry_session_release(&binding.registration);
            let error = original
                .as_ref()
                .ok()
                .filter(|r| r.original.is_ok())
                .is_none()
                .then(|| Code::ReleaseRejected.into());
            Completed::new(original, Ok(ResultDto::Control { error }))
        }
        SessionControl::Preserve => result!(
            ctx.preserve_existing(&binding.registration),
            Code::NoRecoveryCondition
        ),
        SessionControl::Accept => {
            recovery::connect(ctx, job, binding);
            result!(ctx.accept(&binding.registration), Code::SinkUnavailable)
        }
        SessionControl::AcknowledgeRecovery => {
            let original = ctx.acknowledge(&binding.registration);
            let acknowledged = matches!(&original, Ok(Ok(_)));
            let mut result = Completed::new(
                original,
                Ok(ResultDto::Control {
                    error: (!acknowledged).then(|| Code::NoReceipt.into()),
                }),
            );
            result.acknowledged_session = acknowledged.then_some(binding.id);
            result
        }
        SessionControl::ReturnActive => {
            let original = ctx.return_active(&binding.registration);
            // snapshot이나 오류 문구가 아니라 실제 반환된 P가 있는 경우만 custody owner다.
            let has_payload = original.is_ok();
            let error = original.as_ref().err().map(|_| Code::NoDraft.into());
            if let Ok(payload) = &original {
                debug_assert!(payload.input.contains_edit());
            }
            let mut result = Completed::new(original, Ok(ResultDto::Control { error }));
            result.retain_edit = has_payload;
            result
        }
        SessionControl::Revalidate => {
            Completed::reject(job.input.clone(), Code::InvalidInput.into())
        }
    }
}
pub(crate) fn provider() -> Arc<dyn LockService> {
    Arc::new(NoLockService::new())
}
fn dirty(
    outcome: ResultDto,
    custody: &Result<
        crate::data::application::recovery_handoff::CustodyTransition,
        crate::data::application::recovery_handoff::HandoffError,
    >,
) -> ResultDto {
    ResultDto::Dirty {
        outcome: Box::new(outcome),
        custody: custody.as_ref().ok().map(|c| format!("{c:?}")),
        custody_error: custody.as_ref().err().map(handoff_error),
    }
}
fn handoff_error(error: &crate::data::application::recovery_handoff::HandoffError) -> ErrorDto {
    use crate::data::application::recovery_handoff::{HandoffCategory as C, HandoffError as E};
    match error {
        E::Rejected(C::Unavailable) => Code::SinkUnavailable,
        E::Rejected(C::MissingActiveDraft) => Code::NoDraft,
        E::Rejected(C::NoRecoveryCondition) => Code::NoRecoveryCondition,
        _ => Code::SessionRejected,
    }
    .into()
}
