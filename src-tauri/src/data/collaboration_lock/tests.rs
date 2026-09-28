use std::any::Any;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::data::project_relative_path::ProjectRelativePath;

const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static FAKE_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(10_000);

#[derive(Debug, Clone, Copy)]
enum FakeValidation {
    Valid,
    Lost,
    Unknown,
    ProviderFailure,
    MutateTarget,
    MutateProviderInstance,
    BecomeInactive,
}

#[derive(Debug, Default)]
struct FakeState {
    acquire_calls: Vec<String>,
    validate_calls: Vec<String>,
    release_calls: Vec<String>,
    acquire_failure: Option<String>,
    acquire_owner: Option<SafeOwnerIdentity>,
    validation: BTreeMap<String, FakeValidation>,
    release_failures_remaining: BTreeMap<String, usize>,
}

struct FakeLockService {
    instance_id: u64,
    info: LockProviderInfo,
    state: Mutex<FakeState>,
}

impl FakeLockService {
    fn new(kind: LockProviderKind) -> Self {
        Self {
            instance_id: FAKE_INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed),
            info: LockProviderInfo {
                kind,
                capabilities: LockCapabilities {
                    distributed: kind == LockProviderKind::Svn,
                    validation: true,
                    owner_diagnostics: true,
                    steal: kind == LockProviderKind::Svn,
                },
            },
            state: Mutex::new(FakeState::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fake provider mutex poisoned")
    }

    fn fail_acquire(&self, target: &str, owner: Option<SafeOwnerIdentity>) {
        let mut state = self.state();
        state.acquire_failure = Some(target.to_owned());
        state.acquire_owner = owner;
    }

    fn set_validation(&self, target: &str, validation: FakeValidation) {
        self.state()
            .validation
            .insert(target.to_owned(), validation);
    }

    fn fail_release_times(&self, target: &str, times: usize) {
        self.state()
            .release_failures_remaining
            .insert(target.to_owned(), times);
    }
}

impl LockService for FakeLockService {
    fn provider_info(&self) -> LockProviderInfo {
        self.info
    }

    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        let mut state = self.state();
        state
            .acquire_calls
            .push(request.target().as_str().to_owned());
        if state.acquire_failure.as_deref() == Some(request.target().as_str()) {
            let mut error = LockError::for_request(
                LockErrorCategory::AlreadyLocked,
                self.info.kind,
                LockOperation::Acquire,
                &request,
            );
            if let Some(owner) = state.acquire_owner.clone() {
                error = error.with_owner(owner);
            }
            return Err(error);
        }
        Ok(Box::new(FakeHeldLock {
            provider_kind: self.info.kind,
            instance_id: self.instance_id,
            project_fingerprint: request.project_fingerprint().to_owned(),
            session_id: request.session_id().clone(),
            target: request.target().clone(),
            state: HeldLockState::Active,
        }))
    }

    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Validate)?;
        if held.state == HeldLockState::Released {
            return Err(held.error(
                LockErrorCategory::HeldLockAlreadyReleased,
                LockOperation::Validate,
            ));
        }
        let target = held.target.as_str().to_owned();
        let mut state = self.state();
        state.validate_calls.push(target.clone());
        match state
            .validation
            .get(&target)
            .copied()
            .unwrap_or(FakeValidation::Valid)
        {
            FakeValidation::Valid => Ok(()),
            FakeValidation::Lost => {
                Err(held.error(LockErrorCategory::LockLost, LockOperation::Validate))
            }
            FakeValidation::Unknown => {
                Err(held.error(LockErrorCategory::LockStateUnknown, LockOperation::Validate))
            }
            FakeValidation::ProviderFailure => Err(held.error(
                LockErrorCategory::LockValidationFailed,
                LockOperation::Validate,
            )),
            FakeValidation::MutateTarget => {
                held.target = ProjectRelativePath::parse("data/tampered.json")
                    .expect("test path must be valid");
                Ok(())
            }
            FakeValidation::MutateProviderInstance => {
                held.instance_id += 1;
                Ok(())
            }
            FakeValidation::BecomeInactive => {
                held.state = HeldLockState::Released;
                Ok(())
            }
        }
    }

    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Release)?;
        if held.state == HeldLockState::Released {
            return Err(held.error(
                LockErrorCategory::HeldLockAlreadyReleased,
                LockOperation::Release,
            ));
        }
        let target = held.target.as_str().to_owned();
        let mut state = self.state();
        state.release_calls.push(target.clone());
        let remaining = state.release_failures_remaining.entry(target).or_insert(0);
        if *remaining > 0 {
            *remaining -= 1;
            return Err(held.error(LockErrorCategory::LockReleaseFailed, LockOperation::Release));
        }
        held.state = HeldLockState::Released;
        Ok(())
    }
}

impl FakeLockService {
    fn checked<'a>(
        &self,
        held: &'a mut dyn HeldLock,
        operation: LockOperation,
    ) -> Result<&'a mut FakeHeldLock, LockError> {
        if held.provider_kind() != self.info.kind || held.provider_instance_id() != self.instance_id
        {
            return Err(LockError::for_held(
                LockErrorCategory::HeldLockProviderMismatch,
                self.info.kind,
                operation,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ));
        }
        let project = held.project_fingerprint().to_owned();
        let session = held.session_id().clone();
        let target = held.target().clone();
        held.as_any_mut()
            .downcast_mut::<FakeHeldLock>()
            .ok_or_else(|| {
                LockError::for_held(
                    LockErrorCategory::HeldLockProviderMismatch,
                    self.info.kind,
                    operation,
                    &project,
                    &session,
                    &target,
                )
            })
    }
}

struct FakeHeldLock {
    provider_kind: LockProviderKind,
    instance_id: u64,
    project_fingerprint: String,
    session_id: LockSessionId,
    target: ProjectRelativePath,
    state: HeldLockState,
}

impl FakeHeldLock {
    fn error(&self, category: LockErrorCategory, operation: LockOperation) -> LockError {
        LockError::for_held(
            category,
            self.provider_kind,
            operation,
            &self.project_fingerprint,
            &self.session_id,
            &self.target,
        )
    }
}

impl HeldLock for FakeHeldLock {
    fn provider_kind(&self) -> LockProviderKind {
        self.provider_kind
    }

    fn provider_instance_id(&self) -> u64 {
        self.instance_id
    }

    fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    fn target(&self) -> &ProjectRelativePath {
        &self.target
    }

    fn state(&self) -> HeldLockState {
        self.state
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn session() -> Result<LockSessionId, LockSetIdentityError> {
    LockSessionId::generate()
}

fn path(value: &str) -> Result<ProjectRelativePath, Box<dyn std::error::Error>> {
    Ok(ProjectRelativePath::parse(value)?)
}

#[test]
fn session_id_generates_parses_and_rejects_unsafe_formats() -> Result<(), Box<dyn std::error::Error>>
{
    let first = session()?;
    let second = session()?;
    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 69);
    assert_eq!(LockSessionId::parse(first.as_str())?, first);
    for invalid in [
        "",
        "lock-abc",
        "../lock-secret",
        &format!("lock-{}", "A".repeat(64)),
    ] {
        let error = LockSessionId::parse(invalid).expect_err("invalid ID must fail");
        if !invalid.is_empty() {
            assert!(!error.to_string().contains(invalid));
        }
    }
    Ok(())
}

#[test]
fn request_rejects_invalid_project_fingerprint_without_raw_path(
) -> Result<(), Box<dyn std::error::Error>> {
    let session = session()?;
    let target = path("자료/설정.json")?;
    let error = LockAcquireRequest::new("not-a-fingerprint", &session, &target)
        .err()
        .ok_or("invalid fingerprint unexpectedly accepted")?;
    assert_eq!(error, LockSetIdentityError::InvalidProjectFingerprint);
    assert!(!error.to_string().contains("not-a-fingerprint"));
    Ok(())
}

#[test]
fn no_lock_is_a_real_non_distributed_provider_with_identity_and_release_rules(
) -> Result<(), Box<dyn std::error::Error>> {
    let service = NoLockService::new();
    let info = service.provider_info();
    assert_eq!(info.kind, LockProviderKind::None);
    assert!(!info.capabilities.distributed);
    assert!(info.capabilities.validation);
    assert!(!info.capabilities.owner_diagnostics);
    assert!(!info.capabilities.steal);

    let session = session()?;
    let target = path("자료/설정.json")?;
    let request = LockAcquireRequest::new(FINGERPRINT, &session, &target)?;
    let mut held = service.acquire(request)?;
    assert_eq!(held.project_fingerprint(), FINGERPRINT);
    assert_eq!(held.session_id(), &session);
    assert_eq!(held.target(), &target);
    service.validate(held.as_mut())?;
    service.release(held.as_mut())?;
    assert_eq!(held.state(), HeldLockState::Released);
    assert_eq!(
        service
            .validate(held.as_mut())
            .expect_err("released handle must not validate")
            .category(),
        LockErrorCategory::HeldLockAlreadyReleased
    );
    Ok(())
}

#[test]
fn no_lock_uses_the_same_coordinator_path_as_future_providers(
) -> Result<(), Box<dyn std::error::Error>> {
    let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
    let coordinator = LockCoordinator::new(service);
    let session = session()?;
    let mut guard = coordinator.acquire_all(
        FINGERPRINT,
        &session,
        vec![path("자료/설정.json")?, path("data/item.json")?],
    )?;
    assert_eq!(
        guard
            .targets()
            .map(ProjectRelativePath::as_str)
            .collect::<Vec<_>>(),
        ["data/item.json", "자료/설정.json"]
    );
    guard.validate_all()?;
    guard.release_all()?;
    assert_eq!(guard.state(), LockSetState::Released);
    Ok(())
}

#[test]
fn no_lock_rejects_duplicate_release_and_foreign_provider_instance(
) -> Result<(), Box<dyn std::error::Error>> {
    let first = NoLockService::new();
    let second = NoLockService::new();
    let session = session()?;
    let target = path("data/item.json")?;
    let mut held = first.acquire(LockAcquireRequest::new(FINGERPRINT, &session, &target)?)?;
    let mismatch = second
        .release(held.as_mut())
        .expect_err("foreign handle must fail");
    assert_eq!(
        mismatch.category(),
        LockErrorCategory::HeldLockProviderMismatch
    );
    first.release(held.as_mut())?;
    let duplicate = first
        .release(held.as_mut())
        .expect_err("duplicate release must fail");
    assert_eq!(
        duplicate.category(),
        LockErrorCategory::HeldLockAlreadyReleased
    );
    Ok(())
}

#[test]
fn coordinator_sorts_targets_and_returns_guard_only_after_all_acquire(
) -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    let coordinator = LockCoordinator::new(fake.clone());
    let session = session()?;
    let mut guard = coordinator.acquire_all(
        FINGERPRINT,
        &session,
        vec![
            path("자료/나.json")?,
            path("data/z.json")?,
            path("data/a.json")?,
        ],
    )?;
    assert_eq!(
        fake.state().acquire_calls,
        ["data/a.json", "data/z.json", "자료/나.json"]
    );
    assert_eq!(
        guard
            .targets()
            .map(ProjectRelativePath::as_str)
            .collect::<Vec<_>>(),
        ["data/a.json", "data/z.json", "자료/나.json"]
    );
    assert_eq!(guard.project_fingerprint(), FINGERPRINT);
    assert_eq!(guard.session_id(), &session);
    assert!(guard.is_write_eligible());
    guard.validate_all()?;
    Ok(())
}

#[test]
fn coordinator_rejects_empty_and_duplicate_sets_before_provider_calls(
) -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    let coordinator = LockCoordinator::new(fake.clone());
    let session = session()?;
    assert_eq!(
        coordinator
            .acquire_all(FINGERPRINT, &session, Vec::new())
            .expect_err("empty set must fail")
            .category(),
        LockErrorCategory::EmptyLockTargetSet
    );
    assert_eq!(
        coordinator
            .acquire_all(
                FINGERPRINT,
                &session,
                vec![path("data/a.json")?, path(r"data\a.json")?],
            )
            .expect_err("duplicate must fail")
            .category(),
        LockErrorCategory::DuplicateLockTarget
    );
    #[cfg(windows)]
    assert_eq!(
        coordinator
            .acquire_all(
                FINGERPRINT,
                &session,
                vec![path("DATA/File.JSON")?, path("data/file.json")?],
            )
            .expect_err("Windows alias must fail")
            .category(),
        LockErrorCategory::DuplicateLockTarget
    );
    assert!(fake.state().acquire_calls.is_empty());
    Ok(())
}

#[test]
fn partial_acquire_preserves_primary_multiple_cleanup_errors_and_retry_handles(
) -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    fake.fail_acquire(
        "data/c.json",
        Some(SafeOwnerIdentity::parse("collaborator-7")?),
    );
    fake.fail_release_times("data/a.json", 1);
    fake.fail_release_times("data/b.json", 1);
    let coordinator = LockCoordinator::new(fake.clone());
    let session = session()?;
    let mut error = coordinator
        .acquire_all(
            FINGERPRINT,
            &session,
            vec![
                path("data/c.json")?,
                path("data/a.json")?,
                path("data/b.json")?,
            ],
        )
        .expect_err("middle workflow must fail");
    assert_eq!(
        error.category(),
        LockErrorCategory::PartialAcquireReleaseFailed
    );
    let LockCoordinatorError::AcquireFailed {
        acquire_error,
        release_error,
        ..
    } = &error
    else {
        return Err("unexpected coordinator error".into());
    };
    assert_eq!(acquire_error.category(), LockErrorCategory::AlreadyLocked);
    assert_eq!(
        acquire_error
            .owner()
            .ok_or("owner diagnostic missing")?
            .to_string(),
        "collaborator-7"
    );
    assert_eq!(
        release_error
            .as_ref()
            .ok_or("release diagnostics missing")?
            .errors
            .len(),
        2
    );
    assert_eq!(fake.state().release_calls, ["data/b.json", "data/a.json"]);

    let cleanup = error.cleanup_mut().ok_or("cleanup missing")?;
    assert_eq!(cleanup.state(), LockSetState::ReleaseFailed);
    assert_eq!(
        cleanup
            .active_targets()
            .map(ProjectRelativePath::as_str)
            .collect::<Vec<_>>(),
        ["data/a.json", "data/b.json"]
    );
    cleanup.release_remaining()?;
    assert_eq!(cleanup.state(), LockSetState::Released);
    assert!(cleanup.active_targets().next().is_none());
    assert_eq!(
        fake.state().release_calls,
        ["data/b.json", "data/a.json", "data/b.json", "data/a.json"]
    );
    Ok(())
}

#[test]
fn partial_acquire_successful_cleanup_reports_rolled_back() -> Result<(), Box<dyn std::error::Error>>
{
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    fake.fail_acquire("data/b.json", None);
    let coordinator = LockCoordinator::new(fake.clone());
    let error = coordinator
        .acquire_all(
            FINGERPRINT,
            &session()?,
            vec![path("data/a.json")?, path("data/b.json")?],
        )
        .expect_err("acquire must fail");
    assert_eq!(
        error.category(),
        LockErrorCategory::PartialAcquireRolledBack
    );
    assert_eq!(fake.state().release_calls, ["data/a.json"]);
    Ok(())
}

#[test]
fn validation_failures_are_distinct_and_make_guard_ineligible(
) -> Result<(), Box<dyn std::error::Error>> {
    for (validation, category) in [
        (FakeValidation::Lost, LockErrorCategory::LockLost),
        (FakeValidation::Unknown, LockErrorCategory::LockStateUnknown),
        (
            FakeValidation::ProviderFailure,
            LockErrorCategory::LockValidationFailed,
        ),
    ] {
        let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
        fake.set_validation("data/item.json", validation);
        let coordinator = LockCoordinator::new(fake);
        let mut guard =
            coordinator.acquire_all(FINGERPRINT, &session()?, vec![path("data/item.json")?])?;
        let error = guard.validate_all().expect_err("validation must fail");
        assert_eq!(error.category(), category);
        assert_eq!(guard.state(), LockSetState::ValidationFailed);
        assert!(!guard.is_write_eligible());
        assert_eq!(
            guard.validate_all().expect_err("fail closed").category(),
            LockErrorCategory::LockSetNotActive
        );
    }
    Ok(())
}

#[test]
fn provider_cannot_validate_mutated_or_inactive_handle_as_write_eligible(
) -> Result<(), Box<dyn std::error::Error>> {
    for validation in [
        FakeValidation::MutateTarget,
        FakeValidation::MutateProviderInstance,
        FakeValidation::BecomeInactive,
    ] {
        let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
        fake.set_validation("data/item.json", validation);
        let coordinator = LockCoordinator::new(fake);
        let mut guard =
            coordinator.acquire_all(FINGERPRINT, &session()?, vec![path("data/item.json")?])?;
        let error = guard
            .validate_all()
            .expect_err("mutated handle must fail closed");
        assert_eq!(
            error.category(),
            LockErrorCategory::HeldLockProviderMismatch
        );
        assert_eq!(guard.state(), LockSetState::ValidationFailed);
        assert!(!guard.is_write_eligible());
        assert!(matches!(
            guard.write_permit(),
            Err(WritePermitError::LockSetNotActive)
        ));
    }
    Ok(())
}

#[test]
fn write_permit_is_issued_only_from_an_active_validated_guard(
) -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    let coordinator = LockCoordinator::new(fake.clone());
    let session = session()?;
    let mut guard = coordinator.acquire_all(
        FINGERPRINT,
        &session,
        vec![path("data/b.json")?, path("data/a.json")?],
    )?;
    {
        let permit = guard.write_permit()?;
        assert_eq!(permit.project_fingerprint(), FINGERPRINT);
        assert_eq!(permit.session_id(), &session);
        assert_eq!(permit.provider_kind(), LockProviderKind::Svn);
        assert_eq!(
            permit
                .targets()
                .iter()
                .map(ProjectRelativePath::as_str)
                .collect::<Vec<_>>(),
            ["data/a.json", "data/b.json"]
        );
    }
    assert_eq!(fake.state().validate_calls.len(), 2);

    guard.release_all()?;
    assert!(matches!(
        guard.write_permit(),
        Err(WritePermitError::LockSetNotActive)
    ));
    Ok(())
}

#[test]
fn write_permit_validation_failure_is_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    fake.set_validation("data/item.json", FakeValidation::Lost);
    let coordinator = LockCoordinator::new(fake);
    let mut guard =
        coordinator.acquire_all(FINGERPRINT, &session()?, vec![path("data/item.json")?])?;
    let error = match guard.write_permit() {
        Ok(_) => return Err("lost lock unexpectedly issued a permit".into()),
        Err(error) => error,
    };
    assert!(matches!(error, WritePermitError::Validation(_)));
    assert_eq!(guard.state(), LockSetState::ValidationFailed);
    assert!(matches!(
        guard.write_permit(),
        Err(WritePermitError::LockSetNotActive)
    ));
    Ok(())
}

#[test]
fn release_failed_guard_cannot_issue_write_permit() -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    fake.fail_release_times("data/item.json", 1);
    let coordinator = LockCoordinator::new(fake);
    let mut guard =
        coordinator.acquire_all(FINGERPRINT, &session()?, vec![path("data/item.json")?])?;
    guard.release_all().expect_err("release must fail once");
    assert_eq!(guard.state(), LockSetState::ReleaseFailed);
    assert!(matches!(
        guard.write_permit(),
        Err(WritePermitError::LockSetNotActive)
    ));
    Ok(())
}

#[test]
fn guard_release_is_reverse_retryable_and_skips_successes() -> Result<(), Box<dyn std::error::Error>>
{
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    fake.fail_release_times("data/b.json", 1);
    let coordinator = LockCoordinator::new(fake.clone());
    let mut guard = coordinator.acquire_all(
        FINGERPRINT,
        &session()?,
        vec![
            path("data/a.json")?,
            path("data/b.json")?,
            path("data/c.json")?,
        ],
    )?;
    let error = guard.release_all().expect_err("one release must fail");
    assert_eq!(error.errors.len(), 1);
    assert_eq!(guard.state(), LockSetState::ReleaseFailed);
    assert!(!guard.is_write_eligible());
    assert_eq!(
        fake.state().release_calls,
        ["data/c.json", "data/b.json", "data/a.json"]
    );
    guard.release_all()?;
    assert_eq!(guard.state(), LockSetState::Released);
    assert_eq!(
        fake.state().release_calls,
        ["data/c.json", "data/b.json", "data/a.json", "data/b.json"]
    );
    Ok(())
}

#[test]
fn dropping_guard_never_calls_provider_release() -> Result<(), Box<dyn std::error::Error>> {
    let fake = Arc::new(FakeLockService::new(LockProviderKind::Svn));
    let coordinator = LockCoordinator::new(fake.clone());
    let guard = coordinator.acquire_all(FINGERPRINT, &session()?, vec![path("data/item.json")?])?;
    drop(guard);
    assert!(fake.state().release_calls.is_empty());
    Ok(())
}

#[test]
fn diagnostics_only_show_safe_identity_fields() -> Result<(), Box<dyn std::error::Error>> {
    let session = session()?;
    let target = path("자료/설정.json")?;
    let request = LockAcquireRequest::new(FINGERPRINT, &session, &target)?;
    let error = LockError::for_request(
        LockErrorCategory::AuthenticationRequired,
        LockProviderKind::Svn,
        LockOperation::Acquire,
        &request,
    );
    let display = error.to_string();
    assert_eq!(error.project_fingerprint(), Some(FINGERPRINT));
    assert_eq!(error.session_id(), Some(&session));
    assert_eq!(error.target(), Some(&target));
    assert!(display.contains("자료/설정.json"));
    for secret in [
        r"C:\private-project\secret.json",
        "project body secret",
        "password=hunter2",
        "https://svn.example.test/repo?token=secret",
    ] {
        assert!(!display.contains(secret));
    }
    Ok(())
}
