use std::fmt;
use std::sync::Arc;

use super::super::project_relative_path::ProjectRelativePath;
use super::model::{
    validate_project_fingerprint, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
    LockProviderKind, LockSessionId, LockSetIdentityError,
};
use super::service::{HeldLock, HeldLockState, LockService};

pub(crate) struct LockCoordinator {
    service: Arc<dyn LockService>,
    provider_kind: LockProviderKind,
    request_binding: Arc<()>,
}

/// provider를 호출하기 전에 identity와 exact target 집합을 모두 검증한 요청이다.
/// EditSession의 target 교체처럼 기존 잠금을 먼저 해제하면 안 되는 흐름도 같은
/// M1 검증 규칙을 재사용할 수 있도록 별도 타입으로 보존한다.
pub(crate) struct ValidatedLockSetRequest {
    coordinator_binding: Arc<()>,
    project_fingerprint: String,
    session_id: LockSessionId,
    targets: Vec<ProjectRelativePath>,
}

impl ValidatedLockSetRequest {
    pub(crate) fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    pub(crate) fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    pub(crate) fn targets(&self) -> &[ProjectRelativePath] {
        &self.targets
    }
}

impl LockCoordinator {
    pub(crate) fn new(service: Arc<dyn LockService>) -> Self {
        let provider_kind = service.provider_info().kind;
        Self {
            service,
            provider_kind,
            request_binding: Arc::new(()),
        }
    }

    pub(crate) fn provider_kind(&self) -> LockProviderKind {
        self.provider_kind
    }

    pub(crate) fn acquire_all(
        &self,
        project_fingerprint: &str,
        session_id: &LockSessionId,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<LockSetGuard, LockCoordinatorError> {
        let request = self
            .preflight(project_fingerprint, session_id, targets)
            .map_err(LockCoordinatorError::from)?;
        self.acquire_validated(request)
    }

    /// 외부 provider에 손대지 않는 순수 request preflight다.
    pub(crate) fn preflight(
        &self,
        project_fingerprint: &str,
        session_id: &LockSessionId,
        mut targets: Vec<ProjectRelativePath>,
    ) -> Result<ValidatedLockSetRequest, LockRequestValidationError> {
        validate_project_fingerprint(project_fingerprint)
            .map_err(LockRequestValidationError::InvalidIdentity)?;
        let provider = self.provider_kind;
        if targets.is_empty() {
            return Err(LockRequestValidationError::EmptyTargetSet(
                LockError::for_context(
                    LockErrorCategory::EmptyLockTargetSet,
                    provider,
                    LockOperation::Acquire,
                    Some(project_fingerprint),
                    Some(session_id),
                    None,
                ),
            ));
        }

        targets.sort();
        for pair in targets.windows(2) {
            if pair[0] == pair[1] {
                return Err(LockRequestValidationError::DuplicateTarget(
                    LockError::for_context(
                        LockErrorCategory::DuplicateLockTarget,
                        provider,
                        LockOperation::Acquire,
                        Some(project_fingerprint),
                        Some(session_id),
                        Some(&pair[1]),
                    ),
                ));
            }
        }

        // 향후 LockAcquireRequest의 identity 규칙이 늘어나도 provider 호출 전에 같은
        // constructor를 통과하도록 한다.
        for target in &targets {
            LockAcquireRequest::new(project_fingerprint, session_id, target)
                .map_err(LockRequestValidationError::InvalidIdentity)?;
        }

        Ok(ValidatedLockSetRequest {
            coordinator_binding: Arc::clone(&self.request_binding),
            project_fingerprint: project_fingerprint.to_owned(),
            session_id: session_id.clone(),
            targets,
        })
    }

    pub(crate) fn acquire_validated(
        &self,
        request: ValidatedLockSetRequest,
    ) -> Result<LockSetGuard, LockCoordinatorError> {
        if !Arc::ptr_eq(&self.request_binding, &request.coordinator_binding) {
            return Err(LockCoordinatorError::ValidatedRequestMismatch(
                LockError::for_context(
                    LockErrorCategory::HeldLockProviderMismatch,
                    self.provider_kind,
                    LockOperation::Acquire,
                    Some(request.project_fingerprint()),
                    Some(request.session_id()),
                    None,
                ),
            ));
        }
        let ValidatedLockSetRequest {
            coordinator_binding: _,
            project_fingerprint,
            session_id,
            targets,
        } = request;
        let provider = self.provider_kind;

        let mut handles = Vec::with_capacity(targets.len());
        let mut provider_instance_id = None;
        for target in &targets {
            let request = LockAcquireRequest::new(&project_fingerprint, &session_id, target)
                .map_err(LockCoordinatorError::InvalidIdentity)?;
            match self.service.acquire(request) {
                Ok(handle) => {
                    if handle.provider_kind() != provider
                        || handle.project_fingerprint() != project_fingerprint
                        || handle.session_id() != &session_id
                        || handle.target() != target
                        || handle.state() != HeldLockState::Active
                        || provider_instance_id
                            .is_some_and(|expected| expected != handle.provider_instance_id())
                    {
                        let identity_error = LockError::for_context(
                            LockErrorCategory::LockAcquireFailed,
                            provider,
                            LockOperation::Acquire,
                            Some(&project_fingerprint),
                            Some(&session_id),
                            Some(target),
                        );
                        handles.push(handle);
                        let mut cleanup = PartialAcquireCleanup::new(
                            Arc::clone(&self.service),
                            project_fingerprint.clone(),
                            session_id.clone(),
                            handles,
                        );
                        let release_error = cleanup.release_remaining().err();
                        return Err(LockCoordinatorError::AcquireFailed {
                            acquire_error: identity_error,
                            release_error: release_error.map(Box::new),
                            cleanup: Box::new(cleanup),
                        });
                    }
                    provider_instance_id = Some(handle.provider_instance_id());
                    handles.push(handle);
                }
                Err(acquire_error) => {
                    let mut cleanup = PartialAcquireCleanup::new(
                        Arc::clone(&self.service),
                        project_fingerprint.clone(),
                        session_id.clone(),
                        handles,
                    );
                    let release_error = cleanup.release_remaining().err();
                    return Err(LockCoordinatorError::AcquireFailed {
                        acquire_error,
                        release_error: release_error.map(Box::new),
                        cleanup: Box::new(cleanup),
                    });
                }
            }
        }

        Ok(LockSetGuard {
            service: Arc::clone(&self.service),
            project_fingerprint,
            session_id,
            provider_kind: provider,
            provider_instance_id: provider_instance_id
                .expect("non-empty successfully acquired lock set has a provider instance"),
            targets,
            handles,
            state: LockSetState::Active,
            validation_failure: None,
        })
    }
}

#[derive(Debug)]
pub(crate) enum LockRequestValidationError {
    InvalidIdentity(LockSetIdentityError),
    EmptyTargetSet(LockError),
    DuplicateTarget(LockError),
}

impl From<LockRequestValidationError> for LockCoordinatorError {
    fn from(error: LockRequestValidationError) -> Self {
        match error {
            LockRequestValidationError::InvalidIdentity(source) => Self::InvalidIdentity(source),
            LockRequestValidationError::EmptyTargetSet(source) => Self::EmptyTargetSet(source),
            LockRequestValidationError::DuplicateTarget(source) => Self::DuplicateTarget(source),
        }
    }
}

#[derive(Debug)]
pub(crate) enum LockCoordinatorError {
    InvalidIdentity(LockSetIdentityError),
    EmptyTargetSet(LockError),
    DuplicateTarget(LockError),
    ValidatedRequestMismatch(LockError),
    AcquireFailed {
        acquire_error: LockError,
        release_error: Option<Box<LockSetReleaseError>>,
        cleanup: Box<PartialAcquireCleanup>,
    },
}

impl LockCoordinatorError {
    pub(crate) fn category(&self) -> LockErrorCategory {
        match self {
            Self::InvalidIdentity(_) => LockErrorCategory::InvalidLockTarget,
            Self::EmptyTargetSet(error)
            | Self::DuplicateTarget(error)
            | Self::ValidatedRequestMismatch(error) => error.category(),
            Self::AcquireFailed { release_error, .. } => {
                if release_error.is_some() {
                    LockErrorCategory::PartialAcquireReleaseFailed
                } else {
                    LockErrorCategory::PartialAcquireRolledBack
                }
            }
        }
    }

    pub(crate) fn cleanup_mut(&mut self) -> Option<&mut PartialAcquireCleanup> {
        match self {
            Self::AcquireFailed { cleanup, .. } => Some(cleanup.as_mut()),
            _ => None,
        }
    }
}

impl fmt::Display for LockCoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(source) => write!(formatter, "invalid lock identity: {source}"),
            Self::EmptyTargetSet(source) | Self::DuplicateTarget(source) => source.fmt(formatter),
            Self::ValidatedRequestMismatch(_) => {
                formatter.write_str("validated lock request belongs to a different coordinator")
            }
            Self::AcquireFailed {
                acquire_error,
                release_error,
                ..
            } => {
                write!(formatter, "lock acquisition failed: {acquire_error}")?;
                if let Some(release_error) = release_error {
                    write!(
                        formatter,
                        "; partial acquire cleanup failed: {release_error}"
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LockCoordinatorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidIdentity(source) => Some(source),
            Self::EmptyTargetSet(source)
            | Self::DuplicateTarget(source)
            | Self::ValidatedRequestMismatch(source) => Some(source),
            Self::AcquireFailed { acquire_error, .. } => Some(acquire_error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockSetState {
    Active,
    ValidationFailed,
    Releasing,
    ReleaseFailed,
    Released,
}

pub(crate) struct LockSetGuard {
    service: Arc<dyn LockService>,
    project_fingerprint: String,
    session_id: LockSessionId,
    provider_kind: super::LockProviderKind,
    provider_instance_id: u64,
    targets: Vec<ProjectRelativePath>,
    handles: Vec<Box<dyn HeldLock>>,
    state: LockSetState,
    validation_failure: Option<LockError>,
}

impl LockSetGuard {
    pub(crate) fn write_permit(
        &mut self,
    ) -> Result<super::WritePermit<'_>, super::WritePermitError> {
        super::WritePermit::issue(self)
    }

    #[cfg(test)]
    pub(crate) fn into_test_write_permit(
        self,
    ) -> Result<super::WritePermit<'static>, super::WritePermitError> {
        super::WritePermit::issue_owned(Box::new(self))
    }

    pub(crate) fn provider_kind(&self) -> super::LockProviderKind {
        self.provider_kind
    }
    pub(crate) fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    pub(crate) fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    pub(crate) fn state(&self) -> LockSetState {
        self.state
    }

    pub(crate) fn validation_failure(&self) -> Option<&LockError> {
        self.validation_failure.as_ref()
    }

    /// F3는 이 상태가 true인 guard에만 permit 발급 경계를 추가해야 한다.
    pub(crate) fn is_write_eligible(&self) -> bool {
        self.state == LockSetState::Active
    }

    pub(crate) fn targets(&self) -> impl ExactSizeIterator<Item = &ProjectRelativePath> {
        self.targets.iter()
    }

    pub(crate) fn validate_all(&mut self) -> Result<(), LockError> {
        if self.state != LockSetState::Active {
            return Err(self.not_active(LockOperation::Validate));
        }
        for (handle, expected_target) in self.handles.iter_mut().zip(&self.targets) {
            if !handle_identity_matches(
                handle.as_ref(),
                self.provider_kind,
                self.provider_instance_id,
                &self.project_fingerprint,
                &self.session_id,
                expected_target,
            ) {
                self.state = LockSetState::ValidationFailed;
                let error = LockError::for_context(
                    LockErrorCategory::HeldLockProviderMismatch,
                    self.provider_kind,
                    LockOperation::Validate,
                    Some(&self.project_fingerprint),
                    Some(&self.session_id),
                    Some(expected_target),
                );
                self.validation_failure = Some(error.clone());
                return Err(error);
            }
            if let Err(error) = self.service.validate(handle.as_mut()) {
                self.state = LockSetState::ValidationFailed;
                self.validation_failure = Some(error.clone());
                return Err(error);
            }
            if !handle_identity_matches(
                handle.as_ref(),
                self.provider_kind,
                self.provider_instance_id,
                &self.project_fingerprint,
                &self.session_id,
                expected_target,
            ) {
                self.state = LockSetState::ValidationFailed;
                let error = LockError::for_context(
                    LockErrorCategory::HeldLockProviderMismatch,
                    self.provider_kind,
                    LockOperation::Validate,
                    Some(&self.project_fingerprint),
                    Some(&self.session_id),
                    Some(expected_target),
                );
                self.validation_failure = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }

    pub(crate) fn release_all(&mut self) -> Result<(), LockSetReleaseError> {
        if self.state == LockSetState::Released {
            return Err(LockSetReleaseError::single(
                self.not_active(LockOperation::Release),
            ));
        }
        if self.state == LockSetState::Releasing {
            return Err(LockSetReleaseError::single(
                self.not_active(LockOperation::Release),
            ));
        }
        self.state = LockSetState::Releasing;
        let errors = release_active_reverse(&self.service, &mut self.handles);
        if errors.is_empty() {
            self.state = LockSetState::Released;
            Ok(())
        } else {
            self.state = LockSetState::ReleaseFailed;
            Err(LockSetReleaseError { errors })
        }
    }

    fn not_active(&self, operation: LockOperation) -> LockError {
        LockError::for_context(
            LockErrorCategory::LockSetNotActive,
            self.service.provider_info().kind,
            operation,
            Some(&self.project_fingerprint),
            Some(&self.session_id),
            None,
        )
    }
}

fn handle_identity_matches(
    handle: &dyn HeldLock,
    provider_kind: super::LockProviderKind,
    provider_instance_id: u64,
    project_fingerprint: &str,
    session_id: &LockSessionId,
    target: &ProjectRelativePath,
) -> bool {
    handle.provider_kind() == provider_kind
        && handle.provider_instance_id() == provider_instance_id
        && handle.project_fingerprint() == project_fingerprint
        && handle.session_id() == session_id
        && handle.target() == target
        && handle.state() == HeldLockState::Active
}

impl fmt::Debug for LockSetGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LockSetGuard")
            .field("provider", &self.service.provider_info().kind)
            .field("project_fingerprint", &self.project_fingerprint)
            .field("session_id", &self.session_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

pub(crate) struct PartialAcquireCleanup {
    service: Arc<dyn LockService>,
    project_fingerprint: String,
    session_id: LockSessionId,
    handles: Vec<Box<dyn HeldLock>>,
    state: LockSetState,
}

impl PartialAcquireCleanup {
    fn new(
        service: Arc<dyn LockService>,
        project_fingerprint: String,
        session_id: LockSessionId,
        handles: Vec<Box<dyn HeldLock>>,
    ) -> Self {
        Self {
            service,
            project_fingerprint,
            session_id,
            handles,
            state: LockSetState::Active,
        }
    }

    pub(crate) fn state(&self) -> LockSetState {
        self.state
    }

    pub(crate) fn active_targets(&self) -> impl Iterator<Item = &ProjectRelativePath> {
        self.handles
            .iter()
            .filter(|handle| handle.state() == HeldLockState::Active)
            .map(|handle| handle.target())
    }

    pub(crate) fn release_remaining(&mut self) -> Result<(), LockSetReleaseError> {
        if self.state == LockSetState::Released {
            return Err(LockSetReleaseError::single(LockError::for_context(
                LockErrorCategory::LockSetNotActive,
                self.service.provider_info().kind,
                LockOperation::Release,
                Some(&self.project_fingerprint),
                Some(&self.session_id),
                None,
            )));
        }
        self.state = LockSetState::Releasing;
        let errors = release_active_reverse(&self.service, &mut self.handles);
        if errors.is_empty() {
            self.state = LockSetState::Released;
            Ok(())
        } else {
            self.state = LockSetState::ReleaseFailed;
            Err(LockSetReleaseError { errors })
        }
    }
}

impl fmt::Debug for PartialAcquireCleanup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PartialAcquireCleanup")
            .field("provider", &self.service.provider_info().kind)
            .field("project_fingerprint", &self.project_fingerprint)
            .field("session_id", &self.session_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LockSetReleaseError {
    pub(crate) errors: Vec<LockError>,
}

impl LockSetReleaseError {
    fn single(error: LockError) -> Self {
        Self {
            errors: vec![error],
        }
    }
}

impl fmt::Display for LockSetReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} lock release operation(s) failed",
            self.errors.len()
        )
    }
}

impl std::error::Error for LockSetReleaseError {}

fn release_active_reverse(
    service: &Arc<dyn LockService>,
    handles: &mut [Box<dyn HeldLock>],
) -> Vec<LockError> {
    let mut errors = Vec::new();
    for handle in handles.iter_mut().rev() {
        if handle.state() == HeldLockState::Active {
            if let Err(error) = service.release(handle.as_mut()) {
                errors.push(error);
            }
        }
    }
    errors
}
