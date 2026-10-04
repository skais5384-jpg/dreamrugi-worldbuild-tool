import { screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import { render } from "../test/render";
import type { Field, Template } from "../bridge/types";
import { text } from "../strings";
import { ReadonlyTemplate } from "./ReadonlyTemplate";

it("keeps an unlisted archived Field distinguishable from Template/Option states and defaults", () => {
  const field: Field = {
    id: "field",
    label: "선택 필드",
    kind: "SingleChoice",
    lifecycle: "Active",
    required: false,
    presentation: null,
    default: { kind: "single_choice", option: "b" },
    initialDefault: { kind: "single_choice", option: "a" },
    introducedRevision: "1",
    optionOrder: ["b", "a"],
    options: [
      { id: "a", label: "초기 선택", lifecycle: "Active" },
      { id: "b", label: "현재 선택", lifecycle: "Active" },
      { id: "c", label: "보관 선택", lifecycle: "Archived" },
    ],
  };
  const template: Template = {
    id: "template",
    name: "삭제된 템플릿",
    revision: "3",
    lifecycle: "Deleted",
    presentation: null,
    fieldOrder: [field.id],
    fields: [field],
  };
  const { rerender } = render(<ReadonlyTemplate template={template} />);
  const fieldSection = () =>
    screen.getByRole("heading", { name: field.label }).closest("section")!;
  const row = (name: string) =>
    within(fieldSection())
      .getByText(name)
      .closest(".property-row")! as HTMLElement;
  expect(
    within(row(text("field.status"))).getByText(text("field.active")),
  ).toBeVisible();
  rerender(
    <ReadonlyTemplate
      template={{
        ...template,
        fieldOrder: [],
        fields: [{ ...field, lifecycle: "Archived" }],
      }}
    />,
  );
  expect(
    within(row(text("field.status"))).getByText(text("field.archived")),
  ).toBeVisible();
  expect(
    within(row(text("field.status"))).queryByText(text("field.active")),
  ).toBeNull();
  expect(screen.getByText(text("template.readonly"))).toBeVisible();
  expect(
    within(row(text("field.default"))).getByText("현재 선택"),
  ).toBeVisible();
  expect(screen.queryByText(text("field.initial"))).toBeNull();
  // Old interpretation metadata stays in the artifact without a separate history row.
  expect(field.initialDefault).toEqual({ kind: "single_choice", option: "a" });
  const options = within(row(text("field.options"))).getAllByRole("listitem");
  expect(options.map((item) => item.textContent)).toEqual([
    "현재 선택" + text("field.active"),
    "초기 선택" + text("field.active"),
    "보관 선택" + text("field.archived"),
  ]);
});
