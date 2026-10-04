mod codec;
mod document;
pub(crate) mod group;
mod group_provenance;
mod id;
pub(crate) mod layout;
mod revision;
mod schema;
mod template;
mod value;

/// lossless provenance를 갱신할 수 있는 권한은 artifact 구현 내부에서만 소유한다.
///
/// 타입은 JSON 계층의 함수 서명에 쓰이므로 crate 안에서 보이지만, 값과 생성자는 이 모듈에만
/// 존재한다. 따라서 다른 data 모듈은 임의 JSON을 trusted source로 승격할 수 없다.
pub(crate) struct ArtifactProvenanceAuthority {
    _private: (),
}

const ARTIFACT_PROVENANCE_AUTHORITY: ArtifactProvenanceAuthority =
    ArtifactProvenanceAuthority { _private: () };

/// 문자열 path 대신 artifact v1의 실제 소유 위치만 표현한다.
#[derive(Clone, Copy)]
pub(crate) enum ArtifactLosslessPath {
    TemplateCurrentDefault(FieldId),
    TemplateInitialDefault(FieldId),
}

/// 현재 default에서 가져온 provenance는 initial default와 섞이지 않는다.
struct CurrentDefaultFieldValueProvenance {
    template_id: TemplateId,
    field_id: FieldId,
    value: FieldValue,
    source: crate::data::json::LosslessJsonValue,
}

/// historical materialization 전용 initial default provenance다.
struct InitialDefaultFieldValueProvenance {
    template_id: TemplateId,
    field_id: FieldId,
    value: FieldValue,
    source: crate::data::json::LosslessJsonValue,
}

pub(crate) use crate::data::field_engine::validation::FieldKind;
pub(crate) use codec::{
    decode_document, decode_layout, decode_template, encode_document, encode_layout,
    encode_template, inspect_artifact_header, ArtifactCodecError, ArtifactCodecErrorCategory,
    ArtifactCodecStage,
};
pub(crate) use document::document_creation::{
    create_document, create_document_with_term_info, create_document_with_values,
    DocumentCreationError, DocumentCreationErrorCategory, DocumentCreationOutcome,
};
pub(crate) use document::document_materialization::{
    materialize_document, repair_missing_optional_group_values, ChangedDocumentMaterialization,
    DocumentMaterializationError, DocumentMaterializationErrorCategory,
    DocumentMaterializationOutcome, DocumentMaterializationOutcomeKind,
    DocumentMaterializationWarning, DocumentMaterializationWarnings,
};
pub(crate) use document::document_reconciliation::{
    reconcile_document, reconcile_document_for_read, DocumentReconciliationError,
    DocumentReconciliationErrorCategory, DocumentReconciliationIssue,
    DocumentReconciliationIssueCategory, DocumentReconciliationWarning,
    DocumentReconciliationWarningCategory, KnownFieldEntry, OrphanFieldDisposition,
    OrphanFieldEntry, ReconciledDocumentView, ValueProvenance,
};
pub(crate) use document::document_save::{
    check_document_save_original, prepare_document_save, DocumentEdit, DocumentEditSet,
    DocumentSaveError, DocumentSaveErrorCategory, DocumentSaveOutcome, DocumentSaveOutcomeKind,
    DocumentSaveStage, DocumentValueEdit,
};
pub(crate) use document::prepare_version_restore as prepare_document_version_restore;
pub(crate) use document::{DocumentArtifact, OrphanedFieldDefinition, OrphanedOptionDefinition};
pub(crate) use id::{DocumentId, FieldId, OptionId, PersistentIdError, ReferenceId, TemplateId};
pub(crate) use revision::{TemplateRevision, TemplateRevisionError};
pub(crate) use schema::{
    document_migration_registry, document_migration_step_count, template_migration_registry,
    template_migration_step_count, ArtifactHeader, ArtifactType, DOCUMENT_SCHEMA_VERSION,
    TEMPLATE_SCHEMA_VERSION,
};
pub(crate) use template::template_creation::{
    create_template, duplicate_template, TemplateCreationError, TemplateCreationErrorCategory,
    TemplateCreationLocation, TemplateCreationStage,
};
pub(crate) use template::template_mutation;
pub(crate) use template::{
    ChoiceOption, FieldConfiguration, FieldDefinition, FieldLifecycle, OptionLifecycle,
    Presentation, TemplateArtifact, TemplateLifecycle,
};
pub(crate) use value::{FieldValue, RelationLink, RichTextDocument, RICH_TEXT_SCHEMA_VERSION};

use std::fmt;

use crate::data::field_engine::{
    choice::ChoiceValidationErrorCategory,
    rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
    scalar::ScalarValueErrorCategory,
    validation::{FieldValidationError, FieldValidationErrorCategory},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactValidationErrorCategory {
    InvalidWritingGuide,
    SchemaMismatch,
    ArtifactTypeMismatch,
    InvalidTimestamp,
    TimestampOrder,
    FieldOrderMismatch,
    OptionOrderMismatch,
    DuplicateOptionId,
    IntroducedRevisionOutOfRange,
    FieldConfigurationKindMismatch,
    FieldValueKindMismatch,
    OrphanMissingValue,
    OrphanValueKindMismatch,
    OrphanOptionMismatch,
    ForbiddenTreeKey,
    ReservedExtraKey,
    JsonNestingDepthExceeded,
    InvalidJsonValue,
    InvalidScalarValue,
    InvalidChoiceValue,
    InvalidRichTextValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactScalarValueLocation {
    CurrentDefault,
    InitialDefault,
    DocumentField,
    OrphanField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactChoiceValueLocation {
    CurrentDefault,
    InitialDefault,
    DocumentField,
    OrphanField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactRichTextValueLocation {
    CurrentDefault,
    InitialDefault,
    DocumentField,
    OrphanField,
}

impl From<ArtifactScalarValueLocation> for ArtifactChoiceValueLocation {
    fn from(value: ArtifactScalarValueLocation) -> Self {
        match value {
            ArtifactScalarValueLocation::CurrentDefault => Self::CurrentDefault,
            ArtifactScalarValueLocation::InitialDefault => Self::InitialDefault,
            ArtifactScalarValueLocation::DocumentField => Self::DocumentField,
            ArtifactScalarValueLocation::OrphanField => Self::OrphanField,
        }
    }
}

impl From<ArtifactScalarValueLocation> for ArtifactRichTextValueLocation {
    fn from(value: ArtifactScalarValueLocation) -> Self {
        match value {
            ArtifactScalarValueLocation::CurrentDefault => Self::CurrentDefault,
            ArtifactScalarValueLocation::InitialDefault => Self::InitialDefault,
            ArtifactScalarValueLocation::DocumentField => Self::DocumentField,
            ArtifactScalarValueLocation::OrphanField => Self::OrphanField,
        }
    }
}

/// 본문이나 label을 포함하지 않는 구조 검증 오류다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactValidationError {
    field_id: Option<FieldId>,
    category: ArtifactValidationErrorCategory,
    scalar_category: Option<ScalarValueErrorCategory>,
    scalar_location: Option<ArtifactScalarValueLocation>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    choice_location: Option<ArtifactChoiceValueLocation>,
    choice_option_id: Option<OptionId>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_location: Option<ArtifactRichTextValueLocation>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
    rich_text_node_depth: Option<usize>,
    detail: &'static str,
}

impl ArtifactValidationError {
    pub(crate) const fn field_id(self) -> Option<FieldId> {
        self.field_id
    }

    const fn at_field(mut self, field: FieldId) -> Self {
        self.field_id = Some(field);
        self
    }
    pub(crate) const fn category(self) -> ArtifactValidationErrorCategory {
        self.category
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

    pub(crate) const fn choice_option_id(self) -> Option<OptionId> {
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

    const fn new(category: ArtifactValidationErrorCategory, detail: &'static str) -> Self {
        Self {
            field_id: None,
            category,
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

    const fn invalid_writing_guide() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::InvalidWritingGuide,
            "writing guide must be typed text in Template schema 2",
        )
    }
    const fn schema_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::SchemaMismatch,
            "artifact model has the wrong schema version",
        )
    }

    const fn artifact_type_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::ArtifactTypeMismatch,
            "artifact model has the wrong discriminator",
        )
    }

    const fn invalid_timestamp() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::InvalidTimestamp,
            "artifact timestamps must use canonical UTC milliseconds",
        )
    }

    const fn timestamp_order() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::TimestampOrder,
            "artifact updated timestamp precedes its created timestamp",
        )
    }

    const fn field_order_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::FieldOrderMismatch,
            "fieldOrder must contain every active field exactly once",
        )
    }

    const fn option_order_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::OptionOrderMismatch,
            "optionOrder must contain every active option exactly once",
        )
    }

    const fn duplicate_option_id() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::DuplicateOptionId,
            "OptionId must be unique across the artifact",
        )
    }

    const fn introduced_revision_out_of_range() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::IntroducedRevisionOutOfRange,
            "introducedRevision exceeds the Template revision",
        )
    }

    const fn field_configuration_kind_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::FieldConfigurationKindMismatch,
            "Field kind and configuration variant do not match",
        )
    }

    const fn field_value_kind_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::FieldValueKindMismatch,
            "Field default value variant does not match its kind",
        )
    }

    const fn orphan_missing_value() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::OrphanMissingValue,
            "orphan field snapshot requires a corresponding field value",
        )
    }

    const fn orphan_value_kind_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::OrphanValueKindMismatch,
            "orphan field snapshot kind does not match its value",
        )
    }

    const fn orphan_option_mismatch() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::OrphanOptionMismatch,
            "orphan option snapshots must match the selected option IDs",
        )
    }

    const fn forbidden_tree_key() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::ForbiddenTreeKey,
            "Document contains a top-level key owned by the tree placement artifact",
        )
    }

    const fn reserved_key() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::ReservedExtraKey,
            "unknown fields must not shadow reserved wire keys",
        )
    }

    const fn json_nesting_depth() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::JsonNestingDepthExceeded,
            "artifact JSON exceeds the supported nesting depth",
        )
    }

    const fn invalid_json_value() -> Self {
        Self::new(
            ArtifactValidationErrorCategory::InvalidJsonValue,
            "artifact contains a JSON value that cannot be stored safely",
        )
    }

    const fn invalid_scalar_value(
        scalar_category: ScalarValueErrorCategory,
        scalar_location: ArtifactScalarValueLocation,
    ) -> Self {
        Self {
            category: ArtifactValidationErrorCategory::InvalidScalarValue,
            field_id: None,
            scalar_category: Some(scalar_category),
            scalar_location: Some(scalar_location),
            choice_category: None,
            choice_location: None,
            choice_option_id: None,
            rich_text_category: None,
            rich_text_location: None,
            rich_text_structure_location: None,
            rich_text_node_depth: None,
            detail: "artifact contains a noncanonical or invalid known scalar value",
        }
    }

    const fn invalid_choice_value_parts(
        category: ChoiceValidationErrorCategory,
        option_id: Option<OptionId>,
        choice_location: ArtifactChoiceValueLocation,
    ) -> Self {
        Self {
            category: ArtifactValidationErrorCategory::InvalidChoiceValue,
            field_id: None,
            scalar_category: None,
            scalar_location: None,
            choice_category: Some(category),
            choice_location: Some(choice_location),
            choice_option_id: option_id,
            rich_text_category: None,
            rich_text_location: None,
            rich_text_structure_location: None,
            rich_text_node_depth: None,
            detail: "artifact contains a noncanonical or invalid known choice value",
        }
    }

    const fn invalid_rich_text_value_parts(
        category: RichTextValidationErrorCategory,
        structure_location: RichTextErrorLocation,
        node_depth: Option<usize>,
        rich_text_location: ArtifactRichTextValueLocation,
    ) -> Self {
        Self {
            category: ArtifactValidationErrorCategory::InvalidRichTextValue,
            field_id: None,
            scalar_category: None,
            scalar_location: None,
            choice_category: None,
            choice_location: None,
            choice_option_id: None,
            rich_text_category: Some(category),
            rich_text_location: Some(rich_text_location),
            rich_text_structure_location: Some(structure_location),
            rich_text_node_depth: node_depth,
            detail: "artifact contains a noncanonical or invalid known rich-text value",
        }
    }

    /// 통합 계층의 안전한 category를 기존 artifact 오류 surface로 보존해 변환한다.
    fn invalid_field_value(
        error: FieldValidationError,
        scalar_location: ArtifactScalarValueLocation,
    ) -> Self {
        match error.category() {
            FieldValidationErrorCategory::FieldConfigurationKindMismatch => {
                Self::field_configuration_kind_mismatch()
            }
            FieldValidationErrorCategory::FieldValueKindMismatch => {
                Self::field_value_kind_mismatch()
            }
            FieldValidationErrorCategory::InvalidScalarValue => match error.scalar_category() {
                Some(category) => Self::invalid_scalar_value(category, scalar_location),
                None => Self::field_value_kind_mismatch(),
            },
            FieldValidationErrorCategory::InvalidChoiceValue => match error.choice_category() {
                Some(category) => Self::invalid_choice_value_parts(
                    category,
                    error.option_id(),
                    ArtifactChoiceValueLocation::from(scalar_location),
                ),
                None => Self::field_value_kind_mismatch(),
            },
            FieldValidationErrorCategory::InvalidRichTextValue => match (
                error.rich_text_category(),
                error.rich_text_structure_location(),
            ) {
                (Some(category), Some(structure_location)) => Self::invalid_rich_text_value_parts(
                    category,
                    structure_location,
                    error.rich_text_node_depth(),
                    ArtifactRichTextValueLocation::from(scalar_location),
                ),
                _ => Self::field_value_kind_mismatch(),
            },
            // v1 artifact에는 number bounds가 없고 Template default는 required를 강제하지 않는다.
            // 이 분기들은 잘못 결합된 내부 호출도 성공시키지 않는 fail-closed 방어다.
            FieldValidationErrorCategory::ContextLifecycleMismatch
            | FieldValidationErrorCategory::InvalidAssetReference
            | FieldValidationErrorCategory::InvalidMediaUrl
            | FieldValidationErrorCategory::InvalidDocumentReference
            | FieldValidationErrorCategory::RequiredValueUnset
            | FieldValidationErrorCategory::InvalidMinimum
            | FieldValidationErrorCategory::InvalidMaximum
            | FieldValidationErrorCategory::MinimumGreaterThanMaximum
            | FieldValidationErrorCategory::BelowMinimum
            | FieldValidationErrorCategory::AboveMaximum => Self::field_value_kind_mismatch(),
        }
    }
}

impl fmt::Display for ArtifactValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "artifact validation failed ({:?}): {}",
            self.category, self.detail
        )?;
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
        Ok(())
    }
}

impl std::error::Error for ArtifactValidationError {}

#[cfg(test)]
mod tests;

pub(crate) use template::sections::SectionInput;

pub(crate) use codec::transition_format;
