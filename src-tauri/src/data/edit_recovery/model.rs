use super::error::{Category, RecoveryError, Stage};
use crate::data::{
    artifact,
    edit_input::{DocumentEdit, RichNode, TemplateEdit, ValueDto},
    json, utc_time,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_DRAFT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_LIST_ENTRIES: usize = 4096;
pub(crate) const MAX_LIST_READ_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn valid_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| !u.is_nil() && u.to_string() == s)
}
fn reject(category: Category) -> RecoveryError {
    RecoveryError::new(category, Stage::Validate)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Key {
    pub(crate) project_fingerprint: String,
    pub(crate) draft_id: String,
    #[serde(with = "decimal_generation")]
    pub(crate) generation: u64,
}
impl Key {
    pub(crate) fn validate(&self) -> Result<(), RecoveryError> {
        if !valid_digest(&self.project_fingerprint)
            || !valid_id(&self.draft_id)
            || self.generation == 0
        {
            return Err(reject(Category::InvalidId));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "intent",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Intent<T> {
    Keep,
    Set(T),
    Unset,
}

impl<T> Default for Intent<T> {
    fn default() -> Self {
        Self::Keep
    }
}

impl<T> Intent<T> {
    pub(crate) fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

/// 순서는 배열 자체의 순서다. 기존 Field의 제거는 archived로, 새 Field 제거는 부재로 표현한다.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DraftField {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archive_title: Option<Intent<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) writing_guide: Option<Intent<String>>,
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) configuration: DraftConfiguration,
    pub(crate) required: bool,
    pub(crate) presentation: Intent<String>,
    pub(crate) default: Intent<ValueDto>,
    pub(crate) archived: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub(crate) restore: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archive_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) archive_order: Vec<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DraftConfiguration {
    Group {
        #[serde(
            default,
            rename = "cardTitleField",
            skip_serializing_if = "Intent::is_keep"
        )]
        card_title_field: Intent<String>,
        members: Vec<DraftField>,
    },
    SingleLineText {},
    RichText {},
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum: Option<String>,
    },
    Date {},
    Time {},
    Image {},
    File {},
    Url {},
    Duration {},
    SingleChoice {
        options: Vec<DraftOption>,
    },
    MultiChoice {
        options: Vec<DraftOption>,
    },
    Relation {
        multiple: bool,
        #[serde(rename = "allowedTemplates")]
        allowed_templates: Vec<String>,
        #[serde(rename = "reciprocalNotice")]
        reciprocal_notice: bool,
    },
    DocumentLink {},
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DraftOption {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) archived: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub(crate) restore: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archive_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) archive_order: Vec<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DraftValue {
    pub(crate) field: String,
    pub(crate) value: Intent<ValueDto>,
}
/// 이 typed 초안은 보관만 가능하다. canonical 저장·복원 권한이나 OS IME 세션이 아니다.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Draft {
    AdmittedDocument {
        document: String,
        template: String,
        revision: String,
        edits: Vec<DocumentEdit>,
    },
    AdmittedComposite {
        document: String,
        template: String,
        revision: String,
        edit: TemplateEdit,
        edits: Vec<DocumentEdit>,
    },
    Template {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        sections: Vec<crate::data::artifact::SectionInput>,
        template: Option<String>,
        name: String,
        #[serde(default, skip_serializing_if = "bool_is_false")]
        glossary_excluded: bool,
        presentation: Intent<String>,
        fields: Vec<DraftField>,
        composing: bool,
    },
    Document {
        document: Option<String>,
        template: String,
        name: Intent<String>,
        #[serde(default, skip_serializing_if = "Intent::is_keep")]
        english_name: Intent<String>,
        #[serde(default, skip_serializing_if = "Intent::is_keep")]
        glossary_summary: Intent<String>,
        #[serde(default, skip_serializing_if = "Intent::is_keep")]
        glossary_excluded: Intent<bool>,
        fields: Vec<DraftValue>,
        composing: bool,
    },
}
impl Draft {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::AdmittedDocument { .. } => "admitted_document",
            Self::AdmittedComposite { .. } => "admitted_composite",
            Self::Template { .. } => "template",
            Self::Document { .. } => "document",
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Original {
    pub(crate) kind: OriginalKind,
    pub(crate) artifact_id: String,
    pub(crate) schema: u32,
    pub(crate) template_revision: u32,
    pub(crate) source_byte_length: u64,
    pub(crate) source_digest: String,
    // JSON 문자열로 감싸 unknown 숫자 원문과 소유 위치를 codec 그대로 남긴다.
    pub(crate) snapshot: String,
    pub(crate) snapshot_digest: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OriginalKind {
    Template,
    Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SaveState {
    Unknown,
    NotAttempted,
    NoWrite,
    NotApplied,
    RolledBack,
    Committed,
    Uncertain,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Attempt {
    #[serde(with = "decimal_generation")]
    pub(crate) submitted_generation: u64,
    pub(crate) operation_id: String,
    pub(crate) result: SaveState,
    pub(crate) candidate_digest: Option<String>,
    pub(crate) transaction_id: Option<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Envelope {
    pub(crate) key: Key,
    pub(crate) deposit_id: String,
    pub(crate) app_version: String,
    pub(crate) created_at_utc: String,
    pub(crate) originals: Vec<Original>,
    pub(crate) draft: Draft,
    pub(crate) attempt: Option<Attempt>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileV1 {
    recovery_schema_version: u32,
    payload_digest: String,
    envelope: Envelope,
}

/// 재시도는 이 불변 owner를 재사용한다. 변경한 generation은 새 freeze가 필요하다.
pub(crate) struct Deposit {
    envelope: Envelope,
    bytes: Vec<u8>,
    digest: String,
}
impl std::fmt::Debug for Deposit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deposit")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
impl Deposit {
    pub(crate) fn freeze(envelope: Envelope) -> Result<Self, RecoveryError> {
        validate(&envelope)?;
        let digest = digest(
            &serde_json::to_vec(&envelope)
                .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?,
        );
        let bytes = serde_json::to_vec(&FileV1 {
            recovery_schema_version: 5,
            payload_digest: digest.clone(),
            envelope: envelope.clone(),
        })
        .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(reject(Category::TooLarge));
        }
        strict(&bytes)?;
        Ok(Self {
            envelope,
            bytes,
            digest,
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, RecoveryError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(reject(Category::TooLarge));
        }
        let json = strict(bytes)?;
        match json
            .get("recoverySchemaVersion")
            .and_then(serde_json::Value::as_u64)
        {
            Some(1 | 2 | 3 | 4 | 5) => (),
            Some(_) => return Err(reject(Category::UnsupportedVersion)),
            None => return Err(reject(Category::Corrupt)),
        }
        if json["recoverySchemaVersion"]
            .as_u64()
            .is_some_and(|n| n < 4)
            && contains_group(&json["envelope"]["draft"])
        {
            return Err(reject(Category::UnsupportedVersion));
        }
        let kind = json
            .pointer("/envelope/draft/kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| reject(Category::Corrupt))?;
        if !matches!(
            kind,
            "admitted_document" | "admitted_composite" | "template" | "document"
        ) {
            return Err(reject(Category::UnsupportedKind));
        }
        if json["recoverySchemaVersion"]
            .as_u64()
            .is_some_and(|n| n < 3)
            && contains_media(&json["envelope"]["draft"])
        {
            return Err(reject(Category::UnsupportedVersion));
        }
        if json["recoverySchemaVersion"] == 1 {
            let draft = &json["envelope"]["draft"];
            let new_value = |v: &serde_json::Value| {
                v.pointer("/value/kind").and_then(serde_json::Value::as_str)
                    == Some("number_unknown")
            };
            let fields = draft.get("fields").and_then(serde_json::Value::as_array);
            if contains_new_number_variant(draft)
                || draft.get("sections").is_some()
                || fields.is_some_and(|fields| {
                    fields.iter().any(|f| {
                        f["configuration"].get("minimum").is_some()
                            || f["configuration"].get("maximum").is_some()
                            || new_value(&f["value"])
                            || new_value(&f["default"])
                    })
                })
            {
                return Err(reject(Category::UnsupportedVersion));
            }
        }
        let file: FileV1 = serde_json::from_slice(bytes)
            .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
        let mut deposit = Self::freeze(file.envelope)?;
        if file.payload_digest != deposit.digest {
            return Err(reject(Category::DigestMismatch));
        }
        deposit.bytes = bytes.to_vec();
        Ok(deposit)
    }
    pub(crate) fn envelope(&self) -> &Envelope {
        &self.envelope
    }
    pub(crate) fn key(&self) -> &Key {
        &self.envelope.key
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn payload_digest(&self) -> &str {
        &self.digest
    }
}
fn contains_new_number_variant(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            object.get("kind").and_then(serde_json::Value::as_str) == Some("number_unknown")
                || object.values().any(contains_new_number_variant)
        }
        serde_json::Value::Array(values) => values.iter().any(contains_new_number_variant),
        _ => false,
    }
}
fn strict(bytes: &[u8]) -> Result<serde_json::Value, RecoveryError> {
    json::parse_strict_json_object(bytes).map_err(|e| {
        let category = if e.category() == json::StrictJsonErrorCategory::NestingDepthExceeded {
            Category::TooDeep
        } else {
            Category::Corrupt
        };
        RecoveryError::caused(category, Stage::Validate, e)
    })
}

fn validate(e: &Envelope) -> Result<(), RecoveryError> {
    e.key.validate()?;
    validate_draft_depth(&e.draft)?;
    if !valid_id(&e.deposit_id)
        || !utc_time::is_utc_milliseconds(&e.created_at_utc)
        || e.app_version.is_empty()
        || e.app_version.len() > 64
        || e.originals.len() > 2
    {
        return Err(reject(Category::InvalidEnvelope));
    }
    let draft_bytes = serde_json::to_vec(&e.draft)
        .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
    if draft_bytes.len() > MAX_DRAFT_BYTES {
        return Err(reject(Category::TooLarge));
    }
    strict(&draft_bytes)?;
    if let Some(a) = &e.attempt {
        // 저장 S 진행 중 입력 S+1이 생겨도 원 attempt를 S+1로 위조하지 않는다.
        if a.submitted_generation == 0
            || a.submitted_generation > e.key.generation
            || !valid_id(&a.operation_id)
            || a.candidate_digest
                .as_ref()
                .is_some_and(|s| !valid_digest(s))
            || a.transaction_id
                .as_ref()
                .is_some_and(|s| crate::data::transaction::TransactionId::parse(s).is_err())
        {
            return Err(reject(Category::InvalidEnvelope));
        }
    }
    for (index, o) in e.originals.iter().enumerate() {
        if !valid_id(&o.artifact_id)
            || !valid_digest(&o.source_digest)
            || o.source_byte_length == 0
            || e.originals[..index]
                .iter()
                .any(|p| p.kind == o.kind && p.artifact_id == o.artifact_id)
        {
            return Err(reject(Category::InvalidEnvelope));
        }
        if o.snapshot.len() > MAX_SNAPSHOT_BYTES {
            return Err(reject(Category::TooLarge));
        }
        if digest(o.snapshot.as_bytes()) != o.snapshot_digest {
            return Err(reject(Category::DigestMismatch));
        }
        strict(o.snapshot.as_bytes())?;
        let (id, revision, schema) = match o.kind {
            OriginalKind::Template => {
                let a = artifact::decode_template(o.snapshot.as_bytes())
                    .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
                (
                    a.template_id().to_string(),
                    a.revision().get(),
                    a.schema_version().get(),
                )
            }
            OriginalKind::Document => {
                let a = artifact::decode_document(o.snapshot.as_bytes())
                    .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
                (
                    a.document_id().to_string(),
                    a.template_revision().get(),
                    a.schema_version().get(),
                )
            }
        };
        if id != o.artifact_id || revision != o.template_revision || schema != o.schema {
            return Err(reject(Category::InvalidEnvelope));
        }
    }
    let has = |kind, id: &str| {
        valid_id(id)
            && e.originals
                .iter()
                .any(|o| o.kind == kind && o.artifact_id == id)
    };
    let valid = match &e.draft {
        Draft::AdmittedDocument {
            document, template, ..
        }
        | Draft::AdmittedComposite {
            document, template, ..
        } => {
            e.originals.len() == 2
                && has(OriginalKind::Document, document)
                && has(OriginalKind::Template, template)
        }
        Draft::Template { template, .. } => match template {
            Some(id) => e.originals.len() == 1 && has(OriginalKind::Template, id),
            None => e.originals.is_empty(),
        },
        Draft::Document {
            document, template, ..
        } => {
            has(OriginalKind::Template, template)
                && match document {
                    Some(id) => e.originals.len() == 2 && has(OriginalKind::Document, id),
                    None => e.originals.len() == 1,
                }
        }
    };
    if !valid {
        return Err(reject(Category::InvalidEnvelope));
    }
    Ok(())
}

// JS number 왕복으로 generation을 반올림하지 않도록 v1 wire에서는 canonical decimal 문자열이다.
mod decimal_generation {
    use serde::{Deserialize, Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        let n = text
            .parse::<u64>()
            .map_err(|_| serde::de::Error::custom("invalid generation"))?;
        if n == 0 || n.to_string() != text {
            return Err(serde::de::Error::custom("invalid generation"));
        }
        Ok(n)
    }
}
fn validate_draft_depth(draft: &Draft) -> Result<(), RecoveryError> {
    let mut values = Vec::new();
    let mut edit = None;
    match draft {
        Draft::AdmittedDocument { edits, .. } | Draft::AdmittedComposite { edits, .. } => {
            for e in edits {
                if let DocumentEdit::Set { value, .. } = e {
                    values.push(value);
                }
            }
            if let Draft::AdmittedComposite { edit: e, .. } = draft {
                edit = Some(e);
            }
        }
        Draft::Template { fields, .. } => {
            for field in fields {
                if let Intent::Set(value) = &field.default {
                    values.push(value);
                }
            }
        }
        Draft::Document { fields, .. } => {
            for field in fields {
                if let Intent::Set(value) = &field.value {
                    values.push(value);
                }
            }
        }
    }
    if let Some(edit) = edit {
        match edit {
            TemplateEdit::CreateField { default, .. } => values.push(default),
            TemplateEdit::Default { value, .. } => values.push(value),
            TemplateEdit::ArchiveOption {
                repair: Some(value),
                ..
            } => values.push(value),
            _ => (),
        }
    }
    let mut leaves = Vec::new();
    for value in values {
        if let ValueDto::Group { instances } = value {
            for instance in instances {
                for cell in &instance.fields {
                    if let Intent::Set(v) = &cell.value {
                        if matches!(v, ValueDto::Group { .. }) {
                            return Err(reject(Category::Corrupt));
                        }
                        leaves.push(v);
                    }
                }
            }
        } else {
            leaves.push(value);
        }
    }
    for value in leaves {
        if let ValueDto::RichText { content } = value {
            let mut stack = vec![(content, 1)];
            while let Some((node, depth)) = stack.pop() {
                // JSON wrapper 깊이도 있으므로 입력 AST는 48 node depth로 먼저 제한한다.
                if depth > 48 {
                    return Err(reject(Category::TooDeep));
                }
                match node {
                    RichNode::Root { children }
                    | RichNode::Paragraph { children }
                    | RichNode::Heading { children, .. }
                    | RichNode::Blockquote { children }
                    | RichNode::BulletList { children }
                    | RichNode::OrderedList { children }
                    | RichNode::ListItem { children }
                    | RichNode::TaskList { children }
                    | RichNode::TaskItem { children, .. } => {
                        stack.extend(children.iter().map(|child| (child, depth + 1)));
                    }
                    RichNode::Text { .. } | RichNode::HardBreak {} => (),
                }
            }
        }
    }
    Ok(())
}

fn contains_media(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            matches!(
                m.get("kind").and_then(|v| v.as_str()),
                Some("image" | "file" | "url")
            ) || m.values().any(contains_media)
        }
        serde_json::Value::Array(a) => a.iter().any(contains_media),
        _ => false,
    }
}

fn contains_group(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            m.get("kind").and_then(|v| v.as_str()) == Some("group")
                || m.values().any(contains_group)
        }
        serde_json::Value::Array(a) => a.iter().any(contains_group),
        _ => false,
    }
}
