import { useState } from "react";
import { fireEvent, screen, within, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { render } from "../test/render";
import type { Field, Value } from "../bridge/types";
import { GroupEditor, GroupRead } from "./GroupValue";
import { cellKey, groupDraft, groupProblem, type GroupValue } from "./groups";
import { GroupDefinition } from "./GroupDefinition";
import type { DraftField } from "../bridge/workspace";

const child: Field = {
  id: "n",
  label: "값",
  kind: "Number",
  lifecycle: "Active",
  required: true,
  writingGuide: "가이드",
  default: { kind: "number", value: "99" },
  initialDefault: { kind: "number", value: "55" },
  introducedRevision: "1",
  options: [],
  optionOrder: [],
  presentation: null,
};
const field: Field = {
  ...child,
  id: "g",
  kind: "Group",
  label: "그룹",
  members: [child],
  memberOrder: [child.id],
};

it("read cards omit automatic numbered headings and preserve all unassigned values", () => {
  render(
    <GroupRead
      field={field}
      value={{
        kind: "group",
        instances: [
          {
            id: "card",
            source: null,
            fields: [
              {
                field: "n",
                value: {
                  intent: "set",
                  value: { kind: "number", value: "17" },
                },
              },
            ],
          },
        ],
      }}
    />,
  );
  expect(screen.queryByText("항목 1")).not.toBeInTheDocument();
  expect(screen.getByText("17")).toBeInTheDocument();
  expect(screen.getByText("값")).toBeInTheDocument();
});

const richTitle = (value: string): Value => ({
  kind: "rich_text",
  content: {
    kind: "root",
    children: [
      {
        kind: "heading",
        level: 2,
        children: [{ kind: "text", text: value, marks: ["bold"] }],
      },
    ],
  },
});

it("keeps visible checkbox meaning even when the assigned title has no text", () => {
  const title = { ...child, id: "title", kind: "RichText", label: "제목 필드" };
  render(
    <GroupRead
      field={{ ...field, members: [title], cardTitleField: "title" }}
      value={{
        kind: "group",
        instances: [
          {
            id: "one",
            source: null,
            fields: [
              {
                field: "title",
                value: {
                  intent: "set",
                  value: {
                    kind: "rich_text",
                    content: {
                      kind: "root",
                      children: [
                        {
                          kind: "taskList",
                          children: [
                            { kind: "taskItem", checked: true, children: [] },
                          ],
                        },
                      ],
                    },
                  },
                },
              },
            ],
          },
        ],
      }}
    />,
  );
  expect(screen.getByText("☑")).toBeInTheDocument();
  expect(screen.queryByText("제목 필드")).not.toBeInTheDocument();
});
it.each(["見出し 제목 <script>", "긴 제목 ".repeat(30), " "])(
  "promotes only the assigned active rich title once and keeps safe rich content (%s)",
  (value) => {
    const title = {
      ...child,
      id: "title",
      kind: "RichText",
      label: "제목 필드",
    };
    const group = {
      ...field,
      cardTitleField: title.id,
      members: [title, child],
      memberOrder: [child.id, title.id],
    };
    const view = render(
      <GroupRead
        field={group}
        value={{
          kind: "group",
          instances: [
            {
              id: "one",
              source: null,
              fields: [
                {
                  field: title.id,
                  value: { intent: "set", value: richTitle(value) },
                },
                {
                  field: child.id,
                  value: {
                    intent: "set",
                    value: { kind: "number", value: "8" },
                  },
                },
              ],
            },
          ],
        }}
      />,
    );
    expect(screen.queryByText("제목 필드")).not.toBeInTheDocument();
    expect(view.container.querySelectorAll(".repeat-card-title")).toHaveLength(
      value.trim() ? 1 : 0,
    );
    expect(view.container.querySelector("script")).toBeNull();
    expect(view.container.querySelector("h4 h2")).toBeNull();
    expect(screen.getByText("8")).toBeInTheDocument();
    if (value.trim())
      expect(
        view.container.querySelectorAll(".repeat-card-title h2 strong"),
      ).toHaveLength(1);
  },
);
it.each(["Archived", "protected", "missing", "wrong-kind"])(
  "preserves unpromotable title content (%s)",
  (mode) => {
    const title = {
      ...child,
      id: "title",
      kind: mode === "wrong-kind" ? "Number" : "RichText",
      label: "제목 필드",
      lifecycle: mode === "Archived" ? "Archived" : "Active",
    };
    const view = render(
      <GroupRead
        field={{
          ...field,
          cardTitleField: mode === "missing" ? "other" : title.id,
          members: [title],
        }}
        value={{
          kind: "group",
          instances: [
            {
              id: "one",
              source: null,
              protected: mode === "protected" ? [title.id] : [],
              fields: [
                {
                  field: title.id,
                  value: { intent: "set", value: richTitle("보존 제목") },
                },
              ],
            },
          ],
        }}
      />,
    );
    expect(view.container.querySelector(".repeat-card-title")).toBeNull();
    expect(screen.getByText("보존 제목")).toBeInTheDocument();
    expect(screen.getByText("제목 필드")).toBeInTheDocument();
  },
);

function Editor({
  initial = { kind: "group", instances: [] },
  baseline,
  changed = () => {},
}: {
  initial?: GroupValue;
  baseline?: GroupValue;
  changed?: (v: GroupValue) => void;
}) {
  const [value, setValue] = useState(initial);
  const [generation, setGeneration] = useState(1);
  return (
    <GroupEditor
      field={field}
      value={value}
      disabled={false}
      prefix="input-"
      context={{
        owner: "owner",
        generation: String(generation),
        composing: false,
        baseline,
        importCell: vi.fn(),
      }}
      change={(v) => {
        setValue(v);
        setGeneration((g) => g + 1);
        changed(v);
      }}
    />
  );
}

it("새 카드는 빈 입력이고 3개 카드의 복제·수정·정렬·삭제는 ID와 독립 값을 유지한다", () => {
  const changed = vi.fn();
  render(<Editor changed={changed} />);
  fireEvent.click(screen.getByRole("button", { name: "항목 추가" }));
  const input = screen.getByRole("textbox", { name: "값" });
  expect(input).toHaveValue("");
  expect(input).toHaveAttribute("placeholder", "가이드");
  fireEvent.change(input, { target: { value: "0" } });
  fireEvent.click(screen.getByRole("button", { name: "항목 복제" }));
  fireEvent.click(screen.getAllByRole("button", { name: "항목 복제" })[1]);
  const ids = (changed.mock.lastCall![0] as GroupValue).instances.map(
    (i) => i.id,
  );
  expect(new Set(ids).size).toBe(3);
  fireEvent.change(screen.getAllByRole("textbox", { name: "값" })[1], {
    target: { value: "-" },
  });
  expect(
    screen
      .getAllByRole("textbox", { name: "값" })
      .map((e) => (e as HTMLInputElement).value),
  ).toEqual(["0", "-", "0"]);
  fireEvent.keyDown(
    screen.getAllByRole("button", { name: "항목 순서 변경" })[1],
    { altKey: true, key: "ArrowUp" },
  );
  const reordered = changed.mock.lastCall![0] as GroupValue;
  expect(reordered.instances.map((i) => i.id)).toEqual([
    ids[1],
    ids[0],
    ids[2],
  ]);
  expect(groupProblem(field, reordered)).toBe(cellKey("g", ids[1], "n"));
  for (let i = 0; i < 3; i++)
    fireEvent.click(screen.getAllByRole("button", { name: "항목 삭제" })[0]);
  expect((changed.mock.lastCall![0] as GroupValue).instances).toEqual([]);
});

it("접기는 입력 DOM과 raw를 유지하고 저장 변경을 만들지 않으며 오류 위치를 펼친다", async () => {
  const changed = vi.fn();
  render(
    <Editor
      initial={{
        kind: "group",
        instances: [
          {
            id: "a",
            source: null,
            fields: [
              {
                field: "n",
                value: { intent: "set", value: { kind: "number", value: "-" } },
              },
            ],
          },
        ],
      }}
      changed={changed}
    />,
  );
  const input = screen.getByRole("textbox", { name: "값" });
  fireEvent.click(screen.getByRole("button", { name: "항목 1" }));
  expect(input).not.toBeVisible();
  expect(changed).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "입력 확인 · 값" }));
  await waitFor(() => expect(input).toHaveFocus());
  expect(input).toBeVisible();
  expect(input).toHaveValue("-");
  expect(changed).not.toHaveBeenCalled();
});

it("보관 원문은 Keep 출처로 복제하고 표시 DTO를 저장 의도로 재직렬화하지 않는다", () => {
  const baseline: GroupValue = {
    kind: "group",
    instances: [
      {
        id: "a",
        source: "a",
        fields: [
          {
            field: "n",
            value: { intent: "set", value: { kind: "number", value: "8" } },
          },
        ],
      },
    ],
  };
  const changed = vi.fn();
  render(
    <Editor
      initial={groupDraft(baseline)}
      baseline={baseline}
      changed={changed}
    />,
  );
  expect(screen.getByRole("textbox", { name: "값" })).toHaveValue("8");
  fireEvent.click(screen.getByRole("button", { name: "항목 복제" }));
  const copy = (changed.mock.lastCall![0] as GroupValue).instances[1];
  expect(copy.source).toBe("a");
  expect(copy.fields).toEqual([]);
  expect(copy.id).not.toBe("a");
});

it("Template의 선택한 하위 설정만 표시하고 네 타입과 stable 순서를 제공한다", () => {
  function Definition() {
    const [members, setMembers] = useState<DraftField[]>([]);
    return (
      <GroupDefinition
        members={members}
        owner="o"
        generation="1"
        composing={false}
        disabled={false}
        canonical={(id) => id}
        change={setMembers}
      />
    );
  }
  render(<Definition />);
  for (let i = 0; i < 3; i++) {
    fireEvent.click(screen.getByRole("button", { name: "하위 필드 추가" }));
    fireEvent.change(screen.getByRole("textbox", { name: "필드 이름" }), {
      target: { value: "하위 " + i },
    });
  }
  const select = screen.getByRole("combobox");
  expect(within(select).getAllByRole("option")).toHaveLength(6);
  expect(screen.queryByRole("option", { name: "반복 그룹" })).toBeNull();
  expect(screen.getAllByRole("textbox", { name: "필드 이름" })).toHaveLength(1);
  const tablist = screen.getByRole("tablist", { name: "하위 필드" });
  expect(within(tablist).getAllByRole("tab")).toHaveLength(3);
  expect(
    tablist.lastElementChild?.contains(
      screen.getByRole("button", { name: "하위 필드 추가" }),
    ),
  ).toBe(true);
  const panel = screen.getByRole("tabpanel");
  expect(
    within(panel).getByRole("checkbox", { name: "필수 여부" }),
  ).toBeInTheDocument();
  expect(
    within(panel).getByRole("textbox", { name: "작성 가이드" }),
  ).toBeVisible();
});

it("DnD 취소와 오래된 세대는 값을 바꾸지 않는다", () => {
  const changed = vi.fn();
  const initial: GroupValue = {
    kind: "group",
    instances: ["a", "b", "c"].map((id) => ({ id, source: null, fields: [] })),
  };
  render(<Editor initial={initial} changed={changed} />);
  const handles = screen.getAllByRole("button", { name: "항목 순서 변경" });
  const cards = handles.map((e) => e.closest(".repeat-card")!);
  fireEvent.dragStart(handles[0]);
  fireEvent.dragOver(cards[1], { clientY: 20 });
  fireEvent.keyDown(handles[0], { key: "Escape" });
  fireEvent.drop(cards[1]);
  expect(changed).not.toHaveBeenCalled();
  fireEvent.dragStart(handles[0]);
  fireEvent.change(screen.getAllByRole("textbox", { name: "값" })[2], {
    target: { value: "2" },
  });
  const calls = changed.mock.calls.length;
  fireEvent.drop(cards[1]);
  expect(changed).toHaveBeenCalledTimes(calls);
});
