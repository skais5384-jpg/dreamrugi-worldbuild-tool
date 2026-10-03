import type { Intent } from "./workspace";
export interface CellAddress {
  instance: string;
  child: string;
}
export interface GroupInstance {
  id: string;
  source: string | null;
  /** 직전 복제 부모부터 저장 기준까지. source는 여전히 native 원문 출처다. */
  lineage?: string[];
  fields: { field: string; value: Intent<Value> }[];
  labels?: Record<string, string>;
  protected?: string[];
}
export type GroupValue = { kind: "group"; instances: GroupInstance[] };

/** 숫자 본문·revision·counter를 JSON number로 바꾸지 않는다. */
export type Id = string;
export type Lane = "ordinary" | "control";
export interface RetainedRef {
  id: Id;
  generation: Id;
}
export interface BackupRow {
  id: string;
  label: string;
  createdAtUtc: string;
  kind: "manual" | "pre_restore";
  size: string;
  status: "verification_required" | "verified" | "corrupt" | "unsupported";
  coverage: "complete" | "legacy_unknown" | "unknown";
  unnamedOrdinal: number | null;
  locator: string;
}
export interface DeletedBackupRow {
  backup: BackupRow;
  deletedAtUtc: string;
  expiresAtUtc: string;
  operation: string;
  status: "verification_required" | "expired" | "uncertain";
}
export type AssetHealthStatus =
  "used" | "unused" | "in_trash" | "missing" | "corrupt" | "uncertain";
export interface AssetHealthRow {
  id: string;
  name: string | null;
  size: number | null;
  status: AssetHealthStatus;
  reason: string | null;
}
export interface AssetTrashRow {
  id: string;
  name: string;
  size: number;
  reclaimableSize: number | null;
  movedAtUtc: string | null;
  protected: boolean;
  reason: string | null;
}
export interface DeletedTemplateRow {
  id: string;
  name: string;
  size: number;
  removable: boolean;
  reason: string | null;
}
export interface DocumentIssueRow {
  documentId: string;
  reasons: string[];
  relatedResourceIds?: string[];
  relatedTemplateIds?: string[];
  targets?: {
    reason: string;
    resourceIds: string[];
    templateIds: string[];
  }[];
}
export interface AssetInspection {
  ownerProtected?: boolean;
  token: string;
  observedAtUtc: string;
  complete: boolean;
  scannedFiles: number;
  referencedAssets: number;
  usedAssets: number;
  unusedAssets: number;
  missingAssets: number;
  corruptAssets: number;
  uncertainAssets: number;
  rows: AssetHealthRow[];
  trash: AssetTrashRow[];
  deletedTemplates: DeletedTemplateRow[];
  documentIssues?: DocumentIssueRow[];
}
export type G6Destination =
  | { kind: "create_template" }
  | { kind: "duplicate_template"; view: Id }
  | {
      kind: "update_template" | "tombstone_template";
      session: Id;
      view: Id;
      revision: string;
    };
export type G6Intent =
  | { kind: "create_template"; name: string; presentation: string | null }
  | { kind: "duplicate_template" }
  | { kind: "update_template"; revision: string; edit: TemplateEdit }
  | { kind: "tombstone_template"; revision: string };
export type RichNode =
  | {
      kind:
        | "root"
        | "paragraph"
        | "blockquote"
        | "bulletList"
        | "orderedList"
        | "listItem"
        | "taskList";
      children: RichNode[];
    }
  | { kind: "heading"; level: number; children: RichNode[] }
  | { kind: "taskItem"; checked: boolean; children: RichNode[] }
  | {
      kind: "text";
      text: string;
      marks?: ("bold" | "italic" | "underline" | "strikethrough")[];
    }
  | { kind: "hardBreak" };
export type Value =
  | GroupValue
  | { kind: "image"; value: string[] }
  | { kind: "file"; value: string[] }
  | { kind: "unset" }
  | { kind: "number_unknown"; previous_raw?: string }
  | {
      kind: "single_line_text" | "number" | "date" | "time" | "url";
      value: string;
    }
  | { kind: "duration"; milliseconds: string }
  | { kind: "single_choice"; option: string }
  | { kind: "multi_choice"; options: string[] }
  | {
      kind: "relation";
      links: { id: string; document: string; oneWay: boolean; name: string }[];
    }
  | { kind: "document_link"; documents: string[] }
  | { kind: "rich_text"; content: RichNode };
export type DocumentEdit =
  | { kind: "rename"; name: string }
  | { kind: "english_name"; value: string }
  | { kind: "glossary_summary"; value: string }
  | { kind: "glossary_excluded"; excluded: boolean }
  | { kind: "set"; field: string; value: Exclude<Value, { kind: "unset" }> }
  | { kind: "unset"; field: string };
export interface NewOption {
  id: string;
  label: string;
}
export type FieldConfiguration =
  | {
      kind:
        | "single_line_text"
        | "rich_text"
        | "number"
        | "date"
        | "time"
        | "duration"
        | "image"
        | "file"
        | "url";
    }
  | { kind: "single_choice" | "multi_choice"; options: NewOption[] }
  | {
      kind: "relation";
      multiple: boolean;
      allowedTemplates: string[];
      reciprocalNotice: boolean;
    }
  | { kind: "document_link" };
export type TemplateEdit =
  | { kind: "name"; name: string }
  | { kind: "presentation"; token: string | null }
  | { kind: "glossary_excluded"; excluded: boolean }
  | {
      kind: "create_field";
      field: string;
      label: string;
      configuration: FieldConfiguration;
      required: boolean;
      presentation: string | null;
      default: Value;
      index: string | null;
    }
  | { kind: "field_label"; field: string; label: string }
  | { kind: "field_required"; field: string; required: boolean }
  | { kind: "field_presentation"; field: string; token: string | null }
  | { kind: "default"; field: string; value: Value }
  | { kind: "keep_default" | "archive_field"; field: string }
  | { kind: "reorder_fields"; fields: string[] }
  | {
      kind: "add_option";
      field: string;
      option: NewOption;
      index: string | null;
    }
  | { kind: "rename_option"; field: string; option: string; label: string }
  | { kind: "reorder_options"; field: string; options: string[] }
  | {
      kind: "archive_option";
      field: string;
      option: string;
      repair: Value | null;
    };
export type SessionControl =
  | "end"
  | "retry_release"
  | "revalidate"
  | "preserve"
  | "accept"
  | "acknowledge_recovery"
  | "return_active";
interface Bound {
  project: Id;
  session: Id;
}
interface DocumentBound extends Bound {
  document: Id;
  template: Id;
  revision: string;
}
export type Work =
  | {
      kind: "document_workspace";
      project: Id;
      request: import("./documents").DocumentRequest;
    }
  | import("./workspace").WorkspaceWork
  | { kind: "retained_handoff" | "abandon_retained"; retained: RetainedRef }
  | {
      kind: "resume_retained";
      retained: RetainedRef;
      project: Id;
      destination: G6Destination;
    }
  | { kind: "retire_project"; project: Id }
  | { kind: "open"; root: string }
  | { kind: "create_project"; parent: string; name: string }
  | { kind: "project_settings_read" }
  | { kind: "project_settings_write"; default_root: string | null }
  | { kind: "project_copy"; project: Id; parent: string; name: string }
  | {
      kind: "backup_create";
      project: Id;
      storage: string;
      label: string | null;
    }
  | { kind: "backup_list"; project: Id; storage: string; cursor: string | null }
  | { kind: "backup_delete"; project: Id; storage: string; locator: string }
  | { kind: "backup_deleted_list"; project: Id; storage: string }
  | {
      kind: "backup_deleted_restore" | "backup_deleted_purge";
      project: Id;
      storage: string;
      id: string;
      operation: string;
    }
  | { kind: "backup_inspect"; locator: string }
  | { kind: "restore_new"; locator: string; parent: string; name: string }
  | {
      kind: "restore_current";
      project: Id;
      locator: string;
      safety_storage: string;
    }
  | {
      kind: "asset_inspect";
      project: Id;
      observation?: {
        epoch: number;
        generation: number;
        pendingCount: number;
        reRequested: boolean;
      };
    }
  | {
      kind: "asset_trash_move";
      project: Id;
      inspection_token: string;
      assets: string[];
    }
  | {
      kind: "asset_rename";
      project: Id;
      inspection_token: string;
      asset: string;
      name: string;
    }
  | { kind: "asset_trash_restore"; project: Id; assets: string[] }
  | {
      kind: "asset_trash_purge";
      project: Id;
      inspection_token: string;
      assets: string[];
      empty: boolean;
    }
  | {
      kind: "template_purge";
      project: Id;
      inspection_token: string;
      templates: string[];
    }
  | { kind: "template_restore"; project: Id; template: string }
  | { kind: "diagnostic_export"; project: Id; destination: string }
  | { kind: "recover" | "close" | "list_templates"; project: Id }
  | { kind: "read_template"; project: Id; template: string }
  | { kind: "read_document"; project: Id; document: string }
  | {
      kind: "create_template";
      project: Id;
      name: string;
      presentation: string | null;
    }
  | { kind: "duplicate_template"; project: Id; view: Id }
  | { kind: "create_document"; project: Id; view: Id; name: string }
  | {
      kind: "begin_session";
      project: Id;
      views: Id[];
      purpose: "template" | "document" | "composite";
    }
  | (Bound & {
      kind: "update_template";
      view: Id;
      revision: string;
      edit: TemplateEdit;
    })
  | (Bound & { kind: "tombstone_template"; view: Id; revision: string })
  | (DocumentBound & { kind: "materialize_document" })
  | (DocumentBound & { kind: "save_document"; edits: DocumentEdit[] })
  | (DocumentBound & {
      kind: "save_composite";
      edit: TemplateEdit;
      edits: DocumentEdit[];
    })
  | (Bound & { kind: "session_control"; control: SessionControl });
export type Command =
  | { action: "document_progress"; operation: Id; cancel: boolean }
  | { action: "ui_ready" | "ui_close_status" }
  | { action: "ui_close_decision"; attempt: Id; proceed: boolean }
  | { action: "retained_list" }
  | { action: "retained_read"; retained: RetainedRef }
  | { action: "retry_native_cleanup"; generation: string }
  | { action: "reserve"; lane: Lane }
  | { action: "abandon_reservation"; operation: Id }
  | { action: "submit"; operation: Id; input: Work }
  | { action: "operation" | "acknowledge_transport"; operation: Id }
  | {
      action: "project_status" | "acknowledge_shutdown" | "release_round";
      project: Id;
    }
  | { action: "session_status"; project: Id; session: Id }
  | { action: "release_view"; project: Id; view: Id }
  | { action: "foreground_status" | "app_shutdown" | "app_status" };
export interface BoundaryError {
  code: string;
  nextAction: string;
  diagnostic?: {
    stage: string;
    category: string;
    outcome?: string;
    ioKind?: string;
    osCode?: number;
    cleanupOutcome?: string;
    cleanupIoKind?: string;
    cleanupOsCode?: number;
  };
}
export type Disk =
  | "not_attempted"
  | "no_write"
  | "not_applied"
  | "rolled_back"
  | "committed"
  | "uncertain";
export interface Warning {
  category: string;
  field: string | null;
}
export interface TemplateSummary {
  name: string;
  id: string;
  revision: string;
  lifecycle: string;
  glossaryExcluded?: boolean;
}
export interface Field {
  cardTitleField?: string | null;
  members?: Field[];
  memberOrder?: string[];
  minimum?: string | null;
  maximum?: string | null;
  multiple?: boolean;
  allowedTemplates?: string[];
  reciprocalNotice?: boolean;
  id: string;
  label: string;
  kind: string;
  lifecycle: string;
  required: boolean;
  writingGuide?: string | null;
  presentation: string | null;
  default: Value;
  initialDefault: Value;
  introducedRevision: string;
  options: { id: string; label: string; lifecycle: string }[];
  optionOrder: string[];
}
export interface Template extends TemplateSummary {
  schema?: number;
  sections?: Section[];
  name: string;
  presentation: string | null;
  fieldOrder: string[];
  fields: Field[];
}
export interface Section {
  id: string;
  title: string;
  beforeField: string | null;
}
export interface Document {
  id: string;
  template: string;
  templateRevision: string;
  name: string;
  englishName?: string;
  glossarySummary?: string;
  glossaryExcluded?: boolean;
  values: { field: string; value: Value }[];
}
export type ResultDto =
  | {
      kind: "document_workspace";
      value: import("./documents").DocumentResponse;
    }
  | import("./workspace").WorkspaceResult
  | {
      kind: "retained_handled";
      retained: RetainedRef;
      action: "handed_off" | "abandoned";
    }
  | { kind: "project_retired"; project: Id; shutdown: Shutdown }
  | {
      kind: "open";
      project: Id;
      status: string;
      runtime: string | null;
      error: BoundaryError | null;
      created: boolean;
    }
  | { kind: "project_settings"; default_root: string | null }
  | {
      kind: "project_settings_write";
      outcome: "applied" | "not_applied" | "applied_durability_uncertain";
      observed: boolean;
      default_root: string | null;
      error: BoundaryError | null;
      readback_error: BoundaryError | null;
    }
  | {
      kind: "project_data";
      action:
        | "copy"
        | "backup_create"
        | "backup_list"
        | "backup_delete"
        | "backup_deleted_list"
        | "backup_deleted_restore"
        | "backup_deleted_purge"
        | "inspect"
        | "restore_new"
        | "restore_current";
      root: string | null;
      backup: BackupRow | null;
      safety: BackupRow | null;
      backups: BackupRow[];
      deleted?: DeletedBackupRow | null;
      deletedBackups?: DeletedBackupRow[];
      nextCursor: string | null;
      recoveryRequired: boolean;
      outcome: string | null;
      warning: string | null;
    }
  | {
      kind: "asset_maintenance";
      action:
        | "inspect"
        | "trash_move"
        | "asset_rename"
        | "trash_restore"
        | "trash_purge"
        | "template_purge";
      inspection: AssetInspection;
      completed: string[];
      completedCount: number;
      failures: { id: string; category: string }[];
      partial: boolean;
      cleanupRequired: string[];
    }
  | {
      kind: "diagnostic_export";
      result: {
        outcome:
          | "not_applied"
          | "published"
          | "published_verification_uncertain"
          | "not_applied_cleanup_required";
        size: number | null;
        sha256: string | null;
        eventCount: number;
        excluded: string[];
        cleanupRequired: boolean;
      };
    }
  | { kind: "templates"; templates: TemplateSummary[] }
  | { kind: "template"; view: Id; content: Template }
  | { kind: "document"; view: Id; content: Document }
  | { kind: "session"; session: Id; state: string; error: BoundaryError | null }
  | {
      kind: "write";
      session: Id;
      artifact: string | null;
      disk: Disk;
      changed: boolean | null;
      warnings: Warning[];
      cleanup_failed: boolean;
      recovery_required: boolean;
      error: BoundaryError | null;
      diagnostic: {
        stage: string;
        category: string | null;
        sessionState: string;
        lockCategory: string | null;
        nextAction: string;
        operationId?: Id | null;
        observedAtUtc?: string | null;
        failures?: WriteFailure[];
      };
      deletion?:
        | {
            reason: "template_has_documents";
            count: number;
            truncated: boolean;
          }
        | { reason: "source_changed" | "reference_check_failed" };
    }
  | {
      kind: "dirty";
      outcome: ResultDto;
      custody: string | null;
      custody_error: BoundaryError | null;
    }
  | {
      kind: "composite";
      outcome: ResultDto;
      template_changed: boolean | null;
      document_changed: boolean | null;
    }
  | { kind: "control"; error: BoundaryError | null }
  | { kind: "rejected"; error: BoundaryError; input_retained: boolean };
export interface WriteFailure {
  role: string;
  stage: string;
  category: string;
  transactionId: string | null;
  ioKind: string | null;
  osCode: number | null;
  context: string | null;
  secondary: WriteFailure[];
}
export interface Shutdown {
  closing: boolean;
  phase: string;
  round: string;
  blockers: string[];
  reportPending: boolean;
  resourcesComplete: boolean;
  normalExitAllowed: boolean;
  joined: boolean;
  forceActive: boolean;
  forceResults: { document: string; outcome: string; error: string | null }[];
  nextActions: string[];
  reports: {
    round: string;
    initializationFailed: boolean;
    closeFailed: boolean;
    releaseFailures: string;
    coordinationErrors: string[];
  }[];
}
export type Response =
  | {
      kind: "document_progress";
      requested: boolean;
      files: string;
      phase: number;
    }
  | { kind: "ui_close"; enabled: boolean; attempt: Id | null; closing: boolean }
  | { kind: "retained_list"; entries: RetainedRef[] }
  | { kind: "reserved"; operation: Id }
  | { kind: "submitted"; operation: Id }
  | { kind: "acknowledged" }
  | {
      kind: "operation";
      operation: Id;
      state: "reserved" | "pending" | "complete" | "rejected";
      result: ResultDto | null;
      retained: RetainedRef | null;
    }
  | {
      kind: "retained";
      retained: RetainedRef;
      project: Id;
      artifacts: string[];
      intent: G6Intent | null;
      result: ResultDto;
      g6_clearable: boolean;
    }
  | {
      kind: "project";
      project: Id;
      collaborative?: boolean;
      status: string;
      runtime: string | null;
      error: BoundaryError | null;
      validation_failures: {
        session: Id | null;
        category: string;
        lockCategory: string | null;
        preserveFailed: boolean;
      }[];
      shutdown: Shutdown;
    }
  | {
      kind: "session";
      session: Id;
      state: string;
      active_dirty: boolean;
      custody: string | null;
      sink_connected: boolean;
    }
  | {
      kind: "app";
      generation: string;
      closing: boolean;
      projects: string;
      operations: string;
      retained_edits: string;
      normal_exit_allowed: boolean;
      event_error: string | null;
      support_diagnostics: {
        feature: string;
        stage: string;
        category: string;
        causeId: string | null;
        observedAtUtc: string | null;
      }[];
      diagnostics: {
        available: boolean;
        previousExitUnconfirmed: number;
        retainedEvents: number;
        droppedEvents: number;
      };
      native_cleanup: {
        phase: "complete" | "registered" | "pending" | "running" | "failed";
        generation: string;
        attempts: string;
        firstError: string | null;
        latestError: string | null;
        nextAction: string | null;
      };
    };
export interface Hint {
  generation: string;
}
