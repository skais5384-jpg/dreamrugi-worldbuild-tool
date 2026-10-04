import { DocumentContent } from "./DocumentContent";
import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";
import { FormatControl } from "./FormatControl";
import {
  type CSSProperties,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Field as FluentField,
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogContent,
  DialogActions,
  Tooltip,
  Menu,
  MenuTrigger,
  MenuPopover,
  MenuList,
  MenuItem,
  Badge,
  Textarea,
} from "@fluentui/react-components";
import {
  Add20Regular,
  ArrowClockwise20Regular,
  Dismiss16Regular,
  ChevronDown16Regular,
  PanelLeftContract20Regular,
  PanelLeftExpand20Regular,
  PanelRightContract20Regular,
  PanelRightExpand20Regular,
  Edit20Regular,
  DocumentPdf20Regular,
} from "@fluentui/react-icons";
import { Button, Checkbox, Input, Select } from "../ui/Controls";
import { IconCommand } from "../ui/IconCommand";
import { EmptyState } from "../ui/EmptyState";
import { InlineNotice } from "../ui/InlineNotice";
import { collectDocumentIssues, DocumentIssueNotices } from "./DocumentIssues";
import {
  DOCUMENT_NAVIGATION_MAX,
  DOCUMENT_NAVIGATION_MIN,
  DOCUMENT_GLOSSARY_MAX,
  DOCUMENT_GLOSSARY_MIN,
  type DocumentController,
} from "./documentController";
import type { Creation } from "../bridge/documents";
import type { Field } from "../bridge/types";
import { blockField, PropertyRow } from "./PropertyRow";
import { CreationValue } from "./CreationValue";
import { presentationClass, SectionTitles } from "./Presentation";
import { DocumentEditor, editLabel } from "./DocumentEditor";
import { editDirty } from "./documentEdits";
import { PdfExportDialog } from "./PdfExportDialog";
import { MediaActiveContext, MediaPreviewContext } from "./MediaValue";
export { plain } from "./CreationValue";
import { text } from "../strings";
import { EditingFocus } from "./editingFocus";
import { DocumentPositions } from "./documentPosition";
import { DocumentTree } from "./DocumentTree";
import type { SvnStatus } from "./svnClient";
import { svnClient, svnFailure } from "./svnClient";
import { DocumentSearchArea } from "./DocumentSearchArea";
import "./Documents.css";
import {
  buildReferenceIndex,
  type ReferenceContext,
} from "./DocumentReferenceValue";
import { IncomingRelations } from "./IncomingRelations";
import { DocumentGlossary } from "./DocumentGlossary";

export function DocumentWorkspace({
  controller,
  hidden = false,
  trash = false,
  searchMode = false,
  readOnly = false,
  collaborative = false,
  onCommitDocument,
  svnStatus,
  onPdfCompleted,
  onRestoreField,
}: {
  controller: DocumentController;
  hidden?: boolean;
  trash?: boolean;
  searchMode?: boolean;
  readOnly?: boolean;
  collaborative?: boolean;
  onCommitDocument?: (id: string) => void;
  svnStatus?: SvnStatus | null;
  onPdfCompleted?: (cleanupWarning: boolean) => void;
  onRestoreField?: (template: string) => void;
}) {
  const state = useSyncExternalStore(controller.subscribe, controller.snapshot);
  const app = useSyncExternalStore(
    controller.shell.subscribe,
    controller.shell.snapshot,
  );
  const [template, setTemplate] = useState("");
  const [parent, setParent] = useState<string | null>(null);
  const [creation, setCreation] = useState(!!state.draft);
  const [pdfTarget, setPdfTarget] = useState<string | null>(null);
  const [lockBlocked, setLockBlocked] = useState<{
    document: string;
    documentId: string;
    project: string;
    generation: number;
    intent: symbol;
    owner: string | null;
    observation: string | null;
    username: string | null;
    reason: string;
    confirming: boolean;
    busy: boolean;
    done: boolean;
    error: string | null;
  } | null>(null);
  const [conflictBlocked, setConflictBlocked] = useState<string | null>(null);
  const editIntent = useRef<symbol | null>(null);
  useLayoutEffect(() => {
    editIntent.current = null;
  }, [state.ui.active, app.projectId, app.root, hidden, creation, trash]);
  const [creating, setCreating] = useState(!!state.draft);
  const scroll = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const positions = useRef(new DocumentPositions());
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const [viewportMaximum, setViewportMaximum] = useState(
    DOCUMENT_NAVIGATION_MAX,
  );
  const [treeViewportHeight, setTreeViewportHeight] = useState(
    typeof window === "undefined" ? 640 : window.innerHeight,
  );
  const resizeStart = useRef<{ x: number; width: number } | null>(null);
  const glossaryResizeStart = useRef<{ x: number; width: number } | null>(null);
  const [glossaryDragWidth, setGlossaryDragWidth] = useState<number | null>(
    null,
  );
  const pane = creation
    ? "creation:" + (state.draft?.owner ?? "pending")
    : ((trash ? state.read?.id : state.ui.active) ?? "empty");
  useLayoutEffect(() => {
    // read 응답과 active 변경 사이가 아닌, 새 탭의 DOM이 표시된 뒤 복원한다.
    if (body.current) positions.current.restore(pane, body.current);
  }, [pane]);
  useEffect(() => {
    document
      .getElementById("document-tab-" + state.ui.active)
      ?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [state.ui.active]);
  const openDocument = (id: string) => {
    setCreation(false);
    void controller.open(id);
  };
  const focus = useRef(new EditingFocus());
  useEffect(() => {
    // Templates share this controller's media transport even while the document
    // pane is hidden. Rebind on project entry before exposing that transport.
    if (app.projectId && app.project?.runtime === "Ready")
      void controller.load();
  }, [controller, app.projectId, app.project?.runtime, hidden, trash]);
  useEffect(() => {
    if (scroll.current) scroll.current.scrollTop = state.ui.scroll;
  }, [state.list?.snapshot, state.ui.scroll]);
  useEffect(() => {
    let open = false;
    let owner = controller.snapshot().draft?.owner;
    return controller.subscribe(() => {
      const next = controller.snapshot();
      if ((next.prompt || next.editPrompt) && !open) focus.current.prepare();
      open = next.prompt || !!next.editPrompt;
      if (next.draft?.owner !== owner) {
        setCreating(!!next.draft);
        setCreation(!!next.draft);
        owner = next.draft?.owner;
      }
    });
  }, [controller]);
  useEffect(() => {
    if (!state.busy && !state.replace.busy) return;
    const timer = setInterval(() => void controller.pollProgress(), 250);
    return () => clearInterval(timer);
  }, [controller, state.busy, state.replace.busy]);
  useEffect(() => {
    const measure = () =>
      setViewportMaximum(
        Math.max(
          DOCUMENT_NAVIGATION_MIN,
          Math.min(DOCUMENT_NAVIGATION_MAX, window.innerWidth - 613),
        ),
      );
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);
  useLayoutEffect(() => {
    const tree = scroll.current;
    if (!tree || hidden || state.ui.navigationCollapsed) return;
    const measure = () =>
      setTreeViewportHeight(tree.clientHeight || window.innerHeight);
    measure();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measure);
      return () => window.removeEventListener("resize", measure);
    }
    const observer = new ResizeObserver(measure);
    observer.observe(tree);
    return () => observer.disconnect();
  }, [hidden, state.ui.navigationCollapsed]);
  const list = state.list;
  const selected = state.ui.active;
  const byId = new Map(list?.documents.map((d) => [d.id, d]));
  const documentIssues = collectDocumentIssues(
    list,
    app.rows,
    app.assetInspection,
    state.validationIssues,
    state.read,
    app.inspectionPendingDocuments,
    ["waiting", "checking"].includes(app.inspectionRefreshState ?? ""),
  );
  const active =
    list?.documents.filter(
      (d) => list.layout.nodes[d.id]?.state !== "trashed",
    ) ?? [];
  const referenceIndex = useMemo(
    () => buildReferenceIndex(list, app.rows),
    [list, app.rows],
  );
  const reference = (currentDocument?: string): ReferenceContext => ({
    list,
    templates: app.rows,
    index: referenceIndex,
    currentDocument,
    open: (document) => void controller.openReferenceTarget(document),
  });
  const locked =
    state.busy || state.prompt || !!state.editPrompt || app.closing;
  const structureLocked =
    locked || readOnly || controller.hasOwners() || !!list?.problem;
  const navigationWidth = Math.min(
    dragWidth ?? state.ui.navigationWidth,
    viewportMaximum,
  );
  const glossaryWidth = Math.min(
    glossaryDragWidth ?? state.ui.glossaryWidth,
    DOCUMENT_GLOSSARY_MAX,
  );
  function start(parent: string | null) {
    if (!state.draft) {
      setParent(parent);
      setTemplate("");
    }
    setCreating(true);
    setCreation(true);
  }
  function closeCreation() {
    controller.requestCreationClose(() => {
      setCreation(false);
      setCreating(false);
    });
  }
  function tabName(id: string) {
    const name = byId.get(id)?.name ?? text("documents.unknownName");
    if (list?.documents.filter((d) => d.name === name).length === 1)
      return name;
    const parent = list?.layout.nodes[id]?.parentId;
    return (
      name +
      " · " +
      (parent
        ? (byId.get(parent)?.name ?? text("documents.unknownName"))
        : text("documents.root")) +
      " · " +
      `문서 ${(list?.documents.findIndex((d) => d.id === id) ?? 0) + 1}`
    );
  }
  return (
    <MediaPreviewContext.Provider value={!state.previewClosing && !app.closing}>
      <MediaActiveContext.Provider
        value={!hidden && !trash && !app.closing && !state.previewClosing}
      >
        <section
          hidden={hidden}
          className="document-workspace"
          onFocusCapture={(e) => focus.current.capture(e)}
          onBlurCapture={() => focus.current.rememberSelection()}
        >
          <div
            inert={state.prompt || !!state.editPrompt}
            className="document-work-area"
          >
            <div className="document-feedback">
              {state.message && (
                <FloatingNotice intent={state.messageIntent}>
                  <FloatingNoticeContent>{state.message}</FloatingNoticeContent>
                </FloatingNotice>
              )}
              {state.uiError && (
                <FloatingNotice intent="warning">
                  <FloatingNoticeContent>
                    {text("documents.uiError")}
                  </FloatingNoticeContent>
                </FloatingNotice>
              )}
              {list?.problem && (
                <FloatingNotice intent="warning">
                  <FloatingNoticeContent>
                    {text("documents.layoutProblem")}
                    <p>
                      입력은 유지됩니다. 실행 기록에서 확인한 뒤 목록을 다시
                      불러와 주세요.
                    </p>
                  </FloatingNoticeContent>
                </FloatingNotice>
              )}
            </div>
            <div
              className={
                "document-columns" +
                (state.ui.navigationCollapsed ? " navigation-collapsed" : "") +
                (state.ui.glossaryCollapsed ? " glossary-collapsed" : "")
              }
              style={
                {
                  "--document-navigation-width": `${navigationWidth}px`,
                  "--document-glossary-width": `${glossaryWidth}px`,
                } as CSSProperties
              }
            >
              <aside
                id="document-navigation"
                className="document-navigation"
                aria-label={text("documents.navigation")}
              >
                <header className="navigation-heading">
                  <h2 hidden={state.ui.navigationCollapsed}>
                    {text(
                      trash
                        ? "documents.trash"
                        : searchMode
                          ? "documents.searchArea"
                          : "documents.title",
                    )}
                  </h2>
                  <Tooltip
                    content={text(
                      state.ui.navigationCollapsed
                        ? "documents.navigationExpand"
                        : "documents.navigationCollapse",
                    )}
                    relationship="label"
                  >
                    <Button
                      type="button"
                      appearance="subtle"
                      aria-controls="document-navigation-content"
                      aria-expanded={!state.ui.navigationCollapsed}
                      onClick={() => controller.toggleNavigation()}
                      icon={
                        state.ui.navigationCollapsed ? (
                          <PanelLeftExpand20Regular />
                        ) : (
                          <PanelLeftContract20Regular />
                        )
                      }
                    />
                  </Tooltip>
                  <Tooltip content={text("app.message15")} relationship="label">
                    <Button
                      type="button"
                      appearance="subtle"
                      hidden={state.ui.navigationCollapsed}
                      disabled={locked}
                      onClick={() => void controller.load()}
                      icon={<ArrowClockwise20Regular />}
                    />
                  </Tooltip>
                  {!trash && !searchMode && (
                    <Tooltip
                      content={text("documents.new")}
                      relationship="label"
                    >
                      <Button
                        type="button"
                        appearance="subtle"
                        hidden={state.ui.navigationCollapsed}
                        disabled={locked || readOnly || !!list?.problem}
                        onClick={() => start(null)}
                        icon={<Add20Regular />}
                      />
                    </Tooltip>
                  )}
                </header>
                <div
                  id="document-navigation-content"
                  ref={scroll}
                  className="document-tree"
                  hidden={state.ui.navigationCollapsed}
                  onScroll={(e) => {
                    if (!trash && !searchMode)
                      controller.scroll(e.currentTarget.scrollTop);
                  }}
                >
                  {searchMode ? (
                    <DocumentSearchArea
                      controller={controller}
                      templates={app.rows}
                      openDocument={(id) => {
                        setCreation(false);
                        void controller.openSearchResult(id);
                      }}
                    />
                  ) : !trash ? (
                    <DocumentTree
                      openDocument={openDocument}
                      exportPdf={setPdfTarget}
                      controller={controller}
                      locked={locked}
                      structureLocked={structureLocked}
                      createChild={start}
                      viewportHeight={treeViewportHeight}
                      issues={documentIssues}
                      svnStatus={svnStatus}
                    />
                  ) : (
                    <ul className="document-tree-list">
                      {list?.documents
                        .filter(
                          (d) => list.layout.nodes[d.id]?.state === "trashed",
                        )
                        .map((d) => (
                          <li key={d.id}>
                            <Button
                              type="button"
                              appearance="subtle"
                              onClick={() => {
                                setCreation(false);
                                void controller.openTrash(d.id);
                              }}
                            >
                              {d.name}
                            </Button>
                            <div className="actions">
                              <Button
                                type="button"
                                disabled={structureLocked}
                                onClick={() =>
                                  void controller.mutate({
                                    kind: "restore",
                                    document: d.id,
                                    destination: null,
                                  })
                                }
                              >
                                {text("documents.restore")}
                              </Button>
                              <Button
                                type="button"
                                disabled={structureLocked}
                                onClick={() =>
                                  void controller.mutate({
                                    kind: "restore",
                                    document: d.id,
                                    destination: {
                                      parent: null,
                                      index: 4294967295,
                                    },
                                  })
                                }
                              >
                                {text("documents.restoreRoot")}
                              </Button>
                            </div>
                          </li>
                        ))}
                    </ul>
                  )}
                  {list &&
                    !(trash
                      ? list.documents.some(
                          (d) => list.layout.nodes[d.id]?.state === "trashed",
                        )
                      : list.documents.length) && (
                      <p>
                        {text(
                          trash ? "documents.trashEmpty" : "documents.empty",
                        )}
                      </p>
                    )}
                </div>
              </aside>
              <Tooltip
                content={text("documents.navigationResizeHelp")}
                relationship="description"
              >
                <div
                  className="document-navigation-resizer"
                  role="separator"
                  tabIndex={state.ui.navigationCollapsed ? -1 : 0}
                  aria-label={text("documents.navigationResize")}
                  aria-controls="document-navigation"
                  aria-orientation="vertical"
                  aria-valuemin={DOCUMENT_NAVIGATION_MIN}
                  aria-valuemax={Math.round(viewportMaximum)}
                  aria-valuenow={Math.round(navigationWidth)}
                  onPointerDown={(event) => {
                    if (state.ui.navigationCollapsed) return;
                    resizeStart.current = {
                      x: event.clientX,
                      width: navigationWidth,
                    };
                    event.currentTarget.setPointerCapture(event.pointerId);
                    event.preventDefault();
                  }}
                  onPointerMove={(event) => {
                    if (!resizeStart.current) return;
                    setDragWidth(
                      Math.max(
                        DOCUMENT_NAVIGATION_MIN,
                        Math.min(
                          viewportMaximum,
                          resizeStart.current.width +
                            event.clientX -
                            resizeStart.current.x,
                        ),
                      ),
                    );
                  }}
                  onPointerUp={(event) => {
                    const start = resizeStart.current;
                    if (!start) return;
                    event.currentTarget.releasePointerCapture(event.pointerId);
                    resizeStart.current = null;
                    controller.navigationWidth(
                      Math.max(
                        DOCUMENT_NAVIGATION_MIN,
                        Math.min(
                          viewportMaximum,
                          start.width + event.clientX - start.x,
                        ),
                      ),
                    );
                    setDragWidth(null);
                  }}
                  onPointerCancel={() => {
                    resizeStart.current = null;
                    setDragWidth(null);
                  }}
                  onKeyDown={(event) => {
                    let width: number | null = null;
                    if (event.key === "ArrowLeft") width = navigationWidth - 12;
                    if (event.key === "ArrowRight")
                      width = navigationWidth + 12;
                    if (event.key === "Home") width = DOCUMENT_NAVIGATION_MIN;
                    if (event.key === "End") width = viewportMaximum;
                    if (width !== null) {
                      event.preventDefault();
                      controller.navigationWidth(
                        Math.max(
                          DOCUMENT_NAVIGATION_MIN,
                          Math.min(viewportMaximum, width),
                        ),
                      );
                    }
                  }}
                />
              </Tooltip>
              <div className="document-detail">
                <div
                  className="document-tabs"
                  aria-label={text("documents.tabs")}
                >
                  <Menu>
                    <MenuTrigger disableButtonEnhancement>
                      <Button
                        type="button"
                        appearance="subtle"
                        className="document-tab-picker"
                        aria-label={text("documents.openTabs")}
                        icon={<ChevronDown16Regular />}
                      />
                    </MenuTrigger>
                    <MenuPopover>
                      <MenuList>
                        {state.ui.tabs.map((id) => (
                          <MenuItem
                            key={id}
                            disabled={locked}
                            onClick={() => openDocument(id)}
                          >
                            {tabName(id)}
                          </MenuItem>
                        ))}
                      </MenuList>
                    </MenuPopover>
                  </Menu>
                  {state.ui.tabs.map((id) => (
                    <div
                      key={id}
                      className={!creation && selected === id ? "selected" : ""}
                    >
                      <Tooltip content={tabName(id)} relationship="label">
                        <Button
                          id={"document-tab-" + id}
                          type="button"
                          appearance="subtle"
                          aria-pressed={!creation && selected === id}
                          disabled={locked}
                          onClick={() => openDocument(id)}
                          onKeyDown={(e) => {
                            if (
                              e.altKey &&
                              ["ArrowLeft", "ArrowRight"].includes(e.key)
                            ) {
                              e.preventDefault();
                              controller.reorderTab(
                                id,
                                e.key === "ArrowLeft" ? -1 : 1,
                              );
                            }
                          }}
                          aria-keyshortcuts="Alt+ArrowLeft Alt+ArrowRight"
                        >
                          {byId.get(id)?.name ?? text("documents.unknownName")}
                          {state.editors[id] && (
                            <span className="document-tab-state">
                              {" "}
                              · {editLabel(state.editors[id])}
                            </span>
                          )}
                        </Button>
                      </Tooltip>
                      <Button
                        type="button"
                        appearance="subtle"
                        disabled={locked}
                        aria-label={
                          text("documents.closeTab") + " · " + tabName(id)
                        }
                        onClick={(e) => {
                          e.stopPropagation();
                          void controller.closeTab(id);
                        }}
                        icon={<Dismiss16Regular />}
                      />
                    </div>
                  ))}
                  {creating && (
                    <div className={creation ? "selected" : ""}>
                      <Button
                        type="button"
                        appearance="subtle"
                        aria-pressed={creation}
                        onClick={() => setCreation(true)}
                      >
                        {state.draft?.body.name || text("documents.new")} *
                      </Button>
                      <Button
                        type="button"
                        appearance="subtle"
                        disabled={locked}
                        aria-label={text("documents.closeCreationTab")}
                        onClick={closeCreation}
                        icon={<Dismiss16Regular />}
                      />
                    </div>
                  )}
                  {state.ui.glossaryCollapsed && !trash && (
                    <Button
                      type="button"
                      appearance="subtle"
                      className="glossary-open-command"
                      onClick={() => controller.toggleGlossary()}
                      icon={<PanelRightExpand20Regular />}
                    >
                      {text("glossary.open")}
                    </Button>
                  )}
                </div>
                <div
                  ref={body}
                  onScroll={(e) =>
                    positions.current.scroll(pane, e.currentTarget.scrollTop)
                  }
                  onBlurCapture={(e) =>
                    positions.current.remember(pane, e.target, e.currentTarget)
                  }
                  className="document-body"
                >
                  {Object.entries(state.editors).map(([id, entry]) => (
                    <DocumentEditor
                      key={entry.status.owner}
                      id={id}
                      entry={entry}
                      controller={controller}
                      hidden={
                        hidden || creation || trash || state.ui.active !== id
                      }
                      locked={
                        state.prompt ||
                        !!state.editPrompt ||
                        app.closing ||
                        !!entry.closing
                      }
                      reference={reference(id)}
                      issues={documentIssues.get(id) ?? []}
                      exportPdf={setPdfTarget}
                      collaborative={collaborative}
                      onCommit={onCommitDocument}
                    />
                  ))}
                  {!creation &&
                    state.read &&
                    (!state.editors[state.read.id] || trash) && (
                      <article key={state.read.id}>
                        <header className="document-content-header">
                          <div className="document-title-row">
                            <h1 className="document-term-title">
                              <span>{state.read.name}</span>
                              {!!state.read.englishName && (
                                <small>{state.read.englishName}</small>
                              )}
                            </h1>
                            <Badge appearance="tint" color="informative">
                              {text("documents.template")}:{" "}
                              {state.read.template.name}
                            </Badge>
                          </div>
                          {!!state.read.glossarySummary && (
                            <p className="document-term-summary">
                              {state.read.glossarySummary}
                            </p>
                          )}
                          {!trash && (
                            <div className="actions document-header-actions">
                              <IconCommand
                                label={text("pdf.command")}
                                icon={<DocumentPdf20Regular />}
                                disabled={locked}
                                onClick={() => setPdfTarget(state.read!.id)}
                              />
                              <FormatControl
                                key={state.read.id}
                                shell={controller.shell}
                                kind="document"
                                artifact={state.read.id}
                                reference={reference(state.read.id)}
                                locked={
                                  locked ||
                                  readOnly ||
                                  collaborative ||
                                  controller.hasOwners()
                                }
                                // 복원은 이름도 되돌리므로 목록·탭·본문을 함께 다시 읽는다.
                                changed={() => controller.load()}
                              />
                              <IconCommand
                                label={text("documentEdit.begin")}
                                icon={<Edit20Regular />}
                                disabled={
                                  locked ||
                                  (readOnly && !collaborative) ||
                                  !!list?.problem ||
                                  (state.read?.schema ?? 4) < 4 ||
                                  app.project?.runtime !== "Ready"
                                }
                                onClick={() => {
                                  const document = state.read!.id;
                                  const name = state.read!.name;
                                  const project = app.projectId;
                                  const generation =
                                    controller.shell.projectGeneration();
                                  const intent = Symbol("edit-intent");
                                  editIntent.current = intent;
                                  const currentIntent = () =>
                                    editIntent.current === intent &&
                                    controller.snapshot().ui.active ===
                                      document &&
                                    controller.shell.snapshot().projectId ===
                                      project &&
                                    controller.shell.projectGeneration() ===
                                      generation;
                                  void controller
                                    .beginEdit(document)
                                    .then(async (opened) => {
                                      if (opened || !collaborative || !project)
                                        return;
                                      if (!currentIntent()) return;
                                      if (
                                        controller.shell.snapshot()
                                          .projectId !== project ||
                                        controller.shell.projectGeneration() !==
                                          generation
                                      )
                                        return;
                                      const conflict = svnStatus?.entries.some(
                                        (entry) =>
                                          entry.path
                                            .replace(/\\/gu, "/")
                                            .toLowerCase()
                                            .endsWith(
                                              `/documents/${document.toLowerCase()}.json`,
                                            ) &&
                                          ["conflicted", "obstructed"].includes(
                                            entry.local,
                                          ),
                                      );
                                      if (conflict) {
                                        controller.acknowledgeBlockedEdit();
                                        setConflictBlocked(name);
                                        return;
                                      }
                                      try {
                                        const lock =
                                          await svnClient.documentLockOwner(
                                            app.root,
                                            document,
                                          );
                                        if (!currentIntent()) return;
                                        if (
                                          controller.shell.snapshot()
                                            .projectId !== project ||
                                          controller.shell.projectGeneration() !==
                                            generation
                                        )
                                          return;
                                        if (lock.locked) {
                                          const session = await svnClient
                                            .session()
                                            .catch(() => null);
                                          if (!currentIntent()) return;
                                          if (
                                            controller.shell.snapshot()
                                              .projectId !== project ||
                                            controller.shell.projectGeneration() !==
                                              generation
                                          )
                                            return;
                                          controller.acknowledgeBlockedEdit();
                                          setLockBlocked({
                                            document: name,
                                            documentId: document,
                                            project: app.root,
                                            generation,
                                            intent,
                                            owner: lock.owner,
                                            observation: lock.observation,
                                            username: session?.connected
                                              ? (session.username ?? null)
                                              : null,
                                            reason: "",
                                            confirming: false,
                                            busy: false,
                                            done: false,
                                            error: null,
                                          });
                                        }
                                      } catch {
                                        if (!currentIntent()) return;
                                        if (
                                          controller.shell.snapshot()
                                            .projectId !== project ||
                                          controller.shell.projectGeneration() !==
                                            generation
                                        )
                                          return;
                                        setLockBlocked({
                                          document: name,
                                          documentId: document,
                                          project: app.root,
                                          generation,
                                          intent,
                                          owner: null,
                                          observation: null,
                                          username: null,
                                          reason: "",
                                          confirming: false,
                                          busy: false,
                                          done: false,
                                          error: null,
                                        });
                                      }
                                    });
                                }}
                              />
                            </div>
                          )}
                        </header>
                        <DocumentIssueNotices
                          issues={documentIssues.get(state.read.id) ?? []}
                        />
                        <DocumentContent
                          read={state.read}
                          reference={reference(state.read.id)}
                          locked={locked}
                          onRestoreField={onRestoreField}
                        />
                        {!trash && (
                          <IncomingRelations
                            controller={controller}
                            document={state.read.id}
                            references={state.references}
                            busy={state.referencesBusy}
                            error={state.referencesError}
                          />
                        )}
                      </article>
                    )}
                  {!creation &&
                    !state.read &&
                    (state.ui.active && state.error ? (
                      <section>
                        <h2>
                          {byId.get(state.ui.active)?.name ??
                            text("field.emptyLabel")}
                        </h2>
                        <InlineNotice kind="warning">
                          {text("format.sourceUnavailable")}
                        </InlineNotice>
                        <FormatControl
                          shell={controller.shell}
                          kind="document"
                          artifact={state.ui.active}
                          reference={reference(state.ui.active)}
                          locked={
                            locked ||
                            readOnly ||
                            collaborative ||
                            controller.hasOwners()
                          }
                          changed={() => controller.load()}
                        />
                      </section>
                    ) : (
                      <EmptyState>
                        {text("documents.selectDocument")}
                      </EmptyState>
                    ))}
                  <MediaActiveContext.Provider
                    value={
                      !hidden &&
                      !trash &&
                      creation &&
                      !app.closing &&
                      !state.previewClosing
                    }
                  >
                    <section hidden={!creation}>
                      <h1>{text("documents.new")}</h1>
                      {!state.draft ? (
                        <>
                          <p>
                            {parent
                              ? text("documents.parent") +
                                ": " +
                                (byId.get(parent)?.name ??
                                  "이름을 확인할 수 없는 상위 문서")
                              : text("documents.root")}
                          </p>
                          <PropertyRow
                            label={text("documents.template")}
                            htmlFor="creation-template"
                          >
                            <Select
                              id="creation-template"
                              value={template}
                              disabled={locked || !!list?.problem}
                              onChange={(e) => setTemplate(e.target.value)}
                            >
                              <option value="">
                                {text("documents.chooseTemplate")}
                              </option>
                              {app.rows
                                .filter((t) => t.lifecycle === "Active")
                                .map((t) => (
                                  <option key={t.id} value={t.id}>
                                    {t.name}
                                  </option>
                                ))}
                            </Select>
                          </PropertyRow>
                          <Button
                            type="button"
                            appearance="primary"
                            disabled={
                              !template || locked || readOnly || !!list?.problem
                            }
                            onClick={() =>
                              void controller.begin(template, parent)
                            }
                          >
                            {text("documents.begin")}
                          </Button>
                        </>
                      ) : state.restoredOwner !== state.draft.owner ? (
                        <div className="actions creation-pending">
                          {locked ? (
                            <p role="status">{text("documents.creating")}</p>
                          ) : (
                            <>
                              <InlineNotice kind="warning">
                                {text(
                                  state.draft.outcome?.kind === "write" &&
                                    ["committed", "uncertain"].includes(
                                      state.draft.outcome.disk,
                                    )
                                    ? "documents.createUncertain"
                                    : "documents.createFailed",
                                )}
                              </InlineNotice>
                              <Button
                                type="button"
                                appearance="primary"
                                disabled={
                                  readOnly ||
                                  (state.draft.outcome?.kind === "write" &&
                                    ["committed", "uncertain"].includes(
                                      state.draft.outcome.disk,
                                    ))
                                }
                                onClick={() => void controller.retryCreation()}
                              >
                                {text("project.retryDefault")}
                              </Button>
                              <Button
                                type="button"
                                onClick={() =>
                                  controller.requestCreationClose()
                                }
                              >
                                {text("common.cancel")}
                              </Button>
                            </>
                          )}
                        </div>
                      ) : (
                        <CreationForm
                          controller={controller}
                          draft={state.draft}
                          locked={locked || readOnly}
                          parents={active}
                          chooseParent={
                            state.restoredOwner === state.draft.owner
                          }
                          reference={reference()}
                        />
                      )}
                    </section>
                  </MediaActiveContext.Provider>
                </div>
              </div>
              {!state.ui.glossaryCollapsed && !trash && (
                <>
                  <Tooltip
                    content={text("glossary.resizeHelp")}
                    relationship="description"
                  >
                    <div
                      className="document-glossary-resizer"
                      role="separator"
                      tabIndex={0}
                      aria-label={text("glossary.resize")}
                      aria-controls="document-glossary"
                      aria-orientation="vertical"
                      aria-valuemin={DOCUMENT_GLOSSARY_MIN}
                      aria-valuemax={DOCUMENT_GLOSSARY_MAX}
                      aria-valuenow={Math.round(glossaryWidth)}
                      onPointerDown={(event) => {
                        event.currentTarget.focus();
                        glossaryResizeStart.current = {
                          x: event.clientX,
                          width: glossaryWidth,
                        };
                        event.currentTarget.setPointerCapture(event.pointerId);
                        event.preventDefault();
                      }}
                      onPointerMove={(event) => {
                        if (!glossaryResizeStart.current) return;
                        setGlossaryDragWidth(
                          Math.max(
                            DOCUMENT_GLOSSARY_MIN,
                            Math.min(
                              DOCUMENT_GLOSSARY_MAX,
                              glossaryResizeStart.current.width +
                                glossaryResizeStart.current.x -
                                event.clientX,
                            ),
                          ),
                        );
                      }}
                      onPointerUp={(event) => {
                        const start = glossaryResizeStart.current;
                        if (!start) return;
                        event.currentTarget.releasePointerCapture(
                          event.pointerId,
                        );
                        glossaryResizeStart.current = null;
                        controller.glossaryWidth(
                          start.width + start.x - event.clientX,
                        );
                        setGlossaryDragWidth(null);
                      }}
                      onPointerCancel={() => {
                        glossaryResizeStart.current = null;
                        setGlossaryDragWidth(null);
                      }}
                      onKeyDown={(event) => {
                        let width: number | null = null;
                        if (event.key === "ArrowLeft")
                          width = glossaryWidth + 12;
                        if (event.key === "ArrowRight")
                          width = glossaryWidth - 12;
                        if (event.key === "Home") width = DOCUMENT_GLOSSARY_MIN;
                        if (event.key === "End") width = DOCUMENT_GLOSSARY_MAX;
                        if (width !== null) {
                          event.preventDefault();
                          controller.glossaryWidth(width);
                        }
                      }}
                    />
                  </Tooltip>
                  <aside
                    id="document-glossary"
                    className="document-glossary"
                    aria-label={text("glossary.title")}
                  >
                    <header>
                      <h2>{text("glossary.title")}</h2>
                      <Tooltip
                        content={text("glossary.collapse")}
                        relationship="label"
                      >
                        <Button
                          type="button"
                          appearance="subtle"
                          onClick={() => controller.toggleGlossary()}
                          icon={<PanelRightContract20Regular />}
                        />
                      </Tooltip>
                    </header>
                    <DocumentGlossary
                      list={list}
                      templates={app.rows}
                      template={state.ui.glossaryTemplate}
                      selectTemplate={(value) =>
                        controller.glossaryTemplate(value)
                      }
                      open={(id) => {
                        setCreation(false);
                        void controller.openReferenceTarget(id);
                      }}
                    />
                  </aside>
                </>
              )}
            </div>
          </div>
          <Dialog
            open={state.prompt}
            modalType="alert"
            onOpenChange={(_, data) => {
              if (!data.open) {
                controller.cancelClose();
                focus.current.restore();
              }
            }}
          >
            <DialogSurface>
              <DialogBody>
                <DialogTitle>{text("documents.closeDraft")}</DialogTitle>
                <DialogContent>{text("documents.closeHelp")}</DialogContent>
                <DialogActions className="creation-dialog-actions">
                  <Button
                    type="button"
                    disabled={state.busy}
                    onClick={() => void controller.resolveClose("deposit")}
                  >
                    {text("documents.depositClose")}
                  </Button>
                  <Button
                    type="button"
                    danger
                    disabled={state.busy}
                    onClick={() => void controller.resolveClose("discard")}
                  >
                    {text("documents.discard")}
                  </Button>
                  <Button
                    type="button"
                    disabled={state.busy}
                    onClick={() => {
                      controller.cancelClose();
                      focus.current.restore();
                    }}
                  >
                    {text("documents.keepEditing")}
                  </Button>
                </DialogActions>
              </DialogBody>
            </DialogSurface>
          </Dialog>
          <Dialog
            open={!!state.editPrompt}
            modalType="alert"
            onOpenChange={(_, data) => {
              if (!data.open) {
                controller.cancelEditClose();
                focus.current.restore();
              }
            }}
          >
            <DialogSurface>
              <DialogBody>
                <DialogTitle>
                  {text(
                    state.editPromptKind === "required"
                      ? "required.closeTitle"
                      : "documentEdit.closeTitle",
                  )}
                </DialogTitle>
                <DialogContent>
                  {text(
                    state.editPromptKind === "required"
                      ? "required.closeHelp"
                      : "documentEdit.closeHelp",
                  )}
                  {state.requiredPrompt.map((id) => (
                    <p key={id}>{state.editors[id]?.status.read.name}</p>
                  ))}
                </DialogContent>
                {!!state.editPrompt &&
                  state.editors[state.editPrompt]?.error && (
                    <InlineNotice kind="error">
                      {text("documentEdit.closeBlocked")}
                    </InlineNotice>
                  )}
                <DialogActions>
                  <Button
                    type="button"
                    disabled={
                      !!state.editPrompt &&
                      state.editors[state.editPrompt]?.busy
                    }
                    onClick={() =>
                      state.requiredPrompt.length
                        ? controller.leaveRequiredClose()
                        : void controller.depositEditClose()
                    }
                  >
                    {text(
                      state.editPromptKind === "required"
                        ? "required.leave"
                        : "documents.depositClose",
                    )}
                  </Button>
                  <Button
                    type="button"
                    onClick={() => {
                      if (state.requiredPrompt.length)
                        void controller.writeRequired();
                      else {
                        controller.cancelEditClose();
                        focus.current.restore();
                      }
                    }}
                  >
                    {text(
                      state.editPromptKind === "required"
                        ? "required.write"
                        : "documents.keepEditing",
                    )}
                  </Button>
                </DialogActions>
              </DialogBody>
            </DialogSurface>
          </Dialog>
          <Dialog
            open={!!conflictBlocked}
            onOpenChange={(_, data) => {
              if (!data.open) setConflictBlocked(null);
            }}
          >
            <DialogSurface>
              <DialogBody>
                <DialogTitle>{text("svn.conflictDocumentTitle")}</DialogTitle>
                <DialogContent>
                  <p>{conflictBlocked}</p>
                  <p>{text("svn.conflictBlocked")}</p>
                </DialogContent>
                <DialogActions>
                  <Button
                    type="button"
                    onClick={() => setConflictBlocked(null)}
                  >
                    {text("common.close")}
                  </Button>
                </DialogActions>
              </DialogBody>
            </DialogSurface>
          </Dialog>
          <Dialog
            open={!!lockBlocked}
            onOpenChange={(_, data) => {
              if (!data.open && !lockBlocked?.busy) {
                editIntent.current = null;
                setLockBlocked(null);
              }
            }}
          >
            <DialogSurface>
              <DialogBody>
                <DialogTitle>{text("svn.lockBlockedTitle")}</DialogTitle>
                <DialogContent>
                  <p>{lockBlocked?.document}</p>
                  <p>{text("svn.lockBlockedHelp")}</p>
                  <p>
                    {lockBlocked?.owner
                      ? text("svn.lockOwner", { owner: lockBlocked.owner })
                      : text("svn.lockOwnerUnknown")}
                  </p>
                  {lockBlocked?.done && (
                    <p role="status">{text("svn.forceDone")}</p>
                  )}
                  {lockBlocked?.confirming && !lockBlocked.done && (
                    <>
                      <p>{text("svn.forceImpact")}</p>
                      <FluentField label={text("svn.forceReason")} required>
                        <Textarea
                          value={lockBlocked.reason}
                          maxLength={200}
                          disabled={lockBlocked.busy}
                          onChange={(_, data) =>
                            setLockBlocked((current) =>
                              current
                                ? { ...current, reason: data.value }
                                : null,
                            )
                          }
                        />
                      </FluentField>
                    </>
                  )}
                  {lockBlocked?.error && (
                    <InlineNotice kind="error">
                      {lockBlocked.error}
                    </InlineNotice>
                  )}
                </DialogContent>
                <DialogActions>
                  {lockBlocked?.owner &&
                    lockBlocked.observation &&
                    lockBlocked.username &&
                    !lockBlocked.done && (
                      <Button
                        type="button"
                        disabled={
                          lockBlocked.busy ||
                          (lockBlocked.confirming && !lockBlocked.reason.trim())
                        }
                        onClick={() => {
                          const current = lockBlocked;
                          if (!current.confirming) {
                            setLockBlocked({ ...current, confirming: true });
                            return;
                          }
                          if (
                            controller.shell.snapshot().projectId !==
                              app.projectId ||
                            controller.shell.projectGeneration() !==
                              current.generation ||
                            app.root !== current.project ||
                            editIntent.current !== current.intent ||
                            controller.snapshot().ui.active !==
                              current.documentId
                          ) {
                            setLockBlocked({
                              ...current,
                              error: text("svn.forceContextChanged"),
                            });
                            return;
                          }
                          setLockBlocked({
                            ...current,
                            busy: true,
                            error: null,
                          });
                          void svnClient
                            .forceDocumentLock(
                              current.project,
                              current.documentId,
                              current.owner!,
                              current.observation!,
                              current.username!,
                              current.reason,
                            )
                            .then(async () => {
                              if (
                                controller.shell.snapshot().projectId !==
                                  app.projectId ||
                                controller.shell.projectGeneration() !==
                                  current.generation
                              )
                                return;
                              setLockBlocked((latest) =>
                                latest?.intent === current.intent
                                  ? { ...latest, done: true }
                                  : latest,
                              );
                              // Complete the local observation before normal admission:
                              // both commands use the native SVN execution slot.
                              const entry = await svnClient.localDocumentStatus(
                                current.project,
                                current.documentId,
                              );
                              if (
                                controller.shell.snapshot().projectId ===
                                  app.projectId &&
                                controller.shell.projectGeneration() ===
                                  current.generation
                              ) {
                                // Force returns only after verified ownership. Reflect it
                                // before normal edit admission (which rechecks the token).
                                controller.observeSvnLocal(
                                  current.documentId,
                                  entry,
                                );
                                setLockBlocked((latest) =>
                                  latest?.intent === current.intent
                                    ? { ...latest, busy: false, done: true }
                                    : latest,
                                );
                                if (
                                  editIntent.current === current.intent &&
                                  controller.snapshot().ui.active ===
                                    current.documentId
                                ) {
                                  const opened = await controller.beginEdit(
                                    current.documentId,
                                  );
                                  if (
                                    opened &&
                                    editIntent.current === current.intent
                                  ) {
                                    editIntent.current = null;
                                    setLockBlocked(null);
                                  }
                                }
                              }
                            })
                            .catch((error: unknown) => {
                              if (
                                controller.shell.snapshot().projectId !==
                                  app.projectId ||
                                controller.shell.projectGeneration() !==
                                  current.generation
                              )
                                return;
                              setLockBlocked((latest) =>
                                latest?.intent === current.intent
                                  ? {
                                      ...latest,
                                      busy: false,
                                      error: svnFailure(error),
                                    }
                                  : latest,
                              );
                            });
                        }}
                      >
                        {text(
                          lockBlocked.confirming
                            ? "svn.forceConfirm"
                            : "svn.forceLock",
                        )}
                      </Button>
                    )}
                  <Button
                    type="button"
                    disabled={lockBlocked?.busy}
                    onClick={() => {
                      editIntent.current = null;
                      setLockBlocked(null);
                    }}
                  >
                    {text("common.close")}
                  </Button>
                </DialogActions>
              </DialogBody>
            </DialogSurface>
          </Dialog>
          {pdfTarget && (
            <PdfExportDialog
              key={pdfTarget}
              controller={controller}
              document={pdfTarget}
              dirty={
                !!state.editors[pdfTarget] &&
                editDirty(state.editors[pdfTarget])
              }
              close={() => setPdfTarget(null)}
              completed={(cleanupWarning) => onPdfCompleted?.(cleanupWarning)}
            />
          )}
        </section>
      </MediaActiveContext.Provider>
    </MediaPreviewContext.Provider>
  );
}
function CreationForm({
  controller,
  draft,
  locked,
  parents,
  chooseParent,
  reference,
}: {
  controller: DocumentController;
  draft: Creation;
  locked: boolean;
  parents: { id: string; name: string }[];
  chooseParent: boolean;
  reference: ReferenceContext;
}) {
  const edit = controller.edit.bind(controller);
  const restored = draft.problem === "RestoredChooseParent";
  const invalidName = draft.problem === "NameRequired";
  const invalidEnglishName =
    draft.problem === "SingleLineRequired" && draft.field === "englishName";
  const invalidGlossarySummary =
    draft.problem === "SingleLineRequired" && draft.field === "glossarySummary";
  const targetId = invalidEnglishName
    ? "creation-english-name"
    : invalidGlossarySummary
      ? "creation-glossary-summary"
      : draft.field
        ? "creation-" + draft.field
        : invalidName
          ? "creation-name"
          : restored
            ? "creation-parent"
            : null;
  const focusError = () => {
    if (targetId && !draft.body.composing) {
      const target = document.getElementById(targetId);
      target?.focus();
      target?.scrollIntoView?.({ block: "nearest" });
    }
  };
  useEffect(() => {
    if (draft.problem && !locked && !draft.body.composing) {
      const target = targetId && document.getElementById(targetId);
      if (target) {
        target.focus();
        target.scrollIntoView?.({ block: "nearest" });
      }
    }
  }, [draft.problem, targetId, locked, draft.body.composing]);
  return (
    <div
      className="creation-form"
      onCompositionStart={() => edit((b) => ({ ...b, composing: true }))}
      onCompositionEnd={() => edit((b) => ({ ...b, composing: false }))}
    >
      {draft.problem && (
        <FloatingNotice
          intent={restored ? "info" : "error"}
          eventId={draft.outcome ?? draft}
          scope={`${draft.owner}:${draft.generation}:${draft.field ?? ""}`}
          isCurrent={() => {
            const current = controller.snapshot().draft;
            return current === draft && !locked && !current.body.composing;
          }}
        >
          <FloatingNoticeContent>
            {text(restored ? "documents.restored" : "documents.invalid")}
            {targetId && (
              <Button
                type="button"
                appearance="transparent"
                onClick={focusError}
              >
                {text("documents.errorFocus")}
              </Button>
            )}
          </FloatingNoticeContent>
        </FloatingNotice>
      )}
      <PropertyRow
        label={text("documents.name") + " *"}
        htmlFor="creation-name"
      >
        <FluentField
          validationState={invalidName ? "error" : "none"}
          validationMessage={
            invalidName
              ? {
                  id: "creation-name-error",
                  children: text("documents.requiredName"),
                }
              : undefined
          }
        >
          <Input
            id="creation-name"
            aria-required="true"
            disabled={locked}
            aria-invalid={invalidName}
            aria-describedby={invalidName ? "creation-name-error" : undefined}
            value={draft.body.name}
            onChange={(e) => edit((b) => ({ ...b, name: e.target.value }))}
          />
        </FluentField>
      </PropertyRow>
      <PropertyRow
        label={text("glossary.englishName")}
        htmlFor="creation-english-name"
      >
        <FluentField
          validationState={invalidEnglishName ? "error" : "none"}
          validationMessage={
            invalidEnglishName ? text("documents.invalidValue") : undefined
          }
        >
          <Input
            id="creation-english-name"
            disabled={locked}
            aria-invalid={invalidEnglishName}
            value={draft.body.englishName ?? ""}
            onChange={(event) =>
              edit((body) => ({ ...body, englishName: event.target.value }))
            }
          />
        </FluentField>
      </PropertyRow>
      <PropertyRow
        label={text("glossary.summary")}
        htmlFor="creation-glossary-summary"
      >
        <FluentField
          validationState={invalidGlossarySummary ? "error" : "none"}
          validationMessage={
            invalidGlossarySummary ? text("documents.invalidValue") : undefined
          }
        >
          <Input
            id="creation-glossary-summary"
            disabled={locked}
            aria-invalid={invalidGlossarySummary}
            value={draft.body.glossarySummary ?? ""}
            onChange={(event) =>
              edit((body) => ({
                ...body,
                glossarySummary: event.target.value,
              }))
            }
          />
        </FluentField>
      </PropertyRow>
      <PropertyRow label={text("glossary.exclude")}>
        <div>
          <Checkbox
            aria-label={text("glossary.excludeDocument")}
            disabled={locked}
            checked={draft.body.glossaryExcluded ?? false}
            onChange={(_, data) =>
              edit((body) => ({
                ...body,
                glossaryExcluded: data.checked === true,
              }))
            }
          />
          {!!draft.template.glossaryExcluded && (
            <p className="property-help">
              {text("glossary.templateExcludedNotice")}
            </p>
          )}
        </div>
      </PropertyRow>
      <PropertyRow
        label={text("documents.parent")}
        htmlFor={chooseParent ? "creation-parent" : undefined}
      >
        {chooseParent ? (
          <Select
            id="creation-parent"
            disabled={locked}
            value={draft.body.parent ?? ""}
            onChange={(e) =>
              edit((b) => ({ ...b, parent: e.target.value || null }))
            }
          >
            <option value="">{text("documents.root")}</option>
            {parents.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
              </option>
            ))}
          </Select>
        ) : (
          <span>
            {parents.find((d) => d.id === draft.body.parent)?.name ??
              text("documents.root")}
          </span>
        )}
      </PropertyRow>
      {draft.template.fieldOrder
        .map((id) => draft.template.fields.find((f) => f.id === id))
        .filter((f): f is Field => !!f && f.lifecycle === "Active")
        .map((field) => (
          <PropertyRow
            key={field.id}
            before={
              <SectionTitles template={draft.template} before={field.id} />
            }
            presentation={presentationClass(
              draft.template.presentation,
              field.presentation,
            )}
            label={field.label + (field.required ? " *" : "")}
            htmlFor={"creation-" + field.id}
            complex
            block={blockField(field.kind)}
          >
            <CreationValue
              group={{
                owner: draft.owner,
                generation: draft.generation,
                composing: draft.body.composing,
                problem: draft.field,
                importCell: (address, image) =>
                  controller.importAsset(field.id, image, undefined, address),
                reference,
              }}
              reference={reference}
              importAsset={() =>
                controller.importAsset(field.id, field.kind === "Image")
              }
              field={field}
              intent={
                draft.body.fields.find((f) => f.field === field.id)?.value ?? {
                  intent: "keep",
                }
              }
              disabled={locked}
              invalid={!!draft.problem && draft.field === field.id}
              change={(value) =>
                edit((b) => ({
                  ...b,
                  fields: [
                    ...b.fields.filter((f) => f.field !== field.id),
                    { field: field.id, value },
                  ],
                }))
              }
            />
          </PropertyRow>
        ))}
      <SectionTitles template={draft.template} before={null} />
      <div className="actions">
        <Button
          type="button"
          appearance="primary"
          disabled={
            locked ||
            draft.body.composing ||
            (draft.outcome?.kind === "write" &&
              ["committed", "uncertain"].includes(draft.outcome.disk))
          }
          onClick={() => void controller.submit()}
        >
          {text("documents.create")}
        </Button>
        <Button
          type="button"
          disabled={locked}
          onClick={() => void controller.submit(true)}
        >
          {text("documents.deposit")}
        </Button>
        <Button
          type="button"
          disabled={locked}
          onClick={() => controller.requestCreationClose()}
        >
          {text("documents.closeDraft")}
        </Button>
      </div>
      {draft.deposited && <p role="status">{text("documents.deposited")}</p>}
    </div>
  );
}
