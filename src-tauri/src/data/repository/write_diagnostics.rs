use super::{ArtifactSourceId, RepositoryError};
use crate::data::transaction::artifact_diagnostics::{
    FailureDiagnostic, FailureRole, IoDiagnostic,
};
#[cfg(test)]
use crate::data::transaction::CommitFailureSource;
use crate::data::{
    artifact::ArtifactCodecError,
    storage_estimate::{StorageAdmission, StorageQueryErrorKind},
    transaction::{
        CommitOutcome, CommitResultState, CommitStage, PrepareStage, TransactionCommitError,
        TransactionId, TransactionPrepareError,
    },
};
use std::{fmt, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactWriteCategory {
    EmptyPlan,
    DuplicateTarget,
    SourceMismatch,
    SourceMissing,
    TargetExists,
    InvalidEntry,
    NamespaceUnavailable,
    RepositoryRejected,
    CodecRejected,
    Io,
    PermitRejected,
    SchemaMismatch,
    StorageRejected,
    EstimateOverflow,
    PrepareFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactWriteStage {
    BuildPlan,
    BindSource,
    Encode,
    Namespace,
    ReadSource,
    Prepare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SafeIo {
    pub(crate) kind: io::ErrorKind,
    pub(crate) os_code: Option<i32>,
}
impl From<&io::Error> for SafeIo {
    fn from(error: &io::Error) -> Self {
        let detail = IoDiagnostic::new(crate::data::transaction::io_cause(error));
        Self {
            kind: detail.kind,
            os_code: detail.os_code,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactWriteDiagnostic {
    pub(crate) state: CommitResultState,
    pub(crate) recovery_required: bool,
    pub(crate) failures: Vec<FailureDiagnostic>,
    pub(crate) category: ArtifactWriteCategory,
    pub(crate) stage: ArtifactWriteStage,
    pub(crate) target: Option<ArtifactSourceId>,
    pub(crate) prepare_stage: Option<PrepareStage>,
    pub(crate) transaction_id: Option<TransactionId>,
    pub(crate) io: Option<SafeIo>,
    pub(crate) cleanup: Option<SafeIo>,
    pub(crate) codec: Option<ArtifactCodecError>,
    pub(crate) repository: Option<super::diagnostics::RepositoryDiagnostic>,
    pub(crate) storage_query: Option<StorageQueryErrorKind>,
    pub(crate) storage: Option<StorageAdmission>,
}
impl ArtifactWriteDiagnostic {
    pub(crate) fn new(
        category: ArtifactWriteCategory,
        stage: ArtifactWriteStage,
        target: Option<ArtifactSourceId>,
    ) -> Self {
        Self {
            state: CommitResultState::NotApplied,
            recovery_required: false,
            failures: Vec::new(),
            category,
            stage,
            target,
            prepare_stage: None,
            transaction_id: None,
            io: None,
            cleanup: None,
            codec: None,
            repository: None,
            storage_query: None,
            storage: None,
        }
    }
    pub(crate) fn next_action(&self) -> &'static str {
        if self.cleanup.is_some() {
            return "편집 내용과 남은 transaction 자료를 보존하고 명시적으로 프로젝트 복구를 실행하세요";
        }
        match self.category {
            ArtifactWriteCategory::SourceMismatch | ArtifactWriteCategory::SourceMissing | ArtifactWriteCategory::TargetExists => "편집 내용을 보존하고 원본을 다시 읽어 전체 저장 작업을 검토하세요",
            ArtifactWriteCategory::StorageRejected | ArtifactWriteCategory::EstimateOverflow => "편집 내용을 보존하고 저장 공간과 볼륨 상태를 확인한 뒤 전체 작업을 다시 실행하세요",
            ArtifactWriteCategory::NamespaceUnavailable => "프로젝트의 정규 자료 폴더와 접근 권한을 확인하거나 작업을 취소하세요",
            _ => "편집 내용과 원본을 보존하고 오류 원인을 확인한 뒤 전체 작업을 다시 실행하거나 프로젝트를 닫으세요",
        }
    }
}

/// 원래 M1 오류와 2차 cleanup을 그대로 소유하되 자동 로그/source chain에는 넣지 않는다.
enum WriteCause {
    None,
    Io(io::Error),
    Repository(RepositoryError),
    Prepare(Box<TransactionPrepareError>),
}
pub(crate) struct ArtifactWriteError {
    diagnostic: Box<ArtifactWriteDiagnostic>,
    original: WriteCause,
}
impl ArtifactWriteError {
    pub(crate) fn closed(
        category: ArtifactWriteCategory,
        stage: ArtifactWriteStage,
        target: Option<ArtifactSourceId>,
    ) -> Self {
        Self {
            diagnostic: Box::new(ArtifactWriteDiagnostic::new(category, stage, target)),
            original: WriteCause::None,
        }
    }
    pub(super) fn codec(
        id: ArtifactSourceId,
        stage: ArtifactWriteStage,
        error: ArtifactCodecError,
    ) -> Self {
        let mut result = Self::closed(ArtifactWriteCategory::CodecRejected, stage, Some(id));
        result.diagnostic.codec = Some(error);
        result
    }
    pub(super) fn io(id: ArtifactSourceId, stage: ArtifactWriteStage, error: io::Error) -> Self {
        let mut result = Self::closed(ArtifactWriteCategory::Io, stage, Some(id));
        result.diagnostic.io = Some((&error).into());
        let mut detail = FailureDiagnostic::new(
            FailureRole::Primary,
            crate::data::transaction::artifact_diagnostics::FailureStage::Adapter(stage),
            "Io",
        )
        .io(&error);
        detail.target = Some(id);
        result.diagnostic.failures.push(detail);
        result.original = WriteCause::Io(error);
        result
    }
    pub(super) fn repository(error: RepositoryError) -> Self {
        let mut result = Self::closed(
            ArtifactWriteCategory::RepositoryRejected,
            ArtifactWriteStage::Namespace,
            error.diagnostic().source_id,
        );
        result.diagnostic.repository = Some(error.diagnostic());
        result.original = WriteCause::Repository(error);
        result
    }
    pub(crate) fn prepare(error: TransactionPrepareError) -> Self {
        Self {
            diagnostic: Box::new(error.artifact_diagnostic()),
            original: WriteCause::Prepare(Box::new(error)),
        }
    }
    pub(crate) fn diagnostic(&self) -> &ArtifactWriteDiagnostic {
        &self.diagnostic
    }
}
impl fmt::Debug for ArtifactWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArtifactWriteError")
            .field(&self.diagnostic)
            .finish()
    }
}
impl fmt::Display for ArtifactWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "자료 저장 준비 실패 ({:?}/{:?}): {}",
            self.diagnostic.stage,
            self.diagnostic.category,
            self.diagnostic.next_action()
        )
    }
}
impl std::error::Error for ArtifactWriteError {}

/// Ok 안의 rollback도 M1 결과 상태 그대로 관찰한다. 원본 오류는 private하게 유지한다.
pub(crate) struct ArtifactCommitOutcome {
    original: CommitOutcome,
}
pub(crate) struct ArtifactCommitError {
    original: TransactionCommitError,
}
#[derive(Debug, Clone)]
pub(crate) struct ArtifactCommitDiagnostic {
    pub(crate) recovery_required: bool,
    pub(crate) failures: Vec<FailureDiagnostic>,
    pub(crate) state: CommitResultState,
    pub(crate) stage: Option<CommitStage>,
    pub(crate) cleanup_failed: bool,
    pub(crate) rollback_failed: bool,
    pub(crate) io: Option<SafeIo>,
}
impl ArtifactCommitDiagnostic {
    fn new(
        state: CommitResultState,
        failures: Vec<FailureDiagnostic>,
        rollback_failed: bool,
    ) -> Self {
        let stage = failures.first().and_then(|failure| match failure.stage {
            crate::data::transaction::artifact_diagnostics::FailureStage::Commit(stage) => {
                Some(stage)
            }
            _ => None,
        });
        let io = failures
            .first()
            .and_then(|failure| failure.io.as_ref())
            .map(|io| SafeIo {
                kind: io.kind,
                os_code: io.os_code,
            });
        let (cleanup_failed, recovery_required) = match state {
            CommitResultState::Committed | CommitResultState::RolledBack => (false, false),
            // NotApplied도 이미 준비한 journal을 보존하므로 다음 쓰기 전 복구 확인이 필요하다.
            CommitResultState::NotApplied | CommitResultState::RecoveryRequired => (false, true),
            CommitResultState::CommittedCleanupFailed
            | CommitResultState::RolledBackCleanupFailed => (true, true),
        };
        Self {
            state,
            stage,
            cleanup_failed,
            rollback_failed,
            io,
            recovery_required,
            failures,
        }
    }
}
impl ArtifactCommitOutcome {
    pub(crate) fn new(original: CommitOutcome) -> Self {
        Self { original }
    }
    pub(crate) fn result_state(&self) -> CommitResultState {
        self.original.result_state()
    }
    pub(crate) fn diagnostic(&self) -> ArtifactCommitDiagnostic {
        let mut failures = Vec::new();
        match &self.original {
            CommitOutcome::Committed => {}
            CommitOutcome::RolledBack { apply_failure } => {
                failures.push(FailureDiagnostic::commit(
                    apply_failure,
                    FailureRole::Primary,
                ));
            }
            CommitOutcome::RolledBackCleanupFailed {
                apply_failure,
                cleanup_failure,
            } => {
                failures.push(FailureDiagnostic::commit(
                    apply_failure,
                    FailureRole::Primary,
                ));
                failures.push(FailureDiagnostic::recovery(
                    cleanup_failure,
                    FailureRole::Cleanup,
                ));
            }
            CommitOutcome::CommittedCleanupFailed {
                failure,
                cleanup_failure,
            } => {
                failures.push(FailureDiagnostic::commit(failure, FailureRole::Primary));
                if let Some(cleanup) = cleanup_failure {
                    failures.push(FailureDiagnostic::commit(cleanup, FailureRole::Cleanup));
                }
            }
        }
        ArtifactCommitDiagnostic::new(self.result_state(), failures, false)
    }
}
impl ArtifactCommitError {
    #[cfg(test)]
    pub(crate) fn b003_assert_compound_causes(&self) {
        let CommitFailureSource::Io(primary) = self.original.failure().source.as_ref() else {
            panic!("expected IO primary cause")
        };
        assert!(primary.to_string().contains("injected primary failure"));
        let secondary = self
            .original
            .rollback_failure()
            .expect("expected secondary cleanup cause");
        assert!(secondary
            .failure()
            .source
            .to_string()
            .contains("injected secondary cleanup failure"));
    }
    pub(crate) fn new(original: TransactionCommitError) -> Self {
        Self { original }
    }
    pub(crate) fn result_state(&self) -> CommitResultState {
        self.original.result_state()
    }
    pub(crate) fn diagnostic(&self) -> ArtifactCommitDiagnostic {
        let mut failures = vec![FailureDiagnostic::commit(
            self.original.failure(),
            FailureRole::Primary,
        )];
        if let Some(rollback) = self.original.rollback_failure() {
            failures.push(FailureDiagnostic::recovery(rollback, FailureRole::Rollback));
        }
        ArtifactCommitDiagnostic::new(
            self.result_state(),
            failures,
            self.original.rollback_failure().is_some(),
        )
    }
}
impl fmt::Debug for ArtifactCommitOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArtifactCommitOutcome")
            .field(&self.diagnostic())
            .finish()
    }
}
impl fmt::Debug for ArtifactCommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArtifactCommitError")
            .field(&self.diagnostic())
            .finish()
    }
}
impl fmt::Display for ArtifactCommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "자료 저장 실패 ({:?}): 편집 내용과 transaction 자료를 보존하고 프로젝트 복구 상태를 확인하세요", self.result_state())
    }
}
impl std::error::Error for ArtifactCommitError {}

#[cfg(test)]
impl ArtifactCommitError {
    pub(crate) fn fix005_original(&self) -> &TransactionCommitError {
        &self.original
    }
}

#[cfg(test)]
impl ArtifactWriteError {
    pub(crate) fn implementation_original_prepare(&self) -> Option<&TransactionPrepareError> {
        match &self.original {
            WriteCause::Prepare(source) => Some(source),
            _ => None,
        }
    }
}
#[cfg(test)]
impl ArtifactCommitOutcome {
    pub(crate) fn implementation_original(&self) -> &CommitOutcome {
        &self.original
    }
}
