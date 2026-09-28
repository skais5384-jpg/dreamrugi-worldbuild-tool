import {
  $createLineBreakNode,
  $createParagraphNode,
  $createTextNode,
  $getRoot,
  $isElementNode,
  $isLineBreakNode,
  $isParagraphNode,
  $isTextNode,
  type LexicalNode,
} from "lexical";
import {
  $createHeadingNode,
  $isHeadingNode,
  HeadingNode,
  type HeadingTagType,
} from "@lexical/rich-text";
import type { RichNode } from "../../bridge/types";
import { $createStructure, StructureNode } from "./StructureNode";

export const richNodes = [HeadingNode, StructureNode];
const marks = ["bold", "italic", "underline", "strikethrough"] as const;
function $import(node: RichNode): LexicalNode {
  if (node.kind === "text") {
    const result = $createTextNode(node.text);
    for (const mark of node.marks ?? []) result.toggleFormat(mark);
    return result;
  }
  if (node.kind === "hardBreak") return $createLineBreakNode();
  if (node.kind === "root") throw new Error("NestedRichRoot");
  const result =
    node.kind === "paragraph"
      ? $createParagraphNode()
      : node.kind === "heading"
        ? $createHeadingNode(`h${node.level}` as HeadingTagType)
        : $createStructure(node.kind, node.kind === "taskItem" && node.checked);
  result.append(...node.children.map($import));
  return result;
}
export function $installRich(root: RichNode) {
  if (root.kind !== "root") throw new Error("InvalidRichRoot");
  const target = $getRoot();
  target.clear().append(...root.children.map($import));
  if (target.isEmpty()) target.append($createParagraphNode());
}
function $export(node: LexicalNode): RichNode {
  if ($isTextNode(node)) {
    const selected = marks.filter((mark) => node.hasFormat(mark));
    // 미지원 형식을 저장할 때 조용히 잃지 않는다.
    if (
      node.getStyle() ||
      [
        "code",
        "subscript",
        "superscript",
        "highlight",
        "lowercase",
        "uppercase",
        "capitalize",
      ].some((mark) =>
        node.hasFormat(mark as Parameters<typeof node.hasFormat>[0]),
      )
    )
      throw new Error("UnsupportedRichFormat");
    return {
      kind: "text",
      text: node.getTextContent(),
      ...(selected.length ? { marks: selected } : {}),
    };
  }
  if ($isLineBreakNode(node)) return { kind: "hardBreak" };
  if (!$isElementNode(node)) throw new Error("UnsupportedRichNode");
  const children = node.getChildren().map($export);
  if ($isParagraphNode(node)) return { kind: "paragraph", children };
  if ($isHeadingNode(node))
    return { kind: "heading", level: Number(node.getTag().slice(1)), children };
  if (node instanceof StructureNode) {
    const kind = node.getStructure();
    return kind === "taskItem"
      ? { kind, checked: node.getChecked(), children }
      : { kind, children };
  }
  throw new Error("UnsupportedRichNode");
}
export function $exportRich(): RichNode {
  return { kind: "root", children: $getRoot().getChildren().map($export) };
}
export function richHasText(node: RichNode): boolean {
  return node.kind === "text"
    ? node.text.length > 0
    : node.kind !== "hardBreak" && node.children.some(richHasText);
}
