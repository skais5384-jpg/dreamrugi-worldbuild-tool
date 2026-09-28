use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use uuid::Uuid;

use super::{
    ArtifactType, DefaultDraftSnapshot, FieldConfiguration, FieldConfigurationVariant,
    FieldDefinition, FieldId, FieldValue, OptionId, Presentation, TemplateArtifact, TemplateId,
    TemplateLifecycle, TemplateRevision, TEMPLATE_SCHEMA_VERSION,
};
use crate::data::{
    artifact::{
        ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
        ArtifactValidationError, ArtifactValidationErrorCategory,
    },
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
    },
    json::LosslessJsonSource,
    utc_time::is_utc_milliseconds,
};

mod provenance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateCreationErrorCategory {
    InvalidSource,
    InvalidTimestamp,
    IdGenerationFailed,
    IdCollision,
    IncompleteIdMapping,
    InvalidCandidate,
    ProvenanceFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateCreationStage {
    SourceValidation,
    InputValidation,
    IdentityAllocation,
    Remapping,
    CandidateValidation,
    Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateCreationLocation {
    Template,
    Field(FieldId),
    Option(OptionId),
    CurrentDefault(FieldId),
    InitialDefault(FieldId),
}

/// 원본 오류의 문자열/객체 대신 닫힌 진단 값만 복사한다. source chain도 payload를 소유하지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidationSummary {
    category: ArtifactValidationErrorCategory,
    scalar_category: Option<ScalarValueErrorCategory>,
    scalar_location: Option<ArtifactScalarValueLocation>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    choice_location: Option<ArtifactChoiceValueLocation>,
    option_id: Option<OptionId>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_location: Option<ArtifactRichTextValueLocation>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
    rich_text_node_depth: Option<usize>,
}

impl From<ArtifactValidationError> for ValidationSummary {
    fn from(error: ArtifactValidationError) -> Self {
        Self {
            category: error.category(),
            scalar_category: error.scalar_category(),
            scalar_location: error.scalar_location(),
            choice_category: error.choice_category(),
            choice_location: error.choice_location(),
            option_id: error.choice_option_id(),
            rich_text_category: error.rich_text_category(),
            rich_text_location: error.rich_text_location(),
            rich_text_structure_location: error.rich_text_structure_location(),
            rich_text_node_depth: error.rich_text_node_depth(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TemplateCreationError {
    category: TemplateCreationErrorCategory,
    stage: TemplateCreationStage,
    location: TemplateCreationLocation,
    validation: Option<ValidationSummary>,
    provenance: Option<provenance::Failure>,
}

impl TemplateCreationError {
    pub(crate) const fn category(self) -> TemplateCreationErrorCategory {
        self.category
    }
    pub(crate) const fn stage(self) -> TemplateCreationStage {
        self.stage
    }
    pub(crate) const fn location(self) -> TemplateCreationLocation {
        self.location
    }
    pub(crate) const fn validation_category(self) -> Option<ArtifactValidationErrorCategory> {
        match self.validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    fn new(
        category: TemplateCreationErrorCategory,
        stage: TemplateCreationStage,
        location: TemplateCreationLocation,
    ) -> Self {
        Self {
            category,
            stage,
            location,
            validation: None,
            provenance: None,
        }
    }

    fn validation(error: ArtifactValidationError, source: bool) -> Self {
        Self {
            category: if source {
                TemplateCreationErrorCategory::InvalidSource
            } else {
                TemplateCreationErrorCategory::InvalidCandidate
            },
            stage: if source {
                TemplateCreationStage::SourceValidation
            } else {
                TemplateCreationStage::CandidateValidation
            },
            location: TemplateCreationLocation::Template,
            validation: Some(error.into()),
            provenance: None,
        }
    }
}

impl fmt::Display for TemplateCreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Template creation failed ({:?}) at {:?}/{:?}",
            self.category, self.stage, self.location
        )
    }
}

impl std::error::Error for TemplateCreationError {}

// 시간은 기존 Document 생성/mutation과 같이 caller가 주입한다. ID seam은 이 모듈 내부에만 둔다.
trait TemplateIdentitySource {
    fn template_id(&mut self) -> Result<TemplateId, ()>;
    fn field_id(&mut self) -> Result<FieldId, ()>;
    fn option_id(&mut self) -> Result<OptionId, ()>;
}

struct RandomIdentities;

impl TemplateIdentitySource for RandomIdentities {
    fn template_id(&mut self) -> Result<TemplateId, ()> {
        Ok(TemplateId::new())
    }
    fn field_id(&mut self) -> Result<FieldId, ()> {
        Ok(FieldId::new())
    }
    fn option_id(&mut self) -> Result<OptionId, ()> {
        Ok(OptionId::new())
    }
}

/// 빈 active Template을 생성한다. v1 name/token은 opaque 문자열이며 trim/새 grammar를 적용하지 않는다.
pub(crate) fn create_template(
    name: String,
    presentation_token: Option<String>,
    timestamp_utc: String,
) -> Result<TemplateArtifact, TemplateCreationError> {
    create_template_with_ids(
        name,
        presentation_token,
        timestamp_utc,
        &mut RandomIdentities,
    )
}

fn create_template_with_ids(
    name: String,
    presentation_token: Option<String>,
    timestamp_utc: String,
    ids: &mut impl TemplateIdentitySource,
) -> Result<TemplateArtifact, TemplateCreationError> {
    validate_timestamp(&timestamp_utc)?;
    let template_id = issued(ids.template_id(), TemplateCreationLocation::Template)?;
    let candidate = TemplateArtifact {
        sections: vec![],
        schema_version: TEMPLATE_SCHEMA_VERSION,
        artifact_type: ArtifactType::Template,
        template_id,
        revision: TemplateRevision::INITIAL,
        name,
        lifecycle: TemplateLifecycle::Active,
        glossary_excluded: false,
        field_order: Vec::new(),
        fields: BTreeMap::new(),
        presentation: Presentation {
            token: presentation_token,
            extra: BTreeMap::new(),
        },
        created_at_utc: timestamp_utc.clone(),
        updated_at_utc: timestamp_utc,
        extra: BTreeMap::new(),
        lossless_source: LosslessJsonSource::default(),
        default_draft_snapshot: DefaultDraftSnapshot::new(),
    };
    candidate
        .validate_storage()
        .map_err(|error| TemplateCreationError::validation(error, false))?;
    Ok(candidate)
}

/// Clone과 달리 모든 entity ID를 새로 발급한다. deleted source도 새 active identity로 복사할 수 있다.
/// Field/Option lifecycle과 두 default는 보존하고, Template/Field revision과 시각은 새 이력으로 시작한다.
pub(crate) fn duplicate_template(
    source: &TemplateArtifact,
    timestamp_utc: String,
) -> Result<TemplateArtifact, TemplateCreationError> {
    duplicate_template_with_ids(source, timestamp_utc, &mut RandomIdentities)
}

fn duplicate_template_with_ids(
    source: &TemplateArtifact,
    timestamp_utc: String,
    ids: &mut impl TemplateIdentitySource,
) -> Result<TemplateArtifact, TemplateCreationError> {
    source
        .validate_storage()
        .map_err(|error| TemplateCreationError::validation(error, true))?;
    validate_timestamp(&timestamp_utc)?;
    // 완전한 mapping 확보 이전에는 candidate도 provenance 복사본도 만들지 않는다.
    let mapping = IdentityMapping::allocate(source, ids)?;
    let mut candidate = TemplateArtifact {
        sections: source
            .sections
            .iter()
            .map(|s| {
                let mut section = s.clone();
                section.before_field = s.before_field.map(|id| mapping.field(id)).transpose()?;
                Ok(section)
            })
            .collect::<Result<_, TemplateCreationError>>()?,
        schema_version: source.schema_version,
        artifact_type: source.artifact_type,
        template_id: mapping.template_id,
        revision: TemplateRevision::INITIAL,
        name: source.name.clone(),
        glossary_excluded: source.glossary_excluded,
        lifecycle: TemplateLifecycle::Active,
        field_order: source
            .field_order
            .iter()
            .map(|id| mapping.field(*id))
            .collect::<Result<_, _>>()?,
        fields: source
            .fields
            .iter()
            .map(|(id, field)| Ok((mapping.field(*id)?, duplicate_field(*id, field, &mapping)?)))
            .collect::<Result<_, TemplateCreationError>>()?,
        presentation: source.presentation.clone(),
        created_at_utc: timestamp_utc.clone(),
        updated_at_utc: timestamp_utc,
        extra: source.extra.clone(),
        lossless_source: LosslessJsonSource::default(),
        default_draft_snapshot: DefaultDraftSnapshot::new(),
    };
    candidate
        .validate_storage()
        .map_err(|error| TemplateCreationError::validation(error, false))?;
    candidate.set_lossless_source(provenance::relocate(source, &mapping)?);
    // 소유 위치를 먼저 옮긴 후, 실제 typed candidate로 rebase해야 known ID와 시각도 일치한다.
    candidate
        .rebase_lossless_source()
        .map_err(|error| TemplateCreationError::validation(error, false))?;
    Ok(candidate)
}

fn validate_timestamp(timestamp: &str) -> Result<(), TemplateCreationError> {
    if !is_utc_milliseconds(timestamp) {
        return Err(TemplateCreationError::new(
            TemplateCreationErrorCategory::InvalidTimestamp,
            TemplateCreationStage::InputValidation,
            TemplateCreationLocation::Template,
        ));
    }
    Ok(())
}

fn issued<T>(
    result: Result<T, ()>,
    location: TemplateCreationLocation,
) -> Result<T, TemplateCreationError> {
    result.map_err(|()| {
        TemplateCreationError::new(
            TemplateCreationErrorCategory::IdGenerationFailed,
            TemplateCreationStage::IdentityAllocation,
            location,
        )
    })
}

fn reserve(
    id: Uuid,
    used: &mut BTreeSet<Uuid>,
    location: TemplateCreationLocation,
) -> Result<(), TemplateCreationError> {
    if !used.insert(id) {
        return Err(TemplateCreationError::new(
            TemplateCreationErrorCategory::IdCollision,
            TemplateCreationStage::IdentityAllocation,
            location,
        ));
    }
    Ok(())
}

struct IdentityMapping {
    template_id: TemplateId,
    fields: BTreeMap<FieldId, FieldId>,
    options: BTreeMap<OptionId, OptionId>,
}

impl IdentityMapping {
    fn allocate(
        source: &TemplateArtifact,
        ids: &mut impl TemplateIdentitySource,
    ) -> Result<Self, TemplateCreationError> {
        let mut used = BTreeSet::from([source.template_id.as_uuid()]);
        let all_fields: Vec<_> = source
            .fields
            .iter()
            .flat_map(|(id, f)| {
                std::iter::once((id, f)).chain(
                    f.configuration
                        .members()
                        .into_iter()
                        .flat_map(|(_, m)| m.iter()),
                )
            })
            .collect();
        used.extend(all_fields.iter().map(|(id, _)| id.as_uuid()));
        let old_options: BTreeSet<_> = source
            .fields
            .values()
            .filter_map(|field| field.configuration.options())
            .flat_map(|options| options.keys().copied())
            .collect();
        used.extend(old_options.iter().map(|id| id.as_uuid()));
        let template_id = issued(ids.template_id(), TemplateCreationLocation::Template)?;
        reserve(
            template_id.as_uuid(),
            &mut used,
            TemplateCreationLocation::Template,
        )?;
        let mut fields = BTreeMap::new();
        for (old, _) in &all_fields {
            let old = *old;
            let location = TemplateCreationLocation::Field(*old);
            let new = issued(ids.field_id(), location)?;
            reserve(new.as_uuid(), &mut used, location)?;
            fields.insert(*old, new);
        }
        let mut options = BTreeMap::new();
        for old in old_options {
            let location = TemplateCreationLocation::Option(old);
            let new = issued(ids.option_id(), location)?;
            reserve(new.as_uuid(), &mut used, location)?;
            options.insert(old, new);
        }
        Ok(Self {
            template_id,
            fields,
            options,
        })
    }

    fn field(&self, id: FieldId) -> Result<FieldId, TemplateCreationError> {
        self.fields
            .get(&id)
            .copied()
            .ok_or_else(|| Self::missing(TemplateCreationLocation::Field(id)))
    }

    fn option(&self, id: OptionId) -> Result<OptionId, TemplateCreationError> {
        self.options
            .get(&id)
            .copied()
            .ok_or_else(|| Self::missing(TemplateCreationLocation::Option(id)))
    }

    fn missing(location: TemplateCreationLocation) -> TemplateCreationError {
        TemplateCreationError::new(
            TemplateCreationErrorCategory::IncompleteIdMapping,
            TemplateCreationStage::Remapping,
            location,
        )
    }
}

fn duplicate_field(
    id: FieldId,
    source: &FieldDefinition,
    mapping: &IdentityMapping,
) -> Result<FieldDefinition, TemplateCreationError> {
    let variant = match &source.configuration.variant {
        FieldConfigurationVariant::Group {
            member_order,
            members,
        } => FieldConfigurationVariant::Group {
            member_order: member_order
                .iter()
                .map(|id| mapping.field(*id))
                .collect::<Result<_, _>>()?,
            members: members
                .iter()
                .map(|(id, f)| Ok((mapping.field(*id)?, duplicate_field(*id, f, mapping)?)))
                .collect::<Result<_, TemplateCreationError>>()?,
        },
        FieldConfigurationVariant::SingleChoice {
            option_order,
            options,
        }
        | FieldConfigurationVariant::MultiChoice {
            option_order,
            options,
        } => {
            let option_order = option_order
                .iter()
                .map(|id| mapping.option(*id))
                .collect::<Result<_, _>>()?;
            let options = options
                .iter()
                .map(|(id, option)| Ok((mapping.option(*id)?, option.clone())))
                .collect::<Result<_, TemplateCreationError>>()?;
            match &source.configuration.variant {
                FieldConfigurationVariant::SingleChoice { .. } => {
                    FieldConfigurationVariant::SingleChoice {
                        option_order,
                        options,
                    }
                }
                _ => FieldConfigurationVariant::MultiChoice {
                    option_order,
                    options,
                },
            }
        }
        // 새 configuration variant가 생기면 ID 참조 유무를 여기서 다시 감사해야 한다.
        FieldConfigurationVariant::SingleLineText
        | FieldConfigurationVariant::RichText
        | FieldConfigurationVariant::Number { .. }
        | FieldConfigurationVariant::Date
        | FieldConfigurationVariant::Time
        | FieldConfigurationVariant::Image
        | FieldConfigurationVariant::File
        | FieldConfigurationVariant::Url
        | FieldConfigurationVariant::Duration
        | FieldConfigurationVariant::Relation { .. }
        | FieldConfigurationVariant::DocumentLink => source.configuration.variant.clone(),
    };
    Ok(FieldDefinition {
        label: source.label.clone(),
        writing_guide: source.writing_guide.clone(),
        lifecycle: source.lifecycle,
        kind: source.kind,
        required: source.required,
        default_value: duplicate_default(
            &source.default_value,
            mapping,
            TemplateCreationLocation::CurrentDefault(id),
        )?,
        initial_default_value: duplicate_default(
            &source.initial_default_value,
            mapping,
            TemplateCreationLocation::InitialDefault(id),
        )?,
        introduced_revision: TemplateRevision::INITIAL,
        configuration: FieldConfiguration {
            variant,
            extra: source.configuration.extra.clone(),
        },
        presentation: source.presentation.clone(),
        extra: source.extra.clone(),
    })
}

fn duplicate_default(
    source: &FieldValue,
    mapping: &IdentityMapping,
    location: TemplateCreationLocation,
) -> Result<FieldValue, TemplateCreationError> {
    let remap = |id| {
        mapping
            .option(id)
            .map_err(|_| IdentityMapping::missing(location))
    };
    if let Some(id) = source.single_choice() {
        return Ok(
            FieldValue::from_single_choice(remap(id)?).preserve_outer_storage_extra_from(source)
        );
    }
    if let Some(ids) = source.multi_choice() {
        let mut ids = ids
            .iter()
            .map(|id| remap(*id))
            .collect::<Result<Vec<_>, _>>()?;
        // Multi-choice는 표시 순서가 없는 집합이다. 새 UUID 순서로 정렬해야 기존 canonical validator를 통과한다.
        ids.sort();
        return Ok(FieldValue::from_multi_choice(ids).preserve_outer_storage_extra_from(source));
    }
    Ok(source.clone())
}

#[cfg(test)]
mod tests;
