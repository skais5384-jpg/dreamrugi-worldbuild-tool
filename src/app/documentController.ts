import type { TemplateController } from "./controller";
import {
  NAVIGATION_DEFAULT,
  NAVIGATION_MIN,
  NAVIGATION_MAX,
} from "./navigationSizing";
import type {
  Creation,
  CreationBody,
  DocumentList,
  DocumentRead,
  DocumentReferences,
  DocumentReplacePreview,
  DocumentRequest,
  DocumentResponse,
  DocumentSearch,
  DocumentValidationIssues,
  LayoutEdit,
  ReplaceScopes,
} from "../bridge/documents";
import type { GroupValue, ResultDto, Value } from "../bridge/types";
import type { RecoveryRow } from "../bridge/workspace";
import { safeFailure } from "./operations";
import { BridgeFailure } from "../bridge/client";
import { text } from "../strings";
import { DocumentEdits, type EditEntry } from "./documentEdits";
import { cellValue } from "./groups";
import type { SvnStatusEntry } from "./svnClient";

interface ReferenceAddress {
  source: string;
  target: string;
  field: string;
  instance: string | null;
  connection: string;
}
interface ReferenceFocus {
  document: string;
  group: string | null;
  field: string;
  instance: string | null;
  request: number;
}

function containsConnection(
  value: Value | null | undefined,
  connection: string,
  target: string,
) {
  return (
    value?.kind === "relation" &&
    value.links.some(
      (link) => link.id === connection && link.document === target,
    )
  );
}

/** 최신 canonical read와 열린 초안 양쪽에서 같은 root/group 연결 주소가 살아 있어야 한다. */
function resolveReferenceAddress(
  read: DocumentRead,
  reference: ReferenceAddress,
  entry?: EditEntry,
): Omit<ReferenceFocus, "document" | "request"> | null {
  const matches: Omit<ReferenceFocus, "document" | "request">[] = [];
  if (!reference.instance) {
    const definition = read.template.fields.find(
      (field) =>
        field.id === reference.field &&
        field.lifecycle === "Active" &&
        field.kind === "Relation",
    );
    const canonical = read.fields.find(
      (field) => field.id === reference.field,
    )?.value;
    const draft = entry?.body.fields.find(
      (field) => field.field === reference.field,
    )?.value;
    const current =
      draft?.intent === "set"
        ? draft.value
        : draft?.intent === "unset"
          ? null
          : canonical;
    if (
      definition &&
      containsConnection(canonical, reference.connection, reference.target) &&
      containsConnection(current, reference.connection, reference.target)
    )
      matches.push({ group: null, field: reference.field, instance: null });
  } else {
    for (const definition of read.template.fields) {
      if (definition.lifecycle !== "Active" || definition.kind !== "Group")
        continue;
      const child = definition.members?.find(
        (field) =>
          field.id === reference.field &&
          field.lifecycle === "Active" &&
          field.kind === "Relation",
      );
      const canonical = read.fields.find(
        (field) => field.id === definition.id,
      )?.value;
      if (!child || canonical?.kind !== "group") continue;
      const canonicalCard = canonical.instances.find(
        (instance) => instance.id === reference.instance,
      );
      const canonicalCell = canonicalCard
        ? cellValue(canonicalCard, reference.field, canonical)
        : null;
      if (
        !canonicalCell ||
        canonicalCell.intent !== "set" ||
        !containsConnection(
          canonicalCell.value,
          reference.connection,
          reference.target,
        )
      )
        continue;
      const draft = entry?.body.fields.find(
        (field) => field.field === definition.id,
      )?.value;
      const currentGroup: GroupValue | null =
        draft?.intent === "set" && draft.value.kind === "group"
          ? draft.value
          : draft?.intent === "unset"
            ? null
            : canonical;
      const currentCard = currentGroup?.instances.find(
        (instance) => instance.id === reference.instance,
      );
      const current = currentCard
        ? cellValue(currentCard, reference.field, canonical)
        : null;
      if (
        current?.intent === "set" &&
        containsConnection(
          current.value,
          reference.connection,
          reference.target,
        )
      )
        matches.push({
          group: definition.id,
          field: reference.field,
          instance: reference.instance,
        });
    }
  }
  return matches.length === 1 ? matches[0] : null;
}

interface UiState {
  tabs: string[];
  active: string | null;
  collapsed: string[];
  scroll: number;
  navigationWidth: number;
  navigationCollapsed: boolean;
  glossaryWidth: number;
  glossaryCollapsed: boolean;
  glossaryTemplate: string | null;
}
interface State {
  previewClosing: boolean;
  editors: Record<string, EditEntry>;
  svnLocalObservation: {
    document: string;
    sequence: number;
    entry?: SvnStatusEntry;
  } | null;
  editPrompt: string | null;
  list: DocumentList | null;
  validationIssues: DocumentValidationIssues;
  read: DocumentRead | null;
  draft: Creation | null;
  restoredOwner: string | null;
  ui: UiState;
  busy: boolean;
  busyPhase: "locking" | "opening" | null;
  navigationBusy: boolean;
  error: string | null;
  message: string | null;
  messageIntent: "success" | "warning";
  uiError: boolean;
  prompt: boolean;
  progress: { files: string; phase: number; requested: boolean } | null;
  searchQuery: string;
  searchTemplate: string | null;
  search: DocumentSearch | null;
  searchBusy: boolean;
  searchError: string | null;
  replace: {
    initialized: boolean;
    find: string;
    replacement: string;
    template: string | null;
    scopes: ReplaceScopes;
    caseSensitive: boolean;
    wholeWord: boolean;
    preview: DocumentReplacePreview | null;
    busy: boolean;
    cancelling: boolean;
    error: string | null;
  };
  references: DocumentReferences | null;
  referencesBusy: boolean;
  referencesError: string | null;
  referenceFocus: ReferenceFocus | null;
}
export const DOCUMENT_NAVIGATION_MIN = NAVIGATION_MIN;
export const DOCUMENT_NAVIGATION_MAX = NAVIGATION_MAX;
export const DOCUMENT_GLOSSARY_MIN = 220;
export const DOCUMENT_GLOSSARY_MAX = 380;
const defaultNavigationWidth = () => NAVIGATION_DEFAULT;
const emptyUi = (): UiState => ({
  tabs: [],
  active: null,
  collapsed: [],
  scroll: 0,
  navigationWidth: defaultNavigationWidth(),
  navigationCollapsed: false,
  glossaryWidth: 280,
  glossaryCollapsed: true,
  glossaryTemplate: null,
});
const emptyReplace = (): State["replace"] => ({
  initialized: false,
  find: "",
  replacement: "",
  template: null,
  scopes: {
    title: true,
    body: true,
    englishName: true,
    glossarySummary: true,
  },
  caseSensitive: false,
  wholeWord: false,
  preview: null,
  busy: false,
  cancelling: false,
  error: null,
});
/** 화면 교체와 독립적인 초안/작업 owner. 늦은 응답은 제출 이후 입력을 덮지 않는다. */
export class DocumentController {
  private state: State = {
    previewClosing: false,
    editors: {},
    svnLocalObservation: null,
    editPrompt: null,
    list: null,
    validationIssues: [],
    read: null,
    draft: null,
    restoredOwner: null,
    ui: emptyUi(),
    busy: false,
    busyPhase: null,
    navigationBusy: false,
    error: null,
    message: null,
    messageIntent: "success",
    uiError: false,
    prompt: false,
    progress: null,
    searchQuery: "",
    searchTemplate: null,
    search: null,
    searchBusy: false,
    searchError: null,
    replace: emptyReplace(),
    references: null,
    referencesBusy: false,
    referencesError: null,
    referenceFocus: null,
  };
  private listeners = new Set<() => void>();
  private project: string | null = null;
  private projectGeneration = -1;
  private listRequest = 0;
  private ownerRevision = 0;
  private reloadAfterBusy = false;
  private previewEpoch = 0;
  private searchRequest = 0;
  private replaceRequest = 0;
  private replaceOperations = new Set<string>();
  private replaceAllocations = new Set<Promise<void>>();
  private replaceTasks = new Set<Promise<void>>();
  private replaceCleanup: Promise<void> = Promise.resolve();
  private replaceCapabilityPossible = false;
  private searchOperations = new Set<string>();
  private searchAllocations = new Set<Promise<void>>();
  private referenceOperations = new Set<string>();
  private referenceAllocations = new Set<Promise<void>>();
  private searchPauses = 0;
  private searchQueued = false;
  private searchForce = false;
  private selectionRequest = 0;
  private referencesRequest = 0;
  private referenceEditRequest = 0;
  private openingSearch: Promise<void> | null = null;
  suspendPreviews() {
    if (this.state.previewClosing) return;
    ++this.previewEpoch;
    ++this.selectionRequest;
    this.publish({ previewClosing: true });
  }
  resumePreviews() {
    if (!this.state.previewClosing) return;
    ++this.previewEpoch;
    this.publish({ previewClosing: false });
    this.resumeSearch();
  }
  private closeAction: (() => void) | null = null;
  private cancelAction: (() => void) | null = null;
  private operation: string | null = null;
  private pdfOperation: string | null = null;
  async pollProgress(cancel = false) {
    if (!this.operation) return;
    try {
      const progress = await this.shell.client.documentProgress(
        this.operation,
        cancel,
      );
      this.publish({ progress });
    } catch {
      if (cancel) this.publish({ error: text("documents.cancelUnknown") });
    }
  }
  async cancelPdfExport() {
    const operation = this.pdfOperation;
    if (!operation) return false;
    try {
      await this.shell.client.documentProgress(operation, true);
      return true;
    } catch {
      return false;
    }
  }
  readonly edits: DocumentEdits;
  constructor(readonly shell: TemplateController) {
    this.edits = new DocumentEdits(
      (r) => this.work(r),
      () => this.publish({ editors: this.edits.entries }),
      (r, sourceChanged) => {
        if (sourceChanged)
          this.shell.invalidateDocumentInspection([r.document]);
        const list = this.state.list;
        this.publish({
          list: list
            ? {
                ...list,
                documents: list.documents.map((d) =>
                  d.id === r.document
                    ? {
                        ...d,
                        name: r.read.name,
                        englishName: r.read.englishName ?? "",
                        glossarySummary: r.read.glossarySummary ?? "",
                        glossaryExcluded: r.read.glossaryExcluded ?? false,
                      }
                    : d,
                ),
              }
            : null,
          ...(this.state.read?.id === r.document ? { read: r.read } : {}),
        });
        if (sourceChanged) this.observeSvnLocal(r.document);
        void this.refreshSearch(false);
        void this.loadReferences(r.document);
      },
      () => !!this.shell.snapshot().project?.collaborative,
    );
  }
  hasOwners() {
    return !!this.state.draft || this.edits.hasOwners();
  }
  observeSvnLocal(document: string, entry?: SvnStatusEntry) {
    if (!this.shell.snapshot().project?.collaborative) return;
    this.publish({
      svnLocalObservation: {
        document,
        sequence: (this.state.svnLocalObservation?.sequence ?? 0) + 1,
        entry,
      },
    });
  }
  async beginEdit(id: string) {
    let opened = false;
    await this.action(
      async () => {
        await this.edits.begin(id);
        opened = true;
      },
      false,
      "locking",
    );
    if (opened) this.observeSvnLocal(id);
    return opened;
  }
  acknowledgeBlockedEdit() {
    this.publish({ error: null });
  }
  async endEdit(id: string, closeTab = false) {
    if (await this.edits.close(id)) {
      this.observeSvnLocal(id);
      if (closeTab) await this.removeTab(id);
    } else {
      this.closeAction = closeTab ? () => void this.removeTab(id) : null;
      this.publish({ editPrompt: id });
    }
  }
  cancelEditClose() {
    this.closeAction = null;
    this.cancelAction?.();
    this.cancelAction = null;
    this.publish({ editPrompt: null });
  }
  async depositEditClose() {
    const id = this.state.editPrompt;
    if (!id) return;
    if (this.cancelAction) this.suspendPreviews();
    if (!(await this.edits.close(id, true))) {
      this.resumePreviews();
      return;
    }
    this.observeSvnLocal(id);
    const action = this.closeAction;
    this.closeAction = null;
    this.cancelAction = null;
    this.publish({ editPrompt: null });
    action?.();
  }
  snapshot = () => this.state;
  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };
  private publish(p: Partial<State>) {
    if (p.draft !== undefined || p.editors !== undefined) ++this.ownerRevision;
    this.state = { ...this.state, ...p };
    this.listeners.forEach((l) => l());
  }
  private key() {
    return "worldbuild.document-ui." + this.state.list?.fingerprint;
  }
  private persist(ui: UiState) {
    this.publish({ ui });
    try {
      localStorage.setItem(this.key(), JSON.stringify(ui));
      this.publish({ uiError: false });
    } catch {
      this.publish({ uiError: true });
    }
  }
  async media(request: DocumentRequest) {
    const project = this.project;
    const preview = ["asset_read", "asset_chunk"].includes(request.action);
    const epoch = this.previewEpoch;
    const current = () => {
      const app = this.shell.snapshot();
      return (
        project !== null &&
        this.project === project &&
        this.projectGeneration === this.shell.projectGeneration() &&
        (!preview ||
          (!this.state.previewClosing && epoch === this.previewEpoch)) &&
        app.projectId === project &&
        app.project?.project === project &&
        app.project.status === "Ready" &&
        app.project.runtime === "Ready" &&
        !app.closing
      );
    };
    // 재열기 첫 렌더에는 이전 문서가 잠시 남을 수 있다. 이전 project ID로
    // 예약부터 만들지 않도록 읽기 요청도 현재 shell 소유권과 대조한다.
    if (!current()) throw new Error(text("media.changed"));
    const r = await this.work(request, preview ? current : undefined);
    if (!current()) throw new Error(text("media.changed"));
    return r;
  }
  async pdfInspect(document: string) {
    const project = this.project;
    const generation = this.projectGeneration;
    const result = await this.work({ action: "pdf_inspect", document });
    if (
      project !== this.project ||
      generation !== this.projectGeneration ||
      this.shell.snapshot().projectId !== project
    )
      throw new Error(text("pdf.projectChanged"));
    if (result.kind !== "pdf_inspect" || result.document !== document)
      throw new Error(text("pdf.failed"));
    return result;
  }
  async pdfExport(
    document: string,
    source: string,
    destination: string,
    allowMissingImages: boolean,
  ) {
    const project = this.project;
    const generation = this.projectGeneration;
    const result = await this.work({
      action: "pdf_export",
      document,
      source,
      destination,
      allow_missing_images: allowMissingImages,
    });
    if (
      project !== this.project ||
      generation !== this.projectGeneration ||
      this.shell.snapshot().projectId !== project
    )
      throw new Error(text("pdf.projectChanged"));
    if (result.kind !== "pdf_export") throw new Error(text("pdf.failed"));
    return result;
  }
  async importAsset(
    field: string,
    image: boolean,
    document?: string,
    cell?: import("./groups").CellAddress,
  ): Promise<string | null> {
    if (this.state.busy) return null;
    let result: string | null = null;
    let failure: unknown;
    await this.action(async () => {
      try {
        const project = this.project;
        const e = document ? this.edits.entries[document] : undefined;
        const d = this.state.draft;
        const owner = e?.status.owner ?? d?.owner;
        const generation = e?.generation ?? d?.generation;
        if (!owner || !generation || (document && (!e || e.busy))) return;
        // 저장 응답 뒤 화면의 변경 목록은 비워진다. 이미 확인된 세대에
        // 이 정리된 본문을 재전송하면 같은 세대의 다른 입력으로 거절된다.
        if (!e || e.generation !== e.status.generation)
          await this.work(
            e
              ? {
                  action: "edit_draft",
                  owner,
                  generation,
                  body: e.body,
                  save: false,
                }
              : {
                  action: "draft",
                  owner,
                  generation,
                  body: d!.body,
                  save: false,
                },
          );
        const r = await this.media({
          action: "asset_import",
          ...(cell ? { cell } : {}),
          owner,
          generation,
          field,
          image,
        });
        const current = document
          ? this.edits.entries[document]
          : this.state.draft;
        const currentOwner =
          current &&
          ("status" in current ? current.status.owner : current.owner);
        if (
          project !== this.project ||
          currentOwner !== owner ||
          current?.generation !== generation
        )
          throw new Error(text("media.changed"));
        if (r.kind === "asset_error")
          throw new Error(text("media.importFailed"));
        if (r.kind === "asset") result = r.metadata.id;
      } catch (error) {
        failure = error;
        throw error;
      }
    });
    if (failure) throw failure;
    return result;
  }
  private async work(
    request: DocumentRequest,
    previewCurrent?: () => boolean,
  ): Promise<DocumentResponse | ResultDto> {
    const interrupts = ![
      "search",
      "search_read",
      "references",
      "asset_read",
      "asset_chunk",
      "asset_open",
      "url_open",
    ].includes(request.action);
    if (!interrupts) return this.dispatch(request, previewCurrent);
    return this.interruptSearchFor(() =>
      this.dispatch(request, previewCurrent),
    );
  }
  async interruptSearchFor<T>(run: () => Promise<T>): Promise<T> {
    ++this.searchPauses;
    ++this.searchRequest;
    this.publish({ searchBusy: false });
    try {
      await this.cancelDerivedOperations();
      return await run();
    } finally {
      --this.searchPauses;
      this.resumeSearch();
    }
  }
  private resumeSearch() {
    if (!this.searchPauses && !this.state.previewClosing && this.searchQueued) {
      this.searchQueued = false;
      const force = this.searchForce;
      this.searchForce = false;
      void this.refreshSearch(force);
    }
  }
  /** 예약 응답 대기까지 소유한다. 닫기/쓰기보다 먼저 큐 밖 취소 신호를 전달한다. */
  async stopSearch(discardReplacement = false) {
    ++this.searchRequest;
    ++this.selectionRequest;
    this.searchQueued = false;
    this.publish({ searchBusy: false });
    await this.cancelDerivedOperations();
    if (discardReplacement) await this.invalidateReplacement(true);
  }
  private async dispatch(
    request: DocumentRequest,
    previewCurrent?: () => boolean,
  ): Promise<DocumentResponse | ResultDto> {
    const project = this.project;
    if (!project) throw new BridgeFailure("protocol");
    const input = { kind: "document_workspace" as const, project, request };
    if (
      new TextEncoder().encode(
        JSON.stringify({
          action: "submit",
          operation: crypto.randomUUID(),
          input,
        }),
      ).length >
      1024 * 1024
    )
      throw new BridgeFailure("boundary", undefined, {
        code: "full",
        nextAction: "",
      });
    let allocatedOperation: string | null = null;
    let allocated!: () => void;
    const allocation = new Promise<void>((resolve) => {
      allocated = resolve;
    });
    const trackedAllocations =
      request.action === "search"
        ? this.searchAllocations
        : request.action === "references"
          ? this.referenceAllocations
          : ["replace_preview", "replace_page", "replace_apply"].includes(
                request.action,
              )
            ? this.replaceAllocations
            : null;
    const trackedOperations =
      request.action === "search"
        ? this.searchOperations
        : request.action === "references"
          ? this.referenceOperations
          : ["replace_preview", "replace_page", "replace_apply"].includes(
                request.action,
              )
            ? this.replaceOperations
            : null;
    trackedAllocations?.add(allocation);
    const { result } = await this.shell.operations
      .run(
        input,
        text("documents.operation"),
        "ordinary",
        undefined,
        (id) => {
          allocatedOperation = id;
          if (request.action === "pdf_export") this.pdfOperation = id;
          if (trackedOperations && trackedAllocations) {
            trackedOperations.add(id);
            allocated();
            trackedAllocations.delete(allocation);
          } else if (
            !request.action.startsWith("edit_") &&
            ![
              "search",
              "asset_read",
              "asset_chunk",
              "asset_open",
              "url_open",
            ].includes(request.action)
          )
            this.operation = id;
        },
        previewCurrent,
      )
      .finally(() => {
        if (this.pdfOperation === allocatedOperation) this.pdfOperation = null;
        allocated();
        trackedAllocations?.delete(allocation);
        if (trackedOperations && allocatedOperation)
          trackedOperations.delete(allocatedOperation);
      });
    if (
      !request.action.startsWith("edit_") &&
      ![
        "search",
        "references",
        "asset_read",
        "asset_chunk",
        "asset_open",
        "url_open",
      ].includes(request.action)
    )
      this.operation = null;
    if (result.kind === "rejected")
      throw new BridgeFailure("boundary", undefined, result.error);
    const response =
      result.kind === "document_workspace" ? result.value : result;
    if (response.kind === "released" || response.kind === "asset")
      this.shell.resumeDocumentInspection(project);
    return response;
  }
  private async action(
    run: () => Promise<void>,
    navigation = false,
    busyPhase: State["busyPhase"] = null,
  ) {
    if (this.state.busy) return;
    this.publish({
      busy: true,
      busyPhase,
      navigationBusy: navigation,
      error: null,
      progress: null,
    });
    try {
      await run();
    } catch (e) {
      this.publish({
        error:
          e instanceof BridgeFailure && e.boundary?.code === "cancelled"
            ? text("documents.cancelled")
            : safeFailure(e),
      });
    } finally {
      this.publish({ busy: false, busyPhase: null, navigationBusy: false });
      if (this.reloadAfterBusy) {
        this.reloadAfterBusy = false;
        void this.load();
      }
    }
  }
  async load() {
    if (this.state.busy) {
      this.reloadAfterBusy = true;
      return;
    }
    await this.action(
      async () => {
        const project = this.shell.snapshot().projectId;
        if (!project) return;
        const generation = this.shell.projectGeneration();
        const changed =
          this.project !== project || this.projectGeneration !== generation;
        if (changed) {
          if (this.hasOwners()) throw new BridgeFailure("protocol");
          await this.cancelDerivedOperations();
          await this.invalidateReplacement(true);
          this.project = project;
          this.projectGeneration = generation;
          ++this.listRequest;
          ++this.searchRequest;
          ++this.previewEpoch;
          this.publish({ previewClosing: false });
          this.publish({
            ui: emptyUi(),
            list: null,
            validationIssues: [],
            read: null,
            searchQuery: "",
            searchTemplate: null,
            search: null,
            searchBusy: false,
            searchError: null,
            replace: emptyReplace(),
            references: null,
            referencesBusy: false,
            referencesError: null,
            referenceFocus: null,
          });
        }
        await this.refreshList(changed);
      },
      false,
      "opening",
    );
  }
  private async refreshList(
    restoreUi = false,
    refreshSearch = true,
    preserveCommitted = false,
  ) {
    let restoreFailed = false;
    const project = this.project;
    const generation = this.projectGeneration;
    const request = ++this.listRequest;
    const result = await this.work({ action: "list", refreshSearch });
    if (result.kind !== "list") throw new BridgeFailure("protocol");
    if (
      project !== this.project ||
      project !== this.shell.snapshot().projectId ||
      generation !== this.projectGeneration ||
      generation !== this.shell.projectGeneration() ||
      request !== this.listRequest
    )
      return false;
    // A failed inventory refresh cannot replace the just-committed read/tab
    // with a partial response or recategorize a successful creation as lost.
    if (
      preserveCommitted &&
      (result.problem ||
        result.issueStatus !== "complete" ||
        result.unverifiedDocuments?.length)
    )
      return false;
    this.publish({ list: result, validationIssues: result.issues ?? [] });
    if (restoreUi) {
      try {
        const raw = localStorage.getItem(this.key());
        if (raw) {
          const value = restoredUi(JSON.parse(raw));
          if (!value) throw new Error();
          this.publish({ ui: value });
        }
      } catch {
        restoreFailed = true;
      }
    }
    const activeIds = new Set(
      result.documents
        .filter((d) => result.layout.nodes[d.id]?.state !== "trashed")
        .map((d) => d.id),
    );
    const tabs = this.state.ui.tabs.filter(
      (id) => activeIds.has(id) || !!this.edits.entries[id],
    );
    const active =
      this.state.ui.active && tabs.includes(this.state.ui.active)
        ? this.state.ui.active
        : (tabs[0] ?? null);
    this.publish({ list: result });
    this.persist({ ...this.state.ui, tabs, active });
    if (restoreFailed) this.publish({ uiError: true });
    if (active) {
      await this.read(active);
      void this.loadReferences(active);
    } else this.publish({ read: null });
    if (this.state.searchQuery.trim() || this.state.searchTemplate)
      await this.requestSearch(refreshSearch, false);
    return true;
  }

  /** Validate only the list. Diagnostics must not reselect tabs or read a body. */
  async refreshForHealth(
    current: () => boolean = () => true,
  ): Promise<boolean> {
    if (!this.project || this.hasOwners() || this.state.busy) return false;
    const project = this.project;
    const generation = this.projectGeneration;
    const ownerRevision = this.ownerRevision;
    const request = ++this.listRequest;
    try {
      const result = await this.work({ action: "list", refreshSearch: false });
      if (result.kind !== "list") throw new BridgeFailure("protocol");
      if (
        !current() ||
        project !== this.project ||
        project !== this.shell.snapshot().projectId ||
        generation !== this.projectGeneration ||
        generation !== this.shell.projectGeneration() ||
        request !== this.listRequest ||
        ownerRevision !== this.ownerRevision ||
        this.hasOwners() ||
        this.state.busy ||
        this.state.previewClosing
      )
        return false;
      this.publish({ list: result, validationIssues: result.issues ?? [] });
      return (
        !result.problem &&
        result.issueStatus === "complete" &&
        !result.unverifiedDocuments?.length
      );
    } catch {
      return false;
    }
  }

  searchDocuments(query: string, template: string | null) {
    ++this.selectionRequest;
    this.publish({ searchQuery: query, searchTemplate: template });
    void this.requestSearch(false, false);
  }
  startReplaceFromSearch() {
    if (this.state.replace.initialized) return;
    this.publish({
      replace: {
        ...this.state.replace,
        initialized: true,
        find: this.state.searchQuery,
        template: this.state.searchTemplate,
      },
    });
  }
  updateReplace(
    change: Partial<
      Pick<
        State["replace"],
        | "find"
        | "replacement"
        | "template"
        | "scopes"
        | "caseSensitive"
        | "wholeWord"
      >
    >,
  ) {
    const request = ++this.replaceRequest;
    const project = this.project;
    const invalidate =
      this.replaceCapabilityPossible ||
      this.replaceTasks.size > 0 ||
      this.replaceAllocations.size > 0 ||
      this.replaceOperations.size > 0 ||
      this.state.replace.cancelling;
    if (invalidate) this.replaceCapabilityPossible = false;
    this.publish({
      replace: {
        ...this.state.replace,
        ...change,
        initialized: true,
        preview: null,
        busy: !!project && invalidate,
        cancelling: !!project && invalidate,
        error: null,
      },
    });
    if (!project || !invalidate) return;
    void this.queueReplacementDiscard(project).then(
      () => {
        if (request === this.replaceRequest && project === this.project)
          this.publish({
            replace: {
              ...this.state.replace,
              busy: false,
              cancelling: false,
            },
          });
      },
      (error) => {
        if (request === this.replaceRequest && project === this.project)
          this.publish({
            replace: {
              ...this.state.replace,
              busy: false,
              cancelling: false,
              error: safeFailure(error),
            },
          });
      },
    );
  }
  cancelReplace() {
    this.updateReplace({});
  }
  async previewReplace() {
    const form = this.state.replace;
    if (form.busy || !form.find || !Object.values(form.scopes).some(Boolean))
      return;
    const project = this.project;
    if (!project) return;
    const request = ++this.replaceRequest;
    this.publish({
      replace: {
        ...form,
        preview: null,
        busy: true,
        cancelling: true,
        error: null,
      },
    });
    try {
      this.replaceCapabilityPossible = true;
      this.publish({
        replace: { ...this.state.replace, cancelling: false },
      });
      const result = await this.trackReplacement(
        this.work(
          {
            action: "replace_preview",
            find: form.find,
            replacement: form.replacement,
            template: form.template,
            scopes: form.scopes,
            caseSensitive: form.caseSensitive,
            wholeWord: form.wholeWord,
            offset: 0,
            limit: 100,
          },
          () => request === this.replaceRequest && project === this.project,
        ),
      );
      if (request !== this.replaceRequest || project !== this.project) return;
      if (result.kind !== "replace_preview")
        throw new BridgeFailure("protocol");
      this.publish({
        replace: {
          ...this.state.replace,
          preview: result,
          busy: false,
          cancelling: false,
        },
      });
    } catch (error) {
      if (request !== this.replaceRequest || project !== this.project) return;
      this.replaceCapabilityPossible = false;
      try {
        await this.queueReplacementDiscard(project);
      } catch {
        // 아래의 원래 실패가 사용자에게 더 직접적인 재시도 단서를 제공한다.
      }
      if (request !== this.replaceRequest || project !== this.project) return;
      this.publish({
        replace: {
          ...this.state.replace,
          preview: null,
          busy: false,
          cancelling: false,
          error: safeFailure(error),
        },
      });
    }
  }
  async moreReplaceResults() {
    const form = this.state.replace;
    const previous = form.preview;
    if (!previous?.hasMore || form.busy) return;
    const project = this.project;
    const request = ++this.replaceRequest;
    this.publish({ replace: { ...form, busy: true, error: null } });
    try {
      const result = await this.trackReplacement(
        this.work(
          {
            action: "replace_page",
            preview: previous.preview,
            offset: previous.changes.length,
            limit: 100,
          },
          () => request === this.replaceRequest && project === this.project,
        ),
      );
      if (request !== this.replaceRequest || project !== this.project) return;
      if (
        result.kind !== "replace_preview" ||
        result.preview !== previous.preview
      )
        throw new BridgeFailure("protocol");
      this.publish({
        replace: {
          ...this.state.replace,
          preview: {
            ...result,
            offset: 0,
            changes: [...previous.changes, ...result.changes],
          },
          busy: false,
        },
      });
    } catch (error) {
      if (request !== this.replaceRequest || project !== this.project) return;
      this.publish({
        replace: {
          ...this.state.replace,
          busy: false,
          cancelling: false,
          error: safeFailure(error),
        },
      });
    }
  }
  async applyReplace() {
    const preview = this.state.replace.preview;
    if (
      !preview ||
      !preview.totalChanges ||
      preview.blockers.length ||
      this.state.replace.busy
    )
      return;
    const project = this.project;
    if (!project) return;
    const request = ++this.replaceRequest;
    await this.action(async () => {
      this.publish({
        replace: {
          ...this.state.replace,
          preview: null,
          busy: true,
          cancelling: false,
          error: null,
        },
      });
      let result: DocumentResponse | ResultDto;
      try {
        result = await this.trackReplacement(
          this.work(
            { action: "replace_apply", preview: preview.preview },
            () => request === this.replaceRequest && project === this.project,
          ),
        );
      } finally {
        this.replaceCapabilityPossible = false;
        await this.queueReplacementDiscard(project);
      }
      if (result.kind !== "write") throw new BridgeFailure("protocol");
      if (result.disk !== "committed") {
        this.publish({
          replace: {
            ...this.state.replace,
            busy: false,
            cancelling: false,
            error: text(
              result.disk === "uncertain"
                ? "documents.replaceUncertain"
                : "documents.replaceNotApplied",
            ),
          },
        });
        return;
      }
      this.publish({
        message: text("documents.replaceCommitted"),
        replace: {
          ...this.state.replace,
          busy: false,
          cancelling: false,
          error: null,
        },
      });
      this.shell.invalidateDocumentInspection(
        this.state.list?.documents.map((document) => document.id) ?? [],
      );
      try {
        await this.refreshList(false, true);
      } catch (error) {
        this.publish({
          error: `${text("documents.replaceRefreshRequired")} ${safeFailure(error)}`,
        });
      }
    });
    if (this.state.replace.busy)
      this.publish({
        replace: {
          ...this.state.replace,
          busy: false,
          cancelling: false,
          preview: null,
        },
      });
  }
  clearSearch() {
    ++this.selectionRequest;
    this.searchQueued = false;
    void this.cancelSearchOperation().catch((error) =>
      this.publish({ searchError: safeFailure(error) }),
    );
    ++this.searchRequest;
    this.publish({
      searchQuery: "",
      searchTemplate: null,
      search: null,
      searchBusy: false,
      searchError: null,
    });
  }
  async refreshSearch(force = true) {
    if (!this.state.searchQuery.trim() && !this.state.searchTemplate) return;
    await this.requestSearch(force, false);
  }
  async moreSearchResults() {
    if (!this.state.search?.hasMore || this.state.searchBusy) return;
    await this.requestSearch(false, true);
  }
  private async requestSearch(refresh: boolean, append: boolean) {
    if (this.searchPauses || this.state.previewClosing) {
      this.searchQueued = true;
      this.searchForce ||= refresh;
      return;
    }
    const query = this.state.searchQuery;
    const template = this.state.searchTemplate;
    if (!query.trim() && !template) {
      ++this.searchRequest;
      this.publish({
        search: null,
        searchBusy: false,
        searchError: null,
      });
      return;
    }
    const project = this.project;
    if (!project) return;
    const request = ++this.searchRequest;
    const previous = this.state.search;
    const offset = append ? (previous?.results.length ?? 0) : 0;
    this.publish({ searchBusy: true, searchError: null });
    try {
      await this.cancelSearchOperation();
      if (
        request !== this.searchRequest ||
        this.state.previewClosing ||
        this.searchPauses
      )
        return;
      const result = await this.work({
        action: "search",
        query,
        template,
        offset,
        limit: 100,
        refresh,
      });
      if (
        request !== this.searchRequest ||
        project !== this.project ||
        query !== this.state.searchQuery ||
        template !== this.state.searchTemplate
      )
        return;
      if (result.kind !== "search") throw new BridgeFailure("protocol");
      if (append && previous && previous.generation !== result.generation) {
        await this.requestSearch(false, false);
        return;
      }
      this.publish({
        search:
          append && previous
            ? {
                ...result,
                offset: 0,
                results: [...previous.results, ...result.results],
              }
            : result,
        searchBusy: false,
      });
    } catch (error) {
      if (request !== this.searchRequest || project !== this.project) return;
      this.publish({ searchBusy: false, searchError: safeFailure(error) });
    }
  }
  private async cancelSearchOperation() {
    await this.cancelTrackedOperations(
      this.searchAllocations,
      this.searchOperations,
    );
  }
  private async cancelReferenceOperation() {
    await this.cancelTrackedOperations(
      this.referenceAllocations,
      this.referenceOperations,
    );
  }
  private trackReplacement<T>(task: Promise<T>): Promise<T> {
    const settled = task.then(
      () => undefined,
      () => undefined,
    );
    this.replaceTasks.add(settled);
    void settled.then(() => this.replaceTasks.delete(settled));
    return task;
  }
  private async cancelReplacementOperations() {
    while (
      this.replaceTasks.size ||
      this.replaceAllocations.size ||
      this.replaceOperations.size
    ) {
      await this.cancelTrackedOperations(
        this.replaceAllocations,
        this.replaceOperations,
      );
      await this.shell.operations.queryAll();
      if (this.replaceTasks.size)
        await Promise.race([
          Promise.all([...this.replaceTasks]),
          new Promise<void>((resolve) => setTimeout(resolve, 20)),
        ]);
    }
  }
  private queueReplacementDiscard(project: string) {
    this.replaceCapabilityPossible = false;
    const cleanup = this.replaceCleanup
      .catch(() => undefined)
      .then(async () => {
        await this.cancelReplacementOperations();
        if (project !== this.project) return;
        const result = await this.dispatch({
          action: "replace_discard",
          preview: null,
        });
        if (result.kind !== "released") throw new BridgeFailure("protocol");
      });
    this.replaceCleanup = cleanup;
    return cleanup;
  }
  private async invalidateReplacement(showState: boolean) {
    const project = this.project;
    const invalidate =
      this.replaceCapabilityPossible ||
      this.replaceTasks.size > 0 ||
      this.replaceAllocations.size > 0 ||
      this.replaceOperations.size > 0 ||
      this.state.replace.cancelling;
    ++this.replaceRequest;
    this.replaceCapabilityPossible = false;
    if (!project || !invalidate) return;
    if (showState)
      this.publish({
        replace: {
          ...this.state.replace,
          preview: null,
          busy: true,
          cancelling: true,
          error: null,
        },
      });
    try {
      await this.queueReplacementDiscard(project);
    } finally {
      if (showState && project === this.project)
        this.publish({
          replace: {
            ...this.state.replace,
            preview: null,
            busy: false,
            cancelling: false,
          },
        });
    }
  }
  private async cancelDerivedOperations() {
    await Promise.all([
      this.cancelSearchOperation(),
      this.cancelReferenceOperation(),
    ]);
  }
  private async cancelTrackedOperations(
    allocations: Set<Promise<void>>,
    operations: Set<string>,
  ) {
    await Promise.all([...allocations]);
    await Promise.all(
      [...operations].map(async (operation) => {
        try {
          await this.shell.client.documentProgress(operation, true);
        } catch (error) {
          // terminal 조회/ack가 먼저 끝난 경우만 이미 해제된 취소 대상으로 인정한다.
          if (
            !(error instanceof BridgeFailure) ||
            error.boundary?.code !== "unknown_id"
          )
            throw error;
        }
      }),
    );
  }
  private async read(id: string) {
    if (this.edits.entries[id]) {
      this.publish({ read: this.edits.entries[id].status.read });
      return;
    }
    const result = await this.work({ action: "read", document: id });
    if (result.kind !== "read") throw new BridgeFailure("protocol");
    this.publish({ read: result });
  }
  async loadReferences(id: string) {
    const project = this.project;
    if (!project || this.state.previewClosing) return;
    const request = ++this.referencesRequest;
    this.publish({ referencesBusy: true, referencesError: null });
    try {
      // 검색과 역참조는 같은 파생 cache를 읽지만 서로의 UI 요청을 취소하지 않는다.
      await this.cancelReferenceOperation();
      const result = await this.dispatch({
        action: "references",
        document: id,
      });
      if (
        request !== this.referencesRequest ||
        project !== this.project ||
        this.state.ui.active !== id
      )
        return;
      if (result.kind !== "references") throw new BridgeFailure("protocol");
      this.publish({ references: result, referencesBusy: false });
    } catch (error) {
      if (request !== this.referencesRequest || project !== this.project)
        return;
      this.publish({
        references: null,
        referencesBusy: false,
        referencesError: safeFailure(error),
      });
    }
  }
  async openTrash(id: string) {
    await this.action(async () => {
      if (this.state.list?.layout.nodes[id]?.state !== "trashed") return;
      await this.read(id);
    }, true);
  }
  async open(id: string) {
    ++this.selectionRequest;
    if (this.state.ui.active !== id) this.edits.flush(this.state.ui.active);
    await this.action(async () => {
      if (
        !this.state.ui.tabs.includes(id) &&
        this.state.ui.tabs.length >= 256
      ) {
        this.publish({ error: text("documents.tabLimit") });
        return;
      }
      await this.read(id);
      this.persist({
        ...this.state.ui,
        active: id,
        tabs: this.state.ui.tabs.includes(id)
          ? this.state.ui.tabs
          : [...this.state.ui.tabs, id],
      });
      void this.loadReferences(id);
    }, true);
  }
  async openSearchResult(id: string) {
    if (this.state.busy) return;
    const selection = ++this.selectionRequest;
    const project = this.project;
    const active = this.state.ui.active;
    const owner = active ? this.edits.entries[active]?.status.owner : undefined;
    const current = () =>
      selection === this.selectionRequest &&
      project === this.project &&
      !this.state.previewClosing &&
      active === this.state.ui.active &&
      (!active || owner === this.edits.entries[active]?.status.owner);
    ++this.searchPauses;
    ++this.searchRequest;
    this.publish({ searchBusy: false });
    try {
      const opening = this.action(async () => {
        await this.cancelDerivedOperations();
        if (
          !this.state.ui.tabs.includes(id) &&
          this.state.ui.tabs.length >= 256
        ) {
          this.publish({ error: text("documents.tabLimit") });
          return;
        }
        const result = await this.work({ action: "search_read", document: id });
        if (!current()) return;
        if (result.kind === "search_unavailable") {
          this.publish({ search: null });
          this.publish({ error: text("documents.searchTargetUnavailable") });
          await this.refreshSearch(true);
          return;
        }
        if (result.kind !== "read") throw new BridgeFailure("protocol");
        // 처음부터 거절된 대상은 다른 편집을 저장하지 않는다. 유효한 전환에서만
        // 현재 owner의 최신 입력을 저장하고, 대기 중 대상이 바뀌었는지 다시 읽는다.
        if (active !== id) {
          await this.edits.flush(active);
          if (!current()) return;
        }
        const verified =
          active !== id
            ? await this.work({ action: "search_read", document: id })
            : result;
        if (!current()) return;
        if (verified.kind === "search_unavailable") {
          this.publish({
            search: null,
            error: text("documents.searchTargetUnavailable"),
          });
          await this.refreshSearch(true);
          return;
        }
        if (verified.kind !== "read") throw new BridgeFailure("protocol");
        // 이미 편집 중인 탭은 검증된 disk read로 현재 raw/caret 문맥을 덮지 않는다.
        this.publish({ read: this.edits.entries[id]?.status.read ?? verified });
        this.persist({
          ...this.state.ui,
          active: id,
          tabs: this.state.ui.tabs.includes(id)
            ? this.state.ui.tabs
            : [...this.state.ui.tabs, id],
        });
        void this.loadReferences(id);
      });
      this.openingSearch = opening;
      await opening;
    } finally {
      this.openingSearch = null;
      --this.searchPauses;
      this.resumeSearch();
    }
  }
  /** 관계/문서 링크 전환은 검색 결과 상태를 지우거나 다시 게시하지 않는다. */
  async openReferenceTarget(id: string) {
    if (this.state.busy) return;
    const selection = ++this.selectionRequest;
    const project = this.project;
    const active = this.state.ui.active;
    const owner = active ? this.edits.entries[active]?.status.owner : undefined;
    if (this.state.searchQuery.trim() || this.state.searchTemplate)
      this.searchQueued = true;
    const current = () =>
      selection === this.selectionRequest &&
      project === this.project &&
      !this.state.previewClosing &&
      active === this.state.ui.active &&
      (!active || owner === this.edits.entries[active]?.status.owner);
    ++this.searchPauses;
    ++this.searchRequest;
    this.publish({ searchBusy: false });
    try {
      await this.action(async () => {
        await this.cancelDerivedOperations();
        if (
          !this.state.ui.tabs.includes(id) &&
          this.state.ui.tabs.length >= 256
        ) {
          this.publish({ error: text("documents.tabLimit") });
          return;
        }
        const result = await this.work({ action: "search_read", document: id });
        if (!current()) return;
        if (result.kind === "search_unavailable") {
          this.publish({ error: text("reference.targetUnavailable") });
          return;
        }
        if (result.kind !== "read") throw new BridgeFailure("protocol");
        if (active !== id) {
          await this.edits.flush(active);
          if (!current()) return;
        }
        const verified =
          active !== id
            ? await this.work({ action: "search_read", document: id })
            : result;
        if (!current()) return;
        if (verified.kind === "search_unavailable") {
          this.publish({ error: text("reference.targetUnavailable") });
          return;
        }
        if (verified.kind !== "read") throw new BridgeFailure("protocol");
        this.publish({ read: this.edits.entries[id]?.status.read ?? verified });
        this.persist({
          ...this.state.ui,
          active: id,
          tabs: this.state.ui.tabs.includes(id)
            ? this.state.ui.tabs
            : [...this.state.ui.tabs, id],
        });
        void this.loadReferences(id);
      });
    } finally {
      --this.searchPauses;
      this.resumeSearch();
    }
  }
  async editReferenceSource(reference: {
    source: string;
    target: string;
    field: string;
    instance: string | null;
    connection: string;
  }) {
    const request = ++this.referenceEditRequest;
    const project = this.project;
    const target = this.state.ui.active;
    const current = () =>
      request === this.referenceEditRequest &&
      project === this.project &&
      !this.state.previewClosing;
    const reject = () => {
      if (!current()) return;
      this.publish({
        error: text("relation.sourceChanged"),
        referenceFocus:
          this.state.referenceFocus?.request === request
            ? null
            : this.state.referenceFocus,
      });
      if (target && this.state.ui.active === target)
        void this.loadReferences(target);
      else this.publish({ references: null, referencesError: null });
    };
    const preflight = await this.work({
      action: "search_read",
      document: reference.source,
    });
    if (!current()) return;
    if (
      preflight.kind !== "read" ||
      !resolveReferenceAddress(
        preflight,
        reference,
        this.edits.entries[reference.source],
      )
    ) {
      reject();
      return;
    }
    await this.openReferenceTarget(reference.source);
    if (!current() || this.state.ui.active !== reference.source) return;
    await this.beginEdit(reference.source);
    if (!current()) return;
    const entry = this.edits.entries[reference.source];
    const address = entry
      ? resolveReferenceAddress(entry.status.read, reference, entry)
      : null;
    if (!address) {
      reject();
      return;
    }
    if (
      this.state.ui.active === reference.source &&
      this.edits.entries[reference.source] === entry
    ) {
      this.publish({
        referenceFocus: {
          document: reference.source,
          ...address,
          request,
        },
      });
    }
  }
  consumeReferenceFocus(document: string, request: number) {
    if (
      this.state.referenceFocus?.document === document &&
      this.state.referenceFocus.request === request
    )
      this.publish({ referenceFocus: null });
  }
  rejectReferenceFocus(document: string, request: number) {
    if (
      this.state.referenceFocus?.document !== document ||
      this.state.referenceFocus.request !== request
    )
      return;
    this.publish({
      referenceFocus: null,
      error: text("relation.sourceChanged"),
    });
  }
  async closeTab(id: string) {
    if (this.edits.entries[id]) {
      await this.endEdit(id, true);
      return;
    }
    await this.removeTab(id);
  }
  private async removeTab(id: string) {
    await this.action(async () => {
      const tabs = this.state.ui.tabs.filter((t) => t !== id);
      const active =
        this.state.ui.active === id ? (tabs[0] ?? null) : this.state.ui.active;
      this.persist({ ...this.state.ui, tabs, active });
      if (active) {
        await this.read(active);
        void this.loadReferences(active);
      } else this.publish({ read: null });
    });
  }
  reorderTab(id: string, delta: number) {
    const tabs = [...this.state.ui.tabs];
    const i = tabs.indexOf(id);
    if (i < 0 || i + delta < 0 || i + delta >= tabs.length) return;
    [tabs[i], tabs[i + delta]] = [tabs[i + delta], tabs[i]];
    this.persist({ ...this.state.ui, tabs });
  }
  collapse(id: string) {
    const ids = this.state.ui.collapsed;
    this.persist({
      ...this.state.ui,
      collapsed: ids.includes(id) ? ids.filter((v) => v !== id) : [...ids, id],
    });
  }
  scroll(value: number) {
    this.persist({ ...this.state.ui, scroll: value });
  }
  navigationWidth(value: number) {
    if (!Number.isFinite(value)) return;
    this.persist({
      ...this.state.ui,
      navigationWidth: Math.min(
        DOCUMENT_NAVIGATION_MAX,
        Math.max(DOCUMENT_NAVIGATION_MIN, Math.round(value)),
      ),
    });
  }
  toggleNavigation() {
    this.persist({
      ...this.state.ui,
      navigationCollapsed: !this.state.ui.navigationCollapsed,
    });
  }
  glossaryWidth(value: number) {
    if (!Number.isFinite(value)) return;
    this.persist({
      ...this.state.ui,
      glossaryWidth: Math.min(
        DOCUMENT_GLOSSARY_MAX,
        Math.max(DOCUMENT_GLOSSARY_MIN, Math.round(value)),
      ),
    });
  }
  toggleGlossary() {
    this.persist({
      ...this.state.ui,
      glossaryCollapsed: !this.state.ui.glossaryCollapsed,
    });
  }
  glossaryTemplate(template: string | null) {
    this.persist({ ...this.state.ui, glossaryTemplate: template });
  }
  async begin(template: string, parent: string | null = null) {
    await this.action(async () => {
      if (this.state.draft) return;
      const r = await this.work({
        action: "begin",
        template,
        snapshot: this.state.list?.snapshot,
      });
      if (r.kind !== "draft") throw new BridgeFailure("protocol");
      this.publish({ draft: r, restoredOwner: null });
      if (parent !== null) this.edit((b) => ({ ...b, parent }));
    });
  }
  edit(change: (b: CreationBody) => CreationBody) {
    const d = this.state.draft;
    if (
      !d ||
      this.state.prompt ||
      (d.outcome?.kind === "write" &&
        ["committed", "uncertain"].includes(d.outcome.disk))
    )
      return;
    const next = BigInt(d.generation) + 1n;
    if (next > 18446744073709551615n) {
      this.publish({ error: text("documents.invalid") });
      return;
    }
    this.publish({
      draft: {
        ...d,
        body: change(structuredClone(d.body)),
        generation: String(next),
        deposited: false,
        problem: null,
        field: null,
      },
    });
  }
  async submit(deposit = false) {
    await this.action(async () => {
      const d = this.state.draft;
      if (!d) return;
      const r = await this.work(
        deposit
          ? {
              action: "deposit",
              owner: d.owner,
              generation: d.generation,
              body: d.body,
            }
          : {
              action: "draft",
              owner: d.owner,
              generation: d.generation,
              body: d.body,
              save: true,
            },
      );
      if (r.kind !== "draft") throw new BridgeFailure("protocol");
      const latest = this.state.draft;
      if (latest?.owner !== r.owner) throw new BridgeFailure("protocol");
      this.publish({
        draft:
          latest.generation === r.generation
            ? r
            : { ...latest, outcome: r.outcome, deposited: false },
      });
      if (!deposit && r.outcome?.kind === "write") {
        this.publish({
          messageIntent: r.outcome.disk === "committed" ? "success" : "warning",
          message: text(
            r.outcome.disk === "committed"
              ? r.outcome.cleanup_failed || r.outcome.recovery_required
                ? "documents.storedCleanup"
                : "documents.stored"
              : r.outcome.disk === "uncertain"
                ? "documents.uncertain"
                : "documents.notApplied",
          ),
        });
      }
      if (
        !deposit &&
        r.outcome?.kind === "write" &&
        r.outcome.disk === "committed"
      ) {
        if (latest.generation === r.generation) {
          const id = r.outcome.artifact;
          const committed = r.commit;
          const previous = this.state.list;
          if (
            !id ||
            !committed ||
            committed.read.kind !== "read" ||
            committed.read.id !== id ||
            !previous ||
            committed.fingerprint !== previous.fingerprint ||
            previous.layout.nodes[id] ||
            committed.removedDocuments.some(
              (removed) => !!previous.layout.nodes[removed],
            )
          )
            throw new BridgeFailure("protocol");
          const changed = new Map(
            committed.changedDocuments.map((document) => [
              document.id,
              document,
            ]),
          );
          const removed = new Set(committed.removedDocuments);
          const documents = previous.documents
            .filter((document) => !removed.has(document.id))
            .map((document) => changed.get(document.id) ?? document);
          for (const document of committed.changedDocuments)
            if (!documents.some((current) => current.id === document.id))
              documents.push(document);
          documents.sort((left, right) => left.id.localeCompare(right.id));
          if (
            documents.length !== committed.documentCount ||
            !documents.some((document) => document.id === id) ||
            committed.unplaced.some(
              (unplaced) =>
                !documents.some((document) => document.id === unplaced),
            )
          )
            throw new BridgeFailure("protocol");
          const layout = structuredClone(previous.layout);
          const parent = committed.parent;
          if (parent) {
            const node = layout.nodes[parent];
            if (!node || node.state !== "active")
              throw new BridgeFailure("protocol");
            node.childOrder.push(id);
          } else layout.rootOrder.push(id);
          layout.nodes[id] = {
            parentId: parent,
            childOrder: [],
            state: "active",
            trash: null,
          };
          layout.revision = committed.layoutRevision;
          const list: DocumentList = {
            ...previous,
            snapshot: committed.snapshot,
            initial: false,
            layout,
            documents,
            unplaced: committed.unplaced,
            issueStatus: "partial",
            unverifiedDocuments: [
              ...new Set([...(previous.unverifiedDocuments ?? []), id]),
            ],
            problem: null,
          };
          await this.release(false);
          const activeIds = new Set(
            list.documents
              .filter(
                (document) =>
                  list.layout.nodes[document.id]?.state !== "trashed",
              )
              .map((document) => document.id),
          );
          const tabs = this.state.ui.tabs.filter(
            (tab) => activeIds.has(tab) || !!this.edits.entries[tab],
          );
          this.publish({ list, read: committed.read });
          this.shell.invalidateDocumentInspection([id]);
          this.persist({
            ...this.state.ui,
            active: id,
            tabs: [...tabs.filter((tab) => tab !== id), id].slice(-256),
          });
          // A committed read validates this document, but only a fresh list
          // validates the project's complete document inventory.
          try {
            await this.refreshList(false, false, true);
          } catch {
            // Keep the committed document and its unverified marker. A later
            // explicit health check or reload can retry the list validation.
          }
          await this.refreshSearch(false);
        } else this.publish({ error: text("documents.lateInput") });
      } else if (r.problem || (!deposit && r.outcome?.kind === "write"))
        this.publish({ error: text("documents.invalid") });
    });
  }
  private async release(discard: boolean) {
    const d = this.state.draft;
    if (!d) return;
    const r = await this.work({
      action: "release",
      owner: d.owner,
      generation: d.generation,
      discard,
    });
    if (r.kind !== "released") throw new BridgeFailure("protocol");
    this.publish({ draft: null, restoredOwner: null, prompt: false });
  }
  requestClose(action: () => void = () => {}, cancel: () => void = () => {}) {
    if (this.state.busy) {
      const opening = this.openingSearch;
      if (opening) {
        this.suspendPreviews();
        void this.stopSearch(true)
          .then(() => opening)
          .then(() => this.requestClose(action, cancel))
          .catch((error) => {
            this.publish({ error: safeFailure(error) });
            this.resumePreviews();
            cancel();
          });
        return false;
      }
      this.resumePreviews();
      return false;
    }
    this.suspendPreviews();
    const id = Object.keys(this.edits.entries)[0];
    if (id) {
      this.cancelAction = cancel;
      void this.edits.close(id).then((closed) => {
        if (closed) this.requestClose(action, cancel);
        else {
          this.closeAction = () => this.requestClose(action, cancel);
          this.publish({ editPrompt: id });
        }
      });
      return false;
    }
    return this.requestCreationClose(action, cancel);
  }
  requestCreationClose(action: () => void = () => {}, cancel?: () => void) {
    if (this.state.busy) return false;
    if (!this.state.draft) {
      this.cancelAction = null;
      action();
      return true;
    }
    this.closeAction = action;
    this.cancelAction = cancel ?? null;
    this.publish({ prompt: true });
    return false;
  }
  cancelClose() {
    const cancel = this.cancelAction;
    this.cancelAction = null;
    this.closeAction = null;
    this.publish({ prompt: false });
    cancel?.();
  }
  async resolveClose(choice: "deposit" | "discard") {
    const projectClose = !!this.cancelAction;
    if (projectClose) this.suspendPreviews();
    if (choice === "deposit") {
      await this.submit(true);
      if (!this.state.draft?.deposited) {
        this.resumePreviews();
        return;
      }
    }
    await this.action(async () => {
      await this.release(choice === "discard");
      const action = this.closeAction;
      this.closeAction = null;
      this.cancelAction = null;
      action?.();
    });
    if (projectClose && this.state.draft) this.resumePreviews();
  }
  async mutate(edit: LayoutEdit) {
    let stored = false;
    await this.action(async () => {
      const l = this.state.list;
      if (!l || l.problem || this.hasOwners()) return;
      const r = await this.work({
        action: "mutate",
        snapshot: l.snapshot,
        edit,
      });
      if (r.kind === "write")
        this.publish({
          messageIntent:
            r.disk === "committed" || r.disk === "no_write"
              ? "success"
              : "warning",
          message: text(
            r.disk === "committed"
              ? r.cleanup_failed || r.recovery_required
                ? "documents.storedCleanup"
                : "documents.stored"
              : r.disk === "uncertain"
                ? "documents.uncertain"
                : r.disk === "no_write"
                  ? "documents.unchanged"
                  : "documents.notApplied",
          ),
        });
      if (r.kind !== "write" || !["committed", "no_write"].includes(r.disk))
        throw new BridgeFailure("boundary", undefined, {
          code: "save_rejected",
          nextAction: "",
        });
      stored = true;
      await this.refreshList(false, false);
    });
    return stored;
  }
  async restore(row: RecoveryRow) {
    if (
      this.project !== this.shell.snapshot().projectId ||
      this.projectGeneration !== this.shell.projectGeneration()
    )
      await this.load();
    await this.action(async () => {
      if (
        this.state.draft ||
        !row.row.key ||
        !row.row.depositId ||
        !row.row.payloadDigest
      )
        return;
      const r = await this.work({
        action: "restore",
        key: row.row.key,
        deposit_id: row.row.depositId,
        digest: row.row.payloadDigest,
      });
      if (r.kind !== "draft") throw new BridgeFailure("protocol");
      this.publish({ draft: r, restoredOwner: r.owner });
    });
  }
  async restoreEdit(row: RecoveryRow) {
    if (
      this.project !== this.shell.snapshot().projectId ||
      this.projectGeneration !== this.shell.projectGeneration()
    )
      await this.load();
    await this.action(async () => {
      const id = await this.edits.restore(row);
      this.publish({ restoredOwner: this.edits.entries[id].status.owner });
      this.persist({
        ...this.state.ui,
        active: id,
        tabs: this.state.ui.tabs.includes(id)
          ? this.state.ui.tabs
          : [...this.state.ui.tabs, id],
      });
      await this.read(id);
      void this.loadReferences(id);
    });
  }
}
function restoredUi(v: unknown): UiState | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Partial<UiState>;
  const valid =
    Array.isArray(o.tabs) &&
    o.tabs.length <= 256 &&
    o.tabs.every((t) => typeof t === "string") &&
    new Set(o.tabs).size === o.tabs.length &&
    (o.active === null || typeof o.active === "string") &&
    Array.isArray(o.collapsed) &&
    o.collapsed.every((t) => typeof t === "string") &&
    typeof o.scroll === "number" &&
    Number.isFinite(o.scroll) &&
    o.scroll >= 0 &&
    (o.navigationWidth === undefined ||
      (typeof o.navigationWidth === "number" &&
        Number.isFinite(o.navigationWidth))) &&
    (o.navigationCollapsed === undefined ||
      typeof o.navigationCollapsed === "boolean") &&
    (o.glossaryWidth === undefined ||
      (typeof o.glossaryWidth === "number" &&
        Number.isFinite(o.glossaryWidth))) &&
    (o.glossaryCollapsed === undefined ||
      typeof o.glossaryCollapsed === "boolean") &&
    (o.glossaryTemplate === undefined ||
      o.glossaryTemplate === null ||
      typeof o.glossaryTemplate === "string");
  if (!valid) return null;
  return {
    tabs: o.tabs!,
    active: o.active!,
    collapsed: o.collapsed!,
    scroll: o.scroll!,
    navigationWidth: Math.min(
      DOCUMENT_NAVIGATION_MAX,
      Math.max(
        DOCUMENT_NAVIGATION_MIN,
        Math.round(o.navigationWidth ?? defaultNavigationWidth()),
      ),
    ),
    navigationCollapsed: o.navigationCollapsed ?? false,
    glossaryWidth: Math.min(
      DOCUMENT_GLOSSARY_MAX,
      Math.max(DOCUMENT_GLOSSARY_MIN, Math.round(o.glossaryWidth ?? 280)),
    ),
    glossaryCollapsed: o.glossaryCollapsed ?? true,
    glossaryTemplate: o.glossaryTemplate ?? null,
  };
}
