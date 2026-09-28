//! G13이 필요한 닫힌 업무만 위임한다. runtime/service의 mutable accessor는 만들지 않는다.
use super::*;
use crate::data::{
    application::{diagnostics::ApplicationCategory, documents, documents::persistence, templates},
    artifact::DocumentMaterializationOutcome,
};

impl<P, R> WorkerContext<P, R> {
    pub(crate) fn project_fingerprint(&self) -> Option<&str> {
        self.runtime
            .as_ref()
            .map(crate::data::project_runtime::ProjectRuntime::project_fingerprint)
    }
    pub(crate) fn prepare_whole_template(
        &mut self,
        seed: &crate::data::artifact::TemplateArtifact,
        timestamp: &str,
        draft: crate::data::artifact::template_mutation::whole::TemplateDraftInput,
    ) -> Result<templates::PreparedTemplateCreate, templates::TemplatePreparationError> {
        let runtime = self.runtime.as_mut().ok_or_else(|| {
            templates::TemplatePreparationError::Target(ApplicationError::closed(
                ApplicationCategory::RuntimeRejected,
            ))
        })?;
        templates::whole::prepare_create(runtime, seed, timestamp, draft)
    }
    pub(in crate::data::application::worker) fn observations(
        &self,
    ) -> Vec<(Registration, SessionObservation)> {
        self.sessions
            .iter()
            .map(|e| {
                (
                    e.registration.clone(),
                    SessionObservation {
                        snapshot: e.service.snapshot(),
                        active_dirty: e.draft.as_ref().is_some_and(DraftCustody::has_active),
                        capability: e.backend.capability(),
                    },
                )
            })
            .collect()
    }
    pub(crate) fn prepare_template(
        &mut self,
        input: &templates::CreateTemplateInput,
    ) -> Result<templates::PreparedTemplateCreate, templates::TemplatePreparationError> {
        let runtime = self.runtime.as_mut().ok_or_else(|| {
            templates::TemplatePreparationError::Target(ApplicationError::closed(
                ApplicationCategory::RuntimeRejected,
            ))
        })?;
        templates::prepare_create_template(runtime, input)
    }
    pub(crate) fn prepare_duplicate(
        &mut self,
        input: &templates::DuplicateTemplateInput,
    ) -> Result<templates::PreparedTemplateCreate, templates::TemplatePreparationError> {
        let runtime = self.runtime.as_mut().ok_or_else(|| {
            templates::TemplatePreparationError::Target(ApplicationError::closed(
                ApplicationCategory::RuntimeRejected,
            ))
        })?;
        templates::prepare_duplicate_template(runtime, input)
    }
    pub(crate) fn prepare_document(
        &mut self,
        input: &documents::CreateDocumentInput,
    ) -> Result<documents::PreparedDocumentCreate, documents::DocumentPreparationError> {
        let runtime = self.runtime.as_mut().ok_or_else(|| {
            documents::DocumentPreparationError::Target(ApplicationError::closed(
                ApplicationCategory::RuntimeRejected,
            ))
        })?;
        documents::prepare_create_document(runtime, input)
    }
    pub(crate) fn observe_session(
        &self,
        registration: &Registration,
    ) -> Result<(EditSessionSnapshot, bool), WorkerCategory> {
        let entry = self
            .sessions
            .iter()
            .find(|e| e.registration == *registration)
            .ok_or(WorkerCategory::Stale)?;
        Ok((
            entry.service.snapshot(),
            entry.draft.as_ref().is_some_and(DraftCustody::has_active),
        ))
    }
    pub(crate) fn session_control(&mut self) -> CleanupContext<'_, P, R> {
        CleanupContext(self)
    }
}
impl<P, R> SessionWork<'_, P, R> {
    pub(crate) fn change_format(
        &mut self,
        id: crate::data::repository::ArtifactSourceId,
        source: &str,
        restore: Option<&str>,
    ) -> Result<crate::data::application::write::WriteExecution<(), ()>, ApplicationError> {
        crate::data::application::format::execute(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            id,
            source,
            restore,
        )
    }

    pub(crate) fn write_layout(
        &mut self,
        input: &crate::data::application::layout::LayoutWrite,
    ) -> Result<
        crate::data::application::write::WriteExecution<
            crate::data::application::layout::LayoutCommit,
            crate::data::application::layout::LayoutWriteError,
        >,
        ApplicationError,
    > {
        crate::data::application::layout::execute(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }

    pub(crate) fn save_whole_template(
        &mut self,
        input: &templates::whole::WholeTemplateInput,
    ) -> Result<
        crate::data::application::recovery_handoff::DirtySaveResult<
            Result<templates::TemplateMutationExecution, ApplicationError>,
        >,
        crate::data::application::recovery_handoff::HandoffError,
    > {
        let draft = self.entry.draft.as_mut().ok_or(
            crate::data::application::recovery_handoff::HandoffError::Rejected(
                crate::data::application::recovery_handoff::HandoffCategory::MissingActiveDraft,
            ),
        )?;
        crate::data::application::recovery_handoff::save_dirty_template(
            self.runtime,
            &mut self.entry.service,
            &self.entry.backend,
            draft,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn create_template(
        &mut self,
        prepared: &mut templates::PreparedTemplateCreate,
    ) -> crate::data::application::write::WriteExecution<(), templates::TemplateUseCaseError> {
        templates::create_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            prepared,
        )
    }
    pub(crate) fn duplicate_template(
        &mut self,
        prepared: &mut templates::PreparedTemplateCreate,
    ) -> crate::data::application::write::WriteExecution<(), templates::TemplateUseCaseError> {
        templates::duplicate_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            prepared,
        )
    }
    pub(crate) fn create_document(
        &mut self,
        prepared: &mut documents::PreparedDocumentCreate,
    ) -> crate::data::application::write::WriteExecution<(), documents::DocumentUseCaseError> {
        documents::create_document_from_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            prepared,
        )
    }
    pub(crate) fn update_template(
        &mut self,
        input: &templates::UpdateTemplateInput,
    ) -> Result<templates::TemplateMutationExecution, ApplicationError> {
        templates::update_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn tombstone_template(
        &mut self,
        input: &templates::TombstoneTemplateInput,
    ) -> Result<templates::TemplateMutationExecution, ApplicationError> {
        templates::tombstone_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn restore_template(
        &mut self,
        input: &templates::RestoreTemplateInput,
    ) -> Result<templates::TemplateMutationExecution, ApplicationError> {
        templates::restore_template(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn materialize_document(
        &mut self,
        input: &persistence::MaterializeDocumentInput,
    ) -> Result<
        persistence::DocumentUpdateExecution<DocumentMaterializationOutcome>,
        ApplicationError,
    > {
        if self
            .entry
            .draft
            .as_ref()
            .is_some_and(DraftCustody::has_active)
        {
            return Err(ApplicationError::closed(
                ApplicationCategory::InvalidSessionState,
            ));
        }
        persistence::materialize_document(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
    pub(crate) fn repair_missing_optional_group_values(
        &mut self,
        input: &persistence::MaterializeDocumentInput,
    ) -> Result<
        persistence::DocumentUpdateExecution<DocumentMaterializationOutcome>,
        ApplicationError,
    > {
        if self
            .entry
            .draft
            .as_ref()
            .is_some_and(DraftCustody::has_active)
        {
            return Err(ApplicationError::closed(
                ApplicationCategory::InvalidSessionState,
            ));
        }
        persistence::repair_missing_optional_group_values(
            self.runtime,
            &mut self.entry.service,
            context(&self.key),
            input,
        )
    }
}
impl<P, R> CleanupContext<'_, P, R> {
    pub(crate) fn observe_session(
        &self,
        registration: &Registration,
    ) -> Result<(EditSessionSnapshot, bool), WorkerCategory> {
        self.0.observe_session(registration)
    }
}
