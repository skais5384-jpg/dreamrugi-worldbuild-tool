import {
  ElementNode,
  $applyNodeReplacement,
  type NodeKey,
  type SerializedElementNode,
} from "lexical";

export type Structure =
  | "blockquote"
  | "bulletList"
  | "orderedList"
  | "listItem"
  | "taskList"
  | "taskItem";
type SerializedStructure = SerializedElementNode & {
  structure: Structure;
  checked: boolean;
};

/** 기본 listItem의 문단 병합을 피한다. 프로젝트 AST의 블록 경계를 그대로 소유한다. */
export class StructureNode extends ElementNode {
  __structure: Structure;
  __checked: boolean;
  static getType() {
    return "worldbuild-structure";
  }
  static clone(node: StructureNode) {
    return new StructureNode(node.__structure, node.__checked, node.__key);
  }
  constructor(
    structure: Structure = "blockquote",
    checked = false,
    key?: NodeKey,
  ) {
    super(key);
    this.__structure = structure;
    this.__checked = checked;
  }
  static importJSON(value: SerializedStructure) {
    return $createStructure(value.structure, value.checked).updateFromJSON(
      value,
    );
  }
  exportJSON(): SerializedStructure {
    return {
      ...super.exportJSON(),
      type: StructureNode.getType(),
      version: 1,
      structure: this.__structure,
      checked: this.__checked,
    };
  }
  createDOM(): HTMLElement {
    const tag =
      this.__structure === "blockquote"
        ? "blockquote"
        : this.__structure === "orderedList"
          ? "ol"
          : this.__structure.endsWith("List")
            ? "ul"
            : "li";
    const element = document.createElement(tag);
    if (this.__structure === "taskList") element.className = "rich-task-list";
    if (this.__structure === "taskItem") {
      element.dataset.richTask = this.__key;
      element.dataset.checked = String(this.__checked);
    }
    return element;
  }
  updateDOM(previous: StructureNode, element: HTMLElement): boolean {
    if (previous.__structure !== this.__structure) return true;
    if (this.__structure === "taskItem")
      element.dataset.checked = String(this.__checked);
    return false;
  }
  setChecked(checked: boolean) {
    this.getWritable().__checked = checked;
  }
  getStructure() {
    return this.getLatest().__structure;
  }
  /** 목록 종류를 바꿀 때 자식 블록과 현재 선택을 옮기지 않고 컨테이너 의미만 갱신한다. */
  setStructure(structure: Structure) {
    this.getWritable().__structure = structure;
  }
  getChecked() {
    return this.getLatest().__checked;
  }
  canBeEmpty() {
    return true;
  }
}
export function $createStructure(structure: Structure, checked = false) {
  return $applyNodeReplacement(new StructureNode(structure, checked));
}
