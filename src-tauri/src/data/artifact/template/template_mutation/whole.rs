//! 전체 초안의 최종 상태를 한 private 후보에 조립한다. 중간 상태는 공개 revision이 아니다.
use super::*;

#[derive(Clone)]
pub(crate) struct TemplateDraftInput {
    pub(crate) sections: Vec<super::super::sections::SectionInput>,
    pub(crate) name: String,
    pub(crate) glossary_excluded: bool,
    pub(crate) presentation_token: Option<String>,
    /// 배열은 active Field의 최종 순서와 보관된 정의를 함께 담는다.
    pub(crate) fields: Vec<FieldDraftInput>,
}

#[derive(Clone)]
pub(crate) struct FieldDraftInput {
    pub(crate) members: Vec<FieldDraftInput>,
    pub(crate) card_title_field: crate::data::edit_recovery::model::Intent<FieldId>,
    pub(crate) id: FieldId,
    pub(crate) label: String,
    pub(crate) kind: FieldKind,
    pub(crate) configuration: NewFieldConfiguration,
    pub(crate) required: bool,
    pub(crate) writing_guide: Option<String>,
    pub(crate) presentation_token: Option<String>,
    /// None은 이 backend 원본의 current default를 그대로 유지한다.
    pub(crate) default: Option<FieldValueDraft>,
    pub(crate) archived: bool,
    pub(crate) restore: bool,
    pub(crate) restored_options: BTreeSet<OptionId>,
    pub(crate) archived_options: BTreeSet<OptionId>,
}

pub(crate) fn prepare_template_draft(
    source: &TemplateArtifact,
    revision: TemplateRevision,
    timestamp: &str,
    draft: TemplateDraftInput,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    admit(source, revision, timestamp)?;
    // 최대 revision에서도 순효과 없는 초안은 NoWrite다. 실제 변화의 overflow는 finish에서 거부한다.
    let introduction = source
        .revision
        .checked_increment()
        .unwrap_or(source.revision);
    let restoring = restorations(source, &draft)?;
    let candidate = assemble(source, draft, introduction)?;
    finish_candidate_with_restorations(source, candidate, timestamp, false, &restoring)
}

pub(crate) fn unpublished_seed(
    issued_id: crate::data::artifact::TemplateId,
    timestamp: String,
) -> Result<TemplateArtifact, crate::data::artifact::TemplateCreationError> {
    let mut seed = crate::data::artifact::create_template(String::new(), None, timestamp)?;
    seed.template_id = issued_id;
    Ok(seed)
}

/// 생성 registry가 소유한 미공개 빈 seed에 최종 정의를 넣는다. 디스크의 빈 Template를 만들지 않는다.
pub(crate) fn prepare_new_template_draft(
    seed: &TemplateArtifact,
    timestamp: &str,
    draft: TemplateDraftInput,
) -> Result<TemplateArtifact, TemplateMutationError> {
    admit(seed, TemplateRevision::INITIAL, timestamp)?;
    if !seed.fields.is_empty() || seed.revision != TemplateRevision::INITIAL {
        return Err(TemplateMutationError::unexpected_mutation_state());
    }
    let mut candidate = assemble(seed, draft, TemplateRevision::INITIAL)?;
    candidate.created_at_utc = timestamp.to_owned();
    candidate.updated_at_utc = timestamp.to_owned();
    candidate
        .validate_storage()
        .map_err(TemplateMutationError::invalid_candidate)?;
    candidate
        .rebase_lossless_source()
        .map_err(TemplateMutationError::invalid_candidate)?;
    candidate.default_draft_snapshot = DefaultDraftSnapshot::new();
    Ok(candidate)
}

fn admit(
    source: &TemplateArtifact,
    revision: TemplateRevision,
    timestamp: &str,
) -> Result<(), TemplateMutationError> {
    source
        .validate_storage()
        .map_err(TemplateMutationError::invalid_source)?;
    if source.revision != revision {
        return Err(TemplateMutationError::revision_mismatch());
    }
    validate_requested_timestamp(source, timestamp)?;
    if source.lifecycle == TemplateLifecycle::Deleted {
        return Err(TemplateMutationError::template_is_tombstoned(
            source.template_id,
        ));
    }
    Ok(())
}

// Only whole-draft inputs carry this explicit intent. Ordinary mutations retain the
// historical prohibition. Identity, ownership and revision admission are unchanged.
fn restorations(
    source: &TemplateArtifact,
    draft: &TemplateDraftInput,
) -> Result<Restorations, TemplateMutationError> {
    let mut result = Restorations::default();
    for input in &draft.fields {
        let original = source.fields.get(&input.id);
        if input.restore {
            if input.archived || original.is_none_or(|f| f.lifecycle != FieldLifecycle::Archived) {
                return Err(TemplateMutationError::invalid_field_draft(input.id));
            }
            result.fields.insert(input.id);
        }
        for id in &input.restored_options {
            if input.archived_options.contains(id)
                || original
                    .and_then(|f| f.configuration.options())
                    .and_then(|o| o.get(id))
                    .is_none_or(|o| o.lifecycle != OptionLifecycle::Archived)
            {
                return Err(TemplateMutationError::invalid_option_draft(input.id, *id));
            }
            result.options.insert((input.id, *id));
        }
    }
    Ok(result)
}

fn assemble(
    source: &TemplateArtifact,
    draft: TemplateDraftInput,
    introduction: TemplateRevision,
) -> Result<TemplateArtifact, TemplateMutationError> {
    let restoring = restorations(source, &draft)?;
    let mut candidate = source.clone();
    candidate.sections = super::super::sections::assemble(&source.sections, draft.sections)
        .map_err(TemplateMutationError::invalid_candidate)?;
    candidate.name = draft.name;
    candidate.glossary_excluded = draft.glossary_excluded;
    candidate.presentation.token = draft.presentation_token;
    candidate.field_order.clear();
    let mut seen = BTreeSet::new();
    for input in draft.fields {
        let id = input.id;
        if !seen.insert(id) {
            return Err(TemplateMutationError::invalid_field_order());
        }
        let original = source.fields.get(&id);
        let members = input.members;
        if input.kind != FieldKind::Group && !members.is_empty() {
            return Err(TemplateMutationError::invalid_field_draft(id));
        }
        let mut configuration = build_new_configuration(id, input.configuration)?;
        if input.kind == FieldKind::Group {
            let mut child_source = source.clone();
            child_source.sections.clear();
            let (order, fields) = original
                .and_then(|f| f.configuration.members())
                .map(|(o, f)| (o.to_vec(), f.clone()))
                .unwrap_or_default();
            child_source.field_order = order;
            child_source.fields = fields;
            if members.iter().any(|f| f.kind == FieldKind::Group) {
                return Err(TemplateMutationError::invalid_field_draft(id));
            }
            let children = assemble(
                &child_source,
                TemplateDraftInput {
                    sections: vec![],
                    name: child_source.name.clone(),
                    glossary_excluded: child_source.glossary_excluded,
                    presentation_token: child_source.presentation.token.clone(),
                    fields: members,
                },
                introduction,
            )?;
            configuration.variant = FieldConfigurationVariant::Group {
                member_order: children.field_order,
                members: children.fields,
            };
        }
        if configuration.kind() != input.kind {
            return Err(TemplateMutationError::invalid_field_draft(id));
        }
        if let Some(field) = original {
            if input.kind != field.kind {
                return Err(TemplateMutationError::immutable_field_changed(id));
            }
            if let Some(value) = &input.default {
                admit_default_draft_ownership(
                    source,
                    &TemplateMutationCommand::set_current_default(id, value.clone()),
                )?;
            }
            configuration.extra = field.configuration.extra.clone();
        } else if input.archived
            || input
                .default
                .as_ref()
                .is_none_or(|v| !matches!(v.origin, FieldValueDraftOrigin::Fresh))
        {
            return Err(TemplateMutationError::invalid_field_draft(id));
        }
        match &mut configuration.variant {
            FieldConfigurationVariant::SingleChoice {
                option_order,
                options,
            }
            | FieldConfigurationVariant::MultiChoice {
                option_order,
                options,
            } => {
                for archived in &input.archived_options {
                    let option = options
                        .get_mut(archived)
                        .ok_or_else(|| TemplateMutationError::invalid_option_order(id))?;
                    if original
                        .and_then(|f| f.configuration.options())
                        .is_none_or(|o| !o.contains_key(archived))
                    {
                        return Err(TemplateMutationError::invalid_option_draft(id, *archived));
                    }
                    option.lifecycle = OptionLifecycle::Archived;
                }
                option_order.retain(|id| !input.archived_options.contains(id));
                if let Some(old) = original.and_then(|f| f.configuration.options()) {
                    for (oid, option) in options {
                        if let Some(old_option) = old.get(oid) {
                            option.extra = old_option.extra.clone();
                        }
                    }
                }
            }
            _ if !input.archived_options.is_empty() => {
                return Err(TemplateMutationError::invalid_option_order(id))
            }
            _ => (),
        }
        let validate_new_default = input.default.is_some();
        let default = if let Some(old) = original {
            match input.default {
                Some(value) => prepare_current_default_value(&old.default_value, value)
                    .map_err(|()| TemplateMutationError::immutable_field_changed(id))?,
                None => old.default_value.clone(),
            }
        } else {
            let value = input
                .default
                .ok_or_else(|| TemplateMutationError::invalid_field_draft(id))?
                .into_value();
            if value.contains_unknown_storage_data() {
                return Err(TemplateMutationError::invalid_field_draft(id));
            }
            value
        };
        let mut definition = if let Some(old) = original {
            let mut field = old.clone();
            field.label = input.label;
            field.writing_guide = input.writing_guide;
            field.required = input.required;
            field.presentation.token = input.presentation_token;
            field.default_value = default;
            field.configuration = configuration;
            field.lifecycle = if input.archived {
                FieldLifecycle::Archived
            } else {
                FieldLifecycle::Active
            };
            field
        } else {
            FieldDefinition {
                writing_guide: input.writing_guide,
                label: input.label,
                lifecycle: FieldLifecycle::Active,
                kind: input.kind,
                required: input.required,
                initial_default_value: default.clone(),
                default_value: default,
                introduced_revision: introduction,
                configuration,
                presentation: Presentation {
                    token: input.presentation_token,
                    extra: BTreeMap::new(),
                },
                extra: BTreeMap::new(),
            }
        };
        if !matches!(
            input.card_title_field,
            crate::data::edit_recovery::model::Intent::Keep
        ) && original.is_some_and(|f| {
            f.presentation.extra.contains_key("cardTitleField")
                && f.presentation.card_title_field().is_none()
        }) {
            return Err(TemplateMutationError::invalid_field_draft(id));
        }
        match input.card_title_field {
            crate::data::edit_recovery::model::Intent::Keep => (),
            crate::data::edit_recovery::model::Intent::Unset => {
                definition.presentation.set_card_title_field(None)
            }
            crate::data::edit_recovery::model::Intent::Set(title) => {
                let (_, members) = definition
                    .configuration
                    .members()
                    .ok_or_else(|| TemplateMutationError::invalid_field_draft(id))?;
                if members.get(&title).is_none_or(|f| {
                    f.kind != FieldKind::RichText || f.lifecycle != FieldLifecycle::Active
                }) {
                    return Err(TemplateMutationError::invalid_field_draft(id));
                }
                definition.presentation.set_card_title_field(Some(title));
            }
        }
        if validate_new_default && definition.kind == FieldKind::Number {
            let mut active_default = definition.clone();
            active_default.lifecycle = FieldLifecycle::Active;
            active_default
                .validate_fresh_default()
                .map_err(|error| TemplateMutationError::invalid_current_default(id, error))?;
        }
        if !input.archived {
            candidate.field_order.push(id);
        }
        candidate.fields.insert(id, definition);
    }
    // 기존 ID 누락을 물리 삭제나 암묵 keep으로 바꾸지 않고 불완전한 전체 제출을 거절한다.
    for id in source.fields.keys() {
        if !seen.contains(id) {
            return Err(TemplateMutationError::field_removed(*id));
        }
    }
    if source.schema_version.get() == 1
        && candidate.fields.iter().any(|(id, f)| {
            f.writing_guide.as_ref() != source.fields.get(id).and_then(|f| f.writing_guide.as_ref())
        })
    {
        // 기존 확장 정보를 새 가이드로 해석하거나 덮어쓰지 않는다.
        if let Some((id, _)) = source
            .fields
            .iter()
            .find(|(_, f)| f.extra.contains_key("writingGuide"))
        {
            let mut error = TemplateMutationError::new(
                TemplateMutationErrorCategory::GuideMetadataConflict,
                "legacy metadata occupies writing guide; original retained",
            );
            error.field_id = Some(*id);
            return Err(error);
        }
        candidate.schema_version = crate::data::schema::SchemaVersion::new_unchecked(2);
    }
    // 기존 제목의 anchor Field를 보관하면 다음 활성 Field 앞으로 이동한다.
    // 임의 새 anchor 오류를 조용히 보정하지 않는다.
    for section in &mut candidate.sections {
        if let Some(anchor) = section
            .before_field
            .filter(|id| !candidate.field_order.contains(id))
        {
            if source
                .sections
                .iter()
                .any(|old| old.id == section.id && old.before_field == Some(anchor))
            {
                section.before_field = source
                    .field_order
                    .iter()
                    .skip_while(|id| **id != anchor)
                    .skip(1)
                    .find(|id| candidate.field_order.contains(id))
                    .copied();
            }
        }
    }
    validate_historical_invariants_with_restorations(source, &candidate, false, &restoring)?;
    // Option 보관과 default 교체를 모두 반영한 다음 한 번만 의미를 검사한다.
    Ok(candidate)
}

#[cfg(test)]
mod tests;
