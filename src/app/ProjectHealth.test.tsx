import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import type { DocumentList } from "../bridge/documents";
import type { TemplateController } from "./controller";
import { text } from "../strings";
import { ProjectHealth } from "./ProjectHealth";

function list(
  problem: string | null,
  issueStatus: DocumentList["issueStatus"],
): DocumentList {
  return {
    kind: "list",
    fingerprint: "synthetic",
    snapshot: "one",
    layout: { revision: 1, rootOrder: [], nodes: {} },
    initial: false,
    unplaced: [],
    documents: [],
    issueStatus,
    unverifiedDocuments: [],
    problem,
  };
}

function health(
  documentList: DocumentList | null,
  documentCheckFailed = false,
  currentAttempt = true,
) {
  const closeHealth = vi.fn();
  const check = vi.fn();
  const controller = {
    snapshot: () => ({
      busy: false,
      projectId: "synthetic",
      inspectionPendingDocuments: [],
      health: {
        surface: "health",
        check: currentAttempt
          ? {
              id: 1,
              documents: documentCheckFailed ? "unverified" : "verified",
            }
          : undefined,
        phase: "ready",
        inspection: {
          complete: true,
          rows: [],
          deletedTemplates: [],
          documentIssues: [],
          scannedFiles: 0,
          missingAssets: 0,
          corruptAssets: 0,
          uncertainAssets: 0,
        },
        error: null,
        message: null,
        exportConfirm: false,
      },
      app: { support_diagnostics: [] },
    }),
    closeHealth,
    inspectAssets: check,
  } as unknown as TemplateController;
  const view = render(
    <ProjectHealth
      controller={controller}
      documentList={documentList}
      documents={[]}
      openResources={vi.fn()}
      openTrash={vi.fn()}
    />,
  );
  return { closeHealth, check, unmount: view.unmount };
}

describe("project health includes document validation", () => {
  it("shows clean only after both resource and document checks complete", () => {
    health(list(null, "complete"));
    expect(screen.getByText(text("health.badge.complete"))).toBeVisible();
    expect(screen.getByText(text("health.noProblems"))).toBeVisible();
  });

  it("never paints a malformed or unverified document list as clean", () => {
    const view = health(list("document_invalid", "unavailable"));
    expect(screen.getByText(text("health.documentListFailed"))).toBeVisible();
    expect(screen.queryByText(text("health.noProblems"))).toBeNull();
    view.unmount();
    health(list(null, "partial"));
    expect(
      screen.getByText(text("health.documentListUnverified")),
    ).toBeVisible();
    expect(screen.queryByText(text("health.noProblems"))).toBeNull();
  });

  it("does not certify an earlier complete list without a current attempt", () => {
    health(list(null, "complete"), false, false);
    expect(screen.queryByText(text("health.noProblems"))).toBeNull();
    expect(
      screen.getByText(text("health.documentListUnverified")),
    ).toBeVisible();
  });

  it("retries on explicit check and closes with Escape", () => {
    const { check, closeHealth } = health(null, true);
    fireEvent.click(screen.getByRole("button", { name: text("health.check") }));
    expect(check).toHaveBeenCalledOnce();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(closeHealth).toHaveBeenCalledOnce();
  });
});
