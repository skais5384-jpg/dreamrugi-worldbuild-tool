import type { Layout, LayoutEdit } from "../bridge/documents";

export type Placement = "before" | "after" | "inside";

/** backend는 이동할 항목을 먼저 뺀 뒤 최종 순서에 삽입한다. 표시도 같은 순서로 계산한다. */
export function documentPlacement(
  layout: Layout,
  document: string,
  target: string | null,
  position: Placement,
): Extract<LayoutEdit, { kind: "move" }> | null {
  if (!layout.nodes[document] || layout.nodes[document].state !== "active")
    return null;
  if (
    target === document ||
    (target && layout.nodes[target]?.state !== "active")
  )
    return null;
  const parent =
    target === null
      ? null
      : position === "inside"
        ? target
        : layout.nodes[target].parentId;
  const seen = new Set<string>();
  for (let id = parent; id !== null; id = layout.nodes[id]?.parentId ?? null) {
    if (id === document || seen.has(id) || !layout.nodes[id]) return null;
    seen.add(id);
  }
  const order = (
    parent ? layout.nodes[parent].childOrder : layout.rootOrder
  ).filter((id) => id !== document);
  if (target !== null && position !== "inside" && !order.includes(target))
    return null;
  const index =
    target === null || position === "inside"
      ? order.length
      : order.indexOf(target) + (position === "after" ? 1 : 0);
  return { kind: "move", document, parent, index };
}
