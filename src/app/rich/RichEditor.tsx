import { useEffect, useRef, useState } from "react";
import { LexicalComposer } from "@lexical/react/LexicalComposer";
import { RichTextPlugin } from "@lexical/react/LexicalRichTextPlugin";
import { ContentEditable } from "@lexical/react/LexicalContentEditable";
import { HistoryPlugin } from "@lexical/react/LexicalHistoryPlugin";
import { LexicalErrorBoundary } from "@lexical/react/LexicalErrorBoundary";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { RichToolbar } from "./RichToolbar";
import {
  $createParagraphNode,
  $getSelection,
  $setSelection,
  $isRangeSelection,
  $isElementNode,
  $getNodeByKey,
  $getRoot,
  $createRangeSelection,
  $isTextNode,
  $isLineBreakNode,
  COMMAND_PRIORITY_CRITICAL,
  DROP_COMMAND,
  FORMAT_TEXT_COMMAND,
  KEY_ENTER_COMMAND,
  PASTE_COMMAND,
  REDO_COMMAND,
  UNDO_COMMAND,
  type LexicalNode,
  type RangeSelection,
  type LexicalCommand,
} from "lexical";
import { $createHeadingNode } from "@lexical/rich-text";
import { $isHeadingNode } from "@lexical/rich-text";
import { $setBlocksType } from "@lexical/selection";
import { mergeRegister } from "@lexical/utils";
import type { RichNode } from "../../bridge/types";
import { text } from "../../strings";
import { $exportRich, $installRich, richNodes } from "./adapter";

export interface RichSpellPatch {
  block: number;
  start: number;
  end: number;
  expected: string;
  replacement: string;
}
export interface RichSpellRequest {
  patches: RichSpellPatch[];
  active: () => boolean;
  resolve: (applied: boolean) => void;
}

function spellBlocks(): LexicalNode[] {
  const blocks: LexicalNode[] = [];
  const visit = (node: LexicalNode) => {
    if (node.getType() === "paragraph" || $isHeadingNode(node)) {
      blocks.push(node);
      return;
    }
    if ($isElementNode(node)) node.getChildren().forEach(visit);
  };
  $getRoot().getChildren().forEach(visit);
  return blocks;
}

function spellReplace(patches: readonly RichSpellPatch[]): boolean {
  const initial = spellBlocks();
  const occupied = new Map<number, number>();
  for (const patch of [...patches].sort(
    (a, b) => a.block - b.block || a.start - b.start,
  )) {
    const block = initial[patch.block];
    if (
      !block ||
      !$isElementNode(block) ||
      block.getChildren().some($isLineBreakNode)
    )
      return false;
    const content = block
      .getChildren()
      .filter($isTextNode)
      .map((node) => node.getTextContent())
      .join("");
    if (
      patch.start < (occupied.get(patch.block) ?? 0) ||
      patch.end <= patch.start ||
      content.slice(patch.start, patch.end) !== patch.expected
    )
      return false;
    occupied.set(patch.block, patch.end);
  }
  for (const patch of [...patches].sort(
    (a, b) => b.block - a.block || b.start - a.start,
  )) {
    const block = spellBlocks()[patch.block];
    if (!block || !$isElementNode(block)) return false;
    const leaves = block.getChildren();
    if (leaves.some($isLineBreakNode)) return false;
    const textNodes = leaves.filter($isTextNode);
    const content = textNodes.map((node) => node.getTextContent()).join("");
    if (content.slice(patch.start, patch.end) !== patch.expected) return false;
    let cursor = 0;
    let start: { key: string; offset: number } | null = null;
    let end: { key: string; offset: number } | null = null;
    for (const node of textNodes) {
      const length = node.getTextContentSize();
      if (!start && patch.start >= cursor && patch.start < cursor + length)
        start = { key: node.getKey(), offset: patch.start - cursor };
      if (!end && patch.end > cursor && patch.end <= cursor + length)
        end = { key: node.getKey(), offset: patch.end - cursor };
      cursor += length;
    }
    if (!start || !end) return false;
    const selection = $createRangeSelection();
    selection.anchor.set(start.key, start.offset, "text");
    selection.focus.set(end.key, end.offset, "text");
    $setSelection(selection);
    selection.insertText(patch.replacement);
  }
  return true;
}

function SpellReplaceListener() {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    const onReplace = (event: Event) => {
      const request = (event as CustomEvent<RichSpellRequest>).detail;
      if (!request.active() || !editor.isEditable() || editor.isComposing()) {
        request.resolve(false);
        return;
      }
      editor.update(() =>
        request.resolve(request.active() && spellReplace(request.patches)),
      );
    };
    return editor.registerRootListener((root, previous) => {
      previous?.removeEventListener("spell-replace", onReplace);
      root?.addEventListener("spell-replace", onReplace);
    });
  }, [editor]);
  return null;
}
import {
  $createStructure,
  StructureNode,
  type Structure,
} from "./StructureNode";

interface Props {
  id: string;
  label: string;
  value: RichNode;
  disabled: boolean;
  invalid: boolean;
  guide?: string;
  change: (value: RichNode) => void;
}
const formats = ["bold", "italic", "underline", "strikethrough"] as const;
const listStructures = ["bulletList", "orderedList", "taskList"] as const;
type ListStructure = (typeof listStructures)[number];
function isListStructure(value: Structure): value is ListStructure {
  return listStructures.includes(value as ListStructure);
}
function nearestStructure(
  node: LexicalNode,
  structures: readonly Structure[],
): StructureNode | null {
  for (
    let current: LexicalNode | null = node;
    current;
    current = current.getParent()
  )
    if (
      current instanceof StructureNode &&
      structures.includes(current.getStructure())
    )
      return current;
  return null;
}
function selectedBlocks(selection: RangeSelection): LexicalNode[] {
  const blocks: LexicalNode[] = [];
  for (const node of selection.getNodes()) {
    let candidate: LexicalNode | null =
      $isElementNode(node) && !node.isInline() ? node : node.getParent();
    while (candidate && !["paragraph", "heading"].includes(candidate.getType()))
      candidate = candidate.getParent();
    if (candidate && !blocks.includes(candidate)) blocks.push(candidate);
  }
  return blocks;
}
function listItemFor(block: LexicalNode): StructureNode | null {
  return nearestStructure(block, ["listItem", "taskItem"]);
}
function parentListFor(item: StructureNode): StructureNode | null {
  const parent = item.getParent();
  return parent instanceof StructureNode &&
    isListStructure(parent.getStructure())
    ? parent
    : null;
}
interface ListTarget {
  list: StructureNode;
  items: StructureNode[];
}
function isWithin(node: LexicalNode, ancestor: LexicalNode) {
  for (
    let current: LexicalNode | null = node;
    current;
    current = current.getParent()
  )
    if (current.is(ancestor)) return true;
  return false;
}
/** DOM 선택에 걸린 leaf와 실제 목록 변환 대상을 분리해, 부모와 그 자식을 한 명령에서 중복 처리하지 않는다. */
function collectListTargets(blocks: readonly LexicalNode[]) {
  const byList = new Map<StructureNode, StructureNode[]>();
  const plain: LexicalNode[] = [];
  for (const block of blocks) {
    const item = listItemFor(block);
    const list = item ? parentListFor(item) : null;
    if (!item || !list) {
      plain.push(block);
      continue;
    }
    const items = byList.get(list) ?? [];
    if (!items.includes(item)) items.push(item);
    byList.set(list, items);
  }
  return {
    targets: [...byList].map(([list, items]) => ({ list, items })),
    plain,
  };
}
function outermostListTargets(targets: readonly ListTarget[]) {
  return targets.filter(
    (target) =>
      !targets.some(
        (other) =>
          other !== target &&
          other.items.some((item) => isWithin(target.list, item)),
      ),
  );
}
function setItemKind(item: StructureNode, target: ListStructure) {
  const itemKind = target === "taskList" ? "taskItem" : "listItem";
  if (item.getStructure() !== itemKind) {
    item.setStructure(itemKind);
    // 일반 목록에는 체크 의미가 없고, 일반 목록에서 체크 목록으로 들어갈 때는 미완료로 시작한다.
    item.setChecked(false);
  }
}
/** 선택 항목만 새 목록으로 분리하고 앞뒤의 비선택 목록을 같은 위치에 유지한다. */
function extractListRange(
  source: StructureNode,
  selectedItems: readonly StructureNode[],
  target: ListStructure,
): StructureNode | null {
  const children = source.getChildren();
  const indexes = selectedItems
    .map((item) => children.findIndex((child) => child.is(item)))
    .filter((index) => index >= 0);
  if (!indexes.length) return null;
  const first = Math.min(...indexes);
  const last = Math.max(...indexes);
  const range = children
    .slice(first, last + 1)
    .filter((child): child is StructureNode => child instanceof StructureNode);
  if (!range.length) return null;
  const sourceKind = source.getStructure();
  if (!isListStructure(sourceKind)) return null;
  if (first === 0 && last === children.length - 1) {
    source.setStructure(target);
    for (const item of range) setItemKind(item, target);
    return source;
  }
  const trailing = children.slice(last + 1);
  const after = trailing.length
    ? $createStructure(sourceKind).append(...trailing)
    : null;
  const replacement = $createStructure(target);
  if (first === 0) source.insertBefore(replacement);
  else source.insertAfter(replacement);
  if (after) replacement.insertAfter(after);
  for (const item of range) {
    setItemKind(item, target);
    replacement.append(item);
  }
  if (source.getChildrenSize() === 0) source.remove();
  return replacement;
}
function convertToList(blocks: readonly LexicalNode[], target: ListStructure) {
  const { targets, plain } = collectListTargets(blocks);
  for (const { list, items } of outermostListTargets(targets)) {
    if (list.getStructure() !== target) extractListRange(list, items, target);
  }
  const byParent = new Map<LexicalNode, LexicalNode[]>();
  for (const block of plain) {
    const parent = block.getParent();
    if (!parent) continue;
    const siblings = byParent.get(parent) ?? [];
    siblings.push(block);
    byParent.set(parent, siblings);
  }
  for (const siblings of byParent.values()) {
    const wrapper = $createStructure(target);
    siblings[0].insertBefore(wrapper);
    for (const block of siblings)
      wrapper.append(
        $createStructure(
          target === "taskList" ? "taskItem" : "listItem",
        ).append(block),
      );
  }
}
function convertToBlockquote(blocks: readonly LexicalNode[]) {
  const handled = new Set<LexicalNode>();
  for (const block of blocks) {
    if (nearestStructure(block, ["blockquote"])) handled.add(block);
  }
  const eligible = blocks.filter((block) => !handled.has(block));
  const { targets } = collectListTargets(eligible);
  const listTargets = outermostListTargets(
    targets.filter(({ items }) => items.length >= 2),
  );
  // 구조를 옮기기 전에 부모 범위에 포함된 모든 선택 block을 확정한다.
  for (const block of eligible)
    if (
      listTargets.some(({ items }) =>
        items.some((item) => isWithin(block, item)),
      )
    )
      handled.add(block);
  // 한 항목 안의 문단은 그 항목 안에서 인용으로 묶는다. 여러 항목 선택만 목록 범위를 보존한다.
  for (const { list, items } of listTargets) {
    const kind = list.getStructure();
    if (!isListStructure(kind)) continue;
    const segment = extractListRange(list, items, kind);
    if (!segment) continue;
    const quote = $createStructure("blockquote");
    segment.insertBefore(quote);
    quote.append(segment);
  }
  const byParent = new Map<LexicalNode, LexicalNode[]>();
  for (const block of blocks) {
    if (handled.has(block)) continue;
    const parent = block.getParent();
    if (!parent) continue;
    const siblings = byParent.get(parent) ?? [];
    siblings.push(block);
    byParent.set(parent, siblings);
  }
  for (const siblings of byParent.values()) {
    const wrapper = $createStructure("blockquote");
    siblings[0].insertBefore(wrapper);
    wrapper.append(...siblings);
  }
}
function Tools({ change, disabled }: Pick<Props, "change" | "disabled">) {
  const [editor] = useLexicalComposerContext();
  const callback = useRef(change);
  const selectionRef = useRef<RangeSelection | null>(null);
  const [notice, setNotice] = useState(false);
  const [taskChecked, setTaskChecked] = useState<boolean | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  useEffect(() => {
    callback.current = change;
  }, [change]);
  useEffect(() => {
    editor.setEditable(!disabled);
  }, [editor, disabled]);
  useEffect(() => {
    let previous = editor
      .getEditorState()
      .read(() => JSON.stringify($exportRich()));
    return mergeRegister(
      editor.registerUpdateListener(
        ({ editorState, dirtyElements, dirtyLeaves }) => {
          editorState.read(() => {
            const selection = $getSelection();
            if ($isRangeSelection(selection))
              selectionRef.current = selection.clone();
            let task: LexicalNode | null = $isRangeSelection(selection)
              ? selection.anchor.getNode()
              : null;
            while (
              task &&
              !(
                task instanceof StructureNode &&
                task.getStructure() === "taskItem"
              )
            )
              task = task.getParent();
            setTaskChecked(
              task instanceof StructureNode ? task.getChecked() : null,
            );
            if ($isRangeSelection(selection)) {
              const active: string[] = formats.filter((f) =>
                selection.hasFormat(f),
              );
              // 커서가 든 문단과 상위 인용·목록의 서식을 함께 보여준다.
              for (
                let node: LexicalNode | null = selection.anchor.getNode();
                node;
                node = node.getParent()
              ) {
                if (["paragraph", "heading"].includes(node.getType()))
                  active.push(node.getType());
                if (node instanceof StructureNode)
                  active.push(node.getStructure());
              }
              if (task instanceof StructureNode && task.getChecked())
                active.push("toggleTask");
              setSelected(active);
            }
            if (!dirtyElements.size && !dirtyLeaves.size) return;
            const ast = $exportRich();
            const next = JSON.stringify(ast);
            // 선택 이동·초기 설치·동일한 저장 응답은 초안을 새 Set으로 만들지 않는다.
            if (next !== previous) {
              previous = next;
              callback.current(ast);
            }
          });
        },
      ),
      editor.registerCommand(
        FORMAT_TEXT_COMMAND,
        (format) => !formats.includes(format as (typeof formats)[number]),
        COMMAND_PRIORITY_CRITICAL,
      ),
      editor.registerCommand(
        KEY_ENTER_COMMAND,
        (event) => {
          if (editor.isComposing() || event?.isComposing || event?.shiftKey)
            return false;
          const selection = $getSelection();
          if (!$isRangeSelection(selection)) return false;
          let block: LexicalNode | null = selection.anchor.getNode();
          while (
            block &&
            block.getType() !== "paragraph" &&
            block.getType() !== "heading"
          )
            block = block.getParent();
          const item = block?.getParent();
          if (
            !(item instanceof StructureNode) ||
            !["listItem", "taskItem"].includes(item.getStructure())
          )
            return false;
          event?.preventDefault();
          if (item.getTextContent() === "") {
            const list = item.getParentOrThrow();
            const paragraph = $createParagraphNode();
            const following = item.getNextSiblings();
            list.insertAfter(paragraph);
            if (following.length && list instanceof StructureNode)
              paragraph.insertAfter(
                $createStructure(list.getStructure()).append(...following),
              );
            item.remove();
            if (list.getChildrenSize() === 0) list.remove();
            paragraph.select();
          } else {
            const paragraph = selection.insertParagraph();
            if (paragraph) {
              const following = paragraph.getNextSiblings();
              const next = $createStructure(item.getStructure());
              item.insertAfter(next);
              next.append(paragraph, ...following);
              paragraph.selectStart();
            }
          }
          return true;
        },
        COMMAND_PRIORITY_CRITICAL,
      ),
      editor.registerCommand(
        PASTE_COMMAND,
        (event) => {
          event?.preventDefault();
          if (!editor.isEditable()) return true;
          const transfer =
            event && "clipboardData" in event ? event.clipboardData : null;
          const plain = transfer?.getData("text/plain");
          const selection = $getSelection();
          if (plain && $isRangeSelection(selection)) {
            selection.insertRawText(plain);
            setNotice(false);
          } else setNotice(true);
          return true;
        },
        COMMAND_PRIORITY_CRITICAL,
      ),
      editor.registerCommand(
        DROP_COMMAND,
        (event) => {
          event.preventDefault();
          if (!editor.isEditable()) return true;
          const plain = event.dataTransfer?.getData("text/plain");
          const selection = $getSelection();
          if (plain && $isRangeSelection(selection)) {
            selection.insertRawText(plain);
            setNotice(false);
          } else setNotice(true);
          return true;
        },
        COMMAND_PRIORITY_CRITICAL,
      ),
    );
  }, [editor]);
  const restoreSelection = () => {
    if (!$isRangeSelection($getSelection()) && selectionRef.current)
      $setSelection(selectionRef.current.clone());
  };
  const command = <T,>(type: LexicalCommand<T>, payload: T) =>
    editor.update(() => {
      restoreSelection();
      editor.dispatchCommand(type, payload);
    });
  const block = (kind: "paragraph" | "heading" | Structure) =>
    editor.update(() => {
      restoreSelection();
      const selection = $getSelection();
      if (!$isRangeSelection(selection)) return;
      if (kind === "paragraph" || kind === "heading") {
        $setBlocksType(selection, () =>
          kind === "paragraph"
            ? $createParagraphNode()
            : $createHeadingNode("h3"),
        );
        return;
      }
      const blocks = selectedBlocks(selection);
      if (!blocks.length) return;
      if (kind === "blockquote") convertToBlockquote(blocks);
      else if (isListStructure(kind)) convertToList(blocks, kind);
    });
  const toggleTask = () =>
    editor.update(() => {
      restoreSelection();
      const selection = $getSelection();
      if (!$isRangeSelection(selection)) return;
      let node: LexicalNode | null = selection.anchor.getNode();
      while (
        node &&
        !(node instanceof StructureNode && node.getStructure() === "taskItem")
      )
        node = node.getParent();
      if (node instanceof StructureNode) node.setChecked(!node.getChecked());
    });
  return (
    <>
      <RichToolbar
        disabled={disabled}
        selected={selected}
        taskAvailable={taskChecked !== null}
        action={(kind) => {
          switch (kind) {
            case "bold":
            case "italic":
            case "underline":
            case "strikethrough":
              command(FORMAT_TEXT_COMMAND, kind);
              break;
            case "toggleTask":
              toggleTask();
              break;
            case "undo":
              editor.dispatchCommand(UNDO_COMMAND, undefined);
              break;
            case "redo":
              editor.dispatchCommand(REDO_COMMAND, undefined);
              break;
            default:
              block(kind);
          }
        }}
      />
      {notice && <p role="status">{text("rich.plainOnly")}</p>}
    </>
  );
}
/** 각 열린 owner/field의 React 수명 안에서 editor와 history를 유지한다. */
export function RichEditor(props: Props) {
  return (
    <LexicalComposer
      initialConfig={{
        namespace: props.id,
        nodes: richNodes,
        editable: !props.disabled,
        theme: {
          text: {
            bold: "rich-bold",
            italic: "rich-italic",
            underline: "rich-underline",
            strikethrough: "rich-strike",
            underlineStrikethrough: "rich-underline-strike",
          },
        },
        editorState: () => $installRich(props.value),
        onError: (error) => {
          throw error;
        },
      }}
    >
      <div className="rich-editor">
        <Tools change={props.change} disabled={props.disabled} />
        <RichTextPlugin
          contentEditable={
            <ContentEditable
              id={props.id}
              aria-label={props.label}
              aria-invalid={props.invalid}
              aria-description={props.guide}
              className="rich-input"
            />
          }
          placeholder={null}
          ErrorBoundary={LexicalErrorBoundary}
        />
        <HistoryPlugin />
        <SpellReplaceListener />
        <TaskClicks />
      </div>
    </LexicalComposer>
  );
}
function TaskClicks() {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    const click = (event: MouseEvent) => {
      if (!editor.isEditable() || !(event.target instanceof HTMLElement))
        return;
      const item = event.target.closest<HTMLElement>("[data-rich-task]");
      if (!item || event.clientX > item.getBoundingClientRect().left) return;
      event.preventDefault();
      editor.update(() => {
        const node = $getNodeByKey(item.dataset.richTask ?? "");
        if (node instanceof StructureNode) node.setChecked(!node.getChecked());
      });
    };
    return editor.registerRootListener((root, previous) => {
      previous?.removeEventListener("click", click);
      root?.addEventListener("click", click);
    });
  }, [editor]);
  return null;
}
