import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
  Select,
} from "@fluentui/react-components";
import { useEffect, useRef, useState } from "react";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";
import { svnClient, svnFailure, type PolicySnapshot } from "./svnClient";
import "./CollaborationPolicyDialog.css";

export function CollaborationPolicyDialog({
  root,
  canSave,
  close,
  commit,
  openProject,
  home,
  update,
  currentVersion = "—",
}: {
  root: string;
  currentVersion?: string;
  canSave: boolean;
  close: () => void;
  commit: () => void;
  openProject: () => void;
  home: () => void;
  update: () => void;
}) {
  const [storedSnapshot, setSnapshot] = useState<PolicySnapshot | null>(null);
  const [minimum, setMinimum] = useState("");
  const [busy, setBusy] = useState(true);
  const [storedFailure, setFailure] = useState<string | null>(null);
  const [snapshotRoot, setSnapshotRoot] = useState<string | null>(null);
  const snapshot = snapshotRoot === root ? storedSnapshot : null;
  const failure = snapshotRoot === root ? storedFailure : null;
  const epoch = useRef({ generation: 0 });
  const pending = busy || snapshotRoot !== root;
  const run = async (operation: () => Promise<PolicySnapshot>) => {
    const owner = ++epoch.current.generation;
    setBusy(true);
    setFailure(null);
    try {
      const next = await operation();
      if (owner !== epoch.current.generation) return;
      setSnapshotRoot(root);
      setSnapshot(next);
      setMinimum(
        next.availableVersions.includes(next.local?.minimumAppVersion ?? "")
          ? next.local!.minimumAppVersion
          : (next.availableVersions[0] ?? ""),
      );
    } catch (error) {
      if (owner === epoch.current.generation) {
        setSnapshotRoot(root);
        setSnapshot(null);
        setFailure(svnFailure(error));
      }
    } finally {
      if (owner === epoch.current.generation) setBusy(false);
    }
  };
  useEffect(() => {
    const requestState = epoch.current;
    const owner = ++requestState.generation;
    void svnClient.policySnapshot(root).then(
      (next) => {
        if (owner !== requestState.generation) return;
        setSnapshotRoot(root);
        setSnapshot(next);
        setFailure(null);
        setMinimum(
          next.availableVersions.includes(next.local?.minimumAppVersion ?? "")
            ? next.local!.minimumAppVersion
            : (next.availableVersions[0] ?? ""),
        );
        setBusy(false);
      },
      (error) => {
        if (owner !== requestState.generation) return;
        setSnapshotRoot(root);
        setSnapshot(null);
        setFailure(svnFailure(error));
        setBusy(false);
      },
    );
    return () => {
      ++requestState.generation;
    };
  }, [root]);
  const missing = snapshot?.issue === "svn_policy_missing";
  const tooOld = snapshot?.issue?.startsWith("svn_policy_app_too_old:");
  const changed =
    snapshot?.localModified === true &&
    !!snapshot?.local &&
    snapshot.local.minimumAppVersion !== snapshot.server?.minimumAppVersion;
  return (
    <Dialog
      open
      onOpenChange={(_, data) => {
        if (!data.open && !pending) close();
      }}
    >
      <DialogSurface className="policy-dialog">
        <DialogBody>
          <DialogTitle>{text("policy.title")}</DialogTitle>
          <DialogContent>
            <p>
              {text(
                missing ? "policy.initialDescription" : "policy.description",
              )}
            </p>
            {pending && <p role="status">{text("policy.reading")}</p>}
            {failure && <InlineNotice kind="error">{failure}</InlineNotice>}
            {(snapshot || failure) && (
              <table className="policy-table">
                <thead>
                  <tr>
                    <th>{text("policy.kind")}</th>
                    <th>{text("policy.version")}</th>
                    <th>{text("policy.state")}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>{text("policy.current")}</td>
                    <td>{snapshot?.currentAppVersion ?? currentVersion}</td>
                    <td>{text("policy.installed")}</td>
                  </tr>
                  <tr>
                    <td>{text("policy.server")}</td>
                    <td>
                      {snapshot?.server?.minimumAppVersion ??
                        text(snapshot ? "policy.none" : "policy.unconfirmed")}
                    </td>
                    <td>
                      {snapshot?.server
                        ? text("policy.applied")
                        : text(snapshot ? "policy.none" : "policy.unconfirmed")}
                    </td>
                  </tr>
                  <tr>
                    <td>{text("policy.local")}</td>
                    <td>
                      {snapshot?.local?.minimumAppVersion ??
                        text(snapshot ? "policy.none" : "policy.unconfirmed")}
                    </td>
                    <td>
                      {snapshot
                        ? !snapshot.local
                          ? text("policy.none")
                          : snapshot.localModified === null
                            ? text("policy.unconfirmed")
                            : snapshot.localModified
                              ? text("policy.uncommitted")
                              : snapshot.local.minimumAppVersion !==
                                  snapshot.server?.minimumAppVersion
                                ? text("policy.outdated")
                                : text("policy.same")
                        : text("policy.unconfirmed")}
                    </td>
                  </tr>
                </tbody>
              </table>
            )}
            {tooOld && (
              <InlineNotice kind="warning">
                {text("policy.tooOld")}
              </InlineNotice>
            )}
            {snapshot && !tooOld && (missing || canSave) && (
              <Field label={text("policy.value")}>
                {snapshot.availableVersions.length ? (
                  <Select
                    value={minimum}
                    disabled={pending}
                    onChange={(e) => setMinimum(e.target.value)}
                  >
                    {snapshot.availableVersions.map((v) => (
                      <option key={v} value={v}>
                        {v}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <p>{text("policy.noVersion")}</p>
                )}
              </Field>
            )}
          </DialogContent>
          <DialogActions>
            {(tooOld || failure) && (
              <>
                <Button disabled={pending} onClick={home}>
                  {text("policy.home")}
                </Button>
                <Button disabled={pending} onClick={update}>
                  {text("policy.update")}
                </Button>
              </>
            )}
            {missing && (
              <Button
                appearance="primary"
                disabled={pending || !minimum}
                onClick={() =>
                  void run(() =>
                    svnClient.policyInitialize(
                      root,
                      crypto.randomUUID(),
                      minimum,
                    ),
                  )
                }
              >
                {text("policy.initialize")}
              </Button>
            )}
            {snapshot?.server && !snapshot.issue && canSave && (
              <>
                <Button
                  disabled={
                    busy ||
                    !minimum ||
                    minimum === snapshot.local?.minimumAppVersion
                  }
                  onClick={() =>
                    void run(() =>
                      svnClient.policySave(root, minimum, snapshot.revision),
                    )
                  }
                >
                  {text("policy.save")}
                </Button>
                <Button disabled={pending || !changed} onClick={commit}>
                  {text("policy.commit")}
                </Button>
              </>
            )}
            {snapshot?.server && !snapshot.issue && !canSave && (
              <Button disabled={pending} onClick={openProject}>
                {text("policy.open")}
              </Button>
            )}
            <Button
              disabled={pending}
              onClick={() => void run(() => svnClient.policySnapshot(root))}
            >
              {text("policy.retry")}
            </Button>
            <Button disabled={pending} onClick={close}>
              {text("common.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
