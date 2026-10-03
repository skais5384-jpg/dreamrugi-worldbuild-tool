use super::dto::{Id, ResultDto, TemplateDto, ValueDto};
use crate::data::{
    artifact::layout::{DocumentLayout, LayoutEdit},
    edit_recovery::model::{DraftValue, Key},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    AssetImport {
        #[serde(default)]
        cell: Option<crate::data::artifact::group::CellAddress>,
        owner: Id,
        generation: String,
        field: String,
        image: bool,
    },
    AssetRead {
        asset: String,
    },
    AssetChunk {
        asset: String,
        digest: String,
        offset: u64,
    },
    AssetOpen {
        asset: String,
    },
    UrlOpen {
        url: String,
    },
    FormatInspect {
        kind: String,
        artifact: String,
    },
    FormatChange {
        kind: String,
        artifact: String,
        source: String,
        restore: Option<String>,
    },
    EditBegin {
        document: String,
    },
    EditDraft {
        owner: Id,
        generation: String,
        body: EditBody,
        save: bool,
    },
    EditDeposit {
        owner: Id,
        generation: String,
        body: EditBody,
    },
    EditRelease {
        owner: Id,
        generation: String,
    },
    EditRefresh {
        owner: Id,
    },
    EditRetry {
        owner: Id,
        generation: String,
        body: EditBody,
    },
    EditRestore {
        #[serde(default)]
        reapply: Option<super::backend::recovery_merge::Apply>,
        key: Key,
        deposit_id: String,
        digest: String,
    },
    Search {
        query: String,
        template: Option<String>,
        offset: usize,
        limit: usize,
        refresh: bool,
    },
    SearchRead {
        document: String,
    },
    ReplacePreview {
        find: String,
        replacement: String,
        template: Option<String>,
        scopes: ReplaceScopes,
        #[serde(rename = "caseSensitive")]
        case_sensitive: bool,
        #[serde(rename = "wholeWord")]
        whole_word: bool,
        offset: usize,
        limit: usize,
    },
    ReplacePage {
        preview: Id,
        offset: usize,
        limit: usize,
    },
    ReplaceDiscard {
        preview: Option<Id>,
    },
    ReplaceApply {
        preview: Id,
    },
    References {
        document: String,
    },
    List {
        #[serde(default, rename = "refreshSearch")]
        refresh_search: Option<bool>,
    },
    Read {
        document: String,
    },
    PdfInspect {
        document: String,
    },
    PdfExport {
        document: String,
        source: String,
        destination: String,
        allow_missing_images: bool,
    },
    Mutate {
        snapshot: Id,
        edit: LayoutEdit,
    },
    Begin {
        template: String,
        snapshot: Option<Id>,
    },
    Draft {
        owner: Id,
        generation: String,
        body: CreationBody,
        save: bool,
    },
    Deposit {
        owner: Id,
        generation: String,
        body: CreationBody,
    },
    Release {
        owner: Id,
        generation: String,
        discard: bool,
    },
    Restore {
        #[serde(default)]
        reapply: Option<super::backend::recovery_merge::Apply>,
        key: Key,
        deposit_id: String,
        digest: String,
    },
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReplaceScopes {
    pub(crate) title: bool,
    pub(crate) body: bool,
    pub(crate) english_name: bool,
    pub(crate) glossary_summary: bool,
}

impl ReplaceScopes {
    pub(crate) const fn any(self) -> bool {
        self.title || self.body || self.english_name || self.glossary_summary
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EditBody {
    pub(crate) name: crate::data::edit_recovery::model::Intent<String>,
    #[serde(
        default,
        skip_serializing_if = "crate::data::edit_recovery::model::Intent::is_keep"
    )]
    pub(crate) english_name: crate::data::edit_recovery::model::Intent<String>,
    #[serde(
        default,
        skip_serializing_if = "crate::data::edit_recovery::model::Intent::is_keep"
    )]
    pub(crate) glossary_summary: crate::data::edit_recovery::model::Intent<String>,
    #[serde(
        default,
        skip_serializing_if = "crate::data::edit_recovery::model::Intent::is_keep"
    )]
    pub(crate) glossary_excluded: crate::data::edit_recovery::model::Intent<bool>,
    pub(crate) fields: Vec<DraftValue>,
    pub(crate) composing: bool,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreationBody {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) english_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) glossary_summary: String,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub(crate) glossary_excluded: bool,
    pub(crate) parent: Option<String>,
    pub(crate) fields: Vec<DraftValue>,
    pub(crate) composing: bool,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    pub(crate) id: String,
    pub(crate) template: String,
    pub(crate) name: String,
    pub(crate) english_name: String,
    pub(crate) glossary_summary: String,
    pub(crate) glossary_excluded: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentValidationIssue {
    pub(crate) document: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) reasons: Vec<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchExcerpt {
    pub(crate) label: String,
    pub(crate) text: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResult {
    pub(crate) id: String,
    pub(crate) template: String,
    pub(crate) template_name: String,
    pub(crate) name: String,
    pub(crate) path: Vec<String>,
    pub(crate) title_match: bool,
    pub(crate) excerpt: Option<SearchExcerpt>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplaceChange {
    pub(crate) document: String,
    pub(crate) document_name: String,
    pub(crate) template_name: String,
    pub(crate) path: Vec<String>,
    pub(crate) scope: String,
    pub(crate) label: String,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) before_prefix: String,
    pub(crate) before_match: String,
    pub(crate) before_suffix: String,
    pub(crate) after_prefix: String,
    pub(crate) after_match: String,
    pub(crate) after_suffix: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplaceBlocker {
    pub(crate) document: Option<String>,
    pub(crate) document_name: Option<String>,
    pub(crate) label: String,
    pub(crate) reason: String,
}
#[derive(Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum IncomingReference {
    Relation {
        source: String,
        source_name: String,
        source_template: String,
        path: Vec<String>,
        target: String,
        field: String,
        field_label: String,
        instance: Option<String>,
        connection: String,
        relation_name: String,
        one_way: bool,
        missing_reciprocal: bool,
    },
    DocumentLink {
        source: String,
        source_name: String,
        source_template: String,
        path: Vec<String>,
        target: String,
        field: String,
        field_label: String,
        instance: Option<String>,
    },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnavailableIncomingReference {
    pub(crate) source: String,
    pub(crate) source_name: String,
    pub(crate) reason: &'static str,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadField {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) state: String,
    pub(crate) provenance: Option<String>,
    pub(crate) value: Option<ValueDto>,
    pub(crate) problem: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreationCommit {
    pub(crate) fingerprint: String,
    pub(crate) snapshot: Id,
    pub(crate) layout_revision: u32,
    pub(crate) parent: Option<String>,
    pub(crate) changed_documents: Vec<Summary>,
    pub(crate) removed_documents: Vec<String>,
    pub(crate) unplaced: Vec<String>,
    pub(crate) document_count: usize,
    pub(crate) read: Box<Response>,
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Response {
    AssetError {
        error: crate::data::edit_recovery::error::ErrorDto,
        #[serde(rename = "assetName")]
        asset_name: Option<String>,
        #[serde(rename = "assetState")]
        asset_state: Option<&'static str>,
    },
    Asset {
        metadata: crate::data::assets::Metadata,
        content_type: Option<&'static str>,
    },
    AssetChunk {
        data: String,
        done: bool,
    },
    AssetDone {},
    Format {
        schema: u32,
        source: String,
        history: Vec<crate::data::repository::format::Snapshot>,
    },
    Editing {
        owner: Id,
        document: String,
        generation: String,
        saved_generation: Option<String>,
        body: EditBody,
        read: Box<Response>,
        editable: Vec<String>,
        source: String,
        deposited: bool,
        outcome: Option<Box<ResultDto>>,
        problem: Option<String>,
        field: Option<String>,
    },
    List {
        fingerprint: String,
        snapshot: Id,
        layout: DocumentLayout,
        initial: bool,
        unplaced: Vec<String>,
        documents: Vec<Summary>,
        issues: Vec<DocumentValidationIssue>,
        #[serde(rename = "issueStatus")]
        issue_status: String,
        #[serde(rename = "unverifiedDocuments")]
        unverified_documents: Vec<String>,
        problem: Option<String>,
    },
    Search {
        generation: Id,
        offset: usize,
        total: usize,
        #[serde(rename = "hasMore")]
        has_more: bool,
        #[serde(rename = "missingDocuments")]
        missing_documents: usize,
        #[serde(rename = "templateNames")]
        template_names: std::collections::BTreeMap<String, String>,
        results: Vec<SearchResult>,
    },
    SearchUnavailable {
        document: String,
    },
    ReplacePreview {
        preview: Id,
        offset: usize,
        #[serde(rename = "totalDocuments")]
        total_documents: usize,
        #[serde(rename = "totalChanges")]
        total_changes: usize,
        #[serde(rename = "hasMore")]
        has_more: bool,
        changes: Vec<ReplaceChange>,
        blockers: Vec<ReplaceBlocker>,
    },
    References {
        generation: Id,
        document: String,
        incomplete: usize,
        incoming: Vec<IncomingReference>,
        unavailable: Vec<UnavailableIncomingReference>,
    },
    Read {
        schema: u32,
        id: String,
        name: String,
        #[serde(rename = "englishName")]
        english_name: String,
        #[serde(rename = "glossarySummary")]
        glossary_summary: String,
        #[serde(rename = "glossaryExcluded")]
        glossary_excluded: bool,
        template: TemplateDto,
        fields: Vec<ReadField>,
        warnings: Vec<String>,
    },
    PdfInspect {
        document: String,
        name: String,
        source: String,
        missing_images: Vec<String>,
    },
    PdfExport {
        destination: String,
        size: u64,
        sha256: String,
        cleanup_warning: bool,
    },
    Draft {
        owner: Id,
        generation: String,
        body: CreationBody,
        template: TemplateDto,
        deposited: bool,
        outcome: Option<Box<ResultDto>>,
        problem: Option<String>,
        field: Option<String>,
        commit: Option<Box<CreationCommit>>,
    },
    Released {},
}
