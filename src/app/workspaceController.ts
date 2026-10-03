import { DocumentController } from "./documentController";
import { BridgeFailure } from "../bridge/client";
import type { Id, ResultDto, Template, Work } from "../bridge/types";
import type {
  DraftContent,
  DraftStatus,
  RecoveryContent,
  RecoveryError,
  RecoveryPage,
  RecoveryRow,
  RecoverySelection,
  ReapplyIntent,
  TemplateBody,
} from "../bridge/workspace";
import { appController, type TemplateController } from "./controller";
import { safeFailure } from "./operations";
import { text } from "../strings";

export function nextGeneration(value: string): string {
  if (!/^[1-9][0-9]*$/.test(value)) throw new BridgeFailure("protocol");
  const next = BigInt(value) + 1n;
  if (next > 18446744073709551615n)
    throw new BridgeFailure("boundary", undefined, {
      code: "full",
      nextAction: "",
    });
  return next.toString();
}
export interface WholeDraft {
  project: Id;
  status: DraftStatus;
  body: TemplateBody;
  base: Template;
  generation: string;
  lastSave: string | null;
  loaded: boolean;
}
export type Destination =
  | { kind: "select" | "format"; id: Id }
  | { kind: "open_project"; root: string }
  | { kind: "restore_current"; locator: string; storage: string }
  | {
      kind:
        "new" | "browse" | "center" | "close_project" | "duplicate" | "delete";
    }
  | { kind: "close_app"; attempt: Id };
interface WorkspaceState {
  draft: WholeDraft | null;
  busy: boolean;
  pendingAction: "save" | "deposit" | null;
  center: boolean;
  page: RecoveryPage | null;
  selected: RecoverySelection | null;
  recovery: RecoveryContent | null;
  error: string | null;
  recoveryFailure?: RecoveryError | null;
  message: string;
  prompt: Destination | null;
  discard: RecoveryRow | null;
}
function requireResult<K extends ResultDto["kind"]>(
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
function parsed<T>(source: string, keys: string[]): T {
  const value: unknown = JSON.parse(source);
  if (!value || typeof value !== "object" || keys.some((k) => !(k in value)))
    throw new BridgeFailure("protocol");
  return value as T;
}

/** React와 독립적인 전체 초안 owner. 비동기 결과는 제출 세대만 확인하고 최신 원문을 보존한다. */
export class WorkspaceController {
  readonly documents: DocumentController;
  private state: WorkspaceState = {
    draft: null,
    busy: false,
    pendingAction: null,
    center: false,
    page: null,
    selected: null,
    recovery: null,
    error: null,
    message: "",
    prompt: null,
    discard: null,
  };
  private listeners = new Set<() => void>();
  private closeAttempt: Id | null = null;
  private sourceView: Id | null = null;
  constructor(readonly shell: TemplateController = appController()) {
    this.documents = new DocumentController(shell);
    shell.workspaceInspectDocuments = (current) =>
      this.documents.refreshForHealth(current);
    // shell의 읽기 갱신이 초안 session이 보유한 원본 view를 회수하지 않도록 한다.
    shell.workspaceOwnsView = (view) => this.sourceView === view;
    shell.workspaceClose = (attempt) => {
      if (attempt === this.closeAttempt) return;
      this.closeAttempt = attempt;
      if (attempt) void this.navigate({ kind: "close_app", attempt });
      else if (this.state.prompt?.kind === "close_app")
        this.publish({ prompt: null });
    };
  }
  snapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private publish(patch: Partial<WorkspaceState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }
  dirty() {
    const d = this.state.draft;
    return !!d && d.status.savedGeneration !== d.generation;
  }
  edit(change: (body: TemplateBody) => TemplateBody) {
    const d = this.state.draft;
    if (!d || !d.loaded || this.state.prompt || this.state.discard) return;
    try {
      this.publish({
        draft: {
          ...d,
          body: change(structuredClone(d.body)),
          generation: nextGeneration(d.generation),
        },
        error: null,
      });
    } catch (e: unknown) {
      this.publish({ error: safeFailure(e) });
    }
  }
  compose(composing: boolean) {
    if (this.state.draft?.body.composing !== composing)
      this.edit((b) => ({ ...b, composing }));
  }
  private async action(run: () => Promise<void>) {
    if (this.state.busy) return;
    this.publish({ busy: true, error: null, recoveryFailure: null });
    try {
      await run();
    } catch (e: unknown) {
      if (!this.state.error) this.publish({ error: safeFailure(e) });
    } finally {
      this.publish({ busy: false, pendingAction: null });
    }
  }
  private async work(input: Work, control = false): Promise<ResultDto> {
    return this.documents.interruptSearchFor(() =>
      this.dispatchWork(input, control),
    );
  }
  private async dispatchWork(input: Work, control = false): Promise<ResultDto> {
    // 원문은 controller에 둔다. 예약 전에 실제 submit wrapper의 UTF-8 크기를 확인한다.
    const bytes = new TextEncoder().encode(
      JSON.stringify({
        action: "submit",
        operation: "00000000-0000-4000-8000-000000000000",
        input,
      }),
    ).byteLength;
    if (bytes > 1024 * 1024) {
      this.publish({ error: text("whole.requestTooLarge") });
      throw new BridgeFailure("boundary", undefined, {
        code: "full",
        nextAction: "",
      });
    }
    const { result } = await this.shell.operations.run(
      input,
      text("whole.operation"),
      control ? "control" : "ordinary",
    );
    if (result.kind === "recovery_failure") {
      this.publish({
        recoveryFailure: result.failure,
        error:
          text("whole.recoveryError") +
          " (" +
          result.failure.category +
          " / " +
          result.failure.stage +
          ")",
      });
      throw new BridgeFailure("boundary", undefined, {
        code: "recovery_rejected",
        nextAction: "",
      });
    }
    if (result.kind === "rejected")
      throw new BridgeFailure("boundary", undefined, result.error);
    return result;
  }
  private async content(
    status: DraftStatus,
    project: Id,
  ): Promise<DraftContent> {
    return parsed<DraftContent>(
      await this.chunks(
        (offset) => ({
          kind: "template_draft_content",
          project,
          session: status.owner,
          snapshot: status.snapshot,
          offset,
        }),
        status.snapshot,
      ),
      ["base", "body"],
    );
  }
  private async chunks(
    input: (offset: string) => Work,
    snapshot: Id,
  ): Promise<string> {
    let offset = "0",
      source = "";
    for (let count = 0; count < 1024; count++) {
      const part = requireResult(
        await this.work(input(offset)),
        "template_draft_content",
      ).content;
      if (part.snapshot !== snapshot || part.offset !== offset)
        throw new BridgeFailure("protocol");
      source += part.text;
      if (source.length > 32 * 1024 * 1024) throw new BridgeFailure("protocol");
      if (part.next === null) return source;
      if (
        !/^[1-9][0-9]*$/.test(part.next) ||
        BigInt(part.next) <= BigInt(offset)
      )
        throw new BridgeFailure("protocol");
      offset = part.next;
    }
    throw new BridgeFailure("protocol");
  }
  private async install(status: DraftStatus, project: Id) {
    // 본문을 읽기 전에 owner를 먼저 남긴다. 후속 읽기 실패는 닫기/보관 권한을 잃지 않는다.
    const fallback: Template = {
      id: status.artifact,
      name: "",
      glossaryExcluded: false,
      revision: status.baseRevision,
      lifecycle: "Active",
      presentation: null,
      fieldOrder: [],
      fields: [],
    };
    this.publish({
      draft: {
        project,
        status,
        body: {
          name: "",
          glossaryExcluded: false,
          presentation: { intent: "keep" },
          fields: [],
          composing: false,
        },
        base: fallback,
        generation: status.generation,
        lastSave: null,
        loaded: false,
      },
      center: false,
    });
    const content = await this.content(status, project);
    this.publish({
      draft: {
        project,
        status,
        ...content,
        generation: status.generation,
        lastSave: null,
        loaded: true,
      },
    });
  }
  async navigate(target: Destination, documentsResolved = false) {
    const closing =
      target.kind === "close_project" ||
      target.kind === "open_project" ||
      target.kind === "restore_current" ||
      target.kind === "close_app";
    // owner 해제로 읽기 화면이 다시 나타나기 전에 미리보기 수명을 끝낸다.
    if (closing) this.documents.suspendPreviews();
    try {
      await this.documents.stopSearch(closing);
    } catch (error) {
      this.publish({ error: safeFailure(error) });
      if (closing) this.documents.resumePreviews();
      return;
    }
    if (
      !documentsResolved &&
      (target.kind === "close_project" ||
        target.kind === "open_project" ||
        target.kind === "restore_current" ||
        target.kind === "close_app") &&
      this.documents.hasOwners()
    ) {
      this.documents.requestClose(
        () => {
          void this.navigate(target, true);
        },
        () => {
          this.documents.resumePreviews();
          if (target.kind === "close_app")
            void this.shell.client
              .uiCloseDecision(target.attempt, false)
              .catch((e: unknown) => this.publish({ error: safeFailure(e) }));
        },
      );
      return Promise.resolve();
    }
    if (this.state.prompt?.kind === "close_app" && target.kind !== "close_app")
      return Promise.resolve();
    if (this.state.draft && (this.dirty() || this.state.busy)) {
      this.publish({ prompt: target });
      return Promise.resolve();
    }
    if (this.state.busy) {
      if (target.kind === "close_app") this.publish({ prompt: target });
      else if (closing) this.documents.resumePreviews();
      return Promise.resolve();
    }
    return this.action(async () => {
      if (this.state.draft) await this.release(false);
      await this.perform(target);
    }).finally(() => {
      if (closing) this.resumeReadyPreviews();
    });
  }
  async trashTemplates(
    ids: string[],
    expected: { project: string; generation: number },
  ) {
    const current = () =>
      this.shell.snapshot().projectId === expected.project &&
      this.shell.projectGeneration() === expected.generation;
    for (const id of [...new Set(ids)]) {
      if (!current()) break;
      await this.navigate({ kind: "select", id });
      if (!current() || this.shell.snapshot().selection?.content.id !== id)
        break;
      await this.navigate({ kind: "delete" });
      const prepared = this.shell.snapshot().templateAction;
      if (
        !current() ||
        prepared?.kind !== "delete" ||
        prepared.phase !== "confirm" ||
        prepared.source.content.id !== id
      )
        break;
      if (!current()) break;
      await this.shell.executeTemplateAction();
      const completed = this.shell.snapshot().templateAction;
      if (
        !current() ||
        completed?.generation !== prepared.generation ||
        completed.result?.kind !== "write" ||
        !["committed", "no_write"].includes(completed.result.disk)
      )
        break;
      await this.shell.navigate({ kind: "cancel" });
    }
  }
  private resumeReadyPreviews() {
    const app = this.shell.snapshot();
    if (
      !app.closing &&
      app.project?.status === "Ready" &&
      app.project.runtime === "Ready"
    )
      this.documents.resumePreviews();
  }
  private async perform(target: Destination) {
    if (target.kind === "close_app") {
      const response = await this.shell.client.uiCloseDecision(
        target.attempt,
        true,
      );
      if (response.kind !== "ui_close" || !response.closing)
        throw new BridgeFailure("boundary", undefined, {
          code: "owners_remain",
          nextAction: "",
        });
      await this.shell.checkStatus();
      return;
    }
    if (target.kind === "browse") {
      this.publish({ center: false });
      return;
    }
    if (target.kind === "center") {
      this.publish({ center: true });
      await this.list(false);
      return;
    }
    if (target.kind === "open_project") {
      await this.shell.navigate(target);
      return;
    }
    if (target.kind === "restore_current") {
      await this.shell.restoreCurrentBackup(target.locator, target.storage);
      return;
    }
    if (target.kind === "close_project") {
      await this.shell.navigate({ kind: "close_project" });
      if (!this.shell.snapshot().projectId) this.publish({ error: null });
      return;
    }
    if (target.kind === "duplicate" || target.kind === "delete") {
      await this.shell.navigate({ kind: target.kind });
      return;
    }
    const project = this.shell.snapshot().projectId;
    if (!project) throw new BridgeFailure("protocol");
    if (target.kind === "select" || target.kind === "format") {
      await this.shell.navigate({ kind: "select", id: target.id });
      const selection = this.shell.snapshot().selection;
      if (!selection || selection.content.id !== target.id)
        throw new BridgeFailure("protocol");
      if (
        target.kind === "format" ||
        (selection.content.schema ?? 5) < 5 ||
        selection.content.lifecycle !== "Active"
      ) {
        this.publish({ center: false });
        return;
      }
      const result = requireResult(
        await this.work({
          kind: "begin_template_draft",
          project,
          view: selection.view,
        }),
        "template_draft",
      );
      this.sourceView = selection.view;
      await this.install(result.status, project);
    } else {
      const result = requireResult(
        await this.work({ kind: "begin_template_draft", project, view: null }),
        "template_draft",
      );
      await this.install(result.status, project);
    }
  }
  async openProject() {
    const root = await this.shell.chooseReplacementProject();
    if (root) await this.navigate({ kind: "open_project", root });
  }
  async decide(choice: "save" | "deposit" | "discard" | "cancel") {
    const prompt = this.state.prompt;
    if (!prompt || this.state.busy) return;
    if (choice === "cancel") {
      this.resumeReadyPreviews();
      this.publish({ prompt: null });
      if (prompt.kind === "close_app")
        await this.shell.client
          .uiCloseDecision(prompt.attempt, false)
          .catch((e: unknown) => this.publish({ error: safeFailure(e) }));
      return;
    }
    const closing =
      prompt.kind === "close_project" ||
      prompt.kind === "restore_current" ||
      prompt.kind === "close_app";
    if (closing) this.documents.suspendPreviews();
    await this.action(async () => {
      if (choice === "save" || choice === "deposit") {
        await this.submit(choice);
        const d = this.state.draft;
        if (
          d &&
          (choice === "save"
            ? d.status.savedGeneration !== d.generation
            : d.status.receipt?.key.generation !== d.generation)
        )
          return;
      }
      if (this.state.draft) await this.release(choice === "discard");
      if (this.state.prompt !== prompt) return;
      this.publish({ prompt: null });
      await this.perform(prompt);
    });
    if (closing) this.resumeReadyPreviews();
  }
  save() {
    return this.action(() => this.submit("save"));
  }
  deposit() {
    return this.action(() => this.submit("deposit"));
  }
  private async submit(action: "save" | "deposit") {
    let d = this.state.draft;
    if (!d || !d.loaded) return;
    if (action === "save" && d.body.composing) {
      this.publish({ error: text("whole.composing") });
      return;
    }
    if (
      action === "save" &&
      (d.lastSave === d.generation ||
        d.status.receipt?.key.generation === d.generation)
    ) {
      d = { ...d, generation: nextGeneration(d.generation) };
      this.publish({ draft: d });
    }
    this.publish({ pendingAction: action });
    const submitted = structuredClone(d);
    if (action === "save")
      this.publish({ draft: { ...d, lastSave: d.generation } });
    const result = requireResult(
      await this.work(
        {
          kind: "template_draft",
          project: d.project,
          session: d.status.owner,
          generation: d.generation,
          body: structuredClone(d.body),
          action,
        },
        action === "deposit",
      ),
      "template_draft",
    );
    const latest = this.state.draft;
    if (!latest || latest.status.owner !== submitted.status.owner)
      throw new BridgeFailure("protocol");
    if (result.status.generation !== submitted.generation)
      throw new BridgeFailure("protocol");
    this.publish({
      draft: { ...latest, status: result.status },
      error: result.status.error
        ? safeFailure(
            new BridgeFailure("boundary", undefined, result.status.error),
          )
        : null,
      message:
        action === "deposit"
          ? text("whole.depositChecked")
          : text("whole.saveChecked"),
    });
    if (result.status.phase === "saved") {
      const content = await this.content(result.status, d.project);
      const current = this.state.draft;
      if (current?.status.owner === d.status.owner)
        this.publish({ draft: { ...current, base: content.base } });
      await this.shell.refresh();
    }
  }
  refreshSaved() {
    return this.action(async () => {
      const d = this.state.draft;
      if (!d) return;
      const result = requireResult(
        await this.work({
          kind: "refresh_template_draft",
          project: d.project,
          session: d.status.owner,
        }),
        "template_draft",
      );
      const latest = this.state.draft;
      if (latest?.status.owner !== d.status.owner)
        throw new BridgeFailure("protocol");
      this.publish({ draft: { ...latest, status: result.status } });
      const content = await this.content(result.status, d.project);
      const current = this.state.draft;
      if (current?.status.owner === d.status.owner)
        this.publish({ draft: { ...current, base: content.base } });
    });
  }
  reload() {
    return this.action(async () => {
      const d = this.state.draft;
      if (!d || d.loaded) return;
      const content = await this.content(d.status, d.project);
      this.publish({ draft: { ...d, ...content, loaded: true } });
    });
  }
  private async release(discard: boolean) {
    const d = this.state.draft;
    if (!d) return;
    // 미제출 최신 입력을 버리기로 선택한 경우에도 backend에 같은 세대가 있어야 한다.
    // 별도 동기화는 보관 작업이 아닌 메모리 소유권 교환이다.
    const result = await this.work(
      {
        kind: "release_template_draft",
        project: d.project,
        session: d.status.owner,
        generation: d.loaded ? d.generation : d.status.generation,
        body: d.loaded ? structuredClone(d.body) : null,
        discard,
      },
      true,
    );
    if (result.kind === "template_draft") {
      this.publish({ draft: { ...d, status: result.status } });
      throw new BridgeFailure(
        "boundary",
        undefined,
        result.status.error ?? { code: "release_rejected", nextAction: "" },
      );
    }
    const control = requireResult(result, "control");
    if (control.error)
      throw new BridgeFailure("boundary", undefined, control.error);
    this.publish({ draft: null });
    this.shell.resumeDocumentInspection(d.project);
    // 실제 release 응답을 받은 뒤에만 원본 view의 추가 소유권을 해제한다.
    this.sourceView = null;
    await this.shell.releaseUnusedViews();
  }
  showCenter() {
    return this.navigate({ kind: "center" });
  }
  loadPage(next = false) {
    return this.action(() => this.list(next));
  }
  private async list(next: boolean) {
    const cursor = this.state.page?.next ?? null;
    if (!next && cursor)
      await this.work({ kind: "recovery_close_cursor", cursor }, true);
    const result = requireResult(
      await this.work({ kind: "recovery_page", cursor: next ? cursor : null }),
      "recovery_page",
    );
    this.publish({ page: result.page });
  }
  inspect(row: RecoveryRow) {
    return this.action(async () => {
      const r = row.row;
      if (!r.key || !r.depositId || !r.payloadDigest) return;
      if (this.state.selected)
        await this.work(
          {
            kind: "recovery_release_selection",
            snapshot: this.state.selected.snapshot,
          },
          true,
        );
      const selection = requireResult(
        await this.work({
          kind: "recovery_read",
          key: r.key,
          deposit_id: r.depositId,
          digest: r.payloadDigest,
        }),
        "recovery_selection",
      ).selection;
      this.publish({ selected: selection, recovery: null });
      await this.readRecovery(selection);
    });
  }
  private async readRecovery(selection: RecoverySelection) {
    const recovery = parsed<RecoveryContent>(
      await this.chunks(
        (offset) => ({
          kind: "recovery_content",
          snapshot: selection.snapshot,
          offset,
        }),
        selection.snapshot,
      ),
      ["draft", "original", "current", "attempt"],
    );
    this.publish({ selected: selection, recovery });
  }
  restore(reapply: ReapplyIntent[] | null = null) {
    return this.action(async () => {
      const selected = this.state.selected,
        project = this.shell.snapshot().projectId;
      if (!selected || !project || this.state.draft) return;
      const result = await this.work({
        kind: "recovery_restore",
        project,
        snapshot: selected.snapshot,
        reapply,
      });
      if (result.kind === "recovery_selection") {
        await this.readRecovery(result.selection);
        return;
      }
      await this.install(
        requireResult(result, "template_draft").status,
        project,
      );
    });
  }
  async restoreCreation(row: RecoveryRow) {
    await this.documents.restore(row);
    if (this.documents.snapshot().draft) this.publish({ center: false });
  }
  async restoreDocument(row: RecoveryRow) {
    await this.documents.restoreEdit(row);
    if (!this.documents.snapshot().error) this.publish({ center: false });
  }
  requestDiscard(row: RecoveryRow) {
    this.publish({ discard: row });
  }
  cancelDiscard() {
    if (!this.state.busy) this.publish({ discard: null });
  }
  discardRecovery() {
    return this.action(async () => {
      const row = this.state.discard;
      if (!row?.row.key || !row.version) return;
      const result = requireResult(
        await this.work(
          { kind: "recovery_discard", key: row.row.key, version: row.version },
          true,
        ),
        "control",
      );
      if (result.error)
        throw new BridgeFailure("boundary", undefined, result.error);
      this.publish({ discard: null });
      await this.list(false);
    });
  }
  revalidate(row: RecoveryRow) {
    return this.action(async () => {
      const r = row.row;
      if (!r.key || !r.depositId || !r.payloadDigest) return;
      requireResult(
        await this.work(
          {
            kind: "recovery_revalidate",
            key: r.key,
            deposit_id: r.depositId,
            digest: r.payloadDigest,
          },
          true,
        ),
        "recovery_receipt",
      );
      this.publish({ message: text("whole.revalidated") });
    });
  }
}
const controllers = new WeakMap<TemplateController, WorkspaceController>();
export function workspaceController(shell = appController()) {
  let controller = controllers.get(shell);
  if (!controller) {
    controller = new WorkspaceController(shell);
    controllers.set(shell, controller);
  }
  return controller;
}
