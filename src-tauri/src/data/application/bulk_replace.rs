//! 미리보기에서 확정한 여러 Document 후보를 한 exact session·plan·transaction으로 저장한다.
use std::{fmt, sync::Arc};

use super::{
    diagnostics::{ApplicationError, BuildError},
    templates::TemplateWriteContext,
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    artifact::{self, DocumentArtifact},
    edit_session::EditSessionService,
    project_relative_path::ProjectRelativePath,
    project_runtime::ProjectRuntime,
    repository::{ArtifactSourceId, CanonicalWritePlan, CompleteDocumentScan, SourceToken},
};

pub(crate) struct DocumentReplacement {
    pub(crate) source: SourceToken,
    pub(crate) candidate: DocumentArtifact,
}

pub(crate) struct BulkReplaceInput {
    pub(crate) replacements: Vec<DocumentReplacement>,
    pub(crate) documents: Arc<CompleteDocumentScan>,
    pub(crate) layout_source: Option<SourceToken>,
    pub(crate) template_sources: Vec<SourceToken>,
}

pub(crate) struct BulkReplaceRequest<'input> {
    input: &'input BulkReplaceInput,
    targets: ExactWriteTargets,
}

impl<'input> BulkReplaceRequest<'input> {
    pub(crate) fn new(input: &'input BulkReplaceInput) -> Result<Self, ApplicationError> {
        let targets =
            ExactWriteTargets::new(input.replacements.iter().map(|replacement| {
                ArtifactSourceId::Document(replacement.candidate.document_id())
            }))?;
        Ok(Self { input, targets })
    }

    pub(crate) fn session_targets(&self) -> &[ProjectRelativePath] {
        self.targets.session_targets()
    }
}

#[derive(Debug)]
pub(crate) enum BulkReplaceError {
    LayoutChanged,
    DocumentNoLongerActive,
}

pub(crate) fn apply<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    request: &BulkReplaceRequest<'_>,
) -> WriteExecution<(), BulkReplaceError> {
    let input = request.input;
    execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &request.targets,
        },
        &mut (),
        &mut |repository, _| {
            let layout = repository.load_layout()?;
            if layout.as_ref().map(|loaded| loaded.source()) != input.layout_source.as_ref() {
                return Err(BuildError::domain(BulkReplaceError::LayoutChanged));
            }
            if input.replacements.iter().any(|replacement| {
                layout.as_ref().is_some_and(|layout| {
                    layout
                        .artifact()
                        .nodes
                        .get(&replacement.candidate.document_id())
                        .is_some_and(|node| node.state != artifact::layout::LayoutState::Active)
                })
            }) {
                return Err(BuildError::domain(BulkReplaceError::DocumentNoLongerActive));
            }

            let mut plan = CanonicalWritePlan::new();
            for replacement in &input.replacements {
                plan = plan.replace_document(&replacement.candidate, &replacement.source)?;
            }
            for source in &input.template_sources {
                plan = plan.read_dependency(source);
            }
            plan = plan.document_scan(&input.documents);
            // None인 layout도 prepare까지 의존성으로 묶는다. 첫 대상은 위에서 이미
            // 전체 대상의 active 상태를 검사했으며 이 호출은 layout token guard다.
            plan =
                plan.document_state(repository, input.replacements[0].candidate.document_id())?;
            Ok(WriteDecision::Write { plan, value: () })
        },
    )
}

impl fmt::Debug for BulkReplaceInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BulkReplaceInput")
            .field("replacement_count", &self.replacements.len())
            .field("template_source_count", &self.template_sources.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for BulkReplaceRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BulkReplaceRequest([private candidates])")
    }
}
