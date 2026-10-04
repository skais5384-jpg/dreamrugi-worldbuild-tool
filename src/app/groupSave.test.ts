import { afterEach, expect, it, vi } from "vitest";
import { DocumentEdits, editDirty, editableProblem } from "./documentEdits";
import { cellValue, copyCard, rebaseGroup } from "./groups";
import type { DocumentEditing, DocumentResponse } from "../bridge/documents";
import type { GroupInstance, GroupValue, Field } from "../bridge/types";

afterEach(() => vi.useRealTimers());

it("그룹 복제는 관계 연결 ID를 새로 만들고 단방향 예외를 초기화한다", () => {
  const card: GroupInstance = {
    id: "source-card",
    source: "source-card",
    fields: [
      {
        field: "relation-field",
        value: {
          intent: "set",
          value: {
            kind: "relation",
            links: [
              {
                id: "old-connection",
                document: "target",
                oneWay: true,
                name: "친구",
              },
            ],
          },
        },
      },
    ],
  };

  const copy = copyCard(card, "copied-card");
  const relation = copy.fields[0].value;
  expect(relation.intent).toBe("set");
  if (relation.intent !== "set" || relation.value.kind !== "relation")
    throw new Error("관계 값 복제 결과가 아닙니다.");
  expect(relation.value.links[0]).toMatchObject({
    document: "target",
    oneWay: false,
    name: "친구",
  });
  expect(relation.value.links[0].id).not.toBe("old-connection");
});

it("저장된 Keep 카드 복제는 canonical 관계를 한 번 materialize하고 재기준화에도 새 ID를 유지한다", () => {
  const baseline: GroupValue = {
    kind: "group",
    instances: [
      {
        id: "saved-card",
        source: "saved-card",
        fields: [
          {
            field: "relation-field",
            value: {
              intent: "set",
              value: {
                kind: "relation",
                links: [
                  {
                    id: "saved-connection",
                    document: "target",
                    oneWay: true,
                    name: "가족",
                  },
                ],
              },
            },
          },
        ],
      },
    ],
  };
  const keep: GroupInstance = {
    id: "saved-card",
    source: "saved-card",
    fields: [],
  };

  const copied = copyCard(keep, "copied-card", baseline);
  const copiedValue = copied.fields[0].value;
  expect(copiedValue.intent).toBe("set");
  if (copiedValue.intent !== "set" || copiedValue.value.kind !== "relation")
    throw new Error("저장된 관계가 복제 초안에 materialize되지 않았습니다.");
  expect(copiedValue.value.links).toHaveLength(1);
  expect(copiedValue.value.links[0]).toMatchObject({
    document: "target",
    oneWay: false,
    name: "가족",
  });
  expect(copiedValue.value.links[0].id).not.toBe("saved-connection");

  const capturedId = copiedValue.value.links[0].id;
  const rebased = rebaseGroup(
    { kind: "group", instances: [copied] },
    baseline,
    baseline,
  );
  const after = rebased.instances[0].fields[0].value;
  if (after.intent !== "set" || after.value.kind !== "relation")
    throw new Error("재기준화가 복제 관계를 잃었습니다.");
  expect(after.value.links[0].id).toBe(capturedId);
  expect(after.value.links[0].oneWay).toBe(false);
});

it.each(["parent retained", "parent deleted", "original deleted", "chain"])(
  "F1: %s preserves divergent siblings and captured values",
  (scenario) => {
    const baseline: GroupValue = {
      kind: "group",
      instances: [{ id: "a", source: "a", fields: [num("1")] }],
    };
    const b: GroupInstance = {
      id: "b",
      source: "a",
      fields: [num("3")],
      lineage: ["a"],
    };
    const c = copyCard(b, "c");
    const d = copyCard(c, "d");
    const saved: GroupValue = {
      kind: "group",
      instances: [
        { id: "a", source: "a", fields: [num("2")] },
        { id: "b", source: "b", fields: [num("4")] },
      ],
    };
    if (scenario === "original deleted")
      saved.instances = saved.instances.filter((i) => i.id !== "a");
    if (scenario === "parent deleted")
      saved.instances = saved.instances.filter((i) => i.id !== "b");
    const latest: GroupValue = {
      kind: "group",
      instances: [scenario === "chain" ? d : c],
    };
    const rebased = rebaseGroup(latest, baseline, saved);
    expect(cellValue(rebased.instances[0], "n", saved)).toEqual(num("3").value);
    expect(rebased.instances.map((i) => i.id)).toEqual(
      latest.instances.map((i) => i.id),
    );
    expect(rebased.instances[0].source).toBe(
      scenario === "parent deleted" ? "a" : "b",
    );
    const second: GroupValue = {
      kind: "group",
      instances: [{ ...rebased.instances[0], fields: [num("3")] }],
    };
    const next = copyCard(rebased.instances[0], "next");
    expect(
      cellValue(
        rebaseGroup({ kind: "group", instances: [next] }, saved, second)
          .instances[0],
        "n",
        second,
      ),
    ).toEqual(num("3").value);
  },
);

it("F1: legacy source is exact; deleted source is never guessed from a sibling", () => {
  const old: GroupValue = {
    kind: "group",
    instances: [{ id: "a", source: "a", fields: [num("1")] }],
  };
  const card: GroupInstance = { id: "c", source: "a", fields: [] };
  const changed: GroupValue = {
    kind: "group",
    instances: [{ id: "a", source: "a", fields: [num("2")] }],
  };
  const next = rebaseGroup({ kind: "group", instances: [card] }, old, changed);
  expect(cellValue(next.instances[0], "n", changed)).toEqual(num("1").value);
  const missing = rebaseGroup({ kind: "group", instances: [card] }, old, {
    kind: "group",
    instances: [{ id: "b", source: "b", fields: [num("9")] }],
  });
  expect(missing.instances[0]).toEqual(card);
});

it("F1: changed baseline only adds changed cells; protected/archived/shared assets remain native Keep", () => {
  const fields = [
    num("1"),
    {
      field: "asset",
      value: {
        intent: "set" as const,
        value: { kind: "file" as const, value: ["shared"] },
      },
    },
  ];
  const old: GroupValue = {
    kind: "group",
    instances: [
      {
        id: "a",
        source: "a",
        fields,
        protected: ["opaque"],
        labels: { archived: "old" },
      },
    ],
  };
  const saved: GroupValue = {
    kind: "group",
    instances: [
      {
        id: "b",
        source: "b",
        fields: [num("2"), fields[1]],
        protected: ["opaque"],
        labels: { archived: "old" },
      },
    ],
  };
  const c: GroupInstance = { id: "c", source: "a", lineage: ["b"], fields: [] };
  const next = rebaseGroup({ kind: "group", instances: [c] }, old, saved)
    .instances[0];
  expect(next.fields).toEqual([num("1")]);
  expect(cellValue(next, "asset", saved)).toEqual(fields[1].value);
});
const num = (value: string) => ({
  field: "n",
  value: { intent: "set" as const, value: { kind: "number" as const, value } },
});
const child: Field = {
  id: "n",
  label: "Number",
  kind: "Number",
  lifecycle: "Active",
  required: false,
  default: { kind: "unset" },
  initialDefault: { kind: "unset" },
  introducedRevision: "1",
  options: [],
  optionOrder: [],
  presentation: null,
};
const group: Field = {
  ...child,
  id: "g",
  label: "Group",
  kind: "Group",
  members: [child],
  memberOrder: ["n"],
};
function fixture(): DocumentEditing {
  return {
    kind: "editing",
    document: "doc",
    owner: "owner",
    generation: "1",
    saved_generation: "1",
    body: { name: { intent: "keep" }, fields: [], composing: false },
    read: {
      kind: "read",
      id: "doc",
      name: "doc",
      template: {
        id: "t",
        name: "t",
        revision: "1",
        lifecycle: "Active",
        presentation: null,
        fieldOrder: ["g"],
        fields: [group],
      },
      fields: [
        {
          id: "g",
          label: "Group",
          state: "Active",
          value: {
            kind: "group",
            instances: [{ id: "a", source: "a", fields: [num("1")] }],
          },
        },
      ],
      warnings: [],
    },
    editable: ["g"],
    source: "base",
    deposited: false,
    outcome: null,
    problem: null,
    field: null,
  };
}
it.each(["response", "refresh"])(
  "F1: A1 → B → A2 → S → C from B preserves C1 through %s",
  async (mode) => {
    vi.useFakeTimers();
    const base = fixture();
    let resolve!: (r: DocumentResponse) => void;
    const work = vi.fn(async (): Promise<DocumentResponse> =>
      structuredClone(base),
    );
    const edits = new DocumentEdits(
      work,
      () => {},
      () => {},
    );
    await edits.begin("doc");
    const sent: GroupInstance[] = [
      { id: "a", source: "a", fields: [num("2")] },
      { id: "b", source: "a", fields: [] },
    ];
    edits.field("doc", "g", {
      intent: "set",
      value: { kind: "group", instances: sent },
    });
    work.mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r;
        }),
    );
    const saving = edits.submit("doc");
    const c = copyCard(sent[1], "c");
    edits.field("doc", "g", {
      intent: "set",
      value: { kind: "group", instances: [...sent, c] },
    });
    const before = cellValue(c, "n", base.read.fields[0].value as GroupValue);
    expect(before).toEqual(num("1").value);
    const savedGroup: GroupValue = {
      kind: "group",
      instances: [
        { id: "a", source: "a", fields: [num("2")] },
        { id: "b", source: "b", fields: [num("1")] },
      ],
    };
    const confirmed: DocumentEditing = {
      ...base,
      generation: "2",
      saved_generation: "2",
      body: {
        ...base.body,
        fields: [
          {
            field: "g",
            value: { intent: "set", value: { kind: "group", instances: sent } },
          },
        ],
      },
      read: {
        ...base.read,
        fields: [
          { id: "g", label: "Group", state: "Active", value: savedGroup },
        ],
      },
    };
    resolve(
      mode === "response"
        ? confirmed
        : { ...confirmed, read: base.read, problem: "SavedReadRequired" },
    );
    await saving;
    if (mode === "refresh") {
      expect(edits.entries.doc.paused).toBe(true);
      work.mockResolvedValueOnce(confirmed);
      await edits.refresh("doc");
    }
    const intent = edits.entries.doc.body.fields[0].value;
    if (intent.intent !== "set" || intent.value.kind !== "group")
      throw Error("lost group");
    expect(editDirty(edits.entries.doc)).toBe(true);
    expect(
      cellValue(
        intent.value.instances.find((i) => i.id === "c")!,
        "n",
        savedGroup,
      ),
    ).toEqual(before);
    expect(intent.value.instances.find((i) => i.id === "c")!.source).toBe("b");
  },
);

it("required absence remains a warning and permits a name-only edit", async () => {
  const base = fixture();
  base.read.template.fields = [
    { ...group, members: [{ ...child, required: true }] },
  ];
  base.read.fields[0].value = {
    kind: "group",
    instances: [{ id: "a", source: "a", fields: [] }],
  };
  const edits = new DocumentEdits(
    async () => base,
    () => {},
    () => {},
  );
  await edits.begin("doc");
  edits.update("doc", (b) => ({
    ...b,
    name: { intent: "set", value: "renamed" },
  }));
  expect(editableProblem(edits.entries.doc)).toBeNull();
});
