use serde::{Deserialize, Serialize};

use super::super::{
    migration::{MigrationError, MigrationRegistry, MigrationStep},
    schema::SchemaVersion,
};

// This header also marks the bounded draft/version policy. Older applications
// must reject these artifacts instead of bypassing the new persistence rules.
pub(crate) const TEMPLATE_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new_unchecked(8);
pub(crate) const DOCUMENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new_unchecked(7);

static TEMPLATE_MIGRATION_STEPS: [MigrationStep; 7] = [
    MigrationStep::new(
        1,
        2,
        migrate_guide_schema,
        validate_template_v1,
        validate_template_v2,
    ),
    MigrationStep::new(
        2,
        3,
        migrate_template_v3,
        validate_template_v2,
        validate_template_v3,
    ),
    MigrationStep::new(
        3,
        4,
        migrate_template_v4,
        validate_template_v3,
        validate_template_v4,
    ),
    MigrationStep::new(
        4,
        5,
        migrate_template_v5,
        validate_template_v4,
        validate_template_v5,
    ),
    MigrationStep::new(
        5,
        6,
        migrate_template_v6,
        validate_template_v5,
        validate_template_v6,
    ),
    MigrationStep::new(
        6,
        7,
        migrate_template_v7,
        validate_template_v6,
        validate_template_v7,
    ),
    MigrationStep::new(
        7,
        8,
        migrate_template_v8,
        validate_template_v7,
        validate_template_v8,
    ),
];
fn validate_template_version(
    value: &serde_json::Value,
    version: u64,
) -> Result<(), super::super::migration::MigrationStepError> {
    use super::super::migration::MigrationStepError;
    if value["schemaVersion"].as_u64() != Some(version) {
        return Err(MigrationStepError::new("unexpected Template schema"));
    }
    let bytes =
        serde_json::to_vec(value).map_err(|_| MigrationStepError::new("invalid Template JSON"))?;
    super::codec::decode_template(&bytes)
        .map(|_| ())
        .map_err(|_| MigrationStepError::new("invalid Template definition"))
}
fn validate_template_v1(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 1)
}
fn validate_template_v2(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 2)
}
fn migrate_guide_schema(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    if v["fields"]
        .as_object()
        .is_none_or(|fields| fields.values().any(|f| f.get("writingGuide").is_some()))
    {
        return Err(super::super::migration::MigrationStepError::new(
            "legacy writing guide member must remain uninterpreted",
        ));
    }
    v["schemaVersion"] = 2.into();
    Ok(v)
}
static DOCUMENT_MIGRATION_STEPS: [MigrationStep; 6] = [
    MigrationStep::new(
        1,
        2,
        migrate_document_v2,
        validate_document_v1,
        validate_document_v2,
    ),
    MigrationStep::new(
        2,
        3,
        migrate_document_v3,
        validate_document_v2,
        validate_document_v3,
    ),
    MigrationStep::new(
        3,
        4,
        migrate_document_v4,
        validate_document_v3,
        validate_document_v4,
    ),
    MigrationStep::new(
        4,
        5,
        migrate_document_v5,
        validate_document_v4,
        validate_document_v5,
    ),
    MigrationStep::new(
        5,
        6,
        migrate_document_v6,
        validate_document_v5,
        validate_document_v6,
    ),
    MigrationStep::new(
        6,
        7,
        migrate_document_v7,
        validate_document_v6,
        validate_document_v7,
    ),
];
fn validate_template_v3(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 3)
}
fn migrate_template_v3(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    if v.get("sections").is_some()
        || v["fields"].as_object().is_none_or(|fields| {
            fields.values().any(|f| {
                f["configuration"]["kind"].as_str() == Some("number")
                    && (f["configuration"].get("minimum").is_some()
                        || f["configuration"].get("maximum").is_some())
            })
        })
    {
        return Err(super::super::migration::MigrationStepError::new(
            "legacy metadata conflicts with Template v3",
        ));
    }
    v["schemaVersion"] = 3.into();
    Ok(v)
}
fn migrate_document_v2(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 2.into();
    Ok(v)
}
fn validate_document_version(
    v: &serde_json::Value,
    version: u64,
) -> Result<(), super::super::migration::MigrationStepError> {
    use super::super::migration::MigrationStepError;
    if v["schemaVersion"].as_u64() != Some(version) {
        return Err(MigrationStepError::new("unexpected Document schema"));
    }
    let bytes =
        serde_json::to_vec(v).map_err(|_| MigrationStepError::new("invalid Document JSON"))?;
    super::codec::decode_document(&bytes)
        .map(|_| ())
        .map_err(|_| MigrationStepError::new("invalid Document"))
}
fn validate_document_v1(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 1)
}
fn validate_document_v2(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 2)
}

/// 같은 숫자여도 Template과 Document가 각자 registry와 상수를 소유한다.
pub(crate) fn template_migration_registry() -> Result<MigrationRegistry<'static>, MigrationError> {
    MigrationRegistry::try_new(TEMPLATE_SCHEMA_VERSION.get(), &TEMPLATE_MIGRATION_STEPS)
}

pub(crate) fn document_migration_registry() -> Result<MigrationRegistry<'static>, MigrationError> {
    MigrationRegistry::try_new(DOCUMENT_SCHEMA_VERSION.get(), &DOCUMENT_MIGRATION_STEPS)
}

pub(crate) const fn template_migration_step_count() -> usize {
    TEMPLATE_MIGRATION_STEPS.len()
}

pub(crate) const fn document_migration_step_count() -> usize {
    DOCUMENT_MIGRATION_STEPS.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactType {
    Template,
    Document,
    DocumentLayout,
}

/// Serde discriminator는 private wire 모델에서만 사용한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ArtifactTypeWire {
    Template,
    Document,
    DocumentLayout,
}

impl From<ArtifactTypeWire> for ArtifactType {
    fn from(value: ArtifactTypeWire) -> Self {
        match value {
            ArtifactTypeWire::Template => Self::Template,
            ArtifactTypeWire::Document => Self::Document,
            ArtifactTypeWire::DocumentLayout => Self::DocumentLayout,
        }
    }
}

impl From<ArtifactType> for ArtifactTypeWire {
    fn from(value: ArtifactType) -> Self {
        match value {
            ArtifactType::Template => Self::Template,
            ArtifactType::Document => Self::Document,
            ArtifactType::DocumentLayout => Self::DocumentLayout,
        }
    }
}

impl ArtifactType {
    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::Template => "template",
            Self::Document => "document",
            Self::DocumentLayout => "documentLayout",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactHeader {
    artifact_type: ArtifactType,
    schema_version: SchemaVersion,
}

impl ArtifactHeader {
    pub(super) const fn new(artifact_type: ArtifactType, schema_version: SchemaVersion) -> Self {
        Self {
            artifact_type,
            schema_version,
        }
    }

    pub(crate) const fn artifact_type(self) -> ArtifactType {
        self.artifact_type
    }

    pub(crate) const fn schema_version(self) -> SchemaVersion {
        self.schema_version
    }
}

/// 배치는 문서 본문과 독립적으로 버전을 진화시킨다.
pub(crate) fn layout_migration_registry() -> Result<MigrationRegistry<'static>, MigrationError> {
    MigrationRegistry::try_new(1, &[])
}

fn validate_template_v4(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 4)
}
fn validate_document_v3(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 3)
}
fn migrate_template_v4(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 4.into();
    Ok(v)
}
fn migrate_document_v3(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 3.into();
    Ok(v)
}

fn validate_template_v5(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 5)
}
fn validate_document_v4(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 4)
}
fn migrate_template_v5(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 5.into();
    Ok(v)
}
fn migrate_document_v4(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 4.into();
    Ok(v)
}

fn validate_template_v6(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 6)
}
fn validate_document_v5(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 5)
}
fn migrate_template_v6(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 6.into();
    Ok(v)
}
fn migrate_document_v5(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 5.into();
    Ok(v)
}

fn validate_template_v7(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 7)
}

fn migrate_template_v7(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 7.into();
    Ok(v)
}

fn validate_document_v6(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 6)
}

fn migrate_document_v6(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 6.into();
    Ok(v)
}

fn validate_template_v8(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_template_version(v, 8)
}
fn migrate_template_v8(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 8.into();
    Ok(v)
}
fn validate_document_v7(
    v: &serde_json::Value,
) -> Result<(), super::super::migration::MigrationStepError> {
    validate_document_version(v, 7)
}
fn migrate_document_v7(
    mut v: serde_json::Value,
) -> Result<serde_json::Value, super::super::migration::MigrationStepError> {
    v["schemaVersion"] = 7.into();
    Ok(v)
}
