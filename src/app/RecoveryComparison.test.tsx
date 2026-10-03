import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RecoveryComparison } from "./RecoveryComparison";
import { ArchivedDefinitionActions } from "./ArchivedDefinitionActions";
import { recoveryText } from "./recoveryText";
import type { RecoveryContent, DraftField } from "../bridge/workspace";

const id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
const content = {
  draft: {
    kind: "document",
    template: "template",
    document: "document",
    name: { intent: "keep" },
    fields: [],
    composing: false,
  },
  original: null,
  current: null,
  attempt: null,
  comparison: [
    {
      id: "change",
      path: ["name"],
      status: "conflict",
      original: "before",
      current: "current text",
      preserved: "preserved text",
    },
    {
      id: "blocked",
      path: ["fields", "$items", id],
      status: "blocked",
      reason: "정의를 먼저 복원하세요",
      original: null,
      current: null,
      preserved: { kind: "url", value: "https://example.com/media.mp4" },
    },
  ],
} as RecoveryContent;
describe("recovery comparison choices", () => {
  it("names a new definition once and translates its type without changing entered text", () => {
    render(
      <RecoveryComparison
        content={{
          ...content,
          comparison: [
            {
              id: "new-child",
              path: ["fields", "$items", id],
              status: "proposed",
              original: null,
              current: null,
              preserved: {
                id,
                label: "새 필드",
                configuration: { kind: "single_line_text" },
                writingGuide: { intent: "set", value: "single_line_text" },
              },
            },
          ],
        }}
        selected={[]}
        change={vi.fn()}
      />,
    );
    expect(screen.getByRole("checkbox")).toHaveAccessibleName("새 필드");
    expect(screen.getByText("한 줄 텍스트")).toBeVisible();
    expect(screen.getByText("single_line_text")).toBeVisible();
  });
  it("keeps current by default, displays both texts, and forwards only an explicit supported choice", () => {
    const change = vi.fn();
    render(
      <RecoveryComparison content={content} selected={[]} change={change} />,
    );
    const boxes = screen.getAllByRole("checkbox");
    expect(boxes[0]).not.toBeChecked();
    expect(boxes[1]).toBeDisabled();
    expect(screen.getByText("보관 내용을 적용할 항목 0 / 1")).toBeVisible();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "정의를 먼저 복원하세요",
    );
    expect(screen.getByRole("alert").querySelector("svg")).not.toBeNull();
    expect(
      document.querySelectorAll(".recovery-original")[0],
    ).not.toHaveAttribute("open");
    expect(screen.getByText("current text")).toBeVisible();
    expect(screen.getByText("preserved text")).toBeVisible();
    expect(screen.getByText("https://example.com/media.mp4")).toBeVisible();
    expect(document.querySelector("iframe,img,video,audio")).toBeNull();
    fireEvent.click(boxes[0]);
    expect(change).toHaveBeenCalledWith(["change"]);
  });
  it("lets the user keep current after selecting preserved input", () => {
    const change = vi.fn();
    render(
      <RecoveryComparison
        content={content}
        selected={["change"]}
        change={change}
      />,
    );
    fireEvent.click(screen.getAllByRole("checkbox")[0]);
    expect(change).toHaveBeenCalledWith([]);
  });
  it("copies admitted template and document content as readable text without fetching media", () => {
    const text = recoveryText({
      ...content,
      draft: {
        kind: "admitted_composite",
        template: "template",
        document: "document",
        revision: "1",
        edit: { kind: "name", name: "staged template" },
        edits: [{ kind: "rename", name: "staged document" }],
      },
    });
    expect(text).toContain("staged template");
    expect(text).toContain("staged document");
  });
});
it("restores an archived parent and chosen child while preserving another archived child and unrelated input", () => {
  const member = (key: string): DraftField => ({
    id: key,
    label: key,
    configuration: { kind: "rich_text" },
    default: { intent: "keep" },
    presentation: { intent: "keep" },
    required: false,
    archived: true,
  });
  const field: DraftField = {
    ...member("parent"),
    label: "unrelated draft label",
    archiveTitle: { intent: "set", value: "title" },
    configuration: {
      kind: "group",
      members: [member("title"), member("other")],
      cardTitleField: { intent: "unset" },
    },
  };
  const change = vi.fn();
  render(
    <ArchivedDefinitionActions
      field={field}
      disabled={false}
      canonical={(id) => id}
      change={change}
    />,
  );
  fireEvent.click(screen.getAllByRole("button")[0]);
  const result = change.mock.calls[0][0] as DraftField;
  expect(result.label).toBe("unrelated draft label");
  expect(result.archived).toBe(false);
  expect(result.configuration).toMatchObject({
    members: [
      { id: "title", archived: false },
      { id: "other", archived: true },
    ],
    cardTitleField: { intent: "set", value: "title" },
  });
});
