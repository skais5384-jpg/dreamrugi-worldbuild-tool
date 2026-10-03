import { describe, expect, it } from "vitest";
import {
  archiveDefinition,
  restoreDefinition,
  consumeRestorationIntents,
  changeGroupMembers,
} from "./archiveDefinition";

describe("archive draft undo and explicit restoration", () => {
  it("undoes one item at its old position while preserving unrelated draft edits", () => {
    const initial = ["a", "b", "c"].map((id) => ({
      id,
      archived: false,
      label: id,
    }));
    const archived = archiveDefinition(initial, "b");
    archived[0] = { ...archived[0], label: "unrelated edit" };
    const restored = restoreDefinition(archived, "b", false);
    expect(restored.map((v) => v.id)).toEqual(["a", "b", "c"]);
    expect(restored[0].label).toBe("unrelated edit");
    expect(restored[1].restore).toBe(false);
    expect(initial[1].archived).toBe(false);
  });
  it("appends historical archived items with explicit intent and keeps other archives", () => {
    const items = [
      { id: "a", archived: false },
      { id: "b", archived: true },
      { id: "c", archived: true },
    ];
    const restored = restoreDefinition(items, "b", true);
    expect(restored).toEqual([
      { id: "a", archived: false },
      { id: "b", archived: false, restore: true },
      { id: "c", archived: true },
    ]);
    expect(archiveDefinition(restored, "b")[1].restore).toBe(false);
  });
});

it.each([
  ["a", "b"],
  ["b", "a"],
])(
  "restores multiple archived siblings in original relative order (%s then %s)",
  (first, second) => {
    const initial = ["a", "b", "c"].map((id) => ({
      id,
      archived: false,
      label: id,
    }));
    let items = archiveDefinition(archiveDefinition(initial, "a"), "b");
    items[2].label = "other edit";
    items = restoreDefinition(
      restoreDefinition(items, first, false),
      second,
      false,
    );
    expect(items.map((item) => item.id)).toEqual(["a", "b", "c"]);
    expect(items[2].label).toBe("other edit");
  },
);
it("restores a cancelled title archive and preserves a later explicit title choice", () => {
  const member: import("../bridge/workspace").DraftField = {
    id: "title",
    label: "title",
    configuration: { kind: "rich_text" },
    required: false,
    presentation: { intent: "keep" },
    default: { intent: "keep" },
    archived: false,
  };
  const group: import("../bridge/workspace").DraftField = {
    ...member,
    id: "group",
    configuration: {
      kind: "group",
      members: [member],
      cardTitleField: { intent: "set", value: "title" },
    },
  };
  const archived = changeGroupMembers(
    group,
    archiveDefinition([member], "title"),
    null,
    (id) => id,
  );
  expect(
    archived.configuration.kind === "group" &&
      archived.configuration.cardTitleField,
  ).toEqual({ intent: "unset" });
  const undone = changeGroupMembers(
    archived,
    restoreDefinition(
      archived.configuration.kind === "group"
        ? archived.configuration.members
        : [],
      "title",
      false,
    ),
    null,
    (id) => id,
  );
  expect(
    undone.configuration.kind === "group" &&
      undone.configuration.cardTitleField,
  ).toEqual({ intent: "set", value: "title" });
  expect(
    changeGroupMembers(
      { ...archived, archiveTitle: undefined },
      [member],
      null,
      (id) => id,
    ).configuration,
  ).toMatchObject({ cardTitleField: { intent: "unset" } });
});
it("consumes only metadata after confirmed save without losing editable values", () => {
  const body: import("../bridge/workspace").TemplateBody = {
    name: "draft",
    presentation: { intent: "keep" },
    composing: false,
    fields: [
      {
        id: "field",
        label: "other edit",
        configuration: {
          kind: "single_choice",
          options: [
            { id: "option", label: "kept", archived: false, restore: true },
          ],
        },
        required: false,
        presentation: { intent: "keep" },
        default: { intent: "keep" },
        archived: false,
        restore: true,
        archiveOrder: ["field"],
      },
    ],
  };
  const rebased = consumeRestorationIntents(body);
  expect(rebased.fields[0].restore).toBeUndefined();
  expect(rebased.fields[0].label).toBe("other edit");
  expect(body.fields[0].restore).toBe(true);
});

it("preserves a later active order change through multiple undo anchors", () => {
  let items = archiveDefinition(
    ["a", "b", "c"].map((id) => ({ id, archived: false })),
    "a",
  );
  items = [items[2], items[1], items[0]];
  items = archiveDefinition(items, "b");
  items = restoreDefinition(restoreDefinition(items, "a", false), "b", false);
  expect(items.map((item) => item.id)).toEqual(["c", "a", "b"]);
});
