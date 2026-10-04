import { expect, it } from "vitest";
import type { Field, Value } from "../bridge/types";
import type { DocumentRead } from "../bridge/documents";
import { missingRequired, requiredWarnings } from "./requiredWarnings";
import { cellInvalid } from "./groups";
import { createElement } from "react";
import { screen } from "@testing-library/react";
import { render } from "../test/render";
import { ValueRead } from "./FieldValue";
import { text } from "../strings";

const field: Field = {
  id: "f",
  label: "필수 내용",
  kind: "SingleLineText",
  lifecycle: "Active",
  required: true,
  presentation: null,
  default: { kind: "unset" },
  initialDefault: { kind: "unset" },
  introducedRevision: "1",
  optionOrder: [],
  options: [],
};

it("reads missing required values as unwritten while keeping valid zero and optional empty distinct", () => {
  const { rerender } = render(
    createElement(ValueRead, {
      field,
      value: { kind: "unset" },
      options: [],
    }),
  );
  expect(screen.getByText(text("required.unwritten"))).toBeInTheDocument();
  rerender(
    createElement(ValueRead, {
      field,
      value: { kind: "number", value: "0" },
      options: [],
    }),
  );
  expect(screen.getByText("0")).toBeInTheDocument();
  expect(
    screen.queryByText(text("required.unwritten")),
  ).not.toBeInTheDocument();
  rerender(
    createElement(ValueRead, {
      field: { ...field, required: false },
      value: { kind: "unset" },
      options: [],
    }),
  );
  expect(screen.getByText(text("field.unsetValue"))).toBeInTheDocument();
});

it("warns for missing and whitespace input without treating zero or unknown as missing", () => {
  const missing: Value[] = [
    { kind: "unset" },
    { kind: "single_line_text", value: " \t\n" },
    {
      kind: "rich_text",
      content: {
        kind: "root",
        children: [
          {
            kind: "paragraph",
            children: [{ kind: "text", text: " \t" }, { kind: "hardBreak" }],
          },
        ],
      },
    },
    { kind: "image", value: [] },
    { kind: "relation", links: [] },
  ];
  missing.forEach((value) => expect(missingRequired(field, value)).toBe(true));
  expect(missingRequired(field, { kind: "number", value: "0" })).toBe(false);
  expect(missingRequired(field, { kind: "number_unknown" })).toBe(false);
  expect(missingRequired({ ...field, lifecycle: "Archived" }, null)).toBe(
    false,
  );
});

it("counts only actual group instances and active required members, including omitted cells", () => {
  const group: Field = {
    ...field,
    id: "g",
    kind: "Group",
    required: false,
    members: [field, { ...field, id: "archived", lifecycle: "Archived" }],
    memberOrder: ["f"],
  };
  const read: DocumentRead = {
    kind: "read",
    id: "d",
    name: "문서",
    warnings: [],
    template: {
      id: "t",
      name: "템플릿",
      revision: "1",
      lifecycle: "Active",
      presentation: null,
      fields: [group],
      fieldOrder: ["g"],
    },
    fields: [
      {
        id: "g",
        label: "그룹",
        state: "Active",
        value: { kind: "group", instances: [] },
      },
    ],
  };
  expect(requiredWarnings(read)).toEqual([]);
  read.fields[0].value = {
    kind: "group",
    instances: [
      { id: "i", source: "i", fields: [] },
      {
        id: "j",
        source: "j",
        fields: [
          {
            field: "f",
            value: { intent: "set", value: { kind: "number", value: "0" } },
          },
        ],
      },
    ],
  };
  expect(requiredWarnings(read)).toMatchObject([{ group: "g", instance: "i" }]);
});

it("empty required cells save while invalid numbers, duplicates and multiplicity still fail", () => {
  expect(cellInvalid(field, { intent: "unset" })).toBe(false);
  expect(
    cellInvalid(
      { ...field, kind: "Number" },
      { intent: "set", value: { kind: "number", value: "wrong" } },
    ),
  ).toBe(true);
  expect(
    cellInvalid(
      { ...field, kind: "DocumentLink" },
      {
        intent: "set",
        value: { kind: "document_link", documents: ["d", "d"] },
      },
    ),
  ).toBe(true);
});

it("follows displayed field and member order rather than storage object order", () => {
  const other = { ...field, id: "a", label: "첫 필드" };
  const read: DocumentRead = {
    kind: "read",
    id: "d",
    name: "문서",
    warnings: [],
    template: {
      id: "t",
      name: "템플릿",
      revision: "1",
      lifecycle: "Active",
      presentation: null,
      fields: [field, other],
      fieldOrder: ["a", "f"],
    },
    fields: [],
  };
  expect(requiredWarnings(read).map((warning) => warning.field)).toEqual([
    "a",
    "f",
  ]);
});

it("uses memberOrder inside each real repeat instance", () => {
  const first = { ...field, id: "first", label: "첫 하위 필드" };
  const group: Field = {
    ...field,
    id: "g",
    kind: "Group",
    required: false,
    members: [field, first],
    memberOrder: ["first", "f"],
  };
  const read: DocumentRead = {
    kind: "read",
    id: "d",
    name: "문서",
    warnings: [],
    template: {
      id: "t",
      name: "템플릿",
      revision: "1",
      lifecycle: "Active",
      presentation: null,
      fields: [group],
      fieldOrder: ["g"],
    },
    fields: [
      {
        id: "g",
        label: "그룹",
        state: "Active",
        value: {
          kind: "group",
          instances: [{ id: "i", source: "i", fields: [] }],
        },
      },
    ],
  };
  expect(requiredWarnings(read).map((warning) => warning.label)).toEqual([
    "필수 내용 · 첫 하위 필드",
    "필수 내용 · 필수 내용",
  ]);
});
