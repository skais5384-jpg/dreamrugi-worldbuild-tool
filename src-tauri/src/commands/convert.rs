use super::dto::*;
use crate::data::{
    application::templates::TemplateEditIntent,
    artifact::{self, template_mutation as tm},
    field_engine::rich_text,
};

pub(crate) fn id<T: std::str::FromStr>(s: &str) -> Reply<T> {
    s.parse().map_err(|_| Code::InvalidInput.into())
}
pub(crate) fn revision(s: &str) -> Reply<artifact::TemplateRevision> {
    let n: u32 = id(s)?;
    if n.to_string() != s {
        return Err(Code::InvalidInput.into());
    }
    n.try_into().map_err(|_| Code::InvalidInput.into())
}
fn index(s: &Option<String>) -> Reply<Option<usize>> {
    s.as_ref()
        .map(|s| {
            let n: usize = id(s)?;
            if n.to_string() != *s {
                return Err(Code::InvalidInput.into());
            }
            Ok(n)
        })
        .transpose()
}
fn rich(node: &RichNode) -> Reply<rich_text::NormalizedRichText> {
    // 새 known edit만 값으로 바꾼다. 저장 artifact와 unknown 숫자 provenance는 이 변환을 통과하지 않는다.
    let value = serde_json::to_value(node).map_err(|_| ErrorDto::new(Code::InvalidInput))?;
    rich_text::normalize_rich_text(
        artifact::RICH_TEXT_SCHEMA_VERSION,
        value.as_object().ok_or(Code::InvalidInput)?,
    )
    .map_err(|_| Code::InvalidInput.into())
}
impl ValueDto {
    pub(crate) fn default_draft(&self) -> Reply<tm::FieldValueDraft> {
        use tm::FieldValueDraft as D;
        Ok(match self {
            Self::Group { .. } => return Err(Code::InvalidInput.into()),
            Self::Unset {} => D::unset(),
            Self::SingleLineText { value } => D::single_line_text(value.clone()),
            Self::Number { value } => D::number(value.clone()),
            Self::NumberUnknown { .. } => D::number_unknown(),
            Self::Date { value } => D::date(value.clone()),
            Self::Time { value } => D::time(value.clone()),
            Self::Image { value } => D::image(value.clone()),
            Self::File { value } => D::file(value.clone()),
            Self::Url { value } => D::url(value.clone()),
            Self::Duration { milliseconds } => D::duration(milliseconds.clone()),
            Self::SingleChoice { option } => D::single_choice(id(option)?),
            Self::MultiChoice { options } => {
                D::multi_choice(options.iter().map(|s| id(s)).collect::<Reply<_>>()?)
            }
            Self::Relation { links } => D::relation(
                links
                    .iter()
                    .map(|link| {
                        Ok((
                            id(&link.id)?,
                            id(&link.document)?,
                            link.one_way,
                            link.name.clone(),
                        ))
                    })
                    .collect::<Reply<_>>()?,
            ),
            Self::DocumentLink { documents } => D::document_link(
                documents
                    .iter()
                    .map(|value| id(value))
                    .collect::<Reply<_>>()?,
            ),
            Self::RichText { content } => D::from_normalized_rich_text(rich(content)?),
        })
    }
    pub(crate) fn creation_value(&self) -> Reply<artifact::FieldValue> {
        // 비어 있는 신규 입력만 미설정이다. 공백, 0, 잘못된 원시 입력은 지우거나 보정하지 않는다.
        let empty = match self {
            Self::Group { .. } => false,
            Self::Unset {} => true,
            Self::SingleLineText { value }
            | Self::Number { value }
            | Self::Date { value }
            | Self::Time { value }
            | Self::Url { value } => value.is_empty(),
            Self::Image { value } | Self::File { value } => value.is_empty(),
            Self::Duration { milliseconds } => milliseconds.is_empty(),
            Self::SingleChoice { option } => option.is_empty(),
            Self::MultiChoice { options } => options.is_empty(),
            Self::Relation { links } => links.is_empty(),
            Self::DocumentLink { documents } => documents.is_empty(),
            Self::RichText {
                content: RichNode::Root { children },
            } => {
                children.is_empty()
                    || matches!(children.as_slice(), [RichNode::Paragraph { children }] if children.is_empty())
            }
            Self::RichText { .. } | Self::NumberUnknown { .. } => false,
        };
        if empty {
            return Ok(artifact::DocumentValueEdit::unset().into_field_value());
        }
        Ok(self.document_value()?.into_field_value())
    }
    fn document_value(&self) -> Reply<artifact::DocumentValueEdit> {
        use artifact::DocumentValueEdit as D;
        Ok(match self {
            Self::Group { .. } => return Err(Code::InvalidInput.into()),
            Self::Unset {} => return Err(Code::InvalidInput.into()),
            Self::SingleLineText { value } => D::single_line_text(value.clone()),
            Self::Number { value } => D::number(value.clone()),
            Self::NumberUnknown { .. } => D::number_unknown(),
            Self::Date { value } => D::date(value.clone()),
            Self::Time { value } => D::time(value.clone()),
            Self::Image { value } => D::image(value.clone()),
            Self::File { value } => D::file(value.clone()),
            Self::Url { value } => D::url(value.clone()),
            Self::Duration { milliseconds } => D::duration(milliseconds.clone()),
            Self::SingleChoice { option } => D::single_choice(id(option)?),
            Self::MultiChoice { options } => {
                D::multi_choice(options.iter().map(|s| id(s)).collect::<Reply<_>>()?)
            }
            Self::Relation { links } => D::relation(
                links
                    .iter()
                    .map(|link| {
                        Ok((
                            id(&link.id)?,
                            id(&link.document)?,
                            link.one_way,
                            link.name.clone(),
                        ))
                    })
                    .collect::<Reply<_>>()?,
            ),
            Self::DocumentLink { documents } => D::document_link(
                documents
                    .iter()
                    .map(|value| id(value))
                    .collect::<Reply<_>>()?,
            ),
            Self::RichText { content } => D::from_normalized_rich_text(rich(content)?),
        })
    }
}
pub(crate) fn edits(values: &[DocumentEdit]) -> Reply<artifact::DocumentEditSet> {
    use artifact::DocumentEdit as D;
    let mut fields = std::collections::BTreeSet::new();
    let mut renamed = false;
    let mut english_name = false;
    let mut glossary_summary = false;
    let mut glossary_excluded = false;
    let mut result = Vec::new();
    for edit in values {
        result.push(match edit {
            DocumentEdit::Rename { name } => {
                if std::mem::replace(&mut renamed, true) {
                    return Err(Code::InvalidInput.into());
                }
                D::Rename(name.clone())
            }
            DocumentEdit::EnglishName { value } => {
                if std::mem::replace(&mut english_name, true) {
                    return Err(Code::InvalidInput.into());
                }
                D::SetEnglishName(value.clone())
            }
            DocumentEdit::GlossarySummary { value } => {
                if std::mem::replace(&mut glossary_summary, true) {
                    return Err(Code::InvalidInput.into());
                }
                D::SetGlossarySummary(value.clone())
            }
            DocumentEdit::GlossaryExcluded { excluded } => {
                if std::mem::replace(&mut glossary_excluded, true) {
                    return Err(Code::InvalidInput.into());
                }
                D::SetGlossaryExcluded(*excluded)
            }
            DocumentEdit::Set { field, value } => {
                let f = id(field)?;
                if !fields.insert(f) {
                    return Err(Code::InvalidInput.into());
                }
                if let ValueDto::Group { instances } = value {
                    D::SetGroup(f, group_inputs(instances)?)
                } else {
                    D::SetValue(f, value.document_value()?)
                }
            }
            DocumentEdit::Unset { field } => {
                let f = id(field)?;
                if !fields.insert(f) {
                    return Err(Code::InvalidInput.into());
                }
                D::Unset(f)
            }
        });
    }
    Ok(artifact::DocumentEditSet::new(result))
}
fn option(value: &NewOption) -> Reply<tm::NewChoiceOptionDraft> {
    Ok(tm::NewChoiceOptionDraft::new(
        id(&value.id)?,
        value.label.clone(),
    ))
}
fn configuration(
    value: &FieldConfiguration,
) -> Reply<(artifact::FieldKind, tm::NewFieldConfiguration)> {
    use artifact::FieldKind as K;
    use tm::NewFieldConfiguration as C;
    Ok(match value {
        FieldConfiguration::SingleLineText {} => (K::SingleLineText, C::single_line_text()),
        FieldConfiguration::RichText {} => (K::RichText, C::rich_text()),
        FieldConfiguration::Number {} => (K::Number, C::number()),
        FieldConfiguration::Date {} => (K::Date, C::date()),
        FieldConfiguration::Time {} => (K::Time, C::time()),
        FieldConfiguration::Image {} => (K::Image, C::image()),
        FieldConfiguration::File {} => (K::File, C::file()),
        FieldConfiguration::Url {} => (K::Url, C::url()),
        FieldConfiguration::Duration {} => (K::Duration, C::duration()),
        FieldConfiguration::SingleChoice { options } => (
            K::SingleChoice,
            C::single_choice(
                options.iter().map(|o| id(&o.id)).collect::<Reply<_>>()?,
                options.iter().map(option).collect::<Reply<_>>()?,
            ),
        ),
        FieldConfiguration::MultiChoice { options } => (
            K::MultiChoice,
            C::multi_choice(
                options.iter().map(|o| id(&o.id)).collect::<Reply<_>>()?,
                options.iter().map(option).collect::<Reply<_>>()?,
            ),
        ),
        FieldConfiguration::Relation {
            multiple,
            allowed_templates,
            reciprocal_notice,
        } => (
            K::Relation,
            C::relation(
                *multiple,
                allowed_templates
                    .iter()
                    .map(|value| id(value))
                    .collect::<Reply<_>>()?,
                *reciprocal_notice,
            ),
        ),
        FieldConfiguration::DocumentLink {} => (K::DocumentLink, C::document_link()),
    })
}
pub(crate) fn intent(edit: &TemplateEdit) -> Reply<TemplateEditIntent> {
    use TemplateEditIntent as E;
    Ok(match edit {
        TemplateEdit::Name { name } => E::SetName(name.clone()),
        TemplateEdit::Presentation { token } => E::SetPresentationToken(token.clone()),
        TemplateEdit::GlossaryExcluded { excluded } => E::SetGlossaryExcluded(*excluded),
        TemplateEdit::CreateField {
            field,
            label,
            configuration: config,
            required,
            presentation,
            default,
            index: at,
        } => {
            let (kind, config) = configuration(config)?;
            E::CreateField {
                draft: tm::NewFieldDraft::new(
                    id(field)?,
                    label.clone(),
                    kind,
                    config,
                    *required,
                    presentation.clone(),
                    default.default_draft()?,
                ),
                insertion: index(at)?
                    .map_or(tm::NewFieldInsertion::Append, tm::NewFieldInsertion::At),
            }
        }
        TemplateEdit::FieldLabel { field, label } => E::SetFieldLabel {
            field: id(field)?,
            label: label.clone(),
        },
        TemplateEdit::FieldRequired { field, required } => E::SetFieldRequired {
            field: id(field)?,
            required: *required,
        },
        TemplateEdit::FieldPresentation { field, token } => E::SetFieldPresentationToken {
            field: id(field)?,
            token: token.clone(),
        },
        TemplateEdit::Default { field, value } => E::SetCurrentDefault {
            field: id(field)?,
            value: value.default_draft()?,
        },
        TemplateEdit::KeepDefault { field } => E::KeepCurrentDefault { field: id(field)? },
        TemplateEdit::ReorderFields { fields } => {
            E::ReorderFields(fields.iter().map(|s| id(s)).collect::<Reply<_>>()?)
        }
        TemplateEdit::ArchiveField { field } => E::ArchiveField(id(field)?),
        TemplateEdit::AddOption {
            field,
            option: value,
            index: at,
        } => E::AddOption {
            field: id(field)?,
            draft: option(value)?,
            insertion: index(at)?
                .map_or(tm::NewOptionInsertion::Append, tm::NewOptionInsertion::At),
        },
        TemplateEdit::RenameOption {
            field,
            option,
            label,
        } => E::RenameOption {
            field: id(field)?,
            option: id(option)?,
            label: label.clone(),
        },
        TemplateEdit::ReorderOptions { field, options } => E::ReorderOptions {
            field: id(field)?,
            order: options.iter().map(|s| id(s)).collect::<Reply<_>>()?,
        },
        TemplateEdit::ArchiveOption {
            field,
            option,
            repair,
        } => E::ArchiveOption {
            field: id(field)?,
            option: id(option)?,
            repair: repair.as_ref().map(ValueDto::default_draft).transpose()?,
        },
    })
}

pub(crate) fn group_inputs(
    drafts: &[artifact::group::InstanceDraft],
) -> Reply<Vec<artifact::group::InstanceInput>> {
    use crate::data::edit_recovery::model::Intent;
    drafts
        .iter()
        .map(|d| {
            Ok(artifact::group::InstanceInput {
                id: d.id,
                source: d.source,
                fields: d
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(artifact::group::CellInput {
                            field: id(&f.field)?,
                            value: match &f.value {
                                Intent::Keep => Intent::Keep,
                                Intent::Unset => Intent::Unset,
                                Intent::Set(v) => Intent::Set(v.creation_value()?),
                            },
                        })
                    })
                    .collect::<Reply<_>>()?,
            })
        })
        .collect()
}
