use std::fmt;

use crate::data::{
    artifact::{
        DocumentId, FieldId, FieldValue, OptionId, ReferenceId, RelationLink, RichTextDocument,
    },
    field_engine::rich_text::NormalizedRichText,
};

/// 저장 가능한 known payload만 받는다. persisted FieldValue나 extra map을 받는 생성자는 없다.
#[derive(Clone, PartialEq)]
pub(crate) struct DocumentValueEdit {
    pub(super) value: FieldValue,
}

impl DocumentValueEdit {
    pub(crate) fn unset() -> Self {
        Self {
            value: FieldValue::unset(),
        }
    }
    pub(crate) fn into_field_value(self) -> FieldValue {
        self.value
    }
    pub(crate) fn single_line_text(value: String) -> Self {
        Self {
            value: FieldValue::single_line_text(value),
        }
    }

    pub(crate) fn from_normalized_rich_text(value: NormalizedRichText) -> Self {
        Self {
            value: match value {
                NormalizedRichText::Unset => FieldValue::unset(),
                NormalizedRichText::RichText(value) => {
                    FieldValue::from_rich_text(RichTextDocument::from_canonical(value))
                }
            },
        }
    }

    pub(crate) fn number_unknown() -> Self {
        Self {
            value: FieldValue::number_unknown(),
        }
    }

    pub(crate) fn number(value: String) -> Self {
        Self {
            value: FieldValue::number(value),
        }
    }

    pub(crate) fn date(value: String) -> Self {
        Self {
            value: FieldValue::date(value),
        }
    }

    pub(crate) fn image(value: Vec<String>) -> Self {
        Self {
            value: FieldValue::image(value),
        }
    }

    pub(crate) fn file(value: Vec<String>) -> Self {
        Self {
            value: FieldValue::file(value),
        }
    }

    pub(crate) fn url(value: String) -> Self {
        Self {
            value: FieldValue::url(value),
        }
    }

    pub(crate) fn time(value: String) -> Self {
        Self {
            value: FieldValue::time(value),
        }
    }

    pub(crate) fn duration(milliseconds: String) -> Self {
        Self {
            value: FieldValue::duration(milliseconds),
        }
    }

    pub(crate) fn single_choice(option_id: OptionId) -> Self {
        Self {
            value: FieldValue::from_single_choice(option_id),
        }
    }

    /// 저장된 값의 canonical 규칙을 그대로 검사한다. empty/중복/역순을 자동 수정하지 않는다.
    pub(crate) fn multi_choice(option_ids: Vec<OptionId>) -> Self {
        Self {
            value: FieldValue::from_multi_choice(option_ids),
        }
    }

    pub(crate) fn relation(links: Vec<(ReferenceId, DocumentId, bool, String)>) -> Self {
        Self {
            value: FieldValue::relation(
                links
                    .into_iter()
                    .map(|(id, document, one_way, name)| {
                        RelationLink::named(id, document, one_way, name)
                    })
                    .collect(),
            ),
        }
    }

    pub(crate) fn document_link(documents: Vec<DocumentId>) -> Self {
        Self {
            value: FieldValue::document_link(documents),
        }
    }
}

impl fmt::Debug for DocumentValueEdit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentValueEdit")
            .field("kind", &self.value.kind())
            .field("payload_redacted", &true)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum DocumentEdit {
    Rename(String),
    SetEnglishName(String),
    SetGlossarySummary(String),
    SetGlossaryExcluded(bool),
    SetValue(FieldId, DocumentValueEdit),
    SetGroup(FieldId, Vec<crate::data::artifact::group::InstanceInput>),
    Unset(FieldId),
}

impl fmt::Debug for DocumentEdit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetGroup(id, _) => formatter
                .debug_tuple("DocumentEdit::SetGroup")
                .field(id)
                .finish(),
            Self::Rename(_) => formatter.write_str("DocumentEdit::Rename(<redacted>)"),
            Self::SetEnglishName(_) => {
                formatter.write_str("DocumentEdit::SetEnglishName(<redacted>)")
            }
            Self::SetGlossarySummary(_) => {
                formatter.write_str("DocumentEdit::SetGlossarySummary(<redacted>)")
            }
            Self::SetGlossaryExcluded(value) => formatter
                .debug_tuple("DocumentEdit::SetGlossaryExcluded")
                .field(value)
                .finish(),
            Self::SetValue(field_id, _) => formatter
                .debug_tuple("DocumentEdit::SetValue")
                .field(field_id)
                .field(&"<redacted>")
                .finish(),
            Self::Unset(field_id) => formatter
                .debug_tuple("DocumentEdit::Unset")
                .field(field_id)
                .finish(),
        }
    }
}

/// 목록을 map으로 먼저 축소하지 않아 중복/상충 의도를 검증 전에 잃지 않는다.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct DocumentEditSet {
    pub(super) edits: Vec<DocumentEdit>,
}

impl DocumentEditSet {
    pub(crate) fn new(edits: Vec<DocumentEdit>) -> Self {
        Self { edits }
    }
}

impl fmt::Debug for DocumentEditSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DocumentEditSet")
            .field("edit_count", &self.edits.len())
            .finish()
    }
}
