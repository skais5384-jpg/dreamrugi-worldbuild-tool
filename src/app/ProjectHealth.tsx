import {
  Badge,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  MessageBar,
  MessageBarBody,
} from "@fluentui/react-components";
import {
  CheckmarkCircle20Filled,
  Info16Regular,
  Warning20Filled,
} from "@fluentui/react-icons";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";
import type { TemplateController } from "./controller";
import "./ProjectHealth.css";

export interface HealthDocument {
  id: string;
  name: string;
  template: string | null;
}

export function ProjectHealth({
  controller,
  documents,
  openResources,
  openTrash,
}: {
  controller: TemplateController;
  documents: HealthDocument[];
  openResources: (keys: string[]) => void;
  openTrash: (keys: string[]) => void;
}) {
  const state = controller.snapshot();
  const dialog = state.health;
  if (!dialog || dialog.surface !== "health") return null;
  const inspection = dialog.inspection;
  const pending =
    state.busy || ["checking", "exporting"].includes(dialog.phase);
  const resourceProblems = inspection?.rows.filter((row) =>
    ["missing", "corrupt", "uncertain"].includes(row.status),
  );
  const trashedResources = inspection?.rows.filter(
    (row) => row.status === "in_trash",
  );
  const deletedTemplateIds = new Set(
    inspection?.deletedTemplates.map((row) => row.id) ?? [],
  );
  const templateProblemIds = new Set(
    (inspection?.documentIssues ?? [])
      .filter((row) =>
        row.reasons.some((reason) =>
          ["deleted_template", "missing_template"].includes(reason),
        ),
      )
      .map((row) => row.documentId),
  );
  const templateProblemDocuments = documents.filter(
    (row) =>
      templateProblemIds.has(row.id) ||
      (!!row.template && deletedTemplateIds.has(row.template)),
  );
  const restorableTemplateKeys = [
    ...new Set(
      templateProblemDocuments
        .map((row) => row.template)
        .filter(
          (template): template is string =>
            !!template && deletedTemplateIds.has(template),
        )
        .map((template) => `template:${template}`),
    ),
  ];
  const problemCount =
    (resourceProblems?.length ?? 0) +
    (trashedResources?.length ?? 0) +
    templateProblemDocuments.length;
  const status =
    state.inspectionPendingDocuments.length && !inspection
      ? { color: "warning" as const, label: text("health.badge.partial") }
      : !inspection
        ? {
            color: "informative" as const,
            label: text("health.badge.unchecked"),
          }
        : !inspection.complete
          ? { color: "warning" as const, label: text("health.badge.partial") }
          : problemCount > 0
            ? {
                color: "warning" as const,
                label: text("health.badge.problems", {
                  count: String(problemCount),
                }),
              }
            : {
                color: "success" as const,
                label: text("health.badge.complete"),
              };
  const summary = inspection
    ? [
        "Worldbuild project diagnostics v2",
        `complete=${inspection.complete}`,
        `scanned=${inspection.scannedFiles}`,
        `missing=${inspection.missingAssets}`,
        `corrupt=${inspection.corruptAssets}`,
        `uncertain=${inspection.uncertainAssets}`,
        `templateProblemDocuments=${templateProblemDocuments.length}`,
      ].join("\n")
    : "Worldbuild project diagnostics v2\ninspection=unavailable";
  return (
    <Dialog open modalType="modal">
      <DialogSurface className="health-surface">
        <DialogBody className="health-body">
          <DialogTitle>{text("health.title")}</DialogTitle>
          <DialogContent className="health-content">
            <div className="health-intro">
              <Info16Regular aria-hidden="true" />
              <p>{text("health.problemHelp")}</p>
            </div>
            {dialog.error && (
              <MessageBar intent="error" role="alert">
                <MessageBarBody>{dialog.error}</MessageBarBody>
              </MessageBar>
            )}
            {dialog.message && (
              <MessageBar intent="success" role="status">
                <MessageBarBody>{dialog.message}</MessageBarBody>
              </MessageBar>
            )}
            <section className="health-section">
              <div className="health-section-heading">
                <div className="health-heading-copy">
                  <h3>{text("health.status")}</h3>
                  <Badge appearance="tint" color={status.color}>
                    {status.label}
                  </Badge>
                </div>
                <Button
                  type="button"
                  disabled={pending}
                  onClick={() => void controller.inspectAssets()}
                >
                  {text("health.check")}
                </Button>
              </div>
              {!inspection && state.inspectionPendingDocuments.length ? (
                <InlineNotice kind="warning" className="health-section-help">
                  {text("health.checkPartial")}
                </InlineNotice>
              ) : !inspection ? (
                <p className="health-section-help">
                  {text("health.notChecked")}
                </p>
              ) : !inspection.complete ? (
                <InlineNotice kind="warning" className="health-section-help">
                  {text("health.checkPartial")}
                </InlineNotice>
              ) : problemCount === 0 ? (
                <div className="health-clear" role="status">
                  <CheckmarkCircle20Filled aria-hidden="true" />
                  <span>{text("health.noProblems")}</span>
                </div>
              ) : (
                <div className="health-problems">
                  {!!resourceProblems?.length && (
                    <article className="health-problem">
                      <span className="health-problem-icon" aria-hidden="true">
                        <Warning20Filled />
                      </span>
                      <div className="health-problem-copy">
                        <strong>
                          {text("health.resourceProblems", {
                            count: String(resourceProblems.length),
                          })}
                        </strong>
                        <p>{text("health.resourceProblemsHelp")}</p>
                      </div>
                      <Button
                        type="button"
                        onClick={() =>
                          openResources(
                            resourceProblems.map((row) => `resource:${row.id}`),
                          )
                        }
                      >
                        {text("health.openResources")}
                      </Button>
                    </article>
                  )}
                  {!!trashedResources?.length && (
                    <article className="health-problem">
                      <span className="health-problem-icon" aria-hidden="true">
                        <Warning20Filled />
                      </span>
                      <div className="health-problem-copy">
                        <strong>
                          {text("health.resourcesInTrash", {
                            count: String(trashedResources.length),
                          })}
                        </strong>
                        <p>{text("health.resourcesInTrashHelp")}</p>
                      </div>
                      <Button
                        type="button"
                        onClick={() =>
                          openTrash(
                            trashedResources.map((row) => `resource:${row.id}`),
                          )
                        }
                      >
                        {text("common.confirm")}
                      </Button>
                    </article>
                  )}
                  {!!templateProblemDocuments.length && (
                    <article className="health-problem">
                      <span className="health-problem-icon" aria-hidden="true">
                        <Warning20Filled />
                      </span>
                      <div className="health-problem-copy">
                        <strong>
                          {text("health.documentsWithoutTemplate", {
                            count: String(templateProblemDocuments.length),
                          })}
                        </strong>
                        <p>{text("health.documentsWithoutTemplateHelp")}</p>
                      </div>
                      {!!restorableTemplateKeys.length && (
                        <Button
                          type="button"
                          onClick={() => openTrash(restorableTemplateKeys)}
                        >
                          {text("common.confirm")}
                        </Button>
                      )}
                    </article>
                  )}
                </div>
              )}
            </section>
            <details className="health-details">
              <summary>
                <span className="health-details-summary">
                  <Info16Regular aria-hidden="true" />
                  {text("health.diagnostics")}
                </span>
              </summary>
              <div className="health-details-body">
                <p>{text("health.diagnosticsHelp")}</p>
                {state.app?.support_diagnostics.map((diagnostic) => (
                  <p
                    key={`${diagnostic.feature}-${diagnostic.stage}-${diagnostic.causeId ?? "none"}`}
                  >
                    {text("supportDiagnostics.youtube", {
                      stage: diagnostic.stage,
                      category: diagnostic.category,
                      cause:
                        diagnostic.causeId ?? text("supportDiagnostics.none"),
                    })}
                  </p>
                ))}
                <div className="health-export-actions">
                  <Button
                    type="button"
                    onClick={() => void navigator.clipboard.writeText(summary)}
                  >
                    {text("health.copy")}
                  </Button>
                  <Button
                    type="button"
                    disabled={pending}
                    onClick={() => controller.showDiagnosticExport()}
                  >
                    {text("health.export")}
                  </Button>
                </div>
              </div>
            </details>
            {dialog.exportConfirm && (
              <section className="health-section health-export">
                <h3>{text("health.exportTitle")}</h3>
                <p>{text("health.exportIncludes")}</p>
                <p>{text("health.exportExcludes")}</p>
                <div className="health-export-actions">
                  <Button
                    type="button"
                    onClick={() => controller.cancelDiagnosticExport()}
                  >
                    {text("common.cancel")}
                  </Button>
                  <Button
                    type="button"
                    appearance="primary"
                    onClick={() => void controller.exportDiagnostics()}
                  >
                    {text("health.exportChoose")}
                  </Button>
                </div>
              </section>
            )}
          </DialogContent>
          <DialogActions className="health-dialog-actions">
            <Button
              type="button"
              disabled={pending}
              onClick={() => controller.closeHealth()}
            >
              {text("common.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
