use super::super::recovery_merge::Plan;
use super::*;

fn intent<T: Clone>(input: &Intent<T>, current: Option<T>) -> Intent<T> {
    match input {
        Intent::Keep => current.map(Intent::Set).unwrap_or(Intent::Unset),
        other => other.clone(),
    }
}

// Resolve Keep against the preserved origin for comparison only. The raw current
// body still uses Keep so untouched opaque defaults stay owned by the canonical source.
fn semantic(body: &TemplateBody, source: &TemplateArtifact) -> Reply<TemplateBody> {
    let mut result = body.clone();
    result.presentation = intent(
        &body.presentation,
        source.presentation().token().map(str::to_owned),
    );
    for field in &mut result.fields {
        let original = convert::id(&field.id)
            .ok()
            .and_then(|id| source.fields().get(&id));
        field.presentation = intent(
            &field.presentation,
            original
                .and_then(|f| f.presentation().token())
                .map(str::to_owned),
        );
        field.writing_guide = Some(intent(
            field.writing_guide.as_ref().unwrap_or(&Intent::Keep),
            original.and_then(|f| f.writing_guide()).map(str::to_owned),
        ));
        field.default = match &field.default {
            Intent::Keep => original
                .map(|f| projection::value(f.default_value()))
                .transpose()?
                .map(Intent::Set)
                .unwrap_or(Intent::Unset),
            other => other.clone(),
        };
        if let DraftConfiguration::Group {
            members,
            card_title_field,
        } = &mut field.configuration
        {
            *card_title_field = intent(
                card_title_field,
                original
                    .and_then(|f| f.presentation().card_title_field())
                    .map(|id| id.to_string()),
            );
            let scope =
                source.member_scope(convert::id(&field.id).unwrap_or_else(|_| FieldId::new()));
            let child = semantic(
                &TemplateBody {
                    sections: vec![],
                    name: String::new(),
                    glossary_excluded: false,
                    presentation: Intent::Keep,
                    fields: members.clone(),
                    composing: false,
                },
                &scope,
            )?;
            *members = child.fields;
        }
    }
    Ok(result)
}

pub(super) fn plan(
    body: &TemplateBody,
    original: &TemplateArtifact,
    current: &TemplateArtifact,
) -> Reply<Plan> {
    let base = body_from_source(original)?;
    let now = body_from_source(current)?;
    let value = |body: &TemplateBody| -> Reply<serde_json::Value> {
        serde_json::to_value(body).map_err(|_| Code::SerializationFailed.into())
    };
    let mut plan = Plan::new(
        value(&semantic(&base, original)?)?,
        value(&semantic(&now, current)?)?,
        value(&semantic(body, original)?)?,
        value(&now)?,
        value(body)?,
    );
    for change in &mut plan.changes {
        if change.path.first().is_some_and(|key| key == "fields")
            && change.path.get(1).is_some_and(|key| key == "$items")
        {
            let raw = change.path.get(2).ok_or(Code::WrongBinding)?;
            if raw.starts_with("new:") {
                continue;
            }
            let field = convert::id(raw)
                .ok()
                .and_then(|id| current.fields().get(&id));
            if field.is_none() {
                change.status = "blocked";
                change.reason=Some("현재 템플릿에 대응하는 정의가 없습니다. 원 보관본을 남겨 두고 나머지 항목을 회수할 수 있습니다.".into());
            } else if field
                .is_some_and(|field| field.lifecycle() == artifact::FieldLifecycle::Archived)
                && change.path.last().is_none_or(|key| key != "archived")
            {
                change.status = "blocked";
                change.reason = Some(
                    "보관된 필드를 먼저 복원한 뒤 다시 비교하세요. 원 보관본은 유지됩니다.".into(),
                );
            }
            if let Some(member_position) = change.path.iter().enumerate().find_map(|(i, key)| {
                (key == "members" && change.path.get(i + 1).is_some_and(|key| key == "$items"))
                    .then_some(i)
            }) {
                let raw = change
                    .path
                    .get(member_position + 2)
                    .ok_or(Code::WrongBinding)?;
                if !raw.starts_with("new:") {
                    let member = raw.parse().ok().and_then(|id| {
                        field
                            .and_then(|f| f.configuration().members())
                            .and_then(|(_, members)| members.get(&id))
                    });
                    if member.is_none()
                        || (member
                            .is_some_and(|f| f.lifecycle() == artifact::FieldLifecycle::Archived)
                            && change.path.last().is_none_or(|key| key != "archived"))
                    {
                        change.status = "blocked";
                        change.reason=Some("하위 필드를 먼저 복원하거나 관련 템플릿 변경을 회수한 뒤 다시 비교하세요.".into());
                    }
                }
            }
            if let Some(option_position) = change.path.iter().enumerate().find_map(|(i, key)| {
                (key == "options" && change.path.get(i + 1).is_some_and(|key| key == "$items"))
                    .then_some(i)
            }) {
                let raw = change
                    .path
                    .get(option_position + 2)
                    .ok_or(Code::WrongBinding)?;
                if !raw.starts_with("new:") {
                    let option = raw.parse().ok().and_then(|id| {
                        field
                            .and_then(|f| f.configuration().options())
                            .and_then(|options| options.get(&id))
                    });
                    if option.is_none()
                        || (option
                            .is_some_and(|f| f.lifecycle() == artifact::OptionLifecycle::Archived)
                            && change.path.last().is_none_or(|key| key != "archived"))
                    {
                        change.status = "blocked";
                        change.reason = Some("선택지를 먼저 복원한 뒤 다시 비교하세요.".into());
                    }
                }
            }
            if change.path.last().is_some_and(|key| key == "kind") {
                change.status = "blocked";
                change.reason =
                    Some("저장된 필드의 타입은 변경할 수 없습니다. 원 보관본은 유지됩니다.".into());
            }
        }
    }
    Ok(plan)
}

pub(in crate::commands::backend) fn composite_preview(envelope: &Envelope) -> Reply<TemplateBody> {
    let crate::data::edit_recovery::model::Draft::AdmittedComposite { edit, .. } = &envelope.draft
    else {
        return Err(Code::WrongBinding.into());
    };
    let original = envelope
        .originals
        .iter()
        .find(|o| o.kind == crate::data::edit_recovery::model::OriginalKind::Template)
        .ok_or(Code::WrongBinding)?;
    let source = artifact::decode_template(original.snapshot.as_bytes())
        .map_err(|_| Code::RecoveryRejected)?;
    let candidate = convert::intent(edit)?
        .recovery_candidate(&source, &timestamp()?)
        .map_err(|_| Code::PreparationRejected)?;
    let mut body = semantic(&body_from_source(&candidate)?, &candidate)?;
    let mut mappings = std::collections::BTreeMap::new();
    collect_new_ids(&body.fields, &source, &mut mappings)?;
    remap_template(&mut body, &mappings);
    Ok(body)
}

fn collect_new_ids(
    fields: &[DraftField],
    source: &TemplateArtifact,
    map: &mut std::collections::BTreeMap<String, String>,
) -> Reply<()> {
    for field in fields {
        let id: FieldId = convert::id(&field.id)?;
        let original = source.fields().get(&id);
        if original.is_none() {
            map.insert(field.id.clone(), format!("new:{}", field.id));
        }
        if let DraftConfiguration::SingleChoice { options }
        | DraftConfiguration::MultiChoice { options } = &field.configuration
        {
            for option in options {
                if original
                    .and_then(|f| f.configuration().options())
                    .is_none_or(|options| {
                        option
                            .id
                            .parse()
                            .ok()
                            .is_none_or(|id| !options.contains_key(&id))
                    })
                {
                    map.insert(option.id.clone(), format!("new:{}", option.id));
                }
            }
        }
        if let DraftConfiguration::Group { members, .. } = &field.configuration {
            collect_new_ids(members, &source.member_scope(id), map)?;
        }
    }
    Ok(())
}
fn mapped(raw: &str, map: &std::collections::BTreeMap<String, String>) -> String {
    map.get(raw).cloned().unwrap_or_else(|| raw.into())
}
fn remap_value(value: &mut ValueDto, map: &std::collections::BTreeMap<String, String>) {
    match value {
        ValueDto::SingleChoice { option } => *option = mapped(option, map),
        ValueDto::MultiChoice { options } => {
            for option in options {
                *option = mapped(option, map);
            }
        }
        ValueDto::Group { instances } => {
            for card in instances {
                for field in &mut card.fields {
                    field.field = mapped(&field.field, map);
                    if let Intent::Set(value) = &mut field.value {
                        remap_value(value, map);
                    }
                }
            }
        }
        _ => (),
    }
}
fn remap_template(body: &mut TemplateBody, map: &std::collections::BTreeMap<String, String>) {
    fn fields(items: &mut [DraftField], map: &std::collections::BTreeMap<String, String>) {
        for field in items {
            field.id = mapped(&field.id, map);
            if let Intent::Set(value) = &mut field.default {
                remap_value(value, map);
            }
            match &mut field.configuration {
                DraftConfiguration::SingleChoice { options }
                | DraftConfiguration::MultiChoice { options } => {
                    for option in options {
                        option.id = mapped(&option.id, map);
                    }
                }
                DraftConfiguration::Group {
                    members,
                    card_title_field,
                } => {
                    fields(members, map);
                    if let Intent::Set(title) = card_title_field {
                        *title = mapped(title, map);
                    }
                }
                _ => (),
            }
        }
    }
    fields(&mut body.fields, map);
    for section in &mut body.sections {
        if let Some(field) = &mut section.before_field {
            *field = mapped(field, map);
        }
    }
}
// A staged component uses the same deterministic identity allocation as the whole
// template owner. Missing definitions remain blocked; no name-based matching occurs.
pub(in crate::commands::backend) fn map_composite_document(
    envelope: &Envelope,
    current: &TemplateArtifact,
    body: &mut crate::commands::document_workspace::EditBody,
) -> Reply<()> {
    if !matches!(
        envelope.draft,
        crate::data::edit_recovery::model::Draft::AdmittedComposite { .. }
    ) {
        return Ok(());
    }
    let preview = composite_preview(envelope)?;
    let mut map = std::collections::BTreeMap::new();
    fn collect(
        fields: &[DraftField],
        draft: &str,
        map: &mut std::collections::BTreeMap<String, String>,
    ) -> Reply<()> {
        for field in fields {
            let canonical = if field.id.starts_with("new:") {
                super::allocated_id(draft, "field", &field.id)?
            } else {
                field.id.clone()
            };
            if let Some(raw) = field.id.strip_prefix("new:") {
                map.insert(raw.into(), canonical.clone());
            }
            match &field.configuration {
                DraftConfiguration::SingleChoice { options }
                | DraftConfiguration::MultiChoice { options } => {
                    for option in options {
                        if let Some(raw) = option.id.strip_prefix("new:") {
                            map.insert(
                                raw.into(),
                                super::allocated_id(
                                    draft,
                                    &format!("option/{canonical}"),
                                    &option.id,
                                )?,
                            );
                        }
                    }
                }
                DraftConfiguration::Group { members, .. } => collect(members, draft, map)?,
                _ => (),
            }
        }
        Ok(())
    }
    collect(&preview.fields, &envelope.key.draft_id, &mut map)?;
    // Allocation does not assert that a component has been saved; plan() validates
    // each actual current definition and blocks absent or archived targets.
    let _ = current;
    for field in &mut body.fields {
        field.field = mapped(&field.field, &map);
        if let Intent::Set(value) = &mut field.value {
            remap_value(value, &map);
        }
    }
    Ok(())
}

pub(super) fn restoration_intents(body: &mut TemplateBody, source: &TemplateArtifact) -> Reply<()> {
    for field in &mut body.fields {
        let original = convert::id(&field.id)
            .ok()
            .and_then(|id| source.fields().get(&id));
        field.restore = !field.archived
            && original.is_some_and(|f| f.lifecycle() == artifact::FieldLifecycle::Archived);
        match &mut field.configuration {
            DraftConfiguration::Group { members, .. } => {
                let mut child = TemplateBody {
                    sections: vec![],
                    name: String::new(),
                    glossary_excluded: false,
                    presentation: Intent::Keep,
                    fields: members.clone(),
                    composing: false,
                };
                if let Some(id) = convert::id(&field.id).ok() {
                    restoration_intents(&mut child, &source.member_scope(id))?;
                }
                *members = child.fields;
            }
            DraftConfiguration::SingleChoice { options }
            | DraftConfiguration::MultiChoice { options } => {
                for option in options {
                    option.restore = !option.archived
                        && original
                            .and_then(|f| f.configuration().options())
                            .and_then(|map| option.id.parse().ok().and_then(|id| map.get(&id)))
                            .is_some_and(|option| {
                                option.lifecycle() == artifact::OptionLifecycle::Archived
                            });
                }
            }
            _ => (),
        }
    }
    Ok(())
}
