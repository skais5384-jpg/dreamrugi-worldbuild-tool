import type { Field, Value } from "../bridge/types";
import type { Intent } from "../bridge/workspace";
import { numberInBounds } from "./numberBounds";
import { richHasText } from "./rich/adapter";
import type { ReferenceContext } from "./DocumentReferenceValue";

export type { CellAddress, GroupInstance, GroupValue } from "../bridge/types";
import type { CellAddress, GroupInstance, GroupValue } from "../bridge/types";
export interface GroupContext {
  owner: string;
  generation: string;
  composing: boolean;
  baseline?: GroupValue;
  problem?: string | null;
  importCell?: (address: CellAddress, image: boolean) => Promise<string | null>;
  reference?: ReferenceContext;
}
export const cellKey = (group: string, instance: string, child: string) =>
  JSON.stringify([group, instance, child]);

export function copyCard(
  card: GroupInstance,
  id: string,
  baseline?: GroupValue,
): GroupInstance {
  const copy = structuredClone(card);
  const source = baseline?.instances.find(
    (instance) => instance.id === (card.source ?? card.id),
  );
  const fields = new Set([
    ...copy.fields.map((field) => field.field),
    ...(source?.fields.map((field) => field.field) ?? []),
  ]);
  for (const child of fields) {
    const value = cellValue(card, child, baseline);
    if (value.intent === "set" && value.value.kind === "relation") {
      const transformed = {
        field: child,
        value: {
          intent: "set" as const,
          value: {
            ...value.value,
            links: value.value.links.map((link) => ({
              ...link,
              id: crypto.randomUUID(),
              // 복제는 같은 연결을 공유하지 않는다. 사용자가 다시 선택하지 않은
              // 단방향 예외까지 복제하면 새 관계의 상호성 경고가 조용히 사라진다.
              oneWay: false,
            })),
          },
        },
      };
      copy.fields = [
        ...copy.fields.filter((field) => field.field !== child),
        transformed,
      ];
    }
  }
  return {
    ...copy,
    id,
    lineage: [card.id, ...(card.lineage ?? [])],
  };
}

/** 응답 전 유효값을 새 canonical에 잇는다. 계보는 원문 선택, 차이만 Set은 값 보존을 맡는다. */
export function rebaseGroup(
  value: GroupValue,
  before: GroupValue | undefined,
  saved: GroupValue,
): GroupValue {
  return {
    ...value,
    instances: value.instances.map((card) => {
      const anchor = [
        card.id,
        ...(card.lineage ?? []),
        ...(card.source ? [card.source] : []),
      ].find((id) => saved.instances.some((i) => i.id === id));
      // 출처가 없는 새 카드는 입력 자체가 전부다. 사라진 구형 출처는 추측하지 않고 native 거부를 유지한다.
      if (!anchor) return card;
      const next: GroupInstance = {
        ...card,
        source: anchor,
        lineage: [],
        fields: [...card.fields],
      };
      const original = before?.instances.find(
        (i) => i.id === (card.source ?? card.id),
      );
      const target = saved.instances.find((i) => i.id === anchor)!;
      const children = new Set(
        [...(original?.fields ?? []), ...target.fields, ...card.fields].map(
          (f) => f.field,
        ),
      );
      for (const child of children) {
        const captured = cellValue(card, child, before);
        const rebased = cellValue(next, child, saved);
        if (JSON.stringify(captured) !== JSON.stringify(rebased)) {
          next.fields = [
            ...next.fields.filter((f) => f.field !== child),
            { field: child, value: captured },
          ];
        }
      }
      return next;
    }),
  };
}

/** 화면의 known DTO를 다시 저장하지 않는다. 편집 전 셀은 native 원문을 Keep한다. */
export function groupDraft(baseline?: GroupValue): GroupValue {
  return {
    kind: "group",
    instances:
      baseline?.instances.map((i) => ({
        id: i.id,
        source: i.id,
        fields: [],
      })) ?? [],
  };
}
export function cellValue(
  instance: GroupInstance,
  child: string,
  baseline?: GroupValue,
): Intent<Value> {
  const intent = instance.fields.find((f) => f.field === child)?.value;
  if (intent && intent.intent !== "keep") return intent;
  return (
    baseline?.instances
      .find((i) => i.id === (instance.source ?? instance.id))
      ?.fields.find((f) => f.field === child)?.value ?? { intent: "unset" }
  );
}
export function cellInvalid(field: Field, intent: Intent<Value>): boolean {
  if (intent.intent === "keep") return false;
  if (
    intent.intent === "unset" ||
    (intent.intent === "set" && intent.value.kind === "unset")
  )
    return field.required;
  const v = intent.value;
  if (v.kind === "number_unknown") return false;
  if (v.kind === "number")
    return v.value === ""
      ? field.required
      : !numberInBounds(v.value, field.minimum, field.maximum);
  if (v.kind === "rich_text") return field.required && !richHasText(v.content);
  if (v.kind === "image" || v.kind === "file")
    return v.value.length > 32 || (field.required && !v.value.length);
  if (v.kind === "relation")
    return (
      (field.required && !v.links.length) ||
      (field.multiple === false && v.links.length > 1) ||
      new Set(v.links.map((link) => link.document)).size !== v.links.length
    );
  if (v.kind === "document_link")
    return (
      (field.required && !v.documents.length) ||
      new Set(v.documents).size !== v.documents.length
    );
  return false;
}
export function groupProblem(
  field: Field,
  value: GroupValue,
  baseline?: GroupValue,
): string | null {
  for (const card of value.instances) {
    for (const child of field.members ?? []) {
      if (
        child.lifecycle === "Active" &&
        cellInvalid(child, cellValue(card, child.id, baseline))
      )
        return cellKey(field.id, card.id, child.id);
    }
  }
  return null;
}
