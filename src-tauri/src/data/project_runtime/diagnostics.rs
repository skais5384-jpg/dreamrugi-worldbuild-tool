use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use super::super::atomic_file::{AtomicWriteStage, SaveError, SaveOutcome};
use super::super::project_lock::{MetadataUpdateError, ProjectLockError};
use super::super::transaction::{
    RecoveryError, RecoveryResultState, RecoveryStage, TransactionModelError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeCategory {
    InvalidProjectPath,
    LockDirectoryUnavailable,
    LockFileOpenFailed,
    AlreadyLocked,
    LockOperationFailed,
    MetadataWriteFailed,
    ReleaseFailed,
    ProjectBindingFailed,
    ProjectIdentityMismatch,
    ProjectNotEmpty,
    ProjectInitializationFailed,
    RecoveryPending,
    RecoveryRequired,
    ManualRecoveryRequired,
    RolledBackCleanupFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeStage {
    Acquire,
    Bind,
    Initialize,
    Recover(RecoveryStage),
    ReviewReport,
    Access,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitializationCleanupOutcome {
    Removed,
    PreservedExternalEntries,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IoDiagnostic {
    pub(crate) kind: io::ErrorKind,
    pub(crate) os_code: Option<i32>,
}

impl From<&io::Error> for IoDiagnostic {
    fn from(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            os_code: error.raw_os_error(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeDiagnostic {
    pub(crate) category: RuntimeCategory,
    pub(crate) stage: RuntimeStage,
    /// M1의 결과 variant를 그대로 보존한다. Cleanup stage만으로 disk old/new를 추측하지 않는다.
    pub(crate) recovery_result: Option<RecoveryResultState>,
    pub(crate) io: Option<IoDiagnostic>,
    pub(crate) metadata_stage: Option<AtomicWriteStage>,
    pub(crate) metadata_outcome: Option<SaveOutcome>,
    pub(crate) metadata_cleanup: Option<IoDiagnostic>,
    pub(crate) initialization_cleanup_outcome: Option<InitializationCleanupOutcome>,
    pub(crate) initialization_cleanup: Option<IoDiagnostic>,
    pub(crate) release_failure: Option<IoDiagnostic>,
}

impl RuntimeDiagnostic {
    pub(crate) fn next_action(&self) -> &'static str {
        match self.category {
            RuntimeCategory::ProjectBindingFailed | RuntimeCategory::ProjectIdentityMismatch => {
                "프로젝트 위치와 접근 권한을 확인한 뒤 복구를 재시도하거나 프로젝트를 닫으세요"
            }
            RuntimeCategory::ProjectNotEmpty => {
                "빈 폴더를 선택하거나 기존 프로젝트 열기를 사용하세요"
            }
            RuntimeCategory::AlreadyLocked => "프로젝트를 사용하는 다른 창을 닫은 뒤 다시 여세요",
            RuntimeCategory::ManualRecoveryRequired => {
                "복구 자료를 보존하고 수동 점검을 받거나 프로젝트를 닫으세요"
            }
            RuntimeCategory::RolledBackCleanupFailed => {
                "복구 자료를 보존하고 원인을 해결한 뒤 정리를 재시도하거나 프로젝트를 닫으세요"
            }
            RuntimeCategory::ReleaseFailed => {
                "복구 자료를 보존하고 잠금 상태를 확인한 뒤 다시 여세요"
            }
            RuntimeCategory::RecoveryPending | RuntimeCategory::RecoveryRequired => {
                "원인을 해결한 뒤 복구를 명시적으로 재시도하거나 프로젝트를 닫으세요"
            }
            _ => "프로젝트 위치와 접근 권한을 확인한 뒤 다시 시도하세요",
        }
    }
}

/// 원인은 private 소유권으로 보존하지만 자동 Debug/Display/source 출력에는 연결하지 않는다.
/// snapshot과 반환 오류는 제한된 진단만 복사하며 오류 history를 누적하지 않는다.
#[derive(Clone)]
pub(crate) struct RuntimeError {
    diagnostic: RuntimeDiagnostic,
    original: Option<Arc<dyn Error + Send + Sync>>,
    initialization_cleanup: Option<Arc<io::Error>>,
    release: Option<Arc<RuntimeError>>,
}

impl RuntimeError {
    pub(crate) fn diagnostic(&self) -> RuntimeDiagnostic {
        self.diagnostic
    }

    pub(super) fn closed(category: RuntimeCategory, stage: RuntimeStage) -> Self {
        Self {
            diagnostic: RuntimeDiagnostic {
                category,
                stage,
                recovery_result: None,
                io: None,
                metadata_stage: None,
                metadata_outcome: None,
                metadata_cleanup: None,
                initialization_cleanup_outcome: None,
                initialization_cleanup: None,
                release_failure: None,
            },
            original: None,
            initialization_cleanup: None,
            release: None,
        }
    }

    pub(super) fn with_initialization_cleanup(mut self, cleanup: Result<(), io::Error>) -> Self {
        match cleanup {
            Ok(()) => {
                self.diagnostic.initialization_cleanup_outcome =
                    Some(InitializationCleanupOutcome::Removed);
            }
            Err(error) => {
                self.diagnostic.initialization_cleanup_outcome =
                    Some(if error.kind() == io::ErrorKind::DirectoryNotEmpty {
                        InitializationCleanupOutcome::PreservedExternalEntries
                    } else {
                        InitializationCleanupOutcome::Failed
                    });
                self.diagnostic.initialization_cleanup = Some((&error).into());
                self.initialization_cleanup = Some(Arc::new(error));
            }
        }
        self
    }

    pub(super) fn with_release(mut self, release: Self) -> Self {
        self.diagnostic.release_failure = release.diagnostic.io;
        self.release = Some(Arc::new(release));
        self
    }

    pub(super) fn binding(source: TransactionModelError) -> Self {
        let category = match &source {
            TransactionModelError::LockProjectMismatch { .. } => {
                RuntimeCategory::ProjectIdentityMismatch
            }
            _ => RuntimeCategory::ProjectBindingFailed,
        };
        let mut error = Self::closed(category, RuntimeStage::Bind);
        if let TransactionModelError::ProjectBindingFailed { source, .. } = &source {
            error.diagnostic.io = Some(source.into());
        }
        error.original = Some(Arc::new(source));
        error
    }

    pub(super) fn project_not_empty() -> Self {
        Self::closed(RuntimeCategory::ProjectNotEmpty, RuntimeStage::Initialize)
    }

    pub(super) fn project_initialization(source: io::Error) -> Self {
        let mut error = Self::closed(
            RuntimeCategory::ProjectInitializationFailed,
            RuntimeStage::Initialize,
        );
        error.diagnostic.io = Some((&source).into());
        error.original = Some(Arc::new(source));
        error
    }

    pub(super) fn restore_recovery(source: super::super::project_backup::BackupError) -> Self {
        let mut error = Self::closed(
            RuntimeCategory::RecoveryRequired,
            RuntimeStage::ReviewReport,
        );
        error.diagnostic.io = source.io_kind().map(|kind| IoDiagnostic {
            kind,
            os_code: None,
        });
        error.original = Some(Arc::new(source));
        error
    }

    pub(super) fn maintenance_recovery(source: super::super::asset_maintenance::Error) -> Self {
        let mut error = Self::closed(
            RuntimeCategory::RecoveryRequired,
            RuntimeStage::ReviewReport,
        );
        error.original = Some(Arc::new(source));
        error
    }

    pub(super) fn recovery(source: RecoveryError) -> Self {
        let category = match &source {
            RecoveryError::RecoveryRequired(_) => RuntimeCategory::RecoveryRequired,
            RecoveryError::ManualRecoveryRequired(_) => RuntimeCategory::ManualRecoveryRequired,
            RecoveryError::RolledBackCleanupFailed(_) => RuntimeCategory::RolledBackCleanupFailed,
        };
        let mut error = Self::closed(category, RuntimeStage::Recover(source.failure().stage));
        error.diagnostic.recovery_result = Some(source.result_state());
        error.diagnostic.io = Some(source.failure().io_cause().into());
        error.original = Some(Arc::new(source));
        error
    }

    pub(super) fn lock(source: ProjectLockError) -> Self {
        use ProjectLockError as L;
        let category = match &source {
            L::InvalidProjectPath { .. } => RuntimeCategory::InvalidProjectPath,
            L::LockDirectoryUnavailable { .. } => RuntimeCategory::LockDirectoryUnavailable,
            L::LockFileOpenFailed { .. } => RuntimeCategory::LockFileOpenFailed,
            L::AlreadyLocked { .. } => RuntimeCategory::AlreadyLocked,
            L::LockOperationFailed { .. } => RuntimeCategory::LockOperationFailed,
            L::MetadataWriteFailed { .. } => RuntimeCategory::MetadataWriteFailed,
            L::UnlockFailed { .. } => RuntimeCategory::ReleaseFailed,
        };
        let stage = if category == RuntimeCategory::ReleaseFailed {
            RuntimeStage::Close
        } else {
            RuntimeStage::Acquire
        };
        let mut error = Self::closed(category, stage);
        match &source {
            L::InvalidProjectPath { source, .. }
            | L::LockDirectoryUnavailable { source, .. }
            | L::LockFileOpenFailed { source, .. }
            | L::LockOperationFailed { source, .. }
            | L::UnlockFailed { source, .. } => error.diagnostic.io = Some(source.into()),
            L::MetadataWriteFailed {
                source,
                unlock_error,
                ..
            } => {
                error.diagnostic.release_failure = unlock_error.as_ref().map(Into::into);
                if let MetadataUpdateError::Save(SaveError::AtomicWrite(source)) = source {
                    error.diagnostic.io = Some((&source.source).into());
                    error.diagnostic.metadata_stage = Some(source.stage);
                    error.diagnostic.metadata_outcome = Some(source.outcome);
                    error.diagnostic.metadata_cleanup =
                        source.cleanup_error.as_ref().map(Into::into);
                }
            }
            L::AlreadyLocked { .. } => {}
        }
        error.original = Some(Arc::new(source));
        error
    }
}

impl fmt::Debug for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeError")
            .field("diagnostic", &self.diagnostic)
            .field(
                "initialization_cleanup",
                &self.initialization_cleanup.as_ref().map(|_| "[redacted]"),
            )
            .field("release", &self.release)
            .finish()
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "프로젝트 작업 실패 ({:?}, {:?}); {}",
            self.diagnostic.category,
            self.diagnostic.stage,
            self.diagnostic.next_action()
        )
    }
}

impl Error for RuntimeError {}

#[cfg(test)]
impl RuntimeError {
    pub(crate) fn fix005_recovery(source: RecoveryError) -> Self {
        Self::recovery(source)
    }
    pub(crate) fn fix005_original(&self) -> &RecoveryError {
        self.original
            .as_ref()
            .and_then(|x| x.downcast_ref::<RecoveryError>())
            .unwrap()
    }

    pub(crate) fn m51_initialization_with_cleanup(
        source: io::Error,
        cleanup: Result<(), io::Error>,
    ) -> Self {
        Self::project_initialization(source).with_initialization_cleanup(cleanup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::atomic_file::AtomicWriteError;
    use crate::data::project_relative_path::ProjectRelativePath;
    use crate::data::project_runtime::tests::assert_redacted;
    use crate::data::transaction::test_support::RecoveryFailure;

    const CANARY: &str = "C:/Users/private-user/raw-journal-body?credential=secret";

    #[test]
    fn all_recovery_variants_keep_original_privately_and_redact_entire_chain() {
        for kind in [
            RecoveryResultState::RecoveryRequired,
            RecoveryResultState::ManualRecoveryRequired,
            RecoveryResultState::RolledBackCleanupFailed,
        ] {
            let failure = Box::new(RecoveryFailure {
                stage: RecoveryStage::CleanupTransaction,
                transaction_id: None,
                project_fingerprint: CANARY.into(),
                target: Some(Box::new(
                    ProjectRelativePath::parse("data/private-user.json").unwrap(),
                )),
                source: io::Error::other(io::Error::other(CANARY)),
            });
            let source = match kind {
                RecoveryResultState::RecoveryRequired => RecoveryError::RecoveryRequired(failure),
                RecoveryResultState::ManualRecoveryRequired => {
                    RecoveryError::ManualRecoveryRequired(failure)
                }
                RecoveryResultState::RolledBackCleanupFailed => {
                    RecoveryError::RolledBackCleanupFailed(failure)
                }
            };
            let error = RuntimeError::recovery(source);
            assert_eq!(error.diagnostic.recovery_result, Some(kind));
            assert_eq!(
                error.diagnostic.stage,
                RuntimeStage::Recover(RecoveryStage::CleanupTransaction)
            );
            assert_eq!(error.diagnostic.io.unwrap().kind, io::ErrorKind::Other);
            assert_redacted(&error);
            let original = error
                .original
                .as_ref()
                .unwrap()
                .downcast_ref::<RecoveryError>()
                .unwrap();
            assert_eq!(original.failure().source.to_string(), CANARY);
        }
    }

    #[test]
    fn metadata_primary_cleanup_and_unlock_diagnostics_are_separate_and_redacted() {
        let error = RuntimeError::lock(ProjectLockError::MetadataWriteFailed {
            fingerprint: CANARY.into(),
            source: MetadataUpdateError::Save(SaveError::AtomicWrite(AtomicWriteError {
                stage: AtomicWriteStage::WriteTemporary,
                target: CANARY.into(),
                temporary_path: Some(CANARY.into()),
                source: io::Error::new(io::ErrorKind::WriteZero, CANARY),
                cleanup_error: Some(io::Error::new(io::ErrorKind::PermissionDenied, CANARY)),
                outcome: SaveOutcome::NotApplied,
            })),
            unlock_error: Some(io::Error::other(CANARY)),
        });
        let diagnostic = error.diagnostic();
        assert_eq!(diagnostic.category, RuntimeCategory::MetadataWriteFailed);
        assert_eq!(
            diagnostic.metadata_stage,
            Some(AtomicWriteStage::WriteTemporary)
        );
        assert_eq!(diagnostic.io.unwrap().kind, io::ErrorKind::WriteZero);
        assert_eq!(
            diagnostic.metadata_cleanup.unwrap().kind,
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            diagnostic.release_failure.unwrap().kind,
            io::ErrorKind::Other
        );
        assert_redacted(&error);
        assert!(error.original.as_ref().unwrap().is::<ProjectLockError>());
    }
}
