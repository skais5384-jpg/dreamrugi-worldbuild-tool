use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::id::{DocumentId, OptionId, ReferenceId};
use super::{ArtifactScalarValueLocation, ArtifactValidationError};
use crate::data::field_engine::{
    rich_text::{
        rich_text_contains_unknown_storage_data, validate_persistent_rich_text, CanonicalRichText,
        RichTextValidationError,
    },
    validation::{
        validate_standalone_document_value, FieldKind, FieldValidationOutcome, FieldValueView,
        PersistentRichTextValue,
    },
};
use crate::data::json::{
    is_reserved_json_object_key, validate_json_object_for_storage, validate_json_value_for_storage,
    JsonStorageError, JsonStorageErrorCategory,
};

pub(crate) const RICH_TEXT_SCHEMA_VERSION: u32 =
    crate::data::field_engine::rich_text::RICH_TEXT_SCHEMA_VERSION;
pub(super) type ExtraFields = BTreeMap<String, Value>;

const FIELD_VALUE_KNOWN_KEYS: &[&str] = &[
    "kind",
    "value",
    "document",
    "milliseconds",
    "optionId",
    "optionIds",
    "links",
    "documentIds",
];
const RICH_TEXT_KNOWN_KEYS: &[&str] = &["schemaVersion", "content"];

pub(super) fn validate_extra_keys(
    extra: &ExtraFields,
    reserved: &[&str],
) -> Result<(), ArtifactValidationError> {
    if extra
        .keys()
        .any(|key| reserved.contains(&key.as_str()) || is_reserved_json_object_key(key))
    {
        return Err(ArtifactValidationError::reserved_key());
    }
    for value in extra.values() {
        validate_json_value_for_storage(value).map_err(map_json_storage_error)?;
    }
    Ok(())
}

pub(super) fn map_json_storage_error(error: JsonStorageError) -> ArtifactValidationError {
    match error.category() {
        JsonStorageErrorCategory::NestingDepthExceeded => {
            ArtifactValidationError::json_nesting_depth()
        }
        JsonStorageErrorCategory::ReservedObjectKey => ArtifactValidationError::reserved_key(),
        JsonStorageErrorCategory::NonFiniteFloat
        | JsonStorageErrorCategory::SerializationFailure => {
            ArtifactValidationError::invalid_json_value()
        }
    }
}

pub(super) fn validate_exact_active_order<T: Ord + Copy>(
    order: &[T],
    active: impl Iterator<Item = T>,
    mismatch: fn() -> ArtifactValidationError,
) -> Result<(), ArtifactValidationError> {
    let ordered: BTreeSet<_> = order.iter().copied().collect();
    if ordered.len() != order.len() || ordered != active.collect() {
        return Err(mismatch());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum FieldKindWire {
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

impl From<FieldKindWire> for FieldKind {
    fn from(value: FieldKindWire) -> Self {
        match value {
            FieldKindWire::Group => Self::Group,
            FieldKindWire::SingleLineText => Self::SingleLineText,
            FieldKindWire::RichText => Self::RichText,
            FieldKindWire::Number => Self::Number,
            FieldKindWire::Date => Self::Date,
            FieldKindWire::Time => Self::Time,
            FieldKindWire::Image => Self::Image,
            FieldKindWire::File => Self::File,
            FieldKindWire::Url => Self::Url,
            FieldKindWire::Duration => Self::Duration,
            FieldKindWire::SingleChoice => Self::SingleChoice,
            FieldKindWire::MultiChoice => Self::MultiChoice,
            FieldKindWire::Relation => Self::Relation,
            FieldKindWire::DocumentLink => Self::DocumentLink,
        }
    }
}

impl From<FieldKind> for FieldKindWire {
    fn from(value: FieldKind) -> Self {
        match value {
            FieldKind::Group => Self::Group,
            FieldKind::SingleLineText => Self::SingleLineText,
            FieldKind::RichText => Self::RichText,
            FieldKind::Number => Self::Number,
            FieldKind::Date => Self::Date,
            FieldKind::Time => Self::Time,
            FieldKind::Image => Self::Image,
            FieldKind::File => Self::File,
            FieldKind::Url => Self::Url,
            FieldKind::Duration => Self::Duration,
            FieldKind::SingleChoice => Self::SingleChoice,
            FieldKind::MultiChoice => Self::MultiChoice,
            FieldKind::Relation => Self::Relation,
            FieldKind::DocumentLink => Self::DocumentLink,
        }
    }
}

/// 관계 예외는 표시 이름이나 배열 위치가 아니라 연결 ID와 함께 이동한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationLink {
    id: ReferenceId,
    document: DocumentId,
    one_way: bool,
    name: String,
}

impl RelationLink {
    pub(crate) const fn new(id: ReferenceId, document: DocumentId, one_way: bool) -> Self {
        Self {
            id,
            document,
            one_way,
            name: String::new(),
        }
    }

    /// 관계 이름은 출처 문서가 상대 문서를 부르는 연결별 표현이다.
    pub(crate) const fn named(
        id: ReferenceId,
        document: DocumentId,
        one_way: bool,
        name: String,
    ) -> Self {
        Self {
            id,
            document,
            one_way,
            name,
        }
    }

    pub(crate) const fn id(&self) -> ReferenceId {
        self.id
    }

    pub(crate) const fn document(&self) -> DocumentId {
        self.document
    }

    pub(crate) const fn one_way(&self) -> bool {
        self.one_way
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RelationLinkWire {
    id: ReferenceId,
    document: DocumentId,
    #[serde(default)]
    one_way: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
}

impl From<RelationLinkWire> for RelationLink {
    fn from(value: RelationLinkWire) -> Self {
        Self::named(value.id, value.document, value.one_way, value.name)
    }
}

impl From<&RelationLink> for RelationLinkWire {
    fn from(value: &RelationLink) -> Self {
        Self {
            id: value.id,
            document: value.document,
            one_way: value.one_way,
            name: value.name.clone(),
        }
    }
}

/// 검증된 content도 편집 가능한 raw JSON이나 mutable AST로 노출하지 않는 envelope다.
#[derive(Clone, PartialEq)]
pub(crate) struct RichTextDocument {
    schema_version: u32,
    content: Map<String, Value>,
    extra: ExtraFields,
}

impl fmt::Debug for RichTextDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RichTextDocument")
            .field("schema_version", &self.schema_version)
            .field("content_redacted", &true)
            .field("has_extra", &!self.extra.is_empty())
            .finish()
    }
}

impl RichTextDocument {
    /// Field Engine이 검증하고 canonicalize한 소유 값만 artifact envelope로 옮긴다.
    /// schema와 envelope extra는 caller 입력이 아니라 artifact 계층이 결정한다.
    pub(super) fn from_canonical(canonical: CanonicalRichText) -> Self {
        Self {
            schema_version: RICH_TEXT_SCHEMA_VERSION,
            content: canonical.content().clone(),
            extra: ExtraFields::new(),
        }
    }

    pub(crate) const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// UI projection은 이 불변 내용을 읽되 known semantic member만 복사해야 한다.
    pub(crate) fn content_for_projection(&self) -> &Map<String, Value> {
        &self.content
    }

    fn validate_storage_structure(&self) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, RICH_TEXT_KNOWN_KEYS)?;
        validate_json_object_for_storage(&self.content).map_err(map_json_storage_error)
    }

    /// envelope와 AST 어느 위치든 미래 metadata가 있으면 위치 대응을 추측하지 않는다.
    pub(crate) fn contains_unknown_storage_data(&self) -> bool {
        !self.extra.is_empty() || rich_text_contains_unknown_storage_data(&self.content)
    }
}

impl PersistentRichTextValue for RichTextDocument {
    fn validate_persistent(&self) -> Result<(), RichTextValidationError> {
        validate_persistent_rich_text(self.schema_version, &self.content)
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RichTextDocumentWire {
    schema_version: u32,
    content: Map<String, Value>,
    #[serde(flatten)]
    extra: ExtraFields,
}

impl From<RichTextDocumentWire> for RichTextDocument {
    fn from(wire: RichTextDocumentWire) -> Self {
        Self {
            schema_version: wire.schema_version,
            content: wire.content,
            extra: wire.extra,
        }
    }
}

impl From<&RichTextDocument> for RichTextDocumentWire {
    fn from(document: &RichTextDocument) -> Self {
        Self {
            schema_version: document.schema_version,
            content: document.content.clone(),
            extra: document.extra.clone(),
        }
    }
}

/// Production 값은 variant payload와 unknown field를 private하게 감싼다.
#[derive(Clone, PartialEq)]
pub(crate) struct FieldValue {
    variant: FieldValueVariant,
    extra: ExtraFields,
}

#[derive(Clone, PartialEq)]
enum FieldValueVariant {
    Group(super::group::GroupValue),
    Unset,
    Text(String),
    RichText(RichTextDocument),
    Number(String),
    NumberUnknown,
    Date(String),
    Time(String),
    Image(Vec<String>),
    File(Vec<String>),
    Url(String),
    Duration(String),
    SingleChoice(OptionId),
    MultiChoice(Vec<OptionId>),
    Relation(Vec<RelationLink>),
    DocumentLink(Vec<DocumentId>),
}

impl fmt::Debug for FieldValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FieldValue")
            .field("kind", &self.kind())
            .field("is_unset", &self.is_unset())
            .field("payload_redacted", &!self.is_unset())
            .field("has_extra", &!self.extra.is_empty())
            .finish()
    }
}

impl FieldValue {
    pub(crate) fn from_group(value: super::group::GroupValue) -> Self {
        Self {
            variant: FieldValueVariant::Group(value),
            extra: ExtraFields::new(),
        }
    }
    pub(crate) fn group(&self) -> Option<&super::group::GroupValue> {
        if let FieldValueVariant::Group(v) = &self.variant {
            Some(v)
        } else {
            None
        }
    }
    /// 편집 command가 canonical empty를 별도 payload로 발명하지 않도록 unset을 직접 만든다.
    pub(super) fn unset() -> Self {
        Self {
            variant: FieldValueVariant::Unset,
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn single_line_text(value: String) -> Self {
        Self {
            variant: FieldValueVariant::Text(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn from_rich_text(document: RichTextDocument) -> Self {
        Self {
            variant: FieldValueVariant::RichText(document),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn number_unknown() -> Self {
        Self {
            variant: FieldValueVariant::NumberUnknown,
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn number(value: String) -> Self {
        Self {
            variant: FieldValueVariant::Number(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn date(value: String) -> Self {
        Self {
            variant: FieldValueVariant::Date(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn image(value: Vec<String>) -> Self {
        Self {
            variant: FieldValueVariant::Image(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn file(value: Vec<String>) -> Self {
        Self {
            variant: FieldValueVariant::File(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn url(value: String) -> Self {
        Self {
            variant: FieldValueVariant::Url(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn time(value: String) -> Self {
        Self {
            variant: FieldValueVariant::Time(value),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn duration(milliseconds: String) -> Self {
        Self {
            variant: FieldValueVariant::Duration(milliseconds),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn from_single_choice(option_id: OptionId) -> Self {
        Self {
            variant: FieldValueVariant::SingleChoice(option_id),
            extra: ExtraFields::new(),
        }
    }

    /// 입력 순서를 그대로 보존하고 최종 Field Engine이 canonical sorted/unique/non-empty를 검사한다.
    pub(super) fn from_multi_choice(option_ids: Vec<OptionId>) -> Self {
        Self {
            variant: FieldValueVariant::MultiChoice(option_ids),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn relation(links: Vec<RelationLink>) -> Self {
        Self {
            variant: FieldValueVariant::Relation(links),
            extra: ExtraFields::new(),
        }
    }

    pub(super) fn document_link(documents: Vec<DocumentId>) -> Self {
        Self {
            variant: FieldValueVariant::DocumentLink(documents),
            extra: ExtraFields::new(),
        }
    }

    pub(crate) fn is_unset(&self) -> bool {
        matches!(self.variant, FieldValueVariant::Unset)
    }

    /// unset은 어느 Field에서도 허용되므로 대응 Field kind가 없다.
    pub(crate) fn kind(&self) -> Option<FieldKind> {
        match &self.variant {
            FieldValueVariant::Group(_) => Some(FieldKind::Group),
            FieldValueVariant::Unset => None,
            FieldValueVariant::Text(_) => Some(FieldKind::SingleLineText),
            FieldValueVariant::RichText(_) => Some(FieldKind::RichText),
            FieldValueVariant::Number(_) | FieldValueVariant::NumberUnknown => {
                Some(FieldKind::Number)
            }
            FieldValueVariant::Date(_) => Some(FieldKind::Date),
            FieldValueVariant::Time(_) => Some(FieldKind::Time),
            FieldValueVariant::Image(_) => Some(FieldKind::Image),
            FieldValueVariant::File(_) => Some(FieldKind::File),
            FieldValueVariant::Url(_) => Some(FieldKind::Url),
            FieldValueVariant::Duration(_) => Some(FieldKind::Duration),
            FieldValueVariant::SingleChoice(_) => Some(FieldKind::SingleChoice),
            FieldValueVariant::MultiChoice(_) => Some(FieldKind::MultiChoice),
            FieldValueVariant::Relation(_) => Some(FieldKind::Relation),
            FieldValueVariant::DocumentLink(_) => Some(FieldKind::DocumentLink),
        }
    }

    pub(crate) fn text(&self) -> Option<&str> {
        match &self.variant {
            FieldValueVariant::Text(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn rich_text(&self) -> Option<&RichTextDocument> {
        match &self.variant {
            FieldValueVariant::RichText(document) => Some(document),
            _ => None,
        }
    }

    pub(crate) fn single_choice(&self) -> Option<OptionId> {
        match &self.variant {
            FieldValueVariant::SingleChoice(option_id) => Some(*option_id),
            _ => None,
        }
    }

    pub(crate) fn multi_choice(&self) -> Option<&[OptionId]> {
        match &self.variant {
            FieldValueVariant::MultiChoice(option_ids) => Some(option_ids),
            _ => None,
        }
    }

    pub(crate) fn relations(&self) -> Option<&[RelationLink]> {
        match &self.variant {
            FieldValueVariant::Relation(links) => Some(links),
            _ => None,
        }
    }

    pub(crate) fn document_links(&self) -> Option<&[DocumentId]> {
        match &self.variant {
            FieldValueVariant::DocumentLink(documents) => Some(documents),
            _ => None,
        }
    }

    pub(crate) fn has_named_relation(&self) -> bool {
        self.relations()
            .is_some_and(|links| links.iter().any(|link| !link.name().is_empty()))
            || self.group().is_some_and(|group| {
                group
                    .instances
                    .values()
                    .any(|instance| instance.values.values().any(FieldValue::has_named_relation))
            })
    }

    pub(super) fn matches_kind_or_unset(&self, kind: FieldKind) -> bool {
        self.is_unset() || self.kind() == Some(kind)
    }

    // 읽기 DTO도 같은 known-value view만 사용하며 extra/provenance를 편집 입력으로 내보내지 않는다.
    pub(crate) fn validation_view(&self) -> FieldValueView<'_> {
        match &self.variant {
            FieldValueVariant::Group(v) => FieldValueView::Group(v),
            FieldValueVariant::Unset => FieldValueView::Unset,
            FieldValueVariant::Text(value) => FieldValueView::SingleLineText(value),
            FieldValueVariant::RichText(document) => FieldValueView::RichText(document),
            FieldValueVariant::NumberUnknown => FieldValueView::NumberUnknown,
            FieldValueVariant::Number(value) => FieldValueView::Number(value),
            FieldValueVariant::Date(value) => FieldValueView::Date(value),
            FieldValueVariant::Time(value) => FieldValueView::Time(value),
            FieldValueVariant::Image(value) => FieldValueView::Image(value),
            FieldValueVariant::File(value) => FieldValueView::File(value),
            FieldValueVariant::Url(value) => FieldValueView::Url(value),
            FieldValueVariant::Duration(value) => FieldValueView::Duration(value),
            FieldValueVariant::SingleChoice(option_id) => FieldValueView::SingleChoice(*option_id),
            FieldValueVariant::MultiChoice(option_ids) => FieldValueView::MultiChoice(option_ids),
            FieldValueVariant::Relation(links) => FieldValueView::Relation(links),
            FieldValueVariant::DocumentLink(documents) => FieldValueView::DocumentLink(documents),
        }
    }

    pub(super) fn validate_storage_structure(&self) -> Result<(), ArtifactValidationError> {
        validate_extra_keys(&self.extra, FIELD_VALUE_KNOWN_KEYS)?;
        if let FieldValueVariant::RichText(document) = &self.variant {
            document.validate_storage_structure()?;
        }
        Ok(())
    }

    /// 알려진 payload를 바꾸는 명령도 모든 wire 계층의 알 수 없는 member를 잃지 않게 한다.
    ///
    /// rich-text metadata를 node 사이에 대응시키는 것은 미래 의미를 추측하게 된다. 어느 쪽이든
    /// unknown data가 있으면 document 전체가 같을 때만 허용하고, metadata가 전혀 없으면 정상적인
    /// known content 변경을 막지 않는다.
    pub(super) fn has_same_storage_extras(&self, other: &Self) -> bool {
        if self.extra != other.extra {
            return false;
        }
        match (&self.variant, &other.variant) {
            (FieldValueVariant::RichText(left), FieldValueVariant::RichText(right)) => {
                // FieldValue envelope extra는 위에서 exact equality를 확인했으므로 안전하게
                // 운반할 수 있다. 위치 대응을 추측할 수 없는 document/AST metadata만
                // rich-text subtree 전체 equality를 요구한다.
                let has_unknown =
                    left.contains_unknown_storage_data() || right.contains_unknown_storage_data();
                !has_unknown || self == other
            }
            (FieldValueVariant::RichText(document), FieldValueVariant::Unset)
            | (FieldValueVariant::Unset, FieldValueVariant::RichText(document)) => {
                // 위에서 동일성을 확인한 outer extra는 unset에도 같은 위치로 운반한다.
                // 사라지거나 새로 주입될 document/AST 내부 metadata만 이 전이를 막는다.
                !document.contains_unknown_storage_data()
            }
            (FieldValueVariant::RichText(document), _)
            | (_, FieldValueVariant::RichText(document)) => {
                self.extra.is_empty() && !document.contains_unknown_storage_data()
            }
            _ => true,
        }
    }

    pub(crate) fn has_same_outer_extras(&self, original: &Self) -> bool {
        self.extra == original.extra
    }
    pub(crate) fn has_unknown_outer_data(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Recovery may use a current source only if every opaque member still has
    /// the same identity-bound custody. Known values may differ independently.
    pub(crate) fn preserves_unknown_from(&self, original: &Self) -> bool {
        if !original.has_same_storage_extras(self) {
            return false;
        }
        match (&original.variant, &self.variant) {
            (FieldValueVariant::Group(old), FieldValueVariant::Group(current)) => old
                .instances
                .iter()
                .filter(|(_, card)| card.contains_unknown_storage_data())
                .all(|(id, card)| {
                    current
                        .instances
                        .get(id)
                        .is_some_and(|now| now.preserves_unknown_from(card))
                }),
            (FieldValueVariant::Group(old), _) => !old.contains_unknown_storage_data(),
            _ => true,
        }
    }

    /// 새 Field draft는 과거 wire에서 복사한 미래 metadata를 새 ID 아래에 주입할 수 없다.
    pub(crate) fn contains_unknown_storage_data(&self) -> bool {
        !self.extra.is_empty()
            || match &self.variant {
                FieldValueVariant::Group(v) => v.contains_unknown_storage_data(),
                FieldValueVariant::RichText(document) => document.contains_unknown_storage_data(),
                FieldValueVariant::Unset
                | FieldValueVariant::Text(_)
                | FieldValueVariant::Number(_)
                | FieldValueVariant::NumberUnknown
                | FieldValueVariant::Date(_)
                | FieldValueVariant::Image(_)
                | FieldValueVariant::File(_)
                | FieldValueVariant::Url(_)
                | FieldValueVariant::Time(_)
                | FieldValueVariant::Duration(_)
                | FieldValueVariant::SingleChoice(_)
                | FieldValueVariant::MultiChoice(_)
                | FieldValueVariant::Relation(_)
                | FieldValueVariant::DocumentLink(_) => false,
            }
    }

    /// known current payload 교체는 FieldValue envelope의 미래 member를 그대로 운반한다.
    /// rich-text 내부 metadata는 이 방식으로 대응시키지 않으며 historical invariant가 계속 막는다.
    pub(super) fn preserve_outer_storage_extra_from(mut self, source: &Self) -> Self {
        self.extra.clone_from(&source.extra);
        self
    }

    pub(super) fn validate_structure(
        &self,
        scalar_location: ArtifactScalarValueLocation,
    ) -> Result<(), ArtifactValidationError> {
        self.validate_storage_structure()?;
        let outcome =
            validate_standalone_document_value(self.validation_view()).map_err(|error| {
                ArtifactValidationError::invalid_field_value(error, scalar_location)
            })?;
        match outcome {
            FieldValidationOutcome::Valid => Ok(()),
            // Standalone context는 진단을 만들지 않는다. 정책이 바뀌면 codec이 진단을
            // 버리고 성공하지 않도록 fail-closed한다.
            FieldValidationOutcome::ValidWithDiagnostics(_) => {
                Err(ArtifactValidationError::field_value_kind_mismatch())
            }
        }
    }

    #[cfg(test)]
    pub(super) fn corrupt_scalar_for_test(&mut self, raw: &str) {
        let value = match &mut self.variant {
            FieldValueVariant::Text(value)
            | FieldValueVariant::Number(value)
            | FieldValueVariant::Date(value)
            | FieldValueVariant::Time(value)
            | FieldValueVariant::Duration(value) => value,
            _ => panic!("test fixture must contain a scalar field value"),
        };
        *value = raw.to_owned();
    }

    #[cfg(test)]
    pub(super) fn corrupt_multi_choice_for_test(&mut self, option_ids: Vec<OptionId>) {
        let FieldValueVariant::MultiChoice(value) = &mut self.variant else {
            panic!("test fixture must contain a multi-choice field value");
        };
        *value = option_ids;
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_content_for_test(&mut self, reserved_key: &str) {
        self.corrupt_rich_text_value_for_test(
            "testOnlyCorruption",
            Value::Object(Map::from_iter([(
                reserved_key.to_owned(),
                Value::String("test-only payload".to_owned()),
            )])),
        );
    }

    #[cfg(test)]
    pub(super) fn corrupt_rich_text_value_for_test(&mut self, key: &str, value: Value) {
        let FieldValueVariant::RichText(document) = &mut self.variant else {
            panic!("test fixture must contain a rich-text field value");
        };
        // Production에는 mutable AST accessor를 만들지 않고 encode 재검증만 시험한다.
        document.content.insert(key.to_owned(), value);
    }

    #[cfg(test)]
    pub(super) fn replace_rich_text_content_for_test(&mut self, content: Map<String, Value>) {
        let FieldValueVariant::RichText(document) = &mut self.variant else {
            panic!("test fixture must contain a rich-text field value");
        };
        document.content = content;
    }
}

/// Serde는 artifact 내부 private wire enum에만 존재한다.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum FieldValueWire {
    Group {
        #[serde(flatten)]
        value: super::group::GroupValueWire,
    },
    Unset {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Text {
        value: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    RichText {
        document: RichTextDocumentWire,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    NumberUnknown {
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Number {
        value: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Date {
        value: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Image {
        value: Vec<String>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    File {
        value: Vec<String>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Url {
        value: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Time {
        value: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Duration {
        milliseconds: String,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    SingleChoice {
        option_id: OptionId,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    MultiChoice {
        option_ids: Vec<OptionId>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    Relation {
        links: Vec<RelationLinkWire>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
    DocumentLink {
        document_ids: Vec<DocumentId>,
        #[serde(flatten)]
        extra: ExtraFields,
    },
}

impl From<FieldValueWire> for FieldValue {
    fn from(wire: FieldValueWire) -> Self {
        let (variant, extra) = match wire {
            FieldValueWire::Group { value } => {
                let (v, extra) = value.into_value();
                (FieldValueVariant::Group(v), extra)
            }
            FieldValueWire::Unset { extra } => (FieldValueVariant::Unset, extra),
            FieldValueWire::Text { value, extra } => (FieldValueVariant::Text(value), extra),
            FieldValueWire::RichText { document, extra } => (
                FieldValueVariant::RichText(RichTextDocument::from(document)),
                extra,
            ),
            FieldValueWire::NumberUnknown { extra } => (FieldValueVariant::NumberUnknown, extra),
            FieldValueWire::Number { value, extra } => (FieldValueVariant::Number(value), extra),
            FieldValueWire::Date { value, extra } => (FieldValueVariant::Date(value), extra),
            FieldValueWire::Time { value, extra } => (FieldValueVariant::Time(value), extra),
            FieldValueWire::Image { value, extra } => (FieldValueVariant::Image(value), extra),
            FieldValueWire::File { value, extra } => (FieldValueVariant::File(value), extra),
            FieldValueWire::Url { value, extra } => (FieldValueVariant::Url(value), extra),
            FieldValueWire::Duration {
                milliseconds,
                extra,
            } => (FieldValueVariant::Duration(milliseconds), extra),
            FieldValueWire::SingleChoice { option_id, extra } => {
                (FieldValueVariant::SingleChoice(option_id), extra)
            }
            FieldValueWire::MultiChoice { option_ids, extra } => {
                (FieldValueVariant::MultiChoice(option_ids), extra)
            }
            FieldValueWire::Relation { links, extra } => (
                FieldValueVariant::Relation(links.into_iter().map(Into::into).collect()),
                extra,
            ),
            FieldValueWire::DocumentLink {
                document_ids,
                extra,
            } => (FieldValueVariant::DocumentLink(document_ids), extra),
        };
        Self { variant, extra }
    }
}

impl From<&FieldValue> for FieldValueWire {
    fn from(value: &FieldValue) -> Self {
        let extra = value.extra.clone();
        match &value.variant {
            FieldValueVariant::Group(v) => Self::Group {
                value: super::group::GroupValueWire::from_value(v, extra),
            },
            FieldValueVariant::Unset => Self::Unset { extra },
            FieldValueVariant::Text(value) => Self::Text {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::RichText(document) => Self::RichText {
                document: RichTextDocumentWire::from(document),
                extra,
            },
            FieldValueVariant::NumberUnknown => Self::NumberUnknown { extra },
            FieldValueVariant::Number(value) => Self::Number {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::Date(value) => Self::Date {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::Image(value) => Self::Image {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::File(value) => Self::File {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::Url(value) => Self::Url {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::Time(value) => Self::Time {
                value: value.clone(),
                extra,
            },
            FieldValueVariant::Duration(milliseconds) => Self::Duration {
                milliseconds: milliseconds.clone(),
                extra,
            },
            FieldValueVariant::SingleChoice(option_id) => Self::SingleChoice {
                option_id: *option_id,
                extra,
            },
            FieldValueVariant::MultiChoice(option_ids) => Self::MultiChoice {
                option_ids: option_ids.clone(),
                extra,
            },
            FieldValueVariant::Relation(links) => Self::Relation {
                links: links.iter().map(Into::into).collect(),
                extra,
            },
            FieldValueVariant::DocumentLink(document_ids) => Self::DocumentLink {
                document_ids: document_ids.clone(),
                extra,
            },
        }
    }
}
