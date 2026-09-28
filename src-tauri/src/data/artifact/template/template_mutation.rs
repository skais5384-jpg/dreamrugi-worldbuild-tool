use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use super::super::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactValidationError, ArtifactValidationErrorCategory, DocumentArtifact, DocumentId,
    FieldId, FieldKind, FieldValue, OptionId, ReferenceId, RelationLink, RichTextDocument,
    TemplateId, TemplateRevision,
};
use super::{
    ChoiceOption, DefaultDraftSnapshot, FieldConfiguration, FieldConfigurationVariant,
    FieldDefinition, FieldLifecycle, OptionLifecycle, Presentation, TemplateArtifact,
    TemplateLifecycle,
};
use crate::data::{
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{NormalizedRichText, RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
    },
    utc_time::is_utc_milliseconds,
};

pub(crate) mod whole;

/// caller가 aggregate나 wire DTO를 직접 조립하지 못하게 하는 닫힌 mutation command다.
pub(crate) struct TemplateMutationCommand {
    operation: TemplateMutationOperation,
}

enum TemplateMutationOperation {
    SetTemplateName {
        name: String,
    },
    SetTemplatePresentationToken {
        token: Option<String>,
    },
    SetGlossaryExcluded {
        excluded: bool,
    },
    TombstoneTemplate {
        assessment: TemplateReferenceAssessment,
    },
    CreateField {
        draft: NewFieldDraft,
        insertion: NewFieldInsertion,
    },
    SetFieldLabel {
        field_id: FieldId,
        label: String,
    },
    SetFieldRequired {
        field_id: FieldId,
        required: bool,
    },
    SetFieldPresentationToken {
        field_id: FieldId,
        token: Option<String>,
    },
    SetCurrentDefault {
        field_id: FieldId,
        value: FieldValueDraft,
    },
    ReorderFields {
        field_order: Vec<FieldId>,
    },
    ArchiveField {
        field_id: FieldId,
    },
    AddOption {
        field_id: FieldId,
        draft: NewChoiceOptionDraft,
        insertion: NewOptionInsertion,
    },
    RenameOption {
        field_id: FieldId,
        option_id: OptionId,
        label: String,
    },
    ReorderOptions {
        field_id: FieldId,
        option_order: Vec<OptionId>,
    },
    ArchiveOption {
        field_id: FieldId,
        option_id: OptionId,
        current_default_repair: Option<FieldValueDraft>,
    },
    #[cfg(test)]
    Test(TestMutation),
}

/// 오류에 노출할 수 있는 참조 수는 고정 상한에서 멈춘다. 실제 차단 판정은 상한 이후에도 유지된다.
const MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES: u16 = 1_024;

/// 기존 pure scanner의 닫힌 snapshot이다. slice constructor는 계속 test 전용이다.
/// G3 production은 본문 collection 대신 실제 완료 scan을 받는 별도 bridge를 사용한다.
#[must_use = "a complete Document snapshot must be scanned before it is discarded"]
pub(crate) struct CompleteDocumentSnapshot<'documents> {
    documents: &'documents [DocumentArtifact],
}

impl fmt::Debug for CompleteDocumentSnapshot<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompleteDocumentSnapshot")
            .field("documents_redacted", &true)
            .finish()
    }
}

impl<'documents> CompleteDocumentSnapshot<'documents> {
    /// 실제 authority가 없는 M2-4c에서는 production caller가 임의 collection을 완전 scan으로
    /// 승격하지 못하게 하고, 동일 module의 scanner 검증에만 test seam을 둔다.
    #[cfg(test)]
    fn from_complete_enumeration_for_test(documents: &'documents [DocumentArtifact]) -> Self {
        Self { documents }
    }
}

/// 참조가 없다는 assessment는 raw Document나 ID 목록을 보존하지 않고 target/revision만 가진다.
/// Clone/Copy를 구현하지 않아 한 tombstone 시도에서 소유권을 소비한다.
#[must_use = "Template reference assessment must be consumed by a tombstone command"]
#[derive(PartialEq, Eq)]
pub(crate) struct TemplateReferenceAssessment {
    template_id: TemplateId,
    source_revision: TemplateRevision,
}

impl TemplateReferenceAssessment {
    pub(crate) const fn template_id(&self) -> TemplateId {
        self.template_id
    }

    pub(crate) const fn source_revision(&self) -> TemplateRevision {
        self.source_revision
    }
}

impl fmt::Debug for TemplateReferenceAssessment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TemplateReferenceAssessment")
            .field("template_id", &self.template_id)
            .field("source_revision", &self.source_revision)
            .field("documents_redacted", &true)
            .finish()
    }
}

/// 완전 snapshot의 모든 Document를 순회하며 `templateId` 일치만 참조로 판정한다.
/// fieldValues, orphan snapshot, document 내용과 tree placement는 판정에 영향을 주지 않는다.
pub(crate) fn assess_template_references(
    source: &TemplateArtifact,
    snapshot: CompleteDocumentSnapshot<'_>,
) -> Result<TemplateReferenceAssessment, TemplateMutationError> {
    // pure snapshot은 전체 storage admission을 유지한다. G3에서는 이 검사가 실제
    // 원본 bytes의 decode 경계에서 완료된 뒤에만 최소 record를 보관한다.
    for document in snapshot.documents {
        validate_document_snapshot_entry(document)
            .map_err(TemplateMutationError::invalid_document_snapshot)?;
    }

    assess_admitted_reference_ids(
        source.template_id,
        source.revision,
        snapshot.documents.iter().map(DocumentArtifact::template_id),
    )
}

/// caller slice/count나 raw Template를 받지 않는다. target 원본과 검증된 모든 참조 record가
/// 결합된 repository 완료 결과만 소비하므로 무관한 증표를 바꿔 끼울 수 없다.
pub(crate) fn assess_repository_template_references(
    scan: crate::data::repository::CompleteTemplateReferenceScan,
) -> Result<TemplateReferenceAssessment, TemplateMutationError> {
    assess_admitted_reference_ids(
        scan.source().artifact().template_id(),
        scan.source().artifact().revision(),
        scan.template_ids(),
    )
}

fn assess_admitted_reference_ids(
    template_id: TemplateId,
    source_revision: TemplateRevision,
    document_template_ids: impl Iterator<Item = TemplateId>,
) -> Result<TemplateReferenceAssessment, TemplateMutationError> {
    let mut reference_count = 0_u16;
    let mut reference_count_truncated = false;
    for document_template_id in document_template_ids {
        if document_template_id != template_id {
            continue;
        }
        if reference_count < MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES {
            reference_count += 1;
        } else {
            reference_count_truncated = true;
        }
    }
    if reference_count != 0 {
        return Err(TemplateMutationError::template_has_documents(
            template_id,
            reference_count,
            reference_count_truncated,
        ));
    }
    Ok(TemplateReferenceAssessment {
        template_id,
        source_revision,
    })
}

/// Document codec과 같은 aggregate 및 전체-wire storage admission을 proof 경계에서 재사용한다.
fn validate_document_snapshot_entry(
    document: &DocumentArtifact,
) -> Result<(), ArtifactValidationError> {
    document.validate_storage()
}

/// 새 Choice Option은 ID와 label만 받으며 lifecycle과 unknown extra는 engine이 결정한다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NewChoiceOptionDraft {
    option_id: OptionId,
    label: String,
    lifecycle: OptionLifecycle,
}

impl NewChoiceOptionDraft {
    pub(crate) fn new(option_id: OptionId, label: String) -> Self {
        Self {
            option_id,
            label,
            lifecycle: OptionLifecycle::Active,
        }
    }

    #[cfg(test)]
    fn archived_for_test(option_id: OptionId, label: String) -> Self {
        Self {
            option_id,
            label,
            lifecycle: OptionLifecycle::Archived,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum NewFieldConfigurationVariant {
    Group,
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
        options: Vec<NewChoiceOptionDraft>,
    },
    MultiChoice {
        option_order: Vec<OptionId>,
        options: Vec<NewChoiceOptionDraft>,
    },
    Relation {
        multiple: bool,
        allowed_templates: Vec<TemplateId>,
        reciprocal_notice: bool,
    },
    DocumentLink,
}

/// Field configuration draft는 v1의 알려진 member만 표현하며 raw wire나 extra map을 받지 않는다.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NewFieldConfiguration {
    variant: NewFieldConfigurationVariant,
}

/// command payload는 raw wire 대신 닫힌 FieldValue variant만 만들 수 있다.
/// 문자열의 canonical 의미와 Choice membership은 source에 결합된 mutation pipeline에서 검사한다.
#[derive(Clone, PartialEq)]
pub(crate) struct FieldValueDraft {
    value: FieldValue,
    origin: FieldValueDraftOrigin,
}

#[derive(Clone, PartialEq)]
enum FieldValueDraftOrigin {
    Fresh,
    SourceOwned(Box<SourceOwnedCurrentDefault>),
    ProvenanceFree,
}

/// Template이 직접 발급한 current default만 담는다. bare FieldValue로는 생성할 수 없다.
#[derive(Clone, PartialEq)]
struct SourceOwnedCurrentDefault {
    snapshot: DefaultDraftSnapshot,
    template_id: TemplateId,
    field_id: FieldId,
    provenance: crate::data::json::LosslessJsonValue,
}

impl FieldValueDraft {
    pub(crate) fn unset() -> Self {
        Self::fresh(FieldValue::unset())
    }

    pub(crate) fn single_line_text(value: String) -> Self {
        Self::fresh(FieldValue::single_line_text(value))
    }

    /// Editor JSON은 Field Engine에서 먼저 검증·정규화되어야 하며 raw AST는 이 경계를 넘지 않는다.
    pub(crate) fn from_normalized_rich_text(value: NormalizedRichText) -> Self {
        match value {
            NormalizedRichText::Unset => Self::unset(),
            NormalizedRichText::RichText(canonical) => Self::fresh(FieldValue::from_rich_text(
                RichTextDocument::from_canonical(canonical),
            )),
        }
    }

    pub(crate) fn number_unknown() -> Self {
        Self::fresh(FieldValue::number_unknown())
    }

    pub(crate) fn number(value: String) -> Self {
        Self::fresh(FieldValue::number(value))
    }

    pub(crate) fn date(value: String) -> Self {
        Self::fresh(FieldValue::date(value))
    }

    pub(crate) fn image(value: Vec<String>) -> Self {
        Self::fresh(FieldValue::image(value))
    }

    pub(crate) fn file(value: Vec<String>) -> Self {
        Self::fresh(FieldValue::file(value))
    }

    pub(crate) fn url(value: String) -> Self {
        Self::fresh(FieldValue::url(value))
    }

    pub(crate) fn time(value: String) -> Self {
        Self::fresh(FieldValue::time(value))
    }

    pub(crate) fn duration(milliseconds: String) -> Self {
        Self::fresh(FieldValue::duration(milliseconds))
    }

    pub(crate) fn single_choice(option_id: OptionId) -> Self {
        Self::fresh(FieldValue::from_single_choice(option_id))
    }

    /// 입력 배열을 정렬하거나 비운 뒤 unset으로 바꾸지 않고 validator에 그대로 전달한다.
    pub(crate) fn multi_choice(option_ids: Vec<OptionId>) -> Self {
        Self::fresh(FieldValue::from_multi_choice(option_ids))
    }

    pub(crate) fn relation(links: Vec<(ReferenceId, DocumentId, bool, String)>) -> Self {
        Self::fresh(FieldValue::relation(
            links
                .into_iter()
                .map(|(id, document, one_way, name)| {
                    RelationLink::named(id, document, one_way, name)
                })
                .collect(),
        ))
    }

    pub(crate) fn document_link(documents: Vec<DocumentId>) -> Self {
        Self::fresh(FieldValue::document_link(documents))
    }

    /// 값만 받은 요청에는 ownership이 없다. 현재 M2의 default 교체/repair에서는 명시적으로 거부한다.
    /// 새 Field의 초기값에는 기존 목적지가 없으며 기존 unknown metadata 금지 검사를 계속 적용한다.
    pub(crate) fn provenance_free_replacement(value: FieldValue) -> Self {
        Self {
            value,
            origin: FieldValueDraftOrigin::ProvenanceFree,
        }
    }

    fn into_value(self) -> FieldValue {
        self.value
    }

    fn into_parts(self) -> (FieldValue, FieldValueDraftOrigin) {
        (self.value, self.origin)
    }

    fn fresh(value: FieldValue) -> Self {
        Self {
            value,
            origin: FieldValueDraftOrigin::Fresh,
        }
    }
}

impl TemplateArtifact {
    /// 현재 snapshot의 정확한 Field에서 value와 lossless provenance를 함께 발급한다.
    /// clone된 draft의 재사용은 같은 immutable snapshot에만 허용되며 변경된 결과에는 적용할 수 없다.
    pub(crate) fn current_default_draft(
        &self,
        field_id: FieldId,
    ) -> Result<FieldValueDraft, TemplateMutationError> {
        self.validate_storage()
            .map_err(TemplateMutationError::invalid_source)?;
        if self.lifecycle == TemplateLifecycle::Deleted {
            return Err(TemplateMutationError::template_is_tombstoned(
                self.template_id,
            ));
        }
        let field = self
            .fields
            .get(&field_id)
            .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
        if field.lifecycle == FieldLifecycle::Archived {
            return Err(TemplateMutationError::field_is_archived(field_id));
        }
        let provenance = self.current_default_provenance(field_id).ok_or_else(|| {
            TemplateMutationError::new(
                TemplateMutationErrorCategory::InvalidSource,
                "current default has no owned provenance; reload the Template before retrying",
            )
        })?;
        Ok(FieldValueDraft {
            value: provenance.value,
            origin: FieldValueDraftOrigin::SourceOwned(Box::new(SourceOwnedCurrentDefault {
                snapshot: self.default_draft_snapshot.clone(),
                template_id: provenance.template_id,
                field_id: provenance.field_id,
                provenance: provenance.source,
            })),
        })
    }
}

impl NewFieldConfiguration {
    pub(crate) fn group() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Group,
        }
    }
    pub(crate) const fn single_line_text() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::SingleLineText,
        }
    }

    pub(crate) const fn rich_text() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::RichText,
        }
    }

    pub(crate) fn bounded_number(minimum: Option<String>, maximum: Option<String>) -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Number { minimum, maximum },
        }
    }

    pub(crate) const fn number() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Number {
                minimum: None,
                maximum: None,
            },
        }
    }

    pub(crate) const fn date() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Date,
        }
    }

    pub(crate) const fn image() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Image,
        }
    }
    pub(crate) const fn file() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::File,
        }
    }
    pub(crate) const fn url() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Url,
        }
    }
    pub(crate) const fn time() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Time,
        }
    }

    pub(crate) const fn duration() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Duration,
        }
    }

    pub(crate) fn single_choice(
        option_order: Vec<OptionId>,
        options: Vec<NewChoiceOptionDraft>,
    ) -> Self {
        Self {
            variant: NewFieldConfigurationVariant::SingleChoice {
                option_order,
                options,
            },
        }
    }

    pub(crate) fn multi_choice(
        option_order: Vec<OptionId>,
        options: Vec<NewChoiceOptionDraft>,
    ) -> Self {
        Self {
            variant: NewFieldConfigurationVariant::MultiChoice {
                option_order,
                options,
            },
        }
    }

    pub(crate) fn relation(
        multiple: bool,
        allowed_templates: Vec<TemplateId>,
        reciprocal_notice: bool,
    ) -> Self {
        Self {
            variant: NewFieldConfigurationVariant::Relation {
                multiple,
                allowed_templates,
                reciprocal_notice,
            },
        }
    }

    pub(crate) const fn document_link() -> Self {
        Self {
            variant: NewFieldConfigurationVariant::DocumentLink,
        }
    }
}

/// 생성 시 caller가 지정하는 값만 소유한다. lifecycle/revision/initial snapshot/extra는 입력에 없다.
#[derive(Clone, PartialEq)]
pub(crate) struct NewFieldDraft {
    field_id: FieldId,
    label: String,
    kind: FieldKind,
    configuration: NewFieldConfiguration,
    required: bool,
    presentation_token: Option<String>,
    first_default: FieldValueDraft,
}

impl NewFieldDraft {
    pub(crate) fn new(
        field_id: FieldId,
        label: String,
        kind: FieldKind,
        configuration: NewFieldConfiguration,
        required: bool,
        presentation_token: Option<String>,
        first_default: FieldValueDraft,
    ) -> Self {
        Self {
            field_id,
            label,
            kind,
            configuration,
            required,
            presentation_token,
            first_default,
        }
    }
}

/// index는 active `fieldOrder` 기준이며 길이와 같은 값은 append와 동일하다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewFieldInsertion {
    Append,
    At(usize),
}

/// index는 대상 Choice Field의 active `optionOrder` 기준이며 길이와 같은 값은 append다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewOptionInsertion {
    Append,
    At(usize),
}

impl TemplateMutationCommand {
    /// Template v1의 유일한 mutable top-level metadata인 `name`을 exact string으로 교체한다.
    pub(crate) fn set_template_name(name: String) -> Self {
        Self {
            operation: TemplateMutationOperation::SetTemplateName { name },
        }
    }

    /// Template presentation v1의 유일한 known member를 바꾸며 unknown extra는 그대로 둔다.
    pub(crate) fn set_template_presentation_token(token: Option<String>) -> Self {
        Self {
            operation: TemplateMutationOperation::SetTemplatePresentationToken { token },
        }
    }

    pub(crate) fn set_glossary_excluded(excluded: bool) -> Self {
        Self {
            operation: TemplateMutationOperation::SetGlossaryExcluded { excluded },
        }
    }

    /// tombstone은 같은 Template/revision에서 만든 완전 Document scan assessment를 소비한다.
    pub(crate) fn tombstone_template(assessment: TemplateReferenceAssessment) -> Self {
        Self {
            operation: TemplateMutationOperation::TombstoneTemplate { assessment },
        }
    }

    pub(crate) fn create_field(draft: NewFieldDraft, insertion: NewFieldInsertion) -> Self {
        Self {
            operation: TemplateMutationOperation::CreateField { draft, insertion },
        }
    }

    pub(crate) fn set_field_label(field_id: FieldId, label: String) -> Self {
        Self {
            operation: TemplateMutationOperation::SetFieldLabel { field_id, label },
        }
    }

    pub(crate) fn set_field_required(field_id: FieldId, required: bool) -> Self {
        Self {
            operation: TemplateMutationOperation::SetFieldRequired { field_id, required },
        }
    }

    /// Presentation v1의 유일한 known member만 바꾸고 미래 extra는 그대로 둔다.
    pub(crate) fn set_field_presentation_token(field_id: FieldId, token: Option<String>) -> Self {
        Self {
            operation: TemplateMutationOperation::SetFieldPresentationToken { field_id, token },
        }
    }

    pub(crate) fn set_current_default(field_id: FieldId, value: FieldValueDraft) -> Self {
        Self {
            operation: TemplateMutationOperation::SetCurrentDefault { field_id, value },
        }
    }

    pub(crate) fn reorder_fields(field_order: Vec<FieldId>) -> Self {
        Self {
            operation: TemplateMutationOperation::ReorderFields { field_order },
        }
    }

    pub(crate) fn archive_field(field_id: FieldId) -> Self {
        Self {
            operation: TemplateMutationOperation::ArchiveField { field_id },
        }
    }

    pub(crate) fn add_option(
        field_id: FieldId,
        draft: NewChoiceOptionDraft,
        insertion: NewOptionInsertion,
    ) -> Self {
        Self {
            operation: TemplateMutationOperation::AddOption {
                field_id,
                draft,
                insertion,
            },
        }
    }

    pub(crate) fn rename_option(field_id: FieldId, option_id: OptionId, label: String) -> Self {
        Self {
            operation: TemplateMutationOperation::RenameOption {
                field_id,
                option_id,
                label,
            },
        }
    }

    pub(crate) fn reorder_options(field_id: FieldId, option_order: Vec<OptionId>) -> Self {
        Self {
            operation: TemplateMutationOperation::ReorderOptions {
                field_id,
                option_order,
            },
        }
    }

    /// `current_default_repair`는 target Option을 참조하는 current default에만 허용된다.
    /// None은 repair 생략, Some은 explicit replacement 또는 unset을 뜻한다.
    pub(crate) fn archive_option(
        field_id: FieldId,
        option_id: OptionId,
        current_default_repair: Option<FieldValueDraft>,
    ) -> Self {
        Self {
            operation: TemplateMutationOperation::ArchiveOption {
                field_id,
                option_id,
                current_default_repair,
            },
        }
    }
}

/// mutation 성공은 원본과 분리된 검증 완료 snapshot만 소유한다.
#[must_use = "template mutation outcome must be inspected explicitly"]
#[derive(Clone, PartialEq)]
pub(crate) enum TemplateMutationOutcome {
    Unchanged,
    Changed(Box<TemplateArtifact>),
}

impl TemplateMutationOutcome {
    pub(crate) const fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    pub(crate) fn changed(&self) -> Option<&TemplateArtifact> {
        match self {
            Self::Unchanged => None,
            Self::Changed(template) => Some(template.as_ref()),
        }
    }

    pub(crate) fn into_changed(self) -> Option<TemplateArtifact> {
        match self {
            Self::Unchanged => None,
            Self::Changed(template) => Some(*template),
        }
    }
}

impl fmt::Debug for TemplateMutationOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unchanged => formatter.write_str("TemplateMutationOutcome::Unchanged"),
            Self::Changed(_) => formatter
                .debug_struct("TemplateMutationOutcome::Changed")
                .field("template_redacted", &true)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateMutationErrorCategory {
    GuideMetadataConflict,
    InvalidSource,
    InvalidDocumentSnapshot,
    RevisionMismatch,
    RevisionOverflow,
    InvalidTimestamp,
    TimestampRegression,
    TemplateIsTombstoned,
    TemplateHasDocuments,
    ReferenceAssessmentMismatch,
    ReferenceAssessmentStale,
    FieldNotFound,
    FieldAlreadyExists,
    FieldIsArchived,
    FieldIsNotChoice,
    OptionNotFound,
    OptionAlreadyExists,
    OptionIsArchived,
    InvalidInsertionPosition,
    InvalidOptionInsertionPosition,
    InvalidFieldOrder,
    InvalidOptionOrder,
    InvalidFieldDraft,
    InvalidOptionDraft,
    InvalidCurrentDefault,
    DefaultDraftOwnershipMismatch,
    ProvenanceFreeDefaultReplacement,
    CurrentDefaultRepairRequired,
    InvalidCurrentDefaultRepair,
    ImmutableTemplateIdentityChanged,
    ImmutableFieldChanged,
    ImmutableInitialDefaultChanged,
    ImmutableOptionChanged,
    FieldRemoved,
    OptionRemoved,
    OptionOwnerChanged,
    LifecycleReactivation,
    InvalidCandidate,
    UnexpectedMutationState,
}

/// 오류는 원문 aggregate나 command payload 대신 안전한 category와 문제 ID 하나만 보존한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MutationValidationSummary {
    category: ArtifactValidationErrorCategory,
    scalar_category: Option<ScalarValueErrorCategory>,
    scalar_location: Option<ArtifactScalarValueLocation>,
    choice_category: Option<ChoiceValidationErrorCategory>,
    choice_location: Option<ArtifactChoiceValueLocation>,
    rich_text_category: Option<RichTextValidationErrorCategory>,
    rich_text_location: Option<ArtifactRichTextValueLocation>,
    rich_text_structure_location: Option<RichTextErrorLocation>,
}

impl MutationValidationSummary {
    const fn from_artifact(error: ArtifactValidationError) -> Self {
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

/// 원문 오류 객체 대신 닫힌 enum 진단과 문제 위치의 ID만 복사한다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TemplateMutationError {
    category: TemplateMutationErrorCategory,
    template_id: Option<TemplateId>,
    field_id: Option<FieldId>,
    option_id: Option<OptionId>,
    reference_count: Option<u16>,
    reference_count_truncated: bool,
    validation: Option<MutationValidationSummary>,
    detail: &'static str,
}

impl TemplateMutationError {
    pub(crate) const fn category(self) -> TemplateMutationErrorCategory {
        self.category
    }

    pub(crate) const fn template_id(self) -> Option<TemplateId> {
        self.template_id
    }

    pub(crate) const fn field_id(self) -> Option<FieldId> {
        self.field_id
    }

    pub(crate) const fn option_id(self) -> Option<OptionId> {
        self.option_id
    }

    pub(crate) const fn reference_count(self) -> Option<u16> {
        self.reference_count
    }

    pub(crate) const fn reference_count_truncated(self) -> bool {
        self.reference_count_truncated
    }

    pub(crate) const fn validation_category(self) -> Option<ArtifactValidationErrorCategory> {
        match self.validation {
            Some(validation) => Some(validation.category),
            None => None,
        }
    }

    pub(crate) const fn validation_scalar_category(self) -> Option<ScalarValueErrorCategory> {
        match self.validation {
            Some(validation) => validation.scalar_category,
            None => None,
        }
    }

    pub(crate) const fn validation_scalar_location(self) -> Option<ArtifactScalarValueLocation> {
        match self.validation {
            Some(validation) => validation.scalar_location,
            None => None,
        }
    }

    pub(crate) const fn validation_choice_category(self) -> Option<ChoiceValidationErrorCategory> {
        match self.validation {
            Some(validation) => validation.choice_category,
            None => None,
        }
    }

    pub(crate) const fn validation_choice_location(self) -> Option<ArtifactChoiceValueLocation> {
        match self.validation {
            Some(validation) => validation.choice_location,
            None => None,
        }
    }

    pub(crate) const fn validation_rich_text_category(
        self,
    ) -> Option<RichTextValidationErrorCategory> {
        match self.validation {
            Some(validation) => validation.rich_text_category,
            None => None,
        }
    }

    pub(crate) const fn validation_rich_text_location(
        self,
    ) -> Option<ArtifactRichTextValueLocation> {
        match self.validation {
            Some(validation) => validation.rich_text_location,
            None => None,
        }
    }

    pub(crate) const fn validation_rich_text_structure_location(
        self,
    ) -> Option<RichTextErrorLocation> {
        match self.validation {
            Some(validation) => validation.rich_text_structure_location,
            None => None,
        }
    }

    const fn new(category: TemplateMutationErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            template_id: None,
            field_id: None,
            option_id: None,
            reference_count: None,
            reference_count_truncated: false,
            validation: None,
            detail,
        }
    }

    const fn invalid_source(source: ArtifactValidationError) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidSource,
            "source Template snapshot failed admission validation",
        );
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error
    }

    const fn invalid_document_snapshot(source: ArtifactValidationError) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidDocumentSnapshot,
            "Document snapshot failed admission validation",
        );
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error
    }

    const fn revision_mismatch() -> Self {
        Self::new(
            TemplateMutationErrorCategory::RevisionMismatch,
            "expected Template revision does not match the source snapshot",
        )
    }

    const fn revision_overflow() -> Self {
        Self::new(
            TemplateMutationErrorCategory::RevisionOverflow,
            "changed Template revision cannot be incremented",
        )
    }

    const fn invalid_timestamp() -> Self {
        Self::new(
            TemplateMutationErrorCategory::InvalidTimestamp,
            "updated timestamp must use canonical UTC milliseconds",
        )
    }

    const fn timestamp_regression() -> Self {
        Self::new(
            TemplateMutationErrorCategory::TimestampRegression,
            "updated timestamp precedes the admitted Template timestamps",
        )
    }

    const fn template_is_tombstoned(template_id: TemplateId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::TemplateIsTombstoned,
            "tombstoned Template cannot accept mutation commands",
        );
        error.template_id = Some(template_id);
        error
    }

    const fn template_has_documents(
        template_id: TemplateId,
        reference_count: u16,
        reference_count_truncated: bool,
    ) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::TemplateHasDocuments,
            "Template tombstone is blocked by referencing Documents",
        );
        error.template_id = Some(template_id);
        error.reference_count = Some(reference_count);
        error.reference_count_truncated = reference_count_truncated;
        error
    }

    const fn reference_assessment_mismatch(template_id: TemplateId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::ReferenceAssessmentMismatch,
            "reference assessment belongs to another Template",
        );
        error.template_id = Some(template_id);
        error
    }

    const fn reference_assessment_stale(template_id: TemplateId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::ReferenceAssessmentStale,
            "reference assessment does not match the source Template revision",
        );
        error.template_id = Some(template_id);
        error
    }

    const fn field_not_found(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::FieldNotFound,
            "Field command target does not exist",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn field_already_exists(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::FieldAlreadyExists,
            "new FieldId collides with a persisted Field",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn field_is_archived(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::FieldIsArchived,
            "archived Field cannot be modified",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn field_is_not_choice(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::FieldIsNotChoice,
            "Option command target is not a Choice Field",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn option_not_found(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::OptionNotFound,
            "Option command target does not exist in the target Field",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn option_already_exists(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::OptionAlreadyExists,
            "new OptionId collides with a persisted Option",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn option_is_archived(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::OptionIsArchived,
            "archived Option cannot be modified",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn invalid_insertion_position(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidInsertionPosition,
            "new Field insertion index is outside active fieldOrder",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn invalid_option_insertion_position(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidOptionInsertionPosition,
            "new Option insertion index is outside active optionOrder",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn invalid_field_order() -> Self {
        Self::new(
            TemplateMutationErrorCategory::InvalidFieldOrder,
            "Field order must be an exact permutation of active FieldIds",
        )
    }

    const fn invalid_option_order(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidOptionOrder,
            "Option order must be an exact permutation of active OptionIds",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn invalid_field_draft(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidFieldDraft,
            "new Field draft violates the closed creation contract",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn invalid_option_draft(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidOptionDraft,
            "Option input violates the closed mutation contract",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn invalid_field_draft_validation(
        field_id: FieldId,
        source: ArtifactValidationError,
    ) -> Self {
        let mut error = Self::invalid_field_draft(field_id);
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error
    }

    const fn invalid_current_default(field_id: FieldId, source: ArtifactValidationError) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidCurrentDefault,
            "current default failed the integrated Field validation contract",
        );
        error.field_id = Some(field_id);
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error
    }

    const fn current_default_repair_required(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::CurrentDefaultRepairRequired,
            "archived Option is referenced by the current default and requires explicit repair",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn invalid_current_default_repair(field_id: FieldId, option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
            "Option archive current-default repair violates the closed mutation contract",
        );
        error.field_id = Some(field_id);
        error.option_id = Some(option_id);
        error
    }

    const fn invalid_current_default_repair_validation(
        field_id: FieldId,
        option_id: OptionId,
        source: ArtifactValidationError,
    ) -> Self {
        let mut error = Self::invalid_current_default_repair(field_id, option_id);
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error
    }

    const fn immutable_template_identity_changed() -> Self {
        Self::new(
            TemplateMutationErrorCategory::ImmutableTemplateIdentityChanged,
            "mutation changed immutable Template identity metadata",
        )
    }

    const fn immutable_field_changed(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::ImmutableFieldChanged,
            "mutation changed immutable persisted Field state",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn immutable_initial_default_changed(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::ImmutableInitialDefaultChanged,
            "mutation changed immutable initial Field default",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn immutable_option_changed(option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::ImmutableOptionChanged,
            "mutation changed immutable archived Option state",
        );
        error.option_id = Some(option_id);
        error
    }

    const fn field_removed(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::FieldRemoved,
            "mutation physically removed a persisted Field",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn option_removed(option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::OptionRemoved,
            "mutation physically removed a persisted Option",
        );
        error.option_id = Some(option_id);
        error
    }

    const fn option_owner_changed(option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::OptionOwnerChanged,
            "mutation moved a persisted Option to another Field",
        );
        error.option_id = Some(option_id);
        error
    }

    const fn field_reactivated(field_id: FieldId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::LifecycleReactivation,
            "mutation reactivated an archived Field",
        );
        error.field_id = Some(field_id);
        error
    }

    const fn option_reactivated(option_id: OptionId) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::LifecycleReactivation,
            "mutation reactivated an archived Option",
        );
        error.option_id = Some(option_id);
        error
    }

    const fn template_reactivated() -> Self {
        Self::new(
            TemplateMutationErrorCategory::LifecycleReactivation,
            "mutation reactivated a tombstoned Template",
        )
    }

    const fn invalid_candidate(source: ArtifactValidationError) -> Self {
        let mut error = Self::new(
            TemplateMutationErrorCategory::InvalidCandidate,
            "final Template candidate failed aggregate validation",
        );
        error.validation = Some(MutationValidationSummary::from_artifact(source));
        error.field_id = source.field_id();
        error.option_id = source.choice_option_id();
        error
    }

    const fn unexpected_mutation_state() -> Self {
        Self::new(
            TemplateMutationErrorCategory::UnexpectedMutationState,
            "closed mutation operation produced an unsupported intermediate state",
        )
    }
}

impl fmt::Debug for TemplateMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TemplateMutationError")
            .field("category", &self.category)
            .field("template_id", &self.template_id)
            .field("field_id", &self.field_id)
            .field("option_id", &self.option_id)
            .field("reference_count", &self.reference_count)
            .field("reference_count_truncated", &self.reference_count_truncated)
            .field("validation", &self.validation)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for TemplateMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Template mutation failed ({:?}): {}",
            self.category, self.detail
        )?;
        if let Some(template_id) = self.template_id {
            write!(formatter, " (TemplateId {template_id})")?;
        }
        if let Some(field_id) = self.field_id {
            write!(formatter, " (FieldId {field_id})")?;
        }
        if let Some(option_id) = self.option_id {
            write!(formatter, " (OptionId {option_id})")?;
        }
        if let Some(reference_count) = self.reference_count {
            write!(formatter, " (Document references {reference_count}")?;
            if self.reference_count_truncated {
                formatter.write_str("+")?;
            }
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

impl std::error::Error for TemplateMutationError {}

/// immutable source를 admission한 뒤 private candidate 하나에서만 명령을 적용하고 최종 검증한다.
pub(crate) fn apply_template_mutation(
    source: &TemplateArtifact,
    expected_revision: TemplateRevision,
    updated_at_utc: &str,
    command: TemplateMutationCommand,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    source
        .validate_storage()
        .map_err(TemplateMutationError::invalid_source)?;

    // no-op도 optimistic precondition을 우회하지 않도록 candidate 생성 전에 확인한다.
    if expected_revision != source.revision {
        return Err(TemplateMutationError::revision_mismatch());
    }
    validate_requested_timestamp(source, updated_at_utc)?;
    if source.lifecycle == TemplateLifecycle::Deleted {
        return Err(TemplateMutationError::template_is_tombstoned(
            source.template_id,
        ));
    }

    // equality로 owner를 추측하지 않는다. candidate 생성 전 command의 발급 snapshot을 확인한다.
    admit_default_draft_ownership(source, &command)?;
    let mut candidate = source.clone();
    apply_command(&mut candidate, command)?;
    finish_candidate(source, candidate, updated_at_utc, false)
}

/// 휴지통의 Template에만 허용되는 닫힌 복원 경계다. 일반 mutation의
/// tombstone 거절과 Field/Option의 archived 상태는 그대로 유지한다.
pub(crate) fn restore_tombstoned_template(
    source: &TemplateArtifact,
    expected_revision: TemplateRevision,
    updated_at_utc: &str,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    source
        .validate_storage()
        .map_err(TemplateMutationError::invalid_source)?;
    if expected_revision != source.revision {
        return Err(TemplateMutationError::revision_mismatch());
    }
    validate_requested_timestamp(source, updated_at_utc)?;
    if source.lifecycle != TemplateLifecycle::Deleted {
        return Err(TemplateMutationError::unexpected_mutation_state());
    }
    let mut candidate = source.clone();
    candidate.lifecycle = TemplateLifecycle::Active;
    finish_candidate(source, candidate, updated_at_utc, true)
}

/// 개별 명령과 전체 초안은 같은 마지막 검증·이력 확정 경계를 공유한다.
fn finish_candidate(
    source: &TemplateArtifact,
    mut candidate: TemplateArtifact,
    updated_at_utc: &str,
    allow_template_restore: bool,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    validate_historical_invariants(source, &candidate, allow_template_restore)?;

    // revision/timestamp까지 포함한 전체 equality라 unknown extra 차이도 no-op으로 축소되지 않는다.
    if candidate == *source {
        return Ok(TemplateMutationOutcome::Unchanged);
    }

    let next_revision = source
        .revision
        .checked_increment()
        .map_err(|_| TemplateMutationError::revision_overflow())?;
    candidate.revision = next_revision;
    candidate.updated_at_utc = updated_at_utc.to_owned();
    validate_new_field_revisions(source, &candidate, next_revision)?;
    candidate
        .validate_storage()
        .map_err(TemplateMutationError::invalid_candidate)?;
    // Unknown storage 불변식을 모두 확인한 뒤에만 known 변경을 새 provenance 기준에 반영한다.
    candidate
        .rebase_lossless_source()
        .map_err(TemplateMutationError::invalid_candidate)?;
    candidate.default_draft_snapshot = DefaultDraftSnapshot::new();

    Ok(TemplateMutationOutcome::Changed(Box::new(candidate)))
}

fn admit_default_draft_ownership(
    source: &TemplateArtifact,
    command: &TemplateMutationCommand,
) -> Result<(), TemplateMutationError> {
    let (field_id, draft, repair_option) = match &command.operation {
        TemplateMutationOperation::SetCurrentDefault { field_id, value } => {
            (*field_id, value, None)
        }
        TemplateMutationOperation::ArchiveOption {
            field_id,
            option_id,
            current_default_repair: Some(value),
        } => (*field_id, value, Some(*option_id)),
        TemplateMutationOperation::CreateField { draft, .. }
            if matches!(
                draft.first_default.origin,
                FieldValueDraftOrigin::SourceOwned(_)
            ) =>
        {
            return Err(TemplateMutationError::invalid_field_draft(draft.field_id));
        }
        _ => return Ok(()),
    };
    let category = match &draft.origin {
        FieldValueDraftOrigin::Fresh => return Ok(()),
        FieldValueDraftOrigin::ProvenanceFree => {
            TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement
        }
        FieldValueDraftOrigin::SourceOwned(owned) => {
            // 같은 ID/revision/내용만으로는 별도 decode나 fork를 구분할 수 없다.
            if source.template_id != owned.template_id
                || field_id != owned.field_id
                || !source
                    .default_draft_snapshot
                    .is_same_snapshot(&owned.snapshot)
            {
                TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch
            } else {
                // identity를 확인한 뒤의 방어 검사다. 이 비교 자체가 provenance를 발급하지 않는다.
                let current = source.current_default_provenance(field_id);
                if current.is_some_and(|current| {
                    current.value == draft.value && current.source == owned.provenance
                }) {
                    return Ok(());
                }
                TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch
            }
        }
    };
    if let Some(option_id) = repair_option {
        return Err(TemplateMutationError::invalid_current_default_repair(
            field_id, option_id,
        ));
    }
    let mut error = TemplateMutationError::new(category,
        "default draft lacks current Field ownership; use a fresh typed edit or request a draft from this Template Field");
    error.field_id = Some(field_id);
    Err(error)
}

fn validate_requested_timestamp(
    source: &TemplateArtifact,
    updated_at_utc: &str,
) -> Result<(), TemplateMutationError> {
    if !is_utc_milliseconds(updated_at_utc) {
        return Err(TemplateMutationError::invalid_timestamp());
    }
    if updated_at_utc < source.created_at_utc.as_str()
        || updated_at_utc < source.updated_at_utc.as_str()
    {
        return Err(TemplateMutationError::timestamp_regression());
    }
    Ok(())
}

fn apply_command(
    candidate: &mut TemplateArtifact,
    command: TemplateMutationCommand,
) -> Result<(), TemplateMutationError> {
    match command.operation {
        TemplateMutationOperation::SetTemplateName { name } => {
            candidate.name = name;
            Ok(())
        }
        TemplateMutationOperation::SetTemplatePresentationToken { token } => {
            candidate.presentation.token = token;
            Ok(())
        }
        TemplateMutationOperation::SetGlossaryExcluded { excluded } => {
            candidate.glossary_excluded = excluded;
            Ok(())
        }
        TemplateMutationOperation::TombstoneTemplate { assessment } => {
            apply_template_tombstone(candidate, assessment)
        }
        TemplateMutationOperation::CreateField { draft, insertion } => {
            apply_create_field(candidate, draft, insertion)
        }
        TemplateMutationOperation::SetFieldLabel { field_id, label } => {
            active_field_mut(candidate, field_id)?.label = label;
            Ok(())
        }
        TemplateMutationOperation::SetFieldRequired { field_id, required } => {
            active_field_mut(candidate, field_id)?.required = required;
            Ok(())
        }
        TemplateMutationOperation::SetFieldPresentationToken { field_id, token } => {
            active_field_mut(candidate, field_id)?.presentation.token = token;
            Ok(())
        }
        TemplateMutationOperation::SetCurrentDefault { field_id, value } => {
            apply_current_default(candidate, field_id, value)
        }
        TemplateMutationOperation::ReorderFields { field_order } => {
            apply_field_order(candidate, field_order)
        }
        TemplateMutationOperation::ArchiveField { field_id } => {
            apply_archive_field(candidate, field_id)
        }
        TemplateMutationOperation::AddOption {
            field_id,
            draft,
            insertion,
        } => apply_add_option(candidate, field_id, draft, insertion),
        TemplateMutationOperation::RenameOption {
            field_id,
            option_id,
            label,
        } => apply_rename_option(candidate, field_id, option_id, label),
        TemplateMutationOperation::ReorderOptions {
            field_id,
            option_order,
        } => apply_option_order(candidate, field_id, option_order),
        TemplateMutationOperation::ArchiveOption {
            field_id,
            option_id,
            current_default_repair,
        } => apply_archive_option(candidate, field_id, option_id, current_default_repair),
        #[cfg(test)]
        TemplateMutationOperation::Test(operation) => apply_test_mutation(candidate, operation),
    }
}

fn apply_template_tombstone(
    candidate: &mut TemplateArtifact,
    assessment: TemplateReferenceAssessment,
) -> Result<(), TemplateMutationError> {
    if assessment.template_id != candidate.template_id {
        return Err(TemplateMutationError::reference_assessment_mismatch(
            candidate.template_id,
        ));
    }
    if assessment.source_revision != candidate.revision {
        return Err(TemplateMutationError::reference_assessment_stale(
            candidate.template_id,
        ));
    }
    candidate.lifecycle = TemplateLifecycle::Deleted;
    Ok(())
}

fn active_field_mut(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
) -> Result<&mut FieldDefinition, TemplateMutationError> {
    let field = candidate
        .fields
        .get_mut(&field_id)
        .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
    if field.lifecycle == FieldLifecycle::Archived {
        return Err(TemplateMutationError::field_is_archived(field_id));
    }
    Ok(field)
}

fn apply_current_default(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
    draft: FieldValueDraft,
) -> Result<(), TemplateMutationError> {
    let field = active_field_mut(candidate, field_id)?;
    field.default_value = prepare_current_default_value(&field.default_value, draft)
        .map_err(|()| TemplateMutationError::immutable_field_changed(field_id))?;

    field
        .validate_fresh_default()
        .map_err(|error| TemplateMutationError::invalid_current_default(field_id, error))?;

    // source는 이미 admission을 통과했으므로 이 시점의 실패는 바뀐 current default에 귀속된다.
    candidate
        .validate_storage()
        .map_err(|error| TemplateMutationError::invalid_current_default(field_id, error))
}

/// Fresh는 source envelope를 직접 복사한다. SourceOwned는 앞선 발급 snapshot admission이 필수다.
fn prepare_current_default_value(
    source: &FieldValue,
    draft: FieldValueDraft,
) -> Result<FieldValue, ()> {
    let (value, origin) = draft.into_parts();
    match origin {
        // Fresh typed constructor에는 outer extra 입력 경로가 없으므로 persisted envelope를
        // 그대로 운반한다. rich-text 내부 metadata는 historical invariant가 계속 비교한다.
        // NormalizedRichText도 미래 AST metadata를 운반할 수 있다. 정규화는 ownership 증거가
        // 아니므로 Fresh는 실제 known-only 입력만 허용하고 기존 envelope는 직접 clone한다.
        FieldValueDraftOrigin::Fresh if !value.contains_unknown_storage_data() => {
            Ok(value.preserve_outer_storage_extra_from(source))
        }
        FieldValueDraftOrigin::Fresh => Err(()),
        FieldValueDraftOrigin::SourceOwned(_) => Ok(value),
        FieldValueDraftOrigin::ProvenanceFree => Err(()),
    }
}

fn require_active_choice_field(
    candidate: &TemplateArtifact,
    field_id: FieldId,
) -> Result<(), TemplateMutationError> {
    let field = candidate
        .fields
        .get(&field_id)
        .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
    if field.lifecycle == FieldLifecycle::Archived {
        return Err(TemplateMutationError::field_is_archived(field_id));
    }
    if field.configuration.options().is_none() {
        return Err(TemplateMutationError::field_is_not_choice(field_id));
    }
    Ok(())
}

fn active_choice_configuration_mut(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
) -> Result<(&mut Vec<OptionId>, &mut BTreeMap<OptionId, ChoiceOption>), TemplateMutationError> {
    let field = candidate
        .fields
        .get_mut(&field_id)
        .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
    if field.lifecycle == FieldLifecycle::Archived {
        return Err(TemplateMutationError::field_is_archived(field_id));
    }
    match &mut field.configuration.variant {
        FieldConfigurationVariant::SingleChoice {
            option_order,
            options,
        }
        | FieldConfigurationVariant::MultiChoice {
            option_order,
            options,
        } => Ok((option_order, options)),
        FieldConfigurationVariant::Group { .. }
        | FieldConfigurationVariant::SingleLineText
        | FieldConfigurationVariant::RichText
        | FieldConfigurationVariant::Number { .. }
        | FieldConfigurationVariant::Date
        | FieldConfigurationVariant::Time
        | FieldConfigurationVariant::Image
        | FieldConfigurationVariant::File
        | FieldConfigurationVariant::Url
        | FieldConfigurationVariant::Duration
        | FieldConfigurationVariant::Relation { .. }
        | FieldConfigurationVariant::DocumentLink => {
            Err(TemplateMutationError::field_is_not_choice(field_id))
        }
    }
}

fn apply_add_option(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
    draft: NewChoiceOptionDraft,
    insertion: NewOptionInsertion,
) -> Result<(), TemplateMutationError> {
    require_active_choice_field(candidate, field_id)?;
    let option_id = draft.option_id;
    if draft.lifecycle != OptionLifecycle::Active {
        return Err(TemplateMutationError::invalid_option_draft(
            field_id, option_id,
        ));
    }
    if candidate.fields.values().any(|field| {
        field
            .configuration
            .options()
            .is_some_and(|options| options.contains_key(&option_id))
    }) {
        return Err(TemplateMutationError::option_already_exists(
            field_id, option_id,
        ));
    }

    let (option_order, options) = active_choice_configuration_mut(candidate, field_id)?;
    let insertion_index = match insertion {
        NewOptionInsertion::Append => option_order.len(),
        NewOptionInsertion::At(index) if index <= option_order.len() => index,
        NewOptionInsertion::At(_) => {
            return Err(TemplateMutationError::invalid_option_insertion_position(
                field_id, option_id,
            ));
        }
    };
    option_order.insert(insertion_index, option_id);
    options.insert(
        option_id,
        ChoiceOption {
            label: draft.label,
            lifecycle: OptionLifecycle::Active,
            extra: BTreeMap::new(),
        },
    );
    Ok(())
}

fn apply_rename_option(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
    option_id: OptionId,
    label: String,
) -> Result<(), TemplateMutationError> {
    let (_, options) = active_choice_configuration_mut(candidate, field_id)?;
    let option = options
        .get_mut(&option_id)
        .ok_or_else(|| TemplateMutationError::option_not_found(field_id, option_id))?;
    if option.lifecycle == OptionLifecycle::Archived {
        return Err(TemplateMutationError::option_is_archived(
            field_id, option_id,
        ));
    }
    option.label = label;
    Ok(())
}

fn apply_option_order(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
    option_order: Vec<OptionId>,
) -> Result<(), TemplateMutationError> {
    let (stored_order, options) = active_choice_configuration_mut(candidate, field_id)?;
    let active_count = options
        .values()
        .filter(|option| option.lifecycle == OptionLifecycle::Active)
        .count();
    if option_order.len() != active_count {
        return Err(TemplateMutationError::invalid_option_order(field_id));
    }

    let mut seen = BTreeSet::new();
    for option_id in &option_order {
        if !seen.insert(*option_id)
            || options
                .get(option_id)
                .is_none_or(|option| option.lifecycle != OptionLifecycle::Active)
        {
            return Err(TemplateMutationError::invalid_option_order(field_id));
        }
    }

    *stored_order = option_order;
    Ok(())
}

fn apply_archive_option(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
    option_id: OptionId,
    current_default_repair: Option<FieldValueDraft>,
) -> Result<(), TemplateMutationError> {
    let field = candidate
        .fields
        .get_mut(&field_id)
        .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
    if field.lifecycle == FieldLifecycle::Archived {
        return Err(TemplateMutationError::field_is_archived(field_id));
    }
    if field.configuration.options().is_none() {
        return Err(TemplateMutationError::field_is_not_choice(field_id));
    }
    let option = field
        .configuration
        .options()
        .and_then(|options| options.get(&option_id))
        .ok_or_else(|| TemplateMutationError::option_not_found(field_id, option_id))?;
    if option.lifecycle == OptionLifecycle::Archived {
        return Err(TemplateMutationError::option_is_archived(
            field_id, option_id,
        ));
    }

    let current_references_target = field.default_value.single_choice() == Some(option_id)
        || field
            .default_value
            .multi_choice()
            .is_some_and(|option_ids| option_ids.contains(&option_id));
    match (current_references_target, current_default_repair.is_some()) {
        (true, false) => {
            return Err(TemplateMutationError::current_default_repair_required(
                field_id, option_id,
            ));
        }
        // Archive는 unrelated current default 편집의 우회 경로가 아니다.
        (false, true) => {
            return Err(TemplateMutationError::invalid_current_default_repair(
                field_id, option_id,
            ));
        }
        (true, true) | (false, false) => {}
    }

    match &mut field.configuration.variant {
        FieldConfigurationVariant::SingleChoice {
            option_order,
            options,
        }
        | FieldConfigurationVariant::MultiChoice {
            option_order,
            options,
        } => {
            option_order.retain(|ordered| *ordered != option_id);
            let Some(option) = options.get_mut(&option_id) else {
                return Err(TemplateMutationError::option_not_found(field_id, option_id));
            };
            option.lifecycle = OptionLifecycle::Archived;
        }
        FieldConfigurationVariant::Group { .. }
        | FieldConfigurationVariant::SingleLineText
        | FieldConfigurationVariant::RichText
        | FieldConfigurationVariant::Number { .. }
        | FieldConfigurationVariant::Date
        | FieldConfigurationVariant::Time
        | FieldConfigurationVariant::Image
        | FieldConfigurationVariant::File
        | FieldConfigurationVariant::Url
        | FieldConfigurationVariant::Duration
        | FieldConfigurationVariant::Relation { .. }
        | FieldConfigurationVariant::DocumentLink => {
            return Err(TemplateMutationError::field_is_not_choice(field_id));
        }
    }

    if let Some(repair) = current_default_repair {
        field.default_value =
            prepare_current_default_value(&field.default_value, repair).map_err(|()| {
                TemplateMutationError::invalid_current_default_repair(field_id, option_id)
            })?;
        candidate.validate_storage().map_err(|error| {
            TemplateMutationError::invalid_current_default_repair_validation(
                field_id, option_id, error,
            )
        })?;
    }
    Ok(())
}

fn apply_field_order(
    candidate: &mut TemplateArtifact,
    field_order: Vec<FieldId>,
) -> Result<(), TemplateMutationError> {
    let active_count = candidate
        .fields
        .values()
        .filter(|field| field.lifecycle == FieldLifecycle::Active)
        .count();
    if field_order.len() != active_count {
        return Err(TemplateMutationError::invalid_field_order());
    }

    let mut seen = BTreeSet::new();
    for field_id in &field_order {
        if !seen.insert(*field_id) {
            return Err(TemplateMutationError::invalid_field_order());
        }
        let Some(field) = candidate.fields.get(field_id) else {
            return Err(TemplateMutationError::invalid_field_order());
        };
        if field.lifecycle != FieldLifecycle::Active {
            return Err(TemplateMutationError::invalid_field_order());
        }
    }

    candidate.field_order = field_order;
    Ok(())
}

fn apply_archive_field(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
) -> Result<(), TemplateMutationError> {
    let field = candidate
        .fields
        .get_mut(&field_id)
        .ok_or_else(|| TemplateMutationError::field_not_found(field_id))?;
    if field.lifecycle == FieldLifecycle::Archived {
        return Ok(());
    }

    // 복구 snapshot인 definition은 lifecycle 외에 손대지 않고 표시 order에서만 제거한다.
    field.lifecycle = FieldLifecycle::Archived;
    let next = candidate
        .field_order
        .iter()
        .position(|id| *id == field_id)
        .and_then(|i| candidate.field_order.get(i + 1))
        .copied();
    for section in &mut candidate.sections {
        if section.before_field == Some(field_id) {
            section.before_field = next;
        }
    }
    candidate.field_order.retain(|ordered| *ordered != field_id);
    Ok(())
}

fn apply_create_field(
    candidate: &mut TemplateArtifact,
    draft: NewFieldDraft,
    insertion: NewFieldInsertion,
) -> Result<(), TemplateMutationError> {
    let field_id = draft.field_id;
    if candidate.fields.contains_key(&field_id) {
        return Err(TemplateMutationError::field_already_exists(field_id));
    }

    let insertion_index = match insertion {
        NewFieldInsertion::Append => candidate.field_order.len(),
        NewFieldInsertion::At(index) if index <= candidate.field_order.len() => index,
        NewFieldInsertion::At(_) => {
            return Err(TemplateMutationError::invalid_insertion_position(field_id));
        }
    };
    let first_default = draft.first_default.into_value();
    if first_default.contains_unknown_storage_data() {
        return Err(TemplateMutationError::invalid_field_draft(field_id));
    }

    let next_revision = next_revision_for_new_field(candidate)?;
    let configuration = build_new_configuration(field_id, draft.configuration)?;
    if configuration.kind() != draft.kind {
        return Err(TemplateMutationError::invalid_field_draft(field_id));
    }

    let mut option_ids = candidate
        .fields
        .values()
        .filter_map(|field| field.configuration.options())
        .flat_map(|options| options.keys().copied())
        .collect::<BTreeSet<_>>();
    let definition = FieldDefinition {
        writing_guide: None,
        label: draft.label,
        lifecycle: FieldLifecycle::Active,
        kind: draft.kind,
        required: draft.required,
        default_value: first_default.clone(),
        initial_default_value: first_default,
        introduced_revision: next_revision,
        configuration,
        presentation: Presentation {
            token: draft.presentation_token,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    };
    definition
        .validate_structure(candidate.schema_version, next_revision, &mut option_ids)
        .map_err(|error| TemplateMutationError::invalid_field_draft_validation(field_id, error))?;
    if definition.kind == FieldKind::Number {
        definition.validate_fresh_default().map_err(|error| {
            TemplateMutationError::invalid_field_draft_validation(field_id, error)
        })?;
    }

    candidate.fields.insert(field_id, definition);
    candidate.field_order.insert(insertion_index, field_id);
    Ok(())
}

fn build_new_configuration(
    field_id: FieldId,
    configuration: NewFieldConfiguration,
) -> Result<FieldConfiguration, TemplateMutationError> {
    let variant = match configuration.variant {
        NewFieldConfigurationVariant::Group => FieldConfigurationVariant::Group {
            member_order: vec![],
            members: BTreeMap::new(),
        },
        NewFieldConfigurationVariant::SingleLineText => FieldConfigurationVariant::SingleLineText,
        NewFieldConfigurationVariant::RichText => FieldConfigurationVariant::RichText,
        NewFieldConfigurationVariant::Number { minimum, maximum } => {
            FieldConfigurationVariant::Number { minimum, maximum }
        }
        NewFieldConfigurationVariant::Date => FieldConfigurationVariant::Date,
        NewFieldConfigurationVariant::Time => FieldConfigurationVariant::Time,
        NewFieldConfigurationVariant::Image => FieldConfigurationVariant::Image,
        NewFieldConfigurationVariant::File => FieldConfigurationVariant::File,
        NewFieldConfigurationVariant::Url => FieldConfigurationVariant::Url,
        NewFieldConfigurationVariant::Duration => FieldConfigurationVariant::Duration,
        NewFieldConfigurationVariant::SingleChoice {
            option_order,
            options,
        } => FieldConfigurationVariant::SingleChoice {
            option_order,
            options: build_new_options(field_id, options)?,
        },
        NewFieldConfigurationVariant::MultiChoice {
            option_order,
            options,
        } => FieldConfigurationVariant::MultiChoice {
            option_order,
            options: build_new_options(field_id, options)?,
        },
        NewFieldConfigurationVariant::Relation {
            multiple,
            allowed_templates,
            reciprocal_notice,
        } => FieldConfigurationVariant::Relation {
            multiple,
            allowed_templates,
            reciprocal_notice,
        },
        NewFieldConfigurationVariant::DocumentLink => FieldConfigurationVariant::DocumentLink,
    };
    Ok(FieldConfiguration {
        variant,
        extra: BTreeMap::new(),
    })
}

fn build_new_options(
    field_id: FieldId,
    drafts: Vec<NewChoiceOptionDraft>,
) -> Result<BTreeMap<OptionId, ChoiceOption>, TemplateMutationError> {
    let mut options = BTreeMap::new();
    for draft in drafts {
        if draft.lifecycle != OptionLifecycle::Active
            || options
                .insert(
                    draft.option_id,
                    ChoiceOption {
                        label: draft.label,
                        lifecycle: draft.lifecycle,
                        extra: BTreeMap::new(),
                    },
                )
                .is_some()
        {
            return Err(TemplateMutationError::invalid_field_draft(field_id));
        }
    }
    Ok(options)
}

fn validate_historical_invariants(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    allow_template_restore: bool,
) -> Result<(), TemplateMutationError> {
    if (source.schema_version != candidate.schema_version
        && !(source.schema_version.get() == 1
            && candidate.schema_version.get() == 2
            && candidate.fields.iter().any(|(id, field)| {
                field.writing_guide.as_ref()
                    != source.fields.get(id).and_then(|f| f.writing_guide.as_ref())
            })))
        || source.artifact_type != candidate.artifact_type
        || source.template_id != candidate.template_id
        || source.created_at_utc != candidate.created_at_utc
    {
        return Err(TemplateMutationError::immutable_template_identity_changed());
    }

    // command는 engine-owned metadata나 unknown transport state를 직접 다룰 수 없다.
    if source.revision != candidate.revision
        || source.updated_at_utc != candidate.updated_at_utc
        || source.extra != candidate.extra
        || source.presentation.extra != candidate.presentation.extra
    {
        return Err(TemplateMutationError::unexpected_mutation_state());
    }

    match (source.lifecycle, candidate.lifecycle) {
        (TemplateLifecycle::Deleted, TemplateLifecycle::Active) if allow_template_restore => {}
        (TemplateLifecycle::Deleted, TemplateLifecycle::Active) => {
            return Err(TemplateMutationError::template_reactivated());
        }
        (TemplateLifecycle::Active, TemplateLifecycle::Active)
        | (TemplateLifecycle::Active, TemplateLifecycle::Deleted)
        | (TemplateLifecycle::Deleted, TemplateLifecycle::Deleted) => {}
    }

    for (field_id, source_field) in &source.fields {
        let candidate_field = candidate
            .fields
            .get(field_id)
            .ok_or_else(|| TemplateMutationError::field_removed(*field_id))?;

        if source_field.kind != candidate_field.kind
            || source_field.introduced_revision != candidate_field.introduced_revision
        {
            return Err(TemplateMutationError::immutable_field_changed(*field_id));
        }
        if source_field.initial_default_value != candidate_field.initial_default_value {
            return Err(TemplateMutationError::immutable_initial_default_changed(
                *field_id,
            ));
        }
        if source_field.extra != candidate_field.extra
            || source_field.presentation.extra != candidate_field.presentation.extra
            || source_field.configuration.extra != candidate_field.configuration.extra
            || !source_field
                .default_value
                .has_same_storage_extras(&candidate_field.default_value)
        {
            return Err(TemplateMutationError::immutable_field_changed(*field_id));
        }

        match (source_field.lifecycle, candidate_field.lifecycle) {
            (FieldLifecycle::Archived, FieldLifecycle::Active) => {
                return Err(TemplateMutationError::field_reactivated(*field_id));
            }
            (FieldLifecycle::Active, FieldLifecycle::Active)
            | (FieldLifecycle::Active, FieldLifecycle::Archived)
            | (FieldLifecycle::Archived, FieldLifecycle::Archived) => {}
        }

        // Option ID/owner/lifecycle는 archived Field 전체 비교보다 구체적인 기존 오류를 유지한다.
        validate_persisted_options(candidate, *field_id, source_field)?;

        // archived Field는 마지막 label/default/configuration/option을 포함한 복구 snapshot이다.
        // 속성을 열거하면 미래 member가 빠질 수 있으므로 persisted definition 전체를 고정한다.
        if source_field.lifecycle == FieldLifecycle::Archived && source_field != candidate_field {
            return Err(TemplateMutationError::immutable_field_changed(*field_id));
        }
    }
    Ok(())
}

fn validate_persisted_options(
    candidate: &TemplateArtifact,
    source_field_id: FieldId,
    source_field: &super::FieldDefinition,
) -> Result<(), TemplateMutationError> {
    let Some(source_options) = source_field.configuration.options() else {
        return Ok(());
    };
    for (option_id, source_option) in source_options {
        let candidate_option = candidate
            .fields
            .get(&source_field_id)
            .and_then(|field| field.configuration.options())
            .and_then(|options| options.get(option_id));
        let Some(candidate_option) = candidate_option else {
            let found_elsewhere = candidate.fields.iter().any(|(field_id, field)| {
                *field_id != source_field_id
                    && field
                        .configuration
                        .options()
                        .is_some_and(|options| options.contains_key(option_id))
            });
            return Err(if found_elsewhere {
                TemplateMutationError::option_owner_changed(*option_id)
            } else {
                TemplateMutationError::option_removed(*option_id)
            });
        };

        match (source_option.lifecycle, candidate_option.lifecycle) {
            (OptionLifecycle::Archived, OptionLifecycle::Active) => {
                return Err(TemplateMutationError::option_reactivated(*option_id));
            }
            (OptionLifecycle::Active, OptionLifecycle::Active)
            | (OptionLifecycle::Active, OptionLifecycle::Archived)
            | (OptionLifecycle::Archived, OptionLifecycle::Archived) => {}
        }
        if source_option.extra != candidate_option.extra
            || (source_option.lifecycle == OptionLifecycle::Archived
                && source_option.label != candidate_option.label)
        {
            return Err(TemplateMutationError::immutable_option_changed(*option_id));
        }
    }
    Ok(())
}

fn validate_new_field_revisions(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    next_revision: TemplateRevision,
) -> Result<(), TemplateMutationError> {
    for (field_id, field) in &candidate.fields {
        if !source.fields.contains_key(field_id) && field.introduced_revision != next_revision {
            return Err(TemplateMutationError::immutable_field_changed(*field_id));
        }
    }
    Ok(())
}

/// 새 Field의 introducedRevision은 command 입력이 아니라 admitted source에서 계산한다.
/// M2-4b의 Field 생성 variant만 이 private helper를 사용한다.
fn next_revision_for_new_field(
    candidate: &TemplateArtifact,
) -> Result<TemplateRevision, TemplateMutationError> {
    candidate
        .revision
        .checked_increment()
        .map_err(|_| TemplateMutationError::revision_overflow())
}

#[cfg(test)]
enum TestMutation {
    Noop,
    Rename(String),
    RenameField {
        field_id: FieldId,
        label: String,
    },
    RenameAndPresentation {
        name: String,
        token: String,
    },
    RenameThenReject(String),
    ChangeTemplateId(super::super::TemplateId),
    ChangeSchemaVersion(crate::data::schema::SchemaVersion),
    ChangeArtifactType(super::super::ArtifactType),
    ChangeCreatedAt(String),
    ChangeRevision(TemplateRevision),
    ChangeUpdatedAt(String),
    ChangeRootExtra,
    RemoveField(FieldId),
    ChangeFieldKind {
        field_id: FieldId,
        kind: super::super::FieldKind,
    },
    ChangeIntroducedRevision {
        field_id: FieldId,
        revision: TemplateRevision,
    },
    ReplaceInitialWithCurrent(FieldId),
    ReplaceCurrentDefault {
        field_id: FieldId,
        value: super::super::FieldValue,
    },
    ReplaceCurrentRichTextContent {
        field_id: FieldId,
        content: serde_json::Map<String, serde_json::Value>,
    },
    ReplaceFieldDefinition {
        field_id: FieldId,
        source_field_id: FieldId,
    },
    RemoveOption {
        field_id: FieldId,
        option_id: OptionId,
    },
    MoveOption {
        source_field_id: FieldId,
        target_field_id: FieldId,
        option_id: OptionId,
    },
    ReactivateField(FieldId),
    ReactivateOption {
        field_id: FieldId,
        option_id: OptionId,
    },
    ArchiveOptionOnly {
        field_id: FieldId,
        option_id: OptionId,
    },
    ArchiveOptionAndUnsetCurrent {
        field_id: FieldId,
        option_id: OptionId,
        unset_source_field_id: FieldId,
    },
    ReactivateTemplate,
    RenameArchivedOption {
        field_id: FieldId,
        option_id: OptionId,
    },
    ClearFieldOrder,
    ClearOptionOrder(FieldId),
    DuplicateOptionInField {
        source_field_id: FieldId,
        target_field_id: FieldId,
        option_id: OptionId,
    },
    CorruptCurrentScalar {
        field_id: FieldId,
        raw: String,
    },
    CorruptCurrentMultiChoice {
        field_id: FieldId,
        option_ids: Vec<OptionId>,
    },
    AddFieldWithRevision {
        source_field_id: FieldId,
        new_field_id: FieldId,
        revision: TemplateRevision,
    },
    AddInvalidInitialText {
        source_field_id: FieldId,
        new_field_id: FieldId,
    },
}

#[cfg(test)]
impl TemplateMutationCommand {
    fn test(operation: TestMutation) -> Self {
        Self {
            operation: TemplateMutationOperation::Test(operation),
        }
    }
}

#[cfg(test)]
fn apply_test_mutation(
    candidate: &mut TemplateArtifact,
    operation: TestMutation,
) -> Result<(), TemplateMutationError> {
    match operation {
        TestMutation::Noop => {}
        TestMutation::Rename(name) => candidate.name = name,
        TestMutation::RenameField { field_id, label } => {
            test_field_mut(candidate, field_id).label = label;
        }
        TestMutation::RenameAndPresentation { name, token } => {
            candidate.name = name;
            candidate.presentation.token = Some(token);
        }
        TestMutation::RenameThenReject(name) => {
            candidate.name = name;
            return Err(TemplateMutationError::unexpected_mutation_state());
        }
        TestMutation::ChangeTemplateId(template_id) => candidate.template_id = template_id,
        TestMutation::ChangeSchemaVersion(schema_version) => {
            candidate.schema_version = schema_version;
        }
        TestMutation::ChangeArtifactType(artifact_type) => {
            candidate.artifact_type = artifact_type;
        }
        TestMutation::ChangeCreatedAt(created_at_utc) => {
            candidate.created_at_utc = created_at_utc;
        }
        TestMutation::ChangeRevision(revision) => candidate.revision = revision,
        TestMutation::ChangeUpdatedAt(updated_at_utc) => {
            candidate.updated_at_utc = updated_at_utc;
        }
        TestMutation::ChangeRootExtra => {
            candidate.extra.insert(
                "testOnlyChangedExtra".to_owned(),
                serde_json::Value::Bool(true),
            );
        }
        TestMutation::RemoveField(field_id) => {
            candidate.fields.remove(&field_id);
            candidate.field_order.retain(|id| *id != field_id);
        }
        TestMutation::ChangeFieldKind { field_id, kind } => {
            test_field_mut(candidate, field_id).kind = kind;
        }
        TestMutation::ChangeIntroducedRevision { field_id, revision } => {
            test_field_mut(candidate, field_id).introduced_revision = revision;
        }
        TestMutation::ReplaceInitialWithCurrent(field_id) => {
            let field = test_field_mut(candidate, field_id);
            field.initial_default_value = field.default_value.clone();
        }
        TestMutation::ReplaceCurrentDefault { field_id, value } => {
            test_field_mut(candidate, field_id).default_value = value;
        }
        TestMutation::ReplaceCurrentRichTextContent { field_id, content } => {
            candidate.replace_rich_text_default_content_for_test(field_id, content, false);
        }
        TestMutation::ReplaceFieldDefinition {
            field_id,
            source_field_id,
        } => {
            let lifecycle = candidate.fields[&field_id].lifecycle;
            let mut replacement = candidate.fields[&source_field_id].clone();
            replacement.lifecycle = lifecycle;
            candidate.fields.insert(field_id, replacement);
        }
        TestMutation::RemoveOption {
            field_id,
            option_id,
        } => {
            let (order, options) = test_choice_configuration_mut(candidate, field_id);
            order.retain(|id| *id != option_id);
            options.remove(&option_id);
        }
        TestMutation::MoveOption {
            source_field_id,
            target_field_id,
            option_id,
        } => {
            let option = {
                let (order, options) = test_choice_configuration_mut(candidate, source_field_id);
                order.retain(|id| *id != option_id);
                options
                    .remove(&option_id)
                    .expect("test source Field must own the Option")
            };
            let (order, options) = test_choice_configuration_mut(candidate, target_field_id);
            if option.lifecycle == OptionLifecycle::Active {
                order.push(option_id);
            }
            options.insert(option_id, option);
        }
        TestMutation::ReactivateField(field_id) => {
            let field = test_field_mut(candidate, field_id);
            field.lifecycle = FieldLifecycle::Active;
            candidate.field_order.push(field_id);
        }
        TestMutation::ReactivateOption {
            field_id,
            option_id,
        } => {
            let (order, options) = test_choice_configuration_mut(candidate, field_id);
            options
                .get_mut(&option_id)
                .expect("test Field must own the Option")
                .lifecycle = OptionLifecycle::Active;
            order.push(option_id);
        }
        TestMutation::ArchiveOptionOnly {
            field_id,
            option_id,
        } => test_archive_option(candidate, field_id, option_id),
        TestMutation::ArchiveOptionAndUnsetCurrent {
            field_id,
            option_id,
            unset_source_field_id,
        } => {
            test_archive_option(candidate, field_id, option_id);
            let unset = candidate.fields[&unset_source_field_id]
                .default_value
                .clone();
            test_field_mut(candidate, field_id).default_value = unset;
        }
        TestMutation::ReactivateTemplate => candidate.lifecycle = TemplateLifecycle::Active,
        TestMutation::RenameArchivedOption {
            field_id,
            option_id,
        } => {
            test_choice_configuration_mut(candidate, field_id)
                .1
                .get_mut(&option_id)
                .expect("test Field must own the Option")
                .label = "changed archived label".to_owned();
        }
        TestMutation::ClearFieldOrder => candidate.field_order.clear(),
        TestMutation::ClearOptionOrder(field_id) => {
            test_choice_configuration_mut(candidate, field_id).0.clear();
        }
        TestMutation::DuplicateOptionInField {
            source_field_id,
            target_field_id,
            option_id,
        } => {
            let option = candidate.fields[&source_field_id]
                .configuration
                .options()
                .and_then(|options| options.get(&option_id))
                .expect("test source Field must own the Option")
                .clone();
            let (order, options) = test_choice_configuration_mut(candidate, target_field_id);
            if option.lifecycle == OptionLifecycle::Active {
                order.push(option_id);
            }
            options.insert(option_id, option);
        }
        TestMutation::CorruptCurrentScalar { field_id, raw } => {
            test_field_mut(candidate, field_id)
                .default_value
                .corrupt_scalar_for_test(&raw);
        }
        TestMutation::CorruptCurrentMultiChoice {
            field_id,
            option_ids,
        } => {
            test_field_mut(candidate, field_id)
                .default_value
                .corrupt_multi_choice_for_test(option_ids);
        }
        TestMutation::AddFieldWithRevision {
            source_field_id,
            new_field_id,
            revision,
        } => {
            let mut field = candidate.fields[&source_field_id].clone();
            field.introduced_revision = revision;
            candidate.fields.insert(new_field_id, field);
            candidate.field_order.push(new_field_id);
        }
        TestMutation::AddInvalidInitialText {
            source_field_id,
            new_field_id,
        } => {
            let mut field = candidate.fields[&source_field_id].clone();
            field.introduced_revision = next_revision_for_new_field(candidate)?;
            field.initial_default_value = field.default_value.clone();
            field
                .initial_default_value
                .corrupt_scalar_for_test("line one\nline two");
            candidate.fields.insert(new_field_id, field);
            candidate.field_order.push(new_field_id);
        }
    }
    Ok(())
}

#[cfg(test)]
fn test_field_mut(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
) -> &mut super::FieldDefinition {
    candidate
        .fields
        .get_mut(&field_id)
        .expect("test fixture must contain the Field")
}

#[cfg(test)]
fn test_archive_option(candidate: &mut TemplateArtifact, field_id: FieldId, option_id: OptionId) {
    let (order, options) = test_choice_configuration_mut(candidate, field_id);
    order.retain(|id| *id != option_id);
    options
        .get_mut(&option_id)
        .expect("test Field must own the Option")
        .lifecycle = OptionLifecycle::Archived;
}

#[cfg(test)]
fn test_choice_configuration_mut(
    candidate: &mut TemplateArtifact,
    field_id: FieldId,
) -> (
    &mut Vec<OptionId>,
    &mut std::collections::BTreeMap<OptionId, ChoiceOption>,
) {
    match &mut test_field_mut(candidate, field_id).configuration.variant {
        FieldConfigurationVariant::SingleChoice {
            option_order,
            options,
        }
        | FieldConfigurationVariant::MultiChoice {
            option_order,
            options,
        } => (option_order, options),
        FieldConfigurationVariant::Group { .. }
        | FieldConfigurationVariant::SingleLineText
        | FieldConfigurationVariant::RichText
        | FieldConfigurationVariant::Number { .. }
        | FieldConfigurationVariant::Date
        | FieldConfigurationVariant::Time
        | FieldConfigurationVariant::Image
        | FieldConfigurationVariant::File
        | FieldConfigurationVariant::Url
        | FieldConfigurationVariant::Duration
        | FieldConfigurationVariant::Relation { .. }
        | FieldConfigurationVariant::DocumentLink => {
            panic!("test fixture Field must have choice configuration")
        }
    }
}

#[cfg(test)]
mod tests;
