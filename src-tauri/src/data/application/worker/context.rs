//! mutable runtime/session을 외부에 반환하지 않는 동기 작업 view와 정리 전용 view.
mod guarded;
mod whole;
use super::*;
use crate::data::{
    collaboration_lock::{LockError, LockSetReleaseError},
    edit_session::RetainedFailure,
};

/// 해제 성공으로 service 진단이 사라져도 정리 요청의 원인 기록은 결과 owner에게 남긴다.
/// lock handle, provider token과 P/R을 복제하거나 공개하지 않는다.
#[derive(Debug, Clone)]
pub(crate) struct SessionReleaseObservation {
    pub(crate) snapshot: EditSessionSnapshot,
    pub(crate) acquire_error: Option<LockError>,
    pub(crate) release: Option<SessionReleaseDiagnostics>,
}
#[derive(Debug, Clone)]
pub(crate) struct SessionReleaseDiagnostics {
    pub(crate) first: LockSetReleaseError,
    pub(crate) latest: LockSetReleaseError,
    pub(crate) retry_attempts: usize,
    pub(crate) failure_count: usize,
}
#[derive(Debug)]
#[must_use]
pub(crate) struct SessionReleaseRetry {
    pub(crate) before: SessionReleaseObservation,
    pub(crate) original: Result<(), EditSessionError>,
    pub(crate) after: SessionReleaseObservation,
}
fn observe_release<P, R>(service: &EditSessionService<P, R>) -> SessionReleaseObservation {
    let (acquire_error, release) = match service.retained_failure() {
        Some(RetainedFailure::PartialAcquire {
            acquire_error,
            release_diagnostics,
        }) => (Some(acquire_error.clone()), Some(release_diagnostics)),
        Some(
            RetainedFailure::Release {
                release_diagnostics,
                ..
            }
            | RetainedFailure::Recovery {
                release_diagnostics,
                ..
            },
        ) => (None, release_diagnostics),
        _ => (None, None),
    };
    SessionReleaseObservation {
        snapshot: service.snapshot(),
        acquire_error,
        release: release.map(|d| SessionReleaseDiagnostics {
            first: d.first().clone(),
            latest: d.latest().clone(),
            retry_attempts: d.retry_attempts(),
            failure_count: d.failure_count(),
        }),
    }
}
use crate::data::{
    application::{
        bulk_replace::{self, BulkReplaceInput, BulkReplaceRequest},
        composite::{CompositeSaveExecution, CompositeSaveInput, CompositeWriteRequest},
        diagnostics::ApplicationError,
        documents::persistence::{self, DocumentUpdateExecution, SaveDocumentInput},
        recovery_handoff::{self, Acknowledged, DirtySaveResult, DraftCustody, RecoveryBackend},
        templates::TemplateWriteContext,
    },
    artifact::DocumentSaveOutcome,
    collaboration_lock::LockService,
    edit_session::{EditSessionService, EditSessionSnapshot},
    project_relative_path::ProjectRelativePath,
    project_runtime::{RecoveryReadyProject, RuntimeCloseError},
};

pub(super) struct Entry<P, R> {
    registration: Registration,
    pub(super) service: EditSessionService<P, R>,
    backend: RecoveryBackend<P, R>,
    draft: Option<DraftCustody<P>>,
}
#[derive(Debug)]
pub(crate) struct SessionRegistration {
    pub(crate) registration: Registration,
    pub(crate) key: Option<SessionKey>,
    pub(crate) original: Result<(), EditSessionError>,
}
#[derive(Debug)]
pub(crate) enum CompositeDispatchError {
    Request(ApplicationError),
    Handoff(HandoffError),
}
pub(crate) struct DraftRejected<P> {
    pub(crate) input: P,
    pub(crate) category: WorkerCategory,
    pub(crate) original: Option<HandoffError>,
}
impl<P> fmt::Debug for DraftRejected<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftRejected")
            .field("category", &self.category)
            .field("original", &self.original)
            .finish_non_exhaustive()
    }
}
pub(crate) struct WorkerContext<P, R> {
    pub(super) runtime: Option<ProjectRuntime>,
    // 현재 프로젝트의 저장 집합이 런타임 밖에서 교체된 뒤에는 새 접근권을 발급하지 않는다.
    // 정상 종료 정리 슬롯만 이 표시를 소비해 새 저장 집합을 재검증하고 lock을 닫는다.
    pub(super) runtime_invalidated: bool,
    input_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(super) sessions: Vec<Entry<P, R>>,
    marker: Arc<()>,
    next_registration: u64,
    max_sessions: usize,
    pub(super) retained_failure_owners: Vec<Registration>,
    // 외부로 반환한 미저장 P는 등록이 제거되어도 프로젝트 정상 종료 근거가 되지 않는다.
    pub(super) caller_custody: usize,
    pub(super) runtime_close: Option<super::super::shutdown::CloseObservation>,
    // 기존 실제 소비형 close의 반환 오류만 worker 안에서 관찰하는 test-only 경계다.
    #[cfg(test)]
    pub(super) close_runtime_with:
        fn(ProjectRuntime) -> Result<RuntimeSnapshot, Box<RuntimeCloseError>>,
}
impl<P, R> WorkerContext<P, R> {
    pub(super) fn new(runtime: ProjectRuntime, marker: Arc<()>, max_sessions: usize) -> Self {
        Self {
            runtime: Some(runtime),
            runtime_invalidated: false,
            input_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)),
            sessions: Vec::new(),
            marker,
            next_registration: 0,
            max_sessions,
            retained_failure_owners: Vec::new(),
            caller_custody: 0,
            runtime_close: None,
            #[cfg(test)]
            close_runtime_with: ProjectRuntime::close,
        }
    }
    pub(super) fn key(&self, entry: &Entry<P, R>) -> Option<SessionKey> {
        let snapshot = entry.service.snapshot();
        Some(SessionKey {
            registration: entry.registration.clone(),
            project: snapshot.project_fingerprint()?.to_owned(),
            session: snapshot.session_id()?.clone(),
        })
    }
    pub(super) fn current(&self, key: &SessionKey) -> Result<&Entry<P, R>, WorkerCategory> {
        let entry = self
            .sessions
            .iter()
            .find(|e| e.registration == key.registration)
            .ok_or(WorkerCategory::Stale)?;
        if self.key(entry).as_ref() != Some(key) {
            return Err(WorkerCategory::Stale);
        }
        if self
            .runtime
            .as_ref()
            .map(ProjectRuntime::project_fingerprint)
            != Some(key.project.as_str())
        {
            return Err(WorkerCategory::Stale);
        }
        Ok(entry)
    }
    pub(crate) fn begin_session(
        &mut self,
        provider: Arc<dyn LockService>,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<SessionRegistration, WorkerCategory> {
        self.register_template_owner(provider, Some(targets))
    }
    /// 미공개 생성은 worker 등록만 한다. 실제 edit session/permit은 명시적 첫 저장 때 얻는다.
    pub(crate) fn register_template_owner(
        &mut self,
        provider: Arc<dyn LockService>,
        targets: Option<Vec<ProjectRelativePath>>,
    ) -> Result<SessionRegistration, WorkerCategory> {
        if self.sessions.len() == self.max_sessions {
            return Err(WorkerCategory::Full);
        }
        let rt = self.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        self.next_registration = self
            .next_registration
            .checked_add(1)
            .ok_or(WorkerCategory::Exhausted)?;
        let registration = Registration {
            worker: self.marker.clone(),
            serial: self.next_registration,
        };
        let mut entry = Entry {
            registration: registration.clone(),
            service: EditSessionService::new(provider),
            backend: RecoveryBackend::default(),
            draft: None,
        };
        let original = match targets {
            Some(targets) => entry.service.begin_edit(rt.project_fingerprint(), targets),
            None => Ok(()),
        };
        let key = self.key(&entry);
        // 부분 acquire cleanup 실패도 session이 소유하므로 실패한 등록을 Drop하지 않는다.
        self.sessions.push(entry);
        Ok(SessionRegistration {
            registration,
            key,
            original,
        })
    }
    pub(crate) fn session_key(&self, registration: &Registration) -> Option<SessionKey> {
        self.sessions
            .iter()
            .find(|e| e.registration == *registration)
            .and_then(|e| self.key(e))
    }
    pub(crate) fn session_snapshot(
        &self,
        registration: &Registration,
    ) -> Result<EditSessionSnapshot, WorkerCategory> {
        self.sessions
            .iter()
            .find(|e| e.registration == *registration)
            .map(|e| e.service.snapshot())
            .ok_or(WorkerCategory::Stale)
    }
    pub(crate) fn runtime_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.runtime.as_ref().map(ProjectRuntime::snapshot)
    }
    pub(crate) fn read<T>(
        &mut self,
        read: impl FnOnce(&RecoveryReadyProject<'_>) -> T,
    ) -> Result<T, ApplicationError> {
        let rt = self.runtime.as_mut().ok_or_else(|| {
            ApplicationError::closed(
                super::super::diagnostics::ApplicationCategory::RuntimeRejected,
            )
        })?;
        let ready = rt.ready().map_err(ApplicationError::runtime)?;
        Ok(read(&ready))
    }

    pub(crate) fn preserve_latest_input(
        &self,
        envelope: crate::data::edit_recovery::model::Envelope,
    ) -> std::io::Result<crate::data::repository::drafts::Checkpoint> {
        if self.runtime_invalidated {
            return Err(std::io::Error::other("project access invalidated"));
        }
        self.runtime
            .as_ref()
            .ok_or_else(|| std::io::Error::other("project closed"))?
            .preserve_latest_input(envelope)
    }

    /// A new owner may resume an old unknown save only after actual runtime
    /// recovery and when no other session/caller retains work on that target.
    pub(crate) fn can_recheck_latest_input(
        &mut self,
        own: &Registration,
        target: &ProjectRelativePath,
    ) -> bool {
        if self.runtime_invalidated || self.caller_custody != 0 {
            return false;
        }
        let Some(current) = self
            .sessions
            .iter()
            .find(|entry| &entry.registration == own)
        else {
            return false;
        };
        if current.draft.is_some()
            || current.service.snapshot().state()
                != crate::data::edit_session::EditSessionState::Editing
            || !current.service.snapshot().targets().contains(target)
        {
            return false;
        }
        if !self.retained_failure_owners.is_empty()
            || self.sessions.iter().any(|entry| {
                &entry.registration != own
                    && (entry.draft.is_some()
                        || entry.service.snapshot().targets().contains(target))
            })
        {
            return false;
        }
        self.runtime
            .as_mut()
            .is_some_and(|runtime| runtime.ready().is_ok())
    }

    pub(crate) fn has_project_owners(&self) -> bool {
        !self.sessions.is_empty() || self.caller_custody != 0
    }

    pub(crate) fn session(
        &mut self,
        key: &SessionKey,
    ) -> Result<SessionWork<'_, P, R>, WorkerCategory> {
        self.current(key)?;
        let blocked = self.retained_failure_owners.contains(&key.registration);
        let runtime = self.runtime.as_mut().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .sessions
            .iter_mut()
            .find(|e| e.registration == key.registration)
            .ok_or(WorkerCategory::Stale)?;
        Ok(SessionWork {
            runtime,
            entry,
            identity_change_blocked: blocked,
            key: key.clone(),
        })
    }
    pub(crate) fn recover(
        &mut self,
    ) -> Result<crate::data::project_runtime::RecoverySummary, ApplicationError> {
        self.runtime
            .as_mut()
            .ok_or_else(|| {
                ApplicationError::closed(
                    super::super::diagnostics::ApplicationCategory::RuntimeRejected,
                )
            })?
            .recover()
            .map_err(ApplicationError::runtime)
    }
    pub(crate) fn invalidate_runtime(&mut self) -> Result<(), ApplicationError> {
        self.runtime
            .as_mut()
            .ok_or_else(|| {
                ApplicationError::closed(
                    super::super::diagnostics::ApplicationCategory::RuntimeRejected,
                )
            })?
            .invalidate_recovery();
        self.runtime_invalidated = true;
        self.input_epoch
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    }
    pub(super) fn revalidate(
        &mut self,
        key: &SessionKey,
        reasons: Reasons,
    ) -> (ValidationCompletion, Option<RevalidationFailure>) {
        let work = match self.session(key) {
            Ok(work) => work,
            Err(_) => return (ValidationCompletion::Stale, None),
        };
        if work.entry.service.snapshot().state() != EditSessionState::Editing {
            return (ValidationCompletion::Inactive, None);
        }
        match work.entry.service.revalidate() {
            Ok(()) => (ValidationCompletion::Validated, None),
            Err(original) => {
                let preserve = work
                    .entry
                    .draft
                    .as_mut()
                    .filter(|d| d.has_active())
                    .map(|draft| draft.preserve_existing(work.runtime, &mut work.entry.service));
                (
                    ValidationCompletion::Failed,
                    Some(RevalidationFailure {
                        key: key.clone(),
                        reasons,
                        original,
                        preserve,
                    }),
                )
            }
        }
    }
}

/// 모든 borrow는 한 worker job 안에서 끝난다. guard/runtime/service 자체를 꺼내는 accessor는 없다.
pub(crate) struct SessionWork<'a, P, R> {
    runtime: &'a mut ProjectRuntime,
    entry: &'a mut Entry<P, R>,
    identity_change_blocked: bool,
    key: SessionKey,
}
impl<P, R> SessionWork<'_, P, R> {
    pub(crate) fn snapshot(&self) -> EditSessionSnapshot {
        self.entry.service.snapshot()
    }
    pub(crate) fn bind_draft(&mut self, payload: P) -> Result<(), DraftRejected<P>> {
        if self.entry.draft.is_some() {
            return Err(DraftRejected {
                input: payload,
                category: WorkerCategory::OwnersRemain,
                original: None,
            });
        }
        match DraftCustody::bind(self.runtime, &self.entry.service, payload) {
            Ok(draft) => {
                self.entry.draft = Some(draft);
                Ok(())
            }
            Err(failure) => {
                let (payload, error) = failure.into_parts();
                Err(DraftRejected {
                    input: payload,
                    category: WorkerCategory::Stale,
                    original: Some(error),
                })
            }
        }
    }
    pub(crate) fn connect_backend(
        &mut self,
        backend: RecoveryBackend<P, R>,
    ) -> Result<(), RecoveryBackend<P, R>> {
        if self.entry.backend.capability() == recovery_handoff::SinkCapability::Connected {
            return Err(backend);
        }
        self.entry.backend = backend;
        Ok(())
    }
    pub(crate) fn save_document(
        &mut self,
        input: &SaveDocumentInput,
    ) -> Result<DocumentUpdateExecution<DocumentSaveOutcome>, ApplicationError> {
        if self.entry.draft.is_some()
            && self.entry.service.snapshot().state() == EditSessionState::Editing
        {
            return Err(ApplicationError::closed(
                super::super::diagnostics::ApplicationCategory::InvalidSessionState,
            ));
        }
        persistence::save_document(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn apply_bulk_replace(
        &mut self,
        input: &BulkReplaceInput,
    ) -> Result<
        crate::data::application::write::WriteExecution<(), bulk_replace::BulkReplaceError>,
        ApplicationError,
    > {
        if self.entry.draft.is_some() {
            return Err(ApplicationError::closed(
                super::super::diagnostics::ApplicationCategory::InvalidSessionState,
            ));
        }
        let request = BulkReplaceRequest::new(input)?;
        Ok(bulk_replace::apply(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            &request,
        ))
    }
    pub(crate) fn save_dirty_document(
        &mut self,
        input: &SaveDocumentInput,
    ) -> Result<
        DirtySaveResult<Result<DocumentUpdateExecution<DocumentSaveOutcome>, ApplicationError>>,
        HandoffError,
    > {
        let draft = self.entry.draft.as_mut().ok_or(HandoffError::Rejected(
            recovery_handoff::HandoffCategory::MissingActiveDraft,
        ))?;
        recovery_handoff::save_dirty_document(
            self.runtime,
            &mut self.entry.service,
            &self.entry.backend,
            draft,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn save_dirty_composite(
        &mut self,
        input: &CompositeSaveInput,
    ) -> Result<DirtySaveResult<CompositeSaveExecution>, CompositeDispatchError> {
        let request = CompositeWriteRequest::new(input).map_err(CompositeDispatchError::Request)?;
        let draft = self.entry.draft.as_mut().ok_or_else(|| {
            CompositeDispatchError::Handoff(HandoffError::Rejected(
                recovery_handoff::HandoffCategory::MissingActiveDraft,
            ))
        })?;
        recovery_handoff::save_dirty_composite(
            self.runtime,
            &mut self.entry.service,
            &self.entry.backend,
            draft,
            context(&self.key),
            &request,
        )
        .map_err(CompositeDispatchError::Handoff)
    }
    pub(crate) fn change_targets(
        &mut self,
        targets: Vec<ProjectRelativePath>,
    ) -> Result<Result<(), EditSessionError>, WorkerCategory> {
        if self.entry.draft.is_some() || self.identity_change_blocked {
            return Err(WorkerCategory::OwnersRemain);
        }
        Ok(self.entry.service.change_targets(targets))
    }
    pub(crate) fn preserve_existing(&mut self) -> Result<CustodyTransition, HandoffError> {
        self.entry
            .draft
            .as_mut()
            .ok_or(HandoffError::Rejected(
                recovery_handoff::HandoffCategory::MissingActiveDraft,
            ))?
            .preserve_existing(self.runtime, &mut self.entry.service)
    }
    pub(crate) fn accept(&mut self) -> Result<(), HandoffError> {
        accept(self.runtime, self.entry)
    }
    pub(crate) fn acknowledge(&mut self) -> Result<Acknowledged<R>, HandoffError> {
        acknowledge(self.runtime, self.entry)
    }
}
fn context(key: &SessionKey) -> TemplateWriteContext<'_> {
    TemplateWriteContext {
        project: &key.project,
        // view 안에서 ack가 identity를 지워도 옛 context를 재획득 권한으로 승격하지 않는다.
        session: &key.session,
    }
}
fn accept<P, R>(runtime: &ProjectRuntime, entry: &mut Entry<P, R>) -> Result<(), HandoffError> {
    entry
        .draft
        .as_ref()
        .ok_or(HandoffError::Rejected(
            recovery_handoff::HandoffCategory::MissingActiveDraft,
        ))?
        .accept(runtime, &mut entry.service, &mut entry.backend)
}
fn acknowledge<P, R>(
    runtime: &ProjectRuntime,
    entry: &mut Entry<P, R>,
) -> Result<Acknowledged<R>, HandoffError> {
    entry
        .draft
        .as_mut()
        .ok_or(HandoffError::Rejected(
            recovery_handoff::HandoffCategory::MissingActiveDraft,
        ))?
        .acknowledge(runtime, &mut entry.service)
}

/// 일반 admission을 닫은 뒤 별도 한 칸으로 요청한다. end/인수/반환은 모두 명시적 호출이다.
pub(crate) struct CleanupContext<'a, P, R>(pub(super) &'a mut WorkerContext<P, R>);
impl<P, R> CleanupContext<'_, P, R> {
    /// Read-only proof for cancelling an unresolved comparison. An active
    /// worker payload must use its normal custody/release path instead.
    pub(crate) fn hold_comparison_input(
        &mut self,
        registration: &Registration,
        checkpoint: &crate::data::repository::drafts::Checkpoint,
    ) -> std::io::Result<crate::data::repository::drafts::VerifiedCheckpoint> {
        let session = self
            .0
            .sessions
            .iter()
            .find(|entry| entry.registration == *registration)
            .ok_or_else(|| std::io::Error::other("comparison session unavailable"))?;
        if session.draft.is_some() {
            return Err(std::io::Error::other("comparison retains active work"));
        }
        self.0
            .read(|ready| {
                let repository = crate::data::repository::ArtifactRepository::new(ready)
                    .map_err(std::io::Error::other)?;
                checkpoint.hold_current(&repository)
            })
            .map_err(std::io::Error::other)?
    }
    pub(crate) fn preserve_latest_input(
        &self,
        envelope: crate::data::edit_recovery::model::Envelope,
    ) -> std::io::Result<crate::data::repository::drafts::Checkpoint> {
        self.0.preserve_latest_input(envelope)
    }

    pub(crate) fn latest_input_sink(
        &self,
    ) -> std::io::Result<crate::data::repository::drafts::InputSink> {
        if self.0.runtime_invalidated {
            return Err(std::io::Error::other("project access invalidated"));
        }
        self.0
            .runtime
            .as_ref()
            .ok_or_else(|| std::io::Error::other("project closed"))?
            .latest_input_sink(self.0.input_epoch.clone())
    }

    /// A confirmed content version is cleanup metadata, never a canonical write.
    /// The editing session must already have ended successfully.
    pub(crate) fn confirm_content_version(
        &mut self,
        registration: &Registration,
        id: crate::data::repository::ArtifactSourceId,
        timestamp: &str,
        expected: &str,
    ) -> std::io::Result<u64> {
        let entry = self
            .0
            .sessions
            .iter()
            .find(|entry| entry.registration == *registration)
            .ok_or_else(|| std::io::Error::other("version session unavailable"))?;
        if entry.service.snapshot().state() != crate::data::edit_session::EditSessionState::ReadOnly
        {
            return Err(std::io::Error::other(
                "version requires a completed editing session",
            ));
        }
        self.0
            .read(|ready| {
                let repository = crate::data::repository::ArtifactRepository::new(ready)
                    .map_err(std::io::Error::other)?;
                crate::data::repository::versions::confirm_source(
                    &repository,
                    id,
                    timestamp,
                    Some(expected),
                )
            })
            .map_err(std::io::Error::other)?
    }
    pub(in crate::data::application) fn shutdown_sessions(
        &self,
    ) -> Vec<super::super::shutdown::SessionOwner> {
        use super::super::shutdown::{Custody, SessionOwner};
        self.0
            .sessions
            .iter()
            .map(|entry| {
                let snapshot = entry.service.snapshot();
                let custody = if entry.draft.as_ref().is_some_and(DraftCustody::has_active) {
                    Custody::Active
                } else if entry
                    .draft
                    .as_ref()
                    .is_some_and(DraftCustody::is_acknowledged)
                {
                    Custody::Acknowledged
                } else {
                    match snapshot.recovery_handoff() {
                        Some(crate::data::edit_session::RecoveryHandoffStatus::Pending) => {
                            Custody::Pending
                        }
                        Some(crate::data::edit_session::RecoveryHandoffStatus::DurablyAccepted) => {
                            Custody::Receipt
                        }
                        Some(_) => Custody::InProgress,
                        None => Custody::Empty,
                    }
                };
                SessionOwner {
                    registration: entry.registration.clone(),
                    release: observe_release(&entry.service),
                    release_pending: entry.service.has_release_work(),
                    can_preserve_existing: self.0.runtime.as_ref().is_some_and(|runtime| {
                        entry.draft.as_ref().is_some_and(|draft| {
                            draft.can_preserve_existing(runtime, &entry.service)
                        })
                    }),
                    custody,
                    capability: entry.backend.capability(),
                }
            })
            .collect()
    }
    pub(in crate::data::application) fn shutdown_runtime(&mut self) -> Option<RuntimeSnapshot> {
        self.0.runtime.as_ref().map(|runtime| {
            if self.0.runtime_invalidated {
                runtime.shutdown_snapshot()
            } else {
                runtime.snapshot()
            }
        })
    }
    pub(in crate::data::application) fn caller_custody(&self) -> usize {
        self.0.caller_custody
    }
    pub(in crate::data::application) fn runtime_close_observation(
        &self,
    ) -> Option<super::super::shutdown::CloseObservation> {
        self.0.runtime_close.clone()
    }
    /// admission을 다시 열지 않고 같은 runtime의 실제 G2 recovery를 한 번 실행한다.
    pub(crate) fn recover_runtime(
        &mut self,
    ) -> Result<crate::data::project_runtime::RecoverySummary, ApplicationError> {
        self.0.recover()
    }
    /// 실제 backend만 연결한다. 이미 연결된 sink나 잘못된 등록은 입력 owner째 반환한다.
    pub(crate) fn connect_backend(
        &mut self,
        registration: &Registration,
        backend: RecoveryBackend<P, R>,
    ) -> Result<(), RecoveryBackend<P, R>> {
        let Some(entry) = self
            .0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
        else {
            return Err(backend);
        };
        if entry.backend.capability() == recovery_handoff::SinkCapability::Connected {
            return Err(backend);
        }
        entry.backend = backend;
        Ok(())
    }
    pub(crate) fn end_session_observed(
        &mut self,
        registration: &Registration,
    ) -> Result<SessionReleaseRetry, WorkerCategory> {
        let entry = self
            .0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)?;
        let before = observe_release(&entry.service);
        let original = entry.service.end_edit();
        let after = observe_release(&entry.service);
        Ok(SessionReleaseRetry {
            before,
            original,
            after,
        })
    }
    pub(crate) fn preserve_existing(
        &mut self,
        registration: &Registration,
    ) -> Result<Result<CustodyTransition, HandoffError>, WorkerCategory> {
        let rt = self.0.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)?;
        Ok(entry
            .draft
            .as_mut()
            .ok_or(HandoffError::Rejected(
                recovery_handoff::HandoffCategory::MissingActiveDraft,
            ))
            .and_then(|draft| draft.preserve_existing(rt, &mut entry.service)))
    }
    fn entry(&mut self, registration: &Registration) -> Result<&mut Entry<P, R>, WorkerCategory> {
        self.0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)
    }
    pub(crate) fn end_session(
        &mut self,
        registration: &Registration,
    ) -> Result<Result<(), EditSessionError>, WorkerCategory> {
        Ok(self.entry(registration)?.service.end_edit())
    }
    pub(crate) fn session_release_observation(
        &mut self,
        registration: &Registration,
    ) -> Result<SessionReleaseObservation, WorkerCategory> {
        Ok(observe_release(&self.entry(registration)?.service))
    }
    /// 해당 worker/project에 묶인 등록 owner로 찾으므로 partial cleanup도 도달한다.
    /// 한 요청은 하위 service 한 pass다. 자동 반복, 재획득, accept/ack는 하지 않는다.
    pub(crate) fn retry_session_release(
        &mut self,
        registration: &Registration,
    ) -> Result<SessionReleaseRetry, WorkerCategory> {
        let entry = self.entry(registration)?;
        let before = observe_release(&entry.service);
        let original = entry.service.retry_release();
        // 성공 뒤 identity가 없어져도 같은 entry에서 관측하고 이미 얻은 Result를 돌려준다.
        let after = observe_release(&entry.service);
        Ok(SessionReleaseRetry {
            before,
            original,
            after,
        })
    }
    pub(crate) fn return_active(
        &mut self,
        registration: &Registration,
    ) -> Result<P, WorkerCategory> {
        let entry = self.entry(registration)?;
        let draft = entry.draft.take().ok_or(WorkerCategory::Inactive)?;
        match draft.into_active() {
            Ok(payload) => {
                self.0.caller_custody = self.0.caller_custody.saturating_add(1);
                Ok(payload)
            }
            Err(draft) => {
                entry.draft = Some(draft);
                Err(WorkerCategory::OwnersRemain)
            }
        }
    }
    pub(crate) fn accept(
        &mut self,
        registration: &Registration,
    ) -> Result<Result<(), HandoffError>, WorkerCategory> {
        let rt = self.0.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)?;
        Ok(accept(rt, entry))
    }
    pub(crate) fn acknowledge(
        &mut self,
        registration: &Registration,
    ) -> Result<Result<Acknowledged<R>, HandoffError>, WorkerCategory> {
        let rt = self.0.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .0
            .sessions
            .iter_mut()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)?;
        Ok(acknowledge(rt, entry))
    }
    pub(crate) fn remove_session(
        &mut self,
        registration: &Registration,
    ) -> Result<(), WorkerCategory> {
        if self.0.retained_failure_owners.contains(registration) {
            return Err(WorkerCategory::OwnersRemain);
        }
        let entry = self.entry(registration)?;
        if entry.service.snapshot().state() != EditSessionState::ReadOnly
            || entry.draft.as_ref().is_some_and(|d| !d.is_acknowledged())
        {
            return Err(WorkerCategory::OwnersRemain);
        }
        self.0.sessions.retain(|e| e.registration != *registration);
        Ok(())
    }
    pub(crate) fn close_runtime(
        &mut self,
    ) -> Result<Result<RuntimeSnapshot, Box<RuntimeCloseError>>, WorkerCategory> {
        if !self.0.sessions.is_empty() {
            return Err(WorkerCategory::OwnersRemain);
        }
        let mut runtime = self.0.runtime.take().ok_or(WorkerCategory::Unavailable)?;
        if self.0.runtime_invalidated {
            // 외부 current-restore 뒤에는 일반 접근권만 폐기됐다. admission이 닫힌
            // cleanup lane에서만 정상 close snapshot을 허용한다.
            runtime.prepare_close_after_invalidation();
            self.0.runtime_invalidated = false;
        }
        #[cfg(test)]
        let result = (self.0.close_runtime_with)(runtime);
        #[cfg(not(test))]
        let result = runtime.close();
        self.0.runtime_close = Some(super::super::shutdown::CloseObservation::from_result(
            &result,
        ));
        Ok(result)
    }
}
