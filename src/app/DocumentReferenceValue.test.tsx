import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import type { DocumentList } from "../bridge/documents";
import type { Field, TemplateSummary, Value } from "../bridge/types";
import { ReferenceEditor, ReferenceRead } from "./DocumentReferenceValue";
import { text } from "../strings";

const list: DocumentList = {
  kind: "list",
  fingerprint: "fixture",
  snapshot: "snapshot",
  initial: false,
  unplaced: [],
  problem: null,
  documents: [
    { id: "self", template: "t1", name: "현재" },
    { id: "allowed", template: "t2", name: "허용 문서" },
    { id: "blocked", template: "t3", name: "다른 문서" },
    { id: "trash", template: "t2", name: "휴지통 문서" },
  ],
  layout: {
    revision: 1,
    rootOrder: ["self", "allowed", "blocked", "trash"],
    nodes: Object.fromEntries(
      ["self", "allowed", "blocked", "trash"].map((id) => [
        id,
        {
          parentId: null,
          childOrder: [],
          state: id === "trash" ? "trashed" : "active",
          trash: null,
        },
      ]),
    ),
  },
};
const templates: TemplateSummary[] = [
  { id: "t1", name: "인물", revision: "1", lifecycle: "Active" },
  { id: "t2", name: "장소", revision: "1", lifecycle: "Active" },
  { id: "t3", name: "사건", revision: "1", lifecycle: "Active" },
];
const baseField: Field = {
  id: "relation",
  label: "관련 장소",
  kind: "Relation",
  lifecycle: "Active",
  required: false,
  presentation: null,
  default: { kind: "unset" },
  initialDefault: { kind: "unset" },
  introducedRevision: "1",
  options: [],
  optionOrder: [],
  multiple: true,
  allowedTemplates: ["t2"],
  reciprocalNotice: true,
};
const context = {
  list,
  templates,
  currentDocument: "self",
  open: vi.fn(),
};

describe("문서 참조 입력", () => {
  it("관계 후보에서 자기 자신·휴지통·비허용 Template을 제외하고 stable 연결 ID를 만든다", () => {
    const change = vi.fn();
    render(
      <ReferenceEditor
        id="relation"
        value={{ kind: "relation", links: [] }}
        field={baseField}
        context={context}
        disabled={false}
        change={change}
      />,
    );
    const combobox = screen.getByRole("combobox", {
      name: text("reference.search"),
    });
    fireEvent.click(combobox);
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(screen.getByRole("option", { name: /허용 문서/ })).toBeVisible();
    expect(screen.queryByRole("option", { name: /현재/ })).toBeNull();
    expect(screen.queryByRole("option", { name: /다른 문서/ })).toBeNull();
    fireEvent.click(screen.getByRole("option", { name: /허용 문서/ }));
    const next = change.mock.calls[0][0] as Extract<
      Value,
      { kind: "relation" }
    >;
    expect(next.links[0]).toMatchObject({
      document: "allowed",
      oneWay: false,
    });
    expect(next.links[0].id).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("문서 링크는 자기 자신을 허용하고 관계의 단방향 선택을 값에 보존한다", () => {
    const relationChange = vi.fn();
    const { unmount } = render(
      <ReferenceEditor
        id="relation"
        value={{
          kind: "relation",
          links: [
            { id: "connection", document: "allowed", oneWay: false, name: "" },
          ],
        }}
        field={baseField}
        context={context}
        disabled={false}
        change={relationChange}
      />,
    );
    fireEvent.click(
      screen.getByRole("checkbox", { name: text("reference.oneWay") }),
    );
    expect(relationChange.mock.calls[0][0].links[0].oneWay).toBe(true);
    unmount();

    render(
      <ReferenceEditor
        id="link"
        value={{ kind: "document_link", documents: [] }}
        field={{ ...baseField, kind: "DocumentLink" }}
        context={context}
        disabled={false}
        change={vi.fn()}
      />,
    );
    fireEvent.click(
      screen.getByRole("combobox", { name: text("reference.search") }),
    );
    expect(screen.getByRole("option", { name: /현재/ })).toBeVisible();
  });

  it("선택된 이름 자체를 열고 작은 제거 동작은 탐색과 분리한다", () => {
    const open = vi.fn();
    const change = vi.fn();
    render(
      <ReferenceEditor
        id="relation"
        value={{
          kind: "relation",
          links: [
            { id: "connection", document: "allowed", oneWay: false, name: "" },
          ],
        }}
        field={baseField}
        context={{ ...context, open }}
        disabled={false}
        change={change}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "허용 문서" }));
    expect(open).toHaveBeenCalledOnce();
    expect(open).toHaveBeenCalledWith("allowed");
    const relationInput = screen.getByRole("textbox", {
      name: `허용 문서: ${text("reference.relationName")}`,
    });
    expect(relationInput.closest(".relation-tag")).toBeNull();
    expect(relationInput.closest(".relation-edit-controls")).not.toBeNull();
    expect(
      screen
        .getByRole("checkbox", { name: text("reference.oneWay") })
        .closest(".relation-edit-actions"),
    ).not.toBeNull();
    fireEvent.click(
      screen.getByRole("button", {
        name: `허용 문서: ${text("reference.remove")}`,
      }),
    );
    expect(open).toHaveBeenCalledOnce();
    expect(change.mock.calls[0][0]).toEqual({ kind: "relation", links: [] });
  });

  it("읽기 화면에서 관계는 지속 태그, 문서 링크는 표준 링크로 이름만 표시한다", () => {
    const open = vi.fn();
    const { unmount } = render(
      <ReferenceRead
        value={{
          kind: "relation",
          links: [
            {
              id: "connection",
              document: "allowed",
              oneWay: false,
              name: "친구",
            },
          ],
        }}
        context={{ ...context, open }}
      />,
    );
    const relation = screen.getByRole("button", { name: "허용 문서" });
    expect(relation.closest(".relation-tag")).not.toBeNull();
    expect(relation.closest(".relation-tag")).toHaveTextContent(
      "허용 문서– 친구",
    );
    expect(screen.queryByText("장소", { exact: true })).toBeNull();
    fireEvent.click(relation);
    expect(open).toHaveBeenCalledWith("allowed");
    unmount();

    render(
      <ReferenceRead
        value={{ kind: "document_link", documents: ["allowed"] }}
        context={{ ...context, open }}
      />,
    );
    const link = screen.getByRole("button", { name: "허용 문서" });
    expect(link).toHaveClass("reference-link");
    expect(link.closest(".relation-tag")).toBeNull();
  });

  it("관계 이름을 편집·비워도 연결 ID, 대상, 방향을 그대로 보존한다", () => {
    const change = vi.fn();
    const relation = {
      id: "stable-connection",
      document: "allowed",
      oneWay: true,
      name: "친구",
    };
    render(
      <ReferenceEditor
        id="relation"
        value={{ kind: "relation", links: [relation] }}
        field={baseField}
        context={context}
        disabled={false}
        change={change}
      />,
    );
    const input = screen.getByRole("textbox", {
      name: `허용 문서: ${text("reference.relationName")}`,
    });
    fireEvent.change(input, { target: { value: "단짝" } });
    expect(change.mock.calls[0][0].links[0]).toEqual({
      ...relation,
      name: "단짝",
    });
    fireEvent.change(input, { target: { value: "" } });
    expect(change.mock.calls[1][0].links[0]).toEqual({
      ...relation,
      name: "",
    });
  });

  it("같은 이름 후보도 전체 주소로 검색하고 문서 ID로 정확히 선택한다", () => {
    const change = vi.fn();
    const duplicateList: DocumentList = {
      ...list,
      documents: [
        ...list.documents,
        { id: "allowed-2", template: "t2", name: "허용 문서" },
      ],
      layout: {
        ...list.layout,
        rootOrder: [...list.layout.rootOrder, "allowed-2"],
        nodes: {
          ...list.layout.nodes,
          "allowed-2": {
            parentId: "self",
            childOrder: [],
            state: "active",
            trash: null,
          },
        },
      },
    };
    render(
      <ReferenceEditor
        id="relation"
        value={{ kind: "relation", links: [] }}
        field={baseField}
        context={{ ...context, list: duplicateList }}
        disabled={false}
        change={change}
      />,
    );
    const combobox = screen.getByRole("combobox", {
      name: text("reference.search"),
    });
    fireEvent.click(combobox);
    fireEvent.change(combobox, { target: { value: "현재" } });
    const option = screen.getByRole("option", { name: /허용 문서.*현재/ });
    fireEvent.click(option);
    expect(change.mock.calls[0][0].links[0].document).toBe("allowed-2");
  });

  it("IME 조합 중 option 확정을 막고 Escape는 검색어만 지운다", () => {
    const change = vi.fn();
    render(
      <ReferenceEditor
        id="relation"
        value={{ kind: "relation", links: [] }}
        field={baseField}
        context={context}
        disabled={false}
        change={change}
      />,
    );
    const combobox = screen.getByRole("combobox", {
      name: text("reference.search"),
    });
    fireEvent.click(combobox);
    fireEvent.compositionStart(combobox);
    fireEvent.click(screen.getByRole("option", { name: /허용 문서/ }));
    expect(change).not.toHaveBeenCalled();
    fireEvent.compositionEnd(combobox);
    fireEvent.change(combobox, { target: { value: "허용" } });
    expect(combobox).toHaveValue("허용");
    fireEvent.keyDown(combobox, { key: "Escape" });
    expect(combobox).toHaveValue("");
  });
});
