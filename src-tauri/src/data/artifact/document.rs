use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};

use super::id::{DocumentId, FieldId, OptionId, TemplateId};
use super::revision::TemplateRevision;
use super::schema::{ArtifactType, ArtifactTypeWire, DOCUMENT_SCHEMA_VERSION};
use super::template::validate_timestamps;
use super::value::{
    map_json_storage_error, validate_extra_keys, ExtraFields, FieldKindWire, FieldValue,
    FieldValueWire,
};
use super::{
    ArtifactScalarValueLocation, ArtifactValidationError, CurrentDefaultFieldValueProvenance,
    FieldKind, InitialDefaultFieldValueProvenance, ARTIFACT_PROVENANCE_AUTHORITY,
};
use crate::data::{
    field_engine::scalar::validate_optional_single_line_text,
    json::{validate_serializable_for_storage, LosslessJsonSource, LosslessJsonValue},
    schema::SchemaVersion,
};

pub(crate) mod document_creation;
pub(crate) mod document_materialization;
pub(crate) mod document_reconciliation;
pub(crate) mod document_save;
mod document_snapshot;

const ORPHAN_OPTION_KNOWN_KEYS: &[&str] = &["label"];
const ORPHAN_FIELD_KNOWN_KEYS: &[&str] = &["label", "kind", "options"];
const DOCUMENT_KNOWN_KEYS: &[&str] = &[
    "schemaVersion",
    "artifactType",
    "documentId",
    "templateId",
    "templateRevision",
    "name",
    "englishName",
    "glossarySummary",
    "glossaryExcluded",
    "fieldValues",
    "orphanedFieldDefinitions",
    "createdAtUtc",
    "updatedAtUtc",
];

/// 문서 트리 배치는 별도 artifact가 소유하므로 Document top-level에서만 예약한다.
pub(super) const FORBIDDEN_TREE_KEYS: &[&str] =
    &["parentId", "parentDocumentId", "treeOrder", "children"];

#[derive(Clone, PartialEq)]
pub(crate) struct OrphanedOptionDefinition {
    label: String,
    extra: ExtraFields,
}

impl fmt::Debug for OrphanedOptionDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrphanedOptionDefinition")
            .field("label_redacted", &true)
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl OrphanedOptionDefinition {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    fn contains_unknown_storage_data(&self) -> bool {
        !self.extra.is_empty()
    }

    fn validate_structure(&self) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, ORPHAN_OPTION_KNOWN_KEYS)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrphanedOptionDefinitionWire {
    label: String,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl From<OrphanedOptionDefinitionWire> for OrphanedOptionDefinition {
    fn from(wire: OrphanedOptionDefinitionWire) -> Self {
        Self {
            label: wire.label,
            extra: wire.extra,
        }
    }
}

impl From<&OrphanedOptionDefinition> for OrphanedOptionDefinitionWire {
    fn from(option: &OrphanedOptionDefinition) -> Self {
        Self {
            label: option.label.clone(),
            extra: option.extra.clone(),
        }
    }
}

/// Template에서 더는 찾지 못하는 값도 마지막 label/type을 잃지 않게 하는 snapshot이다.
#[derive(Clone, PartialEq)]
pub(crate) struct OrphanedFieldDefinition {
    label: String,
    kind: FieldKind,
    options: BTreeMap<OptionId, OrphanedOptionDefinition>,
    extra: ExtraFields,
}

impl fmt::Debug for OrphanedFieldDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrphanedFieldDefinition")
            .field("label_redacted", &true)
            .field("kind", &self.kind)
            .field("option_count", &self.options.len())
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl OrphanedFieldDefinition {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn kind(&self) -> FieldKind {
        self.kind
    }

    pub(crate) fn options(&self) -> &BTreeMap<OptionId, OrphanedOptionDefinition> {
        &self.options
    }

    /// 재결합으로 snapshot을 제거할 때 의미를 모르는 metadata가 사라지지 않게 한다.
    fn contains_unknown_storage_data(&self) -> bool {
        !self.extra.is_empty()
            || self
                .options
                .values()
                .any(OrphanedOptionDefinition::contains_unknown_storage_data)
    }

    fn validate_structure(
        &self,
        option_ids: &mut BTreeSet<OptionId>,
    ) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, ORPHAN_FIELD_KNOWN_KEYS)?;
        for (id, option) in &self.options {
            if !option_ids.insert(*id) {
                return Err(ArtifactValidationError::duplicate_option_id());
            }
            option.validate_structure()?;
        }
        Ok(())
    }

    fn validate_value(&self, value: &FieldValue) -> Result<(), ArtifactValidationError> {
        if !value.matches_kind_or_unset(self.kind) {
            return Err(ArtifactValidationError::orphan_value_kind_mismatch());
        }

        let selected: BTreeSet<OptionId> = match self.kind {
            FieldKind::SingleChoice => value.single_choice().into_iter().collect(),
            FieldKind::MultiChoice => value
                .multi_choice()
                .unwrap_or_default()
                .iter()
                .copied()
                .collect(),
            _ => {
                if self.options.is_empty() {
                    return Ok(());
                }
                return Err(ArtifactValidationError::orphan_option_mismatch());
            }
        };
        let snapshots: BTreeSet<_> = self.options.keys().copied().collect();
        if selected != snapshots {
            return Err(ArtifactValidationError::orphan_option_mismatch());
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrphanedFieldDefinitionWire {
    label: String,
    kind: FieldKindWire,
    options: BTreeMap<OptionId, OrphanedOptionDefinitionWire>,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl From<OrphanedFieldDefinitionWire> for OrphanedFieldDefinition {
    fn from(wire: OrphanedFieldDefinitionWire) -> Self {
        Self {
            label: wire.label,
            kind: wire.kind.into(),
            options: wire
                .options
                .into_iter()
                .map(|(id, option)| (id, option.into()))
                .collect(),
            extra: wire.extra,
        }
    }
}

impl From<&OrphanedFieldDefinition> for OrphanedFieldDefinitionWire {
    fn from(field: &OrphanedFieldDefinition) -> Self {
        Self {
            label: field.label.clone(),
            kind: field.kind.into(),
            options: field
                .options
                .iter()
                .map(|(id, option)| (*id, OrphanedOptionDefinitionWire::from(option)))
                .collect(),
            extra: field.extra.clone(),
        }
    }
}

/// Document v1 production model은 Template 결합과 tree placement 없이 독립적으로 검증된다.
#[derive(Clone, PartialEq)]
pub(crate) struct DocumentArtifact {
    schema_version: SchemaVersion,
    artifact_type: ArtifactType,
    document_id: DocumentId,
    template_id: TemplateId,
    template_revision: TemplateRevision,
    name: String,
    english_name: String,
    glossary_summary: String,
    glossary_excluded: bool,
    field_values: BTreeMap<FieldId, FieldValue>,
    orphaned_field_definitions: BTreeMap<FieldId, OrphanedFieldDefinition>,
    created_at_utc: String,
    updated_at_utc: String,
    extra: ExtraFields,
    lossless_source: LosslessJsonSource,
}

impl fmt::Debug for DocumentArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentArtifact")
            .field("schema_version", &self.schema_version)
            .field("template_revision", &self.template_revision)
            .field("name_redacted", &true)
            .field("field_value_count", &self.field_values.len())
            .field(
                "orphaned_field_definition_count",
                &self.orphaned_field_definitions.len(),
            )
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl DocumentArtifact {
    pub(super) fn try_from_wire(wire: DocumentWire) -> Result<Self, ArtifactValidationError> {
        let artifact = Self {
            schema_version: wire.schema_version,
            artifact_type: wire.artifact_type.into(),
            document_id: wire.document_id,
            template_id: wire.template_id,
            template_revision: wire.template_revision,
            name: wire.name,
            english_name: wire.english_name,
            glossary_summary: wire.glossary_summary,
            glossary_excluded: wire.glossary_excluded,
            field_values: wire
                .field_values
                .into_iter()
                .map(|(id, value)| (id, value.into()))
                .collect(),
            orphaned_field_definitions: wire
                .orphaned_field_definitions
                .into_iter()
                .map(|(id, definition)| (id, definition.into()))
                .collect(),
            created_at_utc: wire.created_at_utc,
            updated_at_utc: wire.updated_at_utc,
            extra: wire.extra,
            lossless_source: LosslessJsonSource::default(),
        };
        artifact.validate_structure()?;
        Ok(artifact)
    }

    pub(crate) fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub(crate) fn document_id(&self) -> DocumentId {
        self.document_id
    }

    pub(crate) fn template_id(&self) -> TemplateId {
        self.template_id
    }

    pub(crate) fn template_revision(&self) -> TemplateRevision {
        self.template_revision
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn english_name(&self) -> &str {
        &self.english_name
    }

    pub(crate) fn glossary_summary(&self) -> &str {
        &self.glossary_summary
    }

    pub(crate) fn glossary_excluded(&self) -> bool {
        self.glossary_excluded
    }

    pub(crate) fn field_values(&self) -> &BTreeMap<FieldId, FieldValue> {
        &self.field_values
    }

    pub(crate) fn orphaned_field_definitions(&self) -> &BTreeMap<FieldId, OrphanedFieldDefinition> {
        &self.orphaned_field_definitions
    }

    pub(crate) fn created_at_utc(&self) -> &str {
        &self.created_at_utc
    }

    pub(crate) fn updated_at_utc(&self) -> &str {
        &self.updated_at_utc
    }

    pub(super) fn set_lossless_source(&mut self, source: LosslessJsonValue) {
        self.lossless_source
            .attach_artifact_source(&ARTIFACT_PROVENANCE_AUTHORITY, source);
    }

    pub(super) fn lossless_source(&self) -> Option<&LosslessJsonValue> {
        self.lossless_source
            .artifact_value(&ARTIFACT_PROVENANCE_AUTHORITY)
    }

    /// 검증된 Document 변경을 새 기준으로 만들면 제거된 metadata는 source에서도 함께 사라진다.
    pub(super) fn rebase_lossless_source(&mut self) -> Result<(), ArtifactValidationError> {
        let wire = DocumentWire::from(&*self);
        self.lossless_source
            .rebase_artifact(&ARTIFACT_PROVENANCE_AUTHORITY, &wire)
            .map_err(map_json_storage_error)
    }

    /// 다른 aggregate에서 복사한 FieldValue provenance를 정확한 Document 소유 위치에만 붙인다.
    pub(super) fn graft_current_default_provenance(
        &mut self,
        provenance: CurrentDefaultFieldValueProvenance,
    ) -> bool {
        if provenance.template_id != self.template_id
            || self.field_values.get(&provenance.field_id) != Some(&provenance.value)
        {
            return false;
        }
        self.lossless_source.graft_artifact_field_value(
            &ARTIFACT_PROVENANCE_AUTHORITY,
            provenance.field_id,
            &provenance.source,
        )
    }

    pub(super) fn graft_initial_default_provenance(
        &mut self,
        provenance: InitialDefaultFieldValueProvenance,
    ) -> bool {
        if provenance.template_id != self.template_id
            || self.field_values.get(&provenance.field_id) != Some(&provenance.value)
        {
            return false;
        }
        self.lossless_source.graft_artifact_field_value(
            &ARTIFACT_PROVENANCE_AUTHORITY,
            provenance.field_id,
            &provenance.source,
        )
    }

    pub(super) fn validate_structure(&self) -> Result<(), ArtifactValidationError> {
        if ![1, 2, 3, 4, 5, DOCUMENT_SCHEMA_VERSION.get()].contains(&self.schema_version.get())
            || (self.schema_version.get() == 1
                && self.field_values.values().any(|v| {
                    matches!(
                        v.validation_view(),
                        crate::data::field_engine::validation::FieldValueView::NumberUnknown
                    )
                }))
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.schema_version.get() < 4 && self.field_values.values().any(|v| v.group().is_some())
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.schema_version.get() < 5
            && self.field_values.values().any(|value| {
                matches!(
                    value.kind(),
                    Some(super::FieldKind::Relation | super::FieldKind::DocumentLink)
                )
            })
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.schema_version.get() < 6
            && self
                .field_values
                .values()
                .any(FieldValue::has_named_relation)
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        fn contains_self_relation(value: &FieldValue, document: DocumentId) -> bool {
            value
                .relations()
                .is_some_and(|links| links.iter().any(|link| link.document() == document))
                || value.group().is_some_and(|group| {
                    group.instances.values().any(|instance| {
                        instance
                            .values
                            .values()
                            .any(|value| contains_self_relation(value, document))
                    })
                })
        }
        if let Some((field, _)) = self
            .field_values
            .iter()
            .find(|(_, value)| contains_self_relation(value, self.document_id))
        {
            return Err(ArtifactValidationError::field_value_kind_mismatch().at_field(*field));
        }
        if self.schema_version.get() < 3
            && self.field_values.values().any(|v| {
                matches!(
                    v.kind(),
                    Some(super::FieldKind::Image | super::FieldKind::File | super::FieldKind::Url)
                )
            })
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.artifact_type != ArtifactType::Document {
            return Err(ArtifactValidationError::artifact_type_mismatch());
        }
        for value in [&self.english_name, &self.glossary_summary] {
            validate_optional_single_line_text(value).map_err(|error| {
                ArtifactValidationError::invalid_scalar_value(
                    error.category(),
                    ArtifactScalarValueLocation::DocumentField,
                )
            })?;
        }
        validate_extra_keys(&self.extra, DOCUMENT_KNOWN_KEYS)?;
        if self
            .extra
            .keys()
            .any(|key| FORBIDDEN_TREE_KEYS.contains(&key.as_str()))
        {
            return Err(ArtifactValidationError::forbidden_tree_key());
        }
        validate_timestamps(&self.created_at_utc, &self.updated_at_utc)?;
        for (field_id, value) in &self.field_values {
            let location = if self.orphaned_field_definitions.contains_key(field_id) {
                ArtifactScalarValueLocation::OrphanField
            } else {
                ArtifactScalarValueLocation::DocumentField
            };
            value.validate_structure(location)?;
        }
        let mut option_ids = BTreeSet::new();
        for (field_id, definition) in &self.orphaned_field_definitions {
            let value = self
                .field_values
                .get(field_id)
                .ok_or_else(ArtifactValidationError::orphan_missing_value)?;
            definition.validate_structure(&mut option_ids)?;
            definition.validate_value(value)?;
        }
        Ok(())
    }

    /// aggregate 의미 검증 뒤 실제 private wire 전체를 공통 storage 경계로 검사한다.
    pub(super) fn validate_storage(&self) -> Result<(), ArtifactValidationError> {
        self.validate_structure()?;
        validate_serializable_for_storage(&DocumentWire::from(self)).map_err(map_json_storage_error)
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_content_for_test(
        &mut self,
        field_id: FieldId,
        reserved_key: &str,
    ) {
        self.field_values
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field")
            .corrupt_rich_text_content_for_test(reserved_key);
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_depth_for_test(
        &mut self,
        field_id: FieldId,
        value: serde_json::Value,
    ) {
        self.field_values
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field")
            .corrupt_rich_text_value_for_test("testOnlyDepth", value);
    }

    #[cfg(test)]
    pub(super) fn replace_rich_text_content_for_test(
        &mut self,
        field_id: FieldId,
        content: serde_json::Map<String, serde_json::Value>,
    ) {
        self.field_values
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field")
            .replace_rich_text_content_for_test(content);
    }

    #[cfg(test)]
    pub(super) fn corrupt_extra_depth_for_test(&mut self, value: serde_json::Value) {
        self.extra.insert("testOnlyDepth".to_owned(), value);
    }

    #[cfg(test)]
    pub(super) fn corrupt_extra_for_test(&mut self, key: &str, value: serde_json::Value) {
        self.extra.insert(key.to_owned(), value);
    }

    #[cfg(test)]
    pub(super) fn corrupt_scalar_value_for_test(&mut self, field_id: FieldId, raw: &str) {
        self.field_values
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field")
            .corrupt_scalar_for_test(raw);
    }

    #[cfg(test)]
    pub(super) fn corrupt_multi_choice_value_for_test(
        &mut self,
        field_id: FieldId,
        option_ids: Vec<OptionId>,
    ) {
        self.field_values
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field")
            .corrupt_multi_choice_for_test(option_ids);
    }

    #[cfg(test)]
    pub(super) fn corrupt_orphan_kind_for_test(&mut self, field_id: FieldId, kind: FieldKind) {
        self.orphaned_field_definitions
            .get_mut(&field_id)
            .expect("test fixture must contain the requested orphan snapshot")
            .kind = kind;
    }

    #[cfg(test)]
    pub(super) fn duplicate_orphan_option_for_test(
        &mut self,
        source_field_id: FieldId,
        target_field_id: FieldId,
        option_id: OptionId,
    ) {
        let option = self.orphaned_field_definitions[&source_field_id].options[&option_id].clone();
        self.orphaned_field_definitions
            .get_mut(&target_field_id)
            .expect("test fixture must contain the target orphan snapshot")
            .options
            .insert(option_id, option);
    }

    #[cfg(test)]
    pub(super) fn remove_orphan_option_for_test(&mut self, field_id: FieldId, option_id: OptionId) {
        self.orphaned_field_definitions
            .get_mut(&field_id)
            .expect("test fixture must contain the requested orphan snapshot")
            .options
            .remove(&option_id)
            .expect("test fixture must contain the requested orphan Option");
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DocumentWire {
    schema_version: SchemaVersion,
    artifact_type: ArtifactTypeWire,
    document_id: DocumentId,
    template_id: TemplateId,
    template_revision: TemplateRevision,
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    english_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    glossary_summary: String,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    glossary_excluded: bool,
    field_values: BTreeMap<FieldId, FieldValueWire>,
    orphaned_field_definitions: BTreeMap<FieldId, OrphanedFieldDefinitionWire>,
    created_at_utc: String,
    updated_at_utc: String,
    #[serde(flatten)]
    extra: ExtraFields,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

impl From<&DocumentArtifact> for DocumentWire {
    fn from(artifact: &DocumentArtifact) -> Self {
        Self {
            schema_version: artifact.schema_version,
            artifact_type: artifact.artifact_type.into(),
            document_id: artifact.document_id,
            template_id: artifact.template_id,
            template_revision: artifact.template_revision,
            name: artifact.name.clone(),
            english_name: artifact.english_name.clone(),
            glossary_summary: artifact.glossary_summary.clone(),
            glossary_excluded: artifact.glossary_excluded,
            field_values: artifact
                .field_values
                .iter()
                .map(|(id, value)| (*id, FieldValueWire::from(value)))
                .collect(),
            orphaned_field_definitions: artifact
                .orphaned_field_definitions
                .iter()
                .map(|(id, field)| (*id, OrphanedFieldDefinitionWire::from(field)))
                .collect(),
            created_at_utc: artifact.created_at_utc.clone(),
            updated_at_utc: artifact.updated_at_utc.clone(),
            extra: artifact.extra.clone(),
        }
    }
}
