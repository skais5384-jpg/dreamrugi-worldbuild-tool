import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import {
  Menu,
  MenuTrigger,
  MenuPopover,
  MenuList,
  MenuItem,
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogContent,
  DialogActions,
  Tooltip,
} from "@fluentui/react-components";
import { InlineNotice } from "../ui/InlineNotice";
import {
  Add20Regular,
  ArrowMove20Regular,
  ChevronRight16Regular,
  ChevronDown16Regular,
  Delete20Regular,
  DocumentPdf20Regular,
  MoreHorizontal16Regular,
  Warning16Filled,
  Info16Regular,
} from "@fluentui/react-icons";
import { Button, Select } from "../ui/Controls";
import type { DocumentController } from "./documentController";
import { documentPlacement, type Placement } from "./documentPlacement";
import { text } from "../strings";
import type { DocumentIssue } from "./DocumentIssues";
import type { SvnStatus, SvnStatusEntry } from "./svnClient";
import { overlayForEntry } from "./svnOverlay";
import { OverlayIcon, overlayNames } from "./SvnToolbar";

export const DOCUMENT_TREE_ROW_HEIGHT = 24;

export function DocumentTree({
  controller,
  locked,
  structureLocked,
  createChild,
  openDocument,
  exportPdf,
  viewportHeight,
  issues = new Map(),
  svnStatus,
}: {
  controller: DocumentController;
  locked: boolean;
  structureLocked: boolean;
  createChild: (id: string) => void;
  openDocument: (id: string) => void;
  exportPdf?: (id: string) => void;
  viewportHeight: number;
  issues?: ReadonlyMap<string, DocumentIssue[]>;
  svnStatus?: SvnStatus | null;
}) {
  const state = controller.snapshot();
  const [notice, setNotice] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const selectionAnchor = useRef<string | null>(null);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(false), 5000);
    return () => clearTimeout(timer);
  }, [notice]);
  const list = state.list;
  const drag = useRef<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const expand = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (expand.current) clearTimeout(expand.current);
    },
    [],
  );
  const [drop, setDrop] = useState<{
    id: string;
    position: Placement;
    valid: boolean;
  } | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const [moving, setMoving] = useState<string | null>(null);
  const [destination, setDestination] = useState("");
  const [position, setPosition] = useState<Placement>("inside");
  const names = new Map(list?.documents.map((d) => [d.id, d.name]));
  const svnEntries = new Map<string, SvnStatusEntry>();
  for (const entry of svnStatus?.entries ?? []) {
    const match = entry.path
      .replace(/\\/gu, "/")
      .match(/(?:^|\/)documents\/([0-9a-f-]{36})\.json$/iu);
    if (match) svnEntries.set(match[1].toLowerCase(), entry);
  }
  const rows: { id: string; depth: number }[] = [];
  const rootOrder = [...(list?.layout.rootOrder ?? [])];
  for (const id of list?.unplaced ?? [])
    if (!rootOrder.includes(id)) rootOrder.push(id);
  for (const document of list?.documents ?? [])
    if (!list?.layout.nodes[document.id] && !rootOrder.includes(document.id))
      rootOrder.push(document.id);
  const pending = rootOrder.map((id) => ({ id, depth: 0 })).reverse();
  const seen = new Set<string>();
  while (pending.length) {
    const row = pending.pop()!;
    if (seen.has(row.id)) continue;
    seen.add(row.id);
    rows.push(row);
    if (!state.ui.collapsed.includes(row.id)) {
      const children = list?.layout.nodes[row.id]?.childOrder ?? [];
      for (let i = children.length - 1; i >= 0; i--)
        pending.push({ id: children[i], depth: row.depth + 1 });
    }
  }
  const currentIds = new Set(rows.map((row) => row.id));
  const currentSelection = selected.filter((id) => currentIds.has(id));
  const choose = (
    id: string,
    event: React.MouseEvent | React.KeyboardEvent,
  ) => {
    const order = rows.map((row) => row.id);
    if (
      event.shiftKey &&
      selectionAnchor.current &&
      order.includes(selectionAnchor.current)
    ) {
      const from = order.indexOf(selectionAnchor.current);
      const to = order.indexOf(id);
      const range = order.slice(Math.min(from, to), Math.max(from, to) + 1);
      setSelected(
        event.ctrlKey ? [...new Set([...currentSelection, ...range])] : range,
      );
      return;
    }
    selectionAnchor.current = id;
    if (event.ctrlKey || event.metaKey) {
      setSelected(
        currentSelection.includes(id)
          ? currentSelection.filter((current) => current !== id)
          : [...currentSelection, id],
      );
      return;
    }
    setSelected([id]);
    openDocument(id);
  };
  const trashDocuments = async (selection: string[]) => {
    const depth = new Map(rows.map((row) => [row.id, row.depth]));
    const targets = selection
      .filter((id) => list?.layout.nodes[id]?.state === "active")
      .sort((left, right) => (depth.get(right) ?? 0) - (depth.get(left) ?? 0));
    for (const document of targets)
      await controller.mutate({ kind: "trash", document });
    setSelected([]);
    selectionAnchor.current = null;
  };
  const rowHeight = DOCUMENT_TREE_ROW_HEIGHT;
  const virtual = rows.length > 400;
  const first = virtual
    ? Math.max(0, Math.floor(state.ui.scroll / rowHeight) - 12)
    : 0;
  const last = virtual
    ? Math.min(
        rows.length,
        Math.ceil((state.ui.scroll + viewportHeight) / rowHeight) + 12,
      )
    : rows.length;
  const visibleRows = rows
    .slice(first, last)
    .map((row, offset) => ({ row, index: first + offset, pinned: false }));
  const draggingIndex = dragging
    ? rows.findIndex((row) => row.id === dragging)
    : -1;
  if (draggingIndex >= 0 && (draggingIndex < first || draggingIndex >= last))
    visibleRows.push({
      row: rows[draggingIndex],
      index: draggingIndex,
      pinned: true,
    });
  const pendingFocus = useRef<string | null>(null);
  useEffect(() => {
    const id = pendingFocus.current;
    if (!id) return;
    const target = document.getElementById("tree-name-" + id);
    if (target) {
      pendingFocus.current = null;
      target.focus();
    }
  }, [state.ui.scroll, first, last]);
  function focusRow(id: string, index: number) {
    const target = document.getElementById("tree-name-" + id);
    if (target) {
      target.focus();
      return;
    }
    pendingFocus.current = id;
    controller.scroll(index * rowHeight);
  }
  const clearDrop = useCallback(() => {
    setDrop(null);
    if (expand.current) clearTimeout(expand.current);
    expand.current = null;
  }, []);
  const finishDrag = useCallback(() => {
    drag.current = null;
    setDragging(null);
    clearDrop();
  }, [clearDrop]);
  useEffect(() => {
    if (!dragging) return;
    /**
     * WebView나 자동화가 원본 행의 dragend를 누락해도 Esc/창 이탈/외부 drop은
     * 앱 내부 owner를 버린다. 늦게 도착한 drop은 drag.current가 비어 저장하지 않는다.
     */
    const cancelWithEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      finishDrag();
    };
    document.addEventListener("keydown", cancelWithEscape, true);
    document.addEventListener("dragend", finishDrag);
    document.addEventListener("drop", finishDrag);
    window.addEventListener("blur", finishDrag);
    return () => {
      document.removeEventListener("keydown", cancelWithEscape, true);
      document.removeEventListener("dragend", finishDrag);
      document.removeEventListener("drop", finishDrag);
      window.removeEventListener("blur", finishDrag);
    };
  }, [dragging, finishDrag]);
  const move =
    moving && list
      ? documentPlacement(list.layout, moving, destination || null, position)
      : null;
  return (
    <>
      {notice && (
        <div className="document-structure-notice" role="status">
          {text("documentEdit.structureLocked")}
        </div>
      )}
      <span className="sr-only" role="status">
        {drop &&
          (drop.valid
            ? `${names.get(drop.id)}: ${text(`documents.${drop.position}`)}`
            : text("documents.invalidMove"))}
      </span>
      {!!currentSelection.length && (
        <div className="document-tree-selection-actions">
          <span>
            {text("documents.selectedCount", {
              count: String(currentSelection.length),
            })}
          </span>
        </div>
      )}
      <ul
        className="document-tree-list"
        aria-label={text("documents.list")}
        style={
          {
            "--document-tree-row-height": `${rowHeight}px`,
            ...(virtual
              ? {
                  paddingTop: first * rowHeight,
                  paddingBottom: (rows.length - last) * rowHeight,
                }
              : {}),
          } as CSSProperties
        }
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null))
            clearDrop();
        }}
      >
        {visibleRows.map(
          ({ row: { id, depth }, index: absoluteIndex, pinned }) => {
            const persistedNode = list!.layout.nodes[id];
            const node = persistedNode ?? {
              parentId: null,
              childOrder: [],
              state: "active" as const,
              trash: null,
            };
            const name = names.get(id) ?? text("documents.unknownName");
            const svnEntry = svnEntries.get(id.toLowerCase());
            const svnOverlay = svnEntry ? overlayForEntry(svnEntry) : null;
            const path = [name];
            const ancestors = new Set([id]);
            let parent = node.parentId;
            while (parent && !ancestors.has(parent)) {
              ancestors.add(parent);
              path.unshift(names.get(parent) ?? text("documents.unknownName"));
              parent = list!.layout.nodes[parent]?.parentId ?? null;
            }
            const fullName = path.join(" / ");
            const collapsed = state.ui.collapsed.includes(id);
            const problemDescriptions = (issues.get(id) ?? []).map(
              (issue) => issue.message,
            );
            const contextSelection =
              currentSelection.includes(id) && currentSelection.length > 1
                ? currentSelection
                : [id];
            const multipleContext = contextSelection.length > 1;
            return (
              <li
                key={id}
                className={
                  "document-depth-" +
                  Math.min(depth, 8) +
                  (pinned ? " drag-source-pinned" : "") +
                  (dragging === id ? " drag-source-active" : "")
                }
                style={{ paddingInlineStart: depth * 12 }}
                aria-posinset={absoluteIndex + 1}
                aria-setsize={rows.length}
              >
                <Menu
                  open={!!persistedNode && menu === id}
                  onOpenChange={(_, data) =>
                    setMenu(persistedNode && data.open ? id : null)
                  }
                  openOnContext={!!persistedNode}
                >
                  <MenuTrigger disableButtonEnhancement>
                    <div
                      className={
                        "document-tree-row" +
                        (state.ui.active === id ? " current" : "") +
                        (currentSelection.includes(id) ? " selected" : "") +
                        (drop?.id === id
                          ? " drop-" + (drop.valid ? drop.position : "invalid")
                          : "")
                      }
                      draggable={!structureLocked && !!persistedNode}
                      onDragStart={(e) => {
                        if (structureLocked || !persistedNode) {
                          e.preventDefault();
                          return;
                        }
                        drag.current = id;
                        setDragging(id);
                        e.dataTransfer.effectAllowed = "move";
                        e.dataTransfer.setData("text/plain", id);
                      }}
                      onDragEnd={finishDrag}
                      onDragOver={(e) => {
                        if (structureLocked || !drag.current || !list) return;
                        e.preventDefault();
                        const rect = e.currentTarget.getBoundingClientRect();
                        const fraction = (e.clientY - rect.top) / rect.height;
                        const position: Placement =
                          fraction < 0.25
                            ? "before"
                            : fraction > 0.75
                              ? "after"
                              : "inside";
                        const valid = !!documentPlacement(
                          list.layout,
                          drag.current,
                          id,
                          position,
                        );
                        e.dataTransfer.dropEffect = valid ? "move" : "none";
                        if (drop?.id !== id || drop.position !== position) {
                          clearDrop();
                          setDrop({ id, position, valid });
                          if (valid && position === "inside" && collapsed)
                            expand.current = setTimeout(
                              () => controller.collapse(id),
                              650,
                            );
                        }
                        const panel = e.currentTarget.closest(".document-tree");
                        if (panel) {
                          const bounds = panel.getBoundingClientRect();
                          if (e.clientY < bounds.top + rowHeight)
                            panel.scrollTop -= rowHeight / 2;
                          if (e.clientY > bounds.bottom - rowHeight)
                            panel.scrollTop += rowHeight / 2;
                        }
                      }}
                      onDrop={(e) => {
                        e.preventDefault();
                        const edit =
                          drag.current && drop?.id === id && list
                            ? documentPlacement(
                                list.layout,
                                drag.current,
                                id,
                                drop.position,
                              )
                            : null;
                        finishDrag();
                        if (!structureLocked && edit)
                          void controller.mutate(edit);
                      }}
                      onKeyDown={(e) => {
                        if (
                          (e.key === " " || e.key === "Enter") &&
                          (e.ctrlKey || e.metaKey || e.shiftKey) &&
                          e.target instanceof HTMLElement &&
                          e.target.classList.contains("tree-name")
                        ) {
                          e.preventDefault();
                          choose(id, e);
                          return;
                        }
                        if (
                          e.key === "ContextMenu" ||
                          (e.shiftKey && e.key === "F10")
                        ) {
                          e.preventDefault();
                          setMenu(id);
                          return;
                        }
                        if (
                          e.altKey ||
                          e.ctrlKey ||
                          e.metaKey ||
                          !(e.target instanceof HTMLElement) ||
                          !e.target.classList.contains("tree-name")
                        )
                          return;
                        const index = absoluteIndex;
                        let next: string | undefined;
                        if (e.key === "ArrowDown") next = rows[index + 1]?.id;
                        if (e.key === "ArrowUp") next = rows[index - 1]?.id;
                        if (e.key === "Home") next = rows[0]?.id;
                        if (e.key === "End") next = rows[rows.length - 1]?.id;
                        if (e.key === "ArrowRight" && node.childOrder.length) {
                          if (collapsed) controller.collapse(id);
                          else next = node.childOrder[0];
                        }
                        if (e.key === "ArrowLeft") {
                          if (!collapsed && node.childOrder.length)
                            controller.collapse(id);
                          else next = node.parentId ?? undefined;
                        }
                        if (
                          [
                            "ArrowDown",
                            "ArrowUp",
                            "ArrowLeft",
                            "ArrowRight",
                            "Home",
                            "End",
                          ].includes(e.key)
                        )
                          e.preventDefault();
                        if (next) {
                          const nextIndex = rows.findIndex(
                            (row) => row.id === next,
                          );
                          if (nextIndex >= 0) focusRow(next, nextIndex);
                        }
                      }}
                    >
                      {node.childOrder.length ? (
                        <Button
                          type="button"
                          appearance="subtle"
                          className="tree-expand"
                          disabled={
                            locked || !persistedNode || !node.childOrder.length
                          }
                          aria-label={text("documents.collapse")}
                          aria-expanded={!collapsed}
                          onClick={() => controller.collapse(id)}
                          icon={
                            node.childOrder.length ? (
                              collapsed ? (
                                <ChevronRight16Regular />
                              ) : (
                                <ChevronDown16Regular />
                              )
                            ) : undefined
                          }
                        />
                      ) : (
                        <span className="tree-expand-slot" aria-hidden="true" />
                      )}
                      <Tooltip
                        content={
                          svnEntry?.lockOwner
                            ? `${fullName} · ${text("svn.lockOwner", { owner: svnEntry.lockOwner })}`
                            : fullName
                        }
                        relationship="label"
                      >
                        <Button
                          type="button"
                          appearance="subtle"
                          id={"tree-name-" + id}
                          className="tree-name"
                          aria-selected={currentSelection.includes(id)}
                          aria-current={
                            state.ui.active === id ? "page" : undefined
                          }
                          disabled={locked}
                          onClick={(event) => choose(id, event)}
                        >
                          {name}
                        </Button>
                      </Tooltip>
                      {svnOverlay && (
                        <Tooltip
                          content={overlayNames[svnOverlay]}
                          relationship="description"
                        >
                          <span
                            className="document-tree-svn-overlay"
                            role="img"
                            aria-label={overlayNames[svnOverlay]}
                          >
                            <OverlayIcon overlay={svnOverlay} />
                          </span>
                        </Tooltip>
                      )}
                      {!!problemDescriptions.length && (
                        <Tooltip
                          content={problemDescriptions.join("\n")}
                          relationship="description"
                        >
                          <span
                            className="document-tree-warning"
                            role="img"
                            aria-label={problemDescriptions.join(" ")}
                          >
                            {(issues.get(id) ?? []).every(
                              (issue) => issue.reason === "inspection_waiting",
                            ) ? (
                              <Info16Regular aria-hidden="true" />
                            ) : (
                              <Warning16Filled aria-hidden="true" />
                            )}
                          </span>
                        </Tooltip>
                      )}
                      {!!persistedNode && (
                        <Button
                          type="button"
                          appearance="subtle"
                          className="tree-menu"
                          aria-label={text("documents.menu")}
                          onClick={() => setMenu(id)}
                          icon={<MoreHorizontal16Regular />}
                        />
                      )}
                    </div>
                  </MenuTrigger>
                  <MenuPopover>
                    <MenuList>
                      {!!persistedNode && (
                        <>
                          {!multipleContext && (
                            <>
                              {exportPdf && (
                                <MenuItem
                                  icon={<DocumentPdf20Regular />}
                                  disabled={locked}
                                  onClick={() => exportPdf(id)}
                                >
                                  {text("pdf.command")}
                                </MenuItem>
                              )}
                              <MenuItem
                                icon={<Add20Regular />}
                                disabled={locked}
                                onClick={() => createChild(id)}
                              >
                                {text("documents.newChild")}
                              </MenuItem>
                              <MenuItem
                                icon={<ArrowMove20Regular />}
                                disabled={locked}
                                onClick={() => {
                                  if (structureLocked) {
                                    setNotice(true);
                                    return;
                                  }
                                  setMoving(id);
                                  setDestination("");
                                  setPosition("inside");
                                }}
                              >
                                {text("documents.move")}
                              </MenuItem>
                            </>
                          )}
                          <MenuItem
                            icon={<Delete20Regular />}
                            className="destructive-menu-item"
                            disabled={
                              structureLocked ||
                              contextSelection.some(
                                (target) =>
                                  list?.layout.nodes[target]?.childOrder.some(
                                    (child) =>
                                      !contextSelection.includes(child),
                                  ) ?? false,
                              )
                            }
                            onClick={() =>
                              void trashDocuments(contextSelection)
                            }
                          >
                            {text("documents.toTrash")}
                          </MenuItem>
                        </>
                      )}
                    </MenuList>
                  </MenuPopover>
                </Menu>
              </li>
            );
          },
        )}
      </ul>
      <Dialog
        open={moving !== null}
        onOpenChange={(_, data) => {
          if (!data.open) setMoving(null);
        }}
      >
        <DialogSurface>
          <DialogBody>
            <DialogTitle>{text("documents.move")}</DialogTitle>
            <DialogContent>
              <p>{moving && names.get(moving)}</p>
              <Select
                aria-label={text("documents.destination")}
                value={destination}
                onChange={(e) => setDestination(e.target.value)}
              >
                <option value="">{text("documents.root")}</option>
                {list?.documents
                  .filter(
                    (d) =>
                      d.id !== moving &&
                      list.layout.nodes[d.id]?.state === "active",
                  )
                  .map((d) => (
                    <option key={d.id} value={d.id}>
                      {d.name}
                    </option>
                  ))}
              </Select>
              {destination && (
                <Select
                  aria-label={text("documents.position")}
                  value={position}
                  onChange={(e) => setPosition(e.target.value as Placement)}
                >
                  {(["before", "after", "inside"] as const).map((p) => (
                    <option key={p} value={p}>
                      {text(`documents.${p}`)}
                    </option>
                  ))}
                </Select>
              )}
              {!move && (
                <InlineNotice kind="error">
                  {text("documents.invalidMove")}
                </InlineNotice>
              )}
            </DialogContent>
            <DialogActions>
              <Button
                type="button"
                appearance="primary"
                disabled={structureLocked || !move}
                onClick={() => {
                  if (move) void controller.mutate(move);
                  setMoving(null);
                }}
              >
                {text("documents.move")}
              </Button>
              <Button type="button" onClick={() => setMoving(null)}>
                {text("documents.cancel")}
              </Button>
            </DialogActions>
          </DialogBody>
        </DialogSurface>
      </Dialog>
    </>
  );
}
