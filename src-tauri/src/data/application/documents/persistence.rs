//! 기존 Document의 저장 binding과 편집에 사용한 Template source를 별도로 검증한다.
use super::*;
use crate::data::{
    artifact::{
        DocumentEditSet, DocumentMaterializationError, DocumentMaterializationOutcome,
        DocumentSaveError, DocumentSaveOutcome, DocumentSaveOutcomeKind,
    },
    repository::{LoadedArtifact, SourceToken},
};

pub(crate) struct DocumentSource {
    pub(crate) id: DocumentId,
    pub(crate) token: SourceToken,
}

/// template은 typed edits를 해석할 때 사용한 snapshot의 source다. 최신 source로 치환하지 않는다.
pub(crate) struct SaveDocumentInput {
    pub(crate) document: DocumentSource,
    pub(crate) template: TemplateSource,
    pub(crate) edits: DocumentEditSet,
    pub(crate) timestamp_utc: String,
}

pub(crate) struct MaterializeDocumentInput {
    pub(crate) document: DocumentSource,
    pub(crate) template: TemplateSource,
    pub(crate) timestamp_utc: String,
}

#[derive(Debug)]
pub(crate) enum DocumentUpdateError {
    DocumentSourceMismatch,
    DocumentTrashed,
    TemplateSourceMismatch,
    TemplateBindingMismatch,
    TemplateRevisionMismatch,
    Save(DocumentSaveError),
    Materialization(DocumentMaterializationError),
}

/// 순수 결과는 encode/prepare/commit보다 먼저 이 owner에 보관한다.
/// outcome의 존재는 디스크 commit 증거가 아니며 execution의 원래 상태를 함께 확인해야 한다.
pub(crate) struct DocumentUpdateExecution<T> {
    pub(crate) execution: WriteExecution<(), DocumentUpdateError>,
    outcome: Option<Box<T>>,
}

impl<T> DocumentUpdateExecution<T> {
    pub(crate) fn outcome(&self) -> Option<&T> {
        self.outcome.as_deref()
    }
}

pub(crate) fn save_document<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &SaveDocumentInput,
) -> Result<DocumentUpdateExecution<DocumentSaveOutcome>, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Document(input.document.id)])?;
    let mut outcome = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut outcome,
        &mut |repository, outcome| {
            let (document, template) =
                checked_sources(repository, &input.document, &input.template, |error| error)?;
            // 과거 required/unset도 편집으로 충족할 수 있으므로 materialize 선행 gate를 두지 않는다.
            let candidate = artifact::prepare_document_save(
                template.artifact(),
                input.template.expected_revision,
                document.artifact(),
                &input.edits,
                &input.timestamp_utc,
            )
            .map_err(|cause| BuildError::domain(DocumentUpdateError::Save(cause)))?;
            let candidate = outcome.insert(Box::new(candidate));
            if candidate.kind() == DocumentSaveOutcomeKind::Unchanged {
                return Ok(WriteDecision::NoWrite(()));
            }
            let plan = CanonicalWritePlan::new()
                .replace_document(candidate.document(), document.source())?
                .document_state(repository, input.document.id)?;
            Ok(WriteDecision::Write { plan, value: () })
        },
    );
    Ok(DocumentUpdateExecution { execution, outcome })
}

pub(crate) fn materialize_document<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &MaterializeDocumentInput,
) -> Result<DocumentUpdateExecution<DocumentMaterializationOutcome>, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Document(input.document.id)])?;
    let mut outcome = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut outcome,
        &mut |repository, outcome| {
            let (document, template) =
                checked_sources(repository, &input.document, &input.template, |error| error)?;
            let candidate = artifact::materialize_document(
                template.artifact(),
                input.template.expected_revision,
                document.artifact(),
                input.timestamp_utc.clone(),
            )
            .map_err(|cause| BuildError::domain(DocumentUpdateError::Materialization(cause)))?;
            let candidate = outcome.insert(Box::new(candidate));
            // Unchanged도 warning을 소유하는 정상 결과다. 빈 plan으로 prepare하지 않는다.
            match candidate.document() {
                None => Ok(WriteDecision::NoWrite(())),
                Some(candidate) => {
                    let plan = CanonicalWritePlan::new()
                        .replace_document(candidate, document.source())?
                        .document_state(repository, input.document.id)?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    Ok(DocumentUpdateExecution { execution, outcome })
}

pub(crate) fn repair_missing_optional_group_values<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &MaterializeDocumentInput,
) -> Result<DocumentUpdateExecution<DocumentMaterializationOutcome>, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Document(input.document.id)])?;
    let mut outcome = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut outcome,
        &mut |repository, outcome| {
            let (document, template) =
                checked_sources(repository, &input.document, &input.template, |error| error)?;
            let candidate = artifact::repair_missing_optional_group_values(
                template.artifact(),
                input.template.expected_revision,
                document.artifact(),
                input.timestamp_utc.clone(),
            )
            .map_err(|cause| BuildError::domain(DocumentUpdateError::Materialization(cause)))?;
            let candidate = outcome.insert(Box::new(candidate));
            match candidate.document() {
                None => Ok(WriteDecision::NoWrite(())),
                Some(candidate) => {
                    let plan = CanonicalWritePlan::new()
                        .replace_document(candidate, document.source())?
                        .document_state(repository, input.document.id)?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    Ok(DocumentUpdateExecution { execution, outcome })
}

pub(in crate::data::application) fn checked_sources<E>(
    repository: &ArtifactRepository<'_, '_>,
    document: &DocumentSource,
    template: &TemplateSource,
    domain: impl Fn(DocumentUpdateError) -> E,
) -> Result<
    (
        LoadedArtifact<DocumentArtifact>,
        LoadedArtifact<TemplateArtifact>,
    ),
    BuildError<E>,
> {
    if repository.load_layout()?.as_ref().is_some_and(|l| {
        l.artifact()
            .nodes
            .get(&document.id)
            .is_some_and(|n| n.state == artifact::layout::LayoutState::Trashed)
    }) {
        return Err(BuildError::domain(domain(
            DocumentUpdateError::DocumentTrashed,
        )));
    }
    let loaded_document = repository.load_document(document.id)?;
    if loaded_document.source() != &document.token {
        return Err(BuildError::domain(domain(
            DocumentUpdateError::DocumentSourceMismatch,
        )));
    }
    if loaded_document.artifact().template_id() != template.id {
        return Err(BuildError::domain(domain(
            DocumentUpdateError::TemplateBindingMismatch,
        )));
    }
    let loaded_template = repository.load_template(template.id)?;
    if loaded_template.source() != &template.token {
        return Err(BuildError::domain(domain(
            DocumentUpdateError::TemplateSourceMismatch,
        )));
    }
    // Document의 역사 binding revision과 비교하지 않는다. 편집 snapshot과 final Template만 비교한다.
    if loaded_template.artifact().revision() != template.expected_revision {
        return Err(BuildError::domain(domain(
            DocumentUpdateError::TemplateRevisionMismatch,
        )));
    }
    Ok((loaded_document, loaded_template))
}

impl<T> fmt::Debug for DocumentUpdateExecution<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentUpdateExecution")
            .field("execution", &self.execution)
            .field("has_outcome", &self.outcome.is_some())
            .finish()
    }
}
macro_rules! private_debug { ($($ty:ty),+ $(,)?) => {$(impl fmt::Debug for $ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(concat!(stringify!($ty), "([private contents])"))
    }
})+}; }
private_debug!(DocumentSource, SaveDocumentInput, MaterializeDocumentInput);
impl fmt::Display for DocumentUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Document 갱신 거부 ({self:?}): 입력과 source 상태를 확인하세요"
        )
    }
}
impl std::error::Error for DocumentUpdateError {}
