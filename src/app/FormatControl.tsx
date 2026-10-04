import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
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
import { Button } from "../ui/Controls";
import { IconCommand } from "../ui/IconCommand";
import { InlineNotice } from "../ui/InlineNotice";
import type { ReferenceContext } from "./DocumentReferenceValue";
import { DocumentContent } from "./DocumentContent";
import { ReadonlyTemplate } from "./ReadonlyTemplate";
import {
  MediaActiveContext,
  MediaContext,
  ExternalMediaPreviewContext,
  AssetNamesContext,
  MediaTargetContext,
} from "./MediaValue";
import { text } from "../strings";
import { formatLocalDateTime } from "./localDateTime";

type Versions = Extract<DocumentResponse, { kind: "versions" }>;
type Preview = Extract<DocumentResponse, { kind: "version_preview" }>;
type FormatControlProps = {
  shell: TemplateController;
  kind: "template" | "document";
  artifact: string;
  locked: boolean;
  reference?: ReferenceContext;
  changed: () => Promise<void>;
};
export function FormatControl(props: FormatControlProps) {
  const { shell, kind, artifact } = props;
  const projectId = useSyncExternalStore(
    shell.subscribe,
    shell.snapshot,
    shell.snapshot,
  ).projectId;
  return (
    <FormatControlSession
      key={JSON.stringify([projectId, kind, artifact])}
      {...props}
    />
  );
}
function FormatControlSession({
  shell,
  kind,
  artifact,
  locked,
  changed,
  reference,
}: FormatControlProps) {
  const [open, setOpen] = useState(false);
  const [versions, setVersions] = useState<Versions | null>(null);
  const [selected, setSelected] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [confirm, setConfirm] = useState(false);
  const pending = useRef(0);
  useEffect(() => {
    const gate = pending;
    return () => {
      ++gate.current;
    };
  }, []);
  useEffect(() => {
    if (!busy) return;
    const timer = setInterval(() => void shell.operations.queryAll(), 250);
    return () => clearInterval(timer);
  }, [busy, shell]);
  const request = useCallback(
    async (request: DocumentRequest) => {
      const project = shell.snapshot().projectId;
      if (!project) throw new BridgeFailure("protocol");
      const generation = shell.projectGeneration();
      const { result } = await shell.operations.run(
        { kind: "document_workspace", project, request },
        text("format.title"),
      );
      if (
        shell.snapshot().projectId !== project ||
        shell.projectGeneration() !== generation
      )
        throw new BridgeFailure("protocol");
      if (result.kind === "rejected")
        throw new BridgeFailure("boundary", undefined, result.error);
      return result.kind === "document_workspace" ? result.value : result;
    },
    [shell],
  );
  const previewMedia = useMemo(
    () => ({
      media: request,
      pollProgress: () => shell.operations.queryAll(),
    }),
    [request, shell],
  );
  const mediaTarget = useMemo(() => ({ kind, artifact }), [kind, artifact]);
  async function select(version: string) {
    const current = ++pending.current;
    setSelected(version);
    setPreview(null);
    setBusy(true);
    setMessage("");
    setConfirm(false);
    try {
      const response = await request({
        action: "version_preview",
        kind,
        artifact,
        version,
      });
      if (current !== pending.current) return;
      if (response.kind !== "version_preview")
        throw new BridgeFailure("protocol");
      setPreview(response);
    } catch (error) {
      if (current === pending.current) setMessage(safeFailure(error));
    } finally {
      if (current === pending.current) setBusy(false);
    }
  }
  async function show() {
    const current = ++pending.current;
    setOpen(true);
    setBusy(true);
    setMessage("");
    setVersions(null);
    setPreview(null);
    setConfirm(false);
    try {
      const response = await request({
        action: "versions_list",
        kind,
        artifact,
      });
      if (current !== pending.current) return;
      if (response.kind !== "versions") throw new BridgeFailure("protocol");
      setVersions(response);
      const newest = response.versions.find((row) => row.available);
      if (newest) {
        await select(newest.version);
        return;
      }
    } catch (error) {
      if (current === pending.current) setMessage(safeFailure(error));
    } finally {
      if (current === pending.current) setBusy(false);
    }
  }
  async function restore() {
    if (!preview || locked) return;
    const current = ++pending.current;
    setBusy(true);
    setMessage("");
    try {
      const response = await request({
        action: "version_restore",
        kind,
        artifact,
        version: preview.version,
        source: preview.source,
      });
      if (current !== pending.current) return;
      if (response.kind !== "write") throw new BridgeFailure("protocol");
      if (
        !["committed", "no_write"].includes(response.disk) ||
        response.error ||
        response.cleanup_failed ||
        response.recovery_required
      ) {
        setMessage(text("format.incomplete"));
        await shell.checkStatus();
        return;
      }
      await changed();
      setOpen(false);
      setConfirm(false);
    } catch (error) {
      if (current === pending.current) {
        setMessage(safeFailure(error));
        setConfirm(false);
      }
    } finally {
      if (current === pending.current) setBusy(false);
    }
  }
  return (
    <>
      <IconCommand
        label={text("format.title")}
        icon={<History20Regular />}
        disabled={busy}
        onClick={() => void show()}
      />
      <Dialog
        open={open}
        modalType="modal"
        onOpenChange={(_, data) => {
          if (!busy) {
            ++pending.current;
            setOpen(data.open);
          }
        }}
      >
        <DialogSurface className="content-versions-dialog">
          <DialogBody>
            <DialogTitle>{text("format.title")}</DialogTitle>
            <DialogContent>
              <p className="format-history-help">{text("format.help")}</p>
              {message && <InlineNotice kind="warning">{message}</InlineNotice>}
              {busy && <p role="status">{text("format.loading")}</p>}
              {versions && !versions.versions.length && (
                <p>{text("format.empty")}</p>
              )}
              {versions &&
                versions.versions.length > 0 &&
                !versions.versions.some((row) => row.available) && (
                  <InlineNotice kind="warning">
                    {text("format.backupNeeded")}
                  </InlineNotice>
                )}
              {!!versions?.versions.length && (
                <div className="content-versions-grid">
                  <div
                    role="listbox"
                    aria-label={text("format.savedVersions")}
                    className="content-versions-list"
                  >
                    {versions.versions.map((row) => (
                      <button
                        type="button"
                        role="option"
                        key={row.version}
                        aria-selected={selected === row.version}
                        disabled={busy || !row.available}
                        onClick={() => void select(row.version)}
                      >
                        <strong>
                          {text("format.version", { version: row.version })}
                        </strong>
                        <span>
                          {row.available
                            ? formatLocalDateTime(row.recorded_at_utc)
                            : text("format.unavailable")}
                        </span>
                      </button>
                    ))}
                  </div>
                  <section
                    className="content-version-preview"
                    aria-label={text("format.preview")}
                  >
                    <MediaContext.Provider value={previewMedia}>
                      <MediaTargetContext.Provider value={mediaTarget}>
                        <MediaActiveContext.Provider value={false}>
                          <ExternalMediaPreviewContext.Provider value={false}>
                            <AssetNamesContext.Provider
                              value={preview?.asset_names ?? {}}
                            >
                              {preview?.template && (
                                <ReadonlyTemplate template={preview.template} />
                              )}
                              {preview?.document && (
                                <article>
                                  <header>
                                    <h2>{preview.document.name}</h2>
                                    {!!preview.document.englishName && (
                                      <p>{preview.document.englishName}</p>
                                    )}
                                    {!!preview.document.glossarySummary && (
                                      <p>{preview.document.glossarySummary}</p>
                                    )}
                                  </header>
                                  <DocumentContent
                                    read={preview.document}
                                    reference={
                                      reference
                                        ? {
                                            ...reference,
                                            preview: true,
                                            open: () => {},
                                          }
                                        : undefined
                                    }
                                  />
                                </article>
                              )}
                            </AssetNamesContext.Provider>
                          </ExternalMediaPreviewContext.Provider>
                        </MediaActiveContext.Provider>
                      </MediaTargetContext.Provider>
                    </MediaContext.Provider>
                  </section>
                </div>
              )}
              {confirm && (
                <InlineNotice kind="warning">
                  {text(
                    kind === "template"
                      ? "format.restoreTemplateHelp"
                      : "format.restoreDocumentHelp",
                  )}
                </InlineNotice>
              )}
            </DialogContent>
            <DialogActions>
              {confirm ? (
                <>
                  <Button
                    type="button"
                    disabled={busy}
                    onClick={() => setConfirm(false)}
                  >
                    {text("common.cancel")}
                  </Button>
                  <Button
                    type="button"
                    appearance="primary"
                    disabled={busy || locked}
                    onClick={() => void restore()}
                  >
                    {text("format.confirmRestore")}
                  </Button>
                </>
              ) : (
                <Button
                  type="button"
                  appearance="primary"
                  disabled={!preview || busy || locked}
                  onClick={() => setConfirm(true)}
                >
                  {text("format.restore")}
                </Button>
              )}
              <Button
                type="button"
                disabled={busy}
                onClick={() => {
                  ++pending.current;
                  setOpen(false);
                }}
              >
                {text("common.close")}
              </Button>
            </DialogActions>
          </DialogBody>
        </DialogSurface>
      </Dialog>
    </>
  );
}
