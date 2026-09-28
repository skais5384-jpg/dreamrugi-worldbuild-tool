mod apply;
pub(crate) mod artifact_diagnostics;
mod model;
mod owned_temp;
mod prepare;
mod protocol;
mod recovery;

#[cfg(test)]
mod apply_tests;
#[cfg(test)]
mod crash_tests;
#[cfg(test)]
mod prepare_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use super::collaboration_lock::{WritePermit, WritePermitError};
use super::project_lock::{canonical_project_identity, ProjectLock};
use super::project_relative_path::{ProjectRelativePath, ProjectRelativePathError};
use super::utc_time::now_utc_milliseconds;

pub(crate) use apply::CommitFailureSource;
// 복구/commit 진단은 JSON wrapper의 원 OS code만 좁게 조회한다.
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "M1-5C commit API는 M1-5D 전까지 실제 저장 흐름에 연결하지 않는다"
    )
)]
#[cfg_attr(test, allow(unused_imports, reason = "테스트별 사용 타입이 다르다"))]
pub(crate) use apply::{
    CommitFailure, CommitOutcome, CommitResultState, CommitStage, TransactionCommitError,
};
pub(crate) use model::{
    CommittedMarker, RolledBackMarker, TransactionManifest, TransactionOperation, TransactionState,
    TransactionStateRecord, TRANSACTION_SCHEMA_VERSION,
};
pub(crate) use prepare::{prepare_canonical, PrepareStage, PreparedArtifactTransaction};
#[cfg(test)]
pub(crate) use prepare::{
    prepare_canonical_with_hooks, prepare_with_hooks, PrepareFailPoint, PrepareHooks,
};
pub(crate) use prepare::{PreparedTransaction, TransactionPlan, TransactionPrepareError};
pub(crate) use protocol::io_cause;
#[cfg(not(test))]
pub(crate) use recovery::recover_pending_transactions_with_scope;
#[allow(
    unused_imports,
    reason = "M1-5D recovery API는 프로젝트 열기 계층에서 연결한다"
)]
pub(crate) use recovery::{
    recover_pending_transactions, RecoveryError, RecoveryReport, RecoveryResultState,
    RecoveryScope, RecoveryStage,
};

const TRANSACTION_ID_PREFIX: &str = "txn-";
const TRANSACTION_ID_VERSION: &[u8] = b"worldbuild-transaction-v1";
static TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) type TransactionModelResult<T> = Result<T, TransactionModelError>;

/// 잠금과 canonical 프로젝트 루트를 결합해 다른 프로젝트의 guard 오용을 막는다.
/// 필드와 생성자를 외부에 공개하지 않아 검증 없이 만들 수 없다.
pub(crate) struct LockedProject<'lock> {
    _lock: &'lock ProjectLock,
    canonical_root: PathBuf,
    fingerprint: String,
}

impl<'lock> LockedProject<'lock> {
    pub(crate) fn bind(
        lock: &'lock ProjectLock,
        project_root: &Path,
    ) -> TransactionModelResult<Self> {
        let (canonical_root, project_fingerprint) = canonical_project_identity(project_root)
            .map_err(|source| TransactionModelError::ProjectBindingFailed {
                lock_fingerprint: lock.fingerprint().to_owned(),
                source,
            })?;
        if lock.fingerprint() != project_fingerprint {
            return Err(TransactionModelError::LockProjectMismatch {
                lock_fingerprint: lock.fingerprint().to_owned(),
                project_fingerprint,
            });
        }
        Ok(Self {
            _lock: lock,
            canonical_root,
            fingerprint: project_fingerprint,
        })
    }

    pub(crate) fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn transactions_root(&self) -> PathBuf {
        self.canonical_root.join(".worldbuild").join("transactions")
    }
}

/// 안전한 ASCII 파일명으로 제한된 고정 길이 transaction 식별자다.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TransactionId(String);

impl TransactionId {
    pub(crate) fn parse(value: &str) -> TransactionModelResult<Self> {
        let hex = value.strip_prefix(TRANSACTION_ID_PREFIX).ok_or({
            TransactionModelError::InvalidTransactionId {
                reason: "transaction ID must start with txn-",
            }
        })?;
        if !is_lowercase_sha256(hex) {
            return Err(TransactionModelError::InvalidTransactionId {
                reason: "transaction ID must contain 64 lowercase hexadecimal digits",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// 후속 prepare 단계는 이 후보로 `create_dir`를 시도하고 AlreadyExists일 때만
    /// 새 후보를 요청해야 한다. 후보 생성 자체는 ID 할당 성공을 의미하지 않는다.
    pub(crate) fn new_candidate(
        project_fingerprint: &str,
    ) -> TransactionModelResult<(Self, String)> {
        if !is_lowercase_sha256(project_fingerprint) {
            return Err(TransactionModelError::InvalidJournal {
                transaction_id: None,
                field: "projectFingerprint",
                reason: "fingerprint must be 64 lowercase hexadecimal digits",
            });
        }
        let created_at_utc = now_utc_milliseconds()
            .map_err(|source| TransactionModelError::TransactionTimeFailed { source })?;
        let counter = TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut hasher = Sha256::new();
        hasher.update(TRANSACTION_ID_VERSION);
        hasher.update([0]);
        hasher.update(created_at_utc.as_bytes());
        hasher.update([0]);
        hasher.update(std::process::id().to_le_bytes());
        hasher.update(counter.to_le_bytes());
        hasher.update(project_fingerprint.as_bytes());
        let value = format!(
            "{TRANSACTION_ID_PREFIX}{}",
            lowercase_hex(&hasher.finalize())
        );
        Ok((Self(value), created_at_utc))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for TransactionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TransactionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug)]
pub(crate) enum TransactionModelError {
    InvalidTargetPath {
        source: ProjectRelativePathError,
    },
    DuplicateTarget {
        target: ProjectRelativePath,
    },
    InvalidTransactionId {
        reason: &'static str,
    },
    InvalidJournal {
        transaction_id: Option<TransactionId>,
        field: &'static str,
        reason: &'static str,
    },
    UnsupportedTransactionSchema {
        found: u32,
        supported: u32,
    },
    LockProjectMismatch {
        lock_fingerprint: String,
        project_fingerprint: String,
    },
    ProjectBindingFailed {
        lock_fingerprint: String,
        source: io::Error,
    },
    TransactionTimeFailed {
        source: time::error::Format,
    },
}

impl TransactionModelError {
    fn invalid_journal(
        transaction_id: Option<&TransactionId>,
        field: &'static str,
        reason: &'static str,
    ) -> Self {
        Self::InvalidJournal {
            transaction_id: transaction_id.cloned(),
            field,
            reason,
        }
    }
}

impl fmt::Display for TransactionModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTargetPath { source } => {
                write!(formatter, "invalid transaction target: {source}")
            }
            Self::DuplicateTarget { target } => {
                write!(formatter, "duplicate transaction target '{target}'")
            }
            Self::InvalidTransactionId { reason } => {
                write!(formatter, "invalid transaction ID: {reason}")
            }
            Self::InvalidJournal {
                transaction_id,
                field,
                reason,
            } => {
                formatter.write_str("invalid transaction journal")?;
                if let Some(id) = transaction_id {
                    write!(formatter, " {id}")?;
                }
                write!(formatter, ": field {field}: {reason}")
            }
            Self::UnsupportedTransactionSchema { found, supported } => write!(
                formatter,
                "unsupported transaction schema {found}; supported schema is {supported}"
            ),
            Self::LockProjectMismatch {
                lock_fingerprint,
                project_fingerprint,
            } => write!(
                formatter,
                "project lock {lock_fingerprint} does not match project {project_fingerprint}"
            ),
            Self::ProjectBindingFailed {
                lock_fingerprint,
                source,
            } => write!(
                formatter,
                "failed to inspect project for lock {lock_fingerprint}: {source}"
            ),
            Self::TransactionTimeFailed { source } => {
                write!(formatter, "failed to create transaction UTC time: {source}")
            }
        }
    }
}

impl std::error::Error for TransactionModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTargetPath { source } => Some(source),
            Self::ProjectBindingFailed { source, .. } => Some(source),
            Self::TransactionTimeFailed { source } => Some(source),
            _ => None,
        }
    }
}

impl From<ProjectRelativePathError> for TransactionModelError {
    fn from(source: ProjectRelativePathError) -> Self {
        Self::InvalidTargetPath { source }
    }
}

fn staged_artifact_path(index: u32) -> String {
    format!("staged/{index:06}.json")
}

fn backup_artifact_path(index: u32) -> String {
    format!("backups/{index:06}.json")
}

#[cfg(test)]
pub(super) fn test_write_permit(
    project_fingerprint: &str,
    targets: Vec<ProjectRelativePath>,
) -> WritePermit<'static> {
    use super::collaboration_lock::{LockCoordinator, LockService, LockSessionId, NoLockService};
    use std::sync::Arc;

    let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
    let coordinator = LockCoordinator::new(service);
    let session = LockSessionId::generate().expect("test lock session generation must succeed");
    let guard = coordinator
        .acquire_all(project_fingerprint, &session, targets)
        .expect("test lock acquisition must succeed");
    guard
        .into_test_write_permit()
        .expect("test lock permit issuance must succeed")
}

pub(super) fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) mod boundary;
    pub(crate) use super::recovery::RecoveryFailure;
    use std::cell::Cell;
    use std::fs;
    use std::io;
    use std::io::Write;
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use super::apply::{commit_with_hooks, CommitFailPoint, CommitHooks};
    use super::recovery::{
        recover_pending_transactions_with_hooks_and_scope, RecoveryFailPoint, RecoveryHooks,
    };
    use super::{
        CommitOutcome, LockedProject, PreparedTransaction, RecoveryError, RecoveryReport,
        RecoveryScope, TransactionCommitError,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum CommitTestPoint {
        ManifestRevalidation,
        ApplyingState,
        TargetPrecondition,
        StagedRevalidation,
        Rename,
        TargetSync,
        TargetParentSync,
        AppliedVerification,
        ProgressState,
        CommittedMarkerWrite,
        CommittedMarkerAfterReplace,
        CommittedMarkerVerify,
        CommittedMarkerSync,
        CommittedState,
        Cleanup,
    }

    struct ConfiguredCommitHooks;

    // G5도 공개 canonical prepare를 그대로 호출한다. 관측/실패는 기존 PrepareHooks에만 연결한다.
    type PrepareIoFactory = Box<dyn Fn(super::PrepareFailPoint, Option<u32>) -> io::Result<()>>;
    #[derive(Default, Debug, Clone, Copy)]
    pub(crate) struct PrepareCounts {
        pub(crate) calls: usize,
        pub(crate) allocations: usize,
    }
    #[derive(Default)]
    struct PrepareConfiguration {
        factory: Option<PrepareIoFactory>,
        counts: PrepareCounts,
    }
    thread_local! {
        static PREPARE_CONFIGURATION: std::cell::RefCell<PrepareConfiguration> = std::cell::RefCell::new(PrepareConfiguration::default());
    }
    pub(crate) struct ConfiguredPrepareHooks;
    impl ConfiguredPrepareHooks {
        pub(crate) fn entered() {
            PREPARE_CONFIGURATION.with(|slot| slot.borrow_mut().counts.calls += 1);
        }
    }
    impl super::PrepareHooks for ConfiguredPrepareHooks {
        fn check(&self, point: super::PrepareFailPoint, operation: Option<u32>) -> io::Result<()> {
            boundary::check(boundary::Point::Prepare(point), operation)?;
            PREPARE_CONFIGURATION.with(|slot| {
                let mut state = slot.borrow_mut();
                if point == super::PrepareFailPoint::TransactionDirectoryAllocated {
                    state.counts.allocations += 1;
                }
                match &state.factory {
                    Some(factory) => factory(point, operation),
                    None => Ok(()),
                }
            })
        }
    }
    pub(crate) fn with_canonical_prepare_hooks<T>(
        factory: impl Fn(super::PrepareFailPoint, Option<u32>) -> io::Result<()> + 'static,
        run: impl FnOnce() -> T,
    ) -> (T, PrepareCounts) {
        struct Reset(Option<PrepareConfiguration>);
        impl Drop for Reset {
            fn drop(&mut self) {
                if let Some(previous) = self.0.take() {
                    PREPARE_CONFIGURATION.with(|slot| *slot.borrow_mut() = previous);
                }
            }
        }
        let _reset = Reset(Some(PREPARE_CONFIGURATION.with(|slot| {
            slot.replace(PrepareConfiguration {
                factory: Some(Box::new(factory)),
                counts: PrepareCounts::default(),
            })
        })));
        let result = run();
        (
            result,
            PREPARE_CONFIGURATION.with(|slot| slot.borrow().counts),
        )
    }

    impl CommitHooks for ConfiguredCommitHooks {
        fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()> {
            let point = map_commit(point);
            boundary::check(boundary::Point::Commit(point), operation)?;
            fail_if_configured(Some(point), None)?;
            pause_if_configured(commit_crash_selected(point, operation))
        }

        fn check_recovery(
            &self,
            point: RecoveryFailPoint,
            operation: Option<u32>,
        ) -> io::Result<()> {
            let point = map_recovery(point);
            boundary::check(boundary::Point::Recovery(point), operation)?;
            fail_if_configured(None, Some(point))?;
            pause_if_configured(recovery_crash_selected(point, operation))
        }
    }

    pub(crate) fn commit_with_configured_test_hooks(
        prepared: PreparedTransaction<'_, '_, '_>,
    ) -> Result<CommitOutcome, TransactionCommitError> {
        commit_with_hooks(prepared, &ConfiguredCommitHooks)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum RecoveryTestPoint {
        Enumerate,
        RollingBackState,
        BackupVerify,
        RestoreExisting,
        RemoveNew,
        RestoredVerify,
        ProgressState,
        RolledBackMarkerWrite,
        RolledBackMarkerVerify,
        RolledBackMarkerSync,
        RolledBackState,
        Cleanup,
        CommittedTargetVerify,
        JournalDecision,
    }

    #[derive(Clone, Copy, Default)]
    struct FailureConfiguration {
        commit: Option<CommitTestPoint>,
        recovery: Option<RecoveryTestPoint>,
    }

    thread_local! {
        static FAILURE_CONFIGURATION: Cell<FailureConfiguration> =
            const { Cell::new(FailureConfiguration { commit: None, recovery: None }) };
    }

    type IoFactory =
        Box<dyn Fn(Option<CommitTestPoint>, Option<RecoveryTestPoint>) -> io::Result<()>>;
    thread_local! {
        static IO_FACTORY: std::cell::RefCell<Option<IoFactory>> = const { std::cell::RefCell::new(None) };
    }
    /// 실제 commit의 기존 fault 경계에서만 원인 객체를 주입한다. production 저장 경로는 그대로다.
    pub(crate) fn with_commit_io_factory<T>(
        factory: impl Fn(Option<CommitTestPoint>, Option<RecoveryTestPoint>) -> io::Result<()> + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        struct Reset(Option<IoFactory>);
        impl Drop for Reset {
            fn drop(&mut self) {
                IO_FACTORY.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let _reset = Reset(IO_FACTORY.with(|slot| slot.replace(Some(Box::new(factory)))));
        run()
    }

    pub(crate) fn with_commit_failures<T>(
        commit: Option<CommitTestPoint>,
        recovery: Option<RecoveryTestPoint>,
        run: impl FnOnce() -> T,
    ) -> T {
        struct Reset(FailureConfiguration);
        impl Drop for Reset {
            fn drop(&mut self) {
                FAILURE_CONFIGURATION.with(|configuration| configuration.set(self.0));
            }
        }
        let previous = FAILURE_CONFIGURATION
            .with(|configuration| configuration.replace(FailureConfiguration { commit, recovery }));
        let _reset = Reset(previous);
        run()
    }

    fn fail_if_configured(
        commit: Option<CommitTestPoint>,
        recovery: Option<RecoveryTestPoint>,
    ) -> io::Result<()> {
        IO_FACTORY.with(|slot| -> io::Result<()> {
            if let Some(factory) = slot.borrow().as_ref() {
                factory(commit, recovery)?;
            }
            Ok(())
        })?;
        let selected = FAILURE_CONFIGURATION.with(|configuration| {
            let configuration = configuration.get();
            commit.is_some_and(|point| configuration.commit == Some(point))
                || recovery.is_some_and(|point| configuration.recovery == Some(point))
        });
        if selected {
            Err(io::Error::other("injected migration transaction failure"))
        } else {
            Ok(())
        }
    }

    struct ConfiguredRecoveryHooks;

    impl RecoveryHooks for ConfiguredRecoveryHooks {
        fn check(&self, point: RecoveryFailPoint, operation: Option<u32>) -> io::Result<()> {
            let point = map_recovery(point);
            pause_if_configured(recovery_crash_selected(point, operation))
        }
    }

    pub(crate) fn recover_with_configured_test_hooks_and_scope<S: RecoveryScope>(
        project: &LockedProject<'_>,
        scope: &S,
    ) -> Result<RecoveryReport, RecoveryError> {
        recover_pending_transactions_with_hooks_and_scope(project, &ConfiguredRecoveryHooks, scope)
    }

    /// G2도 실제 M1 recovery 분기를 사용한다. 기존 thread-local failure scope만 읽고
    /// migration crash 환경 변수나 성공 결과 provider에는 연결하지 않는다.
    pub(crate) fn recover_for_runtime_test(
        project: &LockedProject<'_>,
    ) -> Result<RecoveryReport, RecoveryError> {
        struct RuntimeHooks;
        impl RecoveryHooks for RuntimeHooks {
            fn check(&self, point: RecoveryFailPoint, operation: Option<u32>) -> io::Result<()> {
                boundary::check(boundary::Point::Recovery(map_recovery(point)), operation)?;
                fail_if_configured(None, Some(map_recovery(point)))
            }
        }
        super::recovery::recover_pending_transactions_with_hooks(project, &RuntimeHooks)
    }

    fn commit_crash_selected(point: CommitTestPoint, operation: Option<u32>) -> bool {
        match migration_crash_point().as_deref() {
            Some("prepared-state") => point == CommitTestPoint::ManifestRevalidation,
            Some("applying-state") => {
                point == CommitTestPoint::TargetPrecondition && operation == Some(0)
            }
            Some("applied-first") => {
                point == CommitTestPoint::TargetPrecondition && operation == Some(1)
            }
            Some("applied-middle") => {
                point == CommitTestPoint::TargetPrecondition && operation == Some(2)
            }
            Some("applied-last-before-marker") => point == CommitTestPoint::CommittedMarkerWrite,
            Some("committed-marker-after-replace") => {
                point == CommitTestPoint::CommittedMarkerAfterReplace
            }
            Some("committed-marker") => point == CommitTestPoint::CommittedState,
            Some("committed-state") => point == CommitTestPoint::Cleanup,
            _ => false,
        }
    }

    fn recovery_crash_selected(point: RecoveryTestPoint, operation: Option<u32>) -> bool {
        match migration_crash_point().as_deref() {
            Some("rolling-back-state") => {
                point == RecoveryTestPoint::BackupVerify && operation == Some(2)
            }
            Some("rollback-first") => {
                point == RecoveryTestPoint::ProgressState && operation == Some(1)
            }
            Some("rollback-middle") => {
                point == RecoveryTestPoint::ProgressState && operation == Some(0)
            }
            Some("rolled-back-marker") => point == RecoveryTestPoint::RolledBackState,
            _ => false,
        }
    }

    fn migration_crash_point() -> Option<String> {
        std::env::var_os("WORLDBUILD_MIGRATION_CRASH_PROJECT")?;
        std::env::var("WORLDBUILD_CRASH_POINT").ok()
    }

    fn pause_if_configured(selected: bool) -> io::Result<()> {
        if !selected {
            return Ok(());
        }
        let ready = std::env::var_os("WORLDBUILD_CRASH_READY")
            .ok_or_else(|| io::Error::other("migration crash ready path missing"))?;
        let point = migration_crash_point()
            .ok_or_else(|| io::Error::other("migration crash point missing"))?;
        publish_ready(Path::new(&ready), &point)?;
        loop {
            thread::park_timeout(Duration::from_secs(1));
        }
    }

    fn publish_ready(ready: &Path, payload: &str) -> io::Result<()> {
        let temporary = ready.with_extension("tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(payload.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, ready)
    }

    fn map_commit(point: CommitFailPoint) -> CommitTestPoint {
        match point {
            CommitFailPoint::ManifestRevalidation => CommitTestPoint::ManifestRevalidation,
            CommitFailPoint::ApplyingState => CommitTestPoint::ApplyingState,
            CommitFailPoint::TargetPrecondition => CommitTestPoint::TargetPrecondition,
            CommitFailPoint::StagedRevalidation => CommitTestPoint::StagedRevalidation,
            CommitFailPoint::Rename => CommitTestPoint::Rename,
            CommitFailPoint::TargetSync => CommitTestPoint::TargetSync,
            CommitFailPoint::TargetParentSync => CommitTestPoint::TargetParentSync,
            CommitFailPoint::AppliedVerification => CommitTestPoint::AppliedVerification,
            CommitFailPoint::ProgressState => CommitTestPoint::ProgressState,
            CommitFailPoint::CommittedMarkerWrite => CommitTestPoint::CommittedMarkerWrite,
            CommitFailPoint::CommittedMarkerAfterReplace => {
                CommitTestPoint::CommittedMarkerAfterReplace
            }
            CommitFailPoint::CommittedMarkerVerify => CommitTestPoint::CommittedMarkerVerify,
            CommitFailPoint::CommittedMarkerSync => CommitTestPoint::CommittedMarkerSync,
            CommitFailPoint::CommittedState => CommitTestPoint::CommittedState,
            CommitFailPoint::Cleanup => CommitTestPoint::Cleanup,
        }
    }

    fn map_recovery(point: RecoveryFailPoint) -> RecoveryTestPoint {
        match point {
            RecoveryFailPoint::Enumerate => RecoveryTestPoint::Enumerate,
            RecoveryFailPoint::RollingBackState => RecoveryTestPoint::RollingBackState,
            RecoveryFailPoint::BackupVerify => RecoveryTestPoint::BackupVerify,
            RecoveryFailPoint::RestoreExisting => RecoveryTestPoint::RestoreExisting,
            RecoveryFailPoint::RemoveNew => RecoveryTestPoint::RemoveNew,
            RecoveryFailPoint::RestoredVerify => RecoveryTestPoint::RestoredVerify,
            RecoveryFailPoint::ProgressState => RecoveryTestPoint::ProgressState,
            RecoveryFailPoint::RolledBackMarkerWrite => RecoveryTestPoint::RolledBackMarkerWrite,
            RecoveryFailPoint::RolledBackMarkerVerify => RecoveryTestPoint::RolledBackMarkerVerify,
            RecoveryFailPoint::RolledBackMarkerSync => RecoveryTestPoint::RolledBackMarkerSync,
            RecoveryFailPoint::RolledBackState => RecoveryTestPoint::RolledBackState,
            RecoveryFailPoint::Cleanup => RecoveryTestPoint::Cleanup,
            RecoveryFailPoint::CommittedTargetVerify => RecoveryTestPoint::CommittedTargetVerify,
            RecoveryFailPoint::JournalDecision => RecoveryTestPoint::JournalDecision,
        }
    }
}
