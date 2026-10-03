import { BridgeFailure, GuardedClient } from "../bridge/client";
import type {
  Id,
  BackupRow,
  DeletedBackupRow,
  AssetInspection,
  Response,
  ResultDto,
  RetainedRef,
  Template,
  TemplateSummary,
  Work,
} from "../bridge/types";
import { Operations, safeFailure, type OperationNotice } from "./operations";
import {
  editField,
  fieldDraft,
  fieldDirty,
  fieldKind,
  fieldInputError,
  optionKey,
  savedFieldDrafts,
  editOwner,
  sameEditTarget,
  orderIds,
  sourceOrder,
  exactOrder,
  moveId,
  managementError,
  type FieldEdit,
  type FieldEditor,
  type FieldProperty,
} from "./fieldEditing";
import { text } from "../strings";

type Project = Extract<Response, { kind: "project" }>;
type Retained = Extract<Response, { kind: "retained" }>;
type CloseStatus = Extract<Response, { kind: "ui_close" }>;
interface Connection {
  listening: boolean;
  registration: "idle" | "pending" | "unconfirmed" | "confirmed";
}
interface Selection {
  project: Id;
  view: Id;
  content: Template;
}
export interface TemplateAction {
  kind: "duplicate" | "delete";
  source: Selection;
  generation: number;
  phase: "confirm" | "pending" | "complete";
  handedOff: boolean;
  result: ResultDto | null;
  artifact: Id | null;
  message: string;
}
export interface Form {
  kind: "create" | "rename";
  name: string;
  initial: string;
  source: Selection | null;
  session: Id | null;
  submitted: boolean;
  handedOff: boolean;
}
export interface NewProjectDraft {
  name: string;
  error: string | null;
}
export interface ProjectDataDialog {
  kind: "copy" | "backups" | "restore_new";
  name: string;
  label: string;
  storage: string | null;
  backups: BackupRow[];
  deletedBackups?: DeletedBackupRow[];
  view?: "active" | "deleted";
  selectedDeleted?: string | null;
  nextCursor: string | null;
  selected: string | null;
  locator: string | null;
  error: string | null;
  detail?: string | null;
  noticeEvent?: number;
  errorSource?: "deleted_list" | "cleanup" | "other" | null;
  deletedCleanupWarning?: string | null;
  deletedCleanupBackupId?: string | null;
  deletedCleanupFacts?: DeletedCleanupFact[];
  deletedListWarning?: string | null;
  dialogGeneration?: number;
  confirm: "restore_current" | "delete" | "purge_deleted" | null;
  completedRoot: string | null;
  cancellable: boolean;
}
interface DeletedCleanupFact {
  id: string;
  operation: string;
  outcome:
    | "restored_receipt_cleanup_required"
    | "deleted_receipt_cleanup_required"
    | "partially_deleted";
  warning: string;
}

// Native returns bare IDs for receipt rows, but orphan marker/package rows use
// the exact entry name. This comparison only retains warnings; it never grants
// restore or purge permission to an uncertain row.
function cleanupRowMatches(id: string, row: DeletedBackupRow): boolean {
  if (row.backup.id === id) return true;
  const uuid =
    /^(?:[0-9a-f]{32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/iu;
  if (!uuid.test(id)) return false;
  const target = id.replace(/-/gu, "").toLowerCase();
  if (
    uuid.test(row.backup.id) &&
    row.backup.id.replace(/-/gu, "").toLowerCase() === target
  )
    return true;
  if (row.status !== "uncertain") return false;
  for (const suffix of [".purging", ".worldbuild-backup"]) {
    if (!row.backup.id.endsWith(suffix)) continue;
    const stem = row.backup.id.slice(0, -suffix.length);
    if (uuid.test(stem) && stem.replace(/-/gu, "").toLowerCase() === target)
      return true;
  }
  return false;
}

function recordCleanupFact(
  facts: DeletedCleanupFact[],
  fact: DeletedCleanupFact,
): DeletedCleanupFact[] {
  return [
    ...facts.filter(
      (item) => item.id !== fact.id || item.operation !== fact.operation,
    ),
    fact,
  ];
}
interface FeedbackNotice {
  id: number;
  message: string;
  undo?: {
    project: Id;
    storage: string;
    id: string;
    operation: string;
    generation: number;
    expiresAtUtc: string;
  };
}
export interface HealthDialog {
  surface: "health" | "resources" | "trash";
  check?: { id: number; documents: "checking" | "verified" | "unverified" };
  phase:
    | "idle"
    | "checking"
    | "ready"
    | "moving"
    | "restoring"
    | "purging"
    | "exporting"
    | "failed";
  inspection: AssetInspection | null;
  selected: string[];
  selectedTrash: string[];
  selectedTemplates: string[];
  confirm:
    | { kind: "trash_selected"; ids: string[] }
    | { kind: "trash_all"; ids: string[] }
    | { kind: "templates"; ids: string[] }
    | null;
  exportConfirm: boolean;
  message: string | null;
  error: string | null;
}
type Navigation =
  | {
      kind:
        | "create"
        | "cancel"
        | "close_project"
        | "new_field"
        | "close_field"
        | "fields_order"
        | "archive_field"
        | "duplicate"
        | "delete";
    }
  | { kind: "open_project"; root: string }
  | { kind: "select" | "field"; id: Id };
export interface Prompt {
  attempt: Id | null;
  navigation: Navigation | null;
}
interface Session {
  project: Id;
  id: Id;
  problem: string | null;
  observation: Extract<Response, { kind: "session" }> | null;
  views: Id[];
}
export interface ScreenState {
  ready: boolean;
  root: string;
  project: Project | null;
  projectId: Id | null;
  rows: TemplateSummary[];
  listState: "idle" | "loading" | "ready" | "failed";
  selection: Selection | null;
  form: Form | null;
  fieldEditor: FieldEditor | null;
  templateAction: TemplateAction | null;
  picking: boolean;
  startupLoading: boolean;
  defaultProjectRoot: string | null;
  defaultProjectObserved: boolean;
  startupFailure: string | null;
  startupFailureEvent: number;
  startupFailureKind: "settings_read" | "project_open" | null;
  settingsNotice: {
    kind: "not_applied" | "durability_uncertain";
    message: string;
  } | null;
  errorEvent: number;
  newProject: NewProjectDraft | null;
  projectData: ProjectDataDialog | null;
  health: HealthDialog | null;
  assetInspection: AssetInspection | null;
  inspectionPendingDocuments: string[];
  inspectionRefreshState?: "waiting" | "checking" | "failed" | "idle";
  fileManagerTarget: {
    surface: "resources" | "trash";
    keys: string[];
  } | null;
  prompt: Prompt | null;
  deciding: boolean;
  busy: boolean;
  closing: boolean;
  message: string;
  feedback: FeedbackNotice | null;
  error: string | null;
  errorDetail: string | null;
  operations: OperationNotice[];
  retained: Retained[];
  retainedRefs: RetainedRef[];
  sessions: Session[];
  app: Extract<Response, { kind: "app" }> | null;
}

function resultError(result: ResultDto): string | null {
  if ("error" in result && result.error) {
    return safeFailure(new BridgeFailure("boundary", undefined, result.error));
  }
  return null;
}
function cleanShutdownReport(shutdown: Project["shutdown"]): boolean {
  return (
    shutdown.reportPending &&
    !shutdown.forceActive &&
    (shutdown.reports.length > 0 || shutdown.forceResults.length > 0) &&
    // Native keeps Results while the successful report awaits its caller.
    // Acknowledging that report removes this owner; other blockers still need
    // their own explicit resolution before exit can be approved.
    shutdown.blockers.every((blocker) => blocker === "Results") &&
    shutdown.reports.every(
      (report) =>
        !report.initializationFailed &&
        !report.closeFailed &&
        report.releaseFailures === "0" &&
        report.coordinationErrors.length === 0,
    ) &&
    shutdown.forceResults.every((result) => result.outcome === "verified")
  );
}

function failureDetail(error: unknown): string | null {
  if (!(error instanceof BridgeFailure)) return null;
  const values = [
    error.boundary?.code,
    error.boundary?.diagnostic?.stage,
    error.boundary?.diagnostic?.category,
    error.boundary?.diagnostic?.outcome,
  ].filter(
    (value): value is string => !!value && /^[a-z0-9_]{1,64}$/u.test(value),
  );
  return values.length ? values.join(" · ") : null;
}
function requireKind<K extends ResultDto["kind"]>(
  result: ResultDto,
  kind: K,
): Extract<ResultDto, { kind: K }> {
  if (result.kind !== kind)
    throw new BridgeFailure(
      "boundary",
      undefined,
      result.kind === "rejected"
        ? result.error
        : { code: "unavailable", nextAction: "" },
    );
  return result as Extract<ResultDto, { kind: K }>;
}

function validProjectName(name: string) {
  if (
    !name ||
    name.length > 255 ||
    name !== name.trim() ||
    Array.from(name).some((character) => character.charCodeAt(0) <= 0x1f) ||
    /[<>:"/\\|?*]/u.test(name) ||
    /[. ]$/u.test(name) ||
    name === "." ||
    name === ".."
  )
    return false;
  const stem = name.split(".", 1)[0].toUpperCase();
  return !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/u.test(stem);
}

function backupStorageKey(root: string) {
  return `worldbuild.backup-location.v1:${root}`;
}

function rememberedBackupStorage(root: string): string | null {
  try {
    return root ? window.localStorage.getItem(backupStorageKey(root)) : null;
  } catch {
    return null;
  }
}

function rememberBackupStorage(root: string, storage: string) {
  try {
    if (root) window.localStorage.setItem(backupStorageKey(root), storage);
  } catch {
    // 완성 백업의 결과와 위치는 설정 기록 실패 때문에 실패로 바꾸지 않는다.
  }
}

function sameProjectPath(left: string, right: string) {
  const normalize = (value: string) => {
    let path = value.replace(/\\/gu, "/");
    path = /^\/\/\?\/UNC\//iu.test(path)
      ? `//${path.slice(8)}`
      : path.replace(/^\/\/\?\//u, "");
    return path.replace(/\/+$/u, "").toLocaleLowerCase();
  };
  return normalize(left) === normalize(right);
}

/** 폼, 고정 source와 제출 owner는 앱이 소유한다. 화면 unmount는 작업 취소가 아니다. */
export class TemplateController {
  private state: ScreenState = {
    ready: false,
    root: "",
    project: null,
    projectId: null,
    rows: [],
    listState: "idle",
    selection: null,
    form: null,
    fieldEditor: null,
    templateAction: null,
    picking: false,
    startupLoading: true,
    defaultProjectRoot: null,
    defaultProjectObserved: false,
    startupFailure: null,
    startupFailureEvent: 0,
    startupFailureKind: null,
    settingsNotice: null,
    errorEvent: 0,
    newProject: null,
    projectData: null,
    health: null,
    assetInspection: null,
    inspectionPendingDocuments: [],
    fileManagerTarget: null,
    prompt: null,
    deciding: false,
    busy: false,
    closing: false,
    message: text("controller.message01"),
    feedback: null,
    error: null,
    errorDetail: null,
    operations: [],
    retained: [],
    retainedRefs: [],
    sessions: [],
    app: null,
  };
  private readonly listeners = new Set<() => void>();
  readonly operations: Operations;
  // 전체 편집 화면의 앱 owner가 native close의 미제출 입력 판단을 맡는다.
  workspaceClose: ((attempt: Id | null) => void) | null = null;
  workspaceOwnsView: ((view: Id) => boolean) | null = null;
  workspaceInspectDocuments:
    ((current: () => boolean) => Promise<boolean>) | null = null;
  private connection: Connection | null = null;
  private closeRequest = 0;
  private decision: { prompt: Prompt; request: number } | null = null;
  private refreshing = false;
  private refreshAgain = false;
  private selectionGeneration = 0;
  private projectDataOperation: Id | null = null;
  private fieldGeneration = 0;
  private listGeneration = 0;
  private managementGeneration = 0;
  private pickerGeneration = 0;
  private startupGeneration = 0;
  private noticeSequence = 0;
  private retiredProject: Id | null = null;
  private startupStarted = false;
  private projectRequestGeneration = 0;
  private inspectionEpoch = 0;
  private healthRequest = 0;
  private inspectionRefreshing = false;
  private inspectionRefreshRequested = false;
  private inspectionReleaseRetryProject: string | null = null;
  private healthInspectionRequested = false;
  private healthInspectionProject: string | null = null;
  private readonly inspectionPending = new Set<string>();
  projectGeneration() {
    return this.projectRequestGeneration;
  }
  private backupListGeneration = 0;
  private backupDialogGeneration = 0;
  private feedbackSequence = 0;
  private readonly views = new Map<Id, Id>();
  constructor(
    readonly client = new GuardedClient(),
    private readonly folderPicker = async (
      purpose:
        "open" | "create" | "copy" | "backup" | "backup_snapshot" = "open",
    ) => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const defaultPath =
        purpose === "create" || purpose === "copy"
          ? await import("@tauri-apps/api/path").then(({ desktopDir }) =>
              desktopDir(),
            )
          : undefined;
      return open({
        directory: true,
        multiple: false,
        defaultPath,
        title: text(
          purpose === "create"
            ? "picker.createTitle"
            : purpose === "copy"
              ? "picker.copyTitle"
              : purpose === "backup"
                ? "picker.backupTitle"
                : purpose === "backup_snapshot"
                  ? "picker.backupSnapshotTitle"
                  : "picker.title",
        ),
      });
    },
  ) {
    this.operations = new Operations(client, () =>
      this.publish({ operations: this.operations.notices() }),
    );
  }
  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private publish(patch: Partial<ScreenState>) {
    // New failures are distinct requests even when their displayed text agrees.
    // Busy/disabled updates omit these fields and retain the current event.
    if (patch.error) patch = { ...patch, errorEvent: ++this.noticeSequence };
    if (patch.startupFailure)
      patch = { ...patch, startupFailureEvent: ++this.noticeSequence };
    if (
      patch.error !== undefined &&
      patch.error !== this.state.error &&
      patch.errorDetail === undefined
    )
      patch = { ...patch, errorDetail: null };
    const undo = this.state.feedback?.undo;
    if (
      undo &&
      (this.projectRequestGeneration !== undo.generation ||
        (patch.projectId !== undefined && patch.projectId !== undo.project) ||
        (patch.projectData && patch.projectData.storage !== undo.storage))
    )
      patch = { ...patch, feedback: null };
    this.state = { ...this.state, ...patch };
    if (
      this.healthInspectionRequested &&
      (this.healthInspectionProject !== this.state.projectId ||
        this.state.health?.surface !== "health")
    ) {
      this.healthInspectionRequested = false;
      this.healthInspectionProject = null;
    }
    for (const listener of this.listeners) listener();
    if (
      this.healthInspectionRequested &&
      !this.state.busy &&
      this.state.health?.surface === "health"
    ) {
      this.healthInspectionRequested = false;
      void this.inspectProjectHealth();
    }
  }
  dismissFeedback(id: number) {
    if (this.state.feedback?.id === id) this.publish({ feedback: null });
  }
  private completeFeedback(message: string) {
    if (this.state.feedback?.undo) return;
    this.publish({ feedback: { id: ++this.feedbackSequence, message } });
  }
  private backupUndoFeedback(
    message: string,
    project: Id,
    storage: string,
    deleted: DeletedBackupRow,
  ) {
    this.publish({
      feedback: {
        id: ++this.feedbackSequence,
        message,
        undo: {
          project,
          storage,
          id: deleted.backup.id,
          operation: deleted.operation,
          generation: this.projectRequestGeneration,
          expiresAtUtc: deleted.expiresAtUtc,
        },
      },
    });
  }
  async undoBackupDeletion() {
    const notice = this.state.feedback;
    const undo = notice?.undo;
    if (
      !undo ||
      this.state.busy ||
      this.state.projectId !== undo.project ||
      this.projectRequestGeneration !== undo.generation
    )
      return;
    if (Date.parse(undo.expiresAtUtc) <= Date.now()) {
      this.dismissFeedback(notice.id);
      return;
    }
    await this.restoreDeletedBackup(undo.storage, undo.id, undo.operation);
  }
  private startupGate: Promise<void> | null = null;
  setStartupGate(gate: Promise<void>) {
    if (!this.startupStarted) this.startupGate = gate;
  }
  start() {
    if (this.connection) return;
    const connection: Connection = { listening: false, registration: "idle" };
    this.connection = connection;
    this.client.subscribe(
      () => {
        if (this.connection === connection) void this.checkStatus();
      },
      (error) => {
        if (this.connection !== connection) return;
        this.connection = null;
        this.publish({ ready: false, error: safeFailure(error) });
      },
      () => {
        if (this.connection !== connection) return;
        connection.listening = true;
        void this.initialize(connection);
      },
    );
  }
  private confirmReady(close: CloseStatus, connection: Connection) {
    // status는 같은 caller의 등록만 증명한다. 현재 listener와 등록 요청도 있어야 편집을 연다.
    if (
      this.connection !== connection ||
      !connection.listening ||
      connection.registration === "idle" ||
      !close.enabled ||
      close.closing
    )
      return;
    connection.registration = "confirmed";
    if (!this.state.ready)
      this.publish({
        ready: true,
        error: null,
        message: text("controller.message02"),
      });
    void this.loadStartupSettings(connection);
  }
  private async initialize(connection: Connection) {
    if (
      this.connection !== connection ||
      !connection.listening ||
      connection.registration === "pending" ||
      connection.registration === "confirmed"
    )
      return;
    connection.registration = "pending";
    try {
      const close = await this.client.uiReady();
      if (!close.enabled || close.closing) throw new BridgeFailure("protocol");
      this.confirmReady(close, connection);
      if (this.connection === connection) {
        void this.checkStatus();
      }
    } catch (error: unknown) {
      // 먼저 성공한 readback을 늦은 등록 응답의 실패가 되돌리지 않는다.
      if (this.connection === connection && !this.state.ready)
        this.publish({ error: safeFailure(error) });
    } finally {
      if (connection.registration === "pending")
        connection.registration = "unconfirmed";
    }
  }
  private async action<T>(work: () => Promise<T>): Promise<T | undefined> {
    if (this.state.busy || this.state.prompt || this.state.deciding) return;
    const actionProject = this.state.projectId;
    this.publish({ busy: true });
    try {
      return await work();
    } catch (error: unknown) {
      if (
        actionProject &&
        this.state.projectId !== actionProject &&
        error instanceof BridgeFailure &&
        error.boundary?.code === "unknown_id"
      )
        return;
      const current = this.state.projectData;
      if (current)
        this.publish({
          projectData: {
            ...current,
            noticeEvent: ++this.noticeSequence,
            error: safeFailure(error),
            detail: failureDetail(error),
            errorSource: "other",
          },
        });
      else
        this.publish({
          error: safeFailure(error),
          errorDetail: failureDetail(error),
        });
    } finally {
      this.publish({ busy: false });
      await this.checkStatus();
    }
  }
  setRoot(root: string) {
    if (
      !this.state.busy &&
      !this.state.startupLoading &&
      !this.state.projectId &&
      !this.state.prompt &&
      !this.state.closing
    ) {
      ++this.pickerGeneration;
      this.publish({ root });
    }
  }
  private async loadStartupSettings(connection: Connection) {
    if (this.startupStarted) return;
    this.startupStarted = true;
    if (this.startupGate) {
      this.publish({ startupLoading: true });
      await this.startupGate;
      if (this.connection !== connection || this.state.closing) return;
    }
    await this.readStartupSettings(connection, true);
  }
  private async readStartupSettings(
    connection: Connection,
    openDefault: boolean,
  ) {
    const generation = ++this.startupGeneration;
    this.publish({ startupLoading: true });
    try {
      const { result } = await this.operations.run(
        { kind: "project_settings_read" },
        text("project.defaultReading"),
      );
      if (
        this.connection !== connection ||
        generation !== this.startupGeneration ||
        this.state.closing
      )
        return;
      if (result.kind !== "project_settings") {
        this.publish({
          startupFailure:
            resultError(result) ?? text("project.defaultReadFailed"),
          startupFailureKind: "settings_read",
          defaultProjectObserved: false,
        });
        return;
      }
      const notice = this.state.settingsNotice;
      this.publish({
        defaultProjectRoot: result.default_root,
        defaultProjectObserved: true,
        startupFailure: null,
        startupFailureKind: null,
        settingsNotice: notice
          ? {
              ...notice,
              message: text(
                notice.kind === "durability_uncertain"
                  ? "project.defaultUncertainObserved"
                  : "project.defaultNotAppliedObserved",
              ),
            }
          : null,
      });
      if (openDefault && result.default_root && !this.state.projectId) {
        const problem = await this.openProject(
          result.default_root,
          "automatic",
          false,
        );
        if (problem) {
          await this.retireInitializationFailure();
          if (
            this.connection === connection &&
            generation === this.startupGeneration
          )
            this.publish({
              startupFailure: problem,
              startupFailureKind: "project_open",
              error: null,
            });
        }
      }
    } catch (error: unknown) {
      if (
        this.connection === connection &&
        generation === this.startupGeneration
      )
        this.publish({
          startupFailure: safeFailure(error),
          startupFailureKind: "settings_read",
          defaultProjectObserved: false,
        });
    } finally {
      if (
        this.connection === connection &&
        generation === this.startupGeneration
      )
        this.publish({ startupLoading: false });
    }
  }
  retryProjectSettings() {
    const connection = this.connection;
    if (
      !connection ||
      !this.state.ready ||
      this.state.busy ||
      this.state.startupLoading ||
      this.state.closing
    )
      return Promise.resolve();
    return this.action(async () => {
      await this.readStartupSettings(connection, !this.state.projectId);
    });
  }
  async pickFolder(
    purpose: "open" | "create" | "copy" | "backup" | "backup_snapshot" = "open",
  ): Promise<boolean> {
    const folder = await this.chooseFolder(purpose);
    if (folder === null) return false;
    this.publish({ root: folder, error: null });
    return true;
  }
  private async chooseFolder(
    purpose: "open" | "create" | "copy" | "backup" | "backup_snapshot",
    allowCurrentProject = false,
  ): Promise<string | null> {
    const currentProject = this.state.projectId;
    if (
      !this.state.ready ||
      this.state.picking ||
      this.state.busy ||
      (!allowCurrentProject && currentProject) ||
      this.state.prompt ||
      this.state.closing
    )
      return null;
    const generation = ++this.pickerGeneration;
    this.publish({ picking: true });
    const current = () =>
      generation === this.pickerGeneration &&
      !this.state.closing &&
      !this.state.prompt &&
      (allowCurrentProject
        ? this.state.projectId === currentProject
        : !this.state.projectId);
    try {
      const folder = await this.folderPicker(purpose);
      if (!current()) return null;
      if (folder !== null && typeof folder !== "string")
        throw new BridgeFailure("protocol");
      if (folder !== null) this.publish({ error: null });
      return folder;
    } catch {
      if (!current()) return null;
      this.publish({ error: text("picker.failed") });
      return null;
    } finally {
      this.publish({ picking: false });
    }
  }
  chooseReplacementProject() {
    return this.chooseFolder("open", true);
  }
  setName(name: string) {
    const form = this.state.form;
    if (
      this.state.ready &&
      form &&
      !form.submitted &&
      !this.state.busy &&
      !this.state.deciding &&
      !this.state.prompt &&
      !this.state.closing
    )
      this.publish({ form: { ...form, name } });
  }
  dirty() {
    return (
      !!this.state.fieldEditor?.drafts.some(fieldDirty) ||
      (!!this.state.form &&
        !this.state.form.handedOff &&
        this.state.form.name !== this.state.form.initial)
    );
  }
  async open(setDefault = false) {
    if (
      !this.state.ready ||
      this.state.projectId ||
      this.state.closing ||
      this.state.picking ||
      this.state.startupLoading
    )
      return;
    if (!this.state.root && !(await this.pickFolder("open"))) return;
    return this.action(async () => {
      await this.openProject(this.state.root, "open", setDefault);
    });
  }
  async openExistingProject(setDefault = false) {
    if (
      !this.state.ready ||
      this.state.projectId ||
      this.state.closing ||
      this.state.picking ||
      this.state.startupLoading
    )
      return;
    const root = await this.chooseFolder("open");
    if (!root) return;
    return this.action(async () => {
      await this.openProject(root, "open", setDefault);
    });
  }
  async openDownloadedProject(root: string): Promise<boolean> {
    if (!this.state.ready || this.state.projectId || this.state.closing)
      return false;
    return (
      (await this.action(async () => this.openProject(root, "open", false))) ===
      null
    );
  }
  showNewProject() {
    if (
      !this.state.ready ||
      this.state.projectId ||
      this.state.closing ||
      this.state.busy ||
      this.state.picking ||
      this.state.startupLoading
    )
      return;
    this.publish({ newProject: { name: "", error: null }, error: null });
  }
  setNewProjectName(name: string) {
    if (!this.state.newProject || this.state.busy || this.state.picking) return;
    this.publish({ newProject: { name, error: null } });
  }
  cancelNewProject() {
    if (!this.state.busy && !this.state.picking)
      this.publish({ newProject: null });
  }
  showProjectCopy() {
    if (!this.state.projectId || this.state.busy || this.state.picking) return;
    const current =
      this.state.root
        .replace(/[\\/]+$/u, "")
        .split(/[\\/]/u)
        .pop() ?? "";
    this.publish({
      projectData: {
        kind: "copy",
        name: current
          ? text("backup.copyDefaultName", { name: current })
          : text("backup.copyFallbackName"),
        label: "",
        storage: this.state.projectData?.storage ?? null,
        backups: [],
        deletedBackups: [],
        view: "active",
        selectedDeleted: null,
        nextCursor: null,
        selected: null,
        locator: null,
        error: null,
        dialogGeneration: ++this.backupDialogGeneration,
        confirm: null,
        completedRoot: null,
        cancellable: false,
      },
    });
  }
  async showBackupCenter() {
    if (!this.state.projectId || this.state.busy || this.state.picking) return;
    const previous =
      this.state.projectData?.storage ??
      rememberedBackupStorage(this.state.root);
    this.publish({
      projectData: {
        kind: "backups",
        name: "",
        label: "",
        storage: previous,
        backups: [],
        deletedBackups: [],
        view: "active",
        selectedDeleted: null,
        nextCursor: null,
        selected: null,
        locator: null,
        error: null,
        dialogGeneration: ++this.backupDialogGeneration,
        confirm: null,
        completedRoot: null,
        cancellable: false,
      },
    });
    if (previous) await this.refreshBackups();
  }
  showHealth() {
    if (this.state.busy || this.state.picking) return;
    this.publish({
      health: {
        surface: "health",
        phase: "idle",
        inspection: null,
        selected: [],
        selectedTrash: [],
        selectedTemplates: [],
        confirm: null,
        exportConfirm: false,
        message: null,
        error: null,
      },
    });
    if (this.state.projectId) return this.inspectProjectHealth();
  }
  /** Both results belong to this opening/retry; closing retires the attempt. */
  private async inspectProjectHealth() {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (dialog?.surface !== "health" || !project || this.state.busy) return;
    const id = ++this.healthRequest;
    const generation = this.projectRequestGeneration;
    const epoch = this.inspectionEpoch;
    this.publish({
      health: {
        ...dialog,
        phase: "checking",
        check: { id, documents: "checking" },
        inspection: null,
        message: null,
        error: null,
      },
    });
    const current = () =>
      this.state.projectId === project &&
      this.projectRequestGeneration === generation &&
      this.inspectionEpoch === epoch &&
      this.state.health?.surface === "health" &&
      this.state.health.check?.id === id;
    const [documents, resources] = await Promise.allSettled([
      this.workspaceInspectDocuments?.(current) ?? Promise.resolve(false),
      this.operations.run(
        {
          kind: "asset_inspect",
          project,
          observation: this.inspectionObservation(),
        },
        text("health.checking"),
      ),
    ]);
    if (!current()) return;
    const latest = this.state.health!;
    const verified = documents.status === "fulfilled" && documents.value;
    const check = {
      id,
      documents: verified ? ("verified" as const) : ("unverified" as const),
    };
    try {
      if (resources.status === "rejected") throw resources.reason;
      const data = requireKind(resources.value.result, "asset_maintenance");
      if (data.action !== "inspect") throw new BridgeFailure("protocol");
      if (data.inspection.complete) this.inspectionPending.clear();
      this.publish({
        assetInspection: data.inspection,
        inspectionRefreshState: data.inspection.complete
          ? "idle"
          : data.inspection.ownerProtected
            ? "waiting"
            : "failed",
        inspectionPendingDocuments: [...this.inspectionPending],
        health: {
          ...latest,
          check,
          phase: "ready",
          inspection: data.inspection,
          message: text(
            verified && data.inspection.complete
              ? "health.checkComplete"
              : "health.checkPartial",
          ),
          error: null,
        },
      });
    } catch (error) {
      this.publish({
        inspectionRefreshState: "failed",
        health: {
          ...latest,
          check,
          phase: "failed",
          error: safeFailure(error),
          message: null,
        },
      });
    }
  }
  showFileManager(surface: "resources" | "trash") {
    if (this.state.busy || this.state.picking) return;
    const current = this.state.health;
    const inspection = current?.inspection ?? this.state.assetInspection;
    this.publish({
      health: {
        surface,
        phase: inspection ? "ready" : "idle",
        inspection,
        selected: surface === "resources" ? (current?.selected ?? []) : [],
        selectedTrash:
          surface === "trash" ? (current?.selectedTrash ?? []) : [],
        selectedTemplates:
          surface === "trash" ? (current?.selectedTemplates ?? []) : [],
        confirm: null,
        exportConfirm: false,
        message: null,
        error: null,
      },
    });
    if (!inspection && this.state.projectId) void this.inspectAssets();
  }
  focusFileManager(
    surface: "resources" | "trash",
    keys: string | readonly string[],
  ) {
    this.publish({
      fileManagerTarget: {
        surface,
        keys: [...new Set(typeof keys === "string" ? [keys] : keys)],
      },
    });
  }
  clearFileManagerTarget(surface: "resources" | "trash") {
    if (this.state.fileManagerTarget?.surface === surface)
      this.publish({ fileManagerTarget: null });
  }
  closeHealth() {
    if (!this.state.busy) this.publish({ health: null });
  }
  showDiagnosticExport() {
    const dialog = this.state.health;
    if (!dialog || this.state.busy) return;
    this.publish({ health: { ...dialog, exportConfirm: true, error: null } });
  }
  cancelDiagnosticExport() {
    const dialog = this.state.health;
    if (!dialog || this.state.busy) return;
    this.publish({ health: { ...dialog, exportConfirm: false } });
  }
  /** Canonical document writes retire only the affected inspection rows. */
  invalidateDocumentInspection(ids: readonly string[]) {
    if (!this.state.projectId || !ids.length) return;
    ++this.inspectionEpoch;
    for (const id of ids) this.inspectionPending.add(id);
    const pending = [...this.inspectionPending];
    const retire = (inspection: AssetInspection | null) =>
      inspection && {
        ...inspection,
        complete: false,
        documentIssues: inspection.documentIssues?.filter(
          (issue) => !this.inspectionPending.has(issue.documentId),
        ),
      };
    const dialog = this.state.health;
    this.publish({
      assetInspection: retire(this.state.assetInspection),
      inspectionPendingDocuments: pending,
      ...(dialog
        ? {
            health: {
              ...dialog,
              ...(dialog.surface === "health"
                ? { check: undefined, phase: "idle" as const }
                : {}),
              inspection: retire(dialog.inspection),
              message: null,
            },
          }
        : {}),
    });
    void this.refreshDocumentInspection();
  }
  /** Only confirmed native owner/custody releases trigger a retry. */
  resumeDocumentInspection(project: string) {
    if (project !== this.state.projectId || !this.inspectionPending.size)
      return;
    ++this.inspectionEpoch;
    this.inspectionReleaseRetryProject = project;
    if (this.state.health?.surface === "health") {
      this.healthInspectionRequested = true;
      this.healthInspectionProject = project;
      this.publish({
        health: {
          ...this.state.health,
          phase: "idle",
          check: undefined,
          inspection: null,
          message: null,
        },
      });
      return;
    }
    void this.refreshDocumentInspection();
  }
  private inspectionObservation(reRequested = false) {
    const released =
      this.inspectionReleaseRetryProject === this.state.projectId;
    this.inspectionReleaseRetryProject = null;
    return {
      epoch: this.inspectionEpoch,
      generation: this.projectRequestGeneration,
      pendingCount: this.inspectionPending.size,
      reRequested: reRequested || released,
    };
  }
  private async refreshDocumentInspection() {
    if (this.inspectionRefreshing) {
      this.inspectionRefreshRequested = true;
      return;
    }
    this.inspectionRefreshing = true;
    try {
      while (this.inspectionPending.size && this.state.projectId) {
        const observation = this.inspectionObservation(
          this.inspectionRefreshRequested,
        );
        this.inspectionRefreshRequested = false;
        this.publish({ inspectionRefreshState: "checking" });
        const project = this.state.projectId;
        const generation = this.projectRequestGeneration;
        const epoch = this.inspectionEpoch;
        try {
          const { result } = await this.operations.run(
            {
              kind: "asset_inspect",
              project,
              observation,
            },
            text("health.checking"),
          );
          const data = requireKind(result, "asset_maintenance");
          if (data.action !== "inspect") throw new BridgeFailure("protocol");
          if (
            this.state.projectId !== project ||
            this.projectRequestGeneration !== generation
          )
            return;
          if (this.inspectionEpoch !== epoch) continue;
          if (data.inspection.complete) this.inspectionPending.clear();
          const dialog = this.state.health;
          this.publish({
            assetInspection: data.inspection,
            inspectionRefreshState: data.inspection.complete
              ? "idle"
              : data.inspection.ownerProtected
                ? "waiting"
                : "failed",
            inspectionPendingDocuments: [...this.inspectionPending],
            ...(dialog && dialog.surface !== "health"
              ? {
                  health: {
                    ...dialog,
                    inspection: data.inspection,
                    phase: "ready" as const,
                  },
                }
              : {}),
          });
          // A release observed during publication must not lose its retry.
          if (this.inspectionRefreshRequested) continue;
          break;
        } catch {
          if (
            this.state.projectId !== project ||
            this.projectRequestGeneration !== generation
          )
            return;
          if (this.inspectionEpoch !== epoch) continue;
          this.publish({ inspectionRefreshState: "failed" });
          break;
        }
      }
    } finally {
      this.inspectionRefreshing = false;
      if (this.inspectionRefreshRequested && this.inspectionPending.size)
        void this.refreshDocumentInspection();
    }
  }
  async exportDiagnostics() {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (!dialog?.exportConfirm || !project || this.state.busy) return;
    let destination: string | null = null;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      destination = await save({
        defaultPath: "worldbuild-diagnostics.json",
        title: text("health.exportPicker"),
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
    } catch {
      const current = this.state.health;
      if (current)
        this.publish({
          health: { ...current, error: text("health.exportFailed") },
        });
      return;
    }
    if (!destination || this.state.health !== dialog) return;
    await this.action(async () => {
      const current = this.state.health;
      if (!current) return;
      this.publish({
        health: {
          ...current,
          phase: "exporting",
          exportConfirm: false,
          error: null,
        },
      });
      try {
        const { result } = await this.operations.run(
          { kind: "diagnostic_export", project, destination },
          text("health.exporting"),
        );
        const data = requireKind(result, "diagnostic_export");
        const latest = this.state.health;
        const published = data.result.outcome === "published";
        if (latest)
          this.publish({
            health: {
              ...latest,
              phase: published ? "ready" : "failed",
              message: published
                ? text("health.exportComplete")
                : data.result.outcome === "published_verification_uncertain"
                  ? text("health.exportUncertain")
                  : null,
              error: published
                ? null
                : text(
                    data.result.cleanupRequired
                      ? "health.exportCleanupRequired"
                      : "health.exportNotApplied",
                  ),
            },
          });
        void data.result.size;
      } catch (error) {
        const latest = this.state.health;
        if (latest)
          this.publish({
            health: {
              ...latest,
              phase: "failed",
              error: safeFailure(error),
            },
          });
        throw error;
      }
    });
  }
  async inspectAssets() {
    const dialog = this.state.health;
    if (dialog?.surface === "health") return this.inspectProjectHealth();
    const project = this.state.projectId;
    if (!dialog || !project || this.state.busy) return;
    const generation = this.projectRequestGeneration;
    const epoch = this.inspectionEpoch;
    await this.action(async () => {
      const current = this.state.health;
      if (!current) return;
      this.publish({
        health: { ...current, phase: "checking", error: null, message: null },
      });
      try {
        const { result } = await this.operations.run(
          {
            kind: "asset_inspect",
            project,
            observation: this.inspectionObservation(),
          },
          text("health.checking"),
        );
        const data = requireKind(result, "asset_maintenance");
        if (data.action !== "inspect") throw new BridgeFailure("protocol");
        if (
          this.state.projectId !== project ||
          this.projectRequestGeneration !== generation ||
          this.inspectionEpoch !== epoch
        ) {
          if (this.inspectionEpoch !== epoch)
            void this.refreshDocumentInspection();
          return;
        }
        if (data.inspection.complete) this.inspectionPending.clear();
        const latest = this.state.health;
        if (latest)
          this.publish({
            assetInspection: data.inspection,
            inspectionPendingDocuments: [...this.inspectionPending],
            health: {
              ...latest,
              phase: "ready",
              inspection: data.inspection,
              selected: [],
              selectedTrash: [],
              selectedTemplates: [],
              confirm: null,
              message: text(
                data.inspection.complete
                  ? "health.checkComplete"
                  : "health.checkPartial",
              ),
              error: null,
            },
          });
      } catch (error) {
        const latest = this.state.health;
        if (latest)
          this.publish({
            health: {
              ...latest,
              phase: "failed",
              error: safeFailure(error),
            },
          });
        throw error;
      }
    });
  }
  toggleHealthAsset(id: string) {
    const dialog = this.state.health;
    if (!dialog || dialog.phase !== "ready" || !dialog.inspection?.complete)
      return;
    const row = dialog.inspection.rows.find((item) => item.id === id);
    if (row?.status !== "unused") return;
    const selected = dialog.selected.includes(id)
      ? dialog.selected.filter((item) => item !== id)
      : dialog.selected.length < 100
        ? [...dialog.selected, id]
        : dialog.selected;
    this.publish({ health: { ...dialog, selected, error: null } });
  }
  setHealthAssets(ids: string[]) {
    const dialog = this.state.health;
    if (!dialog?.inspection?.complete || dialog.phase !== "ready") return;
    const allowed = new Set(
      dialog.inspection.rows
        .filter((row) => row.status === "unused")
        .map((row) => row.id),
    );
    this.publish({
      health: {
        ...dialog,
        selected: [...new Set(ids)]
          .filter((id) => allowed.has(id))
          .slice(0, 100),
        error: null,
      },
    });
  }
  toggleHealthTrash(id: string) {
    const dialog = this.state.health;
    if (!dialog || dialog.phase !== "ready" || !dialog.inspection?.complete)
      return;
    const row = dialog.inspection.trash.find((item) => item.id === id);
    if (!row || row.protected) return;
    const selectedTrash = dialog.selectedTrash.includes(id)
      ? dialog.selectedTrash.filter((item) => item !== id)
      : dialog.selectedTrash.length < 100
        ? [...dialog.selectedTrash, id]
        : dialog.selectedTrash;
    this.publish({
      health: { ...dialog, selectedTrash, confirm: null, error: null },
    });
  }
  setHealthTrash(ids: string[]) {
    const dialog = this.state.health;
    if (!dialog?.inspection?.complete || dialog.phase !== "ready") return;
    const allowed = new Set(
      dialog.inspection.trash
        .filter((row) => !row.protected)
        .map((row) => row.id),
    );
    this.publish({
      health: {
        ...dialog,
        selectedTrash: [...new Set(ids)]
          .filter((id) => allowed.has(id))
          .slice(0, 100),
        error: null,
      },
    });
  }
  setHealthTemplates(ids: string[]) {
    const dialog = this.state.health;
    if (!dialog?.inspection?.complete || dialog.phase !== "ready") return;
    const allowed = new Set(
      dialog.inspection.deletedTemplates
        .filter((row) => row.removable)
        .map((row) => row.id),
    );
    this.publish({
      health: {
        ...dialog,
        selectedTemplates: [...new Set(ids)]
          .filter((id) => allowed.has(id))
          .slice(0, 100),
        error: null,
      },
    });
  }

  async renameAsset(id: string, name: string) {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (!dialog?.inspection?.complete || !project || this.state.busy) return;
    const token = dialog.inspection.token;
    await this.action(async () => {
      const { result } = await this.operations.run(
        {
          kind: "asset_rename",
          project,
          inspection_token: token,
          asset: id,
          name,
        },
        text("resources.renaming"),
      );
      const data = requireKind(result, "asset_maintenance");
      if (data.action !== "asset_rename") throw new BridgeFailure("protocol");
      const latest = this.state.health;
      if (latest)
        this.publish({
          assetInspection: data.inspection,
          health: {
            ...latest,
            phase: "ready",
            inspection: data.inspection,
            selected: [id],
            message: text("resources.renamed"),
            error: null,
          },
        });
    });
  }
  toggleHealthTemplate(id: string) {
    const dialog = this.state.health;
    if (!dialog || dialog.phase !== "ready" || !dialog.inspection?.complete)
      return;
    const row = dialog.inspection.deletedTemplates.find(
      (item) => item.id === id,
    );
    if (!row?.removable) return;
    const selectedTemplates = dialog.selectedTemplates.includes(id)
      ? dialog.selectedTemplates.filter((item) => item !== id)
      : dialog.selectedTemplates.length < 100
        ? [...dialog.selectedTemplates, id]
        : dialog.selectedTemplates;
    this.publish({
      health: { ...dialog, selectedTemplates, confirm: null, error: null },
    });
  }
  showHealthPurgeConfirm(kind: "trash_selected" | "trash_all" | "templates") {
    const dialog = this.state.health;
    if (!dialog?.inspection?.complete || dialog.phase !== "ready") return;
    const ids =
      kind === "trash_selected"
        ? dialog.selectedTrash
        : kind === "trash_all"
          ? dialog.inspection.trash
              .filter((row) => !row.protected)
              .map((row) => row.id)
          : dialog.selectedTemplates;
    if (!ids.length) return;
    this.publish({
      health: { ...dialog, confirm: { kind, ids: [...ids] }, error: null },
    });
  }
  cancelHealthPurge() {
    const dialog = this.state.health;
    if (dialog && !this.state.busy)
      this.publish({ health: { ...dialog, confirm: null } });
  }
  async purgeHealthSelection() {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (!dialog?.inspection?.complete || !dialog.confirm || !project)
      return null;
    const confirm = dialog.confirm;
    const token = dialog.inspection.token;
    let refreshTemplates = false;
    const outcome = await this.action(async () => {
      const current = this.state.health;
      if (!current) return;
      this.publish({
        health: { ...current, phase: "purging", confirm: null, error: null },
      });
      try {
        const input =
          confirm.kind === "templates"
            ? {
                kind: "template_purge" as const,
                project,
                inspection_token: token,
                templates: confirm.ids,
              }
            : {
                kind: "asset_trash_purge" as const,
                project,
                inspection_token: token,
                assets: confirm.kind === "trash_all" ? [] : confirm.ids,
                empty: confirm.kind === "trash_all",
              };
        const { result } = await this.operations.run(
          input,
          text("health.purging"),
        );
        const data = requireKind(result, "asset_maintenance");
        const expected =
          confirm.kind === "templates" ? "template_purge" : "trash_purge";
        if (data.action !== expected) throw new BridgeFailure("protocol");
        const summary = {
          completedCount: data.completedCount,
          failures: data.failures,
          cleanupRequired: data.cleanupRequired,
        };
        refreshTemplates = confirm.kind === "templates";
        const latest = this.state.health;
        if (latest)
          this.publish({
            assetInspection: data.inspection,
            health: {
              ...latest,
              phase: "ready",
              inspection: data.inspection,
              selectedTrash: [],
              selectedTemplates: [],
              message: data.failures.length
                ? text("health.purgePartial")
                : text(
                    confirm.kind === "templates"
                      ? "health.templatePurgeComplete"
                      : "health.purgeComplete",
                  ),
              error: data.failures.length ? text("health.purgePartial") : null,
            },
          });
        return summary;
      } catch (error) {
        const latest = this.state.health;
        if (latest)
          this.publish({
            health: { ...latest, phase: "failed", error: safeFailure(error) },
          });
        throw error;
      }
    });
    if (refreshTemplates) await this.refresh();
    return outcome ?? null;
  }
  async moveSelectedAssetsToTrash() {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (
      !dialog?.inspection?.complete ||
      !dialog.selected.length ||
      !project ||
      this.state.busy
    )
      return;
    const selected = [...dialog.selected];
    const token = dialog.inspection.token;
    await this.action(async () => {
      const current = this.state.health;
      if (!current) return;
      this.publish({ health: { ...current, phase: "moving", error: null } });
      try {
        const { result } = await this.operations.run(
          {
            kind: "asset_trash_move",
            project,
            inspection_token: token,
            assets: selected,
          },
          text("health.moving"),
        );
        const data = requireKind(result, "asset_maintenance");
        if (data.action !== "trash_move") throw new BridgeFailure("protocol");
        const latest = this.state.health;
        if (latest)
          this.publish({
            assetInspection: data.inspection,
            health: {
              ...latest,
              phase: "ready",
              inspection: data.inspection,
              selected: [],
              selectedTrash: [],
              selectedTemplates: [],
              confirm: null,
              message: data.failures.length
                ? text("health.movePartial")
                : text("health.moveComplete"),
              error: data.failures.length ? text("health.movePartial") : null,
            },
          });
      } catch (error) {
        const latest = this.state.health;
        if (latest)
          this.publish({
            health: { ...latest, phase: "failed", error: safeFailure(error) },
          });
        throw error;
      }
    });
  }
  async restoreAsset(id: string) {
    const dialog = this.state.health;
    const project = this.state.projectId;
    if (!dialog?.inspection?.trash.some((row) => row.id === id) || !project)
      return null;
    const outcome = await this.action(async () => {
      const current = this.state.health;
      if (!current) return;
      this.publish({ health: { ...current, phase: "restoring", error: null } });
      try {
        const { result } = await this.operations.run(
          { kind: "asset_trash_restore", project, assets: [id] },
          text("health.restoring"),
        );
        const data = requireKind(result, "asset_maintenance");
        if (data.action !== "trash_restore")
          throw new BridgeFailure("protocol");
        const summary = {
          completedCount: data.completedCount,
          failures: data.failures,
          cleanupRequired: data.cleanupRequired,
        };
        const latest = this.state.health;
        if (latest)
          this.publish({
            assetInspection: data.inspection,
            health: {
              ...latest,
              phase: "ready",
              inspection: data.inspection,
              selected: [],
              selectedTrash: [],
              selectedTemplates: [],
              confirm: null,
              message: data.failures.length
                ? text("health.restoreConflict")
                : text("health.restoreComplete"),
              error: data.failures.length
                ? text("health.restoreConflict")
                : null,
            },
          });
        return summary;
      } catch (error) {
        const latest = this.state.health;
        if (latest)
          this.publish({
            health: { ...latest, phase: "failed", error: safeFailure(error) },
          });
        throw error;
      }
    });
    return outcome ?? null;
  }
  async restoreTemplate(id: string) {
    const project = this.state.projectId;
    if (!project || this.state.busy) return false;
    const restored = await this.action(async () => {
      const { result } = await this.operations.run(
        { kind: "template_restore", project, template: id },
        text("trash.restoringTemplate"),
      );
      const data = requireKind(result, "write");
      if (data.disk !== "committed" && data.disk !== "no_write")
        throw new BridgeFailure("boundary", undefined, data.error ?? undefined);
      this.publish({ message: text("trash.templateRestored") });
      return true;
    });
    if (restored) {
      await this.refresh();
      await this.inspectAssets();
    }
    return restored;
  }
  closeProjectData() {
    if (!this.state.busy && !this.state.picking)
      this.publish({ projectData: null });
  }
  openBackupDiagnostics() {
    const message = this.state.projectData?.error;
    if (!message || this.state.busy) return;
    this.publish({ projectData: null });
    this.showHealth();
    const health = this.state.health;
    if (health) this.publish({ health: { ...health, message } });
  }
  setProjectDataName(name: string) {
    const dialog = this.state.projectData;
    if (!dialog || this.state.busy || this.state.picking) return;
    this.publish({
      projectData: { ...dialog, name, error: null, errorSource: null },
    });
  }
  setBackupLabel(label: string) {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || label.length > 120) return;
    this.publish({ projectData: { ...dialog, label } });
  }
  async selectBackup(id: string) {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || this.state.busy) return;
    let selected = dialog.backups.find((backup) => backup.id === id);
    if (selected?.status === "verification_required") {
      await this.action(async () => {
        const { result } = await this.runProjectData(
          { kind: "backup_inspect", locator: selected!.locator },
          text("backup.inspecting"),
        );
        const data = requireKind(result, "project_data");
        if (data.action !== "inspect" || !data.backup)
          throw new BridgeFailure("protocol");
        const current = this.state.projectData;
        if (current?.kind !== "backups") return;
        selected = data.backup;
        this.publish({
          projectData: {
            ...current,
            backups: current.backups.map((row) =>
              row.id === id ? data.backup! : row,
            ),
          },
        });
      });
    }
    if (selected?.status !== "verified") selected = undefined;
    const current = this.state.projectData;
    if (!current || current.kind !== "backups") return;
    this.publish({
      projectData: {
        ...current,
        selected: selected?.id ?? null,
        locator: selected?.locator ?? null,
        confirm: null,
      },
    });
  }
  async chooseBackupStorage() {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups") return;
    const storage = await this.chooseFolder("backup", true);
    if (!storage || this.state.projectData !== dialog) return;
    rememberBackupStorage(this.state.root, storage);
    this.publish({
      projectData: {
        ...dialog,
        storage,
        error: null,
        errorSource: null,
        deletedCleanupWarning: null,
        deletedCleanupBackupId: null,
        deletedCleanupFacts: [],
        deletedListWarning: null,
      },
    });
    if (dialog.view === "deleted") await this.refreshDeletedBackups();
    else await this.refreshBackups();
  }
  private async runProjectData(input: Work, label: string) {
    this.projectDataOperation = null;
    try {
      return await this.operations.run(
        input,
        label,
        "ordinary",
        undefined,
        (operation) => {
          this.projectDataOperation = operation;
          const dialog = this.state.projectData;
          if (dialog)
            this.publish({ projectData: { ...dialog, cancellable: true } });
        },
      );
    } finally {
      this.projectDataOperation = null;
      const dialog = this.state.projectData;
      if (dialog?.cancellable)
        this.publish({ projectData: { ...dialog, cancellable: false } });
    }
  }
  async cancelProjectDataOperation() {
    const operation = this.projectDataOperation;
    if (!operation) return;
    try {
      await this.client.documentProgress(operation, true);
      this.publish({ message: text("backup.cancelRequested") });
    } catch {
      this.publish({ error: text("backup.cancelUnknown") });
    }
  }
  async refreshBackups(append = false) {
    const dialog = this.state.projectData;
    const project = this.state.projectId;
    if (!dialog || dialog.kind !== "backups" || !dialog.storage || !project)
      return;
    const generation = ++this.backupListGeneration;
    await this.action(async () => {
      const { result } = await this.runProjectData(
        {
          kind: "backup_list",
          project,
          storage: dialog.storage!,
          cursor: append ? dialog.nextCursor : null,
        },
        text("backup.loading"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "backup_list") throw new BridgeFailure("protocol");
      const current = this.state.projectData;
      if (
        generation === this.backupListGeneration &&
        current?.kind === "backups" &&
        current.storage === dialog.storage
      )
        this.publish({
          projectData: {
            ...current,
            backups: append
              ? [...current.backups, ...data.backups]
              : data.backups,
            nextCursor: data.nextCursor,
            selected: null,
            locator: null,
          },
        });
    });
  }
  async loadMoreBackups() {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || !dialog.nextCursor) return;
    await this.refreshBackups(true);
  }
  async showDeletedBackups() {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || this.state.busy) return;
    const view = dialog.view === "deleted" ? "active" : "deleted";
    this.publish({
      projectData: {
        ...dialog,
        view,
        selected: null,
        locator: null,
        selectedDeleted: null,
        confirm: null,
      },
    });
    if (view === "deleted") await this.refreshDeletedBackups();
    else await this.refreshBackups();
  }
  async refreshDeletedBackups() {
    const dialog = this.state.projectData;
    const project = this.state.projectId;
    if (!dialog || dialog.kind !== "backups" || !dialog.storage || !project)
      return;
    const generation = ++this.backupListGeneration;
    const projectGeneration = this.projectRequestGeneration;
    const dialogGeneration = dialog.dialogGeneration;
    const currentDialog = () => {
      const current = this.state.projectData;
      return generation === this.backupListGeneration &&
        this.projectRequestGeneration === projectGeneration &&
        this.state.projectId === project &&
        current?.kind === "backups" &&
        current.dialogGeneration === dialogGeneration &&
        current.storage === dialog.storage &&
        current.view === "deleted"
        ? current
        : null;
    };
    await this.action(async () => {
      try {
        const { result } = await this.runProjectData(
          { kind: "backup_deleted_list", project, storage: dialog.storage! },
          text("backup.deletedLoading"),
        );
        const data = requireKind(result, "project_data");
        if (
          data.action !== "backup_deleted_list" ||
          !Array.isArray(data.deletedBackups)
        )
          throw new BridgeFailure("protocol");
        const rows = data.deletedBackups;
        const current = currentDialog();
        if (!current) return;
        const ownError = current.errorSource === "deleted_list";
        const otherError = current.errorSource === "other" && !!current.error;
        // A complete scan can prove receipt cleanup only after all native
        // representations of that target disappear. Partial deletion requires
        // a separate definitive command result, not merely an empty list.
        const facts = (current.deletedCleanupFacts ?? []).filter(
          (fact) =>
            fact.outcome === "partially_deleted" ||
            rows.some((row) => cleanupRowMatches(fact.id, row)),
        );
        const listWarning = data.warning ?? current.deletedListWarning ?? null;
        const warning = facts[0]?.warning ?? listWarning;
        const cleanupError = current.errorSource === "cleanup";
        this.publish({
          projectData: {
            ...current,
            deletedBackups: rows,
            selectedDeleted: null,
            noticeEvent: otherError
              ? current.noticeEvent
              : ++this.noticeSequence,
            error: otherError
              ? current.error
              : warning
                ? text("backup.deletedCleanupRequired")
                : ownError || cleanupError
                  ? null
                  : current.error,
            detail: otherError
              ? current.detail
              : (warning ?? (ownError || cleanupError ? null : current.detail)),
            errorSource: otherError
              ? "other"
              : warning
                ? "cleanup"
                : ownError || cleanupError
                  ? null
                  : current.errorSource,
            deletedCleanupFacts: facts,
            deletedListWarning: listWarning,
            deletedCleanupWarning: warning,
            deletedCleanupBackupId:
              facts.find((fact) => fact.outcome !== "partially_deleted")?.id ??
              null,
          },
        });
      } catch (error: unknown) {
        const current = currentDialog();
        if (!current) return;
        // 목록 실패는 다른 명령의 미해결 오류를 덮지 않는다.
        if (current.errorSource === "other" && current.error) return;
        this.publish({
          projectData: {
            ...current,
            noticeEvent: ++this.noticeSequence,
            error: safeFailure(error),
            detail: failureDetail(error),
            errorSource: "deleted_list",
          },
        });
      }
    });
  }
  selectDeletedBackup(id: string) {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || this.state.busy) return;
    const selected = dialog.deletedBackups?.find((row) => row.backup.id === id);
    this.publish({
      projectData: {
        ...dialog,
        selectedDeleted:
          selected && selected.status !== "uncertain" ? id : null,
        confirm: null,
      },
    });
  }
  async restoreSelectedDeletedBackup() {
    const dialog = this.state.projectData;
    const selected = dialog?.deletedBackups?.find(
      (row) => row.backup.id === dialog.selectedDeleted,
    );
    if (
      !dialog?.storage ||
      !selected ||
      selected.status !== "verification_required"
    )
      return;
    await this.restoreDeletedBackup(
      dialog.storage,
      selected.backup.id,
      selected.operation,
    );
  }
  private async restoreDeletedBackup(
    storage: string,
    id: string,
    operation: string,
  ) {
    const project = this.state.projectId;
    if (!project || this.state.busy) return;
    const dialogAtStart = this.state.projectData;
    const projectGeneration = this.projectRequestGeneration;
    let restored = false;
    await this.action(async () => {
      const { result } = await this.operations.run(
        { kind: "backup_deleted_restore", project, storage, id, operation },
        text("backup.deletedRestoring"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "backup_deleted_restore")
        throw new BridgeFailure("protocol");
      restored =
        data.outcome === "restored" ||
        data.outcome === "restored_receipt_cleanup_required";
      if (restored) {
        const current = this.state.feedback;
        if (current?.undo?.operation === operation)
          this.publish({ feedback: null });
        if (data.outcome === "restored_receipt_cleanup_required") {
          const commandWarning =
            data.warning ?? text("backup.deletedCleanupRequired");
          const dialog = this.state.projectData;
          if (
            dialog?.kind === "backups" &&
            dialog.storage === storage &&
            dialog.dialogGeneration === dialogAtStart?.dialogGeneration &&
            this.state.projectId === project &&
            this.projectRequestGeneration === projectGeneration
          ) {
            const facts = recordCleanupFact(dialog.deletedCleanupFacts ?? [], {
              id,
              operation,
              outcome: "restored_receipt_cleanup_required",
              warning: commandWarning,
            });
            const otherError = dialog.errorSource === "other" && !!dialog.error;
            this.publish({
              projectData: {
                ...dialog,
                noticeEvent: otherError
                  ? dialog.noticeEvent
                  : ++this.noticeSequence,
                error: otherError
                  ? dialog.error
                  : text("backup.deletedCleanupRequired"),
                detail: otherError ? dialog.detail : facts[0].warning,
                errorSource: otherError ? "other" : "cleanup",
                deletedCleanupFacts: facts,
                deletedCleanupWarning: facts[0].warning,
                deletedCleanupBackupId:
                  facts.find((fact) => fact.outcome !== "partially_deleted")
                    ?.id ?? null,
              },
            });
          } else if (
            this.state.projectId === project &&
            this.projectRequestGeneration === projectGeneration
          )
            this.publish({
              error: text("backup.deletedCleanupRequired"),
              errorDetail: commandWarning,
            });
        }
      } else {
        const current = this.state.projectData;
        if (current?.kind === "backups")
          this.publish({
            projectData: {
              ...current,
              noticeEvent: ++this.noticeSequence,
              error: text("backup.restoreUncertain"),
              detail: data.warning,
              errorSource: "other",
            },
          });
        else
          this.publish({
            error: text("backup.restoreUncertain"),
            errorDetail: data.warning,
          });
      }
    });
    if (restored) {
      const current = this.state.projectData;
      if (current?.kind === "backups" && current.storage === storage) {
        if (current.view === "deleted") await this.refreshDeletedBackups();
        else await this.refreshBackups();
      }
      this.completeFeedback(text("backup.deletedRestored"));
    }
  }
  async purgeSelectedDeletedBackup() {
    const dialog = this.state.projectData;
    const project = this.state.projectId;
    const selected = dialog?.deletedBackups?.find(
      (row) => row.backup.id === dialog.selectedDeleted,
    );
    if (
      !dialog?.storage ||
      dialog.confirm !== "purge_deleted" ||
      !selected ||
      !project
    )
      return;
    const projectGeneration = this.projectRequestGeneration;
    await this.action(async () => {
      const { result } = await this.operations.run(
        {
          kind: "backup_deleted_purge",
          project,
          storage: dialog.storage!,
          id: selected.backup.id,
          operation: selected.operation,
        },
        text("backup.deletedPurging"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "backup_deleted_purge")
        throw new BridgeFailure("protocol");
      const current = this.state.projectData;
      if (
        current?.kind === "backups" &&
        current.storage === dialog.storage &&
        current.dialogGeneration === dialog.dialogGeneration &&
        this.state.projectId === project &&
        this.projectRequestGeneration === projectGeneration
      ) {
        const facts = (current.deletedCleanupFacts ?? []).filter(
          (fact) =>
            fact.id !== selected.backup.id ||
            fact.operation !== selected.operation,
        );
        if (data.outcome !== "deleted")
          facts.push({
            id: selected.backup.id,
            operation: selected.operation,
            outcome:
              data.outcome === "deleted_receipt_cleanup_required"
                ? "deleted_receipt_cleanup_required"
                : "partially_deleted",
            warning: data.warning ?? text("backup.deletedCleanupRequired"),
          });
        const warning = facts[0]?.warning ?? current.deletedListWarning ?? null;
        const otherError = current.errorSource === "other" && !!current.error;
        this.publish({
          projectData: {
            ...current,
            confirm: null,
            selectedDeleted: null,
            noticeEvent: otherError
              ? current.noticeEvent
              : ++this.noticeSequence,
            error: otherError
              ? current.error
              : warning
                ? text("backup.deletedCleanupRequired")
                : null,
            detail: otherError ? current.detail : warning,
            errorSource: otherError ? "other" : warning ? "cleanup" : null,
            deletedCleanupFacts: facts,
            deletedCleanupWarning: warning,
            deletedCleanupBackupId:
              facts.find((fact) => fact.outcome !== "partially_deleted")?.id ??
              null,
          },
        });
      }
    });
    await this.refreshDeletedBackups();
  }
  async createBackup() {
    let dialog = this.state.projectData;
    const project = this.state.projectId;
    if (!dialog || dialog.kind !== "backups" || !project) return;
    if (!dialog.storage) {
      await this.chooseBackupStorage();
      dialog = this.state.projectData;
    }
    if (!dialog || dialog.kind !== "backups" || !dialog.storage) return;
    let created = false;
    await this.action(async () => {
      const { result } = await this.runProjectData(
        {
          kind: "backup_create",
          project,
          storage: dialog!.storage!,
          label: dialog!.label.trim() || null,
        },
        text("backup.creating"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "backup_create" || !data.backup)
        throw new BridgeFailure("protocol");
      const current = this.state.projectData;
      if (current?.kind === "backups")
        this.publish({
          projectData: { ...current, label: "", selected: data.backup.id },
        });
      created = true;
    });
    if (created) {
      await this.refreshBackups();
      this.completeFeedback(text("backup.created"));
    }
  }
  async showRestoreFromBackup() {
    if (this.state.projectId || this.state.busy || this.state.picking) return;
    const locator = await this.chooseFolder("backup_snapshot");
    if (!locator) return;
    await this.action(async () => {
      const { result } = await this.runProjectData(
        { kind: "backup_inspect", locator },
        text("backup.inspecting"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "inspect" || !data.backup)
        throw new BridgeFailure("protocol");
      this.publish({
        projectData: {
          kind: "restore_new",
          name: data.backup.label || text("backup.restoredProjectName"),
          label: "",
          storage: null,
          backups: [data.backup],
          nextCursor: null,
          selected: data.backup.id,
          locator,
          error: null,
          confirm: null,
          completedRoot: null,
          cancellable: false,
        },
      });
    });
  }
  startRestoreNew() {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups" || !dialog.locator) return;
    const selected = dialog.backups.find(
      (backup) => backup.id === dialog.selected,
    );
    this.publish({
      projectData: {
        ...dialog,
        kind: "restore_new",
        name: selected?.label || text("backup.restoredProjectName"),
        backups: selected ? [selected] : [],
        error: null,
        confirm: null,
      },
    });
  }
  async confirmProjectDataName(beforeCopy?: () => Promise<boolean>) {
    const dialog = this.state.projectData;
    if (!dialog || !["copy", "restore_new"].includes(dialog.kind)) return;
    if (!validProjectName(dialog.name)) {
      this.publish({
        projectData: {
          ...dialog,
          noticeEvent: ++this.noticeSequence,
          error: text("project.nameInvalid"),
        },
      });
      return;
    }
    if (dialog.kind === "copy" && beforeCopy && !(await beforeCopy())) {
      if (this.state.projectData === dialog)
        this.publish({
          projectData: {
            ...dialog,
            noticeEvent: ++this.noticeSequence,
            error: text("backup.copySaveFirst"),
          },
        });
      return;
    }
    if (this.state.projectData !== dialog) return;
    const parent = await this.chooseFolder("copy", !!this.state.projectId);
    if (!parent || this.state.projectData !== dialog) return;
    await this.action(async () => {
      const input: Work =
        dialog.kind === "copy"
          ? {
              kind: "project_copy",
              project: this.state.projectId!,
              parent,
              name: dialog.name,
            }
          : {
              kind: "restore_new",
              locator: dialog.locator!,
              parent,
              name: dialog.name,
            };
      const { result } = await this.runProjectData(
        input,
        text(dialog.kind === "copy" ? "backup.copying" : "backup.restoring"),
      );
      const data = requireKind(result, "project_data");
      if (!data.root) throw new BridgeFailure("protocol");
      const current = this.state.projectData;
      if (
        current &&
        current.dialogGeneration === dialog.dialogGeneration &&
        current.kind === dialog.kind
      )
        this.publish({
          projectData: { ...current, completedRoot: data.root },
          message: text(
            data.outcome === "published_verification_uncertain"
              ? "backup.publishedUncertain"
              : dialog.kind === "copy"
                ? "backup.copyCreated"
                : "backup.restoredNew",
          ),
        });
    });
  }
  requestProjectDataConfirm(
    confirm: "restore_current" | "delete" | "purge_deleted",
  ) {
    const dialog = this.state.projectData;
    if (!dialog || dialog.kind !== "backups") return;
    if (confirm === "purge_deleted" && !dialog.selectedDeleted) return;
    if (confirm !== "purge_deleted" && !dialog.locator) return;
    this.publish({ projectData: { ...dialog, confirm } });
  }
  cancelProjectDataConfirm() {
    const dialog = this.state.projectData;
    if (dialog) this.publish({ projectData: { ...dialog, confirm: null } });
  }
  async deleteSelectedBackup() {
    const dialog = this.state.projectData;
    const project = this.state.projectId;
    if (
      !dialog?.locator ||
      !dialog.storage ||
      dialog.confirm !== "delete" ||
      !project
    )
      return;
    let quarantined = false;
    let uncertain = false;
    await this.action(async () => {
      const { result } = await this.operations.run(
        {
          kind: "backup_delete",
          project,
          storage: dialog.storage!,
          locator: dialog.locator!,
        },
        text("backup.deleting"),
      );
      const data = requireKind(result, "project_data");
      if (data.action !== "backup_delete") throw new BridgeFailure("protocol");
      if (
        data.outcome !== "quarantined" &&
        data.outcome !== "quarantined_unverified"
      )
        throw new BridgeFailure("protocol");
      quarantined = true;
      uncertain = data.outcome === "quarantined_unverified" || !data.deleted;
      const current = this.state.projectData;
      if (current?.kind === "backups")
        this.publish({
          projectData: {
            ...current,
            selected: null,
            locator: null,
            confirm: null,
            noticeEvent: ++this.noticeSequence,
            error: uncertain ? text("backup.deleteUncertain") : null,
            detail: data.warning,
            errorSource: "other",
          },
        });
      if (!uncertain && data.deleted)
        this.backupUndoFeedback(
          text("backup.deletedUndo", {
            name: data.deleted.backup.label || text("backup.unnamedShort"),
          }),
          project,
          dialog.storage!,
          data.deleted,
        );
    });
    if (quarantined) await this.refreshBackups();
    if (uncertain) {
      const current = this.state.projectData;
      if (current?.kind === "backups")
        this.publish({
          projectData: {
            ...current,
            selected: null,
            locator: null,
            confirm: null,
            noticeEvent: ++this.noticeSequence,
            error: text("backup.deleteUncertain"),
            detail: "backup_quarantine_readback_required",
            errorSource: "other",
          },
        });
    }
  }
  selectedBackupRestore(): { locator: string; storage: string } | null {
    const dialog = this.state.projectData;
    return dialog?.kind === "backups" && dialog.locator && dialog.storage
      ? { locator: dialog.locator, storage: dialog.storage }
      : null;
  }
  async restoreCurrentBackup(locator: string, storage: string) {
    const project = this.state.projectId;
    const root = this.state.root;
    if (!project || !root) throw new BridgeFailure("protocol");
    const { result } = await this.runProjectData(
      {
        kind: "restore_current",
        project,
        locator,
        safety_storage: storage,
      },
      text("backup.restoringCurrent"),
    );
    const data = requireKind(result, "project_data");
    if (
      data.action !== "restore_current" ||
      !data.root ||
      !sameProjectPath(data.root, root)
    )
      throw new BridgeFailure("protocol");
    if (data.outcome === "not_applied_safety_created") {
      const dialog = this.state.projectData;
      if (dialog?.kind === "backups")
        this.publish({
          projectData: { ...dialog, confirm: null },
          message: text("backup.notAppliedSafetyCreated"),
        });
      await this.refreshBackups();
      return;
    }
    this.publish({
      projectData: null,
      message: text("backup.restoreAppliedReopening"),
    });
    await this.closeProject();
    if (!this.state.projectId) {
      const problem = await this.openProject(root, "open", false);
      if (problem)
        throw new BridgeFailure("boundary", undefined, {
          code: "restore_rejected",
          nextAction: "",
        });
      this.publish({
        message: text(
          data.outcome === "applied_recovered"
            ? "backup.restoredCurrentRecovered"
            : "backup.restoredCurrent",
        ),
      });
    }
  }
  async confirmNewProject(setDefault = false) {
    const draft = this.state.newProject;
    if (
      !draft ||
      !this.state.ready ||
      this.state.projectId ||
      this.state.closing ||
      this.state.busy ||
      this.state.picking ||
      this.state.startupLoading
    )
      return;
    const name = draft.name;
    if (!validProjectName(name)) {
      this.publish({
        newProject: { ...draft, error: text("project.nameInvalid") },
      });
      return;
    }
    const parent = await this.chooseFolder("create");
    if (!parent) return;
    const separator =
      parent.includes("/") && !parent.includes("\\") ? "/" : "\\";
    const root = `${parent.replace(/[\\/]+$/, "")}${separator}${name}`;
    this.publish({ newProject: null });
    return this.action(async () => {
      const problem = await this.openProject(root, "create", setDefault, {
        parent,
        name,
      });
      if (problem) {
        await this.retireInitializationFailure();
        this.publish({ error: problem });
        return undefined;
      }
      return root;
    });
  }
  async retryDefaultProject() {
    const root = this.state.defaultProjectRoot;
    if (!root || this.state.projectId || this.state.startupLoading) return;
    return this.action(async () => {
      const problem = await this.openProject(root, "automatic", false);
      if (problem) {
        await this.retireInitializationFailure();
        this.publish({
          startupFailure: problem,
          startupFailureKind: "project_open",
          error: null,
        });
      } else {
        this.publish({ startupFailure: null, startupFailureKind: null });
      }
    });
  }
  setCurrentAsDefault() {
    if (!this.state.projectId || !this.state.root) return Promise.resolve();
    return this.action(async () => {
      await this.writeDefaultProject(this.state.root);
    });
  }
  clearDefaultProject() {
    return this.action(async () => {
      await this.writeDefaultProject(null);
    });
  }
  private async openProject(
    root: string,
    purpose: "open" | "create" | "automatic",
    setDefault: boolean,
    creation?: { parent: string; name: string },
  ): Promise<string | null> {
    const generation = ++this.projectRequestGeneration;
    this.publish({
      root,
      ...(purpose === "automatic"
        ? { startupFailure: null, startupFailureKind: null }
        : {}),
    });
    const { result } = await this.operations.run(
      purpose === "create"
        ? {
            kind: "create_project",
            parent: creation?.parent ?? "",
            name: creation?.name ?? "",
          }
        : { kind: "open", root },
      text(
        purpose === "create"
          ? "project.creating"
          : purpose === "automatic"
            ? "project.defaultOpening"
            : "app.message07",
      ),
    );
    const opened = requireKind(result, "open");
    if (generation !== this.projectRequestGeneration || this.state.closing)
      return null;
    // 열기 결과를 인수한 순간 project owner를 남긴다. 다음 status 응답 유실로 잃지 않는다.
    this.retiredProject = null;
    this.publish({ projectId: opened.project });
    const project = await this.client.projectStatus(opened.project);
    if (project.kind !== "project") throw new BridgeFailure("protocol");
    if (
      generation !== this.projectRequestGeneration ||
      this.state.closing ||
      this.state.projectId !== opened.project
    )
      return null;
    const failure = opened.error ?? project.error;
    const problem = failure
      ? safeFailure(new BridgeFailure("boundary", undefined, failure))
      : null;
    const ready =
      !failure && project.status === "Ready" && project.runtime === "Ready";
    this.publish({
      project,
      message: ready
        ? purpose === "create"
          ? text("project.created")
          : text("controller.message03")
        : text("controller.message04"),
      error: ready ? null : problem,
    });
    if (!ready) return problem ?? text("project.initializationFailed");
    await this.loadList();
    if (setDefault) await this.writeDefaultProject(root);
    return null;
  }
  private async writeDefaultProject(root: string | null): Promise<boolean> {
    const generation = ++this.startupGeneration;
    const { result } = await this.operations.run(
      { kind: "project_settings_write", default_root: root },
      text(root ? "project.defaultSaving" : "project.defaultClearing"),
    );
    if (generation !== this.startupGeneration || this.state.closing)
      return false;
    if (result.kind !== "project_settings_write") {
      this.publish({
        error: resultError(result) ?? text("project.defaultWriteFailed"),
      });
      return false;
    }
    const observed = result.observed
      ? {
          defaultProjectRoot: result.default_root,
          defaultProjectObserved: true,
        }
      : { defaultProjectObserved: false };
    if (result.outcome === "not_applied") {
      this.publish({
        ...observed,
        startupFailure:
          result.observed && this.state.startupFailureKind === "settings_read"
            ? null
            : this.state.startupFailure,
        startupFailureKind:
          result.observed && this.state.startupFailureKind === "settings_read"
            ? null
            : this.state.startupFailureKind,
        settingsNotice: {
          kind: "not_applied",
          message: text(
            result.observed
              ? "project.defaultNotAppliedObserved"
              : "project.defaultNotAppliedUnknown",
          ),
        },
        error: null,
      });
      return false;
    }
    if (result.outcome === "applied_durability_uncertain") {
      this.publish({
        ...observed,
        startupFailure: result.observed ? null : this.state.startupFailure,
        startupFailureKind: result.observed
          ? null
          : this.state.startupFailureKind,
        settingsNotice: {
          kind: "durability_uncertain",
          message: text(
            result.observed
              ? "project.defaultUncertainObserved"
              : "project.defaultUncertainUnknown",
          ),
        },
        error: null,
      });
      return false;
    }
    this.publish({
      defaultProjectRoot: result.default_root,
      defaultProjectObserved: true,
      startupFailure: null,
      startupFailureKind: null,
      settingsNotice: null,
      error: null,
      message: text(
        result.default_root ? "project.defaultSaved" : "project.defaultCleared",
      ),
    });
    return true;
  }
  private editable() {
    return (
      this.state.ready &&
      !this.state.closing &&
      this.state.project?.status === "Ready" &&
      this.state.project.runtime === "Ready" &&
      !this.state.sessions.some((s) => s.problem) &&
      !this.state.retainedRefs.length
    );
  }
  refresh() {
    return this.action(async () => {
      await this.loadList();
      if (this.state.selection)
        await this.read(this.state.selection.content.id);
    });
  }
  async refreshAfterExternalFiles() {
    const project = this.state.projectId;
    const generation = this.projectGeneration();
    if (!project) return;
    if (this.state.busy || this.state.prompt || this.state.deciding) {
      await new Promise<void>((resolve, reject) => {
        let unsubscribe: () => void = () => {};
        const timeout = setTimeout(() => {
          unsubscribe();
          reject(new Error("svn_refresh_busy"));
        }, 30_000);
        const check = () => {
          if (
            this.state.projectId !== project ||
            this.projectGeneration() !== generation ||
            (!this.state.busy && !this.state.prompt && !this.state.deciding)
          ) {
            clearTimeout(timeout);
            unsubscribe();
            resolve();
          }
        };
        unsubscribe = this.subscribe(check);
        check();
      });
    }
    if (
      this.state.projectId !== project ||
      this.projectGeneration() !== generation
    )
      return;
    ++this.inspectionEpoch;
    this.inspectionPending.clear();
    const health = this.state.health;
    this.publish({
      assetInspection: null,
      inspectionPendingDocuments: [],
      ...(health
        ? {
            health: {
              ...health,
              phase: "idle" as const,
              inspection: null,
              message: null,
              error: null,
            },
          }
        : {}),
    });
    await this.refresh();
    if (this.state.listState === "failed")
      throw new Error("svn_refresh_failed");
  }
  private async loadList() {
    const project = this.state.project;
    if (!project || this.state.closing) return;
    const generation = ++this.listGeneration;
    this.publish({ listState: "loading" });
    const { result } = await this.operations.run(
      { kind: "list_templates", project: project.project },
      text("controller.message05"),
    );
    if (
      generation !== this.listGeneration ||
      this.state.project?.project !== project.project
    )
      return;
    if (result.kind !== "templates") {
      this.publish({
        listState: "failed",
        error: resultError(result) ?? text("controller.message06"),
      });
      return;
    }
    this.publish({ rows: result.templates, listState: "ready" });
  }
  private async read(id: Id) {
    const project = this.state.project?.project;
    if (!project || this.state.closing) return;
    const generation = ++this.selectionGeneration;
    const { result } = await this.operations.run(
      { kind: "read_template", project, template: id },
      text("controller.message07"),
    );
    const detail = requireKind(result, "template");
    this.views.set(detail.view, project);
    if (
      generation === this.selectionGeneration &&
      this.state.project?.project === project
    ) {
      this.publish({
        selection: { project, view: detail.view, content: detail.content },
      });
    }
    await this.releaseUnusedViews();
  }
  navigate(navigation: Navigation) {
    if (
      this.state.busy ||
      this.state.prompt ||
      this.state.closing ||
      this.state.fieldEditor?.drafts.some((d) => d.submitted && !d.committed) ||
      this.state.form?.submitted ||
      this.state.templateAction?.phase === "pending" ||
      (this.state.templateAction?.handedOff &&
        this.state.retainedRefs.length > 0)
    )
      return Promise.resolve();
    if (this.dirty()) {
      this.publish({ prompt: { attempt: null, navigation } });
      return Promise.resolve();
    }
    return this.action(() => this.performNavigation(navigation));
  }
  private async performNavigation(navigation: Navigation) {
    const previousField = this.state.fieldEditor?.field;
    await this.clearForm();
    this.publish({ fieldEditor: null, templateAction: null });
    await this.releaseUnusedViews();
    const source = this.state.selection;
    if (
      source &&
      this.editable() &&
      (navigation.kind === "duplicate" ||
        (navigation.kind === "delete" && source.content.lifecycle === "Active"))
    ) {
      this.publish({
        templateAction: {
          kind: navigation.kind,
          source,
          generation: ++this.managementGeneration,
          phase: "confirm",
          handedOff: false,
          result: null,
          artifact: null,
          message: "",
        },
      });
    }
    if (source && this.editable() && source.content.lifecycle === "Active") {
      if (navigation.kind === "fields_order")
        this.publish({
          fieldEditor: {
            field: source.content.id,
            generation: ++this.fieldGeneration,
            drafts: [
              fieldDraft(source, "fields_order", {
                kind: "reorder_fields",
                fields: [...source.content.fieldOrder],
              }),
            ],
          },
        });
      if (navigation.kind === "archive_field") {
        const field = source.content.fields.find(
          (f) => f.id === previousField && f.lifecycle === "Active",
        );
        if (field)
          this.publish({
            fieldEditor: {
              field: field.id,
              generation: ++this.fieldGeneration,
              drafts: [
                {
                  ...fieldDraft(source, "archive_field", {
                    kind: "archive_field",
                    field: field.id,
                  }),
                  requested: true,
                },
              ],
            },
          });
      }
      if (navigation.kind === "new_field") {
        const field = crypto.randomUUID();
        this.publish({
          fieldEditor: {
            generation: ++this.fieldGeneration,
            field,
            drafts: [
              fieldDraft(source, "create", {
                kind: "create_field",
                field,
                label: "",
                configuration: { kind: "single_line_text" },
                required: false,
                presentation: null,
                default: { kind: "unset" },
                index: null,
              }),
            ],
          },
        });
      }
    }
    if (source && navigation.kind === "field") {
      const field = source.content.fields.find((f) => f.id === navigation.id);
      if (field)
        this.publish({
          fieldEditor:
            this.editable() &&
            source.content.lifecycle === "Active" &&
            field.lifecycle === "Active" &&
            fieldKind(field)
              ? editField(source, field, ++this.fieldGeneration)
              : {
                  field: field.id,
                  generation: ++this.fieldGeneration,
                  drafts: [],
                },
        });
    }
    if (navigation.kind === "select") await this.read(navigation.id);
    if (navigation.kind === "create" && this.editable())
      this.publish({
        form: {
          kind: "create",
          name: "",
          initial: "",
          source: null,
          session: null,
          submitted: false,
          handedOff: false,
        },
        message: text("controller.message08"),
      });
    if (
      navigation.kind === "close_project" ||
      navigation.kind === "open_project"
    ) {
      await this.closeProject();
      if (navigation.kind === "open_project" && !this.state.projectId)
        await this.openProject(navigation.root, "open", false);
    }
  }
  rename() {
    if (
      !this.editable() ||
      !this.state.selection ||
      this.state.form ||
      this.state.fieldEditor ||
      this.state.selection.content.lifecycle !== "Active"
    )
      return Promise.resolve();
    return this.action(async () => {
      const source = this.state.selection;
      if (!source) return;
      const { result } = await this.operations.run(
        {
          kind: "begin_session",
          project: source.project,
          views: [source.view],
          purpose: "template",
        },
        text("controller.message09"),
      );
      const session = requireKind(result, "session");
      this.rememberSession(source.project, session.session, [source.view]);
      if (session.error || session.state !== "Editing") {
        this.sessionProblem(
          session.session,
          resultError(session) ?? "session_rejected",
        );
        this.publish({
          error: resultError(session) ?? text("controller.message10"),
        });
        return;
      }
      this.publish({
        form: {
          kind: "rename",
          name: source.content.name,
          initial: source.content.name,
          source,
          session: session.session,
          submitted: false,
          handedOff: false,
        },
        message: text("controller.message11"),
      });
    });
  }
  /** 한 확인은 저장된 원 view에 결합한다. 읽기 실패 재시도는 새 쓰기를 만들지 않는다. */
  executeTemplateAction() {
    const action = this.state.templateAction;
    if (!action || action.phase !== "confirm" || !this.editable())
      return Promise.resolve();
    if (action.source !== this.state.selection) {
      this.publish({
        templateAction: { ...action, message: text("template.sourceChanged") },
      });
      return Promise.resolve();
    }
    return this.action(async () => {
      const update = (patch: Partial<TemplateAction>) => {
        if (this.state.templateAction?.generation === action.generation)
          this.publish({
            templateAction: { ...this.state.templateAction, ...patch },
          });
      };
      update({ phase: "pending", message: text("template.pending") });
      const { source } = action;
      let sessionId: Id | null = null;
      if (action.kind === "delete") {
        try {
          const { result } = await this.operations.run(
            {
              kind: "begin_session",
              project: source.project,
              views: [source.view],
              purpose: "template",
            },
            text("template.deleteBegin"),
          );
          const session = requireKind(result, "session");
          sessionId = session.session;
          this.rememberSession(source.project, sessionId, [source.view]);
          if (session.error || session.state !== "Editing") {
            this.sessionProblem(
              sessionId,
              resultError(session) ?? text("template.sessionFailed"),
            );
            update({
              phase: "complete",
              result,
              message: text("template.unconfirmed"),
            });
            return;
          }
        } catch (error: unknown) {
          update({ phase: "confirm" });
          throw error;
        }
      }
      // 세션을 기다리는 동안 종료가 시작되면 새 쓰기를 인수시키지 않고 얻은 owner만 정리한다.
      if (
        this.state.closing ||
        this.state.project?.project !== source.project ||
        this.state.selection !== source ||
        this.state.templateAction?.generation !== action.generation
      ) {
        update({ phase: "complete", message: text("template.unconfirmed") });
        if (sessionId) await this.endSession(source.project, sessionId);
        return;
      }
      const input: Work =
        action.kind === "duplicate"
          ? {
              kind: "duplicate_template",
              project: source.project,
              view: source.view,
            }
          : {
              kind: "tombstone_template",
              project: source.project,
              session: sessionId!,
              view: source.view,
              revision: source.content.revision,
            };
      const { result, retained } = await this.operations
        .run(
          input,
          text(
            action.kind === "duplicate"
              ? "template.duplicate"
              : "template.delete",
          ),
          "ordinary",
          () => update({ handedOff: true }),
        )
        .catch(async (error: unknown) => {
          update({ phase: "confirm" });
          if (sessionId) await this.endSession(source.project, sessionId);
          throw error;
        });
      if (result.kind === "write")
        this.rememberSession(
          source.project,
          result.session,
          action.kind === "delete" ? [source.view] : [],
        );
      if (retained)
        this.publish({ retainedRefs: [...this.state.retainedRefs, retained] });
      const committed =
        result.kind === "write" &&
        (result.disk === "committed" || result.disk === "no_write");
      update({
        phase: "complete",
        result,
        artifact: committed ? result.artifact : null,
        message: text(
          committed
            ? action.kind === "duplicate"
              ? "template.duplicated"
              : "template.deleted"
            : "template.unconfirmed",
        ),
      });
      if (committed) {
        // 먼저 확정 사실과 새 artifact를 보관한다. 아래 release/read 실패로 실행 버튼을 되살리지 않는다.
        try {
          await this.endSession(source.project, result.session);
          if (
            !this.state.closing &&
            this.state.templateAction?.generation === action.generation &&
            this.state.selection === source
          ) {
            await this.loadList();
            if (result.artifact) await this.read(result.artifact);
          }
        } catch (error: unknown) {
          update({ message: text("template.readFollowup") });
          this.publish({ error: safeFailure(error) });
        }
      } else {
        if (!retained && sessionId)
          await this.endSession(source.project, sessionId);
        await this.refreshRetained();
      }
    });
  }
  readTemplateAction() {
    const action = this.state.templateAction;
    if (
      !action?.artifact ||
      action.phase !== "complete" ||
      this.state.project?.project !== action.source.project ||
      this.state.closing
    )
      return Promise.resolve();
    return this.action(async () => {
      await this.loadList();
      if (this.state.templateAction?.generation === action.generation)
        await this.read(action.artifact!);
    });
  }
  setField(property: FieldProperty, edit: FieldEdit) {
    const editor = this.state.fieldEditor;
    if (
      !editor ||
      !this.editable() ||
      this.state.busy ||
      this.state.prompt ||
      this.state.deciding
    )
      return;
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: editor.drafts.map((d) =>
          d.property === property &&
          !d.submitted &&
          sameEditTarget(d.edit, edit) &&
          (d.edit.kind === edit.kind ||
            (d.property === "default" &&
              ["default", "keep_default"].includes(edit.kind)))
            ? { ...d, edit, message: "", confirmation: false }
            : d,
        ),
      },
    });
  }
  startOrder() {
    const editor = this.state.fieldEditor;
    const source = this.state.selection;
    const field = source?.content.fields.find((f) => f.id === editor?.field);
    if (
      !editor ||
      !source ||
      !field ||
      !this.editable() ||
      this.state.busy ||
      this.state.prompt ||
      field.lifecycle !== "Active" ||
      source.content.lifecycle !== "Active" ||
      !["SingleChoice", "MultiChoice"].includes(field.kind) ||
      editor.drafts.some((d) => d.property === "options_order")
    )
      return;
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: [
          ...editor.drafts,
          fieldDraft(source, "options_order", {
            kind: "reorder_options",
            field: field.id,
            options: [...field.optionOrder],
          }),
        ],
      },
    });
  }
  moveOrder(property: FieldProperty, id: Id, position: number) {
    const draft = this.state.fieldEditor?.drafts.find(
      (d) => d.property === property,
    );
    if (!draft || !this.orderMovable(property)) return;
    const edit = draft.edit;
    if (edit.kind === "reorder_fields")
      this.setField(property, {
        ...edit,
        fields: moveId(edit.fields, id, position),
      });
    if (edit.kind === "reorder_options")
      this.setField(property, {
        ...edit,
        options: moveId(edit.options, id, position),
      });
  }
  orderMovable(property: FieldProperty) {
    const draft = this.state.fieldEditor?.drafts.find(
      (d) => d.property === property,
    );
    const source = this.state.selection;
    const order = draft && orderIds(draft.edit);
    const active = draft && source && sourceOrder(source, draft.edit);
    return (
      !!draft &&
      !draft.submitted &&
      !this.state.busy &&
      !this.state.prompt &&
      !this.state.deciding &&
      this.editable() &&
      !!order &&
      !!active &&
      exactOrder(order, active) &&
      source?.content.lifecycle === "Active" &&
      (draft.edit.kind === "reorder_fields" ||
        source.content.fields.some(
          (f) => f.id === editOwner(draft.edit) && f.lifecycle === "Active",
        ))
    );
  }
  cancelOrder(property: FieldProperty) {
    const draft = this.state.fieldEditor?.drafts.find(
      (d) => d.property === property,
    );
    if (draft && orderIds(draft.initial))
      this.setField(property, structuredClone(draft.initial));
  }
  startArchive(optionId?: Id) {
    const editor = this.state.fieldEditor;
    const source = this.state.selection;
    const field = source?.content.fields.find((f) => f.id === editor?.field);
    if (
      !editor ||
      !source ||
      !field ||
      !this.editable() ||
      this.state.busy ||
      this.state.prompt ||
      field.lifecycle !== "Active" ||
      source.content.lifecycle !== "Active"
    )
      return;
    if (!optionId && editor.drafts.some(fieldDirty)) {
      this.publish({
        prompt: { attempt: null, navigation: { kind: "archive_field" } },
      });
      return;
    }
    if (
      optionId &&
      !field.options.some((o) => o.id === optionId && o.lifecycle === "Active")
    )
      return;
    const property = optionId
      ? optionKey(field.id, optionId, "archive")
      : "archive_field";
    if (editor.drafts.some((d) => d.property === property)) return;
    const edit: FieldEdit = optionId
      ? {
          kind: "archive_option",
          field: field.id,
          option: optionId,
          repair: null,
        }
      : { kind: "archive_field", field: field.id };
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: [
          ...editor.drafts,
          { ...fieldDraft(source, property, edit), requested: true },
        ],
      },
    });
  }
  confirmArchive(property: FieldProperty, confirmation: boolean) {
    const editor = this.state.fieldEditor;
    if (
      !editor ||
      !this.editable() ||
      this.state.busy ||
      this.state.prompt ||
      this.state.deciding
    )
      return;
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: editor.drafts.map((d) =>
          d.property === property && !d.submitted
            ? { ...d, confirmation, message: "" }
            : d,
        ),
      },
    });
  }
  startOption(optionId?: Id) {
    const editor = this.state.fieldEditor;
    const source = this.state.selection;
    const field = source?.content.fields.find((f) => f.id === editor?.field);
    if (
      !editor ||
      !source ||
      !field ||
      !this.editable() ||
      this.state.busy ||
      this.state.prompt ||
      source.content.lifecycle !== "Active" ||
      field.lifecycle !== "Active" ||
      !["single_choice", "multi_choice"].includes(fieldKind(field))
    )
      return;
    const option = optionId
      ? field.options.find((o) => o.id === optionId && o.lifecycle === "Active")
      : { id: crypto.randomUUID(), label: "" };
    if (!option) return;
    const property = optionKey(
      field.id,
      option.id,
      optionId ? "rename" : "add",
    );
    if (
      editor.drafts.some(
        (d) =>
          d.property === property ||
          (d.edit.kind === "add_option" && !optionId),
      )
    )
      return;
    const edit: FieldEdit = optionId
      ? {
          kind: "rename_option",
          field: field.id,
          option: option.id,
          label: option.label,
        }
      : { kind: "add_option", field: field.id, option, index: null };
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: [
          ...editor.drafts,
          { ...fieldDraft(source, property, edit), requested: !optionId },
        ],
      },
    });
  }
  cancelFieldDraft(property: FieldProperty) {
    const editor = this.state.fieldEditor;
    if (!editor || this.state.busy || this.state.prompt || this.state.deciding)
      return;
    const draft = editor.drafts.find((d) => d.property === property);
    if (!draft || draft.submitted) return;
    this.publish({
      fieldEditor: {
        ...editor,
        drafts: editor.drafts.filter((d) => d !== draft),
      },
    });
    void this.releaseUnusedViews();
  }
  reconfirmField(property: FieldProperty) {
    return this.action(async () => {
      const editor = this.state.fieldEditor;
      const draft = editor?.drafts.find((d) => d.property === property);
      if (
        !editor ||
        !draft ||
        (draft.submitted && !draft.committed) ||
        !this.editable()
      )
        return;
      await this.read(draft.source.content.id);
      const source = this.state.selection;
      if (
        this.state.fieldEditor?.generation !== editor.generation ||
        !source ||
        source.project !== draft.source.project ||
        source.content.id !== draft.source.content.id
      )
        return;
      // 버튼을 누른 속성만 새 읽기에 결합한다. 다른 원 입력의 source를 자동 승격하지 않는다.
      const field = source.content.fields.find((f) => f.id === editor.field);
      if (
        !draft.committed &&
        (source.content.lifecycle !== "Active" ||
          (property !== "create" &&
            property !== "fields_order" &&
            (field?.lifecycle !== "Active" || !fieldKind(field))))
      ) {
        this.publish({ error: text("field.unavailable") });
        return;
      }
      if (draft.committed && (field || property === "fields_order")) {
        this.publish({
          fieldEditor:
            property === "create" && field
              ? editField(source, field, editor.generation)
              : {
                  ...editor,
                  drafts: savedFieldDrafts(editor, draft, source),
                },
        });
      } else {
        const oldOrder = orderIds(draft.edit);
        if (
          oldOrder &&
          !exactOrder(oldOrder, sourceOrder(source, draft.edit) ?? [])
        ) {
          this.publish({ error: text("order.membershipChanged") });
          return;
        }
        this.publish({
          fieldEditor: {
            ...editor,
            drafts: editor.drafts.map((d) =>
              d === draft
                ? { ...d, source, confirmation: false, message: "" }
                : d,
            ),
          },
        });
      }
      await this.releaseUnusedViews();
    });
  }
  saveField(property: FieldProperty) {
    if (!this.editable()) return Promise.resolve();
    return this.action(async () => {
      const editor = this.state.fieldEditor;
      const draft = editor?.drafts.find((d) => d.property === property);
      if (!editor || !draft || draft.submitted) return;
      const bound = () =>
        this.state.fieldEditor?.generation === editor.generation &&
        this.state.fieldEditor.field === editor.field &&
        this.state.projectId === draft.source.project;
      const update = (patch: Partial<typeof draft>) => {
        const current = this.state.fieldEditor;
        if (bound() && current)
          this.publish({
            fieldEditor: {
              ...current,
              drafts: current.drafts.map((d) =>
                d.property === property ? { ...d, ...patch } : d,
              ),
            },
          });
      };
      const latest = this.state.selection;
      const archive =
        draft.edit.kind === "archive_option" ||
        draft.edit.kind === "archive_field";
      const latestField = latest?.content.fields.find(
        (f) => f.id === editOwner(draft.edit),
      );
      if (
        latest?.content.lifecycle !== "Active" ||
        (draft.edit.kind !== "reorder_fields" &&
          draft.edit.kind !== "create_field" &&
          latestField?.lifecycle !== "Active") ||
        ((draft.edit.kind === "rename_option" ||
          draft.edit.kind === "archive_option") &&
          !latestField?.options.some(
            (o) =>
              o.id ===
                (
                  draft.edit as Extract<
                    FieldEdit,
                    { kind: "rename_option" | "archive_option" }
                  >
                ).option && o.lifecycle === "Active",
          ))
      ) {
        update({ message: text("field.unavailable") });
        return;
      }
      if (archive && latest.view !== draft.source.view) {
        update({ confirmation: false, message: text("archive.stale") });
        return;
      }
      if (
        draft.edit.kind === "archive_field" &&
        editor.drafts.some((d) => d !== draft && fieldDirty(d))
      ) {
        update({ message: text("archive.otherDrafts") });
        return;
      }
      const inputError = fieldInputError(draft.edit) ?? managementError(draft);
      const order = orderIds(draft.edit);
      if (order && !exactOrder(order, sourceOrder(latest, draft.edit) ?? [])) {
        update({ message: text("order.membershipChanged") });
        return;
      }
      if (inputError) {
        update({ message: inputError });
        return;
      }
      const { result: begun } = await this.operations.run(
        {
          kind: "begin_session",
          project: draft.source.project,
          views: [draft.source.view],
          purpose: "template",
        },
        text("field.begin"),
      );
      const session = requireKind(begun, "session");
      this.rememberSession(draft.source.project, session.session, [
        draft.source.view,
      ]);
      if (session.error || session.state !== "Editing") {
        this.sessionProblem(
          session.session,
          resultError(session) ?? text("field.unavailable"),
        );
        update({ message: resultError(session) ?? text("field.unavailable") });
        return;
      }
      update({ submitted: true, message: text("field.saving") });
      if (this.state.closing || !bound()) {
        await this.endSession(draft.source.project, session.session);
        return;
      }
      let completed;
      try {
        completed = await this.operations.run(
          {
            kind: "update_template",
            project: draft.source.project,
            session: session.session,
            view: draft.source.view,
            revision: draft.source.content.revision,
            edit: draft.edit,
          },
          text("field.save"),
          "ordinary",
          () => update({ handedOff: true }),
        );
      } catch (error: unknown) {
        update({
          submitted: false,
          handedOff: false,
          message: safeFailure(error),
        });
        await this.endSession(draft.source.project, session.session);
        throw error;
      }
      const { result, retained } = completed;
      if (retained)
        this.publish({ retainedRefs: [...this.state.retainedRefs, retained] });
      if (
        result.kind === "write" &&
        (result.disk === "committed" || result.disk === "no_write")
      ) {
        const message =
          result.changed === false
            ? text("field.unchanged")
            : text("field.saved");
        // 확정 결과를 먼저 보존한다. 아래 읽기/해제 실패는 다시 생성할 이유가 아니다.
        update({ message, handedOff: true, committed: true });
        this.publish({ message, error: resultError(result) });
        try {
          await this.endSession(draft.source.project, result.session);
          if (!this.state.closing) {
            await this.loadList();
            await this.read(draft.source.content.id);
            const source = this.state.selection;
            if (
              bound() &&
              !this.state.closing &&
              source &&
              source.content.id === draft.source.content.id
            ) {
              const field = source.content.fields.find(
                (f) => f.id === editor.field,
              );
              const current = this.state.fieldEditor;
              if ((field || property === "fields_order") && current) {
                this.publish({
                  fieldEditor:
                    property === "create" && field
                      ? editField(source, field, editor.generation)
                      : {
                          ...current,
                          drafts: savedFieldDrafts(current, draft, source).map(
                            (d) =>
                              d.property === property ? { ...d, message } : d,
                          ),
                        },
                });
              }
            }
          }
          if (
            result.cleanup_failed ||
            result.recovery_required ||
            result.warnings.length
          )
            this.publish({ message: text("field.followup") });
        } catch (error: unknown) {
          this.publish({
            message: text("field.followup"),
            error: safeFailure(error),
          });
        }
      } else {
        update({ message: resultError(result) ?? text("field.unconfirmed") });
        this.publish({
          message: text("field.unconfirmed"),
          error: resultError(result),
        });
        if (!retained && result.kind === "rejected" && !result.input_retained) {
          update({ submitted: false, handedOff: false });
          await this.endSession(draft.source.project, session.session);
        }
        await this.refreshRetained();
      }
    });
  }
  private rememberSession(project: Id, id: Id, views: Id[] = []) {
    if (!this.state.sessions.some((s) => s.id === id))
      this.publish({
        sessions: [
          ...this.state.sessions,
          { project, id, problem: null, observation: null, views },
        ],
      });
  }
  save() {
    if (!this.editable() || !this.state.form || this.state.form.submitted)
      return Promise.resolve();
    return this.action(async () => {
      const form = this.state.form;
      const project = this.state.project?.project;
      if (!form || !project) return;
      this.publish({
        form: { ...form, submitted: true },
        message: text("controller.message12"),
      });
      const input: Work =
        form.kind === "create"
          ? {
              kind: "create_template",
              project,
              name: form.name,
              presentation: null,
            }
          : {
              kind: "update_template",
              project,
              session: form.session!,
              view: form.source!.view,
              revision: form.source!.content.revision,
              edit: { kind: "name", name: form.name },
            };
      const { result, retained } = await this.operations
        .run(
          input,
          form.kind === "create"
            ? text("controller.message13")
            : text("controller.message14"),
          "ordinary",
          () => {
            // 예약/전송 시도만으로 backend 인수를 추측하지 않는다. 수락 또는 같은 ID 조회가 근거다.
            if (this.state.form)
              this.publish({ form: { ...this.state.form, handedOff: true } });
          },
        )
        .catch((error: unknown) => {
          // 예약 자체가 실패하면 아직 submit을 호출하지 않았다. 이 폼은 미제출로 남긴다.
          if (this.state.form && !this.state.closing)
            this.publish({ form: { ...form, submitted: false } });
          throw error;
        });
      if (result.kind === "write")
        this.rememberSession(project, result.session);
      if (retained)
        this.publish({ retainedRefs: [...this.state.retainedRefs, retained] });
      if (
        result.kind === "write" &&
        (result.disk === "committed" || result.disk === "no_write")
      ) {
        const saved =
          result.changed === false
            ? text("controller.message15")
            : text("controller.message16");
        this.publish({
          form: null,
          message:
            result.cleanup_failed || result.recovery_required
              ? text("template.postSave")
              : result.warnings.length
                ? text("template.savedWarnings", {
                    count: String(result.warnings.length),
                  })
                : saved,
          error: resultError(result),
        });
        await this.endSession(project, result.session);
        if (!this.state.closing) {
          await this.loadList();
          const id = result.artifact ?? form.source?.content.id;
          if (id) await this.read(id);
        }
      } else {
        this.publish({
          error: resultError(result) ?? text("controller.message17"),
          message: text("controller.message18"),
        });
        await this.refreshRetained();
      }
    });
  }
  private async clearForm() {
    const form = this.state.form;
    if (form?.submitted)
      throw new BridgeFailure("boundary", undefined, {
        code: "owners_remain",
        nextAction: "",
      });
    if (form?.session)
      await this.endSession(form.source!.project, form.session);
    this.publish({ form: null });
    await this.releaseUnusedViews();
  }
  private async endSession(project: Id, session: Id) {
    const { result } = await this.operations
      .run(
        { kind: "session_control", project, session, control: "end" },
        text("controller.message19"),
        "control",
      )
      .catch((error: unknown) => {
        this.sessionProblem(session, safeFailure(error));
        throw error;
      });
    const error = resultError(result);
    if (result.kind === "control" && !error) {
      this.publish({
        sessions: this.state.sessions.filter((s) => s.id !== session),
      });
      this.resumeDocumentInspection(project);
    } else
      this.publish({
        sessions: this.state.sessions.map((s) =>
          s.id === session ? { ...s, problem: error ?? "session_rejected" } : s,
        ),
        error,
      });
  }
  private sessionProblem(id: Id, problem: string) {
    this.publish({
      sessions: this.state.sessions.map((s) =>
        s.id === id ? { ...s, problem } : s,
      ),
    });
  }
  async releaseUnusedViews() {
    if (this.state.operations.length) return;
    for (const [view, project] of this.views) {
      if (
        view === this.state.selection?.view ||
        this.workspaceOwnsView?.(view) ||
        view === this.state.form?.source?.view ||
        view === this.state.templateAction?.source.view ||
        this.state.fieldEditor?.drafts.some((d) => d.source.view === view) ||
        this.state.sessions.some((session) => session.views.includes(view))
      )
        continue;
      try {
        await this.client.releaseView(project, view);
        this.views.delete(view);
      } catch (error: unknown) {
        this.publish({ error: safeFailure(error) });
      }
    }
  }
  private async refreshRetained() {
    const references = await this.client.listRetained();
    this.publish({ retainedRefs: references });
    const retained: Retained[] = [];
    for (const ref of references)
      retained.push(await this.client.readRetained(ref));
    this.publish({ retained });
  }
  private async observeSessions() {
    for (const session of this.state.sessions.filter((s) => s.problem)) {
      const observation = await this.client.sessionStatus(
        session.project,
        session.id,
      );
      if (observation.kind !== "session" || observation.session !== session.id)
        throw new BridgeFailure("protocol");
      this.publish({
        sessions: this.state.sessions.map((s) =>
          s.id === session.id ? { ...s, observation } : s,
        ),
      });
    }
  }
  abandon(id: Id) {
    const retained = this.state.retained.find((r) => r.retained.id === id);
    if (!retained?.g6_clearable) return Promise.resolve();
    return this.action(async () => {
      const { result } = await this.operations.run(
        { kind: "abandon_retained", retained: retained.retained },
        text("controller.message20"),
        "control",
      );
      if (result.kind !== "retained_handled" || result.action !== "abandoned") {
        this.publish({ error: resultError(result) ?? "owners_remain" });
        return;
      }
      this.publish({
        form: null,
        fieldEditor: this.state.fieldEditor
          ? {
              ...this.state.fieldEditor,
              drafts: this.state.fieldEditor.drafts.map((d) =>
                d.submitted &&
                retained.intent?.kind === "update_template" &&
                JSON.stringify(retained.intent.edit) === JSON.stringify(d.edit)
                  ? {
                      ...d,
                      submitted: false,
                      handedOff: false,
                      message: text("field.stale"),
                    }
                  : d,
              ),
            }
          : null,
        message: text("controller.message21"),
      });
      for (const session of [...this.state.sessions])
        await this.endSession(session.project, session.id);
      await this.refreshRetained();
      if (!this.state.closing) await this.loadList();
      await this.releaseUnusedViews();
    });
  }
  recover() {
    return this.action(async () => {
      const project = this.state.project?.project;
      if (project) {
        const { result } = await this.operations.run(
          { kind: "recover", project },
          text("controller.message22"),
          "control",
        );
        this.publish({ error: resultError(result) });
      }
    });
  }
  finishSession(id: Id, retry = false) {
    return this.action(async () => {
      const session = this.state.sessions.find((s) => s.id === id);
      if (!session) return;
      const observed = session.observation;
      if (
        !observed ||
        observed.active_dirty ||
        observed.custody ||
        (retry
          ? observed.state !== "ReleaseFailed"
          : !["Editing", "ReadOnly", "LockLost"].includes(observed.state))
      )
        return;
      if (retry) {
        const { result } = await this.operations.run(
          {
            kind: "session_control",
            project: session.project,
            session: id,
            control: "retry_release",
          },
          text("controller.message23"),
          "control",
        );
        if (resultError(result)) {
          this.publish({ error: resultError(result) });
          return;
        }
      }
      await this.endSession(session.project, id);
      await this.releaseUnusedViews();
    });
  }
  private async closeProject() {
    const project = this.state.project?.project;
    if (!project || this.state.retained.length || this.state.sessions.length) {
      this.publish({
        error: text("controller.message24"),
      });
      return;
    }
    this.publish({ selection: null });
    await this.releaseUnusedViews();
    const { result } = await this.operations.run(
      { kind: "close", project },
      text("app.message10"),
      "control",
    );
    if (resultError(result)) {
      this.publish({ error: resultError(result) });
      return;
    }
    let status: Project | null = null;
    for (let attempt = 0; attempt < 3000; ++attempt) {
      const observed = await this.client.projectStatus(project);
      if (observed.kind !== "project") throw new BridgeFailure("protocol");
      if (this.state.projectId !== project) return;
      status = observed;
      this.publish({
        project: observed,
        message: text(
          observed.shutdown.forceActive
            ? "svn.forceClosing"
            : "controller.message25",
        ),
      });
      if (observed.shutdown.reportPending) {
        // worker가 pending을 먼저 알리고 상세 보고를 다음 상태 조회에 넘길 수 있다.
        if (
          observed.shutdown.reports.length === 0 &&
          observed.shutdown.forceResults.length === 0
        ) {
          await new Promise((resolve) => setTimeout(resolve, 10));
          continue;
        }
        // 성공 보고는 별도 확인 동작 없이 인수한다. 실패 보고는 화면에 남긴다.
        if (!cleanShutdownReport(observed.shutdown)) return;
        await this.client.acknowledgeShutdown(project);
        continue;
      }
      if (observed.shutdown.joined) break;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    if (status?.shutdown.joined && status.shutdown.normalExitAllowed)
      await this.retireProjectOwner(project);
  }

  private async retireProjectOwner(project: Id): Promise<boolean> {
    if (this.state.projectId !== project) return false;
    const { result } = await this.operations.run(
      { kind: "retire_project", project },
      text("controller.message26"),
      "control",
    );
    // 상태 조회와 명시 닫기가 동시에 끝나도 먼저 퇴역한 owner의 결과를 다시 게시하지 않는다.
    if (this.state.projectId !== project) return false;
    if (result.kind !== "project_retired") {
      this.publish({ error: resultError(result) ?? "owners_remain" });
      return false;
    }
    if (this.state.projectId !== project) return false;
    this.retiredProject = project;
    ++this.selectionGeneration;
    ++this.listGeneration;
    this.publish({
      project: null,
      projectId: null,
      selection: null,
      rows: [],
      listState: "idle",
      sessions: [],
      projectData: null,
      health: null,
      assetInspection: null,
      inspectionPendingDocuments: [],
      fileManagerTarget: null,
      error: null,
      errorDetail: null,
      message: text("controller.message27"),
    });
    this.inspectionPending.clear();
    ++this.inspectionEpoch;
    return true;
  }
  /** 확정 초기화 실패 owner만 정상 종료·인수한 뒤 입력을 보존한 chooser로 돌아간다. */
  async correctProjectPath(): Promise<boolean> {
    let corrected = false;
    await this.action(async () => {
      corrected = await this.retireInitializationFailure();
    });
    return corrected;
  }
  private async retireInitializationFailure(): Promise<boolean> {
    const projectId = this.state.projectId;
    if (!projectId || this.state.retained.length || this.state.sessions.length)
      return false;
    let project = this.state.project;
    if (!project) {
      const status = await this.client.projectStatus(projectId);
      if (status.kind !== "project") throw new BridgeFailure("protocol");
      if (this.state.projectId !== projectId) return false;
      project = status;
      this.publish({ project });
    }
    if (
      project.error?.code !== "initialization_failed" &&
      project.error?.code !== "project_not_empty"
    )
      return false;
    if (!project.shutdown.joined) {
      const { result } = await this.operations.run(
        { kind: "close", project: projectId },
        text("app.message10"),
        "control",
      );
      const problem = resultError(result);
      if (problem) {
        this.publish({ error: problem });
        return false;
      }
      // close 결과는 종료 요청의 인수만 뜻한다. 초기화 실패 worker의 join과
      // 보고서 인수까지 상태를 진행시킨 뒤에만 project owner를 퇴역시킨다.
      for (let attempt = 0; attempt < 500; ++attempt) {
        const status = await this.client.projectStatus(projectId);
        if (status.kind !== "project") throw new BridgeFailure("protocol");
        if (this.state.projectId !== projectId) return false;
        project = status;
        this.publish({ project });
        if (project.shutdown.reportPending) {
          await this.client.acknowledgeShutdown(projectId);
          continue;
        }
        if (project.shutdown.joined) break;
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
    if (project.shutdown.reportPending) {
      await this.client.acknowledgeShutdown(projectId);
      const status = await this.client.projectStatus(projectId);
      if (status.kind !== "project") throw new BridgeFailure("protocol");
      if (this.state.projectId !== projectId) return false;
      project = status;
      this.publish({ project });
    }
    const terminalInitializationFailure =
      project.shutdown.joined &&
      project.shutdown.resourcesComplete &&
      !project.shutdown.reportPending &&
      ["initialization_failed", "project_not_empty"].includes(
        project.error?.code ?? "",
      );
    if (
      !project.shutdown.joined ||
      (!project.shutdown.normalExitAllowed && !terminalInitializationFailure)
    )
      return false;
    return this.retireProjectOwner(projectId);
  }
  retireProject() {
    return this.action(async () => {
      const project = this.state.project;
      if (!project?.shutdown.joined || !project.shutdown.normalExitAllowed)
        return;
      await this.retireProjectOwner(project.project);
    });
  }
  requestClose() {
    ++this.pickerGeneration;
    return this.client
      .shutdown()
      .then(() => this.checkStatus())
      .catch((error: unknown) => this.publish({ error: safeFailure(error) }));
  }
  async decide(proceed: boolean) {
    const prompt = this.state.prompt;
    if (!prompt || this.state.deciding) return;
    const decision = { prompt, request: ++this.closeRequest };
    this.decision = decision;
    this.publish({ deciding: true });
    try {
      if (prompt.attempt) {
        const reply = await this.client.uiCloseDecision(
          prompt.attempt,
          proceed,
        );
        // A의 늦은 응답은 새 조회/선택 B의 확인창이나 입력을 변경할 수 없다.
        if (
          this.decision === decision &&
          decision.request === this.closeRequest
        )
          await this.reconcileClose(reply);
      } else {
        this.publish({ prompt: null });
        if (proceed && prompt.navigation)
          await this.performNavigation(prompt.navigation);
      }
    } catch (error: unknown) {
      if (this.decision === decision)
        this.publish({ error: safeFailure(error) });
    } finally {
      if (this.decision === decision) {
        this.decision = null;
        this.publish({ deciding: false });
      }
      await this.checkStatus();
    }
  }
  private async reconcileClose(close: CloseStatus) {
    if (!close.closing && this.workspaceClose) {
      this.workspaceClose(close.attempt);
      return;
    }
    if (close.closing || close.attempt) ++this.pickerGeneration;
    if (close.closing) {
      ++this.projectRequestGeneration;
      ++this.startupGeneration;
    }
    if (close.closing) {
      this.decision = null;
      this.publish({
        closing: true,
        prompt: null,
        deciding: false,
        form: this.state.form?.handedOff ? this.state.form : null,
        fieldEditor: this.state.fieldEditor
          ? {
              ...this.state.fieldEditor,
              drafts: this.state.fieldEditor.drafts.filter((d) => d.handedOff),
            }
          : null,
      });
    } else if (
      !this.state.closing &&
      (close.attempt || this.state.prompt?.attempt) &&
      close.attempt !== this.state.prompt?.attempt
    ) {
      // 회수된 A 대신 현재 B를 제시하되, A의 취소/버리기 의사는 승계하지 않는다.
      this.decision = null;
      this.publish({
        prompt: close.attempt
          ? { attempt: close.attempt, navigation: null }
          : null,
        deciding: false,
      });
      if (close.attempt && !this.dirty()) await this.decide(true);
    }
  }
  private async checkCloseStatus(connection: Connection) {
    const request = ++this.closeRequest;
    try {
      const close = await this.client.uiCloseStatus();
      if (this.connection !== connection || request !== this.closeRequest)
        return;
      this.confirmReady(close, connection);
      await this.reconcileClose(close);
      if (
        !close.enabled &&
        !close.closing &&
        connection.registration === "unconfirmed"
      )
        await this.initialize(connection);
    } catch (error: unknown) {
      if (this.connection === connection && request === this.closeRequest)
        this.publish({ error: safeFailure(error) });
    }
  }
  async checkStatus() {
    this.start();
    const connection = this.connection;
    if (!connection?.listening) return;
    // 닫기 복구는 오래 걸리는 일반 상태 조회나 이전 확인 응답 뒤에 갇히지 않는다.
    await this.checkCloseStatus(connection);
    if (this.refreshing) {
      this.refreshAgain = true;
      return;
    }
    this.refreshing = true;
    let queriedProject: Id | null = null;
    try {
      await this.operations.queryAll();
      const app = await this.client.appStatus();
      if (app.kind === "app") {
        this.publish({ app, closing: this.state.closing || app.closing });
        if (app.retained_edits !== "0" || this.state.retainedRefs.length)
          await this.refreshRetained();
      }
      const projectId = this.state.projectId;
      if (projectId) {
        queriedProject = projectId;
        const project = await this.client.projectStatus(projectId);
        if (project.kind === "project" && this.state.projectId === projectId) {
          let observed = project;
          // 앱 종료도 정상 결과는 해당 owner에서 인수한다. 실패 보고는 사용자 조치로 남긴다.
          if (
            observed.shutdown.closing &&
            cleanShutdownReport(observed.shutdown)
          ) {
            await this.client.acknowledgeShutdown(projectId);
            const after = await this.client.projectStatus(projectId);
            if (after.kind !== "project") throw new BridgeFailure("protocol");
            observed = after;
          }
          if (this.state.projectId === projectId)
            this.publish({ project: observed });
          if (
            this.state.closing &&
            observed.shutdown.joined &&
            observed.shutdown.resourcesComplete &&
            !observed.shutdown.reportPending &&
            ["initialization_failed", "project_not_empty"].includes(
              observed.error?.code ?? "",
            )
          ) {
            await this.retireInitializationFailure();
          }
          if (
            observed.shutdown.closing &&
            !this.state.closing &&
            !this.state.busy &&
            observed.shutdown.joined &&
            observed.shutdown.normalExitAllowed &&
            this.state.projectId === projectId
          )
            await this.retireProjectOwner(projectId);
        }
      }
      if (!this.state.project?.shutdown.joined) await this.observeSessions();
    } catch (error: unknown) {
      // 정상 퇴역과 겹친 늦은 상태 응답은 이미 없는 owner의 오류다.
      const staleRetiredOwner =
        error instanceof BridgeFailure &&
        error.boundary?.code === "unknown_id" &&
        ((queriedProject && this.state.projectId !== queriedProject) ||
          (this.retiredProject && !this.state.projectId));
      if (!staleRetiredOwner) this.publish({ error: safeFailure(error) });
    } finally {
      this.refreshing = false;
      if (this.refreshAgain) {
        this.refreshAgain = false;
        void this.checkStatus();
      }
    }
  }
  acknowledgeShutdown() {
    return this.action(async () => {
      const project = this.state.project?.project;
      if (!project) return;
      await this.client.acknowledgeShutdown(project);
      const status = await this.client.projectStatus(project);
      if (status.kind !== "project") throw new BridgeFailure("protocol");
      if (this.state.projectId !== project) return;
      this.publish({ project: status });
      if (
        this.state.closing &&
        status.shutdown.joined &&
        status.shutdown.resourcesComplete &&
        !status.shutdown.reportPending &&
        ["initialization_failed", "project_not_empty"].includes(
          status.error?.code ?? "",
        )
      ) {
        await this.retireInitializationFailure();
        return;
      }
      if (status.shutdown.joined && status.shutdown.normalExitAllowed)
        await this.retireProjectOwner(project);
    });
  }
  releaseRound() {
    return this.action(async () => {
      const project = this.state.project?.project;
      if (project) await this.client.releaseRound(project);
    });
  }
  retryNative() {
    return this.action(async () => {
      const generation = this.state.app?.native_cleanup.generation;
      if (generation) await this.client.retryNativeCleanup(generation);
    });
  }
}

let singleton: TemplateController | undefined;
export function appController() {
  return (singleton ??= new TemplateController());
}
