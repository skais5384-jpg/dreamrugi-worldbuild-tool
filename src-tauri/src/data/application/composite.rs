//! 명시적 Template 변경과 관련 Document 편집을 한 exact 집합·plan·transaction에 결합한다.
use std::fmt;

use super::{
    diagnostics::{ApplicationError, BuildError},
    documents::persistence::{checked_sources, DocumentSource, DocumentUpdateError},
    templates::{TemplateEditIntent, TemplateSource, TemplateWriteContext},
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    artifact::{
        self,
        template_mutation::{
            apply_template_mutation, TemplateMutationError, TemplateMutationOutcome,
        },
        DocumentEditSet, DocumentSaveError, DocumentSaveOutcome, DocumentSaveOutcomeKind,
    },
    edit_session::EditSessionService,
    project_relative_path::ProjectRelativePath,
    project_runtime::ProjectRuntime,
    repository::{ArtifactSourceId, CanonicalWritePlan},
};

pub(crate) struct CompositeSaveInput {
    pub(crate) template: TemplateSource,
    pub(crate) document: DocumentSource,
    pub(crate) intent: TemplateEditIntent,
    pub(crate) edits: DocumentEditSet,
    pub(crate) timestamp_utc: String,
}

/// 입력을 차용하므로 target을 만든 뒤 ID/intent/edit를 교체하거나 소유권을 잃을 수 없다.
/// 이 요청은 Ready·source 유효성·저장 성공을 증명하지 않으며 caller가 세션을 구성해야 한다.
pub(crate) struct CompositeWriteRequest<'input> {
    input: &'input CompositeSaveInput,
    targets: ExactWriteTargets,
}
impl<'input> CompositeWriteRequest<'input> {
    pub(crate) fn new(input: &'input CompositeSaveInput) -> Result<Self, ApplicationError> {
        let targets = ExactWriteTargets::new([
            ArtifactSourceId::Template(input.template.id),
            ArtifactSourceId::Document(input.document.id),
        ])?;
        Ok(Self { input, targets })
    }

    pub(crate) fn session_targets(&self) -> &[ProjectRelativePath] {
        self.targets.session_targets()
    }
}

#[derive(Debug)]
pub(crate) enum CompositeSaveError {
    Source(DocumentUpdateError),
    OriginalDocument(DocumentSaveError),
    Template(TemplateMutationError),
    Document(DocumentSaveError),
    TemplateUnchangedDocumentChanged,
    TemplateChangedDocumentUnchanged,
}

/// 각 pure 단계 직후부터 결과를 소유한다. 후보의 존재와 전체 디스크 결과는 별개다.
#[must_use = "두 후보와 실제 disk·recovery·session 결과를 함께 확인하세요"]
pub(crate) struct CompositeSaveExecution {
    pub(crate) execution: WriteExecution<(), CompositeSaveError>,
    template_outcome: Option<TemplateMutationOutcome>,
    document_outcome: Option<Box<DocumentSaveOutcome>>,
}
impl CompositeSaveExecution {
    pub(crate) fn template_outcome(&self) -> Option<&TemplateMutationOutcome> {
        self.template_outcome.as_ref()
    }

    pub(crate) fn document_outcome(&self) -> Option<&DocumentSaveOutcome> {
        self.document_outcome.as_deref()
    }
}

pub(crate) fn update_template_and_save_document<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    request: &CompositeWriteRequest<'_>,
) -> CompositeSaveExecution {
    let input = request.input;
    let mut outcomes = (None, None);
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &request.targets,
        },
        &mut outcomes,
        &mut |repository, (template_outcome, document_outcome)| {
            let (document, template) = checked_sources(
                repository,
                &input.document,
                &input.template,
                CompositeSaveError::Source,
            )?;
            // 새 R′로 원본 손상을 가리지 않도록 원 R/D의 좁은 guard를 먼저 적용한다.
            artifact::check_document_save_original(template.artifact(), document.artifact())
                .map_err(|cause| BuildError::domain(CompositeSaveError::OriginalDocument(cause)))?;
            let command = input
                .intent
                .command(template.artifact())
                .map_err(|cause| BuildError::domain(CompositeSaveError::Template(cause)))?;
            let template_outcome = template_outcome.insert(
                apply_template_mutation(
                    template.artifact(),
                    input.template.expected_revision,
                    &input.timestamp_utc,
                    command,
                )
                .map_err(|cause| BuildError::domain(CompositeSaveError::Template(cause)))?,
            );
            // R은 요청의 원본 조건이고 R′은 같은 명령의 검증된 결과다. 원 Document binding은 그대로다.
            let prospective = template_outcome.changed().unwrap_or(template.artifact());
            let document_outcome = document_outcome.insert(Box::new(
                artifact::prepare_document_save(
                    prospective,
                    prospective.revision(),
                    document.artifact(),
                    &input.edits,
                    &input.timestamp_utc,
                )
                .map_err(|cause| BuildError::domain(CompositeSaveError::Document(cause)))?,
            ));
            match (template_outcome, document_outcome.kind()) {
                (TemplateMutationOutcome::Unchanged, DocumentSaveOutcomeKind::Unchanged) => {
                    Ok(WriteDecision::NoWrite(()))
                }
                (TemplateMutationOutcome::Unchanged, DocumentSaveOutcomeKind::Changed) => Err(
                    BuildError::domain(CompositeSaveError::TemplateUnchangedDocumentChanged),
                ),
                (TemplateMutationOutcome::Changed(_), DocumentSaveOutcomeKind::Unchanged) => Err(
                    BuildError::domain(CompositeSaveError::TemplateChangedDocumentUnchanged),
                ),
                (TemplateMutationOutcome::Changed(candidate), DocumentSaveOutcomeKind::Changed) => {
                    // 첫 encode 이전부터 두 outcome을 보관한다. 원 두 token만 Replace 조건에 결합한다.
                    let plan = CanonicalWritePlan::new()
                        .replace_template(candidate, template.source())?
                        .replace_document(document_outcome.document(), document.source())?
                        .document_state(repository, document.artifact().document_id())?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    CompositeSaveExecution {
        execution,
        template_outcome: outcomes.0,
        document_outcome: outcomes.1,
    }
}

impl fmt::Debug for CompositeSaveInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CompositeSaveInput([private contents])")
    }
}
impl fmt::Debug for CompositeWriteRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CompositeWriteRequest([private contents])")
    }
}
impl fmt::Debug for CompositeSaveExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompositeSaveExecution")
            .field("execution", &self.execution)
            .field(
                "template_outcome_retained",
                &self.template_outcome.is_some(),
            )
            .field(
                "document_outcome_retained",
                &self.document_outcome.is_some(),
            )
            .finish()
    }
}
impl fmt::Display for CompositeSaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Template·Document 결합 저장 거부 ({self:?}): 입력을 보존하고 두 원본과 변경 의도를 확인하세요")
    }
}
impl std::error::Error for CompositeSaveError {}

#[cfg(all(test, windows))]
pub(crate) mod tests;
