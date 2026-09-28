import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Field,
} from "@fluentui/react-components";
import { CheckmarkCircle16Filled } from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { Button, Input } from "../ui/Controls";
import { text } from "../strings";
import type { TemplateController } from "./controller";
import { svnClient, svnFailure, type SvnProbe } from "./svnClient";
import "./SvnDialog.css";

function readableWindowsPath(path: string): string {
  return path.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
}

export function SvnDialog({
  open,
  entry,
  onClose,
  onSessionChanged,
  controller,
}: {
  open: boolean;
  entry: "connect" | "checkout";
  onClose: () => void;
  onSessionChanged: (outcome: "success" | "failure" | "logout") => void;
  controller: TemplateController;
}) {
  const [cliPath, setCliPath] = useState("");
  const [guiPath, setGuiPath] = useState("");
  const [probe, setProbe] = useState<SvnProbe | null>(null);
  const [editingTool, setEditingTool] = useState<"cli" | "gui" | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [checkoutUrl, setCheckoutUrl] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(false);
  const [connected, setConnected] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);
  const [sessionMessage, setSessionMessage] = useState<string | null>(null);
  const [destination, setDestination] = useState("");
  const [checkoutError, setCheckoutError] = useState<string | null>(null);
  const [pending, setPending] = useState<{
    action: string;
    request: string;
  } | null>(null);
  const pendingRef = useRef<string | null>(null);
  const scope = useRef(0);

  useEffect(() => {
    const scopeRef = scope;
    const epoch = ++scopeRef.current;
    if (!open) return;
    queueMicrotask(() => {
      if (scope.current !== epoch) return;
      setPassword("");
      setSessionMessage(null);
      setLoginError(null);
      setCheckoutError(null);
    });
    void svnClient.probe().then(
      (result) => {
        if (scope.current !== epoch) return;
        setProbe(result);
        if (result.path) setCliPath(readableWindowsPath(result.path));
        if (result.guiPath) setGuiPath(readableWindowsPath(result.guiPath));
        setProbeError(result.installed ? null : text("svn.probeMissing"));
      },
      (error: unknown) => {
        if (scope.current === epoch) setProbeError(svnFailure(error));
      },
    );
    void svnClient.session().then(
      (session) => {
        if (scope.current !== epoch) return;
        setConnected(session.connected);
        setUrl(session.url ?? "");
        setCheckoutUrl(session.url ?? "");
        setUsername(session.username ?? "");
        setRemember(session.remembered);
      },
      (error: unknown) => {
        if (scope.current === epoch) setLoginError(svnFailure(error));
      },
    );
    return () => {
      scopeRef.current++;
    };
  }, [open, entry]);

  async function run<T>(
    action: string,
    work: (request: string) => Promise<T>,
  ): Promise<T | null> {
    if (pendingRef.current) return null;
    const epoch = scope.current;
    const request = crypto.randomUUID();
    pendingRef.current = request;
    setPending({ action, request });
    try {
      const result = await work(request);
      return scope.current === epoch ? result : null;
    } finally {
      if (pendingRef.current === request) {
        pendingRef.current = null;
        if (scope.current === epoch) setPending(null);
      }
    }
  }
  async function checkInstall() {
    if (pendingRef.current) return;
    const epoch = scope.current;
    try {
      const result = await svnClient.probe(
        cliPath.trim() || undefined,
        guiPath.trim() || undefined,
      );
      if (scope.current !== epoch) return;
      setProbe(result);
      if (result.path) setCliPath(readableWindowsPath(result.path));
      if (result.guiPath) setGuiPath(readableWindowsPath(result.guiPath));
      if (result.installed && result.guiInstalled) setEditingTool(null);
      setProbeError(result.installed ? null : text("svn.probeMissing"));
    } catch (error) {
      if (scope.current === epoch) setProbeError(svnFailure(error));
    }
  }
  async function chooseExecutable(tool: "cli" | "gui") {
    if (pendingRef.current) return;
    const epoch = scope.current;
    const { open: choose } = await import("@tauri-apps/plugin-dialog");
    const selected = await choose({
      directory: false,
      multiple: false,
      filters: [{ name: "Windows 실행 파일", extensions: ["exe"] }],
    });
    if (scope.current !== epoch || typeof selected !== "string") return;
    if (tool === "cli") setCliPath(selected);
    else setGuiPath(selected);
  }
  async function login() {
    const epoch = scope.current;
    try {
      const result = await run("login", (request) =>
        svnClient.login(
          request,
          url.trim(),
          username.trim(),
          password,
          remember,
        ),
      );
      if (!result) return;
      setConnected(true);
      setUrl(result.url);
      setCheckoutUrl(result.url);
      setUsername(username.trim());
      setPassword("");
      setLoginError(null);
      setSessionMessage(null);
      onSessionChanged("success");
    } catch (error) {
      if (scope.current === epoch) {
        setLoginError(svnFailure(error));
        onSessionChanged("failure");
      }
    }
  }
  async function logout() {
    if (pendingRef.current) return;
    const epoch = scope.current;
    try {
      await svnClient.logout();
      if (scope.current !== epoch) return;
      setConnected(false);
      setPassword("");
      setSessionMessage(text("svn.logoutDone"));
      setLoginError(null);
      onSessionChanged("logout");
    } catch (error) {
      if (scope.current === epoch) setLoginError(svnFailure(error));
    }
  }
  async function chooseDestination() {
    if (pendingRef.current) return;
    const epoch = scope.current;
    const { open: choose } = await import("@tauri-apps/plugin-dialog");
    const selected = await choose({ directory: true, multiple: false });
    if (scope.current === epoch && typeof selected === "string")
      setDestination(selected);
  }
  async function checkout() {
    const epoch = scope.current;
    try {
      const result = await run("checkout", (request) =>
        svnClient.checkout(request, checkoutUrl.trim(), destination.trim()),
      );
      if (!result) return;
      const opened = await controller.openDownloadedProject(result.root);
      if (scope.current !== epoch) return;
      if (opened) onClose();
      else setCheckoutError(text("svn.errorOpenAfterCheckout"));
    } catch (error) {
      if (scope.current === epoch) setCheckoutError(svnFailure(error));
    }
  }
  async function cancel() {
    if (pendingRef.current) await svnClient.cancel(pendingRef.current);
  }
  const installReady = !!probe?.installed && !!probe.guiInstalled;
  const editingInstall = !installReady || editingTool !== null;
  const toolField = (tool: "cli" | "gui") => (
    <Field label={text(tool === "cli" ? "svn.cliPath" : "svn.guiPath")}>
      <div className="svn-path-picker">
        <Input
          aria-label={text(tool === "cli" ? "svn.cliPath" : "svn.guiPath")}
          value={tool === "cli" ? cliPath : guiPath}
          onChange={(event) =>
            tool === "cli"
              ? setCliPath(event.target.value)
              : setGuiPath(event.target.value)
          }
          disabled={!!pending}
        />
        <Button
          type="button"
          disabled={!!pending}
          onClick={() => void chooseExecutable(tool)}
        >
          {text("svn.chooseExecutable")}
        </Button>
      </div>
    </Field>
  );
  return (
    <Dialog
      open={open}
      onOpenChange={(_, data) => {
        if (!data.open && !pending) onClose();
      }}
    >
      <DialogSurface className="svn-dialog">
        <DialogBody>
          <DialogTitle>{text("svn.title")}</DialogTitle>
          <DialogContent className="svn-content">
            <section>
              <div className="svn-section-heading">
                <h3>{text("svn.probe")}</h3>
                {installReady && (
                  <span className="svn-verified" role="status">
                    <CheckmarkCircle16Filled aria-hidden="true" />
                    {text("svn.installVerified")}
                  </span>
                )}
              </div>
              {installReady && (
                <div className="svn-summary-list">
                  {(["cli", "gui"] as const).map((tool) => (
                    <div className="svn-summary-row" key={tool}>
                      <div className="svn-summary-value">
                        <span className="svn-summary-label">
                          {text(tool === "cli" ? "svn.cliPath" : "svn.guiPath")}
                        </span>
                        <span className="svn-path-text">
                          {readableWindowsPath(
                            (tool === "cli" ? probe.path : probe.guiPath) ?? "",
                          )}
                        </span>
                      </div>
                      <Button
                        type="button"
                        appearance="secondary"
                        disabled={!!pending}
                        onClick={() => setEditingTool(tool)}
                      >
                        {text("svn.changePath")}
                      </Button>
                    </div>
                  ))}
                </div>
              )}
              {editingInstall && (
                <>
                  {(!installReady || editingTool === "cli") && toolField("cli")}
                  {(!installReady || editingTool === "gui") && toolField("gui")}
                  <div className="actions">
                    <Button
                      type="button"
                      disabled={!!pending}
                      onClick={() => void checkInstall()}
                    >
                      {text("svn.probe")}
                    </Button>
                  </div>
                </>
              )}
              {probe && !probe.guiInstalled && (
                <p role="status">{text("svn.errorMissingGui")}</p>
              )}
              {probeError && (
                <p role="alert" className="error">
                  {probeError}
                </p>
              )}
            </section>
            <section>
              <div className="svn-section-heading">
                <h3>{text("svn.connect")}</h3>
                {connected && (
                  <span className="svn-verified" role="status">
                    <CheckmarkCircle16Filled aria-hidden="true" />
                    {text("svn.connectionReady")}
                  </span>
                )}
              </div>
              {connected && (
                <div className="svn-summary-row">
                  <div className="svn-summary-value">
                    <span className="svn-summary-label">{text("svn.url")}</span>
                    <span className="svn-path-text">{url}</span>
                    <span className="svn-summary-label">
                      {text("svn.username")}
                    </span>
                    <span>{username}</span>
                  </div>
                  <Button
                    type="button"
                    appearance="secondary"
                    disabled={!!pending}
                    onClick={() => void logout()}
                  >
                    {text("svn.logout")}
                  </Button>
                </div>
              )}
              {!connected && (
                <>
                  <Field label={text("svn.url")}>
                    <Input
                      value={url}
                      onChange={(e) => setUrl(e.target.value)}
                      disabled={!!pending}
                      autoComplete="url"
                    />
                  </Field>
                  <Field label={text("svn.username")}>
                    <Input
                      value={username}
                      onChange={(e) => setUsername(e.target.value)}
                      disabled={!!pending}
                      autoComplete="username"
                    />
                  </Field>
                  <Field label={text("svn.password")}>
                    <Input
                      type="password"
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      disabled={!!pending}
                      autoComplete="current-password"
                    />
                  </Field>
                  <label>
                    <input
                      type="checkbox"
                      checked={remember}
                      disabled={!!pending}
                      onChange={(e) => setRemember(e.target.checked)}
                    />{" "}
                    {text("svn.remember")}
                  </label>
                  <div className="actions">
                    <Button
                      type="button"
                      disabled={
                        !installReady ||
                        editingTool !== null ||
                        !!pending ||
                        !password
                      }
                      onClick={() => void login()}
                    >
                      {text("svn.login")}
                    </Button>
                  </div>
                </>
              )}
              {sessionMessage && <p role="status">{sessionMessage}</p>}
              {loginError && (
                <p role="alert" className="error">
                  {loginError}
                </p>
              )}
            </section>
            {entry === "checkout" && (
              <section>
                <h3>{text("svn.checkoutTitle")}</h3>
                <Field label={text("svn.checkoutUrl")}>
                  <Input
                    value={checkoutUrl}
                    onChange={(event) => setCheckoutUrl(event.target.value)}
                    disabled={!!pending}
                    autoComplete="url"
                  />
                </Field>
                <Field label={text("svn.destination")}>
                  <Input
                    value={destination}
                    onChange={(e) => setDestination(e.target.value)}
                    disabled={!!pending}
                  />
                </Field>
                <div className="actions">
                  <Button
                    type="button"
                    disabled={!!pending}
                    onClick={() => void chooseDestination()}
                  >
                    {text("svn.chooseDestination")}
                  </Button>
                  <Button
                    type="button"
                    appearance="primary"
                    disabled={
                      !!pending ||
                      !connected ||
                      !checkoutUrl.trim() ||
                      !destination
                    }
                    onClick={() => void checkout()}
                  >
                    {text("svn.checkoutAction")}
                  </Button>
                </div>
                {checkoutError && (
                  <p role="alert" className="error">
                    {checkoutError}
                  </p>
                )}
              </section>
            )}
            {pending && (
              <div className="actions">
                <p role="status">{pending.action}</p>
                <Button type="button" onClick={() => void cancel()}>
                  {text("svn.cancel")}
                </Button>
              </div>
            )}
          </DialogContent>
          <DialogActions>
            <Button type="button" disabled={!!pending} onClick={onClose}>
              {text("svn.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
