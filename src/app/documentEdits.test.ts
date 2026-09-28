import { afterEach, describe, expect, it, vi } from "vitest";
import { DocumentEdits, editDirty } from "./documentEdits";
import type {
  DocumentEditing,
  DocumentRequest,
  DocumentResponse,
} from "../bridge/documents";
import type { RecoveryRow } from "../bridge/workspace";

function fixture(id: string): DocumentEditing {
  return {
    kind: "editing",
    document: id,
    owner: "owner-" + id,
    generation: "1",
    saved_generation: "1",
    body: { name: { intent: "keep" }, fields: [], composing: false },
    read: {
      kind: "read",
      id,
      name: "same",
      template: {
        id: "t",
        name: "T",
        revision: "1",
        lifecycle: "Active",
        presentation: null,
        fieldOrder: ["n"],
        fields: [
          {
            id: "n",
            label: "number",
            kind: "Number",
            lifecycle: "Active",
            required: false,
            presentation: null,
            default: { kind: "unset" },
            initialDefault: { kind: "unset" },
            introducedRevision: "1",
            optionOrder: [],
            options: [],
          },
        ],
      },
      fields: [
        {
          id: "n",
          label: "number",
          state: "Active",
          value: { kind: "number", value: "1" },
        },
      ],
      warnings: [],
    },
    editable: ["n"],
    source: "base-" + id,
    deposited: false,
    outcome: null,
    problem: null,
    field: null,
  };
}
function setup(
  initialProblem: DocumentEditing["problem"] = null,
  manualSave = false,
) {
  const states = new Map<string, DocumentEditing>();
  const requests: DocumentRequest[] = [];
  let hold: ((r: DocumentResponse) => void) | null = null;
  let delay = false;
  let submitted: DocumentEditing | null = null;
  const work = vi.fn(async (q: DocumentRequest): Promise<DocumentResponse> => {
    requests.push(q);
    if (q.action === "edit_begin") {
      const d = { ...fixture(q.document), problem: initialProblem };
      states.set(d.owner, d);
      return structuredClone(d);
    }
    if (q.action === "edit_draft" || q.action === "edit_deposit") {
      const d = states.get(q.owner)!;
      const r = {
        ...d,
        body: structuredClone(q.body),
        generation: q.generation,
        saved_generation:
          q.action === "edit_draft" ? q.generation : d.saved_generation,
        source: q.action === "edit_draft" ? "saved-" + d.document : d.source,
        deposited: q.action === "edit_deposit",
      };
      if (q.body.name.intent === "set")
        r.read = { ...r.read, name: q.body.name.value };
      states.set(q.owner, r);
      if (delay) {
        submitted = r;
        return new Promise((resolve) => {
          hold = resolve;
        });
      }
      return r;
    }
    if (q.action === "edit_release") return { kind: "released" };
    throw new Error(q.action);
  });
  const edits = new DocumentEdits(
    work,
    () => {},
    () => {},
    () => manualSave,
  );
  return {
    edits,
    requests,
    work,
    delay: () => {
      delay = true;
    },
    resolve: () => {
      delay = false;
      hold!(submitted!);
    },
  };
}
afterEach(() => vi.useRealTimers());
it("collaborative input saves the canonical document automatically and preserves uncommitted work", async () => {
  vi.useFakeTimers();
  const h = setup(null, true);
  await h.edits.begin("a");
  h.edits.update("a", (body) => ({
    ...body,
    name: { intent: "set", value: "새 이름" },
  }));
  await vi.advanceTimersByTimeAsync(5000);
  await h.edits.flush("a");
  expect(
    h.requests.filter((request) => request.action === "edit_draft"),
  ).toHaveLength(1);
  expect(
    h.requests.filter((request) => request.action === "edit_deposit"),
  ).toHaveLength(0);
  expect(h.edits.entries.a.status.read.name).toBe("새 이름");
  expect(h.edits.entries.a.localUncommitted).toBe(true);
  expect(await h.edits.close("a")).toBe(true);
  expect(
    h.requests.filter((request) => request.action === "edit_deposit"),
  ).toHaveLength(0);
});
const recoveryRow: RecoveryRow = {
  row: {
    locatorFingerprint: "fixture",
    key: { projectFingerprint: "fixture", draftId: "draft", generation: "1" },
    depositId: "deposit",
    payloadKind: "document",
    payloadDigest: "digest",
    error: null,
  },
  version: null,
  createdAtUtc: null,
  artifact: "a",
};

it("optional group automatic repair rejection pauses editing before input starts", async () => {
  const h = setup("OptionalGroupRepairRejected");
  await h.edits.begin("a");
  expect(h.edits.entries.a.paused).toBe(true);
  expect(h.edits.entries.a.status.problem).toBe("OptionalGroupRepairRejected");
});

for (const action of ["refresh", "retry"] as const)
  for (const success of [false, true]) {
    it(`${action} keeps current owner raw on ${success ? "success" : "failure"} and supports later save`, async () => {
      vi.useFakeTimers();
      const h = setup();
      await h.edits.begin("a");
      let resolve!: (value: DocumentResponse) => void;
      let reject!: (error: Error) => void;
      h.work.mockImplementationOnce(
        () =>
          new Promise((yes, no) => {
            resolve = yes;
            reject = no;
          }),
      );
      const waiting = h.edits[action]("a");
      h.edits.update("a", (b) => ({
        ...b,
        name: { intent: "set", value: "later name" },
      }));
      h.edits.field("a", "n", {
        intent: "set",
        value: { kind: "number", value: "-" },
      });
      const raw = structuredClone(h.edits.entries.a.body);
      if (success)
        resolve({
          ...fixture("a"),
          generation: action === "retry" ? "2" : "1",
        });
      else reject(new Error("real rejection boundary"));
      await waiting;
      expect(h.edits.entries.a.body).toEqual(raw);
      expect(h.edits.entries.a.generation).toBe(
        success && action === "retry" ? "4" : "3",
      );
      expect(h.edits.entries.a.paused).toBe(!success);
      expect(editDirty(h.edits.entries.a)).toBe(true);
      if (!success) {
        const latest = h.edits.entries.a;
        h.work.mockResolvedValueOnce({
          ...latest.status,
          generation: String(BigInt(latest.generation) + 1n),
        });
        await h.edits.retry("a");
      }
      h.edits.field("a", "n", {
        intent: "set",
        value: { kind: "number", value: "0" },
      });
      await h.edits.submit("a");
      expect(editDirty(h.edits.entries.a)).toBe(false);
    });
    it(`${action} ${success ? "success" : "failure"} finishes before release and same-ID reopen`, async () => {
      vi.useFakeTimers();
      const h = setup();
      await h.edits.begin("a");
      let resolve!: (value: DocumentResponse) => void;
      let reject!: (error: Error) => void;
      h.work.mockImplementationOnce(
        () =>
          new Promise((yes, no) => {
            resolve = yes;
            reject = no;
          }),
      );
      const waiting = h.edits[action]("a");
      const closing = h.edits.close("a", true);
      await Promise.resolve();
      expect(h.requests.some((q) => q.action === "edit_release")).toBe(false);
      if (success)
        resolve({
          ...fixture("a"),
          generation: action === "retry" ? "2" : "1",
        });
      else reject(new Error("late failure"));
      await waiting;
      expect(await closing).toBe(true);
      expect(h.edits.entries.a).toBeUndefined();
      h.work.mockResolvedValueOnce({ ...fixture("a"), owner: "new-owner" });
      await h.edits.begin("a");
      expect(h.edits.entries.a.status.owner).toBe("new-owner");
      expect(h.edits.entries.a.error).toBeNull();
    });
    it(`late ${action} ${success ? "success" : "failure"} cannot modify a replacement owner`, async () => {
      vi.useFakeTimers();
      const h = setup();
      await h.edits.begin("a");
      let resolve!: (value: DocumentResponse) => void;
      let reject!: (error: Error) => void;
      h.work.mockImplementationOnce(
        () =>
          new Promise((yes, no) => {
            resolve = yes;
            reject = no;
          }),
      );
      const waiting = h.edits[action]("a");
      const closing = h.edits.close("a", true);
      // 별도 복원 응답이 새 owner를 설치한 경계도 응답의 document ID만으로 승인하지 않는다.
      h.work.mockResolvedValueOnce({
        ...fixture("a"),
        owner: "restored-owner",
      });
      await h.edits.restore(recoveryRow);
      const replacement = structuredClone(h.edits.entries.a);
      if (success)
        resolve({
          ...fixture("a"),
          generation: action === "retry" ? "2" : "1",
        });
      else reject(new Error("old owner failure"));
      await waiting;
      expect(await closing).toBe(false);
      expect(h.edits.entries.a).toEqual(replacement);
      expect(h.requests.some((q) => q.action === "edit_release")).toBe(false);
    });
  }
describe("Document whole autosave", () => {
  it("keeps SaveFailed paused across edits and tab flush until explicit retry", async () => {
    vi.useFakeTimers();
    const h = setup("SaveFailed");
    await h.edits.begin("a");
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "latest after failure" },
    }));
    h.edits.flush("a");
    await vi.advanceTimersByTimeAsync(2500);
    expect(h.edits.entries.a.paused).toBe(true);
    expect(h.requests.filter((q) => q.action === "edit_draft")).toHaveLength(0);
    h.work.mockResolvedValueOnce({ ...fixture("a"), generation: "3" });
    await h.edits.retry("a");
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.requests.filter((q) => q.action === "edit_draft")).toHaveLength(1);
    expect(h.edits.entries.a.body.name).toEqual({
      intent: "set",
      value: "latest after failure",
    });
  });
  for (const action of ["refresh", "retry"] as const)
    it(`${action} resumes one delayed save for valid raw entered while paused`, async () => {
      vi.useFakeTimers();
      const h = setup("SavedReadRequired");
      await h.edits.begin("a");
      let resolve!: (response: DocumentResponse) => void;
      h.work.mockImplementationOnce(
        () =>
          new Promise((done) => {
            resolve = done;
          }),
      );
      const waiting = h.edits[action]("a");
      h.edits.update("a", (body) => ({
        ...body,
        name: { intent: "set", value: "latest raw" },
      }));
      resolve({
        ...fixture("a"),
        generation: action === "retry" ? "2" : "1",
      });
      await waiting;
      await vi.advanceTimersByTimeAsync(999);
      expect(
        h.requests.filter((request) => request.action === "edit_draft"),
      ).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(1);
      const drafts = h.requests.filter(
        (request) => request.action === "edit_draft",
      );
      expect(drafts).toHaveLength(1);
      expect(drafts[0]).toMatchObject({
        body: { name: { intent: "set", value: "latest raw" } },
      });
    });
  it("keeps failed reconciliation paused without scheduling its preserved raw", async () => {
    vi.useFakeTimers();
    const h = setup("SavedReadRequired");
    await h.edits.begin("a");
    let reject!: (error: Error) => void;
    h.work.mockImplementationOnce(
      () =>
        new Promise((_resolve, fail) => {
          reject = fail;
        }),
    );
    const waiting = h.edits.refresh("a");
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "preserved raw" },
    }));
    reject(new Error("refresh rejected"));
    await waiting;
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.edits.entries.a.paused).toBe(true);
    expect(h.edits.entries.a.body.name).toEqual({
      intent: "set",
      value: "preserved raw",
    });
    expect(
      h.requests.filter((request) => request.action === "edit_draft"),
    ).toHaveLength(0);
  });
  it("clears the resumed timer when an explicit flush saves the same raw", async () => {
    vi.useFakeTimers();
    const h = setup("SavedReadRequired");
    await h.edits.begin("a");
    let resolve!: (response: DocumentResponse) => void;
    h.work.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const waiting = h.edits.refresh("a");
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "flush once" },
    }));
    resolve(fixture("a"));
    await waiting;
    h.edits.flush("a");
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    expect(
      h.requests.filter((request) => request.action === "edit_draft"),
    ).toHaveLength(1);
  });
  it("does not leave a resumed timer behind when close saves the latest raw", async () => {
    vi.useFakeTimers();
    const h = setup("SavedReadRequired");
    await h.edits.begin("a");
    let resolve!: (response: DocumentResponse) => void;
    h.work.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const waiting = h.edits.refresh("a");
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "close once" },
    }));
    const closing = h.edits.close("a");
    resolve(fixture("a"));
    await waiting;
    expect(await closing).toBe(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(
      h.requests.filter((request) => request.action === "edit_draft"),
    ).toHaveLength(1);
  });
  it("pauses failed saves, preserves raw on failed deposit and refuses close without proof", async () => {
    vi.useFakeTimers();
    const h = setup();
    await h.edits.begin("a");
    h.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "raw" },
    }));
    h.work.mockRejectedValueOnce(new Error("transport"));
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.edits.entries.a.paused).toBe(true);
    h.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "later" },
    }));
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.work).toHaveBeenCalledTimes(3);
    expect(
      h.requests.find((request) => request.action === "edit_deposit"),
    ).toMatchObject({
      action: "edit_deposit",
      body: { name: { intent: "set", value: "raw" } },
    });
    expect(await h.edits.close("a")).toBe(false);
    h.work.mockRejectedValueOnce(new Error("sink"));
    expect(await h.edits.close("a", true)).toBe(false);
    expect(h.edits.entries.a.body.name).toEqual({
      intent: "set",
      value: "later",
    });
    expect(await h.edits.close("a", true)).toBe(true);
  });
  it("shows a confirmed automatic recovery deposit and does not submit it twice", async () => {
    vi.useFakeTimers();
    const h = setup();
    await h.edits.begin("a");
    h.work.mockImplementationOnce(async (q) => ({
      ...fixture("a"),
      generation: q.action === "edit_draft" ? q.generation : "2",
      body: q.action === "edit_draft" ? q.body : fixture("a").body,
      problem: "SaveFailed",
    }));
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "unsaved but protected" },
    }));
    await vi.advanceTimersByTimeAsync(1000);
    const entry = h.edits.entries.a;
    expect(entry.paused).toBe(true);
    expect(entry.status.deposited).toBe(true);
    expect(entry.status.problem).toBe("SaveFailed");
    expect(entry.body.name).toEqual({
      intent: "set",
      value: "unsaved but protected",
    });
    const deposits = h.requests.filter((r) => r.action === "edit_deposit");
    expect(deposits).toHaveLength(1);
    await h.edits.submit("a", true);
    expect(h.requests.filter((r) => r.action === "edit_deposit")).toHaveLength(
      1,
    );
  });
  it("does not re-deposit a confirmed local save when the close choice requests preservation", async () => {
    vi.useFakeTimers();
    const h = setup(null, true);
    await h.edits.begin("a");
    h.edits.update("a", (body) => ({
      ...body,
      name: { intent: "set", value: "saved locally" },
    }));
    await vi.advanceTimersByTimeAsync(1000);
    expect(editDirty(h.edits.entries.a)).toBe(false);
    expect(await h.edits.close("a", true)).toBe(true);
    expect(h.requests.filter((r) => r.action === "edit_deposit")).toHaveLength(
      0,
    );
  });
  it("deposits an unknown required-number value and releases the editor owner on close", async () => {
    vi.useFakeTimers();
    const h = setup("SaveFailed");
    await h.edits.begin("a");
    h.edits.field("a", "n", {
      intent: "set",
      value: { kind: "number_unknown", previous_raw: "1" },
    });
    expect(await h.edits.close("a", true)).toBe(true);
    expect(h.edits.entries.a).toBeUndefined();
    expect(h.requests).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          action: "edit_deposit",
          body: {
            name: { intent: "keep" },
            composing: false,
            fields: [
              {
                field: "n",
                value: {
                  intent: "set",
                  value: { kind: "number_unknown", previous_raw: "1" },
                },
              },
            ],
          },
        }),
        expect.objectContaining({
          action: "edit_release",
          owner: "owner-a",
        }),
      ]),
    );
  });
  it("uses a fresh save generation after immutable deposit", async () => {
    vi.useFakeTimers();
    const h = setup();
    await h.edits.begin("a");
    h.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "kept" },
    }));
    await h.edits.submit("a", true);
    expect(h.edits.entries.a.generation).toBe("2");
    await h.edits.submit("a");
    expect(h.requests[h.requests.length - 1]).toMatchObject({
      action: "edit_draft",
      generation: "3",
    });
  });
  it("debounces final input for 1s, retains invalid raw and blocks composition", async () => {
    vi.useFakeTimers();
    const h = setup();
    await h.edits.begin("a");
    h.edits.field("a", "n", {
      intent: "set",
      value: { kind: "number", value: "-" },
    });
    await vi.advanceTimersByTimeAsync(5000);
    expect(h.requests).toHaveLength(1);
    expect(h.edits.entries.a.body.fields[0].value).toEqual({
      intent: "set",
      value: { kind: "number", value: "-" },
    });
    h.edits.update("a", (b) => ({ ...b, composing: true }));
    h.edits.field("a", "n", {
      intent: "set",
      value: { kind: "number", value: "0" },
    });
    await vi.advanceTimersByTimeAsync(2000);
    expect(h.requests).toHaveLength(1);
    h.edits.update("a", (b) => ({ ...b, composing: false }));
    await vi.advanceTimersByTimeAsync(999);
    expect(h.requests).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(h.requests).toHaveLength(2);
    expect(editDirty(h.edits.entries.a)).toBe(false);
  });
  it("preserves S+1 and sends one follow-up on the confirmed new source", async () => {
    vi.useFakeTimers();
    const h = setup();
    await h.edits.begin("a");
    h.delay();
    h.edits.update("a", (b) => ({ ...b, name: { intent: "set", value: "S" } }));
    await vi.advanceTimersByTimeAsync(1000);
    h.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "S+1" },
    }));
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.requests.filter((q) => q.action === "edit_draft")).toHaveLength(1);
    h.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.requests.filter((q) => q.action === "edit_draft")).toHaveLength(2);
    expect(h.edits.entries.a.status.read.name).toBe("S+1");
    expect(editDirty(h.edits.entries.a)).toBe(false);
  });
  it("keeps three independent owners and only flushes valid original tab", async () => {
    vi.useFakeTimers();
    const h = setup();
    for (const id of ["a", "b", "c"]) await h.edits.begin(id);
    h.edits.field("a", "n", {
      intent: "set",
      value: { kind: "number", value: "-" },
    });
    h.edits.update("b", (b) => ({ ...b, name: { intent: "set", value: "B" } }));
    h.edits.flush("b");
    await vi.advanceTimersByTimeAsync(0);
    expect(editDirty(h.edits.entries.a)).toBe(true);
    expect(editDirty(h.edits.entries.b)).toBe(false);
    expect(h.edits.entries.c.status.source).toBe("base-c");
    expect(await h.edits.close("a")).toBe(false);
    expect(await h.edits.close("a", true)).toBe(true);
    expect(h.requests.find((q) => q.action === "edit_deposit")).toMatchObject({
      owner: "owner-a",
      body: {
        fields: [
          {
            field: "n",
            value: { intent: "set", value: { kind: "number", value: "-" } },
          },
        ],
      },
    });
  });
});

it("group S acknowledgement preserves S+1 raw, order and native clone provenance", async () => {
  vi.useFakeTimers();
  const h = setup();
  await h.edits.begin("a");
  const base = h.edits.entries.a.status;
  const child = base.read.template.fields[0];
  base.read.template.fields = [
    { ...child, id: "g", kind: "Group", members: [child], memberOrder: ["n"] },
  ];
  base.read.fields = [
    {
      id: "g",
      label: "Group",
      state: "Active",
      value: {
        kind: "group",
        instances: [
          {
            id: "old",
            source: "old",
            fields: [
              {
                field: "n",
                value: { intent: "set", value: { kind: "number", value: "1" } },
              },
            ],
          },
        ],
      },
    },
  ];
  h.edits.field("a", "g", {
    intent: "set",
    value: {
      kind: "group",
      instances: [{ id: "b", source: "old", fields: [] }],
    },
  });
  let resolve!: (r: DocumentResponse) => void;
  h.work.mockImplementationOnce(
    () =>
      new Promise((yes) => {
        resolve = yes;
      }),
  );
  const saving = h.edits.submit("a");
  h.edits.field("a", "g", {
    intent: "set",
    value: {
      kind: "group",
      instances: [
        {
          id: "c",
          source: "old",
          lineage: ["b"],
          fields: [
            {
              field: "n",
              value: { intent: "set", value: { kind: "number", value: "-" } },
            },
          ],
        },
        { id: "b", source: "old", fields: [] },
      ],
    },
  });
  resolve({
    ...base,
    generation: "2",
    saved_generation: "2",
    read: {
      ...base.read,
      fields: [
        {
          id: "g",
          label: "Group",
          state: "Active",
          value: {
            kind: "group",
            instances: [
              {
                id: "b",
                source: "b",
                fields: [
                  {
                    field: "n",
                    value: {
                      intent: "set",
                      value: { kind: "number", value: "1" },
                    },
                  },
                ],
              },
            ],
          },
        },
      ],
    },
  });
  await saving;
  const value = h.edits.entries.a.body.fields[0].value;
  expect(
    value.intent === "set" &&
      value.value.kind === "group" &&
      value.value.instances,
  ).toEqual([
    {
      id: "c",
      source: "b",
      lineage: [],
      fields: [
        {
          field: "n",
          value: { intent: "set", value: { kind: "number", value: "-" } },
        },
      ],
    },
    { id: "b", source: "b", lineage: [], fields: [] },
  ]);
  expect(h.edits.entries.a.generation).toBe("3");
  expect(editDirty(h.edits.entries.a)).toBe(true);
});
