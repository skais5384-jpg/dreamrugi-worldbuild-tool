//! 전체 Template 편집의 wire 계약. 원본 aggregate와 쓰기 권한은 이 DTO에 포함하지 않는다.
use super::dto::{ErrorDto, Id, ResultDto};
use crate::data::edit_recovery::model::{Draft, DraftField, Intent, Key};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TemplateBody {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sections: Vec<crate::data::artifact::SectionInput>,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub(crate) glossary_excluded: bool,
    pub(crate) presentation: Intent<String>,
    pub(crate) fields: Vec<DraftField>,
    pub(crate) composing: bool,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}
impl TemplateBody {
    pub(crate) fn recovery(&self, template: Option<String>) -> Draft {
        Draft::Template {
            sections: self.sections.clone(),
            template,
            name: self.name.clone(),
            glossary_excluded: self.glossary_excluded,
            presentation: self.presentation.clone(),
            fields: self.fields.clone(),
            composing: self.composing,
        }
    }
    pub(crate) fn from_recovery(draft: &Draft) -> Option<Self> {
        match draft {
            Draft::Template {
                sections,
                name,
                glossary_excluded,
                presentation,
                fields,
                composing,
                ..
            } => Some(Self {
                sections: sections.clone(),
                name: name.clone(),
                glossary_excluded: *glossary_excluded,
                presentation: presentation.clone(),
                fields: fields.clone(),
                composing: *composing,
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DraftAction {
    Checkpoint,
    Save,
    Deposit,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ReapplyIntent {
    ComponentTemplate,
    Change { change: String },
    Name,
    Presentation,
    FieldLabel { field: String },
    FieldRequired { field: String },
    FieldWritingGuide { field: String },
    FieldPresentation { field: String },
    FieldCardTitle { field: String },
    FieldDefault { field: String },
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoverySelection {
    pub(crate) snapshot: Id,
    pub(crate) key: Key,
    pub(crate) phase: &'static str,
    pub(crate) can_restore: bool,
    pub(crate) intents: Vec<ReapplyIntent>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReceiptDto {
    pub(crate) key: Key,
    pub(crate) deposit_id: String,
    pub(crate) digest: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftProblem {
    pub(crate) category: String,
    pub(crate) field: Option<String>,
    pub(crate) option: Option<String>,
    pub(crate) property: DraftProblemProperty,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DraftProblemProperty {
    Global,
    Default,
    Configuration,
    Option,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftStatus {
    /// owner/session은 현재 worker에만 유효하고 draftId는 파일의 논리적 초안 ID다.
    pub(crate) owner: Id,
    pub(crate) draft_id: String,
    pub(crate) project_fingerprint: String,
    pub(crate) artifact: String,
    pub(crate) generation: String,
    pub(crate) saved_generation: Option<String>,
    pub(crate) base_revision: String,
    pub(crate) source_digest: Option<String>,
    pub(crate) snapshot: Id,
    pub(crate) phase: &'static str,
    pub(crate) receipt: Option<ReceiptDto>,
    pub(crate) error: Option<ErrorDto>,
    pub(crate) problems: Vec<DraftProblem>,
    pub(crate) identities: std::collections::BTreeMap<String, String>,
    pub(crate) outcome: Option<Box<ResultDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) comparison: Option<Vec<super::backend::recovery_merge::Change>>,
    pub(crate) remaining_input: bool,
}

/// snapshot별로 고정한 projection을 UTF-8 조각으로 읽는다. 원 artifact JSON은 노출하지 않는다.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContentChunk {
    pub(crate) snapshot: Id,
    pub(crate) offset: String,
    pub(crate) next: Option<String>,
    pub(crate) text: String,
}
