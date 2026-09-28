//! 복제 시 native 원문의 알려진 instance map 위치만 옮긴다. opaque 문자열은 수정하지 않는다.
use super::{id::InstanceId, ArtifactValidationError, DocumentArtifact, DocumentEdit, FieldId};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

type Object = BTreeMap<String, Box<RawValue>>;

pub(super) fn relocate(
    template: &super::TemplateArtifact,
    document: &mut DocumentArtifact,
    edits: &[DocumentEdit],
) -> Result<(), ArtifactValidationError> {
    if !edits
        .iter()
        .any(|e| matches!(e, DocumentEdit::SetGroup(_, _)))
    {
        return Ok(());
    }
    let fail = || ArtifactValidationError::invalid_json_value();
    let bytes = super::encode_document(document).map_err(|_| fail())?;
    let template_bytes = super::encode_template(template).map_err(|_| fail())?;
    let template_root: Object = serde_json::from_slice(&template_bytes).map_err(|_| fail())?;
    let template_fields: BTreeMap<FieldId, Box<RawValue>> =
        serde_json::from_str(template_root.get("fields").ok_or_else(fail)?.get())
            .map_err(|_| fail())?;
    let mut root: Object = serde_json::from_slice(&bytes).map_err(|_| fail())?;
    let raw = root.get("fieldValues").ok_or_else(fail)?;
    let mut values: BTreeMap<FieldId, Box<RawValue>> =
        serde_json::from_str(raw.get()).map_err(|_| fail())?;
    for edit in edits {
        let DocumentEdit::SetGroup(group, drafts) = edit else {
            continue;
        };
        let Some(raw) = values.get(group) else {
            continue;
        };
        let mut value: Object = serde_json::from_str(raw.get()).map_err(|_| fail())?;
        let Some(raw) = value.get("instances") else {
            continue;
        };
        let mut instances: BTreeMap<InstanceId, Box<RawValue>> =
            serde_json::from_str(raw.get()).map_err(|_| fail())?;
        let original = instances.clone();
        let definition = template.fields().get(group).ok_or_else(fail)?;
        let Some((_, members)) = definition.configuration().members() else {
            return Err(fail());
        };
        let raw_definition: Object =
            serde_json::from_str(template_fields.get(group).ok_or_else(fail)?.get())
                .map_err(|_| fail())?;
        let config: Object =
            serde_json::from_str(raw_definition.get("configuration").ok_or_else(fail)?.get())
                .map_err(|_| fail())?;
        let raw_members: BTreeMap<FieldId, Box<RawValue>> =
            serde_json::from_str(config.get("members").ok_or_else(fail)?.get())
                .map_err(|_| fail())?;
        for draft in drafts {
            let source = if original.contains_key(&draft.id) {
                Some(draft.id)
            } else {
                draft.source
            };
            if let Some(source) = source {
                let raw = original.get(&source).ok_or_else(fail)?;
                let mut instance: Object = serde_json::from_str(raw.get()).map_err(|_| fail())?;
                let revision: super::TemplateRevision =
                    serde_json::from_str(instance.get("revision").ok_or_else(fail)?.get())
                        .map_err(|_| fail())?;
                let mut cells: BTreeMap<FieldId, Box<RawValue>> =
                    serde_json::from_str(instance.get("values").ok_or_else(fail)?.get())
                        .map_err(|_| fail())?;
                for (id, child) in members {
                    if child.lifecycle() == super::FieldLifecycle::Active
                        && child.introduced_revision() > revision
                        && !cells.contains_key(id)
                    {
                        let child: Object =
                            serde_json::from_str(raw_members.get(id).ok_or_else(fail)?.get())
                                .map_err(|_| fail())?;
                        cells.insert(
                            *id,
                            child.get("initialDefaultValue").ok_or_else(fail)?.clone(),
                        );
                    }
                }
                instance.insert(
                    "values".into(),
                    serde_json::value::to_raw_value(&cells).map_err(|_| fail())?,
                );
                instances.insert(
                    draft.id,
                    serde_json::value::to_raw_value(&instance).map_err(|_| fail())?,
                );
            }
        }
        value.insert(
            "instances".into(),
            serde_json::value::to_raw_value(&instances).map_err(|_| fail())?,
        );
        values.insert(
            *group,
            serde_json::value::to_raw_value(&value).map_err(|_| fail())?,
        );
    }
    root.insert(
        "fieldValues".into(),
        serde_json::value::to_raw_value(&values).map_err(|_| fail())?,
    );
    let bytes = serde_json::to_vec(&root).map_err(|_| fail())?;
    let source =
        crate::data::json::parse_strict_lossless_json_object(&bytes).map_err(|_| fail())?;
    document.set_lossless_source(source);
    Ok(())
}
