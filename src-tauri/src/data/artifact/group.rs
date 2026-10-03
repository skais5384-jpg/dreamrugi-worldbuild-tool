//! 반복 카드의 ID와 하위 값은 그룹 안에서만 해석한다. 복제 원본은 native 원문에서 찾는다.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    id::InstanceId,
    value::{validate_extra_keys, ExtraFields, FieldValueWire},
    FieldDefinition, FieldId, FieldKind, FieldLifecycle, FieldValue, TemplateRevision,
};
use crate::data::{
    edit_recovery::model::{DraftValue, Intent},
    field_engine::validation::{
        BoundDocumentValueContext, FieldValidationError, FieldValidationLocation,
        FieldValidationOutcome,
    },
};

#[derive(Clone, PartialEq)]
pub(crate) struct GroupValue {
    pub(crate) order: Vec<InstanceId>,
    pub(crate) instances: BTreeMap<InstanceId, Instance>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Instance {
    pub(crate) revision: TemplateRevision,
    pub(crate) values: BTreeMap<FieldId, FieldValue>,
    pub(crate) labels: BTreeMap<FieldId, String>,
    extra: ExtraFields,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InstanceDraft {
    pub(crate) id: InstanceId,
    /// 복제된 카드의 보관 원문 출처. 입력 DTO의 raw는 신뢰하지 않는다.
    pub(crate) source: Option<InstanceId>,
    /// 화면 저장 응답의 복제 계보. native 원문 권한은 source만 검증한다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) lineage: Vec<InstanceId>,
    pub(crate) fields: Vec<DraftValue>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) labels: BTreeMap<FieldId, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) protected: Vec<FieldId>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupValueWire {
    instance_order: Vec<InstanceId>,
    instances: BTreeMap<InstanceId, InstanceWire>,
    #[serde(flatten)]
    extra: ExtraFields,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceWire {
    revision: TemplateRevision,
    values: BTreeMap<FieldId, FieldValueWire>,
    labels: BTreeMap<FieldId, String>,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl Instance {
    pub(crate) fn contains_unknown_storage_data(&self) -> bool {
        !self.extra.is_empty()
            || self
                .values
                .values()
                .any(FieldValue::contains_unknown_storage_data)
    }
}

impl GroupValueWire {
    pub(super) fn into_value(self) -> (GroupValue, ExtraFields) {
        (
            GroupValue {
                order: self.instance_order,
                instances: self
                    .instances
                    .into_iter()
                    .map(|(id, v)| {
                        (
                            id,
                            Instance {
                                revision: v.revision,
                                values: v
                                    .values
                                    .into_iter()
                                    .map(|(id, v)| (id, v.into()))
                                    .collect(),
                                labels: v.labels,
                                extra: v.extra,
                            },
                        )
                    })
                    .collect(),
            },
            self.extra,
        )
    }
    pub(super) fn from_value(v: &GroupValue, extra: ExtraFields) -> Self {
        Self {
            instance_order: v.order.clone(),
            instances: v
                .instances
                .iter()
                .map(|(id, v)| {
                    (
                        *id,
                        InstanceWire {
                            revision: v.revision,
                            values: v.values.iter().map(|(id, v)| (*id, v.into())).collect(),
                            labels: v.labels.clone(),
                            extra: v.extra.clone(),
                        },
                    )
                })
                .collect(),
            extra,
        }
    }
}

fn invalid() -> FieldValidationError {
    FieldValidationError::value_kind_mismatch(FieldValidationLocation::ExistingDocumentValue)
}
impl GroupValue {
    pub(crate) fn validate(&self) -> Result<FieldValidationOutcome, FieldValidationError> {
        let ids: BTreeSet<_> = self.order.iter().copied().collect();
        if ids.len() != self.order.len() || ids != self.instances.keys().copied().collect() {
            return Err(invalid());
        }
        for instance in self.instances.values() {
            validate_extra_keys(&instance.extra, &["revision", "values", "labels"])
                .map_err(|_| invalid())?;
            if instance
                .values
                .keys()
                .any(|id| !instance.labels.contains_key(id))
            {
                return Err(invalid());
            }
            for value in instance.values.values() {
                if value.group().is_some() {
                    return Err(invalid());
                }
                value
                    .validate_structure(super::ArtifactScalarValueLocation::DocumentField)
                    .map_err(|_| invalid())?;
            }
        }
        Ok(FieldValidationOutcome::Valid)
    }
    pub(crate) fn contains_unknown_storage_data(&self) -> bool {
        self.instances
            .values()
            .any(|v| v.contains_unknown_storage_data())
    }
}

pub(crate) fn validate_bound(
    members: &BTreeMap<FieldId, FieldDefinition>,
    value: &FieldValue,
    context: BoundDocumentValueContext,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    validate_cells(members, value, context).map_err(|(_, cause)| cause)
}

/// 읽기 projection·최종 검증·SetGroup이 공유하는 과거 누락값 해석. explicit unset은 그대로다.
pub(crate) fn effective_value<'a>(
    instance: Option<&'a Instance>,
    id: &FieldId,
    child: &'a FieldDefinition,
) -> std::borrow::Cow<'a, FieldValue> {
    use std::borrow::Cow;
    if let Some(instance) = instance {
        if let Some(value) = instance.values.get(id) {
            return Cow::Borrowed(value);
        }
        if child.introduced_revision() > instance.revision {
            return Cow::Borrowed(child.initial_default_value());
        }
    }
    Cow::Owned(FieldValue::unset())
}

pub(crate) fn validate_cells(
    members: &BTreeMap<FieldId, FieldDefinition>,
    value: &FieldValue,
    context: BoundDocumentValueContext,
) -> Result<FieldValidationOutcome, (GroupError, FieldValidationError)> {
    // 카드 0개는 필수 하위 입력의 존재를 강제하지 않는다.
    if value.is_unset() {
        return Ok(FieldValidationOutcome::Valid);
    }
    let group = value
        .group()
        .ok_or_else(|| (GroupError::new("InvalidGroup", None, None), invalid()))?;
    let _ = group
        .validate()
        .map_err(|cause| (GroupError::new("InvalidGroup", None, None), cause))?;
    for instance_id in &group.order {
        let instance = &group.instances[instance_id];
        for (id, definition) in members {
            if definition.lifecycle() == FieldLifecycle::Active || instance.values.contains_key(id)
            {
                let value = effective_value(Some(instance), id, definition);
                let _ = definition
                    .validate_document_value(
                        &value,
                        if definition.lifecycle() == FieldLifecycle::Archived {
                            BoundDocumentValueContext::ExistingDocumentValue
                        } else {
                            context
                        },
                    )
                    .map_err(|cause| {
                        (
                            GroupError::new(
                                "RequiredOrInvalidChild",
                                Some(*instance_id),
                                Some(*id),
                            ),
                            cause,
                        )
                    })?;
            }
        }
    }
    Ok(FieldValidationOutcome::Valid)
}

/// 저장 후보에만 적용한다. 읽기는 원본을 materialize하거나 덮어쓰지 않는다.
pub(crate) fn assemble(
    definition: &FieldDefinition,
    revision: TemplateRevision,
    source: Option<&FieldValue>,
    drafts: &[InstanceInput],
) -> Result<FieldValue, GroupError> {
    let (_, members) = definition
        .configuration()
        .members()
        .ok_or_else(|| GroupError::new("InvalidGroup", None, None))?;
    let old = source.and_then(FieldValue::group);
    let mut result = GroupValue {
        order: vec![],
        instances: BTreeMap::new(),
    };
    for draft in drafts {
        let fail = |category, field| GroupError::new(category, Some(draft.id), field);
        if result.instances.contains_key(&draft.id) {
            return Err(fail("DuplicateInstance", None));
        }
        let existing = old.and_then(|g| g.instances.get(&draft.id));
        if existing.is_some() && draft.source.is_some_and(|id| id != draft.id) {
            return Err(fail("ForeignInstanceSource", None));
        }
        let original = match draft.source {
            Some(id) => Some(
                old.and_then(|g| g.instances.get(&id))
                    .ok_or_else(|| fail("MissingInstanceSource", None))?,
            ),
            None => existing,
        };
        if original.is_some_and(|i| i.revision > revision) {
            return Err(fail("FutureInstanceRevision", None));
        }
        let copied = existing.is_none() && draft.source.is_some();
        let original_reference_ids: BTreeSet<_> = original
            .into_iter()
            .flat_map(|instance| instance.values.values())
            .filter_map(FieldValue::relations)
            .flatten()
            .map(super::RelationLink::id)
            .collect();
        if copied {
            // 복제 relation은 canonical source를 그대로 clone하면 연결 정체성까지 공유한다.
            // 화면이 한 번 만든 Set을 요구해 저장 재시도에도 같은 새 ID를 유지하되,
            // native는 원 source의 관계마다 변환이 존재하고 old ID/oneWay가 남지 않았는지 확인한다.
            for (id, child) in members {
                let Some(previous) = original.and_then(|instance| instance.values.get(id)) else {
                    continue;
                };
                if child.kind() != FieldKind::Relation || previous.relations().is_none() {
                    continue;
                }
                let replacement = draft.fields.iter().find(|cell| cell.field == *id);
                let valid = match replacement.map(|cell| &cell.value) {
                    Some(Intent::Unset) => child.lifecycle() == FieldLifecycle::Active,
                    Some(Intent::Set(value)) => value.relations().is_some_and(|links| {
                        links.iter().all(|link| {
                            !link.one_way() && !original_reference_ids.contains(&link.id())
                        })
                    }),
                    _ => false,
                };
                if !valid {
                    return Err(fail("InvalidCopiedRelation", Some(*id)));
                }
            }
        }
        let mut instance = original.cloned().unwrap_or(Instance {
            revision,
            values: BTreeMap::new(),
            labels: BTreeMap::new(),
            extra: BTreeMap::new(),
        });
        let mut seen = BTreeSet::new();
        for cell in &draft.fields {
            let id = cell.field;
            if !seen.insert(id) {
                return Err(fail("DuplicateChild", Some(id)));
            }
            let child = members
                .get(&id)
                .ok_or_else(|| fail("ForeignChild", Some(id)))?;
            if matches!(cell.value, Intent::Keep) {
                continue;
            }
            let copied_relation = copied
                && child.kind() == FieldKind::Relation
                && matches!(cell.value, Intent::Set(_));
            if child.lifecycle() != FieldLifecycle::Active && !copied_relation {
                return Err(fail("ArchivedChild", Some(id)));
            }
            if !matches!(
                child.kind(),
                FieldKind::RichText
                    | FieldKind::Number
                    | FieldKind::Image
                    | FieldKind::File
                    | FieldKind::Relation
                    | FieldKind::DocumentLink
            ) {
                return Err(fail("NestedOrUnsupportedChild", Some(id)));
            }
            let value = match &cell.value {
                Intent::Keep => continue,
                Intent::Unset => FieldValue::unset(),
                Intent::Set(v) => v.clone(),
            };
            if copied_relation
                && !value.relations().is_some_and(|links| {
                    links
                        .iter()
                        .all(|link| !link.one_way() && !original_reference_ids.contains(&link.id()))
                })
            {
                return Err(fail("InvalidCopiedRelation", Some(id)));
            }
            let value = if let Some(previous) = instance.values.get(&id) {
                let value = value.preserve_outer_storage_extra_from(previous);
                if !previous.has_same_storage_extras(&value) {
                    return Err(fail("ProtectedChild", Some(id)));
                }
                value
            } else {
                value
            };
            let _ = child
                .validate_document_value(&value, BoundDocumentValueContext::NewDocumentValue)
                .map_err(|_| fail("InvalidChildValue", Some(id)))?;
            instance.values.insert(id, value);
        }
        for (id, child) in members {
            if child.lifecycle() != FieldLifecycle::Active {
                continue;
            }
            if !instance.values.contains_key(id) {
                // 기존 카드의 새 하위 정의는 역사적 initial, 새 카드는 빈 값으로 시작한다.
                let value = effective_value(original, id, child).into_owned();
                instance.values.insert(*id, value);
            }
            instance.labels.insert(*id, child.label().to_owned());
            let _ = child
                .validate_document_value(
                    &instance.values[id],
                    BoundDocumentValueContext::ExistingDocumentValue,
                )
                .map_err(|_| fail("RequiredOrInvalidChild", Some(*id)))?;
        }
        instance.revision = revision;
        result.order.push(draft.id);
        result.instances.insert(draft.id, instance);
    }
    Ok(FieldValue::from_group(result))
}

#[derive(Debug)]
pub(crate) struct GroupError {
    pub(crate) category: &'static str,
    pub(crate) instance: Option<InstanceId>,
    pub(crate) field: Option<FieldId>,
}
impl GroupError {
    fn new(category: &'static str, instance: Option<InstanceId>, field: Option<FieldId>) -> Self {
        Self {
            category,
            instance,
            field,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct InstanceInput {
    pub(crate) id: InstanceId,
    pub(crate) source: Option<InstanceId>,
    pub(crate) fields: Vec<CellInput>,
}
#[derive(Clone, PartialEq)]
pub(crate) struct CellInput {
    pub(crate) field: FieldId,
    pub(crate) value: Intent<FieldValue>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CellAddress {
    pub(crate) instance: InstanceId,
    pub(crate) child: FieldId,
}

pub(crate) fn asset_target(
    template: &super::TemplateArtifact,
    fields: &[DraftValue],
    source: Option<&super::DocumentArtifact>,
    group: &str,
    cell: &CellAddress,
    image: bool,
) -> bool {
    let Ok(id) = group.parse() else {
        return false;
    };
    let Some(definition) = template
        .fields()
        .get(&id)
        .filter(|f| f.lifecycle() == FieldLifecycle::Active)
    else {
        return false;
    };
    let Some((_, members)) = definition.configuration().members() else {
        return false;
    };
    if !members.get(&cell.child).is_some_and(|f| {
        f.lifecycle() == FieldLifecycle::Active
            && f.kind()
                == if image {
                    FieldKind::Image
                } else {
                    FieldKind::File
                }
    }) {
        return false;
    }
    let old = source
        .and_then(|d| d.field_values().get(&id))
        .and_then(FieldValue::group);
    let matches: Vec<_> = fields.iter().filter(|f| f.field == group).collect();
    if matches.len() > 1 {
        return false;
    }
    match matches.first().map(|f| &f.value) {
        Some(Intent::Set(crate::data::edit_input::ValueDto::Group { instances })) => {
            let ids: BTreeSet<_> = instances.iter().map(|i| i.id).collect();
            ids.len() == instances.len()
                && instances
                    .iter()
                    .find(|i| i.id == cell.instance)
                    .is_some_and(|i| {
                        i.source
                            .is_none_or(|s| old.is_some_and(|g| g.instances.contains_key(&s)))
                    })
        }
        Some(Intent::Unset) | Some(Intent::Set(_)) => false,
        _ => old.is_some_and(|g| g.instances.contains_key(&cell.instance)),
    }
}
