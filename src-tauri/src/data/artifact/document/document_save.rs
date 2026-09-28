use std::{collections::BTreeSet, fmt};

use super::{
    document_materialization::{materialization_warnings, DocumentMaterializationWarnings},
    document_reconciliation::{
        reconcile_admitted_document, ArtifactValidationSummary, DocumentReconciliationError,
        DocumentReconciliationErrorCategory, DocumentReconciliationIssue,
        DocumentReconciliationIssueCategory,
    },
    document_snapshot::{apply_snapshot_policy, selected_options, update_snapshot_membership},
    DocumentArtifact, DOCUMENT_SCHEMA_VERSION,
};
use crate::data::{
    artifact::{
        ArtifactValidationError, ArtifactValidationErrorCategory, FieldId, FieldLifecycle,
        FieldValue, OptionLifecycle, TemplateArtifact, TemplateLifecycle, TemplateRevision,
    },
    field_engine::validation::{BoundDocumentValueContext, FieldValidationError},
    utc_time::is_utc_milliseconds,
};

mod edits;
pub(crate) use edits::{DocumentEdit, DocumentEditSet, DocumentValueEdit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentSaveErrorCategory {
    InvalidTemplate,
    InvalidDocument,
    TemplateIdMismatch,
    TemplateIsTombstoned,
    FutureDocumentRevision,
    RevisionMismatch,
    InvalidTimestamp,
    TimestampRegression,
    DuplicateNameEdit,
    DuplicateFieldEdit,
    UnknownField,
    ArchivedField,
    MissingKnownFieldValue,
    InvalidEditValue,
    UnknownOption,
    ArchivedOptionAdded,
    UnknownEditMetadata,
    LossyValueEdit,
    SnapshotBlocked,
    BlockingIssues,
    InvalidCandidate,
    IncompleteCandidate,
    InternalInvariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentSaveStage {
    SourceAdmission,
    Preconditions,
    HistoricalDefaults,
    Edits,
    Snapshots,
    FinalStorage,
    FinalReconciliation,
    Provenance,
}

/// 닫힌 진단 요약만 소유한다. source/candidate/edit나 하위 오류의 원문 chain은 넣지 않는다.
#[derive(Debug)]
pub(crate) struct DocumentSaveError {
    category: DocumentSaveErrorCategory,
    stage: DocumentSaveStage,
    field_id: Option<FieldId>,
    storage_validation: Option<ArtifactValidationSummary>,
    issue: Option<DocumentReconciliationIssue>,
    issue_count: usize,
    issues_truncated: bool,
}

impl DocumentSaveError {
    pub(crate) const fn category(&self) -> DocumentSaveErrorCategory {
        self.category
    }

    pub(crate) const fn stage(&self) -> DocumentSaveStage {
        self.stage
    }

    pub(crate) const fn field_id(&self) -> Option<FieldId> {
        self.field_id
    }

    pub(crate) const fn storage_validation_category(
        &self,
    ) -> Option<ArtifactValidationErrorCategory> {
        match self.storage_validation {
            Some(summary) => Some(summary.category()),
            None => None,
        }
    }

    pub(crate) const fn issue(&self) -> Option<DocumentReconciliationIssue> {
        self.issue
    }

    pub(crate) const fn issue_count(&self) -> usize {
        self.issue_count
    }

    pub(crate) const fn issues_truncated(&self) -> bool {
        self.issues_truncated
    }

    fn plain(category: DocumentSaveErrorCategory, stage: DocumentSaveStage) -> Self {
        Self {
            category,
            stage,
            field_id: None,
            storage_validation: None,
            issue: None,
            issue_count: 0,
            issues_truncated: false,
        }
    }

    fn field(
        category: DocumentSaveErrorCategory,
        stage: DocumentSaveStage,
        field_id: FieldId,
    ) -> Self {
        let mut error = Self::plain(category, stage);
        error.field_id = Some(field_id);
        error
    }

    fn storage(
        category: DocumentSaveErrorCategory,
        stage: DocumentSaveStage,
        cause: ArtifactValidationError,
    ) -> Self {
        let mut error = Self::plain(category, stage);
        error.storage_validation = Some(ArtifactValidationSummary::from_error(cause));
        error
    }

    fn validation(field_id: FieldId, cause: FieldValidationError) -> Self {
        let mut error = Self::field(
            DocumentSaveErrorCategory::InvalidEditValue,
            DocumentSaveStage::Edits,
            field_id,
        );
        error.issue = Some(DocumentReconciliationIssue::from_validation(
            field_id, cause,
        ));
        error.issue_count = 1;
        error
    }

    fn snapshot((field_id, category): (FieldId, DocumentReconciliationIssueCategory)) -> Self {
        let mut error = Self::field(
            DocumentSaveErrorCategory::SnapshotBlocked,
            DocumentSaveStage::Snapshots,
            field_id,
        );
        error.issue = Some(DocumentReconciliationIssue::plain(category, field_id));
        error.issue_count = 1;
        error
    }

    fn reconciliation(cause: DocumentReconciliationError) -> Self {
        let category = match cause.category() {
            DocumentReconciliationErrorCategory::TemplateIdMismatch => {
                DocumentSaveErrorCategory::TemplateIdMismatch
            }
            DocumentReconciliationErrorCategory::TemplateIsTombstoned => {
                DocumentSaveErrorCategory::TemplateIsTombstoned
            }
            DocumentReconciliationErrorCategory::FutureDocumentRevision => {
                DocumentSaveErrorCategory::FutureDocumentRevision
            }
            _ => DocumentSaveErrorCategory::InternalInvariant,
        };
        Self::plain(category, DocumentSaveStage::FinalReconciliation)
    }
}

impl fmt::Display for DocumentSaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Document save preparation failed ({:?} at {:?}); retain edits and review the inputs before retrying",
            self.category, self.stage
        )
    }
}

impl std::error::Error for DocumentSaveError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentSaveOutcomeKind {
    Changed,
    Unchanged,
}

/// 메모리 candidate와 최종 warning을 함께 소유하며 디스크 저장/권한/내구성을 주장하지 않는다.
#[must_use = "inspect the candidate state and all final warnings"]
pub(crate) struct DocumentSaveOutcome {
    kind: DocumentSaveOutcomeKind,
    document: DocumentArtifact,
    warnings: DocumentMaterializationWarnings,
}

impl DocumentSaveOutcome {
    pub(crate) const fn kind(&self) -> DocumentSaveOutcomeKind {
        self.kind
    }

    pub(crate) fn document(&self) -> &DocumentArtifact {
        &self.document
    }

    #[must_use = "all final Document warnings must be inspected"]
    pub(crate) fn warnings(&self) -> &DocumentMaterializationWarnings {
        &self.warnings
    }
}

impl fmt::Debug for DocumentSaveOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentSaveOutcome")
            .field("kind", &self.kind)
            .field("warnings", &self.warnings)
            .field("document_redacted", &true)
            .finish()
    }
}

/// 편집 시작 revision은 전달된 메모리 Template와만 비교한다. 실제 source/session/동시성은
/// 후속 application gate가 검증해야 하며 이 순수 함수는 source token을 발급하지 않는다.
pub(crate) fn prepare_document_save(
    template: &TemplateArtifact,
    expected_template_revision: TemplateRevision,
    document: &DocumentArtifact,
    edits: &DocumentEditSet,
    updated_at_utc: &str,
) -> Result<DocumentSaveOutcome, DocumentSaveError> {
    admit_sources(template, document)?;
    check_preconditions(
        template,
        expected_template_revision,
        document,
        edits,
        updated_at_utc,
    )?;
    let mut candidate = historical_candidate(template, document)?;
    crate::data::artifact::group_provenance::relocate(template, &mut candidate, &edits.edits)
        .map_err(|cause| {
            DocumentSaveError::storage(
                DocumentSaveErrorCategory::InvalidCandidate,
                DocumentSaveStage::Provenance,
                cause,
            )
        })?;
    apply_edits(template, &mut candidate, edits)?;
    // 구 문서를 읽어 관계명을 처음 붙이는 순간에는 그 값을 표현할 수 있는 현재
    // Document schema로 함께 승격한다. 읽기 호환을 위해 무명 v5 자체는 그대로 허용한다.
    if candidate.schema_version.get() < DOCUMENT_SCHEMA_VERSION.get()
        && candidate
            .field_values
            .values()
            .any(FieldValue::has_named_relation)
    {
        candidate.schema_version = DOCUMENT_SCHEMA_VERSION;
    }
    apply_snapshot_policy(template, &mut candidate).map_err(DocumentSaveError::snapshot)?;
    candidate.template_revision = template.revision();
    // timestamp를 바꾸기 전에 실제 name/value/snapshot/binding 변화만 비교한다.
    let kind = if candidate == *document {
        DocumentSaveOutcomeKind::Unchanged
    } else {
        candidate.updated_at_utc = updated_at_utc.to_owned();
        DocumentSaveOutcomeKind::Changed
    };
    finalize_save(template, candidate, kind)
}

/// 결합 저장은 원 Template의 future binding/current missing을 명령으로 가리면 안 된다.
/// 전체 bound validation이나 materialization을 요구하지 않아 historical required edit는 허용한다.
pub(crate) fn check_document_save_original(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
) -> Result<(), DocumentSaveError> {
    admit_sources(template, document)?;
    check_current_missing(template, document)
}

fn admit_sources(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
) -> Result<(), DocumentSaveError> {
    use DocumentSaveErrorCategory as Category;
    let stage = DocumentSaveStage::SourceAdmission;
    template
        .validate_storage()
        .map_err(|cause| DocumentSaveError::storage(Category::InvalidTemplate, stage, cause))?;
    document
        .validate_storage()
        .map_err(|cause| DocumentSaveError::storage(Category::InvalidDocument, stage, cause))?;
    let category = if template.template_id() != document.template_id() {
        Some(Category::TemplateIdMismatch)
    } else if template.lifecycle() == TemplateLifecycle::Deleted {
        Some(Category::TemplateIsTombstoned)
    } else if document.template_revision() > template.revision() {
        Some(Category::FutureDocumentRevision)
    } else {
        None
    };
    match category {
        Some(category) => Err(DocumentSaveError::plain(category, stage)),
        None => Ok(()),
    }
}

fn check_preconditions(
    template: &TemplateArtifact,
    expected_template_revision: TemplateRevision,
    document: &DocumentArtifact,
    edits: &DocumentEditSet,
    updated_at_utc: &str,
) -> Result<(), DocumentSaveError> {
    use DocumentSaveErrorCategory as Category;
    let stage = DocumentSaveStage::Preconditions;
    if expected_template_revision != template.revision() {
        return Err(DocumentSaveError::plain(Category::RevisionMismatch, stage));
    }
    if !is_utc_milliseconds(updated_at_utc) {
        return Err(DocumentSaveError::plain(Category::InvalidTimestamp, stage));
    }
    if updated_at_utc < document.updated_at_utc() {
        return Err(DocumentSaveError::plain(
            Category::TimestampRegression,
            stage,
        ));
    }
    let mut renamed = false;
    let mut english_name = false;
    let mut glossary_summary = false;
    let mut fields = BTreeSet::new();
    for edit in &edits.edits {
        let field_id = match edit {
            DocumentEdit::Rename(_) => {
                if renamed {
                    return Err(DocumentSaveError::plain(Category::DuplicateNameEdit, stage));
                }
                renamed = true;
                continue;
            }
            DocumentEdit::SetEnglishName(_) => {
                if english_name {
                    return Err(DocumentSaveError::plain(Category::DuplicateNameEdit, stage));
                }
                english_name = true;
                continue;
            }
            DocumentEdit::SetGlossarySummary(_) => {
                if glossary_summary {
                    return Err(DocumentSaveError::plain(Category::DuplicateNameEdit, stage));
                }
                glossary_summary = true;
                continue;
            }
            DocumentEdit::SetGlossaryExcluded(_) => continue,
            DocumentEdit::SetValue(field_id, _)
            | DocumentEdit::SetGroup(field_id, _)
            | DocumentEdit::Unset(field_id) => *field_id,
        };
        if !fields.insert(field_id) {
            return Err(DocumentSaveError::field(
                Category::DuplicateFieldEdit,
                stage,
                field_id,
            ));
        }
        let field = template
            .fields()
            .get(&field_id)
            .ok_or_else(|| DocumentSaveError::field(Category::UnknownField, stage, field_id))?;
        if field.lifecycle() != FieldLifecycle::Active {
            return Err(DocumentSaveError::field(
                Category::ArchivedField,
                stage,
                field_id,
            ));
        }
    }
    check_current_missing(template, document)
}

fn check_current_missing(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
) -> Result<(), DocumentSaveError> {
    // 편집이 key를 채울 수 있어도 current missing은 손상이다. 원래 binding으로만 판정한다.
    for (field_id, field) in template.fields() {
        if !document.field_values().contains_key(field_id)
            && document.template_revision() >= field.introduced_revision()
        {
            return Err(DocumentSaveError::field(
                DocumentSaveErrorCategory::MissingKnownFieldValue,
                DocumentSaveStage::Preconditions,
                *field_id,
            ));
        }
    }
    Ok(())
}

fn historical_candidate(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
) -> Result<DocumentArtifact, DocumentSaveError> {
    let mut candidate = document.clone();
    let mut historical = Vec::new();
    for (field_id, field) in template.fields() {
        if !document.field_values().contains_key(field_id)
            && document.template_revision() < field.introduced_revision()
        {
            candidate
                .field_values
                .insert(*field_id, field.initial_default_value().clone());
            historical.push(*field_id);
        }
    }
    if !historical.is_empty() {
        candidate.rebase_lossless_source().map_err(|cause| {
            DocumentSaveError::storage(
                DocumentSaveErrorCategory::InvalidCandidate,
                DocumentSaveStage::HistoricalDefaults,
                cause,
            )
        })?;
        // 편집 전 실제로 복사한 initial provenance를 붙인다. 뒤에서 payload가 바뀌어도
        // 같은 envelope에 보존한 unknown extra의 원 숫자 표기는 이 소유 위치에 남는다.
        for field_id in historical {
            if let Some(provenance) = template.initial_default_provenance(field_id) {
                candidate.graft_initial_default_provenance(provenance);
            }
        }
    }
    Ok(candidate)
}

fn apply_edits(
    template: &TemplateArtifact,
    candidate: &mut DocumentArtifact,
    edits: &DocumentEditSet,
) -> Result<(), DocumentSaveError> {
    use DocumentSaveErrorCategory as Category;
    let stage = DocumentSaveStage::Edits;
    for edit in &edits.edits {
        let (field_id, replacement) = match edit {
            DocumentEdit::Rename(name) => {
                candidate.name.clone_from(name);
                continue;
            }
            DocumentEdit::SetEnglishName(value) => {
                candidate.english_name.clone_from(value);
                continue;
            }
            DocumentEdit::SetGlossarySummary(value) => {
                candidate.glossary_summary.clone_from(value);
                continue;
            }
            DocumentEdit::SetGlossaryExcluded(excluded) => {
                candidate.glossary_excluded = *excluded;
                continue;
            }
            DocumentEdit::SetGroup(field_id, drafts) => {
                let definition = template.fields().get(field_id).ok_or_else(|| {
                    DocumentSaveError::field(Category::InternalInvariant, stage, *field_id)
                })?;
                let value = crate::data::artifact::group::assemble(
                    definition,
                    template.revision(),
                    candidate.field_values.get(field_id),
                    drafts,
                )
                .map_err(|_| {
                    DocumentSaveError::field(Category::LossyValueEdit, stage, *field_id)
                })?;
                // assemble가 native 원문과 각 keep/set의 소속·unknown 보존을 검증했다.
                let value = if let Some(old) = candidate.field_values.get(field_id) {
                    value.preserve_outer_storage_extra_from(old)
                } else {
                    value
                };
                candidate.field_values.insert(*field_id, value);
                continue;
            }
            DocumentEdit::SetValue(field_id, value) => (*field_id, value.value.clone()),
            DocumentEdit::Unset(field_id) => (*field_id, FieldValue::unset()),
        };
        let source = candidate.field_values.get(&field_id).ok_or_else(|| {
            DocumentSaveError::field(Category::InternalInvariant, stage, field_id)
        })?;
        let field = template.fields().get(&field_id).ok_or_else(|| {
            DocumentSaveError::field(Category::InternalInvariant, stage, field_id)
        })?;
        if replacement.contains_unknown_storage_data() {
            return Err(DocumentSaveError::field(
                Category::UnknownEditMetadata,
                stage,
                field_id,
            ));
        }
        let replacement = replacement.preserve_outer_storage_extra_from(source);
        if !source.has_same_storage_extras(&replacement) {
            return Err(DocumentSaveError::field(
                Category::LossyValueEdit,
                stage,
                field_id,
            ));
        }
        // 기존 archived 선택은 유지 가능하지만 추가된 ID에는 별도 active 조건을 강제한다.
        // ExistingDocumentValue context 자체를 완화하거나 historical 선택을 새 입력으로 위장하지 않는다.
        let previous = selected_options(source);
        let selected = selected_options(&replacement);
        if let Some(options) = field.configuration().options() {
            for id in selected.difference(&previous) {
                let option = options.get(id).ok_or_else(|| {
                    DocumentSaveError::field(Category::UnknownOption, stage, field_id)
                })?;
                if option.lifecycle() != OptionLifecycle::Active {
                    return Err(DocumentSaveError::field(
                        Category::ArchivedOptionAdded,
                        stage,
                        field_id,
                    ));
                }
            }
        }
        let validation = field
            .validate_document_value(
                &replacement,
                BoundDocumentValueContext::ExistingDocumentValue,
            )
            .map_err(|cause| DocumentSaveError::validation(field_id, cause))?;
        // 읽기에서 허용한 과거 범위 초과를 새 Set 입력의 허용으로 오해하지 않는다.
        if validation.diagnostic().is_some_and(|d| d.category()==crate::data::field_engine::validation::FieldDiagnosticCategory::ExistingNumberOutsideRange) {
            let checked = field.validate_document_value(&replacement, BoundDocumentValueContext::NewDocumentValue)
                .map_err(|cause| DocumentSaveError::validation(field_id, cause))?;
            let _diagnostic = checked.into_diagnostic();
        }
        // 중간 warning은 결과에 옮기지 않고 모든 편집 후 final reconciliation에서 다시 계산한다.
        let _diagnostic = validation.into_diagnostic();
        if let Some(snapshot) = candidate.orphaned_field_definitions.get(&field_id) {
            let snapshot = update_snapshot_membership(field, &replacement, snapshot)
                .map_err(|issue| DocumentSaveError::snapshot((field_id, issue)))?;
            candidate
                .orphaned_field_definitions
                .insert(field_id, snapshot);
        }
        candidate.field_values.insert(field_id, replacement);
    }
    Ok(())
}

fn finalize_save(
    template: &TemplateArtifact,
    mut candidate: DocumentArtifact,
    kind: DocumentSaveOutcomeKind,
) -> Result<DocumentSaveOutcome, DocumentSaveError> {
    candidate.validate_storage().map_err(|cause| {
        DocumentSaveError::storage(
            DocumentSaveErrorCategory::InvalidCandidate,
            DocumentSaveStage::FinalStorage,
            cause,
        )
    })?;
    let view = reconcile_admitted_document(template, &candidate)
        .map_err(DocumentSaveError::reconciliation)?;
    if !view.can_materialize() {
        let mut error = DocumentSaveError::plain(
            DocumentSaveErrorCategory::BlockingIssues,
            DocumentSaveStage::FinalReconciliation,
        );
        error.issue = view.blocking_issues().first().copied();
        error.field_id = error.issue.map(|issue| issue.field_id());
        error.issue_count = view.blocking_issues().len().min(1_024);
        error.issues_truncated = view.blocking_issues().len() > 1_024;
        return Err(error);
    }
    if view.materialization_required() {
        return Err(DocumentSaveError::plain(
            DocumentSaveErrorCategory::IncompleteCandidate,
            DocumentSaveStage::FinalReconciliation,
        ));
    }
    let warnings = materialization_warnings(view.warnings());
    candidate.rebase_lossless_source().map_err(|cause| {
        DocumentSaveError::storage(
            DocumentSaveErrorCategory::InvalidCandidate,
            DocumentSaveStage::Provenance,
            cause,
        )
    })?;
    Ok(DocumentSaveOutcome {
        kind,
        document: candidate,
        warnings,
    })
}

#[cfg(test)]
mod tests;
