//! 저장 결과와 opaque 편집 원본의 소유권을 연결한다. 실제 durable 저장은 M3 adapter 책임이다.
use std::fmt;

use super::{
    composite::{self, CompositeSaveError, CompositeSaveExecution, CompositeWriteRequest},
    diagnostics::ApplicationError,
    documents::persistence::{
        self, DocumentUpdateError, DocumentUpdateExecution, SaveDocumentInput,
    },
    templates::TemplateWriteContext,
    write::{BodyOutcome, WriteExecution},
};
use crate::data::{
    artifact::{DocumentSaveErrorCategory, DocumentSaveOutcome},
    collaboration_lock::LockSessionId,
    edit_session::{
        DurableRecoverySink, EditSessionError, EditSessionService, EditSessionState,
        RecoveryEnvelope, RecoveryHandoffCallError, RecoveryHandoffStatus, RecoverySinkFailure,
        RecoverySinkFailureCategory,
    },
    project_relative_path::ProjectRelativePath,
    project_runtime::{ProjectRuntime, RuntimeState},
    repository::{ArtifactSourceId, ArtifactWriteCategory},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SinkCapability {
    Unconnected,
    Connected,
}

/// bool이나 receipt를 연결 증거로 받지 않는다. backend가 실제 호출할 adapter를 소유한다.
pub(crate) struct RecoveryBackend<P, R> {
    sink: Option<ConnectedSink<P, R>>,
}
struct ConnectedSink<P, R>(Box<dyn DurableRecoverySink<P, Receipt = R>>);
impl<P, R> DurableRecoverySink<P> for ConnectedSink<P, R> {
    type Receipt = R;
    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        payload: P,
    ) -> Result<R, RecoverySinkFailure<P>> {
        self.0.accept_durably(envelope, payload)
    }
}
impl<P, R> Default for RecoveryBackend<P, R> {
    fn default() -> Self {
        Self { sink: None }
    }
}
impl<P, R> RecoveryBackend<P, R> {
    pub(crate) fn connected(sink: impl DurableRecoverySink<P, Receipt = R> + 'static) -> Self {
        Self {
            sink: Some(ConnectedSink(Box::new(sink))),
        }
    }
    pub(crate) fn capability(&self) -> SinkCapability {
        if self.sink.is_some() {
            SinkCapability::Connected
        } else {
            SinkCapability::Unconnected
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffCategory {
    ProjectMismatch,
    SessionMismatch,
    TargetsMismatch,
    InvalidState,
    MissingActiveDraft,
    Unavailable,
    NoRecoveryCondition,
}
#[derive(Debug)]
pub(crate) enum HandoffError {
    ExplicitAccept(RecoverySinkFailureCategory),
    Rejected(HandoffCategory),
    Preserve(EditSessionError),
    Accept(RecoveryHandoffCallError),
    Acknowledge(EditSessionError),
}
impl fmt::Display for HandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExplicitAccept(_) => f.write_str(
                "명시적 보관이 완료되지 않았습니다. 같은 초안을 유지하고 다시 보관하세요",
            ),
            Self::Rejected(HandoffCategory::Unavailable) => f.write_str(
                "복구 보관 기능이 연결되지 않았습니다. 편집 원본을 유지하고 연결을 확인하세요",
            ),
            Self::Rejected(_) => f.write_str(
                "현재 편집과 보관 상태를 확인하세요. 원본을 유지하고 해당 세션에서 다시 요청하세요",
            ),
            Self::Preserve(e) | Self::Acknowledge(e) => e.fmt(f),
            Self::Accept(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for HandoffError {}
impl HandoffError {
    pub(crate) fn sink_category(&self) -> Option<RecoverySinkFailureCategory> {
        match self {
            Self::ExplicitAccept(category) => Some(*category),
            Self::Rejected(HandoffCategory::Unavailable) => {
                Some(RecoverySinkFailureCategory::Unavailable)
            }
            Self::Accept(RecoveryHandoffCallError::Sink(e)) => Some(e.sink_category()),
            _ => None,
        }
    }
}
fn rejected(category: HandoffCategory) -> HandoffError {
    HandoffError::Rejected(category)
}

/// 원본은 한 슬롯 또는 기존 service 중 한 곳에만 있다. binding은 실제 세션에서만 얻는다.
#[must_use]
pub(crate) struct DraftCustody<P> {
    project: String,
    session: LockSessionId,
    targets: Vec<ProjectRelativePath>,
    active: Option<P>,
    receipt_returned: bool,
}

/// 결합 실패도 P를 돌려주되 기본 진단에서 그 내용을 출력하지 않는다.
pub(crate) struct DraftBindingFailure<P> {
    payload: P,
    error: HandoffError,
}
impl<P> DraftBindingFailure<P> {
    pub(crate) fn into_parts(self) -> (P, HandoffError) {
        (self.payload, self.error)
    }
}
impl<P> fmt::Debug for DraftBindingFailure<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftBindingFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
impl<P> fmt::Display for DraftBindingFailure<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
impl<P> std::error::Error for DraftBindingFailure<P> {}
impl<P> fmt::Debug for DraftCustody<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftCustody")
            .field("active", &self.active.is_some())
            .field("receipt_returned", &self.receipt_returned)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CustodyTransition {
    Active,
    Preserved,
}
/// 원 업무 결과를 축약하지 않는다. 보관 실패도 별도 owner로 남긴다.
#[derive(Debug)]
#[must_use]
pub(crate) struct DirtySaveResult<E> {
    pub(crate) original: E,
    pub(crate) custody: Result<CustodyTransition, HandoffError>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandoffSnapshot {
    pub(crate) active_dirty: bool,
    pub(crate) handoff: Option<RecoveryHandoffStatus>,
    pub(crate) receipt_returned: bool,
    pub(crate) session: EditSessionState,
    pub(crate) runtime: RuntimeState,
    pub(crate) normal_exit_allowed: bool,
}

#[must_use]
pub(crate) struct Acknowledged<R> {
    pub(crate) receipt: R,
    pub(crate) before: HandoffSnapshot,
    pub(crate) after: HandoffSnapshot,
}
impl<R> fmt::Debug for Acknowledged<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Acknowledged")
            .field("before", &self.before)
            .field("after", &self.after)
            .finish_non_exhaustive()
    }
}

impl<P> DraftCustody<P> {
    pub(in crate::data::application) fn replace_active<R>(
        &mut self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
        payload: P,
        admits: impl FnOnce(&P, &P) -> bool,
    ) -> Result<P, DraftBindingFailure<P>> {
        if let Err(error) = self.identity(runtime, session) {
            return Err(DraftBindingFailure { payload, error });
        }
        let Some(old) = self.active.as_mut() else {
            return Err(DraftBindingFailure {
                payload,
                error: rejected(HandoffCategory::InvalidState),
            });
        };
        if !admits(old, &payload) {
            return Err(DraftBindingFailure {
                payload,
                error: rejected(HandoffCategory::InvalidState),
            });
        }
        Ok(std::mem::replace(old, payload))
    }
    pub(in crate::data::application) fn active_matches(
        &self,
        admits: impl FnOnce(&P) -> bool,
    ) -> bool {
        self.active.as_ref().is_some_and(admits)
    }
    pub(in crate::data::application) fn deposit_active<R>(
        &mut self,
        runtime: &ProjectRuntime,
        session: &mut EditSessionService<P, R>,
        backend: &mut RecoveryBackend<P, R>,
    ) -> Result<R, HandoffError> {
        self.identity(runtime, session)?;
        let sink = backend
            .sink
            .as_mut()
            .ok_or_else(|| rejected(HandoffCategory::Unavailable))?;
        let payload = self
            .active
            .take()
            .ok_or_else(|| rejected(HandoffCategory::MissingActiveDraft))?;
        match session.deposit_active(payload, sink) {
            Ok(receipt) => {
                self.receipt_returned = true;
                Ok(receipt)
            }
            Err(e) => {
                let (payload, category) = e.into_parts();
                self.active = Some(payload);
                Err(HandoffError::ExplicitAccept(category))
            }
        }
    }
    /// worker는 내용이 아닌 owner 유무만 조회한다. 보관 실패 때 P를 새로 만들지 않는다.
    pub(crate) fn has_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn is_acknowledged(&self) -> bool {
        self.receipt_returned && self.active.is_none()
    }

    /// 명시적 제어 요청만 active 원본을 caller에게 돌려준다. 보관 중인 P/R은 소비하지 않는다.
    pub(crate) fn into_active(mut self) -> Result<P, Self> {
        match self.active.take() {
            Some(payload) => Ok(payload),
            None => Err(self),
        }
    }

    pub(crate) fn bind<R>(
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
        payload: P,
    ) -> Result<Self, DraftBindingFailure<P>> {
        let snapshot = session.snapshot();
        if snapshot.project_fingerprint() != Some(runtime.project_fingerprint()) {
            return Err(DraftBindingFailure {
                payload,
                error: rejected(HandoffCategory::ProjectMismatch),
            });
        }
        let Some(id) = snapshot.session_id() else {
            return Err(DraftBindingFailure {
                payload,
                error: rejected(HandoffCategory::SessionMismatch),
            });
        };
        Ok(Self {
            project: runtime.project_fingerprint().to_owned(),
            session: id.clone(),
            targets: snapshot.targets().to_vec(),
            active: Some(payload),
            receipt_returned: false,
        })
    }

    fn identity<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
    ) -> Result<(), HandoffError> {
        let current = session.snapshot();
        if self.project != runtime.project_fingerprint()
            || current.project_fingerprint() != Some(self.project.as_str())
        {
            return Err(rejected(HandoffCategory::ProjectMismatch));
        }
        if current.session_id() != Some(&self.session) {
            return Err(rejected(HandoffCategory::SessionMismatch));
        }
        if current.targets() != self.targets {
            return Err(rejected(HandoffCategory::TargetsMismatch));
        }
        Ok(())
    }

    fn before_save<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
        backend: &RecoveryBackend<P, R>,
        context: &TemplateWriteContext<'_>,
        targets: &[ProjectRelativePath],
    ) -> Result<(), HandoffError> {
        self.identity(runtime, session)?;
        if context.project != self.project {
            return Err(rejected(HandoffCategory::ProjectMismatch));
        }
        if context.session != &self.session {
            return Err(rejected(HandoffCategory::SessionMismatch));
        }
        if targets != self.targets {
            return Err(rejected(HandoffCategory::TargetsMismatch));
        }
        if session.snapshot().state() != EditSessionState::Editing || self.receipt_returned {
            return Err(rejected(HandoffCategory::InvalidState));
        }
        if self.active.is_none() {
            return Err(rejected(HandoffCategory::MissingActiveDraft));
        }
        if backend.capability() == SinkCapability::Unconnected {
            return Err(rejected(HandoffCategory::Unavailable));
        }
        Ok(())
    }

    // take 뒤에는 ?를 쓰지 않고, 거부된 P와 원 service error를 모두 되돌려 받는다.
    fn preserve<R>(
        &mut self,
        session: &mut EditSessionService<P, R>,
    ) -> Result<CustodyTransition, HandoffError> {
        let Some(payload) = self.active.take() else {
            return Err(rejected(HandoffCategory::MissingActiveDraft));
        };
        match session.preserve_for_recovery(payload) {
            Ok(()) => Ok(CustodyTransition::Preserved),
            Err(failure) => {
                let (payload, error) = failure.into_parts();
                self.active = Some(payload);
                Err(HandoffError::Preserve(error))
            }
        }
    }

    fn existing_recovery_condition<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
    ) -> Result<(), HandoffError> {
        self.identity(runtime, session)?;
        if runtime.snapshot().state == RuntimeState::Ready
            && session.snapshot().state() == EditSessionState::Editing
        {
            return Err(rejected(HandoffCategory::NoRecoveryCondition));
        }
        Ok(())
    }

    /// 종료 전에 기존 보관 자격만 읽는다. 실제 preserve는 같은 조건을 다시 검사한다.
    pub(in crate::data::application) fn can_preserve_existing<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
    ) -> bool {
        self.active.is_some()
            && self.existing_recovery_condition(runtime, session).is_ok()
            // 하위 preserve_for_recovery가 받는 Held phase만 허용한다. sink 연결은 무관하다.
            && matches!(
                session.snapshot().state(),
                EditSessionState::Editing | EditSessionState::LockLost
            )
    }

    /// sink 미연결이어도 이미 막힌 편집 원본을 보관한다. 외부 오류 라벨은 받지 않는다.
    pub(crate) fn preserve_existing<R>(
        &mut self,
        runtime: &ProjectRuntime,
        session: &mut EditSessionService<P, R>,
    ) -> Result<CustodyTransition, HandoffError> {
        self.existing_recovery_condition(runtime, session)?;
        self.preserve(session)
    }

    pub(crate) fn accept<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &mut EditSessionService<P, R>,
        backend: &mut RecoveryBackend<P, R>,
    ) -> Result<(), HandoffError> {
        self.identity(runtime, session)?;
        if self.active.is_some()
            || self.receipt_returned
            || session.snapshot().recovery_handoff() != Some(RecoveryHandoffStatus::Pending)
        {
            return Err(rejected(HandoffCategory::InvalidState));
        }
        let sink = backend
            .sink
            .as_mut()
            .ok_or_else(|| rejected(HandoffCategory::Unavailable))?;
        session
            .accept_recovery_durably(sink)
            .map_err(HandoffError::Accept)
    }

    pub(crate) fn acknowledge<R>(
        &mut self,
        runtime: &ProjectRuntime,
        session: &mut EditSessionService<P, R>,
    ) -> Result<Acknowledged<R>, HandoffError> {
        self.identity(runtime, session)?;
        if self.active.is_some()
            || self.receipt_returned
            || session.snapshot().recovery_handoff() != Some(RecoveryHandoffStatus::DurablyAccepted)
        {
            return Err(rejected(HandoffCategory::InvalidState));
        }
        let before = self.observe(runtime, session);
        let receipt = session
            .acknowledge_recovery()
            .map_err(HandoffError::Acknowledge)?;
        self.receipt_returned = true;
        // 해제 뒤 ack는 service identity를 지우므로, 이 동일 borrow 안에서 전후를 관측한다.
        let after = self.observe(runtime, session);
        Ok(Acknowledged {
            receipt,
            before,
            after,
        })
    }

    /// 상태 조회는 실행 권한이 아니다. lock 해제와 payload 인수 완료를 분리한다.
    pub(crate) fn snapshot<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
    ) -> Result<HandoffSnapshot, HandoffError> {
        self.identity(runtime, session)?;
        Ok(self.observe(runtime, session))
    }
    fn observe<R>(
        &self,
        runtime: &ProjectRuntime,
        session: &EditSessionService<P, R>,
    ) -> HandoffSnapshot {
        let current = session.snapshot();
        let handoff = current.recovery_handoff().or(self
            .receipt_returned
            .then_some(RecoveryHandoffStatus::Acknowledged));
        HandoffSnapshot {
            active_dirty: self.active.is_some(),
            handoff,
            receipt_returned: self.receipt_returned,
            session: current.state(),
            runtime: runtime.snapshot().state,
            normal_exit_allowed: self.active.is_none()
                && self.receipt_returned
                && current.state() == EditSessionState::ReadOnly
                && runtime.snapshot().state == RuntimeState::Ready
                && current.release_failure_count() == 0,
        }
    }
}

fn document_stale(error: &DocumentUpdateError) -> bool {
    match error {
        DocumentUpdateError::DocumentSourceMismatch
        | DocumentUpdateError::DocumentTrashed
        | DocumentUpdateError::TemplateSourceMismatch
        | DocumentUpdateError::TemplateRevisionMismatch => true,
        DocumentUpdateError::Save(e) => e.category() == DocumentSaveErrorCategory::RevisionMismatch,
        DocumentUpdateError::TemplateBindingMismatch | DocumentUpdateError::Materialization(_) => {
            false
        }
    }
}
fn stale<E>(execution: &WriteExecution<(), E>, domain: impl FnOnce(&E) -> bool) -> bool {
    if execution.diagnostic().artifact_write.is_some_and(|d| {
        matches!(
            d.category,
            ArtifactWriteCategory::SourceMismatch | ArtifactWriteCategory::SourceMissing
        )
    }) {
        return true;
    }
    match execution.body() {
        Some(BodyOutcome::Rejected(e)) => e.domain_cause().is_some_and(domain),
        _ => false,
    }
}
// bool은 이 모듈의 같은 동기 저장 호출에서만 계산한다. 외부 result/snapshot은 authority가 아니다.
fn finish<P, R>(
    draft: &mut DraftCustody<P>,
    runtime: &ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    source_stale: bool,
) -> Result<CustodyTransition, HandoffError> {
    if source_stale
        || runtime.snapshot().state != RuntimeState::Ready
        || session.snapshot().state() != EditSessionState::Editing
    {
        draft.preserve(session)
    } else {
        Ok(CustodyTransition::Active)
    }
}

pub(crate) fn save_dirty_document<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    backend: &RecoveryBackend<P, R>,
    draft: &mut DraftCustody<P>,
    context: TemplateWriteContext<'_>,
    input: &SaveDocumentInput,
) -> Result<
    DirtySaveResult<Result<DocumentUpdateExecution<DocumentSaveOutcome>, ApplicationError>>,
    HandoffError,
> {
    let target = ArtifactSourceId::Document(input.document.id)
        .path()
        .map_err(|_| rejected(HandoffCategory::TargetsMismatch))?;
    draft.before_save(runtime, session, backend, &context, &[target])?;
    let original = persistence::save_document(runtime, session, context, input);
    let source_stale = original
        .as_ref()
        .is_ok_and(|r| stale(&r.execution, document_stale));
    let custody = finish(draft, runtime, session, source_stale);
    Ok(DirtySaveResult { original, custody })
}

pub(crate) fn save_dirty_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    backend: &RecoveryBackend<P, R>,
    draft: &mut DraftCustody<P>,
    context: TemplateWriteContext<'_>,
    input: &super::templates::whole::WholeTemplateInput,
) -> Result<
    DirtySaveResult<Result<super::templates::TemplateMutationExecution, ApplicationError>>,
    HandoffError,
> {
    let target = ArtifactSourceId::Template(input.source.id)
        .path()
        .map_err(|_| rejected(HandoffCategory::TargetsMismatch))?;
    draft.before_save(runtime, session, backend, &context, &[target])?;
    let original = super::templates::whole::update(runtime, session, context, input);
    let source_stale = original.as_ref().is_ok_and(|r| {
        stale(&r.execution, |e| {
            matches!(
                e,
                super::templates::TemplateUseCaseError::SourceMismatch
                    | super::templates::TemplateUseCaseError::RevisionMismatch
            )
        })
    });
    let custody = finish(draft, runtime, session, source_stale);
    Ok(DirtySaveResult { original, custody })
}

pub(crate) fn create_dirty_template<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    backend: &RecoveryBackend<P, R>,
    draft: &mut DraftCustody<P>,
    context: TemplateWriteContext<'_>,
    prepared: &mut super::templates::PreparedTemplateCreate,
) -> Result<DirtySaveResult<WriteExecution<(), super::templates::TemplateUseCaseError>>, HandoffError>
{
    draft.before_save(
        runtime,
        session,
        backend,
        &context,
        prepared.session_targets(),
    )?;
    let original = super::templates::create_template(runtime, session, context, prepared);
    let custody = finish(draft, runtime, session, false);
    Ok(DirtySaveResult { original, custody })
}

pub(crate) fn save_dirty_composite<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    backend: &RecoveryBackend<P, R>,
    draft: &mut DraftCustody<P>,
    context: TemplateWriteContext<'_>,
    request: &CompositeWriteRequest<'_>,
) -> Result<DirtySaveResult<CompositeSaveExecution>, HandoffError> {
    draft.before_save(
        runtime,
        session,
        backend,
        &context,
        request.session_targets(),
    )?;
    let original = composite::update_template_and_save_document(runtime, session, context, request);
    let source_stale = stale(&original.execution, |e| match e {
        CompositeSaveError::Source(e) => document_stale(e),
        CompositeSaveError::Document(e) | CompositeSaveError::OriginalDocument(e) => {
            e.category() == DocumentSaveErrorCategory::RevisionMismatch
        }
        CompositeSaveError::Template(_)
        | CompositeSaveError::TemplateUnchangedDocumentChanged
        | CompositeSaveError::TemplateChangedDocumentUnchanged => false,
    });
    let custody = finish(draft, runtime, session, source_stale);
    Ok(DirtySaveResult { original, custody })
}

#[cfg(all(test, windows))]
mod tests;
