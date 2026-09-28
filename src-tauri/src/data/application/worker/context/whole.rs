//! 전체 Template도 기존 G10의 실제 P를 보관한다. 앱 owner의 세대 대조는 호출자가 수행한다.
use super::*;
use crate::data::application::{diagnostics::DiskState, templates, write::WriteExecution};
type Updated = Result<
    DirtySaveResult<Result<templates::TemplateMutationExecution, ApplicationError>>,
    HandoffError,
>;
type Created =
    Result<DirtySaveResult<WriteExecution<(), templates::TemplateUseCaseError>>, HandoffError>;

impl<P, R> SessionWork<'_, P, R> {
    pub(crate) fn bind_whole_draft(
        &mut self,
        payload: P,
        admits: impl FnOnce(&P, &P) -> bool,
    ) -> Result<Option<P>, DraftRejected<P>> {
        if self
            .entry
            .draft
            .as_ref()
            .is_some_and(DraftCustody::is_acknowledged)
        {
            self.entry.draft = None;
        }
        let Some(draft) = self.entry.draft.as_mut() else {
            return self.bind_draft(payload).map(|()| None);
        };
        draft
            .replace_active(self.runtime, &self.entry.service, payload, admits)
            .map(Some)
            .map_err(|e| {
                let (input, original) = e.into_parts();
                DraftRejected {
                    input,
                    category: WorkerCategory::OwnersRemain,
                    original: Some(original),
                }
            })
    }
    fn resolve_committed(&mut self, disk: Option<DiskState>) -> Option<P> {
        if !matches!(disk, Some(DiskState::Committed | DiskState::NoWrite))
            || !self
                .entry
                .draft
                .as_ref()
                .is_some_and(DraftCustody::has_active)
        {
            return None;
        }
        // 같은 동기 실행의 확정 결과만 이 경로를 연다. 프런트의 saved bool은 받지 않는다.
        match self.entry.draft.take()?.into_active() {
            Ok(p) => Some(p),
            Err(d) => {
                self.entry.draft = Some(d);
                None
            }
        }
    }
    pub(crate) fn write_whole_template(
        &mut self,
        input: &templates::whole::WholeTemplateInput,
    ) -> (Updated, Option<P>) {
        let result = self.save_whole_template(input);
        let disk = result
            .as_ref()
            .ok()
            .and_then(|d| d.original.as_ref().ok())
            .map(|r| r.execution.diagnostic().disk);
        let resolved = self.resolve_committed(disk);
        (result, resolved)
    }
    /// 단일 문서의 확정 저장도 같은 P 회수 규칙을 사용한다.
    pub(crate) fn write_whole_document(
        &mut self,
        input: &SaveDocumentInput,
    ) -> (
        Result<
            DirtySaveResult<Result<DocumentUpdateExecution<DocumentSaveOutcome>, ApplicationError>>,
            HandoffError,
        >,
        Option<P>,
    ) {
        let result = self.save_dirty_document(input);
        let disk = result
            .as_ref()
            .ok()
            .and_then(|d| d.original.as_ref().ok())
            .map(|r| r.execution.diagnostic().disk);
        let resolved = self.resolve_committed(disk);
        (result, resolved)
    }
    pub(crate) fn create_whole_template(
        &mut self,
        prepared: &mut templates::PreparedTemplateCreate,
    ) -> (Created, Option<P>) {
        let result = match self.entry.draft.as_mut() {
            Some(draft) => recovery_handoff::create_dirty_template(
                self.runtime,
                &mut self.entry.service,
                &self.entry.backend,
                draft,
                context(&self.key),
                prepared,
            ),
            None => Err(HandoffError::Rejected(
                recovery_handoff::HandoffCategory::MissingActiveDraft,
            )),
        };
        let disk = result.as_ref().ok().map(|r| r.original.diagnostic().disk);
        let resolved = self.resolve_committed(disk);
        (result, resolved)
    }
}
impl<P, R> CleanupContext<'_, P, R> {
    /// 정상 active P는 상태를 바꾸지 않고 실제 sink proof를 받는다. Pending P는 기존 accept/ack를 쓴다.
    pub(crate) fn deposit_whole_draft(
        &mut self,
        registration: &Registration,
    ) -> Result<Result<Option<R>, HandoffError>, WorkerCategory> {
        let rt = self.0.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .0
            .sessions
            .iter_mut()
            .find(|e| &e.registration == registration)
            .ok_or(WorkerCategory::Stale)?;
        let Some(draft) = entry.draft.as_mut() else {
            return Ok(Ok(None));
        };
        if draft.is_acknowledged() {
            return Ok(Ok(None));
        }
        if draft.has_active() {
            return Ok(draft
                .deposit_active(rt, &mut entry.service, &mut entry.backend)
                .map(Some));
        }
        if entry.service.snapshot().recovery_handoff()
            == Some(crate::data::edit_session::RecoveryHandoffStatus::Pending)
        {
            if let Err(e) = draft.accept(rt, &mut entry.service, &mut entry.backend) {
                return Ok(Err(e));
            }
        }
        Ok(draft
            .acknowledge(rt, &mut entry.service)
            .map(|a| Some(a.receipt)))
    }
    pub(crate) fn discard_whole_active(
        &mut self,
        registration: &Registration,
        admits: impl FnOnce(&P) -> bool,
    ) -> Result<Option<P>, WorkerCategory> {
        let entry = self.entry(registration)?;
        let Some(draft) = entry.draft.as_ref() else {
            return Ok(None);
        };
        if draft.is_acknowledged() {
            entry.draft = None;
            return Ok(None);
        }
        if !draft.has_active() {
            let payload = entry
                .service
                .discard_recovery(admits)
                .map_err(|_| WorkerCategory::OwnersRemain)?;
            entry.draft = None;
            return Ok(Some(payload));
        }
        if !draft.active_matches(admits) {
            return Err(WorkerCategory::OwnersRemain);
        }
        let draft = entry.draft.take().ok_or(WorkerCategory::Inactive)?;
        match draft.into_active() {
            Ok(p) => Ok(Some(p)),
            Err(d) => {
                entry.draft = Some(d);
                Err(WorkerCategory::OwnersRemain)
            }
        }
    }
}

impl<P, R> WorkerContext<P, R> {
    pub(crate) fn activate_template_owner(
        &mut self,
        registration: &Registration,
        target: ProjectRelativePath,
    ) -> Result<Result<(), crate::data::edit_session::EditSessionError>, WorkerCategory> {
        let rt = self.runtime.as_ref().ok_or(WorkerCategory::Unavailable)?;
        let entry = self
            .sessions
            .iter_mut()
            .find(|e| &e.registration == registration)
            .ok_or(WorkerCategory::Stale)?;
        if entry.service.snapshot().state() != EditSessionState::ReadOnly || entry.draft.is_some() {
            return Err(WorkerCategory::OwnersRemain);
        }
        Ok(entry
            .service
            .begin_edit(rt.project_fingerprint(), vec![target]))
    }
}
