use std::fmt;

use super::{
    choice::{
        validate_choice_membership, ChoiceMembershipContext, ChoiceMembershipDiagnosticCategory,
        ChoiceMembershipOutcome, ChoiceOptionLifecycle, ChoiceOptionLookup, ChoiceSelection,
        ChoiceValidationError, ChoiceValidationErrorCategory,
    },
    rich_text::{RichTextErrorLocation, RichTextValidationError, RichTextValidationErrorCategory},
    scalar::{
        validate_number_constraint_order, CalendarDate, CanonicalDecimal, DurationMilliseconds,
        LocalTime, ScalarValueError, ScalarValueErrorCategory, SingleLineText,
    },
};
use crate::data::artifact::{DocumentId, OptionId, ReferenceId, RelationLink};

/// v1 Field 종류를 semantic 계층이 소유한다. Artifact wire enum은 이 타입으로 exhaustive 변환한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    Group,
    SingleLineText,
    RichText,
    Number,
    Date,
    Time,
    Image,
    File,
    Url,
    Duration,
    SingleChoice,
    MultiChoice,
    Relation,
    DocumentLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldLifecycleView {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Requiredness {
    Optional,
    Required,
}

/// Template default 슬롯은 required Document 값과 다른 정책을 갖는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateDefaultContext {
    ActiveFieldCurrentDefault,
    HistoricalInitialDefault,
    ArchivedFieldPreserved,
}

/// Template과 명시적으로 결합된 Document 값만 이 context를 사용한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundDocumentValueContext {
    NewDocumentValue,
    ExistingDocumentValue,
    MaterializedHistorical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldValidationLocation {
    FieldRule,
    NumberConstraint,
    ActiveFieldCurrentDefault,
    HistoricalInitialDefault,
    ArchivedFieldPreservedDefault,
    NewDocumentValue,
    ExistingDocumentValue,
    MaterializedHistoricalValue,
    StandaloneDocumentValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldValidationErrorCategory {
    FieldConfigurationKindMismatch,
    FieldValueKindMismatch,
    ContextLifecycleMismatch,
    RequiredValueUnset,
    InvalidScalarValue,
    InvalidChoiceValue,
    InvalidRichTextValue,
    InvalidAssetReference,
    InvalidMediaUrl,
    InvalidDocumentReference,
    InvalidMinimum,
    InvalidMaximum,
    MinimumGreaterThanMaximum,
    BelowMinimum,
    AboveMaximum,
}

/// 값 본문이나 label을 소유하지 않는 통합 검증 오류다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FieldValidationError {
    category: FieldValidationErrorCategory,
    location: FieldValidationLocation,
    scalar_category: Option<ScalarValueErrorCategory>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    option_id: Option<OptionId>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
    rich_text_node_depth: Option<usize>,
    detail: &'static str,
}

impl FieldValidationError {
    pub(crate) const fn category(self) -> FieldValidationErrorCategory {
        self.category
    }

    pub(crate) const fn location(self) -> FieldValidationLocation {
        self.location
    }

    pub(crate) const fn scalar_category(self) -> Option<ScalarValueErrorCategory> {
        self.scalar_category
    }

    pub(crate) const fn choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        self.choice_category
    }

    pub(crate) const fn option_id(self) -> Option<OptionId> {
        self.option_id
    }

    pub(crate) const fn rich_text_category(self) -> Option<RichTextValidationErrorCategory> {
        self.rich_text_category
    }

    pub(crate) const fn rich_text_structure_location(self) -> Option<RichTextErrorLocation> {
        self.rich_text_structure_location
    }

    pub(crate) const fn rich_text_node_depth(self) -> Option<usize> {
        self.rich_text_node_depth
    }

    const fn new(
        category: FieldValidationErrorCategory,
        location: FieldValidationLocation,
        detail: &'static str,
    ) -> Self {
        Self {
            category,
            location,
            scalar_category: None,
            choice_category: None,
            option_id: None,
            rich_text_category: None,
            rich_text_structure_location: None,
            rich_text_node_depth: None,
            detail,
        }
    }

    const fn configuration_kind_mismatch() -> Self {
        Self::new(
            FieldValidationErrorCategory::FieldConfigurationKindMismatch,
            FieldValidationLocation::FieldRule,
            "Field kind and configuration kind do not match",
        )
    }

    pub(crate) const fn value_kind_mismatch(location: FieldValidationLocation) -> Self {
        Self::new(
            FieldValidationErrorCategory::FieldValueKindMismatch,
            location,
            "Field kind and value kind do not match",
        )
    }

    const fn context_lifecycle_mismatch(location: FieldValidationLocation) -> Self {
        Self::new(
            FieldValidationErrorCategory::ContextLifecycleMismatch,
            location,
            "validation context does not apply to this Field lifecycle",
        )
    }

    const fn required_value_unset(location: FieldValidationLocation) -> Self {
        Self::new(
            FieldValidationErrorCategory::RequiredValueUnset,
            location,
            "active required Field requires a non-unset final Document value",
        )
    }

    const fn invalid_scalar(error: ScalarValueError, location: FieldValidationLocation) -> Self {
        let mut result = Self::new(
            FieldValidationErrorCategory::InvalidScalarValue,
            location,
            "Field contains an invalid or noncanonical scalar value",
        );
        result.scalar_category = Some(error.category());
        result
    }

    const fn invalid_choice(
        error: ChoiceValidationError,
        location: FieldValidationLocation,
    ) -> Self {
        let mut result = Self::new(
            FieldValidationErrorCategory::InvalidChoiceValue,
            location,
            "Field contains an invalid or noncanonical choice value",
        );
        result.choice_category = Some(error.category());
        result.option_id = error.option_id();
        result
    }

    const fn invalid_rich_text(
        error: RichTextValidationError,
        location: FieldValidationLocation,
    ) -> Self {
        let mut result = Self::new(
            FieldValidationErrorCategory::InvalidRichTextValue,
            location,
            "Field contains an invalid or noncanonical rich-text value",
        );
        result.rich_text_category = Some(error.category());
        result.rich_text_structure_location = Some(error.location());
        result.rich_text_node_depth = error.node_depth();
        result
    }

    const fn invalid_minimum(error: ScalarValueError) -> Self {
        let mut result = Self::new(
            FieldValidationErrorCategory::InvalidMinimum,
            FieldValidationLocation::NumberConstraint,
            "number minimum is not a canonical decimal",
        );
        result.scalar_category = Some(error.category());
        result
    }

    const fn invalid_maximum(error: ScalarValueError) -> Self {
        let mut result = Self::new(
            FieldValidationErrorCategory::InvalidMaximum,
            FieldValidationLocation::NumberConstraint,
            "number maximum is not a canonical decimal",
        );
        result.scalar_category = Some(error.category());
        result
    }

    const fn minimum_greater_than_maximum() -> Self {
        Self::new(
            FieldValidationErrorCategory::MinimumGreaterThanMaximum,
            FieldValidationLocation::NumberConstraint,
            "number minimum exceeds number maximum",
        )
    }

    const fn below_minimum(location: FieldValidationLocation) -> Self {
        Self::new(
            FieldValidationErrorCategory::BelowMinimum,
            location,
            "number value is below its inclusive minimum",
        )
    }

    const fn above_maximum(location: FieldValidationLocation) -> Self {
        Self::new(
            FieldValidationErrorCategory::AboveMaximum,
            location,
            "number value is above its inclusive maximum",
        )
    }
}

impl fmt::Debug for FieldValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldValidationError")
            .field("category", &self.category)
            .field("location", &self.location)
            .field("scalar_category", &self.scalar_category)
            .field("choice_category", &self.choice_category)
            .field("option_id", &self.option_id)
            .field("rich_text_category", &self.rich_text_category)
            .field(
                "rich_text_structure_location",
                &self.rich_text_structure_location,
            )
            .field("rich_text_node_depth", &self.rich_text_node_depth)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for FieldValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Field validation failed ({:?} at {:?}): {}",
            self.category, self.location, self.detail
        )?;
        if let Some(category) = self.scalar_category {
            write!(formatter, " ({category:?})")?;
        }
        if let Some(category) = self.choice_category {
            write!(formatter, " ({category:?})")?;
        }
        if let Some(option_id) = self.option_id {
            write!(formatter, " (OptionId {option_id})")?;
        }
        if let (Some(category), Some(location)) =
            (self.rich_text_category, self.rich_text_structure_location)
        {
            write!(formatter, " ({category:?} at {location:?})")?;
        }
        if let Some(depth) = self.rich_text_node_depth {
            write!(formatter, " (node depth {depth})")?;
        }
        Ok(())
    }
}

impl std::error::Error for FieldValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldDiagnosticCategory {
    ExistingSelectionContainsArchivedOption,
    ExistingNumberOutsideRange,
    ExistingRelationExceedsMultiplicity,
}

/// Diagnostic은 payload 대신 bounded count만 보존한다.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FieldDiagnostic {
    category: FieldDiagnosticCategory,
    archived_option_count: usize,
}

impl FieldDiagnostic {
    pub(crate) const fn category(&self) -> FieldDiagnosticCategory {
        self.category
    }

    pub(crate) const fn archived_option_count(&self) -> usize {
        self.archived_option_count
    }
}

/// Diagnostic이 있는 성공을 일반 성공으로 암시적으로 축소할 수 없는 결과다.
#[must_use = "field validation diagnostics must be inspected explicitly"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FieldValidationOutcome {
    Valid,
    ValidWithDiagnostics(FieldDiagnostic),
}

impl FieldValidationOutcome {
    pub(crate) const fn is_plain_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    pub(crate) const fn diagnostic(&self) -> Option<&FieldDiagnostic> {
        match self {
            Self::Valid => None,
            Self::ValidWithDiagnostics(diagnostic) => Some(diagnostic),
        }
    }

    pub(crate) const fn into_diagnostic(self) -> Option<FieldDiagnostic> {
        match self {
            Self::Valid => None,
            Self::ValidWithDiagnostics(diagnostic) => Some(diagnostic),
        }
    }
}

/// Rich-text raw JSON은 통합 계층을 통과하지 않고 기존 restricted grammar validator 뒤에 숨는다.
pub(crate) trait PersistentRichTextValue {
    fn validate_persistent(&self) -> Result<(), RichTextValidationError>;
}

/// Artifact의 private FieldValue가 제공하는 immutable semantic view다.
#[derive(Clone, Copy)]
pub(crate) enum FieldValueView<'a> {
    Group(&'a crate::data::artifact::group::GroupValue),
    Unset,
    SingleLineText(&'a str),
    RichText(&'a dyn PersistentRichTextValue),
    Number(&'a str),
    NumberUnknown,
    Date(&'a str),
    Time(&'a str),
    Image(&'a [String]),
    File(&'a [String]),
    Url(&'a str),
    Duration(&'a str),
    SingleChoice(OptionId),
    MultiChoice(&'a [OptionId]),
    Relation(&'a [RelationLink]),
    DocumentLink(&'a [DocumentId]),
}

impl FieldValueView<'_> {
    pub(crate) const fn is_unset(self) -> bool {
        matches!(self, Self::Unset)
    }

    pub(crate) const fn kind(self) -> Option<FieldKind> {
        match self {
            Self::Group(_) => Some(FieldKind::Group),
            Self::Unset => None,
            Self::SingleLineText(_) => Some(FieldKind::SingleLineText),
            Self::RichText(_) => Some(FieldKind::RichText),
            Self::Number(_) | Self::NumberUnknown => Some(FieldKind::Number),
            Self::Date(_) => Some(FieldKind::Date),
            Self::Time(_) => Some(FieldKind::Time),
            Self::Image(_) => Some(FieldKind::Image),
            Self::File(_) => Some(FieldKind::File),
            Self::Url(_) => Some(FieldKind::Url),
            Self::Duration(_) => Some(FieldKind::Duration),
            Self::SingleChoice(_) => Some(FieldKind::SingleChoice),
            Self::MultiChoice(_) => Some(FieldKind::MultiChoice),
            Self::Relation(_) => Some(FieldKind::Relation),
            Self::DocumentLink(_) => Some(FieldKind::DocumentLink),
        }
    }
}

impl fmt::Debug for FieldValueView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldValueView")
            .field("kind", &self.kind())
            .field("is_unset", &self.is_unset())
            .field("payload_redacted", &!self.is_unset())
            .finish()
    }
}

/// Wire에 bounds가 추가되기 전에도 exact decimal constraint 판정을 독립적으로 시험한다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NumberConstraint {
    minimum: Option<CanonicalDecimal>,
    maximum: Option<CanonicalDecimal>,
}

impl NumberConstraint {
    pub(crate) fn try_new(
        minimum: Option<&str>,
        maximum: Option<&str>,
    ) -> Result<Self, FieldValidationError> {
        let minimum = minimum
            .map(CanonicalDecimal::parse)
            .transpose()
            .map_err(FieldValidationError::invalid_minimum)?;
        let maximum = maximum
            .map(CanonicalDecimal::parse)
            .transpose()
            .map_err(FieldValidationError::invalid_maximum)?;
        validate_number_constraint_order(minimum.as_ref(), maximum.as_ref())
            .map_err(|_| FieldValidationError::minimum_greater_than_maximum())?;
        Ok(Self { minimum, maximum })
    }

    fn validate(
        &self,
        value: &CanonicalDecimal,
        location: FieldValidationLocation,
    ) -> Result<(), FieldValidationError> {
        if self.minimum.as_ref().is_some_and(|minimum| value < minimum) {
            return Err(FieldValidationError::below_minimum(location));
        }
        if self.maximum.as_ref().is_some_and(|maximum| value > maximum) {
            return Err(FieldValidationError::above_maximum(location));
        }
        Ok(())
    }
}

impl fmt::Debug for NumberConstraint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NumberConstraint")
            .field("has_minimum", &self.minimum.is_some())
            .field("has_maximum", &self.maximum.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
enum FieldConfigurationVariant {
    Group,
    SingleLineText,
    RichText,
    Number(Option<NumberConstraint>),
    Date,
    Time,
    Image,
    File,
    Url,
    Duration,
    SingleChoice(ChoiceOptionLookup),
    MultiChoice(ChoiceOptionLookup),
    Relation { multiple: bool },
    DocumentLink,
}

/// Configuration은 label과 display order를 제외한 semantic 최소 view만 소유한다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FieldConfigurationView {
    variant: FieldConfigurationVariant,
}

impl FieldConfigurationView {
    pub(crate) const fn group() -> Self {
        Self {
            variant: FieldConfigurationVariant::Group,
        }
    }
    pub(crate) const fn single_line_text() -> Self {
        Self {
            variant: FieldConfigurationVariant::SingleLineText,
        }
    }

    pub(crate) const fn rich_text() -> Self {
        Self {
            variant: FieldConfigurationVariant::RichText,
        }
    }

    /// `None`은 v1 artifact number configuration처럼 실제 persisted bound가 없음을 뜻한다.
    pub(crate) const fn number(constraint: Option<NumberConstraint>) -> Self {
        Self {
            variant: FieldConfigurationVariant::Number(constraint),
        }
    }

    pub(crate) const fn date() -> Self {
        Self {
            variant: FieldConfigurationVariant::Date,
        }
    }

    pub(crate) const fn image() -> Self {
        Self {
            variant: FieldConfigurationVariant::Image,
        }
    }
    pub(crate) const fn file() -> Self {
        Self {
            variant: FieldConfigurationVariant::File,
        }
    }
    pub(crate) const fn url() -> Self {
        Self {
            variant: FieldConfigurationVariant::Url,
        }
    }
    pub(crate) const fn time() -> Self {
        Self {
            variant: FieldConfigurationVariant::Time,
        }
    }

    pub(crate) const fn duration() -> Self {
        Self {
            variant: FieldConfigurationVariant::Duration,
        }
    }

    pub(crate) fn try_single_choice(
        options: impl IntoIterator<Item = (OptionId, ChoiceOptionLifecycle)>,
    ) -> Result<Self, FieldValidationError> {
        ChoiceOptionLookup::try_from_options(options)
            .map(|lookup| Self {
                variant: FieldConfigurationVariant::SingleChoice(lookup),
            })
            .map_err(|error| {
                FieldValidationError::invalid_choice(error, FieldValidationLocation::FieldRule)
            })
    }

    pub(crate) fn try_multi_choice(
        options: impl IntoIterator<Item = (OptionId, ChoiceOptionLifecycle)>,
    ) -> Result<Self, FieldValidationError> {
        ChoiceOptionLookup::try_from_options(options)
            .map(|lookup| Self {
                variant: FieldConfigurationVariant::MultiChoice(lookup),
            })
            .map_err(|error| {
                FieldValidationError::invalid_choice(error, FieldValidationLocation::FieldRule)
            })
    }

    pub(crate) const fn relation(multiple: bool) -> Self {
        Self {
            variant: FieldConfigurationVariant::Relation { multiple },
        }
    }

    pub(crate) const fn document_link() -> Self {
        Self {
            variant: FieldConfigurationVariant::DocumentLink,
        }
    }

    pub(crate) const fn kind(&self) -> FieldKind {
        match self.variant {
            FieldConfigurationVariant::Group => FieldKind::Group,
            FieldConfigurationVariant::SingleLineText => FieldKind::SingleLineText,
            FieldConfigurationVariant::RichText => FieldKind::RichText,
            FieldConfigurationVariant::Number(_) => FieldKind::Number,
            FieldConfigurationVariant::Date => FieldKind::Date,
            FieldConfigurationVariant::Time => FieldKind::Time,
            FieldConfigurationVariant::Image => FieldKind::Image,
            FieldConfigurationVariant::File => FieldKind::File,
            FieldConfigurationVariant::Url => FieldKind::Url,
            FieldConfigurationVariant::Duration => FieldKind::Duration,
            FieldConfigurationVariant::SingleChoice(_) => FieldKind::SingleChoice,
            FieldConfigurationVariant::MultiChoice(_) => FieldKind::MultiChoice,
            FieldConfigurationVariant::Relation { .. } => FieldKind::Relation,
            FieldConfigurationVariant::DocumentLink => FieldKind::DocumentLink,
        }
    }

    fn choice_lookup(&self) -> Option<&ChoiceOptionLookup> {
        match &self.variant {
            FieldConfigurationVariant::SingleChoice(lookup)
            | FieldConfigurationVariant::MultiChoice(lookup) => Some(lookup),
            _ => None,
        }
    }

    fn number_constraint(&self) -> Option<&NumberConstraint> {
        match &self.variant {
            FieldConfigurationVariant::Number(constraint) => constraint.as_ref(),
            _ => None,
        }
    }

    fn relation_multiple(&self) -> Option<bool> {
        match self.variant {
            FieldConfigurationVariant::Relation { multiple } => Some(multiple),
            _ => None,
        }
    }
}

impl fmt::Debug for FieldConfigurationView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut result = formatter.debug_struct("FieldConfigurationView");
        result.field("kind", &self.kind());
        match &self.variant {
            FieldConfigurationVariant::Number(constraint) => {
                result.field("has_number_constraint", &constraint.is_some());
            }
            FieldConfigurationVariant::SingleChoice(lookup)
            | FieldConfigurationVariant::MultiChoice(lookup) => {
                result.field("choice_lookup", lookup);
            }
            _ => {}
        }
        result.finish()
    }
}

/// 유효한 kind/configuration 조합과 required/lifecycle 정책을 한 값으로 묶는다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FieldRule {
    kind: FieldKind,
    lifecycle: FieldLifecycleView,
    requiredness: Requiredness,
    configuration: FieldConfigurationView,
}

impl FieldRule {
    pub(crate) fn try_new(
        kind: FieldKind,
        lifecycle: FieldLifecycleView,
        requiredness: Requiredness,
        configuration: FieldConfigurationView,
    ) -> Result<Self, FieldValidationError> {
        if configuration.kind() != kind {
            return Err(FieldValidationError::configuration_kind_mismatch());
        }
        Ok(Self {
            kind,
            lifecycle,
            requiredness,
            configuration,
        })
    }

    pub(crate) const fn kind(&self) -> FieldKind {
        self.kind
    }

    pub(crate) const fn lifecycle(&self) -> FieldLifecycleView {
        self.lifecycle
    }

    pub(crate) const fn requiredness(&self) -> Requiredness {
        self.requiredness
    }
}

impl fmt::Debug for FieldRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldRule")
            .field("kind", &self.kind)
            .field("lifecycle", &self.lifecycle)
            .field("requiredness", &self.requiredness)
            .field("configuration", &self.configuration)
            .finish()
    }
}

/// Template default는 required Field에서도 unset을 허용하고 값 자체만 검증한다.
pub(crate) fn validate_template_default(
    rule: &FieldRule,
    value: FieldValueView<'_>,
    context: TemplateDefaultContext,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let location = match context {
        TemplateDefaultContext::ActiveFieldCurrentDefault => {
            FieldValidationLocation::ActiveFieldCurrentDefault
        }
        TemplateDefaultContext::HistoricalInitialDefault => {
            FieldValidationLocation::HistoricalInitialDefault
        }
        TemplateDefaultContext::ArchivedFieldPreserved => {
            FieldValidationLocation::ArchivedFieldPreservedDefault
        }
    };
    let lifecycle_matches = matches!(
        (rule.lifecycle, context),
        (
            FieldLifecycleView::Active,
            TemplateDefaultContext::ActiveFieldCurrentDefault
                | TemplateDefaultContext::HistoricalInitialDefault
        ) | (
            FieldLifecycleView::Archived,
            TemplateDefaultContext::ArchivedFieldPreserved
        )
    );
    if !lifecycle_matches {
        return Err(FieldValidationError::context_lifecycle_mismatch(location));
    }
    if value.is_unset() {
        return Ok(FieldValidationOutcome::Valid);
    }

    let choice_context = match context {
        TemplateDefaultContext::ActiveFieldCurrentDefault => {
            ChoiceMembershipContext::ActiveFieldCurrentDefault
        }
        TemplateDefaultContext::HistoricalInitialDefault => {
            ChoiceMembershipContext::HistoricalInitialDefault
        }
        TemplateDefaultContext::ArchivedFieldPreserved => {
            ChoiceMembershipContext::ArchivedFieldPreservedDefault
        }
    };
    validate_value_against_rule(rule, value, choice_context, location)
}

/// Template과 결합된 final Document 값에서만 active required를 강제한다.
pub(crate) fn validate_bound_document_value(
    rule: &FieldRule,
    value: FieldValueView<'_>,
    context: BoundDocumentValueContext,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let location = match context {
        BoundDocumentValueContext::NewDocumentValue => FieldValidationLocation::NewDocumentValue,
        BoundDocumentValueContext::ExistingDocumentValue => {
            FieldValidationLocation::ExistingDocumentValue
        }
        BoundDocumentValueContext::MaterializedHistorical => {
            FieldValidationLocation::MaterializedHistoricalValue
        }
    };
    if matches!(
        (rule.lifecycle, context),
        (
            FieldLifecycleView::Archived,
            BoundDocumentValueContext::NewDocumentValue
        ) | (
            FieldLifecycleView::Archived,
            BoundDocumentValueContext::MaterializedHistorical
        )
    ) {
        return Err(FieldValidationError::context_lifecycle_mismatch(location));
    }
    if value.is_unset() {
        if rule.lifecycle == FieldLifecycleView::Active
            && rule.requiredness == Requiredness::Required
        {
            return Err(FieldValidationError::required_value_unset(location));
        }
        return Ok(FieldValidationOutcome::Valid);
    }

    let choice_context = match (rule.lifecycle, context) {
        (FieldLifecycleView::Archived, BoundDocumentValueContext::ExistingDocumentValue) => {
            ChoiceMembershipContext::ExistingDocumentValue
        }
        (FieldLifecycleView::Active, BoundDocumentValueContext::NewDocumentValue) => {
            ChoiceMembershipContext::NewDocumentOrNewSelection
        }
        (FieldLifecycleView::Active, BoundDocumentValueContext::ExistingDocumentValue) => {
            ChoiceMembershipContext::ExistingDocumentValue
        }
        (FieldLifecycleView::Active, BoundDocumentValueContext::MaterializedHistorical) => {
            ChoiceMembershipContext::HistoricalInitialDefault
        }
        (FieldLifecycleView::Archived, _) => {
            return Err(FieldValidationError::context_lifecycle_mismatch(location));
        }
    };
    validate_value_against_rule(rule, value, choice_context, location)
}

/// Template이 없는 Document codec은 required와 choice membership을 추측하지 않는다.
pub(crate) fn validate_standalone_document_value(
    value: FieldValueView<'_>,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let location = FieldValidationLocation::StandaloneDocumentValue;
    match value {
        FieldValueView::Group(v) => v.validate(),
        FieldValueView::Unset | FieldValueView::NumberUnknown | FieldValueView::SingleChoice(_) => {
            Ok(FieldValidationOutcome::Valid)
        }
        FieldValueView::SingleLineText(value) => SingleLineText::parse(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
        FieldValueView::RichText(value) => value
            .validate_persistent()
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_rich_text(error, location)),
        FieldValueView::Number(value) => CanonicalDecimal::parse(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
        FieldValueView::Date(value) => CalendarDate::parse(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
        FieldValueView::Image(ids) | FieldValueView::File(ids) => {
            if ids.is_empty()
                || ids.len() > 32
                || ids.iter().any(|id| !crate::data::media::valid_id(id))
                || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
            {
                Err(FieldValidationError::new(
                    FieldValidationErrorCategory::InvalidAssetReference,
                    location,
                    "invalid asset references",
                ))
            } else {
                Ok(FieldValidationOutcome::Valid)
            }
        }
        FieldValueView::Url(value) => crate::data::media::media_url(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|_| {
                FieldValidationError::new(
                    FieldValidationErrorCategory::InvalidMediaUrl,
                    location,
                    "unsupported media URL",
                )
            }),
        FieldValueView::Time(value) => LocalTime::parse(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
        FieldValueView::Duration(value) => DurationMilliseconds::parse(value)
            .map(|_| FieldValidationOutcome::Valid)
            .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
        FieldValueView::MultiChoice(option_ids) => {
            super::choice::validate_persistent_multi_choice(option_ids)
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_choice(error, location))
        }
        FieldValueView::Relation(links) => validate_relation_links(links, location),
        FieldValueView::DocumentLink(documents) => validate_document_links(documents, location),
    }
}

fn invalid_reference(location: FieldValidationLocation) -> FieldValidationError {
    FieldValidationError::new(
        FieldValidationErrorCategory::InvalidDocumentReference,
        location,
        "document references must be non-empty, canonical and unique within one field value",
    )
}

fn validate_relation_links(
    links: &[RelationLink],
    location: FieldValidationLocation,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let ids: std::collections::BTreeSet<ReferenceId> = links.iter().map(RelationLink::id).collect();
    let documents: std::collections::BTreeSet<DocumentId> =
        links.iter().map(RelationLink::document).collect();
    if links.is_empty()
        || links.len() > 256
        || ids.len() != links.len()
        || documents.len() != links.len()
        || links
            .iter()
            .any(|link| !link.name().is_empty() && SingleLineText::parse(link.name()).is_err())
    {
        return Err(invalid_reference(location));
    }
    Ok(FieldValidationOutcome::Valid)
}

fn validate_document_links(
    documents: &[DocumentId],
    location: FieldValidationLocation,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let unique: std::collections::BTreeSet<_> = documents.iter().copied().collect();
    if documents.is_empty() || documents.len() > 256 || unique.len() != documents.len() {
        return Err(invalid_reference(location));
    }
    Ok(FieldValidationOutcome::Valid)
}

fn validate_value_against_rule(
    rule: &FieldRule,
    value: FieldValueView<'_>,
    choice_context: ChoiceMembershipContext,
    location: FieldValidationLocation,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    if value.kind() != Some(rule.kind) {
        return Err(FieldValidationError::value_kind_mismatch(location));
    }

    match rule.kind {
        FieldKind::SingleLineText => match value {
            FieldValueView::SingleLineText(value) => SingleLineText::parse(value)
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::RichText => match value {
            FieldValueView::RichText(value) => value
                .validate_persistent()
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_rich_text(error, location)),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::Number => match value {
            FieldValueView::NumberUnknown => Ok(FieldValidationOutcome::Valid),
            FieldValueView::Number(value) => {
                let value = CanonicalDecimal::parse(value)
                    .map_err(|error| FieldValidationError::invalid_scalar(error, location))?;
                if let Some(constraint) = rule.configuration.number_constraint() {
                    if let Err(error) = constraint.validate(&value, location) {
                        if matches!(
                            location,
                            FieldValidationLocation::ExistingDocumentValue
                                | FieldValidationLocation::MaterializedHistoricalValue
                        ) {
                            return Ok(FieldValidationOutcome::ValidWithDiagnostics(
                                FieldDiagnostic {
                                    category: FieldDiagnosticCategory::ExistingNumberOutsideRange,
                                    archived_option_count: 0,
                                },
                            ));
                        }
                        return Err(error);
                    }
                }
                Ok(FieldValidationOutcome::Valid)
            }
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::Group => validate_standalone_document_value(value),
        FieldKind::Date => match value {
            FieldValueView::Date(value) => CalendarDate::parse(value)
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::Image | FieldKind::File | FieldKind::Url => {
            validate_standalone_document_value(value)
        }
        FieldKind::Time => match value {
            FieldValueView::Time(value) => LocalTime::parse(value)
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::Duration => match value {
            FieldValueView::Duration(value) => DurationMilliseconds::parse(value)
                .map(|_| FieldValidationOutcome::Valid)
                .map_err(|error| FieldValidationError::invalid_scalar(error, location)),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::SingleChoice => match value {
            FieldValueView::SingleChoice(option_id) => validate_choice(
                rule,
                ChoiceSelection::Single(option_id),
                choice_context,
                location,
            ),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::MultiChoice => match value {
            FieldValueView::MultiChoice(option_ids) => validate_choice(
                rule,
                ChoiceSelection::Multi(option_ids),
                choice_context,
                location,
            ),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::Relation => match value {
            FieldValueView::Relation(links) => {
                let _ = validate_relation_links(links, location)?;
                if rule.configuration.relation_multiple() == Some(false) && links.len() > 1 {
                    if matches!(
                        location,
                        FieldValidationLocation::ExistingDocumentValue
                            | FieldValidationLocation::MaterializedHistoricalValue
                    ) {
                        return Ok(FieldValidationOutcome::ValidWithDiagnostics(
                            FieldDiagnostic {
                                category:
                                    FieldDiagnosticCategory::ExistingRelationExceedsMultiplicity,
                                archived_option_count: 0,
                            },
                        ));
                    }
                    return Err(invalid_reference(location));
                }
                Ok(FieldValidationOutcome::Valid)
            }
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
        FieldKind::DocumentLink => match value {
            FieldValueView::DocumentLink(documents) => validate_document_links(documents, location),
            _ => Err(FieldValidationError::value_kind_mismatch(location)),
        },
    }
}

fn validate_choice(
    rule: &FieldRule,
    selection: ChoiceSelection<'_>,
    context: ChoiceMembershipContext,
    location: FieldValidationLocation,
) -> Result<FieldValidationOutcome, FieldValidationError> {
    let Some(lookup) = rule.configuration.choice_lookup() else {
        return Err(FieldValidationError::configuration_kind_mismatch());
    };
    match validate_choice_membership(selection, lookup, context)
        .map_err(|error| FieldValidationError::invalid_choice(error, location))?
    {
        ChoiceMembershipOutcome::Valid => Ok(FieldValidationOutcome::Valid),
        ChoiceMembershipOutcome::ValidWithDiagnostic(diagnostic) => {
            let category = match diagnostic.category() {
                ChoiceMembershipDiagnosticCategory::ExistingSelectionContainsArchivedOption => {
                    FieldDiagnosticCategory::ExistingSelectionContainsArchivedOption
                }
            };
            Ok(FieldValidationOutcome::ValidWithDiagnostics(
                FieldDiagnostic {
                    category,
                    archived_option_count: diagnostic.archived_option_count(),
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, str::FromStr};

    use serde_json::{json, Map, Value};

    use super::*;
    use crate::data::field_engine::rich_text::{
        validate_persistent_rich_text, RICH_TEXT_SCHEMA_VERSION,
    };

    const ACTIVE: &str = "11111111-1111-4111-8111-111111111111";
    const ARCHIVED: &str = "22222222-2222-4222-8222-222222222222";
    const UNKNOWN: &str = "33333333-3333-4333-8333-333333333333";
    const ARCHIVED_SECOND: &str = "44444444-4444-4444-8444-444444444444";
    const ALL_KINDS: [FieldKind; 8] = [
        FieldKind::SingleLineText,
        FieldKind::RichText,
        FieldKind::Number,
        FieldKind::Date,
        FieldKind::Time,
        FieldKind::Duration,
        FieldKind::SingleChoice,
        FieldKind::MultiChoice,
    ];

    fn option(value: &str) -> OptionId {
        OptionId::from_str(value).expect("test OptionId should be canonical UUID v4")
    }

    struct TestRichText {
        content: Map<String, Value>,
    }

    impl PersistentRichTextValue for TestRichText {
        fn validate_persistent(&self) -> Result<(), RichTextValidationError> {
            validate_persistent_rich_text(RICH_TEXT_SCHEMA_VERSION, &self.content)
        }
    }

    fn rich_text(text: Option<&str>) -> TestRichText {
        let children = text.map_or_else(
            || vec![json!({"kind":"hardBreak"})],
            |text| vec![json!({"kind":"text","text":text})],
        );
        TestRichText {
            content: json!({
                "kind":"root",
                "children":[{"kind":"paragraph","children":children}]
            })
            .as_object()
            .expect("test rich-text root should be an object")
            .clone(),
        }
    }

    fn choice_options() -> [(OptionId, ChoiceOptionLifecycle); 3] {
        [
            (option(ACTIVE), ChoiceOptionLifecycle::Active),
            (option(ARCHIVED), ChoiceOptionLifecycle::Archived),
            (option(ARCHIVED_SECOND), ChoiceOptionLifecycle::Archived),
        ]
    }

    fn configuration(kind: FieldKind) -> FieldConfigurationView {
        match kind {
            FieldKind::Group => FieldConfigurationView::group(),
            FieldKind::SingleLineText => FieldConfigurationView::single_line_text(),
            FieldKind::RichText => FieldConfigurationView::rich_text(),
            FieldKind::Number => FieldConfigurationView::number(None),
            FieldKind::Date => FieldConfigurationView::date(),
            FieldKind::Time => FieldConfigurationView::time(),
            FieldKind::Image => FieldConfigurationView::image(),
            FieldKind::File => FieldConfigurationView::file(),
            FieldKind::Url => FieldConfigurationView::url(),
            FieldKind::Duration => FieldConfigurationView::duration(),
            FieldKind::SingleChoice => {
                FieldConfigurationView::try_single_choice(choice_options()).unwrap()
            }
            FieldKind::MultiChoice => {
                FieldConfigurationView::try_multi_choice(choice_options()).unwrap()
            }
            FieldKind::Relation => FieldConfigurationView::relation(true),
            FieldKind::DocumentLink => FieldConfigurationView::document_link(),
        }
    }

    fn rule(kind: FieldKind, requiredness: Requiredness) -> FieldRule {
        FieldRule::try_new(
            kind,
            FieldLifecycleView::Active,
            requiredness,
            configuration(kind),
        )
        .unwrap()
    }

    fn valid_value<'a>(kind: FieldKind, rich: &'a TestRichText) -> FieldValueView<'a> {
        match kind {
            FieldKind::Group => FieldValueView::Unset,
            FieldKind::SingleLineText => FieldValueView::SingleLineText("  "),
            FieldKind::RichText => FieldValueView::RichText(rich),
            FieldKind::Number => FieldValueView::Number("12345678901234567890.01"),
            FieldKind::Date => FieldValueView::Date("2024-02-29"),
            FieldKind::Time => FieldValueView::Time("23:59:59.999"),
            FieldKind::Image => FieldValueView::Image(&[]),
            FieldKind::File => FieldValueView::File(&[]),
            FieldKind::Url => FieldValueView::Url("https://example.com/video.mp4"),
            FieldKind::Duration => FieldValueView::Duration("-9223372036854775808"),
            FieldKind::SingleChoice => FieldValueView::SingleChoice(option(ACTIVE)),
            FieldKind::MultiChoice => FieldValueView::MultiChoice(&[]),
            FieldKind::Relation => FieldValueView::Relation(&[]),
            FieldKind::DocumentLink => FieldValueView::DocumentLink(&[]),
        }
    }

    fn valid_multi() -> [OptionId; 1] {
        [option(ACTIVE)]
    }

    #[test]
    fn all_eight_kinds_share_the_integrated_context_matrix() {
        let rich = rich_text(Some("본문"));
        let multi = valid_multi();
        for kind in ALL_KINDS {
            let optional = rule(kind, Requiredness::Optional);
            let required = rule(kind, Requiredness::Required);
            let value = if kind == FieldKind::MultiChoice {
                FieldValueView::MultiChoice(&multi)
            } else {
                valid_value(kind, &rich)
            };

            assert_eq!(
                validate_template_default(
                    &required,
                    value,
                    TemplateDefaultContext::ActiveFieldCurrentDefault,
                ),
                Ok(FieldValidationOutcome::Valid),
                "current default for {kind:?}"
            );
            assert_eq!(
                validate_template_default(
                    &required,
                    value,
                    TemplateDefaultContext::HistoricalInitialDefault,
                ),
                Ok(FieldValidationOutcome::Valid),
                "initial default for {kind:?}"
            );
            assert_eq!(
                validate_standalone_document_value(value),
                Ok(FieldValidationOutcome::Valid),
                "standalone value for {kind:?}"
            );
            assert_eq!(
                validate_bound_document_value(
                    &optional,
                    value,
                    BoundDocumentValueContext::NewDocumentValue,
                ),
                Ok(FieldValidationOutcome::Valid),
                "bound value for {kind:?}"
            );
            assert_eq!(
                validate_bound_document_value(
                    &optional,
                    FieldValueView::Unset,
                    BoundDocumentValueContext::ExistingDocumentValue,
                ),
                Ok(FieldValidationOutcome::Valid),
                "optional unset for {kind:?}"
            );
            assert_eq!(
                validate_bound_document_value(
                    &required,
                    FieldValueView::Unset,
                    BoundDocumentValueContext::NewDocumentValue,
                )
                .unwrap_err()
                .category(),
                FieldValidationErrorCategory::RequiredValueUnset,
                "required unset for {kind:?}"
            );

            let wrong_value = if kind == FieldKind::SingleLineText {
                FieldValueView::Number("1")
            } else {
                FieldValueView::SingleLineText("x")
            };
            assert_eq!(
                validate_bound_document_value(
                    &optional,
                    wrong_value,
                    BoundDocumentValueContext::ExistingDocumentValue,
                )
                .unwrap_err()
                .category(),
                FieldValidationErrorCategory::FieldValueKindMismatch,
                "wrong value for {kind:?}"
            );

            let wrong_configuration = if kind == FieldKind::SingleLineText {
                FieldConfigurationView::number(None)
            } else {
                FieldConfigurationView::single_line_text()
            };
            assert_eq!(
                FieldRule::try_new(
                    kind,
                    FieldLifecycleView::Active,
                    Requiredness::Optional,
                    wrong_configuration,
                )
                .unwrap_err()
                .category(),
                FieldValidationErrorCategory::FieldConfigurationKindMismatch,
                "wrong configuration for {kind:?}"
            );
        }
    }

    #[test]
    fn each_kind_rejects_its_invalid_semantic_value() {
        let invalid_rich = rich_text(None);
        let empty_multi: [OptionId; 0] = [];
        for (kind, value) in [
            (
                FieldKind::SingleLineText,
                FieldValueView::SingleLineText(""),
            ),
            (FieldKind::RichText, FieldValueView::RichText(&invalid_rich)),
            (FieldKind::Number, FieldValueView::Number("01")),
            (FieldKind::Date, FieldValueView::Date("2023-02-29")),
            (FieldKind::Time, FieldValueView::Time("24:00:00.000")),
            (FieldKind::Duration, FieldValueView::Duration("+1")),
            (
                FieldKind::SingleChoice,
                FieldValueView::SingleChoice(option(UNKNOWN)),
            ),
            (
                FieldKind::MultiChoice,
                FieldValueView::MultiChoice(&empty_multi),
            ),
        ] {
            let error = validate_bound_document_value(
                &rule(kind, Requiredness::Optional),
                value,
                BoundDocumentValueContext::ExistingDocumentValue,
            )
            .unwrap_err();
            assert!(
                matches!(
                    error.category(),
                    FieldValidationErrorCategory::InvalidScalarValue
                        | FieldValidationErrorCategory::InvalidChoiceValue
                        | FieldValidationErrorCategory::InvalidRichTextValue
                ),
                "invalid semantic value for {kind:?}: {error:?}"
            );
        }
    }

    #[test]
    fn relation_name_is_optional_single_line_text_bound_to_the_same_edge() {
        let reference: ReferenceId = "55555555-5555-4555-8555-555555555555".parse().unwrap();
        let document: DocumentId = "66666666-6666-4666-8666-666666666666".parse().unwrap();
        let valid = [RelationLink::named(
            reference,
            document,
            true,
            "부모".into(),
        )];
        assert_eq!(
            validate_standalone_document_value(FieldValueView::Relation(&valid)),
            Ok(FieldValidationOutcome::Valid)
        );
        let unnamed = [RelationLink::new(reference, document, true)];
        assert_eq!(
            validate_standalone_document_value(FieldValueView::Relation(&unnamed)),
            Ok(FieldValidationOutcome::Valid)
        );
        let multiline = [RelationLink::named(
            reference,
            document,
            true,
            "부모\n자식".into(),
        )];
        assert_eq!(
            validate_standalone_document_value(FieldValueView::Relation(&multiline))
                .unwrap_err()
                .category(),
            FieldValidationErrorCategory::InvalidDocumentReference
        );
    }

    #[test]
    fn required_policy_distinguishes_defaults_bound_values_and_archived_fields() {
        let required = rule(FieldKind::SingleLineText, Requiredness::Required);
        for context in [
            TemplateDefaultContext::ActiveFieldCurrentDefault,
            TemplateDefaultContext::HistoricalInitialDefault,
        ] {
            assert_eq!(
                validate_template_default(&required, FieldValueView::Unset, context),
                Ok(FieldValidationOutcome::Valid)
            );
        }
        for context in [
            BoundDocumentValueContext::NewDocumentValue,
            BoundDocumentValueContext::ExistingDocumentValue,
            BoundDocumentValueContext::MaterializedHistorical,
        ] {
            let error = validate_bound_document_value(&required, FieldValueView::Unset, context)
                .unwrap_err();
            assert_eq!(
                error.category(),
                FieldValidationErrorCategory::RequiredValueUnset
            );
            assert!(error.option_id().is_none());
        }

        let archived = FieldRule::try_new(
            FieldKind::SingleLineText,
            FieldLifecycleView::Archived,
            Requiredness::Required,
            FieldConfigurationView::single_line_text(),
        )
        .unwrap();
        assert_eq!(
            validate_template_default(
                &archived,
                FieldValueView::Unset,
                TemplateDefaultContext::ArchivedFieldPreserved,
            ),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_bound_document_value(
                &archived,
                FieldValueView::Unset,
                BoundDocumentValueContext::ExistingDocumentValue,
            ),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_standalone_document_value(FieldValueView::Unset),
            Ok(FieldValidationOutcome::Valid)
        );
    }

    #[test]
    fn whitespace_text_satisfies_required_but_hard_break_only_rich_text_does_not() {
        let text = rule(FieldKind::SingleLineText, Requiredness::Required);
        assert_eq!(
            validate_bound_document_value(
                &text,
                FieldValueView::SingleLineText(" \t "),
                BoundDocumentValueContext::NewDocumentValue,
            ),
            Ok(FieldValidationOutcome::Valid)
        );

        let empty_rich = rich_text(None);
        let rich = rule(FieldKind::RichText, Requiredness::Required);
        let error = validate_bound_document_value(
            &rich,
            FieldValueView::RichText(&empty_rich),
            BoundDocumentValueContext::NewDocumentValue,
        )
        .unwrap_err();
        assert_eq!(
            error.rich_text_category(),
            Some(RichTextValidationErrorCategory::SemanticEmpty)
        );
    }

    #[test]
    fn number_constraints_use_exact_inclusive_arbitrary_precision_ordering() {
        let min_only = NumberConstraint::try_new(Some("-10.5"), None).unwrap();
        let max_only = NumberConstraint::try_new(None, Some("10.5")).unwrap();
        let bounded = NumberConstraint::try_new(
            Some("-999999999999999999999999999999.0001"),
            Some("999999999999999999999999999999.0001"),
        )
        .unwrap();

        for (constraint, values) in [
            (&min_only, ["-10.5", "0", "999999999999999999999999"]),
            (&max_only, ["-999999999999999999999999", "0", "10.5"]),
            (
                &bounded,
                [
                    "-999999999999999999999999999999.0001",
                    "-0.0001",
                    "999999999999999999999999999999.0001",
                ],
            ),
        ] {
            let rule = FieldRule::try_new(
                FieldKind::Number,
                FieldLifecycleView::Active,
                Requiredness::Optional,
                FieldConfigurationView::number(Some(constraint.clone())),
            )
            .unwrap();
            for value in values {
                assert_eq!(
                    validate_bound_document_value(
                        &rule,
                        FieldValueView::Number(value),
                        BoundDocumentValueContext::ExistingDocumentValue,
                    ),
                    Ok(FieldValidationOutcome::Valid)
                );
            }
        }

        let fractional = NumberConstraint::try_new(Some("1.2"), Some("1.21")).unwrap();
        let fractional_rule = FieldRule::try_new(
            FieldKind::Number,
            FieldLifecycleView::Active,
            Requiredness::Optional,
            FieldConfigurationView::number(Some(fractional)),
        )
        .unwrap();
        assert_eq!(
            validate_bound_document_value(
                &fractional_rule,
                FieldValueView::Number("1.201"),
                BoundDocumentValueContext::ExistingDocumentValue,
            ),
            Ok(FieldValidationOutcome::Valid)
        );

        let default_rule = FieldRule::try_new(
            FieldKind::Number,
            FieldLifecycleView::Active,
            Requiredness::Required,
            FieldConfigurationView::number(Some(
                NumberConstraint::try_new(Some("1"), Some("2")).unwrap(),
            )),
        )
        .unwrap();
        for context in [
            TemplateDefaultContext::ActiveFieldCurrentDefault,
            TemplateDefaultContext::HistoricalInitialDefault,
        ] {
            assert_eq!(
                validate_template_default(&default_rule, FieldValueView::Number("1.5"), context),
                Ok(FieldValidationOutcome::Valid)
            );
            assert_eq!(
                validate_template_default(&default_rule, FieldValueView::Number("0.9"), context)
                    .unwrap_err()
                    .category(),
                FieldValidationErrorCategory::BelowMinimum
            );
        }
    }

    #[test]
    fn number_constraint_categories_cover_invalid_order_and_both_sides() {
        assert_eq!(
            NumberConstraint::try_new(Some("01"), None)
                .unwrap_err()
                .category(),
            FieldValidationErrorCategory::InvalidMinimum
        );
        assert_eq!(
            NumberConstraint::try_new(None, Some("1.0"))
                .unwrap_err()
                .category(),
            FieldValidationErrorCategory::InvalidMaximum
        );
        assert_eq!(
            NumberConstraint::try_new(Some("2"), Some("1"))
                .unwrap_err()
                .category(),
            FieldValidationErrorCategory::MinimumGreaterThanMaximum
        );

        let constraint = NumberConstraint::try_new(Some("-1"), Some("1")).unwrap();
        let rule = FieldRule::try_new(
            FieldKind::Number,
            FieldLifecycleView::Active,
            Requiredness::Optional,
            FieldConfigurationView::number(Some(constraint)),
        )
        .unwrap();
        for (value, expected) in [
            ("-1.0001", FieldValidationErrorCategory::BelowMinimum),
            ("1.0001", FieldValidationErrorCategory::AboveMaximum),
        ] {
            assert!(!validate_bound_document_value(
                &rule,
                FieldValueView::Number(value),
                BoundDocumentValueContext::ExistingDocumentValue
            )
            .unwrap()
            .is_plain_valid());
            assert_eq!(
                validate_bound_document_value(
                    &rule,
                    FieldValueView::Number(value),
                    BoundDocumentValueContext::NewDocumentValue,
                )
                .unwrap_err()
                .category(),
                expected
            );
        }
    }

    #[test]
    fn choice_contexts_preserve_diagnostics_and_prioritize_unknown_errors() {
        let active = rule(FieldKind::MultiChoice, Requiredness::Optional);
        let active_only = [option(ACTIVE)];
        let archived = [option(ARCHIVED)];
        let active_archived = [option(ACTIVE), option(ARCHIVED)];
        let multiple_archived = [option(ACTIVE), option(ARCHIVED), option(ARCHIVED_SECOND)];
        let archived_unknown = [option(ARCHIVED), option(UNKNOWN)];

        let valid = validate_bound_document_value(
            &active,
            FieldValueView::MultiChoice(&active_only),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap();
        assert!(valid.is_plain_valid());
        assert!(valid.diagnostic().is_none());
        assert!(valid.into_diagnostic().is_none());

        assert_eq!(
            validate_template_default(
                &active,
                FieldValueView::MultiChoice(&archived),
                TemplateDefaultContext::ActiveFieldCurrentDefault,
            )
            .unwrap_err()
            .choice_category(),
            Some(ChoiceValidationErrorCategory::ArchivedOptionNotSelectable)
        );
        assert_eq!(
            validate_template_default(
                &active,
                FieldValueView::MultiChoice(&archived),
                TemplateDefaultContext::HistoricalInitialDefault,
            ),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_bound_document_value(
                &active,
                FieldValueView::MultiChoice(&archived),
                BoundDocumentValueContext::MaterializedHistorical,
            ),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_bound_document_value(
                &active,
                FieldValueView::MultiChoice(&archived),
                BoundDocumentValueContext::NewDocumentValue,
            )
            .unwrap_err()
            .choice_category(),
            Some(ChoiceValidationErrorCategory::ArchivedOptionNotSelectable)
        );

        let outcome = validate_bound_document_value(
            &active,
            FieldValueView::MultiChoice(&active_archived),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap();
        assert!(!outcome.is_plain_valid());
        let diagnostic = outcome
            .diagnostic()
            .expect("archived existing selection must return a diagnostic");
        assert_eq!(
            diagnostic.category(),
            FieldDiagnosticCategory::ExistingSelectionContainsArchivedOption
        );
        assert_eq!(diagnostic.archived_option_count(), 1);
        let diagnostic_debug = format!("{outcome:?}");
        for forbidden in [
            ACTIVE,
            ARCHIVED,
            "private label",
            "credential=choice-secret",
        ] {
            assert!(!diagnostic_debug.contains(forbidden));
        }
        let owned_diagnostic = outcome
            .into_diagnostic()
            .expect("diagnostic ownership must be recoverable exactly once from the outcome");
        assert_eq!(owned_diagnostic.archived_option_count(), 1);

        let multiple = validate_bound_document_value(
            &active,
            FieldValueView::MultiChoice(&multiple_archived),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap()
        .into_diagnostic()
        .expect("all archived selections must remain observable");
        assert_eq!(multiple.archived_option_count(), 2);

        let error = validate_bound_document_value(
            &active,
            FieldValueView::MultiChoice(&archived_unknown),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap_err();
        assert_eq!(
            error.choice_category(),
            Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
        );
        assert_eq!(error.option_id(), Some(option(UNKNOWN)));
    }

    #[test]
    fn archived_field_preservation_skips_required_but_keeps_existing_choice_diagnostic() {
        let archived_rule = FieldRule::try_new(
            FieldKind::SingleChoice,
            FieldLifecycleView::Archived,
            Requiredness::Required,
            FieldConfigurationView::try_single_choice(choice_options()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            validate_template_default(
                &archived_rule,
                FieldValueView::SingleChoice(option(ARCHIVED)),
                TemplateDefaultContext::ArchivedFieldPreserved,
            ),
            Ok(FieldValidationOutcome::Valid)
        );
        let diagnostic = validate_bound_document_value(
            &archived_rule,
            FieldValueView::SingleChoice(option(ARCHIVED)),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap()
        .into_diagnostic()
        .expect("existing archived selection remains diagnostic on an archived Field");
        assert_eq!(diagnostic.archived_option_count(), 1);
    }

    #[test]
    fn standalone_validation_checks_shape_but_not_required_or_membership() {
        assert_eq!(
            validate_standalone_document_value(FieldValueView::SingleChoice(option(UNKNOWN))),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_standalone_document_value(FieldValueView::Unset),
            Ok(FieldValidationOutcome::Valid)
        );
        assert_eq!(
            validate_standalone_document_value(FieldValueView::MultiChoice(&[]))
                .unwrap_err()
                .choice_category(),
            Some(ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset)
        );
    }

    #[test]
    fn contexts_reject_impossible_lifecycle_combinations() {
        let archived = FieldRule::try_new(
            FieldKind::Date,
            FieldLifecycleView::Archived,
            Requiredness::Optional,
            FieldConfigurationView::date(),
        )
        .unwrap();
        assert_eq!(
            validate_template_default(
                &archived,
                FieldValueView::Unset,
                TemplateDefaultContext::ActiveFieldCurrentDefault,
            )
            .unwrap_err()
            .category(),
            FieldValidationErrorCategory::ContextLifecycleMismatch
        );
        assert_eq!(
            validate_bound_document_value(
                &archived,
                FieldValueView::Unset,
                BoundDocumentValueContext::NewDocumentValue,
            )
            .unwrap_err()
            .category(),
            FieldValidationErrorCategory::ContextLifecycleMismatch
        );
    }

    #[test]
    fn error_outcome_and_views_redact_payloads_and_have_no_source_chain() {
        let raw = "credential=field-engine-secret\nC:\\private\\field.txt";
        let rule = rule(FieldKind::SingleLineText, Requiredness::Optional);
        let value = FieldValueView::SingleLineText(raw);
        let error = validate_bound_document_value(
            &rule,
            value,
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .unwrap_err();
        for rendered in [
            format!("{value:?}"),
            format!("{error:?}"),
            error.to_string(),
        ] {
            assert!(!rendered.contains(raw));
            assert!(!rendered.contains("credential=field-engine-secret"));
            assert!(!rendered.contains("C:\\private\\field.txt"));
        }
        assert!(error.source().is_none());

        let constraint = NumberConstraint::try_new(Some("123456789.1"), Some("123456789.2"))
            .expect("constraint should be valid");
        let rendered = format!("{constraint:?}");
        assert!(!rendered.contains("123456789"));
    }
}
