use std::any::Any;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use super::*;
use crate::data::collaboration_lock::{
    HeldLock, HeldLockState, LockAcquireRequest, LockCapabilities, LockOperation, LockProviderInfo,
    NoLockService,
};
use crate::data::json::to_deterministic_json_bytes;
use crate::data::project_lock::ProjectLock;
use crate::data::transaction::{
    CommitOutcome, CommitResultState, LockedProject, TransactionCommitError, TransactionPlan,
    TransactionPrepareError,
};

const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_FINGERPRINT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SECRET_PAYLOAD: &str = "draft-secret-body";
const SECRET_PROVIDER_TOKEN: &str = "raw-provider-token";
const SECRET_RECEIPT: &str = "credential=receipt-secret&path=C:\\private\\receipt";
const SECRET_RECEIPT_PARTS: [&str; 3] = [
    "credential=receipt-secret",
    "C:\\private\\receipt",
    "receipt-secret",
];
const SECRET_OPERATION_ERROR: &str =
    "credential=operation-secret&path=C:\\private\\operation-error";
static FAKE_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(20_000);
static TEMP_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TestDocument<'a> {
    schema_version: u32,
    title: &'a str,
}

#[derive(Debug)]
enum ScopedTransactionError {
    Prepare(TransactionPrepareError),
    Commit(TransactionCommitError),
}

impl From<TransactionPrepareError> for ScopedTransactionError {
    fn from(error: TransactionPrepareError) -> Self {
        Self::Prepare(error)
    }
}

impl From<TransactionCommitError> for ScopedTransactionError {
    fn from(error: TransactionCommitError) -> Self {
        Self::Commit(error)
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let count = TEMP_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "worldbuild-edit-session-{}-{count}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "edit session test directory collision",
        ))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

fn with_locked_project<F>(test: F) -> TestResult
where
    F: FnOnce(&Path, &LockedProject<'_>) -> TestResult,
{
    let temporary = TestDirectory::new()?;
    let root = temporary.0.join("project-under-test");
    fs::create_dir(&root)?;
    fs::create_dir(root.join("data"))?;
    let lock = ProjectLock::try_acquire(&root, &temporary.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    test(&root, &project)
}

fn transaction_plan(target: &str, title: &str) -> Result<TransactionPlan, TransactionPrepareError> {
    let mut plan = TransactionPlan::new();
    plan.add_json(
        target,
        &TestDocument {
            schema_version: 1,
            title,
        },
    )?;
    Ok(plan)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FakeCall {
    Acquire { target: String, session: String },
    Validate(String),
    Release(String),
}

#[derive(Debug, Clone, Copy)]
enum FakeValidation {
    Valid,
    Lost,
    Unknown,
    ProviderFailure,
}

#[derive(Debug, Default)]
struct FakeState {
    calls: Vec<FakeCall>,
    provider_info_calls: usize,
    acquire_failure: Option<String>,
    validations: BTreeMap<String, FakeValidation>,
    release_failures_remaining: BTreeMap<String, usize>,
}

struct FakeLockService {
    instance_id: u64,
    state: Mutex<FakeState>,
}

impl FakeLockService {
    fn new() -> Self {
        Self {
            instance_id: FAKE_INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(FakeState::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fake provider mutex poisoned")
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.state().calls.clone()
    }

    fn provider_info_calls(&self) -> usize {
        self.state().provider_info_calls
    }

    fn clear_calls(&self) {
        self.state().calls.clear();
    }

    fn fail_acquire(&self, target: &str) {
        self.state().acquire_failure = Some(target.to_owned());
    }

    fn clear_acquire_failure(&self) {
        self.state().acquire_failure = None;
    }

    fn set_validation(&self, target: &str, validation: FakeValidation) {
        self.state()
            .validations
            .insert(target.to_owned(), validation);
    }

    fn fail_release_times(&self, target: &str, times: usize) {
        self.state()
            .release_failures_remaining
            .insert(target.to_owned(), times);
    }

    fn checked<'a>(
        &self,
        held: &'a mut dyn HeldLock,
        operation: LockOperation,
    ) -> Result<&'a mut FakeHeldLock, LockError> {
        if held.provider_kind() != LockProviderKind::Svn
            || held.provider_instance_id() != self.instance_id
        {
            return Err(LockError::for_held(
                LockErrorCategory::HeldLockProviderMismatch,
                LockProviderKind::Svn,
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
                    LockProviderKind::Svn,
                    operation,
                    &project,
                    &session,
                    &target,
                )
            })
    }
}

impl LockService for FakeLockService {
    fn provider_info(&self) -> LockProviderInfo {
        self.state().provider_info_calls += 1;
        LockProviderInfo {
            kind: LockProviderKind::Svn,
            capabilities: LockCapabilities {
                distributed: true,
                validation: true,
                owner_diagnostics: true,
                steal: true,
            },
        }
    }

    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        let target = request.target().as_str().to_owned();
        let mut state = self.state();
        state.calls.push(FakeCall::Acquire {
            target: target.clone(),
            session: request.session_id().as_str().to_owned(),
        });
        if state.acquire_failure.as_deref() == Some(&target) {
            return Err(LockError::for_request(
                LockErrorCategory::AlreadyLocked,
                LockProviderKind::Svn,
                LockOperation::Acquire,
                &request,
            ));
        }
        Ok(Box::new(FakeHeldLock {
            instance_id: self.instance_id,
            project_fingerprint: request.project_fingerprint().to_owned(),
            session_id: request.session_id().clone(),
            target: request.target().clone(),
            state: HeldLockState::Active,
            _provider_token: SECRET_PROVIDER_TOKEN.to_owned(),
        }))
    }

    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Validate)?;
        let target = held.target.as_str().to_owned();
        let validation = {
            let mut state = self.state();
            state.calls.push(FakeCall::Validate(target.clone()));
            state
                .validations
                .get(&target)
                .copied()
                .unwrap_or(FakeValidation::Valid)
        };
        match validation {
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
        }
    }

    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Release)?;
        let target = held.target.as_str().to_owned();
        let mut state = self.state();
        state.calls.push(FakeCall::Release(target.clone()));
        let remaining = state.release_failures_remaining.entry(target).or_insert(0);
        if *remaining > 0 {
            *remaining -= 1;
            return Err(held.error(LockErrorCategory::LockReleaseFailed, LockOperation::Release));
        }
        held.state = HeldLockState::Released;
        Ok(())
    }
}

struct FakeHeldLock {
    instance_id: u64,
    project_fingerprint: String,
    session_id: LockSessionId,
    target: ProjectRelativePath,
    state: HeldLockState,
    _provider_token: String,
}

impl FakeHeldLock {
    fn error(&self, category: LockErrorCategory, operation: LockOperation) -> LockError {
        LockError::for_held(
            category,
            LockProviderKind::Svn,
            operation,
            &self.project_fingerprint,
            &self.session_id,
            &self.target,
        )
    }
}

impl HeldLock for FakeHeldLock {
    fn provider_kind(&self) -> LockProviderKind {
        LockProviderKind::Svn
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawSinkFailure(&'static str);

impl fmt::Display for RawSinkFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for RawSinkFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DurableReceipt {
    sequence: usize,
    diagnostic: &'static str,
}

impl DurableReceipt {
    fn issued(sequence: usize) -> Self {
        Self {
            sequence,
            diagnostic: SECRET_RECEIPT,
        }
    }
}

impl fmt::Display for DurableReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.sequence, self.diagnostic)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretOperationError(&'static str);

impl fmt::Display for SecretOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for SecretOperationError {}

struct DropPayload {
    marker: String,
    drops: Arc<AtomicUsize>,
}

impl DropPayload {
    fn new(marker: &str, drops: Arc<AtomicUsize>) -> Self {
        Self {
            marker: marker.to_owned(),
            drops,
        }
    }
}

impl Drop for DropPayload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeDurableSink {
    attempts: usize,
    failures_remaining: usize,
    durable_acceptances: usize,
    custody_payloads: Vec<String>,
    envelopes: Vec<RecoveryEnvelope>,
    raw_failures_handled: usize,
}

impl DurableRecoverySink<String> for FakeDurableSink {
    type Receipt = DurableReceipt;

    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        payload: String,
    ) -> Result<Self::Receipt, RecoverySinkFailure<String>> {
        self.attempts += 1;
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            let raw = RawSinkFailure("credential=raw-secret&path=C:\\private\\draft");
            let _sink_private_diagnostic = raw.to_string();
            self.raw_failures_handled += 1;
            return Err(RecoverySinkFailure::new(
                payload,
                RecoverySinkFailureCategory::Unavailable,
            ));
        }
        self.envelopes.push(envelope.clone());
        self.custody_payloads.push(payload);
        self.durable_acceptances += 1;
        // 이 지점은 실제 저장소의 crash-safe custody 완료를 모사한다. fake도 이 상태를
        // 먼저 기록한 뒤에만 durable receipt를 발급한다.
        Ok(DurableReceipt::issued(self.durable_acceptances))
    }
}

fn path(value: &str) -> ProjectRelativePath {
    ProjectRelativePath::parse(value).expect("test path must be valid")
}

fn fake_session() -> (
    EditSessionService<String, DurableReceipt>,
    Arc<FakeLockService>,
) {
    let provider = Arc::new(FakeLockService::new());
    let service: Arc<dyn LockService> = provider.clone();
    (EditSessionService::new(service), provider)
}

fn save_error(
    service: &mut EditSessionService<String, DurableReceipt>,
    fingerprint: &str,
    targets: &[ProjectRelativePath],
) -> EditSessionError {
    match service.run_validated_write(fingerprint, targets, |_| Ok::<(), ()>(())) {
        Ok(_) => panic!("save permit unexpectedly issued"),
        Err(error) => error,
    }
}

fn acquire_targets(calls: &[FakeCall]) -> Vec<&str> {
    calls
        .iter()
        .filter_map(|call| match call {
            FakeCall::Acquire { target, .. } => Some(target.as_str()),
            _ => None,
        })
        .collect()
}

fn release_targets(calls: &[FakeCall]) -> Vec<&str> {
    calls
        .iter()
        .filter_map(|call| match call {
            FakeCall::Release(target) => Some(target.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_error_chain_omits(error: &(dyn Error + 'static), sentinel: &str) {
    let mut source = Some(error);
    while let Some(current) = source {
        assert!(!current.to_string().contains(sentinel));
        assert!(!format!("{current:?}").contains(sentinel));
        source = current.source();
    }
}

#[test]
fn initial_snapshot_is_read_only_and_contains_no_internal_resource() {
    let (service, _) = fake_session();
    let snapshot = service.snapshot();
    assert_eq!(snapshot.state(), EditSessionState::ReadOnly);
    assert_eq!(snapshot.project_fingerprint(), None);
    assert_eq!(snapshot.session_id(), None);
    assert_eq!(snapshot.provider_kind(), None);
    assert!(snapshot.targets().is_empty());
    assert_eq!(snapshot.recovery_handoff(), None);
    assert_eq!(snapshot.release_failure_count(), 0);
    let diagnostic = format!("{snapshot:?}");
    assert!(!diagnostic.contains(SECRET_PROVIDER_TOKEN));
    assert!(!diagnostic.contains(SECRET_PAYLOAD));
}

#[test]
fn read_only_rejects_operations_that_require_owned_locks_or_recovery() {
    let (mut service, _) = fake_session();
    let target = path("data/a.json");
    let mut sink = FakeDurableSink::default();

    assert_eq!(
        service.revalidate().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        save_error(&mut service, FINGERPRINT, std::slice::from_ref(&target)).category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.end_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.retry_release().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service
            .preserve_for_recovery(String::new())
            .unwrap_err()
            .category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.change_targets(vec![target]).unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service
            .accept_recovery_durably(&mut sink)
            .unwrap_err()
            .category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn preserve_failure_returns_owned_payload_without_exposing_or_dropping_it() {
    let provider: Arc<dyn LockService> = Arc::new(FakeLockService::new());
    let mut service = EditSessionService::<DropPayload, DurableReceipt>::new(provider);
    let drops = Arc::new(AtomicUsize::new(0));
    let failure = service
        .preserve_for_recovery(DropPayload::new(SECRET_PAYLOAD, drops.clone()))
        .unwrap_err();

    assert_eq!(failure.category(), EditSessionErrorCategory::InvalidState);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(!failure.to_string().contains(SECRET_PAYLOAD));
    assert!(!format!("{failure:?}").contains(SECRET_PAYLOAD));
    assert!(failure.source().is_none());

    let (payload, error) = failure.into_parts();
    assert_eq!(payload.marker, SECRET_PAYLOAD);
    assert_eq!(error.category(), EditSessionErrorCategory::InvalidState);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(payload);
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    let dropped_failure = service
        .preserve_for_recovery(DropPayload::new(SECRET_PAYLOAD, drops.clone()))
        .unwrap_err();
    assert!(!format!("{dropped_failure:?}").contains(SECRET_PAYLOAD));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(dropped_failure);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn duplicate_preserve_returns_only_the_new_payload_and_empty_payload_is_recoverable() {
    let provider = Arc::new(FakeLockService::new());
    let service: Arc<dyn LockService> = provider;
    let first_drops = Arc::new(AtomicUsize::new(0));
    let second_drops = Arc::new(AtomicUsize::new(0));
    let mut service = EditSessionService::<DropPayload, DurableReceipt>::new(service);
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service
        .preserve_for_recovery(DropPayload::new("first", first_drops.clone()))
        .unwrap();

    let failure = service
        .preserve_for_recovery(DropPayload::new("second", second_drops.clone()))
        .unwrap_err();
    assert_eq!(first_drops.load(Ordering::SeqCst), 0);
    assert_eq!(second_drops.load(Ordering::SeqCst), 0);
    let recovered = failure.into_payload();
    assert_eq!(recovered.marker, "second");
    drop(recovered);
    assert_eq!(second_drops.load(Ordering::SeqCst), 1);
    assert_eq!(first_drops.load(Ordering::SeqCst), 0);

    let (mut string_service, _) = fake_session();
    let empty = string_service
        .preserve_for_recovery(String::new())
        .unwrap_err()
        .into_payload();
    assert!(empty.is_empty());
}

#[test]
fn single_and_multiple_targets_enter_editing_only_after_deterministic_acquire() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
    service.end_edit().unwrap();

    provider.clear_calls();
    service
        .begin_edit(
            FINGERPRINT,
            vec![
                path("data/c.json"),
                path("data/a.json"),
                path("data/b.json"),
            ],
        )
        .unwrap();
    assert_eq!(
        acquire_targets(&provider.calls()),
        ["data/a.json", "data/b.json", "data/c.json"]
    );
    assert_eq!(
        service
            .snapshot()
            .targets()
            .iter()
            .map(ProjectRelativePath::as_str)
            .collect::<Vec<_>>(),
        ["data/a.json", "data/b.json", "data/c.json"]
    );
}

#[test]
fn invalid_and_duplicate_acquire_inputs_fail_cleanly_without_permit() {
    let (mut service, provider) = fake_session();
    let error = service
        .begin_edit("invalid-project-fingerprint", vec![path("data/a.json")])
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::InvalidIdentity);
    assert!(!error.to_string().contains("invalid-project-fingerprint"));
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );

    let error = service.begin_edit(FINGERPRINT, Vec::new()).unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );

    let duplicate = path("data/a.json");
    let error = service
        .begin_edit(FINGERPRINT, vec![duplicate.clone(), duplicate.clone()])
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    assert!(provider.calls().is_empty());
    assert_eq!(
        save_error(&mut service, FINGERPRINT, &[duplicate]).category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn coordinator_caches_provider_metadata_and_every_preflight_is_strictly_provider_free() {
    let provider = Arc::new(FakeLockService::new());
    let lock_service: Arc<dyn LockService> = provider.clone();
    let coordinator = LockCoordinator::new(lock_service);
    assert_eq!(provider.provider_info_calls(), 1);

    let session = LockSessionId::generate().unwrap();
    let duplicate = path("data/a.json");
    assert!(coordinator
        .preflight(
            "invalid-project-fingerprint",
            &session,
            vec![duplicate.clone()]
        )
        .is_err());
    assert!(coordinator
        .preflight(FINGERPRINT, &session, Vec::new())
        .is_err());
    assert!(coordinator
        .preflight(FINGERPRINT, &session, vec![duplicate.clone(), duplicate])
        .is_err());
    for _ in 0..3 {
        let request = coordinator
            .preflight(FINGERPRINT, &session, vec![path("data/valid.json")])
            .unwrap();
        assert_eq!(request.project_fingerprint(), FINGERPRINT);
    }

    assert_eq!(provider.provider_info_calls(), 1);
    assert!(provider.calls().is_empty());
}

#[test]
fn validated_request_cannot_be_acquired_by_a_different_coordinator() {
    let first_provider = Arc::new(FakeLockService::new());
    let first_lock_service: Arc<dyn LockService> = first_provider.clone();
    let first = LockCoordinator::new(first_lock_service);
    let second_provider = Arc::new(FakeLockService::new());
    let second_lock_service: Arc<dyn LockService> = second_provider.clone();
    let second = LockCoordinator::new(second_lock_service);
    let session = LockSessionId::generate().unwrap();
    let request = first
        .preflight(FINGERPRINT, &session, vec![path("data/a.json")])
        .unwrap();

    let error = second.acquire_validated(request).unwrap_err();
    assert!(matches!(
        error,
        LockCoordinatorError::ValidatedRequestMismatch(source)
            if source.category() == LockErrorCategory::HeldLockProviderMismatch
    ));
    assert_eq!(first_provider.provider_info_calls(), 1);
    assert_eq!(second_provider.provider_info_calls(), 1);
    assert!(first_provider.calls().is_empty());
    assert!(second_provider.calls().is_empty());
}

#[test]
fn first_target_acquire_failure_returns_to_read_only_without_cleanup_call() {
    let (mut service, provider) = fake_session();
    provider.fail_acquire("data/a.json");
    let target = path("data/a.json");
    let error = service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(
        error.acquire_error().map(LockError::category),
        Some(LockErrorCategory::AlreadyLocked)
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    assert_eq!(acquire_targets(&provider.calls()), ["data/a.json"]);
    assert!(release_targets(&provider.calls()).is_empty());
    assert_eq!(
        save_error(&mut service, FINGERPRINT, &[target]).category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn middle_acquire_failure_cleans_previous_handles_in_reverse_and_returns_read_only() {
    let (mut service, provider) = fake_session();
    provider.fail_acquire("data/c.json");
    let error = service
        .begin_edit(
            FINGERPRINT,
            vec![
                path("data/c.json"),
                path("data/a.json"),
                path("data/b.json"),
            ],
        )
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    assert_eq!(
        acquire_targets(&provider.calls()),
        ["data/a.json", "data/b.json", "data/c.json"]
    );
    assert_eq!(
        release_targets(&provider.calls()),
        ["data/b.json", "data/a.json"]
    );
}

#[test]
fn partial_acquire_cleanup_failure_is_retained_and_retry_releases_only_remaining_handle() {
    let (mut service, provider) = fake_session();
    provider.fail_acquire("data/c.json");
    provider.fail_release_times("data/b.json", 1);
    let error = service
        .begin_edit(
            FINGERPRINT,
            vec![
                path("data/a.json"),
                path("data/b.json"),
                path("data/c.json"),
            ],
        )
        .unwrap_err();
    assert_eq!(
        error.category(),
        EditSessionErrorCategory::PartialAcquireCleanupFailed
    );
    assert_eq!(
        error.acquire_error().map(LockError::category),
        Some(LockErrorCategory::AlreadyLocked)
    );
    assert!(error.release_error().is_some());
    assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
    match service.retained_failure() {
        Some(RetainedFailure::PartialAcquire {
            acquire_error,
            release_diagnostics,
        }) => {
            assert_eq!(acquire_error.category(), LockErrorCategory::AlreadyLocked);
            assert_eq!(release_diagnostics.failure_count(), 1);
        }
        _ => panic!("partial acquire errors were not retained"),
    }
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );

    provider.clear_calls();
    service.retry_release().unwrap();
    assert_eq!(release_targets(&provider.calls()), ["data/b.json"]);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn editing_rejects_a_second_begin_and_invalid_release_retry() {
    let (mut service, _) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    assert_eq!(
        service
            .begin_edit(FINGERPRINT, vec![path("data/b.json")])
            .unwrap_err()
            .category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.retry_release().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn save_permit_requires_exact_project_and_sorted_target_identity() {
    let (mut service, provider) = fake_session();
    let targets = vec![path("data/a.json"), path("data/b.json")];
    service.begin_edit(FINGERPRINT, targets.clone()).unwrap();
    provider.clear_calls();

    assert_eq!(
        save_error(&mut service, OTHER_FINGERPRINT, &targets).category(),
        EditSessionErrorCategory::ProjectMismatch
    );
    assert_eq!(
        save_error(&mut service, FINGERPRINT, &[path("data/a.json")]).category(),
        EditSessionErrorCategory::TargetMismatch
    );
    assert!(provider.calls().is_empty());

    let outcome = service
        .run_validated_write(FINGERPRINT, &targets, |permit| {
            assert_eq!(permit.project_fingerprint(), FINGERPRINT);
            assert_eq!(permit.provider_kind(), LockProviderKind::Svn);
            assert_eq!(permit.targets(), targets);
            assert!(permit.session_id().as_str().starts_with("lock-"));
            Ok::<(), ()>(())
        })
        .unwrap();
    assert!(outcome.operation_result().is_ok());
    assert_eq!(outcome.session_state(), EditSessionState::Editing);
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
}

#[test]
fn save_validation_lost_and_unknown_are_fail_closed_and_preserve_diagnosis() {
    for (validation, expected) in [
        (FakeValidation::Lost, EditSessionErrorCategory::LockLost),
        (
            FakeValidation::Unknown,
            EditSessionErrorCategory::LockStateUnknown,
        ),
    ] {
        let (mut service, provider) = fake_session();
        let target = path("data/a.json");
        service
            .begin_edit(FINGERPRINT, vec![target.clone()])
            .unwrap();
        provider.set_validation(target.as_str(), validation);
        let error = save_error(&mut service, FINGERPRINT, std::slice::from_ref(&target));
        assert_eq!(error.category(), expected);
        assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
        assert_eq!(
            save_error(&mut service, FINGERPRINT, &[target]).category(),
            EditSessionErrorCategory::InvalidState
        );
        match service.retained_failure() {
            Some(RetainedFailure::Validation(ValidationFailureRef::Save(source))) => {
                assert!(matches!(source, WritePermitError::Validation(_)));
            }
            _ => panic!("save validation diagnosis was not retained"),
        }
    }
}

#[test]
fn write_scope_reconciles_lock_loss_even_when_operation_reports_success() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();

    let outcome = service
        .run_validated_write(FINGERPRINT, std::slice::from_ref(&target), |mut permit| {
            provider.set_validation(target.as_str(), FakeValidation::Lost);
            let _ignored_by_application =
                permit.validate_for(FINGERPRINT, std::slice::from_ref(&target));
            Ok::<_, ()>("application-result")
        })
        .unwrap();

    assert_eq!(outcome.operation_result(), &Ok("application-result"));
    assert_eq!(outcome.session_state(), EditSessionState::LockLost);
    assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
    assert!(matches!(
        service.retained_failure(),
        Some(RetainedFailure::Validation(ValidationFailureRef::Save(
            WritePermitError::Validation(source)
        ))) if source.category() == LockErrorCategory::LockLost
    ));
}

#[test]
fn ordinary_operation_error_is_opaque_and_keeps_an_active_session_editable() {
    let (mut service, _) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();

    let outcome = service
        .run_validated_write(FINGERPRINT, std::slice::from_ref(&target), |_| {
            Err::<(), _>(SecretOperationError(SECRET_OPERATION_ERROR))
        })
        .unwrap();
    assert_eq!(outcome.session_state(), EditSessionState::Editing);
    assert!(outcome.validation_failure().is_none());
    assert!(service.retained_failure().is_none());
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
    let diagnostic = format!("{outcome:?}");
    assert!(!diagnostic.contains(SECRET_OPERATION_ERROR));
    assert!(!diagnostic.contains("operation-secret"));

    let recovered = outcome.into_operation_result().unwrap_err();
    assert_eq!(recovered, SecretOperationError(SECRET_OPERATION_ERROR));
    assert!(recovered.to_string().contains(SECRET_OPERATION_ERROR));

    let follow_up = service
        .run_validated_write(FINGERPRINT, std::slice::from_ref(&target), |_| {
            Ok::<_, SecretOperationError>("next-write")
        })
        .unwrap();
    assert_eq!(follow_up.operation_result(), &Ok("next-write"));
    assert_eq!(follow_up.session_state(), EditSessionState::Editing);
}

#[test]
fn transaction_prepare_validation_failure_updates_session_and_creates_no_artifacts() -> TestResult {
    with_locked_project(|root, project| {
        let provider = Arc::new(FakeLockService::new());
        let lock_service: Arc<dyn LockService> = provider.clone();
        let mut service = EditSessionService::<String, DurableReceipt>::new(lock_service);
        let target = path("missing/item.json");
        service
            .begin_edit(project.fingerprint(), vec![target.clone()])
            .unwrap();
        let plan = transaction_plan(target.as_str(), "new")?;

        let outcome = service
            .run_validated_write(
                project.fingerprint(),
                std::slice::from_ref(&target),
                |permit| {
                    provider.set_validation(target.as_str(), FakeValidation::Lost);
                    plan.prepare(project, permit).map(|_prepared| ())
                },
            )
            .unwrap();

        assert!(matches!(
            outcome.operation_result(),
            Err(TransactionPrepareError::Permit(WritePermitError::Validation(source)))
                if source.category() == LockErrorCategory::LockLost
        ));
        assert_eq!(outcome.session_state(), EditSessionState::LockLost);
        assert!(matches!(
            outcome.validation_failure(),
            Some(ValidationFailureRef::Save(WritePermitError::Validation(source)))
                if source.category() == LockErrorCategory::LockLost
        ));
        assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
        assert!(matches!(
            service.retained_failure(),
            Some(RetainedFailure::Validation(ValidationFailureRef::Save(
                WritePermitError::Validation(source)
            ))) if source.category() == LockErrorCategory::LockLost
        ));
        assert!(!root.join("missing/item.json").exists());
        assert!(!root.join(".worldbuild").exists());
        assert_eq!(
            save_error(
                &mut service,
                project.fingerprint(),
                std::slice::from_ref(&target)
            )
            .category(),
            EditSessionErrorCategory::InvalidState
        );
        service
            .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
            .unwrap();
        service.end_edit().unwrap();
        assert_eq!(
            service.snapshot().state(),
            EditSessionState::RecoveryRequired
        );
        Ok(())
    })
}

#[test]
fn transaction_commit_validation_failure_preserves_operation_error_and_lock_loss() -> TestResult {
    with_locked_project(|root, project| {
        let provider = Arc::new(FakeLockService::new());
        let lock_service: Arc<dyn LockService> = provider.clone();
        let mut service = EditSessionService::<String, DurableReceipt>::new(lock_service);
        let target = path("data/item.json");
        let old_bytes = to_deterministic_json_bytes(&TestDocument {
            schema_version: 1,
            title: "old",
        })?;
        fs::write(root.join(target.as_str()), &old_bytes)?;
        service
            .begin_edit(project.fingerprint(), vec![target.clone()])
            .unwrap();
        let plan = transaction_plan(target.as_str(), "new")?;

        let outcome = service
            .run_validated_write(
                project.fingerprint(),
                std::slice::from_ref(&target),
                |permit| {
                    let prepared = plan
                        .prepare(project, permit)
                        .map_err(ScopedTransactionError::from)?;
                    provider.set_validation(target.as_str(), FakeValidation::Unknown);
                    prepared.commit().map_err(ScopedTransactionError::from)
                },
            )
            .unwrap();

        match outcome.operation_result() {
            Err(ScopedTransactionError::Commit(error)) => {
                assert_eq!(error.result_state(), CommitResultState::NotApplied);
            }
            Err(ScopedTransactionError::Prepare(error)) => {
                panic!("transaction unexpectedly failed during prepare: {error}")
            }
            Ok(CommitOutcome::Committed) => panic!("transaction unexpectedly committed"),
            Ok(_) => panic!("transaction returned an unexpected successful outcome"),
        }
        assert_eq!(outcome.session_state(), EditSessionState::LockLost);
        assert!(matches!(
            outcome.validation_failure(),
            Some(ValidationFailureRef::Save(WritePermitError::Validation(source)))
                if source.category() == LockErrorCategory::LockStateUnknown
        ));
        assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
        assert_eq!(fs::read(root.join(target.as_str()))?, old_bytes);
        assert_eq!(
            save_error(
                &mut service,
                project.fingerprint(),
                std::slice::from_ref(&target)
            )
            .category(),
            EditSessionErrorCategory::InvalidState
        );
        service
            .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
            .unwrap();
        service.end_edit().unwrap();
        assert_eq!(
            service.snapshot().state(),
            EditSessionState::RecoveryRequired
        );
        Ok(())
    })
}

#[test]
fn explicit_revalidation_succeeds_or_transitions_any_provider_failure_to_lock_lost() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    service.revalidate().unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);

    provider.set_validation(target.as_str(), FakeValidation::ProviderFailure);
    let error = service.revalidate().unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::ValidationFailed);
    assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
    assert_eq!(
        service.revalidate().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn normal_end_releases_all_handles_in_reverse_and_clears_write_authority() {
    let (mut service, provider) = fake_session();
    let targets = vec![
        path("data/a.json"),
        path("data/b.json"),
        path("data/c.json"),
    ];
    service.begin_edit(FINGERPRINT, targets.clone()).unwrap();
    provider.clear_calls();
    service.end_edit().unwrap();
    assert_eq!(
        release_targets(&provider.calls()),
        ["data/c.json", "data/b.json", "data/a.json"]
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    assert_eq!(
        save_error(&mut service, FINGERPRINT, &targets).category(),
        EditSessionErrorCategory::InvalidState
    );
}

#[test]
fn dropping_an_edit_session_does_not_hide_an_external_release_attempt() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    provider.clear_calls();
    drop(service);
    assert!(release_targets(&provider.calls()).is_empty());
}

#[test]
fn lock_lost_can_still_release_held_resources() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    provider.set_validation(target.as_str(), FakeValidation::Lost);
    service.revalidate().unwrap_err();
    provider.clear_calls();
    service.end_edit().unwrap();
    assert_eq!(release_targets(&provider.calls()), ["data/a.json"]);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn every_reverse_release_position_is_retryable_without_releasing_completed_handles_again() {
    for failed_target in ["data/c.json", "data/b.json", "data/a.json"] {
        let (mut service, provider) = fake_session();
        service
            .begin_edit(
                FINGERPRINT,
                vec![
                    path("data/a.json"),
                    path("data/b.json"),
                    path("data/c.json"),
                ],
            )
            .unwrap();
        provider.fail_release_times(failed_target, 1);
        provider.clear_calls();
        let error = service.end_edit().unwrap_err();
        assert_eq!(error.category(), EditSessionErrorCategory::ReleaseFailed);
        assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
        assert_eq!(
            release_targets(&provider.calls()),
            ["data/c.json", "data/b.json", "data/a.json"]
        );

        provider.clear_calls();
        service.retry_release().unwrap();
        assert_eq!(release_targets(&provider.calls()), [failed_target]);
        assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    }
}

#[test]
fn release_retry_failure_accumulates_diagnostics_and_blocks_new_work() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    provider.fail_release_times(target.as_str(), 2);
    service.end_edit().unwrap_err();
    assert_eq!(
        service
            .begin_edit(FINGERPRINT, vec![path("data/b.json")])
            .unwrap_err()
            .category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        save_error(&mut service, FINGERPRINT, std::slice::from_ref(&target),).category(),
        EditSessionErrorCategory::InvalidState
    );
    let error = service.retry_release().unwrap_err();
    assert_eq!(
        error.category(),
        EditSessionErrorCategory::ReleaseRetryFailed
    );
    assert_eq!(service.snapshot().release_failure_count(), 2);
    match service.retained_failure() {
        Some(RetainedFailure::Release {
            validation_failure,
            release_diagnostics: Some(release_diagnostics),
        }) => {
            assert!(validation_failure.is_none());
            assert_eq!(release_diagnostics.failure_count(), 2);
            assert_eq!(release_diagnostics.retry_attempts(), 1);
        }
        _ => panic!("release failures were not retained"),
    }
    service.retry_release().unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn repeated_release_failures_keep_only_bounded_first_and_latest_diagnostics() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    provider.fail_release_times(target.as_str(), 80);
    service.end_edit().unwrap_err();
    for _ in 0..64 {
        service.retry_release().unwrap_err();
    }

    assert_eq!(service.snapshot().release_failure_count(), 65);
    match service.retained_failure() {
        Some(RetainedFailure::Release {
            release_diagnostics: Some(diagnostics),
            ..
        }) => {
            assert_eq!(diagnostics.retry_attempts(), 64);
            assert_eq!(diagnostics.failure_count(), 65);
            assert_eq!(diagnostics.first().errors.len(), 1);
            assert_eq!(diagnostics.latest().errors.len(), 1);
            assert_eq!(
                diagnostics.first().errors[0].category(),
                LockErrorCategory::LockReleaseFailed
            );
            assert_eq!(
                diagnostics.latest().errors[0].category(),
                LockErrorCategory::LockReleaseFailed
            );
        }
        _ => panic!("bounded release diagnostics were not retained"),
    }
}

#[test]
fn release_retry_count_saturates_and_latest_replaces_without_changing_first() {
    let target = path("data/a.json");
    let session = LockSessionId::generate().unwrap();
    let first = LockSetReleaseError {
        errors: vec![LockError::for_context(
            LockErrorCategory::LockReleaseFailed,
            LockProviderKind::Svn,
            LockOperation::Release,
            Some(FINGERPRINT),
            Some(&session),
            Some(&target),
        )],
    };
    let latest = LockSetReleaseError {
        errors: vec![LockError::for_context(
            LockErrorCategory::LockStateUnknown,
            LockProviderKind::Svn,
            LockOperation::Release,
            Some(FINGERPRINT),
            Some(&session),
            Some(&target),
        )],
    };
    let mut diagnostics = ReleaseDiagnostics::new(first);
    diagnostics.retry_attempts = usize::MAX;
    diagnostics.record_retry(latest);

    let retained = diagnostics.as_ref();
    assert_eq!(retained.retry_attempts(), usize::MAX);
    assert_eq!(retained.failure_count(), usize::MAX);
    assert_eq!(
        retained.first().errors[0].category(),
        LockErrorCategory::LockReleaseFailed
    );
    assert_eq!(
        retained.latest().errors[0].category(),
        LockErrorCategory::LockStateUnknown
    );
}

#[test]
fn target_change_releases_old_set_then_acquires_new_set_with_new_session() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    let old_session = service.snapshot().session_id().unwrap().clone();
    provider.clear_calls();
    service
        .change_targets(vec![path("data/c.json"), path("data/b.json")])
        .unwrap();
    let calls = provider.calls();
    assert_eq!(release_targets(&calls), ["data/a.json"]);
    assert_eq!(acquire_targets(&calls), ["data/b.json", "data/c.json"]);
    assert!(matches!(calls.first(), Some(FakeCall::Release(target)) if target == "data/a.json"));
    assert_ne!(service.snapshot().session_id(), Some(&old_session));
}

#[test]
fn invalid_target_change_preflight_preserves_old_guard_and_write_scope() {
    let (mut service, provider) = fake_session();
    let old_target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![old_target.clone()])
        .unwrap();
    let old_session = service.snapshot().session_id().unwrap().clone();

    for invalid_targets in [Vec::new(), vec![path("data/b.json"), path("data/b.json")]] {
        provider.clear_calls();
        let provider_info_baseline = provider.provider_info_calls();
        let error = service.change_targets(invalid_targets).unwrap_err();
        assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
        assert_eq!(provider.provider_info_calls(), provider_info_baseline);
        assert!(provider.calls().is_empty());
        assert_eq!(service.snapshot().state(), EditSessionState::Editing);
        assert_eq!(service.snapshot().session_id(), Some(&old_session));

        let outcome = service
            .run_validated_write(FINGERPRINT, std::slice::from_ref(&old_target), |_| {
                Ok::<(), ()>(())
            })
            .unwrap();
        assert!(outcome.operation_result().is_ok());
    }

    provider.set_validation(old_target.as_str(), FakeValidation::Lost);
    service.revalidate().unwrap_err();
    let retained_category = match service.retained_failure() {
        Some(RetainedFailure::Validation(ValidationFailureRef::Revalidate(source))) => {
            source.category()
        }
        _ => panic!("lock loss was not retained"),
    };
    provider.clear_calls();
    let provider_info_baseline = provider.provider_info_calls();
    let error = service.change_targets(Vec::new()).unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(provider.provider_info_calls(), provider_info_baseline);
    assert!(provider.calls().is_empty());
    assert_eq!(service.snapshot().state(), EditSessionState::LockLost);
    assert_eq!(service.snapshot().session_id(), Some(&old_session));
    assert!(matches!(
        service.retained_failure(),
        Some(RetainedFailure::Validation(ValidationFailureRef::Revalidate(source)))
            if source.category() == retained_category
    ));
}

#[test]
fn target_change_does_not_acquire_when_old_release_fails() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    provider.fail_release_times("data/a.json", 1);
    provider.clear_calls();
    let error = service
        .change_targets(vec![path("data/b.json")])
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::ReleaseFailed);
    assert_eq!(release_targets(&provider.calls()), ["data/a.json"]);
    assert!(acquire_targets(&provider.calls()).is_empty());
    assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
}

#[test]
fn target_change_reports_new_acquire_failure_after_old_release() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    provider.fail_acquire("data/b.json");
    provider.clear_calls();
    let error = service
        .change_targets(vec![path("data/b.json")])
        .unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(error.operation(), EditSessionOperation::ChangeTargets);
    let calls = provider.calls();
    assert!(matches!(calls.first(), Some(FakeCall::Release(target)) if target == "data/a.json"));
    assert_eq!(acquire_targets(&calls), ["data/b.json"]);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
    provider.clear_acquire_failure();
    provider.clear_calls();
    service.resume_edit().unwrap();
    assert_eq!(acquire_targets(&provider.calls()), ["data/b.json"]);
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
}

#[test]
fn target_change_preserves_partial_cleanup_when_new_acquire_cleanup_fails() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    provider.fail_acquire("data/c.json");
    provider.fail_release_times("data/b.json", 1);
    let error = service
        .change_targets(vec![path("data/b.json"), path("data/c.json")])
        .unwrap_err();
    assert_eq!(
        error.category(),
        EditSessionErrorCategory::PartialAcquireCleanupFailed
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
    assert!(matches!(
        service.retained_failure(),
        Some(RetainedFailure::PartialAcquire { .. })
    ));
}

#[test]
fn lock_lost_resume_releases_then_reacquires_with_a_new_session_identity() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    let previous = service.snapshot().session_id().unwrap().clone();
    provider.set_validation(target.as_str(), FakeValidation::Lost);
    service.revalidate().unwrap_err();
    provider.set_validation(target.as_str(), FakeValidation::Valid);
    provider.clear_calls();
    service.resume_edit().unwrap();
    let calls = provider.calls();
    assert!(matches!(calls.first(), Some(FakeCall::Release(value)) if value == target.as_str()));
    assert_eq!(acquire_targets(&calls), [target.as_str()]);
    assert_ne!(service.snapshot().session_id(), Some(&previous));
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
}

#[test]
fn durable_recovery_acceptance_requires_release_and_explicit_acknowledgement() {
    let (mut service, _) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        save_error(&mut service, FINGERPRINT, &[target]).category(),
        EditSessionErrorCategory::InvalidState
    );

    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(sink.attempts, 1);
    assert_eq!(sink.durable_acceptances, 1);
    assert_eq!(sink.custody_payloads, [SECRET_PAYLOAD]);
    assert_eq!(sink.envelopes[0].project_fingerprint(), FINGERPRINT);
    assert_eq!(sink.envelopes[0].provider_kind(), LockProviderKind::Svn);
    assert_eq!(sink.envelopes[0].targets(), [path("data/a.json")]);
    assert_eq!(
        sink.envelopes[0].reason(),
        RecoveryReason::SaveRecoveryRequired
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    service.end_edit().unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap(),
        DurableReceipt::issued(1)
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn durable_receipt_is_redacted_until_the_caller_explicitly_acknowledges_it() {
    let (mut service, _) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();

    let snapshot = service.snapshot();
    assert_eq!(
        snapshot.recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    let snapshot_diagnostic = format!("{snapshot:?}");
    let retained_diagnostic = format!("{:?}", service.retained_failure());

    let begin_error = service
        .begin_edit(FINGERPRINT, vec![path("data/b.json")])
        .unwrap_err();
    let write_error = save_error(&mut service, FINGERPRINT, std::slice::from_ref(&target));
    let resume_error = service.resume_edit().unwrap_err();
    let duplicate_handoff = service.accept_recovery_durably(&mut sink).unwrap_err();
    for sentinel in SECRET_RECEIPT_PARTS {
        assert!(!snapshot_diagnostic.contains(sentinel));
        assert!(!retained_diagnostic.contains(sentinel));
        assert_error_chain_omits(&begin_error, sentinel);
        assert_error_chain_omits(&write_error, sentinel);
        assert_error_chain_omits(&resume_error, sentinel);
        assert_error_chain_omits(&duplicate_handoff, sentinel);
    }
    assert_eq!(sink.attempts, 1);

    let receipt = service.acknowledge_recovery().unwrap();
    assert_eq!(receipt, DurableReceipt::issued(1));
    assert!(format!("{receipt:?}").contains("receipt-secret"));
    assert!(receipt.to_string().contains(SECRET_RECEIPT));
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Acknowledged)
    );
    for sentinel in SECRET_RECEIPT_PARTS {
        assert!(!format!("{:?}", service.snapshot()).contains(sentinel));
    }
    let duplicate_acknowledgement = service.acknowledge_recovery().unwrap_err();
    for sentinel in SECRET_RECEIPT_PARTS {
        assert_error_chain_omits(&duplicate_acknowledgement, sentinel);
    }
    service.end_edit().unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn durable_receipt_and_guard_survive_release_failure_until_retry_and_acknowledgement() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();
    provider.fail_release_times(target.as_str(), 1);

    service.end_edit().unwrap_err();
    assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(release_targets(&provider.calls()), [target.as_str()]);
    assert_eq!(
        service.resume_edit().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );

    service.retry_release().unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap(),
        DurableReceipt::issued(1)
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn released_recovery_retries_multiple_failed_handoffs_before_durable_acceptance() {
    let (mut service, _) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    service.end_edit().unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );

    let mut sink = FakeDurableSink {
        failures_remaining: 3,
        ..FakeDurableSink::default()
    };
    for attempt in 1..=3 {
        let error = service.accept_recovery_durably(&mut sink).unwrap_err();
        assert_eq!(
            error.category(),
            EditSessionErrorCategory::RecoveryHandoffFailed
        );
        assert_error_chain_omits(&error, SECRET_PAYLOAD);
        assert_eq!(sink.attempts, attempt);
        assert_eq!(sink.durable_acceptances, 0);
        assert!(sink.custody_payloads.is_empty());
        assert_eq!(
            service.snapshot().state(),
            EditSessionState::RecoveryRequired
        );
        assert_eq!(
            service.snapshot().recovery_handoff(),
            Some(RecoveryHandoffStatus::Pending)
        );
    }

    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(sink.attempts, 4);
    assert_eq!(sink.durable_acceptances, 1);
    assert_eq!(sink.custody_payloads, [SECRET_PAYLOAD]);
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );

    let receipt = service.acknowledge_recovery().unwrap();
    assert_eq!(receipt, DurableReceipt::issued(1));
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn recovery_sink_failure_restores_payload_for_retry_and_redacts_it_from_diagnostics() {
    let (mut service, _) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    let mut sink = FakeDurableSink {
        failures_remaining: 1,
        ..FakeDurableSink::default()
    };
    let error = service.accept_recovery_durably(&mut sink).unwrap_err();
    assert_eq!(
        error.category(),
        EditSessionErrorCategory::RecoveryHandoffFailed
    );
    assert!(!error.to_string().contains(SECRET_PAYLOAD));
    assert!(!format!("{error:?}").contains(SECRET_PAYLOAD));
    assert!(!error
        .to_string()
        .contains("credential=raw-secret&path=C:\\private\\draft"));
    match &error {
        RecoveryHandoffCallError::Sink(source) => {
            assert_eq!(
                source.sink_category(),
                RecoverySinkFailureCategory::Unavailable
            );
            assert!(source.source().is_none());
        }
        RecoveryHandoffCallError::Session(_) => panic!("expected sink failure"),
    }
    let mut source: Option<&(dyn Error + 'static)> = Some(&error);
    while let Some(current) = source {
        let display = current.to_string();
        let debug = format!("{current:?}");
        assert!(!display.contains("raw-secret"));
        assert!(!debug.contains("raw-secret"));
        source = current.source();
    }
    assert_eq!(sink.raw_failures_handled, 1);
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Pending)
    );
    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(sink.custody_payloads, [SECRET_PAYLOAD]);
}

#[test]
fn empty_recovery_payload_is_valid_and_duplicate_handoff_is_rejected() {
    let (mut service, _) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service.preserve_for_recovery(String::new()).unwrap();
    assert_eq!(
        service.acknowledge_recovery().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Pending)
    );
    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(sink.custody_payloads, [""]);
    assert_eq!(
        service
            .accept_recovery_durably(&mut sink)
            .unwrap_err()
            .category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap(),
        DurableReceipt::issued(1)
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Acknowledged)
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    service.end_edit().unwrap();
}

#[test]
fn recovery_payload_can_be_handed_off_after_locks_are_released() {
    let (mut service, _) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    service.end_edit().unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Pending)
    );
    assert!(matches!(
        service.retained_failure(),
        Some(RetainedFailure::Recovery {
            reason: RecoveryReason::SaveRecoveryRequired,
            validation_failure: None,
            release_diagnostics: None
        })
    ));
    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(sink.custody_payloads, [SECRET_PAYLOAD]);
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap(),
        DurableReceipt::issued(1)
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn validation_and_release_failures_remain_available_during_recovery() {
    let (mut service, provider) = fake_session();
    let target = path("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![target.clone()])
        .unwrap();
    provider.set_validation(target.as_str(), FakeValidation::Lost);
    service.revalidate().unwrap_err();
    service
        .preserve_for_recovery(SECRET_PAYLOAD.to_owned())
        .unwrap();
    provider.fail_release_times(target.as_str(), 1);
    service.end_edit().unwrap_err();

    match service.retained_failure() {
        Some(RetainedFailure::Recovery {
            reason,
            validation_failure: Some(ValidationFailureRef::Revalidate(validation)),
            release_diagnostics: Some(release_diagnostics),
        }) => {
            assert_eq!(reason, RecoveryReason::LockLost);
            assert_eq!(validation.category(), LockErrorCategory::LockLost);
            assert_eq!(release_diagnostics.failure_count(), 1);
        }
        _ => panic!("recovery did not retain validation and release failures"),
    }

    let mut sink = FakeDurableSink::default();
    service.accept_recovery_durably(&mut sink).unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::ReleaseFailed);
    service.retry_release().unwrap();
    assert_eq!(
        service.snapshot().state(),
        EditSessionState::RecoveryRequired
    );
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(
        service.acknowledge_recovery().unwrap(),
        DurableReceipt::issued(1)
    );
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn recovery_failure_container_debug_redacts_payload() {
    let failure = RecoverySinkFailure::new(
        SECRET_PAYLOAD.to_owned(),
        RecoverySinkFailureCategory::DurabilityUncertain,
    );
    let diagnostic = format!("{failure:?}");
    assert!(!diagnostic.contains(SECRET_PAYLOAD));
}

#[test]
fn no_lock_provider_uses_the_same_full_session_and_permit_flow() {
    let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
    let mut session = EditSessionService::<String, DurableReceipt>::new(service);
    let targets = vec![path("data/b.json"), path("data/a.json")];
    session.begin_edit(FINGERPRINT, targets).unwrap();
    assert_eq!(
        session.snapshot().provider_kind(),
        Some(LockProviderKind::None)
    );
    let sorted = vec![path("data/a.json"), path("data/b.json")];
    let outcome = session
        .run_validated_write(FINGERPRINT, &sorted, |mut permit| {
            permit.validate_for(FINGERPRINT, &sorted)
        })
        .unwrap();
    assert!(outcome.into_operation_result().is_ok());
    session.revalidate().unwrap();
    session.end_edit().unwrap();
    assert_eq!(session.snapshot().state(), EditSessionState::ReadOnly);
}

#[test]
fn acquire_calls_for_reacquire_use_only_the_new_session_identity() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    let first = service.snapshot().session_id().unwrap().as_str().to_owned();
    service.end_edit().unwrap();
    provider.clear_calls();
    service.resume_edit().unwrap();
    let calls = provider.calls();
    let sessions = calls
        .iter()
        .filter_map(|call| match call {
            FakeCall::Acquire { session, .. } => Some(session.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(sessions.len(), 1);
    assert_ne!(sessions[0], first);
    assert_eq!(
        Some(sessions[0]),
        service.snapshot().session_id().map(LockSessionId::as_str)
    );
}

#[test]
fn failed_acquire_can_be_resumed_only_after_failure_configuration_is_cleared() {
    let (mut service, provider) = fake_session();
    provider.fail_acquire("data/a.json");
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap_err();
    provider.clear_acquire_failure();
    service.resume_edit().unwrap();
    assert_eq!(service.snapshot().state(), EditSessionState::Editing);
}

#[test]
fn failed_resume_reports_resume_operation_and_keeps_a_safe_retry_request() {
    let (mut service, provider) = fake_session();
    service
        .begin_edit(FINGERPRINT, vec![path("data/a.json")])
        .unwrap();
    service.end_edit().unwrap();
    provider.fail_acquire("data/a.json");
    let error = service.resume_edit().unwrap_err();
    assert_eq!(error.category(), EditSessionErrorCategory::AcquireFailed);
    assert_eq!(error.operation(), EditSessionOperation::ResumeEdit);
    assert_eq!(service.snapshot().state(), EditSessionState::ReadOnly);
}
