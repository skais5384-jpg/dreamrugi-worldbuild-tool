import type {
  Field,
  FieldConfiguration,
  Id,
  Template,
  TemplateEdit,
} from "../bridge/types";
import { text } from "../strings";

export interface FieldSource {
  project: Id;
  view: Id;
  content: Template;
}
export type FieldEdit =
  | Extract<
      TemplateEdit,
      {
        kind:
          | "create_field"
          | "field_label"
          | "field_required"
          | "field_presentation"
          | "add_option"
          | "rename_option"
          | "reorder_options"
          | "reorder_fields"
          | "archive_option"
          | "archive_field"
          | "default";
      }
    >
  | { kind: "keep_default"; field: Id }
  | { kind: "archive_field"; field: Id };
export type FieldProperty =
  | "create"
  | "label"
  | "required"
  | "presentation"
  | "default"
  | "fields_order"
  | "options_order"
  | "archive_field"
  | `option:${string}:${string}:add`
  | `option:${string}:${string}:rename`
  | `option:${string}:${string}:archive`;
export interface FieldDraft {
  property: FieldProperty;
  source: FieldSource;
  edit: FieldEdit;
  initial: FieldEdit;
  submitted: boolean;
  handedOff: boolean;
  committed: boolean;
  message: string;
  requested?: boolean;
  confirmation?: boolean;
}
export interface FieldEditor {
  generation: number;
  field: Id;
  drafts: FieldDraft[];
}
export const kinds = {
  SingleLineText: "single_line_text",
  RichText: "rich_text",
  Number: "number",
  Date: "date",
  Time: "time",
  Image: "image",
  File: "file",
  Url: "url",
  Duration: "duration",
  SingleChoice: "single_choice",
  MultiChoice: "multi_choice",
  Relation: "relation",
  DocumentLink: "document_link",
} as const satisfies Record<string, FieldConfiguration["kind"]>;
export function fieldKind(field: Field) {
  return kinds[field.kind as keyof typeof kinds];
}
export function fieldDraft(
  source: FieldSource,
  property: FieldProperty,
  edit: FieldEdit,
): FieldDraft {
  return {
    source,
    property,
    edit,
    initial: structuredClone(edit),
    submitted: false,
    handedOff: false,
    committed: false,
    message: "",
  };
}
export function fieldDirty(draft: FieldDraft) {
  return (
    !draft.handedOff &&
    (draft.requested ||
      JSON.stringify(draft.edit) !== JSON.stringify(draft.initial))
  );
}
export function optionKey(
  field: Id,
  option: Id,
  operation: "add" | "rename" | "archive",
): FieldProperty {
  return `option:${field}:${option}:${operation}`;
}
/** 저장한 입력만 갱신한다. 다른 입력과 원 revision을 함께 덮어쓰지 않는다. */
export function savedFieldDrafts(
  editor: FieldEditor,
  draft: FieldDraft,
  source: FieldSource,
): FieldDraft[] {
  if (draft.edit.kind === "reorder_fields")
    return [
      fieldDraft(source, draft.property, {
        kind: "reorder_fields",
        fields: source.content.fieldOrder,
      }),
    ];
  const field = source.content.fields.find((f) => f.id === editor.field);
  if (!field) return editor.drafts;
  if (draft.edit.kind === "archive_field") return [];
  const fresh = editField(source, field, editor.generation);
  return editor.drafts.flatMap((d) => {
    if (d.property !== draft.property) return [d];
    if (
      d.edit.kind === "add_option" ||
      d.edit.kind === "archive_option" ||
      d.edit.kind === "archive_field"
    )
      return [];
    if (d.edit.kind === "reorder_options")
      return [
        fieldDraft(source, d.property, {
          ...d.edit,
          options: field.optionOrder,
        }),
      ];
    if (d.edit.kind === "rename_option") {
      const optionId = d.edit.option;
      const option = field.options.find((o) => o.id === optionId);
      return option
        ? [fieldDraft(source, d.property, { ...d.edit, label: option.label })]
        : [];
    }
    return fresh.drafts.filter((f) => f.property === d.property);
  });
}
export function editOwner(edit: FieldEdit) {
  return "field" in edit ? edit.field : null;
}
export function sameEditTarget(a: FieldEdit, b: FieldEdit) {
  if (editOwner(a) !== editOwner(b)) return false;
  if ("option" in a || "option" in b) {
    if (!("option" in a) || !("option" in b)) return false;
    return (
      (typeof a.option === "string" ? a.option : a.option.id) ===
      (typeof b.option === "string" ? b.option : b.option.id)
    );
  }
  return true;
}
export function orderIds(edit: FieldEdit): Id[] | null {
  return edit.kind === "reorder_fields"
    ? edit.fields
    : edit.kind === "reorder_options"
      ? edit.options
      : null;
}
export function sourceOrder(source: FieldSource, edit: FieldEdit): Id[] | null {
  return edit.kind === "reorder_fields"
    ? source.content.fieldOrder
    : edit.kind === "reorder_options"
      ? (source.content.fields.find((f) => f.id === edit.field)?.optionOrder ??
        null)
      : null;
}
export function exactOrder(order: Id[], active: Id[]) {
  return (
    order.length === active.length &&
    new Set(order).size === order.length &&
    order.every((id) => active.includes(id))
  );
}
/** 마우스와 키보드는 표시 순서만 바꾼다. 선택 기본값의 canonical 배열과 분리한다. */
export function moveId(order: Id[], id: Id, position: number): Id[] {
  const from = order.indexOf(id);
  if (from < 0 || position < 0 || position >= order.length) return order;
  const next = order.filter((value) => value !== id);
  next.splice(position, 0, id);
  return next;
}
export function referencesOption(field: Field, option: Id) {
  return field.default.kind === "single_choice"
    ? field.default.option === option
    : field.default.kind === "multi_choice" &&
        field.default.options.includes(option);
}
export function managementError(draft: FieldDraft): string | null {
  const edit = draft.edit;
  const order = orderIds(edit);
  if (order && !exactOrder(order, sourceOrder(draft.source, edit) ?? []))
    return text("order.invalid");
  if (edit.kind !== "archive_option" && edit.kind !== "archive_field")
    return null;
  if (!draft.confirmation) return text("archive.confirmRequired");
  if (edit.kind === "archive_field") return null;
  const field = draft.source.content.fields.find((f) => f.id === edit.field);
  if (!field) return text("field.unavailable");
  const required = referencesOption(field, edit.option);
  if (!required)
    return edit.repair === null ? null : text("archive.unnecessaryRepair");
  const repair = edit.repair;
  if (!repair) return text("archive.repairRequired");
  if (repair.kind === "unset") return null;
  const ids =
    repair.kind === "single_choice" && field.kind === "SingleChoice"
      ? [repair.option]
      : repair.kind === "multi_choice" && field.kind === "MultiChoice"
        ? repair.options
        : [];
  const active = field.optionOrder.filter((id) => id !== edit.option);
  return ids.length &&
    new Set(ids).size === ids.length &&
    ids.every((id) => active.includes(id))
    ? null
    : text("archive.invalidRepair");
}
/** 입력 중인 문자열은 그대로 두고 명백한 숫자 오류를 표시한다. 최종 검증은 backend 책임이다. */
export function fieldInputError(edit: FieldEdit): string | null {
  const value =
    edit.kind === "default"
      ? edit.value
      : edit.kind === "create_field"
        ? edit.default
        : null;
  if (!value) return null;
  if (
    value.kind === "number" &&
    (!/^-?(0|[1-9]\d*)(\.\d*[1-9])?$/.test(value.value) || value.value === "-0")
  )
    return text("field.validation.number");
  if (value.kind === "duration") {
    if (!/^(0|-?[1-9]\d*)$/.test(value.milliseconds))
      return text("field.validation.duration");
    const n = BigInt(value.milliseconds);
    if (n < -9223372036854775808n || n > 9223372036854775807n)
      return text("field.validation.duration");
  }
  return null;
}
export function editField(
  source: FieldSource,
  field: Field,
  generation: number,
): FieldEditor {
  return {
    generation,
    field: field.id,
    drafts: [
      fieldDraft(source, "label", {
        kind: "field_label",
        field: field.id,
        label: field.label,
      }),
      fieldDraft(source, "required", {
        kind: "field_required",
        field: field.id,
        required: field.required,
      }),
      fieldDraft(source, "presentation", {
        kind: "field_presentation",
        field: field.id,
        token: field.presentation,
      }),
      fieldDraft(source, "default", { kind: "keep_default", field: field.id }),
    ],
  };
}
