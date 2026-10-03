use super::dto::*;
use crate::data::{artifact::*, field_engine::validation::FieldValueView};

fn rich(value: &serde_json::Map<String, serde_json::Value>) -> Reply<RichNode> {
    // future node/mark metadata는 backend 원본에 남긴다. read projection만 known member를 선택한다.
    let mut known = serde_json::Map::new();
    for key in ["kind", "text", "marks", "level", "checked"] {
        if let Some(v) = value.get(key) {
            known.insert(key.to_owned(), v.clone());
        }
    }
    if let Some(children) = value.get("children") {
        let children = children
            .as_array()
            .ok_or(Code::SerializationFailed)?
            .iter()
            .map(|v| rich(v.as_object().ok_or(Code::SerializationFailed)?))
            .collect::<Reply<Vec<_>>>()?;
        known.insert(
            "children".into(),
            serde_json::to_value(children).map_err(|_| Code::SerializationFailed)?,
        );
    }
    serde_json::from_value(serde_json::Value::Object(known))
        .map_err(|_| Code::SerializationFailed.into())
}
pub(crate) fn value(value: &FieldValue) -> Reply<ValueDto> {
    Ok(match value.validation_view() {
        FieldValueView::Group(g) => ValueDto::Group {
            instances: g
                .order
                .iter()
                .map(|id| {
                    let instance = &g.instances[id];
                    Ok(crate::data::artifact::group::InstanceDraft {
                        id: *id,
                        source: Some(*id),
                        lineage: vec![],
                        labels: instance.labels.clone(),
                        protected: instance
                            .values
                            .iter()
                            .filter_map(|(id, v)| {
                                v.rich_text()
                                    .is_some_and(|r| r.contains_unknown_storage_data())
                                    .then_some(*id)
                            })
                            .collect(),
                        fields: instance
                            .values
                            .iter()
                            .map(|(field, v)| {
                                Ok(crate::data::edit_recovery::model::DraftValue {
                                    field: field.to_string(),
                                    value: crate::data::edit_recovery::model::Intent::Set(
                                        self::value(v)?,
                                    ),
                                })
                            })
                            .collect::<Reply<_>>()?,
                    })
                })
                .collect::<Reply<_>>()?,
        },
        FieldValueView::Unset => ValueDto::Unset {},
        FieldValueView::SingleLineText(v) => ValueDto::SingleLineText { value: v.into() },
        FieldValueView::NumberUnknown => ValueDto::NumberUnknown { previous_raw: None },
        FieldValueView::Number(v) => ValueDto::Number { value: v.into() },
        FieldValueView::Date(v) => ValueDto::Date { value: v.into() },
        FieldValueView::Time(v) => ValueDto::Time { value: v.into() },
        FieldValueView::Image(v) => ValueDto::Image { value: v.into() },
        FieldValueView::File(v) => ValueDto::File { value: v.into() },
        FieldValueView::Url(v) => ValueDto::Url { value: v.into() },
        FieldValueView::Duration(v) => ValueDto::Duration {
            milliseconds: v.into(),
        },
        FieldValueView::SingleChoice(v) => ValueDto::SingleChoice {
            option: v.to_string(),
        },
        FieldValueView::MultiChoice(v) => ValueDto::MultiChoice {
            options: v.iter().map(ToString::to_string).collect(),
        },
        FieldValueView::Relation(links) => ValueDto::Relation {
            links: links
                .iter()
                .map(|link| crate::data::edit_input::RelationLinkDto {
                    id: link.id().to_string(),
                    document: link.document().to_string(),
                    one_way: link.one_way(),
                    name: link.name().to_owned(),
                })
                .collect(),
        },
        FieldValueView::DocumentLink(documents) => ValueDto::DocumentLink {
            documents: documents.iter().map(ToString::to_string).collect(),
        },
        FieldValueView::RichText(_) => ValueDto::RichText {
            content: rich(
                value
                    .rich_text()
                    .ok_or(Code::SerializationFailed)?
                    .content_for_projection(),
            )?,
        },
    })
}
pub(crate) fn template(t: &TemplateArtifact) -> Reply<TemplateDto> {
    Ok(TemplateDto {
        schema: t.schema_version().get(),
        sections: t.sections().iter().map(|s| s.input()).collect(),
        id: t.template_id().to_string(),
        revision: t.revision().get().to_string(),
        name: t.name().into(),
        lifecycle: format!("{:?}", t.lifecycle()),
        glossary_excluded: t.glossary_excluded(),
        presentation: t.presentation().token().map(str::to_owned),
        field_order: t.field_order().iter().map(ToString::to_string).collect(),
        fields: t
            .fields()
            .iter()
            .map(|(id, f)| field(*id, f))
            .collect::<Reply<_>>()?,
    })
}

/// 읽기 projection의 역사적 미입력도 보여 주되 canonical 파일은 바꾸지 않는다.
pub(crate) fn field_value(v: &FieldValue, definition: Option<&FieldDefinition>) -> Reply<ValueDto> {
    let mut projected = value(v)?;
    if let (Some(group), Some((_, members)), ValueDto::Group { instances }) = (
        v.group(),
        definition.and_then(|f| f.configuration().members()),
        &mut projected,
    ) {
        for card in instances {
            let original = &group.instances[&card.id];
            for (id, child) in members {
                if child.lifecycle() != FieldLifecycle::Active || original.values.contains_key(id) {
                    continue;
                }
                let effective =
                    crate::data::artifact::group::effective_value(Some(original), id, child);
                let value = self::value(&effective)?;
                card.fields
                    .push(crate::data::edit_recovery::model::DraftValue {
                        field: id.to_string(),
                        value: crate::data::edit_recovery::model::Intent::Set(value),
                    });
                card.labels.insert(*id, child.label().to_owned());
                if effective
                    .rich_text()
                    .is_some_and(|v| v.contains_unknown_storage_data())
                {
                    card.protected.push(*id);
                }
            }
        }
    }
    Ok(projected)
}

/// 자기 저장이 증명된 보관본만 새 기준에 잇는다. 원 보관 bytes는 수정하지 않는다.
pub(crate) fn rebase_group_drafts(
    fields: &mut [crate::data::edit_recovery::model::DraftValue],
    before: &DocumentArtifact,
    saved: &DocumentArtifact,
    template: &TemplateArtifact,
) -> Reply<()> {
    use crate::data::edit_recovery::model::{DraftValue, Intent};
    for field in fields {
        let Intent::Set(ValueDto::Group { instances }) = &mut field.value else {
            continue;
        };
        let id: FieldId = field.field.parse().map_err(|_| Code::InvalidInput)?;
        let definition = template.fields().get(&id);
        let old = before
            .field_values()
            .get(&id)
            .map(|v| field_value(v, definition))
            .transpose()?;
        let new = saved
            .field_values()
            .get(&id)
            .map(|v| field_value(v, definition))
            .transpose()?;
        let Some(ValueDto::Group {
            instances: saved_cards,
        }) = new
        else {
            continue;
        };
        let old_cards = match &old {
            Some(ValueDto::Group { instances }) => instances.as_slice(),
            _ => &[],
        };
        for card in instances {
            let anchor = std::iter::once(card.id)
                .chain(card.lineage.iter().copied())
                .chain(card.source)
                .find_map(|id| saved_cards.iter().find(|i| i.id == id));
            let Some(anchor) = anchor else {
                continue;
            };
            let original = old_cards
                .iter()
                .find(|i| i.id == card.source.unwrap_or(card.id));
            let mut children = std::collections::BTreeSet::new();
            for f in original
                .into_iter()
                .flat_map(|i| &i.fields)
                .chain(&anchor.fields)
                .chain(&card.fields)
            {
                children.insert(f.field.clone());
            }
            for child in children {
                if card
                    .fields
                    .iter()
                    .any(|f| f.field == child && !matches!(f.value, Intent::Keep))
                {
                    continue;
                }
                let old = original
                    .and_then(|i| i.fields.iter().find(|f| f.field == child))
                    .map(|f| f.value.clone())
                    .unwrap_or(Intent::Unset);
                let new = anchor
                    .fields
                    .iter()
                    .find(|f| f.field == child)
                    .map(|f| f.value.clone())
                    .unwrap_or(Intent::Unset);
                if old != new {
                    card.fields.retain(|f| f.field != child);
                    card.fields.push(DraftValue {
                        field: child,
                        value: old,
                    });
                }
            }
            card.source = Some(anchor.id);
            card.lineage.clear();
        }
    }
    Ok(())
}
pub(crate) fn document(d: &DocumentArtifact) -> Reply<DocumentDto> {
    Ok(DocumentDto {
        id: d.document_id().to_string(),
        template: d.template_id().to_string(),
        template_revision: d.template_revision().get().to_string(),
        name: d.name().into(),
        english_name: d.english_name().into(),
        glossary_summary: d.glossary_summary().into(),
        glossary_excluded: d.glossary_excluded(),
        values: d
            .field_values()
            .iter()
            .map(|(id, v)| {
                Ok(DocumentFieldDto {
                    field: id.to_string(),
                    value: value(v)?,
                })
            })
            .collect::<Reply<_>>()?,
    })
}
pub(crate) fn warnings(w: &DocumentMaterializationWarnings) -> Vec<WarningDto> {
    w.as_slice()
        .iter()
        .map(|w| WarningDto {
            category: format!("{:?}", w.category()),
            field: Some(w.field_id().to_string()),
        })
        .collect()
}

pub(crate) fn field(id: FieldId, f: &FieldDefinition) -> Reply<FieldDto> {
    Ok(FieldDto {
        card_title_field: f.presentation().card_title_field().map(|id| id.to_string()),
        members: f
            .configuration()
            .members()
            .map(|(_, fs)| {
                fs.iter()
                    .map(|(id, f)| field(*id, f))
                    .collect::<Reply<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        member_order: f
            .configuration()
            .members()
            .map(|(o, _)| o.iter().map(ToString::to_string).collect())
            .unwrap_or_default(),
        minimum: f.configuration().bounds().0.map(str::to_owned),
        maximum: f.configuration().bounds().1.map(str::to_owned),
        multiple: f.configuration().relation_settings().map(|value| value.0),
        allowed_templates: f
            .configuration()
            .relation_settings()
            .map(|value| value.1.iter().map(ToString::to_string).collect())
            .unwrap_or_default(),
        reciprocal_notice: f.configuration().relation_settings().map(|value| value.2),
        writing_guide: f.writing_guide().map(str::to_owned),
        id: id.to_string(),
        label: f.label().into(),
        kind: format!("{:?}", f.kind()),
        lifecycle: format!("{:?}", f.lifecycle()),
        required: f.required(),
        presentation: f.presentation().token().map(str::to_owned),
        default: value(f.default_value())?,
        initial_default: value(f.initial_default_value())?,
        introduced_revision: f.introduced_revision().get().to_string(),
        options: f
            .configuration()
            .options()
            .into_iter()
            .flatten()
            .map(|(id, o)| OptionDto {
                id: id.to_string(),
                label: o.label().into(),
                lifecycle: format!("{:?}", o.lifecycle()),
            })
            .collect(),
        option_order: f
            .configuration()
            .option_order()
            .unwrap_or(&[])
            .iter()
            .map(ToString::to_string)
            .collect(),
    })
}
