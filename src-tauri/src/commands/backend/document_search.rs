//! 저장된 문서의 표시 가능한 값만 읽는 파생 검색 자료다. 쓰기·복구 권한과 분리한다.
use super::*;
use crate::commands::document_workspace::{
    IncomingReference, Response, SearchExcerpt, SearchResult, UnavailableIncomingReference,
};
use crate::data::{
    artifact::layout::{DocumentLayout, LayoutState},
    edit_recovery::model::Intent,
};
use std::collections::{BTreeMap, BTreeSet};

/// 사용자 원문 전체 대신 제목과 활성 필드의 표시 텍스트만 프로젝트 수명 동안 보관한다.
#[derive(Clone)]
pub(super) struct Cache {
    generation: Id,
    entries: Vec<Entry>,
    layout: DocumentLayout,
    names: BTreeMap<artifact::DocumentId, String>,
    templates: BTreeMap<artifact::TemplateId, crate::data::repository::SourceToken>,
    missing_documents: usize,
}
impl Cache {
    pub(super) fn contains_active_document(&self, id: artifact::DocumentId) -> bool {
        let id = id.to_string();
        self.entries
            .iter()
            .any(|entry| entry.active && entry.id == id)
    }
}
#[derive(Clone)]
struct Entry {
    id: String,
    template: String,
    template_name: String,
    name: String,
    path: Vec<String>,
    fields: Vec<SearchText>,
    references: Vec<ReferenceEdge>,
    active: bool,
}
#[derive(Clone)]
enum ReferenceKind {
    Relation,
    DocumentLink,
}
#[derive(Clone)]
struct ReferenceEdge {
    kind: ReferenceKind,
    target: String,
    field: String,
    field_label: String,
    instance: Option<String>,
    connection: String,
    relation_name: String,
    one_way: bool,
    reciprocal_notice: bool,
}
#[derive(Clone)]
struct SearchText {
    label: String,
    text: String,
}

fn append_rich_text(node: &crate::data::edit_input::RichNode, output: &mut String) {
    use crate::data::edit_input::RichNode;
    match node {
        RichNode::Text { text, .. } => output.push_str(text),
        RichNode::HardBreak {} => output.push('\n'),
        RichNode::Root { children }
        | RichNode::Paragraph { children }
        | RichNode::Heading { children, .. }
        | RichNode::Blockquote { children }
        | RichNode::BulletList { children }
        | RichNode::OrderedList { children }
        | RichNode::ListItem { children }
        | RichNode::TaskList { children }
        | RichNode::TaskItem { children, .. } => {
            for child in children {
                append_rich_text(child, output);
            }
            if !matches!(node, RichNode::Root { .. }) && !output.ends_with('\n') {
                output.push('\n');
            }
        }
    }
}

fn push_value(
    value: &crate::data::edit_input::ValueDto,
    definition: Option<&artifact::FieldDefinition>,
    label: &str,
    output: &mut Vec<SearchText>,
) {
    use crate::data::edit_input::ValueDto;
    let text = match value {
        ValueDto::Unset {}
        | ValueDto::Image { .. }
        | ValueDto::File { .. }
        | ValueDto::Relation { .. }
        | ValueDto::DocumentLink { .. } => None,
        ValueDto::SingleLineText { value }
        | ValueDto::Number { value }
        | ValueDto::Date { value }
        | ValueDto::Time { value }
        | ValueDto::Url { value } => Some(value.clone()),
        ValueDto::Duration { milliseconds } => Some(milliseconds.clone()),
        ValueDto::NumberUnknown { .. } => Some("불명".to_owned()),
        ValueDto::SingleChoice { option } => definition
            .and_then(|field| field.configuration().options())
            .and_then(|options| option.parse().ok().and_then(|id| options.get(&id)))
            .map(|option| option.label().to_owned()),
        ValueDto::MultiChoice { options } => {
            let definitions = definition.and_then(|field| field.configuration().options());
            let labels: Vec<_> = options
                .iter()
                .filter_map(|option| {
                    definitions.and_then(|definitions| {
                        option
                            .parse()
                            .ok()
                            .and_then(|id| definitions.get(&id))
                            .map(|option| option.label().to_owned())
                    })
                })
                .collect();
            (!labels.is_empty()).then(|| labels.join(", "))
        }
        ValueDto::RichText { content } => {
            let mut text = String::new();
            append_rich_text(content, &mut text);
            let text = text.trim_end_matches('\n').to_owned();
            (!text.is_empty()).then_some(text)
        }
        ValueDto::Group { instances } => {
            let members = definition.and_then(|field| field.configuration().members());
            for instance in instances {
                for child in &instance.fields {
                    let Some((_, member_definitions)) = members else {
                        continue;
                    };
                    let Ok(child_id) = child.field.parse() else {
                        continue;
                    };
                    let Some(child_definition) = member_definitions.get(&child_id) else {
                        continue;
                    };
                    if child_definition.lifecycle() != artifact::FieldLifecycle::Active {
                        continue;
                    }
                    if let Intent::Set(value) = &child.value {
                        push_value(
                            value,
                            Some(child_definition),
                            &format!("{label} · {}", child_definition.label()),
                            output,
                        );
                    }
                }
            }
            None
        }
    };
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        output.push(SearchText {
            label: label.to_owned(),
            text,
        });
    }
}

fn path(
    id: artifact::DocumentId,
    layout: &DocumentLayout,
    names: &BTreeMap<artifact::DocumentId, String>,
) -> Vec<String> {
    let mut path = Vec::new();
    let mut parent = layout.nodes.get(&id).and_then(|node| node.parent_id);
    while let Some(id) = parent {
        if path.len() >= 256 {
            break;
        }
        if let Some(name) = names.get(&id) {
            path.push(name.clone());
        }
        parent = layout.nodes.get(&id).and_then(|node| node.parent_id);
    }
    path.reverse();
    path
}

fn project_entry(
    document: &artifact::DocumentArtifact,
    template: &artifact::TemplateArtifact,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Entry> {
    let reconciled =
        artifact::reconcile_document_for_read(template, document).map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?;
    let mut fields = Vec::new();
    if !document.english_name().is_empty() {
        fields.push(SearchText {
            label: "영어 이름".into(),
            text: document.english_name().into(),
        });
    }
    if !document.glossary_summary().is_empty() {
        fields.push(SearchText {
            label: "한줄 설명".into(),
            text: document.glossary_summary().into(),
        });
    }
    let mut references = Vec::new();
    for field in reconciled.known_fields() {
        if field.lifecycle() != artifact::FieldLifecycle::Active {
            continue;
        }
        let Some(value) = field.value() else {
            continue;
        };
        let definition = template.fields().get(&field.field_id());
        let value = projection::field_value(value, definition)?;
        push_value(&value, definition, field.label(), &mut fields);
        collect_references(
            &value,
            definition,
            &field.field_id().to_string(),
            field.label(),
            None,
            &mut references,
        );
    }
    Ok(Entry {
        id: document.document_id().to_string(),
        template: document.template_id().to_string(),
        template_name: template.name().to_owned(),
        name: document.name().to_owned(),
        path: Vec::new(),
        fields,
        references,
        active: true,
    })
}

fn validation_codes(reconciled: &artifact::ReconciledDocumentView<'_>) -> Vec<String> {
    reconciled
        .warnings()
        .iter()
        .map(|warning| format!("{:?}", warning.category()))
        .chain(
            reconciled
                .blocking_issues()
                .iter()
                .map(|issue| format!("{:?}", issue.category())),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn validation_codes_for_document(
    document: &artifact::DocumentArtifact,
    template: &artifact::TemplateArtifact,
) -> Result<Vec<String>, artifact::DocumentReconciliationError> {
    artifact::reconcile_document_for_read(template, document).map(|value| validation_codes(&value))
}

fn collect_references(
    value: &crate::data::edit_input::ValueDto,
    definition: Option<&artifact::FieldDefinition>,
    field: &str,
    field_label: &str,
    instance: Option<&str>,
    output: &mut Vec<ReferenceEdge>,
) {
    use crate::data::edit_input::ValueDto;
    match value {
        ValueDto::Relation { links } => {
            let notice = definition
                .and_then(|definition| definition.configuration().relation_settings())
                .is_none_or(|(_, _, notice)| notice);
            output.extend(links.iter().map(|link| ReferenceEdge {
                kind: ReferenceKind::Relation,
                target: link.document.clone(),
                field: field.to_owned(),
                field_label: field_label.to_owned(),
                instance: instance.map(str::to_owned),
                connection: link.id.clone(),
                relation_name: link.name.clone(),
                one_way: link.one_way,
                reciprocal_notice: notice,
            }));
        }
        ValueDto::DocumentLink { documents } => {
            output.extend(documents.iter().map(|document| ReferenceEdge {
                kind: ReferenceKind::DocumentLink,
                target: document.clone(),
                field: field.to_owned(),
                field_label: field_label.to_owned(),
                instance: instance.map(str::to_owned),
                connection: String::new(),
                relation_name: String::new(),
                one_way: false,
                reciprocal_notice: false,
            }));
        }
        ValueDto::Group { instances } => {
            let members = definition.and_then(|definition| definition.configuration().members());
            for card in instances {
                for child in &card.fields {
                    let Some((_, definitions)) = members else {
                        continue;
                    };
                    let Ok(child_id) = child.field.parse() else {
                        continue;
                    };
                    let Some(child_definition) = definitions.get(&child_id) else {
                        continue;
                    };
                    if child_definition.lifecycle() != artifact::FieldLifecycle::Active {
                        continue;
                    }
                    if let Intent::Set(child_value) = &child.value {
                        collect_references(
                            child_value,
                            Some(child_definition),
                            &child.field,
                            child_definition.label(),
                            Some(&card.id.to_string()),
                            output,
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

fn refresh_paths(cache: &mut Cache) -> Reply<()> {
    for entry in &mut cache.entries {
        let id = entry.id.parse().map_err(|_| Code::SerializationFailed)?;
        entry.path = path(id, &cache.layout, &cache.names);
    }
    Ok(())
}

/// 확정 저장 뒤 실제 candidate 한 건만 다시 읽어 파생 검색 자료에 반영한다.
/// source가 다르면 옛 자료를 최신으로 표시하지 않도록 호출자가 cache를 폐기한다.
pub(super) fn apply_committed_change(
    ctx: &mut Context,
    cache: &mut Cache,
    change: &crate::data::repository::DocumentScanChange,
    generation: Id,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<()> {
    let id = change.document_id();
    let (document, template) = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready)?;
            let document = repository.load_document(id)?;
            let template = repository.load_template(document.artifact().template_id())?;
            Ok::<_, crate::data::repository::RepositoryError>((document, template))
        })
        .map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RuntimeRejected)
        })?
        .map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?;
    if document.source() != change.current_source() {
        return Err(Code::WrongBinding.into());
    }
    // 문서만 같아도 표시 정의가 달라지면 한 행을 새 정의로 섞을 수 없다.
    // 이미 읽은 Template의 실제 bytes 증거가 다르면 호출자가 전체 자료를 폐기한다.
    if cache.templates.get(&document.artifact().template_id()) != Some(template.source()) {
        return Err(Code::WrongBinding.into());
    }
    let mut entry = project_entry(document.artifact(), template.artifact(), observations)?;
    let Some(index) = cache
        .entries
        .iter()
        .position(|entry| entry.id == id.to_string())
    else {
        return Err(Code::WrongBinding.into());
    };
    entry.active = cache.entries[index].active;
    cache.names.insert(id, entry.name.clone());
    cache.entries[index] = entry;
    cache.generation = generation;
    refresh_paths(cache)
}

/// 이미 layout 쓰기가 검증한 전체 membership을 재사용해 포함 여부와 경로만 갱신한다.
/// 복원·생성으로 새로 active가 된 문서만 실제 source를 한 건씩 읽는다.
pub(super) fn apply_layout_commit(
    ctx: &mut Context,
    cache: &mut Cache,
    layout: &DocumentLayout,
    documents: &crate::data::repository::CompleteDocumentScan,
    generation: Id,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<()> {
    let mut next = cache.clone();
    next.layout = layout.clone();
    // strict 쓰기가 검증한 완전 membership으로 갱신한 뒤에는 이전 누락 안내도 끝낸다.
    next.missing_documents = 0;
    next.names = documents
        .summaries()
        .map(|(id, _, name, _, _, _)| (id, name.to_owned()))
        .collect();
    let present_documents: BTreeSet<_> = documents
        .summaries()
        .map(|(id, _, _, _, _, _)| id)
        .collect();
    next.entries.retain(|entry| {
        entry
            .id
            .parse()
            .ok()
            .is_some_and(|id| present_documents.contains(&id))
    });
    for entry in &mut next.entries {
        let id = entry.id.parse().map_err(|_| Code::SerializationFailed)?;
        entry.active = !layout
            .nodes
            .get(&id)
            .is_some_and(|node| node.state == LayoutState::Trashed);
    }
    let present: BTreeSet<_> = next
        .entries
        .iter()
        .filter_map(|entry| entry.id.parse().ok())
        .collect();
    for id in present_documents.difference(&present).copied() {
        let (document, template) = ctx
            .read(|ready| {
                let repository = ArtifactRepository::new(ready)?;
                let document = repository.load_document(id)?;
                let template = repository.load_template(document.artifact().template_id())?;
                Ok::<_, crate::data::repository::RepositoryError>((document, template))
            })
            .map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RuntimeRejected)
            })?
            .map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
        if documents.source(id) != Some(document.source()) {
            return Err(Code::WrongBinding.into());
        }
        let template_id = document.artifact().template_id();
        if next
            .templates
            .get(&template_id)
            .is_some_and(|source| source != template.source())
        {
            return Err(Code::WrongBinding.into());
        }
        next.templates
            .insert(template_id, template.source().clone());
        let mut entry = project_entry(document.artifact(), template.artifact(), observations)?;
        entry.active = !layout
            .nodes
            .get(&id)
            .is_some_and(|node| node.state == LayoutState::Trashed);
        next.entries.push(entry);
    }
    next.generation = generation;
    refresh_paths(&mut next)?;
    *cache = next;
    Ok(())
}

pub(super) fn build(
    ctx: &mut Context,
    generation: Id,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Cache> {
    let (entries, layout, ids, names, templates, missing_documents) = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready).map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
            let loaded_layout = repository.load_layout().map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
            let layout_source = loaded_layout.as_ref().map(|loaded| loaded.source().clone());
            let mut layout = loaded_layout.map(|loaded| loaded.into_artifact());
            let mut ids = BTreeSet::new();
            let mut names = BTreeMap::new();
            let mut templates = BTreeMap::new();
            let mut unreadable_documents = 0usize;
            let entries = repository
                .scan_documents_with(|document| -> Reply<Option<Entry>> {
                    let id = document.document_id();
                    let template_id = document.template_id();
                    ids.insert(id);
                    names.insert(id, document.name().to_owned());
                    let active = !layout
                        .as_ref()
                        .and_then(|layout| layout.nodes.get(&id))
                        .is_some_and(|node| node.state != LayoutState::Active);
                    if !templates.contains_key(&template_id) {
                        let loaded = match repository.load_template(template_id) {
                            Ok(loaded) => loaded,
                            Err(error) => {
                                observations.push(Box::new(error));
                                unreadable_documents += 1;
                                return Ok(None);
                            }
                        };
                        templates.insert(template_id, loaded);
                    }
                    let template = templates[&template_id].artifact();
                    match project_entry(document, template, observations) {
                        Ok(mut entry) => {
                            entry.active = active;
                            Ok(Some(entry))
                        }
                        Err(_) => {
                            unreadable_documents += 1;
                            Ok(None)
                        }
                    }
                })
                .map_err(|error| match error {
                    crate::data::repository::DocumentScanVisitError::Repository(error) => {
                        super::document_workspace::observed(
                            observations,
                            error,
                            Code::RepositoryRejected,
                        )
                    }
                    crate::data::repository::DocumentScanVisitError::Visitor(error) => error,
                })?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            let layout = layout
                .take()
                .unwrap_or_else(|| DocumentLayout::flat(ids.iter().copied()));
            // 검색은 layout에 남은 참조의 실제 NotFound만 불완전 읽기로 알린다.
            // 이 집합을 쓰기용 CompleteDocumentScan으로 만들거나 layout을 고치지 않는다.
            let mut missing_documents = 0;
            let mut readable_ids = ids.clone();
            for id in layout.nodes.keys().filter(|id| !ids.contains(id)) {
                match repository.load_document(*id) {
                    Err(error)
                        if error.diagnostic().category
                            == crate::data::repository::RepositoryCategory::NotFound =>
                    {
                        missing_documents += 1;
                        readable_ids.insert(*id);
                    }
                    Err(error) => {
                        return Err(super::document_workspace::observed(
                            observations,
                            error,
                            Code::RepositoryRejected,
                        ))
                    }
                    Ok(_) => return Err(Code::WrongBinding.into()),
                }
            }
            missing_documents += unreadable_documents;
            layout.reconcile(&readable_ids).map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
            // 구축 중 정의나 배치가 바뀌면 완성된 세대로 게시하지 않는다.
            for template in templates.values() {
                if !repository
                    .reread_bytes_match(template.source())
                    .map_err(|error| {
                        super::document_workspace::observed(
                            observations,
                            error,
                            Code::RepositoryRejected,
                        )
                    })?
                {
                    return Err(Code::WrongBinding.into());
                }
            }
            let after_layout = repository.load_layout().map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
            if layout_source.as_ref() != after_layout.as_ref().map(|loaded| loaded.source()) {
                return Err(Code::WrongBinding.into());
            }
            let membership = ids
                .iter()
                .copied()
                .map(crate::data::repository::ArtifactSourceId::Document)
                .collect();
            repository
                .confirm_document_membership(&membership)
                .map_err(|error| {
                    super::document_workspace::observed(
                        observations,
                        error,
                        Code::RepositoryRejected,
                    )
                })?;
            let templates = templates
                .into_iter()
                .map(|(id, loaded)| (id, loaded.source().clone()))
                .collect();
            Ok::<_, ErrorDto>((entries, layout, ids, names, templates, missing_documents))
        })
        .map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RuntimeRejected)
        })??;
    debug_assert_eq!(ids.len(), names.len());
    let mut cache = Cache {
        generation,
        entries,
        layout,
        names,
        templates,
        missing_documents,
    };
    refresh_paths(&mut cache)?;
    Ok(cache)
}

fn excerpt(text: &str) -> String {
    let mut result: String = text.chars().take(180).collect();
    if result.chars().count() < text.chars().count() {
        result.push('…');
    }
    result
}

pub(super) fn response(
    cache: &Cache,
    query: &str,
    template: Option<&str>,
    offset: usize,
    limit: usize,
) -> Response {
    let query = query.trim().to_lowercase();
    let mut matches: Vec<_> = cache
        .entries
        .iter()
        .filter(|entry| entry.active)
        .filter_map(|entry| {
            if template.is_some_and(|template| entry.template != template) {
                return None;
            }
            let title_match = query.is_empty() || entry.name.to_lowercase().contains(&query);
            let field = (!query.is_empty())
                .then(|| {
                    entry
                        .fields
                        .iter()
                        .find(|field| field.text.to_lowercase().contains(&query))
                })
                .flatten();
            if !title_match && field.is_none() {
                return None;
            }
            Some((entry, title_match, field))
        })
        .collect();
    matches.sort_by(|(left, left_title, _), (right, right_title, _)| {
        right_title
            .cmp(left_title)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| {
                left.template_name
                    .to_lowercase()
                    .cmp(&right.template_name.to_lowercase())
            })
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.id.cmp(&right.id))
    });
    let total = matches.len();
    let results = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(entry, title_match, field)| SearchResult {
            id: entry.id.clone(),
            template: entry.template.clone(),
            template_name: entry.template_name.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
            title_match,
            excerpt: field.map(|field| SearchExcerpt {
                label: field.label.clone(),
                text: excerpt(&field.text),
            }),
        })
        .collect::<Vec<_>>();
    Response::Search {
        generation: cache.generation,
        offset,
        total,
        has_more: offset.saturating_add(results.len()) < total,
        missing_documents: cache.missing_documents,
        // 현재 page의 일치 여부와 무관하게 같은 cache가 관측한 정의 이름을 전달한다.
        // 검색과 별도 수명인 Template 관리 목록/편집 원본은 바꾸지 않는다.
        template_names: cache
            .entries
            .iter()
            .filter(|entry| entry.active)
            .map(|entry| (entry.template.clone(), entry.template_name.clone()))
            .collect(),
        results,
    }
}

/// 문서를 열기 전에도 트리 상태를 표시할 수 있도록 같은 세대에서 계산한
/// 문서별 검증 코드만 투영한다. 본문이나 필드 값은 UI로 반환하지 않는다.
pub(super) fn references_response(cache: &Cache, document: &str) -> Response {
    let reciprocal_sources: BTreeSet<_> = cache
        .entries
        .iter()
        .filter(|entry| entry.active)
        .flat_map(|entry| {
            entry
                .references
                .iter()
                .filter(|reference| matches!(reference.kind, ReferenceKind::Relation))
                .map(move |reference| (entry.id.as_str(), reference.target.as_str()))
        })
        .collect();
    let mut incoming = cache
        .entries
        .iter()
        .filter(|entry| entry.active)
        .flat_map(|entry| {
            entry
                .references
                .iter()
                .filter(move |reference| reference.target == document)
                .map(|reference| match reference.kind {
                    ReferenceKind::Relation => IncomingReference::Relation {
                        source: entry.id.clone(),
                        source_name: entry.name.clone(),
                        source_template: entry.template_name.clone(),
                        path: entry.path.clone(),
                        target: reference.target.clone(),
                        field: reference.field.clone(),
                        field_label: reference.field_label.clone(),
                        instance: reference.instance.clone(),
                        connection: reference.connection.clone(),
                        relation_name: reference.relation_name.clone(),
                        one_way: reference.one_way,
                        missing_reciprocal: reference.reciprocal_notice
                            && !reference.one_way
                            && !reciprocal_sources.contains(&(document, entry.id.as_str())),
                    },
                    ReferenceKind::DocumentLink => IncomingReference::DocumentLink {
                        source: entry.id.clone(),
                        source_name: entry.name.clone(),
                        source_template: entry.template_name.clone(),
                        path: entry.path.clone(),
                        target: reference.target.clone(),
                        field: reference.field.clone(),
                        field_label: reference.field_label.clone(),
                        instance: reference.instance.clone(),
                    },
                })
        })
        .collect::<Vec<_>>();
    let mut unavailable = cache
        .entries
        .iter()
        .filter(|entry| !entry.active)
        .filter(|entry| {
            entry
                .references
                .iter()
                .any(|reference| reference.target == document)
        })
        .map(|entry| UnavailableIncomingReference {
            source: entry.id.clone(),
            source_name: entry.name.clone(),
            reason: "trashed",
        })
        .collect::<Vec<_>>();
    unavailable.sort_by(|left, right| {
        left.source_name
            .to_lowercase()
            .cmp(&right.source_name.to_lowercase())
            .then_with(|| left.source.cmp(&right.source))
    });
    incoming.sort_by(|left, right| {
        let parts = |value: &IncomingReference| match value {
            IncomingReference::Relation {
                source_name,
                field_label,
                connection,
                ..
            } => (
                source_name.to_lowercase(),
                field_label.clone(),
                connection.clone(),
                0_u8,
            ),
            IncomingReference::DocumentLink {
                source_name,
                field_label,
                ..
            } => (
                source_name.to_lowercase(),
                field_label.clone(),
                String::new(),
                1_u8,
            ),
        };
        let left = parts(left);
        let right = parts(right);
        left.0
            .cmp(&right.0)
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    Response::References {
        generation: cache.generation,
        document: document.to_owned(),
        incomplete: unavailable.len(),
        incoming,
        unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::edit_input::{RichNode, ValueDto};

    fn entry(id: &str, name: &str, template: &str, fields: &[(&str, &str)]) -> Entry {
        Entry {
            id: id.into(),
            template: template.into(),
            template_name: format!("Template {template}"),
            name: name.into(),
            path: vec!["상위".into()],
            fields: fields
                .iter()
                .map(|(label, text)| SearchText {
                    label: (*label).into(),
                    text: (*text).into(),
                })
                .collect(),
            references: Vec::new(),
            active: true,
        }
    }

    #[test]
    fn keeps_field_boundaries_and_prioritizes_title_matches() {
        let cache = Cache {
            generation: Id::new(),
            entries: vec![
                entry(
                    "field",
                    "나 문서",
                    "a",
                    &[("첫째", "alpha"), ("둘째", "beta")],
                ),
                entry("title", "ALPHA 문서", "a", &[]),
                entry("other", "다 문서", "b", &[("설명", "alpha 내용")]),
            ],
            layout: DocumentLayout::flat(std::iter::empty()),
            names: BTreeMap::new(),
            templates: BTreeMap::new(),
            missing_documents: 0,
        };
        let Response::Search { results, total, .. } = response(&cache, " alpha ", None, 0, 100)
        else {
            panic!("search response")
        };
        assert_eq!(total, 3);
        assert_eq!(results[0].id, "title");
        assert!(results[0].title_match);
        assert_eq!(results[1].id, "field");
        assert!(!results[1].title_match);

        let Response::Search { total, .. } = response(&cache, "pha be", None, 0, 100) else {
            panic!("search response")
        };
        assert_eq!(total, 0, "separate fields must not create a match");
    }

    #[test]
    fn filters_and_pages_without_dropping_the_total() {
        let cache = Cache {
            generation: Id::new(),
            entries: vec![
                entry("a", "가", "one", &[]),
                entry("b", "나", "one", &[]),
                entry("c", "다", "two", &[]),
            ],
            layout: DocumentLayout::flat(std::iter::empty()),
            names: BTreeMap::new(),
            templates: BTreeMap::new(),
            missing_documents: 0,
        };
        let wire = serde_json::to_value(response(&cache, "", Some("one"), 0, 1)).unwrap();
        assert_eq!(wire["hasMore"], true);
        assert!(wire.get("has_more").is_none());
        let Response::Search {
            results,
            total,
            has_more,
            ..
        } = response(&cache, "", Some("one"), 1, 1)
        else {
            panic!("search response")
        };
        assert_eq!(total, 2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "b");
        assert!(!has_more);
    }

    #[test]
    fn rich_text_joins_inline_text_but_preserves_block_breaks() {
        let value = ValueDto::RichText {
            content: RichNode::Root {
                children: vec![
                    RichNode::Paragraph {
                        children: vec![
                            RichNode::Text {
                                text: "강조".into(),
                                marks: vec![],
                            },
                            RichNode::Text {
                                text: " 본문".into(),
                                marks: vec![],
                            },
                        ],
                    },
                    RichNode::Paragraph {
                        children: vec![RichNode::Text {
                            text: "다음".into(),
                            marks: vec![],
                        }],
                    },
                ],
            },
        };
        let mut output = Vec::new();
        push_value(&value, None, "설명", &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].text, "강조 본문\n다음");
    }

    #[test]
    fn incoming_references_keep_relation_and_document_link_kinds_separate() {
        let mut source = entry("source", "출처", "a", &[]);
        source.references = vec![
            ReferenceEdge {
                kind: ReferenceKind::Relation,
                target: "target".into(),
                field: "field-a".into(),
                field_label: "관계 A".into(),
                instance: None,
                connection: "connection-a".into(),
                relation_name: "친구".into(),
                one_way: false,
                reciprocal_notice: true,
            },
            ReferenceEdge {
                kind: ReferenceKind::DocumentLink,
                target: "target".into(),
                field: "field-link".into(),
                field_label: "문서 링크".into(),
                instance: None,
                connection: String::new(),
                relation_name: String::new(),
                one_way: false,
                reciprocal_notice: false,
            },
            ReferenceEdge {
                kind: ReferenceKind::Relation,
                target: "missing".into(),
                field: "field-b".into(),
                field_label: "관계 B".into(),
                instance: None,
                connection: "connection-b".into(),
                relation_name: String::new(),
                one_way: true,
                reciprocal_notice: true,
            },
        ];
        let mut target = entry("target", "대상", "b", &[]);
        target.references.push(ReferenceEdge {
            kind: ReferenceKind::Relation,
            target: "source".into(),
            field: "field-c".into(),
            field_label: "역관계".into(),
            instance: None,
            connection: "connection-c".into(),
            relation_name: "친구".into(),
            one_way: false,
            reciprocal_notice: true,
        });
        let mut cache = Cache {
            generation: Id::new(),
            entries: vec![source, target],
            layout: DocumentLayout::flat(std::iter::empty()),
            names: BTreeMap::new(),
            templates: BTreeMap::new(),
            missing_documents: 0,
        };
        let Response::References { incoming, .. } = references_response(&cache, "target") else {
            panic!("references response")
        };
        assert_eq!(incoming.len(), 2);
        assert!(matches!(
            &incoming[0],
            IncomingReference::Relation {
                relation_name,
                missing_reciprocal: false,
                ..
            } if relation_name == "친구"
        ));
        assert!(matches!(
            &incoming[1],
            IncomingReference::DocumentLink { .. }
        ));
        cache.entries[1].references[0].kind = ReferenceKind::DocumentLink;
        let Response::References { incoming, .. } = references_response(&cache, "target") else {
            panic!("references response")
        };
        assert!(matches!(
            &incoming[0],
            IncomingReference::Relation {
                missing_reciprocal: true,
                ..
            }
        ));
        let Response::References { incoming, .. } = references_response(&cache, "missing") else {
            panic!("references response")
        };
        assert_eq!(incoming.len(), 1);
        assert!(matches!(
            &incoming[0],
            IncomingReference::Relation {
                missing_reciprocal: false,
                ..
            }
        ));
        cache.entries[0].active = false;
        let Response::References {
            incoming,
            unavailable,
            incomplete,
            ..
        } = references_response(&cache, "target")
        else {
            panic!("references response")
        };
        assert!(incoming.is_empty());
        assert_eq!(incomplete, 1);
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[0].source_name, "출처");
        assert_eq!(unavailable[0].reason, "trashed");
    }

    #[test]
    fn root_relation_and_document_link_values_project_distinct_reference_edges() {
        let relation: ValueDto = serde_json::from_value(serde_json::json!({
            "kind":"relation",
            "links":[{
                "id":"cccccccc-cccc-4ccc-8ccc-000000000001",
                "document":"bbbbbbbb-bbbb-4bbb-8bbb-000000000001",
                "oneWay":false,
                "name":"친구"
            }]
        }))
        .unwrap();
        let document_link: ValueDto = serde_json::from_value(serde_json::json!({
            "kind":"document_link",
            "documents":["bbbbbbbb-bbbb-4bbb-8bbb-000000000001"]
        }))
        .unwrap();
        let mut output = Vec::new();
        collect_references(&relation, None, "relation", "관계", None, &mut output);
        collect_references(
            &document_link,
            None,
            "document-link",
            "문서 링크",
            None,
            &mut output,
        );
        assert_eq!(output.len(), 2);
        assert!(matches!(output[0].kind, ReferenceKind::Relation));
        assert_eq!(output[0].relation_name, "친구");
        assert!(matches!(output[1].kind, ReferenceKind::DocumentLink));
        assert_eq!(output[1].field, "document-link");
    }

    #[test]
    fn scalar_text_keeps_exact_numbers_and_distinguishes_unknown_from_unset() {
        let mut output = Vec::new();
        push_value(
            &ValueDto::Number {
                value: "12345678901234567890.001".into(),
            },
            None,
            "정밀 숫자",
            &mut output,
        );
        push_value(
            &ValueDto::NumberUnknown { previous_raw: None },
            None,
            "불명 숫자",
            &mut output,
        );
        push_value(&ValueDto::Unset {}, None, "미입력", &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].text, "12345678901234567890.001");
        assert_eq!(output[1].text, "불명");
    }
}
