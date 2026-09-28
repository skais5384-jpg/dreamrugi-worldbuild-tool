//! G4 경계에서만 쓰는 읽기 전용 값이다. 원 오류나 경로를 format하지 않는다.
use super::{
    apply::CommitFailure, owned_temp, prepare::ManagedJsonError, protocol, CommitFailureSource,
    CommitStage, PrepareStage, RecoveryError, RecoveryResultState, RecoveryStage, TransactionId,
    TransactionModelError,
};
use crate::data::{
    atomic_file::{AtomicWriteStage, SaveError, SaveOutcome},
    collaboration_lock::{LockErrorCategory, LockOperation, LockProviderKind, WritePermitError},
    repository::ArtifactSourceId,
};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureRole {
    Primary,
    Rollback,
    Cleanup,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureStage {
    Adapter(crate::data::repository::ArtifactWriteStage),
    Admission,
    Prepare(PrepareStage),
    PrepareCleanup,
    Commit(CommitStage),
    Recovery(RecoveryStage),
    AtomicCleanup,
}
#[derive(Clone)]
pub(crate) enum IoContext {
    Journal(protocol::JournalDiagnostic),
    Owned(owned_temp::OwnedStage),
    Atomic {
        stage: AtomicWriteStage,
        outcome: SaveOutcome,
        cleanup: Option<Box<IoDiagnostic>>,
    },
    Json(JsonDiagnostic),
    Recovery {
        state: RecoveryResultState,
        stage: RecoveryStage,
        transaction_id: Option<TransactionId>,
        target: Option<ArtifactSourceId>,
    },
}
impl std::fmt::Debug for IoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 새 원인 타입이 생겨도 raw 객체가 자동으로 로그에 합류하지 않게 고정 projection만 쓴다.
        match self {
            Self::Journal(detail) => f.debug_tuple("Journal").field(detail).finish(),
            Self::Owned(stage) => f.debug_tuple("Owned").field(stage).finish(),
            Self::Atomic {
                stage,
                outcome,
                cleanup,
            } => f
                .debug_struct("Atomic")
                .field("stage", stage)
                .field("outcome", outcome)
                .field("cleanup", cleanup)
                .finish(),
            Self::Json(detail) => f.debug_tuple("Json").field(detail).finish(),
            Self::Recovery {
                state,
                stage,
                transaction_id,
                target,
            } => f
                .debug_struct("Recovery")
                .field("state", state)
                .field("stage", stage)
                .field("transaction_id", transaction_id)
                .field("target", target)
                .finish(),
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct IoDiagnostic {
    pub(crate) kind: io::ErrorKind,
    pub(crate) os_code: Option<i32>,
    pub(crate) context: Vec<IoContext>,
}
impl IoDiagnostic {
    pub(crate) fn new(mut error: &io::Error) -> Self {
        let mut context = Vec::new();
        // 알려진 private wrapper만 따라간다. 임의 Error::source나 Display는 호출하지 않는다.
        loop {
            if let Some((journal, cause)) = protocol::diagnostic(error) {
                context.push(IoContext::Journal(journal));
                if let Some(cause) = cause {
                    error = cause;
                    continue;
                }
            }
            if let Some((stage, cause)) = owned_temp::diagnostic(error) {
                context.push(IoContext::Owned(stage));
                error = cause;
                continue;
            }
            if let Some(recovery) = error
                .get_ref()
                .and_then(|e| e.downcast_ref::<RecoveryError>())
            {
                let failure = recovery.failure();
                context.push(IoContext::Recovery {
                    state: recovery.result_state(),
                    stage: failure.stage,
                    transaction_id: failure.transaction_id.clone(),
                    target: failure
                        .target
                        .as_deref()
                        .and_then(ArtifactSourceId::from_target),
                });
                error = &failure.source;
                continue;
            }
            if let Some(save) = error.get_ref().and_then(|e| e.downcast_ref::<SaveError>()) {
                match save {
                    SaveError::AtomicWrite(failure) => {
                        context.push(IoContext::Atomic {
                            stage: failure.stage,
                            outcome: failure.outcome,
                            cleanup: failure
                                .cleanup_error
                                .as_ref()
                                .map(|e| Box::new(Self::new(e))),
                        });
                        error = &failure.source;
                        continue;
                    }
                    SaveError::Serialize(failure) => context.push(IoContext::Json(failure.into())),
                }
            }
            return Self {
                kind: error.kind(),
                os_code: error.raw_os_error(),
                context,
            };
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct JsonDiagnostic {
    pub(crate) category: &'static str,
    pub(crate) line: usize,
    pub(crate) column: usize,
}
impl From<&serde_json::Error> for JsonDiagnostic {
    fn from(error: &serde_json::Error) -> Self {
        Self {
            category: match error.classify() {
                serde_json::error::Category::Io => "Io",
                serde_json::error::Category::Syntax => "Syntax",
                serde_json::error::Category::Data => "Data",
                serde_json::error::Category::Eof => "Eof",
            },
            line: error.line(),
            column: error.column(),
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct FailureDiagnostic {
    pub(crate) role: FailureRole,
    pub(crate) stage: FailureStage,
    pub(crate) category: &'static str,
    pub(crate) transaction_id: Option<TransactionId>,
    pub(crate) target: Option<ArtifactSourceId>,
    pub(crate) io: Option<IoDiagnostic>,
    pub(crate) json: Option<JsonDiagnostic>,
    pub(crate) atomic_stage: Option<AtomicWriteStage>,
    pub(crate) atomic_outcome: Option<SaveOutcome>,
    pub(crate) recovery_state: Option<RecoveryResultState>,
    pub(crate) lock: Option<(LockErrorCategory, LockOperation, LockProviderKind)>,
    pub(crate) secondary: Vec<FailureDiagnostic>,
}
impl FailureDiagnostic {
    pub(crate) fn new(role: FailureRole, stage: FailureStage, category: &'static str) -> Self {
        Self {
            role,
            stage,
            category,
            transaction_id: None,
            target: None,
            io: None,
            json: None,
            atomic_stage: None,
            atomic_outcome: None,
            recovery_state: None,
            lock: None,
            secondary: Vec::new(),
        }
    }
    pub(crate) fn io(mut self, error: &io::Error) -> Self {
        self.io = Some(IoDiagnostic::new(error));
        self
    }
    pub(crate) fn managed(mut self, error: &ManagedJsonError) -> Self {
        match error {
            ManagedJsonError::Parse(source) => {
                self.category = "ManagedJsonParse";
                self.json = Some(source.into());
            }
            ManagedJsonError::NotObject => self.category = "ManagedJsonNotObject",
            ManagedJsonError::InvalidSchema(source) => {
                self.category = "ManagedJsonInvalidSchema";
                self.json = Some(source.into());
            }
        }
        self
    }
    pub(crate) fn save(mut self, error: &SaveError) -> Self {
        match error {
            SaveError::Serialize(source) => {
                self.category = "AtomicSerialize";
                self.json = Some(source.into());
            }
            SaveError::AtomicWrite(source) => {
                self.category = "AtomicWrite";
                self.atomic_stage = Some(source.stage);
                self.atomic_outcome = Some(source.outcome);
                self.io = Some(IoDiagnostic::new(&source.source));
                if let Some(cleanup) = &source.cleanup_error {
                    self.secondary.push(
                        Self::new(FailureRole::Cleanup, FailureStage::AtomicCleanup, "Io")
                            .io(cleanup),
                    );
                }
            }
        }
        self
    }
    pub(crate) fn model(mut self, error: &TransactionModelError) -> Self {
        self.category = match error {
            TransactionModelError::InvalidTargetPath { .. } => "InvalidTargetPath",
            TransactionModelError::DuplicateTarget { target } => {
                self.target = ArtifactSourceId::from_target(target);
                "DuplicateTarget"
            }
            TransactionModelError::InvalidTransactionId { .. } => "InvalidTransactionId",
            TransactionModelError::InvalidJournal { transaction_id, .. } => {
                self.transaction_id = transaction_id.clone();
                "InvalidJournal"
            }
            TransactionModelError::UnsupportedTransactionSchema { .. } => {
                "UnsupportedTransactionSchema"
            }
            TransactionModelError::LockProjectMismatch { .. } => "LockProjectMismatch",
            TransactionModelError::ProjectBindingFailed { source, .. } => {
                self.io = Some(IoDiagnostic::new(source));
                "ProjectBindingFailed"
            }
            TransactionModelError::TransactionTimeFailed { .. } => "TransactionTimeFailed",
        };
        self
    }
    pub(crate) fn permit(mut self, error: &WritePermitError) -> Self {
        self.category = match error {
            WritePermitError::LockSetNotActive => "PermitLockSetNotActive",
            WritePermitError::ProjectMismatch => "PermitProjectMismatch",
            WritePermitError::TargetMismatch => "PermitTargetMismatch",
            WritePermitError::IdentityChanged => "PermitIdentityChanged",
            WritePermitError::Validation(source) => {
                self.lock = Some((source.category, source.operation, source.provider));
                self.target = source.target().and_then(ArtifactSourceId::from_target);
                "PermitValidation"
            }
        };
        self
    }
    pub(crate) fn commit(failure: &CommitFailure, role: FailureRole) -> Self {
        let mut result = Self::new(role, FailureStage::Commit(failure.stage), "Io");
        result = match failure.source.as_ref() {
            CommitFailureSource::Io(source) => result.io(source),
            CommitFailureSource::AtomicSave(source) => result.save(source),
            CommitFailureSource::Json(source) => {
                result.category = "Json";
                result.json = Some(source.into());
                result
            }
            CommitFailureSource::ManagedJson(source) => result.managed(source),
            CommitFailureSource::Model(source) => result.model(source),
            CommitFailureSource::Permit(source) => result.permit(source),
        };
        result.transaction_id = Some(failure.transaction_id.clone());
        result.target = failure
            .target
            .as_deref()
            .and_then(ArtifactSourceId::from_target);
        result
    }
    pub(crate) fn recovery(error: &RecoveryError, role: FailureRole) -> Self {
        let failure = error.failure();
        let mut result = Self::new(role, FailureStage::Recovery(failure.stage), "RecoveryIo")
            .io(&failure.source);
        result.transaction_id = failure.transaction_id.clone();
        result.target = failure
            .target
            .as_deref()
            .and_then(ArtifactSourceId::from_target);
        result.recovery_state = Some(error.result_state());
        result
    }
}
