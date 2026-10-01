import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Tooltip,
} from "@fluentui/react-components";
import {
  ArrowClockwise20Regular,
  ArrowDownload20Regular,
  Broom20Regular,
  CheckmarkCircle20Filled,
  DismissCircle20Filled,
  PlugConnected20Regular,
  DocumentAdd20Regular,
  LockClosed20Filled,
  DocumentLock20Regular,
  EyeOff20Regular,
  QuestionCircle20Regular,
} from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { Button } from "../ui/Controls";
import { text } from "../strings";
import type { WorkspaceController } from "./workspaceController";
import {
  dirtyCount,
  incomingChange,
  projectOverlay,
  type SvnOverlay,
} from "./svnOverlay";
import {
  svnClient,
  svnFailure,
  type SvnSession,
  type SvnStatus,
  type SvnStatusEntry,
} from "./svnClient";
import "./SvnToolbar.css";
import { SvnCommitIcon } from "./SvnCommitIcon";

export const overlayNames: Record<SvnOverlay, string> = {
  conflicted: "충돌 또는 경로 장애",
  modified: "수정됨",
  deleted: "삭제 예정 또는 파일 없음",
  added: "추가 예정",
  normal: "로컬 변경 없음",
  needsLock: "잠금 필요",
  locked: "잠금됨",
  ignored: "무시됨",
  unversioned: "버전 관리되지 않음",
  unknown: "로컬 상태 확인 필요",
};
export function OverlayIcon({ overlay }: { overlay: SvnOverlay }) {
  switch (overlay) {
    case "normal":
      return <CheckmarkCircle20Filled className="svn-overlay-normal" />;
    case "modified":
      return (
        <svg aria-hidden="true" width="20" height="20" viewBox="0 0 20 20">
          <circle cx="10" cy="10" r="8" fill="#c22424" />
          <text
            x="10"
            y="14.5"
            textAnchor="middle"
            fill="white"
            fontWeight="bold"
            fontSize="14"
          >
            !
          </text>
        </svg>
      );
    case "conflicted":
      return (
        <svg aria-hidden="true" width="20" height="20" viewBox="0 0 20 20">
          <path d="M10 2 19 18H1Z" fill="#e9b308" />
          <text
            x="10"
            y="15.5"
            textAnchor="middle"
            fill="#111"
            fontWeight="bold"
            fontSize="12"
          >
            !
          </text>
        </svg>
      );
    case "added":
      return <DocumentAdd20Regular className="svn-overlay-added" />;
    case "deleted":
      return <DismissCircle20Filled className="svn-overlay-deleted" />;
    case "locked":
      return <LockClosed20Filled className="svn-overlay-lock" />;
    case "needsLock":
      return <DocumentLock20Regular className="svn-overlay-needs-lock" />;
    case "ignored":
      return <EyeOff20Regular className="svn-overlay-ignored" />;
    default:
      return <QuestionCircle20Regular className="svn-overlay-unknown" />;
  }
}
function sameOrigin(left: string, right: string) {
  try {
    return new URL(left).origin === new URL(right).origin;
  } catch {
    return false;
  }
}
function readableWindowsPath(path: string): string {
  return path.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
}
export function SvnToolbar({
  root,
  collaborative,
  generation,
  controller,
  openConnection,
  onUpdatingChange,
  onConnectionVerified,
  onStatus,
  connectionFailed,
  sessionRevision,
  commitRevision = 0,
  localObservation = null,
  templateObservation,
  resourceObservation,
  openCommit = () => {},
  blocked,
}: {
  root: string;
  collaborative: boolean;
  generation: number;
  controller: WorkspaceController;
  openConnection: () => void;
  onUpdatingChange: (updating: boolean) => void;
  onConnectionVerified: () => void;
  onStatus?: (status: SvnStatus | null) => void;
  connectionFailed: boolean;
  sessionRevision: number;
  commitRevision?: number;
  localObservation?: {
    document: string;
    sequence: number;
    entry?: SvnStatusEntry;
  } | null;
  templateObservation?: unknown;
  resourceObservation?: unknown;
  openCommit?: () => void;
  blocked: boolean;
}) {
  const [status, setStatus] = useState<SvnStatus | null>(null);
  const [statusContext, setStatusContext] = useState("");
  const [session, setSession] = useState<SvnSession | null>(null);
  const [checkedAt, setCheckedAt] = useState<Date | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);
  const [cleanupDetailsOpen, setCleanupDetailsOpen] = useState(false);
  const [busy, setBusy] = useState<"refresh" | "update" | "cleanup" | null>(
    null,
  );
  const [refreshRevision, setRefreshRevision] = useState(0);
  const contextKey = `${root}\u0000${generation}\u0000${sessionRevision}\u0000${refreshRevision}\u0000${commitRevision}\u0000${localObservation?.sequence ?? 0}`;
  const scope = useRef(0);
  const active = useRef(false);
  const statusRef = useRef<SvnStatus | null>(null);
  const localObservationSequence = useRef(0);
  const queuedRefresh = useRef<{ path: string; epoch: number } | null>(null);
  const queuedLocal = useRef<{
    path: string;
    epoch: number;
    document: string;
    sequence: number;
    key: string;
    entry?: SvnStatusEntry;
  } | null>(null);
  const updating = useRef(false);

  async function refreshLocal(
    path: string,
    epoch: number,
    document: string,
    sequence: number,
    key: string,
    confirmedEntry?: SvnStatusEntry,
  ) {
    if (
      scope.current !== epoch ||
      localObservationSequence.current !== sequence
    )
      return;
    if (active.current) {
      queuedLocal.current = {
        path,
        epoch,
        document,
        sequence,
        key,
        entry: confirmedEntry,
      };
      return;
    }
    active.current = true;
    try {
      const entry =
        confirmedEntry ?? (await svnClient.localDocumentStatus(path, document));
      if (
        scope.current !== epoch ||
        localObservationSequence.current !== sequence
      )
        return;
      const previous = statusRef.current;
      if (!previous) return;
      const sameDocument = (value: string) =>
        value
          .replace(/\\/gu, "/")
          .toLowerCase()
          .endsWith(`/documents/${document.toLowerCase()}.json`);
      const entries = previous.entries.filter((row) => !sameDocument(row.path));
      const prior = previous.entries.find((row) => sameDocument(row.path));
      entries.push({
        ...entry,
        remote: prior?.remote ?? null,
        remoteProperties: prior?.remoteProperties ?? null,
      });
      const next = {
        ...previous,
        entries,
        updateBlock:
          previous.updateBlock &&
          previous.updateBlock !== "svn_dirty_working_copy"
            ? previous.updateBlock
            : dirtyCount({ ...previous, entries }) > 0
              ? "svn_dirty_working_copy"
              : null,
      };
      statusRef.current = next;
      setStatus(next);
      onStatus?.(next);
      setStatusContext(key);
      setError(null);
    } catch {
      if (
        scope.current === epoch &&
        localObservationSequence.current === sequence
      ) {
        statusRef.current = null;
        setStatus(null);
        onStatus?.(null);
        setStatusContext("");
        setError(text("svn.localStatusUnknown"));
      }
    } finally {
      active.current = false;
      const queued = queuedRefresh.current;
      queuedRefresh.current = null;
      if (queued && scope.current === queued.epoch)
        void refresh(queued.path, queued.epoch);
      else {
        const local = queuedLocal.current;
        queuedLocal.current = null;
        if (local && scope.current === local.epoch)
          void refreshLocal(
            local.path,
            local.epoch,
            local.document,
            local.sequence,
            local.key,
            local.entry,
          );
      }
    }
  }

  async function refresh(path: string, epoch: number) {
    if (active.current) {
      queuedRefresh.current = { path, epoch };
      return;
    }
    active.current = true;
    setBusy("refresh");
    const request = crypto.randomUUID();
    try {
      const nextSession = await svnClient.session();
      if (scope.current !== epoch) return;
      const nextStatus = await svnClient.status(request, path);
      if (scope.current !== epoch) return;
      setSession(nextSession);
      statusRef.current = nextStatus;
      setStatus(nextStatus);
      onStatus?.(nextStatus);
      setStatusContext(contextKey);
      setCheckedAt(new Date());
      setError(nextStatus.serverError ? text("svn.serverUnknown") : null);
      if (
        !nextStatus.serverError &&
        nextSession.connected &&
        sameOrigin(nextSession.url ?? "", nextStatus.info.url)
      )
        onConnectionVerified();
    } catch (failure) {
      if (scope.current === epoch) {
        statusRef.current = null;
        setStatus(null);
        onStatus?.(null);
        setCheckedAt(null);
        setError(svnFailure(failure));
      }
    } finally {
      active.current = false;
      if (scope.current === epoch) setBusy(null);
      const queued = queuedRefresh.current;
      queuedRefresh.current = null;
      if (queued && scope.current === queued.epoch)
        void refresh(queued.path, queued.epoch);
      else {
        const local = queuedLocal.current;
        queuedLocal.current = null;
        if (local && scope.current === local.epoch)
          void refreshLocal(
            local.path,
            local.epoch,
            local.document,
            local.sequence,
            local.key,
            local.entry,
          );
      }
    }
  }
  useEffect(() => {
    const scopeRef = scope;
    const epoch = ++scopeRef.current;
    if (updating.current) return;
    if (root && collaborative) void refresh(root, epoch);
    else
      queueMicrotask(() => {
        if (scopeRef.current !== epoch) return;
        statusRef.current = null;
        setStatus(null);
        onStatus?.(null);
        setSession(null);
        setError(null);
        setCheckedAt(null);
      });
    return () => {
      scopeRef.current++;
    };
    // Project generation changes only on an actual close/open, never on tab focus.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    root,
    collaborative,
    generation,
    sessionRevision,
    refreshRevision,
    commitRevision,
    templateObservation,
    resourceObservation,
  ]);

  useEffect(() => {
    if (!root || !collaborative || !localObservation) return;
    localObservationSequence.current = localObservation.sequence;
    // The prior Normal row is no longer a proven fact while the local file is
    // being read. Do not trigger a server-wide status request on each save.
    if (!localObservation.entry) {
      queueMicrotask(() => {
        if (localObservationSequence.current !== localObservation.sequence)
          return;
        setStatusContext("");
        onStatus?.(null);
      });
    }
    void refreshLocal(
      root,
      scope.current,
      localObservation.document,
      localObservation.sequence,
      contextKey,
      localObservation.entry,
    );
    // Only a confirmed canonical document save starts a local read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [localObservation?.sequence]);

  async function guiAction(action: "update" | "cleanup") {
    if (!root || active.current || blocked) return;
    active.current = true;
    updating.current = true;
    onUpdatingChange(true);
    ++scope.current;
    setBusy(action);
    const path = root;
    const project = controller.shell.snapshot().projectId;
    const generationBefore = controller.shell.projectGeneration();
    setUpdateError(null);
    let failure: string | null = null;
    try {
      if (action === "update")
        await svnClient.update(crypto.randomUUID(), path);
      else await svnClient.cleanup(crypto.randomUUID(), path);
    } catch (reason) {
      failure = svnFailure(reason);
    } finally {
      // Tortoise may have applied some files even when its GUI was cancelled.
      // Keep the project and UI owner alive, then reread the same project's
      // lists and active document from disk without resetting open tabs.
      if (
        controller.shell.snapshot().projectId === project &&
        controller.shell.snapshot().root === path &&
        controller.shell.projectGeneration() === generationBefore
      ) {
        try {
          await controller.shell.refreshAfterExternalFiles();
          await controller.documents.load();
        } catch {
          failure = [failure, text("svn.errorRefreshAfterUpdate")]
            .filter(Boolean)
            .join(" ");
        }
      }
      updating.current = false;
      onUpdatingChange(false);
      active.current = false;
      setBusy(null);
      if (failure) setUpdateError(failure);
      ++scope.current;
      if (controller.shell.snapshot().projectId === project)
        setRefreshRevision((value) => value + 1);
      // Neither dialog's exit status proves that the working copy recovered.
    }
  }
  if (!root) return null;
  if (!collaborative)
    return (
      <span className="svn-toolbar svn-personal">
        {text("svn.personalProject")}
      </span>
    );

  const visibleStatus = statusContext === contextKey ? status : null;
  const overlay = projectOverlay(visibleStatus);
  const localCause =
    visibleStatus?.recovery === "cleanupRequired"
      ? text("svn.localCleanupRequired")
      : visibleStatus?.recovery === "resumeRequired"
        ? text("svn.localResumeRequired")
        : overlay === "conflicted"
          ? visibleStatus?.entries.some((entry) => entry.local === "conflicted")
            ? "충돌"
            : "경로 장애"
          : overlay === "deleted"
            ? visibleStatus?.entries.some((entry) => entry.local === "deleted")
              ? "삭제 예정"
              : "파일 없음"
            : overlayNames[overlay];
  const incoming =
    visibleStatus && !visibleStatus.serverError
      ? incomingChange(visibleStatus)
      : false;
  const remoteLabel =
    !visibleStatus || visibleStatus.serverError
      ? text("svn.remoteUnknown")
      : incoming
        ? text("svn.remoteIncoming")
        : text("svn.remoteCurrent");
  // The account session belongs to its server, while the current working
  // copy may be local (file://) or temporarily unable to query its server.
  // Do not present either condition as a failed login.
  const connected = !!session?.connected;
  const remoteKind =
    !visibleStatus || visibleStatus.serverError
      ? "unknown"
      : incoming
        ? "incoming"
        : "current";
  const updateBlock = visibleStatus?.updateBlock ?? null;
  const lockOwners =
    visibleStatus?.entries.flatMap((entry) =>
      entry.lockOwner ? [entry.lockOwner] : [],
    ) ?? [];
  const detail = `${localCause} · ${remoteLabel} · ${text("svn.localChanges")}: ${visibleStatus ? dirtyCount(visibleStatus) : "?"} · ${text("svn.serverRevision")}: ${visibleStatus?.serverRevision ?? "?"} · ${statusContext === contextKey ? (checkedAt?.toLocaleString() ?? text("svn.notChecked")) : text("svn.notChecked")}${busy === "refresh" ? ` · ${text("svn.checking")}` : ""}${lockOwners.length ? ` · 잠금 소유: ${[...new Set(lockOwners)].join(", ")}` : ""}`;
  return (
    <div className="svn-toolbar" role="group" aria-label={text("svn.toolbar")}>
      <Tooltip content={detail} relationship="description">
        <span className="svn-status-badge" role="status" aria-label={detail}>
          <OverlayIcon overlay={overlay} /> <span>{localCause}</span>{" "}
          <span className={`svn-remote svn-remote-${remoteKind}`}>
            {remoteLabel}
          </span>
        </span>
      </Tooltip>
      <Tooltip
        content={
          connected
            ? text("svn.connectionVerified")
            : connectionFailed
              ? text("svn.connectionMissing")
              : text("svn.connect")
        }
        relationship="description"
      >
        <Button
          type="button"
          appearance="subtle"
          icon={
            <span className="svn-connection-icon">
              <PlugConnected20Regular />
              {connected ? (
                <CheckmarkCircle20Filled className="svn-connection-ok" />
              ) : connectionFailed ? (
                <DismissCircle20Filled className="svn-connection-fail" />
              ) : null}
            </span>
          }
          aria-label={text("svn.connect")}
          disabled={blocked || !!busy}
          onClick={openConnection}
        />
      </Tooltip>
      <Tooltip content={text("svn.refreshHelp")} relationship="description">
        <Button
          type="button"
          appearance="subtle"
          icon={<ArrowClockwise20Regular />}
          aria-label={text("svn.refresh")}
          disabled={blocked || !!busy}
          onClick={() => void refresh(root, scope.current)}
        />
      </Tooltip>
      <Tooltip
        content={updateBlock ? svnFailure(updateBlock) : text("svn.updateHelp")}
        relationship="description"
      >
        <Button
          type="button"
          appearance="subtle"
          icon={<ArrowDownload20Regular />}
          aria-label={text("svn.update")}
          disabled={blocked || !!busy || !visibleStatus || !!updateBlock}
          onClick={() => void guiAction("update")}
        />
      </Tooltip>
      <Tooltip content={text("svn.commit")} relationship="description">
        <Button
          type="button"
          appearance="subtle"
          icon={<SvnCommitIcon />}
          aria-label={text("svn.commit")}
          disabled={
            blocked || !!busy || !visibleStatus || !!visibleStatus.recovery
          }
          onClick={openCommit}
        />
      </Tooltip>
      <Tooltip content={text("svn.cleanupAction")} relationship="description">
        <Button
          type="button"
          appearance="subtle"
          icon={<Broom20Regular />}
          className={
            visibleStatus?.recovery === "cleanupRequired"
              ? "svn-cleanup-needed"
              : undefined
          }
          aria-label={text("svn.cleanupAction")}
          disabled={
            blocked || !!busy || visibleStatus?.recovery !== "cleanupRequired"
          }
          onClick={() => setCleanupDetailsOpen(true)}
        />
      </Tooltip>
      <Dialog
        open={
          cleanupDetailsOpen && visibleStatus?.recovery === "cleanupRequired"
        }
        onOpenChange={(_, data) => setCleanupDetailsOpen(data.open)}
      >
        <DialogSurface className="svn-cleanup-dialog">
          <DialogBody>
            <DialogTitle>{text("svn.cleanupDialogTitle")}</DialogTitle>
            <DialogContent>
              <p>{text("svn.cleanupRequired")}</p>
              <p className="svn-cleanup-tip">{text("svn.cleanupLocksHelp")}</p>
              <div className="svn-recovery-path">
                <strong>{text("svn.cleanupTarget")}</strong>
                <span>
                  {readableWindowsPath(visibleStatus?.info.wcRoot ?? root)}
                </span>
              </div>
            </DialogContent>
            <DialogActions>
              <Button
                type="button"
                appearance="primary"
                onClick={() => {
                  setCleanupDetailsOpen(false);
                  void guiAction("cleanup");
                }}
              >
                {text("svn.cleanupAction")}
              </Button>
              <Button
                type="button"
                appearance="secondary"
                onClick={() => setCleanupDetailsOpen(false)}
              >
                {text("common.cancel")}
              </Button>
            </DialogActions>
          </DialogBody>
        </DialogSurface>
      </Dialog>
      {(error || updateError) && (
        <FloatingNotice intent="error">{updateError || error}</FloatingNotice>
      )}
    </div>
  );
}
import { FloatingNotice } from "../ui/FloatingNotice";
