//! 프로젝트 잠금 획득과 복구 완료를 서로 다른 권한 단계로 다룬다.
mod diagnostics;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::with_release_fault;

use std::path::{Path, PathBuf};
use std::{fmt, fs, io::ErrorKind};

use super::project_lock::ProjectLock;
use super::transaction::{LockedProject, RecoveryReport};
pub(crate) use diagnostics::{
    InitializationCleanupOutcome, RuntimeCategory, RuntimeDiagnostic, RuntimeError, RuntimeStage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeState {
    Pending,
    Recovering,
    Ready,
    Blocked,
}

/// 관찰용 집계만 복사한다. ID 목록은 아예 내보내지 않으며 모든 count는 원 report 그대로다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoverySummary {
    pub(crate) discovered_transactions: usize,
    pub(crate) rolled_back_transactions: usize,
    pub(crate) committed_cleanups: usize,
    pub(crate) rolled_back_cleanups: usize,
    pub(crate) preparing_orphan_cleanups: usize,
    pub(crate) manual_recovery_required: bool,
}

impl From<&RecoveryReport> for RecoverySummary {
    fn from(report: &RecoveryReport) -> Self {
        Self {
            discovered_transactions: report.discovered_transactions,
            rolled_back_transactions: report.rolled_back_transactions,
            committed_cleanups: report.committed_cleanups,
            rolled_back_cleanups: report.rolled_back_cleanups,
            preparing_orphan_cleanups: report.preparing_orphan_cleanups,
            manual_recovery_required: report.manual_recovery_required(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeSnapshot {
    pub(crate) state: RuntimeState,
    /// 마지막 성공 report의 집계다. 현재 Ready 여부는 반드시 state로 구별한다.
    pub(crate) last_report: Option<RecoverySummary>,
    pub(crate) last_failure: Option<RuntimeDiagnostic>,
}

impl RuntimeSnapshot {
    pub(crate) fn next_action(&self) -> &'static str {
        match self.state {
            RuntimeState::Pending => "일반 자료 접근 전에 복구를 실행하거나 프로젝트를 닫으세요",
            RuntimeState::Recovering => "복구가 끝날 때까지 기다리세요",
            RuntimeState::Ready => "자료별 형식과 호환성을 검증한 뒤 접근하세요",
            RuntimeState::Blocked => self.last_failure.map_or(
                "복구 자료를 보존하고 프로젝트를 닫으세요",
                |failure| failure.next_action(),
            ),
        }
    }
}

pub(crate) struct ProjectRuntime {
    lock: ProjectLock,
    canonical_root: PathBuf,
    state: RuntimeState,
    access_revoked: bool,
    last_report: Option<RecoverySummary>,
    last_error: Option<RuntimeError>,
}

impl fmt::Debug for ProjectRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectRuntime")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

/// 생성자·필드를 공개하지 않는다. borrow가 살아 있는 동안 recover/invalidate/close는 불가능하다.
pub(crate) struct RecoveryReadyProject<'runtime> {
    project: LockedProject<'runtime>,
}

impl fmt::Debug for RecoveryReadyProject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryReadyProject([redacted])")
    }
}

impl RecoveryReadyProject<'_> {
    /// G3는 이 접근 타입을 입력으로 요구하고 작업 중에만 M1 binding을 빌린다.
    /// inner lock lifetime도 이 borrow로 줄여 접근권보다 오래 보유할 수 없게 한다.
    pub(crate) fn locked_project(&self) -> &LockedProject<'_> {
        &self.project
    }
}

impl ProjectRuntime {
    /// G5 preflight는 caller 문자열 대신 실제 소유한 프로젝트 잠금의 identity와 비교한다.
    pub(super) fn project_fingerprint(&self) -> &str {
        self.lock.fingerprint()
    }

    pub(crate) fn acquire(project_root: &Path, lock_root: &Path) -> Result<Self, RuntimeError> {
        let lock = ProjectLock::try_acquire(project_root, lock_root).map_err(RuntimeError::lock)?;
        Self::bind_acquired(lock, project_root)
    }

    /// 새 프로젝트는 필요할 때 정확한 root 하나를 만든 뒤 잠금과 실제 root handle로 빈 폴더를 확인한다.
    /// 현재 형식은 빈 프로젝트 manifest를 요구하지 않으므로 사용자 파일을 만들지 않는다.
    pub(crate) fn acquire_empty(
        project_root: &Path,
        lock_root: &Path,
        create_directory: bool,
    ) -> Result<Self, RuntimeError> {
        let created = if create_directory {
            match fs::create_dir(project_root) {
                Ok(()) => true,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
                Err(error) => return Err(RuntimeError::project_initialization(error)),
            }
        } else {
            false
        };
        let acquired = (|| {
            let runtime = Self::acquire(project_root, lock_root)?;
            let root = super::project_file::directory::ProjectDirectory::open_root(
                &runtime.canonical_root,
            )
            .map_err(RuntimeError::project_initialization)?;
            let mut entries = root
                .read_dir()
                .map_err(RuntimeError::project_initialization)?;
            match entries.next() {
                Some(Ok(_)) => return Err(RuntimeError::project_not_empty()),
                Some(Err(error)) => return Err(RuntimeError::project_initialization(error)),
                None => {}
            }
            root.validate()
                .map_err(RuntimeError::project_initialization)?;
            Ok(runtime)
        })();
        if acquired.is_err() && created {
            // 이번 요청이 만든 정확한 빈 디렉터리만 되돌린다. 외부 파일이 생겼다면
            // remove_dir가 실패하므로 사용자 자료를 재귀적으로 지우지 않는다.
            let cleanup = {
                #[cfg(test)]
                {
                    tests::remove_created_directory(project_root)
                }
                #[cfg(not(test))]
                {
                    fs::remove_dir(project_root)
                }
            };
            return acquired.map_err(|error| error.with_initialization_cleanup(cleanup));
        }
        acquired
    }

    fn bind_acquired(lock: ProjectLock, project_root: &Path) -> Result<Self, RuntimeError> {
        let canonical_root = match LockedProject::bind(&lock, project_root) {
            Ok(project) => project.canonical_root().to_path_buf(),
            Err(source) => {
                let error = RuntimeError::binding(source);
                // binding 실패도 획득한 자원은 명시적으로 해제한다. 2차 실패로 원인을 덮지 않는다.
                return Err(match release_lock(lock) {
                    Ok(()) => error,
                    Err(release) => error.with_release(release),
                });
            }
        };
        Ok(Self {
            lock,
            canonical_root,
            state: RuntimeState::Pending,
            access_revoked: false,
            last_report: None,
            last_error: None,
        })
    }

    pub(crate) fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            state: if self.access_revoked {
                RuntimeState::Pending
            } else {
                self.state
            },
            last_report: self.last_report,
            last_failure: self.last_error.as_ref().map(RuntimeError::diagnostic),
        }
    }

    pub(crate) fn shutdown_snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            state: self.state,
            last_report: self.last_report,
            last_failure: self.last_error.as_ref().map(RuntimeError::diagnostic),
        }
    }

    /// 명시적 호출마다 한 번 재검사한다. Ready에서 호출해도 먼저 권한을 내린다.
    /// 실패 시 self를 소비하지 않으므로 동일 잠금으로 재시도하거나 닫을 수 있다.
    pub(crate) fn recover(&mut self) -> Result<RecoverySummary, RuntimeError> {
        self.access_revoked = false;
        self.state = RuntimeState::Recovering;
        #[cfg(test)]
        tests::observe_recovering(self);
        let result: Result<RecoveryReport, RuntimeError> = (|| {
            // 먼저 기존 lock과 canonical root가 아직 같은 프로젝트를 가리키는지 묶는다.
            // 이는 제품 reader 발급이 아니라 recovery가 다른/사라진 root를 만지지 않게 하는
            // 권한 경계이며, binding 실패를 일반 recovery 실패로 가리지 않는다.
            let project = LockedProject::bind(&self.lock, &self.canonical_root)
                .map_err(RuntimeError::binding)?;
            // restore commit marker는 일반 reader와 기존 JSON transaction 복구보다 먼저
            // 완결한다. 알 수 없는 journal은 보존하고 Ready를 발급하지 않는다.
            super::project_backup::recover_pending(&self.canonical_root)
                .map_err(RuntimeError::restore_recovery)?;
            #[cfg(not(test))]
            let report = super::transaction::recover_pending_transactions(&project);
            #[cfg(test)]
            let report = super::transaction::test_support::recover_for_runtime_test(&project);
            let report = report.map_err(RuntimeError::recovery)?;
            // 문서 purge intent는 canonical layout transaction의 commit/rollback 결과를
            // 기준으로 file 삭제를 수렴시킨다. 따라서 transaction recovery 뒤이면서
            // 일반 reader 발급 전인 이 위치에서 모든 maintenance intent를 완결한다.
            super::asset_maintenance::recover_pending(&self.canonical_root)
                .map_err(RuntimeError::maintenance_recovery)?;
            Ok(report)
        })();
        match result {
            Ok(report) => self.finish_recovery(report),
            Err(error) => {
                self.block(error.clone());
                Err(error)
            }
        }
    }

    fn finish_recovery(&mut self, report: RecoveryReport) -> Result<RecoverySummary, RuntimeError> {
        let summary = RecoverySummary::from(&report);
        self.last_report = Some(summary);
        if report.manual_recovery_required() {
            let error = RuntimeError::closed(
                RuntimeCategory::ManualRecoveryRequired,
                RuntimeStage::ReviewReport,
            );
            self.block(error.clone());
            return Err(error);
        }
        self.last_error = None;
        self.state = RuntimeState::Ready;
        Ok(summary)
    }

    pub(crate) fn ready(&mut self) -> Result<RecoveryReadyProject<'_>, RuntimeError> {
        if self.access_revoked || self.state != RuntimeState::Ready {
            return Err(self.last_error.clone().unwrap_or_else(|| {
                RuntimeError::closed(RuntimeCategory::RecoveryPending, RuntimeStage::Access)
            }));
        }
        match LockedProject::bind(&self.lock, &self.canonical_root) {
            Ok(project) => Ok(RecoveryReadyProject { project }),
            Err(source) => {
                let error = RuntimeError::binding(source);
                // 성공 binding은 lock을 빌려 반환하므로 실패 기록은 서로 다른 필드에만 쓴다.
                // 경로가 돌아와도 관측한 실패를 잊지 않고 실제 복구 전까지 발급을 막는다.
                self.state = RuntimeState::Blocked;
                self.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// G5 연결용 단방향 전환이다. 완료 snapshot이나 bool로 권한을 올리는 API는 없다.
    pub(crate) fn invalidate_recovery(&mut self) {
        self.access_revoked = true;
    }

    pub(crate) fn prepare_close_after_invalidation(&mut self) {
        self.access_revoked = false;
    }

    fn block(&mut self, error: RuntimeError) {
        self.access_revoked = false;
        self.state = RuntimeState::Blocked;
        self.last_error = Some(error);
    }

    /// 반환 snapshot은 닫기 직전 관찰값이며 접근권이 아니다. 실패해도 guard는 소비된다.
    pub(crate) fn close(self) -> Result<RuntimeSnapshot, Box<RuntimeCloseError>> {
        let snapshot = self.snapshot();
        release_lock(self.lock).map_err(|release| {
            Box::new(RuntimeCloseError {
                snapshot,
                previous_failure: self.last_error,
                release,
            })
        })?;
        Ok(snapshot)
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeCloseError {
    pub(crate) snapshot: RuntimeSnapshot,
    pub(crate) previous_failure: Option<RuntimeError>,
    pub(crate) release: RuntimeError,
}

impl fmt::Display for RuntimeCloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "프로젝트 닫기 실패: {}; 복구 자료를 보존하고 잠금 상태를 확인하세요",
            self.release
        )
    }
}

impl std::error::Error for RuntimeCloseError {}

fn release_lock(lock: ProjectLock) -> Result<(), RuntimeError> {
    #[cfg(test)]
    return tests::release_with_fault(lock).map_err(RuntimeError::lock);
    #[cfg(not(test))]
    lock.release().map_err(RuntimeError::lock)
}
