//! 짧은 배치 session에서만 문서 생성 pair와 구조 변경을 실행한다.
use super::{
    diagnostics::{ApplicationError, BuildError},
    templates::{TemplateSource, TemplateWriteContext},
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    artifact::{
        layout::{DocumentLayout, LayoutEdit, LayoutError},
        DocumentArtifact, DocumentId, TemplateLifecycle,
    },
    edit_session::EditSessionService,
    project_relative_path::ProjectRelativePath,
    project_runtime::ProjectRuntime,
    repository::{
        ArtifactRepository, ArtifactSourceId, CanonicalWritePlan, CompleteDocumentScan, SourceToken,
    },
};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone)]
pub(crate) struct LayoutSnapshot {
    pub(crate) layout: DocumentLayout,
    pub(crate) source: Option<SourceToken>,
}
pub(crate) struct LayoutWrite {
    pub(crate) base: LayoutSnapshot,
    pub(crate) documents: Arc<CompleteDocumentScan>,
    pub(crate) edit: LayoutEdit,
    pub(crate) timestamp: String,
    pub(crate) create: Option<(DocumentArtifact, TemplateSource, Option<DocumentId>)>,
}
#[derive(Clone)]
pub(crate) struct LayoutDocumentSummary {
    pub(crate) id: DocumentId,
    pub(crate) template: crate::data::artifact::TemplateId,
    pub(crate) name: String,
    pub(crate) english_name: String,
    pub(crate) glossary_summary: String,
    pub(crate) glossary_excluded: bool,
}
/// 저장 직전의 완전한 문서 scan과 실제로 기록할 layout을 함께 소유한다.
/// 생성 완료 화면은 이 값을 재사용해 같은 대형 목록을 다시 decode하지 않는다.
#[derive(Clone)]
pub(crate) struct LayoutCommit {
    pub(crate) layout: DocumentLayout,
    pub(crate) documents: Vec<LayoutDocumentSummary>,
    pub(crate) unplaced: Vec<DocumentId>,
}
#[derive(Debug)]
pub(crate) enum LayoutWriteError {
    Stale,
    Relations(LayoutError),
    TemplateStale,
    TemplateUnavailable,
    Occupied,
}
impl LayoutWrite {
    pub(crate) fn targets(&self) -> Result<Vec<ProjectRelativePath>, ApplicationError> {
        Ok(self.exact()?.session_targets().to_vec())
    }
    fn exact(&self) -> Result<ExactWriteTargets, ApplicationError> {
        ExactWriteTargets::new(
            std::iter::once(ArtifactSourceId::DocumentLayout).chain(
                self.create
                    .iter()
                    .map(|(d, _, _)| ArtifactSourceId::Document(d.document_id())),
            ),
        )
    }
}
pub(crate) fn execute<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &LayoutWrite,
) -> Result<WriteExecution<LayoutCommit, LayoutWriteError>, ApplicationError> {
    let targets = input.exact()?;
    Ok(execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut (),
        &mut |repository, _| build(repository, input),
    ))
}
fn build(
    repository: &ArtifactRepository<'_, '_>,
    input: &LayoutWrite,
) -> Result<WriteDecision<LayoutCommit>, BuildError<LayoutWriteError>> {
    let current = repository.load_layout()?;
    if current.as_ref().map(|v| v.source()) != input.base.source.as_ref() {
        return Err(BuildError::domain(LayoutWriteError::Stale));
    }
    // 목록의 strict scan은 후보 계산에만 재사용한다. 아래 plan이 커밋 직전 모든 source의
    // 정확한 bytes와 전체 membership을 다시 대조하므로 UI snapshot 자체는 쓰기 권한이 아니다.
    let scan = &input.documents;
    let ids: BTreeSet<_> = scan.summaries().map(|(id, _, _, _, _, _)| id).collect();
    let mut documents: Vec<_> = scan
        .summaries()
        .map(
            |(id, template, name, english_name, glossary_summary, glossary_excluded)| {
                LayoutDocumentSummary {
                    id,
                    template,
                    name: name.into(),
                    english_name: english_name.into(),
                    glossary_summary: glossary_summary.into(),
                    glossary_excluded,
                }
            },
        )
        .collect();
    let relation = |e| BuildError::domain(LayoutWriteError::Relations(e));
    let mut next = if input.create.is_some() {
        // 생성은 새 문서 하나만 배치한다. 이미 보이는 미배치 문서는 메뉴에서 따로 반영한다.
        input.base.layout.reconcile(&ids).map_err(relation)?;
        input.base.layout.clone()
    } else {
        input
            .base
            .layout
            .changed(&ids, &input.edit, &input.timestamp)
            .map_err(relation)?
    };
    if let LayoutEdit::Purge { document } = &input.edit {
        documents.retain(|summary| summary.id != *document);
    }
    let mut plan = CanonicalWritePlan::new();
    if let LayoutEdit::Restore { document, .. } = &input.edit {
        // The list snapshot is not authority for a trash-to-active transition.
        // Re-read the bound document and its current Template inside the write
        // operation, then pin both sources through the canonical prepare step.
        let bound_document = repository.load_document(*document)?;
        if input.documents.source(*document) != Some(bound_document.source()) {
            return Err(BuildError::domain(LayoutWriteError::Stale));
        }
        let template = repository.load_template(bound_document.artifact().template_id())?;
        if template.artifact().lifecycle() != TemplateLifecycle::Active {
            return Err(BuildError::domain(LayoutWriteError::TemplateUnavailable));
        }
        plan = plan.read_dependency(template.source());
    }
    if let Some((document, template, parent)) = &input.create {
        if ids.contains(&document.document_id()) {
            return Err(BuildError::domain(LayoutWriteError::Occupied));
        }
        let loaded = repository.load_template(template.id)?;
        if loaded.source() != &template.token
            || loaded.artifact().revision() != template.expected_revision
        {
            return Err(BuildError::domain(LayoutWriteError::TemplateStale));
        }
        next.insert(document.document_id(), *parent)
            .map_err(relation)?;
        documents.push(LayoutDocumentSummary {
            id: document.document_id(),
            template: document.template_id(),
            name: document.name().into(),
            english_name: document.english_name().into(),
            glossary_summary: document.glossary_summary().into(),
            glossary_excluded: document.glossary_excluded(),
        });
        if next.revision == input.base.layout.revision {
            next.bump().map_err(relation)?;
        }
        plan = plan
            .create_document(document)?
            .read_dependency(&template.token);
    } else if next == input.base.layout && current.is_some() {
        return Ok(WriteDecision::NoWrite(LayoutCommit {
            unplaced: next.reconcile(&ids).map_err(relation)?,
            layout: next,
            documents,
        }));
    }
    if current.is_none() {
        next.revision = 1;
    }
    // scan source도 prepare 직전 재대조한다. 본문을 가짜 write target으로 포함하지 않는다.
    plan = plan.document_scan(scan);
    plan = plan.layout(&next, current.as_ref().map(|v| v.source()))?;
    let post_ids = documents.iter().map(|summary| summary.id).collect();
    let unplaced = next.reconcile(&post_ids).map_err(relation)?;
    documents.sort_by_key(|summary| summary.id);
    Ok(WriteDecision::Write {
        plan,
        value: LayoutCommit {
            layout: next,
            documents,
            unplaced,
        },
    })
}
