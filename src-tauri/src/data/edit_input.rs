//! IPC와 복구가 공유하는 명시적 입력 의도. 문자열의 의미 검증은 저장 시점에 수행한다.
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ValueDto {
    Group {
        instances: Vec<super::artifact::group::InstanceDraft>,
    },
    Unset {},
    SingleLineText {
        value: String,
    },
    Number {
        value: String,
    },
    NumberUnknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_raw: Option<String>,
    },
    Date {
        value: String,
    },
    Time {
        value: String,
    },
    Image {
        value: Vec<String>,
    },
    File {
        value: Vec<String>,
    },
    Url {
        value: String,
    },
    Duration {
        milliseconds: String,
    },
    SingleChoice {
        option: String,
    },
    MultiChoice {
        options: Vec<String>,
    },
    Relation {
        links: Vec<RelationLinkDto>,
    },
    DocumentLink {
        documents: Vec<String>,
    },
    RichText {
        content: RichNode,
    },
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RelationLinkDto {
    pub(crate) id: String,
    pub(crate) document: String,
    #[serde(default)]
    pub(crate) one_way: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) name: String,
}
/// 저장된 arbitrary AST 대신 지원하는 semantic member만 표현한다.
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum RichNode {
    Root {
        children: Vec<RichNode>,
    },
    Paragraph {
        children: Vec<RichNode>,
    },
    Heading {
        level: u8,
        children: Vec<RichNode>,
    },
    Text {
        text: String,
        #[serde(default)]
        marks: Vec<Mark>,
    },
    HardBreak {},
    Blockquote {
        children: Vec<RichNode>,
    },
    BulletList {
        children: Vec<RichNode>,
    },
    OrderedList {
        children: Vec<RichNode>,
    },
    ListItem {
        children: Vec<RichNode>,
    },
    TaskList {
        children: Vec<RichNode>,
    },
    TaskItem {
        checked: bool,
        children: Vec<RichNode>,
    },
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Mark {
    Bold,
    Italic,
    Underline,
    Strikethrough,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DocumentEdit {
    Rename { name: String },
    EnglishName { value: String },
    GlossarySummary { value: String },
    GlossaryExcluded { excluded: bool },
    Set { field: String, value: ValueDto },
    Unset { field: String },
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum FieldConfiguration {
    SingleLineText {},
    RichText {},
    Number {},
    Date {},
    Time {},
    Image {},
    File {},
    Url {},
    Duration {},
    SingleChoice {
        options: Vec<NewOption>,
    },
    MultiChoice {
        options: Vec<NewOption>,
    },
    Relation {
        multiple: bool,
        allowed_templates: Vec<String>,
        reciprocal_notice: bool,
    },
    DocumentLink {},
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewOption {
    pub(crate) id: String,
    pub(crate) label: String,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TemplateEdit {
    Name {
        name: String,
    },
    Presentation {
        token: Option<String>,
    },
    GlossaryExcluded {
        excluded: bool,
    },
    CreateField {
        field: String,
        label: String,
        configuration: FieldConfiguration,
        required: bool,
        presentation: Option<String>,
        default: ValueDto,
        index: Option<String>,
    },
    FieldLabel {
        field: String,
        label: String,
    },
    FieldRequired {
        field: String,
        required: bool,
    },
    FieldPresentation {
        field: String,
        token: Option<String>,
    },
    Default {
        field: String,
        value: ValueDto,
    },
    KeepDefault {
        field: String,
    },
    ReorderFields {
        fields: Vec<String>,
    },
    ArchiveField {
        field: String,
    },
    AddOption {
        field: String,
        option: NewOption,
        index: Option<String>,
    },
    RenameOption {
        field: String,
        option: String,
        label: String,
    },
    ReorderOptions {
        field: String,
        options: Vec<String>,
    },
    ArchiveOption {
        field: String,
        option: String,
        repair: Option<ValueDto>,
    },
}
