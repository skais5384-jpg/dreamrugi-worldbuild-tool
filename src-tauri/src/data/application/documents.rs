//! 실제 Template 읽기와 Document 생성·갱신을 G5의 단일 Document 저장 경계에 연결한다.
use std::fmt;

pub(crate) mod persistence;

use super::{
    diagnostics::{ApplicationError, BuildError, DiskState},
    templates::{CreationState, TemplateSource, TemplateWriteContext},
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    artifact::{self, DocumentArtifact, DocumentCreationError, DocumentId, TemplateArtifact},
    edit_session::EditSessionService,
    project_relative_path::ProjectRelativePath,
    project_runtime::{ProjectRuntime, RuntimeError},
    repository::{ArtifactRepository, ArtifactSourceId, CanonicalWritePlan, RepositoryError},
};

pub(crate) struct CreateDocumentInput {
    pub(crate) source: TemplateSource,
    pub(crate) name: String,
    pub(crate) timestamp_utc: String,
}

pub(crate) struct PreparedDocumentCreate {
    project: String,
    candidate: Box<DocumentArtifact>,
    targets: ExactWriteTargets,
    source: TemplateSource,
    state: CreationState,
}

impl PreparedDocumentCreate {
    pub(crate) fn document_id(&self) -> DocumentId {
        self.candidate.document_id()
    }
    pub(crate) fn candidate(&self) -> &DocumentArtifact {
        &self.candidate
    }
    pub(crate) fn session_targets(&self) -> &[ProjectRelativePath] {
        self.targets.session_targets()
    }
    pub(crate) fn state(&self) -> CreationState {
        self.state
    }
}

pub(crate) enum DocumentUseCaseError {
    SourceMismatch,
    RevisionMismatch,
    TargetOccupied,
    PreparationProjectMismatch,
    PreparationAlreadyCommitted,
    PreparationRequiresRecovery,
    Creation(DocumentCreationError),
}

pub(crate) enum DocumentPreparationError {
    Runtime(RuntimeError),
    Repository(RepositoryError),
    Target(ApplicationError),
    Domain(DocumentUseCaseError),
    Source(BuildError<DocumentUseCaseError>),
}

pub(crate) fn prepare_create_document(
    runtime: &mut ProjectRuntime,
    input: &CreateDocumentInput,
) -> Result<PreparedDocumentCreate, DocumentPreparationError> {
    let project = runtime.project_fingerprint().to_owned();
    let ready = runtime.ready().map_err(DocumentPreparationError::Runtime)?;
    let repository =
        ArtifactRepository::new(&ready).map_err(DocumentPreparationError::Repository)?;
    let loaded =
        checked_source(&repository, &input.source).map_err(DocumentPreparationError::Source)?;
    let candidate = artifact::create_document(
        loaded.artifact(),
        input.source.expected_revision,
        DocumentId::new(),
        input.name.clone(),
        input.timestamp_utc.clone(),
    )
    .map_err(|error| DocumentPreparationError::Domain(DocumentUseCaseError::Creation(error)))?
    .into_document();
    let targets = ExactWriteTargets::new([ArtifactSourceId::Document(candidate.document_id())])
        .map_err(DocumentPreparationError::Target)?;
    Ok(PreparedDocumentCreate {
        project,
        candidate: Box::new(candidate),
        targets,
        source: TemplateSource {
            id: input.source.id,
            token: loaded.source().clone(),
            expected_revision: input.source.expected_revision,
        },
        state: CreationState::Uncommitted,
    })
}

/// 준비된 candidate는 final Template source가 정확히 같을 때만 단일 Document Create plan으로 소비한다.
pub(crate) fn create_document_from_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    prepared: &mut PreparedDocumentCreate,
) -> WriteExecution<(), DocumentUseCaseError> {
    let mut ticket = &*prepared;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &prepared.targets,
        },
        &mut ticket,
        &mut |repository, ticket| {
            let reject = |error| Err(BuildError::domain(error));
            if ticket.project != context.project {
                return reject(DocumentUseCaseError::PreparationProjectMismatch);
            }
            match ticket.state {
                CreationState::Uncommitted => {}
                CreationState::Committed => {
                    return reject(DocumentUseCaseError::PreparationAlreadyCommitted)
                }
                CreationState::RecoveryRequired => {
                    return reject(DocumentUseCaseError::PreparationRequiresRecovery)
                }
            }
            checked_source(repository, &ticket.source)?;
            if repository
                .scan_documents()?
                .sources()
                .any(|source| source.id() == ArtifactSourceId::Document(ticket.document_id()))
            {
                return reject(DocumentUseCaseError::TargetOccupied);
            }
            let plan = CanonicalWritePlan::new().create_document(&ticket.candidate)?;
            Ok(WriteDecision::Write { plan, value: () })
        },
    );
    let diagnostic = execution.diagnostic();
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
    BuildError<DocumentUseCaseError>,
> {
    let loaded = repository.load_template(source.id)?;
    if loaded.source() != &source.token {
        return Err(BuildError::domain(DocumentUseCaseError::SourceMismatch));
    }
    if loaded.artifact().revision() != source.expected_revision {
        return Err(BuildError::domain(DocumentUseCaseError::RevisionMismatch));
    }
    Ok(loaded)
}

macro_rules! private_debug { ($($ty:ty),+ $(,)?) => {$(impl fmt::Debug for $ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(concat!(stringify!($ty), "([private contents])")) }
})+}; }
private_debug!(CreateDocumentInput, PreparedDocumentCreate);
impl fmt::Debug for DocumentUseCaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceMismatch => f.write_str("SourceMismatch"),
            Self::RevisionMismatch => f.write_str("RevisionMismatch"),
            Self::TargetOccupied => f.write_str("TargetOccupied"),
            Self::PreparationProjectMismatch => f.write_str("PreparationProjectMismatch"),
            Self::PreparationAlreadyCommitted => f.write_str("PreparationAlreadyCommitted"),
            Self::PreparationRequiresRecovery => f.write_str("PreparationRequiresRecovery"),
            Self::Creation(error) => f.debug_tuple("Creation").field(error).finish(),
        }
    }
}
impl fmt::Display for DocumentUseCaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Document 요청 거부 ({self:?}): 입력과 Template 조건을 확인하세요"
        )
    }
}
impl std::error::Error for DocumentUseCaseError {}
impl fmt::Debug for DocumentPreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => f.debug_tuple("Runtime").field(error).finish(),
            Self::Repository(error) => f.debug_tuple("Repository").field(error).finish(),
            Self::Target(error) => f.debug_tuple("Target").field(error).finish(),
            Self::Domain(error) => f.debug_tuple("Domain").field(error).finish(),
            Self::Source(error) => f.debug_tuple("Source").field(error).finish(),
        }
    }
}
impl fmt::Display for DocumentPreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Document 준비 실패 ({self:?}): 입력과 프로젝트 상태를 확인하세요"
        )
    }
}
impl std::error::Error for DocumentPreparationError {}

#[cfg(all(test, windows))]
mod tests;
