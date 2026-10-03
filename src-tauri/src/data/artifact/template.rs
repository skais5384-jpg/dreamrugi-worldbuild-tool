use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use super::id::{FieldId, OptionId, TemplateId};
use super::revision::TemplateRevision;
use super::schema::{ArtifactType, ArtifactTypeWire, TEMPLATE_SCHEMA_VERSION};
use super::value::{
    map_json_storage_error, validate_exact_active_order, validate_extra_keys, ExtraFields,
    FieldKindWire, FieldValue, FieldValueWire,
};
use super::{
    ArtifactLosslessPath, ArtifactScalarValueLocation, ArtifactValidationError,
    CurrentDefaultFieldValueProvenance, FieldKind, InitialDefaultFieldValueProvenance,
    ARTIFACT_PROVENANCE_AUTHORITY,
};
use crate::data::{
    field_engine::{
        choice::ChoiceOptionLifecycle,
        validation::{
            validate_bound_document_value, validate_template_default, BoundDocumentValueContext,
            FieldConfigurationView, FieldLifecycleView, FieldRule, FieldValidationError,
            FieldValidationOutcome, Requiredness, TemplateDefaultContext,
        },
    },
    json::{validate_serializable_for_storage, LosslessJsonSource, LosslessJsonValue},
    schema::SchemaVersion,
    utc_time::is_utc_milliseconds,
};

pub(crate) mod sections;
use sections::Section;
pub(crate) mod template_creation;
pub(crate) mod template_mutation;

/// 같은 ID/revision으로 다시 decode한 값도 서로 다른 ownership snapshot이다.
/// immutable clone만 증표를 공유하고, 성공한 mutation은 새 증표를 발급한다.
#[derive(Clone)]
struct DefaultDraftSnapshot(Arc<()>);

impl DefaultDraftSnapshot {
    fn new() -> Self {
        Self(Arc::new(()))
    }

    fn is_same_snapshot(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

// 증표는 domain equality의 일부가 아니다. 권한 검사는 위 pointer identity로만 수행한다.
impl PartialEq for DefaultDraftSnapshot {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

const PRESENTATION_KNOWN_KEYS: &[&str] = &["token"];
const CHOICE_OPTION_KNOWN_KEYS: &[&str] = &["label", "lifecycle"];
const FIELD_CONFIGURATION_KNOWN_KEYS: &[&str] = &["kind", "optionOrder", "options"];
const FIELD_DEFINITION_KNOWN_KEYS: &[&str] = &[
    "label",
    "lifecycle",
    "kind",
    "required",
    "defaultValue",
    "initialDefaultValue",
    "introducedRevision",
    "configuration",
    "presentation",
    "writingGuide",
];
const TEMPLATE_KNOWN_KEYS: &[&str] = &[
    "schemaVersion",
    "artifactType",
    "templateId",
    "revision",
    "name",
    "lifecycle",
    "glossaryExcluded",
    "fieldOrder",
    "sections",
    "fields",
    "presentation",
    "createdAtUtc",
    "updatedAtUtc",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateLifecycle {
    Active,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TemplateLifecycleWire {
    Active,
    Deleted,
}

impl From<TemplateLifecycleWire> for TemplateLifecycle {
    fn from(value: TemplateLifecycleWire) -> Self {
        match value {
            TemplateLifecycleWire::Active => Self::Active,
            TemplateLifecycleWire::Deleted => Self::Deleted,
        }
    }
}

impl From<TemplateLifecycle> for TemplateLifecycleWire {
    fn from(value: TemplateLifecycle) -> Self {
        match value {
            TemplateLifecycle::Active => Self::Active,
            TemplateLifecycle::Deleted => Self::Deleted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldLifecycle {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FieldLifecycleWire {
    Active,
    Archived,
}

impl From<FieldLifecycleWire> for FieldLifecycle {
    fn from(value: FieldLifecycleWire) -> Self {
        match value {
            FieldLifecycleWire::Active => Self::Active,
            FieldLifecycleWire::Archived => Self::Archived,
        }
    }
}

impl From<FieldLifecycle> for FieldLifecycleWire {
    fn from(value: FieldLifecycle) -> Self {
        match value {
            FieldLifecycle::Active => Self::Active,
            FieldLifecycle::Archived => Self::Archived,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OptionLifecycle {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OptionLifecycleWire {
    Active,
    Archived,
}

impl From<OptionLifecycleWire> for OptionLifecycle {
    fn from(value: OptionLifecycleWire) -> Self {
        match value {
            OptionLifecycleWire::Active => Self::Active,
            OptionLifecycleWire::Archived => Self::Archived,
        }
    }
}

impl From<OptionLifecycle> for OptionLifecycleWire {
    fn from(value: OptionLifecycle) -> Self {
        match value {
            OptionLifecycle::Active => Self::Active,
            OptionLifecycle::Archived => Self::Archived,
        }
    }
}

/// 구체 token vocabulary는 후속 semantic 계층이 정하고 wire object는 안정적으로 유지한다.
#[derive(Clone, PartialEq)]
pub(crate) struct Presentation {
    token: Option<String>,
    extra: ExtraFields,
}

impl fmt::Debug for Presentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Presentation")
            .field("has_token", &self.token.is_some())
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl Presentation {
    fn preserved_extra_matches(&self, other: &Self, allow_card_title: bool) -> bool {
        if !allow_card_title
            || (self.extra.contains_key("cardTitleField") && self.card_title_field().is_none())
        {
            return self.extra == other.extra;
        }
        self.extra
            .iter()
            .filter(|(k, _)| k.as_str() != "cardTitleField")
            .eq(other
                .extra
                .iter()
                .filter(|(k, _)| k.as_str() != "cardTitleField"))
    }
    pub(crate) fn card_title_field(&self) -> Option<FieldId> {
        self.extra
            .get("cardTitleField")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok())
    }
    pub(crate) fn set_card_title_field(&mut self, id: Option<FieldId>) {
        if let Some(id) = id {
            self.extra.insert(
                "cardTitleField".into(),
                serde_json::Value::String(id.to_string()),
            );
        } else {
            self.extra.remove("cardTitleField");
        }
    }
    pub(crate) fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn validate_structure(&self) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, PRESENTATION_KNOWN_KEYS)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresentationWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl From<PresentationWire> for Presentation {
    fn from(wire: PresentationWire) -> Self {
        Self {
            token: wire.token,
            extra: wire.extra,
        }
    }
}

impl From<&Presentation> for PresentationWire {
    fn from(presentation: &Presentation) -> Self {
        Self {
            token: presentation.token.clone(),
            extra: presentation.extra.clone(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct ChoiceOption {
    label: String,
    lifecycle: OptionLifecycle,
    extra: ExtraFields,
}

impl fmt::Debug for ChoiceOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChoiceOption")
            .field("label_redacted", &true)
            .field("lifecycle", &self.lifecycle)
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl ChoiceOption {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn lifecycle(&self) -> OptionLifecycle {
        self.lifecycle
    }

    fn validate_structure(&self) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, CHOICE_OPTION_KNOWN_KEYS)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChoiceOptionWire {
    label: String,
    lifecycle: OptionLifecycleWire,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl From<ChoiceOptionWire> for ChoiceOption {
    fn from(wire: ChoiceOptionWire) -> Self {
        Self {
            label: wire.label,
            lifecycle: wire.lifecycle.into(),
            extra: wire.extra,
        }
    }
}

impl From<&ChoiceOption> for ChoiceOptionWire {
    fn from(option: &ChoiceOption) -> Self {
        Self {
            label: option.label.clone(),
            lifecycle: option.lifecycle.into(),
            extra: option.extra.clone(),
        }
    }
}

/// Production configuration은 private variant와 extra를 감싸 외부 조립을 차단한다.
#[derive(Clone, PartialEq)]
pub(crate) struct FieldConfiguration {
    variant: FieldConfigurationVariant,
    extra: ExtraFields,
}

#[derive(Clone, PartialEq)]
enum FieldConfigurationVariant {
    Group {
        member_order: Vec<FieldId>,
        members: BTreeMap<FieldId, FieldDefinition>,
    },
    SingleLineText,
    RichText,
    Number {
        minimum: Option<String>,
        maximum: Option<String>,
    },
    Date,
    Time,
    Image,
    File,
    Url,
    Duration,
    SingleChoice {
        option_order: Vec<OptionId>,
        options: BTreeMap<OptionId, ChoiceOption>,
    },
    MultiChoice {
        option_order: Vec<OptionId>,
        options: BTreeMap<OptionId, ChoiceOption>,
    },
    Relation {
        multiple: bool,
        allowed_templates: Vec<TemplateId>,
        reciprocal_notice: bool,
    },
    DocumentLink,
}

impl fmt::Debug for FieldConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (option_count, active_option_count, archived_option_count) = match &self.variant {
            FieldConfigurationVariant::SingleChoice { options, .. }
            | FieldConfigurationVariant::MultiChoice { options, .. } => {
                let active = options
                    .values()
                    .filter(|option| option.lifecycle == OptionLifecycle::Active)
                    .count();
                (options.len(), active, options.len() - active)
            }
            _ => (0, 0, 0),
        };
        formatter
            .debug_struct("FieldConfiguration")
            .field("kind", &self.kind())
            .field("option_count", &option_count)
            .field("active_option_count", &active_option_count)
            .field("archived_option_count", &archived_option_count)
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl FieldConfiguration {
    pub(crate) fn members(&self) -> Option<(&[FieldId], &BTreeMap<FieldId, FieldDefinition>)> {
        if let FieldConfigurationVariant::Group {
            member_order,
            members,
        } = &self.variant
        {
            Some((member_order, members))
        } else {
            None
        }
    }
    pub(crate) fn bounds(&self) -> (Option<&str>, Option<&str>) {
        match &self.variant {
            FieldConfigurationVariant::Number { minimum, maximum } => {
                (minimum.as_deref(), maximum.as_deref())
            }
            _ => (None, None),
        }
    }
    pub(crate) fn kind(&self) -> FieldKind {
        match &self.variant {
            FieldConfigurationVariant::Group { .. } => FieldKind::Group,
            FieldConfigurationVariant::SingleLineText => FieldKind::SingleLineText,
            FieldConfigurationVariant::RichText => FieldKind::RichText,
            FieldConfigurationVariant::Number { .. } => FieldKind::Number,
            FieldConfigurationVariant::Date => FieldKind::Date,
            FieldConfigurationVariant::Time => FieldKind::Time,
            FieldConfigurationVariant::Image => FieldKind::Image,
            FieldConfigurationVariant::File => FieldKind::File,
            FieldConfigurationVariant::Url => FieldKind::Url,
            FieldConfigurationVariant::Duration => FieldKind::Duration,
            FieldConfigurationVariant::SingleChoice { .. } => FieldKind::SingleChoice,
            FieldConfigurationVariant::MultiChoice { .. } => FieldKind::MultiChoice,
            FieldConfigurationVariant::Relation { .. } => FieldKind::Relation,
            FieldConfigurationVariant::DocumentLink => FieldKind::DocumentLink,
        }
    }

    pub(crate) fn relation_settings(&self) -> Option<(bool, &[TemplateId], bool)> {
        match &self.variant {
            FieldConfigurationVariant::Relation {
                multiple,
                allowed_templates,
                reciprocal_notice,
            } => Some((*multiple, allowed_templates, *reciprocal_notice)),
            _ => None,
        }
    }

    pub(crate) fn option_order(&self) -> Option<&[OptionId]> {
        match &self.variant {
            FieldConfigurationVariant::SingleChoice { option_order, .. }
            | FieldConfigurationVariant::MultiChoice { option_order, .. } => Some(option_order),
            _ => None,
        }
    }

    pub(crate) fn options(&self) -> Option<&BTreeMap<OptionId, ChoiceOption>> {
        match &self.variant {
            FieldConfigurationVariant::SingleChoice { options, .. }
            | FieldConfigurationVariant::MultiChoice { options, .. } => Some(options),
            _ => None,
        }
    }

    fn validation_view(&self) -> Result<FieldConfigurationView, FieldValidationError> {
        let option_lifecycle = |option: &ChoiceOption| match option.lifecycle() {
            OptionLifecycle::Active => ChoiceOptionLifecycle::Active,
            OptionLifecycle::Archived => ChoiceOptionLifecycle::Archived,
        };
        match &self.variant {
            FieldConfigurationVariant::SingleLineText => {
                Ok(FieldConfigurationView::single_line_text())
            }
            FieldConfigurationVariant::Group { .. } => Ok(FieldConfigurationView::group()),
            FieldConfigurationVariant::RichText => Ok(FieldConfigurationView::rich_text()),
            // v1 number wire에는 min/max member가 없으므로 artifact 결합은 unbounded다.
            FieldConfigurationVariant::Number { minimum, maximum } => {
                Ok(FieldConfigurationView::number(Some(
                    crate::data::field_engine::validation::NumberConstraint::try_new(
                        minimum.as_deref(),
                        maximum.as_deref(),
                    )?,
                )))
            }
            FieldConfigurationVariant::Date => Ok(FieldConfigurationView::date()),
            FieldConfigurationVariant::Time => Ok(FieldConfigurationView::time()),
            FieldConfigurationVariant::Image => Ok(FieldConfigurationView::image()),
            FieldConfigurationVariant::File => Ok(FieldConfigurationView::file()),
            FieldConfigurationVariant::Url => Ok(FieldConfigurationView::url()),
            FieldConfigurationVariant::Duration => Ok(FieldConfigurationView::duration()),
            FieldConfigurationVariant::SingleChoice { options, .. } => {
                FieldConfigurationView::try_single_choice(
                    options
                        .iter()
                        .map(|(option_id, option)| (*option_id, option_lifecycle(option))),
                )
            }
            FieldConfigurationVariant::MultiChoice { options, .. } => {
                FieldConfigurationView::try_multi_choice(
                    options
                        .iter()
                        .map(|(option_id, option)| (*option_id, option_lifecycle(option))),
                )
            }
            FieldConfigurationVariant::Relation { multiple, .. } => {
                Ok(FieldConfigurationView::relation(*multiple))
            }
            FieldConfigurationVariant::DocumentLink => Ok(FieldConfigurationView::document_link()),
        }
    }

    fn validate_structure(
        &self,
        option_ids: &mut BTreeSet<OptionId>,
    ) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, FIELD_CONFIGURATION_KNOWN_KEYS)?;
        if let FieldConfigurationVariant::Relation {
            allowed_templates, ..
        } = &self.variant
        {
            let unique: BTreeSet<_> = allowed_templates.iter().copied().collect();
            if unique.len() != allowed_templates.len() {
                return Err(ArtifactValidationError::field_configuration_kind_mismatch());
            }
        }
        let (option_order, options) = match &self.variant {
            FieldConfigurationVariant::SingleChoice {
                option_order,
                options,
            }
            | FieldConfigurationVariant::MultiChoice {
                option_order,
                options,
            } => (option_order, options),
            _ => return Ok(()),
        };
        validate_exact_active_order(
            option_order,
            options.iter().filter_map(|(id, option)| {
                (option.lifecycle == OptionLifecycle::Active).then_some(*id)
            }),
            ArtifactValidationError::option_order_mismatch,
        )?;
        for (id, option) in options {
            if !option_ids.insert(*id) {
                return Err(ArtifactValidationError::duplicate_option_id());
            }
            option.validate_structure()?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum FieldConfigurationWire {
    Group {
        member_order: Vec<FieldId>,
        members: BTreeMap<FieldId, FieldDefinitionWire>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    SingleLineText {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    RichText {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum: Option<String>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Date {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Time {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Image {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    File {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Url {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Duration {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    SingleChoice {
        option_order: Vec<OptionId>,
        options: BTreeMap<OptionId, ChoiceOptionWire>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    MultiChoice {
        option_order: Vec<OptionId>,
        options: BTreeMap<OptionId, ChoiceOptionWire>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Relation {
        multiple: bool,
        #[serde(default)]
        allowed_templates: Vec<TemplateId>,
        reciprocal_notice: bool,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    DocumentLink {
        #[serde(flatten)]
        extra: ExtraFields,
    },
}

impl TryFrom<FieldConfigurationWire> for FieldConfiguration {
    type Error = ArtifactValidationError;
    fn try_from(wire: FieldConfigurationWire) -> Result<Self, Self::Error> {
        let (variant, extra) = match wire {
            FieldConfigurationWire::Group {
                member_order,
                members,
                extra,
            } => (
                FieldConfigurationVariant::Group {
                    member_order,
                    members: members
                        .into_iter()
                        .map(|(id, f)| {
                            Ok((id, FieldDefinition::from_wire(f, TEMPLATE_SCHEMA_VERSION)?))
                        })
                        .collect::<Result<_, ArtifactValidationError>>()?,
                },
                extra,
            ),
            FieldConfigurationWire::SingleLineText { extra } => {
                (FieldConfigurationVariant::SingleLineText, extra)
            }
            FieldConfigurationWire::RichText { extra } => {
                (FieldConfigurationVariant::RichText, extra)
            }
            FieldConfigurationWire::Number {
                minimum,
                maximum,
                extra,
            } => (
                FieldConfigurationVariant::Number { minimum, maximum },
                extra,
            ),
            FieldConfigurationWire::Date { extra } => (FieldConfigurationVariant::Date, extra),
            FieldConfigurationWire::Time { extra } => (FieldConfigurationVariant::Time, extra),
            FieldConfigurationWire::Image { extra } => (FieldConfigurationVariant::Image, extra),
            FieldConfigurationWire::File { extra } => (FieldConfigurationVariant::File, extra),
            FieldConfigurationWire::Url { extra } => (FieldConfigurationVariant::Url, extra),
            FieldConfigurationWire::Duration { extra } => {
                (FieldConfigurationVariant::Duration, extra)
            }
            FieldConfigurationWire::SingleChoice {
                option_order,
                options,
                extra,
            } => (
                FieldConfigurationVariant::SingleChoice {
                    option_order,
                    options: options
                        .into_iter()
                        .map(|(id, option)| (id, option.into()))
                        .collect(),
                },
                extra,
            ),
            FieldConfigurationWire::MultiChoice {
                option_order,
                options,
                extra,
            } => (
                FieldConfigurationVariant::MultiChoice {
                    option_order,
                    options: options
                        .into_iter()
                        .map(|(id, option)| (id, option.into()))
                        .collect(),
                },
                extra,
            ),
            FieldConfigurationWire::Relation {
                multiple,
                allowed_templates,
                reciprocal_notice,
                extra,
            } => (
                FieldConfigurationVariant::Relation {
                    multiple,
                    allowed_templates,
                    reciprocal_notice,
                },
                extra,
            ),
            FieldConfigurationWire::DocumentLink { extra } => {
                (FieldConfigurationVariant::DocumentLink, extra)
            }
        };
        Ok(Self { variant, extra })
    }
}

impl From<&FieldConfiguration> for FieldConfigurationWire {
    fn from(configuration: &FieldConfiguration) -> Self {
        let extra = configuration.extra.clone();
        match &configuration.variant {
            FieldConfigurationVariant::Group {
                member_order,
                members,
            } => Self::Group {
                member_order: member_order.clone(),
                members: members.iter().map(|(id, f)| (*id, f.into())).collect(),
                extra,
            },
            FieldConfigurationVariant::SingleLineText => Self::SingleLineText { extra },
            FieldConfigurationVariant::RichText => Self::RichText { extra },
            FieldConfigurationVariant::Number { minimum, maximum } => Self::Number {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                extra,
            },
            FieldConfigurationVariant::Date => Self::Date { extra },
            FieldConfigurationVariant::Time => Self::Time { extra },
            FieldConfigurationVariant::Image => Self::Image { extra },
            FieldConfigurationVariant::File => Self::File { extra },
            FieldConfigurationVariant::Url => Self::Url { extra },
            FieldConfigurationVariant::Duration => Self::Duration { extra },
            FieldConfigurationVariant::SingleChoice {
                option_order,
                options,
            } => Self::SingleChoice {
                option_order: option_order.clone(),
                options: options
                    .iter()
                    .map(|(id, option)| (*id, ChoiceOptionWire::from(option)))
                    .collect(),
                extra,
            },
            FieldConfigurationVariant::MultiChoice {
                option_order,
                options,
            } => Self::MultiChoice {
                option_order: option_order.clone(),
                options: options
                    .iter()
                    .map(|(id, option)| (*id, ChoiceOptionWire::from(option)))
                    .collect(),
                extra,
            },
            FieldConfigurationVariant::Relation {
                multiple,
                allowed_templates,
                reciprocal_notice,
            } => Self::Relation {
                multiple: *multiple,
                allowed_templates: allowed_templates.clone(),
                reciprocal_notice: *reciprocal_notice,
                extra,
            },
            FieldConfigurationVariant::DocumentLink => Self::DocumentLink { extra },
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct FieldDefinition {
    label: String,
    lifecycle: FieldLifecycle,
    kind: FieldKind,
    required: bool,
    writing_guide: Option<String>,
    default_value: FieldValue,
    initial_default_value: FieldValue,
    introduced_revision: TemplateRevision,
    configuration: FieldConfiguration,
    presentation: Presentation,
    extra: ExtraFields,
}

impl fmt::Debug for FieldDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldDefinition")
            .field("label_redacted", &true)
            .field("lifecycle", &self.lifecycle)
            .field("kind", &self.kind)
            .field("required", &self.required)
            .field("current_default_is_unset", &self.default_value.is_unset())
            .field(
                "initial_default_is_unset",
                &self.initial_default_value.is_unset(),
            )
            .field("introduced_revision", &self.introduced_revision)
            .field("configuration", &self.configuration)
            .field("presentation", &self.presentation)
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl FieldDefinition {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn lifecycle(&self) -> FieldLifecycle {
        self.lifecycle
    }

    pub(crate) fn kind(&self) -> FieldKind {
        self.kind
    }

    pub(crate) fn required(&self) -> bool {
        self.required
    }

    pub(crate) fn writing_guide(&self) -> Option<&str> {
        self.writing_guide.as_deref()
    }

    pub(crate) fn default_value(&self) -> &FieldValue {
        &self.default_value
    }

    pub(crate) fn initial_default_value(&self) -> &FieldValue {
        &self.initial_default_value
    }

    pub(crate) fn introduced_revision(&self) -> TemplateRevision {
        self.introduced_revision
    }

    pub(crate) fn configuration(&self) -> &FieldConfiguration {
        &self.configuration
    }

    pub(crate) fn presentation(&self) -> &Presentation {
        &self.presentation
    }

    fn validation_rule(&self) -> Result<FieldRule, FieldValidationError> {
        let lifecycle = match self.lifecycle {
            FieldLifecycle::Active => FieldLifecycleView::Active,
            FieldLifecycle::Archived => FieldLifecycleView::Archived,
        };
        let requiredness = if self.required {
            Requiredness::Required
        } else {
            Requiredness::Optional
        };
        FieldRule::try_new(
            self.kind,
            lifecycle,
            requiredness,
            self.configuration.validation_view()?,
        )
    }

    // 과거값 보존 검사와 새로 입력한 default의 현재 범위 검사를 분리한다.
    fn validate_fresh_default(&self) -> Result<(), ArtifactValidationError> {
        let location = ArtifactScalarValueLocation::CurrentDefault;
        let rule = self
            .validation_rule()
            .map_err(|e| ArtifactValidationError::invalid_field_value(e, location))?;
        let result = validate_template_default(
            &rule,
            self.default_value.validation_view(),
            TemplateDefaultContext::ActiveFieldCurrentDefault,
        )
        .map_err(|e| ArtifactValidationError::invalid_field_value(e, location))?;
        if !result.is_plain_valid() {
            return Err(ArtifactValidationError::field_value_kind_mismatch());
        }
        Ok(())
    }

    fn validate_default_without_diagnostics(
        rule: &FieldRule,
        value: &FieldValue,
        context: TemplateDefaultContext,
        location: ArtifactScalarValueLocation,
    ) -> Result<(), ArtifactValidationError> {
        // 저장된 숫자 default와 immutable initial은 현재 범위 변경 때문에 지우거나 고치지 않는다.
        // 새 숫자 default의 현재 범위 검사는 mutation 입력 경계에서 한다.
        if rule.kind() == FieldKind::Number
            && matches!(
                value.validation_view(),
                crate::data::field_engine::validation::FieldValueView::Number(_)
            )
        {
            return value.validate_structure(location);
        }
        let outcome = validate_template_default(rule, value.validation_view(), context)
            .map_err(|error| ArtifactValidationError::invalid_field_value(error, location))?;
        match outcome {
            FieldValidationOutcome::Valid => Ok(()),
            // Template default context는 진단 성공을 만들지 않는다. 정책이 바뀌면 조용히
            // 버리지 않고 artifact validation을 fail-closed한다.
            FieldValidationOutcome::ValidWithDiagnostics(_) => {
                Err(ArtifactValidationError::field_value_kind_mismatch())
            }
        }
    }

    /// M2-5가 명시적으로 Template과 결합한 단일 Document 값을 검증할 순수 경계다.
    /// 이 함수는 값을 materialize하거나 Document를 수정하지 않는다.
    pub(crate) fn validate_document_value(
        &self,
        value: &FieldValue,
        context: BoundDocumentValueContext,
    ) -> Result<FieldValidationOutcome, FieldValidationError> {
        if let Some((_, members)) = self.configuration.members() {
            return super::group::validate_bound(members, value, context);
        }
        let rule = self.validation_rule()?;
        validate_bound_document_value(&rule, value.validation_view(), context)
    }

    fn validate_structure(
        &self,
        schema: SchemaVersion,
        template_revision: TemplateRevision,
        option_ids: &mut BTreeSet<OptionId>,
    ) -> Result<(), ArtifactValidationError> {
        let known = if schema.get() == 1 {
            &FIELD_DEFINITION_KNOWN_KEYS[..FIELD_DEFINITION_KNOWN_KEYS.len() - 1]
        } else {
            FIELD_DEFINITION_KNOWN_KEYS
        };
        validate_extra_keys(&self.extra, known)?;
        if schema.get() == 1 && self.writing_guide.is_some() {
            return Err(ArtifactValidationError::invalid_writing_guide());
        }
        if self.introduced_revision > template_revision {
            return Err(ArtifactValidationError::introduced_revision_out_of_range());
        }
        self.configuration.validate_structure(option_ids)?;
        self.default_value.validate_storage_structure()?;
        self.initial_default_value.validate_storage_structure()?;

        let rule = self.validation_rule().map_err(|error| {
            ArtifactValidationError::invalid_field_value(
                error,
                ArtifactScalarValueLocation::CurrentDefault,
            )
        })?;
        let current_context = match self.lifecycle {
            FieldLifecycle::Active => TemplateDefaultContext::ActiveFieldCurrentDefault,
            FieldLifecycle::Archived => TemplateDefaultContext::ArchivedFieldPreserved,
        };
        let initial_context = match self.lifecycle {
            FieldLifecycle::Active => TemplateDefaultContext::HistoricalInitialDefault,
            FieldLifecycle::Archived => TemplateDefaultContext::ArchivedFieldPreserved,
        };
        Self::validate_default_without_diagnostics(
            &rule,
            &self.default_value,
            current_context,
            ArtifactScalarValueLocation::CurrentDefault,
        )?;
        Self::validate_default_without_diagnostics(
            &rule,
            &self.initial_default_value,
            initial_context,
            ArtifactScalarValueLocation::InitialDefault,
        )?;
        self.presentation.validate_structure()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FieldDefinitionWire {
    label: String,
    lifecycle: FieldLifecycleWire,
    kind: FieldKindWire,
    required: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_guide"
    )]
    writing_guide: Option<serde_json::Value>,
    default_value: FieldValueWire,
    initial_default_value: FieldValueWire,
    introduced_revision: TemplateRevision,
    configuration: FieldConfigurationWire,
    presentation: PresentationWire,
    #[serde(flatten)]
    extra: ExtraFields,
}

// v1에서 이미 존재한 null도 알 수 없는 확장 정보로 보존해야 하므로 누락과 구분한다.
fn present_guide<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}
impl FieldDefinition {
    fn from_wire(
        mut wire: FieldDefinitionWire,
        schema: SchemaVersion,
    ) -> Result<Self, ArtifactValidationError> {
        let writing_guide = if schema.get() == 1 {
            if let Some(value) = wire.writing_guide {
                wire.extra.insert("writingGuide".into(), value);
            }
            None
        } else {
            match wire.writing_guide {
                None => None,
                Some(serde_json::Value::String(s)) => Some(s),
                _ => return Err(ArtifactValidationError::invalid_writing_guide()),
            }
        };
        Ok(Self {
            label: wire.label,
            lifecycle: wire.lifecycle.into(),
            kind: wire.kind.into(),
            required: wire.required,
            writing_guide,
            default_value: wire.default_value.into(),
            initial_default_value: wire.initial_default_value.into(),
            introduced_revision: wire.introduced_revision,
            configuration: wire.configuration.try_into()?,
            presentation: wire.presentation.into(),
            extra: wire.extra,
        })
    }
}

impl From<&FieldDefinition> for FieldDefinitionWire {
    fn from(field: &FieldDefinition) -> Self {
        Self {
            label: field.label.clone(),
            lifecycle: field.lifecycle.into(),
            kind: field.kind.into(),
            required: field.required,
            writing_guide: field.writing_guide.clone().map(serde_json::Value::String),
            default_value: FieldValueWire::from(&field.default_value),
            initial_default_value: FieldValueWire::from(&field.initial_default_value),
            introduced_revision: field.introduced_revision,
            configuration: FieldConfigurationWire::from(&field.configuration),
            presentation: PresentationWire::from(&field.presentation),
            extra: field.extra.clone(),
        }
    }
}

/// Template v1은 codec 또는 닫힌 domain API의 전체 storage validation 뒤에만 반환된다.
#[derive(Clone, PartialEq)]
pub(crate) struct TemplateArtifact {
    schema_version: SchemaVersion,
    artifact_type: ArtifactType,
    template_id: TemplateId,
    revision: TemplateRevision,
    name: String,
    lifecycle: TemplateLifecycle,
    glossary_excluded: bool,
    field_order: Vec<FieldId>,
    sections: Vec<Section>,
    fields: BTreeMap<FieldId, FieldDefinition>,
    presentation: Presentation,
    created_at_utc: String,
    updated_at_utc: String,
    extra: ExtraFields,
    lossless_source: LosslessJsonSource,
    default_draft_snapshot: DefaultDraftSnapshot,
}

impl fmt::Debug for TemplateArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active_field_count = self
            .fields
            .values()
            .filter(|field| field.lifecycle == FieldLifecycle::Active)
            .count();
        formatter
            .debug_struct("TemplateArtifact")
            .field("schema_version", &self.schema_version)
            .field("revision", &self.revision)
            .field("lifecycle", &self.lifecycle)
            .field("name_redacted", &true)
            .field("field_count", &self.fields.len())
            .field("active_field_count", &active_field_count)
            .field(
                "archived_field_count",
                &(self.fields.len() - active_field_count),
            )
            .field("presentation", &self.presentation)
            .field("extra_count", &self.extra.len())
            .finish()
    }
}

impl TemplateArtifact {
    /// 전체 Template assembler를 하위 정의에도 사용한다. 이 투영은 저장 artifact가 아니다.
    pub(crate) fn member_scope(&self, group: FieldId) -> Self {
        let mut scoped = self.clone();
        let (order, fields) = self
            .fields
            .get(&group)
            .and_then(|f| f.configuration.members())
            .map(|(o, f)| (o.to_vec(), f.clone()))
            .unwrap_or_default();
        scoped.field_order = order;
        scoped.fields = fields;
        scoped.sections.clear();
        scoped
    }

    pub(super) fn try_from_wire(wire: TemplateWire) -> Result<Self, ArtifactValidationError> {
        let artifact = Self {
            sections: wire.sections,
            schema_version: wire.schema_version,
            artifact_type: wire.artifact_type.into(),
            template_id: wire.template_id,
            revision: wire.revision,
            name: wire.name,
            lifecycle: wire.lifecycle.into(),
            glossary_excluded: wire.glossary_excluded,
            field_order: wire.field_order,
            fields: wire
                .fields
                .into_iter()
                .map(|(id, field)| {
                    FieldDefinition::from_wire(field, wire.schema_version).map(|f| (id, f))
                })
                .collect::<Result<_, _>>()?,
            presentation: wire.presentation.into(),
            created_at_utc: wire.created_at_utc,
            updated_at_utc: wire.updated_at_utc,
            extra: wire.extra,
            lossless_source: LosslessJsonSource::default(),
            default_draft_snapshot: DefaultDraftSnapshot::new(),
        };
        artifact.validate_structure()?;
        Ok(artifact)
    }

    pub(crate) fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub(crate) fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    pub(crate) fn template_id(&self) -> TemplateId {
        self.template_id
    }

    pub(crate) fn revision(&self) -> TemplateRevision {
        self.revision
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn field_order(&self) -> &[FieldId] {
        &self.field_order
    }

    pub(crate) fn fields(&self) -> &BTreeMap<FieldId, FieldDefinition> {
        &self.fields
    }

    pub(crate) fn lifecycle(&self) -> TemplateLifecycle {
        self.lifecycle
    }

    pub(crate) fn glossary_excluded(&self) -> bool {
        self.glossary_excluded
    }

    pub(crate) fn presentation(&self) -> &Presentation {
        &self.presentation
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

    /// 검증된 Template mutation의 새 wire를 provenance 기준으로 확정한다.
    pub(super) fn rebase_lossless_source(&mut self) -> Result<(), ArtifactValidationError> {
        let wire = TemplateWire::from(&*self);
        self.lossless_source
            .rebase_artifact(&ARTIFACT_PROVENANCE_AUTHORITY, &wire)
            .map_err(map_json_storage_error)
    }

    /// Document로 실제 복사되는 FieldValue 하나의 provenance만 분리해 반환한다.
    pub(super) fn current_default_provenance(
        &self,
        field_id: FieldId,
    ) -> Option<CurrentDefaultFieldValueProvenance> {
        let field = self.fields.get(&field_id)?;
        Some(CurrentDefaultFieldValueProvenance {
            template_id: self.template_id,
            field_id,
            value: field.default_value.clone(),
            source: self.lossless_source.artifact_subtree(
                &ARTIFACT_PROVENANCE_AUTHORITY,
                ArtifactLosslessPath::TemplateCurrentDefault(field_id),
            )?,
        })
    }

    pub(super) fn initial_default_provenance(
        &self,
        field_id: FieldId,
    ) -> Option<InitialDefaultFieldValueProvenance> {
        let field = self.fields.get(&field_id)?;
        Some(InitialDefaultFieldValueProvenance {
            template_id: self.template_id,
            field_id,
            value: field.initial_default_value.clone(),
            source: self.lossless_source.artifact_subtree(
                &ARTIFACT_PROVENANCE_AUTHORITY,
                ArtifactLosslessPath::TemplateInitialDefault(field_id),
            )?,
        })
    }

    pub(super) fn validate_structure(&self) -> Result<(), ArtifactValidationError> {
        if ![1, 2, 3, 4, 5, 6, TEMPLATE_SCHEMA_VERSION.get()].contains(&self.schema_version.get()) {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.artifact_type != ArtifactType::Template {
            return Err(ArtifactValidationError::artifact_type_mismatch());
        }
        validate_extra_keys(&self.extra, TEMPLATE_KNOWN_KEYS)?;
        validate_timestamps(&self.created_at_utc, &self.updated_at_utc)?;
        validate_exact_active_order(
            &self.field_order,
            self.fields.iter().filter_map(|(id, field)| {
                (field.lifecycle == FieldLifecycle::Active).then_some(*id)
            }),
            ArtifactValidationError::field_order_mismatch,
        )?;

        if self.schema_version.get() < 4
            && self
                .fields
                .values()
                .any(|f| matches!(f.kind, FieldKind::Image | FieldKind::File | FieldKind::Url))
        {
            return Err(ArtifactValidationError::field_value_kind_mismatch());
        }
        if self.schema_version.get() < 6
            && self.fields.values().any(|field| {
                matches!(field.kind, FieldKind::Relation | FieldKind::DocumentLink)
                    || field.configuration.members().is_some_and(|(_, members)| {
                        members.values().any(|member| {
                            matches!(member.kind, FieldKind::Relation | FieldKind::DocumentLink)
                        })
                    })
            })
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.schema_version.get() < 7
            && self.fields.values().any(|field| {
                field.default_value.has_named_relation()
                    || field.initial_default_value.has_named_relation()
                    || field.configuration.members().is_some_and(|(_, members)| {
                        members.values().any(|member| {
                            member.default_value.has_named_relation()
                                || member.initial_default_value.has_named_relation()
                        })
                    })
            })
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        if self.schema_version.get() < 3
            && (!self.sections.is_empty()
                || self.fields.values().any(|f| {
                    f.configuration.bounds() != (None, None)
                        || matches!(
                            f.default_value.validation_view(),
                            crate::data::field_engine::validation::FieldValueView::NumberUnknown
                        )
                        || matches!(
                            f.initial_default_value.validation_view(),
                            crate::data::field_engine::validation::FieldValueView::NumberUnknown
                        )
                }))
        {
            return Err(ArtifactValidationError::schema_mismatch());
        }
        sections::validate(&self.sections, &self.field_order)?;
        if self.sections.iter().any(|section| {
            section
                .id
                .parse::<FieldId>()
                .is_ok_and(|id| self.fields.contains_key(&id))
        }) {
            return Err(ArtifactValidationError::field_order_mismatch());
        }
        let mut option_ids = BTreeSet::new();
        let mut all_ids: BTreeSet<_> = self.fields.keys().copied().collect();
        for field in self.fields.values() {
            if let Some((order, members)) = field.configuration.members() {
                if self.schema_version.get() < 5
                    || !field.default_value.is_unset()
                    || !field.initial_default_value.is_unset()
                {
                    return Err(ArtifactValidationError::schema_mismatch());
                }
                validate_exact_active_order(
                    order,
                    members.iter().filter_map(|(id, f)| {
                        (f.lifecycle == FieldLifecycle::Active).then_some(*id)
                    }),
                    ArtifactValidationError::field_order_mismatch,
                )?;
                for (id, child) in members {
                    if !all_ids.insert(*id)
                        || !matches!(
                            child.kind,
                            FieldKind::RichText
                                | FieldKind::Number
                                | FieldKind::Image
                                | FieldKind::File
                                | FieldKind::Relation
                                | FieldKind::DocumentLink
                        )
                    {
                        return Err(ArtifactValidationError::field_order_mismatch());
                    }
                    child.validate_structure(
                        self.schema_version,
                        self.revision,
                        &mut option_ids,
                    )?;
                }
            }
        }
        for (id, field) in &self.fields {
            // aggregate 검증에서도 실제 필드 위치를 보존한다. label/원문은 오류에 넣지 않는다.
            field
                .validate_structure(self.schema_version, self.revision, &mut option_ids)
                .map_err(|error| error.at_field(*id))?;
        }
        self.presentation.validate_structure()
    }

    /// aggregate 의미와 실제 private wire root의 storage 제약을 하나의 admission으로 묶는다.
    /// 이 경로를 통과한 snapshot은 codec이 별도 wrapper 보정 없이 즉시 encode할 수 있다.
    pub(super) fn validate_storage(&self) -> Result<(), ArtifactValidationError> {
        self.validate_structure()?;
        validate_serializable_for_storage(&TemplateWire::from(self)).map_err(map_json_storage_error)
    }

    #[cfg(test)]
    pub(super) fn corrupt_field_order_for_test(&mut self) {
        self.field_order.clear();
    }

    #[cfg(test)]
    pub(super) fn corrupt_reserved_extra_for_test(&mut self) {
        self.extra.insert(
            "$serde_json::private::FutureTransport".to_owned(),
            serde_json::Value::String("test-only invalid extra".to_owned()),
        );
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_default_for_test(
        &mut self,
        field_id: FieldId,
        reserved_key: &str,
        initial: bool,
    ) {
        let field = self
            .fields
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field");
        let value = if initial {
            &mut field.initial_default_value
        } else {
            &mut field.default_value
        };
        value.corrupt_rich_text_content_for_test(reserved_key);
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_default_depth_for_test(
        &mut self,
        field_id: FieldId,
        nested: serde_json::Value,
        initial: bool,
    ) {
        let field = self
            .fields
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field");
        let value = if initial {
            &mut field.initial_default_value
        } else {
            &mut field.default_value
        };
        value.corrupt_rich_text_value_for_test("testOnlyDepth", nested);
    }

    #[cfg(test)]
    pub(super) fn replace_rich_text_default_content_for_test(
        &mut self,
        field_id: FieldId,
        content: serde_json::Map<String, serde_json::Value>,
        initial: bool,
    ) {
        let field = self
            .fields
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field");
        let value = if initial {
            &mut field.initial_default_value
        } else {
            &mut field.default_value
        };
        value.replace_rich_text_content_for_test(content);
    }

    #[cfg(test)]
    pub(super) fn corrupt_scalar_default_for_test(
        &mut self,
        field_id: FieldId,
        raw: &str,
        initial: bool,
    ) {
        let field = self
            .fields
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field");
        let value = if initial {
            &mut field.initial_default_value
        } else {
            &mut field.default_value
        };
        value.corrupt_scalar_for_test(raw);
    }

    #[cfg(test)]
    pub(super) fn corrupt_multi_choice_default_for_test(
        &mut self,
        field_id: FieldId,
        option_ids: Vec<OptionId>,
        initial: bool,
    ) {
        let field = self
            .fields
            .get_mut(&field_id)
            .expect("test fixture must contain the requested field");
        let value = if initial {
            &mut field.initial_default_value
        } else {
            &mut field.default_value
        };
        value.corrupt_multi_choice_for_test(option_ids);
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TemplateWire {
    schema_version: SchemaVersion,
    artifact_type: ArtifactTypeWire,
    template_id: TemplateId,
    revision: TemplateRevision,
    name: String,
    lifecycle: TemplateLifecycleWire,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    glossary_excluded: bool,
    field_order: Vec<FieldId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sections: Vec<Section>,
    fields: BTreeMap<FieldId, FieldDefinitionWire>,
    presentation: PresentationWire,
    created_at_utc: String,
    updated_at_utc: String,
    #[serde(flatten)]
    extra: ExtraFields,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

impl From<&TemplateArtifact> for TemplateWire {
    fn from(artifact: &TemplateArtifact) -> Self {
        Self {
            sections: artifact.sections.clone(),
            schema_version: artifact.schema_version,
            artifact_type: artifact.artifact_type.into(),
            template_id: artifact.template_id,
            revision: artifact.revision,
            name: artifact.name.clone(),
            lifecycle: artifact.lifecycle.into(),
            glossary_excluded: artifact.glossary_excluded,
            field_order: artifact.field_order.clone(),
            fields: artifact
                .fields
                .iter()
                .map(|(id, field)| (*id, FieldDefinitionWire::from(field)))
                .collect(),
            presentation: PresentationWire::from(&artifact.presentation),
            created_at_utc: artifact.created_at_utc.clone(),
            updated_at_utc: artifact.updated_at_utc.clone(),
            extra: artifact.extra.clone(),
        }
    }
}

pub(super) fn validate_timestamps(
    created_at_utc: &str,
    updated_at_utc: &str,
) -> Result<(), ArtifactValidationError> {
    if !is_utc_milliseconds(created_at_utc) || !is_utc_milliseconds(updated_at_utc) {
        return Err(ArtifactValidationError::invalid_timestamp());
    }
    if updated_at_utc < created_at_utc {
        return Err(ArtifactValidationError::timestamp_order());
    }
    Ok(())
}
