import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DocumentList } from "../bridge/documents";
import type { TemplateSummary } from "../bridge/types";
import { text } from "../strings";
import { render } from "../test/render";
import { DocumentGlossary } from "./DocumentGlossary";

const templates: TemplateSummary[] = [
  { id: "person", name: "인물", revision: "1", lifecycle: "Active" },
  { id: "place", name: "장소", revision: "1", lifecycle: "Active" },
  {
    id: "hidden-template",
    name: "숨김",
    revision: "1",
    lifecycle: "Active",
    glossaryExcluded: true,
  },
];

function list(documents: DocumentList["documents"]): DocumentList {
  return {
    kind: "list",
    fingerprint: "fixture",
    snapshot: "snapshot",
    initial: false,
    unplaced: [],
    problem: null,
    documents,
    layout: {
      revision: 1,
      rootOrder: documents.map((document) => document.id),
      nodes: Object.fromEntries(
        documents.map((document) => [
          document.id,
          {
            parentId: document.id === "same-b" ? "parent" : null,
            childOrder: [],
            state: document.id === "trash" ? "trashed" : "active",
            trash: null,
          },
        ]),
      ),
    },
  };
}

describe("document glossary", () => {
  it("Template 카테고리별 3열을 정렬하고 제외·휴지통·빈 셀을 보존한다", () => {
    const open = vi.fn();
    const view = render(
      <DocumentGlossary
        list={list([
          {
            id: "same-b",
            template: "person",
            name: "같은 이름",
            englishName: "Same Name",
            glossarySummary: "아주 긴 설명도 저장 원문을 유지한다",
          },
          { id: "parent", template: "place", name: "상위" },
          {
            id: "place-last",
            template: "place",
            name: "후순위 장소",
            englishName: "Last Place",
          },
          { id: "same-a", template: "person", name: "같은 이름" },
          {
            id: "doc-hidden",
            template: "person",
            name: "문서 제외",
            glossaryExcluded: true,
          },
          {
            id: "template-hidden",
            template: "hidden-template",
            name: "템플릿 제외",
          },
          { id: "trash", template: "person", name: "휴지통" },
        ])}
        templates={templates}
        template={null}
        selectTemplate={vi.fn()}
        open={open}
      />,
    );

    expect(
      screen.getAllByRole("columnheader").map((cell) => cell.textContent),
    ).toEqual([
      text("glossary.documentName"),
      text("glossary.englishName"),
      text("glossary.summary"),
    ]);
    expect(
      [...view.container.querySelectorAll(".glossary-category")].map(
        (row) => row.textContent,
      ),
    ).toEqual(["인물", "장소"]);
    const names = [...view.container.querySelectorAll("tbody")].map((body) =>
      [...body.querySelectorAll(".glossary-document-button")].map(
        (button) => button.textContent,
      ),
    );
    expect(names).toEqual([
      ["같은 이름", "같은 이름상위"],
      ["상위", "후순위 장소"],
    ]);
    expect(screen.queryByText("문서 제외")).toBeNull();
    expect(screen.queryByText("템플릿 제외")).toBeNull();
    expect(screen.queryByText("휴지통")).toBeNull();
    expect(screen.getByText("Same Name")).toBeVisible();
    expect(
      screen.getByText("아주 긴 설명도 저장 원문을 유지한다"),
    ).toBeVisible();
    expect(
      view.container.querySelectorAll(".glossary-entry-detail"),
    ).toHaveLength(1);
    expect(
      [
        ...view.container.querySelectorAll("tbody[aria-label='인물'] td"),
      ].filter((cell) => cell.textContent === ""),
    ).toHaveLength(2);
    const personBody = view.container.querySelector(
      "tbody[aria-label='인물']",
    )!;
    fireEvent.click(
      personBody.querySelectorAll(".glossary-document-button")[0],
    );
    expect(open).toHaveBeenCalledWith("same-a");
    fireEvent.click(
      screen.getByRole("button", {
        name: text("glossary.collapseCategory", { name: "인물" }),
      }),
    );
    expect(
      personBody.querySelectorAll(".glossary-document-button"),
    ).toHaveLength(0);
    fireEvent.click(
      screen.getByRole("button", {
        name: text("glossary.expandCategory", { name: "인물" }),
      }),
    );
    expect(
      personBody.querySelectorAll(".glossary-document-button"),
    ).toHaveLength(2);
  });

  it("Template 필터와 100개 단위 더 보기를 제공하고 사라진 필터를 해제한다", () => {
    const selectTemplate = vi.fn();
    const documents = Array.from({ length: 101 }, (_, index) => ({
      id: `person-${String(index).padStart(3, "0")}`,
      template: "person",
      name: `인물 ${String(index).padStart(3, "0")}`,
    }));
    const view = render(
      <DocumentGlossary
        list={list([
          ...documents,
          { id: "place-one", template: "place", name: "장소 하나" },
        ])}
        templates={templates}
        template="person"
        selectTemplate={selectTemplate}
        open={vi.fn()}
      />,
    );
    expect(screen.getAllByRole("button")).toHaveLength(102);
    expect(screen.queryByText("장소 하나")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: text("glossary.more") }),
    );
    expect(screen.getAllByRole("button")).toHaveLength(102);
    expect(screen.getByText("인물 100")).toBeVisible();

    view.rerender(
      <DocumentGlossary
        list={list(documents)}
        templates={templates.filter((row) => row.id !== "person")}
        template="person"
        selectTemplate={selectTemplate}
        open={vi.fn()}
      />,
    );
    expect(selectTemplate).toHaveBeenCalledWith(null);
  });
});
