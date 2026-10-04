import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { DraftConflictChoices } from "./DraftConflictChoices";
import type { RecoveryChange } from "../bridge/workspace";
import type { Field, Template } from "../bridge/types";
const template: Template = {
  id: "internal-template",
  name: "템플릿",
  revision: "9",
  lifecycle: "Active",
  glossaryExcluded: false,
  presentation: null,
  fields: [],
  fieldOrder: [],
};
const item: RecoveryChange = {
  id: "internal-choice-id",
  path: ["name"],
  status: "conflict",
  current: "현재 이름",
  preserved: "작성 중 이름",
  original: "기준 이름",
};
describe("latest draft conflict choices", () => {
  it("offers cancellation before a choice, without applying a merge", () => {
    const apply = vi.fn(async () => {});
    const cancel = vi.fn(async () => {});
    render(
      <DraftConflictChoices
        template={template}
        changes={[item]}
        busy={false}
        apply={apply}
        cancel={cancel}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "비교 취소" }));
    expect(cancel).toHaveBeenCalledOnce();
    expect(apply).not.toHaveBeenCalled();
  });
  it("asks only for actual conflicts and requires explicit current or draft selection", () => {
    const apply = vi.fn(async () => {});
    render(
      <DraftConflictChoices
        template={template}
        changes={[
          item,
          {
            ...item,
            id: "automatic",
            status: "proposed",
            path: ["glossarySummary"],
            preserved: "자동 합성",
          },
        ]}
        busy={false}
        apply={apply}
      />,
    );
    const button = screen.getByRole("button", {
      name: "선택한 내용으로 편집 이어가기",
    });
    expect(button).toBeDisabled();
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
    expect(screen.queryByText("자동 합성")).toBeNull();
    expect(screen.queryByText("internal-choice-id")).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: "현재 내용 사용" }));
    expect(button).toBeEnabled();
    fireEvent.click(button);
    expect(apply).toHaveBeenCalledWith([]);
  });
  it("keeps structured rich text readable and never loads external media during comparison", () => {
    const preserved = {
      intent: "set",
      value: {
        kind: "rich_text",
        content: {
          kind: "root",
          children: [
            {
              kind: "paragraph",
              children: [{ kind: "text", text: "보존 본문", marks: ["bold"] }],
            },
          ],
        },
      },
    };
    render(
      <DraftConflictChoices
        template={template}
        changes={[
          {
            ...item,
            path: ["value"],
            current: { kind: "url", value: "https://example.org/preview.png" },
            preserved,
          },
        ]}
        busy={false}
        apply={vi.fn(async () => {})}
      />,
    );
    expect(screen.getByText("보존 본문").tagName).toBe("STRONG");
    expect(screen.getByText("https://example.org/preview.png")).toBeVisible();
    expect(document.querySelector("img,iframe,video")).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: "작성 중 내용 사용" }));
    expect(
      screen.getByRole("button", { name: "선택한 내용으로 편집 이어가기" }),
    ).toBeEnabled();
  });
  it("uses a readable missing-name fallback for internal identities while preserving literal user text", () => {
    const id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    render(
      <DraftConflictChoices
        template={template}
        changes={[
          { ...item, path: ["cardTitleField"], current: id, preserved: null },
          {
            ...item,
            id: "literal",
            path: ["value"],
            current: id,
            preserved: "본문",
          },
        ]}
        busy={false}
        apply={vi.fn(async () => {})}
      />,
    );
    expect(screen.getByText("이름을 확인할 수 없는 항목")).toBeVisible();
    expect(screen.getAllByText(id)).toHaveLength(1);
  });
  it("renders the native atomic-group projection with cards and hides unnamed internal child IDs", () => {
    const child = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const group: Field = {
      id: "g",
      label: "상세 섹션",
      kind: "Group",
      lifecycle: "Active",
      required: false,
      presentation: null,
      default: { kind: "unset" },
      initialDefault: { kind: "unset" },
      introducedRevision: "1",
      optionOrder: [],
      options: [],
      members: [],
      memberOrder: [],
    };
    const preserved = {
      intent: "set",
      value: {
        kind: "group",
        instances: [
          {
            id: "internal-card",
            source: null,
            fields: [
              {
                field: child,
                value: {
                  intent: "set",
                  value: { kind: "single_line_text", value: "보존 카드 본문" },
                },
              },
            ],
          },
        ],
      },
    };
    render(
      <DraftConflictChoices
        template={{ ...template, fields: [group], fieldOrder: [group.id] }}
        changes={[
          {
            ...item,
            path: ["fields", "$items", "g", "value"],
            current: { intent: "unset" },
            preserved,
          },
        ]}
        busy={false}
        apply={vi.fn(async () => {})}
      />,
    );
    expect(screen.getByText("보존 카드 본문")).toBeVisible();
    expect(
      screen.getByText(/이 반복 그룹은 전체 내용을 선택해야 해요/),
    ).toBeVisible();
    expect(screen.getByText("이름을 확인할 수 없는 하위 필드")).toBeVisible();
    expect(screen.queryByText(child)).toBeNull();
    expect(
      screen.getByRole("button", { name: "선택한 내용으로 편집 이어가기" }),
    ).toBeDisabled();
    fireEvent.click(screen.getByRole("radio", { name: "작성 중 내용 사용" }));
    expect(
      screen.getByRole("button", { name: "선택한 내용으로 편집 이어가기" }),
    ).toBeEnabled();
  });
  it("distinguishes child and option conflicts in their visible and accessible names", () => {
    const field = (id: string, label: string): Field => ({
      id,
      label,
      kind: "SingleLineText",
      lifecycle: "Active",
      required: false,
      presentation: null,
      default: { kind: "unset" },
      initialDefault: { kind: "unset" },
      introducedRevision: "1",
      optionOrder: [],
      options: [],
    });
    const group = {
      ...field("g", "상세 섹션"),
      kind: "Group",
      members: [field("a", "섹션 제목"), field("b", "본문")],
      memberOrder: ["a", "b"],
    };
    const choice = {
      ...field("choice", "관측 상태"),
      kind: "SingleChoice",
      options: [
        { id: "first", label: "준비", lifecycle: "Active" },
        { id: "second", label: "완료", lifecycle: "Active" },
      ],
    };
    const changes = [
      {
        ...item,
        id: "child-a",
        path: [
          "fields",
          "$items",
          "g",
          "configuration",
          "members",
          "$items",
          "a",
          "required",
        ],
        current: true,
        preserved: false,
      },
      {
        ...item,
        id: "child-b",
        path: [
          "fields",
          "$items",
          "g",
          "configuration",
          "members",
          "$items",
          "b",
          "required",
        ],
        current: false,
        preserved: true,
      },
      {
        ...item,
        id: "option-a",
        path: [
          "fields",
          "$items",
          "choice",
          "configuration",
          "options",
          "$items",
          "first",
          "archived",
        ],
        current: true,
        preserved: false,
      },
      {
        ...item,
        id: "option-b",
        path: [
          "fields",
          "$items",
          "choice",
          "configuration",
          "options",
          "$items",
          "second",
          "archived",
        ],
        current: true,
        preserved: false,
      },
    ];
    render(
      <DraftConflictChoices
        template={{ ...template, fields: [group, choice] }}
        changes={changes}
        busy={false}
        apply={vi.fn(async () => {})}
      />,
    );
    for (const label of [
      "상세 섹션 · 섹션 제목 · 필수 입력",
      "상세 섹션 · 본문 · 필수 입력",
      "관측 상태 · 준비 · 보관 상태",
      "관측 상태 · 완료 · 보관 상태",
    ])
      expect(screen.getByRole("radiogroup", { name: label })).toBeVisible();
  });
  it("shows readable card order and allowed template names instead of identities", () => {
    const changes = [
      {
        ...item,
        id: "order",
        path: [
          "fields",
          "$items",
          "g",
          "value",
          "value",
          "instances",
          "$order",
        ],
        original: ["card-a", "card-b"],
        current: ["card-b", "card-a"],
        preserved: ["card-a", "card-b"],
      },
      {
        ...item,
        id: "templates",
        path: ["configuration", "allowedTemplates"],
        current: ["t-one"],
        preserved: ["t-two"],
      },
    ];
    render(
      <DraftConflictChoices
        template={template}
        changes={changes}
        busy={false}
        apply={vi.fn(async () => {})}
        reference={{
          list: null,
          templates: [
            { id: "t-one", name: "인물", revision: "1", lifecycle: "Active" },
            { id: "t-two", name: "지역", revision: "1", lifecycle: "Active" },
          ],
          open: () => {},
        }}
      />,
    );
    expect(screen.getAllByText("반복 항목 1")).toHaveLength(2);
    expect(screen.getAllByText("반복 항목 2")).toHaveLength(2);
    expect(screen.getByText("인물")).toBeVisible();
    expect(screen.getByText("지역")).toBeVisible();
    expect(screen.queryByText("card-a")).toBeNull();
    expect(screen.queryByText("t-one")).toBeNull();
  });
  it("continues editable content while unsupported input remains preserved", () => {
    const apply = vi.fn(async () => {});
    render(
      <DraftConflictChoices
        template={template}
        changes={[
          {
            ...item,
            status: "blocked",
            current: { secretInternalKey: "schemaHash" },
            preserved: { secretInternalKey: "rawPayload" },
          },
        ]}
        busy={false}
        apply={apply}
      />,
    );
    expect(
      screen.getByRole("button", { name: "선택한 내용으로 편집 이어가기" }),
    ).toBeEnabled();
    expect(screen.queryByText("rawPayload")).toBeNull();
    expect(screen.queryByText("schemaHash")).toBeNull();
    expect(apply).not.toHaveBeenCalled();
  });
});
