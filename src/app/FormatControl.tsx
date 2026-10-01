import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";
import { useEffect, useState } from "react";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { History20Regular } from "@fluentui/react-icons";
import type { TemplateController } from "./controller";
import type { DocumentRequest, DocumentResponse } from "../bridge/documents";
import { BridgeFailure } from "../bridge/client";
import { safeFailure } from "./operations";
import { Button, Select } from "../ui/Controls";
import { IconCommand } from "../ui/IconCommand";
import { text } from "../strings";
import { formatLocalDateTime } from "./localDateTime";
type Inspection = Extract<DocumentResponse, { kind: "format" }>;
/** 쓰기 버튼을 누르기 전에 현재 버전·복원 자료를 조회하고 사용자가 동작을 고른다. */
export function FormatControl({
  shell,
  kind,
  artifact,
  locked,
  changed,
}: {
  shell: TemplateController;
  kind: "template" | "document";
  artifact: string;
  locked: boolean;
  changed: () => Promise<void>;
}) {
  const [info, setInfo] = useState<Inspection | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false),
    [message, setMessage] = useState("");
  const [restore, setRestore] = useState("");
  useEffect(() => {
    setInfo(null);
    setRestore("");
    setMessage("");
  }, [artifact, kind]);
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => void shell.operations.queryAll(), 250);
    return () => clearInterval(timer);
  }, [busy, shell]);
  async function request(request: DocumentRequest) {
    const project = shell.snapshot().projectId;
    if (!project) throw new BridgeFailure("protocol");
    const { result } = await shell.operations.run(
      { kind: "document_workspace", project, request },
      text("format.title"),
    );
    if (result.kind === "rejected")
      throw new BridgeFailure("boundary", undefined, result.error);
    return result.kind === "document_workspace" ? result.value : result;
  }
  async function inspect() {
    setBusy(true);
    setMessage("");
    try {
      const r = await request({ action: "format_inspect", kind, artifact });
      if (r.kind !== "format") throw new BridgeFailure("protocol");
      setInfo(r);
    } catch (e) {
      setMessage(safeFailure(e));
    } finally {
      setBusy(false);
    }
  }
  function show() {
    setOpen(true);
    void inspect();
  }
  async function apply(hash: string | null) {
    if (!info) return;
    setBusy(true);
    setMessage("");
    try {
      const r = await request({
        action: "format_change",
        kind,
        artifact,
        source: info.source,
        restore: hash,
      });
      if (r.kind !== "write") throw new BridgeFailure("protocol");
      if (
        r.disk !== "committed" ||
        r.error ||
        r.cleanup_failed ||
        r.recovery_required
      ) {
        setMessage(text("format.incomplete"));
        await shell.checkStatus();
        return;
      }
      setInfo(null);
      setRestore("");
      setMessage("");
      await changed();
      setOpen(false);
    } catch (e) {
      setMessage(safeFailure(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <IconCommand
        label={text("format.title")}
        icon={<History20Regular />}
        disabled={busy}
        onClick={show}
      />
      <Dialog
        open={open}
        modalType="modal"
        onOpenChange={(_, data) => setOpen(data.open)}
      >
        <DialogSurface className="format-history-dialog">
          <DialogBody>
            <DialogTitle>{text("format.title")}</DialogTitle>
            <DialogContent>
              <p className="format-history-help">{text("format.help")}</p>
              {busy && !info && <p role="status">{text("format.loading")}</p>}
              {message && (
                <FloatingNotice intent="warning">
                  <FloatingNoticeContent>{message}</FloatingNoticeContent>
                </FloatingNotice>
              )}
              {info && (
                <div className="format-history-content">
                  <section>
                    <h3>{text("format.current")}</h3>
                    <p>
                      {text("format.version", {
                        version: String(info.schema),
                      })}
                    </p>
                    {info.schema < (kind === "template" ? 5 : 4) && (
                      <Button
                        type="button"
                        disabled={locked || busy}
                        onClick={() => void apply(null)}
                      >
                        {text("format.upgrade")}
                      </Button>
                    )}
                  </section>
                  <section>
                    <h3>{text("format.savedVersions")}</h3>
                    {info.history.length > 0 ? (
                      <>
                        <Select
                          aria-label={text("format.restore")}
                          value={restore}
                          disabled={locked || busy}
                          onChange={(e) => setRestore(e.target.value)}
                        >
                          <option value="">{text("format.choose")}</option>
                          {info.history.map((h, index) => (
                            <option key={h.digest} value={h.digest}>
                              {text("format.version", {
                                version: String(h.schema),
                              })}{" "}
                              · {index + 1} ·{" "}
                              {formatLocalDateTime(h.content_updated_at)}
                            </option>
                          ))}
                        </Select>
                        <p className="format-history-help">
                          {text("format.restoreHelp")}
                        </p>
                        <Button
                          type="button"
                          disabled={!restore || locked || busy}
                          onClick={() => void apply(restore)}
                        >
                          {text("format.restore")}
                        </Button>
                      </>
                    ) : (
                      <p>{text("format.empty")}</p>
                    )}
                  </section>
                </div>
              )}
            </DialogContent>
            <DialogActions>
              <Button type="button" onClick={() => setOpen(false)}>
                {text("common.close")}
              </Button>
            </DialogActions>
          </DialogBody>
        </DialogSurface>
      </Dialog>
    </>
  );
}
