//! 저장된 활성 문서의 literal 일괄 바꾸기 미리보기와 exact multi-document 적용.
use super::*;
use crate::commands::document_workspace::{ReplaceBlocker, ReplaceChange, ReplaceScopes, Response};
use crate::data::{
    application::bulk_replace::{BulkReplaceInput, DocumentReplacement},
    artifact::{
        self, group::CellInput, group::InstanceInput, DocumentEdit, DocumentEditSet,
        DocumentValueEdit, FieldKind, FieldLifecycle,
    },
    edit_recovery::model::Intent,
    field_engine::{rich_text::normalize_rich_text, scalar::validate_optional_single_line_text},
    repository::{ArtifactRepository, ArtifactSourceId},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy)]
struct MatchOptions {
    case_sensitive: bool,
    whole_word: bool,
}

pub(super) struct Preview {
    pub(super) id: Id,
    pub(super) changes: Vec<ReplaceChange>,
    pub(super) blockers: Vec<ReplaceBlocker>,
    pub(super) targets: BTreeSet<artifact::DocumentId>,
    pub(super) input: Option<BulkReplaceInput>,
}

struct TextReplacement {
    value: String,
    ranges: Vec<(usize, usize)>,
}

fn is_word_character(character: char) -> bool {
    unicode_ident::is_xid_continue(character)
}

fn boundary_matches(value: &str, start: usize, end: usize) -> bool {
    let before = value[..start].chars().next_back();
    let after = value[end..].chars().next();
    !before.is_some_and(is_word_character) && !after.is_some_and(is_word_character)
}

fn match_ranges(value: &str, needle: &str, options: MatchOptions) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    if options.case_sensitive {
        let mut cursor = 0;
        while let Some(relative) = value[cursor..].find(needle) {
            let start = cursor + relative;
            let end = start + needle.len();
            if !options.whole_word || boundary_matches(value, start, end) {
                ranges.push((start, end));
            }
            cursor = end;
        }
        return ranges;
    }

    // Rust의 Unicode lowercase 확장을 사용한다. 원문 문자 경계에서 시작·끝나는
    // 경우만 채택해 다문자 소문자 결과의 중간을 부분 일치로 오인하지 않는다.
    let folded_needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    let mut folded = String::new();
    let mut boundaries = vec![(0_usize, 0_usize)];
    for (original_start, character) in value.char_indices() {
        folded.extend(character.to_lowercase());
        boundaries.push((folded.len(), original_start + character.len_utf8()));
    }
    let folded_to_original: BTreeMap<_, _> = boundaries.iter().copied().collect();
    let mut minimum_original = 0;
    for (folded_start, original_start) in boundaries.iter().copied() {
        if original_start < minimum_original || !folded[folded_start..].starts_with(&folded_needle)
        {
            continue;
        }
        let folded_end = folded_start + folded_needle.len();
        let Some(original_end) = folded_to_original.get(&folded_end).copied() else {
            continue;
        };
        if !options.whole_word || boundary_matches(value, original_start, original_end) {
            ranges.push((original_start, original_end));
        }
        minimum_original = original_end;
    }
    ranges
}

fn replace_text(
    value: &str,
    needle: &str,
    replacement: &str,
    options: MatchOptions,
) -> TextReplacement {
    let ranges: Vec<_> = match_ranges(value, needle, options)
        .into_iter()
        .filter(|(start, end)| &value[*start..*end] != replacement)
        .collect();
    if ranges.is_empty() {
        return TextReplacement {
            value: value.to_owned(),
            ranges,
        };
    }
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    for (start, end) in &ranges {
        output.push_str(&value[cursor..*start]);
        output.push_str(replacement);
        cursor = *end;
    }
    output.push_str(&value[cursor..]);
    TextReplacement {
        value: output,
        ranges,
    }
}

struct SnippetParts {
    prefix: String,
    highlight: String,
    suffix: String,
}

fn snippet_parts(value: &str, start: usize, end: usize, replacement: Option<&str>) -> SnippetParts {
    let before: String = value[..start]
        .chars()
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = value[end..].chars().take(48).collect();
    let after_truncated = after.chars().count() < value[end..].chars().count();
    let mut prefix = String::new();
    if before.chars().count() < value[..start].chars().count() {
        prefix.push('…');
    }
    prefix.push_str(&before);
    let mut suffix = after;
    if after_truncated {
        suffix.push('…');
    }
    SnippetParts {
        prefix,
        highlight: replacement.unwrap_or(&value[start..end]).to_owned(),
        suffix,
    }
}

fn change_rows(
    document: &artifact::DocumentArtifact,
    template: &artifact::TemplateArtifact,
    path: &[String],
    scope: &str,
    label: &str,
    original: &str,
    ranges: &[(usize, usize)],
    replacement: &str,
) -> Vec<ReplaceChange> {
    ranges
        .iter()
        .map(|(start, end)| {
            let before = snippet_parts(original, *start, *end, None);
            let after = snippet_parts(original, *start, *end, Some(replacement));
            ReplaceChange {
                document: document.document_id().to_string(),
                document_name: document.name().to_owned(),
                template_name: template.name().to_owned(),
                path: path.to_vec(),
                scope: scope.to_owned(),
                label: label.to_owned(),
                before: format!("{}{}{}", before.prefix, before.highlight, before.suffix),
                after: format!("{}{}{}", after.prefix, after.highlight, after.suffix),
                before_prefix: before.prefix,
                before_match: before.highlight,
                before_suffix: before.suffix,
                after_prefix: after.prefix,
                after_match: after.highlight,
                after_suffix: after.suffix,
            }
        })
        .collect()
}

#[derive(Clone)]
struct InlineNode {
    object: Map<String, Value>,
    start: usize,
    end: usize,
}

fn emit_original_range(
    nodes: &[InlineNode],
    text: &str,
    start: usize,
    end: usize,
    output: &mut Vec<Value>,
) {
    if start == end {
        return;
    }
    for node in nodes {
        let from = start.max(node.start);
        let to = end.min(node.end);
        if from >= to {
            continue;
        }
        let mut object = node.object.clone();
        object.insert("text".into(), Value::String(text[from..to].to_owned()));
        output.push(Value::Object(object));
    }
}

fn replace_inline_segment(
    values: &[Value],
    needle: &str,
    replacement: &str,
    options: MatchOptions,
    previews: &mut Vec<(String, Vec<(usize, usize)>)>,
) -> Reply<Vec<Value>> {
    let mut text = String::new();
    let mut nodes = Vec::with_capacity(values.len());
    for value in values {
        let object = value.as_object().ok_or(Code::SerializationFailed)?;
        let value = object
            .get("text")
            .and_then(Value::as_str)
            .ok_or(Code::SerializationFailed)?;
        let start = text.len();
        text.push_str(value);
        nodes.push(InlineNode {
            object: object.clone(),
            start,
            end: text.len(),
        });
    }
    let replaced = replace_text(&text, needle, replacement, options);
    if replaced.ranges.is_empty() {
        return Ok(values.to_vec());
    }
    previews.push((text.clone(), replaced.ranges.clone()));
    let mut output = Vec::new();
    let mut cursor = 0;
    for (start, end) in replaced.ranges {
        emit_original_range(&nodes, &text, cursor, start, &mut output);
        if !replacement.is_empty() {
            let source = nodes
                .iter()
                .find(|node| node.start <= start && start < node.end)
                .ok_or(Code::SerializationFailed)?;
            let mut object = source.object.clone();
            object.insert("text".into(), Value::String(replacement.to_owned()));
            output.push(Value::Object(object));
        }
        cursor = end;
    }
    emit_original_range(&nodes, &text, cursor, text.len(), &mut output);
    Ok(output)
}

fn replace_inline_children(
    children: &[Value],
    needle: &str,
    replacement: &str,
    options: MatchOptions,
    previews: &mut Vec<(String, Vec<(usize, usize)>)>,
) -> Reply<Vec<Value>> {
    let mut output = Vec::new();
    let mut segment = Vec::new();
    let flush = |segment: &mut Vec<Value>, output: &mut Vec<Value>, previews: &mut Vec<_>| {
        if segment.is_empty() {
            return Ok(());
        }
        output.extend(replace_inline_segment(
            segment,
            needle,
            replacement,
            options,
            previews,
        )?);
        segment.clear();
        Ok::<_, ErrorDto>(())
    };
    for child in children {
        let kind = child
            .as_object()
            .and_then(|object| object.get("kind"))
            .and_then(Value::as_str)
            .ok_or(Code::SerializationFailed)?;
        if kind == "text" {
            segment.push(child.clone());
        } else {
            flush(&mut segment, &mut output, previews)?;
            output.push(child.clone());
        }
    }
    flush(&mut segment, &mut output, previews)?;
    Ok(output)
}

fn replace_rich_node(
    node: &mut Map<String, Value>,
    needle: &str,
    replacement: &str,
    options: MatchOptions,
    previews: &mut Vec<(String, Vec<(usize, usize)>)>,
) -> Reply<()> {
    let kind = node
        .get("kind")
        .and_then(Value::as_str)
        .ok_or(Code::SerializationFailed)?
        .to_owned();
    let Some(children) = node.get_mut("children") else {
        return Ok(());
    };
    let values = children.as_array_mut().ok_or(Code::SerializationFailed)?;
    if matches!(kind.as_str(), "paragraph" | "heading") {
        *values = replace_inline_children(values, needle, replacement, options, previews)?;
        return Ok(());
    }
    for child in values {
        replace_rich_node(
            child.as_object_mut().ok_or(Code::SerializationFailed)?,
            needle,
            replacement,
            options,
            previews,
        )?;
    }
    Ok(())
}

fn rich_replacement(
    document: &artifact::RichTextDocument,
    needle: &str,
    replacement: &str,
    options: MatchOptions,
) -> Reply<(Map<String, Value>, Vec<(String, Vec<(usize, usize)>)>)> {
    let mut content = document.content_for_projection().clone();
    let mut previews = Vec::new();
    replace_rich_node(&mut content, needle, replacement, options, &mut previews)?;
    Ok((content, previews))
}

fn blocker(
    document: Option<&artifact::DocumentArtifact>,
    label: impl Into<String>,
    reason: impl Into<String>,
) -> ReplaceBlocker {
    ReplaceBlocker {
        document: document.map(|document| document.document_id().to_string()),
        document_name: document.map(|document| document.name().to_owned()),
        label: label.into(),
        reason: reason.into(),
    }
}

fn document_path(
    id: artifact::DocumentId,
    layout: &artifact::layout::DocumentLayout,
    names: &BTreeMap<artifact::DocumentId, String>,
) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = layout.nodes.get(&id).and_then(|node| node.parent_id);
    while let Some(parent) = current {
        if result.len() >= 256 {
            break;
        }
        if let Some(name) = names.get(&parent) {
            result.push(name.clone());
        }
        current = layout.nodes.get(&parent).and_then(|node| node.parent_id);
    }
    result.reverse();
    result
}

struct DocumentCandidate {
    edits: Vec<DocumentEdit>,
    changes: Vec<ReplaceChange>,
    blockers: Vec<ReplaceBlocker>,
}

struct ReplaceContext<'a> {
    document: &'a artifact::DocumentArtifact,
    template: &'a artifact::TemplateArtifact,
    path: &'a [String],
    needle: &'a str,
    replacement: &'a str,
    options: MatchOptions,
}

fn scalar_edit(
    context: &ReplaceContext<'_>,
    value: &artifact::FieldValue,
    scope: &str,
    label: &str,
) -> (
    Option<DocumentValueEdit>,
    Vec<ReplaceChange>,
    Option<ReplaceBlocker>,
) {
    let Some(original) = value.text() else {
        return (None, Vec::new(), None);
    };
    let replaced = replace_text(
        original,
        context.needle,
        context.replacement,
        context.options,
    );
    if replaced.ranges.is_empty() {
        return (None, Vec::new(), None);
    }
    let changes = change_rows(
        context.document,
        context.template,
        context.path,
        scope,
        label,
        original,
        &replaced.ranges,
        context.replacement,
    );
    if value.contains_unknown_storage_data() {
        return (
            None,
            changes,
            Some(blocker(
                Some(context.document),
                label,
                "보존해야 할 미지원 metadata가 있어 이 값을 손실 없이 바꿀 수 없습니다.",
            )),
        );
    }
    let edit = if replaced.value.is_empty() {
        DocumentValueEdit::unset()
    } else {
        DocumentValueEdit::single_line_text(replaced.value)
    };
    (Some(edit), changes, None)
}

fn rich_edit(
    context: &ReplaceContext<'_>,
    value: &artifact::FieldValue,
    scope: &str,
    label: &str,
) -> Reply<(
    Option<DocumentValueEdit>,
    Vec<ReplaceChange>,
    Option<ReplaceBlocker>,
)> {
    let Some(rich) = value.rich_text() else {
        return Ok((None, Vec::new(), None));
    };
    let (content, previews) =
        rich_replacement(rich, context.needle, context.replacement, context.options)?;
    if previews.is_empty() {
        return Ok((None, Vec::new(), None));
    }
    let changes = previews
        .iter()
        .flat_map(|(original, ranges)| {
            change_rows(
                context.document,
                context.template,
                context.path,
                scope,
                label,
                original,
                ranges,
                context.replacement,
            )
        })
        .collect();
    if rich.contains_unknown_storage_data() || value.contains_unknown_storage_data() {
        return Ok((
            None,
            changes,
            Some(blocker(
                Some(context.document),
                label,
                "보존해야 할 미지원 rich text metadata가 있어 이 값을 손실 없이 바꿀 수 없습니다.",
            )),
        ));
    }
    let normalized =
        normalize_rich_text(rich.schema_version(), &content).map_err(|_| Code::InvalidInput)?;
    Ok((
        Some(DocumentValueEdit::from_normalized_rich_text(normalized)),
        changes,
        None,
    ))
}

fn build_document_candidate(
    document: &artifact::DocumentArtifact,
    template: &artifact::TemplateArtifact,
    path: &[String],
    scopes: ReplaceScopes,
    needle: &str,
    replacement: &str,
    options: MatchOptions,
) -> Reply<DocumentCandidate> {
    let context = ReplaceContext {
        document,
        template,
        path,
        needle,
        replacement,
        options,
    };
    let mut edits = Vec::new();
    let mut changes = Vec::new();
    let mut blockers = Vec::new();

    for (enabled, scope, label, original, make_edit) in [
        (scopes.title, "title", "문서 제목", document.name(), 0_u8),
        (
            scopes.english_name,
            "english_name",
            "영어 이름",
            document.english_name(),
            1_u8,
        ),
        (
            scopes.glossary_summary,
            "glossary_summary",
            "한줄 설명",
            document.glossary_summary(),
            2_u8,
        ),
    ] {
        if !enabled {
            continue;
        }
        let replaced = replace_text(original, needle, replacement, options);
        if replaced.ranges.is_empty() {
            continue;
        }
        changes.extend(change_rows(
            document,
            template,
            path,
            scope,
            label,
            original,
            &replaced.ranges,
            replacement,
        ));
        edits.push(match make_edit {
            0 => DocumentEdit::Rename(replaced.value),
            1 => DocumentEdit::SetEnglishName(replaced.value),
            _ => DocumentEdit::SetGlossarySummary(replaced.value),
        });
    }

    if scopes.body {
        for (field_id, definition) in template.fields() {
            if definition.lifecycle() != FieldLifecycle::Active {
                continue;
            }
            let Some(value) = document.field_values().get(field_id) else {
                continue;
            };
            match definition.kind() {
                FieldKind::SingleLineText => {
                    let (edit, rows, blocked) =
                        scalar_edit(&context, value, "body", definition.label());
                    changes.extend(rows);
                    blockers.extend(blocked);
                    if let Some(edit) = edit {
                        edits.push(if edit.clone().into_field_value().is_unset() {
                            DocumentEdit::Unset(*field_id)
                        } else {
                            DocumentEdit::SetValue(*field_id, edit)
                        });
                    }
                }
                FieldKind::RichText => {
                    let (edit, rows, blocked) =
                        rich_edit(&context, value, "body", definition.label())?;
                    changes.extend(rows);
                    blockers.extend(blocked);
                    if let Some(edit) = edit {
                        let field_value = edit.clone().into_field_value();
                        edits.push(if field_value.is_unset() {
                            DocumentEdit::Unset(*field_id)
                        } else {
                            DocumentEdit::SetValue(*field_id, edit)
                        });
                    }
                }
                FieldKind::Group => {
                    let Some((_, members)) = definition.configuration().members() else {
                        continue;
                    };
                    let Some(group) = value.group() else {
                        continue;
                    };
                    let mut group_changed = false;
                    let mut instances = Vec::with_capacity(group.order.len());
                    for (index, instance_id) in group.order.iter().enumerate() {
                        let instance = &group.instances[instance_id];
                        let mut fields = Vec::new();
                        for (child_id, child_definition) in members {
                            if child_definition.lifecycle() != FieldLifecycle::Active {
                                continue;
                            }
                            let Some(child_value) = instance.values.get(child_id) else {
                                continue;
                            };
                            let label = format!(
                                "{} · {} · {}번째",
                                definition.label(),
                                child_definition.label(),
                                index + 1
                            );
                            let result = match child_definition.kind() {
                                FieldKind::RichText => {
                                    rich_edit(&context, child_value, "body", &label)
                                }
                                _ => continue,
                            }?;
                            changes.extend(result.1);
                            blockers.extend(result.2);
                            if let Some(edit) = result.0 {
                                group_changed = true;
                                let value = edit.into_field_value();
                                fields.push(CellInput {
                                    field: *child_id,
                                    value: if value.is_unset() {
                                        Intent::Unset
                                    } else {
                                        Intent::Set(value)
                                    },
                                });
                            }
                        }
                        instances.push(InstanceInput {
                            id: *instance_id,
                            source: Some(*instance_id),
                            fields,
                        });
                    }
                    if group_changed {
                        edits.push(DocumentEdit::SetGroup(*field_id, instances));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(DocumentCandidate {
        edits,
        changes,
        blockers,
    })
}

pub(super) fn build(
    ctx: &mut Context,
    _project: Id,
    preview_id: Id,
    scopes: ReplaceScopes,
    find: &str,
    replacement: &str,
    template_filter: Option<&str>,
    case_sensitive: bool,
    whole_word: bool,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<Preview> {
    if find.is_empty()
        || find.len() > 512
        || replacement.len() > 4096
        || !scopes.any()
        || validate_optional_single_line_text(find).is_err()
        || validate_optional_single_line_text(replacement).is_err()
    {
        return Err(Code::InvalidInput.into());
    }
    let template_filter = template_filter
        .map(convert::id::<artifact::TemplateId>)
        .transpose()?;
    let options = MatchOptions {
        case_sensitive,
        whole_word,
    };
    ctx.read(|ready| -> Reply<Preview> {
        let repository = ArtifactRepository::new(ready).map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?;
        let documents = Arc::new(repository.scan_documents().map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?);
        let loaded_layout = repository.load_layout().map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?;
        let layout_source = loaded_layout.as_ref().map(|loaded| loaded.source().clone());
        let ids: BTreeSet<_> = documents
            .summaries()
            .map(|(id, _, _, _, _, _)| id)
            .collect();
        let names: BTreeMap<_, _> = documents
            .summaries()
            .map(|(id, _, name, _, _, _)| (id, name.to_owned()))
            .collect();
        let layout = loaded_layout
            .map(|loaded| loaded.into_artifact())
            .unwrap_or_else(|| artifact::layout::DocumentLayout::flat(ids.iter().copied()));
        if layout.nodes.keys().any(|id| !ids.contains(id)) {
            return Err(Code::WrongBinding.into());
        }
        layout.reconcile(&ids).map_err(|error| {
            super::document_workspace::observed(observations, error, Code::RepositoryRejected)
        })?;

        let mut templates = BTreeMap::new();
        let mut changes = Vec::new();
        let mut blockers = Vec::new();
        let mut replacements = Vec::new();
        let mut targets = BTreeSet::new();
        let timestamp = super::timestamp()?;
        for (id, template_id, _, _, _, _) in documents.summaries() {
            crate::data::repository::progress::checkpoint(false).map_err(|error| {
                let code = if error.diagnostic().category
                    == crate::data::repository::RepositoryCategory::Cancelled
                {
                    Code::Cancelled
                } else {
                    Code::RepositoryRejected
                };
                super::document_workspace::observed(observations, error, code)
            })?;
            if template_filter.is_some_and(|filter| filter != template_id)
                || layout
                    .nodes
                    .get(&id)
                    .is_some_and(|node| node.state != artifact::layout::LayoutState::Active)
            {
                continue;
            }
            let loaded_document = repository.load_document(id).map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;
            if documents.source(id) != Some(loaded_document.source()) {
                return Err(Code::WrongBinding.into());
            }
            if !templates.contains_key(&template_id) {
                let loaded = repository.load_template(template_id).map_err(|error| {
                    super::document_workspace::observed(
                        observations,
                        error,
                        Code::RepositoryRejected,
                    )
                })?;
                templates.insert(template_id, loaded);
            }
            let template = templates[&template_id].artifact();
            let path = document_path(id, &layout, &names);
            let candidate = build_document_candidate(
                loaded_document.artifact(),
                template,
                &path,
                scopes,
                find,
                replacement,
                options,
            )?;
            if candidate.changes.is_empty() {
                continue;
            }
            targets.insert(id);
            changes.extend(candidate.changes);
            blockers.extend(candidate.blockers);
            if blockers
                .iter()
                .any(|blocker| blocker.document.as_deref() == Some(&id.to_string()))
            {
                continue;
            }
            match artifact::prepare_document_save(
                template,
                template.revision(),
                loaded_document.artifact(),
                &DocumentEditSet::new(candidate.edits),
                &timestamp,
            ) {
                Ok(outcome) if outcome.kind() == artifact::DocumentSaveOutcomeKind::Changed => {
                    replacements.push(DocumentReplacement {
                        source: loaded_document.source().clone(),
                        candidate: outcome.document().clone(),
                    });
                }
                Ok(_) => blockers.push(blocker(
                    Some(loaded_document.artifact()),
                    "문서",
                    "일치가 실제 저장 변경으로 이어지지 않습니다.",
                )),
                Err(_) => blockers.push(blocker(
                    Some(loaded_document.artifact()),
                    "문서",
                    "변경 결과가 기존 문서 검증을 통과하지 못했습니다.",
                )),
            }
        }

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
        if repository
            .load_layout()
            .map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?
            .as_ref()
            .map(|loaded| loaded.source())
            != layout_source.as_ref()
        {
            return Err(Code::WrongBinding.into());
        }
        repository
            .confirm_document_membership(
                &ids.iter()
                    .copied()
                    .map(ArtifactSourceId::Document)
                    .collect(),
            )
            .map_err(|error| {
                super::document_workspace::observed(observations, error, Code::RepositoryRejected)
            })?;

        let template_sources = templates
            .into_values()
            .map(|loaded| loaded.source().clone())
            .collect();
        let input = (!changes.is_empty() && blockers.is_empty()).then_some(BulkReplaceInput {
            replacements,
            documents,
            layout_source,
            template_sources,
        });
        Ok(Preview {
            id: preview_id,
            changes,
            blockers,
            targets,
            input,
        })
    })
    .map_err(|error| {
        super::document_workspace::observed(observations, error, Code::RuntimeRejected)
    })?
}

pub(super) fn response(preview: &Preview, offset: usize, limit: usize) -> Response {
    let changes = preview
        .changes
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    Response::ReplacePreview {
        preview: preview.id,
        offset,
        total_documents: preview.targets.len(),
        total_changes: preview.changes.len(),
        has_more: offset.saturating_add(changes.len()) < preview.changes.len(),
        changes,
        blockers: preview.blockers.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(case_sensitive: bool, whole_word: bool) -> MatchOptions {
        MatchOptions {
            case_sensitive,
            whole_word,
        }
    }

    #[test]
    fn literal_matching_is_non_overlapping_and_never_reprocesses_inserted_text() {
        let result = replace_text("aaaa", "aa", "aaa", options(true, false));
        assert_eq!(result.value, "aaaaaa");
        assert_eq!(result.ranges, vec![(0, 2), (2, 4)]);
        let symbols = replace_text("a.*[x]$1\\", ".*[x]$1\\", "ok", options(true, false));
        assert_eq!(symbols.value, "aok");
    }

    #[test]
    fn whole_word_uses_unicode_continue_characters() {
        assert!(match_ranges("아린 아린은", "아린", options(true, true)).eq(&[(0, 6)]));
        assert!(match_ranges("foo_bar foo-bar", "foo", options(true, true)).eq(&[(8, 11)]));
        assert!(match_ranges("e\u{301} e", "e", options(true, true)).eq(&[(4, 5)]));
    }

    #[test]
    fn unicode_lowercase_requires_original_character_boundaries() {
        assert_eq!(match_ranges("Ä ä", "ä", options(false, false)).len(), 2);
        assert!(match_ranges("İ", "i", options(false, false)).is_empty());
    }

    #[test]
    fn snippets_preserve_whitespace_and_show_each_actual_change() {
        let result = replace_text(" before Foo after ", "foo", "bar", options(false, false));
        assert_eq!(result.ranges, vec![(8, 11)]);
        let before = snippet_parts(" before Foo after ", 8, 11, None);
        let after = snippet_parts(" before Foo after ", 8, 11, Some("bar"));
        assert_eq!(
            (before.prefix, before.highlight, before.suffix),
            (" before ".into(), "Foo".into(), " after ".into())
        );
        assert_eq!(
            (after.prefix, after.highlight, after.suffix),
            (" before ".into(), "bar".into(), " after ".into())
        );
    }

    #[test]
    fn rich_inline_match_crosses_marks_and_uses_the_start_marks_only() {
        let mut root = serde_json::json!({
            "kind":"root",
            "children":[
                {"kind":"paragraph","children":[
                    {"kind":"text","text":"Old ","marks":["bold"]},
                    {"kind":"text","text":"Name","marks":["italic"]}
                ]},
                {"kind":"paragraph","children":[
                    {"kind":"text","text":"Old"},
                    {"kind":"hardBreak"},
                    {"kind":"text","text":"Name"}
                ]}
            ]
        })
        .as_object()
        .unwrap()
        .clone();
        let mut previews = Vec::new();
        replace_rich_node(
            &mut root,
            "Old Name",
            "New",
            options(true, false),
            &mut previews,
        )
        .unwrap();
        assert_eq!(
            previews.len(),
            1,
            "hardBreak and paragraph boundaries must stop matches"
        );
        let first = &root["children"][0]["children"];
        assert_eq!(first.as_array().unwrap().len(), 1);
        assert_eq!(first[0]["text"], "New");
        assert_eq!(first[0]["marks"], serde_json::json!(["bold"]));
        assert_eq!(root["children"][1]["children"][0]["text"], "Old");
    }
}
