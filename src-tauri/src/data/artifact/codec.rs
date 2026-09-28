use std::fmt;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::document::{DocumentArtifact, DocumentWire};
use super::schema::{
    document_migration_registry, template_migration_registry, ArtifactHeader, ArtifactType,
};
use super::template::{TemplateArtifact, TemplateWire};
use super::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactValidationError, ArtifactValidationErrorCategory, ARTIFACT_PROVENANCE_AUTHORITY,
};
use crate::data::compatibility::{SchemaCompatibility, SchemaSupportPolicy};
use crate::data::field_engine::{
    choice::ChoiceValidationErrorCategory,
    rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
    scalar::ScalarValueErrorCategory,
};
use crate::data::json::{
    parse_strict_json_object, parse_strict_lossless_json_object,
    to_deterministic_artifact_json_bytes, to_deterministic_json_bytes_categorized,
    JsonStorageErrorCategory, LosslessJsonValue, StrictJsonErrorCategory,
};
use crate::data::migration::MigrationRegistry;
use crate::data::schema::SchemaVersion;

const ARTIFACT_TYPE_FIELD: &str = "artifactType";
const SCHEMA_VERSION_FIELD: &str = "schemaVersion";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactCodecErrorCategory {
    InvalidEncoding,
    MalformedJson,
    DuplicateJsonKey,
    RootTypeMismatch,
    MissingArtifactType,
    InvalidArtifactType,
    MissingSchemaVersion,
    InvalidSchemaVersion,
    UnexpectedArtifactType,
    UnsupportedFuture,
    UnsupportedPast,
    MigrationRequired,
    InvalidRegistry,
    InvalidStructure,
    ArtifactSemanticValidationFailure,
    SerializationFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactCodecStage {
    StrictJson,
    Header,
    Compatibility,
    Deserialize,
    StructuralValidation,
    SemanticValidation,
    Serialize,
}

/// Codec 오류는 사용자 JSON 본문·경로·credential을 보존하거나 출력하지 않는다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactCodecError {
    category: ArtifactCodecErrorCategory,
    stage: ArtifactCodecStage,
    artifact_type: Option<ArtifactType>,
    schema_version: Option<SchemaVersion>,
    validation_category: Option<ArtifactValidationErrorCategory>,
    scalar_category: Option<ScalarValueErrorCategory>,
    scalar_location: Option<ArtifactScalarValueLocation>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    choice_location: Option<ArtifactChoiceValueLocation>,
    choice_option_id: Option<super::OptionId>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_location: Option<ArtifactRichTextValueLocation>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
    rich_text_node_depth: Option<usize>,
    detail: &'static str,
}

impl ArtifactCodecError {
    pub(crate) const fn category(self) -> ArtifactCodecErrorCategory {
        self.category
    }

    pub(crate) const fn stage(self) -> ArtifactCodecStage {
        self.stage
    }

    pub(crate) const fn artifact_type(self) -> Option<ArtifactType> {
        self.artifact_type
    }

    pub(crate) const fn schema_version(self) -> Option<SchemaVersion> {
        self.schema_version
    }

    pub(crate) const fn validation_category(self) -> Option<ArtifactValidationErrorCategory> {
        self.validation_category
    }

    pub(crate) const fn scalar_category(self) -> Option<ScalarValueErrorCategory> {
        self.scalar_category
    }

    pub(crate) const fn scalar_location(self) -> Option<ArtifactScalarValueLocation> {
        self.scalar_location
    }

    pub(crate) const fn choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        self.choice_category
    }

    pub(crate) const fn choice_location(self) -> Option<ArtifactChoiceValueLocation> {
        self.choice_location
    }

    pub(crate) const fn choice_option_id(self) -> Option<super::OptionId> {
        self.choice_option_id
    }

    pub(crate) const fn rich_text_category(self) -> Option<RichTextValidationErrorCategory> {
        self.rich_text_category
    }

    pub(crate) const fn rich_text_location(self) -> Option<ArtifactRichTextValueLocation> {
        self.rich_text_location
    }

    pub(crate) const fn rich_text_structure_location(self) -> Option<RichTextErrorLocation> {
        self.rich_text_structure_location
    }

    pub(crate) const fn rich_text_node_depth(self) -> Option<usize> {
        self.rich_text_node_depth
    }

    const fn new(
        category: ArtifactCodecErrorCategory,
        stage: ArtifactCodecStage,
        artifact_type: Option<ArtifactType>,
        schema_version: Option<SchemaVersion>,
        detail: &'static str,
    ) -> Self {
        Self {
            category,
            stage,
            artifact_type,
            schema_version,
            validation_category: None,
            scalar_category: None,
            scalar_location: None,
            choice_category: None,
            choice_location: None,
            choice_option_id: None,
            rich_text_category: None,
            rich_text_location: None,
            rich_text_structure_location: None,
            rich_text_node_depth: None,
            detail,
        }
    }

    fn strict(category: StrictJsonErrorCategory) -> Self {
        let category = match category {
            StrictJsonErrorCategory::InvalidEncoding => ArtifactCodecErrorCategory::InvalidEncoding,
            StrictJsonErrorCategory::MalformedJson => ArtifactCodecErrorCategory::MalformedJson,
            StrictJsonErrorCategory::DuplicateKey => ArtifactCodecErrorCategory::DuplicateJsonKey,
            StrictJsonErrorCategory::ReservedKey => ArtifactCodecErrorCategory::InvalidStructure,
            StrictJsonErrorCategory::NestingDepthExceeded => {
                ArtifactCodecErrorCategory::InvalidStructure
            }
            StrictJsonErrorCategory::RootTypeMismatch => {
                ArtifactCodecErrorCategory::RootTypeMismatch
            }
        };
        Self::new(
            category,
            ArtifactCodecStage::StrictJson,
            None,
            None,
            "artifact input did not pass strict JSON inspection",
        )
    }

    fn validation(kind: ArtifactType, error: ArtifactValidationError) -> Self {
        let is_semantic = matches!(
            error.category(),
            ArtifactValidationErrorCategory::InvalidScalarValue
                | ArtifactValidationErrorCategory::InvalidChoiceValue
                | ArtifactValidationErrorCategory::InvalidRichTextValue
        );
        let mut codec_error = Self::new(
            if is_semantic {
                ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure
            } else {
                ArtifactCodecErrorCategory::InvalidStructure
            },
            if is_semantic {
                ArtifactCodecStage::SemanticValidation
            } else {
                ArtifactCodecStage::StructuralValidation
            },
            Some(kind),
            None,
            if is_semantic {
                "artifact contains an invalid known semantic value"
            } else {
                "artifact violates a structural invariant"
            },
        );
        codec_error.validation_category = Some(error.category());
        codec_error.scalar_category = error.scalar_category();
        codec_error.scalar_location = error.scalar_location();
        codec_error.choice_category = error.choice_category();
        codec_error.choice_location = error.choice_location();
        codec_error.choice_option_id = error.choice_option_id();
        codec_error.rich_text_category = error.rich_text_category();
        codec_error.rich_text_location = error.rich_text_location();
        codec_error.rich_text_structure_location = error.rich_text_structure_location();
        codec_error.rich_text_node_depth = error.rich_text_node_depth();
        codec_error
    }
}

impl fmt::Debug for ArtifactCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactCodecError")
            .field("category", &self.category)
            .field("stage", &self.stage)
            .field("artifact_type", &self.artifact_type)
            .field("schema_version", &self.schema_version)
            .field("validation_category", &self.validation_category)
            .field("scalar_category", &self.scalar_category)
            .field("scalar_location", &self.scalar_location)
            .field("choice_category", &self.choice_category)
            .field("choice_location", &self.choice_location)
            .field("choice_option_id", &self.choice_option_id)
            .field("rich_text_category", &self.rich_text_category)
            .field("rich_text_location", &self.rich_text_location)
            .field(
                "rich_text_structure_location",
                &self.rich_text_structure_location,
            )
            .field("rich_text_node_depth", &self.rich_text_node_depth)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for ArtifactCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact codec failed ({:?}) at {:?}",
            self.category, self.stage
        )?;
        if let Some(kind) = self.artifact_type {
            write!(formatter, " for {}", kind.wire_name())?;
        }
        if let Some(version) = self.schema_version {
            write!(formatter, " schema {}", version.get())?;
        }
        if let Some(category) = self.validation_category {
            write!(formatter, " ({category:?})")?;
        }
        if let (Some(category), Some(location)) = (self.scalar_category, self.scalar_location) {
            write!(formatter, " ({category:?} at {location:?})")?;
        }
        if let (Some(category), Some(location)) = (self.choice_category, self.choice_location) {
            write!(formatter, " ({category:?} at {location:?})")?;
        }
        if let Some(option_id) = self.choice_option_id {
            write!(formatter, " (OptionId {option_id})")?;
        }
        if let (Some(category), Some(location), Some(structure_location)) = (
            self.rich_text_category,
            self.rich_text_location,
            self.rich_text_structure_location,
        ) {
            write!(
                formatter,
                " ({category:?} at {location:?}/{structure_location:?})"
            )?;
        }
        if let Some(depth) = self.rich_text_node_depth {
            write!(formatter, " (node depth {depth})")?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl std::error::Error for ArtifactCodecError {}

pub(crate) fn inspect_artifact_header(bytes: &[u8]) -> Result<ArtifactHeader, ArtifactCodecError> {
    let value = parse_strict_json_object(bytes)
        .map_err(|error| ArtifactCodecError::strict(error.category()))?;
    header_from_value(&value)
}

pub(crate) fn decode_template(bytes: &[u8]) -> Result<TemplateArtifact, ArtifactCodecError> {
    let header = inspect_for_decode(bytes, ArtifactType::Template)?;
    // v1은 현재 typed wire로 직접 읽고, 작성 가이드가 추가된 v2부터는 registry로 호환성을 확인한다.
    if ![1, 2, 3, 4, 5, 6].contains(&header.schema_version().get()) {
        ensure_compatible(header, template_migration_registry)?;
    }
    if header.schema_version().get() < 3 {
        let value = parse_strict_json_object(bytes)
            .map_err(|e| ArtifactCodecError::strict(e.category()))?;
        if value.get("sections").is_some()
            || value["fields"].as_object().is_some_and(|fields| {
                fields.values().any(|f| {
                    f["configuration"]["kind"].as_str() == Some("number")
                        && (f["configuration"].get("minimum").is_some()
                            || f["configuration"].get("maximum").is_some())
                })
            })
        {
            return Err(ArtifactCodecError::new(
                ArtifactCodecErrorCategory::InvalidStructure,
                ArtifactCodecStage::StructuralValidation,
                Some(ArtifactType::Template),
                Some(header.schema_version()),
                "legacy metadata conflicts with Template v3; source retained",
            ));
        }
    }
    let lossless_source = parse_lossless_source(bytes)?;
    let wire: TemplateWire = deserialize(bytes, ArtifactType::Template)?;
    let mut artifact = TemplateArtifact::try_from_wire(wire)
        .map_err(|error| ArtifactCodecError::validation(ArtifactType::Template, error))?;
    artifact.set_lossless_source(lossless_source);
    Ok(artifact)
}

pub(crate) fn decode_document(bytes: &[u8]) -> Result<DocumentArtifact, ArtifactCodecError> {
    let header = inspect_for_decode(bytes, ArtifactType::Document)?;
    if ![1, 2, 3, 4, 5].contains(&header.schema_version().get()) {
        ensure_compatible(header, document_migration_registry)?;
    }
    let lossless_source = parse_lossless_source(bytes)?;
    let wire: DocumentWire = deserialize(bytes, ArtifactType::Document)?;
    let mut artifact = DocumentArtifact::try_from_wire(wire)
        .map_err(|error| ArtifactCodecError::validation(ArtifactType::Document, error))?;
    artifact.set_lossless_source(lossless_source);
    Ok(artifact)
}

pub(crate) fn encode_template(artifact: &TemplateArtifact) -> Result<Vec<u8>, ArtifactCodecError> {
    artifact
        .validate_storage()
        .map_err(|error| ArtifactCodecError::validation(ArtifactType::Template, error))?;
    encode(
        &TemplateWire::from(artifact),
        ArtifactType::Template,
        artifact.lossless_source(),
    )
}

pub(crate) fn encode_document(artifact: &DocumentArtifact) -> Result<Vec<u8>, ArtifactCodecError> {
    artifact
        .validate_storage()
        .map_err(|error| ArtifactCodecError::validation(ArtifactType::Document, error))?;
    encode(
        &DocumentWire::from(artifact),
        ArtifactType::Document,
        artifact.lossless_source(),
    )
}

pub(crate) fn decode_layout(
    bytes: &[u8],
) -> Result<super::layout::DocumentLayout, ArtifactCodecError> {
    let header = inspect_for_decode(bytes, ArtifactType::DocumentLayout)?;
    ensure_compatible(header, super::schema::layout_migration_registry)?;
    let mut layout: super::layout::DocumentLayout =
        deserialize(bytes, ArtifactType::DocumentLayout)?;
    layout.validate().map_err(layout_error)?;
    layout.source = Some(parse_lossless_source(bytes)?);
    Ok(layout)
}
pub(crate) fn encode_layout(
    layout: &super::layout::DocumentLayout,
) -> Result<Vec<u8>, ArtifactCodecError> {
    layout.validate().map_err(layout_error)?;
    encode(layout, ArtifactType::DocumentLayout, layout.source.as_ref())
}
fn layout_error(_: super::layout::LayoutError) -> ArtifactCodecError {
    ArtifactCodecError::new(
        ArtifactCodecErrorCategory::InvalidStructure,
        ArtifactCodecStage::StructuralValidation,
        Some(ArtifactType::DocumentLayout),
        None,
        "invalid document layout relationship",
    )
}
fn parse_lossless_source(bytes: &[u8]) -> Result<LosslessJsonValue, ArtifactCodecError> {
    parse_strict_lossless_json_object(bytes)
        .map_err(|error| ArtifactCodecError::strict(error.category()))
}

fn inspect_for_decode(
    bytes: &[u8],
    expected: ArtifactType,
) -> Result<ArtifactHeader, ArtifactCodecError> {
    let value = parse_strict_json_object(bytes)
        .map_err(|error| ArtifactCodecError::strict(error.category()))?;
    let header = header_from_value(&value)?;
    if header.artifact_type() != expected {
        return Err(ArtifactCodecError::new(
            ArtifactCodecErrorCategory::UnexpectedArtifactType,
            ArtifactCodecStage::Header,
            Some(expected),
            Some(header.schema_version()),
            "artifact discriminator does not match the requested loader",
        ));
    }
    Ok(header)
}

fn header_from_value(value: &Value) -> Result<ArtifactHeader, ArtifactCodecError> {
    let object = value.as_object().ok_or_else(|| {
        ArtifactCodecError::new(
            ArtifactCodecErrorCategory::RootTypeMismatch,
            ArtifactCodecStage::Header,
            None,
            None,
            "artifact root must be an object",
        )
    })?;
    let artifact_type = match object.get(ARTIFACT_TYPE_FIELD) {
        None => {
            return Err(ArtifactCodecError::new(
                ArtifactCodecErrorCategory::MissingArtifactType,
                ArtifactCodecStage::Header,
                None,
                None,
                "artifactType is required",
            ));
        }
        Some(Value::String(value)) if value == ArtifactType::Template.wire_name() => {
            ArtifactType::Template
        }
        Some(Value::String(value)) if value == ArtifactType::DocumentLayout.wire_name() => {
            ArtifactType::DocumentLayout
        }
        Some(Value::String(value)) if value == ArtifactType::Document.wire_name() => {
            ArtifactType::Document
        }
        Some(_) => {
            return Err(ArtifactCodecError::new(
                ArtifactCodecErrorCategory::InvalidArtifactType,
                ArtifactCodecStage::Header,
                None,
                None,
                "artifactType must be a known string discriminator",
            ));
        }
    };
    let schema_version = match object.get(SCHEMA_VERSION_FIELD) {
        None => {
            return Err(ArtifactCodecError::new(
                ArtifactCodecErrorCategory::MissingSchemaVersion,
                ArtifactCodecStage::Header,
                Some(artifact_type),
                None,
                "schemaVersion is required",
            ));
        }
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| SchemaVersion::try_from(value).ok())
            .ok_or_else(|| {
                ArtifactCodecError::new(
                    ArtifactCodecErrorCategory::InvalidSchemaVersion,
                    ArtifactCodecStage::Header,
                    Some(artifact_type),
                    None,
                    "schemaVersion must be a positive u32 integer",
                )
            })?,
        Some(_) => {
            return Err(ArtifactCodecError::new(
                ArtifactCodecErrorCategory::InvalidSchemaVersion,
                ArtifactCodecStage::Header,
                Some(artifact_type),
                None,
                "schemaVersion must be a positive u32 integer",
            ));
        }
    };
    Ok(ArtifactHeader::new(artifact_type, schema_version))
}

fn ensure_compatible(
    header: ArtifactHeader,
    registry: fn() -> Result<MigrationRegistry<'static>, crate::data::migration::MigrationError>,
) -> Result<(), ArtifactCodecError> {
    let registry = registry().map_err(|_| {
        ArtifactCodecError::new(
            ArtifactCodecErrorCategory::InvalidRegistry,
            ArtifactCodecStage::Compatibility,
            Some(header.artifact_type()),
            Some(header.schema_version()),
            "artifact migration registry is invalid",
        )
    })?;
    let policy: SchemaSupportPolicy = registry.support_policy().map_err(|_| {
        ArtifactCodecError::new(
            ArtifactCodecErrorCategory::InvalidRegistry,
            ArtifactCodecStage::Compatibility,
            Some(header.artifact_type()),
            Some(header.schema_version()),
            "artifact support policy is invalid",
        )
    })?;
    match policy.classify(header.schema_version()) {
        SchemaCompatibility::Current { .. } => Ok(()),
        SchemaCompatibility::MigrationRequired { .. } => Err(ArtifactCodecError::new(
            ArtifactCodecErrorCategory::MigrationRequired,
            ArtifactCodecStage::Compatibility,
            Some(header.artifact_type()),
            Some(header.schema_version()),
            "artifact requires a registered migration",
        )),
        SchemaCompatibility::UnsupportedFuture { .. } => Err(ArtifactCodecError::new(
            ArtifactCodecErrorCategory::UnsupportedFuture,
            ArtifactCodecStage::Compatibility,
            Some(header.artifact_type()),
            Some(header.schema_version()),
            "future artifact schema is not supported",
        )),
        SchemaCompatibility::UnsupportedPast { .. } => Err(ArtifactCodecError::new(
            ArtifactCodecErrorCategory::UnsupportedPast,
            ArtifactCodecStage::Compatibility,
            Some(header.artifact_type()),
            Some(header.schema_version()),
            "artifact schema predates the supported range",
        )),
    }
}

fn deserialize<T: DeserializeOwned>(
    bytes: &[u8],
    kind: ArtifactType,
) -> Result<T, ArtifactCodecError> {
    serde_json::from_slice(bytes).map_err(|_| {
        ArtifactCodecError::new(
            ArtifactCodecErrorCategory::InvalidStructure,
            ArtifactCodecStage::Deserialize,
            Some(kind),
            None,
            "artifact members do not match the v1 wire structure",
        )
    })
}

fn encode<T: serde::Serialize + ?Sized>(
    artifact: &T,
    kind: ArtifactType,
    lossless_source: Option<&LosslessJsonValue>,
) -> Result<Vec<u8>, ArtifactCodecError> {
    let encoded = match lossless_source {
        Some(source) => {
            to_deterministic_artifact_json_bytes(&ARTIFACT_PROVENANCE_AUTHORITY, artifact, source)
        }
        None => to_deterministic_json_bytes_categorized(artifact),
    };
    encoded.map_err(|error| match error.category() {
        JsonStorageErrorCategory::NestingDepthExceeded => {
            ArtifactCodecError::validation(kind, ArtifactValidationError::json_nesting_depth())
        }
        JsonStorageErrorCategory::ReservedObjectKey => {
            ArtifactCodecError::validation(kind, ArtifactValidationError::reserved_key())
        }
        JsonStorageErrorCategory::NonFiniteFloat => {
            ArtifactCodecError::validation(kind, ArtifactValidationError::invalid_json_value())
        }
        JsonStorageErrorCategory::SerializationFailure => ArtifactCodecError::new(
            ArtifactCodecErrorCategory::SerializationFailure,
            ArtifactCodecStage::Serialize,
            Some(kind),
            None,
            "validated artifact could not be serialized deterministically",
        ),
    })
}

/// M3-6 전환은 내용, revision, 시각을 바꾸지 않는다. registry를 건너뛰는 JSON 패치가 아니다.
pub(crate) fn transition_format(
    bytes: &[u8],
    target: &crate::data::project_relative_path::ProjectRelativePath,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use crate::data::migration_batch::{
        preflight_migration_batch, MigrationBatchDecision, MigrationBatchInput,
    };
    let header = inspect_artifact_header(bytes)?;
    let registry = match header.artifact_type() {
        ArtifactType::Template => super::schema::template_migration_registry()?,
        ArtifactType::Document => super::schema::document_migration_registry()?,
        ArtifactType::DocumentLayout => return Err("layout has no transition".into()),
    };
    let MigrationBatchDecision::Prepared(batch) = preflight_migration_batch(
        vec![MigrationBatchInput::new(target.clone(), bytes.to_vec())],
        registry,
    )?
    else {
        return Err("artifact already uses current format".into());
    };
    let result = batch
        .entries()
        .first()
        .ok_or("empty format transition batch")?;
    let preserved = crate::data::json::artifact_schema_bytes(
        &ARTIFACT_PROVENANCE_AUTHORITY,
        bytes,
        result.target_version().get(),
    )?;
    let expected: serde_json::Value = serde_json::from_slice(result.migrated_bytes())?;
    let actual: serde_json::Value = serde_json::from_slice(&preserved)?;
    if actual != expected {
        return Err("registry transition changed content unexpectedly".into());
    }
    match header.artifact_type() {
        ArtifactType::Template => {
            decode_template(&preserved)?;
        }
        ArtifactType::Document => {
            decode_document(&preserved)?;
        }
        ArtifactType::DocumentLayout => unreachable!(),
    }
    Ok(preserved)
}
