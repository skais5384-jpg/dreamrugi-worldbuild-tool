use std::collections::{BTreeMap, BTreeSet};

use super::{
    document_reconciliation::DocumentReconciliationIssueCategory, DocumentArtifact,
    OrphanedFieldDefinition, OrphanedOptionDefinition,
};
use crate::data::artifact::{
    FieldDefinition, FieldId, FieldLifecycle, FieldValue, OptionId, OptionLifecycle,
    TemplateArtifact,
};

type Issue = DocumentReconciliationIssueCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SnapshotAction {
    Preserve,
    Create,
    Remove,
}

pub(super) fn selected_options(value: &FieldValue) -> BTreeSet<OptionId> {
    value
        .single_choice()
        .into_iter()
        .chain(value.multi_choice().unwrap_or_default().iter().copied())
        .collect()
}

/// 읽기와 쓰기가 같은 필요 조건을 사용해야 생성/제거가 매 저장마다 반복되지 않는다.
/// 이 판정은 source를 빌리기만 하며 label이나 snapshot을 새로 만들지 않는다.
pub(super) fn assess_snapshot(
    definition: Option<&FieldDefinition>,
    value: &FieldValue,
    snapshot: Option<&OrphanedFieldDefinition>,
) -> Result<SnapshotAction, Issue> {
    let Some(definition) = definition else {
        return if snapshot.is_some() {
            Ok(SnapshotAction::Preserve)
        } else {
            Err(Issue::OrphanSnapshotMissing)
        };
    };
    if snapshot.is_some_and(|snapshot| snapshot.kind() != definition.kind()) {
        return Err(Issue::ReattachmentSnapshotConflict);
    }
    let selected = selected_options(value);
    let options = definition.configuration().options();
    let archived_selection = selected.iter().any(|id| {
        options
            .and_then(|options| options.get(id))
            .is_some_and(|option| option.lifecycle() == OptionLifecycle::Archived)
    });
    let required = definition.lifecycle() == FieldLifecycle::Archived || archived_selection;
    if required {
        if snapshot.is_some() {
            // 역사 label/extra는 현재 정의와 달라도 원래 소유 위치에 그대로 남긴다.
            return Ok(SnapshotAction::Preserve);
        }
        if selected
            .iter()
            .any(|id| options.and_then(|options| options.get(id)).is_none())
        {
            return Err(Issue::OptionSnapshotUnavailable);
        }
        return Ok(SnapshotAction::Create);
    }
    let Some(snapshot) = snapshot else {
        return Ok(SnapshotAction::Preserve);
    };
    // Template에 같은 extra가 있어도 snapshot 소유 metadata를 버릴 권한은 생기지 않는다.
    if snapshot.contains_unknown_storage_data() {
        return Err(Issue::LossyOrphanReattachment);
    }
    // 같은 ID의 label rename은 값의 비호환 변경이 아니다. 역사 label을 덮어쓰지 않고
    // 현재 owner에 속한 active Option인지 확인하며, 실제 값/configuration은 bound 검증한다.
    if snapshot.options().keys().any(|id| {
        options
            .and_then(|options| options.get(id))
            .is_none_or(|option| option.lifecycle() != OptionLifecycle::Active)
    }) {
        return Err(Issue::ReattachmentSnapshotConflict);
    }
    Ok(SnapshotAction::Remove)
}

fn option_snapshot(
    definition: &FieldDefinition,
    id: OptionId,
) -> Result<OrphanedOptionDefinition, Issue> {
    let option = definition
        .configuration()
        .options()
        .and_then(|options| options.get(&id))
        .ok_or(Issue::OptionSnapshotUnavailable)?;
    Ok(OrphanedOptionDefinition {
        label: option.label().to_owned(),
        extra: BTreeMap::new(),
    })
}

/// 명시적 편집의 membership만 갱신한다. 남은 Option의 역사 정보는 clone하고,
/// 제거할 Option에 미래 metadata가 있으면 부분 갱신을 공개하지 않고 거부한다.
pub(super) fn update_snapshot_membership(
    definition: &FieldDefinition,
    value: &FieldValue,
    snapshot: &OrphanedFieldDefinition,
) -> Result<OrphanedFieldDefinition, Issue> {
    if snapshot.kind() != definition.kind() {
        return Err(Issue::ReattachmentSnapshotConflict);
    }
    let selected = selected_options(value);
    if snapshot
        .options()
        .iter()
        .any(|(id, option)| !selected.contains(id) && option.contains_unknown_storage_data())
    {
        return Err(Issue::LossySnapshotMembership);
    }
    let mut result = snapshot.clone();
    result.options.retain(|id, _| selected.contains(id));
    for id in selected {
        if let std::collections::btree_map::Entry::Vacant(entry) = result.options.entry(id) {
            entry.insert(option_snapshot(definition, id)?);
        }
    }
    Ok(result)
}

/// 생성/편집/no-edit materialization이 소유한 private candidate에만 적용한다.
pub(super) fn apply_snapshot_policy(
    template: &TemplateArtifact,
    candidate: &mut DocumentArtifact,
) -> Result<(), (FieldId, Issue)> {
    for (field_id, value) in &candidate.field_values {
        let definition = template.fields().get(field_id);
        let action = assess_snapshot(
            definition,
            value,
            candidate.orphaned_field_definitions.get(field_id),
        )
        .map_err(|issue| (*field_id, issue))?;
        match action {
            SnapshotAction::Preserve => {}
            SnapshotAction::Remove => {
                candidate.orphaned_field_definitions.remove(field_id);
            }
            SnapshotAction::Create => {
                let definition = definition.ok_or((*field_id, Issue::OrphanSnapshotMissing))?;
                let options = selected_options(value)
                    .into_iter()
                    .map(|id| option_snapshot(definition, id).map(|snapshot| (id, snapshot)))
                    .collect::<Result<_, _>>()
                    .map_err(|issue| (*field_id, issue))?;
                candidate.orphaned_field_definitions.insert(
                    *field_id,
                    OrphanedFieldDefinition {
                        label: definition.label().to_owned(),
                        kind: definition.kind(),
                        options,
                        extra: BTreeMap::new(),
                    },
                );
            }
        }
    }
    Ok(())
}
