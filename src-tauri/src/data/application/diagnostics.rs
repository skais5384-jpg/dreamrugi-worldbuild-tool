use super::write::{BodyOutcome, TransactionResult, WriteExecution};
use crate::data::{
    collaboration_lock::{LockErrorCategory, LockOperation, LockProviderKind, WritePermitError},
    edit_session::{
        EditSessionError, EditSessionErrorCategory, EditSessionState, ValidationFailureRef,
    },
    project_runtime::{RuntimeDiagnostic, RuntimeError, RuntimeSnapshot},
    repository::{
        ArtifactCommitDiagnostic, ArtifactWriteDiagnostic, ArtifactWriteError,
        RepositoryDiagnostic, RepositoryError,
    },
    transaction::CommitResultState,
};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplicationCategory {
    EmptyTargets,
    DuplicateTarget,
    InvalidTarget,
    ProjectMismatch,
    SessionMismatch,
    SessionTargetsMismatch,
    InvalidSessionState,
    RuntimeRejected,
    SessionRejected,
    RepositoryRejected,
    DomainRejected,
    ArtifactRejected,
    PlanTargetsMismatch,
}

enum ApplicationCause {
    Closed,
    Runtime(RuntimeError),
    Session(EditSessionError),
}
pub(crate) struct ApplicationError {
    category: ApplicationCategory,
    cause: ApplicationCause,
}
impl ApplicationError {
    pub(crate) fn closed(category: ApplicationCategory) -> Self {
        Self {
            category,
            cause: ApplicationCause::Closed,
        }
    }
    pub(crate) fn runtime(error: RuntimeError) -> Self {
        Self {
            category: ApplicationCategory::RuntimeRejected,
            cause: ApplicationCause::Runtime(error),
        }
    }
    pub(crate) fn session(error: EditSessionError) -> Self {
        Self {
            category: ApplicationCategory::SessionRejected,
            cause: ApplicationCause::Session(error),
        }
    }
    pub(crate) fn category(&self) -> ApplicationCategory {
        self.category
    }
}
impl fmt::Debug for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ApplicationError")
            .field(&self.category)
            .finish()
    }
}
impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "저장 요청 거부 ({:?}): 편집 내용을 보존하고 프로젝트·세션·대상을 확인하세요",
            self.category
        )
    }
}
impl std::error::Error for ApplicationError {}

/// 업무 오류를 문자열로 다시 만들지 않는다. 원 owner는 private하게 보유하고 자동 출력에서 제외한다.
enum BuildCause<E> {
    Domain(E),
    Repository(RepositoryError),
    Artifact(ArtifactWriteError),
}
pub(crate) struct BuildError<E> {
    cause: BuildCause<E>,
}
impl<E> BuildError<E> {
    pub(crate) fn domain(error: E) -> Self {
        Self {
            cause: BuildCause::Domain(error),
        }
    }
    pub(crate) fn domain_cause(&self) -> Option<&E> {
        match &self.cause {
            BuildCause::Domain(e) => Some(e),
            _ => None,
        }
    }
    pub(crate) fn repository_cause(&self) -> Option<&RepositoryError> {
        match &self.cause {
            BuildCause::Repository(error) => Some(error),
            _ => None,
        }
    }
}
impl<E> From<RepositoryError> for BuildError<E> {
    fn from(error: RepositoryError) -> Self {
        Self {
            cause: BuildCause::Repository(error),
        }
    }
}
impl<E> From<ArtifactWriteError> for BuildError<E> {
    fn from(error: ArtifactWriteError) -> Self {
        Self {
            cause: BuildCause::Artifact(error),
        }
    }
}
impl<E> fmt::Debug for BuildError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BuildError([private cause])")
    }
}
impl<E> fmt::Display for BuildError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("저장 계산 실패: 편집 내용을 보존하고 원본과 입력 조건을 다시 확인하세요")
    }
}
impl<E> std::error::Error for BuildError<E> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiskState {
    NotAttempted,
    NoWrite,
    NotApplied,
    RolledBack,
    Committed,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplicationStage {
    Preflight,
    Access,
    Permit,
    Build,
    Plan,
    Prepare,
    Commit,
    NoWrite,
}
fn disk(state: CommitResultState) -> DiskState {
    match state {
        CommitResultState::NotApplied => DiskState::NotApplied,
        CommitResultState::RolledBack | CommitResultState::RolledBackCleanupFailed => {
            DiskState::RolledBack
        }
        CommitResultState::Committed | CommitResultState::CommittedCleanupFailed => {
            DiskState::Committed
        }
        CommitResultState::RecoveryRequired => DiskState::Uncertain,
    }
}

#[derive(Debug)]
pub(crate) enum PermitCategory {
    LockSetNotActive,
    ProjectMismatch,
    TargetMismatch,
    IdentityChanged,
    Validation,
}
#[derive(Debug)]
pub(crate) struct PermitDiagnostic {
    pub(crate) category: PermitCategory,
    pub(crate) lock_category: Option<LockErrorCategory>,
    pub(crate) provider: Option<LockProviderKind>,
    pub(crate) operation: Option<LockOperation>,
    pub(crate) target: Option<crate::data::repository::ArtifactSourceId>,
}
impl PermitDiagnostic {
    fn lock(error: &crate::data::collaboration_lock::LockError) -> Self {
        Self {
            category: PermitCategory::Validation,
            lock_category: Some(error.category()),
            provider: Some(error.provider),
            operation: Some(error.operation),
            target: error
                .target()
                .and_then(crate::data::repository::ArtifactSourceId::from_target),
        }
    }
    fn permit(error: &WritePermitError) -> Self {
        let category = match error {
            WritePermitError::Validation(error) => return Self::lock(error),
            WritePermitError::LockSetNotActive => PermitCategory::LockSetNotActive,
            WritePermitError::ProjectMismatch => PermitCategory::ProjectMismatch,
            WritePermitError::TargetMismatch => PermitCategory::TargetMismatch,
            WritePermitError::IdentityChanged => PermitCategory::IdentityChanged,
        };
        Self {
            category,
            lock_category: None,
            provider: None,
            operation: None,
            target: None,
        }
    }
}
#[derive(Debug)]
pub(crate) struct ApplicationDiagnostic {
    pub(crate) category: Option<ApplicationCategory>,
    pub(crate) stage: ApplicationStage,
    pub(crate) disk: DiskState,
    pub(crate) recovery_required: bool,
    pub(crate) runtime: RuntimeSnapshot,
    pub(crate) session_state: EditSessionState,
    pub(crate) session_error: Option<EditSessionErrorCategory>,
    pub(crate) permit: Option<PermitDiagnostic>,
    pub(crate) runtime_error: Option<RuntimeDiagnostic>,
    pub(crate) repository: Option<RepositoryDiagnostic>,
    pub(crate) artifact_write: Option<ArtifactWriteDiagnostic>,
    pub(crate) artifact_commit: Option<ArtifactCommitDiagnostic>,
}
impl ApplicationDiagnostic {
    pub(crate) fn next_action(&self) -> &'static str {
        if self.recovery_required {
            return "적용 결과와 편집 내용을 보존하고 명시적으로 프로젝트 복구를 실행하세요. 이전 저장을 자동 반복하지 마세요";
        }
        if self.session_state != EditSessionState::Editing {
            return "편집 내용을 보존하고 잠금 상태를 확인한 뒤 명시적으로 세션을 재개하거나 종료하세요";
        }
        match self.disk {
            DiskState::Committed => "저장 결과를 반영하고 다음 작업을 진행하세요",
            DiskState::NoWrite => "검증 결과와 경고를 확인하세요",
            _ => "편집 내용을 보존하고 원본과 요청 조건을 확인한 뒤 다시 시도하거나 취소하세요",
        }
    }
    pub(crate) fn from_execution<T, E>(execution: &WriteExecution<T, E>) -> Self {
        let mut result = Self {
            category: None,
            stage: ApplicationStage::Preflight,
            disk: DiskState::NotAttempted,
            recovery_required: false,
            runtime: execution.runtime,
            session_state: execution.session.state(),
            session_error: None,
            permit: None,
            runtime_error: None,
            repository: None,
            artifact_write: None,
            artifact_commit: None,
        };
        match &execution.result {
            Err(error) => {
                result.category = Some(error.category);
                match &error.cause {
                    ApplicationCause::Closed => {}
                    ApplicationCause::Runtime(error) => {
                        result.stage = ApplicationStage::Access;
                        result.runtime_error = Some(error.diagnostic());
                    }
                    ApplicationCause::Session(error) => {
                        result.stage = ApplicationStage::Permit;
                        result.session_error = Some(error.category());
                        if let EditSessionError::Validation { source, .. } = error {
                            use crate::data::edit_session::ValidationErrorSource;
                            result.permit = Some(match source {
                                ValidationErrorSource::Lock(e) => PermitDiagnostic::lock(e),
                                ValidationErrorSource::Permit(e) => PermitDiagnostic::permit(e),
                            });
                        }
                    }
                }
            }
            Ok(outcome) => {
                result.stage = ApplicationStage::Build;
                result.permit = outcome.validation_failure().map(|failure| match failure {
                    ValidationFailureRef::Revalidate(e) => PermitDiagnostic::lock(e),
                    ValidationFailureRef::Save(e) => PermitDiagnostic::permit(e),
                });
                let body = match outcome.operation_result() {
                    Ok(body) => body,
                    Err(never) => match *never {},
                };
                match body {
                    BodyOutcome::Rejected(error) => match &error.cause {
                        BuildCause::Domain(_) => {
                            result.category = Some(ApplicationCategory::DomainRejected)
                        }
                        BuildCause::Repository(e) => {
                            result.category = Some(ApplicationCategory::RepositoryRejected);
                            result.repository = Some(e.diagnostic());
                        }
                        BuildCause::Artifact(e) => {
                            result.category = Some(ApplicationCategory::ArtifactRejected);
                            result.write_error(e);
                        }
                    },
                    BodyOutcome::NoWrite(_) => {
                        result.disk = DiskState::NoWrite;
                        result.stage = ApplicationStage::NoWrite;
                    }
                    BodyOutcome::Write { transaction, .. } => match transaction {
                        TransactionResult::PlanMismatch => {
                            result.category = Some(ApplicationCategory::PlanTargetsMismatch);
                            result.stage = ApplicationStage::Plan;
                        }
                        TransactionResult::PrepareFailed(e) => {
                            result.category = Some(ApplicationCategory::ArtifactRejected);
                            result.stage = ApplicationStage::Prepare;
                            result.write_error(e);
                        }
                        TransactionResult::Commit(commit) => {
                            result.stage = ApplicationStage::Commit;
                            let diagnostic = match commit {
                                Ok(outcome) => outcome.diagnostic(),
                                Err(error) => error.diagnostic(),
                            };
                            result.disk = disk(diagnostic.state);
                            result.recovery_required = diagnostic.recovery_required;
                            result.artifact_commit = Some(diagnostic);
                        }
                    },
                }
            }
        }
        result
    }
    fn write_error(&mut self, error: &ArtifactWriteError) {
        self.disk = disk(error.diagnostic().state);
        self.recovery_required = error.diagnostic().recovery_required;
        self.artifact_write = Some(error.diagnostic().clone());
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn all_disk_result_variants_are_mapped_without_guessing_cleanup_state() {
        for (state, expected) in [
            (CommitResultState::NotApplied, DiskState::NotApplied),
            (CommitResultState::RolledBack, DiskState::RolledBack),
            (
                CommitResultState::RolledBackCleanupFailed,
                DiskState::RolledBack,
            ),
            (CommitResultState::Committed, DiskState::Committed),
            (
                CommitResultState::CommittedCleanupFailed,
                DiskState::Committed,
            ),
            (CommitResultState::RecoveryRequired, DiskState::Uncertain),
        ] {
            assert_eq!(disk(state), expected);
        }
    }

    #[test]
    fn defensive_permit_variants_keep_distinct_safe_categories() {
        for (error, expected) in [
            (WritePermitError::LockSetNotActive, "LockSetNotActive"),
            (WritePermitError::ProjectMismatch, "ProjectMismatch"),
            (WritePermitError::TargetMismatch, "TargetMismatch"),
            (WritePermitError::IdentityChanged, "IdentityChanged"),
        ] {
            let diagnostic = PermitDiagnostic::permit(&error);
            assert_eq!(format!("{:?}", diagnostic.category), expected);
            assert!(diagnostic.lock_category.is_none());
        }
    }
}
