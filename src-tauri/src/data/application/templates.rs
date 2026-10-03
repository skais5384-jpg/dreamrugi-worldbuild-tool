//! 닫힌 Template 업무 입력을 G5의 실제 세션/저장 경계에 연결한다.
use artifact::template_mutation::{
    self, FieldValueDraft, NewChoiceOptionDraft, NewFieldDraft, NewFieldInsertion,
    NewOptionInsertion, TemplateMutationCommand, TemplateMutationOutcome,
};
use artifact::{FieldId, OptionId};
use std::fmt;

pub(crate) mod whole;

use super::{
    diagnostics::{ApplicationError, BuildError, DiskState},
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    artifact::{self, TemplateArtifact, TemplateId, TemplateRevision},
    collaboration_lock::LockSessionId,
    edit_session::EditSessionService,
    project_relative_path::ProjectRelativePath,
    project_runtime::{ProjectRuntime, RuntimeError},
    repository::{
        ArtifactRepository, ArtifactSourceId, CanonicalWritePlan, RepositoryError, SourceToken,
    },
};

pub(crate) struct CreateTemplateInput {
    pub(crate) name: String,
    pub(crate) presentation_token: Option<String>,
    pub(crate) timestamp_utc: String,
}

/// source는 실제 읽기에서만 발급된다. ID/revision은 사용자 편집의 precondition이다.
pub(crate) struct TemplateSource {
    pub(crate) id: TemplateId,
    pub(crate) token: SourceToken,
    pub(crate) expected_revision: TemplateRevision,
}

pub(crate) struct DuplicateTemplateInput {
    pub(crate) source: TemplateSource,
    pub(crate) timestamp_utc: String,
}

/// authority를 소비하는 Tombstone은 이 enum에 없으며 별도 업무가 최종 scan을 수행한다.
pub(crate) enum TemplateEditIntent {
    SetName(String),
    SetPresentationToken(Option<String>),
    SetGlossaryExcluded(bool),
    CreateField {
        draft: NewFieldDraft,
        insertion: NewFieldInsertion,
    },
    SetFieldLabel {
        field: FieldId,
        label: String,
    },
    SetFieldRequired {
        field: FieldId,
        required: bool,
    },
    SetFieldPresentationToken {
        field: FieldId,
        token: Option<String>,
    },
    SetCurrentDefault {
        field: FieldId,
        value: FieldValueDraft,
    },
    KeepCurrentDefault {
        field: FieldId,
    },
    ReorderFields(Vec<FieldId>),
    ArchiveField(FieldId),
    AddOption {
        field: FieldId,
        draft: NewChoiceOptionDraft,
        insertion: NewOptionInsertion,
    },
    RenameOption {
        field: FieldId,
        option: OptionId,
        label: String,
    },
    ReorderOptions {
        field: FieldId,
        order: Vec<OptionId>,
    },
    ArchiveOption {
        field: FieldId,
        option: OptionId,
        repair: Option<FieldValueDraft>,
    },
}
impl TemplateEditIntent {
    /// Build a read-only recovery candidate against the preserved source. This
    /// performs normal artifact admission but does not acquire an owner or write.
    pub(crate) fn recovery_candidate(
        &self,
        source: &TemplateArtifact,
        timestamp: &str,
    ) -> Result<TemplateArtifact, template_mutation::TemplateMutationError> {
        Ok(
            match template_mutation::apply_template_mutation(
                source,
                source.revision(),
                timestamp,
                self.command(source)?,
            )? {
                TemplateMutationOutcome::Unchanged => source.clone(),
                TemplateMutationOutcome::Changed(candidate) => *candidate,
            },
        )
    }
    /// 복합 보관을 문서 편집으로 연결할 때 Template 의도가 남지 않았음을 순수 검증한다.
    pub(crate) fn is_unchanged(
        &self,
        source: &TemplateArtifact,
        timestamp: &str,
    ) -> Result<bool, template_mutation::TemplateMutationError> {
        Ok(matches!(
            template_mutation::apply_template_mutation(
                source,
                source.revision(),
                timestamp,
                self.command(source)?
            )?,
            TemplateMutationOutcome::Unchanged { .. }
        ))
    }
    pub(super) fn command(
        &self,
        source: &TemplateArtifact,
    ) -> Result<TemplateMutationCommand, template_mutation::TemplateMutationError> {
        Ok(match self {
            Self::SetName(name) => TemplateMutationCommand::set_template_name(name.clone()),
            Self::SetPresentationToken(token) => {
                TemplateMutationCommand::set_template_presentation_token(token.clone())
            }
            Self::SetGlossaryExcluded(excluded) => {
                TemplateMutationCommand::set_glossary_excluded(*excluded)
            }
            Self::CreateField { draft, insertion } => {
                TemplateMutationCommand::create_field(draft.clone(), *insertion)
            }
            Self::SetFieldLabel { field, label } => {
                TemplateMutationCommand::set_field_label(*field, label.clone())
            }
            Self::SetFieldRequired { field, required } => {
                TemplateMutationCommand::set_field_required(*field, *required)
            }
            Self::SetFieldPresentationToken { field, token } => {
                TemplateMutationCommand::set_field_presentation_token(*field, token.clone())
            }
            Self::SetCurrentDefault { field, value } => {
                TemplateMutationCommand::set_current_default(*field, value.clone())
            }
            Self::KeepCurrentDefault { field } => {
                // 최종 읽기에서 검증한 동일 snapshot의 draft만 발급하고 아래 domain 호출에서 소비한다.
                // 외부 draft를 재승인하지 않으며 Field/lifecycle/time 검증과 실제 Unchanged 판정을 유지한다.
                TemplateMutationCommand::set_current_default(
                    *field,
                    source.current_default_draft(*field)?,
                )
            }
            Self::ReorderFields(order) => TemplateMutationCommand::reorder_fields(order.clone()),
            Self::ArchiveField(field) => TemplateMutationCommand::archive_field(*field),
            Self::AddOption {
                field,
                draft,
                insertion,
            } => TemplateMutationCommand::add_option(*field, draft.clone(), *insertion),
            Self::RenameOption {
                field,
                option,
                label,
            } => TemplateMutationCommand::rename_option(*field, *option, label.clone()),
            Self::ReorderOptions { field, order } => {
                TemplateMutationCommand::reorder_options(*field, order.clone())
            }
            Self::ArchiveOption {
                field,
                option,
                repair,
            } => TemplateMutationCommand::archive_option(*field, *option, repair.clone()),
        })
    }
}

pub(crate) struct UpdateTemplateInput {
    pub(crate) source: TemplateSource,
    pub(crate) timestamp_utc: String,
    pub(crate) intent: TemplateEditIntent,
}
pub(crate) struct TombstoneTemplateInput {
    pub(crate) source: TemplateSource,
    pub(crate) timestamp_utc: String,
}
pub(crate) struct RestoreTemplateInput {
    pub(crate) source: TemplateSource,
    pub(crate) timestamp_utc: String,
}

/// 후보와 실제 저장 결과를 분리한다. 원 오류 객체는 G5 결과 안에 보존한다.
#[must_use = "候補ではなく実際の保存・復旧・セッション結果を確認してください"]
pub(crate) struct TemplateMutationExecution {
    pub(crate) execution: WriteExecution<(), TemplateUseCaseError>,
    candidate: Option<Box<TemplateArtifact>>,
}
impl TemplateMutationExecution {
    /// 원 거부만 빌려준다. DTO를 위해 재scan하거나 오류 문자열을 파싱하지 않는다.
    pub(crate) fn rejection(&self) -> Option<&BuildError<TemplateUseCaseError>> {
        match self.execution.body()? {
            super::write::BodyOutcome::Rejected(error) => Some(error),
            _ => None,
        }
    }
    pub(crate) fn candidate(&self) -> Option<&TemplateArtifact> {
        self.candidate.as_deref()
    }
}

pub(crate) struct TemplateWriteContext<'a> {
    pub(crate) project: &'a str,
    pub(crate) session: &'a LockSessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreationState {
    Uncommitted,
    Committed,
    RecoveryRequired,
}

/// 생성자는 backend 내부뿐이며 candidate/target을 변경하거나 독립 복제할 수 없다.
pub(crate) struct PreparedTemplateCreate {
    project: String,
    candidate: Box<TemplateArtifact>,
    targets: ExactWriteTargets,
    source: Option<TemplateSource>,
    state: CreationState,
}
impl PreparedTemplateCreate {
    pub(crate) fn template_id(&self) -> TemplateId {
        self.candidate.template_id()
    }
    pub(crate) fn session_targets(&self) -> &[ProjectRelativePath] {
        self.targets.session_targets()
    }
    pub(crate) fn candidate(&self) -> &TemplateArtifact {
        &self.candidate
    }
    pub(crate) fn state(&self) -> CreationState {
        self.state
    }
}

pub(crate) enum TemplateUseCaseError {
    SourceMismatch,
    RevisionMismatch,
    TargetOccupied,
    PreparationProjectMismatch,
    PreparationAlreadyCommitted,
    PreparationRequiresRecovery,
    Creation(artifact::TemplateCreationError),
    Mutation(artifact::template_mutation::TemplateMutationError),
}
pub(crate) enum TemplatePreparationError {
    Runtime(RuntimeError),
    Repository(RepositoryError),
    Target(ApplicationError),
    Domain(TemplateUseCaseError),
    Source(BuildError<TemplateUseCaseError>),
}

pub(crate) fn prepare_create_template(
    runtime: &mut ProjectRuntime,
    input: &CreateTemplateInput,
) -> Result<PreparedTemplateCreate, TemplatePreparationError> {
    let project = runtime.project_fingerprint().to_owned();
    let _ready = runtime.ready().map_err(TemplatePreparationError::Runtime)?;
    let candidate = artifact::create_template(
        input.name.clone(),
        input.presentation_token.clone(),
        input.timestamp_utc.clone(),
    )
    .map_err(|e| TemplatePreparationError::Domain(TemplateUseCaseError::Creation(e)))?;
    ticket(project, candidate, None)
}

fn ticket(
    project: String,
    candidate: TemplateArtifact,
    source: Option<TemplateSource>,
) -> Result<PreparedTemplateCreate, TemplatePreparationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(candidate.template_id())])
        .map_err(TemplatePreparationError::Target)?;
    Ok(PreparedTemplateCreate {
        project,
        candidate: Box::new(candidate),
        targets,
        source,
        state: CreationState::Uncommitted,
    })
}

pub(crate) fn prepare_duplicate_template(
    runtime: &mut ProjectRuntime,
    input: &DuplicateTemplateInput,
) -> Result<PreparedTemplateCreate, TemplatePreparationError> {
    let project = runtime.project_fingerprint().to_owned();
    let ready = runtime.ready().map_err(TemplatePreparationError::Runtime)?;
    let repository =
        ArtifactRepository::new(&ready).map_err(TemplatePreparationError::Repository)?;
    let loaded =
        checked_source(&repository, &input.source).map_err(TemplatePreparationError::Source)?;
    let candidate = artifact::duplicate_template(loaded.artifact(), input.timestamp_utc.clone())
        .map_err(|e| TemplatePreparationError::Domain(TemplateUseCaseError::Creation(e)))?;
    ticket(
        project,
        candidate,
        Some(TemplateSource {
            id: input.source.id,
            token: loaded.source().clone(),
            expected_revision: input.source.expected_revision,
        }),
    )
}

pub(crate) fn duplicate_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    prepared: &mut PreparedTemplateCreate,
) -> WriteExecution<(), TemplateUseCaseError> {
    // 준비 결과가 후보와 원본 조건을 함께 묶으므로 실제 저장 경계는 Create와 동일하다.
    create_template(runtime, session, context, prepared)
}

pub(crate) fn update_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &UpdateTemplateInput,
) -> Result<TemplateMutationExecution, ApplicationError> {
    mutate(
        runtime,
        session,
        context,
        &input.source,
        &input.timestamp_utc,
        Some(&input.intent),
    )
}

pub(crate) fn tombstone_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &TombstoneTemplateInput,
) -> Result<TemplateMutationExecution, ApplicationError> {
    mutate(
        runtime,
        session,
        context,
        &input.source,
        &input.timestamp_utc,
        None,
    )
}

pub(crate) fn restore_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &RestoreTemplateInput,
) -> Result<TemplateMutationExecution, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(input.source.id)])?;
    let mut candidate = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut candidate,
        &mut |repository, candidate| {
            let loaded = checked_source(repository, &input.source)?;
            match template_mutation::restore_tombstoned_template(
                loaded.artifact(),
                input.source.expected_revision,
                &input.timestamp_utc,
            )
            .map_err(|e| BuildError::domain(TemplateUseCaseError::Mutation(e)))?
            {
                TemplateMutationOutcome::Unchanged => Ok(WriteDecision::NoWrite(())),
                TemplateMutationOutcome::Changed(changed) => {
                    let changed = candidate.insert(changed);
                    let plan =
                        CanonicalWritePlan::new().replace_template(changed, loaded.source())?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    Ok(TemplateMutationExecution {
        execution,
        candidate,
    })
}

fn mutate<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    source: &TemplateSource,
    timestamp: &str,
    intent: Option<&TemplateEditIntent>,
) -> Result<TemplateMutationExecution, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(source.id)])?;
    let mut candidate = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut candidate,
        &mut |repository, candidate| {
            let loaded = checked_source(repository, source)?;
            let command = if let Some(intent) = intent {
                intent
                    .command(loaded.artifact())
                    .map_err(|e| BuildError::domain(TemplateUseCaseError::Mutation(e)))?
            } else {
                let scan = repository.scan_template_references(source.id)?;
                if scan.source().source() != loaded.source()
                    || !repository.reread_matches(loaded.source())?
                {
                    return Err(BuildError::domain(TemplateUseCaseError::SourceMismatch));
                }
                let assessment = template_mutation::assess_repository_template_references(scan)
                    .map_err(|e| BuildError::domain(TemplateUseCaseError::Mutation(e)))?;
                TemplateMutationCommand::tombstone_template(assessment)
            };
            match template_mutation::apply_template_mutation(
                loaded.artifact(),
                source.expected_revision,
                timestamp,
                command,
            )
            .map_err(|e| BuildError::domain(TemplateUseCaseError::Mutation(e)))?
            {
                TemplateMutationOutcome::Unchanged => Ok(WriteDecision::NoWrite(())),
                TemplateMutationOutcome::Changed(changed) => {
                    // encode 자체가 실패해도 domain 후보와 caller 입력은 서로 별도로 남는다.
                    let changed = candidate.insert(changed);
                    let plan =
                        CanonicalWritePlan::new().replace_template(changed, loaded.source())?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    Ok(TemplateMutationExecution {
        execution,
        candidate,
    })
}

/// begin_edit는 이 준비 결과의 session_targets를 사용한다. 실패해도 준비 결과의 owner는 caller다.
pub(crate) fn create_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    prepared: &mut PreparedTemplateCreate,
) -> WriteExecution<(), TemplateUseCaseError> {
    let mut input = &*prepared;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &prepared.targets,
        },
        &mut input,
        &mut |repository, ticket| {
            let reject = |e| Err(BuildError::domain(e));
            if ticket.project != context.project {
                return reject(TemplateUseCaseError::PreparationProjectMismatch);
            }
            match ticket.state {
                CreationState::Uncommitted => {}
                CreationState::Committed => {
                    return reject(TemplateUseCaseError::PreparationAlreadyCommitted)
                }
                CreationState::RecoveryRequired => {
                    return reject(TemplateUseCaseError::PreparationRequiresRecovery)
                }
            }
            if let Some(source) = &ticket.source {
                checked_source(repository, source)?;
            }
            // active/deleted 모두 점유다. 열거/codec 오류를 부재로 축소하지 않는다.
            if repository.scan_templates()?.records().iter().any(|record| {
                record.source().id() == ArtifactSourceId::Template(ticket.template_id())
            }) {
                return reject(TemplateUseCaseError::TargetOccupied);
            }
            let plan = CanonicalWritePlan::new().create_template(&ticket.candidate)?;
            Ok(WriteDecision::Write { plan, value: () })
        },
    );
    let diagnostic = execution.diagnostic();
    // cleanup 실패여도 Committed는 완료다. 불확실한 요청의 재실행은 복구 이후에도 자동 허용하지 않는다.
    if diagnostic.disk == DiskState::Committed {
        prepared.state = CreationState::Committed;
    } else if diagnostic.recovery_required {
        prepared.state = CreationState::RecoveryRequired;
    }
    execution
}

fn checked_source(
    repository: &ArtifactRepository<'_, '_>,
    source: &TemplateSource,
) -> Result<
    crate::data::repository::LoadedArtifact<TemplateArtifact>,
    BuildError<TemplateUseCaseError>,
> {
    let loaded = repository.load_template(source.id)?;
    if loaded.source() != &source.token {
        return Err(BuildError::domain(TemplateUseCaseError::SourceMismatch));
    }
    if loaded.artifact().revision() != source.expected_revision {
        return Err(BuildError::domain(TemplateUseCaseError::RevisionMismatch));
    }
    Ok(loaded)
}

macro_rules! private_debug {
    ($($ty:ty),+ $(,)?) => {$(impl fmt::Debug for $ty {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($ty), "([private contents])")) }
    })+};
}
private_debug!(
    CreateTemplateInput,
    TemplateSource,
    TemplateWriteContext<'_>,
    PreparedTemplateCreate,
    DuplicateTemplateInput,
    UpdateTemplateInput,
    TombstoneTemplateInput,
    RestoreTemplateInput,
    TemplateEditIntent
);
impl fmt::Debug for TemplateMutationExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateMutationExecution")
            .field("execution", &self.execution)
            .field("candidate_retained", &self.candidate.is_some())
            .finish()
    }
}
impl fmt::Debug for TemplateUseCaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Creation(e) => f.debug_tuple("Creation").field(e).finish(),
            Self::Mutation(e) => f.debug_tuple("Mutation").field(e).finish(),
            Self::SourceMismatch => f.write_str("SourceMismatch"),
            Self::RevisionMismatch => f.write_str("RevisionMismatch"),
            Self::TargetOccupied => f.write_str("TargetOccupied"),
            Self::PreparationProjectMismatch => f.write_str("PreparationProjectMismatch"),
            Self::PreparationAlreadyCommitted => f.write_str("PreparationAlreadyCommitted"),
            Self::PreparationRequiresRecovery => f.write_str("PreparationRequiresRecovery"),
        }
    }
}
impl fmt::Debug for TemplatePreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(e) => f.debug_tuple("Runtime").field(e).finish(),
            Self::Repository(e) => f.debug_tuple("Repository").field(e).finish(),
            Self::Target(e) => f.debug_tuple("Target").field(e).finish(),
            Self::Domain(e) => f.debug_tuple("Domain").field(e).finish(),
            Self::Source(e) => f.debug_tuple("Source").field(e).finish(),
        }
    }
}
impl fmt::Display for TemplateUseCaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Template 요청 거부 ({self:?}): 입력을 보존하고 원본과 요청 조건을 확인하세요"
        )
    }
}
impl std::error::Error for TemplateUseCaseError {}
impl fmt::Display for TemplatePreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Template 준비 실패 ({self:?}): 입력을 보존하고 프로젝트 상태를 확인하세요"
        )
    }
}
impl std::error::Error for TemplatePreparationError {}

#[cfg(all(test, windows))]
mod tests;
