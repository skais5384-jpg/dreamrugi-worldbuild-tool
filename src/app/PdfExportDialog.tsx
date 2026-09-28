import { useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  MessageBar,
  MessageBarBody,
} from "@fluentui/react-components";
import { Button } from "../ui/Controls";
import type { DocumentController } from "./documentController";
import type { DocumentResponse } from "../bridge/documents";
import { BridgeFailure } from "../bridge/client";
import { safeFailure } from "./operations";
import { text } from "../strings";

type Inspection = Extract<DocumentResponse, { kind: "pdf_inspect" }>;

export function pdfFilename(name: string): string {
  let stem = Array.from(name)
    .map((character) =>
      character.charCodeAt(0) < 32 || '<>:"/\\|?*'.includes(character)
        ? "_"
        : character,
    )
    .join("")
    .trim()
    .replace(/[. ]+$/g, "")
    .replace(/\.pdf$/i, "")
    .trim()
    .slice(0, 120)
    .replace(/[. ]+$/g, "");
  if (!stem || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(stem))
    stem = text("pdf.defaultName");
  return stem + ".pdf";
}

function pdfError(error: unknown): string {
  if (error instanceof BridgeFailure) {
    if (error.boundary?.code === "duplicate_conflict")
      return text("pdf.conflict");
    if (error.boundary?.code === "wrong_binding")
      return text("pdf.sourceChanged");
    if (error.boundary?.code === "cancelled") return "";
  }
  return safeFailure(error);
}

export function PdfExportDialog({
  controller,
  document,
  dirty,
  close,
  completed,
}: {
  controller: DocumentController;
  document: string;
  dirty: boolean;
  close: () => void;
  completed: (cleanupWarning: boolean) => void;
}) {
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [phase, setPhase] = useState<
    "inspecting" | "ready" | "choosing" | "rendering"
  >("inspecting");
  const [error, setError] = useState("");
  const [cancelRequested, setCancelRequested] = useState(false);
  const owner = useRef<symbol | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      owner.current = null;
    };
  }, []);

  useEffect(() => {
    let current = true;
    void controller
      .pdfInspect(document)
      .then((result) => {
        if (!current) return;
        setInspection(result);
        setPhase("ready");
      })
      .catch((cause) => {
        if (!current) return;
        setError(pdfError(cause));
        setPhase("ready");
      });
    return () => {
      current = false;
    };
  }, [controller, document]);

  const retry = async () => {
    setError("");
    setPhase("inspecting");
    try {
      setInspection(await controller.pdfInspect(document));
    } catch (cause) {
      setInspection(null);
      setError(pdfError(cause));
    } finally {
      setPhase("ready");
    }
  };

  const exportPdf = async () => {
    if (!inspection || phase !== "ready" || owner.current) return;
    // save() can open an OS modal before React renders another phase.
    const request = Symbol("pdf-export");
    owner.current = request;
    const current = () => mounted.current && owner.current === request;
    setPhase("choosing");
    let destination: string | null;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      if (!current()) return;
      destination = await save({
        title: text("pdf.title"),
        defaultPath: pdfFilename(inspection.name),
        filters: [{ name: "PDF", extensions: ["pdf"] }],
      });
    } catch (cause) {
      if (current()) {
        setError(pdfError(cause));
        setPhase("ready");
      }
      if (owner.current === request) owner.current = null;
      return;
    }
    try {
      if (!current()) return;
      if (!destination) {
        close();
        return;
      }
      setPhase("rendering");
      setError("");
      setCancelRequested(false);
      const result = await controller.pdfExport(
        document,
        inspection.source,
        destination,
        inspection.missing_images.length > 0,
      );
      if (!current()) return;
      completed(result.cleanup_warning);
      close();
    } catch (cause) {
      if (!current()) return;
      const message = pdfError(cause);
      if (!message) {
        close();
        return;
      }
      setError(message);
      setPhase("ready");
    } finally {
      if (owner.current === request) owner.current = null;
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(_, data) => {
        if (!data.open && !owner.current) close();
      }}
    >
      <DialogSurface>
        <DialogBody>
          <DialogTitle>{text("pdf.title")}</DialogTitle>
          <DialogContent>
            <p>{text("pdf.purpose")}</p>
            {inspection && (
              <p>
                <strong>{inspection.name}</strong> · {text("pdf.savedBasis")}
              </p>
            )}
            {dirty && (
              <MessageBar intent="warning">
                <MessageBarBody>{text("pdf.savedOnly")}</MessageBarBody>
              </MessageBar>
            )}
            {inspection && inspection.missing_images.length > 0 && (
              <MessageBar intent="warning">
                <MessageBarBody>
                  {text("pdf.missing")}
                  <ul>
                    {inspection.missing_images.map((name, index) => (
                      <li key={index}>{name}</li>
                    ))}
                  </ul>
                </MessageBarBody>
              </MessageBar>
            )}
            {phase === "inspecting" && (
              <p role="status">{text("pdf.inspecting")}</p>
            )}
            {phase === "rendering" && (
              <p role="status">{text("pdf.rendering")}</p>
            )}
            {!!error && (
              <MessageBar intent="error">
                <MessageBarBody>{error}</MessageBarBody>
              </MessageBar>
            )}
          </DialogContent>
          <DialogActions>
            {phase === "rendering" ? (
              <Button
                type="button"
                disabled={cancelRequested}
                onClick={() => {
                  setCancelRequested(true);
                  void controller
                    .cancelPdfExport()
                    .then((accepted) => {
                      if (!accepted && mounted.current && owner.current) {
                        setCancelRequested(false);
                        setError(text("documents.cancelUnknown"));
                      }
                    })
                    .catch(() => {
                      if (mounted.current && owner.current) {
                        setCancelRequested(false);
                        setError(text("documents.cancelUnknown"));
                      }
                    });
                }}
              >
                {text("pdf.cancel")}
              </Button>
            ) : (
              <>
                {!inspection && phase === "ready" && (
                  <Button type="button" onClick={() => void retry()}>
                    {text("pdf.retry")}
                  </Button>
                )}
                {inspection && (
                  <Button
                    type="button"
                    appearance="primary"
                    disabled={phase === "choosing"}
                    onClick={() => void exportPdf()}
                  >
                    {text(
                      inspection.missing_images.length
                        ? "pdf.continue"
                        : "pdf.choose",
                    )}
                  </Button>
                )}
                <Button
                  type="button"
                  onClick={close}
                  disabled={phase === "choosing"}
                >
                  {text("pdf.close")}
                </Button>
              </>
            )}
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
