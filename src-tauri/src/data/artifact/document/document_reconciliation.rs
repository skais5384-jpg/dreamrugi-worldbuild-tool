use std::fmt;

use crate::data::field_engine::{
    choice::ChoiceValidationErrorCategory,
    rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
    scalar::ScalarValueErrorCategory,
    validation::{
        BoundDocumentValueContext, FieldDiagnosticCategory, FieldValidationError,
        FieldValidationErrorCategory, FieldValidationLocation, FieldValidationOutcome,
    },
};

use super::{
    document_snapshot::{assess_snapshot, SnapshotAction},
    DocumentArtifact, OrphanedFieldDefinition,
};
use crate::data::artifact::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactValidationError, ArtifactValidationErrorCategory, FieldDefinition, FieldId, FieldKind,
    FieldLifecycle, FieldValue, OptionLifecycle, TemplateArtifact, TemplateId, TemplateLifecycle,
    TemplateRevision,
};

const WARNING_COUNT_LIMIT: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocumentReconciliationErrorCategory {
    InvalidTemplate,
    InvalidDocument,
    TemplateIdMismatch,
    TemplateIsTombstoned,
    FutureDocumentRevision,
    InternalInvariant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ArtifactValidationSummary {
    category: ArtifactValidationErrorCategory,
    scalar_category: Option<ScalarValueErrorCategory>,
    scalar_location: Option<ArtifactScalarValueLocation>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    choice_location: Option<ArtifactChoiceValueLocation>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_location: Option<ArtifactRichTextValueLocation>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
}

impl ArtifactValidationSummary {
    pub(super) const fn category(self) -> ArtifactValidationErrorCategory {
        self.category
    }

    pub(super) const fn from_error(error: ArtifactValidationError) -> Self {
        Self {
            category: error.category(),
            scalar_category: error.scalar_category(),
            scalar_location: error.scalar_location(),
            choice_category: error.choice_category(),
            choice_location: error.choice_location(),
            rich_text_category: error.rich_text_category(),
            rich_text_location: error.rich_text_location(),
            rich_text_structure_location: error.rich_text_structure_location(),
        }
    }
}

impl fmt::Debug for ArtifactValidationSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactValidationSummary")
            .field("category", &self.category)
            .field("scalar_category", &self.scalar_category)
            .field("scalar_location", &self.scalar_location)
            .field("choice_category", &self.choice_category)
            .field("choice_location", &self.choice_location)
            .field("rich_text_category", &self.rich_text_category)
            .field("rich_text_location", &self.rich_text_location)
            .field(
                "rich_text_structure_location",
                &self.rich_text_structure_location,
            )
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentReconciliationError {
    category: DocumentReconciliationErrorCategory,
    validation: Option<ArtifactValidationSummary>,
    detail: &'static str,
}

impl DocumentReconciliationError {
    pub(crate) const fn category(self) -> DocumentReconciliationErrorCategory {
        self.category
    }

    pub(crate) const fn validation_category(self) -> Option<ArtifactValidationErrorCategory> {
        match self.validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    pub(crate) const fn scalar_category(self) -> Option<ScalarValueErrorCategory> {
        match self.validation {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn scalar_location(self) -> Option<ArtifactScalarValueLocation> {
        match self.validation {
            Some(summary) => summary.scalar_location,
            None => None,
        }
    }

    pub(crate) const fn choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        match self.validation {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn choice_location(self) -> Option<ArtifactChoiceValueLocation> {
        match self.validation {
            Some(summary) => summary.choice_location,
            None => None,
        }
    }

    pub(crate) const fn rich_text_category(self) -> Option<RichTextValidationErrorCategory> {
        match self.validation {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn rich_text_location(self) -> Option<ArtifactRichTextValueLocation> {
        match self.validation {
            Some(summary) => summary.rich_text_location,
            None => None,
        }
    }

    pub(crate) const fn rich_text_structure_location(self) -> Option<RichTextErrorLocation> {
        match self.validation {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    const fn new(category: DocumentReconciliationErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            validation: None,
            detail,
        }
    }

    const fn invalid_template(error: ArtifactValidationError) -> Self {
        let mut result = Self::new(
            DocumentReconciliationErrorCategory::InvalidTemplate,
            "Template failed storage admission",
        );
        result.validation = Some(ArtifactValidationSummary::from_error(error));
        result
    }

    const fn invalid_document(error: ArtifactValidationError) -> Self {
        let mut result = Self::new(
            DocumentReconciliationErrorCategory::InvalidDocument,
            "Document failed storage admission",
        );
        result.validation = Some(ArtifactValidationSummary::from_error(error));
        result
    }

    const fn template_id_mismatch() -> Self {
        Self::new(
            DocumentReconciliationErrorCategory::TemplateIdMismatch,
            "Document and Template identities do not match",
        )
    }

    const fn template_is_tombstoned() -> Self {
        Self::new(
            DocumentReconciliationErrorCategory::TemplateIsTombstoned,
            "tombstoned Template cannot produce a reconciliation view",
        )
    }

    const fn future_document_revision() -> Self {
        Self::new(
            DocumentReconciliationErrorCategory::FutureDocumentRevision,
            "Document references a future Template revision",
        )
    }

    const fn internal_invariant() -> Self {
        Self::new(
            DocumentReconciliationErrorCategory::InternalInvariant,
            "reconciliation inputs violated an admitted aggregate invariant",
        )
    }
}

impl fmt::Debug for DocumentReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentReconciliationError")
            .field("category", &self.category)
            .field("validation", &self.validation)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for DocumentReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Document reconciliation failed ({:?}): {}",
            self.category, self.detail
        )
    }
}

impl std::error::Error for DocumentReconciliationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueProvenance {
    ExistingValue,
    ExplicitUnset,
    HistoricalInitialDefault,
}

pub(crate) struct KnownFieldEntry<'a> {
    field_id: FieldId,
    definition: &'a FieldDefinition,
    value: Option<&'a FieldValue>,
    provenance: Option<ValueProvenance>,
}

impl KnownFieldEntry<'_> {
    pub(crate) const fn field_id(&self) -> FieldId {
        self.field_id
    }

    pub(crate) fn lifecycle(&self) -> FieldLifecycle {
        self.definition.lifecycle()
    }

    pub(crate) fn kind(&self) -> FieldKind {
        self.definition.kind()
    }

    pub(crate) fn label(&self) -> &str {
        self.definition.label()
    }

    pub(crate) const fn value(&self) -> Option<&FieldValue> {
        self.value
    }

    pub(crate) const fn provenance(&self) -> Option<ValueProvenance> {
        self.provenance
    }
}

impl fmt::Debug for KnownFieldEntry<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KnownFieldEntry")
            .field("field_id", &self.field_id)
            .field("lifecycle", &self.definition.lifecycle())
            .field("kind", &self.definition.kind())
            .field("provenance", &self.provenance)
            .field("has_value", &self.value.is_some())
            .field("label_redacted", &true)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrphanFieldDisposition {
    PreservedOrphan,
    SnapshotMissing,
    SnapshotRequired,
    ReattachableOrphan,
    ReattachmentBlocked,
}

pub(crate) struct OrphanFieldEntry<'a> {
    field_id: FieldId,
    value: &'a FieldValue,
    snapshot: Option<&'a OrphanedFieldDefinition>,
    current_definition: Option<&'a FieldDefinition>,
    disposition: OrphanFieldDisposition,
}

impl OrphanFieldEntry<'_> {
    pub(crate) const fn field_id(&self) -> FieldId {
        self.field_id
    }

    pub(crate) const fn value(&self) -> &FieldValue {
        self.value
    }

    pub(crate) const fn snapshot(&self) -> Option<&OrphanedFieldDefinition> {
        self.snapshot
    }

    pub(crate) const fn disposition(&self) -> OrphanFieldDisposition {
        self.disposition
    }

    pub(crate) fn display_label(&self) -> Option<&str> {
        self.current_definition
            .map(FieldDefinition::label)
            .or_else(|| self.snapshot.map(OrphanedFieldDefinition::label))
    }

    pub(crate) fn uses_id_fallback(&self) -> bool {
        self.display_label().is_none()
    }
}

impl fmt::Debug for OrphanFieldEntry<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrphanFieldEntry")
            .field("field_id", &self.field_id)
            .field("disposition", &self.disposition)
            .field("has_snapshot", &self.snapshot.is_some())
            .field("has_current_definition", &self.current_definition.is_some())
            .field("value_redacted", &true)
            .field("label_redacted", &true)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocumentReconciliationWarningCategory {
    ArchivedOptionSelected,
    ExistingNumberOutsideRange,
    ExistingRelationExceedsMultiplicity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentReconciliationWarning {
    category: DocumentReconciliationWarningCategory,
    field_id: FieldId,
    count: usize,
    truncated: bool,
}

impl DocumentReconciliationWarning {
    pub(crate) const fn category(self) -> DocumentReconciliationWarningCategory {
        self.category
    }

    pub(crate) const fn field_id(self) -> FieldId {
        self.field_id
    }

    pub(crate) const fn count(self) -> usize {
        self.count
    }

    pub(crate) const fn truncated(self) -> bool {
        self.truncated
    }

    fn archived_options(field_id: FieldId, count: usize) -> Self {
        Self {
            category: DocumentReconciliationWarningCategory::ArchivedOptionSelected,
            field_id,
            count: count.min(WARNING_COUNT_LIMIT),
            truncated: count > WARNING_COUNT_LIMIT,
        }
    }
}

impl fmt::Debug for DocumentReconciliationWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentReconciliationWarning")
            .field("category", &self.category)
            .field("field_id", &self.field_id)
            .field("count", &self.count)
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DocumentReconciliationIssueCategory {
    RequiredValueUnset,
    MissingKnownFieldValue,
    InvalidKnownFieldValue,
    UnknownSelectedOption,
    OrphanSnapshotMissing,
    ReattachmentSnapshotConflict,
    LossyOrphanReattachment,
    OptionSnapshotUnavailable,
    LossySnapshotMembership,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BoundValidationSummary {
    category: FieldValidationErrorCategory,
    location: FieldValidationLocation,
    scalar_category: Option<ScalarValueErrorCategory>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
}

impl BoundValidationSummary {
    const fn from_error(error: FieldValidationError) -> Self {
        Self {
            category: error.category(),
            location: error.location(),
            scalar_category: error.scalar_category(),
            choice_category: error.choice_category(),
            rich_text_category: error.rich_text_category(),
            rich_text_structure_location: error.rich_text_structure_location(),
        }
    }
}

impl fmt::Debug for BoundValidationSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundValidationSummary")
            .field("category", &self.category)
            .field("location", &self.location)
            .field("scalar_category", &self.scalar_category)
            .field("choice_category", &self.choice_category)
            .field("rich_text_category", &self.rich_text_category)
            .field(
                "rich_text_structure_location",
                &self.rich_text_structure_location,
            )
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentReconciliationIssue {
    category: DocumentReconciliationIssueCategory,
    field_id: FieldId,
    validation: Option<BoundValidationSummary>,
}

impl DocumentReconciliationIssue {
    pub(crate) const fn category(self) -> DocumentReconciliationIssueCategory {
        self.category
    }

    pub(crate) const fn field_id(self) -> FieldId {
        self.field_id
    }

    pub(crate) const fn validation_category(self) -> Option<FieldValidationErrorCategory> {
        match self.validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    pub(crate) const fn validation_location(self) -> Option<FieldValidationLocation> {
        match self.validation {
            Some(summary) => Some(summary.location),
            None => None,
        }
    }

    pub(crate) const fn choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        match self.validation {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn scalar_category(self) -> Option<ScalarValueErrorCategory> {
        match self.validation {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn rich_text_category(self) -> Option<RichTextValidationErrorCategory> {
        match self.validation {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn rich_text_structure_location(self) -> Option<RichTextErrorLocation> {
        match self.validation {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    pub(super) const fn plain(
        category: DocumentReconciliationIssueCategory,
        field_id: FieldId,
    ) -> Self {
        Self {
            category,
            field_id,
            validation: None,
        }
    }

    pub(super) const fn from_validation(field_id: FieldId, error: FieldValidationError) -> Self {
        let category = match (error.category(), error.choice_category()) {
            (FieldValidationErrorCategory::RequiredValueUnset, _) => {
                DocumentReconciliationIssueCategory::RequiredValueUnset
            }
            (
                FieldValidationErrorCategory::InvalidChoiceValue,
                Some(ChoiceValidationErrorCategory::UnknownSelectedOption),
            ) => DocumentReconciliationIssueCategory::UnknownSelectedOption,
            _ => DocumentReconciliationIssueCategory::InvalidKnownFieldValue,
        };
        Self {
            category,
            field_id,
            validation: Some(BoundValidationSummary::from_error(error)),
        }
    }
}

impl fmt::Debug for DocumentReconciliationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentReconciliationIssue")
            .field("category", &self.category)
            .field("field_id", &self.field_id)
            .field("validation", &self.validation)
            .finish()
    }
}

#[must_use = "reconciliation warnings and blocking issues must be inspected"]
pub(crate) struct ReconciledDocumentView<'a> {
    template_id: TemplateId,
    template_revision: TemplateRevision,
    document_template_revision: TemplateRevision,
    known_fields: Vec<KnownFieldEntry<'a>>,
    orphan_fields: Vec<OrphanFieldEntry<'a>>,
    warnings: Vec<DocumentReconciliationWarning>,
    blocking_issues: Vec<DocumentReconciliationIssue>,
    materialization_required: bool,
}

impl<'a> ReconciledDocumentView<'a> {
    pub(crate) const fn template_id(&self) -> TemplateId {
        self.template_id
    }

    pub(crate) const fn template_revision(&self) -> TemplateRevision {
        self.template_revision
    }

    pub(crate) const fn document_template_revision(&self) -> TemplateRevision {
        self.document_template_revision
    }

    pub(crate) fn known_fields(&self) -> &[KnownFieldEntry<'a>] {
        &self.known_fields
    }

    pub(crate) fn orphan_fields(&self) -> &[OrphanFieldEntry<'a>] {
        &self.orphan_fields
    }

    pub(crate) fn warnings(&self) -> &[DocumentReconciliationWarning] {
        &self.warnings
    }

    pub(crate) fn blocking_issues(&self) -> &[DocumentReconciliationIssue] {
        &self.blocking_issues
    }

    pub(crate) const fn materialization_required(&self) -> bool {
        self.materialization_required
    }

    pub(crate) fn can_materialize(&self) -> bool {
        self.blocking_issues.is_empty()
    }
}

impl fmt::Debug for ReconciledDocumentView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconciledDocumentView")
            .field("template_revision", &self.template_revision)
            .field(
                "document_template_revision",
                &self.document_template_revision,
            )
            .field("known_field_count", &self.known_fields.len())
            .field("orphan_field_count", &self.orphan_fields.len())
            .field("warning_count", &self.warnings.len())
            .field("blocking_issue_count", &self.blocking_issues.len())
            .field("materialization_required", &self.materialization_required)
            .finish()
    }
}

pub(crate) fn reconcile_document<'a>(
    template: &'a TemplateArtifact,
    document: &'a DocumentArtifact,
) -> Result<ReconciledDocumentView<'a>, DocumentReconciliationError> {
    template
        .validate_storage()
        .map_err(DocumentReconciliationError::invalid_template)?;
    document
        .validate_storage()
        .map_err(DocumentReconciliationError::invalid_document)?;
    reconcile_admitted_document(template, document)
}

/// Read-only projections preserve the last saved document values even while the
/// owning Template is in the trash. Mutation paths continue to use
/// `reconcile_document`/`reconcile_admitted_document` and therefore reject a
/// tombstoned Template.
pub(crate) fn reconcile_document_for_read<'a>(
    template: &'a TemplateArtifact,
    document: &'a DocumentArtifact,
) -> Result<ReconciledDocumentView<'a>, DocumentReconciliationError> {
    template
        .validate_storage()
        .map_err(DocumentReconciliationError::invalid_template)?;
    document
        .validate_storage()
        .map_err(DocumentReconciliationError::invalid_document)?;
    reconcile_admitted_document_with_deleted_template(template, document, true)
}

/// 이미 전체 storage admission을 마친 caller가 같은 분류 규칙을 재사용하는 private 경계다.
pub(super) fn reconcile_admitted_document<'a>(
    template: &'a TemplateArtifact,
    document: &'a DocumentArtifact,
) -> Result<ReconciledDocumentView<'a>, DocumentReconciliationError> {
    reconcile_admitted_document_with_deleted_template(template, document, false)
}

fn reconcile_admitted_document_with_deleted_template<'a>(
    template: &'a TemplateArtifact,
    document: &'a DocumentArtifact,
    allow_deleted_template: bool,
) -> Result<ReconciledDocumentView<'a>, DocumentReconciliationError> {
    if template.template_id() != document.template_id() {
        return Err(DocumentReconciliationError::template_id_mismatch());
    }
    if !allow_deleted_template && template.lifecycle() == TemplateLifecycle::Deleted {
        return Err(DocumentReconciliationError::template_is_tombstoned());
    }
    if document.template_revision() > template.revision() {
        return Err(DocumentReconciliationError::future_document_revision());
    }

    let mut known_fields = Vec::with_capacity(template.fields().len());
    let mut orphan_fields = Vec::new();
    let mut warnings = Vec::new();
    let mut blocking_issues = Vec::new();
    // M2-5d가 현재 Template revision으로 binding을 갱신해야 하는 경우도 materialization이다.
    let mut materialization_required = document.template_revision() != template.revision();

    for field_id in template.field_order() {
        let definition = template
            .fields()
            .get(field_id)
            .ok_or_else(DocumentReconciliationError::internal_invariant)?;
        classify_known_field(
            *field_id,
            definition,
            document,
            &mut known_fields,
            &mut orphan_fields,
            &mut warnings,
            &mut blocking_issues,
            &mut materialization_required,
        );
    }
    for (field_id, definition) in template
        .fields()
        .iter()
        .filter(|(_, field)| field.lifecycle() == FieldLifecycle::Archived)
    {
        classify_known_field(
            *field_id,
            definition,
            document,
            &mut known_fields,
            &mut orphan_fields,
            &mut warnings,
            &mut blocking_issues,
            &mut materialization_required,
        );
    }

    for (field_id, value) in document
        .field_values()
        .iter()
        .filter(|(field_id, _)| !template.fields().contains_key(field_id))
    {
        let snapshot = document.orphaned_field_definitions().get(field_id);
        let disposition = if snapshot.is_some() {
            OrphanFieldDisposition::PreservedOrphan
        } else {
            materialization_required = true;
            blocking_issues.push(DocumentReconciliationIssue::plain(
                DocumentReconciliationIssueCategory::OrphanSnapshotMissing,
                *field_id,
            ));
            OrphanFieldDisposition::SnapshotMissing
        };
        orphan_fields.push(OrphanFieldEntry {
            field_id: *field_id,
            value,
            snapshot,
            current_definition: None,
            disposition,
        });
    }
    // Orphan entry는 active/archived/unknown 세 경로에서 모이므로 표시용 fieldOrder가 아니라
    // identity의 안정 순서로 한 번만 확정한다.
    orphan_fields.sort_by_key(|entry| entry.field_id);

    Ok(ReconciledDocumentView {
        template_id: template.template_id(),
        template_revision: template.revision(),
        document_template_revision: document.template_revision(),
        known_fields,
        orphan_fields,
        warnings,
        blocking_issues,
        materialization_required,
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_known_field<'a>(
    field_id: FieldId,
    definition: &'a FieldDefinition,
    document: &'a DocumentArtifact,
    known_fields: &mut Vec<KnownFieldEntry<'a>>,
    orphan_fields: &mut Vec<OrphanFieldEntry<'a>>,
    warnings: &mut Vec<DocumentReconciliationWarning>,
    blocking_issues: &mut Vec<DocumentReconciliationIssue>,
    materialization_required: &mut bool,
) {
    let existing = document.field_values().get(&field_id);
    let snapshot = document.orphaned_field_definitions().get(&field_id);
    let mut existing_bound_valid = false;
    let (value, provenance) = match existing {
        Some(value) => {
            let provenance = if value.is_unset() {
                ValueProvenance::ExplicitUnset
            } else {
                ValueProvenance::ExistingValue
            };
            match definition
                .validate_document_value(value, BoundDocumentValueContext::ExistingDocumentValue)
            {
                Ok(outcome) => {
                    existing_bound_valid = true;
                    record_validation_outcome(field_id, outcome, warnings);
                }
                Err(error) => {
                    *materialization_required = true;
                    blocking_issues.push(DocumentReconciliationIssue::from_validation(
                        field_id, error,
                    ));
                }
            }
            (Some(value), Some(provenance))
        }
        None if document.template_revision() < definition.introduced_revision() => {
            *materialization_required = true;
            let value = definition.initial_default_value();
            if definition.lifecycle() == FieldLifecycle::Active
                && definition.required()
                && value.is_unset()
            {
                blocking_issues.push(DocumentReconciliationIssue::plain(
                    DocumentReconciliationIssueCategory::RequiredValueUnset,
                    field_id,
                ));
            }
            record_historical_archived_options(field_id, definition, value, warnings);
            (Some(value), Some(ValueProvenance::HistoricalInitialDefault))
        }
        None => {
            *materialization_required = true;
            blocking_issues.push(DocumentReconciliationIssue::plain(
                DocumentReconciliationIssueCategory::MissingKnownFieldValue,
                field_id,
            ));
            (None, None)
        }
    };

    known_fields.push(KnownFieldEntry {
        field_id,
        definition,
        value,
        provenance,
    });

    if let Some(value) = value {
        let disposition = match assess_snapshot(Some(definition), value, snapshot) {
            Ok(SnapshotAction::Preserve) if snapshot.is_none() => return,
            Ok(SnapshotAction::Preserve) => OrphanFieldDisposition::PreservedOrphan,
            Ok(SnapshotAction::Create) => {
                *materialization_required = true;
                OrphanFieldDisposition::SnapshotRequired
            }
            Ok(SnapshotAction::Remove) => {
                *materialization_required = true;
                if existing_bound_valid {
                    OrphanFieldDisposition::ReattachableOrphan
                } else {
                    OrphanFieldDisposition::ReattachmentBlocked
                }
            }
            Err(category) => {
                *materialization_required = true;
                blocking_issues.push(DocumentReconciliationIssue::plain(category, field_id));
                OrphanFieldDisposition::ReattachmentBlocked
            }
        };
        orphan_fields.push(OrphanFieldEntry {
            field_id,
            value,
            snapshot,
            current_definition: Some(definition),
            disposition,
        });
    }
}

fn record_validation_outcome(
    field_id: FieldId,
    outcome: FieldValidationOutcome,
    warnings: &mut Vec<DocumentReconciliationWarning>,
) {
    if let Some(diagnostic) = outcome.into_diagnostic() {
        match diagnostic.category() {
            FieldDiagnosticCategory::ExistingNumberOutsideRange => {
                warnings.push(DocumentReconciliationWarning {
                    category: DocumentReconciliationWarningCategory::ExistingNumberOutsideRange,
                    field_id,
                    count: 1,
                    truncated: false,
                })
            }
            FieldDiagnosticCategory::ExistingSelectionContainsArchivedOption => {
                warnings.push(DocumentReconciliationWarning::archived_options(
                    field_id,
                    diagnostic.archived_option_count(),
                ));
            }
            FieldDiagnosticCategory::ExistingRelationExceedsMultiplicity => {
                warnings.push(DocumentReconciliationWarning {
                    category:
                        DocumentReconciliationWarningCategory::ExistingRelationExceedsMultiplicity,
                    field_id,
                    count: 1,
                    truncated: false,
                })
            }
        }
    }
}

fn record_historical_archived_options(
    field_id: FieldId,
    definition: &FieldDefinition,
    value: &FieldValue,
    warnings: &mut Vec<DocumentReconciliationWarning>,
) {
    let Some(options) = definition.configuration().options() else {
        return;
    };
    let archived_count = match definition.kind() {
        FieldKind::SingleChoice => value
            .single_choice()
            .and_then(|option_id| options.get(&option_id))
            .filter(|option| option.lifecycle() == OptionLifecycle::Archived)
            .map_or(0, |_| 1),
        FieldKind::MultiChoice => value.multi_choice().map_or(0, |selected| {
            selected
                .iter()
                .filter(|option_id| {
                    options
                        .get(option_id)
                        .is_some_and(|option| option.lifecycle() == OptionLifecycle::Archived)
                })
                .count()
        }),
        _ => 0,
    };
    if archived_count > 0 {
        warnings.push(DocumentReconciliationWarning::archived_options(
            field_id,
            archived_count,
        ));
    }
}

#[cfg(test)]
mod tests;
