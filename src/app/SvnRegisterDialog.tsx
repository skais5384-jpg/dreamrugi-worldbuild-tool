import {
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
  Input,
  Textarea,
} from "@fluentui/react-components";
import { useEffect, useRef, useState } from "react";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";
import {
  svnClient,
  svnFailure,
  type SvnRegisterPreview,
  type SvnRegisterResult,
} from "./svnClient";
import "./SvnRegisterDialog.css";

export function SvnRegisterDialog({
  root,
  kind,
  sessionRevision,
  close,
  connect,
  openProject,
}: {
  root: string;
  kind: "new" | "copy";
  sessionRevision: number;
  close: () => void;
  connect: () => void;
  openProject: (root: string) => Promise<void>;
}) {
  const [connected, setConnected] = useState(false);
  const [url, setUrl] = useState("");
  const [message, setMessage] = useState("");
  const [preview, setPreview] = useState<SvnRegisterPreview | null>(null);
  const [result, setResult] = useState<SvnRegisterResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [resumeEmptyChild, setResumeEmptyChild] = useState(false);
  const [resumeAvailable, setResumeAvailable] = useState(false);
  const request = useRef<string | null>(null);
  const name =
    root
      .replace(/[\\/]+$/u, "")
      .split(/[\\/]/u)
      .pop() ?? "";

  useEffect(() => {
    let active = true;
    void svnClient.session().then(
      (session) => {
        if (!active) return;
        setConnected(session.connected && !!session.url);
        if (session.connected && session.url) {
          const suggested = `${session.url.replace(/\/+$/u, "")}/${encodeURIComponent(name)}`;
          setUrl((current) => current || suggested);
          setMessage(
            (current) =>
              current || text("svn.registerDefaultMessage", { name }),
          );
        }
      },
      () => {
        if (active) setConnected(false);
      },
    );
    return () => {
      active = false;
    };
  }, [sessionRevision, name]);

  async function loadPreview() {
    if (busy || !url.trim()) return;
    const id = crypto.randomUUID();
    request.current = id;
    setBusy(true);
    setError(null);
    setPreview(null);
    try {
      const next = await svnClient.registerPreview(id, root, url.trim());
      if (request.current === id) setPreview(next);
    } catch (failure) {
      if (request.current === id) setError(svnFailure(failure));
    } finally {
      if (request.current === id) {
        request.current = null;
        setBusy(false);
      }
    }
  }

  async function register() {
    if (busy || !preview || !message.trim()) return;
    const id = crypto.randomUUID();
    request.current = id;
    setBusy(true);
    setError(null);
    try {
      const registered = await svnClient.register(
        id,
        root,
        preview.url,
        message,
        preview.fingerprint,
        resumeEmptyChild,
      );
      if (request.current === id) setResult(registered);
    } catch (failure) {
      if (request.current === id) {
        setPreview(null);
        setResumeAvailable(
          typeof failure === "string" &&
            failure === "svn_register_checkout_unverified",
        );
        setResumeEmptyChild(false);
        setError(svnFailure(failure));
      }
    } finally {
      if (request.current === id) {
        request.current = null;
        setBusy(false);
      }
    }
  }

  return (
    <Dialog
      open
      modalType="alert"
      onOpenChange={(_, data) => !data.open && !busy && close()}
    >
      <DialogSurface
        className={`svn-register-dialog${preview ? " has-preview" : ""}`}
      >
        <DialogBody>
          <DialogTitle>{text("svn.registerTitle")}</DialogTitle>
          <DialogContent>
            <p>
              {text(
                kind === "new" ? "svn.registerNewHelp" : "svn.registerCopyHelp",
              )}
            </p>
            <p className="svn-register-root">{root}</p>
            {!connected && (
              <p role="status">{text("svn.registerConnectFirst")}</p>
            )}
            {!result && connected && (
              <>
                <Field label={text("svn.registerUrl")} required>
                  <Input
                    value={url}
                    disabled={busy}
                    onChange={(_, data) => {
                      setUrl(data.value);
                      setPreview(null);
                      setResumeEmptyChild(false);
                      setResumeAvailable(false);
                    }}
                  />
                </Field>
                {resumeAvailable && (
                  <Checkbox
                    label={text("svn.registerResumeEmpty")}
                    checked={resumeEmptyChild}
                    disabled={busy}
                    onChange={(_, data) =>
                      setResumeEmptyChild(data.checked === true)
                    }
                  />
                )}
                <Field label={text("svn.commitMessage")} required>
                  <Textarea
                    value={message}
                    disabled={busy}
                    maxLength={4096}
                    onChange={(_, data) => setMessage(data.value)}
                  />
                </Field>
                {preview && (
                  <section className="svn-register-preview">
                    <p>
                      {text("svn.registerSummary", {
                        files: String(preview.files.length),
                        directories: String(preview.directories.length),
                        excluded: String(preview.excludedFiles),
                      })}
                    </p>
                    <div className="svn-register-table-wrap">
                      <table>
                        <thead>
                          <tr>
                            <th scope="col">{text("svn.registerKind")}</th>
                            <th scope="col">{text("svn.registerPath")}</th>
                            <th scope="col">{text("svn.registerState")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {preview.directories.map((path) => (
                            <tr key={`dir:${path}`}>
                              <td>{text("svn.registerDirectory")}</td>
                              <td>{path}</td>
                              <td>{text("svn.registerPending")}</td>
                            </tr>
                          ))}
                          {preview.files.map((path) => (
                            <tr key={`file:${path}`}>
                              <td>{text("svn.registerFile")}</td>
                              <td>{path}</td>
                              <td>{text("svn.registerPending")}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </section>
                )}
              </>
            )}
            {result && (
              <div role="status">
                <p>{text("svn.registerDone")}</p>
                <p>
                  {text("svn.registerRevisions", {
                    setup: result.setupRevision,
                    content: result.revision,
                  })}
                </p>
              </div>
            )}
            {error && <InlineNotice kind="error">{error}</InlineNotice>}
          </DialogContent>
          <DialogActions>
            {!connected && !result && (
              <Button type="button" onClick={connect} disabled={busy}>
                {text("svn.connect")}
              </Button>
            )}
            {busy && (
              <Button
                type="button"
                onClick={() =>
                  request.current && void svnClient.cancel(request.current)
                }
              >
                {text("svn.cancel")}
              </Button>
            )}

            {!result && connected && (
              <Button
                type="button"
                disabled={busy || !url.trim()}
                onClick={() => void loadPreview()}
              >
                {text("svn.registerPreview")}
              </Button>
            )}
            {!result && connected && (
              <Button
                type="button"
                appearance="primary"
                disabled={busy || !preview || !message.trim()}
                onClick={() => void register()}
              >
                {text("svn.registerAction")}
              </Button>
            )}
            {result && (
              <Button
                type="button"
                appearance="primary"
                onClick={() => void openProject(result.root)}
              >
                {text("svn.registerOpen")}
              </Button>
            )}
            <Button type="button" disabled={busy} onClick={close}>
              {text("svn.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
