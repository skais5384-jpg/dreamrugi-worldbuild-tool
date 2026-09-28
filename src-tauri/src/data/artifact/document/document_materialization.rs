use std::fmt;
use std::ops::Index;

use crate::data::{
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
        validation::{FieldValidationErrorCategory, FieldValidationLocation},
    },
    utc_time::is_utc_milliseconds,
};

use super::{
    document_reconciliation::{
        reconcile_admitted_document, DocumentReconciliationError,
        DocumentReconciliationErrorCategory, DocumentReconciliationIssue,
        DocumentReconciliationIssueCategory, DocumentReconciliationWarning,
        DocumentReconciliationWarningCategory, ValueProvenance,
    },
    document_snapshot::apply_snapshot_policy,
    DocumentArtifact,
};
use crate::data::artifact::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactValidationError, ArtifactValidationErrorCategory, FieldId, FieldKind, FieldLifecycle,
    FieldValue, TemplateArtifact, TemplateRevision,
};
use std::collections::BTreeMap;

const ISSUE_COUNT_LIMIT: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentMaterializationErrorCategory {
    InvalidTemplate,
    InvalidDocument,
    RevisionMismatch,
    InvalidTimestamp,
    TimestampRegression,
    TemplateIdMismatch,
    TemplateIsTombstoned,
    FutureDocumentRevision,
    BlockingIssues,
    InvalidCandidate,
    InternalInvariant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ArtifactValidationSummary {
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
    const fn from_error(error: ArtifactValidationError) -> Self {
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
struct IssueSummary {
    category: DocumentReconciliationIssueCategory,
    field_id: FieldId,
    validation_category: Option<FieldValidationErrorCategory>,
    validation_location: Option<FieldValidationLocation>,
    scalar_category: Option<ScalarValueErrorCategory>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
}

impl IssueSummary {
    fn from_issue(issue: &DocumentReconciliationIssue) -> Self {
        Self {
            category: issue.category(),
            field_id: issue.field_id(),
            validation_category: issue.validation_category(),
            validation_location: issue.validation_location(),
            scalar_category: issue.scalar_category(),
            choice_category: issue.choice_category(),
            rich_text_category: issue.rich_text_category(),
            rich_text_structure_location: issue.rich_text_structure_location(),
        }
    }
}

impl fmt::Debug for IssueSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssueSummary")
            .field("category", &self.category)
            .field("field_id", &self.field_id)
            .field("validation_category", &self.validation_category)
            .field("validation_location", &self.validation_location)
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

pub(crate) struct DocumentMaterializationError {
    category: DocumentMaterializationErrorCategory,
    artifact_validation: Option<ArtifactValidationSummary>,
    issue: Option<IssueSummary>,
    issue_count: usize,
    issues_truncated: bool,
    detail: &'static str,
}

impl DocumentMaterializationError {
    pub(crate) const fn category(&self) -> DocumentMaterializationErrorCategory {
        self.category
    }

    pub(crate) const fn field_id(&self) -> Option<FieldId> {
        match self.issue {
            Some(issue) => Some(issue.field_id),
            None => None,
        }
    }

    pub(crate) const fn issue_category(&self) -> Option<DocumentReconciliationIssueCategory> {
        match self.issue {
            Some(issue) => Some(issue.category),
            None => None,
        }
    }

    pub(crate) const fn issue_count(&self) -> usize {
        self.issue_count
    }

    pub(crate) const fn issues_truncated(&self) -> bool {
        self.issues_truncated
    }

    pub(crate) const fn artifact_validation_category(
        &self,
    ) -> Option<ArtifactValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    pub(crate) const fn artifact_scalar_category(&self) -> Option<ScalarValueErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_scalar_location(&self) -> Option<ArtifactScalarValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.scalar_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_choice_category(&self) -> Option<ChoiceValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_choice_location(&self) -> Option<ArtifactChoiceValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.choice_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_category(
        &self,
    ) -> Option<RichTextValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_location(
        &self,
    ) -> Option<ArtifactRichTextValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_structure_location(
        &self,
    ) -> Option<RichTextErrorLocation> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    pub(crate) const fn issue_validation_category(&self) -> Option<FieldValidationErrorCategory> {
        match self.issue {
            Some(summary) => summary.validation_category,
            None => None,
        }
    }

    pub(crate) const fn issue_validation_location(&self) -> Option<FieldValidationLocation> {
        match self.issue {
            Some(summary) => summary.validation_location,
            None => None,
        }
    }

    pub(crate) const fn issue_scalar_category(&self) -> Option<ScalarValueErrorCategory> {
        match self.issue {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn issue_choice_category(&self) -> Option<ChoiceValidationErrorCategory> {
        match self.issue {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn issue_rich_text_category(&self) -> Option<RichTextValidationErrorCategory> {
        match self.issue {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn issue_rich_text_structure_location(&self) -> Option<RichTextErrorLocation> {
        match self.issue {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    const fn plain(category: DocumentMaterializationErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            artifact_validation: None,
            issue: None,
            issue_count: 0,
            issues_truncated: false,
            detail,
        }
    }

    const fn invalid_artifact(
        category: DocumentMaterializationErrorCategory,
        detail: &'static str,
        error: ArtifactValidationError,
    ) -> Self {
        let mut result = Self::plain(category, detail);
        result.artifact_validation = Some(ArtifactValidationSummary::from_error(error));
        result
    }

    fn blocking_issues(issues: &[DocumentReconciliationIssue]) -> Self {
        let mut result = Self::plain(
            DocumentMaterializationErrorCategory::BlockingIssues,
            "Document reconciliation has unresolved blocking issues",
        );
        result.issue = issues.first().map(IssueSummary::from_issue);
        result.issue_count = issues.len().min(ISSUE_COUNT_LIMIT);
        result.issues_truncated = issues.len() > ISSUE_COUNT_LIMIT;
        result
    }

    fn from_admitted_reconciliation(error: DocumentReconciliationError) -> Self {
        let (category, detail) = match error.category() {
            DocumentReconciliationErrorCategory::TemplateIdMismatch => (
                DocumentMaterializationErrorCategory::TemplateIdMismatch,
                "Document and Template identities do not match",
            ),
            DocumentReconciliationErrorCategory::TemplateIsTombstoned => (
                DocumentMaterializationErrorCategory::TemplateIsTombstoned,
                "tombstoned Template cannot materialize a Document",
            ),
            DocumentReconciliationErrorCategory::FutureDocumentRevision => (
                DocumentMaterializationErrorCategory::FutureDocumentRevision,
                "Document references a future Template revision",
            ),
            _ => (
                DocumentMaterializationErrorCategory::InternalInvariant,
                "admitted reconciliation failed unexpectedly",
            ),
        };
        Self::plain(category, detail)
    }

    const fn internal_invariant() -> Self {
        Self::plain(
            DocumentMaterializationErrorCategory::InternalInvariant,
            "materialization violated an admitted aggregate invariant",
        )
    }
}

impl fmt::Debug for DocumentMaterializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentMaterializationError")
            .field("category", &self.category)
            .field("artifact_validation", &self.artifact_validation)
            .field("issue", &self.issue)
            .field("issue_count", &self.issue_count)
            .field("issues_truncated", &self.issues_truncated)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for DocumentMaterializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Document materialization failed ({:?}): {}",
            self.category, self.detail
        )?;
        if let Some(issue) = self.issue {
            write!(formatter, " (FieldId {})", issue.field_id)?;
        }
        Ok(())
    }
}

impl std::error::Error for DocumentMaterializationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentMaterializationOutcomeKind {
    Unchanged,
    Changed,
}

#[must_use = "materialization warnings must be inspected"]
#[derive(PartialEq, Eq)]
pub(crate) struct DocumentMaterializationWarning {
    category: DocumentReconciliationWarningCategory,
    field_id: FieldId,
    count: usize,
    truncated: bool,
}

impl DocumentMaterializationWarning {
    fn from_reconciliation(warning: &DocumentReconciliationWarning) -> Self {
        Self {
            category: warning.category(),
            field_id: warning.field_id(),
            count: warning.count(),
            truncated: warning.truncated(),
        }
    }

    pub(crate) const fn category(&self) -> DocumentReconciliationWarningCategory {
        self.category
    }

    pub(crate) const fn field_id(&self) -> FieldId {
        self.field_id
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }

    pub(crate) const fn truncated(&self) -> bool {
        self.truncated
    }
}

impl fmt::Debug for DocumentMaterializationWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentMaterializationWarning")
            .field("category", &self.category)
            .field("field_id", &self.field_id)
            .field("count", &self.count)
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// 성공 warning을 일반 `Vec`으로 축소해 무심코 버리지 못하게 하는 owned collection이다.
#[must_use = "all materialization warnings must be inspected"]
#[derive(PartialEq, Eq)]
pub(crate) struct DocumentMaterializationWarnings {
    warnings: Vec<DocumentMaterializationWarning>,
}

impl DocumentMaterializationWarnings {
    #[must_use = "the borrowed materialization warnings must be inspected"]
    pub(crate) fn as_slice(&self) -> &[DocumentMaterializationWarning] {
        &self.warnings
    }

    #[must_use = "the materialization warning count must be inspected"]
    pub(crate) fn len(&self) -> usize {
        self.warnings.len()
    }

    #[must_use = "whether materialization has warnings must be inspected"]
    pub(crate) fn is_empty(&self) -> bool {
        self.warnings.is_empty()
    }
}

impl Index<usize> for DocumentMaterializationWarnings {
    type Output = DocumentMaterializationWarning;

    fn index(&self, index: usize) -> &Self::Output {
        &self.warnings[index]
    }
}

impl fmt::Debug for DocumentMaterializationWarnings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentMaterializationWarnings")
            .field("count", &self.warnings.len())
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// Changed candidate와 warning collection의 소유권을 분리하지 않는 consuming 결과다.
#[must_use = "the changed Document and all materialization warnings must be inspected"]
pub(crate) struct ChangedDocumentMaterialization {
    document: DocumentArtifact,
    warnings: DocumentMaterializationWarnings,
}

impl ChangedDocumentMaterialization {
    #[must_use = "the changed Document borrow must be used"]
    pub(crate) fn document(&self) -> &DocumentArtifact {
        &self.document
    }

    #[must_use = "the changed materialization warnings must be inspected"]
    pub(crate) fn warnings(&self) -> &DocumentMaterializationWarnings {
        &self.warnings
    }
}

impl fmt::Debug for ChangedDocumentMaterialization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChangedDocumentMaterialization")
            .field("warnings", &self.warnings)
            .field("document_redacted", &true)
            .finish()
    }
}

enum MaterializationState {
    Unchanged,
    Changed(DocumentArtifact),
}

#[must_use = "materialization state and warnings must be inspected"]
pub(crate) struct DocumentMaterializationOutcome {
    state: MaterializationState,
    warnings: DocumentMaterializationWarnings,
}

impl DocumentMaterializationOutcome {
    pub(crate) const fn kind(&self) -> DocumentMaterializationOutcomeKind {
        match self.state {
            MaterializationState::Unchanged => DocumentMaterializationOutcomeKind::Unchanged,
            MaterializationState::Changed(_) => DocumentMaterializationOutcomeKind::Changed,
        }
    }

    pub(crate) fn document(&self) -> Option<&DocumentArtifact> {
        match &self.state {
            MaterializationState::Unchanged => None,
            MaterializationState::Changed(document) => Some(document),
        }
    }

    #[must_use = "the materialization warnings must be inspected"]
    pub(crate) fn warnings(&self) -> &DocumentMaterializationWarnings {
        &self.warnings
    }

    #[must_use = "the changed Document and all materialization warnings must be inspected"]
    pub(crate) fn into_changed(
        self,
    ) -> Result<ChangedDocumentMaterialization, DocumentMaterializationWarnings> {
        match self.state {
            MaterializationState::Unchanged => Err(self.warnings),
            MaterializationState::Changed(document) => Ok(ChangedDocumentMaterialization {
                document,
                warnings: self.warnings,
            }),
        }
    }
}

impl fmt::Debug for DocumentMaterializationOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentMaterializationOutcome")
            .field("kind", &self.kind())
            .field("warning_count", &self.warnings.len())
            .field("warnings", &self.warnings)
            .field("document_redacted", &true)
            .finish()
    }
}

pub(super) fn materialization_warnings(
    warnings: &[DocumentReconciliationWarning],
) -> DocumentMaterializationWarnings {
    DocumentMaterializationWarnings {
        warnings: warnings
            .iter()
            .map(DocumentMaterializationWarning::from_reconciliation)
            .collect(),
    }
}

/// 검증된 두 aggregate에서 저장 전 private Document candidate만 계산한다.
pub(crate) fn materialize_document(
    template: &TemplateArtifact,
    expected_template_revision: TemplateRevision,
    document: &DocumentArtifact,
    updated_at_utc: String,
) -> Result<DocumentMaterializationOutcome, DocumentMaterializationError> {
    let prepared = prepare_materialization(
        template,
        expected_template_revision,
        document,
        updated_at_utc,
    )?;
    match prepared.candidate {
        Some(candidate) => finalize_materialization(template, candidate, prepared.warnings),
        None => Ok(DocumentMaterializationOutcome {
            state: MaterializationState::Unchanged,
            warnings: prepared.warnings,
        }),
    }
}

/// 현재 revision에서 선택 입력인 반복 그룹의 key 자체가 누락된 경우에만 canonical
/// 빈 목록을 만든다. 필수/스칼라/과거 revision 누락은 값을 추론하지 않는다.
pub(crate) fn repair_missing_optional_group_values(
    template: &TemplateArtifact,
    expected_template_revision: TemplateRevision,
    document: &DocumentArtifact,
    updated_at_utc: String,
) -> Result<DocumentMaterializationOutcome, DocumentMaterializationError> {
    template.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidTemplate,
            "source Template failed storage admission",
            error,
        )
    })?;
    document.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidDocument,
            "source Document failed storage admission",
            error,
        )
    })?;
    if template.revision() != expected_template_revision
        || document.template_revision() != template.revision()
    {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::RevisionMismatch,
            "automatic empty-group repair requires the current Template revision",
        ));
    }
    if !is_utc_milliseconds(&updated_at_utc) {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::InvalidTimestamp,
            "updated timestamp must use canonical UTC milliseconds",
        ));
    }
    if updated_at_utc.as_str() < document.updated_at_utc() {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::TimestampRegression,
            "updated timestamp cannot precede the source Document timestamp",
        ));
    }
    let view = reconcile_admitted_document(template, document)
        .map_err(DocumentMaterializationError::from_admitted_reconciliation)?;
    let repair_ids = view
        .blocking_issues()
        .iter()
        .filter(|issue| {
            issue.category() == DocumentReconciliationIssueCategory::MissingKnownFieldValue
        })
        .filter_map(|issue| {
            template.fields().get(&issue.field_id()).and_then(|field| {
                (field.lifecycle() == FieldLifecycle::Active
                    && !field.required()
                    && field.kind() == FieldKind::Group
                    && field.introduced_revision() <= document.template_revision())
                .then_some(issue.field_id())
            })
        })
        .collect::<Vec<_>>();
    let warnings = materialization_warnings(view.warnings());
    if repair_ids.is_empty() {
        return Ok(DocumentMaterializationOutcome {
            state: MaterializationState::Unchanged,
            warnings,
        });
    }
    let mut candidate = document.clone();
    for field_id in &repair_ids {
        if candidate
            .field_values
            .insert(
                *field_id,
                FieldValue::from_group(crate::data::artifact::group::GroupValue {
                    order: vec![],
                    instances: BTreeMap::new(),
                }),
            )
            .is_some()
        {
            return Err(DocumentMaterializationError::internal_invariant());
        }
    }
    candidate.updated_at_utc = updated_at_utc;
    candidate.rebase_lossless_source().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidCandidate,
            "repaired Document failed lossless provenance admission",
            error,
        )
    })?;
    candidate.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidCandidate,
            "repaired Document failed storage admission",
            error,
        )
    })?;
    let final_view = reconcile_admitted_document(template, &candidate)
        .map_err(DocumentMaterializationError::from_admitted_reconciliation)?;
    if final_view.blocking_issues().iter().any(|issue| {
        repair_ids.contains(&issue.field_id())
            && issue.category() == DocumentReconciliationIssueCategory::MissingKnownFieldValue
    }) {
        return Err(DocumentMaterializationError::internal_invariant());
    }
    let warnings = materialization_warnings(final_view.warnings());
    Ok(DocumentMaterializationOutcome {
        state: MaterializationState::Changed(candidate),
        warnings,
    })
}

struct PreparedMaterialization {
    candidate: Option<DocumentArtifact>,
    warnings: DocumentMaterializationWarnings,
}

fn prepare_materialization(
    template: &TemplateArtifact,
    expected_template_revision: TemplateRevision,
    document: &DocumentArtifact,
    updated_at_utc: String,
) -> Result<PreparedMaterialization, DocumentMaterializationError> {
    template.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidTemplate,
            "source Template failed storage admission",
            error,
        )
    })?;
    document.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidDocument,
            "source Document failed storage admission",
            error,
        )
    })?;
    if template.revision() != expected_template_revision {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::RevisionMismatch,
            "expected Template revision does not match the source snapshot",
        ));
    }
    if !is_utc_milliseconds(&updated_at_utc) {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::InvalidTimestamp,
            "updated timestamp must use canonical UTC milliseconds",
        ));
    }
    if updated_at_utc.as_str() < document.updated_at_utc() {
        return Err(DocumentMaterializationError::plain(
            DocumentMaterializationErrorCategory::TimestampRegression,
            "updated timestamp cannot precede the source Document timestamp",
        ));
    }

    let view = reconcile_admitted_document(template, document)
        .map_err(DocumentMaterializationError::from_admitted_reconciliation)?;
    if !view.can_materialize() {
        return Err(DocumentMaterializationError::blocking_issues(
            view.blocking_issues(),
        ));
    }
    let warnings = materialization_warnings(view.warnings());
    if !view.materialization_required() {
        return Ok(PreparedMaterialization {
            candidate: None,
            warnings,
        });
    }

    // Source admission과 blocker 판정이 끝난 뒤 정확히 한 번만 owned candidate를 만든다.
    let mut candidate = document.clone();
    let mut historical_field_ids = Vec::new();
    for entry in view.known_fields() {
        if entry.provenance() != Some(ValueProvenance::HistoricalInitialDefault) {
            continue;
        }
        let value = entry
            .value()
            .ok_or_else(DocumentMaterializationError::internal_invariant)?;
        if candidate
            .field_values
            .insert(entry.field_id(), value.clone())
            .is_some()
        {
            return Err(DocumentMaterializationError::internal_invariant());
        }
        historical_field_ids.push(entry.field_id());
    }
    apply_snapshot_policy(template, &mut candidate).map_err(|(field_id, category)| {
        DocumentMaterializationError::blocking_issues(&[DocumentReconciliationIssue::plain(
            category, field_id,
        )])
    })?;

    let structural_change = candidate != *document;
    let revision_change = document.template_revision != template.revision();
    if !structural_change && !revision_change {
        return Err(DocumentMaterializationError::internal_invariant());
    }
    candidate.template_revision = template.revision();
    candidate.updated_at_utc = updated_at_utc;
    // 기존 Document provenance는 검증된 구조 변경에 맞춰 재기준화하고, 새 historical 값은
    // Template initialDefaultValue subtree만 정확한 FieldValue 소유 위치로 전달한다.
    candidate.rebase_lossless_source().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidCandidate,
            "final Document candidate failed lossless provenance admission",
            error,
        )
    })?;
    for field_id in historical_field_ids {
        let Some(provenance) = template.initial_default_provenance(field_id) else {
            continue;
        };
        // 의미가 다르면 stale provenance를 붙이지 않고 현재 canonical 값을 유지한다.
        candidate.graft_initial_default_provenance(provenance);
    }
    Ok(PreparedMaterialization {
        candidate: Some(candidate),
        warnings,
    })
}

fn finalize_materialization(
    template: &TemplateArtifact,
    candidate: DocumentArtifact,
    expected_warnings: DocumentMaterializationWarnings,
) -> Result<DocumentMaterializationOutcome, DocumentMaterializationError> {
    candidate.validate_storage().map_err(|error| {
        DocumentMaterializationError::invalid_artifact(
            DocumentMaterializationErrorCategory::InvalidCandidate,
            "final Document candidate failed storage admission",
            error,
        )
    })?;
    let final_view = reconcile_admitted_document(template, &candidate)
        .map_err(DocumentMaterializationError::from_admitted_reconciliation)?;
    if !final_view.can_materialize() {
        return Err(DocumentMaterializationError::blocking_issues(
            final_view.blocking_issues(),
        ));
    }
    if final_view.materialization_required() {
        return Err(DocumentMaterializationError::internal_invariant());
    }
    let final_warnings = materialization_warnings(final_view.warnings());
    if final_warnings != expected_warnings {
        return Err(DocumentMaterializationError::internal_invariant());
    }
    Ok(DocumentMaterializationOutcome {
        state: MaterializationState::Changed(candidate),
        warnings: final_warnings,
    })
}

/// Test code가 public mutation surface를 만들지 않고 production finalization을 직접 통과시킨다.
#[cfg(test)]
enum FinalCandidateCorruption {
    RemoveKnownField(FieldId),
    ReplaceWithUnset(FieldId),
    ReplaceWithNumber(FieldId, String),
    ReplaceWithSingleChoice(FieldId, crate::data::artifact::OptionId),
    NonCanonicalMultiChoice(FieldId, Vec<crate::data::artifact::OptionId>),
    SemanticEmptyRichText(FieldId),
    WholeWireDepth(serde_json::Value),
    ReservedRootKey(String),
    Timestamp(String),
}

#[cfg(test)]
fn materialize_document_with_final_corruption_for_test(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
    updated_at_utc: String,
    corruption: FinalCandidateCorruption,
) -> Result<DocumentMaterializationOutcome, DocumentMaterializationError> {
    let prepared =
        prepare_materialization(template, template.revision(), document, updated_at_utc)?;
    let mut candidate = prepared
        .candidate
        .ok_or_else(DocumentMaterializationError::internal_invariant)?;
    match corruption {
        FinalCandidateCorruption::RemoveKnownField(field_id) => {
            candidate.field_values.remove(&field_id);
        }
        FinalCandidateCorruption::ReplaceWithUnset(field_id) => {
            candidate
                .field_values
                .insert(field_id, super::FieldValue::unset());
        }
        FinalCandidateCorruption::ReplaceWithNumber(field_id, value) => {
            candidate
                .field_values
                .insert(field_id, super::FieldValue::number(value));
        }
        FinalCandidateCorruption::ReplaceWithSingleChoice(field_id, option_id) => {
            candidate
                .field_values
                .insert(field_id, super::FieldValue::from_single_choice(option_id));
        }
        FinalCandidateCorruption::NonCanonicalMultiChoice(field_id, option_ids) => {
            candidate.corrupt_multi_choice_value_for_test(field_id, option_ids);
        }
        FinalCandidateCorruption::SemanticEmptyRichText(field_id) => {
            candidate.replace_rich_text_content_for_test(
                field_id,
                serde_json::json!({"kind":"root","children":[]})
                    .as_object()
                    .expect("test corruption must be an object")
                    .clone(),
            );
        }
        FinalCandidateCorruption::WholeWireDepth(value) => {
            candidate.corrupt_extra_depth_for_test(value);
        }
        FinalCandidateCorruption::ReservedRootKey(key) => {
            candidate.corrupt_extra_for_test(&key, serde_json::Value::Bool(true));
        }
        FinalCandidateCorruption::Timestamp(timestamp) => {
            candidate.updated_at_utc = timestamp;
        }
    }
    finalize_materialization(template, candidate, prepared.warnings)
}

#[cfg(test)]
mod tests;
