import {
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
  Textarea,
  Tooltip,
} from "@fluentui/react-components";
import { Info16Regular, LockClosed16Filled } from "@fluentui/react-icons";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";
import {
  svnClient,
  svnFailure,
  type SvnCommitCandidate,
  type SvnCommitResult,
} from "./svnClient";
import type { DocumentController } from "./documentController";
import { editDirty } from "./documentEdits";
import { OverlayIcon, overlayNames } from "./SvnToolbar";
import { overlayForEntry } from "./svnOverlay";
import "./SvnCommitDialog.css";

type CommitMemory = {
  result: SvnCommitResult | null;
  unknown: boolean;
  delivered: string[];
};
const commitMemory = new Map<string, CommitMemory>();

export function SvnCommitDialog({
  root,
  document,
  controller,
  close,
  committed,
}: {
  root: string;
  document: string | null;
  controller: DocumentController;
  close: () => void;
  committed: (paths: string[]) => Promise<string[]>;
}) {
  const memoryKey = `${root}\u0000${controller.shell?.projectGeneration?.() ?? 0}`;
  const [candidates, setCandidates] = useState<SvnCommitCandidate[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [resultUnknown, setResultUnknown] = useState(
    () => commitMemory.get(memoryKey)?.unknown ?? false,
  );
  const [result, setResult] = useState<SvnCommitResult | null>(
    () => commitMemory.get(memoryKey)?.result ?? null,
  );
  const [revision, setRevision] = useState(0);
  const activeRequest = useRef<string | null>(null);
  const documentState = useSyncExternalStore(
    controller.subscribe,
    controller.snapshot,
  );

  useEffect(() => {
    let active = true;
    void svnClient.commitCandidates(root).then(
      (rows) => {
        if (!active) return;
        setCandidates(rows);
        const initial = rows.find(
          (row) =>
            row.eligible &&
            !!document &&
            row.path === `documents/${document}.json`,
        );
        setSelected(initial ? [initial.path, ...initial.required] : []);
        setLoading(false);
      },
      (failure) => {
        if (!active) return;
        setError(svnFailure(failure));
        setLoading(false);
      },
    );
    return () => {
      active = false;
    };
  }, [root, document, revision]);

  const unsaved = selected.some((path) => {
    const id = path.match(/^documents\/([^/]+)\.json$/u)?.[1];
    const entry = id && documentState.editors[id];
    return !!entry && (editDirty(entry) || entry.busy || entry.paused);
  });
  const missingRequired = selected.some((path) => {
    const candidate = candidates.find((row) => row.path === path);
    return candidate?.required.some((required) => !selected.includes(required));
  });
  const savedName = (path: string) => {
    const candidate = candidates.find((row) => row.path === path);
    if (candidate?.name) return candidate.name;
    const id = path.match(/^documents\/([^/]+)\.json$/u)?.[1];
    if (id)
      return (
        documentState.list?.documents.find((item) => item.id === id)?.name ??
        text("svn.documentNameUnknown")
      );
    if (path === "workspace/document-layout.json") return "문서 배치";
    if (path.startsWith("assets/"))
      return path.endsWith("/metadata.json") ? "이미지 정보" : "이미지 원본";
    return "공유 항목";
  };
  const duplicateNames = new Set(
    candidates
      .map((candidate) => savedName(candidate.path))
      .filter((name, index, names) => names.indexOf(name) !== index),
  );
  const itemContext = (candidate: SvnCommitCandidate) => {
    const path = candidate.path;
    const kind = path.startsWith("documents/")
      ? "문서"
      : path.startsWith("templates/")
        ? "Template"
        : path.startsWith("assets/")
          ? path.endsWith("/metadata.json")
            ? "이미지 정보"
            : "이미지 원본"
          : "프로젝트 구성";
    const identifier = duplicateNames.has(savedName(path))
      ? ` · ${
          path.startsWith("assets/")
            ? path.split("/")[1]?.slice(0, 8)
            : path
                .split("/")
                .slice(-1)[0]
                ?.replace(/\.json$/u, "")
                .slice(0, 8)
        }`
      : "";
    return `${kind}${identifier}${["deleted", "missing"].includes(candidate.local) ? " · 삭제 예정" : ""}`;
  };
  const localLabel = (value: string) =>
    ({
      modified: text("svn.statusModified"),
      added: text("svn.statusAdded"),
      unversioned: text("svn.statusUnversioned"),
      deleted: text("svn.statusDeleted"),
      missing: text("svn.statusMissing"),
      conflicted: text("svn.statusConflicted"),
      normal: text("svn.statusNormal"),
    })[value] ?? text("svn.statusUnknown");
  const propertyLabel = (value: string | null) =>
    value === "normal" || value === "none"
      ? text("svn.propertiesUnchanged")
      : value === "modified"
        ? text("svn.propertiesModified")
        : value === "conflicted"
          ? text("svn.statusConflicted")
          : text("svn.statusUnknown");
  async function submit() {
    if (
      busy ||
      !selected.length ||
      !message.trim() ||
      resultUnknown ||
      unsaved ||
      missingRequired
    )
      return;
    const request = crypto.randomUUID();
    activeRequest.current = request;
    setBusy(true);
    setError(null);
    try {
      const outcome = await svnClient.commit(request, root, selected, message);
      if (activeRequest.current !== request) return;
      const unknownPaths = new Set(
        outcome.verificationUnknown.map((item) => item.path),
      );
      const verifiedPaths = outcome.paths.filter(
        (path) => !unknownPaths.has(path),
      );
      let failedRelease: string[];
      try {
        failedRelease = verifiedPaths.length
          ? await committed(verifiedPaths)
          : [];
      } catch {
        failedRelease = verifiedPaths;
      }
      const recorded = {
        ...outcome,
        unlockPending: [
          ...new Set([...outcome.unlockPending, ...failedRelease]),
        ],
      };
      commitMemory.set(memoryKey, {
        result: recorded,
        unknown: false,
        delivered: verifiedPaths,
      });
      setResult(recorded);
    } catch (failure) {
      if (activeRequest.current === request) {
        setError(svnFailure(failure));
        if (
          failure === "svn_commit_unverified" ||
          failure === "svn_commit_applied_unverified"
        ) {
          setResultUnknown(true);
          commitMemory.set(memoryKey, {
            result: null,
            unknown: true,
            delivered: [],
          });
        }
      }
    } finally {
      if (activeRequest.current === request) {
        activeRequest.current = null;
        setBusy(false);
      }
    }
  }

  async function recheck() {
    if (!result || busy) return;
    setBusy(true);
    setError(null);
    try {
      const refreshed = await svnClient.commitRecheck(root, result);
      const prior = commitMemory.get(memoryKey)?.delivered ?? [];
      const unknown = new Set(
        refreshed.verificationUnknown.map((item) => item.path),
      );
      const newlyVerified = refreshed.paths.filter(
        (path) => !unknown.has(path) && !prior.includes(path),
      );
      let failedRelease: string[];
      try {
        failedRelease = newlyVerified.length
          ? await committed(newlyVerified)
          : [];
      } catch {
        failedRelease = newlyVerified;
      }
      const recorded = {
        ...refreshed,
        unlockPending: [
          ...new Set([...refreshed.unlockPending, ...failedRelease]),
        ],
      };
      commitMemory.set(memoryKey, {
        result: recorded,
        unknown: false,
        delivered: [...prior, ...newlyVerified],
      });
      setResult(recorded);
    } catch (failure) {
      setError(svnFailure(failure));
    } finally {
      setBusy(false);
    }
  }

  async function scheduleDelete(target: string) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await svnClient.scheduleDelete(root, target);
      setLoading(true);
      setRevision((value) => value + 1);
    } catch (failure) {
      setError(svnFailure(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open
      modalType="alert"
      onOpenChange={(_, data) => {
        if (!data.open && !busy) close();
      }}
    >
      <DialogSurface className="svn-commit-dialog">
        <DialogBody>
          <DialogTitle>{text("svn.commitTitle")}</DialogTitle>
          <DialogContent>
            {result ? (
              <div role="status" className="svn-commit-result">
                <p>{text("svn.commitDone")}</p>
                <p>
                  {text("svn.commitRevision")} {result.revision}
                </p>
                {result.verificationUnknown.some(
                  (item) => item.check === "remoteLock",
                ) && (
                  <InlineNotice kind="warning">
                    {text("svn.commitLockUnknown")}
                  </InlineNotice>
                )}
                {result.verificationUnknown.some(
                  (item) => item.check !== "remoteLock",
                ) && (
                  <InlineNotice kind="warning">
                    {text("svn.commitStatusUnknown")}
                  </InlineNotice>
                )}
                {!!result.unlockPending.length && (
                  <InlineNotice kind="warning">
                    {text("svn.commitUnlockPending")}
                  </InlineNotice>
                )}
                {(!!result.verificationUnknown.length ||
                  !!result.unlockPending.length) && (
                  <details>
                    <summary>{text("error.details")}</summary>
                    <ul>
                      {result.verificationUnknown.map((item) => (
                        <li key={`${item.path}:${item.check}`}>
                          {item.path} · {item.check}: {item.reason}
                        </li>
                      ))}
                      {result.unlockPending.map((path) => (
                        <li key={path}>
                          {path} · {text("svn.lockHeld")}
                        </li>
                      ))}
                    </ul>
                  </details>
                )}
              </div>
            ) : (
              <>
                {resultUnknown && (
                  <p role="alert">{text("svn.commitResultUnknownHelp")}</p>
                )}
                <Field label={text("svn.commitMessage")} required>
                  <Textarea
                    value={message}
                    onChange={(_, data) => setMessage(data.value)}
                    disabled={busy}
                    maxLength={4096}
                    resize="vertical"
                  />
                </Field>
                <div className="svn-commit-selection">
                  <strong>{text("svn.commitSelection")}</strong>
                  {loading && <p role="status">{text("svn.checking")}</p>}
                  {!loading && !candidates.length && (
                    <p>{text("svn.commitNoChanges")}</p>
                  )}
                  {!!candidates.length && (
                    <table className="svn-commit-table">
                      <thead>
                        <tr>
                          <th scope="col">{text("svn.commitSelectColumn")}</th>
                          <th scope="col">{text("svn.commitItemColumn")}</th>
                          <th scope="col">{text("svn.commitContentColumn")}</th>
                          <th scope="col">
                            {text("svn.commitPropertyColumn")}
                          </th>
                          <th scope="col">{text("svn.commitLockColumn")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {candidates.map((candidate) => (
                          <tr key={candidate.path}>
                            <td>
                              <Checkbox
                                checked={selected.includes(candidate.path)}
                                disabled={busy || !candidate.eligible}
                                aria-label={`${savedName(candidate.path)} · ${candidate.path}`}
                                onChange={(_, data) =>
                                  setSelected((current) =>
                                    data.checked
                                      ? [
                                          ...new Set([
                                            ...current,
                                            candidate.path,
                                            ...candidate.required,
                                          ]),
                                        ]
                                      : current.filter(
                                          (path) => path !== candidate.path,
                                        ),
                                  )
                                }
                              />
                            </td>
                            <td className="svn-commit-path">
                              <div className="svn-commit-item-name">
                                <strong>{savedName(candidate.path)}</strong>
                                <Tooltip
                                  content={candidate.path}
                                  relationship="description"
                                >
                                  <span
                                    tabIndex={0}
                                    role="img"
                                    aria-label={`${savedName(candidate.path)} · ${candidate.path}`}
                                  >
                                    <Info16Regular aria-hidden="true" />
                                  </span>
                                </Tooltip>
                              </div>
                              <small>{itemContext(candidate)}</small>
                              {!candidate.eligible && (
                                <small>
                                  {candidate.blockedReason
                                    ? svnFailure(candidate.blockedReason)
                                    : text("svn.commitBlocked")}
                                </small>
                              )}
                              {!!candidate.required.length && (
                                <small>
                                  {text("svn.commitRequired", {
                                    paths: candidate.required.join(", "),
                                  })}
                                </small>
                              )}
                              {candidate.canScheduleDelete && (
                                <Button
                                  type="button"
                                  disabled={busy}
                                  onClick={() =>
                                    void scheduleDelete(candidate.path)
                                  }
                                >
                                  {text("svn.scheduleDelete")}
                                </Button>
                              )}
                            </td>
                            <td>
                              {(() => {
                                const overlay = overlayForEntry({
                                  path: candidate.path,
                                  local: candidate.local,
                                  properties: candidate.properties,
                                  remote: null,
                                  remoteProperties: null,
                                  lockOwner: null,
                                  wcLocked: candidate.held,
                                  workingCopyLocked: false,
                                  needsLock: !candidate.newPath,
                                  remoteOnly: false,
                                });
                                return (
                                  <Tooltip
                                    content={localLabel(candidate.local)}
                                    relationship="description"
                                  >
                                    <span
                                      className="svn-commit-state"
                                      role="img"
                                      aria-label={`${overlayNames[overlay]} · ${localLabel(candidate.local)}`}
                                    >
                                      <OverlayIcon overlay={overlay} />
                                    </span>
                                  </Tooltip>
                                );
                              })()}
                            </td>
                            <td>{propertyLabel(candidate.properties)}</td>
                            <td>
                              {candidate.held && (
                                <Tooltip
                                  content={text("svn.lockHeld")}
                                  relationship="description"
                                >
                                  <span
                                    className="svn-commit-state"
                                    role="img"
                                    aria-label={text("svn.lockHeld")}
                                  >
                                    <LockClosed16Filled />
                                  </span>
                                </Tooltip>
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </div>
                {unsaved && <p role="status">{text("svn.commitSaveFirst")}</p>}
                {missingRequired && (
                  <p role="status">{text("svn.commitSelectRequired")}</p>
                )}
              </>
            )}
            {error && <p role="alert">{error}</p>}
          </DialogContent>
          <DialogActions>
            {result &&
              (!!result.verificationUnknown.length ||
                !!result.unlockPending.length) && (
                <Button
                  type="button"
                  disabled={busy}
                  onClick={() => void recheck()}
                >
                  {text("svn.commitRecheck")}
                </Button>
              )}
            {result && !result.verificationUnknown.length && (
              <Button
                type="button"
                disabled={busy}
                onClick={() => {
                  commitMemory.delete(memoryKey);
                  setResult(null);
                  setResultUnknown(false);
                  setLoading(true);
                  setRevision((value) => value + 1);
                }}
              >
                {text("svn.commitNext")}
              </Button>
            )}
            {!result && !busy && (
              <Button
                type="button"
                onClick={() => {
                  setLoading(true);
                  setError(null);
                  setRevision((value) => value + 1);
                }}
              >
                {text("svn.commitRefresh")}
              </Button>
            )}
            {busy && (
              <Button
                type="button"
                onClick={() => {
                  if (activeRequest.current)
                    void svnClient.cancel(activeRequest.current);
                }}
              >
                {text("svn.cancel")}
              </Button>
            )}
            {!result && (
              <Button
                type="button"
                appearance="primary"
                disabled={
                  loading ||
                  busy ||
                  !selected.length ||
                  !message.trim() ||
                  resultUnknown ||
                  unsaved ||
                  missingRequired
                }
                onClick={() => void submit()}
              >
                {text("svn.commitAction")}
              </Button>
            )}
            <Button type="button" disabled={busy} onClick={close}>
              {text(result ? "svn.close" : "svn.commitCancel")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
