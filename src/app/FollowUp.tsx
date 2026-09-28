import { useState } from "react";
import {
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogContent,
  DialogActions,
} from "@fluentui/react-components";
import { InlineNotice } from "../ui/InlineNotice";
import { Button } from "../ui/Controls";
import type { TemplateController } from "./controller";
import { svnFailure } from "./svnClient";
import { text } from "../strings";

/** Ordinary progress stays with its owner; only problems with a user action appear here. */
export function FollowUp({ controller }: { controller: TemplateController }) {
  const state = controller.snapshot();
  const operations = state.operations.filter(
    (operation) => operation.problem || operation.phase === "reserved",
  );
  const sessions = state.sessions.filter((session) => session.problem);
  const shutdown = state.project?.shutdown;
  const failedReport =
    !!shutdown?.reportPending &&
    shutdown.reports.some(
      (report) =>
        report.initializationFailed ||
        report.closeFailed ||
        report.releaseFailures !== "0" ||
        report.coordinationErrors.length > 0,
    );
  const pendingUnknownReport =
    !!shutdown?.reportPending &&
    shutdown.reports.length === 0 &&
    shutdown.forceResults.length === 0;
  const blockers = shutdown?.blockers ?? [];
  const nativeCleanupFailed = state.app?.native_cleanup.phase === "failed";
  const needsAction =
    operations.length > 0 ||
    state.retainedRefs.length > 0 ||
    sessions.length > 0 ||
    failedReport ||
    pendingUnknownReport ||
    blockers.length > 0 ||
    !!shutdown?.forceActive ||
    !!shutdown?.forceResults.some(
      (result) => result.outcome !== "verified" || result.error,
    ) ||
    nativeCleanupFailed;
  const [requested, setRequested] = useState(false);
  if (!needsAction) return null;

  return (
    <>
      <div className="follow-up-floating" role="status">
        <InlineNotice kind="warning">
          {text("followUp.actionRequired")}
          <Button type="button" size="small" onClick={() => setRequested(true)}>
            {text("followUp.openActions")}
          </Button>
        </InlineNotice>
      </div>
      {requested && (
        <Dialog
          open
          onOpenChange={(_, data) => {
            if (!data.open) setRequested(false);
          }}
        >
          <DialogSurface className="follow-up-dialog">
            <DialogBody>
              <DialogTitle>{text("followUp.actionRequired")}</DialogTitle>
              <DialogContent className="follow-up-actions">
                {state.retainedRefs.length > state.retained.length && (
                  <p>{text("followUp.message02")}</p>
                )}
                {operations.map((operation) => (
                  <section key={operation.id} className="follow-up-action">
                    <strong>{operation.label}</strong>
                    <p>
                      {text(
                        operation.phase === "reserved"
                          ? "followUp.message04"
                          : operation.phase === "acknowledging"
                            ? "followUp.message03"
                            : "followUp.message05",
                      )}
                    </p>
                    {operation.problem && <p>{operation.problem}</p>}
                    {operation.phase === "reserved" && (
                      <Button
                        type="button"
                        onClick={() =>
                          void controller.operations.resubmit(operation.id)
                        }
                      >
                        {text("followUp.message07")}
                      </Button>
                    )}
                  </section>
                ))}
                {state.retained.map((retained) => (
                  <section
                    key={retained.retained.id}
                    className="follow-up-action"
                  >
                    <strong>{text("followUp.message08")}</strong>
                    <p>
                      {retained.intent?.kind === "create_template"
                        ? retained.intent.name
                        : retained.intent?.kind === "update_template" &&
                            retained.intent.edit.kind === "name"
                          ? retained.intent.edit.name
                          : text("followUp.message09")}
                    </p>
                    <p>
                      {text(
                        retained.g6_clearable
                          ? "followUp.message10"
                          : "followUp.message11",
                      )}
                    </p>
                    <Button
                      type="button"
                      disabled={
                        state.busy || !!state.prompt || !retained.g6_clearable
                      }
                      onClick={() =>
                        void controller.abandon(retained.retained.id)
                      }
                    >
                      {text("followUp.message12")}
                    </Button>
                  </section>
                ))}
                {sessions.map((session) => (
                  <section key={session.id} className="follow-up-action">
                    <strong>{text("followUp.sessionTitle")}</strong>
                    <p>
                      {text("followup.sessionProblem", {
                        problem: session.problem ?? "",
                      })}
                    </p>
                    <Button
                      type="button"
                      disabled={
                        state.busy ||
                        !session.observation ||
                        session.observation.active_dirty ||
                        !!session.observation.custody ||
                        !["Editing", "ReadOnly", "LockLost"].includes(
                          session.observation.state,
                        )
                      }
                      onClick={() => void controller.finishSession(session.id)}
                    >
                      {text("followUp.message13")}
                    </Button>
                    <Button
                      type="button"
                      disabled={
                        state.busy ||
                        session.observation?.state !== "ReleaseFailed" ||
                        session.observation.active_dirty ||
                        !!session.observation.custody
                      }
                      onClick={() =>
                        void controller.finishSession(session.id, true)
                      }
                    >
                      {text("controller.message23")}
                    </Button>
                  </section>
                ))}
                {(failedReport ||
                  pendingUnknownReport ||
                  blockers.length > 0 ||
                  shutdown?.forceActive) && (
                  <section className="follow-up-action">
                    <strong>{text("followUp.shutdownProblem")}</strong>
                    {shutdown?.forceActive && <p>{text("svn.forceClosing")}</p>}
                    {shutdown?.forceResults.map((result) => (
                      <p key={result.document} role="alert">
                        {text(
                          result.outcome === "verified"
                            ? "svn.forceCloseVerified"
                            : "svn.forceCloseUnknown",
                        )}
                        {result.error ? ` (${svnFailure(result.error)})` : ""}
                      </p>
                    ))}
                    <p>{text("followUp.shutdownHelp")}</p>
                    {(failedReport || !!shutdown?.forceResults.length) &&
                      !shutdown?.forceActive && (
                        <details>
                          <summary>{text("error.details")}</summary>
                          <p>
                            {text("followup.shutdownReport", {
                              count: String(shutdown?.reports.length ?? 0),
                              failures:
                                shutdown?.reports
                                  .map((report) => report.releaseFailures)
                                  .join(", ") || "0",
                            })}
                          </p>
                        </details>
                      )}
                    {blockers.includes("ReleaseBudget") && (
                      <Button
                        type="button"
                        disabled={state.busy}
                        onClick={() => void controller.releaseRound()}
                      >
                        {text("followUp.message16")}
                      </Button>
                    )}
                    {failedReport && (
                      <Button
                        type="button"
                        disabled={state.busy}
                        onClick={() => void controller.acknowledgeShutdown()}
                      >
                        {text("followUp.confirmFailedReport")}
                      </Button>
                    )}
                  </section>
                )}
                {nativeCleanupFailed && (
                  <section className="follow-up-action">
                    <strong>{text("followUp.nativeCleanupProblem")}</strong>
                    <Button
                      type="button"
                      disabled={state.busy}
                      onClick={() => void controller.retryNative()}
                    >
                      {text("followUp.message17")}
                    </Button>
                  </section>
                )}
              </DialogContent>
              <DialogActions>
                <Button
                  type="button"
                  disabled={state.busy}
                  onClick={() => void controller.checkStatus()}
                >
                  {text("followUp.message06")}
                </Button>
                <Button type="button" onClick={() => setRequested(false)}>
                  {text("common.close")}
                </Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      )}
    </>
  );
}
