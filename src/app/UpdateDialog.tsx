import { useEffect, useSyncExternalStore } from "react";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  ProgressBar,
  Spinner,
} from "@fluentui/react-components";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text, type StringKey } from "../strings";
import { type UpdateController, type UpdateStatus } from "./updaterClient";
import "./UpdateDialog.css";

export function updateMessage(status: UpdateStatus | null): StringKey {
  if (!status) return "update.unavailable";
  if (status.distribution === "store") return "update.store";
  if (status.phase === "unavailable") return "update.dev";
  switch (status.phase) {
    case "available":
      return "update.available";
    case "latest":
      return "update.latest";
    case "ready":
      return "update.ready";
    case "preparing":
      return "update.preparing";
    case "installing":
      return status.testMode ? "update.testInstalling" : "update.installing";
    case "install_failed":
      return status.error === "test_handoff"
        ? "update.testComplete"
        : "update.installFailed";
    case "cancelled":
      return "update.cancelled";
    case "skipped":
      return "update.skipped";
    case "failed":
      switch (status.error) {
        case "signature":
          return "update.signature";
        case "target":
        case "metadata":
          return "update.target";
        case "settings":
          return "update.settings";
        case "recovery_handoff":
        case "handoff":
          return "update.handoffFailed";
        case "active_work":
        case "shutdown_blocked":
          return "update.blocked";
        default:
          return "update.network";
      }
    case "downloading":
      return "update.downloading";
    default:
      return "update.checking";
  }
}

export function UpdateDialog({
  controller,
  open,
  onClose,
}: {
  controller: UpdateController;
  open: boolean;
  onClose: () => void;
}) {
  const status = useSyncExternalStore(
    controller.subscribe,
    controller.snapshot,
  );
  const closing = ["preparing", "installing"].includes(status?.phase ?? "");
  useEffect(() => {
    if (open) void controller.refresh();
  }, [controller, open]);
  const close = () => {
    if (closing) return;
    onClose();
    if (
      status &&
      !status.startupComplete &&
      !closing &&
      status.phase !== "install_failed"
    )
      void controller.continue();
  };
  const retryable =
    status &&
    ["failed", "cancelled"].includes(status.phase) &&
    !status.startupComplete &&
    !["signature", "target", "metadata"].includes(status.error ?? "");
  return (
    <Dialog
      open={open}
      onOpenChange={(_, data) => {
        if (!data.open) close();
      }}
    >
      <DialogSurface className="update-dialog">
        <DialogBody>
          <DialogTitle>{text("update.title")}</DialogTitle>
          <DialogContent>
            {status?.installTest && (
              <InlineNotice kind="info">
                {text("update.installTest")}
              </InlineNotice>
            )}
            {status?.testMode && (
              <InlineNotice kind="info">{text("update.testMode")}</InlineNotice>
            )}
            <dl className="update-versions">
              <dt>{text("update.current")}</dt>
              <dd>{status?.currentVersion ?? "—"}</dd>
              {status?.version && (
                <>
                  <dt>{text("update.new")}</dt>
                  <dd>{status.version}</dd>
                </>
              )}
            </dl>
            <InlineNotice
              kind={
                status?.phase === "failed" ||
                (status?.phase === "install_failed" &&
                  status.error !== "test_handoff")
                  ? "error"
                  : "info"
              }
            >
              {text(updateMessage(status))}
            </InlineNotice>
            {status?.phase === "checking" && <Spinner size="tiny" />}
            {status?.phase === "downloading" && (
              <div className="update-progress">
                <ProgressBar
                  aria-label={text("update.downloading")}
                  max={status.total ?? undefined}
                  value={status.total ? status.downloaded : undefined}
                />
                <p>
                  {new Intl.NumberFormat(undefined, {
                    maximumFractionDigits: 1,
                  }).format(status.downloaded / 1024 / 1024)}{" "}
                  MB
                  {status.total
                    ? ` / ${new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(status.total / 1024 / 1024)} MB`
                    : ""}
                </p>
              </div>
            )}
            {status?.releaseUrl && (
              <Button
                appearance="subtle"
                onClick={() => void controller.openRelease()}
              >
                {text("update.release")}
              </Button>
            )}
            {status?.handoff &&
              (status.handoff.recovery.length > 0 ||
                status.handoff.pendingSvn.length > 0) && (
                <p>{text("update.handoffPreserved")}</p>
              )}
            {status?.notes && (
              <section className="update-notes">
                <h3>{text("update.notes")}</h3>
                <p>{status.notes}</p>
              </section>
            )}
            {status?.phase === "downloading" && (
              <p className="update-secondary">
                {text("update.closeContinues")}
              </p>
            )}
            {(status?.phase === "available" || status?.phase === "ready") && (
              <p className="update-secondary">{text("update.protect")}</p>
            )}
          </DialogContent>
          <DialogActions className="update-actions">
            {(status?.phase === "available" || status?.phase === "ready") &&
              !status.startupComplete && (
                <Button
                  appearance="primary"
                  disabled={status.cancelPending}
                  onClick={() => void controller.update()}
                >
                  {text("update.install")}
                </Button>
              )}
            {status?.phase === "downloading" && (
              <Button
                disabled={status.cancelPending}
                onClick={() => void controller.cancel()}
              >
                {text("update.cancel")}
              </Button>
            )}
            {retryable && (
              <Button
                disabled={status.cancelPending}
                onClick={() => void controller.retry()}
              >
                {text("update.retry")}
              </Button>
            )}
            {status?.phase === "install_failed" && (
              <Button onClick={() => void controller.closeFailed()}>
                {text("update.exit")}
              </Button>
            )}
            <Button autoFocus disabled={closing} onClick={close}>
              {text(
                status &&
                  !status.startupComplete &&
                  !closing &&
                  status.phase !== "install_failed"
                  ? "update.later"
                  : "update.close",
              )}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
