/** DOM 회귀 전용 transport. 실제 Tauri/디스크 증거로 집계하지 않는다. */
import type { Transport } from "../bridge/client";
import type {
  Command,
  Hint,
  Id,
  Response,
  ResultDto,
  RetainedRef,
  Template,
  Work,
} from "../bridge/types";

type CloseAction = "ui_ready" | "ui_close_status" | "ui_close_decision";
type CloseCommand = Extract<Command, { action: CloseAction }>;
type CloseReply = Extract<Response, { kind: "ui_close" }>;

export class TestTransport implements Transport {
  workspaceResult?: (input: Work) => ResultDto | undefined;
  readonly commands: Command[] = [];
  readonly writes: { operation: Id; input: Work }[] = [];
  readonly templates = new Map<Id, Template>();
  readonly views = new Map<Id, Template>();
  readonly released: Id[] = [];
  readonly retained = new Map<Id, Extract<Response, { kind: "retained" }>>();
  readonly results = new Map<Id, Extract<Response, { kind: "operation" }>>();
  readonly sessions = new Set<Id>();
  readonly backups: import("../bridge/types").BackupRow[] = [];
  readonly deletedBackups: import("../bridge/types").DeletedBackupRow[] = [];
  assetInspection: import("../bridge/types").AssetInspection = {
    token: "inspection-one",
    observedAtUtc: "2026-09-21T00:00:00.000Z",
    complete: true,
    scannedFiles: 3,
    referencedAssets: 1,
    usedAssets: 1,
    unusedAssets: 1,
    missingAssets: 0,
    corruptAssets: 0,
    uncertainAssets: 0,
    rows: [
      {
        id: "11111111-1111-4111-8111-111111111111",
        name: "used.bin",
        size: 4,
        status: "used",
        reason: "stored_reference",
      },
      {
        id: "22222222-2222-4222-8222-222222222222",
        name: "unused.bin",
        size: 6,
        status: "unused",
        reason: null,
      },
    ],
    trash: [],
    deletedTemplates: [],
    documentIssues: [],
  };
  project = "project-one";
  nextProjectId: string | null = null;
  backupDeleteOutcome: "quarantined" | "quarantined_unverified" = "quarantined";
  omitDeletedBackupRows = false;
  deletedListWarning: string | null = null;
  private serial = 0;
  private generation = 0;
  private callback?: (hint: Hint) => void;
  private readonly closeGates = new Map<
    CloseAction,
    {
      observed: (value: { command: CloseCommand; reply: CloseReply }) => void;
      wait: Promise<void>;
    }
  >();
  enabled = false;
  closing = false;
  attempt: Id | null = null;
  status = "Ready";
  runtime: string | null = "Ready";
  projectStopped = false;
  projectCloseRequested = false;
  reportPending = false;
  shutdownBlockers: string[] = [];
  shutdownReleaseFailures = "0";
  emptyShutdownReportReads = 0;
  gateListener: Promise<void> | null = null;
  gateReserve: Promise<void> | null = null;
  listenerCount = 0;
  hold: Work["kind"] | null = null;
  loseSubmit = false;
  loseResult = false;
  loseAck = false;
  loseDecision = false;
  loseRetainedRead = false;
  loseProjectStatus = false;
  failList = false;
  failRead = false;
  refuseWriteOnce = false;
  failRelease = false;
  cleanup = false;
  uncertain = false;
  rejectWrite = false;
  deletion: Extract<ResultDto, { kind: "write" }>["deletion"] = undefined;
  initializationFailed = false;
  initializationCleanupOutcome:
    "removed" | "preserved_external_entries" | "failed" | null = null;
  projectNotEmpty = false;
  defaultProjectRoot: string | null = null;
  failSettingsRead = false;
  failSettingsWrite = false;
  settingsWriteUncertain = false;
  failSettingsReadback = false;
  canonicalizeSettingsWrite = false;
  supportDiagnostics: Extract<
    Response,
    { kind: "app" }
  >["support_diagnostics"] = [];
  supportDiagnosticChanged(
    diagnostics: Extract<Response, { kind: "app" }>["support_diagnostics"],
  ) {
    this.supportDiagnostics = structuredClone(diagnostics);
    this.hint();
  }
  private held: { operation: Id; input: Work } | null = null;
  private id() {
    return `id-${++this.serial}`;
  }
  async listen(_event: "guarded-state", callback: (hint: Hint) => void) {
    ++this.listenerCount;
    if (this.gateListener) await this.gateListener;
    this.callback = callback;
    return () => {
      this.callback = undefined;
    };
  }
  hint() {
    this.callback?.({ generation: String(++this.generation) });
  }
  nativeClose(notify = true) {
    this.attempt ??= this.id();
    if (notify) this.hint();
  }
  /** backend 처리를 먼저 끝내고 응답만 지연/유실해 요청 순서를 결정적으로 검증한다. */
  deferClose(action: CloseAction) {
    if (this.closeGates.has(action)) throw new Error("duplicate close gate");
    let release!: () => void;
    let lose!: () => void;
    let observed!: (value: {
      command: CloseCommand;
      reply: CloseReply;
    }) => void;
    const received = new Promise<{ command: CloseCommand; reply: CloseReply }>(
      (resolve) => {
        observed = resolve;
      },
    );
    const wait = new Promise<void>((resolve, reject) => {
      release = resolve;
      lose = () => reject(new Error("M271_RAW_ERROR_CANARY"));
    });
    this.closeGates.set(action, { observed, wait });
    return { received, release, lose };
  }
  private async closeResponse(command: CloseCommand): Promise<CloseReply> {
    const reply = this.close();
    const gate = this.closeGates.get(command.action);
    if (gate) {
      this.closeGates.delete(command.action);
      gate.observed({ command, reply });
      await gate.wait;
    }
    return reply;
  }
  private close(): Extract<Response, { kind: "ui_close" }> {
    return {
      kind: "ui_close",
      enabled: this.enabled,
      attempt: this.attempt,
      closing: this.closing,
    };
  }
  private shutdown() {
    const hideReport = this.reportPending && this.emptyShutdownReportReads > 0;
    if (hideReport) --this.emptyShutdownReportReads;
    return {
      phase: this.projectStopped ? "Complete" : "Active",
      round: "1",
      blockers: this.shutdownBlockers,
      reportPending: this.reportPending,
      resourcesComplete: this.projectStopped,
      normalExitAllowed:
        this.projectStopped &&
        !this.reportPending &&
        !this.initializationFailed,
      closing: this.closing || this.projectCloseRequested,
      forceActive: false,
      forceResults: [],
      joined: this.projectStopped,
      nextActions: [],
      reports:
        this.reportPending && !hideReport
          ? [
              {
                round: "1",
                initializationFailed: this.initializationFailed,
                closeFailed: false,
                releaseFailures: this.shutdownReleaseFailures,
                coordinationErrors: [],
              },
            ]
          : [],
    };
  }
  completeHeld() {
    const held = this.held;
    if (!held) throw new Error("missing gate");
    this.held = null;
    this.complete(held.operation, held.input);
    this.hint();
  }
  private complete(operation: Id, input: Work) {
    const workspace = this.workspaceResult?.(input);
    if (workspace) {
      this.results.set(operation, {
        kind: "operation",
        operation,
        state: "complete",
        result: workspace,
        retained: null,
      });
      return;
    }
    let result: ResultDto;
    let retained: RetainedRef | null = null;
    const rejected = (): ResultDto => ({
      kind: "rejected",
      error: { code: "save_rejected", nextAction: "M271_RAW_ERROR_CANARY" },
      input_retained: true,
    });
    switch (input.kind) {
      case "open":
      case "create_project":
        if (this.nextProjectId) {
          this.project = this.nextProjectId;
          this.nextProjectId = null;
        }
        this.projectStopped = false;
        this.projectCloseRequested = false;
        this.reportPending = false;
        this.shutdownBlockers = [];
        result = {
          kind: "open",
          project: this.project,
          status: this.status,
          runtime: this.runtime,
          error: this.projectNotEmpty
            ? { code: "project_not_empty", nextAction: "" }
            : null,
          created: input.kind === "create_project",
        };
        break;
      case "project_settings_read":
        result = this.failSettingsRead
          ? {
              kind: "rejected",
              error: { code: "settings_read_failed", nextAction: "" },
              input_retained: false,
            }
          : {
              kind: "project_settings",
              default_root: this.defaultProjectRoot,
            };
        break;
      case "project_settings_write":
        if (this.failSettingsWrite) {
          result = {
            kind: "project_settings_write",
            outcome: "not_applied",
            observed: !this.failSettingsReadback,
            default_root: this.failSettingsReadback
              ? null
              : this.defaultProjectRoot,
            error: { code: "settings_write_failed", nextAction: "" },
            readback_error: this.failSettingsReadback
              ? { code: "settings_read_failed", nextAction: "" }
              : null,
          };
        } else {
          this.defaultProjectRoot =
            this.canonicalizeSettingsWrite && input.default_root
              ? `\\\\?\\${input.default_root}`
              : input.default_root;
          result = {
            kind: "project_settings_write",
            outcome: this.settingsWriteUncertain
              ? "applied_durability_uncertain"
              : "applied",
            observed: !this.failSettingsReadback,
            default_root: this.failSettingsReadback
              ? null
              : this.defaultProjectRoot,
            error: this.settingsWriteUncertain
              ? { code: "settings_write_failed", nextAction: "" }
              : null,
            readback_error: this.failSettingsReadback
              ? { code: "settings_read_failed", nextAction: "" }
              : null,
          };
        }
        break;
      case "project_copy":
        result = {
          kind: "project_data",
          action: "copy",
          root: `${input.parent}\\${input.name}`,
          backup: null,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: "published_verified",
          warning: null,
        };
        break;
      case "backup_create": {
        const backup = {
          id: this.id(),
          label: input.label ?? "",
          createdAtUtc: "2026-09-21T00:00:00.000Z",
          kind: "manual" as const,
          size: "12",
          status: "verified" as const,
          coverage: "complete" as const,
          unnamedOrdinal: null,
          locator: `${input.storage}\\worldbuild-backups\\fixture`,
        };
        this.backups.unshift(backup);
        result = {
          kind: "project_data",
          action: "backup_create",
          root: null,
          backup,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: "published_verified",
          warning: null,
        };
        break;
      }
      case "backup_list": {
        const start = input.cursor
          ? Math.max(
              0,
              this.backups.findIndex((backup) => backup.id === input.cursor) +
                1,
            )
          : 0;
        const backups = structuredClone(this.backups.slice(start, start + 100));
        result = {
          kind: "project_data",
          action: "backup_list",
          root: null,
          backup: null,
          safety: null,
          backups,
          nextCursor:
            start + backups.length < this.backups.length
              ? (backups[backups.length - 1]?.id ?? null)
              : null,
          recoveryRequired: false,
          outcome: null,
          warning: null,
        };
        break;
      }
      case "backup_delete": {
        const removed = this.backups.find(
          (backup) => backup.locator === input.locator,
        );
        const deletedAt = Date.now();
        const deleted =
          removed && this.backupDeleteOutcome === "quarantined"
            ? {
                backup: removed,
                deletedAtUtc: new Date(deletedAt).toISOString(),
                expiresAtUtc: new Date(
                  deletedAt + 7 * 24 * 60 * 60 * 1000,
                ).toISOString(),
                operation: this.id(),
                status: "verification_required" as const,
              }
            : null;
        if (removed) {
          this.backups.splice(
            0,
            this.backups.length,
            ...this.backups.filter(
              (backup) => backup.locator !== input.locator,
            ),
          );
        }
        if (deleted) this.deletedBackups.unshift(deleted);
        result = {
          kind: "project_data",
          action: "backup_delete",
          root: null,
          backup: null,
          safety: null,
          backups: [],
          deleted,
          nextCursor: null,
          recoveryRequired: false,
          outcome: this.backupDeleteOutcome,
          warning:
            this.backupDeleteOutcome === "quarantined_unverified"
              ? "backup_quarantine_readback_required"
              : null,
        };
        break;
      }
      case "backup_deleted_list":
        result = {
          kind: "project_data",
          action: "backup_deleted_list",
          root: null,
          backup: null,
          safety: null,
          backups: [],
          ...(this.omitDeletedBackupRows
            ? {}
            : { deletedBackups: structuredClone(this.deletedBackups) }),
          nextCursor: null,
          recoveryRequired: false,
          outcome: null,
          warning: this.deletedListWarning,
        };
        break;
      case "backup_deleted_restore": {
        const found = this.deletedBackups.find(
          (entry) =>
            entry.backup.id === input.id && entry.operation === input.operation,
        );
        if (found) {
          this.backups.unshift(found.backup);
          this.deletedBackups.splice(this.deletedBackups.indexOf(found), 1);
        }
        result = {
          kind: "project_data",
          action: "backup_deleted_restore",
          root: null,
          backup: null,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: found ? "restored" : "restore_unverified",
          warning: null,
        };
        break;
      }
      case "backup_deleted_purge": {
        const index = this.deletedBackups.findIndex(
          (entry) =>
            entry.backup.id === input.id && entry.operation === input.operation,
        );
        if (index >= 0) this.deletedBackups.splice(index, 1);
        result = {
          kind: "project_data",
          action: "backup_deleted_purge",
          root: null,
          backup: null,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: index >= 0 ? "deleted" : "partially_deleted",
          warning: null,
        };
        break;
      }
      case "backup_inspect": {
        const found = this.backups.find(
          (backup) => backup.locator === input.locator,
        );
        result = {
          kind: "project_data",
          action: "inspect",
          root: null,
          backup: found
            ? { ...structuredClone(found), status: "verified" }
            : {
                id: "backup-one",
                label: "",
                createdAtUtc: "2026-09-21T00:00:00.000Z",
                kind: "manual",
                size: "12",
                status: "verified",
                coverage: "complete",
                unnamedOrdinal: null,
                locator: input.locator,
              },
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: null,
          warning: null,
        };
        break;
      }
      case "restore_new":
        result = {
          kind: "project_data",
          action: "restore_new",
          root: `${input.parent}\\${input.name}`,
          backup: null,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: "published_verified",
          warning: null,
        };
        break;
      case "restore_current":
        result = {
          kind: "project_data",
          action: "restore_current",
          root: "C:\\fixture",
          backup: null,
          safety: null,
          backups: [],
          nextCursor: null,
          recoveryRequired: false,
          outcome: "applied_verified",
          warning: null,
        };
        break;
      case "asset_inspect":
        result = {
          kind: "asset_maintenance",
          action: "inspect",
          inspection: structuredClone(this.assetInspection),
          completed: [],
          completedCount: 0,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      case "asset_trash_move": {
        const moved = this.assetInspection.rows.filter((row) =>
          input.assets.includes(row.id),
        );
        this.assetInspection.rows = this.assetInspection.rows.filter(
          (row) => !input.assets.includes(row.id),
        );
        this.assetInspection.trash.push(
          ...moved.map((row) => ({
            id: row.id,
            name: row.name ?? "unknown.bin",
            size: row.size ?? 0,
            reclaimableSize: row.size ?? 0,
            movedAtUtc: "2026-09-21T00:01:00.000Z",
            protected: false,
            reason: null,
          })),
        );
        this.assetInspection.unusedAssets = this.assetInspection.rows.filter(
          (row) => row.status === "unused",
        ).length;
        this.assetInspection.token = "inspection-two";
        result = {
          kind: "asset_maintenance",
          action: "trash_move",
          inspection: structuredClone(this.assetInspection),
          completed: input.assets,
          completedCount: input.assets.length,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      }
      case "asset_rename": {
        const row = this.assetInspection.rows.find(
          (item) => item.id === input.asset,
        );
        if (row) row.name = input.name;
        this.assetInspection.token = "inspection-after-rename";
        result = {
          kind: "asset_maintenance",
          action: "asset_rename",
          inspection: structuredClone(this.assetInspection),
          completed: [input.asset],
          completedCount: 1,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      }
      case "asset_trash_restore": {
        const restored = this.assetInspection.trash.filter((row) =>
          input.assets.includes(row.id),
        );
        this.assetInspection.trash = this.assetInspection.trash.filter(
          (row) => !input.assets.includes(row.id),
        );
        this.assetInspection.rows.push(
          ...restored.map((row) => ({
            id: row.id,
            name: row.name,
            size: row.size,
            status: "unused" as const,
            reason: null,
          })),
        );
        this.assetInspection.unusedAssets = this.assetInspection.rows.filter(
          (row) => row.status === "unused",
        ).length;
        result = {
          kind: "asset_maintenance",
          action: "trash_restore",
          inspection: structuredClone(this.assetInspection),
          completed: input.assets,
          completedCount: input.assets.length,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      }
      case "asset_trash_purge": {
        const ids = input.empty
          ? this.assetInspection.trash
              .filter((row) => !row.protected)
              .map((row) => row.id)
          : input.assets;
        this.assetInspection.trash = this.assetInspection.trash.filter(
          (row) => !ids.includes(row.id),
        );
        this.assetInspection.token = "inspection-after-trash-purge";
        result = {
          kind: "asset_maintenance",
          action: "trash_purge",
          inspection: structuredClone(this.assetInspection),
          completed: ids.slice(0, 100),
          completedCount: ids.length,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      }
      case "template_purge": {
        this.assetInspection.deletedTemplates =
          this.assetInspection.deletedTemplates.filter(
            (row) => !input.templates.includes(row.id),
          );
        this.assetInspection.token = "inspection-after-template-purge";
        result = {
          kind: "asset_maintenance",
          action: "template_purge",
          inspection: structuredClone(this.assetInspection),
          completed: input.templates.slice(0, 100),
          completedCount: input.templates.length,
          failures: [],
          partial: false,
          cleanupRequired: [],
        };
        break;
      }
      case "template_restore": {
        this.assetInspection.deletedTemplates =
          this.assetInspection.deletedTemplates.filter(
            (row) => row.id !== input.template,
          );
        const session = this.id();
        result = {
          kind: "write",
          session,
          artifact: input.template,
          disk: "committed",
          changed: true,
          warnings: [],
          cleanup_failed: false,
          recovery_required: false,
          error: null,
          diagnostic: {
            stage: "Write",
            category: null,
            sessionState: "Released",
            lockCategory: null,
            nextAction: "",
          },
        };
        break;
      }
      case "diagnostic_export":
        result = {
          kind: "diagnostic_export",
          result: {
            outcome: "published",
            size: 512,
            sha256: "a".repeat(64),
            eventCount: 3,
            excluded: ["document_content"],
            cleanupRequired: false,
          },
        };
        break;
      case "list_templates":
        result = this.failList
          ? {
              kind: "rejected",
              error: {
                code: "repository_rejected",
                nextAction: "M271_RAW_ERROR_CANARY",
              },
              input_retained: false,
            }
          : {
              kind: "templates",
              templates: [...this.templates.values()].map((t) => ({
                id: t.id,
                name: t.name,
                revision: t.revision,
                lifecycle: t.lifecycle,
              })),
            };
        break;
      case "read_template": {
        if (this.failRead) {
          result = {
            kind: "rejected",
            error: {
              code: "repository_rejected",
              nextAction: "M271_RAW_ERROR_CANARY",
            },
            input_retained: false,
          };
          break;
        }
        const template = this.templates.get(input.template);
        if (!template) throw new Error("missing test template");
        const view = this.id();
        this.views.set(view, structuredClone(template));
        result = { kind: "template", view, content: structuredClone(template) };
        break;
      }
      case "begin_session": {
        const session = this.id();
        this.sessions.add(session);
        result = { kind: "session", session, state: "Editing", error: null };
        break;
      }
      case "create_template":
      case "update_template": {
        this.writes.push({ operation, input: structuredClone(input) });
        const source =
          input.kind === "update_template"
            ? this.views.get(input.view)
            : undefined;
        const current = source ? this.templates.get(source.id) : undefined;
        if (
          this.rejectWrite ||
          (input.kind === "update_template" &&
            (!current || current.revision !== input.revision))
        ) {
          result = rejected();
          break;
        }
        const session =
          input.kind === "create_template" ? this.id() : input.session;
        this.sessions.add(session);
        const name =
          input.kind === "create_template"
            ? input.name
            : input.edit.kind === "name"
              ? input.edit.name
              : (current?.name ?? "unexpected");
        const id = source?.id ?? this.id();
        const edited = current ? structuredClone(current) : undefined;
        if (edited && input.kind === "update_template") {
          const edit = input.edit;
          const field =
            "field" in edit
              ? edited.fields.find((f) => f.id === edit.field)
              : undefined;
          if (edit.kind === "name") edited.name = edit.name;
          if (edit.kind === "reorder_fields") edited.fieldOrder = edit.fields;
          if (edit.kind === "create_field") {
            const options =
              "options" in edit.configuration ? edit.configuration.options : [];
            const kind = {
              single_line_text: "SingleLineText",
              rich_text: "RichText",
              number: "Number",
              date: "Date",
              time: "Time",
              image: "Image",
              file: "File",
              url: "Url",
              duration: "Duration",
              single_choice: "SingleChoice",
              multi_choice: "MultiChoice",
              relation: "Relation",
              document_link: "DocumentLink",
            }[edit.configuration.kind];
            if (!kind) throw new Error("unsupported test field kind");
            edited.fieldOrder.push(edit.field);
            edited.fields.push({
              id: edit.field,
              label: edit.label,
              kind,
              lifecycle: "Active",
              required: edit.required,
              presentation: edit.presentation,
              default: edit.default,
              initialDefault: edit.default,
              introducedRevision: String(BigInt(edited.revision) + 1n),
              options: options.map((o) => ({ ...o, lifecycle: "Active" })),
              optionOrder: options.map((o) => o.id),
            });
          }
          if (field) {
            if (edit.kind === "add_option") {
              field.options.push({ ...edit.option, lifecycle: "Active" });
              field.optionOrder.splice(
                edit.index === null
                  ? field.optionOrder.length
                  : Number(edit.index),
                0,
                edit.option.id,
              );
            }
            if (edit.kind === "rename_option") {
              const option = field.options.find((o) => o.id === edit.option);
              if (option) option.label = edit.label;
            }
            if (edit.kind === "reorder_options")
              field.optionOrder = edit.options;
            if (edit.kind === "archive_option") {
              const option = field.options.find((o) => o.id === edit.option);
              if (option) option.lifecycle = "Archived";
              field.optionOrder = field.optionOrder.filter(
                (id) => id !== edit.option,
              );
              if (edit.repair) field.default = edit.repair;
            }
            if (edit.kind === "archive_field") {
              field.lifecycle = "Archived";
              edited.fieldOrder = edited.fieldOrder.filter(
                (id) => id !== field.id,
              );
            }
            if (edit.kind === "field_label") field.label = edit.label;
            if (edit.kind === "field_required") field.required = edit.required;
            if (edit.kind === "field_presentation")
              field.presentation = edit.token;
            if (edit.kind === "default") field.default = edit.value;
          }
        }
        const changed =
          !current || JSON.stringify(edited) !== JSON.stringify(current);
        if (!this.uncertain)
          this.templates.set(
            id,
            current
              ? {
                  ...edited!,
                  name,
                  revision: changed
                    ? String(BigInt(current.revision) + 1n)
                    : current.revision,
                }
              : {
                  id,
                  name,
                  revision: "1",
                  lifecycle: "Active",
                  presentation: null,
                  fieldOrder: [],
                  fields: [],
                },
          );
        result = {
          kind: "write",
          session,
          artifact: id,
          disk: this.uncertain
            ? "uncertain"
            : changed
              ? "committed"
              : "no_write",
          changed,
          warnings: [],
          cleanup_failed: this.cleanup,
          recovery_required: this.uncertain,
          error: this.uncertain
            ? { code: "save_rejected", nextAction: "M271_RAW_ERROR_CANARY" }
            : null,
          diagnostic: {
            stage: "Write",
            category: null,
            sessionState: "Editing",
            lockCategory: null,
            nextAction: "M271_RAW_ERROR_CANARY",
          },
        };
        break;
      }
      case "duplicate_template":
      case "tombstone_template": {
        this.writes.push({ operation, input: structuredClone(input) });
        const source = this.views.get(input.view)!;
        const session =
          input.kind === "duplicate_template" ? this.id() : input.session;
        this.sessions.add(session);
        if (this.rejectWrite) {
          result = rejected();
          break;
        }
        const id = input.kind === "duplicate_template" ? this.id() : source.id;
        const deletion =
          input.kind === "tombstone_template" ? this.deletion : undefined;
        if (!deletion && !this.uncertain)
          this.templates.set(id, {
            ...structuredClone(source),
            id,
            revision:
              input.kind === "duplicate_template"
                ? "1"
                : String(BigInt(source.revision) + 1n),
            lifecycle:
              input.kind === "duplicate_template" ? "Active" : "Deleted",
          });
        result = {
          kind: "write",
          session,
          artifact: id,
          disk: deletion
            ? "not_attempted"
            : this.uncertain
              ? "uncertain"
              : "committed",
          changed: !deletion,
          warnings: [],
          cleanup_failed: this.cleanup,
          recovery_required: this.uncertain,
          error: deletion
            ? { code: "save_rejected", nextAction: "M274_RAW_CANARY" }
            : null,
          diagnostic: {
            stage: "Build",
            category: deletion ? "DomainRejected" : null,
            sessionState: "Editing",
            lockCategory: null,
            nextAction: "M274_RAW_CANARY",
          },
          deletion,
        };
        break;
      }
      case "session_control": {
        if (this.failRelease)
          result = {
            kind: "control",
            error: {
              code: "release_rejected",
              nextAction: "M271_RAW_ERROR_CANARY",
            },
          };
        else {
          if (input.control === "end") this.sessions.delete(input.session);
          result = { kind: "control", error: null };
        }
        break;
      }
      case "abandon_retained": {
        const owner = this.retained.get(input.retained.id);
        if (!owner?.g6_clearable)
          result = {
            kind: "rejected",
            error: { code: "owners_remain", nextAction: "" },
            input_retained: false,
          };
        else {
          this.retained.delete(input.retained.id);
          result = {
            kind: "retained_handled",
            retained: input.retained,
            action: "abandoned",
          };
        }
        break;
      }
      case "close":
        this.projectCloseRequested = true;
        this.projectStopped = true;
        this.reportPending = true;
        result = { kind: "control", error: null };
        break;
      case "retire_project":
        result = {
          kind: "project_retired",
          project: this.project,
          shutdown: this.shutdown(),
        };
        break;
      case "recover":
        result = { kind: "control", error: null };
        break;
      default:
        throw new Error(`unexpected test work ${input.kind}`);
    }
    if (
      (input.kind === "create_template" ||
        input.kind === "update_template" ||
        input.kind === "duplicate_template" ||
        input.kind === "tombstone_template") &&
      (result.kind === "rejected" ||
        this.uncertain ||
        (result.kind === "write" && result.disk === "not_attempted"))
    ) {
      retained = { id: this.id(), generation: this.id() };
      this.retained.set(retained.id, {
        kind: "retained",
        retained,
        project: input.project,
        artifacts: [],
        intent:
          input.kind === "create_template"
            ? {
                kind: "create_template",
                name: input.name,
                presentation: input.presentation,
              }
            : input.kind === "update_template"
              ? {
                  kind: "update_template",
                  revision: input.revision,
                  edit: input.edit,
                }
              : input.kind === "tombstone_template"
                ? { kind: "tombstone_template", revision: input.revision }
                : { kind: "duplicate_template" },
        result,
        g6_clearable: !this.uncertain,
      });
    }
    this.results.set(operation, {
      kind: "operation",
      operation,
      state: "complete",
      result,
      retained,
    });
  }
  async invoke(_command: "guarded", body: Uint8Array): Promise<Response> {
    const command = JSON.parse(new TextDecoder().decode(body)) as Command;
    this.commands.push(command);
    switch (command.action) {
      case "ui_ready":
        this.enabled = true;
        return this.closeResponse(command);
      case "ui_close_status":
        return this.closeResponse(command);
      case "ui_close_decision":
        if (command.attempt !== this.attempt)
          throw { code: "wrong_binding", nextAction: "" };
        this.attempt = null;
        this.closing = command.proceed;
        if (this.loseDecision) {
          this.loseDecision = false;
          throw new Error("M271_RAW_ERROR_CANARY");
        }
        return this.closeResponse(command);
      case "app_shutdown":
        this.nativeClose();
        return this.invokeApp();
      case "app_status":
        return this.invokeApp();
      case "project_status":
        if (this.loseProjectStatus) throw new Error("M271_RAW_ERROR_CANARY");
        return {
          kind: "project",
          project: this.project,
          status: this.projectStopped ? "Stopped" : this.status,
          runtime: this.runtime,
          error: this.initializationFailed
            ? {
                code: "initialization_failed",
                nextAction: "M274_RAW_CANARY",
                diagnostic: this.initializationCleanupOutcome
                  ? {
                      stage: "Initialize",
                      category: "ProjectInitializationFailed",
                      cleanupOutcome: this.initializationCleanupOutcome,
                    }
                  : undefined,
              }
            : this.projectNotEmpty
              ? { code: "project_not_empty", nextAction: "" }
              : null,
          validation_failures: [],
          shutdown: this.shutdown(),
        };
      case "session_status":
        return {
          kind: "session",
          session: command.session,
          state: this.failRelease ? "ReleaseFailed" : "Editing",
          active_dirty: false,
          custody: null,
          sink_connected: false,
        };
      case "document_progress":
        if (
          command.cancel &&
          this.held?.operation === command.operation &&
          this.held.input.kind === "document_workspace" &&
          ["replace_preview", "replace_page", "replace_apply"].includes(
            this.held.input.request.action,
          )
        ) {
          this.results.set(command.operation, {
            kind: "operation",
            operation: command.operation,
            state: "rejected",
            result: {
              kind: "rejected",
              error: { code: "cancelled", nextAction: "" },
              input_retained: false,
            },
            retained: null,
          });
          this.held = null;
        }
        return {
          kind: "document_progress",
          requested: command.cancel,
          files: "0",
          phase: 0,
        };
      case "reserve": {
        if (this.gateReserve) await this.gateReserve;
        const operation = this.id();
        this.results.set(operation, {
          kind: "operation",
          operation,
          state: "reserved",
          result: null,
          retained: null,
        });
        return { kind: "reserved", operation };
      }
      case "abandon_reservation": {
        const result = this.results.get(command.operation);
        if (!result) throw { code: "unknown_id", nextAction: "" };
        if (result.state !== "reserved")
          throw { code: "not_terminal", nextAction: "" };
        this.results.delete(command.operation);
        return { kind: "acknowledged" };
      }
      case "submit": {
        if (
          this.refuseWriteOnce &&
          [
            "update_template",
            "duplicate_template",
            "tombstone_template",
          ].includes(command.input.kind)
        ) {
          this.refuseWriteOnce = false;
          throw { code: "full", nextAction: "M271_RAW_ERROR_CANARY" };
        }
        if (this.results.get(command.operation)?.state === "reserved") {
          if (command.input.kind === this.hold) {
            this.results.set(command.operation, {
              kind: "operation",
              operation: command.operation,
              state: "pending",
              result: null,
              retained: null,
            });
            this.held = { operation: command.operation, input: command.input };
          } else this.complete(command.operation, command.input);
        }
        if (
          this.loseSubmit &&
          (command.input.kind === "create_template" ||
            command.input.kind === "update_template" ||
            command.input.kind === "duplicate_template" ||
            command.input.kind === "tombstone_template")
        ) {
          this.loseSubmit = false;
          throw new Error("M271_RAW_ERROR_CANARY");
        }
        return { kind: "submitted", operation: command.operation };
      }
      case "operation": {
        if (this.loseResult) {
          this.loseResult = false;
          throw new Error("M271_RAW_ERROR_CANARY");
        }
        const result = this.results.get(command.operation);
        if (!result)
          throw { code: "unknown_id", nextAction: "M271_RAW_ERROR_CANARY" };
        return result;
      }
      case "acknowledge_transport": {
        if (!this.results.delete(command.operation))
          throw { code: "unknown_id", nextAction: "M271_RAW_ERROR_CANARY" };
        if (this.loseAck) {
          this.loseAck = false;
          throw new Error("M271_RAW_ERROR_CANARY");
        }
        return { kind: "acknowledged" };
      }
      case "retained_list":
        return {
          kind: "retained_list",
          entries: [...this.retained.values()].map((r) => r.retained),
        };
      case "retained_read": {
        if (this.loseRetainedRead) throw new Error("M271_RAW_ERROR_CANARY");
        const retained = this.retained.get(command.retained.id);
        if (!retained) throw { code: "unknown_id", nextAction: "" };
        return retained;
      }
      case "release_view":
        this.views.delete(command.view);
        this.released.push(command.view);
        return { kind: "acknowledged" };
      case "acknowledge_shutdown":
        this.reportPending = false;
        this.shutdownBlockers = this.shutdownBlockers.filter(
          (b) => b !== "Results",
        );
        return { kind: "acknowledged" };
      case "release_round":
      case "retry_native_cleanup":
        return { kind: "acknowledged" };
      default:
        throw new Error(`unexpected test action ${command.action}`);
    }
  }
  private invokeApp(): Extract<Response, { kind: "app" }> {
    return {
      kind: "app",
      generation: String(this.generation),
      closing: this.closing,
      projects: "1",
      operations: String(this.results.size),
      retained_edits: String(this.retained.size),
      normal_exit_allowed: false,
      event_error: null,
      support_diagnostics: structuredClone(this.supportDiagnostics),
      diagnostics: {
        available: true,
        previousExitUnconfirmed: 0,
        retainedEvents: 0,
        droppedEvents: 0,
      },
      native_cleanup: {
        phase: "registered",
        generation: "0",
        attempts: "0",
        firstError: null,
        latestError: null,
        nextAction: null,
      },
    };
  }
}
