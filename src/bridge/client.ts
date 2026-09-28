import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BoundaryError,
  Command,
  Hint,
  Id,
  Lane,
  Response,
  ResultDto,
  Work,
  RetainedRef,
  G6Destination,
} from "./types";

export interface Transport {
  invoke(command: "guarded", body: Uint8Array): Promise<Response>;
  listen(
    event: "guarded-state",
    callback: (hint: Hint) => void,
  ): Promise<() => void>;
}
const native: Transport = {
  invoke: (command, body) => invoke<Response>(command, body),
  listen: (event, callback) =>
    listen<Hint>(event, ({ payload }) => callback(payload)),
};
export class BridgeFailure extends Error {
  constructor(
    readonly category: "boundary" | "transport" | "protocol",
    readonly operation?: Id,
    readonly boundary?: BoundaryError,
  ) {
    super("작업 ID와 입력을 보존하고 같은 작업의 상태를 다시 확인하세요.");
  }
}
function boundary(error: unknown): BoundaryError | undefined {
  if (
    typeof error !== "object" ||
    error === null ||
    !("code" in error) ||
    !("nextAction" in error)
  )
    return undefined;
  return typeof error.code === "string" && typeof error.nextAction === "string"
    ? { code: error.code, nextAction: error.nextAction }
    : undefined;
}

// 새 편집/반환 P를 만들지 않는 후속 요청만 별도 여유를 쓴다. backend lane/권한 검사는 그대로다.
function recoveryControl(input: Work): boolean {
  switch (input.kind) {
    case "release_template_draft":
    case "recovery_close_cursor":
    case "recovery_release_selection":
    case "recovery_revalidate":
    case "recovery_discard":
      return true;
    case "template_draft":
      return input.action === "deposit";
    case "retained_handoff":
    case "abandon_retained":
    case "retire_project":
    case "recover":
    case "close":
      return true;
    case "session_control":
      return [
        "end",
        "retry_release",
        "revalidate",
        "preserve",
        "accept",
        "acknowledge_recovery",
      ].includes(input.control);
    default:
      return false;
  }
}
function sameReference(a: RetainedRef, b: RetainedRef): boolean {
  return a.id === b.id && a.generation === b.generation;
}

export class GuardedClient {
  async documentProgress(operation: Id, cancel = false) {
    const reply = await this.call({
      action: "document_progress",
      operation,
      cancel,
    });
    if (reply.kind !== "document_progress") throw new BridgeFailure("protocol");
    return reply;
  }
  // 예약 ID를 호출자에게 먼저 돌려준다. submit 응답을 잃어도 이 ID를 바꾸거나 저장을 재실행하지 않는다.
  private readonly inputs = new Map<Id, Work>();
  private readonly controls = new Map<Id, Work>();
  private readonly results = new Map<Id, ResultDto>();
  private readonly owners = new Map<Id, RetainedRef | null>();
  private readonly acknowledgements = new Map<Id, Work>();
  constructor(private readonly transport: Transport = native) {}
  private input(operation: Id): Work | undefined {
    return this.inputs.get(operation) ?? this.controls.get(operation);
  }
  private forget(operation: Id): void {
    this.inputs.delete(operation);
    this.controls.delete(operation);
    this.results.delete(operation);
    this.owners.delete(operation);
    this.acknowledgements.delete(operation);
  }
  private async call(command: Command, operation?: Id): Promise<Response> {
    try {
      return await this.transport.invoke(
        "guarded",
        new TextEncoder().encode(JSON.stringify(command)),
      );
    } catch (error: unknown) {
      const detail = boundary(error);
      throw new BridgeFailure(
        detail ? "boundary" : "transport",
        operation,
        detail,
      );
    }
  }
  async reserve(lane: Lane = "ordinary"): Promise<Id> {
    const reply = await this.call({ action: "reserve", lane });
    if (reply.kind !== "reserved") throw new BridgeFailure("protocol");
    return reply.operation;
  }
  async submit(operation: Id, input: Work): Promise<void> {
    const retained = this.input(operation);
    if (retained && JSON.stringify(retained) !== JSON.stringify(input))
      throw new BridgeFailure("protocol", operation);
    const pool = recoveryControl(input) ? this.controls : this.inputs;
    const limit = pool === this.controls ? 8 : 40;
    if (!retained && pool.size >= limit)
      throw new BridgeFailure("protocol", operation);
    // 같은 ID의 재조회/재전송은 동일 entry를 유지한다. 지연된 응답은 이 entry에만 반영한다.
    const owned = retained ?? structuredClone(input);
    if (!retained) pool.set(operation, owned);
    const reply = await this.call(
      {
        action: "submit",
        operation,
        input: owned,
      },
      operation,
    );
    if (reply.kind !== "submitted" || reply.operation !== operation)
      throw new BridgeFailure("protocol", operation);
  }
  async abandonPreviewReservation(operation: Id): Promise<void> {
    const input = this.input(operation);
    if (
      input &&
      (input.kind !== "document_workspace" ||
        ![
          "asset_read",
          "asset_chunk",
          "replace_preview",
          "replace_page",
          "replace_apply",
        ].includes(input.request.action))
    )
      throw new BridgeFailure("protocol", operation);
    try {
      const reply = await this.call(
        { action: "abandon_reservation", operation },
        operation,
      );
      if (reply.kind !== "acknowledged")
        throw new BridgeFailure("protocol", operation);
    } catch (error) {
      // 빈 예약은 eviction될 수도 있다. ID는 재사용하지 않으며 unknown이면
      // 이 예약으로 뒤늦게 submit하는 것도 불가능하다. 수락된 ID는 native가 거부한다.
      if (
        !(error instanceof BridgeFailure) ||
        error.boundary?.code !== "unknown_id"
      )
        throw error;
    }
    this.forget(operation);
  }
  retainedInput(operation: Id): Work | undefined {
    const value = this.input(operation);
    return value ? structuredClone(value) : undefined;
  }
  async result(
    operation: Id,
  ): Promise<Extract<Response, { kind: "operation" }>> {
    const input = this.input(operation);
    let reply: Response;
    try {
      reply = await this.call({ action: "operation", operation }, operation);
    } catch (error: unknown) {
      // 미전송 제어의 빈 예약은 교체될 수 있다. 수락 ID는 재사용되지 않으므로 UnknownId 뒤
      // 같은 ID의 늦은 submit도 실행될 수 없다. 원 편집과 확인된 terminal 결과는 여기서 버리지 않는다.
      if (
        input &&
        this.controls.get(operation) === input &&
        !this.results.has(operation) &&
        error instanceof BridgeFailure &&
        error.category === "boundary" &&
        error.boundary?.code === "unknown_id"
      )
        this.forget(operation);
      throw error;
    }
    if (reply.kind !== "operation" || reply.operation !== operation)
      throw new BridgeFailure("protocol", operation);
    if (
      reply.result?.kind === "retained_handled" &&
      input &&
      !(
        (input.kind === "retained_handoff" ||
          input.kind === "abandon_retained") &&
        sameReference(input.retained, reply.result.retained) &&
        reply.result.action ===
          (input.kind === "retained_handoff" ? "handed_off" : "abandoned") &&
        (!reply.retained ||
          (input.kind === "retained_handoff" &&
            sameReference(input.retained, reply.retained)))
      )
    )
      throw new BridgeFailure("protocol", operation);
    if (
      reply.result &&
      (reply.state === "complete" || reply.state === "rejected") &&
      input &&
      this.input(operation) === input
    ) {
      this.results.set(operation, structuredClone(reply.result));
      this.owners.set(
        operation,
        reply.retained ? structuredClone(reply.retained) : null,
      );
    }
    return reply;
  }
  async acknowledgeTransport(operation: Id): Promise<void> {
    const input = this.input(operation);
    const result = this.results.get(operation);
    const owner = this.owners.get(operation);
    const explicitTransfer =
      result?.kind === "retained_handled"
        ? result.retained
        : input?.kind === "resume_retained" &&
            result &&
            this.owners.has(operation)
          ? input.retained
          : undefined;
    // ack 대기 중 교체된 사본과 다른 제어 요청의 미인수 결과는 정리 대상이 아니다.
    const previousInputs = explicitTransfer
      ? [...this.inputs].filter(([old]) => {
          const retained = this.owners.get(old);
          return (
            old !== operation &&
            retained &&
            sameReference(retained, explicitTransfer)
          );
        })
      : [];
    const previousAttempt =
      input && this.acknowledgements.get(operation) === input;
    if (input && result) this.acknowledgements.set(operation, input);
    let reply: Response;
    try {
      reply = await this.call(
        { action: "acknowledge_transport", operation },
        operation,
      );
    } catch (error: unknown) {
      // terminal을 관측한 수락 ID는 eviction/재사용되지 않는다. 이전 ack 응답을 잃은 뒤
      // 같은 ID의 명시적 ack 재확인에서 UnknownId면 backend 인수가 끝났다는 뜻이다.
      if (
        !previousAttempt ||
        !result ||
        !(error instanceof BridgeFailure) ||
        error.category !== "boundary" ||
        error.boundary?.code !== "unknown_id"
      )
        throw error;
      reply = { kind: "acknowledged" };
    }
    if (reply.kind !== "acknowledged")
      throw new BridgeFailure("protocol", operation);
    if (!input || this.input(operation) !== input) return;
    for (const [old, previous] of previousInputs) {
      if (this.input(old) === previous) this.forget(old);
    }
    // 편집 유무는 backend의 실제 결과 owner 관측으로 판단한다. action/error 문자열로 추측하지 않는다.
    if (
      result &&
      (owner === null ||
        (input.kind === "retained_handoff" &&
          result.kind === "retained_handled" &&
          owner &&
          sameReference(owner, input.retained)))
    ) {
      // handoff의 작은 요청 사본도 목적 backend owner 인수와 ack 확인 뒤 회수한다.
      // 반환 P/불확정 편집은 이 분기에 들어오지 않고 일반 40개 보관 한도에 남는다.
      this.forget(operation);
    }
    // 실패 편집은 backend와 client가 계속 보관한다. transport ack는 편집 폐기 명령이 아니다.
  }
  retainedReference(operation: Id): RetainedRef | undefined {
    const value = this.owners.get(operation);
    return value ? structuredClone(value) : undefined;
  }
  async listRetained(): Promise<RetainedRef[]> {
    const reply = await this.call({ action: "retained_list" });
    if (reply.kind !== "retained_list") throw new BridgeFailure("protocol");
    return reply.entries;
  }
  async readRetained(
    retained: RetainedRef,
  ): Promise<Extract<Response, { kind: "retained" }>> {
    const reply = await this.call({ action: "retained_read", retained });
    if (
      reply.kind !== "retained" ||
      reply.retained.id !== retained.id ||
      reply.retained.generation !== retained.generation
    )
      throw new BridgeFailure("protocol");
    return reply;
  }
  handoffRetained(operation: Id, retained: RetainedRef) {
    return this.submit(operation, { kind: "retained_handoff", retained });
  }
  /** 원 G6 입력에 대한 별도 사용자 선택이다. transport ack/구독 해제는 이를 호출하지 않는다. */
  abandonRetained(operation: Id, retained: RetainedRef) {
    return this.submit(operation, { kind: "abandon_retained", retained });
  }
  resumeRetained(
    operation: Id,
    retained: RetainedRef,
    project: Id,
    destination: G6Destination,
  ) {
    return this.submit(operation, {
      kind: "resume_retained",
      retained,
      project,
      destination,
    });
  }
  retireProject(operation: Id, project: Id) {
    return this.submit(operation, { kind: "retire_project", project });
  }
  retryNativeCleanup(generation: string) {
    return this.call({ action: "retry_native_cleanup", generation });
  }
  projectStatus(project: Id) {
    return this.call({ action: "project_status", project });
  }
  sessionStatus(project: Id, session: Id) {
    return this.call({ action: "session_status", project, session });
  }
  releaseView(project: Id, view: Id) {
    return this.call({ action: "release_view", project, view });
  }
  appStatus() {
    return this.call({ action: "app_status" });
  }
  async uiReady() {
    return this.closeReply(await this.call({ action: "ui_ready" }));
  }
  async uiCloseStatus() {
    return this.closeReply(await this.call({ action: "ui_close_status" }));
  }
  async uiCloseDecision(attempt: Id, proceed: boolean) {
    return this.closeReply(
      await this.call({ action: "ui_close_decision", attempt, proceed }),
    );
  }
  private closeReply(reply: Response) {
    if (reply.kind !== "ui_close") throw new BridgeFailure("protocol");
    return reply;
  }
  shutdown() {
    return this.call({ action: "app_shutdown" });
  }
  acknowledgeShutdown(project: Id) {
    return this.call({ action: "acknowledge_shutdown", project });
  }
  releaseRound(project: Id) {
    return this.call({ action: "release_round", project });
  }

  open(operation: Id, root: string) {
    return this.submit(operation, { kind: "open", root });
  }
  createProject(operation: Id, parent: string, name: string) {
    return this.submit(operation, { kind: "create_project", parent, name });
  }
  readProjectSettings(operation: Id) {
    return this.submit(operation, { kind: "project_settings_read" });
  }
  writeProjectSettings(operation: Id, defaultRoot: string | null) {
    return this.submit(operation, {
      kind: "project_settings_write",
      default_root: defaultRoot,
    });
  }
  recover(operation: Id, project: Id) {
    return this.submit(operation, { kind: "recover", project });
  }
  close(operation: Id, project: Id) {
    return this.submit(operation, { kind: "close", project });
  }
  listTemplates(operation: Id, project: Id) {
    return this.submit(operation, { kind: "list_templates", project });
  }
  readTemplate(operation: Id, project: Id, template: string) {
    return this.submit(operation, { kind: "read_template", project, template });
  }
  readDocument(operation: Id, project: Id, document: string) {
    return this.submit(operation, { kind: "read_document", project, document });
  }
  createTemplate(
    operation: Id,
    input: Omit<Extract<Work, { kind: "create_template" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "create_template", ...input });
  }
  duplicateTemplate(operation: Id, project: Id, view: Id) {
    return this.submit(operation, {
      kind: "duplicate_template",
      project,
      view,
    });
  }
  createDocument(
    operation: Id,
    input: Omit<Extract<Work, { kind: "create_document" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "create_document", ...input });
  }
  beginSession(
    operation: Id,
    project: Id,
    views: Id[],
    purpose: "template" | "document" | "composite",
  ) {
    return this.submit(operation, {
      kind: "begin_session",
      project,
      views,
      purpose,
    });
  }
  updateTemplate(
    operation: Id,
    input: Omit<Extract<Work, { kind: "update_template" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "update_template", ...input });
  }
  tombstoneTemplate(
    operation: Id,
    input: Omit<Extract<Work, { kind: "tombstone_template" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "tombstone_template", ...input });
  }
  materializeDocument(
    operation: Id,
    input: Omit<Extract<Work, { kind: "materialize_document" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "materialize_document", ...input });
  }
  saveDocument(
    operation: Id,
    input: Omit<Extract<Work, { kind: "save_document" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "save_document", ...input });
  }
  saveComposite(
    operation: Id,
    input: Omit<Extract<Work, { kind: "save_composite" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "save_composite", ...input });
  }
  sessionControl(
    operation: Id,
    input: Omit<Extract<Work, { kind: "session_control" }>, "kind">,
  ) {
    return this.submit(operation, { kind: "session_control", ...input });
  }

  /** 이벤트는 재조회 알림뿐이다. source 교체, 저장, 재시도와 capability 변경을 실행하지 않는다. */
  subscribe(
    onHint: (generation: string) => void,
    onError: (error: BridgeFailure) => void,
    onReady?: () => void,
  ): () => void {
    let stopped = false;
    let unlisten: (() => void) | undefined;
    let latest = -1n;
    void this.transport
      .listen("guarded-state", (hint) => {
        if (stopped || !/^(0|[1-9][0-9]*)$/.test(hint.generation)) return;
        const generation = BigInt(hint.generation);
        if (generation <= latest) return;
        latest = generation;
        onHint(hint.generation);
      })
      .then(
        (stop) => {
          if (stopped) stop();
          else {
            unlisten = stop;
            onReady?.();
          }
        },
        () => {
          if (!stopped) onError(new BridgeFailure("transport"));
        },
      );
    return () => {
      if (!stopped) {
        stopped = true;
        unlisten?.();
      }
    };
  }
}
