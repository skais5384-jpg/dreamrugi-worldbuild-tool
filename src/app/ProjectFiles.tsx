import {
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Badge,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Input,
  Menu,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Select,
  Tooltip,
} from "@fluentui/react-components";
import {
  ArrowClockwise20Regular,
  ArrowUndo20Regular,
  Delete20Regular,
  Edit20Regular,
  FolderOpen20Regular,
  MoreHorizontal16Regular,
  PanelLeftContract20Regular,
  PanelLeftExpand20Regular,
} from "@fluentui/react-icons";
import { Button } from "../ui/Controls";
import { EmptyState } from "../ui/EmptyState";
import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";
import { text } from "../strings";
import type { TemplateController } from "./controller";
import type { DocumentController } from "./documentController";
import type { SvnStatus } from "./svnClient";
import { SvnItemStatus } from "./SvnItemStatus";
import { NAVIGATION_DEFAULT, NAVIGATION_MAX } from "./navigationSizing";
import "./ProjectFiles.css";

type Mode = "resources" | "trash";
type Row = {
  key: string;
  id: string;
  kind: "resource" | "template" | "document";
  name: string;
  size: number | null;
  status: string;
  protected: boolean;
  template?: string | null;
  reason?: string | null;
};

export function ProjectFiles({
  mode,
  shell,
  documents,
  svnStatus,
}: {
  mode: Mode;
  shell: TemplateController;
  documents: DocumentController;
  svnStatus?: SvnStatus | null;
}) {
  const app = useSyncExternalStore(shell.subscribe, shell.snapshot);
  const documentState = useSyncExternalStore(
    documents.subscribe,
    documents.snapshot,
  );
  const [query, setQuery] = useState("");
  const [type, setType] = useState("all");
  const [usage, setUsage] = useState("all");
  const [sort, setSort] = useState("name");
  const [selection, setSelection] = useState<{
    mode: Mode;
    keys: string[];
  }>({ mode, keys: [] });
  const [rename, setRename] = useState<{ id: string; name: string } | null>(
    null,
  );
  const [purgeConfirm, setPurgeConfirm] = useState<Row[] | null>(null);
  const [operationNotice, setOperationNotice] = useState<{
    kind: "info" | "warning";
    text: string;
  } | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const [navigationWidth, setNavigationWidth] = useState(NAVIGATION_DEFAULT);
  const [navigationCollapsed, setNavigationCollapsed] = useState(false);
  const resizeStart = useRef<{ x: number; width: number } | null>(null);
  const anchor = useRef<string | null>(null);
  useEffect(() => {
    shell.showFileManager(mode);
    anchor.current = null;
  }, [mode, shell]);
  const inspection = app.health?.inspection ?? app.assetInspection;
  const allRows = useMemo<Row[]>(() => {
    if (mode === "resources") {
      return (inspection?.rows ?? [])
        .filter(
          (row) =>
            type === "all" ||
            (type === "images") ===
              /\.(png|jpe?g|gif|webp|bmp)$/i.test(row.name ?? ""),
        )
        .filter((row) => usage === "all" || row.status === usage)
        .map((row) => ({
          key: `resource:${row.id}`,
          id: row.id,
          kind: "resource" as const,
          name: row.name ?? text("resources.unknownName"),
          size: row.size,
          status: row.status,
          protected: row.status !== "unused",
          reason: row.reason,
        }));
    }
    const assetRows = (inspection?.trash ?? []).map((row) => ({
      key: `resource:${row.id}`,
      id: row.id,
      kind: "resource" as const,
      name: row.name,
      size: row.size,
      status: "in_trash",
      protected: row.protected,
      reason: row.reason,
    }));
    const templateRows = (inspection?.deletedTemplates ?? []).map((row) => ({
      key: `template:${row.id}`,
      id: row.id,
      kind: "template" as const,
      name: row.name,
      size: row.size,
      status: "in_trash",
      protected: !row.removable,
      reason: row.reason,
    }));
    const list = documentState.list;
    const documentRows = (list?.documents ?? [])
      .filter((row) => list?.layout.nodes[row.id]?.state === "trashed")
      .map((row) => ({
        key: `document:${row.id}`,
        id: row.id,
        kind: "document" as const,
        name: row.name,
        size: null,
        status: "in_trash",
        protected: false,
        template: row.template,
      }));
    return [...documentRows, ...templateRows, ...assetRows];
  }, [documentState.list, inspection, mode, type, usage]);
  const rows = useMemo(
    () =>
      mode === "trash" && type !== "all"
        ? allRows.filter((row) => row.kind === type)
        : allRows,
    [allRows, mode, type],
  );
  const existingKeys = new Set(rows.map((row) => row.key));
  const currentSelection =
    selection.mode === mode
      ? selection.keys.filter((key) => existingKeys.has(key))
      : [];
  useEffect(() => {
    const target = app.fileManagerTarget;
    if (!target || target.surface !== mode) return;
    if (query || type !== "all" || (mode === "resources" && usage !== "all")) {
      window.setTimeout(() => {
        setQuery("");
        setType("all");
        setUsage("all");
      }, 0);
      return;
    }
    const targetKeys = target.keys.filter((key) =>
      rows.some((row) => row.key === key),
    );
    if (!targetKeys.length) {
      window.setTimeout(() => {
        setQuery("");
        setType("all");
        setUsage("all");
      }, 0);
      return;
    }
    window.setTimeout(() => {
      setSelection({ mode, keys: targetKeys });
      anchor.current = targetKeys[0] ?? null;
      document
        .getElementById(`project-file-${targetKeys[0]}`)
        ?.scrollIntoView({ block: "nearest" });
      document.getElementById(`project-file-${targetKeys[0]}`)?.focus();
    }, 0);
    shell.clearFileManagerTarget(mode);
  }, [app.fileManagerTarget, mode, query, rows, shell, type, usage]);
  const visible = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    const filtered = normalized
      ? rows.filter((row) => row.name.toLocaleLowerCase().includes(normalized))
      : [...rows];
    filtered.sort((left, right) =>
      sort === "type"
        ? (mode === "resources"
            ? fileExtension(left.name).localeCompare(fileExtension(right.name))
            : left.kind.localeCompare(right.kind)) ||
          left.name.localeCompare(right.name)
        : left.name.localeCompare(right.name),
    );
    return filtered;
  }, [mode, query, rows, sort]);
  const choose = (row: Row, event: React.MouseEvent | React.KeyboardEvent) => {
    const keys = visible.map((item) => item.key);
    let next: string[];
    if (event.shiftKey && anchor.current && keys.includes(anchor.current)) {
      const from = keys.indexOf(anchor.current);
      const to = keys.indexOf(row.key);
      const range = keys.slice(Math.min(from, to), Math.max(from, to) + 1);
      next = event.ctrlKey
        ? [...new Set([...currentSelection, ...range])]
        : range;
    } else if (event.ctrlKey) {
      next = currentSelection.includes(row.key)
        ? currentSelection.filter((key) => key !== row.key)
        : [...currentSelection, row.key];
      anchor.current = row.key;
    } else {
      next = [row.key];
      anchor.current = row.key;
    }
    setSelection({ mode, keys: next });
  };
  const openResource = (row: Row) => {
    if (row.kind !== "resource" || mode === "trash") return;
    void documents.media({ action: "asset_open", asset: row.id });
  };
  const selectedRows = rows.filter((row) => currentSelection.includes(row.key));
  const moveResources = (targets: Row[]) => {
    shell.setHealthAssets(
      targets
        .filter((row) => row.kind === "resource" && !row.protected)
        .map((row) => row.id),
    );
    void shell.moveSelectedAssetsToTrash();
  };
  const restoreRows = async (targets: Row[]) => {
    const editing = Object.keys(documentState.editors)
      .map(
        (id) =>
          documentState.list?.documents.find((document) => document.id === id)
            ?.name,
      )
      .filter((name): name is string => !!name);
    if (editing.length) {
      setOperationNotice({
        kind: "warning",
        text: text("trash.restoreBlockedByEditors", {
          names: editing.join(", "),
        }),
      });
      return;
    }
    let completed = 0;
    let failed = 0;
    let cleanup = 0;
    const resources = targets.filter((row) => row.kind === "resource");
    for (const row of resources) {
      const outcome = await shell.restoreAsset(row.id);
      if (!outcome) {
        failed += 1;
        continue;
      }
      completed += outcome.completedCount;
      failed += Math.max(
        outcome.failures.length,
        1 - outcome.completedCount - outcome.failures.length,
      );
      cleanup += outcome.cleanupRequired.length;
    }
    const templates = targets.filter((row) => row.kind === "template");
    const restoredTemplates = new Set<string>();
    for (const row of templates) {
      if (await shell.restoreTemplate(row.id)) {
        restoredTemplates.add(row.id);
        completed += 1;
      } else failed += 1;
    }
    const initiallyDeletedTemplates = new Set(
      inspection?.deletedTemplates.map((row) => row.id) ?? [],
    );
    const selectedDocuments = new Set(
      targets.filter((row) => row.kind === "document").map((row) => row.id),
    );
    const initialLayout = documents.snapshot().list?.layout;
    const docs = targets
      .filter((row) => row.kind === "document")
      .sort((left, right) => {
        const depth = (id: string) => {
          let current: string | null = id;
          let value = 0;
          const seen = new Set<string>();
          while (current && !seen.has(current)) {
            seen.add(current);
            const parent: string | null =
              initialLayout?.nodes[current]?.trash?.parentId ?? null;
            if (!parent || !selectedDocuments.has(parent)) break;
            current = parent;
            value += 1;
          }
          return value;
        };
        return depth(left.id) - depth(right.id);
      });
    for (const row of docs) {
      if (
        row.template &&
        initiallyDeletedTemplates.has(row.template) &&
        !restoredTemplates.has(row.template)
      ) {
        failed += 1;
        continue;
      }
      const current = documents.snapshot().list?.layout;
      const original = initialLayout?.nodes[row.id]?.trash;
      const parent = original?.parentId ?? null;
      const originalAvailable =
        parent === null || current?.nodes[parent]?.state === "active";
      const restored = await documents.mutate({
        kind: "restore",
        document: row.id,
        destination: originalAvailable
          ? null
          : { parent: null, index: current?.rootOrder.length ?? 0 },
      });
      if (restored) completed += 1;
      else failed += 1;
    }
    setSelection({ mode, keys: [] });
    setOperationNotice({
      kind: failed || cleanup ? "warning" : "info",
      text:
        failed || cleanup
          ? text("trash.restoreResult", {
              completed: String(completed),
              failed: String(failed),
              cleanup: String(cleanup),
            })
          : text("trash.restoreComplete"),
    });
  };
  const purgeRows = async (targets: Row[]) => {
    const resources = targets.filter(
      (row) => row.kind === "resource" && !row.protected,
    );
    const templates = targets.filter(
      (row) => row.kind === "template" && !row.protected,
    );
    const docs = targets.filter(
      (row) => row.kind === "document" && !row.protected,
    );
    let completed = 0;
    let failed = 0;
    let cleanup = 0;
    for (const row of docs) {
      if (await documents.mutate({ kind: "purge", document: row.id }))
        completed += 1;
      else failed += 1;
    }
    if (docs.length) await shell.inspectAssets();
    for (let offset = 0; offset < resources.length; offset += 100) {
      const ids = resources.slice(offset, offset + 100).map((row) => row.id);
      shell.setHealthTrash(ids);
      shell.showHealthPurgeConfirm("trash_selected");
      const outcome = await shell.purgeHealthSelection();
      if (!outcome) {
        failed += ids.length;
        await shell.inspectAssets();
        continue;
      }
      completed += outcome.completedCount;
      failed += Math.max(
        outcome.failures.length,
        ids.length - outcome.completedCount - outcome.failures.length,
      );
      cleanup += outcome.cleanupRequired.length;
    }
    for (let offset = 0; offset < templates.length; offset += 100) {
      const ids = templates.slice(offset, offset + 100).map((row) => row.id);
      shell.setHealthTemplates(ids);
      shell.showHealthPurgeConfirm("templates");
      const outcome = await shell.purgeHealthSelection();
      if (!outcome) {
        failed += ids.length;
        await shell.inspectAssets();
        continue;
      }
      completed += outcome.completedCount;
      failed += Math.max(
        outcome.failures.length,
        ids.length - outcome.completedCount - outcome.failures.length,
      );
      cleanup += outcome.cleanupRequired.length;
    }
    setSelection({ mode, keys: [] });
    const protectedCount = targets.filter((row) => row.protected).length;
    setOperationNotice({
      kind: protectedCount || failed || cleanup ? "warning" : "info",
      text:
        protectedCount || failed || cleanup
          ? text("trash.batchPartial", {
              completed: String(completed),
              protected: String(protectedCount),
              failed: String(failed),
              cleanup: String(cleanup),
            })
          : text("trash.batchComplete", { count: String(completed) }),
    });
  };
  return (
    <div
      className={
        "workspace project-files-workspace" +
        (navigationCollapsed ? " navigation-collapsed" : "")
      }
      style={
        {
          "--project-files-navigation-width": `${navigationWidth}px`,
        } as CSSProperties
      }
    >
      <aside
        className="panel project-files-panel"
        aria-label={text(
          mode === "resources" ? "resources.title" : "trash.title",
        )}
      >
        <header className="project-files-heading">
          <h2 hidden={navigationCollapsed}>
            {text(mode === "resources" ? "resources.title" : "trash.title")}
          </h2>
          <Tooltip
            content={text(
              navigationCollapsed
                ? "navigation.panelExpand"
                : "navigation.panelCollapse",
            )}
            relationship="label"
          >
            <Button
              type="button"
              appearance="subtle"
              aria-expanded={!navigationCollapsed}
              aria-controls="project-files-navigation-content"
              icon={
                navigationCollapsed ? (
                  <PanelLeftExpand20Regular />
                ) : (
                  <PanelLeftContract20Regular />
                )
              }
              onClick={() => setNavigationCollapsed((value) => !value)}
            />
          </Tooltip>
          <Button
            type="button"
            appearance="subtle"
            icon={<ArrowClockwise20Regular />}
            aria-label={text("app.message15")}
            hidden={navigationCollapsed}
            onClick={() => void shell.inspectAssets()}
          />
        </header>
        <div
          id="project-files-navigation-content"
          className="project-files-navigation-content"
          hidden={navigationCollapsed}
        >
          <p className="project-files-help">
            {text(mode === "resources" ? "resources.help" : "trash.help")}
          </p>
          <div className="project-files-filters">
            <Input
              aria-label={text("resources.search")}
              placeholder={text("resources.search")}
              value={query}
              onChange={(_, data) => setQuery(data.value)}
            />
            <Select
              aria-label={text("resources.typeFilter")}
              value={type}
              onChange={(_, data) => setType(data.value)}
            >
              <option value="all">{text("resources.allTypes")}</option>
              {mode === "resources" ? (
                <>
                  <option value="images">{text("resources.images")}</option>
                  <option value="files">{text("resources.files")}</option>
                </>
              ) : (
                <>
                  <option value="document">{text("trash.document")}</option>
                  <option value="template">{text("trash.template")}</option>
                  <option value="resource">{text("trash.resource")}</option>
                </>
              )}
            </Select>
            {mode === "resources" && (
              <Select
                aria-label={text("resources.usageFilter")}
                value={usage}
                onChange={(_, data) => setUsage(data.value)}
              >
                <option value="all">{text("resources.allUsage")}</option>
                <option value="used">{text("health.status.used")}</option>
                <option value="unused">{text("health.status.unused")}</option>
                <option value="in_trash">
                  {text("health.status.in_trash")}
                </option>
              </Select>
            )}
            <Select
              aria-label={text("resources.sort")}
              value={sort}
              onChange={(_, data) => setSort(data.value)}
            >
              <option value="name">{text("resources.sortName")}</option>
              <option value="type">{text("resources.sortType")}</option>
            </Select>
          </div>
          <div className={`project-files-actions ${mode}`}>
            <span>
              {text("resources.selected", {
                count: String(currentSelection.length),
              })}
            </span>
            {mode === "trash" && (
              <Button
                className="trash-empty-button"
                type="button"
                disabled={!allRows.some((row) => !row.protected)}
                onClick={() => setPurgeConfirm([...allRows])}
              >
                {text("trash.empty")}
              </Button>
            )}
          </div>
          {app.health?.error && (
            <FloatingNotice intent="error">
              <FloatingNoticeContent>{app.health.error}</FloatingNoticeContent>
            </FloatingNotice>
          )}
          {app.health?.message && (
            <FloatingNotice intent="info">
              <FloatingNoticeContent>
                {app.health.message}
              </FloatingNoticeContent>
            </FloatingNotice>
          )}
          {operationNotice && (
            <FloatingNotice intent={operationNotice.kind}>
              <FloatingNoticeContent>
                {operationNotice.text}
              </FloatingNoticeContent>
            </FloatingNotice>
          )}
        </div>
      </aside>
      <div
        className="project-files-resizer"
        role="separator"
        tabIndex={navigationCollapsed ? -1 : 0}
        aria-label={text("navigation.panelResize")}
        aria-orientation="vertical"
        aria-valuemin={220}
        aria-valuemax={NAVIGATION_MAX}
        aria-valuenow={navigationWidth}
        onPointerDown={(event) => {
          if (navigationCollapsed) return;
          resizeStart.current = { x: event.clientX, width: navigationWidth };
          event.currentTarget.setPointerCapture(event.pointerId);
          event.preventDefault();
        }}
        onPointerMove={(event) => {
          if (!resizeStart.current) return;
          setNavigationWidth(
            Math.max(
              220,
              Math.min(
                NAVIGATION_MAX,
                resizeStart.current.width +
                  event.clientX -
                  resizeStart.current.x,
              ),
            ),
          );
        }}
        onPointerUp={(event) => {
          resizeStart.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
      />
      <section
        className="project-files-content"
        aria-label={text(
          mode === "resources" ? "resources.list" : "trash.list",
        )}
      >
        {!!visible.length && (
          <div className="project-files-column-header" aria-hidden="true">
            <span>
              {text(
                mode === "resources"
                  ? "resources.extension"
                  : "resources.typeColumn",
              )}
            </span>
            <span>{text("resources.nameColumn")}</span>
            <span>{text("resources.protectionColumn")}</span>
            <span>{text("resources.sizeColumn")}</span>
            <span />
          </div>
        )}
        <ul className="project-files-list" aria-multiselectable="true">
          {visible.map((row) => {
            const targets =
              currentSelection.includes(row.key) && currentSelection.length > 1
                ? selectedRows
                : [row];
            const multipleContext = targets.length > 1;
            const protectionReason = row.protected
              ? reasonLabel(row.reason)
              : null;
            const extension = extensionBadge(row.name);
            return (
              <li
                key={row.key}
                className={currentSelection.includes(row.key) ? "selected" : ""}
              >
                <Menu
                  open={menu === row.key}
                  onOpenChange={(_, data) =>
                    setMenu(data.open ? row.key : null)
                  }
                >
                  <div
                    className="project-file-menu-row"
                    onContextMenu={(event) => {
                      event.preventDefault();
                      if (!currentSelection.includes(row.key))
                        setSelection({ mode, keys: [row.key] });
                      anchor.current = row.key;
                      setMenu(row.key);
                    }}
                  >
                    <button
                      type="button"
                      id={`project-file-${row.key}`}
                      className="project-file-row"
                      aria-selected={currentSelection.includes(row.key)}
                      onClick={(event) => choose(row, event)}
                      onDoubleClick={() => openResource(row)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") openResource(row);
                        if (
                          event.key === " " &&
                          (event.ctrlKey || event.shiftKey)
                        ) {
                          event.preventDefault();
                          choose(row, event);
                        }
                        if (
                          event.key === "F2" &&
                          mode === "resources" &&
                          row.kind === "resource"
                        ) {
                          event.preventDefault();
                          setRename({ id: row.id, name: row.name });
                        }
                      }}
                    >
                      {mode === "trash" ? (
                        <Badge
                          appearance="outline"
                          color={badgeColor(row.kind)}
                        >
                          {text(`trash.${row.kind}`)}
                        </Badge>
                      ) : (
                        <Badge
                          appearance="tint"
                          color={extension.color}
                          className={`resource-extension-badge resource-extension-${extension.kind}`}
                        >
                          {extension.label}
                        </Badge>
                      )}
                      <span className="project-file-name" title={row.name}>
                        <span>{row.name}</span>
                        {svnStatus && (
                          <SvnItemStatus
                            status={svnStatus}
                            paths={
                              row.kind === "resource" && mode === "resources"
                                ? svnStatus.entries
                                    .filter((entry) => {
                                      const path = entry.path.replace(
                                        /\\/gu,
                                        "/",
                                      );
                                      return (
                                        path.startsWith(`assets/${row.id}/`) ||
                                        path.includes(`/assets/${row.id}/`)
                                      );
                                    })
                                    .map((entry) =>
                                      entry.path.replace(/\\/gu, "/"),
                                    )
                                : row.kind === "template"
                                  ? [`templates/${row.id}.json`]
                                  : row.kind === "document"
                                    ? [`documents/${row.id}.json`]
                                    : []
                            }
                          />
                        )}
                      </span>
                      <span
                        className="project-file-protection"
                        title={protectionReason ?? undefined}
                      >
                        {text(
                          row.protected
                            ? "health.protected"
                            : "health.unprotected",
                        )}
                      </span>
                      <span className="project-file-size">
                        {formatSize(row.size)}
                      </span>
                    </button>
                    <MenuTrigger disableButtonEnhancement>
                      <Button
                        type="button"
                        appearance="subtle"
                        icon={<MoreHorizontal16Regular />}
                        aria-label={`${row.name} · ${text("documents.menu")}`}
                        onClick={() => {
                          if (!currentSelection.includes(row.key))
                            setSelection({ mode, keys: [row.key] });
                          anchor.current = row.key;
                        }}
                      />
                    </MenuTrigger>
                  </div>
                  <MenuPopover>
                    <MenuList>
                      {mode === "resources" &&
                        row.kind === "resource" &&
                        !multipleContext && (
                          <>
                            <MenuItem
                              icon={<FolderOpen20Regular />}
                              onClick={() => openResource(row)}
                            >
                              {text("resources.open")}
                            </MenuItem>
                            <MenuItem
                              icon={<Edit20Regular />}
                              onClick={() =>
                                setRename({ id: row.id, name: row.name })
                              }
                            >
                              {text("resources.rename")}
                            </MenuItem>
                            <MenuItem
                              icon={<Delete20Regular />}
                              className="destructive-menu-item"
                              disabled={
                                !targets.some((target) => !target.protected)
                              }
                              onClick={() => moveResources(targets)}
                            >
                              {text("documents.toTrash")}
                            </MenuItem>
                          </>
                        )}
                      {mode === "resources" && multipleContext && (
                        <MenuItem
                          icon={<Delete20Regular />}
                          className="destructive-menu-item"
                          disabled={
                            !targets.some((target) => !target.protected)
                          }
                          onClick={() => moveResources(targets)}
                        >
                          {text("documents.toTrash")}
                        </MenuItem>
                      )}
                      {mode === "trash" && (
                        <>
                          <MenuItem
                            icon={<ArrowUndo20Regular />}
                            onClick={() => void restoreRows(targets)}
                          >
                            {text("health.restore")}
                          </MenuItem>
                          <MenuItem
                            icon={<Delete20Regular />}
                            className="destructive-menu-item"
                            disabled={
                              !targets.some((target) => !target.protected)
                            }
                            onClick={() => setPurgeConfirm([...targets])}
                          >
                            {text("trash.purge")}
                          </MenuItem>
                        </>
                      )}
                    </MenuList>
                  </MenuPopover>
                </Menu>
              </li>
            );
          })}
        </ul>
        {!visible.length && (
          <EmptyState>
            {text(
              mode === "resources" ? "resources.empty" : "documents.trashEmpty",
            )}
          </EmptyState>
        )}
      </section>
      {rename && (
        <Dialog open modalType="modal">
          <DialogSurface>
            <DialogBody>
              <DialogTitle>{text("resources.rename")}</DialogTitle>
              <DialogContent>
                <Input
                  autoFocus
                  value={rename.name}
                  onChange={(_, data) =>
                    setRename({ ...rename, name: data.value })
                  }
                />
              </DialogContent>
              <DialogActions>
                <Button
                  type="button"
                  appearance="primary"
                  onClick={() => {
                    const value = rename;
                    setRename(null);
                    void shell.renameAsset(value.id, value.name);
                  }}
                >
                  {text("common.save")}
                </Button>
                <Button type="button" onClick={() => setRename(null)}>
                  {text("common.cancel")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
      {purgeConfirm && (
        <Dialog open modalType="modal">
          <DialogSurface>
            <DialogBody>
              <DialogTitle>{text("trash.purgeTitle")}</DialogTitle>
              <DialogContent>
                <p>{text("trash.purgeWarning")}</p>
                <p>
                  {text("trash.purgeSummary", {
                    count: String(
                      purgeConfirm.filter((row) => !row.protected).length,
                    ),
                    protected: String(
                      purgeConfirm.filter((row) => row.protected).length,
                    ),
                  })}
                </p>
              </DialogContent>
              <DialogActions>
                <Button
                  type="button"
                  appearance="primary"
                  onClick={() => {
                    const targets = purgeConfirm;
                    setPurgeConfirm(null);
                    void purgeRows(targets);
                  }}
                >
                  {text("trash.purge")}
                </Button>
                <Button type="button" onClick={() => setPurgeConfirm(null)}>
                  {text("common.cancel")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
    </div>
  );
}

function formatSize(value: number | null) {
  if (value === null || !Number.isFinite(value)) return "—";
  if (value < 1024) return `${value} B`;
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(value / 1024)} KB`;
}

function fileExtension(name: string) {
  const match = /\.([^.]+)$/.exec(name.trim());
  return match?.[1]?.toLocaleUpperCase() ?? "";
}

type ExtensionBadge = {
  label: string;
  kind: "image" | "text" | "document" | "data" | "archive" | "other";
  color: "brand" | "informative" | "success" | "warning" | "severe" | "subtle";
};

function extensionBadge(name: string): ExtensionBadge {
  const extension = fileExtension(name);
  if (/^(PNG|JPE?G|GIF|WEBP|BMP|SVG|AVIF|ICO)$/.test(extension))
    return { label: extension, kind: "image", color: "informative" };
  if (/^(TXT|MD|LOG|RTF)$/.test(extension))
    return { label: extension, kind: "text", color: "success" };
  if (/^(PDF|DOCX?|ODT|PPTX?|XLSX?)$/.test(extension))
    return { label: extension, kind: "document", color: "brand" };
  if (/^(JSON|CSV|TSV|XML|YAML|YML|TOML)$/.test(extension))
    return { label: extension, kind: "data", color: "warning" };
  if (/^(ZIP|7Z|RAR|TAR|GZ|BZ2)$/.test(extension))
    return { label: extension, kind: "archive", color: "severe" };
  return {
    label: extension || text("resources.unknownExtension"),
    kind: "other",
    color: "subtle",
  };
}

function reasonLabel(reason: string | null | undefined) {
  switch (reason) {
    case "stored_reference":
      return text("health.reason.stored_reference");
    case "reference_scan_incomplete":
      return text("health.reason.reference_scan_incomplete");
    default:
      return text("health.reason.reference");
  }
}

function badgeColor(kind: Row["kind"]): "brand" | "informative" | "success" {
  if (kind === "document") return "brand";
  if (kind === "template") return "informative";
  return "success";
}
