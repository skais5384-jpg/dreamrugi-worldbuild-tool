import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { GuardedClient } from "../bridge/client";
import type { DeletedBackupRow, ResultDto } from "../bridge/types";
import { text } from "../strings";
import { render } from "../test/render";
import WorkspaceApp from "./WorkspaceApp";
import { transportFixture } from "./WorkspaceApp.testHarness";
import { TemplateController } from "./controller";
import { WorkspaceController } from "./workspaceController";

const A = "d8d28b8e-8744-4eb6-95a5-34a3ade83771";
const B = "437c895a-e1c1-42ad-8868-faa4b9860dd8";
const warning = "backup_receipt_cleanup_required";

function row(
  id: string,
  status: "verification_required" | "uncertain" = "verification_required",
): DeletedBackupRow {
  return {
    backup: {
      id,
      label: status === "uncertain" ? "" : `AUDIT ${id}`,
      createdAtUtc: "2026-09-24T00:00:00Z",
      kind: "manual",
      size: "12",
      status: status === "uncertain" ? "corrupt" : "verification_required",
      coverage: "complete",
      unnamedOrdinal: null,
      locator: `audit-${id}`,
    },
    deletedAtUtc: "2026-09-24T00:00:00Z",
    expiresAtUtc: "2026-10-01T00:00:00Z",
    operation:
      id === B
        ? "f2f27267-9a4c-4b44-804b-809a3973480c"
        : "08f55111-5c62-44f9-bf61-079770dc47a4",
    status,
  };
}

function result(
  action: "backup_deleted_restore" | "backup_deleted_purge",
  outcome: string,
  message = warning,
): ResultDto {
  return {
    kind: "project_data" as const,
    action,
    root: null,
    backup: null,
    safety: null,
    backups: [],
    nextCursor: null,
    recoveryRequired: false,
    outcome,
    warning: message,
  };
}

beforeEach(() => {
  window.localStorage.clear();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.spyOn(window, "scrollBy").mockImplementation(() => {});
});

async function setup() {
  const fixture = transportFixture();
  const shell = new TemplateController(
    new GuardedClient(fixture.transport),
    vi
      .fn()
      .mockResolvedValueOnce("C:\\audit-project")
      .mockResolvedValue("C:\\audit-backups"),
  );
  render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
  const open = await screen.findByRole("button", {
    name: text("app.message07"),
  });
  await waitFor(() => expect(open).toBeEnabled());
  fireEvent.click(open);
  await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
  await act(async () => {
    await shell.showBackupCenter();
    await shell.chooseBackupStorage();
  });
  fixture.transport.deletedBackups.push(row(A), row(B));
  await act(async () => await shell.showDeletedBackups());
  return { fixture, shell };
}

it.each([".purging", ".worldbuild-backup"])(
  "actual %s orphan row keeps its receipt warning until a complete scan proves absence",
  async (suffix) => {
    const { fixture, shell } = await setup();
    fixture.transport.workspaceResult = (input) => {
      if (input.kind !== "backup_deleted_purge") return undefined;
      fixture.transport.deletedBackups.splice(
        0,
        2,
        row(`${A}${suffix}`, "uncertain"),
      );
      return result(input.kind, "deleted_receipt_cleanup_required");
    };
    act(() => {
      shell.selectDeletedBackup(A);
      shell.requestProjectDataConfirm("purge_deleted");
    });
    await act(async () => await shell.purgeSelectedDeletedBackup());
    expect(shell.snapshot().projectData?.deletedCleanupFacts).toHaveLength(1);
    expect(shell.snapshot().projectData?.deletedCleanupWarning).toBe(warning);
    expect(
      screen.getByText(text("backup.deletedCleanupRequired")),
    ).toBeVisible();
    // Similar prefixes and unknown suffixes never become the original target.
    fixture.transport.deletedBackups.splice(
      0,
      1,
      row(`${B}${suffix}`, "uncertain"),
      row(`${A}.unrelated`, "uncertain"),
    );
    await act(async () => await shell.refreshDeletedBackups());
    expect(shell.snapshot().projectData?.deletedCleanupFacts).toHaveLength(0);
    expect(screen.queryByRole("alert")).toBeNull();
  },
);

it.each([
  [A, B],
  [B, A],
] as const)(
  "sequential restore %s then %s preserves and independently resolves both facts",
  async (first, second) => {
    const { fixture, shell } = await setup();
    const remaining = new Set([A, B]);
    fixture.transport.workspaceResult = (input) => {
      if (input.kind !== "backup_deleted_restore") return undefined;
      remaining.delete(input.id);
      fixture.transport.deletedBackups.splice(
        0,
        2,
        ...[A, B].filter((id) => remaining.has(id)).map((id) => row(id)),
        ...[A, B]
          .filter((id) => !remaining.has(id))
          .map((id) => row(id, "uncertain")),
      );
      return result(input.kind, "restored_receipt_cleanup_required");
    };
    act(() => shell.selectDeletedBackup(first));
    await act(async () => await shell.restoreSelectedDeletedBackup());
    expect(
      shell.snapshot().projectData?.deletedCleanupFacts?.map((fact) => fact.id),
    ).toEqual([first]);
    fireEvent.click(
      screen.getByRole("button", { name: new RegExp(`AUDIT ${second}`) }),
    );
    expect(
      screen.getByRole("button", { name: text("backup.deletedRestore") }),
    ).toBeEnabled();
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedRestore") }),
    );
    await waitFor(() => expect(shell.snapshot().busy).toBe(false));
    expect(
      fixture.transport.commands.filter(
        (item) =>
          item.action === "submit" &&
          item.input.kind === "backup_deleted_restore",
      ),
    ).toHaveLength(2);
    expect(
      shell.snapshot().projectData?.deletedCleanupFacts?.map((fact) => fact.id),
    ).toEqual([first, second]);
    expect(screen.getByRole("alert")).toHaveTextContent(first);
    expect(screen.getByRole("alert")).toHaveTextContent(second);
    fixture.transport.omitDeletedBackupRows = true;
    await act(async () => await shell.refreshDeletedBackups());
    expect(shell.snapshot().projectData?.errorSource).toBe("deleted_list");
    expect(shell.snapshot().projectData?.deletedCleanupFacts).toHaveLength(2);
    expect(screen.getAllByRole("alert")).toHaveLength(2);
    fixture.transport.omitDeletedBackupRows = false;
    fixture.transport.deletedBackups.splice(0, 2, row(first, "uncertain"));
    await act(async () => await shell.refreshDeletedBackups());
    expect(
      shell.snapshot().projectData?.deletedCleanupFacts?.map((fact) => fact.id),
    ).toEqual([first]);
    expect(screen.getByRole("alert")).toHaveTextContent(first);
    fixture.transport.deletedBackups.splice(0);
    await act(async () => await shell.refreshDeletedBackups());
    expect(shell.snapshot().projectData?.deletedCleanupFacts).toHaveLength(0);
    expect(screen.queryByRole("alert")).toBeNull();
  },
);
