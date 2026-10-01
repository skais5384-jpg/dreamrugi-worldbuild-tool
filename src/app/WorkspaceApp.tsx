import { CollaborationPolicyDialog } from "./CollaborationPolicyDialog";
import {
  FloatingMessage,
  FloatingNotice,
  FloatingNoticeContent,
} from "../ui/FloatingNotice";
import { MediaContext } from "./MediaValue";
import { invoke } from "@tauri-apps/api/core";
import { FormatControl } from "./FormatControl";
import {
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Menu,
  MenuTrigger,
  MenuPopover,
  MenuList,
  MenuItem,
  MenuDivider,
  Tooltip,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
  Checkbox,
  Radio,
  RadioGroup,
  Spinner,
} from "@fluentui/react-components";
import {
  Document20Regular,
  DocumentBulletList20Regular,
  Delete20Regular,
  Search20Regular,
  ChevronDown16Regular,
  ArrowSync20Regular,
  DismissCircle20Regular,
  FolderOpen20Regular,
  History20Regular,
  Info16Regular,
  Info20Regular,
  Power20Regular,
  Star20Regular,
  StarOff20Regular,
  Attach20Regular,
  Add20Regular,
  ArrowClockwise20Regular,
  ArrowUndo20Regular,
  BookOpen20Regular,
  Copy20Regular,
  Edit20Regular,
  PanelLeftContract20Regular,
  PanelLeftExpand20Regular,
} from "@fluentui/react-icons";
import { Button, Input } from "../ui/Controls";
import { EmptyState } from "../ui/EmptyState";
import { InlineNotice } from "../ui/InlineNotice";
import { IconCommand } from "../ui/IconCommand";
import {
  workspaceController,
  type WorkspaceController,
} from "./workspaceController";
import { DocumentWorkspace } from "./DocumentWorkspace";
import { DocumentGlossary } from "./DocumentGlossary";
import { WholeTemplate } from "./WholeTemplate";
import { ReadonlyTemplate } from "./ReadonlyTemplate";
import { RecoveryCenter } from "./RecoveryCenter";
import { FollowUp } from "./FollowUp";
import { ProjectHealth } from "./ProjectHealth";
import { ProjectFiles } from "./ProjectFiles";
import { AboutDialog } from "./AboutDialog";
import { UpdateDialog, updateMessage } from "./UpdateDialog";
import { updaterController } from "./updaterClient";
import { TemplateManagement } from "./TemplateManagement";
import { SvnDialog } from "./SvnDialog";
import { SvnToolbar } from "./SvnToolbar";
import { SvnCommitDialog } from "./SvnCommitDialog";
import { SvnRegisterDialog } from "./SvnRegisterDialog";
import { SvnItemStatus } from "./SvnItemStatus";
import { ActivityLog, useActivityLog } from "./ActivityLog";
import type { SvnStatus } from "./svnClient";
import { projectLabel, lifecycleLabel } from "./statusLabels";
import { text } from "../strings";
import { BackupTable } from "./BackupTable";
import { NAVIGATION_DEFAULT, NAVIGATION_MAX } from "./navigationSizing";
import type { BackupRow } from "../bridge/types";
import { EditingFocus } from "./editingFocus";
import { editDirty } from "./documentEdits";
import { formatLocalDateTime } from "./localDateTime";
import "./App.css";
import "./Workspace.css";

function sameProjectRoot(current: string, saved: string | null) {
  if (!saved) return false;
  const normalized = (value: string) => {
    let path = value.replace(/\\/gu, "/");
    path = /^\/\/\?\/UNC\//iu.test(path)
      ? `//${path.slice(8)}`
      : path.replace(/^\/\/\?\//u, "");
    return path.replace(/\/+$/u, "").toLocaleLowerCase();
  };
  return normalized(current) === normalized(saved);
}

function backupSize(value: string) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes) || bytes < 0) return value;
  if (bytes < 1024) return `${new Intl.NumberFormat().format(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let amount = bytes;
  let unit = -1;
  do {
    amount /= 1024;
    unit += 1;
  } while (amount >= 1024 && unit < units.length - 1);
  return `${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(amount)} ${units[unit]}`;
}

function unnamedBackupNumbers(backups: BackupRow[]) {
  return new Map(
    backups
      .filter(
        (backup) =>
          !backup.label.trim() &&
          ["verification_required", "verified"].includes(backup.status),
      )
      .sort(
        (left, right) =>
          new Date(left.createdAtUtc).getTime() -
            new Date(right.createdAtUtc).getTime() ||
          left.id.localeCompare(right.id),
      )
      .map((backup, index) => [backup.id, index + 1]),
  );
}

interface LegacyHandoffStatus {
  state:
    | "pending"
    | "not_applicable"
    | "absent"
    | "preserved"
    | "needs_attention"
    | "failed";
  summary: {
    found: number;
    imported: number;
    alreadyPresent: number;
    needsAttention: number;
  } | null;
  error: { category: string; stage: string } | null;
}

export default function WorkspaceApp({
  controller = workspaceController(),
}: {
  controller?: WorkspaceController;
}) {
  const state = useSyncExternalStore(controller.subscribe, controller.snapshot);
  useEffect(() => {
    // Editor listeners run on document first. Read-only workspaces consume only
    // the browser's Save Page shortcut and never enter an editing owner.
    const consumeBrowserSave = (event: KeyboardEvent) => {
      if (
        event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "s"
      )
        event.preventDefault();
    };
    window.addEventListener("keydown", consumeBrowserSave);
    return () => window.removeEventListener("keydown", consumeBrowserSave);
  }, []);
  const [mode, setMode] = useState<
    "documents" | "glossary" | "templates" | "resources" | "search" | "trash"
  >(state.draft ? "templates" : "documents");
  const [openAsDefault, setOpenAsDefault] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [policyRoot, setPolicyRoot] = useState<string | null>(null);
  const [dismissedPolicy, setDismissedPolicy] = useState<string | null>(null);
  const [updateOpen, setUpdateOpen] = useState(false);
  const [dismissedUpdate, setDismissedUpdate] = useState<string | null>(null);
  const updater = updaterController();
  const updateStatus = useSyncExternalStore(
    updater.subscribe,
    updater.snapshot,
  );
  const updateNotice = `${updateStatus?.candidate}:${updateStatus?.phase}:${updateStatus?.error}`;
  const [svnEntry, setSvnEntry] = useState<"connect" | "checkout" | null>(null);
  const [svnSessionRevision, setSvnSessionRevision] = useState(0);
  const [svnCommitTarget, setSvnCommitTarget] = useState<
    string | null | undefined
  >(undefined);
  const [svnCommitRevision, setSvnCommitRevision] = useState(0);
  const [registerNew, setRegisterNew] = useState(false);
  const [registerCopy, setRegisterCopy] = useState(false);
  const [copyToast, setCopyToast] = useState(false);
  const [actionToast, setActionToast] = useState<string | null>(null);
  const [svnRegister, setSvnRegister] = useState<{
    root: string;
    kind: "new" | "copy";
  } | null>(null);
  const [svnStatus, setSvnStatus] = useState<SvnStatus | null>(null);
  const [svnConnectionFailed, setSvnConnectionFailed] = useState(false);
  const [svnUpdating, setSvnUpdating] = useState(false);
  const [logOpen, setLogOpen] = useState(false);
  const [logNoticeKey, setLogNoticeKey] = useState<string>();
  const [legacyHandoff, setLegacyHandoff] =
    useState<LegacyHandoffStatus | null>(null);
  const [legacyRetrying, setLegacyRetrying] = useState(false);
  useEffect(() => {
    let active = true;
    try {
      void invoke<LegacyHandoffStatus>("legacy_handoff_status")
        .then((status) => {
          if (active) setLegacyHandoff(status);
        })
        .catch(() => {});
    } catch {
      // Browser-only tests have no Tauri transport.
    }
    return () => {
      active = false;
    };
  }, []);
  const retryLegacyHandoff = async () => {
    setLegacyRetrying(true);
    try {
      setLegacyHandoff(
        await invoke<LegacyHandoffStatus>("legacy_handoff_retry"),
      );
    } catch {
      setLegacyHandoff({ state: "failed", summary: null, error: null });
    } finally {
      setLegacyRetrying(false);
    }
  };
  const aboutTrigger = useRef<HTMLButtonElement>(null);
  const [feedbackPause, setFeedbackPause] = useState<{
    id: number;
    hovered: boolean;
    focused: boolean;
  } | null>(null);
  const [pdfCompletion, setPdfCompletion] = useState<{
    id: number;
    scope: string;
    cleanupWarning: boolean;
  } | null>(null);
  const [pdfCompletionPause, setPdfCompletionPause] = useState<{
    id: number;
    hovered: boolean;
    focused: boolean;
  } | null>(null);
  const pdfCompletionSequence = useRef(0);
  const [selectedTemplates, setSelectedTemplates] = useState<{
    scope: string;
    ids: string[];
  }>({ scope: "", ids: [] });
  const [templateTrashConfirm, setTemplateTrashConfirm] = useState<{
    scope: string;
    ids: string[];
  } | null>(null);
  const [templateNavigationWidth, setTemplateNavigationWidth] =
    useState(NAVIGATION_DEFAULT);
  const [templateNavigationCollapsed, setTemplateNavigationCollapsed] =
    useState(false);
  const templateResizeStart = useRef<{ x: number; width: number } | null>(null);
  const [glossaryNavigationWidth, setGlossaryNavigationWidth] =
    useState(NAVIGATION_DEFAULT);
  const [glossaryNavigationCollapsed, setGlossaryNavigationCollapsed] =
    useState(false);
  const glossaryResizeStart = useRef<{ x: number; width: number } | null>(null);
  const templateSelectionAnchor = useRef<{ scope: string; id: string } | null>(
    null,
  );
  const documentState = useSyncExternalStore(
    controller.documents.subscribe,
    controller.documents.snapshot,
  );
  const [showDocumentProgress, setShowDocumentProgress] = useState(false);
  useEffect(() => {
    if (!documentState.busy || documentState.navigationBusy) {
      let active = true;
      queueMicrotask(() => {
        if (active) setShowDocumentProgress(false);
      });
      return () => {
        active = false;
      };
    }
    const timer = window.setTimeout(() => setShowDocumentProgress(true), 350);
    return () => window.clearTimeout(timer);
  }, [documentState.busy, documentState.navigationBusy]);
  useEffect(() => {
    const currentOwner = () =>
      controller.documents.snapshot().restoredOwner ??
      controller.documents.snapshot().draft?.owner;
    let previous = currentOwner();
    return controller.documents.subscribe(() => {
      const owner = currentOwner();
      if (owner && owner !== previous) setMode("documents");
      previous = owner;
    });
  }, [controller]);
  const shell = controller.shell;
  const app = useSyncExternalStore(shell.subscribe, shell.snapshot);
  const activityLog = useActivityLog(true);
  const loggedNotices = useRef<Set<string>>(new Set());
  const recordActivity = activityLog.record;
  const restoredUpdateResults = useRef<Set<string>>(new Set());
  useEffect(() => {
    for (const result of updateStatus?.handoff?.pendingSvn ?? []) {
      if (restoredUpdateResults.current.has(result.operationId)) continue;
      restoredUpdateResults.current.add(result.operationId);
      recordActivity({
        feature: "svn",
        stage: "update_handoff",
        category: "result_unverified",
        outcome: result.outcome,
        summary: text("update.pendingSvn"),
        detail: JSON.stringify(result, null, 2),
      });
    }
  }, [updateStatus?.handoff, recordActivity]);
  useEffect(() => {
    const open = (event: Event) => {
      activityLog.markRead();
      focus.current.prepare();
      setLogNoticeKey(
        event instanceof CustomEvent ? event.detail?.noticeKey : undefined,
      );
      setLogOpen(true);
    };
    window.addEventListener("open-activity-log", open);
    return () => window.removeEventListener("open-activity-log", open);
  }, [activityLog]);
  useEffect(() => {
    if (Reflect.has(window, "__TAURI_INTERNALS__"))
      shell.setStartupGate(updater.start());
    else void updater.initialize();
  }, [shell, updater]);
  useEffect(() => {
    const phase = updateStatus?.phase,
      complete = updateStatus?.startupComplete;
    const timer = window.setTimeout(() => {
      if (
        phase &&
        !complete &&
        [
          "checking",
          "available",
          "failed",
          "cancelled",
          "downloading",
          "ready",
          "install_failed",
        ].includes(phase)
      )
        setUpdateOpen(true);
      if (complete || phase === "preparing") setUpdateOpen(false);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [updateStatus?.phase, updateStatus?.startupComplete]);
  useEffect(() => {
    if (
      updateStatus?.phase === "failed" ||
      updateStatus?.phase === "install_failed"
    ) {
      recordActivity({
        feature: "updater",
        stage: updateStatus.phase,
        category: updateStatus.error ?? "error",
        outcome: "error",
        summary: text(updateMessage(updateStatus)),
      });
    }
  }, [updateStatus, recordActivity]);
  useEffect(() => {
    if (updateStatus?.phase === "preparing" && !app.closing)
      void updater.refresh();
  }, [
    app.closing,
    state.prompt,
    documentState.prompt,
    updater,
    updateStatus?.phase,
  ]);
  useEffect(() => {
    if (!app.project) {
      loggedNotices.current.clear();
      return;
    }
    const notices = [
      ...Object.entries(documentState.editors)
        .filter(([, entry]) => entry.error || entry.paused)
        .map(([id, entry]) => ({
          message: `${entry.status.read.name} · ${id}\n${entry.error ?? entry.status.problem ?? ""}\n${JSON.stringify({ generation: entry.generation, deposited: entry.status.deposited && entry.status.generation === entry.generation, outcome: entry.status.outcome })}`,
          kind: "document-save",
          category: "error",
        })),
      ...(app.operations.some(
        (operation) => operation.problem || operation.phase === "reserved",
      ) ||
      app.sessions.some((session) => session.problem) ||
      app.retainedRefs.length ||
      app.project.shutdown.blockers.length ||
      app.project.shutdown.reportPending ||
      app.app?.native_cleanup.phase === "failed"
        ? [
            {
              message: text("followUp.actionRequired"),
              kind: "follow-up",
              category: "warning",
            },
          ]
        : []),
    ];
    const active = new Set<string>();
    for (const notice of notices) {
      if (!notice.message) continue;
      const key = `${app.projectId}:${notice.message}`;
      if (active.has(key)) continue;
      active.add(key);
      if (loggedNotices.current.has(key)) continue;
      recordActivity({
        feature: "프로젝트",
        stage: notice.kind,
        category: notice.category,
        outcome: "error",
        summary:
          notice.kind === "settings"
            ? "설정 상태를 확인해 주세요"
            : "작업 결과를 확인해 주세요",
        detail: notice.kind === "settings" ? undefined : notice.message,
      });
    }
    loggedNotices.current = active;
  }, [
    app.project,
    app.projectId,
    app.error,
    app.settingsNotice,
    state.error,
    documentState.error,
    documentState.editors,
    app.operations,
    app.sessions,
    app.retainedRefs,
    app.app?.native_cleanup.phase,
    recordActivity,
  ]);
  async function createProject() {
    const root = await shell.confirmNewProject(
      registerNew ? false : openAsDefault,
    );
    if (registerNew && root) {
      await controller.navigate({ kind: "close_project" });
      if (!shell.snapshot().projectId) setSvnRegister({ root, kind: "new" });
    }
  }
  const templateScope = `${shell.projectGeneration()}:${app.projectId ?? ""}`;
  const errorNoticeIdentity = useMemo(
    () => ({ shell, event: app.errorEvent }),
    [shell, app.errorEvent],
  );
  const startupNoticeIdentity = useMemo(
    () => ({ shell, event: app.startupFailureEvent }),
    [shell, app.startupFailureEvent],
  );
  const backupNoticeIdentity = useMemo(
    () => ({ shell, event: app.projectData?.noticeEvent }),
    [shell, app.projectData?.noticeEvent],
  );
  const visiblePdfCompletion =
    pdfCompletion?.scope === templateScope ? pdfCompletion : null;
  const pdfCompletionPaused =
    !!visiblePdfCompletion &&
    pdfCompletionPause?.id === visiblePdfCompletion?.id &&
    (pdfCompletionPause.hovered || pdfCompletionPause.focused);
  useEffect(() => {
    if (!visiblePdfCompletion || pdfCompletionPaused) return;
    const timer = window.setTimeout(
      () =>
        setPdfCompletion((current) =>
          current?.id === visiblePdfCompletion.id ? null : current,
        ),
      4000,
    );
    return () => window.clearTimeout(timer);
  }, [visiblePdfCompletion, pdfCompletionPaused]);
  // 삭제된 Template은 휴지통에서만 관리하며 복원 전에는 내용을 열지 않는다.
  const rows = app.rows.filter((row) => row.lifecycle !== "Deleted");
  const existingTemplateIds = new Set(rows.map((row) => row.id));
  const currentTemplateSelection =
    selectedTemplates.scope === templateScope
      ? selectedTemplates.ids.filter((id) => existingTemplateIds.has(id))
      : [];
  const chooseTemplate = (
    id: string,
    event: React.MouseEvent | React.KeyboardEvent,
  ) => {
    if (!app.projectId) return;
    if (templateSelectionAnchor.current?.scope !== templateScope) {
      templateSelectionAnchor.current = null;
    }
    const order = rows.map((row) => row.id);
    if (
      event.shiftKey &&
      templateSelectionAnchor.current &&
      order.includes(templateSelectionAnchor.current.id)
    ) {
      const from = order.indexOf(templateSelectionAnchor.current.id);
      const to = order.indexOf(id);
      const range = order.slice(Math.min(from, to), Math.max(from, to) + 1);
      setSelectedTemplates({
        scope: templateScope,
        ids:
          event.ctrlKey || event.metaKey
            ? [...new Set([...currentTemplateSelection, ...range])]
            : range,
      });
      return;
    }
    templateSelectionAnchor.current = { scope: templateScope, id };
    if (event.ctrlKey || event.metaKey) {
      setSelectedTemplates({
        scope: templateScope,
        ids: currentTemplateSelection.includes(id)
          ? currentTemplateSelection.filter((current) => current !== id)
          : [...currentTemplateSelection, id],
      });
      return;
    }
    setSelectedTemplates({ scope: templateScope, ids: [id] });
    void controller.navigate({ kind: "select", id });
  };
  const focus = useRef(new EditingFocus());
  const keep = useRef<HTMLButtonElement>(null);
  const prompted = !!state.prompt || !!state.discard;
  const ready =
    app.ready &&
    app.project?.status === "Ready" &&
    app.project.runtime === "Ready";
  const initializationFailed =
    app.project?.error?.code === "initialization_failed" ||
    app.project?.error?.code === "collaboration_policy_rejected" ||
    app.project?.shutdown.reports.some((report) => report.initializationFailed);
  const collaborative = !!app.project?.collaborative;
  const effectivePolicyRoot =
    policyRoot ??
    (app.project?.error?.code === "collaboration_policy_rejected" &&
    dismissedPolicy !== app.projectId
      ? app.root
      : null);
  const dismissPolicy = () => {
    setDismissedPolicy(app.projectId);
    setPolicyRoot(null);
  };
  const locked =
    prompted ||
    documentState.prompt ||
    !!documentState.editPrompt ||
    app.closing;
  const currentIsDefault =
    !!app.project &&
    app.defaultProjectObserved &&
    sameProjectRoot(app.root, app.defaultProjectRoot);
  useEffect(() => {
    shell.start();
  }, [shell]);
  useEffect(() => {
    const completed =
      app.projectData?.kind === "copy" ? app.projectData.completedRoot : null;
    if (!completed) return;
    const timer = window.setTimeout(() => {
      shell.closeProjectData();
      if (registerCopy) setSvnRegister({ root: completed, kind: "copy" });
      else setCopyToast(true);
      setRegisterCopy(false);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [
    app.projectData?.completedRoot,
    app.projectData?.kind,
    registerCopy,
    shell,
  ]);
  useEffect(() => {
    if (!copyToast) return;
    const timer = window.setTimeout(() => setCopyToast(false), 4000);
    return () => window.clearTimeout(timer);
  }, [copyToast]);
  useEffect(() => {
    if (!state.message) return;
    const timer = window.setTimeout(() => setActionToast(state.message), 0);
    return () => window.clearTimeout(timer);
  }, [state.message]);
  useEffect(() => {
    if (!actionToast) return;
    const timer = window.setTimeout(() => setActionToast(null), 4000);
    return () => window.clearTimeout(timer);
  }, [actionToast]);
  useEffect(() => {
    let open = false;
    return controller.subscribe(() => {
      const next =
        !!controller.snapshot().prompt || !!controller.snapshot().discard;
      // React가 배경을 inert로 만들기 전에 실제 스크롤과 입력 위치를 기록한다.
      if (next && !open) focus.current.prepare();
      open = next;
    });
  }, [controller]);
  useEffect(() => {
    if (prompted) keep.current?.focus({ preventScroll: true });
  }, [prompted]);
  useEffect(() => {
    const feedback = app.feedback;
    if (
      !feedback ||
      feedback.undo ||
      (feedbackPause?.id === feedback.id &&
        (feedbackPause.hovered || feedbackPause.focused))
    )
      return;
    const timer = window.setTimeout(
      () => shell.dismissFeedback(feedback.id),
      4000,
    );
    return () => window.clearTimeout(timer);
  }, [app.feedback, feedbackPause, shell]);
  useEffect(() => {
    const feedback = app.feedback;
    if (!feedback?.undo) return;
    const remaining = Date.parse(feedback.undo.expiresAtUtc) - Date.now();
    if (!Number.isFinite(remaining)) return;
    const timer = window.setTimeout(
      () => shell.dismissFeedback(feedback.id),
      Math.max(0, Math.min(remaining, 2_147_483_647)),
    );
    return () => window.clearTimeout(timer);
  }, [app.feedback, shell]);
  const feedbackToast = app.feedback && (
    <div
      className={
        app.projectData
          ? "feedback-toast feedback-toast-dialog"
          : "feedback-toast"
      }
      role="status"
      onMouseEnter={() =>
        setFeedbackPause((current) => ({
          id: app.feedback!.id,
          hovered: true,
          focused: current?.id === app.feedback!.id && current.focused,
        }))
      }
      onMouseLeave={() =>
        setFeedbackPause((current) => ({
          id: app.feedback!.id,
          hovered: false,
          focused: current?.id === app.feedback!.id && current.focused,
        }))
      }
      onFocusCapture={() =>
        setFeedbackPause((current) => ({
          id: app.feedback!.id,
          hovered: current?.id === app.feedback!.id && current.hovered,
          focused: true,
        }))
      }
      onBlurCapture={() =>
        setFeedbackPause((current) => ({
          id: app.feedback!.id,
          hovered: current?.id === app.feedback!.id && current.hovered,
          focused: false,
        }))
      }
    >
      <span>{app.feedback.message}</span>
      {app.feedback.undo && (
        <IconCommand
          label={text("backup.undo")}
          icon={<ArrowUndo20Regular />}
          className="feedback-toast-command"
          disabled={app.busy}
          onClick={() => void shell.undoBackupDeletion()}
        />
      )}
      <IconCommand
        label={text("backup.dismiss")}
        icon={<DismissCircle20Regular />}
        className="feedback-toast-command"
        onClick={() => shell.dismissFeedback(app.feedback!.id)}
      />
    </div>
  );
  const pdfToast = visiblePdfCompletion && (
    <div
      className={`feedback-toast pdf-completion-toast${app.feedback ? " with-feedback" : ""}`}
      role="status"
      onMouseEnter={() =>
        setPdfCompletionPause((current) => ({
          id: visiblePdfCompletion.id,
          hovered: true,
          focused: current?.id === visiblePdfCompletion.id && current.focused,
        }))
      }
      onMouseLeave={() =>
        setPdfCompletionPause((current) => ({
          id: visiblePdfCompletion.id,
          hovered: false,
          focused: current?.id === visiblePdfCompletion.id && current.focused,
        }))
      }
      onFocusCapture={() =>
        setPdfCompletionPause((current) => ({
          id: visiblePdfCompletion.id,
          hovered: current?.id === visiblePdfCompletion.id && current.hovered,
          focused: true,
        }))
      }
      onBlurCapture={() =>
        setPdfCompletionPause((current) => ({
          id: visiblePdfCompletion.id,
          hovered: current?.id === visiblePdfCompletion.id && current.hovered,
          focused: false,
        }))
      }
    >
      <span>
        {visiblePdfCompletion.cleanupWarning
          ? text("pdf.cleanupWarning")
          : text("pdf.completed")}
      </span>
      <IconCommand
        label={text("backup.dismiss")}
        icon={<DismissCircle20Regular />}
        className="feedback-toast-command"
        onClick={() => setPdfCompletion(null)}
      />
    </div>
  );
  const documentProgressToast = showDocumentProgress && documentState.busy && (
    <div
      className={`feedback-toast document-progress-toast${app.feedback ? " with-feedback" : ""}${visiblePdfCompletion ? " with-pdf" : ""}`}
      role="status"
    >
      <Spinner size="tiny" aria-hidden="true" />
      <span>
        {text(
          documentState.busyPhase === "locking"
            ? "documents.locking"
            : documentState.busyPhase === "opening"
              ? "documents.opening"
              : documentState.progress?.phase === 3
                ? "documents.applying"
                : documentState.progress?.requested
                  ? "documents.cancelRequested"
                  : documentState.progress
                    ? "documents.scanning"
                    : "documents.working",
        )}
      </span>
      {!!documentState.progress && (
        <Button
          type="button"
          disabled={
            documentState.progress.requested ||
            documentState.progress.phase === 3
          }
          onClick={() => void controller.documents.pollProgress(true)}
        >
          {text("documents.cancel")}
        </Button>
      )}
    </div>
  );
  return (
    <MediaContext.Provider value={controller.documents}>
      <main className="app-shell workbench-shell">
        <FloatingMessage>
          {!app.projectData && feedbackToast}
          {copyToast && (
            <div className="feedback-toast" role="status">
              {text("backup.copyCreated")}
            </div>
          )}
          {actionToast && !copyToast && (
            <div className="feedback-toast" role="status">
              {actionToast}
            </div>
          )}
          {pdfToast}
          {documentProgressToast}
          {updateStatus?.notify &&
            dismissedUpdate !== updateNotice &&
            !updateOpen &&
            !copyToast &&
            !actionToast &&
            !app.feedback &&
            !pdfToast &&
            !documentProgressToast && (
              <div className="feedback-toast update-toast" role="status">
                <span>{text(updateMessage(updateStatus))}</span>
                <IconCommand
                  label={text("update.details")}
                  icon={<Info20Regular />}
                  className="feedback-toast-command"
                  onClick={() => setUpdateOpen(true)}
                />
                <IconCommand
                  label={text("backup.dismiss")}
                  icon={<DismissCircle20Regular />}
                  className="feedback-toast-command"
                  onClick={() => {
                    setDismissedUpdate(updateNotice);
                    if (
                      updateStatus.phase === "available" ||
                      updateStatus.phase === "ready"
                    )
                      void updater.continue();
                  }}
                />
              </div>
            )}
        </FloatingMessage>
        <div
          className="shell-content"
          inert={prompted}
          onFocusCapture={(event) => focus.current.capture(event)}
          onBlurCapture={() => focus.current.rememberSelection()}
        >
          <header className="project-menu-bar">
            <Menu>
              <MenuTrigger disableButtonEnhancement>
                <Button
                  type="button"
                  appearance="subtle"
                  icon={<ChevronDown16Regular />}
                  iconPosition="after"
                >
                  {app.project
                    ? app.root
                        .replace(/[\\/]+$/, "")
                        .split(/[\\/]/)
                        .pop() || text("documents.projectMenu")
                    : text("documents.projectMenu")}
                </Button>
              </MenuTrigger>
              <MenuPopover>
                <MenuList className="compact-command-menu">
                  <MenuItem
                    icon={<Info20Regular />}
                    disabled={locked || state.busy || app.busy}
                    onClick={() => {
                      if (app.project && collaborative) setPolicyRoot(app.root);
                      else
                        void shell.chooseReplacementProject().then((root) => {
                          if (root) setPolicyRoot(root);
                        });
                    }}
                  >
                    {text("policy.title")}
                  </MenuItem>
                  {app.project ? (
                    <>
                      <MenuItem
                        icon={
                          currentIsDefault ? (
                            <StarOff20Regular />
                          ) : (
                            <Star20Regular />
                          )
                        }
                        disabled={locked || state.busy || app.busy}
                        onClick={() =>
                          void (currentIsDefault
                            ? shell.clearDefaultProject()
                            : shell.setCurrentAsDefault())
                        }
                      >
                        {text(
                          currentIsDefault
                            ? "project.clearDefault"
                            : "project.setDefault",
                        )}
                      </MenuItem>
                      <MenuDivider />
                      <MenuItem
                        icon={<FolderOpen20Regular />}
                        disabled={
                          locked ||
                          svnUpdating ||
                          state.busy ||
                          app.busy ||
                          app.picking
                        }
                        onClick={() => void controller.openProject()}
                      >
                        {text("app.message07")}
                      </MenuItem>
                      <MenuItem
                        icon={<Document20Regular />}
                        disabled={locked || state.busy || app.busy}
                        onClick={() => {
                          setRegisterCopy(false);
                          shell.showProjectCopy();
                        }}
                      >
                        {text("backup.copy")}
                      </MenuItem>
                      {!collaborative && (
                        <MenuItem
                          icon={<Document20Regular />}
                          disabled={locked || state.busy || app.busy}
                          onClick={() => {
                            const root = app.root;
                            void controller
                              .navigate({ kind: "close_project" })
                              .then(() => {
                                if (!shell.snapshot().projectId)
                                  setSvnRegister({ root, kind: "copy" });
                              });
                          }}
                        >
                          {text("svn.registerExisting")}
                        </MenuItem>
                      )}
                      <MenuDivider />
                      <MenuItem
                        icon={<ArrowSync20Regular />}
                        onClick={() => void shell.checkStatus()}
                      >
                        {text("app.message03")}
                      </MenuItem>
                      <MenuItem
                        icon={<Info16Regular />}
                        disabled={
                          !app.ready || locked || state.busy || app.busy
                        }
                        onClick={() => shell.showHealth()}
                      >
                        {text("health.menu")}
                      </MenuItem>
                      <MenuItem
                        icon={<ArrowSync20Regular />}
                        disabled={locked || state.busy || app.busy}
                        onClick={() => void shell.showBackupCenter()}
                      >
                        {text("backup.manage")}
                      </MenuItem>
                      <MenuItem
                        icon={<History20Regular />}
                        disabled={!app.ready || locked}
                        onClick={() => void controller.showCenter()}
                      >
                        {text("whole.center")}
                      </MenuItem>
                      <MenuDivider />
                      <MenuItem
                        icon={<DismissCircle20Regular />}
                        disabled={locked || svnUpdating || state.busy}
                        onClick={() =>
                          void controller.navigate({ kind: "close_project" })
                        }
                      >
                        {text("app.message10")}
                      </MenuItem>
                      <MenuItem
                        icon={<Power20Regular />}
                        disabled={
                          !app.ready || locked || svnUpdating || app.busy
                        }
                        onClick={() => void shell.requestClose()}
                      >
                        {text("app.message04")}
                      </MenuItem>
                    </>
                  ) : (
                    <>
                      <MenuItem
                        icon={<FolderOpen20Regular />}
                        disabled={!app.ready || app.busy || app.startupLoading}
                        onClick={() => setSvnEntry("checkout")}
                      >
                        {text("svn.checkout")}
                      </MenuItem>
                      <MenuDivider />
                      <MenuItem
                        icon={<ArrowSync20Regular />}
                        onClick={() => void shell.checkStatus()}
                      >
                        {text("app.message03")}
                      </MenuItem>
                      <MenuItem
                        icon={<Info16Regular />}
                        disabled={!app.ready || app.busy || app.startupLoading}
                        onClick={() => shell.showHealth()}
                      >
                        {text("health.menu")}
                      </MenuItem>
                      <MenuItem
                        icon={<ArrowSync20Regular />}
                        disabled={
                          !app.ready ||
                          app.busy ||
                          app.picking ||
                          app.startupLoading
                        }
                        onClick={() => void shell.showRestoreFromBackup()}
                      >
                        {text("backup.restoreFromHome")}
                      </MenuItem>
                      <MenuDivider />
                      <MenuItem
                        icon={<Power20Regular />}
                        disabled={!app.ready || app.busy || app.startupLoading}
                        onClick={() => void shell.requestClose()}
                      >
                        {text("app.message04")}
                      </MenuItem>
                    </>
                  )}
                </MenuList>
              </MenuPopover>
            </Menu>
            <Menu>
              <MenuTrigger disableButtonEnhancement>
                <Button ref={aboutTrigger} type="button" appearance="subtle">
                  {text("about.menu")}
                </Button>
              </MenuTrigger>
              <MenuPopover>
                <MenuList className="compact-command-menu">
                  <MenuItem
                    icon={<Info16Regular />}
                    disabled={
                      locked || !!app.projectData || !!app.health || aboutOpen
                    }
                    onClick={() => setAboutOpen(true)}
                  >
                    {text("about.title")}
                  </MenuItem>
                  <MenuItem
                    icon={<ArrowSync20Regular />}
                    disabled={
                      (locked && updateStatus?.phase !== "install_failed") ||
                      aboutOpen ||
                      updateOpen
                    }
                    onClick={() => {
                      setUpdateOpen(true);
                      void updater.refresh();
                    }}
                  >
                    {text("update.menu")}
                  </MenuItem>
                </MenuList>
              </MenuPopover>
            </Menu>
            {app.project && !ready && !initializationFailed && (
              <FloatingNotice>
                {projectLabel(app.project, app.closing)}
              </FloatingNotice>
            )}
            <SvnToolbar
              root={
                ready && !app.closing && !app.project?.shutdown.joined
                  ? app.root
                  : ""
              }
              collaborative={collaborative}
              generation={shell.projectGeneration()}
              controller={controller}
              openConnection={() => setSvnEntry("connect")}
              openCommit={() => setSvnCommitTarget(null)}
              commitRevision={svnCommitRevision}
              localObservation={documentState.svnLocalObservation}
              templateObservation={app.rows}
              resourceObservation={app.assetInspection}
              onUpdatingChange={setSvnUpdating}
              onConnectionVerified={() => setSvnConnectionFailed(false)}
              onStatus={setSvnStatus}
              connectionFailed={svnConnectionFailed}
              sessionRevision={svnSessionRevision}
              blocked={!!locked || !!state.busy || !!app.busy}
            />
          </header>
          <FollowUp controller={shell} />
          <nav className="mode-ribbon" aria-label={text("documents.modes")}>
            {app.project &&
              (
                [
                  ["documents", "documents.title", <Document20Regular />],
                  ["glossary", "glossary.title", <BookOpen20Regular />],
                  [
                    "templates",
                    "documents.templates",
                    <DocumentBulletList20Regular />,
                  ],
                  ["resources", "resources.title", <Attach20Regular />],
                  ["search", "documents.searchArea", <Search20Regular />],
                  ["trash", "documents.trash", <Delete20Regular />],
                ] as const
              ).map(([item, label, icon]) => (
                <Tooltip key={item} content={text(label)} relationship="label">
                  <Button
                    type="button"
                    appearance="subtle"
                    aria-pressed={mode === item}
                    disabled={locked}
                    icon={icon}
                    onClick={() => {
                      setMode(item);
                      if (state.center)
                        void controller.navigate({ kind: "browse" });
                    }}
                  />
                </Tooltip>
              ))}
            <Tooltip content="실행 기록" relationship="label">
              <Button
                type="button"
                appearance="subtle"
                className={`mode-log-button${activityLog.attention ? " needs-attention" : ""}`}
                aria-label={
                  activityLog.attention
                    ? "실행 기록 · 확인할 오류 있음"
                    : "실행 기록"
                }
                aria-pressed={logOpen}
                icon={<History20Regular />}
                onClick={() => {
                  if (logOpen) {
                    activityLog.markRead();
                    setLogOpen(false);
                    focus.current.restore();
                  } else {
                    activityLog.markRead();
                    focus.current.prepare();
                    setLogOpen(true);
                  }
                }}
              />
            </Tooltip>
          </nav>
          {logOpen && (
            <ActivityLog
              noticeKey={logNoticeKey}
              events={activityLog.events}
              droppedEvents={activityLog.droppedEvents}
              close={() => {
                activityLog.markRead();
                setLogOpen(false);
                focus.current.restore();
              }}
            />
          )}
          {(state.error || app.error) && (
            <FloatingNotice
              intent="error"
              eventId={state.error || errorNoticeIdentity}
              scope={templateScope}
              isCurrent={() =>
                `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` ===
                  templateScope &&
                shell.snapshot().errorEvent === app.errorEvent
              }
            >
              <FloatingNoticeContent>
                {state.error || app.error}
                {!state.error &&
                  app.error &&
                  (app.errorDetail || app.projectId) && (
                    <details>
                      <summary>{text("error.details")}</summary>
                      {app.errorDetail && <code>{app.errorDetail}</code>}
                      {app.projectId && (
                        <Button
                          type="button"
                          size="small"
                          onClick={() => shell.showHealth()}
                        >
                          {text("error.openDiagnostics")}
                        </Button>
                      )}
                    </details>
                  )}
              </FloatingNoticeContent>
            </FloatingNotice>
          )}
          {documentState.error && (
            <FloatingNotice intent="error">
              {documentState.error}
            </FloatingNotice>
          )}
          {app.settingsNotice && (
            <FloatingNotice
              eventId={app.settingsNotice}
              scope={templateScope}
              isCurrent={() =>
                shell.snapshot().settingsNotice === app.settingsNotice &&
                `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` ===
                  templateScope
              }
              intent={
                app.settingsNotice.kind === "durability_uncertain"
                  ? "warning"
                  : "error"
              }
            >
              <FloatingNoticeContent>
                <span>{app.settingsNotice.message}</span>
                <div className="actions">
                  <Button
                    type="button"
                    size="small"
                    disabled={app.busy || app.startupLoading || app.picking}
                    onClick={() => void shell.retryProjectSettings()}
                  >
                    {text("project.retrySettings")}
                  </Button>
                </div>
              </FloatingNoticeContent>
            </FloatingNotice>
          )}
          {!app.project && (
            <div className="home-brand">
              <img
                className="brand-main"
                src="/brand/worldbuild-tool-logo.png"
                alt={text("brand.main")}
              />
              <section className="project-open">
                <h1>{text("project.startTitle")}</h1>
                <p>{text("project.startHelp")}</p>
                {(legacyHandoff?.state === "failed" ||
                  legacyHandoff?.state === "needs_attention") && (
                  <FloatingNotice intent="warning" eventId={legacyHandoff}>
                    <FloatingNoticeContent>
                      이전 설치본의 보관 입력을 모두 외부에 확인하지 못했습니다.
                      이전 설치본을 제거하지 말고 보관 상태를 다시 확인해
                      주세요.
                      <div className="actions">
                        <Button
                          type="button"
                          size="small"
                          disabled={legacyRetrying}
                          onClick={() => void retryLegacyHandoff()}
                        >
                          보관 재확인
                        </Button>
                      </div>
                    </FloatingNoticeContent>
                  </FloatingNotice>
                )}
                <div className="project-start-actions">
                  <Button
                    type="button"
                    appearance="primary"
                    disabled={
                      !app.ready ||
                      locked ||
                      app.busy ||
                      app.startupLoading ||
                      !!app.projectId
                    }
                    onClick={() => {
                      setRegisterNew(false);
                      shell.showNewProject();
                    }}
                  >
                    {text("project.create")}
                  </Button>
                  <Button
                    type="button"
                    disabled={
                      !app.ready ||
                      locked ||
                      app.busy ||
                      app.startupLoading ||
                      !!app.projectId
                    }
                    onClick={() =>
                      void shell.openExistingProject(openAsDefault)
                    }
                  >
                    {text("app.message07")}
                  </Button>
                  <Button
                    type="button"
                    disabled={
                      !app.ready || locked || app.busy || app.startupLoading
                    }
                    onClick={() => setSvnEntry("checkout")}
                  >
                    {text("svn.checkout")}
                  </Button>
                </div>
                <Checkbox
                  checked={openAsDefault}
                  disabled={app.startupLoading || app.busy || app.picking}
                  label={text("project.openAsDefault")}
                  onChange={(_, data) =>
                    setOpenAsDefault(data.checked === true)
                  }
                />
                {app.startupLoading &&
                  (!updateStatus || updateStatus.startupComplete) && (
                    <FloatingNotice>
                      {text("project.defaultChecking")}
                    </FloatingNotice>
                  )}
                {app.startupFailure && (
                  <FloatingNotice
                    intent="error"
                    eventId={startupNoticeIdentity}
                    scope={templateScope}
                    isCurrent={() =>
                      shell.snapshot().startupFailureEvent ===
                        app.startupFailureEvent &&
                      `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` ===
                        templateScope
                    }
                  >
                    <FloatingNoticeContent>
                      <span>{app.startupFailure}</span>
                      <div className="actions">
                        {app.startupFailureKind === "settings_read" ? (
                          <Button
                            type="button"
                            size="small"
                            disabled={app.busy || app.picking}
                            onClick={() => void shell.retryProjectSettings()}
                          >
                            {text("project.retrySettings")}
                          </Button>
                        ) : (
                          <Button
                            type="button"
                            size="small"
                            disabled={
                              app.busy || app.picking || !app.defaultProjectRoot
                            }
                            onClick={() => void shell.retryDefaultProject()}
                          >
                            {text("project.retryDefault")}
                          </Button>
                        )}
                        <Button
                          type="button"
                          size="small"
                          disabled={app.busy || app.picking}
                          onClick={() =>
                            void shell.openExistingProject(openAsDefault)
                          }
                        >
                          {text("project.chooseAnother")}
                        </Button>
                        <Button
                          type="button"
                          size="small"
                          disabled={app.busy || app.picking}
                          onClick={() => void shell.clearDefaultProject()}
                        >
                          {text("project.clearDefault")}
                        </Button>
                      </div>
                    </FloatingNoticeContent>
                  </FloatingNotice>
                )}
              </section>
              <footer className="brand-credit">
                <span>{text("brand.developedBy")}</span>
                <div>
                  <img
                    src="/brand/dreamrugi-logo.png"
                    alt={text("brand.dreamrugi")}
                  />
                  <img
                    src="/brand/chatgpt-logo.png"
                    alt={text("brand.chatgpt")}
                  />
                </div>
              </footer>
            </div>
          )}
          {app.project && !ready && !app.closing && (
            <div className="actions">
              <Button
                type="button"
                disabled={locked || app.busy}
                onClick={() => void shell.recover()}
              >
                {text("app.message11")}
              </Button>
              {["initialization_failed", "project_not_empty"].includes(
                app.project.error?.code ?? "",
              ) && (
                <Button
                  type="button"
                  disabled={locked || app.busy}
                  onClick={() =>
                    void shell.correctProjectPath().then((corrected) => {
                      if (corrected) void shell.openExistingProject();
                    })
                  }
                >
                  {text("project.correctPath")}
                </Button>
              )}
            </div>
          )}
          {app.project?.shutdown.joined &&
            app.project.shutdown.normalExitAllowed &&
            !app.closing && (
              <Button
                type="button"
                disabled={locked}
                onClick={() => void shell.retireProject()}
              >
                {text("app.message12")}
              </Button>
            )}
          {app.project && (
            <DocumentWorkspace
              controller={controller.documents}
              readOnly={false}
              collaborative={collaborative}
              svnStatus={collaborative ? svnStatus : null}
              onCommitDocument={(id) => setSvnCommitTarget(id)}
              onPdfCompleted={(cleanupWarning) => {
                setPdfCompletionPause(null);
                setPdfCompletion({
                  id: ++pdfCompletionSequence.current,
                  scope: templateScope,
                  cleanupWarning,
                });
              }}
              hidden={
                mode === "templates" ||
                mode === "glossary" ||
                mode === "resources" ||
                mode === "trash"
              }
              searchMode={mode === "search"}
            />
          )}
          {app.project && (mode === "resources" || mode === "trash") && (
            <ProjectFiles
              mode={mode}
              shell={shell}
              documents={controller.documents}
              svnStatus={collaborative ? svnStatus : null}
            />
          )}
          {app.project && mode === "glossary" && (
            <div
              className={
                "glossary-workspace" +
                (glossaryNavigationCollapsed ? " navigation-collapsed" : "")
              }
              style={
                {
                  "--glossary-navigation-width": `${glossaryNavigationWidth}px`,
                } as CSSProperties
              }
            >
              <aside
                className="glossary-filter-panel"
                aria-label={text("glossary.templateFilter")}
              >
                <header>
                  <h2 hidden={glossaryNavigationCollapsed}>
                    {text("glossary.title")}
                  </h2>
                  <Tooltip
                    content={text(
                      glossaryNavigationCollapsed
                        ? "navigation.panelExpand"
                        : "navigation.panelCollapse",
                    )}
                    relationship="label"
                  >
                    <Button
                      type="button"
                      appearance="subtle"
                      aria-expanded={!glossaryNavigationCollapsed}
                      icon={
                        glossaryNavigationCollapsed ? (
                          <PanelLeftExpand20Regular />
                        ) : (
                          <PanelLeftContract20Regular />
                        )
                      }
                      onClick={() =>
                        setGlossaryNavigationCollapsed((value) => !value)
                      }
                    />
                  </Tooltip>
                </header>
                {!glossaryNavigationCollapsed && (
                  <DocumentGlossary
                    list={documentState.list}
                    templates={app.rows}
                    template={documentState.ui.glossaryTemplate}
                    selectTemplate={(value) =>
                      controller.documents.glossaryTemplate(value)
                    }
                    open={() => undefined}
                    filterOnly
                  />
                )}
              </aside>
              <div
                className="glossary-navigation-resizer"
                role="separator"
                tabIndex={glossaryNavigationCollapsed ? -1 : 0}
                aria-label={text("navigation.panelResize")}
                aria-orientation="vertical"
                aria-valuemin={220}
                aria-valuemax={NAVIGATION_MAX}
                aria-valuenow={glossaryNavigationWidth}
                onPointerDown={(event) => {
                  if (glossaryNavigationCollapsed) return;
                  glossaryResizeStart.current = {
                    x: event.clientX,
                    width: glossaryNavigationWidth,
                  };
                  event.currentTarget.setPointerCapture(event.pointerId);
                  event.preventDefault();
                }}
                onPointerMove={(event) => {
                  if (!glossaryResizeStart.current) return;
                  setGlossaryNavigationWidth(
                    Math.max(
                      220,
                      Math.min(
                        NAVIGATION_MAX,
                        glossaryResizeStart.current.width +
                          event.clientX -
                          glossaryResizeStart.current.x,
                      ),
                    ),
                  );
                }}
                onPointerUp={(event) => {
                  glossaryResizeStart.current = null;
                  event.currentTarget.releasePointerCapture(event.pointerId);
                }}
              />
              <section
                className="glossary-main-panel"
                aria-label={text("glossary.title")}
              >
                <DocumentGlossary
                  list={documentState.list}
                  templates={app.rows}
                  template={documentState.ui.glossaryTemplate}
                  selectTemplate={(value) =>
                    controller.documents.glossaryTemplate(value)
                  }
                  open={(id) => {
                    setMode("documents");
                    void controller.documents.openReferenceTarget(id);
                  }}
                  listOnly
                />
              </section>
            </div>
          )}
          {app.project && mode === "templates" && (
            <div
              className={
                "workspace template-workspace" +
                (templateNavigationCollapsed ? " navigation-collapsed" : "")
              }
              style={
                {
                  "--template-navigation-width": `${templateNavigationWidth}px`,
                } as CSSProperties
              }
            >
              <section
                className="panel template-list"
                aria-label={text("app.message13")}
              >
                <header className="template-list-heading">
                  <h2 hidden={templateNavigationCollapsed}>
                    {text("app.message13")}
                  </h2>
                  <div className="actions">
                    <Tooltip
                      content={text(
                        templateNavigationCollapsed
                          ? "navigation.panelExpand"
                          : "navigation.panelCollapse",
                      )}
                      relationship="label"
                    >
                      <Button
                        type="button"
                        appearance="subtle"
                        aria-expanded={!templateNavigationCollapsed}
                        aria-controls="template-navigation-content"
                        icon={
                          templateNavigationCollapsed ? (
                            <PanelLeftExpand20Regular />
                          ) : (
                            <PanelLeftContract20Regular />
                          )
                        }
                        onClick={() =>
                          setTemplateNavigationCollapsed((value) => !value)
                        }
                      />
                    </Tooltip>
                    <Tooltip
                      content={text("app.message15")}
                      relationship="label"
                    >
                      <Button
                        type="button"
                        appearance="subtle"
                        icon={<ArrowClockwise20Regular />}
                        aria-label={text("app.message15")}
                        hidden={templateNavigationCollapsed}
                        disabled={!ready || locked || app.busy}
                        onClick={() => void shell.refresh()}
                      />
                    </Tooltip>
                    <Tooltip
                      content={text("app.message14")}
                      relationship="label"
                    >
                      <Button
                        type="button"
                        appearance="subtle"
                        icon={<Add20Regular />}
                        aria-label={text("app.message14")}
                        hidden={templateNavigationCollapsed}
                        disabled={!ready || locked || state.busy || app.busy}
                        onClick={() =>
                          void controller.navigate({ kind: "new" })
                        }
                      />
                    </Tooltip>
                  </div>
                </header>
                <div
                  id="template-navigation-content"
                  className="template-navigation-content"
                  hidden={templateNavigationCollapsed}
                >
                  {!!currentTemplateSelection.length && (
                    <div className="template-selection-actions">
                      <span>
                        {text("documents.selectedCount", {
                          count: String(currentTemplateSelection.length),
                        })}
                      </span>
                    </div>
                  )}
                  {app.listState === "loading" && (
                    <p>{text("app.message16")}</p>
                  )}
                  {app.listState === "failed" && (
                    <InlineNotice kind="error">
                      {text("app.message17")}
                    </InlineNotice>
                  )}
                  {app.listState === "ready" && !rows.length && (
                    <p>{text("app.message18")}</p>
                  )}
                  <ul>
                    {rows.map((row) => {
                      const targets =
                        currentTemplateSelection.includes(row.id) &&
                        currentTemplateSelection.length > 1
                          ? currentTemplateSelection
                          : [row.id];
                      return (
                        <li key={row.id}>
                          <Menu openOnContext>
                            <MenuTrigger disableButtonEnhancement>
                              <Button
                                type="button"
                                appearance="subtle"
                                className="template-choice"
                                aria-pressed={
                                  state.draft
                                    ? state.draft.status.artifact === row.id
                                    : app.selection?.content.id === row.id
                                }
                                aria-selected={currentTemplateSelection.includes(
                                  row.id,
                                )}
                                disabled={locked || state.busy || app.busy}
                                onClick={(event) =>
                                  chooseTemplate(row.id, event)
                                }
                                onKeyDown={(event) => {
                                  if (
                                    (event.key === " " ||
                                      event.key === "Enter") &&
                                    (event.ctrlKey ||
                                      event.metaKey ||
                                      event.shiftKey)
                                  ) {
                                    event.preventDefault();
                                    chooseTemplate(row.id, event);
                                  }
                                }}
                              >
                                <strong>
                                  {row.name || text("field.emptyLabel")}
                                </strong>
                                {collaborative && (
                                  <SvnItemStatus
                                    status={svnStatus}
                                    paths={[`templates/${row.id}.json`]}
                                  />
                                )}
                                {row.lifecycle !== "Active" && (
                                  <span className="template-status">
                                    {lifecycleLabel(row.lifecycle)}
                                  </span>
                                )}
                              </Button>
                            </MenuTrigger>
                            <MenuPopover>
                              <MenuList>
                                <MenuItem
                                  icon={<Delete20Regular />}
                                  className="destructive-menu-item"
                                  disabled={
                                    locked ||
                                    state.busy ||
                                    app.busy ||
                                    !targets.some((id) =>
                                      rows.some(
                                        (candidate) =>
                                          candidate.id === id &&
                                          candidate.lifecycle === "Active",
                                      ),
                                    )
                                  }
                                  onClick={() => {
                                    if (app.projectId)
                                      setTemplateTrashConfirm({
                                        scope: templateScope,
                                        ids: [...targets],
                                      });
                                  }}
                                >
                                  {text("documents.toTrash")}
                                </MenuItem>
                              </MenuList>
                            </MenuPopover>
                          </Menu>
                        </li>
                      );
                    })}
                  </ul>
                </div>
              </section>
              <div
                className="template-navigation-resizer"
                role="separator"
                tabIndex={templateNavigationCollapsed ? -1 : 0}
                aria-label={text("navigation.panelResize")}
                aria-orientation="vertical"
                aria-valuemin={220}
                aria-valuemax={NAVIGATION_MAX}
                aria-valuenow={templateNavigationWidth}
                onPointerDown={(event) => {
                  if (templateNavigationCollapsed) return;
                  templateResizeStart.current = {
                    x: event.clientX,
                    width: templateNavigationWidth,
                  };
                  event.currentTarget.setPointerCapture(event.pointerId);
                  event.preventDefault();
                }}
                onPointerMove={(event) => {
                  if (!templateResizeStart.current) return;
                  setTemplateNavigationWidth(
                    Math.max(
                      220,
                      Math.min(
                        NAVIGATION_MAX,
                        templateResizeStart.current.width +
                          event.clientX -
                          templateResizeStart.current.x,
                      ),
                    ),
                  );
                }}
                onPointerUp={(event) => {
                  templateResizeStart.current = null;
                  event.currentTarget.releasePointerCapture(event.pointerId);
                }}
              />
              <section
                className="panel detail"
                aria-label={text("app.message20")}
              >
                {state.draft ? (
                  <WholeTemplate
                    key={state.draft.status.owner}
                    controller={controller}
                    draft={state.draft}
                    locked={locked}
                  />
                ) : (
                  <>
                    {app.selection &&
                      app.selection.content.lifecycle !== "Deleted" && (
                        <>
                          <ReadonlyTemplate
                            template={app.selection!.content}
                            actions={
                              app.selection!.content.lifecycle === "Active" ? (
                                <>
                                  <FormatControl
                                    key={app.selection!.content.id}
                                    shell={shell}
                                    kind="template"
                                    artifact={app.selection!.content.id}
                                    locked={
                                      locked ||
                                      state.busy ||
                                      app.busy ||
                                      collaborative
                                    }
                                    changed={() =>
                                      controller.navigate({
                                        kind: "format",
                                        id: app.selection!.content.id,
                                      })
                                    }
                                  />
                                  <IconCommand
                                    label={text("documentEdit.begin")}
                                    icon={<Edit20Regular />}
                                    disabled={
                                      locked ||
                                      state.busy ||
                                      (app.selection!.content.schema ?? 4) < 4
                                    }
                                    onClick={() =>
                                      void controller.navigate({
                                        kind: "select",
                                        id: app.selection!.content.id,
                                      })
                                    }
                                  />
                                  <IconCommand
                                    label={text("template.duplicate")}
                                    icon={<Copy20Regular />}
                                    disabled={
                                      !ready ||
                                      locked ||
                                      state.busy ||
                                      !!app.templateAction
                                    }
                                    onClick={() =>
                                      void controller.navigate({
                                        kind: "duplicate",
                                      })
                                    }
                                  />
                                </>
                              ) : undefined
                            }
                          />
                        </>
                      )}
                  </>
                )}
                {app.templateAction && (
                  <TemplateManagement
                    key={app.templateAction.generation}
                    controller={shell}
                    locked={locked || state.busy}
                  />
                )}
                {!state.draft &&
                  !app.templateAction &&
                  (!app.selection ||
                    app.selection.content.lifecycle === "Deleted") && (
                    <EmptyState>{text("app.message31")}</EmptyState>
                  )}
              </section>
            </div>
          )}
        </div>
        {state.center && <RecoveryCenter controller={controller} />}
        <ProjectHealth
          controller={shell}
          documentList={documentState.list}
          documents={(documentState.list?.documents ?? []).map((row) => ({
            id: row.id,
            name: row.name,
            template: row.template,
          }))}
          openResources={(keys) => {
            shell.closeHealth();
            shell.focusFileManager("resources", keys);
            setMode("resources");
          }}
          openTrash={(keys) => {
            shell.closeHealth();
            shell.focusFileManager("trash", keys);
            setMode("trash");
          }}
        />
        <Dialog
          open={
            !!templateTrashConfirm &&
            templateTrashConfirm.scope === templateScope
          }
          modalType="modal"
          onOpenChange={(_, data) => {
            if (!data.open) setTemplateTrashConfirm(null);
          }}
        >
          <DialogSurface>
            <DialogBody>
              <DialogTitle>{text("templates.batchTrashTitle")}</DialogTitle>
              <DialogContent>
                {text("templates.batchTrashWarning", {
                  count: String(
                    rows.filter(
                      (row) =>
                        templateTrashConfirm?.ids.includes(row.id) &&
                        row.lifecycle === "Active",
                    ).length,
                  ),
                })}
              </DialogContent>
              <DialogActions>
                <Button
                  type="button"
                  danger
                  onClick={() => {
                    const pending = templateTrashConfirm;
                    if (
                      !pending ||
                      pending.scope !== templateScope ||
                      `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` !==
                        pending.scope
                    ) {
                      setTemplateTrashConfirm(null);
                      return;
                    }
                    const ids = rows
                      .filter(
                        (row) =>
                          pending.ids.includes(row.id) &&
                          row.lifecycle === "Active",
                      )
                      .map((row) => row.id);
                    setTemplateTrashConfirm(null);
                    void controller
                      .trashTemplates(ids, {
                        project: app.projectId!,
                        generation: shell.projectGeneration(),
                      })
                      .then(() => {
                        if (
                          selectedTemplates.scope === pending.scope &&
                          `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` ===
                            pending.scope
                        ) {
                          setSelectedTemplates({
                            scope: pending.scope,
                            ids: [],
                          });
                          templateSelectionAnchor.current = null;
                        }
                      });
                  }}
                >
                  {text("documents.toTrash")}
                </Button>
                <Button
                  type="button"
                  onClick={() => setTemplateTrashConfirm(null)}
                >
                  {text("common.cancel")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
        <Dialog
          open={!!app.newProject}
          modalType="modal"
          onOpenChange={(_, data) => {
            if (!data.open) shell.cancelNewProject();
          }}
        >
          <DialogSurface aria-describedby="new-project-description">
            <DialogBody>
              <DialogTitle>{text("project.newTitle")}</DialogTitle>
              <DialogContent>
                <p id="new-project-description">{text("project.newHelp")}</p>
                <Field
                  label={text("project.name")}
                  validationState={app.newProject?.error ? "error" : "none"}
                  validationMessage={app.newProject?.error}
                >
                  <Input
                    autoFocus
                    value={app.newProject?.name ?? ""}
                    disabled={app.busy || app.picking}
                    autoComplete="off"
                    spellCheck={false}
                    onChange={(event) =>
                      shell.setNewProjectName(event.target.value)
                    }
                    onKeyDown={(event) => {
                      if (
                        event.key === "Enter" &&
                        !event.nativeEvent.isComposing
                      ) {
                        event.preventDefault();
                        void createProject();
                      }
                    }}
                  />
                </Field>
                <RadioGroup
                  value={registerNew ? "collaborative" : "personal"}
                  onChange={(_, data) =>
                    setRegisterNew(data.value === "collaborative")
                  }
                >
                  <Radio value="personal" label={text("svn.personalProject")} />
                  <Radio
                    value="collaborative"
                    label={text("svn.collaborativeProject")}
                  />
                </RadioGroup>
              </DialogContent>
              <DialogActions>
                <Button
                  type="button"
                  appearance="primary"
                  disabled={app.busy || app.picking}
                  onClick={() => void createProject()}
                >
                  {text(
                    app.picking ? "picker.pending" : "project.chooseLocation",
                  )}
                </Button>
                <Button
                  type="button"
                  disabled={app.busy || app.picking}
                  onClick={() => shell.cancelNewProject()}
                >
                  {text("app.message26")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
        <Dialog
          open={!!app.projectData}
          modalType="modal"
          onOpenChange={(_, data) => {
            if (!data.open) shell.closeProjectData();
          }}
        >
          <DialogSurface
            className="project-data-dialog"
            aria-describedby="project-data-description"
          >
            <DialogBody>
              <DialogTitle>
                {text(
                  app.projectData?.kind === "backups"
                    ? "backup.title"
                    : app.projectData?.kind === "copy"
                      ? "backup.copyTitle"
                      : "backup.restoreNewTitle",
                )}
              </DialogTitle>
              <DialogContent>
                {app.projectData?.confirm ? (
                  <>
                    <h3>
                      {text(
                        app.projectData.confirm === "delete"
                          ? "backup.deleteTitle"
                          : app.projectData.confirm === "purge_deleted"
                            ? "backup.deletedPurgeTitle"
                            : "backup.currentConfirmTitle",
                      )}
                    </h3>
                    <p id="project-data-description">
                      {text(
                        app.projectData.confirm === "delete"
                          ? "backup.deleteHelp"
                          : app.projectData.confirm === "purge_deleted"
                            ? "backup.deletedPurgeHelp"
                            : "backup.currentConfirmHelp",
                      )}
                    </p>
                  </>
                ) : app.projectData?.kind === "backups" ? (
                  <>
                    {app.projectData.error &&
                      !(
                        app.projectData.errorSource === "cleanup" &&
                        app.projectData.deletedCleanupWarning
                      ) &&
                      app.projectData.errorSource !== "cleanup" && (
                        <FloatingNotice
                          intent="warning"
                          eventId={backupNoticeIdentity}
                          scope={`${templateScope}:${app.projectData.dialogGeneration ?? ""}:${app.projectData.storage ?? ""}:${app.projectData.selected ?? ""}`}
                          isCurrent={() => {
                            const current = shell.snapshot().projectData;
                            const prior = app.projectData;
                            return (
                              !!prior &&
                              current?.kind === "backups" &&
                              current.noticeEvent === prior.noticeEvent &&
                              current.dialogGeneration ===
                                prior.dialogGeneration &&
                              current.storage === prior.storage &&
                              current.selected === prior.selected &&
                              `${shell.projectGeneration()}:${shell.snapshot().projectId ?? ""}` ===
                                templateScope
                            );
                          }}
                        >
                          <FloatingNoticeContent>
                            {app.projectData.error}
                            <details>
                              <summary>{text("error.details")}</summary>
                              {app.projectData.detail && (
                                <code>{app.projectData.detail}</code>
                              )}
                              <Button
                                type="button"
                                size="small"
                                onClick={() => shell.openBackupDiagnostics()}
                              >
                                {text("error.openDiagnostics")}
                              </Button>
                            </details>
                          </FloatingNoticeContent>
                        </FloatingNotice>
                      )}
                    {app.projectData.deletedCleanupWarning && (
                      <FloatingNotice intent="warning">
                        <FloatingNoticeContent>
                          {text("backup.deletedCleanupRequired")}
                          <details>
                            <summary>{text("error.details")}</summary>
                            {app.projectData.deletedCleanupFacts?.length ? (
                              <ul>
                                {app.projectData.deletedCleanupFacts.map(
                                  (fact) => (
                                    <li key={`${fact.id}:${fact.operation}`}>
                                      <code>{fact.id}</code>{" "}
                                      <code>{fact.warning}</code>
                                    </li>
                                  ),
                                )}
                                {app.projectData.deletedListWarning && (
                                  <li>
                                    <code>
                                      {app.projectData.deletedListWarning}
                                    </code>
                                  </li>
                                )}
                              </ul>
                            ) : (
                              <code>
                                {app.projectData.deletedCleanupWarning}
                              </code>
                            )}
                          </details>
                        </FloatingNoticeContent>
                      </FloatingNotice>
                    )}
                    <div
                      className="backup-view-switch"
                      role="group"
                      aria-label={text("backup.title")}
                    >
                      <Button
                        type="button"
                        appearance="secondary"
                        aria-pressed={app.projectData.view !== "deleted"}
                        disabled={app.busy}
                        onClick={() => {
                          if (app.projectData?.view === "deleted")
                            void shell.showDeletedBackups();
                        }}
                      >
                        {text("backup.activeEntry")}
                      </Button>
                      <Button
                        type="button"
                        appearance="secondary"
                        aria-pressed={app.projectData.view === "deleted"}
                        disabled={app.busy}
                        onClick={() => {
                          if (app.projectData?.view !== "deleted")
                            void shell.showDeletedBackups();
                        }}
                      >
                        {text("backup.deletedEntry")}
                      </Button>
                    </div>
                    {app.projectData.view !== "deleted" && (
                      <div className="backup-create-section">
                        <div className="backup-location-row">
                          <Button
                            type="button"
                            disabled={app.busy || app.picking}
                            onClick={() => void shell.chooseBackupStorage()}
                          >
                            {text("backup.location")}
                          </Button>
                          <span
                            className="backup-location-value"
                            role="status"
                            title={
                              app.projectData.storage ??
                              text("backup.locationMissing")
                            }
                          >
                            {app.projectData.storage ??
                              text("backup.locationMissing")}
                          </span>
                        </div>
                        <Field label={text("backup.label")}>
                          <Input
                            value={app.projectData.label}
                            disabled={app.busy || app.picking}
                            onChange={(event) =>
                              shell.setBackupLabel(event.target.value)
                            }
                          />
                        </Field>
                        <Button
                          type="button"
                          appearance="primary"
                          disabled={app.busy || app.picking}
                          onClick={() => void shell.createBackup()}
                        >
                          {text("backup.create")}
                        </Button>
                        <p
                          id="project-data-description"
                          className="backup-help"
                        >
                          <Info16Regular aria-hidden />
                          <span>{text("backup.help")}</span>
                        </p>
                      </div>
                    )}
                    {app.projectData.view !== "deleted" ? (
                      <section
                        className="backup-history"
                        aria-labelledby="backup-list-title"
                      >
                        <h3 id="backup-list-title">
                          {text("backup.listTitle")}
                        </h3>
                        <div className="backup-list">
                          {!app.projectData.backups.length && (
                            <p>{text("backup.empty")}</p>
                          )}
                          {(() => {
                            const unnamed = unnamedBackupNumbers(
                              app.projectData.backups,
                            );
                            return (
                              <BackupTable
                                rows={app.projectData.backups.map((backup) => ({
                                  id: backup.id,
                                  name:
                                    backup.label ||
                                    text("backup.unnamed", {
                                      number: String(
                                        backup.unnamedOrdinal ??
                                          unnamed.get(backup.id) ??
                                          "-",
                                      ),
                                    }),
                                  state:
                                    backup.status === "verified"
                                      ? text(
                                          backup.kind === "pre_restore"
                                            ? "backup.preRestore"
                                            : "backup.manual",
                                        )
                                      : text(
                                          backup.status ===
                                            "verification_required"
                                            ? "backup.verificationRequired"
                                            : backup.status === "corrupt"
                                              ? "backup.corruptState"
                                              : "backup.unsupportedState",
                                        ),
                                  date: (
                                    <>
                                      {formatLocalDateTime(backup.createdAtUtc)}
                                      {backup.coverage === "legacy_unknown" &&
                                        ` · ${text("backup.legacyCoverage")}`}
                                    </>
                                  ),
                                  size: backupSize(backup.size),
                                  disabled:
                                    ["corrupt", "unsupported"].includes(
                                      backup.status,
                                    ) || app.busy,
                                }))}
                                selected={app.projectData.selected}
                                select={(id) => void shell.selectBackup(id)}
                              />
                            );
                          })()}
                          {app.projectData.nextCursor && (
                            <Button
                              type="button"
                              disabled={app.busy}
                              onClick={() => void shell.loadMoreBackups()}
                            >
                              {text("backup.loadMore")}
                            </Button>
                          )}
                        </div>
                      </section>
                    ) : (
                      <section
                        className="backup-history"
                        aria-labelledby="deleted-backup-title"
                      >
                        <h3 id="deleted-backup-title">
                          {text("backup.deletedEntry")}
                        </h3>
                        {app.projectData.errorSource === "deleted_list" && (
                          <Button
                            type="button"
                            disabled={app.busy}
                            onClick={() => void shell.refreshDeletedBackups()}
                          >
                            {text("backup.deletedRetry")}
                          </Button>
                        )}
                        <div className="backup-list">
                          {!app.projectData.deletedBackups?.length &&
                            (app.busy ? (
                              <p role="status">
                                {text("backup.deletedLoading")}
                              </p>
                            ) : app.projectData.errorSource ===
                              "deleted_list" ? null : (
                              <p>{text("backup.deletedEmpty")}</p>
                            ))}
                          <BackupTable
                            dateLabel="삭제 / 보관 종료"
                            rows={(app.projectData.deletedBackups ?? []).map(
                              (entry) => ({
                                id: entry.backup.id,
                                name:
                                  entry.backup.label ||
                                  text("backup.unnamedShort"),
                                state: text(
                                  entry.status === "expired"
                                    ? "backup.deletedExpired"
                                    : entry.status === "uncertain"
                                      ? "backup.deletedUncertain"
                                      : "backup.verificationRequired",
                                ),
                                date: (
                                  <>
                                    {formatLocalDateTime(entry.deletedAtUtc)}
                                    <br />
                                    {text("backup.expiresAt")}{" "}
                                    {formatLocalDateTime(entry.expiresAtUtc)}
                                  </>
                                ),
                                size: backupSize(entry.backup.size),
                                disabled:
                                  entry.status === "uncertain" || app.busy,
                              }),
                            )}
                            selected={app.projectData.selectedDeleted}
                            select={(id) => shell.selectDeletedBackup(id)}
                          />
                        </div>
                      </section>
                    )}
                  </>
                ) : (
                  <>
                    <p id="project-data-description">
                      {text(
                        app.projectData?.kind === "copy"
                          ? "backup.copyHelp"
                          : "backup.restoreNewHelp",
                      )}
                    </p>
                    <Field
                      label={text("project.name")}
                      validationState={
                        app.projectData?.error ? "error" : "none"
                      }
                      validationMessage={app.projectData?.error}
                    >
                      <Input
                        autoFocus
                        value={app.projectData?.name ?? ""}
                        disabled={app.busy || app.picking}
                        onChange={(event) =>
                          shell.setProjectDataName(event.target.value)
                        }
                      />
                    </Field>
                    {app.projectData?.kind === "copy" && (
                      <RadioGroup
                        value={registerCopy ? "collaborative" : "personal"}
                        onChange={(_, data) =>
                          setRegisterCopy(data.value === "collaborative")
                        }
                      >
                        <Radio
                          value="personal"
                          label={text("svn.personalCopy")}
                        />
                        <Radio
                          value="collaborative"
                          label={text("svn.collaborativeCopy")}
                        />
                      </RadioGroup>
                    )}
                    {app.projectData?.completedRoot &&
                      app.projectData.kind !== "copy" && (
                        <p role="status">{text("backup.restoredNew")}</p>
                      )}
                  </>
                )}
              </DialogContent>
              <DialogActions className="actions">
                {app.busy && (
                  <Button
                    type="button"
                    onClick={() => void shell.checkStatus()}
                  >
                    {text("app.message03")}
                  </Button>
                )}
                {app.busy && app.projectData?.cancellable && (
                  <Button
                    type="button"
                    onClick={() => void shell.cancelProjectDataOperation()}
                  >
                    {text("backup.cancel")}
                  </Button>
                )}

                {app.projectData?.confirm === "delete" && (
                  <Button
                    type="button"
                    danger
                    disabled={app.busy}
                    onClick={() => void shell.deleteSelectedBackup()}
                  >
                    {text("backup.delete")}
                  </Button>
                )}
                {app.projectData?.confirm === "purge_deleted" && (
                  <Button
                    type="button"
                    danger
                    disabled={app.busy}
                    onClick={() => void shell.purgeSelectedDeletedBackup()}
                  >
                    {text("backup.deletedPurge")}
                  </Button>
                )}
                {app.projectData?.confirm === "restore_current" && (
                  <Button
                    type="button"
                    danger
                    disabled={app.busy}
                    onClick={() => {
                      const selected = shell.selectedBackupRestore();
                      if (selected)
                        void controller.navigate({
                          kind: "restore_current",
                          ...selected,
                        });
                    }}
                  >
                    {text("backup.restoreCurrent")}
                  </Button>
                )}
                {!app.projectData?.confirm &&
                  app.projectData?.kind === "backups" &&
                  (app.projectData.view === "deleted" ? (
                    <>
                      <Button
                        type="button"
                        disabled={
                          !app.projectData.selectedDeleted ||
                          app.busy ||
                          app.projectData.deletedBackups?.find(
                            (row) =>
                              row.backup.id ===
                              app.projectData?.selectedDeleted,
                          )?.status !== "verification_required"
                        }
                        onClick={() =>
                          void shell.restoreSelectedDeletedBackup()
                        }
                      >
                        {text("backup.deletedRestore")}
                      </Button>
                      <Button
                        type="button"
                        danger
                        disabled={!app.projectData.selectedDeleted || app.busy}
                        onClick={() =>
                          shell.requestProjectDataConfirm("purge_deleted")
                        }
                      >
                        {text("backup.deletedPurge")}
                      </Button>
                    </>
                  ) : (
                    <>
                      <Button
                        type="button"
                        disabled={!app.projectData.locator || app.busy}
                        onClick={() => shell.startRestoreNew()}
                      >
                        {text("backup.restoreNew")}
                      </Button>
                      <Button
                        type="button"
                        disabled={
                          !app.projectData.locator ||
                          app.busy ||
                          app.projectData.backups.find(
                            (backup) => backup.id === app.projectData?.selected,
                          )?.coverage !== "complete"
                        }
                        onClick={() =>
                          app.projectData?.backups.find(
                            (backup) => backup.id === app.projectData?.selected,
                          )?.coverage === "complete" &&
                          shell.requestProjectDataConfirm("restore_current")
                        }
                      >
                        {text("backup.restoreCurrent")}
                      </Button>
                      <Button
                        type="button"
                        danger
                        disabled={!app.projectData.locator || app.busy}
                        onClick={() =>
                          shell.requestProjectDataConfirm("delete")
                        }
                      >
                        {text("backup.delete")}
                      </Button>
                    </>
                  ))}
                {!app.projectData?.confirm &&
                  app.projectData?.kind !== "backups" &&
                  !app.projectData?.completedRoot && (
                    <Button
                      type="button"
                      appearance="primary"
                      disabled={app.busy || app.picking}
                      onClick={() =>
                        void shell.confirmProjectDataName(async () => {
                          for (const id of Object.keys(
                            controller.documents.edits.entries,
                          ))
                            await controller.documents.edits.flush(id);
                          return Object.values(
                            controller.documents.edits.entries,
                          ).every(
                            (entry) =>
                              !editDirty(entry) &&
                              !entry.body.composing &&
                              !entry.busy &&
                              !entry.paused,
                          );
                        })
                      }
                    >
                      {text(
                        app.projectData?.kind === "copy"
                          ? "backup.copy"
                          : "backup.restoreNew",
                      )}
                    </Button>
                  )}
                <Button
                  type="button"
                  disabled={app.busy || app.picking}
                  onClick={() => {
                    if (app.projectData?.confirm)
                      shell.cancelProjectDataConfirm();
                    else shell.closeProjectData();
                  }}
                >
                  {text(
                    app.projectData?.confirm
                      ? "backup.backToList"
                      : "app.message26",
                  )}
                </Button>
              </DialogActions>
            </DialogBody>
            <FloatingMessage>{app.feedback && feedbackToast}</FloatingMessage>
          </DialogSurface>
        </Dialog>
        <UpdateDialog
          controller={updater}
          open={updateOpen}
          onClose={() => setUpdateOpen(false)}
        />
        <AboutDialog
          open={aboutOpen}
          close={() => {
            setAboutOpen(false);
            requestAnimationFrame(() => aboutTrigger.current?.focus());
          }}
        />
        <Dialog
          open={prompted}
          surfaceMotion={{
            onMotionFinish: (_, data) => {
              if (data.direction === "exit") focus.current.restore();
            },
          }}
          modalType="alert"
          onOpenChange={(event, data) => {
            event.preventDefault();
            if (data.type === "escapeKeyDown" && !state.busy) {
              if (state.discard) controller.cancelDiscard();
              else void controller.decide("cancel");
            }
          }}
        >
          <DialogSurface
            backdrop={{
              onPointerDown: (event) => {
                event.preventDefault();
                keep.current?.focus();
              },
            }}
            className="confirm"
            aria-describedby="whole-confirm-description"
          >
            <DialogBody>
              <DialogTitle>
                {text(state.discard ? "whole.discardFile" : "whole.leave")}
              </DialogTitle>
              <DialogContent className="confirm-content">
                <p id="whole-confirm-description">
                  {text(
                    state.discard ? "whole.discardFileHelp" : "whole.leaveHelp",
                  )}
                </p>
                {state.error && (
                  <InlineNotice kind="error">{state.error}</InlineNotice>
                )}
                <Button
                  type="button"
                  size="small"
                  onClick={() => void shell.checkStatus()}
                >
                  {text("app.message03")}
                </Button>
              </DialogContent>
              <DialogActions className="actions">
                {state.discard ? (
                  <Button
                    type="button"
                    danger
                    disabled={state.busy}
                    onClick={() => void controller.discardRecovery()}
                  >
                    {text("whole.discardFile")}
                  </Button>
                ) : (
                  <>
                    <Button
                      type="button"
                      appearance="primary"
                      disabled={
                        state.busy ||
                        !state.draft?.loaded ||
                        state.draft.body.composing
                      }
                      onClick={() => void controller.decide("save")}
                    >
                      {text("whole.save")}
                    </Button>
                    <Button
                      type="button"
                      disabled={state.busy || !state.draft?.loaded}
                      onClick={() => void controller.decide("deposit")}
                    >
                      {text("whole.deposit")}
                    </Button>
                    <Button
                      type="button"
                      danger
                      disabled={state.busy}
                      onClick={() => void controller.decide("discard")}
                    >
                      {text("whole.discard")}
                    </Button>
                  </>
                )}
                <Button
                  type="button"
                  ref={keep}
                  disabled={state.busy}
                  onClick={() => {
                    if (state.discard) controller.cancelDiscard();
                    else void controller.decide("cancel");
                  }}
                >
                  {text("whole.keepEditing")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
        {effectivePolicyRoot && (
          <CollaborationPolicyDialog
            key={effectivePolicyRoot}
            root={effectivePolicyRoot}
            currentVersion={updateStatus?.currentVersion}
            canSave={ready && collaborative && app.root === effectivePolicyRoot}
            close={dismissPolicy}
            commit={() => {
              dismissPolicy();
              setSvnCommitTarget(null);
            }}
            openProject={() => {
              const root = effectivePolicyRoot;
              dismissPolicy();
              void controller.navigate({ kind: "open_project", root });
            }}
            home={() => {
              dismissPolicy();
              void controller.navigate({ kind: "close_project" });
            }}
            update={() => {
              dismissPolicy();
              void shell.requestClose();
            }}
          />
        )}
        <SvnDialog
          open={svnEntry !== null}
          entry={svnEntry ?? "connect"}
          onClose={() => setSvnEntry(null)}
          onSessionChanged={(outcome) => {
            setSvnConnectionFailed(outcome === "failure");
            if (outcome !== "failure")
              setSvnSessionRevision((value) => value + 1);
          }}
          controller={shell}
        />
        {svnRegister && (
          <SvnRegisterDialog
            key={svnRegister.root}
            root={svnRegister.root}
            kind={svnRegister.kind}
            sessionRevision={svnSessionRevision}
            close={() => setSvnRegister(null)}
            connect={() => setSvnEntry("connect")}
            openProject={async (root) => {
              await controller.navigate({ kind: "open_project", root });
              if (shell.snapshot().projectId) setSvnRegister(null);
            }}
          />
        )}
        {svnCommitTarget !== undefined && app.project && (
          <SvnCommitDialog
            root={app.root}
            document={svnCommitTarget}
            controller={controller.documents}
            close={() => setSvnCommitTarget(undefined)}
            committed={async (paths) => {
              const failedRelease: string[] = [];
              for (const path of paths) {
                const id = path.match(/^documents\/([^/]+)\.json$/u)?.[1];
                if (id) {
                  controller.documents.edits.committed(id);
                  if (!(await controller.documents.edits.close(id)))
                    failedRelease.push(path);
                }
              }
              setSvnCommitRevision((value) => value + 1);
              return failedRelease;
            }}
          />
        )}
      </main>
    </MediaContext.Provider>
  );
}
