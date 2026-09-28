use std::{collections::BTreeMap, fmt};

use crate::data::{
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
        validation::{
            BoundDocumentValueContext, FieldValidationError, FieldValidationErrorCategory,
            FieldValidationLocation, FieldValidationOutcome,
        },
    },
    utc_time::is_utc_milliseconds,
};

use super::{
    document_reconciliation::DocumentReconciliationIssueCategory,
    document_snapshot::apply_snapshot_policy, DocumentArtifact, FieldValue,
};
use crate::data::artifact::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactType, ArtifactValidationError, ArtifactValidationErrorCategory, DocumentId, FieldId,
    FieldLifecycle, TemplateArtifact, TemplateLifecycle, TemplateRevision, DOCUMENT_SCHEMA_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentCreationErrorCategory {
    InvalidSource,
    RevisionMismatch,
    InvalidTimestamp,
    TemplateIsTombstoned,
    RequiredValueUnset,
    InvalidBoundValue,
    InvalidCandidate,
    SnapshotBlocked,
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
pub(crate) struct DocumentCreationError {
    category: DocumentCreationErrorCategory,
    field_id: Option<FieldId>,
    artifact_validation: Option<ArtifactValidationSummary>,
    bound_validation: Option<BoundValidationSummary>,
    snapshot_issue: Option<DocumentReconciliationIssueCategory>,
    detail: &'static str,
}

impl DocumentCreationError {
    pub(crate) const fn category(self) -> DocumentCreationErrorCategory {
        self.category
    }

    pub(crate) const fn field_id(self) -> Option<FieldId> {
        self.field_id
    }

    pub(crate) const fn snapshot_issue(self) -> Option<DocumentReconciliationIssueCategory> {
        self.snapshot_issue
    }

    pub(crate) const fn artifact_validation_category(
        self,
    ) -> Option<ArtifactValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    pub(crate) const fn artifact_scalar_category(self) -> Option<ScalarValueErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_scalar_location(self) -> Option<ArtifactScalarValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.scalar_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_choice_location(self) -> Option<ArtifactChoiceValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.choice_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_category(
        self,
    ) -> Option<RichTextValidationErrorCategory> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_location(self) -> Option<ArtifactRichTextValueLocation> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_location,
            None => None,
        }
    }

    pub(crate) const fn artifact_rich_text_structure_location(
        self,
    ) -> Option<RichTextErrorLocation> {
        match self.artifact_validation {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    pub(crate) const fn bound_validation_category(self) -> Option<FieldValidationErrorCategory> {
        match self.bound_validation {
            Some(summary) => Some(summary.category),
            None => None,
        }
    }

    pub(crate) const fn bound_validation_location(self) -> Option<FieldValidationLocation> {
        match self.bound_validation {
            Some(summary) => Some(summary.location),
            None => None,
        }
    }

    pub(crate) const fn bound_scalar_category(self) -> Option<ScalarValueErrorCategory> {
        match self.bound_validation {
            Some(summary) => summary.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn bound_choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        match self.bound_validation {
            Some(summary) => summary.choice_category,
            None => None,
        }
    }

    pub(crate) const fn bound_rich_text_category(self) -> Option<RichTextValidationErrorCategory> {
        match self.bound_validation {
            Some(summary) => summary.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn bound_rich_text_structure_location(self) -> Option<RichTextErrorLocation> {
        match self.bound_validation {
            Some(summary) => summary.rich_text_structure_location,
            None => None,
        }
    }

    const fn new(category: DocumentCreationErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            field_id: None,
            artifact_validation: None,
            bound_validation: None,
            snapshot_issue: None,
            detail,
        }
    }

    const fn invalid_source(error: ArtifactValidationError) -> Self {
        let mut result = Self::new(
            DocumentCreationErrorCategory::InvalidSource,
            "source Template failed storage admission",
        );
        result.artifact_validation = Some(ArtifactValidationSummary::from_error(error));
        result
    }

    const fn revision_mismatch() -> Self {
        Self::new(
            DocumentCreationErrorCategory::RevisionMismatch,
            "expected Template revision does not match the source snapshot",
        )
    }

    const fn invalid_timestamp() -> Self {
        Self::new(
            DocumentCreationErrorCategory::InvalidTimestamp,
            "Document creation timestamp must use canonical UTC milliseconds",
        )
    }

    const fn template_is_tombstoned() -> Self {
        Self::new(
            DocumentCreationErrorCategory::TemplateIsTombstoned,
            "tombstoned Template cannot create a new Document",
        )
    }

    const fn invalid_bound_value(field_id: FieldId, error: FieldValidationError) -> Self {
        let category = if matches!(
            error.category(),
            FieldValidationErrorCategory::RequiredValueUnset
        ) {
            DocumentCreationErrorCategory::RequiredValueUnset
        } else {
            DocumentCreationErrorCategory::InvalidBoundValue
        };
        let mut result = Self::new(category, "materialized Field value failed bound validation");
        result.field_id = Some(field_id);
        result.bound_validation = Some(BoundValidationSummary::from_error(error));
        result
    }

    const fn unexpected_bound_outcome(field_id: FieldId) -> Self {
        let mut result = Self::new(
            DocumentCreationErrorCategory::InvalidBoundValue,
            "new Document value produced an unsupported diagnostic outcome",
        );
        result.field_id = Some(field_id);
        result
    }

    const fn non_canonical_archived_value(field_id: FieldId) -> Self {
        let mut result = Self::new(
            DocumentCreationErrorCategory::InvalidBoundValue,
            "new Document archived Field value must be canonical unset",
        );
        result.field_id = Some(field_id);
        result
    }

    const fn invalid_candidate(error: ArtifactValidationError) -> Self {
        let mut result = Self::new(
            DocumentCreationErrorCategory::InvalidCandidate,
            "final Document candidate failed storage admission",
        );
        result.artifact_validation = Some(ArtifactValidationSummary::from_error(error));
        result
    }
}

impl fmt::Debug for DocumentCreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentCreationError")
            .field("category", &self.category)
            .field("field_id", &self.field_id)
            .field("artifact_validation", &self.artifact_validation)
            .field("bound_validation", &self.bound_validation)
            .field("snapshot_issue", &self.snapshot_issue)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for DocumentCreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Document creation failed ({:?}): {}",
            self.category, self.detail
        )?;
        if let Some(field_id) = self.field_id {
            write!(formatter, " (FieldId {field_id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for DocumentCreationError {}

#[must_use]
pub(crate) struct DocumentCreationOutcome {
    document: DocumentArtifact,
}

impl DocumentCreationOutcome {
    pub(crate) const fn document(&self) -> &DocumentArtifact {
        &self.document
    }

    pub(crate) fn into_document(self) -> DocumentArtifact {
        self.document
    }
}

impl fmt::Debug for DocumentCreationOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentCreationOutcome")
            .field("document", &self.document)
            .finish()
    }
}

/// 새 문서는 기존 기본값과 가이드를 복사하지 않는다. 필수값은 최종 생성에서 검증한다.
pub(crate) fn create_document(
    source: &TemplateArtifact,
    expected_revision: TemplateRevision,
    document_id: DocumentId,
    name: String,
    timestamp_utc: String,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    create_document_with_values(
        source,
        expected_revision,
        document_id,
        name,
        false,
        timestamp_utc,
        &BTreeMap::new(),
    )
}

/// 필수값이 없는 기본 후보는 공개하지 않는다. typed 초기값을 붙인 뒤 생성 검증을 한 번 수행한다.
pub(crate) fn create_document_with_values(
    source: &TemplateArtifact,
    expected_revision: TemplateRevision,
    document_id: DocumentId,
    name: String,
    glossary_excluded: bool,
    timestamp_utc: String,
    initial: &BTreeMap<FieldId, FieldValue>,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    create_document_with_term_info(
        source,
        expected_revision,
        document_id,
        name,
        String::new(),
        String::new(),
        glossary_excluded,
        timestamp_utc,
        initial,
    )
}

pub(crate) fn create_document_with_term_info(
    source: &TemplateArtifact,
    expected_revision: TemplateRevision,
    document_id: DocumentId,
    name: String,
    english_name: String,
    glossary_summary: String,
    glossary_excluded: bool,
    timestamp_utc: String,
    initial: &BTreeMap<FieldId, FieldValue>,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    source
        .validate_storage()
        .map_err(DocumentCreationError::invalid_source)?;

    if source.revision() != expected_revision {
        return Err(DocumentCreationError::revision_mismatch());
    }
    if !is_utc_milliseconds(&timestamp_utc) {
        return Err(DocumentCreationError::invalid_timestamp());
    }
    if source.lifecycle() == TemplateLifecycle::Deleted {
        return Err(DocumentCreationError::template_is_tombstoned());
    }

    let field_values = source
        .fields()
        .iter()
        .map(|(field_id, _)| (*field_id, FieldValue::unset()))
        .collect();

    let mut candidate = DocumentArtifact {
        schema_version: DOCUMENT_SCHEMA_VERSION,
        artifact_type: ArtifactType::Document,
        document_id,
        template_id: source.template_id(),
        template_revision: source.revision(),
        name,
        english_name,
        glossary_summary,
        glossary_excluded,
        field_values,
        orphaned_field_definitions: BTreeMap::new(),
        created_at_utc: timestamp_utc.clone(),
        updated_at_utc: timestamp_utc,
        extra: BTreeMap::new(),
        lossless_source: Default::default(),
    };

    // Template의 이력은 Template에 남긴다. 사용자가 입력한 값만 새 문서에 들어간다.
    candidate
        .rebase_lossless_source()
        .map_err(DocumentCreationError::invalid_candidate)?;
    for (id, value) in initial {
        let field = source
            .fields()
            .get(id)
            .ok_or_else(|| DocumentCreationError::unexpected_bound_outcome(*id))?;
        if field.lifecycle() != FieldLifecycle::Active {
            return Err(DocumentCreationError::non_canonical_archived_value(*id));
        }
        candidate.field_values.insert(*id, value.clone());
    }
    finalize_document_creation(source, candidate)
}

/// 생성 전용 불변식과 공통 Field/storage 검증을 모두 통과한 candidate만 outcome으로 닫는다.
fn finalize_document_creation(
    source: &TemplateArtifact,
    mut candidate: DocumentArtifact,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    let canonical_unset = FieldValue::unset();
    for (field_id, field) in source.fields() {
        let Some(value) = candidate.field_values.get(field_id) else {
            return Err(DocumentCreationError::unexpected_bound_outcome(*field_id));
        };
        // ExistingDocumentValue는 archived 과거 값을 보존하기 위한 규칙이므로 non-unset도 허용한다.
        // 신규 Document에서는 required/default와 무관하게 extra 없는 canonical unset만 허용한다.
        if field.lifecycle() == FieldLifecycle::Archived && value != &canonical_unset {
            return Err(DocumentCreationError::non_canonical_archived_value(
                *field_id,
            ));
        }
        let context = match field.lifecycle() {
            FieldLifecycle::Active => BoundDocumentValueContext::NewDocumentValue,
            // Field Engine의 NewDocumentValue context는 archived Field를 의도적으로 거부한다.
            // 신규 문서 정책의 explicit unset은 archived existing-value 규칙으로 required를 소급하지
            // 않으면서 Field rule과 결합해 검증한다.
            FieldLifecycle::Archived => BoundDocumentValueContext::ExistingDocumentValue,
        };
        let validation = field
            .validate_document_value(value, context)
            .map_err(|error| DocumentCreationError::invalid_bound_value(*field_id, error))?;
        if !matches!(validation, FieldValidationOutcome::Valid) {
            return Err(DocumentCreationError::unexpected_bound_outcome(*field_id));
        }
    }
    apply_snapshot_policy(source, &mut candidate).map_err(|(field_id, issue)| {
        let mut error = DocumentCreationError::new(
            DocumentCreationErrorCategory::SnapshotBlocked,
            "Document snapshot cannot be preserved; review the Template definitions",
        );
        error.field_id = Some(field_id);
        error.snapshot_issue = Some(issue);
        error
    })?;
    candidate
        .validate_storage()
        .map_err(DocumentCreationError::invalid_candidate)?;
    // snapshot 추가 뒤에도 복사한 current default의 원 숫자 token을 같은 위치에 유지한다.
    candidate
        .rebase_lossless_source()
        .map_err(DocumentCreationError::invalid_candidate)?;
    Ok(DocumentCreationOutcome {
        document: candidate,
    })
}

/// 테스트가 일반 caller 입력을 넓히지 않고 실제 production finalization 경계를 검증한다.
#[cfg(test)]
fn create_document_with_field_value_override_for_test(
    source: &TemplateArtifact,
    document_id: DocumentId,
    field_id: FieldId,
    value: FieldValue,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    let mut candidate = create_document(
        source,
        source.revision(),
        document_id,
        "test-only candidate override".to_owned(),
        "2026-09-05T12:34:56.789Z".to_owned(),
    )?
    .into_document();
    candidate.field_values.insert(field_id, value);
    finalize_document_creation(source, candidate)
}

#[cfg(test)]
mod tests;
