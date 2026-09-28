//! JSON은 권한이 아니다. 닫힌 입력만 받고 오류에는 입력 원문을 복사하지 않는다.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Id(Uuid);
impl Id {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl TryFrom<String> for Id {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&value).map_err(|_| "invalid id")?;
        if id.is_nil() || id.to_string() != value {
            return Err("invalid id");
        }
        Ok(Self(id))
    }
}
impl From<Id> for String {
    fn from(id: Id) -> Self {
        id.0.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Code {
    Cancelled,
    InvalidInput,
    Forbidden,
    CollaborationReadOnly,
    UnknownId,
    WrongBinding,
    DuplicateConflict,
    Full,
    Closed,
    Starting,
    InitializationFailed,
    Unavailable,
    NotTerminal,
    OwnersRemain,
    RuntimeRejected,
    RepositoryRejected,
    SessionRejected,
    PreparationRejected,
    SaveRejected,
    SinkUnavailable,
    RecoveryStoreBusy,
    NoReceipt,
    NoDraft,
    NoRecoveryCondition,
    ReleaseRejected,
    RecoveryRejected,
    CompositeIntentPending,
    ProjectNotEmpty,
    SettingsReadFailed,
    SettingsWriteFailed,
    SnapshotRejected,
    BackupRejected,
    BackupDeleteRejected,
    BackupRetentionFull,
    BackupCorrupt,
    RestoreRejected,
    RestoreRecoveryRequired,
    AssetMaintenanceRejected,
    AssetMaintenanceStale,
    DiagnosticExportRejected,
    PdfExportRejected,
    PdfTemplateUnavailable,
    SerializationFailed,
    PlatformRegistration,
    PlatformRemoval,
    EventDelivery,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorDto {
    pub(crate) code: Code,
    pub(crate) next_action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic: Option<ErrorDiagnosticDto>,
}
impl ErrorDto {
    pub(crate) fn new(code: Code) -> Self {
        Self {
            code,
            next_action: "입력과 작업 ID를 보존하고 상태와 원 결과를 확인한 뒤 명시적으로 다음 작업을 선택하세요",
            diagnostic: None,
        }
    }
}
impl From<Code> for ErrorDto {
    fn from(code: Code) -> Self {
        Self::new(code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorDiagnosticDto {
    pub(crate) stage: String,
    pub(crate) category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) io_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) os_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cleanup_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cleanup_io_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cleanup_os_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsWriteOutcomeDto {
    Applied,
    NotApplied,
    AppliedDurabilityUncertain,
}
pub(crate) type Reply<T> = Result<T, ErrorDto>;

#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Command {
    DocumentProgress { operation: Id, cancel: bool },
    Reserve { lane: Lane },
    AbandonReservation { operation: Id },
    Submit { operation: Id, input: Box<Work> },
    Operation { operation: Id },
    AcknowledgeTransport { operation: Id },
    ProjectStatus { project: Id },
    SessionStatus { project: Id, session: Id },
    ReleaseView { project: Id, view: Id },
    ForegroundStatus {},
    AppShutdown {},
    AppStatus {},
    UiReady {},
    UiCloseStatus {},
    UiCloseDecision { attempt: Id, proceed: bool },
    AcknowledgeShutdown { project: Id },
    ReleaseRound { project: Id },
    RetainedRead { retained: RetainedRef },
    RetainedList {},
    RetryNativeCleanup { generation: String },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lane {
    Ordinary,
    Control,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Work {
    DocumentWorkspace {
        project: Id,
        request: super::document_workspace::Request,
    },
    RecoveryPage {
        cursor: Option<String>,
    },
    RecoveryCloseCursor {
        cursor: String,
    },
    RecoveryRead {
        key: crate::data::edit_recovery::model::Key,
        deposit_id: String,
        digest: String,
    },
    RecoveryContent {
        snapshot: Id,
        offset: String,
    },
    RecoveryReleaseSelection {
        snapshot: Id,
    },
    RecoveryRevalidate {
        key: crate::data::edit_recovery::model::Key,
        deposit_id: String,
        digest: String,
    },
    RecoveryDiscard {
        key: crate::data::edit_recovery::model::Key,
        version: String,
    },
    RecoveryRestore {
        project: Id,
        snapshot: Id,
        reapply: Option<Vec<super::workspace::ReapplyIntent>>,
    },
    BeginTemplateDraft {
        project: Id,
        view: Option<Id>,
    },
    TemplateDraft {
        project: Id,
        session: Id,
        generation: String,
        body: super::workspace::TemplateBody,
        action: super::workspace::DraftAction,
    },
    TemplateDraftContent {
        project: Id,
        session: Id,
        snapshot: Id,
        offset: String,
    },
    RefreshTemplateDraft {
        project: Id,
        session: Id,
    },
    ReleaseTemplateDraft {
        project: Id,
        session: Id,
        generation: String,
        body: Option<super::workspace::TemplateBody>,
        discard: bool,
    },
    RetainedHandoff {
        retained: RetainedRef,
    },
    AbandonRetained {
        retained: RetainedRef,
    },
    ResumeRetained {
        retained: RetainedRef,
        project: Id,
        destination: G6Destination,
    },
    RetireProject {
        project: Id,
    },
    Open {
        root: String,
    },
    CreateProject {
        parent: String,
        name: String,
    },
    ProjectSettingsRead {},
    ProjectSettingsWrite {
        default_root: Option<String>,
    },
    ProjectCopy {
        project: Id,
        parent: String,
        name: String,
    },
    BackupCreate {
        project: Id,
        storage: String,
        label: Option<String>,
    },
    BackupList {
        project: Id,
        storage: String,
        cursor: Option<String>,
    },
    BackupDelete {
        project: Id,
        storage: String,
        locator: String,
    },
    BackupDeletedList {
        project: Id,
        storage: String,
    },
    BackupDeletedRestore {
        project: Id,
        storage: String,
        id: String,
        operation: String,
    },
    BackupDeletedPurge {
        project: Id,
        storage: String,
        id: String,
        operation: String,
    },
    BackupInspect {
        locator: String,
    },
    RestoreNew {
        locator: String,
        parent: String,
        name: String,
    },
    RestoreCurrent {
        project: Id,
        locator: String,
        safety_storage: String,
    },
    AssetInspect {
        project: Id,
    },
    AssetTrashMove {
        project: Id,
        inspection_token: String,
        assets: Vec<String>,
    },
    AssetRename {
        project: Id,
        inspection_token: String,
        asset: String,
        name: String,
    },
    AssetTrashRestore {
        project: Id,
        assets: Vec<String>,
    },
    AssetTrashPurge {
        project: Id,
        inspection_token: String,
        assets: Vec<String>,
        empty: bool,
    },
    TemplatePurge {
        project: Id,
        inspection_token: String,
        templates: Vec<String>,
    },
    TemplateRestore {
        project: Id,
        template: String,
    },
    DiagnosticExport {
        project: Id,
        destination: String,
    },
    Recover {
        project: Id,
    },
    Close {
        project: Id,
    },
    ListTemplates {
        project: Id,
    },
    ReadTemplate {
        project: Id,
        template: String,
    },
    ReadDocument {
        project: Id,
        document: String,
    },
    CreateTemplate {
        project: Id,
        name: String,
        presentation: Option<String>,
    },
    DuplicateTemplate {
        project: Id,
        view: Id,
    },
    CreateDocument {
        project: Id,
        view: Id,
        name: String,
    },
    BeginSession {
        project: Id,
        views: Vec<Id>,
        purpose: EditPurpose,
    },
    UpdateTemplate {
        project: Id,
        session: Id,
        view: Id,
        revision: String,
        edit: TemplateEdit,
    },
    TombstoneTemplate {
        project: Id,
        session: Id,
        view: Id,
        revision: String,
    },
    MaterializeDocument {
        project: Id,
        session: Id,
        document: Id,
        template: Id,
        revision: String,
    },
    SaveDocument {
        project: Id,
        session: Id,
        document: Id,
        template: Id,
        revision: String,
        edits: Vec<DocumentEdit>,
    },
    SaveComposite {
        project: Id,
        session: Id,
        document: Id,
        template: Id,
        revision: String,
        edit: TemplateEdit,
        edits: Vec<DocumentEdit>,
    },
    SessionControl {
        project: Id,
        session: Id,
        control: SessionControl,
    },
}
impl Work {
    pub(crate) fn project(&self) -> Option<Id> {
        match self {
            Self::DocumentWorkspace { project, .. } | Self::RecoveryRestore { project, .. } => {
                Some(*project)
            }
            Self::RecoveryPage { .. }
            | Self::RecoveryRead { .. }
            | Self::RecoveryContent { .. }
            | Self::RecoveryCloseCursor { .. }
            | Self::RecoveryRevalidate { .. }
            | Self::RecoveryDiscard { .. }
            | Self::RecoveryReleaseSelection { .. } => None,
            Self::BeginTemplateDraft { project, .. }
            | Self::TemplateDraft { project, .. }
            | Self::TemplateDraftContent { project, .. }
            | Self::RefreshTemplateDraft { project, .. }
            | Self::ReleaseTemplateDraft { project, .. } => Some(*project),
            Self::Open { .. }
            | Self::CreateProject { .. }
            | Self::ProjectSettingsRead {}
            | Self::ProjectSettingsWrite { .. }
            | Self::BackupInspect { .. }
            | Self::RestoreNew { .. }
            | Self::RetainedHandoff { .. }
            | Self::AbandonRetained { .. } => None,
            Self::ProjectCopy { project, .. }
            | Self::BackupCreate { project, .. }
            | Self::BackupList { project, .. }
            | Self::BackupDelete { project, .. }
            | Self::BackupDeletedList { project, .. }
            | Self::BackupDeletedRestore { project, .. }
            | Self::BackupDeletedPurge { project, .. }
            | Self::RestoreCurrent { project, .. }
            | Self::AssetInspect { project }
            | Self::AssetTrashMove { project, .. }
            | Self::AssetRename { project, .. }
            | Self::AssetTrashRestore { project, .. }
            | Self::AssetTrashPurge { project, .. }
            | Self::TemplatePurge { project, .. }
            | Self::TemplateRestore { project, .. }
            | Self::DiagnosticExport { project, .. } => Some(*project),
            Self::ResumeRetained { project, .. } | Self::RetireProject { project } => {
                Some(*project)
            }
            Self::Recover { project }
            | Self::Close { project }
            | Self::ListTemplates { project }
            | Self::ReadTemplate { project, .. }
            | Self::ReadDocument { project, .. }
            | Self::CreateTemplate { project, .. }
            | Self::DuplicateTemplate { project, .. }
            | Self::CreateDocument { project, .. }
            | Self::BeginSession { project, .. }
            | Self::UpdateTemplate { project, .. }
            | Self::TombstoneTemplate { project, .. }
            | Self::MaterializeDocument { project, .. }
            | Self::SaveDocument { project, .. }
            | Self::SaveComposite { project, .. }
            | Self::SessionControl { project, .. } => Some(*project),
        }
    }
    pub(crate) fn session(&self) -> Option<Id> {
        match self {
            Self::TemplateDraft { session, .. }
            | Self::TemplateDraftContent { session, .. }
            | Self::RefreshTemplateDraft { session, .. }
            | Self::ReleaseTemplateDraft { session, .. } => Some(*session),
            Self::UpdateTemplate { session, .. }
            | Self::TombstoneTemplate { session, .. }
            | Self::MaterializeDocument { session, .. }
            | Self::SaveDocument { session, .. }
            | Self::SaveComposite { session, .. }
            | Self::SessionControl { session, .. } => Some(*session),
            _ => None,
        }
    }
    pub(crate) fn lane(&self) -> Lane {
        match self {
            Self::RecoveryCloseCursor { .. }
            | Self::RecoveryRevalidate { .. }
            | Self::RecoveryDiscard { .. }
            | Self::RecoveryReleaseSelection { .. } => Lane::Control,
            Self::TemplateDraft {
                action: super::workspace::DraftAction::Deposit,
                ..
            }
            | Self::ReleaseTemplateDraft { .. } => Lane::Control,
            Self::RetainedHandoff { .. }
            | Self::AbandonRetained { .. }
            | Self::RetireProject { .. } => Lane::Control,
            Self::Recover { .. } | Self::Close { .. } | Self::SessionControl { .. } => {
                Lane::Control
            }
            _ => Lane::Ordinary,
        }
    }
    pub(crate) fn contains_edit(&self) -> bool {
        matches!(
            self,
            Self::TemplateDraft { .. }
                | Self::CreateTemplate { .. }
                | Self::DuplicateTemplate { .. }
                | Self::TombstoneTemplate { .. }
                | Self::TemplateRestore { .. }
                | Self::CreateDocument { .. }
                | Self::UpdateTemplate { .. }
                | Self::SaveDocument { .. }
                | Self::SaveComposite { .. }
        )
    }
}
/// 이 식별자는 권한/receipt가 아니다. 실제 caller와 backend owner를 함께 대조한다.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedRef {
    pub(crate) id: Id,
    pub(crate) generation: Id,
}
#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum G6Destination {
    #[serde(rename = "create_template")]
    Create {},
    #[serde(rename = "duplicate_template")]
    Duplicate { view: Id },
    #[serde(rename = "update_template")]
    Update {
        session: Id,
        view: Id,
        revision: String,
    },
    #[serde(rename = "tombstone_template")]
    Tombstone {
        session: Id,
        view: Id,
        revision: String,
    },
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum G6Intent {
    #[serde(rename = "create_template")]
    Create {
        name: String,
        presentation: Option<String>,
    },
    #[serde(rename = "duplicate_template")]
    Duplicate {},
    #[serde(rename = "update_template")]
    Update {
        revision: String,
        edit: TemplateEdit,
    },
    #[serde(rename = "tombstone_template")]
    Tombstone { revision: String },
}
#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionControl {
    End,
    RetryRelease,
    Revalidate,
    Preserve,
    Accept,
    AcknowledgeRecovery,
    ReturnActive,
}
#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EditPurpose {
    Template,
    Document,
    Composite,
}

pub(crate) use crate::data::edit_input::{
    DocumentEdit, FieldConfiguration, NewOption, RichNode, TemplateEdit, ValueDto,
};

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Response {
    DocumentProgress {
        requested: bool,
        files: String,
        phase: u8,
    },
    UiClose {
        enabled: bool,
        attempt: Option<Id>,
        closing: bool,
    },
    RetainedList {
        entries: Vec<RetainedRef>,
    },
    Reserved {
        operation: Id,
    },
    Submitted {
        operation: Id,
    },
    Acknowledged,
    Operation {
        operation: Id,
        state: OperationPhase,
        result: Option<Box<ResultDto>>,
        retained: Option<RetainedRef>,
    },
    Retained {
        retained: RetainedRef,
        project: Id,
        artifacts: Vec<String>,
        intent: Option<G6Intent>,
        result: Box<ResultDto>,
        g6_clearable: bool,
    },
    Project {
        project: Id,
        collaborative: bool,
        status: String,
        runtime: Option<String>,
        error: Option<ErrorDto>,
        validation_failures: Vec<ValidationDto>,
        shutdown: ShutdownDto,
    },
    Session {
        session: Id,
        state: String,
        active_dirty: bool,
        custody: Option<String>,
        sink_connected: bool,
        recovery_error: Option<crate::data::edit_recovery::error::ErrorDto>,
    },
    App {
        generation: String,
        closing: bool,
        projects: String,
        operations: String,
        retained_edits: String,
        normal_exit_allowed: bool,
        event_error: Option<Code>,
        native_cleanup: NativeCleanupDto,
        support_diagnostics: Vec<crate::support_diagnostics::SupportDiagnostic>,
        diagnostics: crate::diagnostic_log::Status,
    },
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativePhase {
    Complete,
    Registered,
    Pending,
    Running,
    Failed,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeCleanupDto {
    pub(crate) phase: NativePhase,
    pub(crate) generation: String,
    pub(crate) attempts: String,
    pub(crate) first_error: Option<Code>,
    pub(crate) latest_error: Option<Code>,
    pub(crate) next_action: Option<&'static str>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationPhase {
    Reserved,
    Pending,
    Complete,
    Rejected,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShutdownDto {
    pub(crate) closing: bool,
    pub(crate) phase: String,
    pub(crate) round: String,
    pub(crate) blockers: Vec<String>,
    pub(crate) report_pending: bool,
    pub(crate) resources_complete: bool,
    pub(crate) normal_exit_allowed: bool,
    pub(crate) joined: bool,
    pub(crate) force_active: bool,
    pub(crate) force_results: Vec<ForceResultDto>,
    pub(crate) next_actions: Vec<String>,
    pub(crate) reports: Vec<ShutdownReportDto>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForceResultDto {
    pub(crate) document: String,
    pub(crate) outcome: String,
    pub(crate) error: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShutdownReportDto {
    pub(crate) round: String,
    pub(crate) initialization_failed: bool,
    pub(crate) close_failed: bool,
    pub(crate) release_failures: String,
    pub(crate) coordination_errors: Vec<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidationDto {
    pub(crate) session: Option<Id>,
    pub(crate) category: String,
    pub(crate) lock_category: Option<String>,
    pub(crate) preserve_failed: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticDto {
    pub(crate) stage: String,
    pub(crate) category: Option<String>,
    pub(crate) session_state: String,
    pub(crate) lock_category: Option<String>,
    pub(crate) next_action: &'static str,
    pub(crate) operation_id: Option<Id>,
    pub(crate) observed_at_utc: Option<String>,
    pub(crate) failures: Vec<WriteFailureDto>,
}

/// 원 오류 문자열과 사용자 경로 대신 기존 진단의 닫힌 분류만 전달한다.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WriteFailureDto {
    pub(crate) role: String,
    pub(crate) stage: String,
    pub(crate) category: &'static str,
    pub(crate) transaction_id: Option<String>,
    pub(crate) io_kind: Option<String>,
    pub(crate) os_code: Option<i32>,
    pub(crate) context: Option<String>,
    pub(crate) secondary: Vec<WriteFailureDto>,
}
impl From<&crate::data::transaction::artifact_diagnostics::FailureDiagnostic> for WriteFailureDto {
    fn from(value: &crate::data::transaction::artifact_diagnostics::FailureDiagnostic) -> Self {
        Self {
            role: format!("{:?}", value.role),
            stage: format!("{:?}", value.stage),
            category: value.category,
            transaction_id: value.transaction_id.as_ref().map(ToString::to_string),
            io_kind: value.io.as_ref().map(|io| format!("{:?}", io.kind)),
            os_code: value.io.as_ref().and_then(|io| io.os_code),
            // IoContext의 Debug는 허용된 단계/분류만 출력하는 기존 projection이다.
            context: value.io.as_ref().map(|io| format!("{:?}", io.context)),
            secondary: value.secondary.iter().map(Self::from).collect(),
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResultDto {
    DocumentWorkspace {
        value: Box<super::document_workspace::Response>,
    },
    RecoveryFailure {
        failure: crate::data::edit_recovery::error::ErrorDto,
    },
    RecoveryPage {
        page: crate::data::edit_recovery::center::Page,
    },
    RecoverySelection {
        selection: super::workspace::RecoverySelection,
    },
    RecoveryReceipt {
        receipt: super::workspace::ReceiptDto,
    },
    TemplateDraft {
        status: Box<super::workspace::DraftStatus>,
    },
    TemplateDraftContent {
        content: super::workspace::ContentChunk,
    },
    RetainedHandled {
        retained: RetainedRef,
        action: &'static str,
    },
    ProjectRetired {
        project: Id,
        shutdown: ShutdownDto,
    },
    Open {
        project: Id,
        status: String,
        runtime: Option<String>,
        error: Option<ErrorDto>,
        created: bool,
    },
    ProjectSettings {
        default_root: Option<String>,
    },
    ProjectSettingsWrite {
        outcome: SettingsWriteOutcomeDto,
        observed: bool,
        default_root: Option<String>,
        error: Option<ErrorDto>,
        readback_error: Option<ErrorDto>,
    },
    ProjectData {
        action: &'static str,
        root: Option<String>,
        backup: Option<crate::data::project_backup::BackupRow>,
        safety: Option<crate::data::project_backup::BackupRow>,
        backups: Vec<crate::data::project_backup::BackupRow>,
        deleted: Option<crate::data::project_backup::DeletedBackupRow>,
        #[serde(rename = "deletedBackups")]
        deleted_backups: Vec<crate::data::project_backup::DeletedBackupRow>,
        next_cursor: Option<String>,
        recovery_required: bool,
        outcome: Option<&'static str>,
        warning: Option<&'static str>,
    },
    AssetMaintenance {
        action: &'static str,
        inspection: crate::data::asset_maintenance::Inspection,
        completed: Vec<String>,
        #[serde(rename = "completedCount")]
        completed_count: usize,
        failures: Vec<crate::data::asset_maintenance::ActionFailure>,
        partial: bool,
        #[serde(rename = "cleanupRequired")]
        cleanup_required: Vec<String>,
    },
    DiagnosticExport {
        result: crate::diagnostic_log::ExportResult,
    },
    Templates {
        templates: Vec<TemplateSummary>,
    },
    Template {
        view: Id,
        content: TemplateDto,
    },
    Document {
        view: Id,
        content: DocumentDto,
    },
    Session {
        session: Id,
        state: String,
        error: Option<ErrorDto>,
    },
    Write {
        session: Id,
        artifact: Option<String>,
        disk: DiskDto,
        changed: Option<bool>,
        warnings: Vec<WarningDto>,
        cleanup_failed: bool,
        recovery_required: bool,
        error: Option<ErrorDto>,
        diagnostic: DiagnosticDto,
        #[serde(skip_serializing_if = "Option::is_none")]
        deletion: Option<DeletionSummary>,
    },
    Dirty {
        outcome: Box<ResultDto>,
        custody: Option<String>,
        custody_error: Option<ErrorDto>,
    },
    Composite {
        outcome: Box<ResultDto>,
        template_changed: Option<bool>,
        document_changed: Option<bool>,
    },
    Control {
        error: Option<ErrorDto>,
    },
    Rejected {
        error: ErrorDto,
        input_retained: bool,
    },
}
/// 삭제 실행의 원 오류에서만 투영한다. 경로·문서 내용·원 오류 문자열은 포함하지 않는다.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub(crate) enum DeletionSummary {
    TemplateHasDocuments { count: u16, truncated: bool },
    SourceChanged,
    ReferenceCheckFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiskDto {
    NotAttempted,
    NoWrite,
    NotApplied,
    RolledBack,
    Committed,
    Uncertain,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WarningDto {
    pub(crate) category: String,
    pub(crate) field: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TemplateSummary {
    pub(crate) name: String,
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) lifecycle: String,
    pub(crate) glossary_excluded: bool,
}

#[cfg(test)]
mod glossary_wire_tests {
    use super::TemplateSummary;

    #[test]
    fn template_summary_uses_the_frontend_glossary_exclusion_name() {
        let value = serde_json::to_value(TemplateSummary {
            name: "인물".into(),
            id: "11111111-1111-4111-8111-111111111111".into(),
            revision: "1".into(),
            lifecycle: "Active".into(),
            glossary_excluded: true,
        })
        .expect("serialize Template summary");
        assert_eq!(value["glossaryExcluded"], true);
        assert!(value.get("glossary_excluded").is_none());
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TemplateDto {
    pub(crate) schema: u32,
    pub(crate) sections: Vec<crate::data::artifact::SectionInput>,
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) name: String,
    pub(crate) lifecycle: String,
    pub(crate) glossary_excluded: bool,
    pub(crate) presentation: Option<String>,
    pub(crate) field_order: Vec<String>,
    pub(crate) fields: Vec<FieldDto>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldDto {
    pub(crate) members: Vec<FieldDto>,
    pub(crate) member_order: Vec<String>,
    pub(crate) minimum: Option<String>,
    pub(crate) maximum: Option<String>,
    pub(crate) multiple: Option<bool>,
    pub(crate) allowed_templates: Vec<String>,
    pub(crate) reciprocal_notice: Option<bool>,
    pub(crate) writing_guide: Option<String>,
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) lifecycle: String,
    pub(crate) required: bool,
    pub(crate) presentation: Option<String>,
    pub(crate) default: ValueDto,
    pub(crate) initial_default: ValueDto,
    pub(crate) introduced_revision: String,
    pub(crate) options: Vec<OptionDto>,
    pub(crate) option_order: Vec<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct OptionDto {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) lifecycle: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentDto {
    pub(crate) id: String,
    pub(crate) template: String,
    pub(crate) template_revision: String,
    pub(crate) name: String,
    pub(crate) english_name: String,
    pub(crate) glossary_summary: String,
    pub(crate) glossary_excluded: bool,
    pub(crate) values: Vec<DocumentFieldDto>,
}
#[derive(Clone, Serialize)]
pub(crate) struct DocumentFieldDto {
    pub(crate) field: String,
    pub(crate) value: ValueDto,
}

pub(crate) fn decode(bytes: &[u8]) -> Reply<Command> {
    if bytes.len() > 1024 * 1024 {
        return Err(Code::InvalidInput.into());
    }
    // 기본 serde/Tauri 오류는 임의 enum tag와 invalid value를 반사할 수 있다.
    serde_json::from_slice(bytes).map_err(|_| Code::InvalidInput.into())
}
