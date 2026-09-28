import { groupProblem, groupDraft, rebaseGroup } from "./groups";
import { mediaKind } from "./MediaValue";
import { richHasText } from "./rich/adapter";
import { numberInBounds } from "./numberBounds";
import type {
  DocumentEditing,
  DocumentRequest,
  DocumentResponse,
  EditBody,
} from "../bridge/documents";
import type { ResultDto, Value } from "../bridge/types";
import type { Intent, RecoveryRow } from "../bridge/workspace";
import { BridgeFailure } from "../bridge/client";
import { safeFailure } from "./operations";

export interface EditEntry {
  status: DocumentEditing;
  body: EditBody;
  generation: string;
  busy: boolean;
  error: string | null;
  paused: boolean;
  closing?: boolean;
  /** A local SVN save remains unpublished until this exact document is committed. */
  localUncommitted?: boolean;
}
export function emptyValue(v: Value): boolean {
  if (v.kind === "group") return false;
  if (v.kind === "unset") return true;
  if (v.kind === "number_unknown") return false;
  if ("value" in v) return v.value.length === 0;
  if (v.kind === "duration") return v.milliseconds === "";
  if (v.kind === "single_choice") return v.option === "";
  if (v.kind === "multi_choice") return !v.options.length;
  if (v.kind === "relation") return !v.links.length;
  if (v.kind === "document_link") return !v.documents.length;
  return !richHasText(v.content);
}
function valid(v: Value): boolean {
  if (v.kind === "url") return mediaKind(v.value) !== null;
  if (v.kind === "image" || v.kind === "file") return v.value.length <= 32;
  if (v.kind === "number") return /^-?(?:0|[1-9]\d*)(?:\.\d+)?$/.test(v.value);
  if (v.kind === "duration") return /^\d+$/.test(v.milliseconds);
  if (v.kind === "date") {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(v.value)) return false;
    const d = new Date(v.value + "T00:00:00Z");
    return (
      Number.isFinite(d.getTime()) && d.toISOString().slice(0, 10) === v.value
    );
  }
  if (v.kind === "time")
    return /^(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d{1,3})?)?$/.test(
      v.value,
    );
  return true;
}
export function editableProblem(e: EditEntry): string | null {
  if (e.body.composing) return "Composing";
  if (
    e.body.name.intent === "unset" ||
    (e.body.name.intent === "set" && !e.body.name.value.trim())
  )
    return "name";
  // whole autosave는 변경하지 않은 그룹의 현재 유효값도 검사한다.
  for (const definition of e.status.read.template.fields) {
    if (definition.kind !== "Group" || definition.lifecycle !== "Active")
      continue;
    const draft = e.body.fields.find((f) => f.field === definition.id)?.value;
    const value = e.status.read.fields.find(
      (f) => f.id === definition.id,
    )?.value;
    const baseline = value?.kind === "group" ? value : undefined;
    const current =
      draft?.intent === "unset"
        ? groupDraft()
        : draft?.intent === "set" && draft.value.kind === "group"
          ? draft.value
          : groupDraft(baseline);
    const problem = groupProblem(definition, current, baseline);
    if (problem) return problem;
  }
  for (const f of e.body.fields) {
    if (f.value.intent === "keep") continue;
    const definition = e.status.read.template.fields.find(
      (d) => d.id === f.field,
    );
    if (definition?.kind === "Group") continue;
    const required = definition?.required;
    if (f.value.intent === "unset") {
      if (required) return f.field;
      continue;
    }
    if (
      (f.value.value.kind === "number" &&
        f.value.value.value !== "" &&
        !numberInBounds(
          f.value.value.value,
          definition?.minimum,
          definition?.maximum,
        )) ||
      (!emptyValue(f.value.value) && !valid(f.value.value)) ||
      (required && emptyValue(f.value.value))
    )
      return f.field;
  }
  return null;
}
/** 같은 owner가 확정한 저장만 최신 초안의 기준을 바꾼다. retry/refresh에도 동일하게 적용한다. */
function acknowledgeBody(
  body: EditBody,
  sent: EditBody,
  before: DocumentEditing["read"],
  savedRead: DocumentEditing["read"],
): EditBody {
  body = {
    ...body,
    name:
      JSON.stringify(body.name) === JSON.stringify(sent.name)
        ? { intent: "keep" }
        : body.name,
    englishName:
      JSON.stringify(body.englishName) === JSON.stringify(sent.englishName)
        ? { intent: "keep" }
        : body.englishName,
    glossarySummary:
      JSON.stringify(body.glossarySummary) ===
      JSON.stringify(sent.glossarySummary)
        ? { intent: "keep" }
        : body.glossarySummary,
    glossaryExcluded:
      JSON.stringify(body.glossaryExcluded) ===
      JSON.stringify(sent.glossaryExcluded)
        ? { intent: "keep" }
        : body.glossaryExcluded,
    fields: body.fields.filter(
      (f) =>
        // 저장 응답으로 토글용 raw까지 지우면 불명 해제 시 이전 숫자를 잃는다.
        (f.value.intent === "set" &&
          f.value.value.kind === "number_unknown" &&
          f.value.value.previous_raw !== undefined) ||
        (f.value.intent === "set" &&
          f.value.value.kind === "group" &&
          f.value.value.instances.some((i) =>
            i.fields.some(
              (c) =>
                c.value.intent === "set" &&
                c.value.value.kind === "number_unknown" &&
                c.value.value.previous_raw !== undefined,
            ),
          )) ||
        JSON.stringify(f) !==
          JSON.stringify(sent.fields.find((s) => s.field === f.field)),
    ),
  };
  return rebaseBodyGroups(body, before, savedRead);
}

/** 재확인은 이름/일반 필드 raw를 지우지 않고 그룹의 해석 기준만 옮긴다. */
function rebaseBodyGroups(
  body: EditBody,
  before: DocumentEditing["read"],
  savedRead: DocumentEditing["read"],
): EditBody {
  return {
    ...body,
    fields: body.fields.map((f) => {
      if (f.value.intent !== "set" || f.value.value.kind !== "group") return f;
      const saved = savedRead.fields.find((v) => v.id === f.field)?.value;
      if (saved?.kind !== "group") return f;
      return {
        ...f,
        value: {
          intent: "set" as const,
          value: rebaseGroup(
            f.value.value,
            (() => {
              const previous = before.fields.find(
                (v) => v.id === f.field,
              )?.value;
              return previous?.kind === "group" ? previous : undefined;
            })(),
            saved,
          ),
        },
      };
    }),
  };
}
export const editDirty = (e: EditEntry) =>
  e.generation !== e.status.saved_generation;
const paused = (r: DocumentEditing) =>
  !!r.problem &&
  [
    "Uncertain",
    "SourceChanged",
    "SaveFailed",
    "SessionRejected",
    "RecoveryUnavailable",
    "SavedReadRequired",
    "OptionalGroupRepairRejected",
  ].includes(r.problem);

/** 타이머와 진행 요청은 문서별 owner에 남는다. 응답 S는 후속 입력 S+1을 지우지 않는다. */
export class DocumentEdits {
  entries: Record<string, EditEntry> = {};
  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  private pending = new Map<string, Promise<void>>();
  private follow = new Set<string>();
  private closing = new Set<string>();
  private requests = new Map<string, symbol>();
  constructor(
    private work: (r: DocumentRequest) => Promise<DocumentResponse | ResultDto>,
    private changed: () => void,
    private saved: (r: DocumentEditing, sourceChanged: boolean) => void,
    private collaborative: () => boolean = () => false,
  ) {}
  private publish(id: string, entry: EditEntry) {
    this.entries = { ...this.entries, [id]: entry };
    this.changed();
  }
  private current(id: string, owner: string, request: symbol) {
    const latest = this.entries[id];
    return latest?.status.owner === owner && this.requests.get(id) === request
      ? latest
      : undefined;
  }
  hasOwners() {
    return Object.keys(this.entries).length > 0;
  }
  async begin(id: string) {
    if (this.entries[id]) return;
    const r = await this.work({ action: "edit_begin", document: id });
    if (r.kind !== "editing") throw new BridgeFailure("protocol");
    this.install(r);
  }
  private install(r: DocumentEditing) {
    this.publish(r.document, {
      status: r,
      body: r.body,
      generation: r.generation,
      busy: false,
      error: null,
      paused: paused(r),
    });
  }
  update(id: string, change: (b: EditBody) => EditBody) {
    const e = this.entries[id];
    if (!e || this.closing.has(id)) return;
    const g = BigInt(e.generation) + 1n;
    if (g > 18446744073709551615n) return;
    this.publish(id, {
      ...e,
      body: change(structuredClone(e.body)),
      generation: String(g),
      error: null,
    });
    this.schedule(id);
  }
  field(id: string, field: string, value: Intent<Value>) {
    this.update(id, (b) => ({
      ...b,
      fields: [
        ...b.fields.filter((f) => f.field !== field),
        {
          field,
          value:
            value.intent === "set" && emptyValue(value.value)
              ? { intent: "unset" }
              : value,
        },
      ],
    }));
  }
  private clear(id: string) {
    const t = this.timers.get(id);
    if (t) clearTimeout(t);
    this.timers.delete(id);
  }
  /** 저장 가능 상태가 된 최신 입력만 1초 뒤 제출한다. 재확인 응답은 과거 raw를 되살리지 않는다. */
  private schedule(id: string) {
    this.clear(id);
    const entry = this.entries[id];
    if (
      !entry ||
      this.closing.has(id) ||
      entry.paused ||
      !editDirty(entry) ||
      editableProblem(entry)
    )
      return;
    this.timers.set(
      id,
      setTimeout(() => {
        this.timers.delete(id);
        void this.submit(id);
      }, 1000),
    );
  }
  async flush(id: string | null) {
    if (id) {
      this.clear(id);
      if (this.pending.has(id)) {
        this.follow.add(id);
        await this.pending.get(id);
      } else if (this.entries[id] && !this.entries[id].paused)
        await this.submit(id);
    }
  }
  async submit(id: string, deposit = false): Promise<void> {
    this.clear(id);
    const previous = this.pending.get(id);
    if (previous) {
      if (deposit) {
        const owner = this.entries[id]?.status.owner;
        await previous;
        if (!owner || this.entries[id]?.status.owner !== owner) return;
        return this.submit(id, true);
      }
      this.follow.add(id);
      return previous;
    }
    let e = this.entries[id];
    if (!e || (!deposit && (e.paused || editableProblem(e) || !editDirty(e))))
      return;
    if (deposit && e.status.deposited && e.status.generation === e.generation)
      return;
    // 보관된 세대는 불변이다. 같은 raw를 명시 저장할 때도 새 시도 세대를 사용한다.
    if (
      !deposit &&
      e.status.deposited &&
      e.status.generation === e.generation
    ) {
      const generation = BigInt(e.generation) + 1n;
      if (generation > 18446744073709551615n) return;
      e = { ...e, generation: String(generation) };
      this.publish(id, e);
    }
    const request = Symbol();
    this.requests.set(id, request);
    const run = this.run(id, e, deposit, request);
    this.pending.set(id, run);
    try {
      await run;
    } finally {
      if (this.pending.get(id) === run) {
        this.pending.delete(id);
        const latest = this.entries[id];
        const again = this.follow.delete(id);
        if (
          !this.closing.has(id) &&
          !deposit &&
          again &&
          latest &&
          latest.status.owner === e.status.owner &&
          !latest.paused &&
          editDirty(latest) &&
          !editableProblem(latest)
        )
          void this.submit(id);
      }
    }
  }
  private async run(
    id: string,
    submitted: EditEntry,
    deposit: boolean,
    request: symbol,
  ) {
    this.publish(id, { ...submitted, busy: true, error: null });
    try {
      const common = {
        owner: submitted.status.owner,
        generation: submitted.generation,
        body: submitted.body,
      };
      const r = await this.work(
        deposit
          ? { action: "edit_deposit", ...common }
          : { action: "edit_draft", ...common, save: true },
      );
      if (
        r.kind !== "editing" ||
        r.document !== id ||
        r.owner !== submitted.status.owner ||
        r.generation !== submitted.generation
      )
        throw new BridgeFailure("protocol");
      const latest = this.current(id, submitted.status.owner, request);
      if (!latest) return;
      const confirmed =
        r.saved_generation === submitted.generation && !r.problem;
      let protectedStatus: DocumentEditing | null = null;
      if (
        !confirmed &&
        !deposit &&
        [
          "Uncertain",
          "SourceChanged",
          "SaveFailed",
          "SessionRejected",
          "RecoveryUnavailable",
        ].includes(r.problem ?? "")
      )
        protectedStatus = await this.protectLatestInput(id, latest, request);
      const current = this.current(id, submitted.status.owner, request);
      if (!current) return;
      const body = confirmed
        ? acknowledgeBody(
            current.body,
            submitted.body,
            submitted.status.read,
            r.read,
          )
        : current.body;
      if (confirmed) this.saved(r, r.source !== submitted.status.source);
      this.publish(id, {
        ...current,
        status: protectedStatus ? { ...r, deposited: true } : r,
        body,
        busy: false,
        paused: paused(r),
        error: null,
        localUncommitted:
          current.localUncommitted ||
          (this.collaborative() &&
            confirmed &&
            r.source !== submitted.status.source),
      });
    } catch (error) {
      const latest = this.current(id, submitted.status.owner, request);
      if (latest) {
        const protectedStatus = !deposit
          ? await this.protectLatestInput(id, latest, request)
          : null;
        const current = this.current(id, submitted.status.owner, request);
        if (!current) return;
        this.publish(id, {
          ...current,
          status: protectedStatus ?? current.status,
          busy: false,
          paused: true,
          error: safeFailure(error),
        });
      }
    }
  }
  private async protectLatestInput(
    id: string,
    entry: EditEntry,
    request: symbol,
  ): Promise<DocumentEditing | null> {
    if (!this.current(id, entry.status.owner, request)) return null;
    try {
      // This is the existing app-local recovery deposit; it does not acquire a
      // new lock, retry canonical writes, or query the SVN server.
      const result = await this.work({
        action: "edit_deposit",
        owner: entry.status.owner,
        generation: entry.generation,
        body: entry.body,
      });
      const current = this.current(id, entry.status.owner, request);
      return current?.generation === entry.generation &&
        result.kind === "editing" &&
        result.document === id &&
        result.owner === entry.status.owner &&
        result.generation === entry.generation &&
        result.deposited
        ? result
        : null;
    } catch {
      // Keep the current input and original save failure visible. A failed
      // recovery deposit must not be reported as a successful save.
    }
    return null;
  }
  async refresh(id: string) {
    await this.reconcile(id, false);
  }
  async retry(id: string) {
    await this.reconcile(id, true);
  }
  private async reconcile(id: string, retry: boolean) {
    const e = this.entries[id];
    if (!e || e.busy || this.pending.has(id) || this.closing.has(id)) return;
    this.clear(id);
    const request = Symbol();
    this.requests.set(id, request);
    this.publish(id, { ...e, busy: true });
    const run = this.reconcileResponse(id, e, retry, request);
    this.pending.set(id, run);
    try {
      await run;
    } finally {
      if (this.pending.get(id) === run) {
        this.pending.delete(id);
        const latest = this.current(id, e.status.owner, request);
        const again = this.follow.delete(id);
        if (again && latest && !this.closing.has(id) && !latest.paused)
          void this.submit(id);
      }
    }
  }
  private async reconcileResponse(
    id: string,
    e: EditEntry,
    retry: boolean,
    request: symbol,
  ) {
    try {
      const r = await this.work(
        retry
          ? {
              action: "edit_retry",
              owner: e.status.owner,
              generation: e.generation,
              body: e.body,
            }
          : { action: "edit_refresh", owner: e.status.owner },
      );
      const latest = this.current(id, e.status.owner, request);
      if (!latest) return;
      if (
        r.kind !== "editing" ||
        r.owner !== e.status.owner ||
        r.document !== id ||
        (!retry && r.generation !== e.status.generation) ||
        (retry && BigInt(r.generation) < BigInt(e.generation))
      )
        throw new BridgeFailure("protocol");
      const generation = retry
        ? BigInt(r.generation) +
          BigInt(latest.generation) -
          BigInt(e.generation)
        : BigInt(latest.generation);
      if (generation > 18446744073709551615n)
        throw new BridgeFailure("protocol");
      // 재확인은 새 입력을 제출하지 않는다. 응답이 증명한 source/결과만 채택하고
      // 요청 대기 중의 이름·필드 raw와 세대는 현재 owner에서 이어받는다.
      this.publish(id, {
        ...latest,
        status: r,
        generation: String(generation),
        body:
          r.saved_generation === r.generation && !r.problem
            ? rebaseBodyGroups(latest.body, e.status.read, r.read)
            : latest.body,
        busy: false,
        paused: paused(r),
        error: null,
      });
      // paused 중 입력한 raw도 정상 재확인 뒤에는 새 입력과 같은 debounce 규칙을 따른다.
      this.schedule(id);
      if (!retry && !r.problem) this.saved(r, r.source !== e.status.source);
    } catch (error) {
      const latest = this.current(id, e.status.owner, request);
      if (!latest) return;
      this.publish(id, {
        ...latest,
        busy: false,
        error: safeFailure(error),
        paused: true,
      });
    }
  }
  async close(id: string, deposit = false): Promise<boolean> {
    if (this.closing.has(id)) return false;
    const owner = this.entries[id]?.status.owner;
    if (!owner) return true;
    this.closing.add(id);
    if (this.entries[id])
      this.publish(id, { ...this.entries[id], closing: true });
    try {
      return await this.finishClose(id, deposit, owner);
    } finally {
      this.closing.delete(id);
      if (this.entries[id]?.status.owner === owner)
        this.publish(id, { ...this.entries[id], closing: false });
    }
  }
  private async finishClose(
    id: string,
    deposit: boolean,
    owner: string,
  ): Promise<boolean> {
    this.clear(id);
    this.follow.delete(id);
    const p = this.pending.get(id);
    if (p) await p;
    if (!this.entries[id]) return true;
    if (this.entries[id].status.owner !== owner) return false;
    const entry = this.entries[id];
    // A confirmed local SVN save is already durable even before commit. Its
    // acknowledged UI body can differ from the submitted raw in the same
    // generation, so it must not be submitted as a new recovery draft.
    const preserve =
      deposit && (editDirty(entry) || !!entry.status.problem || entry.paused);
    if (!(
      preserve &&
      entry?.status.deposited &&
      entry.status.generation === entry.generation
    ))
      await this.submit(id, preserve);
    const e = this.entries[id];
    if (!e || e.status.owner !== owner || e.busy) return false;
    if (
      preserve &&
      !(e.status.deposited && e.status.generation === e.generation)
    )
      return false;
    if (
      editDirty(e) &&
      !(e.status.deposited && e.status.generation === e.generation)
    )
      return false;
    try {
      const r = await this.work({
        action: "edit_release",
        owner: e.status.owner,
        generation: e.status.generation,
      });
      if (this.entries[id]?.status.owner !== e.status.owner) return false;
      if (r.kind !== "released") throw new BridgeFailure("protocol");
      this.entries = Object.fromEntries(
        Object.entries(this.entries).filter(([key]) => key !== id),
      );
      this.requests.delete(id);
      this.changed();
      return true;
    } catch (error) {
      const latest = this.entries[id];
      if (latest?.status.owner === e.status.owner)
        this.publish(id, {
          ...latest,
          error: safeFailure(error),
          paused: true,
        });
      return false;
    }
  }
  committed(id: string) {
    const entry = this.entries[id];
    if (entry) this.publish(id, { ...entry, localUncommitted: false });
  }
  async restore(row: RecoveryRow) {
    if (!row.row.key || !row.row.depositId || !row.row.payloadDigest)
      throw new BridgeFailure("protocol");
    const r = await this.work({
      action: "edit_restore",
      key: row.row.key,
      deposit_id: row.row.depositId,
      digest: row.row.payloadDigest,
    });
    if (r.kind !== "editing") throw new BridgeFailure("protocol");
    this.install(r);
    return r.document;
  }
}
