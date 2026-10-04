//! 생성 초안은 화면과 독립적으로 소유한다. 보관 증거와 공개 결과를 각각 유지한다.
use super::*;
use crate::commands::document_workspace::{
    CreationBody, CreationCommit, ReadField, Request, Response, Summary,
};
use crate::data::field_engine::scalar::validate_optional_single_line_text;
use crate::data::{
    application::layout::{LayoutCommit, LayoutSnapshot, LayoutWrite},
    artifact::layout::{DocumentLayout, LayoutEdit, LayoutState},
    edit_recovery::{
        model::{Attempt, Deposit, Draft, Envelope, Intent, Key, Original, OriginalKind},
        Proof,
    },
    repository::{CompleteDocumentScan, DocumentScanChange},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PDF_ASSET_INPUT_BYTES: usize = 512 * 1024 * 1024;

fn pdf_references(
    value: &crate::data::edit_input::ValueDto,
    label: &str,
    assets: &mut BTreeMap<String, (bool, String)>,
    links: &mut BTreeSet<String>,
) {
    use crate::data::edit_input::ValueDto;
    match value {
        ValueDto::Image { value } => {
            for id in value {
                assets.insert(id.clone(), (true, label.to_owned()));
            }
        }
        ValueDto::File { value } => {
            for id in value {
                assets
                    .entry(id.clone())
                    .or_insert_with(|| (false, label.to_owned()));
            }
        }
        ValueDto::Relation { links: values } => {
            for link in values {
                links.insert(link.document.clone());
            }
        }
        ValueDto::DocumentLink { documents } => {
            links.extend(documents.iter().cloned());
        }
        ValueDto::Group { instances } => {
            for instance in instances {
                for field in &instance.fields {
                    if let Intent::Set(value) = &field.value {
                        pdf_references(value, label, assets, links);
                    }
                }
            }
        }
        _ => (),
    }
}

fn pdf_snapshot(
    ready: &crate::data::project_runtime::RecoveryReadyProject<'_>,
    id: artifact::DocumentId,
) -> Reply<crate::pdf_export::Snapshot> {
    let repository = ArtifactRepository::new(ready).map_err(|_| Code::RepositoryRejected)?;
    let layout = repository
        .load_layout()
        .map_err(|_| Code::RepositoryRejected)?;
    if layout
        .as_ref()
        .and_then(|layout| layout.artifact().nodes.get(&id))
        .is_some_and(|node| node.state == LayoutState::Trashed)
    {
        return Err(Code::WrongBinding.into());
    }
    let document = repository
        .load_document(id)
        .map_err(|_| Code::RepositoryRejected)?;
    let template = repository
        .load_template(document.artifact().template_id())
        .map_err(|_| Code::PdfTemplateUnavailable)?;
    // A changed Template cannot be presented as the document's saved revision.
    if document.artifact().template_revision() != template.artifact().revision() {
        return Err(Code::PdfTemplateUnavailable.into());
    }
    let mut ignored = Vec::new();
    let read = read_response(document.artifact(), template.artifact(), &mut ignored)?;
    let Response::Read { fields, name, .. } = &read else {
        return Err(Code::SerializationFailed.into());
    };
    if fields
        .iter()
        .any(|field| field.value.is_none() && field.problem.is_some())
    {
        return Err(Code::RepositoryRejected.into());
    }
    let mut asset_ids = BTreeMap::new();
    let mut link_ids = BTreeSet::new();
    for field in fields {
        if let Some(value) = &field.value {
            pdf_references(value, &field.label, &mut asset_ids, &mut link_ids);
        }
    }
    let root = ready.locked_project().canonical_root();
    let store = if asset_ids.is_empty() {
        None
    } else {
        Some(crate::data::assets::Store::open(root, false).map_err(|_| Code::RepositoryRejected)?)
    };
    let mut source = Sha256::new();
    source.update(ready.locked_project().fingerprint().as_bytes());
    source.update(id.to_string().as_bytes());
    source.update(document.source().sha256());
    source.update(template.source().sha256());
    if let Some(layout) = &layout {
        source.update(layout.source().sha256());
    }
    let mut assets = BTreeMap::new();
    let mut asset_checks = BTreeMap::new();
    let mut missing_images = Vec::new();
    let mut asset_bytes = 0_usize;
    for (id, (image, label)) in asset_ids {
        match store.as_ref().expect("asset IDs require store").read(&id) {
            Ok((metadata, bytes)) => {
                asset_bytes = asset_bytes
                    .checked_add(bytes.len())
                    .filter(|sum| *sum <= MAX_PDF_ASSET_INPUT_BYTES)
                    .ok_or(Code::Full)?;
                asset_checks.insert(
                    id.clone(),
                    Some((
                        metadata.image,
                        metadata.name.clone(),
                        <[u8; 32]>::from(Sha256::digest(&bytes)),
                    )),
                );
                source.update(id.as_bytes());
                source.update(metadata.name.as_bytes());
                source.update([u8::from(metadata.image)]);
                source.update(Sha256::digest(&bytes));
                if metadata.image != image || (image && !crate::pdf_export::image_decodable(&bytes))
                {
                    if image {
                        missing_images.push(label);
                    }
                    assets.insert(id, None);
                } else {
                    assets.insert(
                        id,
                        Some(crate::pdf_export::Asset {
                            name: metadata.name,
                            bytes,
                            image,
                        }),
                    );
                }
            }
            _ => {
                asset_checks.insert(id.clone(), None);
                source.update(id.as_bytes());
                source.update(b"missing");
                if image {
                    missing_images.push(label);
                }
                assets.insert(id, None);
            }
        }
    }
    let mut links = BTreeMap::new();
    let mut link_sources = Vec::new();
    let mut missing_link_ids = Vec::new();
    for target in link_ids {
        let found = target
            .parse()
            .ok()
            .and_then(|id| repository.load_document(id).ok());
        if let Some(found) = found {
            source.update(target.as_bytes());
            source.update(found.source().sha256());
            link_sources.push(found.source().clone());
            links.insert(target, Some(found.artifact().name().to_owned()));
        } else {
            source.update(target.as_bytes());
            source.update(b"missing-link");
            if let Ok(id) = target.parse() {
                missing_link_ids.push(id);
            }
            links.insert(target, None);
        }
    }
    if !repository
        .reread_matches(document.source())
        .map_err(|_| Code::RepositoryRejected)?
        || !repository
            .reread_matches(template.source())
            .map_err(|_| Code::RepositoryRejected)?
    {
        return Err(Code::WrongBinding.into());
    }
    for source in &link_sources {
        if !repository
            .reread_matches(source)
            .map_err(|_| Code::RepositoryRejected)?
        {
            return Err(Code::WrongBinding.into());
        }
    }
    if let Some(layout) = &layout {
        if !repository
            .reread_matches(layout.source())
            .map_err(|_| Code::RepositoryRejected)?
        {
            return Err(Code::WrongBinding.into());
        }
    }
    for id in missing_link_ids {
        if repository.load_document(id).is_ok() {
            return Err(Code::WrongBinding.into());
        }
    }
    for (id, expected) in &asset_checks {
        let actual = store.as_ref().expect("asset IDs require store").read(id);
        match (expected, actual) {
            (Some((image, name, digest)), Ok((metadata, bytes)))
                if metadata.image == *image
                    && metadata.name == *name
                    && <[u8; 32]>::from(Sha256::digest(&bytes)) == *digest => {}
            (None, Err(_)) => {}
            _ => return Err(Code::WrongBinding.into()),
        }
    }
    Ok(crate::pdf_export::Snapshot {
        document: id.to_string(),
        name: name.clone(),
        source: format!("{:x}", source.finalize()),
        read,
        assets,
        links,
        missing_images,
    })
}

#[cfg(test)]
type IssueHook = Box<dyn FnOnce() + Send>;
#[cfg(test)]
static ISSUE_HOOKS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, IssueHook>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
pub(crate) fn install_issue_snapshot_hook(id: String, hook: IssueHook) {
    ISSUE_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(id, hook);
}
#[cfg(test)]
fn run_issue_snapshot_hook(id: String) {
    if let Some(hook) = ISSUE_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&id)
    {
        hook();
    }
}

#[derive(Default)]
pub(crate) struct Registry {
    pub(super) editors: super::document_edit::Registry,
    // 목록 snapshot은 같은 project에서 생성 시작 화면을 여는 데만 재사용한다.
    // 실제 저장 권한과 외부 변경 검사는 write 직전 repository scan/source 대조가 소유한다.
    snapshots: BTreeMap<Id, CachedLayoutSnapshot>,
    drafts: BTreeMap<Id, Creation>,
    previews: BTreeMap<(Id, String), (crate::data::assets::Metadata, Arc<Vec<u8>>)>,
    searches: BTreeMap<Id, super::document_search::Cache>,
    replacements: BTreeMap<Id, super::document_replace::Preview>,
}
struct CachedLayoutSnapshot {
    project: Id,
    layout: LayoutSnapshot,
    summaries: Vec<Summary>,
    documents: Arc<CompleteDocumentScan>,
}
struct Creation {
    project: Id,
    fingerprint: String,
    draft_id: String,
    generation: u64,
    document: artifact::DocumentId,
    template: Arc<View>,
    base: LayoutSnapshot,
    documents: Arc<CompleteDocumentScan>,
    base_documents: Vec<Summary>,
    body: CreationBody,
    deposit: Option<Deposit>,
    restore_assets: Option<Deposit>,
    proof: Option<Proof>,
    outcome: Option<Box<ResultDto>>,
    original: Option<Box<dyn Any + Send>>,
    binding: Option<Binding>,
    problem: Option<String>,
    field: Option<String>,
    committed: bool,
    saved_generation: Option<u64>,
    created_snapshot: Option<Original>,
    attempt: Option<crate::data::edit_recovery::model::Attempt>,
    uncertain: bool,
    commit: Option<Box<CreationCommit>>,
}

fn scan_summaries(scan: &CompleteDocumentScan) -> Vec<Summary> {
    scan.summaries()
        .map(
            |(id, template, name, english_name, glossary_summary, glossary_excluded)| Summary {
                id: id.to_string(),
                template: template.to_string(),
                name: name.into(),
                english_name: english_name.into(),
                glossary_summary: glossary_summary.into(),
                glossary_excluded,
            },
        )
        .collect()
}

/// 편집 저장이 확정한 동일 이전 source를 가진 cache/초안만 새 scan으로 교체한다.
/// generation이나 source가 다른 항목은 손대지 않아 기존 prepare 거절 경계를 보존한다.
fn apply_committed_edit(
    ctx: &mut Context,
    registry: &mut Registry,
    project: Id,
    change: &DocumentScanChange,
    generation: Id,
    observations: &mut Vec<Box<dyn Any + Send>>,
) {
    for snapshot in registry
        .snapshots
        .values_mut()
        .filter(|snapshot| snapshot.project == project)
    {
        if let Some(next) = snapshot.documents.apply_committed_change(change) {
            snapshot.summaries = scan_summaries(&next);
            snapshot.documents = Arc::new(next);
        }
    }
    for draft in registry
        .drafts
        .values_mut()
        .filter(|draft| draft.project == project && !draft.committed && !draft.uncertain)
    {
        if let Some(next) = draft.documents.apply_committed_change(change) {
            draft.base_documents = scan_summaries(&next);
            draft.documents = Arc::new(next);
        }
    }
    let search_updated = registry.searches.get_mut(&project).is_some_and(|cache| {
        super::document_search::apply_committed_change(ctx, cache, change, generation, observations)
            .is_ok()
    });
    if registry.searches.contains_key(&project) && !search_updated {
        // candidate 재독이나 projection이 확정되지 않으면 이전 Entry를 최신으로
        // 표시하지 않는다. 다음 검색이 전체 자료를 안전하게 다시 준비한다.
        registry.searches.remove(&project);
    }
}
type LayoutWriteResult = (
    Completed,
    Option<Binding>,
    Option<(Original, Attempt)>,
    Option<(LayoutCommit, LayoutSnapshot, Arc<CompleteDocumentScan>)>,
);
type LayoutSnapshotResult = (
    LayoutSnapshot,
    Vec<Summary>,
    Vec<String>,
    Option<String>,
    Arc<CompleteDocumentScan>,
);
impl Registry {
    pub(crate) fn len(&self) -> usize {
        self.drafts.len() + self.editors.len()
    }
    pub(crate) fn retains(&self, key: &Key) -> bool {
        self.editors.retains(key)
            || self
                .drafts
                .values()
                .any(|d| d.fingerprint == key.project_fingerprint && d.draft_id == key.draft_id)
    }

    pub(crate) fn release_project(&mut self, project: Id) {
        debug_assert!(!self.drafts.values().any(|draft| draft.project == project));
        self.snapshots
            .retain(|_, snapshot| snapshot.project != project);
        self.previews.retain(|(owner, _), _| *owner != project);
        self.searches.remove(&project);
        self.replacements.remove(&project);
    }

    #[cfg(test)]
    pub(crate) fn has_project_cache(&self, project: Id) -> bool {
        self.snapshots
            .values()
            .any(|snapshot| snapshot.project == project)
            || self.previews.keys().any(|(owner, _)| *owner == project)
            || self.searches.contains_key(&project)
            || self.replacements.contains_key(&project)
    }
}
pub(super) fn reply(value: Response) -> Completed {
    Completed::new(
        (),
        Ok(ResultDto::DocumentWorkspace {
            value: Box::new(value),
        }),
    )
}
fn draft_reply(owner: Id, d: &Creation) -> Reply<Completed> {
    Ok(reply(Response::Draft {
        owner,
        generation: d.generation.to_string(),
        body: d.body.clone(),
        template: projection::template(match &*d.template {
            View::Template(t) => t.artifact(),
            _ => return Err(Code::WrongBinding.into()),
        })?,
        deposited: d
            .deposit
            .as_ref()
            .zip(d.proof.as_ref())
            .is_some_and(|(a, p)| {
                p.matches(
                    &a.envelope().key,
                    &a.envelope().deposit_id,
                    a.payload_digest(),
                )
            }),
        outcome: d.outcome.clone(),
        problem: d.problem.clone(),
        field: d.field.clone(),
        commit: d.commit.clone(),
    }))
}
fn layout_snapshot(
    ctx: &mut Context,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<LayoutSnapshotResult> {
    let result = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready)?;
            let scan = repository.scan_documents()?;
            let ids: BTreeSet<_> = scan.summaries().map(|(id, _, _, _, _, _)| id).collect();
            let documents = scan
                .summaries()
                .map(
                    |(id, t, name, english_name, glossary_summary, glossary_excluded)| Summary {
                        id: id.to_string(),
                        template: t.to_string(),
                        name: name.into(),
                        english_name: english_name.into(),
                        glossary_summary: glossary_summary.into(),
                        glossary_excluded,
                    },
                )
                .collect();
            let loaded = repository.load_layout();
            let (layout, source, problem) = match loaded {
                Ok(Some(v)) => {
                    let source = Some(v.source().clone());
                    (v.into_artifact(), source, None)
                }
                Ok(None) => (DocumentLayout::flat(ids.iter().copied()), None, None),
                Err(e) => {
                    let category = format!("{:?}", e.diagnostic().category);
                    observations.push(Box::new(e));
                    (
                        DocumentLayout::flat(ids.iter().copied()),
                        None,
                        Some(category),
                    )
                }
            };
            let (unplaced, problem) = match layout.reconcile(&ids) {
                Ok(ids) => (ids.iter().map(ToString::to_string).collect(), problem),
                Err(e) => (vec![], Some(format!("{e:?}"))),
            };
            Ok::<_, crate::data::repository::RepositoryError>((
                LayoutSnapshot { layout, source },
                documents,
                unplaced,
                problem,
                Arc::new(scan),
            ))
        })
        .map_err(|e| observed(observations, e, Code::RuntimeRejected))?;
    result.map_err(|e| observed(observations, e, Code::RepositoryRejected))
}

/// Projects validation onto the exact layout/document source set returned by List.
/// A failed second read is explicit uncertainty, never an empty healthy issue set.
fn issue_snapshot(
    ctx: &mut Context,
    layout: &LayoutSnapshot,
    scan: &CompleteDocumentScan,
) -> (
    Vec<crate::commands::document_workspace::DocumentValidationIssue>,
    String,
    Vec<String>,
) {
    use crate::commands::document_workspace::DocumentValidationIssue;
    use crate::data::artifact::TemplateLifecycle;
    use crate::data::repository::{RepositoryCategory, RepositoryError, SourceToken};

    #[derive(Clone, PartialEq, Eq)]
    enum TemplateObservation {
        Source(SourceToken),
        Absent,
        Unreadable,
    }

    let active_ids: Vec<_> = scan
        .summaries()
        .filter(|(id, _, _, _, _, _)| {
            layout
                .layout
                .nodes
                .get(id)
                .is_none_or(|node| node.state == LayoutState::Active)
        })
        .map(|(id, _, _, _, _, _)| id)
        .collect();
    let fallback = || {
        (
            Vec::new(),
            "unavailable".into(),
            active_ids.iter().map(ToString::to_string).collect(),
        )
    };
    let result = ctx.read(|ready| {
        let repository = ArtifactRepository::new(ready)?;
        let current_layout = repository.load_layout()?;
        if current_layout.as_ref().map(|value| value.source()) != layout.source.as_ref() {
            return Ok::<_, RepositoryError>(None);
        }
        let mut issues = Vec::new();
        let mut unverified = Vec::new();
        let mut template_observations = BTreeMap::new();
        let mut template_documents: BTreeMap<_, Vec<String>> = BTreeMap::new();
        let mut unstable_templates = BTreeSet::new();
        for id in &active_ids {
            let Some(expected) = scan.source(*id) else {
                unverified.push(id.to_string());
                continue;
            };
            let document = match repository.load_document(*id) {
                Ok(document) if document.source() == expected => document,
                _ => {
                    unverified.push(id.to_string());
                    continue;
                }
            };
            let mut reasons = Vec::new();
            let template_id = document.artifact().template_id();
            template_documents
                .entry(template_id)
                .or_default()
                .push(id.to_string());
            let loaded = repository.load_template(template_id);
            let observation = match &loaded {
                Ok(template) => TemplateObservation::Source(template.source().clone()),
                Err(error) if error.diagnostic().category == RepositoryCategory::NotFound => {
                    TemplateObservation::Absent
                }
                Err(_) => TemplateObservation::Unreadable,
            };
            if let Some(first) = template_observations.get(&template_id) {
                if first != &observation {
                    unstable_templates.insert(template_id);
                }
            } else {
                template_observations.insert(template_id, observation);
                #[cfg(test)]
                run_issue_snapshot_hook(template_id.to_string());
            }
            if unstable_templates.contains(&template_id) {
                unverified.push(id.to_string());
                continue;
            }
            let template = match loaded {
                Ok(template) => template,
                Err(error) if error.diagnostic().category == RepositoryCategory::NotFound => {
                    issues.push(DocumentValidationIssue {
                        document: id.to_string(),
                        warnings: Vec::new(),
                        reasons: vec!["template_missing".into()],
                    });
                    continue;
                }
                Err(_) => {
                    unverified.push(id.to_string());
                    continue;
                }
            };
            if template.artifact().lifecycle() == TemplateLifecycle::Deleted {
                reasons.push("template_in_trash".into());
            }
            let warnings = match super::document_search::validation_codes_for_document(
                document.artifact(),
                template.artifact(),
            ) {
                Ok(warnings) => warnings,
                Err(_) => {
                    unverified.push(id.to_string());
                    continue;
                }
            };
            if !warnings.is_empty() || !reasons.is_empty() {
                issues.push(DocumentValidationIssue {
                    document: id.to_string(),
                    warnings,
                    reasons,
                });
            }
        }
        for (id, first) in &template_observations {
            let stable = match first {
                TemplateObservation::Source(source) => {
                    repository.reread_bytes_match(source).unwrap_or(false)
                }
                TemplateObservation::Absent => matches!(
                    repository.load_template(*id),
                    Err(error) if error.diagnostic().category == RepositoryCategory::NotFound
                ),
                TemplateObservation::Unreadable => false,
            };
            if !stable {
                unstable_templates.insert(*id);
            }
        }
        for template in unstable_templates {
            if let Some(documents) = template_documents.get(&template) {
                issues.retain(|issue| !documents.contains(&issue.document));
                unverified.extend(documents.iter().cloned());
            }
        }
        unverified.sort();
        unverified.dedup();
        let membership = scan.sources().map(|source| source.id()).collect();
        if repository.confirm_document_membership(&membership).is_err()
            || scan
                .sources()
                .any(|source| !repository.reread_bytes_match(source).unwrap_or(false))
        {
            return Ok(None);
        }
        Ok(Some((issues, unverified)))
    });
    match result {
        Ok(Ok(Some((issues, unverified)))) => (
            issues,
            if unverified.is_empty() {
                "complete"
            } else {
                "partial"
            }
            .into(),
            unverified,
        ),
        _ => fallback(),
    }
}
fn read_response(
    document: &artifact::DocumentArtifact,
    template: &artifact::TemplateArtifact,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Response> {
    let reconciled = artifact::reconcile_document_for_read(template, document)
        .map_err(|error| observed(observations, error, Code::RepositoryRejected))?;
    let field_problems = reconciled
        .warnings()
        .iter()
        .map(|warning| (warning.field_id(), format!("{:?}", warning.category())))
        .chain(
            reconciled
                .blocking_issues()
                .iter()
                .map(|issue| (issue.field_id(), format!("{:?}", issue.category()))),
        )
        .collect::<BTreeMap<_, _>>();
    let mut fields = Vec::new();
    for field in reconciled.known_fields() {
        fields.push(ReadField {
            id: field.field_id().to_string(),
            label: field.label().into(),
            state: format!("{:?}", field.lifecycle()),
            provenance: field.provenance().map(|value| format!("{value:?}")),
            value: field
                .value()
                .map(|value| {
                    projection::field_value(value, template.fields().get(&field.field_id()))
                })
                .transpose()?,
            problem: field_problems.get(&field.field_id()).cloned(),
        });
    }
    for field in reconciled.orphan_fields() {
        // A known definition and its snapshot describe one stored field. Snapshot
        // creation/preservation must not add a second visible row. Keep genuine
        // orphans and blocked reattachments, including their diagnostics.
        if reconciled
            .known_fields()
            .iter()
            .any(|known| known.field_id() == field.field_id())
            && matches!(
                field.disposition(),
                artifact::OrphanFieldDisposition::ReattachableOrphan
                    | artifact::OrphanFieldDisposition::SnapshotRequired
                    | artifact::OrphanFieldDisposition::PreservedOrphan
            )
        {
            continue;
        }
        fields.push(ReadField {
            id: field.field_id().to_string(),
            label: field
                .display_label()
                .map(str::to_owned)
                .unwrap_or_else(|| "현재 템플릿에서 이름을 찾을 수 없는 필드".into()),
            state: "Orphan".into(),
            provenance: Some("ExistingValue".into()),
            value: Some(projection::value(field.value())?),
            problem: None,
        });
    }
    let warnings = reconciled
        .warnings()
        .iter()
        .map(|warning| format!("{:?}", warning.category()))
        .chain(
            reconciled
                .blocking_issues()
                .iter()
                .map(|warning| format!("{:?}", warning.category())),
        )
        .collect();
    Ok(Response::Read {
        schema: document.schema_version().get(),
        id: document.document_id().to_string(),
        name: document.name().into(),
        english_name: document.english_name().into(),
        glossary_summary: document.glossary_summary().into(),
        glossary_excluded: document.glossary_excluded(),
        template: projection::template(template)?,
        fields,
        warnings,
    })
}
pub(super) fn generation(raw: &str) -> Reply<u64> {
    let n = raw.parse::<u64>().map_err(|_| Code::InvalidInput)?;
    if n == 0 || n.to_string() != raw {
        return Err(Code::InvalidInput.into());
    }
    Ok(n)
}
fn update(d: &mut Creation, raw: &str, body: &CreationBody) -> Reply<()> {
    let g = generation(raw)?;
    if g < d.generation || (g == d.generation && body != &d.body) {
        return Err(Code::WrongBinding.into());
    }
    if g != d.generation {
        d.deposit = None;
        d.proof = None;
    }
    d.body = body.clone();
    d.generation = g;
    d.problem = None;
    d.field = None;
    Ok(())
}

/// 새로 Set한 참조만 현재 project/layout에 대조한다. 기존 dangling 값은 Keep 편집을 막지 않는다.
pub(super) fn validate_reference_inputs(
    ctx: &mut Context,
    template: &artifact::TemplateArtifact,
    current: artifact::DocumentId,
    fields: &[crate::data::edit_recovery::model::DraftValue],
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Option<(String, String)>> {
    struct Check {
        address: String,
        target: artifact::DocumentId,
        relation: bool,
        allowed: Vec<artifact::TemplateId>,
    }
    fn collect(
        value: &ValueDto,
        definition: &artifact::FieldDefinition,
        address: String,
        checks: &mut Vec<Check>,
    ) -> Reply<()> {
        match value {
            ValueDto::Relation { links } => {
                let Some((_, allowed, _)) = definition.configuration().relation_settings() else {
                    return Ok(());
                };
                for link in links {
                    checks.push(Check {
                        address: address.clone(),
                        target: convert::id(&link.document)?,
                        relation: true,
                        allowed: allowed.to_vec(),
                    });
                }
            }
            ValueDto::DocumentLink { documents } => {
                if definition.kind() != artifact::FieldKind::DocumentLink {
                    return Ok(());
                }
                for document in documents {
                    checks.push(Check {
                        address: address.clone(),
                        target: convert::id(document)?,
                        relation: false,
                        allowed: Vec::new(),
                    });
                }
            }
            ValueDto::Group { instances } => {
                let Some((_, members)) = definition.configuration().members() else {
                    return Ok(());
                };
                for instance in instances {
                    for child in &instance.fields {
                        let child_id = convert::id(&child.field)?;
                        let Some(child_definition) = members.get(&child_id) else {
                            continue;
                        };
                        if let Intent::Set(value) = &child.value {
                            collect(
                                value,
                                child_definition,
                                serde_json::to_string(&[
                                    address.as_str(),
                                    &instance.id.to_string(),
                                    &child.field,
                                ])
                                .map_err(|_| Code::SerializationFailed)?,
                                checks,
                            )?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut checks = Vec::new();
    for field in fields {
        let Intent::Set(value) = &field.value else {
            continue;
        };
        let id = convert::id(&field.field)?;
        let Some(definition) = template.fields().get(&id) else {
            continue;
        };
        collect(value, definition, field.field.clone(), &mut checks)?;
    }
    for check in checks {
        if check.relation && check.target == current {
            return Ok(Some(("SelfRelation".into(), check.address)));
        }
        let target_template = if !check.relation && check.target == current {
            Some(template.template_id())
        } else {
            ctx.read(|ready| {
                let repository = ArtifactRepository::new(ready)?;
                let layout = repository.load_layout()?;
                if layout.as_ref().is_some_and(|layout| {
                    layout
                        .artifact()
                        .nodes
                        .get(&check.target)
                        .is_some_and(|node| node.state == LayoutState::Trashed)
                }) {
                    return Ok(None);
                }
                match repository.load_document(check.target) {
                    Ok(document) => Ok(Some(document.artifact().template_id())),
                    Err(error)
                        if error.diagnostic().category
                            == crate::data::repository::RepositoryCategory::NotFound =>
                    {
                        Ok(None)
                    }
                    Err(error) => Err(error),
                }
            })
            .map_err(|error| observed(observations, error, Code::RuntimeRejected))?
            .map_err(|error| observed(observations, error, Code::RepositoryRejected))?
        };
        let valid = target_template.is_some_and(|target_template| {
            !check.relation || check.allowed.is_empty() || check.allowed.contains(&target_template)
        });
        if !valid {
            return Ok(Some(("InvalidDocumentReference".into(), check.address)));
        }
    }
    Ok(None)
}
fn load_template(
    ctx: &mut Context,
    id: &str,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Arc<View>> {
    let id = convert::id(id)?;
    let value = ctx
        .read(|ready| ArtifactRepository::new(ready)?.load_template(id))
        .map_err(|e| observed(observations, e, Code::RuntimeRejected))?
        .map_err(|e| observed(observations, e, Code::RepositoryRejected))?;
    if value.artifact().lifecycle() != artifact::TemplateLifecycle::Active {
        return Err(Code::WrongBinding.into());
    }
    Ok(Arc::new(View::Template(value)))
}
pub(super) fn observed<E: Any + Send>(
    observations: &mut Vec<Box<dyn Any + Send>>,
    error: E,
    code: Code,
) -> ErrorDto {
    observations.push(Box::new(error));
    code.into()
}
pub(super) fn finish_session(ctx: &mut Context, binding: &Binding) -> (bool, Box<dyn Any + Send>) {
    let mut cleanup = ctx.session_control();
    let observation = cleanup.observe_session(&binding.registration);
    let Ok((s, _)) = &observation else {
        return (false, Box::new(observation));
    };
    let release = if s.state() != crate::data::edit_session::EditSessionState::ReadOnly {
        Some(
            if s.state() == crate::data::edit_session::EditSessionState::ReleaseFailed {
                cleanup.retry_session_release(&binding.registration)
            } else {
                cleanup.end_session_observed(&binding.registration)
            },
        )
    } else {
        None
    };
    let release_ok = release
        .as_ref()
        .is_none_or(|r| r.as_ref().is_ok_and(|r| r.original.is_ok()));
    let removal = if release_ok {
        Some(cleanup.remove_session(&binding.registration))
    } else {
        None
    };
    (
        matches!(&removal, Some(Ok(()))),
        Box::new((observation, release, removal)),
    )
}
pub(super) fn write_layout(
    ctx: &mut Context,
    job: &Job,
    input: &LayoutWrite,
    recovery_assets: Option<(&Deposit, &Draft)>,
) -> Reply<LayoutWriteResult> {
    // LayoutWrite가 그대로 encode하여 제출하는 후보를 결과와 함께 보유한다.
    // 이 snapshot은 읽기 권한 토큰이 아니며 committed/uncertain 판정은 실제 실행 결과만 한다.
    let candidate = input
        .create
        .as_ref()
        .map(|(document, _, _)| {
            let bytes =
                artifact::encode_document(document).map_err(|_| Code::SerializationFailed)?;
            let digest = crate::data::edit_recovery::model::digest(&bytes);
            Ok::<_, ErrorDto>(Original {
                kind: OriginalKind::Document,
                artifact_id: document.document_id().to_string(),
                schema: document.schema_version().get(),
                template_revision: document.template_revision().get(),
                source_byte_length: bytes.len() as u64,
                source_digest: digest.clone(),
                snapshot_digest: digest,
                snapshot: String::from_utf8(bytes).map_err(|_| Code::SerializationFailed)?,
            })
        })
        .transpose()?;
    let (binding, registration) =
        begin(ctx, job, input.targets().map_err(|_| Code::InvalidInput)?)?;
    let Some(key) = &binding.key else {
        let mut c = Completed::reject(registration, Code::SessionRejected.into());
        c.binding = Some(binding.clone());
        return Ok((c, Some(binding), None, None));
    };
    if let Some((deposit, chosen)) = recovery_assets {
        let copied = (|| -> Reply<()> {
            ctx.read(|ready| {
                let repository = ArtifactRepository::new(ready)?;
                let layout = repository.load_layout()?;
                if layout.as_ref().map(|l| l.source()) != input.base.source.as_ref() {
                    return Ok(false);
                }
                let membership = input
                    .documents
                    .sources()
                    .map(|source| source.id())
                    .collect();
                repository.confirm_document_membership(&membership)?;
                for source in input.documents.sources() {
                    if !repository.reread_bytes_match(source)? {
                        return Ok(false);
                    }
                }
                if let Some((document, template, _)) = &input.create {
                    if repository.load_template(template.id)?.source() != &template.token {
                        return Ok(false);
                    }
                    if input.documents.source(document.document_id()).is_some() {
                        return Ok(false);
                    }
                }
                Ok::<_, crate::data::repository::RepositoryError>(true)
            })
            .map_err(|_| Code::RuntimeRejected)?
            .map_err(|_| Code::RepositoryRejected)
            .and_then(|same| {
                if same {
                    Ok(())
                } else {
                    Err(Code::WrongBinding.into())
                }
            })?;
            let store = job.recovery.connect().map_err(|_| Code::SinkUnavailable)?;
            let store = store.lock().map_err(|_| Code::Unavailable)?;
            ctx.read(|ready| {
                store.restore_selected_assets(
                    deposit,
                    chosen,
                    ready.locked_project().canonical_root(),
                )
            })
            .map_err(|_| Code::RuntimeRejected)?
            .map_err(|_| Code::RecoveryRejected)?;
            if let Some((document, _, _)) = &input.create {
                let raw =
                    artifact::encode_document(document).map_err(|_| Code::SerializationFailed)?;
                ctx.read(|ready| {
                    crate::data::assets::validate_document(
                        ready.locked_project().canonical_root(),
                        &raw,
                    )
                })
                .map_err(|_| Code::RuntimeRejected)?
                .map_err(|_| Code::PreparationRejected)?;
            }
            Ok(())
        })();
        if let Err(error) = copied {
            let (released, cleanup) = finish_session(ctx, &binding);
            let mut completed = Completed::reject((registration, cleanup), error);
            if !released {
                completed.binding = Some(binding.clone());
            }
            return Ok((completed, (!released).then_some(binding), None, None));
        }
    }
    let execution = ctx
        .session(key)
        .map_err(|_| Code::SessionRejected)?
        .write_layout(input);
    let evidence = execution.as_ref().ok().and_then(|e| {
        let diagnostic = e.diagnostic();
        if !matches!(diagnostic.disk, DiskState::Committed | DiskState::Uncertain) {
            return None;
        }
        candidate.map(|snapshot| {
            let mut attempt = recovery::attempt(job.operation, Some(diagnostic));
            attempt.candidate_digest = Some(snapshot.snapshot_digest.clone());
            (snapshot, attempt)
        })
    });
    let commit = execution.as_ref().ok().and_then(|execution| {
        if execution.diagnostic().disk != DiskState::Committed {
            return None;
        }
        execution.value().cloned()
    });
    let baseline = if commit.is_some() {
        input.create.as_ref().map(|(document, _, _)| {
            super::creation_baseline(
                ctx,
                ArtifactSourceId::Document(document.document_id()),
                artifact::encode_document(document),
                &document.updated_at_utc(),
            )
        })
    } else {
        None
    };
    let warnings = if baseline.as_ref().is_some_and(|result| result.is_err()) {
        vec![WarningDto {
            category: "content_version_unavailable".into(),
            field: None,
        }]
    } else {
        vec![]
    };
    let dto = execution
        .as_ref()
        .map(|e| {
            write(
                binding.id,
                input
                    .create
                    .as_ref()
                    .map(|(d, _, _)| d.document_id().to_string()),
                e.diagnostic(),
                Some(true),
                warnings,
            )
        })
        .map_err(|_| Code::SaveRejected.into());
    let (released, cleanup) = finish_session(ctx, &binding);
    // prepare에서 정확히 재대조한 scan을 다음 UI 작업에 재사용한다. 생성이면 커밋된
    // 신규 문서 하나만 읽고, 구조 변경이면 같은 immutable scan을 그대로 유지한다.
    let committed_snapshot = commit.and_then(|commit| {
        let created = input
            .create
            .as_ref()
            .map(|(document, _, _)| document.document_id());
        let loaded = ctx.read(|ready| {
            let repository = ArtifactRepository::new(ready)?;
            let layout = repository.load_layout()?;
            let documents = match created {
                Some(id) => repository.extend_document_scan(&input.documents, id)?,
                None => input.documents.as_ref().clone(),
            };
            Ok::<_, crate::data::repository::RepositoryError>((layout, Arc::new(documents)))
        });
        let Ok(Ok((Some(loaded), documents))) = loaded else {
            return None;
        };
        let same_layout = artifact::encode_layout(loaded.artifact())
            .ok()
            .zip(artifact::encode_layout(&commit.layout).ok())
            .is_some_and(|(actual, expected)| actual == expected);
        if !same_layout {
            return None;
        }
        let source = loaded.source().clone();
        Some((
            commit.clone(),
            LayoutSnapshot {
                layout: commit.layout,
                source: Some(source),
            },
            documents,
        ))
    });
    let mut c = Completed::new((registration, execution, cleanup, baseline), dto);
    if !released {
        c.binding = Some(binding.clone());
    }
    Ok((
        c,
        (!released).then_some(binding),
        evidence,
        committed_snapshot,
    ))
}
pub(crate) fn execute(
    ctx: &mut Context,
    job: &Job,
    project: Id,
    request: &Request,
) -> Reply<Completed> {
    let mut observations: Vec<Box<dyn Any + Send>> = vec![];
    let result = (|| -> Reply<Completed> {
        let mut workspace = job.workspace.lock().map_err(|_| Code::Unavailable)?;
        let template_owner = workspace.has_template_owner(project);
        let registry = &mut workspace.documents;
        match request {
            Request::AssetImport {
                cell,
                owner,
                generation: g,
                field,
                image,
            } => {
                let valid = registry.drafts.get(owner).is_some_and(|d| {
                    d.project == project
                        && d.generation.to_string() == *g
                        && !d.committed
                        && match &*d.template {
                            View::Template(t) if cell.is_some() => {
                                cell.as_ref().is_some_and(|cell| {
                                    artifact::group::asset_target(
                                        t.artifact(),
                                        &d.body.fields,
                                        None,
                                        field,
                                        cell,
                                        *image,
                                    )
                                })
                            }
                            View::Template(t) => field
                                .parse()
                                .ok()
                                .and_then(|id| t.artifact().fields().get(&id))
                                .is_some_and(|f| {
                                    f.lifecycle() == artifact::FieldLifecycle::Active
                                        && f.kind()
                                            == if *image {
                                                artifact::FieldKind::Image
                                            } else {
                                                artifact::FieldKind::File
                                            }
                                }),
                            _ => false,
                        }
                }) || registry.editors.asset_owner(
                    project,
                    *owner,
                    g,
                    field,
                    *image,
                    cell.as_ref(),
                );
                if !valid {
                    return Err(Code::WrongBinding.into());
                }
                let Some(path) = crate::commands::asset_picker::pick(*image)? else {
                    return Ok(reply(Response::AssetDone {}));
                };
                if job.progress.snapshot().0 {
                    return Err(Code::Cancelled.into());
                }
                if job.collaborative {
                    job.provider
                        .authorize_new_asset(&String::from(job.operation))
                        .map_err(|_| Code::RuntimeRejected)?;
                }
                let result = ctx
                    .read(|ready| {
                        crate::data::assets::Store::open(
                            ready.locked_project().canonical_root(),
                            true,
                        )?
                        .import(
                            &path,
                            *image,
                            &String::from(job.operation),
                        )
                    })
                    .map_err(|_| Code::RuntimeRejected)?;
                match result {
                    Ok(metadata) => {
                        let content_type = metadata
                            .image
                            .then(|| crate::data::assets::image_content_type(&metadata.name))
                            .flatten();
                        Ok(reply(Response::Asset {
                            metadata,
                            content_type,
                        }))
                    }
                    Err(error) => Ok(asset_error(error, None, None)),
                }
            }
            Request::AssetRead { asset, .. }
            | Request::AssetChunk { asset, .. }
            | Request::AssetOpen { asset } => {
                // 분할 전송은 검증한 동일 snapshot을 사용한다. 매 64 KiB마다 PNG를 재해독하지 않는다.
                if let Request::AssetChunk { digest, offset, .. } = request {
                    ctx.read(|_| ())
                        .map_err(|e| observed(&mut observations, e, Code::RuntimeRejected))?;
                    if let Some((meta, bytes)) = registry.previews.get(&(project, asset.clone())) {
                        if meta.sha256 == *digest {
                            return asset_chunk(meta, bytes, digest, *offset);
                        }
                    }
                }
                let result = ctx
                    .read(|ready| {
                        let root = ready.locked_project().canonical_root();
                        let loaded = crate::data::assets::Store::open(root, false)
                            .and_then(|store| store.read(asset));
                        let (metadata, bytes) = match loaded {
                            Ok(value) => value,
                            Err(error) => {
                                let mut context = asset_reference_context(root, asset);
                                if context.0.is_none() {
                                    if let Request::AssetRead { target: Some(target), .. } = request {
                                        let id = match target.kind.as_str() {
                                            "document" => target.artifact.parse().ok().map(ArtifactSourceId::Document),
                                            "template" => target.artifact.parse().ok().map(ArtifactSourceId::Template),
                                            _ => None,
                                        };
                                        if let (Some(id), Ok(repository)) = (id, ArtifactRepository::new(ready)) {
                                            context.0 = crate::data::repository::versions::known_asset_name(&repository, id, asset).ok().flatten();
                                        }
                                    }
                                }
                                return Err((error, context));
                            }
                        };
                        if matches!(request, Request::AssetOpen { .. }) {
                            if let Err(error) = super::asset_open::open_asset(&metadata, &bytes) {
                                return Err((
                                    error,
                                    (Some(metadata.name.clone()), Some("open_failed")),
                                ));
                            }
                        }
                        Ok::<
                            _,
                            (
                                crate::data::edit_recovery::error::RecoveryError,
                                (Option<String>, Option<&'static str>),
                            ),
                        >((metadata, bytes))
                    })
                    .map_err(|_| Code::RuntimeRejected)?;
                match result {
                    Err((error, (name, state))) => Ok(asset_error(error, name, state)),
                    Ok((metadata, bytes)) => {
                        if metadata.image && !matches!(request, Request::AssetOpen { .. }) {
                            let key = (project, asset.clone());
                            registry.previews.remove(&key);
                            while registry.previews.len() >= 8
                                || registry
                                    .previews
                                    .values()
                                    .map(|(_, b)| b.len())
                                    .sum::<usize>()
                                    + bytes.len()
                                    > 32 * 1024 * 1024
                            {
                                registry.previews.pop_first();
                            }
                            registry
                                .previews
                                .insert(key, (metadata.clone(), Arc::new(bytes.clone())));
                        }
                        match request {
                            Request::AssetChunk { digest, offset, .. } => {
                                asset_chunk(&metadata, &bytes, digest, *offset)
                            }
                            Request::AssetOpen { .. } => Ok(reply(Response::AssetDone {})),
                            _ => {
                                let content_type = metadata
                                    .image
                                    .then(|| {
                                        crate::data::assets::image_content_type(&metadata.name)
                                    })
                                    .flatten();
                                Ok(reply(Response::Asset {
                                    metadata,
                                    content_type,
                                }))
                            }
                        }
                    }
                }
            }
            Request::UrlOpen { url } => {
                crate::data::media::media_url(url).map_err(|_| Code::InvalidInput)?;
                match super::asset_open::open_url(url) {
                    Ok(()) => Ok(reply(Response::AssetDone {})),
                    Err(e) => Ok(asset_error(e, None, None)),
                }
            }
            Request::VersionsList { kind, artifact } => {
                let id = format_id(kind, artifact)?;
                let result = ctx
                    .read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        crate::data::repository::versions::list(&repository, id)
                    })
                    .map_err(|_| Code::RuntimeRejected)?;
                match result {
                    Ok(versions) => Ok(reply(Response::Versions { versions })),
                    Err(error) => Ok(Completed::reject(error, Code::RepositoryRejected.into())),
                }
            }
            Request::VersionPreview {
                kind,
                artifact,
                version,
            } => {
                let id = format_id(kind, artifact)?;
                let n = version.parse::<u64>().map_err(|_| Code::InvalidInput)?;
                if n == 0 || n.to_string() != *version {
                    return Err(Code::InvalidInput.into());
                }
                let result = ctx
                    .read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        let snapshot =
                            crate::data::repository::versions::snapshot(&repository, id, n)?;
                        let source =
                            crate::data::repository::versions::observation(&repository, id)?;
                        let names =
                            crate::data::repository::versions::asset_names(&repository, id, n)?;
                        Ok::<_, std::io::Error>((snapshot, source, names))
                    })
                    .map_err(|_| Code::RuntimeRejected)?;
                match result {
                    Ok(((bytes, metadata), source, asset_names)) => {
                        let (template, document) = match id {
                            ArtifactSourceId::Template(_) => {
                                let template = artifact::decode_template(&bytes)
                                    .map_err(|_| Code::RepositoryRejected)?;
                                (Some(projection::template(&template)?), None)
                            }
                            ArtifactSourceId::Document(_) => {
                                let document = artifact::decode_document(&bytes)
                                    .map_err(|_| Code::RepositoryRejected)?;
                                let template = artifact::decode_template(
                                    &metadata.ok_or(Code::RepositoryRejected)?,
                                )
                                .map_err(|_| Code::RepositoryRejected)?;
                                (
                                    None,
                                    Some(Box::new(
                                        super::document_edit::project_artifacts(
                                            &document, &template,
                                        )?
                                        .0,
                                    )),
                                )
                            }
                            _ => return Err(Code::InvalidInput.into()),
                        };
                        Ok(reply(Response::VersionPreview {
                            version: version.clone(),
                            source,
                            asset_names,
                            template,
                            document,
                        }))
                    }
                    Err(error) => Ok(Completed::reject(error, Code::RepositoryRejected.into())),
                }
            }
            Request::VersionRestore {
                kind,
                artifact,
                version,
                source,
            } => {
                if template_owner || registry.len() > 0 {
                    return Err(Code::OwnersRemain.into());
                }
                let id = format_id(kind, artifact)?;
                let n = version.parse::<u64>().map_err(|_| Code::InvalidInput)?;
                if n == 0 || n.to_string() != *version {
                    return Err(Code::InvalidInput.into());
                }
                let timestamp = timestamp()?;
                let (binding, registration) =
                    begin(ctx, job, vec![id.path().map_err(|_| Code::InvalidInput)?])?;
                let Some(key) = &binding.key else {
                    let mut completed =
                        Completed::reject(registration, Code::SessionRejected.into());
                    completed.binding = Some(binding);
                    return Ok(completed);
                };
                let execution = match ctx.session(key) {
                    Ok(mut session) => session.restore_version(id, n, source, &timestamp),
                    Err(error) => {
                        let (released, cleanup) = finish_session(ctx, &binding);
                        let mut completed = Completed::reject(
                            (registration, error, cleanup),
                            Code::SessionRejected.into(),
                        );
                        if !released {
                            completed.binding = Some(binding);
                        }
                        return Ok(completed);
                    }
                };
                let mut dto = execution
                    .as_ref()
                    .map(|result| {
                        write(
                            binding.id,
                            Some(artifact.clone()),
                            result.diagnostic(),
                            Some(true),
                            vec![],
                        )
                    })
                    .map_err(|_| Code::SaveRejected.into());
                let confirmed = execution.as_ref().is_ok_and(|result| {
                    matches!(
                        result.diagnostic().disk,
                        crate::data::application::diagnostics::DiskState::Committed
                            | crate::data::application::diagnostics::DiskState::NoWrite
                    )
                });
                // No fallible return after registration: cleanup and its binding
                // must survive a post-write read or version confirmation failure.
                let observation = confirmed.then(|| {
                    ctx.read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        crate::data::repository::versions::observation(&repository, id)
                    })
                });
                let (released, cleanup) = finish_session(ctx, &binding);
                let confirmation = if released {
                    match &observation {
                        Some(Ok(Ok(expected))) => Some(ctx.read(|ready| {
                            let repository =
                                ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                            crate::data::repository::versions::confirm_source(
                                &repository,
                                id,
                                &timestamp,
                                Some(expected),
                            )
                        })),
                        _ => None,
                    }
                } else {
                    None
                };
                if observation
                    .as_ref()
                    .is_some_and(|result| !matches!(result, Ok(Ok(_))))
                    || confirmation
                        .as_ref()
                        .is_some_and(|result| !matches!(result, Ok(Ok(_))))
                {
                    dto = Err(Code::RepositoryRejected.into());
                }
                let mut completed = Completed::new(
                    (registration, execution, observation, cleanup, confirmation),
                    dto,
                );
                if !released {
                    completed.binding = Some(binding);
                }
                Ok(completed)
            }
            Request::FormatInspect { kind, artifact } => {
                let id = format_id(kind, artifact)?;
                let result = ctx
                    .read(|ready| {
                        ArtifactRepository::new(ready)
                            .map(|repo| crate::data::repository::format::inspect(&repo, id))
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RepositoryRejected)?;
                match result {
                    Ok((schema, source, history)) => Ok(reply(Response::Format {
                        schema,
                        source,
                        history,
                    })),
                    Err(error) => Ok(Completed::reject(error, Code::RepositoryRejected.into())),
                }
            }
            Request::FormatChange {
                kind,
                artifact,
                source,
                restore,
            } => {
                // 별도 session을 만들기 전에 열린 초안의 source를 보존한다.
                // 같은 프로세스의 협력 lock만으로는 다른 owner의 변경을 차단할 수 없다.
                if template_owner || registry.len() > 0 {
                    return Err(Code::OwnersRemain.into());
                }
                let id = format_id(kind, artifact)?;
                let (binding, registration) =
                    begin(ctx, job, vec![id.path().map_err(|_| Code::InvalidInput)?])?;
                let Some(key) = &binding.key else {
                    let mut c = Completed::reject(registration, Code::SessionRejected.into());
                    c.binding = Some(binding);
                    return Ok(c);
                };
                let execution = ctx
                    .session(key)
                    .map_err(|_| Code::SessionRejected)?
                    .change_format(id, source, restore.as_deref());
                let dto = execution
                    .as_ref()
                    .map(|e| {
                        write(
                            binding.id,
                            Some(artifact.clone()),
                            e.diagnostic(),
                            Some(true),
                            vec![],
                        )
                    })
                    .map_err(|_| Code::SaveRejected.into());
                let (released, cleanup) = finish_session(ctx, &binding);
                let mut c = Completed::new((registration, execution, cleanup), dto);
                if !released {
                    c.binding = Some(binding);
                }
                Ok(c)
            }
            Request::EditBegin { .. }
            | Request::EditDraft { .. }
            | Request::EditDeposit { .. }
            | Request::EditRelease { .. }
            | Request::EditRefresh { .. }
            | Request::EditResume { .. }
            | Request::EditRetry { .. }
            | Request::EditRestore { .. } => {
                if let Request::EditRestore { key, .. } = request {
                    if registry.retains(key) {
                        return Err(Code::DuplicateConflict.into());
                    }
                }
                if let Request::EditBegin { document } = request {
                    if registry
                        .drafts
                        .values()
                        .any(|d| d.project == project && d.document.to_string() == *document)
                    {
                        return Err(Code::DuplicateConflict.into());
                    }
                }
                let result = super::document_edit::execute(
                    ctx,
                    job,
                    project,
                    request,
                    &mut registry.editors,
                );
                if result.is_ok() {
                    if let Some(change) = super::document_edit::committed_change(
                        &registry.editors,
                        project,
                        request,
                        job.allocated,
                    ) {
                        apply_committed_edit(
                            ctx,
                            registry,
                            project,
                            &change,
                            job.allocated,
                            &mut observations,
                        );
                    }
                }
                workspace.sync_owners();
                result
            }
            Request::Search {
                query,
                template,
                offset,
                limit,
                refresh,
            } => {
                if query.len() > 512 || *limit == 0 || *limit > 200 || *offset > 10_000_000 {
                    return Err(Code::InvalidInput.into());
                }
                if let Some(template) = template {
                    let parsed: artifact::TemplateId =
                        template.parse().map_err(|_| Code::InvalidInput)?;
                    if parsed.to_string() != *template {
                        return Err(Code::InvalidInput.into());
                    }
                }
                if *refresh {
                    registry.searches.remove(&project);
                }
                if !registry.searches.contains_key(&project) {
                    let cache =
                        super::document_search::build(ctx, job.allocated, &mut observations)?;
                    registry.searches.insert(project, cache);
                }
                let cache = registry.searches.get(&project).ok_or(Code::Unavailable)?;
                Ok(reply(super::document_search::response(
                    cache,
                    query,
                    template.as_deref(),
                    *offset,
                    *limit,
                )))
            }
            Request::ReplacePreview {
                find,
                replacement,
                template,
                scopes,
                case_sensitive,
                whole_word,
                offset,
                limit,
            } => {
                // 새 조건은 이전 capability를 먼저 폐기한다. 이후 검증·scan이 실패하거나
                // 취소돼도 오래된 미리보기가 다시 적용 가능한 상태로 남지 않는다.
                registry.replacements.remove(&project);
                if *limit == 0 || *limit > 200 || *offset > 10_000_000 {
                    return Err(Code::InvalidInput.into());
                }
                let mut preview = super::document_replace::build(
                    ctx,
                    project,
                    job.allocated,
                    *scopes,
                    find,
                    replacement,
                    template.as_deref(),
                    *case_sensitive,
                    *whole_word,
                    &mut observations,
                )?;
                if !preview.targets.is_empty() && template_owner {
                    preview
                        .blockers
                        .push(crate::commands::document_workspace::ReplaceBlocker {
                            document: None,
                            document_name: None,
                            label: "Template 편집".into(),
                            reason:
                                "열린 Template 편집을 먼저 저장하거나 종료한 뒤 다시 찾아 주세요."
                                    .into(),
                        });
                }
                for document in preview.targets.iter().copied() {
                    if registry.editors.owns_document(project, document) {
                        preview.blockers.push(
                            crate::commands::document_workspace::ReplaceBlocker {
                                document: Some(document.to_string()),
                                document_name: None,
                                label: "열린 문서 편집".into(),
                                reason:
                                    "이 문서의 편집을 먼저 저장하거나 종료한 뒤 다시 찾아 주세요."
                                        .into(),
                            },
                        );
                    }
                }
                if !preview.blockers.is_empty() {
                    preview.input = None;
                }
                let response = super::document_replace::response(&preview, *offset, *limit);
                registry.replacements.insert(project, preview);
                Ok(reply(response))
            }
            Request::ReplacePage {
                preview,
                offset,
                limit,
            } => {
                if *limit == 0 || *limit > 200 || *offset > 10_000_000 {
                    return Err(Code::InvalidInput.into());
                }
                let preview = registry
                    .replacements
                    .get(&project)
                    .filter(|candidate| candidate.id == *preview)
                    .ok_or(Code::WrongBinding)?;
                Ok(reply(super::document_replace::response(
                    preview, *offset, *limit,
                )))
            }
            Request::ReplaceDiscard { preview } => {
                if let Some(expected) = preview {
                    let current = registry
                        .replacements
                        .get(&project)
                        .ok_or(Code::WrongBinding)?;
                    if current.id != *expected {
                        return Err(Code::WrongBinding.into());
                    }
                }
                registry.replacements.remove(&project);
                Ok(reply(Response::Released {}))
            }
            Request::ReplaceApply { preview } => {
                if registry
                    .replacements
                    .get(&project)
                    .is_none_or(|candidate| candidate.id != *preview)
                {
                    return Err(Code::WrongBinding.into());
                }
                let preview = registry
                    .replacements
                    .remove(&project)
                    .ok_or(Code::WrongBinding)?;
                if template_owner
                    || preview
                        .targets
                        .iter()
                        .any(|document| registry.editors.owns_document(project, *document))
                    || !preview.blockers.is_empty()
                {
                    return Err(Code::OwnersRemain.into());
                }
                let input = preview.input.as_ref().ok_or(Code::InvalidInput)?;
                let request =
                    crate::data::application::bulk_replace::BulkReplaceRequest::new(input)
                        .map_err(|_| Code::PreparationRejected)?;
                let (binding, registration) = begin(ctx, job, request.session_targets().to_vec())?;
                let Some(key) = &binding.key else {
                    let mut completed =
                        Completed::reject((preview, registration), Code::SessionRejected.into());
                    completed.binding = Some(binding);
                    return Ok(completed);
                };
                let execution = ctx
                    .session(key)
                    .map_err(|_| Code::SessionRejected)?
                    .apply_bulk_replace(input)
                    .map_err(|_| Code::SaveRejected)?;
                let diagnostic = execution.diagnostic();
                let disk = diagnostic.disk;
                let dto = write(binding.id, None, diagnostic, Some(true), vec![]);
                if matches!(disk, DiskState::Committed | DiskState::Uncertain) {
                    registry.searches.remove(&project);
                    registry
                        .snapshots
                        .retain(|_, snapshot| snapshot.project != project);
                }
                let (released, cleanup) = finish_session(ctx, &binding);
                let mut completed =
                    Completed::new((preview, registration, execution, cleanup), Ok(dto));
                if !released {
                    completed.binding = Some(binding);
                }
                Ok(completed)
            }
            Request::References { document } => {
                let id: artifact::DocumentId = document.parse().map_err(|_| Code::InvalidInput)?;
                if id.to_string() != *document {
                    return Err(Code::InvalidInput.into());
                }
                if !registry.searches.contains_key(&project) {
                    let cache =
                        super::document_search::build(ctx, job.allocated, &mut observations)?;
                    registry.searches.insert(project, cache);
                }
                let cache = registry.searches.get(&project).ok_or(Code::Unavailable)?;
                Ok(reply(super::document_search::references_response(
                    cache, document,
                )))
            }
            Request::List { refresh_search } => {
                if refresh_search.unwrap_or(true) {
                    registry.searches.remove(&project);
                }
                let mut full = layout_snapshot(ctx, &mut observations);
                if let Ok((_, documents, ..)) = &full {
                    let known = documents
                        .iter()
                        .map(|document| document.id.as_str())
                        .collect::<BTreeSet<_>>();
                    let ids = ctx
                        .read(|ready| {
                            let repository =
                                ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                            crate::data::repository::versions::targets(&repository)
                        })
                        .map_err(|error| observed(&mut observations, error, Code::RuntimeRejected))?
                        .map_err(|error| {
                            observed(&mut observations, error, Code::RepositoryRejected)
                        })?;
                    if ids.iter().any(|id| matches!(id, ArtifactSourceId::Document(document) if !known.contains(document.to_string().as_str()))) {
                        full = Err(Code::RepositoryRejected.into());
                    }
                }
                let (base, documents, unplaced, problem, scan) = match full {
                    Ok(full) => full,
                    Err(error) => {
                        observations.push(Box::new(error));
                        let display = ctx
                            .read(|ready| {
                                let repository = ArtifactRepository::new(ready)
                                    .map_err(std::io::Error::other)?;
                                let documents = repository.display_documents()?;
                                let ids = documents
                                    .iter()
                                    .map(|document| document.id)
                                    .collect::<BTreeSet<_>>();
                                let layout = repository
                                    .load_layout()
                                    .ok()
                                    .flatten()
                                    .map(|loaded| loaded.into_artifact())
                                    .unwrap_or_else(|| DocumentLayout::flat(ids.iter().copied()));
                                let unplaced = layout
                                    .reconcile(&ids)
                                    .map_err(|error| std::io::Error::other(format!("{error:?}")))?
                                    .iter()
                                    .map(ToString::to_string)
                                    .collect();
                                let unverified_documents = documents
                                    .iter()
                                    .filter(|document| !document.available)
                                    .map(|document| document.id.to_string())
                                    .collect();
                                let summaries = documents
                                    .iter()
                                    .map(|document| Summary {
                                        id: document.id.to_string(),
                                        template: document.template.clone(),
                                        name: document.name.clone(),
                                        english_name: document.english_name.clone(),
                                        glossary_summary: document.glossary_summary.clone(),
                                        glossary_excluded: document.glossary_excluded,
                                    })
                                    .collect();
                                Ok::<_, std::io::Error>((
                                    layout,
                                    unplaced,
                                    unverified_documents,
                                    summaries,
                                ))
                            })
                            .map_err(|error| {
                                observed(&mut observations, error, Code::RuntimeRejected)
                            })?
                            .map_err(|error| {
                                observed(&mut observations, error, Code::RepositoryRejected)
                            })?;
                        return Ok(reply(Response::List {
                            fingerprint: ctx.project_fingerprint().ok_or(Code::Unavailable)?.into(),
                            snapshot: job.allocated,
                            initial: false,
                            layout: display.0,
                            unplaced: display.1,
                            documents: display.3,
                            issues: Vec::new(),
                            issue_status: "partial".into(),
                            unverified_documents: display.2,
                            problem: Some("SourceUnavailable".into()),
                        }));
                    }
                };
                registry
                    .snapshots
                    .retain(|_, snapshot| snapshot.project != project);
                if registry.snapshots.len() >= 16 {
                    return Err(Code::Full.into());
                }
                if problem.is_none() {
                    registry.snapshots.insert(
                        job.allocated,
                        CachedLayoutSnapshot {
                            project,
                            layout: base.clone(),
                            summaries: documents.clone(),
                            documents: Arc::clone(&scan),
                        },
                    );
                }
                let (issues, issue_status, unverified_documents) = if problem.is_none() {
                    issue_snapshot(ctx, &base, &scan)
                } else {
                    (
                        Vec::new(),
                        "unavailable".into(),
                        documents.iter().map(|item| item.id.clone()).collect(),
                    )
                };
                Ok(reply(Response::List {
                    fingerprint: ctx.project_fingerprint().ok_or(Code::Unavailable)?.into(),
                    snapshot: job.allocated,
                    initial: base.source.is_none(),
                    layout: base.layout,
                    unplaced,
                    documents,
                    issues,
                    issue_status,
                    unverified_documents,
                    problem,
                }))
            }
            Request::Read { document } => {
                let id = convert::id(document)?;
                let read = ctx
                    .read(|ready| {
                        let r = ArtifactRepository::new(ready)?;
                        let d = r.load_document(id)?;
                        let t = r.load_template(d.artifact().template_id())?;
                        Ok::<_, crate::data::repository::RepositoryError>((d, t))
                    })
                    .map_err(|e| observed(&mut observations, e, Code::RuntimeRejected))?
                    .map_err(|e| observed(&mut observations, e, Code::RepositoryRejected))?;
                let (d, t) = read;
                Ok(reply(read_response(
                    d.artifact(),
                    t.artifact(),
                    &mut observations,
                )?))
            }
            Request::PdfInspect { document } => {
                let id = convert::id(document)?;
                let snapshot = ctx
                    .read(|ready| pdf_snapshot(ready, id))
                    .map_err(|error| observed(&mut observations, error, Code::RuntimeRejected))??;
                Ok(reply(Response::PdfInspect {
                    document: snapshot.document,
                    name: snapshot.name,
                    source: snapshot.source,
                    missing_images: snapshot.missing_images,
                }))
            }
            Request::PdfExport {
                document,
                source,
                destination,
                allow_missing_images,
            } => {
                let id = convert::id(document)?;
                let (snapshot, root) = ctx
                    .read(|ready| {
                        let snapshot = pdf_snapshot(ready, id)?;
                        Ok::<_, crate::commands::dto::ErrorDto>((
                            snapshot,
                            ready.locked_project().canonical_root().to_path_buf(),
                        ))
                    })
                    .map_err(|error| observed(&mut observations, error, Code::RuntimeRejected))??;
                if &snapshot.source != source
                    || (!snapshot.missing_images.is_empty() && !allow_missing_images)
                {
                    return Err(Code::WrongBinding.into());
                }
                let result = crate::pdf_export::export(
                    &snapshot,
                    std::path::Path::new(destination),
                    &root,
                    || job.progress.snapshot().0,
                )
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        Code::Cancelled.into()
                    } else if error.kind() == std::io::ErrorKind::AlreadyExists {
                        observed(&mut observations, error, Code::DuplicateConflict)
                    } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                        observed(&mut observations, error, Code::Forbidden)
                    } else {
                        observed(&mut observations, error, Code::PdfExportRejected)
                    }
                })?;
                Ok(reply(Response::PdfExport {
                    destination: destination.clone(),
                    size: result.size,
                    sha256: result.sha256,
                    cleanup_warning: result.cleanup_warning,
                }))
            }
            Request::SearchRead { document } => {
                let id = convert::id(document)?;
                let read = ctx
                    .read(|ready| {
                        let repository = ArtifactRepository::new(ready)?;
                        let before = repository.load_layout()?;
                        if before.as_ref().is_some_and(|layout| {
                            layout
                                .artifact()
                                .nodes
                                .get(&id)
                                .is_some_and(|node| node.state == LayoutState::Trashed)
                        }) {
                            return Ok(None);
                        }
                        let document = match repository.load_document(id) {
                            Ok(document) => document,
                            Err(error)
                                if error.diagnostic().category
                                    == crate::data::repository::RepositoryCategory::NotFound =>
                            {
                                return Ok(None);
                            }
                            Err(error) => return Err(error),
                        };
                        let template =
                            repository.load_template(document.artifact().template_id())?;
                        let after = repository.load_layout()?;
                        let same_layout = before.as_ref().map(|layout| layout.source())
                            == after.as_ref().map(|layout| layout.source());
                        let active = !after.as_ref().is_some_and(|layout| {
                            layout
                                .artifact()
                                .nodes
                                .get(&id)
                                .is_some_and(|node| node.state == LayoutState::Trashed)
                        });
                        Ok::<_, crate::data::repository::RepositoryError>(
                            (same_layout && active).then_some((document, template)),
                        )
                    })
                    .map_err(|error| observed(&mut observations, error, Code::RuntimeRejected))?
                    .map_err(|error| {
                        observed(&mut observations, error, Code::RepositoryRejected)
                    })?;
                let Some((document, template)) = read else {
                    // 이미 자기 휴지통 전이로 제외한 항목은 cache를 다시 만들 이유가 없다.
                    // 실제로 남은 stale Entry가 있을 때만 전체 투영을 폐기한다.
                    if registry
                        .searches
                        .get(&project)
                        .is_some_and(|cache| cache.contains_active_document(id))
                    {
                        registry.searches.remove(&project);
                    }
                    return Ok(reply(Response::SearchUnavailable {
                        document: id.to_string(),
                    }));
                };
                Ok(reply(read_response(
                    document.artifact(),
                    template.artifact(),
                    &mut observations,
                )?))
            }
            Request::Mutate { snapshot, edit } => {
                // 어느 탭에서 시작했든 진행 생성 owner와 겹치는 구조 작업을 함께 차단한다.
                if registry.drafts.values().any(|d| d.project == project)
                    || registry.editors.len() > 0
                {
                    return Err(Code::OwnersRemain.into());
                }
                let cached = registry.snapshots.get(snapshot).ok_or(Code::UnknownId)?;
                if cached.project != project {
                    return Err(Code::WrongBinding.into());
                }
                let input = LayoutWrite {
                    base: cached.layout.clone(),
                    documents: Arc::clone(&cached.documents),
                    edit: edit.clone(),
                    timestamp: timestamp()?,
                    create: None,
                };
                let document_purge = if let LayoutEdit::Purge { document } = edit {
                    Some(
                        ctx.read(|ready| {
                            crate::data::asset_maintenance::begin_document_purge(
                                ready.locked_project().canonical_root(),
                                &document.to_string(),
                            )
                        })
                        .map_err(|_| Code::RuntimeRejected)?
                        .map_err(|_| Code::AssetMaintenanceRejected)?,
                    )
                } else {
                    None
                };
                let write_result = write_layout(ctx, job, &input, None);
                let (mut c, binding, evidence, committed) = match write_result {
                    Ok(result) => result,
                    Err(error) => {
                        if let Some(purge) = document_purge {
                            let _ = ctx.read(|ready| {
                                crate::data::asset_maintenance::finish_document_purge(
                                    ready.locked_project().canonical_root(),
                                    purge,
                                    false,
                                )
                            });
                        }
                        return Err(error);
                    }
                };
                registry.snapshots.remove(snapshot);
                let committed_disk = matches!(
                    &c.dto,
                    Ok(ResultDto::Write {
                        disk: DiskDto::Committed,
                        ..
                    })
                );
                if let Some(purge) = document_purge {
                    let cleanup_required = ctx
                        .read(|ready| {
                            crate::data::asset_maintenance::finish_document_purge(
                                ready.locked_project().canonical_root(),
                                purge,
                                committed_disk,
                            )
                        })
                        .map_err(|_| Code::RuntimeRejected)?
                        .unwrap_or(committed_disk);
                    if cleanup_required {
                        if let Ok(ResultDto::Write { cleanup_failed, .. }) = &mut c.dto {
                            *cleanup_failed = true;
                        }
                    }
                }
                if let Some((_, snapshot, documents)) = &committed {
                    let updated = registry.searches.get_mut(&project).is_none_or(|cache| {
                        super::document_search::apply_layout_commit(
                            ctx,
                            cache,
                            &snapshot.layout,
                            documents,
                            job.allocated,
                            &mut observations,
                        )
                        .is_ok()
                    });
                    if !updated {
                        registry.searches.remove(&project);
                    }
                } else if committed_disk {
                    // commit 뒤 정확한 layout/scan 재독이 없으면 옛 경로를 게시하지 않는다.
                    registry.searches.remove(&project);
                }
                let _ = (binding, evidence);
                Ok(c)
            }
            Request::Begin { template, snapshot } => {
                if registry.drafts.len() >= 16 {
                    return Err(Code::Full.into());
                }
                let template = load_template(ctx, template, &mut observations)?;
                let (base, base_documents, documents) = if let Some(snapshot) = snapshot {
                    let cached = registry.snapshots.get(snapshot).ok_or(Code::UnknownId)?;
                    if cached.project != project {
                        return Err(Code::WrongBinding.into());
                    }
                    (
                        cached.layout.clone(),
                        cached.summaries.clone(),
                        Arc::clone(&cached.documents),
                    )
                } else {
                    let (base, summaries, _, problem, documents) =
                        layout_snapshot(ctx, &mut observations)?;
                    if problem.is_some() {
                        return Err(Code::RepositoryRejected.into());
                    }
                    (base, summaries, documents)
                };
                let creation = Creation {
                    project,
                    fingerprint: ctx.project_fingerprint().ok_or(Code::Unavailable)?.into(),
                    draft_id: job.allocated.into(),
                    generation: 1,
                    document: artifact::DocumentId::new(),
                    template,
                    base,
                    documents,
                    base_documents,
                    body: CreationBody {
                        name: String::new(),
                        english_name: String::new(),
                        glossary_summary: String::new(),
                        glossary_excluded: false,
                        parent: None,
                        fields: vec![],
                        composing: false,
                    },
                    deposit: None,
                    restore_assets: None,
                    proof: None,
                    outcome: None,
                    original: None,
                    binding: None,
                    problem: None,
                    field: None,
                    committed: false,
                    saved_generation: None,
                    created_snapshot: None,
                    attempt: None,
                    uncertain: false,
                    commit: None,
                };
                let result = draft_reply(job.allocated, &creation)?;
                registry.drafts.insert(job.allocated, creation);
                workspace.sync_owners();
                Ok(result)
            }
            Request::Draft {
                owner,
                generation,
                body,
                ..
            }
            | Request::Deposit {
                owner,
                generation,
                body,
            } => {
                let mut committed_snapshot = None;
                let mut invalidate_committed_search = false;
                let deposit = matches!(request, Request::Deposit { .. });
                let save = matches!(request, Request::Draft { save: true, .. });
                let d = registry.drafts.get_mut(owner).ok_or(Code::UnknownId)?;
                if d.project != project {
                    return Err(Code::WrongBinding.into());
                }
                update(d, generation, body)?;
                if deposit {
                    if d.deposit.is_none() {
                        d.deposit = Some(
                            Deposit::freeze(Envelope {
                                residual: None,
                                residual_ack: None,
                                key: Key {
                                    project_fingerprint: d.fingerprint.clone(),
                                    draft_id: d.draft_id.clone(),
                                    generation: d.generation,
                                },
                                deposit_id: Id::new().into(),
                                app_version: env!("CARGO_PKG_VERSION").into(),
                                created_at_utc: timestamp()?,
                                originals: std::iter::once(
                                    recovery::original(&d.template).map_err(|e| {
                                        observed(&mut observations, e, Code::RecoveryRejected)
                                    })?,
                                )
                                .chain(d.created_snapshot.iter().cloned())
                                .collect(),
                                draft: Draft::Document {
                                    document: if d.committed || d.uncertain {
                                        Some(d.document.to_string())
                                    } else {
                                        None
                                    },
                                    template: d.template.template()?.id.to_string(),
                                    name: Intent::Set(d.body.name.clone()),
                                    english_name: Intent::Set(d.body.english_name.clone()),
                                    glossary_summary: Intent::Set(d.body.glossary_summary.clone()),
                                    glossary_excluded: Intent::Set(d.body.glossary_excluded),
                                    fields: d.body.fields.clone(),
                                    composing: d.body.composing,
                                },
                                attempt: d.attempt.clone(),
                            })
                            .map_err(|e| observed(&mut observations, e, Code::RecoveryRejected))?,
                        );
                    }
                    let a = d.deposit.as_ref().ok_or(Code::NoDraft)?;
                    recovery::capture_assets(ctx, job, a, &mut observations)?;
                    let store = job
                        .recovery
                        .connect()
                        .map_err(|e| observed(&mut observations, e, Code::SinkUnavailable))?;
                    let proof = store
                        .lock()
                        .map_err(|_| Code::Unavailable)?
                        .accept(a)
                        .map_err(|e| observed(&mut observations, e, Code::RecoveryRejected))?;
                    if !proof.matches(
                        &a.envelope().key,
                        &a.envelope().deposit_id,
                        a.payload_digest(),
                    ) {
                        return Err(Code::NoReceipt.into());
                    }
                    d.proof = Some(proof);
                } else if save {
                    if d.committed || d.uncertain {
                        return Err(Code::RecoveryRejected.into());
                    }
                    if d.binding.is_some() {
                        return Err(Code::OwnersRemain.into());
                    }
                    if d.body.composing {
                        d.problem = Some("Composing".into());
                        return draft_reply(*owner, d);
                    }
                    if d.body.name.trim().is_empty() {
                        d.problem = Some("NameRequired".into());
                        return draft_reply(*owner, d);
                    }
                    for (field, value) in [
                        ("englishName", &d.body.english_name),
                        ("glossarySummary", &d.body.glossary_summary),
                    ] {
                        if validate_optional_single_line_text(value).is_err() {
                            d.problem = Some("SingleLineRequired".into());
                            d.field = Some(field.into());
                            return draft_reply(*owner, d);
                        }
                    }
                    let t = match &*d.template {
                        View::Template(t) => t.artifact(),
                        _ => return Err(Code::WrongBinding.into()),
                    };
                    if let Some((problem, field)) = validate_reference_inputs(
                        ctx,
                        t,
                        d.document,
                        &d.body.fields,
                        &mut observations,
                    )? {
                        d.problem = Some(problem);
                        d.field = Some(field);
                        return draft_reply(*owner, d);
                    }
                    let mut initial = BTreeMap::new();
                    for f in &d.body.fields {
                        let id = convert::id(&f.field)?;
                        let value = match &f.value {
                            Intent::Keep => continue,
                            Intent::Unset => {
                                artifact::DocumentValueEdit::unset().into_field_value()
                            }
                            Intent::Set(ValueDto::Group { instances }) => {
                                let definition = t.fields().get(&id).ok_or(Code::InvalidInput)?;
                                let inputs = convert::group_inputs_with_policy(
                                    instances,
                                    d.restore_assets.is_some(),
                                )?;
                                match artifact::group::assemble(
                                    definition,
                                    t.revision(),
                                    None,
                                    &inputs,
                                ) {
                                    Ok(value) => value,
                                    Err(error) => {
                                        d.problem = Some(error.category.into());
                                        d.field = Some(group_error_address(&f.field, &error));
                                        return draft_reply(*owner, d);
                                    }
                                }
                            }
                            Intent::Set(v) => match if d.restore_assets.is_some() {
                                v.preserved_creation_value()
                            } else {
                                v.creation_value()
                            } {
                                Ok(v) => v,
                                Err(_) => {
                                    d.problem = Some("InvalidValue".into());
                                    d.field = Some(f.field.clone());
                                    return draft_reply(*owner, d);
                                }
                            },
                        };
                        if initial.insert(id, value).is_some() {
                            return Err(Code::InvalidInput.into());
                        }
                    }
                    let template = Arc::clone(&d.template);
                    let source = template.template()?;
                    let t = match &*template {
                        View::Template(t) => t.artifact(),
                        _ => return Err(Code::WrongBinding.into()),
                    };
                    let now = timestamp()?;
                    let candidate = match artifact::create_document_with_term_info(
                        t,
                        source.expected_revision,
                        d.document,
                        d.body.name.clone(),
                        d.body.english_name.clone(),
                        d.body.glossary_summary.clone(),
                        d.body.glossary_excluded,
                        now.clone(),
                        &initial,
                    ) {
                        Ok(v) => v.into_document(),
                        Err(e) => {
                            d.problem = Some(format!("{:?}", e.category()));
                            d.field = e.field_id().map(|id| id.to_string());
                            return draft_reply(*owner, d);
                        }
                    };
                    let raw = artifact::encode_document(&candidate)
                        .map_err(|e| observed(&mut observations, e, Code::SerializationFailed))?;
                    if d.restore_assets.is_none() {
                        ctx.read(|ready| {
                            crate::data::assets::validate_document(
                                ready.locked_project().canonical_root(),
                                &raw,
                            )
                        })
                        .map_err(|e| observed(&mut observations, e, Code::RuntimeRejected))?
                        .map_err(|e| observed(&mut observations, e, Code::PreparationRejected))?;
                    }
                    let input = LayoutWrite {
                        base: d.base.clone(),
                        documents: Arc::clone(&d.documents),
                        edit: LayoutEdit::Adopt {},
                        timestamp: now,
                        create: Some((
                            candidate,
                            source,
                            d.body.parent.as_ref().map(|v| convert::id(v)).transpose()?,
                        )),
                    };
                    let chosen = Draft::Document {
                        document: None,
                        template: t.template_id().to_string(),
                        name: Intent::Set(d.body.name.clone()),
                        english_name: Intent::Set(d.body.english_name.clone()),
                        glossary_summary: Intent::Set(d.body.glossary_summary.clone()),
                        glossary_excluded: Intent::Set(d.body.glossary_excluded),
                        fields: d.body.fields.clone(),
                        composing: false,
                    };
                    let (completed, binding, evidence, committed) = write_layout(
                        ctx,
                        job,
                        &input,
                        d.restore_assets.as_ref().map(|deposit| (deposit, &chosen)),
                    )?;
                    d.binding = binding;
                    if let Some((snapshot, mut attempt)) = evidence {
                        attempt.submitted_generation = d.generation;
                        d.created_snapshot = Some(snapshot);
                        d.attempt = Some(attempt);
                    }
                    match &completed.dto {
                        Ok(result) => {
                            if let ResultDto::Rejected { error, .. } = result {
                                d.problem = Some(format!("{:?}", error.code));
                            }
                            d.committed = matches!(
                                result,
                                ResultDto::Write {
                                    disk: DiskDto::Committed,
                                    ..
                                }
                            );
                            d.uncertain = matches!(
                                result,
                                ResultDto::Write {
                                    disk: DiskDto::Uncertain,
                                    ..
                                }
                            );
                            d.outcome = Some(Box::new(result.clone()));
                        }
                        Err(error) => {
                            // Keep the draft and its original execution for recovery, but
                            // never return a silent success when the nested write failed.
                            d.problem = Some(format!("{:?}", error.code));
                            d.outcome = None;
                        }
                    }
                    if d.committed {
                        d.saved_generation = Some(d.generation);
                        if let Some((commit, snapshot, documents_snapshot)) = committed {
                            let (candidate, _, _) =
                                input.create.as_ref().ok_or(Code::Unavailable)?;
                            let read = read_response(candidate, t, &mut observations)?;
                            let documents: Vec<_> = commit
                                .documents
                                .into_iter()
                                .map(|summary| Summary {
                                    id: summary.id.to_string(),
                                    template: summary.template.to_string(),
                                    name: summary.name,
                                    english_name: summary.english_name,
                                    glossary_summary: summary.glossary_summary,
                                    glossary_excluded: summary.glossary_excluded,
                                })
                                .collect();
                            let base: BTreeMap<_, _> = d
                                .base_documents
                                .iter()
                                .map(|summary| (summary.id.as_str(), summary))
                                .collect();
                            let current: BTreeSet<_> = documents
                                .iter()
                                .map(|summary| summary.id.as_str())
                                .collect();
                            let changed_documents = documents
                                .iter()
                                .filter(|summary| {
                                    base.get(summary.id.as_str())
                                        .is_none_or(|previous| *previous != *summary)
                                })
                                .cloned()
                                .collect();
                            let removed_documents = d
                                .base_documents
                                .iter()
                                .filter(|summary| !current.contains(summary.id.as_str()))
                                .map(|summary| summary.id.clone())
                                .collect();
                            d.commit = Some(Box::new(CreationCommit {
                                fingerprint: d.fingerprint.clone(),
                                snapshot: job.allocated,
                                layout_revision: commit.layout.revision,
                                parent: d.body.parent.clone(),
                                changed_documents,
                                removed_documents,
                                unplaced: commit
                                    .unplaced
                                    .into_iter()
                                    .map(|id| id.to_string())
                                    .collect(),
                                document_count: documents.len(),
                                read: Box::new(read),
                            }));
                            committed_snapshot =
                                Some((snapshot, documents.clone(), documents_snapshot));
                        } else {
                            invalidate_committed_search = true;
                        }
                    }
                    d.original = Some(Box::new(completed));
                }
                let response = draft_reply(*owner, d)?;
                if invalidate_committed_search {
                    registry.searches.remove(&project);
                }
                if let Some((snapshot, summaries, documents)) = committed_snapshot {
                    let updated = registry.searches.get_mut(&project).is_none_or(|cache| {
                        super::document_search::apply_layout_commit(
                            ctx,
                            cache,
                            &snapshot.layout,
                            &documents,
                            job.allocated,
                            &mut observations,
                        )
                        .is_ok()
                    });
                    if !updated {
                        registry.searches.remove(&project);
                    }
                    registry
                        .snapshots
                        .retain(|_, snapshot| snapshot.project != project);
                    if registry.snapshots.len() >= 16 {
                        registry.snapshots.pop_first();
                    }
                    registry.snapshots.insert(
                        job.allocated,
                        CachedLayoutSnapshot {
                            project,
                            layout: snapshot,
                            summaries,
                            documents,
                        },
                    );
                }
                Ok(response)
            }
            Request::Release {
                owner,
                generation: raw,
                discard,
            } => {
                let d = registry.drafts.get(owner).ok_or(Code::UnknownId)?;
                if d.project != project || generation(raw)? != d.generation {
                    return Err(Code::WrongBinding.into());
                }
                if !discard
                    && d.saved_generation != Some(d.generation)
                    && !d
                        .deposit
                        .as_ref()
                        .zip(d.proof.as_ref())
                        .is_some_and(|(a, p)| {
                            p.matches(
                                &a.envelope().key,
                                &a.envelope().deposit_id,
                                a.payload_digest(),
                            )
                        })
                {
                    return Err(Code::NoReceipt.into());
                }
                if let Some(b) = &d.binding {
                    let (released, original) = finish_session(ctx, b);
                    observations.push(original);
                    if !released {
                        return Err(Code::ReleaseRejected.into());
                    }
                }
                let original = registry.drafts.remove(owner);
                workspace.sync_owners();
                Ok(Completed::new(
                    original,
                    Ok(ResultDto::DocumentWorkspace {
                        value: Box::new(Response::Released {}),
                    }),
                ))
            }
            Request::Restore {
                reapply,
                key,
                deposit_id,
                digest,
            } => {
                let fingerprint = ctx.project_fingerprint().ok_or(Code::Unavailable)?;
                if key.project_fingerprint != fingerprint {
                    return Err(Code::WrongBinding.into());
                }
                if registry.drafts.len() >= 16 {
                    return Err(Code::Full.into());
                }
                if registry
                    .drafts
                    .values()
                    .any(|d| d.fingerprint == key.project_fingerprint && d.draft_id == key.draft_id)
                {
                    return Err(Code::DuplicateConflict.into());
                }
                let store = job
                    .recovery
                    .connect()
                    .map_err(|e| observed(&mut observations, e, Code::SinkUnavailable))?;
                let store = store.lock().map_err(|_| Code::Unavailable)?;
                let deposit = store
                    .read(key, deposit_id)
                    .map_err(|e| observed(&mut observations, e, Code::RecoveryRejected))?;
                if deposit.payload_digest() != digest {
                    return Err(Code::WrongBinding.into());
                }
                let generation = store
                    .latest_generation(key)
                    .map_err(|e| observed(&mut observations, e, Code::RecoveryRejected))?
                    .checked_add(1)
                    .ok_or(Code::Full)?;
                let envelope = deposit.envelope();
                let Draft::Document {
                    document: None,
                    template,
                    name: Intent::Set(_),
                    ..
                } = &envelope.draft
                else {
                    return Err(Code::WrongBinding.into());
                };
                let template = load_template(ctx, template, &mut observations)?;
                let token = &template.template()?.token;
                let old = envelope.originals.first().ok_or(Code::WrongBinding)?;
                if reapply.is_none()
                    && old.source_digest
                        != token
                            .sha256()
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                {
                    return Err(Code::WrongBinding.into());
                }
                if envelope.attempt.as_ref().is_some_and(|a| {
                    matches!(
                        a.result,
                        crate::data::edit_recovery::model::SaveState::Unknown
                            | crate::data::edit_recovery::model::SaveState::Uncertain
                            | crate::data::edit_recovery::model::SaveState::Committed
                    )
                }) {
                    return Err(Code::RecoveryRejected.into());
                }
                let mut recovered = super::recovery_document::body(&envelope.draft)?;
                if let Some(choices) = reapply {
                    let digest = token
                        .sha256()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    if choices.template_digest != digest || choices.document_digest.is_some() {
                        return Err(Code::WrongBinding.into());
                    }
                    let View::Template(current) = &*template else {
                        return Err(Code::WrongBinding.into());
                    };
                    let plan = super::recovery_document::plan(envelope, current.artifact(), None)?;
                    recovered = super::recovery_document::apply(&plan, &choices.selected)?;
                }
                let (base, base_documents, _, problem, documents) =
                    layout_snapshot(ctx, &mut observations)?;
                if problem.is_some() {
                    return Err(Code::RepositoryRejected.into());
                }
                let d = Creation {
                    project,
                    fingerprint: key.project_fingerprint.clone(),
                    draft_id: key.draft_id.clone(),
                    generation,
                    document: artifact::DocumentId::new(),
                    template,
                    base,
                    documents,
                    base_documents,
                    body: CreationBody {
                        name: match &recovered.name {
                            Intent::Set(name) => name.clone(),
                            _ => String::new(),
                        },
                        english_name: match &recovered.english_name {
                            Intent::Set(value) => value.clone(),
                            Intent::Keep | Intent::Unset => String::new(),
                        },
                        glossary_summary: match &recovered.glossary_summary {
                            Intent::Set(value) => value.clone(),
                            Intent::Keep | Intent::Unset => String::new(),
                        },
                        glossary_excluded: match &recovered.glossary_excluded {
                            Intent::Set(value) => *value,
                            Intent::Keep | Intent::Unset => false,
                        },
                        parent: None,
                        fields: recovered.fields.clone(),
                        // 보관본의 조합 표시는 당시 OS 입력 세션의 증거다. 복원은
                        // 새 세션이므로 이를 활성 조합 상태로 다시 설치하지 않는다.
                        composing: false,
                    },
                    deposit: None,
                    restore_assets: None,
                    proof: None,
                    outcome: None,
                    original: None,
                    binding: None,
                    problem: Some("RestoredChooseParent".into()),
                    field: None,
                    committed: false,
                    saved_generation: None,
                    created_snapshot: None,
                    attempt: None,
                    uncertain: false,
                    commit: None,
                };
                let mut d = d;
                d.restore_assets = Some(deposit);
                let result = draft_reply(job.allocated, &d)?;
                registry.drafts.insert(job.allocated, d);
                workspace.sync_owners();
                Ok(result)
            }
        }
    })();
    let mut completed = result.unwrap_or_else(|error| Completed::reject(job.input.clone(), error));
    if job.progress.snapshot().2 == 5 {
        completed.dto = Err(Code::Cancelled.into());
    }
    completed.original = Box::new((completed.original, observations));
    Ok(completed)
}

fn format_id(kind: &str, id: &str) -> Reply<ArtifactSourceId> {
    match kind {
        "template" => Ok(ArtifactSourceId::Template(convert::id(id)?)),
        "document" => Ok(ArtifactSourceId::Document(convert::id(id)?)),
        _ => Err(Code::InvalidInput.into()),
    }
}

fn asset_error(
    error: crate::data::edit_recovery::error::RecoveryError,
    asset_name: Option<String>,
    asset_state: Option<&'static str>,
) -> Completed {
    let dto = error.dto();
    Completed::new(
        error,
        Ok(ResultDto::DocumentWorkspace {
            value: Box::new(Response::AssetError {
                error: dto,
                asset_name,
                asset_state,
            }),
        }),
    )
}

fn asset_reference_context(
    root: &std::path::Path,
    asset: &str,
) -> (Option<String>, Option<&'static str>) {
    let Ok(inspection) = crate::data::asset_maintenance::inspect(root) else {
        return (None, Some("unavailable"));
    };
    if let Some(row) = inspection.trash.iter().find(|row| row.id == asset) {
        return (Some(row.name.clone()), Some("trashed"));
    }
    if let Some(row) = inspection.rows.iter().find(|row| row.id == asset) {
        let state = match row.status {
            crate::data::asset_maintenance::AssetStatus::InTrash => "trashed",
            crate::data::asset_maintenance::AssetStatus::Missing => "missing",
            crate::data::asset_maintenance::AssetStatus::Corrupt => "corrupt",
            crate::data::asset_maintenance::AssetStatus::Uncertain => "uncertain",
            crate::data::asset_maintenance::AssetStatus::Used
            | crate::data::asset_maintenance::AssetStatus::Unused => "unreadable",
        };
        return (row.name.clone(), Some(state));
    }
    (None, Some("missing"))
}
fn asset_chunk(
    metadata: &crate::data::assets::Metadata,
    bytes: &[u8],
    digest: &str,
    offset: u64,
) -> Reply<Completed> {
    if !metadata.image || digest != metadata.sha256 || offset > bytes.len() as u64 {
        return Err(Code::WrongBinding.into());
    }
    use base64::Engine;
    let start = offset as usize;
    let end = (start + 64 * 1024).min(bytes.len());
    Ok(reply(Response::AssetChunk {
        data: base64::engine::general_purpose::STANDARD.encode(&bytes[start..end]),
        done: end == bytes.len(),
    }))
}

pub(super) fn group_error_address(group: &str, error: &artifact::group::GroupError) -> String {
    // JSON tuple is an unambiguous display/focus key; authorization uses typed IDs independently.
    serde_json::json!([
        group,
        error.instance.map(|v| v.to_string()),
        error.field.map(|v| v.to_string())
    ])
    .to_string()
}
