import type { Field, RichNode, Value } from "../bridge/types";
import type { DocumentRead } from "../bridge/documents";
import type { EditEntry } from "./documentEdits";
import { cellKey, cellValue } from "./groups";

export interface RequiredWarning {
  field: string;
  label: string;
  group?: string;
  instance?: string;
}

function hasText(node: RichNode): boolean {
  return node.kind === "text"
    ? node.text.trim().length > 0
    : node.kind !== "hardBreak" && node.children.some(hasText);
}

/** Missing input is a reminder, never a scalar/type validation error. */
export function missingRequired(field: Field, value?: Value | null): boolean {
  if (field.lifecycle !== "Active" || !field.required) return false;
  if (!value || value.kind === "unset") return true;
  if (value.kind === "number_unknown") return false;
  if (value.kind === "group") return value.instances.length === 0;
  if (value.kind === "rich_text") return !hasText(value.content);
  if ("value" in value)
    return typeof value.value === "string"
      ? value.value.trim().length === 0
      : value.value.length === 0;
  if (value.kind === "duration") return value.milliseconds.trim() === "";
  if (value.kind === "single_choice") return value.option === "";
  if (value.kind === "multi_choice") return !value.options.length;
  if (value.kind === "relation") return !value.links.length;
  return !value.documents.length;
}

export function requiredWarnings(
  read: DocumentRead,
  entry?: EditEntry,
): RequiredWarning[] {
  const warnings: RequiredWarning[] = [];
  const fields = [
    ...read.template.fieldOrder.flatMap((id) =>
      read.template.fields.filter((field) => field.id === id),
    ),
    ...read.template.fields.filter(
      (field) => !read.template.fieldOrder.includes(field.id),
    ),
  ];
  for (const field of fields) {
    if (field.lifecycle !== "Active") continue;
    const saved = read.fields.find((f) => f.id === field.id)?.value;
    const intent = entry?.body.fields.find((f) => f.field === field.id)?.value;
    const value =
      intent?.intent === "set"
        ? intent.value
        : intent?.intent === "unset"
          ? null
          : saved;
    if (missingRequired(field, value))
      warnings.push({ field: field.id, label: field.label });
    // Children exist only in actual instances, including instances added in this draft.
    if (value?.kind !== "group") continue;
    for (const instance of value.instances) {
      const members = [
        ...(field.memberOrder ?? []).flatMap(
          (id) => field.members?.filter((child) => child.id === id) ?? [],
        ),
        ...(field.members ?? []).filter(
          (child) => !field.memberOrder?.includes(child.id),
        ),
      ];
      for (const child of members) {
        const cell = cellValue(
          instance,
          child.id,
          saved?.kind === "group" ? saved : undefined,
        );
        if (missingRequired(child, cell.intent === "set" ? cell.value : null))
          warnings.push({
            field: cellKey(field.id, instance.id, child.id),
            label: `${field.label} · ${child.label}`,
            group: field.id,
            instance: instance.id,
          });
      }
    }
  }
  return warnings;
}
