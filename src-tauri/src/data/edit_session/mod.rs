use std::error::Error;
use std::fmt;
use std::sync::Arc;

use super::collaboration_lock::{
    LockCoordinator, LockCoordinatorError, LockError, LockErrorCategory, LockProviderKind,
    LockRequestValidationError, LockService, LockSessionId, LockSetGuard, LockSetIdentityError,
    LockSetReleaseError, PartialAcquireCleanup, ValidatedLockSetRequest, WritePermit,
    WritePermitError,
};
use super::project_relative_path::ProjectRelativePath;

#[cfg(test)]
mod tests;

/// 편집 세션에서 외부에 공개하는 안정적인 상태다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSessionState {
    ReadOnly,
    Acquiring,
    Editing,
    LockLost,
    Releasing,
    ReleaseFailed,
    RecoveryRequired,
}

/// 오류와 진단에서 작업 단계를 구분한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSessionOperation {
    BeginEdit,
    ValidateForSave,
    Revalidate,
    EndEdit,
    RetryRelease,
    ChangeTargets,
    ResumeEdit,
    PreserveRecovery,
    AcceptRecoveryDurably,
    AcknowledgeRecovery,
    DiscardRecovery,
}

impl fmt::Display for EditSessionOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BeginEdit => "begin edit session",
            Self::ValidateForSave => "validate edit session for save",
            Self::Revalidate => "revalidate edit session",
            Self::EndEdit => "end edit session",
            Self::RetryRelease => "retry edit session release",
            Self::ChangeTargets => "change edit session targets",
            Self::ResumeEdit => "resume edit session",
            Self::PreserveRecovery => "preserve edit recovery payload",
            Self::AcceptRecoveryDurably => "durably accept edit recovery payload",
            Self::AcknowledgeRecovery => "acknowledge edit recovery payload",
            Self::DiscardRecovery => "explicitly discard edit recovery payload",
        })
    }
}

/// 상위 계층이 상태와 안전한 식별 정보만 읽는 snapshot이다.
/// held lock, provider token, recovery payload는 포함하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditSessionSnapshot {
    state: EditSessionState,
    project_fingerprint: Option<String>,
    session_id: Option<LockSessionId>,
    provider_kind: Option<LockProviderKind>,
    targets: Vec<ProjectRelativePath>,
    recovery_handoff: Option<RecoveryHandoffStatus>,
    release_failure_count: usize,
}

impl EditSessionSnapshot {
    pub(crate) fn state(&self) -> EditSessionState {
        self.state
    }

    pub(crate) fn project_fingerprint(&self) -> Option<&str> {
        self.project_fingerprint.as_deref()
    }

    pub(crate) fn session_id(&self) -> Option<&LockSessionId> {
        self.session_id.as_ref()
    }

    pub(crate) fn provider_kind(&self) -> Option<LockProviderKind> {
        self.provider_kind
    }

    pub(crate) fn targets(&self) -> &[ProjectRelativePath] {
        &self.targets
    }

    pub(crate) fn recovery_handoff(&self) -> Option<RecoveryHandoffStatus> {
        self.recovery_handoff
    }

    pub(crate) fn release_failure_count(&self) -> usize {
        self.release_failure_count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryHandoffStatus {
    Pending,
    InProgress,
    DurablyAccepted,
    Acknowledged,
    Discarded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryReason {
    ExplicitDraftDeposit,
    SaveRecoveryRequired,
    LockLost,
    LockStateUnknown,
    LockValidationFailed,
}

/// Recovery Center 형식과 payload 본문을 분리한 안전한 전달 metadata다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryEnvelope {
    project_fingerprint: String,
    session_id: LockSessionId,
    provider_kind: LockProviderKind,
    targets: Vec<ProjectRelativePath>,
    reason: RecoveryReason,
}

impl RecoveryEnvelope {
    pub(crate) fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    pub(crate) fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    pub(crate) fn provider_kind(&self) -> LockProviderKind {
        self.provider_kind
    }

    pub(crate) fn targets(&self) -> &[ProjectRelativePath] {
        &self.targets
    }

    pub(crate) fn reason(&self) -> RecoveryReason {
        self.reason
    }
}

/// crash-safe 저장소가 payload custody를 인수하는 경계다.
/// 구현자는 payload와 durable metadata가 crash-safe하게 저장되기 전에 `Ok`를 반환하면 안 된다.
pub(crate) trait DurableRecoverySink<P> {
    type Receipt;

    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        payload: P,
    ) -> Result<Self::Receipt, RecoverySinkFailure<P>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverySinkFailureCategory {
    Unavailable,
    Rejected,
    DurabilityUncertain,
}

/// sink 내부의 raw 오류는 sink 경계에서 처리하고, service에는 안전한 category와
/// payload 소유권만 돌려준다.
pub(crate) struct RecoverySinkFailure<P> {
    payload: P,
    category: RecoverySinkFailureCategory,
}

impl<P> RecoverySinkFailure<P> {
    pub(crate) fn new(payload: P, category: RecoverySinkFailureCategory) -> Self {
        Self { payload, category }
    }

    pub(crate) fn into_parts(self) -> (P, RecoverySinkFailureCategory) {
        (self.payload, self.category)
    }
}

impl<P> fmt::Debug for RecoverySinkFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoverySinkFailure")
            .field("payload", &"[opaque]")
            .field("category", &self.category)
            .finish()
    }
}

/// preserve가 거부돼도 opaque payload를 caller에게 돌려주는 ownership 오류다.
pub(crate) struct PreserveRecoveryFailure<P> {
    payload: P,
    error: EditSessionError,
}

impl<P> PreserveRecoveryFailure<P> {
    fn new(payload: P, error: EditSessionError) -> Self {
        Self { payload, error }
    }

    pub(crate) fn category(&self) -> EditSessionErrorCategory {
        self.error.category()
    }

    pub(crate) fn into_payload(self) -> P {
        self.payload
    }

    pub(crate) fn into_parts(self) -> (P, EditSessionError) {
        (self.payload, self.error)
    }
}

impl<P> fmt::Debug for PreserveRecoveryFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreserveRecoveryFailure")
            .field("payload", &"[opaque]")
            .field("error", &self.error)
            .finish()
    }
}

impl<P> fmt::Display for PreserveRecoveryFailure<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl<P> Error for PreserveRecoveryFailure<P> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSessionErrorCategory {
    InvalidState,
    InvalidIdentity,
    AcquireFailed,
    PartialAcquireCleanupFailed,
    ValidationFailed,
    LockLost,
    LockStateUnknown,
    ReleaseFailed,
    ReleaseRetryFailed,
    RecoveryHandoffFailed,
    ProjectMismatch,
    TargetMismatch,
    InternalInvariantViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditSessionStateError {
    operation: EditSessionOperation,
    state: EditSessionState,
}

impl EditSessionStateError {
    pub(crate) fn operation(self) -> EditSessionOperation {
        self.operation
    }

    pub(crate) fn state(self) -> EditSessionState {
        self.state
    }
}

impl fmt::Display for EditSessionStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot {} while edit session is {:?}",
            self.operation, self.state
        )
    }
}

impl Error for EditSessionStateError {}

#[derive(Debug, Clone)]
enum ValidationFailure {
    Revalidate(LockError),
    Save(WritePermitError),
}

impl ValidationFailure {
    fn category(&self) -> EditSessionErrorCategory {
        let category = match self {
            Self::Revalidate(source) => source.category(),
            Self::Save(WritePermitError::Validation(source)) => source.category(),
            Self::Save(_) => return EditSessionErrorCategory::ValidationFailed,
        };
        match category {
            LockErrorCategory::LockLost => EditSessionErrorCategory::LockLost,
            LockErrorCategory::LockStateUnknown => EditSessionErrorCategory::LockStateUnknown,
            _ => EditSessionErrorCategory::ValidationFailed,
        }
    }

    fn as_ref(&self) -> ValidationFailureRef<'_> {
        match self {
            Self::Revalidate(source) => ValidationFailureRef::Revalidate(source),
            Self::Save(source) => ValidationFailureRef::Save(source),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ValidationFailureRef<'a> {
    Revalidate(&'a LockError),
    Save(&'a WritePermitError),
}

#[derive(Debug)]
pub(crate) enum EditSessionError {
    State(EditSessionStateError),
    Identity {
        operation: EditSessionOperation,
        source: LockSetIdentityError,
    },
    Acquire {
        operation: EditSessionOperation,
        source: LockError,
    },
    AcquireCleanup {
        operation: EditSessionOperation,
        acquire_error: LockError,
        cleanup_error: LockSetReleaseError,
    },
    Validation {
        operation: EditSessionOperation,
        category: EditSessionErrorCategory,
        source: ValidationErrorSource,
    },
    Release {
        operation: EditSessionOperation,
        source: LockSetReleaseError,
    },
    ProjectMismatch {
        operation: EditSessionOperation,
    },
    TargetMismatch {
        operation: EditSessionOperation,
    },
    InternalInvariant {
        operation: EditSessionOperation,
    },
}

#[derive(Debug)]
pub(crate) enum ValidationErrorSource {
    Lock(LockError),
    Permit(WritePermitError),
}

impl EditSessionError {
    pub(crate) fn category(&self) -> EditSessionErrorCategory {
        match self {
            Self::State(_) => EditSessionErrorCategory::InvalidState,
            Self::Identity { .. } => EditSessionErrorCategory::InvalidIdentity,
            Self::Acquire { .. } => EditSessionErrorCategory::AcquireFailed,
            Self::AcquireCleanup { .. } => EditSessionErrorCategory::PartialAcquireCleanupFailed,
            Self::Validation { category, .. } => *category,
            Self::Release { operation, .. } => {
                if *operation == EditSessionOperation::RetryRelease {
                    EditSessionErrorCategory::ReleaseRetryFailed
                } else {
                    EditSessionErrorCategory::ReleaseFailed
                }
            }
            Self::ProjectMismatch { .. } => EditSessionErrorCategory::ProjectMismatch,
            Self::TargetMismatch { .. } => EditSessionErrorCategory::TargetMismatch,
            Self::InternalInvariant { .. } => EditSessionErrorCategory::InternalInvariantViolation,
        }
    }

    pub(crate) fn acquire_error(&self) -> Option<&LockError> {
        match self {
            Self::Acquire { source, .. } => Some(source),
            Self::AcquireCleanup { acquire_error, .. } => Some(acquire_error),
            _ => None,
        }
    }

    pub(crate) fn operation(&self) -> EditSessionOperation {
        match self {
            Self::State(source) => source.operation(),
            Self::Identity { operation, .. }
            | Self::Acquire { operation, .. }
            | Self::AcquireCleanup { operation, .. }
            | Self::Validation { operation, .. }
            | Self::Release { operation, .. }
            | Self::ProjectMismatch { operation }
            | Self::TargetMismatch { operation }
            | Self::InternalInvariant { operation } => *operation,
        }
    }

    pub(crate) fn release_error(&self) -> Option<&LockSetReleaseError> {
        match self {
            Self::AcquireCleanup { cleanup_error, .. } => Some(cleanup_error),
            Self::Release { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl fmt::Display for EditSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(source) => source.fmt(formatter),
            Self::Identity { operation, .. } => {
                write!(
                    formatter,
                    "invalid edit session identity while attempting to {operation}"
                )
            }
            Self::Acquire { operation, .. } => {
                write!(formatter, "edit session lock acquisition failed during {operation}")
            }
            Self::AcquireCleanup { operation, .. } => write!(
                formatter,
                "edit session lock acquisition failed during {operation} and partial cleanup must be retried"
            ),
            Self::Validation {
                operation,
                category,
                ..
            } => write!(
                formatter,
                "edit session validation failed during {operation} ({category:?})"
            ),
            Self::Release { operation, .. } => {
                write!(
                    formatter,
                    "edit session lock release failed during {operation}"
                )
            }
            Self::ProjectMismatch { operation } => {
                write!(formatter, "edit session project differs during {operation}")
            }
            Self::TargetMismatch { operation } => {
                write!(
                    formatter,
                    "edit session target set differs during {operation}"
                )
            }
            Self::InternalInvariant { operation } => write!(
                formatter,
                "edit session internal invariant failed during {operation}"
            ),
        }
    }
}

impl Error for EditSessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::State(source) => Some(source),
            Self::Identity { source, .. } => Some(source),
            Self::Acquire { source, .. } => Some(source),
            Self::AcquireCleanup { acquire_error, .. } => Some(acquire_error),
            Self::Validation { source, .. } => match source {
                ValidationErrorSource::Lock(source) => Some(source),
                ValidationErrorSource::Permit(source) => Some(source),
            },
            Self::Release { source, .. } => Some(source),
            Self::ProjectMismatch { .. }
            | Self::TargetMismatch { .. }
            | Self::InternalInvariant { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveryHandoffError {
    sink_category: RecoverySinkFailureCategory,
}

impl RecoveryHandoffError {
    pub(crate) fn category(&self) -> EditSessionErrorCategory {
        EditSessionErrorCategory::RecoveryHandoffFailed
    }

    pub(crate) fn sink_category(&self) -> RecoverySinkFailureCategory {
        self.sink_category
    }
}

impl fmt::Display for RecoveryHandoffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "durable edit recovery acceptance failed ({:?})",
            self.sink_category
        )
    }
}

impl Error for RecoveryHandoffError {}

/// application/transaction 결과와 그 호출 뒤의 lock 상태를 함께 돌려준다.
/// `T`와 `E`는 service가 해석하거나 표준 오류 chain에 연결하지 않는다.
pub(crate) struct WriteOperationOutcome<T, E> {
    operation_result: Result<T, E>,
    session_state: EditSessionState,
    validation_failure: Option<ValidationFailure>,
}

impl<T, E> WriteOperationOutcome<T, E> {
    pub(crate) fn operation_result(&self) -> &Result<T, E> {
        &self.operation_result
    }

    pub(crate) fn session_state(&self) -> EditSessionState {
        self.session_state
    }

    pub(crate) fn validation_failure(&self) -> Option<ValidationFailureRef<'_>> {
        self.validation_failure
            .as_ref()
            .map(ValidationFailure::as_ref)
    }

    pub(crate) fn into_operation_result(self) -> Result<T, E> {
        self.operation_result
    }
}

impl<T, E> fmt::Debug for WriteOperationOutcome<T, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WriteOperationOutcome")
            .field(
                "operation_result",
                &if self.operation_result.is_ok() {
                    "Ok([opaque])"
                } else {
                    "Err([opaque])"
                },
            )
            .field("session_state", &self.session_state)
            .field(
                "validation_failure",
                &self
                    .validation_failure
                    .as_ref()
                    .map(ValidationFailure::category),
            )
            .finish()
    }
}

#[derive(Clone)]
struct SessionContext {
    project_fingerprint: String,
    session_id: LockSessionId,
    provider_kind: LockProviderKind,
    targets: Vec<ProjectRelativePath>,
}

impl SessionContext {
    fn from_guard(guard: &LockSetGuard) -> Self {
        Self {
            project_fingerprint: guard.project_fingerprint().to_owned(),
            session_id: guard.session_id().clone(),
            provider_kind: guard.provider_kind(),
            targets: guard.targets().cloned().collect(),
        }
    }

    fn resume_request(&self) -> ResumeRequest {
        ResumeRequest {
            project_fingerprint: self.project_fingerprint.clone(),
            targets: self.targets.clone(),
        }
    }

    fn envelope(&self, reason: RecoveryReason) -> RecoveryEnvelope {
        RecoveryEnvelope {
            project_fingerprint: self.project_fingerprint.clone(),
            session_id: self.session_id.clone(),
            provider_kind: self.provider_kind,
            targets: self.targets.clone(),
            reason,
        }
    }
}

#[derive(Clone)]
struct ResumeRequest {
    project_fingerprint: String,
    targets: Vec<ProjectRelativePath>,
}

enum SessionData<P, R> {
    ReadOnly {
        resume: Option<ResumeRequest>,
    },
    Acquiring(SessionContext),
    Held(HeldSession<P, R>),
    PartialCleanup(PartialCleanupSession),
    RecoveryReleased {
        context: SessionContext,
        recovery: RecoveryData<P, R>,
    },
}

struct HeldSession<P, R> {
    context: SessionContext,
    guard: LockSetGuard,
    phase: HeldPhase<P, R>,
}

enum HeldPhase<P, R> {
    Editing,
    LockLost(ValidationFailure),
    RecoveryRequired(RecoveryData<P, R>),
    Releasing {
        intent: ReleaseIntent<P, R>,
        diagnostics: Option<ReleaseDiagnostics>,
    },
    ReleaseFailed {
        intent: ReleaseIntent<P, R>,
        diagnostics: ReleaseDiagnostics,
    },
    /// payload를 포함한 phase를 move하는 아주 짧은 내부 구간이다. 외부 provider 호출
    /// 전에 반드시 다른 fail-closed phase로 바뀐다.
    Transitioning,
}

struct ReleaseIntent<P, R> {
    after_release: AfterRelease<P, R>,
    validation_failure: Option<ValidationFailure>,
}

enum AfterRelease<P, R> {
    ReadOnly,
    Recovery(RecoveryData<P, R>),
}

struct PartialCleanupSession {
    context: SessionContext,
    cleanup: PartialAcquireCleanup,
    acquire_error: LockError,
    phase: PartialCleanupPhase,
    diagnostics: ReleaseDiagnostics,
}

#[derive(Clone, Copy)]
enum PartialCleanupPhase {
    Releasing,
    ReleaseFailed,
}

struct RecoveryData<P, R> {
    reason: RecoveryReason,
    payload: RecoveryPayloadState<P, R>,
    validation_failure: Option<ValidationFailure>,
    release_diagnostics: Option<ReleaseDiagnostics>,
}

enum RecoveryPayloadState<P, R> {
    Pending(P),
    InProgress,
    DurablyAccepted(R),
    Acknowledged,
    Discarded,
}

struct ReleaseDiagnostics {
    first: LockSetReleaseError,
    latest: LockSetReleaseError,
    retry_attempts: usize,
}

impl ReleaseDiagnostics {
    fn new(error: LockSetReleaseError) -> Self {
        Self {
            first: error.clone(),
            latest: error,
            retry_attempts: 0,
        }
    }

    fn record_retry(&mut self, error: LockSetReleaseError) {
        self.latest = error;
        self.retry_attempts = self.retry_attempts.saturating_add(1);
    }

    fn failure_count(&self) -> usize {
        self.retry_attempts.saturating_add(1)
    }

    fn as_ref(&self) -> ReleaseFailureDiagnostics<'_> {
        ReleaseFailureDiagnostics {
            first: &self.first,
            latest: &self.latest,
            retry_attempts: self.retry_attempts,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReleaseFailureDiagnostics<'a> {
    first: &'a LockSetReleaseError,
    latest: &'a LockSetReleaseError,
    retry_attempts: usize,
}

impl<'a> ReleaseFailureDiagnostics<'a> {
    pub(crate) fn first(self) -> &'a LockSetReleaseError {
        self.first
    }

    pub(crate) fn latest(self) -> &'a LockSetReleaseError {
        self.latest
    }

    pub(crate) fn retry_attempts(self) -> usize {
        self.retry_attempts
    }

    pub(crate) fn failure_count(self) -> usize {
        self.retry_attempts.saturating_add(1)
    }
}

/// M1 lock 자원의 소유권과 편집 가능 상태를 하나의 동기 domain service로 조율한다.
/// provider 호출은 blocking일 수 있으므로 M2-6 adapter가 UI thread 밖에서 호출해야 한다.
pub(crate) struct EditSessionService<P, R> {
    coordinator: LockCoordinator,
    configured_provider: LockProviderKind,
    session: SessionData<P, R>,
}

impl<P, R> EditSessionService<P, R> {
    pub(crate) fn new(service: Arc<dyn LockService>) -> Self {
        let coordinator = LockCoordinator::new(service);
        let configured_provider = coordinator.provider_kind();
        Self {
            coordinator,
            configured_provider,
            session: SessionData::ReadOnly { resume: None },
        }
    }

    pub(crate) fn snapshot(&self) -> EditSessionSnapshot {
        match &self.session {
            SessionData::ReadOnly { .. } => empty_snapshot(EditSessionState::ReadOnly),
            SessionData::Acquiring(context) => {
                context_snapshot(EditSessionState::Acquiring, context, None, 0)
            }
            SessionData::Held(held) => held.snapshot(),
            SessionData::PartialCleanup(cleanup) => context_snapshot(
                match cleanup.phase {
                    PartialCleanupPhase::Releasing => EditSessionState::Releasing,
                    PartialCleanupPhase::ReleaseFailed => EditSessionState::ReleaseFailed,
                },
                &cleanup.context,
                None,
                cleanup.diagnostics.failure_count(),
            ),
            SessionData::RecoveryReleased { context, recovery } => context_snapshot(
                EditSessionState::RecoveryRequired,
                context,
                Some(recovery.payload.status()),
                recovery
                    .release_diagnostics
                    .as_ref()
                    .map(ReleaseDiagnostics::failure_count)
                    .unwrap_or(0),
            ),
        }
    }

    /// RecoveryRequired에는 해제 전후가 모두 포함된다. 종료 조정자는 실제 owner를 조회한다.
    /// guard를 빌려주거나 재획득 권한을 만들지 않는다.
    pub(crate) fn has_release_work(&self) -> bool {
        matches!(
            self.session,
            SessionData::Held(_) | SessionData::PartialCleanup(_)
        )
    }

    pub(crate) fn begin_edit(
        &mut self,
        project_fingerprint: &str,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<(), EditSessionError> {
        self.acquire_for(
            EditSessionOperation::BeginEdit,
            project_fingerprint,
            targets,
        )
    }

    fn acquire_for(
        &mut self,
        operation: EditSessionOperation,
        project_fingerprint: &str,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<(), EditSessionError> {
        self.require_state(operation, EditSessionState::ReadOnly)?;
        let session_id = LockSessionId::generate()
            .map_err(|source| EditSessionError::Identity { operation, source })?;
        let request = self
            .coordinator
            .preflight(project_fingerprint, &session_id, targets)
            .map_err(|source| preflight_error(operation, source))?;
        self.acquire_validated(operation, request)
    }

    fn acquire_validated(
        &mut self,
        operation: EditSessionOperation,
        request: ValidatedLockSetRequest,
    ) -> Result<(), EditSessionError> {
        let attempt = SessionContext {
            project_fingerprint: request.project_fingerprint().to_owned(),
            session_id: request.session_id().clone(),
            provider_kind: self.configured_provider,
            targets: request.targets().to_vec(),
        };
        self.session = SessionData::Acquiring(attempt.clone());

        match self.coordinator.acquire_validated(request) {
            Ok(guard) => {
                let context = SessionContext::from_guard(&guard);
                self.session = SessionData::Held(HeldSession {
                    context,
                    guard,
                    phase: HeldPhase::Editing,
                });
                Ok(())
            }
            Err(error) => Err(self.finish_failed_acquire(attempt, error, operation)),
        }
    }

    pub(crate) fn run_validated_write<T, E, F>(
        &mut self,
        project_fingerprint: &str,
        targets: &[ProjectRelativePath],
        write: F,
    ) -> Result<WriteOperationOutcome<T, E>, EditSessionError>
    where
        F: for<'permit> FnOnce(WritePermit<'permit>) -> Result<T, E>,
    {
        let operation = EditSessionOperation::ValidateForSave;
        let current = self.snapshot().state();
        let SessionData::Held(held) = &mut self.session else {
            return Err(state_error(operation, current));
        };
        if !matches!(held.phase, HeldPhase::Editing) {
            return Err(state_error(operation, held.public_state()));
        }
        if held.context.project_fingerprint != project_fingerprint {
            return Err(EditSessionError::ProjectMismatch { operation });
        }
        if held.context.targets != targets {
            return Err(EditSessionError::TargetMismatch { operation });
        }

        let permit = match held.guard.write_permit() {
            Ok(permit) => permit,
            Err(source) => {
                let failure = held
                    .guard
                    .validation_failure()
                    .cloned()
                    .map(|source| ValidationFailure::Save(WritePermitError::Validation(source)))
                    .unwrap_or_else(|| ValidationFailure::Save(source.clone()));
                let category = failure.category();
                held.phase = HeldPhase::LockLost(failure);
                return Err(EditSessionError::Validation {
                    operation,
                    category,
                    source: ValidationErrorSource::Permit(source),
                });
            }
        };

        // permit이 closure를 벗어나기 전에 소비되므로 caller의 별도 알림 없이도
        // transaction 내부 validation 결과를 즉시 session phase와 맞출 수 있다.
        let operation_result = write(permit);
        let validation_failure = held
            .guard
            .validation_failure()
            .cloned()
            .map(|source| ValidationFailure::Save(WritePermitError::Validation(source)));
        if let Some(failure) = &validation_failure {
            held.phase = HeldPhase::LockLost(failure.clone());
        }
        Ok(WriteOperationOutcome {
            operation_result,
            session_state: held.public_state(),
            validation_failure,
        })
    }

    pub(crate) fn revalidate(&mut self) -> Result<(), EditSessionError> {
        let operation = EditSessionOperation::Revalidate;
        let current = self.snapshot().state();
        let SessionData::Held(held) = &mut self.session else {
            return Err(state_error(operation, current));
        };
        if !matches!(held.phase, HeldPhase::Editing) {
            return Err(state_error(operation, held.public_state()));
        }
        match held.guard.validate_all() {
            Ok(()) => Ok(()),
            Err(source) => {
                let failure = ValidationFailure::Revalidate(source.clone());
                let category = failure.category();
                held.phase = HeldPhase::LockLost(failure);
                Err(EditSessionError::Validation {
                    operation,
                    category,
                    source: ValidationErrorSource::Lock(source),
                })
            }
        }
    }

    /// 정상 편집의 명시적 보관. 실패 상태를 만들지 않고 실제 sink에 같은 P를 넘긴다.
    pub(crate) fn deposit_active<S: DurableRecoverySink<P, Receipt = R>>(
        &mut self,
        payload: P,
        sink: &mut S,
    ) -> Result<R, RecoverySinkFailure<P>> {
        let SessionData::Held(held) = &self.session else {
            return Err(RecoverySinkFailure::new(
                payload,
                RecoverySinkFailureCategory::Rejected,
            ));
        };
        if !matches!(held.phase, HeldPhase::Editing) {
            return Err(RecoverySinkFailure::new(
                payload,
                RecoverySinkFailureCategory::Rejected,
            ));
        }
        sink.accept_durably(
            &held.context.envelope(RecoveryReason::ExplicitDraftDeposit),
            payload,
        )
    }
    pub(crate) fn preserve_for_recovery(
        &mut self,
        payload: P,
    ) -> Result<(), PreserveRecoveryFailure<P>> {
        let operation = EditSessionOperation::PreserveRecovery;
        let current = self.snapshot().state();
        let SessionData::Held(held) = &mut self.session else {
            return Err(PreserveRecoveryFailure::new(
                payload,
                state_error(operation, current),
            ));
        };
        let (reason, validation_failure) = match &held.phase {
            HeldPhase::Editing => (RecoveryReason::SaveRecoveryRequired, None),
            HeldPhase::LockLost(failure) => (recovery_reason(failure), Some(failure.clone())),
            _ => {
                return Err(PreserveRecoveryFailure::new(
                    payload,
                    state_error(operation, held.public_state()),
                ))
            }
        };
        held.phase = HeldPhase::RecoveryRequired(RecoveryData {
            reason,
            payload: RecoveryPayloadState::Pending(payload),
            validation_failure,
            release_diagnostics: None,
        });
        Ok(())
    }

    pub(crate) fn accept_recovery_durably<S>(
        &mut self,
        sink: &mut S,
    ) -> Result<(), RecoveryHandoffCallError>
    where
        S: DurableRecoverySink<P, Receipt = R>,
    {
        let operation = EditSessionOperation::AcceptRecoveryDurably;
        let current = self.snapshot().state();
        let (envelope, payload, released) = match &mut self.session {
            SessionData::Held(held) => {
                let context = held.context.clone();
                let Some(recovery) = held.phase.recovery_mut() else {
                    return Err(RecoveryHandoffCallError::Session(state_error(
                        operation,
                        held.public_state(),
                    )));
                };
                let envelope = context.envelope(recovery.reason);
                let payload = recovery.payload.take_pending().map_err(|()| {
                    RecoveryHandoffCallError::Session(state_error(operation, current))
                })?;
                (envelope, payload, false)
            }
            SessionData::RecoveryReleased { context, recovery } => {
                let envelope = context.envelope(recovery.reason);
                let payload = recovery.payload.take_pending().map_err(|()| {
                    RecoveryHandoffCallError::Session(state_error(operation, current))
                })?;
                (envelope, payload, true)
            }
            _ => {
                return Err(RecoveryHandoffCallError::Session(state_error(
                    operation, current,
                )))
            }
        };

        match sink.accept_durably(&envelope, payload) {
            Ok(receipt) => {
                self.finish_durable_recovery_acceptance(released, receipt, operation)
                    .map_err(RecoveryHandoffCallError::Session)?;
                Ok(())
            }
            Err(failure) => {
                let (payload, sink_category) = failure.into_parts();
                self.restore_recovery_payload(payload, operation)
                    .map_err(RecoveryHandoffCallError::Session)?;
                Err(RecoveryHandoffCallError::Sink(RecoveryHandoffError {
                    sink_category,
                }))
            }
        }
    }

    /// durable custody가 확인된 receipt를 상위 lifecycle에 넘기고 recovery를 승인한다.
    /// lock이 남아 있으면 release 완료 전까지 편집 가능 상태로 돌아가지 않는다.
    pub(crate) fn acknowledge_recovery(&mut self) -> Result<R, EditSessionError> {
        let operation = EditSessionOperation::AcknowledgeRecovery;
        let current = self.snapshot().state();
        match &mut self.session {
            SessionData::Held(held) => {
                let state = held.public_state();
                let Some(recovery) = held.phase.recovery_mut() else {
                    return Err(state_error(operation, state));
                };
                recovery
                    .payload
                    .take_durable_receipt()
                    .map_err(|()| state_error(operation, current))
            }
            SessionData::RecoveryReleased { context, recovery } => {
                let receipt = recovery
                    .payload
                    .take_durable_receipt()
                    .map_err(|()| state_error(operation, current))?;
                let resume = Some(context.resume_request());
                self.session = SessionData::ReadOnly { resume };
                Ok(receipt)
            }
            _ => Err(state_error(operation, current)),
        }
    }

    /// 명시적 폐기는 실제 P의 소유자를 대조한다. 보관 proof나 acknowledge로 위장하지 않는다.
    pub(crate) fn discard_recovery(
        &mut self,
        admits: impl FnOnce(&P) -> bool,
    ) -> Result<P, EditSessionError> {
        let operation = EditSessionOperation::DiscardRecovery;
        let current = self.snapshot().state();
        let recovery = match &mut self.session {
            SessionData::Held(held) => held.phase.recovery_mut(),
            SessionData::RecoveryReleased { recovery, .. } => Some(recovery),
            _ => None,
        }
        .ok_or_else(|| state_error(operation, current))?;
        if !matches!(&recovery.payload, RecoveryPayloadState::Pending(p) if admits(p)) {
            return Err(state_error(operation, current));
        }
        let payload = recovery
            .payload
            .take_pending()
            .map_err(|()| state_error(operation, current))?;
        recovery.payload = RecoveryPayloadState::Discarded;
        if let SessionData::RecoveryReleased { context, .. } = &self.session {
            self.session = SessionData::ReadOnly {
                resume: Some(context.resume_request()),
            };
        }
        Ok(payload)
    }

    pub(crate) fn end_edit(&mut self) -> Result<(), EditSessionError> {
        self.release_guard(EditSessionOperation::EndEdit, false)
    }

    pub(crate) fn retry_release(&mut self) -> Result<(), EditSessionError> {
        let operation = EditSessionOperation::RetryRelease;
        match &mut self.session {
            SessionData::PartialCleanup(cleanup) => {
                if !matches!(cleanup.phase, PartialCleanupPhase::ReleaseFailed) {
                    return Err(state_error(operation, EditSessionState::Releasing));
                }
                cleanup.phase = PartialCleanupPhase::Releasing;
                match cleanup.cleanup.release_remaining() {
                    Ok(()) => {
                        let resume = cleanup.context.resume_request();
                        self.session = SessionData::ReadOnly {
                            resume: Some(resume),
                        };
                        Ok(())
                    }
                    Err(source) => {
                        cleanup.phase = PartialCleanupPhase::ReleaseFailed;
                        cleanup.diagnostics.record_retry(source.clone());
                        Err(EditSessionError::Release { operation, source })
                    }
                }
            }
            SessionData::Held(held) if matches!(held.phase, HeldPhase::ReleaseFailed { .. }) => {
                self.release_guard(operation, true)
            }
            _ => Err(state_error(operation, self.snapshot().state())),
        }
    }

    pub(crate) fn change_targets(
        &mut self,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<(), EditSessionError> {
        let operation = EditSessionOperation::ChangeTargets;
        let project_fingerprint = match &self.session {
            SessionData::Held(held)
                if matches!(held.phase, HeldPhase::Editing | HeldPhase::LockLost(_)) =>
            {
                held.context.project_fingerprint.clone()
            }
            _ => return Err(state_error(operation, self.snapshot().state())),
        };
        let session_id = LockSessionId::generate()
            .map_err(|source| EditSessionError::Identity { operation, source })?;
        let request = self
            .coordinator
            .preflight(&project_fingerprint, &session_id, targets)
            .map_err(|source| preflight_error(operation, source))?;
        self.release_guard(operation, false)?;
        self.acquire_validated(operation, request)
    }

    pub(crate) fn resume_edit(&mut self) -> Result<(), EditSessionError> {
        let operation = EditSessionOperation::ResumeEdit;
        let (resume, release_first) = match &self.session {
            SessionData::Held(held) if matches!(held.phase, HeldPhase::LockLost(_)) => {
                (held.context.resume_request(), true)
            }
            SessionData::ReadOnly {
                resume: Some(request),
            } => (request.clone(), false),
            _ => return Err(state_error(operation, self.snapshot().state())),
        };
        let session_id = LockSessionId::generate()
            .map_err(|source| EditSessionError::Identity { operation, source })?;
        let request = self
            .coordinator
            .preflight(&resume.project_fingerprint, &session_id, resume.targets)
            .map_err(|source| preflight_error(operation, source))?;
        if release_first {
            self.release_guard(operation, false)?;
        }
        self.acquire_validated(operation, request)
    }

    pub(crate) fn retained_failure(&self) -> Option<RetainedFailure<'_>> {
        match &self.session {
            SessionData::PartialCleanup(cleanup) => Some(RetainedFailure::PartialAcquire {
                acquire_error: &cleanup.acquire_error,
                release_diagnostics: cleanup.diagnostics.as_ref(),
            }),
            SessionData::Held(held) => held.retained_failure(),
            SessionData::RecoveryReleased { recovery, .. } => Some(RetainedFailure::Recovery {
                reason: recovery.reason,
                validation_failure: recovery
                    .validation_failure
                    .as_ref()
                    .map(ValidationFailure::as_ref),
                release_diagnostics: recovery
                    .release_diagnostics
                    .as_ref()
                    .map(ReleaseDiagnostics::as_ref),
            }),
            _ => None,
        }
    }

    fn require_state(
        &self,
        operation: EditSessionOperation,
        expected: EditSessionState,
    ) -> Result<(), EditSessionError> {
        let current = self.snapshot().state();
        if current == expected {
            Ok(())
        } else {
            Err(state_error(operation, current))
        }
    }

    fn finish_failed_acquire(
        &mut self,
        attempt: SessionContext,
        error: LockCoordinatorError,
        operation: EditSessionOperation,
    ) -> EditSessionError {
        let resume = Some(attempt.resume_request());
        match error {
            LockCoordinatorError::InvalidIdentity(source) => {
                // validated request에서 이 오류가 재발하면 request invariant 위반이다.
                self.session = SessionData::ReadOnly { resume: None };
                EditSessionError::Identity { operation, source }
            }
            LockCoordinatorError::EmptyTargetSet(source)
            | LockCoordinatorError::DuplicateTarget(source) => {
                self.session = SessionData::ReadOnly { resume: None };
                EditSessionError::Acquire { operation, source }
            }
            LockCoordinatorError::ValidatedRequestMismatch(source) => {
                self.session = SessionData::ReadOnly { resume: None };
                EditSessionError::Acquire { operation, source }
            }
            LockCoordinatorError::AcquireFailed {
                acquire_error,
                release_error,
                cleanup,
            } => match release_error {
                Some(release_error) => {
                    let release_error = *release_error;
                    let returned_acquire = acquire_error.clone();
                    let returned_release = release_error.clone();
                    self.session = SessionData::PartialCleanup(PartialCleanupSession {
                        context: attempt,
                        cleanup: *cleanup,
                        acquire_error,
                        phase: PartialCleanupPhase::ReleaseFailed,
                        diagnostics: ReleaseDiagnostics::new(release_error),
                    });
                    EditSessionError::AcquireCleanup {
                        operation,
                        acquire_error: returned_acquire,
                        cleanup_error: returned_release,
                    }
                }
                None => {
                    self.session = SessionData::ReadOnly { resume };
                    EditSessionError::Acquire {
                        operation,
                        source: acquire_error,
                    }
                }
            },
        }
    }

    fn release_guard(
        &mut self,
        operation: EditSessionOperation,
        retry: bool,
    ) -> Result<(), EditSessionError> {
        let completion = {
            let current = self.snapshot().state();
            let SessionData::Held(held) = &mut self.session else {
                return Err(state_error(operation, current));
            };
            let (intent, diagnostics) = held.take_release_intent(operation, retry)?;
            held.phase = HeldPhase::Releasing {
                intent,
                diagnostics,
            };
            match held.guard.release_all() {
                Ok(()) => {
                    let HeldPhase::Releasing {
                        intent,
                        diagnostics,
                    } = std::mem::replace(&mut held.phase, HeldPhase::Transitioning)
                    else {
                        return Err(EditSessionError::InternalInvariant { operation });
                    };
                    Some((held.context.clone(), intent, diagnostics))
                }
                Err(source) => {
                    let HeldPhase::Releasing {
                        intent,
                        diagnostics,
                    } = std::mem::replace(&mut held.phase, HeldPhase::Transitioning)
                    else {
                        return Err(EditSessionError::InternalInvariant { operation });
                    };
                    let diagnostics = match diagnostics {
                        Some(mut diagnostics) => {
                            diagnostics.record_retry(source.clone());
                            diagnostics
                        }
                        None => ReleaseDiagnostics::new(source.clone()),
                    };
                    held.phase = HeldPhase::ReleaseFailed {
                        intent,
                        diagnostics,
                    };
                    return Err(EditSessionError::Release { operation, source });
                }
            }
        };

        let Some((context, intent, diagnostics)) = completion else {
            return Err(EditSessionError::InternalInvariant { operation });
        };
        let resume = Some(context.resume_request());
        self.session = match intent.after_release {
            AfterRelease::ReadOnly => SessionData::ReadOnly { resume },
            AfterRelease::Recovery(mut recovery) => {
                recovery.release_diagnostics = diagnostics;
                match recovery.payload {
                    RecoveryPayloadState::Acknowledged | RecoveryPayloadState::Discarded => {
                        SessionData::ReadOnly { resume }
                    }
                    RecoveryPayloadState::Pending(_)
                    | RecoveryPayloadState::InProgress
                    | RecoveryPayloadState::DurablyAccepted(_) => {
                        SessionData::RecoveryReleased { context, recovery }
                    }
                }
            }
        };
        Ok(())
    }

    fn finish_durable_recovery_acceptance(
        &mut self,
        released: bool,
        receipt: R,
        operation: EditSessionOperation,
    ) -> Result<(), EditSessionError> {
        if released {
            let SessionData::RecoveryReleased { recovery, .. } = &mut self.session else {
                return Err(EditSessionError::InternalInvariant { operation });
            };
            recovery
                .payload
                .mark_durably_accepted(receipt)
                .map_err(|()| EditSessionError::InternalInvariant { operation })?;
        } else {
            let SessionData::Held(held) = &mut self.session else {
                return Err(EditSessionError::InternalInvariant { operation });
            };
            let Some(recovery) = held.phase.recovery_mut() else {
                return Err(EditSessionError::InternalInvariant { operation });
            };
            recovery
                .payload
                .mark_durably_accepted(receipt)
                .map_err(|()| EditSessionError::InternalInvariant { operation })?;
        }
        Ok(())
    }

    fn restore_recovery_payload(
        &mut self,
        payload: P,
        operation: EditSessionOperation,
    ) -> Result<(), EditSessionError> {
        let recovery = match &mut self.session {
            SessionData::Held(held) => held.phase.recovery_mut(),
            SessionData::RecoveryReleased { recovery, .. } => Some(recovery),
            _ => None,
        }
        .ok_or(EditSessionError::InternalInvariant { operation })?;
        recovery
            .payload
            .restore(payload)
            .map_err(|()| EditSessionError::InternalInvariant { operation })
    }
}

/// handoff 호출 자체의 상태 오류와 sink 오류를 분리한다.
pub(crate) enum RecoveryHandoffCallError {
    Session(EditSessionError),
    Sink(RecoveryHandoffError),
}

impl RecoveryHandoffCallError {
    pub(crate) fn category(&self) -> EditSessionErrorCategory {
        match self {
            Self::Session(source) => source.category(),
            Self::Sink(source) => source.category(),
        }
    }
}

impl fmt::Debug for RecoveryHandoffCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(source) => formatter.debug_tuple("Session").field(source).finish(),
            Self::Sink(source) => formatter.debug_tuple("Sink").field(source).finish(),
        }
    }
}

impl fmt::Display for RecoveryHandoffCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(source) => source.fmt(formatter),
            Self::Sink(source) => source.fmt(formatter),
        }
    }
}

impl Error for RecoveryHandoffCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Session(source) => Some(source),
            Self::Sink(_) => None,
        }
    }
}

impl<P, R> HeldSession<P, R> {
    fn public_state(&self) -> EditSessionState {
        match self.phase {
            HeldPhase::Editing => EditSessionState::Editing,
            HeldPhase::LockLost(_) => EditSessionState::LockLost,
            HeldPhase::RecoveryRequired(_) => EditSessionState::RecoveryRequired,
            HeldPhase::Releasing { .. } => EditSessionState::Releasing,
            HeldPhase::ReleaseFailed { .. } | HeldPhase::Transitioning => {
                EditSessionState::ReleaseFailed
            }
        }
    }

    fn snapshot(&self) -> EditSessionSnapshot {
        let (handoff, failures) = match &self.phase {
            HeldPhase::RecoveryRequired(recovery) => (Some(recovery.payload.status()), 0),
            HeldPhase::Releasing {
                intent,
                diagnostics,
            } => (
                intent.recovery_status(),
                diagnostics
                    .as_ref()
                    .map(ReleaseDiagnostics::failure_count)
                    .unwrap_or(0),
            ),
            HeldPhase::ReleaseFailed {
                intent,
                diagnostics,
            } => (intent.recovery_status(), diagnostics.failure_count()),
            _ => (None, 0),
        };
        context_snapshot(self.public_state(), &self.context, handoff, failures)
    }

    fn take_release_intent(
        &mut self,
        operation: EditSessionOperation,
        retry: bool,
    ) -> Result<(ReleaseIntent<P, R>, Option<ReleaseDiagnostics>), EditSessionError> {
        let phase = std::mem::replace(&mut self.phase, HeldPhase::Transitioning);
        match phase {
            HeldPhase::Editing if !retry => Ok((
                ReleaseIntent {
                    after_release: AfterRelease::ReadOnly,
                    validation_failure: None,
                },
                None,
            )),
            HeldPhase::LockLost(failure) if !retry => Ok((
                ReleaseIntent {
                    after_release: AfterRelease::ReadOnly,
                    validation_failure: Some(failure),
                },
                None,
            )),
            HeldPhase::RecoveryRequired(recovery) if !retry => Ok((
                ReleaseIntent {
                    after_release: AfterRelease::Recovery(recovery),
                    validation_failure: None,
                },
                None,
            )),
            HeldPhase::ReleaseFailed {
                intent,
                diagnostics,
            } if retry => Ok((intent, Some(diagnostics))),
            other => {
                let state = public_state_for_phase(&other);
                self.phase = other;
                Err(state_error(operation, state))
            }
        }
    }

    fn retained_failure(&self) -> Option<RetainedFailure<'_>> {
        match &self.phase {
            HeldPhase::LockLost(failure) => Some(RetainedFailure::Validation(failure.as_ref())),
            HeldPhase::RecoveryRequired(recovery) => Some(RetainedFailure::Recovery {
                reason: recovery.reason,
                validation_failure: recovery
                    .validation_failure
                    .as_ref()
                    .map(ValidationFailure::as_ref),
                release_diagnostics: recovery
                    .release_diagnostics
                    .as_ref()
                    .map(ReleaseDiagnostics::as_ref),
            }),
            HeldPhase::Releasing {
                intent,
                diagnostics,
            } => intent.retained_failure(diagnostics.as_ref()),
            HeldPhase::ReleaseFailed {
                intent,
                diagnostics,
            } => intent.retained_failure(Some(diagnostics)),
            _ => None,
        }
    }
}

impl<P, R> HeldPhase<P, R> {
    fn recovery_mut(&mut self) -> Option<&mut RecoveryData<P, R>> {
        match self {
            Self::RecoveryRequired(recovery) => Some(recovery),
            Self::Releasing { intent, .. } | Self::ReleaseFailed { intent, .. } => {
                intent.recovery_mut()
            }
            _ => None,
        }
    }
}

impl<P, R> ReleaseIntent<P, R> {
    fn recovery_mut(&mut self) -> Option<&mut RecoveryData<P, R>> {
        match &mut self.after_release {
            AfterRelease::Recovery(recovery) => Some(recovery),
            AfterRelease::ReadOnly => None,
        }
    }

    fn recovery_status(&self) -> Option<RecoveryHandoffStatus> {
        match &self.after_release {
            AfterRelease::Recovery(recovery) => Some(recovery.payload.status()),
            AfterRelease::ReadOnly => None,
        }
    }

    fn retained_failure<'a>(
        &'a self,
        release_diagnostics: Option<&'a ReleaseDiagnostics>,
    ) -> Option<RetainedFailure<'a>> {
        match &self.after_release {
            AfterRelease::Recovery(recovery) => Some(RetainedFailure::Recovery {
                reason: recovery.reason,
                validation_failure: recovery
                    .validation_failure
                    .as_ref()
                    .map(ValidationFailure::as_ref),
                release_diagnostics: release_diagnostics.map(ReleaseDiagnostics::as_ref),
            }),
            AfterRelease::ReadOnly => {
                if self.validation_failure.is_none() && release_diagnostics.is_none() {
                    None
                } else {
                    Some(RetainedFailure::Release {
                        validation_failure: self
                            .validation_failure
                            .as_ref()
                            .map(ValidationFailure::as_ref),
                        release_diagnostics: release_diagnostics.map(ReleaseDiagnostics::as_ref),
                    })
                }
            }
        }
    }
}

impl<P, R> RecoveryPayloadState<P, R> {
    fn status(&self) -> RecoveryHandoffStatus {
        match self {
            Self::Pending(_) => RecoveryHandoffStatus::Pending,
            Self::InProgress => RecoveryHandoffStatus::InProgress,
            Self::DurablyAccepted(_) => RecoveryHandoffStatus::DurablyAccepted,
            Self::Acknowledged => RecoveryHandoffStatus::Acknowledged,
            Self::Discarded => RecoveryHandoffStatus::Discarded,
        }
    }

    fn take_pending(&mut self) -> Result<P, ()> {
        match std::mem::replace(self, Self::InProgress) {
            Self::Pending(payload) => Ok(payload),
            other => {
                *self = other;
                Err(())
            }
        }
    }

    fn restore(&mut self, payload: P) -> Result<(), ()> {
        if matches!(self, Self::InProgress) {
            *self = Self::Pending(payload);
            Ok(())
        } else {
            Err(())
        }
    }

    fn mark_durably_accepted(&mut self, receipt: R) -> Result<(), ()> {
        if matches!(self, Self::InProgress) {
            *self = Self::DurablyAccepted(receipt);
            Ok(())
        } else {
            Err(())
        }
    }

    fn take_durable_receipt(&mut self) -> Result<R, ()> {
        match std::mem::replace(self, Self::Acknowledged) {
            Self::DurablyAccepted(receipt) => Ok(receipt),
            other => {
                *self = other;
                Err(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RetainedFailure<'a> {
    PartialAcquire {
        acquire_error: &'a LockError,
        release_diagnostics: ReleaseFailureDiagnostics<'a>,
    },
    Validation(ValidationFailureRef<'a>),
    Release {
        validation_failure: Option<ValidationFailureRef<'a>>,
        release_diagnostics: Option<ReleaseFailureDiagnostics<'a>>,
    },
    Recovery {
        reason: RecoveryReason,
        validation_failure: Option<ValidationFailureRef<'a>>,
        release_diagnostics: Option<ReleaseFailureDiagnostics<'a>>,
    },
}

fn recovery_reason(failure: &ValidationFailure) -> RecoveryReason {
    match failure.category() {
        EditSessionErrorCategory::LockLost => RecoveryReason::LockLost,
        EditSessionErrorCategory::LockStateUnknown => RecoveryReason::LockStateUnknown,
        _ => RecoveryReason::LockValidationFailed,
    }
}

fn preflight_error(
    operation: EditSessionOperation,
    error: LockRequestValidationError,
) -> EditSessionError {
    match error {
        LockRequestValidationError::InvalidIdentity(source) => {
            EditSessionError::Identity { operation, source }
        }
        LockRequestValidationError::EmptyTargetSet(source)
        | LockRequestValidationError::DuplicateTarget(source) => {
            EditSessionError::Acquire { operation, source }
        }
    }
}

fn public_state_for_phase<P, R>(phase: &HeldPhase<P, R>) -> EditSessionState {
    match phase {
        HeldPhase::Editing => EditSessionState::Editing,
        HeldPhase::LockLost(_) => EditSessionState::LockLost,
        HeldPhase::RecoveryRequired(_) => EditSessionState::RecoveryRequired,
        HeldPhase::Releasing { .. } => EditSessionState::Releasing,
        HeldPhase::ReleaseFailed { .. } | HeldPhase::Transitioning => {
            EditSessionState::ReleaseFailed
        }
    }
}

fn state_error(operation: EditSessionOperation, state: EditSessionState) -> EditSessionError {
    EditSessionError::State(EditSessionStateError { operation, state })
}

fn empty_snapshot(state: EditSessionState) -> EditSessionSnapshot {
    EditSessionSnapshot {
        state,
        project_fingerprint: None,
        session_id: None,
        provider_kind: None,
        targets: Vec::new(),
        recovery_handoff: None,
        release_failure_count: 0,
    }
}

fn context_snapshot(
    state: EditSessionState,
    context: &SessionContext,
    recovery_handoff: Option<RecoveryHandoffStatus>,
    release_failure_count: usize,
) -> EditSessionSnapshot {
    EditSessionSnapshot {
        state,
        project_fingerprint: Some(context.project_fingerprint.clone()),
        session_id: Some(context.session_id.clone()),
        provider_kind: Some(context.provider_kind),
        targets: context.targets.clone(),
        recovery_handoff,
        release_failure_count,
    }
}
