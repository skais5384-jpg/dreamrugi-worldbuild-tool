use super::recovery_merge::Plan;
use super::*;
use crate::commands::document_workspace::EditBody;
use crate::data::edit_recovery::model::{Draft, DraftValue, Envelope, Intent, OriginalKind};

pub(super) fn body(draft: &Draft) -> Reply<EditBody> {
    let mut body = EditBody {
        name: Intent::Keep,
        english_name: Intent::Keep,
        glossary_summary: Intent::Keep,
        glossary_excluded: Intent::Keep,
        fields: vec![],
        composing: false,
    };
    match draft {
        Draft::Document {
            name,
            english_name,
            glossary_summary,
            glossary_excluded,
            fields,
            ..
        } => {
            body.name = name.clone();
            body.english_name = english_name.clone();
            body.glossary_summary = glossary_summary.clone();
            body.glossary_excluded = glossary_excluded.clone();
            body.fields = fields.clone();
        }
        Draft::AdmittedDocument { edits, .. } | Draft::AdmittedComposite { edits, .. } => {
            for edit in edits {
                match edit {
                    DocumentEdit::Rename { name } => body.name = Intent::Set(name.clone()),
                    DocumentEdit::EnglishName { value } => {
                        body.english_name = Intent::Set(value.clone())
                    }
                    DocumentEdit::GlossarySummary { value } => {
                        body.glossary_summary = Intent::Set(value.clone())
                    }
                    DocumentEdit::GlossaryExcluded { excluded } => {
                        body.glossary_excluded = Intent::Set(*excluded)
                    }
                    DocumentEdit::Set { field, value } => body.fields.push(DraftValue {
                        field: field.clone(),
                        value: Intent::Set(value.clone()),
                    }),
                    DocumentEdit::Unset { field } => body.fields.push(DraftValue {
                        field: field.clone(),
                        value: Intent::Unset,
                    }),
                }
            }
        }
        _ => return Err(Code::WrongBinding.into()),
    }
    Ok(body)
}

fn baseline(document: Option<&DocumentArtifact>, template: &TemplateArtifact) -> Reply<EditBody> {
    let mut fields = vec![];
    for (id, definition) in template.fields() {
        let value = match document {
            Some(d) => d.field_values().get(id).or_else(|| {
                (d.template_revision() < definition.introduced_revision())
                    .then(|| definition.initial_default_value())
            }),
            None => None,
        };
        fields.push(DraftValue {
            field: id.to_string(),
            value: Intent::Set(
                value
                    .map(|value| projection::field_value(value, Some(definition)))
                    .transpose()?
                    .unwrap_or(ValueDto::Unset {}),
            ),
        });
    }
    if let Some(document) = document {
        for (id, value) in document.field_values() {
            if !template.fields().contains_key(id) {
                fields.push(DraftValue {
                    field: id.to_string(),
                    value: Intent::Set(projection::value(value)?),
                });
            }
        }
    }
    Ok(EditBody {
        name: Intent::Set(document.map(|d| d.name()).unwrap_or("").into()),
        english_name: Intent::Set(document.map(|d| d.english_name()).unwrap_or("").into()),
        glossary_summary: Intent::Set(document.map(|d| d.glossary_summary()).unwrap_or("").into()),
        glossary_excluded: Intent::Set(document.is_some_and(|d| d.glossary_excluded())),
        fields,
        composing: false,
    })
}

fn resolved<T: Clone>(intent: &Intent<T>, base: &Intent<T>) -> Intent<T> {
    match intent {
        Intent::Keep => base.clone(),
        other => other.clone(),
    }
}

fn resolve_group(value: &mut ValueDto, base: Option<&ValueDto>) {
    let ValueDto::Group { instances } = value else {
        return;
    };
    let old = match base {
        Some(ValueDto::Group { instances }) => instances.as_slice(),
        _ => &[],
    };
    for card in instances {
        let previous = old
            .iter()
            .find(|item| item.id == card.source.unwrap_or(card.id));
        if let Some(previous) = previous {
            for field in &mut card.fields {
                if matches!(field.value, Intent::Keep) {
                    field.value = previous
                        .fields
                        .iter()
                        .find(|item| item.field == field.field)
                        .map(|item| item.value.clone())
                        .unwrap_or(Intent::Unset);
                }
            }
            for field in &previous.fields {
                if !card.fields.iter().any(|item| item.field == field.field) {
                    card.fields.push(field.clone());
                }
            }
        }
    }
}

pub(super) fn plan(
    envelope: &Envelope,
    current_template: &TemplateArtifact,
    current_document: Option<&DocumentArtifact>,
) -> Reply<Plan> {
    let original = envelope
        .originals
        .iter()
        .find(|o| o.kind == OriginalKind::Template)
        .ok_or(Code::WrongBinding)?;
    let old_template = artifact::decode_template(original.snapshot.as_bytes())
        .map_err(|_| Code::RecoveryRejected)?;
    if old_template.template_id() != current_template.template_id() {
        return Err(Code::WrongBinding.into());
    }
    let old_document = envelope
        .originals
        .iter()
        .find(|o| o.kind == OriginalKind::Document)
        .map(|o| {
            artifact::decode_document(o.snapshot.as_bytes()).map_err(|_| Code::RecoveryRejected)
        })
        .transpose()?;
    if let (Some(old), Some(current)) = (&old_document, current_document) {
        if old.document_id() != current.document_id() || old.template_id() != current.template_id()
        {
            return Err(Code::WrongBinding.into());
        }
    }
    let base = baseline(old_document.as_ref(), &old_template)?;
    let current = baseline(current_document, current_template)?;
    let mut input = body(&envelope.draft)?;
    super::workspace::recovery_template::map_composite_document(
        envelope,
        current_template,
        &mut input,
    )?;
    let mut effective = base.clone();
    effective.name = resolved(&input.name, &base.name);
    effective.english_name = resolved(&input.english_name, &base.english_name);
    effective.glossary_summary = resolved(&input.glossary_summary, &base.glossary_summary);
    effective.glossary_excluded = resolved(&input.glossary_excluded, &base.glossary_excluded);
    for field in &input.fields {
        let old = base.fields.iter().find(|f| f.field == field.field);
        let mut semantic = field.value.clone();
        if let Intent::Set(value) = &mut semantic {
            let previous = old.and_then(|f| match &f.value {
                Intent::Set(value) => Some(value),
                _ => None,
            });
            resolve_group(value, previous);
        }
        let desired = match &semantic {
            Intent::Keep => old.map(|f| f.value.clone()).unwrap_or(Intent::Keep),
            Intent::Unset => Intent::Set(ValueDto::Unset {}),
            other => other.clone(),
        };
        if let Some(to) = effective.fields.iter_mut().find(|f| f.field == field.field) {
            to.value = desired;
        } else {
            effective.fields.push(DraftValue {
                field: field.field.clone(),
                value: desired,
            });
        }
    }
    // Keep can reuse a native instance only while that current source exists.
    // For a deleted known source, recover its validated old values explicitly;
    // opaque source loss remains blocked by the provenance checks below.
    for field in &mut input.fields {
        let Intent::Set(ValueDto::Group { instances }) = &mut field.value else {
            continue;
        };
        let previous = base
            .fields
            .iter()
            .find(|f| f.field == field.field)
            .and_then(|f| match &f.value {
                Intent::Set(value) => Some(value),
                _ => None,
            });
        let current_cards = current
            .fields
            .iter()
            .find(|f| f.field == field.field)
            .and_then(|f| match &f.value {
                Intent::Set(ValueDto::Group { instances }) => Some(instances),
                _ => None,
            });
        for card in instances {
            if current_cards.is_none_or(|cards| {
                !cards
                    .iter()
                    .any(|now| now.id == card.id || Some(now.id) == card.source)
            }) {
                let mut value = ValueDto::Group {
                    instances: vec![card.clone()],
                };
                resolve_group(&mut value, previous);
                if let ValueDto::Group { mut instances } = value {
                    *card = instances.remove(0);
                }
            }
        }
    }
    let mut current_raw = current.clone();
    current_raw.name = Intent::Keep;
    current_raw.english_name = Intent::Keep;
    current_raw.glossary_summary = Intent::Keep;
    current_raw.glossary_excluded = Intent::Keep;
    for field in &mut current_raw.fields {
        if let Intent::Set(ValueDto::Group { instances }) = &mut field.value {
            for instance in instances {
                for child in &mut instance.fields {
                    child.value = Intent::Keep;
                }
            }
        } else {
            field.value = Intent::Keep;
        }
    }
    let value = |body: &EditBody| -> Reply<serde_json::Value> {
        serde_json::to_value(body).map_err(|_| Code::SerializationFailed.into())
    };
    let mut plan = Plan::new(
        value(&base)?,
        value(&current)?,
        value(&effective)?,
        value(&current_raw)?,
        value(&input)?,
    );
    for change in &mut plan.changes {
        if change.path.first().is_some_and(|key| key == "fields") && change.path.len() > 2 {
            let field = convert::id(&change.path[2]).ok();
            let definition = field.and_then(|id| current_template.fields().get(&id));
            if definition.is_none_or(|f| f.lifecycle() != artifact::FieldLifecycle::Active) {
                change.status = "blocked";
                change.reason=Some(if definition.is_some(){"보관된 정의를 템플릿에서 복원한 뒤 다시 비교하세요."}else{"현재 템플릿에 정의가 없습니다. 관련 템플릿 변경을 먼저 회수하거나 남은 내용을 확인하세요."}.into());
            }

            if let Some(Intent::Set(ValueDto::Group { instances })) = input
                .fields
                .iter()
                .find(|f| Some(f.field.as_str()) == change.path.get(2).map(String::as_str))
                .map(|f| &f.value)
            {
                let card_position = change.path.iter().position(|key| key == "instances");
                let whole_card = card_position.is_none()
                    || card_position.is_some_and(|position| change.path.len() == position + 3);
                if whole_card {
                    for card in instances.iter().filter(|card| {
                        card_position.is_none_or(|position| {
                            Some(card.id.to_string()) == change.path.get(position + 2).cloned()
                        })
                    }) {
                        for child in card
                            .fields
                            .iter()
                            .filter(|child| !matches!(child.value, Intent::Keep))
                        {
                            let member = child.field.parse().ok().and_then(|id| {
                                definition
                                    .and_then(|d| d.configuration().members())
                                    .and_then(|(_, members)| members.get(&id))
                            });
                            if member.is_none_or(|member| {
                                member.lifecycle() != artifact::FieldLifecycle::Active
                            }) {
                                change.status = "blocked";
                                change.reason=Some("그룹의 하위 정의를 먼저 회수·복원한 뒤 다시 비교하세요. 나머지 항목은 선택해 회수할 수 있습니다.".into());
                            }
                        }
                    }
                }
            }
            let old_value = old_document
                .as_ref()
                .and_then(|d| field.and_then(|id| d.field_values().get(&id)));
            let current_value =
                current_document.and_then(|d| field.and_then(|id| d.field_values().get(&id)));
            if old_value.is_some_and(|v| v.contains_unknown_storage_data())
                && (current_value.is_none()
                    || (old_value.is_some_and(|v| {
                        matches!(
                            v.validation_view(),
                            crate::data::field_engine::validation::FieldValueView::Group(_)
                        )
                    }) && current_value.is_none_or(|v| {
                        !matches!(
                            v.validation_view(),
                            crate::data::field_engine::validation::FieldValueView::Group(_)
                        )
                    })))
            {
                change.status = "blocked";
                change.reason=Some("원문에 현재 앱이 해석하지 못하는 정보가 있습니다. 출처가 없는 위치에 다시 만들지 않고 원 보관본에 유지합니다. 나머지 항목은 선택해 회수할 수 있습니다.".into());
            }
            if let Some(crate::data::field_engine::validation::FieldValueView::Group(old)) =
                old_value.map(|v| v.validation_view())
            {
                let now = match current_value.map(|v| v.validation_view()) {
                    Some(crate::data::field_engine::validation::FieldValueView::Group(group)) => {
                        Some(group)
                    }
                    _ => None,
                };
                if !change.path.iter().any(|key| key == "instances") {
                    if let Some(Intent::Set(ValueDto::Group { instances })) = input
                        .fields
                        .iter()
                        .find(|f| Some(f.field.as_str()) == change.path.get(2).map(String::as_str))
                        .map(|f| &f.value)
                    {
                        for card in instances {
                            let source = card.source.unwrap_or(card.id);
                            if old
                                .instances
                                .get(&source)
                                .is_some_and(|instance| instance.contains_unknown_storage_data())
                                && now.is_none_or(|group| {
                                    !group.instances.contains_key(&card.id)
                                        && !group.instances.contains_key(&source)
                                })
                            {
                                change.status = "blocked";
                                change.reason=Some("지원하지 않는 카드 정보의 현재 출처가 없습니다. 원 보관본을 유지하고 다른 항목을 회수하세요.".into());
                            }
                        }
                    }
                }
                if let Some(position) = change.path.iter().position(|key| key == "instances") {
                    if change
                        .path
                        .get(position + 1)
                        .is_some_and(|key| key == "$items")
                    {
                        if let Some(card) = change.path.get(position + 2).and_then(|id| {
                            let id = id.parse().ok()?;
                            let source = input
                                .fields
                                .iter()
                                .find(|f| {
                                    Some(f.field.as_str()) == change.path.get(2).map(String::as_str)
                                })
                                .and_then(|field| match &field.value {
                                    Intent::Set(ValueDto::Group { instances }) => instances
                                        .iter()
                                        .find(|card| card.id == id)
                                        .map(|card| card.source.unwrap_or(card.id)),
                                    _ => None,
                                })
                                .unwrap_or(id);
                            old.instances.get(&source).map(|card| (id, source, card))
                        }) {
                            if card.2.contains_unknown_storage_data()
                                && now.is_none_or(|group| {
                                    !group.instances.contains_key(&card.0)
                                        && !group.instances.contains_key(&card.1)
                                })
                            {
                                change.status = "blocked";
                                change.reason=Some("이 카드의 미지원 정보는 원 보관본에 유지됩니다. 현재 카드에 출처가 없어 손실 없이 다시 만들 수 없으므로 다른 항목을 회수하세요.".into());
                            }
                        }
                        if let Some(child_position) = change
                            .path
                            .iter()
                            .enumerate()
                            .skip(position + 3)
                            .find_map(|(i, k)| (k == "fields").then_some(i))
                        {
                            let child = change
                                .path
                                .get(child_position + 2)
                                .and_then(|id| id.parse().ok())
                                .and_then(|id| {
                                    definition
                                        .and_then(|d| d.configuration().members())
                                        .and_then(|members| members.1.get(&id))
                                });
                            if child.is_none_or(|child| {
                                child.lifecycle() != artifact::FieldLifecycle::Active
                            }) {
                                change.status = "blocked";
                                change.reason=Some("그룹의 하위 정의를 먼저 회수·복원한 뒤 다시 비교하세요. 원 보관본은 유지됩니다.".into());
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(document) = current_document {
        let assessment = artifact::reconcile_document(current_template, document)
            .map_err(|_| Code::RepositoryRejected)?;
        if assessment.blocking_issues().iter().any(|issue| {
            matches!(
                issue.category(),
                artifact::DocumentReconciliationIssueCategory::OrphanSnapshotMissing
                    | artifact::DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict
                    | artifact::DocumentReconciliationIssueCategory::LossyOrphanReattachment
                    | artifact::DocumentReconciliationIssueCategory::OptionSnapshotUnavailable
                    | artifact::DocumentReconciliationIssueCategory::LossySnapshotMembership
            )
        }) {
            for change in &mut plan.changes {
                change.status = "blocked";
                change.reason=Some("현재 문서의 정의 기록이 누락되거나 손실 없이 연결할 수 없는 정보가 있어 정상 저장할 수 없습니다. 원 보관본은 유지됩니다. 아래 보관 내용을 읽고 복사해 새 문서에 회수하거나 정의를 복원한 뒤 다시 비교하세요.".into());
            }
        }
    }
    Ok(plan)
}

pub(super) fn bind_groups(
    body: &mut EditBody,
    current: &DocumentArtifact,
    template: &TemplateArtifact,
) -> Reply<()> {
    for field in &mut body.fields {
        let Intent::Set(ValueDto::Group { instances }) = &mut field.value else {
            continue;
        };
        let id = convert::id(&field.field)?;
        let current = match current
            .field_values()
            .get(&id)
            .map(|v| projection::field_value(v, template.fields().get(&id)))
            .transpose()?
        {
            Some(ValueDto::Group { instances }) => instances,
            _ => vec![],
        };
        for card in instances {
            let latest = current
                .iter()
                .find(|candidate| candidate.id == card.id || Some(candidate.id) == card.source);
            card.source = latest.map(|candidate| candidate.id);
            card.lineage.clear();
            if let Some(latest) = latest {
                card.labels = latest.labels.clone();
                card.protected = latest.protected.clone();
            }
        }
    }
    Ok(())
}

pub(super) fn apply(plan: &Plan, selected: &[String]) -> Reply<EditBody> {
    let mut body: EditBody =
        serde_json::from_value(plan.apply(selected).map_err(|_| Code::WrongBinding)?)
            .map_err(|_| Code::InvalidInput)?;
    for field in &mut body.fields {
        if !plan.changes.iter().any(|change| {
            selected.contains(&change.id)
                && change.path.len() > 2
                && change.path[0] == "fields"
                && change.path[2] == field.field
        }) {
            field.value = Intent::Keep;
        }
    }
    Ok(body)
}
