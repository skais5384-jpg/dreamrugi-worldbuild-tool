import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render } from "../test/render";
import WorkspaceApp from "./WorkspaceApp";
import { WorkspaceController } from "./workspaceController";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import type { BackupRow, DeletedBackupRow } from "../bridge/types";
import { text } from "../strings";
import { setup, transportFixture } from "./WorkspaceApp.testHarness";

beforeEach(() => {
  // jsdom에는 스크롤 배치가 없다. 이동 요청만 검사하며 실제 가시성은 native에서 확인한다.
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.spyOn(window, "scrollBy").mockImplementation(() => {});
  window.localStorage.clear();
});

describe("whole template workspace", () => {
  it("두 백업 중 선택한 행의 locator만 새 프로젝트 복원 명령에 전달한다", async () => {
    const fixture = transportFixture();
    const locatorA = "C:\\backups\\first.worldbuild-backup";
    const locatorB = "C:\\backups\\second.worldbuild-backup";
    fixture.transport.backups.push(
      {
        id: "first-backup",
        label: "첫 백업",
        createdAtUtc: "2026-09-20T00:00:00.000Z",
        kind: "manual",
        size: "12",
        status: "verification_required",
        coverage: "complete",
        unnamedOrdinal: null,
        locator: locatorA,
      },
      {
        id: "second-backup",
        label: "둘째 백업",
        createdAtUtc: "2026-09-21T00:00:00.000Z",
        kind: "manual",
        size: "12",
        status: "verification_required",
        coverage: "complete",
        unnamedOrdinal: null,
        locator: locatorB,
      },
    );
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups")
      .mockResolvedValueOnce("C:\\restored");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "둘째 백업" }));
    await waitFor(() =>
      expect(shell.snapshot().projectData?.selected).toBe("second-backup"),
    );
    expect(shell.snapshot().projectData?.locator).toBe(locatorB);
    expect(screen.getByRole("button", { name: "둘째 백업" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.restoreNew") }),
    );
    await act(async () => shell.confirmProjectDataName());
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "restore_new" &&
          command.input.locator === locatorB,
      ),
    ).toBe(true);
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "restore_new" &&
          command.input.locator === locatorA,
      ),
    ).toBe(false);
  });

  it("백업 센터는 메뉴 진입만으로 만들지 않고 위치·지역 시각·미지정 순번과 삭제 뒤 목록 복귀를 표시한다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    const projectMenu = await screen.findByRole("button", { name: "project" });
    fireEvent.click(projectMenu);
    expect(
      screen.queryByRole("menuitem", { name: text("backup.create") }),
    ).toBeNull();
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );

    expect(
      await screen.findByText(text("backup.locationMissing")),
    ).toBeVisible();
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" && command.input.kind === "backup_create",
      ),
    ).toBe(false);

    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");
    expect(
      screen.getByRole("button", { name: text("backup.activeEntry") }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "backup_deleted_list",
      ),
    ).toBe(false);
    fireEvent.change(screen.getByLabelText(text("backup.label")), {
      target: { value: "첫 백업" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.create") }),
    );

    const row = await screen.findByRole("button", { name: /첫 백업/u });
    const completion = screen.getByText(text("backup.created"));
    vi.useFakeTimers();
    fireEvent.mouseEnter(completion.closest(".feedback-toast")!);
    act(() => vi.advanceTimersByTime(5_000));
    expect(screen.getByText(text("backup.created"))).toBeVisible();
    fireEvent.mouseLeave(completion.closest(".feedback-toast")!);
    act(() => vi.advanceTimersByTime(4_000));
    expect(screen.queryByText(text("backup.created"))).toBeNull();
    vi.useRealTimers();
    fireEvent.click(row);
    expect(row).toHaveAttribute("aria-pressed", "true");
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "backup_create" &&
          command.input.storage === "C:\\backups",
      ),
    ).toBe(true);

    const listCount = fixture.transport.commands.filter(
      (command) =>
        command.action === "submit" && command.input.kind === "backup_list",
    ).length;
    fixture.transport.hold = "backup_list";
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.create") }),
    );
    await waitFor(() =>
      expect(
        fixture.transport.commands.filter(
          (command) =>
            command.action === "submit" && command.input.kind === "backup_list",
        ).length,
      ).toBeGreaterThan(listCount),
    );
    expect(
      screen.getByRole("button", { name: text("app.message03") }),
    ).toBeEnabled();
    vi.useFakeTimers();
    act(() => vi.advanceTimersByTime(5_000));
    expect(screen.queryByText(text("backup.created"))).toBeNull();
    vi.useRealTimers();
    fixture.transport.hold = null;
    act(() => fixture.transport.completeHeld());
    const unnamed = await screen.findByRole("button", {
      name: new RegExp(text("backup.unnamed", { number: "1" }), "u"),
    });
    expect(screen.getByText(text("backup.created"))).toBeVisible();
    expect(document.body.textContent).not.toContain("2026-09-21T00:00:00.000Z");
    expect(document.body.textContent).toContain(
      new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "medium",
      }).format(new Date("2026-09-21T00:00:00.000Z")),
    );

    fireEvent.click(unnamed);
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.delete") }),
    );
    expect(await screen.findByText(text("backup.deleteTitle"))).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.delete") }),
    );
    await waitFor(() =>
      expect(screen.queryByText(text("backup.deleteTitle"))).toBeNull(),
    );
    expect(
      screen.getByRole("heading", { name: text("backup.listTitle") }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: new RegExp(text("backup.unnamed", { number: "1" }), "u"),
      }),
    ).toBeNull();

    vi.useFakeTimers();
    act(() => vi.advanceTimersByTime(60_000));
    expect(
      screen.getByRole("button", { name: text("backup.undo") }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: text("backup.undo") }),
    ).toContainHTML("<svg");
    expect(
      screen.getByRole("button", { name: text("backup.undo") }),
    ).toHaveTextContent("");
    vi.useRealTimers();

    fireEvent.click(screen.getByRole("button", { name: text("backup.undo") }));
    expect(
      await screen.findByText(text("backup.deletedRestored")),
    ).toBeVisible();
    expect(
      await screen.findByRole("button", { name: /이름 미지정 1/u }),
    ).toBeVisible();
    vi.useFakeTimers();
    fireEvent.blur(document.activeElement!);
    const restoredToast = screen
      .getByText(text("backup.deletedRestored"))
      .closest(".feedback-toast")!;
    fireEvent.mouseEnter(restoredToast);
    fireEvent.mouseLeave(restoredToast);
    act(() => vi.advanceTimersByTime(4_000));
    expect(screen.queryByText(text("backup.deletedRestored"))).toBeNull();
    vi.useRealTimers();

    fixture.transport.backupDeleteOutcome = "quarantined_unverified";
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.create") }),
    );
    const partial = await screen.findByRole("button", {
      name: new RegExp(text("backup.unnamed", { number: "1" }), "u"),
    });
    fireEvent.click(partial);
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.delete") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.delete") }),
    );
    await waitFor(() =>
      expect(screen.getByText(text("backup.deleteUncertain"))).toBeVisible(),
    );
    expect(screen.queryByText(text("backup.deleteTitle"))).toBeNull();
    expect(
      screen.getByRole("heading", { name: text("backup.listTitle") }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: text("backup.undo") }),
    ).toBeNull();
    fireEvent.change(screen.getByLabelText(text("backup.label")), {
      target: { value: "다음 백업" },
    });
    expect(screen.getByText(text("backup.deleteUncertain"))).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    expect(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(text("backup.deleteUncertain"))).toBeVisible();
    fireEvent.click(
      within(screen.getByRole("alert")).getByRole("button", {
        name: text("update.details"),
      }),
    );
    fireEvent.click(screen.getByText(text("error.details")));
    expect(
      screen.getByText("backup_quarantine_readback_required"),
    ).toBeVisible();
  }, 15_000);

  it("백업 삭제 알림을 닫아도 보관본은 목록에서 다시 복구할 수 있다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.create") }),
    );
    const backup = await screen.findByRole("button", {
      name: new RegExp(text("backup.unnamed", { number: "1" }), "u"),
    });
    fireEvent.click(backup);
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.delete") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.delete") }),
    );
    await screen.findByRole("button", { name: text("backup.undo") });
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.dismiss") }),
    );
    expect(
      screen.queryByRole("button", { name: text("backup.undo") }),
    ).toBeNull();
    expect(fixture.transport.deletedBackups).toHaveLength(1);
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    const deleted = await screen.findByRole("button", {
      name: new RegExp(text("backup.unnamedShort"), "u"),
    });
    fireEvent.click(deleted);
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedRestore") }),
    );
    await waitFor(() =>
      expect(fixture.transport.deletedBackups).toHaveLength(0),
    );
    expect(fixture.transport.backups).toHaveLength(1);
  }, 15_000);

  it.each(["2026-09-28T12:00:00.000Z", "2036-09-28T12:00:00.000Z"])(
    "%s 삭제 fixture는 7일 동안 undo를 제공하고 만료 시 복구 요청을 보내지 않는다",
    async (now) => {
      const clock = vi.spyOn(Date, "now").mockReturnValue(Date.parse(now));
      const fixture = transportFixture();
      const shell = new TemplateController(
        new GuardedClient(fixture.transport),
        vi
          .fn()
          .mockResolvedValueOnce("C:\\project")
          .mockResolvedValueOnce("C:\\backups"),
      );
      render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
      const open = await screen.findByRole("button", {
        name: text("app.message07"),
      });
      await waitFor(() => expect(open).toBeEnabled());
      fireEvent.click(open);
      await waitFor(() =>
        expect(shell.snapshot().projectId).toBe("project-one"),
      );
      await act(async () => {
        await shell.showBackupCenter();
        await shell.chooseBackupStorage();
        await shell.createBackup();
        await shell.selectBackup(fixture.transport.backups[0].id);
        shell.requestProjectDataConfirm("delete");
        await shell.deleteSelectedBackup();
      });
      const deleted = fixture.transport.deletedBackups[0];
      expect(deleted.deletedAtUtc).toBe(now);
      expect(Date.parse(deleted.expiresAtUtc) - Date.parse(now)).toBe(
        7 * 24 * 60 * 60 * 1000,
      );
      expect(
        screen.getByRole("button", { name: text("backup.undo") }),
      ).toBeVisible();

      clock.mockReturnValue(Date.parse(deleted.expiresAtUtc));
      await act(async () => await shell.undoBackupDeletion());
      expect(
        screen.queryByRole("button", { name: text("backup.undo") }),
      ).toBeNull();
      expect(fixture.transport.deletedBackups).toHaveLength(1);
      expect(
        fixture.transport.commands.some(
          (command) =>
            command.action === "submit" &&
            command.input.kind === "backup_deleted_restore",
        ),
      ).toBe(false);
    },
  );

  it("삭제 목록 응답의 필드가 없으면 빈 목록으로 확정하지 않는다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");
    fixture.transport.omitDeletedBackupRows = true;
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    expect(await screen.findByRole("alert")).toBeVisible();
    expect(screen.queryByText(text("backup.deletedEmpty"))).toBeNull();
  });

  it("삭제 목록의 실패는 같은 소유의 정상 재조회로 해소하고 빈 목록·행을 표시한다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");
    fixture.transport.omitDeletedBackupRows = true;
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    await waitFor(() =>
      expect(shell.snapshot().projectData?.errorSource).toBe("deleted_list"),
    );
    fixture.transport.omitDeletedBackupRows = false;
    await act(async () => {
      await shell.refreshDeletedBackups();
    });
    expect(shell.snapshot().projectData?.error).toBeNull();
    expect(shell.snapshot().projectData?.detail).toBeNull();
    expect(screen.getByText(text("backup.deletedEmpty"))).toBeVisible();

    fixture.transport.deletedBackups.push({
      backup: {
        id: "test-deleted",
        label: "복구 가능한 백업",
        createdAtUtc: "2026-09-21T00:00:00.000Z",
        kind: "manual",
        size: "12",
        status: "verification_required",
        coverage: "complete",
        unnamedOrdinal: null,
        locator: "test-locator",
      },
      deletedAtUtc: "2026-09-22T00:00:00.000Z",
      expiresAtUtc: "2026-09-29T00:00:00.000Z",
      operation: "test-operation",
      status: "verification_required",
    });
    await act(async () => {
      await shell.refreshDeletedBackups();
    });
    expect(
      screen.getByRole("button", { name: /복구 가능한 백업/u }),
    ).toBeVisible();
    expect(screen.queryByText(text("backup.deletedEmpty"))).toBeNull();
  });

  it("삭제 목록 cleanup 경고를 정상 재조회와 함께 유지한다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");
    fixture.transport.deletedListWarning = "cleanup-required";
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    await waitFor(() =>
      expect(shell.snapshot().projectData?.errorSource).toBe("cleanup"),
    );
    fixture.transport.deletedListWarning = null;
    await act(async () => {
      await shell.refreshDeletedBackups();
    });
    expect(
      screen.getByText(text("backup.deletedCleanupRequired")),
    ).toBeVisible();
    expect(shell.snapshot().projectData?.detail).toBe("cleanup-required");
  });

  it.each(["malformed", "rejected"] as const)(
    "복구의 receipt 정리 경고와 %s 목록 실패를 함께 표시하고 실제 정리 후 해소한다",
    async (fault) => {
      const fixture = transportFixture();
      const shell = new TemplateController(
        new GuardedClient(fixture.transport),
        vi
          .fn()
          .mockResolvedValueOnce("C:\\project")
          .mockResolvedValueOnce("C:\\backups"),
      );
      render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
      fireEvent.click(
        await screen.findByRole("button", { name: text("app.message07") }),
      );
      await waitFor(() =>
        expect(shell.snapshot().projectId).toBe("project-one"),
      );
      await act(async () => {
        await shell.showBackupCenter();
        await shell.chooseBackupStorage();
      });
      const row: DeletedBackupRow = {
        backup: {
          id: "receipt-id",
          label: "정리 확인 백업",
          createdAtUtc: "2026-09-24T00:00:00Z",
          kind: "manual",
          size: "12",
          status: "verification_required",
          coverage: "complete",
          unnamedOrdinal: null,
          locator: "receipt-locator",
        },
        deletedAtUtc: "2026-09-24T00:00:00Z",
        expiresAtUtc: "2026-10-01T00:00:00Z",
        operation: "receipt-operation",
        status: "verification_required",
      };
      fixture.transport.deletedBackups.push(row);
      await act(async () => await shell.showDeletedBackups());
      const base = fixture.transport.workspaceResult;
      let failList = true;
      fixture.transport.workspaceResult = (input) =>
        input.kind === "backup_deleted_restore"
          ? {
              kind: "project_data",
              action: "backup_deleted_restore",
              root: null,
              backup: null,
              safety: null,
              backups: [],
              nextCursor: null,
              recoveryRequired: false,
              outcome: "restored_receipt_cleanup_required",
              warning: "backup_receipt_cleanup_required",
            }
          : input.kind === "backup_deleted_list" &&
              fault === "rejected" &&
              failList
            ? {
                kind: "rejected",
                error: { code: "repository_rejected", nextAction: "" },
                input_retained: false,
              }
            : base?.(input);
      shell.selectDeletedBackup(row.backup.id);
      if (fault === "malformed") fixture.transport.omitDeletedBackupRows = true;
      await act(async () => await shell.restoreSelectedDeletedBackup());

      expect(shell.snapshot().projectData?.errorSource).toBe("deleted_list");
      expect(shell.snapshot().projectData?.deletedCleanupWarning).toBe(
        "backup_receipt_cleanup_required",
      );
      expect(shell.snapshot().projectData?.deletedCleanupBackupId).toBe(
        row.backup.id,
      );
      expect(screen.getAllByRole("alert")).toHaveLength(2);
      const cleanupNotice = screen
        .getByText(text("backup.deletedCleanupRequired"))
        .closest(".floating-notice")!;
      expect(cleanupNotice).toHaveAttribute("role", "alert");
      fireEvent.click(
        within(cleanupNotice as HTMLElement).getByRole("button", {
          name: text("update.details"),
        }),
      );
      const record = await screen.findByRole("dialog", { name: "실행 기록" });
      const cleanupRow = within(record)
        .getAllByText(text("backup.deletedCleanupRequired"), {
          selector: "summary",
        })[0]
        .closest("td")!;
      fireEvent.click(within(cleanupRow).getByText(text("error.details")));
      expect(
        within(record)
          .getByText("backup_receipt_cleanup_required")
          .closest("table"),
      ).not.toBeNull();
      fireEvent.click(within(record).getByRole("button", { name: "닫기" }));
      await waitFor(() =>
        expect(screen.queryByRole("dialog", { name: "실행 기록" })).toBeNull(),
      );
      expect(
        await screen.findByRole("button", {
          name: text("backup.deletedRetry"),
        }),
      ).toBeVisible();
      expect(screen.queryByText(text("backup.deletedEmpty"))).toBeNull();

      fixture.transport.omitDeletedBackupRows = false;
      failList = false;
      fixture.transport.deletedBackups[0] = {
        ...row,
        status: "uncertain",
        backup: { ...row.backup, status: "corrupt" },
      };
      await act(async () => await shell.refreshDeletedBackups());
      expect(shell.snapshot().projectData?.errorSource).toBe("cleanup");
      expect(shell.snapshot().projectData?.deletedCleanupWarning).toBe(
        "backup_receipt_cleanup_required",
      );
      await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(1));

      fixture.transport.deletedBackups.splice(0);
      await act(async () => await shell.refreshDeletedBackups());
      expect(shell.snapshot().projectData?.deletedCleanupWarning).toBeNull();
      expect(shell.snapshot().projectData?.error).toBeNull();
      await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
      expect(screen.getByText(text("backup.deletedEmpty"))).toBeVisible();
    },
  );

  it("영구 삭제의 receipt 정리 경고도 자동 목록 실패와 독립적으로 남는다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi
        .fn()
        .mockResolvedValueOnce("C:\\project")
        .mockResolvedValueOnce("C:\\backups"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    await act(async () => {
      await shell.showBackupCenter();
      await shell.chooseBackupStorage();
    });
    const row: DeletedBackupRow = {
      backup: {
        id: "purge-receipt",
        label: "영구 삭제 백업",
        createdAtUtc: "2026-09-24T00:00:00Z",
        kind: "manual",
        size: "12",
        status: "verification_required",
        coverage: "complete",
        unnamedOrdinal: null,
        locator: "purge-locator",
      },
      deletedAtUtc: "2026-09-24T00:00:00Z",
      expiresAtUtc: "2026-10-01T00:00:00Z",
      operation: "purge-operation",
      status: "verification_required",
    };
    fixture.transport.deletedBackups.push(row);
    await act(async () => await shell.showDeletedBackups());
    fixture.transport.workspaceResult = (input) =>
      input.kind === "backup_deleted_purge"
        ? {
            kind: "project_data",
            action: "backup_deleted_purge",
            root: null,
            backup: null,
            safety: null,
            backups: [],
            nextCursor: null,
            recoveryRequired: false,
            outcome: "deleted_receipt_cleanup_required",
            warning: "backup_receipt_cleanup_required",
          }
        : undefined;
    shell.selectDeletedBackup(row.backup.id);
    shell.requestProjectDataConfirm("purge_deleted");
    fixture.transport.omitDeletedBackupRows = true;
    await act(async () => await shell.purgeSelectedDeletedBackup());
    expect(shell.snapshot().projectData?.errorSource).toBe("deleted_list");
    expect(shell.snapshot().projectData?.deletedCleanupWarning).toBe(
      "backup_receipt_cleanup_required",
    );
    expect(screen.getAllByRole("alert")).toHaveLength(2);
    expect(
      screen.getByRole("button", { name: text("backup.deletedRetry") }),
    ).toBeVisible();
  });

  it("더 최신 삭제 목록 요청이 있으면 이전 응답을 현재 화면에 반영하지 않는다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );
    await screen.findByText("C:\\backups");

    fixture.transport.hold = "backup_deleted_list";
    fireEvent.click(
      screen.getByRole("button", { name: text("backup.deletedEntry") }),
    );
    await waitFor(() =>
      expect(
        fixture.transport.commands.some(
          (command) =>
            command.action === "submit" &&
            command.input.kind === "backup_deleted_list",
        ),
      ).toBe(true),
    );
    await shell.refreshDeletedBackups();
    fixture.transport.hold = null;
    act(() => fixture.transport.completeHeld());
    await waitFor(() => expect(shell.snapshot().busy).toBe(false));
    expect(shell.snapshot().projectData?.deletedBackups).toEqual([]);
    expect(shell.snapshot().projectData?.error).toBeNull();

    await act(async () => {
      await shell.refreshDeletedBackups();
    });
    expect(screen.getByText(text("backup.deletedEmpty"))).toBeVisible();
  });

  it("저장되지 않은 편집 세션이 있어도 저장본 기준 사본·백업 메뉴를 연다", async () => {
    await setup();

    fireEvent.click(screen.getByRole("button", { name: "fixture" }));
    const copy = await screen.findByRole("menuitem", {
      name: text("backup.copy"),
    });
    const backups = await screen.findByRole("menuitem", {
      name: text("backup.manage"),
    });

    expect(copy).toBeEnabled();
    expect(backups).toBeEnabled();
    fireEvent.click(backups);
    expect(
      await screen.findByRole("heading", { name: text("backup.listTitle") }),
    ).toBeVisible();
    expect(screen.getByText(text("backup.help"))).toBeVisible();
  });

  it("협업 사본 완료 후 등록 진입을 같은 대화상자에 유지한다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\copies");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.copy") }),
    );
    fireEvent.change(screen.getByLabelText(text("project.name")), {
      target: { value: "collaborative-copy" },
    });
    fireEvent.click(
      screen.getByRole("radio", { name: text("svn.collaborativeCopy") }),
    );
    fireEvent.click(screen.getByRole("button", { name: text("backup.copy") }));
    expect(
      await screen.findByRole("heading", { name: text("svn.registerTitle") }),
    ).toBeVisible();
    expect(shell.snapshot().projectData).toBeNull();
    expect(
      screen.queryByLabelText(text("project.name")),
    ).not.toBeInTheDocument();
  });

  it("리소스와 통합 휴지통은 선택·이동·복원을 같은 탐색 흐름에서 수행한다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));

    fireEvent.click(
      screen.getByRole("button", { name: text("resources.title") }),
    );
    const unused = await screen.findByText("unused.bin");
    const resourceList = screen.getByRole("region", {
      name: text("resources.list"),
    });
    expect(resourceList).toContainElement(unused);
    expect(
      screen.getByRole("complementary", { name: text("resources.title") }),
    ).not.toContainElement(unused);
    fireEvent.click(unused.closest("button")!);
    fireEvent.contextMenu(unused.closest(".project-file-menu-row")!);
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: text("documents.toTrash"),
      }),
    );
    expect(await screen.findByText(text("health.moveComplete"))).toBeVisible();

    fireEvent.click(
      screen.getByRole("button", { name: text("documents.trash") }),
    );
    const trashed = await screen.findByText("unused.bin");
    const trashList = screen.getByRole("region", {
      name: text("trash.list"),
    });
    expect(trashList).toContainElement(trashed);
    fireEvent.click(trashed.closest("button")!);
    fireEvent.click(
      screen.getByRole("button", {
        name: `unused.bin · ${text("documents.menu")}`,
      }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("health.restore") }),
    );
    expect(
      await screen.findByText(text("health.restoreComplete")),
    ).toBeVisible();
    await waitFor(() =>
      expect(
        trashList.querySelector(".content-empty-state img"),
      ).toHaveAttribute("width", "125"),
    );
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "asset_trash_restore",
      ),
    ).toBe(true);
  });

  it("통합 휴지통 비우기는 문서·Template·리소스의 실제 완료와 보호 수를 분리한다", async () => {
    const fixture = transportFixture();
    const documentId = "66666666-6666-4666-8666-666666666666";
    const assetId = "77777777-7777-4777-8777-777777777777";
    const protectedAssetId = "88888888-8888-4888-8888-888888888888";
    const templateId = "99999999-9999-4999-8999-999999999999";
    const protectedTemplateId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    fixture.transport.assetInspection = {
      ...fixture.transport.assetInspection,
      rows: [],
      usedAssets: 0,
      unusedAssets: 0,
      trash: [
        {
          id: assetId,
          name: "삭제할 리소스.txt",
          size: 10,
          reclaimableSize: 10,
          movedAtUtc: "2026-09-21T00:00:00.000Z",
          protected: false,
          reason: null,
        },
        {
          id: protectedAssetId,
          name: "보호 리소스.txt",
          size: 20,
          reclaimableSize: 0,
          movedAtUtc: "2026-09-21T00:00:00.000Z",
          protected: true,
          reason: "stored_reference",
        },
      ],
      deletedTemplates: [
        {
          id: templateId,
          name: "삭제할 Template",
          size: 30,
          removable: true,
          reason: null,
        },
        {
          id: protectedTemplateId,
          name: "보호 Template",
          size: 40,
          removable: false,
          reason: "stored_reference",
        },
      ],
    };
    let purged = false;
    fixture.transport.workspaceResult = (
      input,
    ): import("../bridge/types").ResultDto | undefined => {
      if (input.kind !== "document_workspace") return undefined;
      if (input.request.action === "mutate") {
        expect(input.request.edit).toEqual({
          kind: "purge",
          document: documentId,
        });
        purged = true;
        return {
          kind: "write",
          session: "document-purge-session",
          artifact: documentId,
          disk: "committed",
          changed: true,
          warnings: [],
          cleanup_failed: false,
          recovery_required: false,
          error: null,
          diagnostic: {
            stage: "Commit",
            category: null,
            sessionState: "Released",
            lockCategory: null,
            nextAction: "",
          },
        };
      }
      if (input.request.action === "list") {
        const nodes: import("../bridge/documents").Layout["nodes"] = {};
        if (!purged)
          nodes[documentId] = {
            parentId: null,
            childOrder: [],
            state: "trashed",
            trash: {
              parentId: null,
              index: 0,
              trashedAtUtc: "2026-09-21T00:00:00.000Z",
            },
          };
        return {
          kind: "document_workspace",
          value: {
            kind: "list",
            fingerprint: "fixture",
            snapshot: purged ? "after-purge" : "before-purge",
            layout: { revision: purged ? 2 : 1, rootOrder: [], nodes },
            initial: true,
            unplaced: [],
            documents: purged
              ? []
              : [
                  {
                    id: documentId,
                    name: "삭제할 문서",
                    template: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
                  },
                ],
            problem: null,
          },
        };
      }
      return undefined;
    };
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.trash") }),
    );
    await screen.findByText("삭제할 문서");
    const badgeClasses = [
      "삭제할 문서",
      "삭제할 Template",
      "삭제할 리소스.txt",
    ].map(
      (name) =>
        screen.getByText(name).closest("li")!.querySelector(".fui-Badge")!
          .className,
    );
    expect(new Set(badgeClasses).size).toBe(3);
    const protectedTemplate = screen.getByText("보호 Template");
    expect(
      within(protectedTemplate.closest("li")!).getByText(
        text("health.protected"),
      ),
    ).toBeVisible();
    expect(
      within(screen.getByText("보호 리소스.txt").closest("li")!).getByText(
        text("health.protected"),
      ),
    ).toBeVisible();
    fireEvent.contextMenu(protectedTemplate.closest(".project-file-menu-row")!);
    expect(
      await screen.findByRole("menuitem", { name: text("trash.purge") }),
    ).toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: text("trash.empty") }));
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText(
        text("trash.purgeSummary", { count: "3", protected: "2" }),
      ),
    ).toBeVisible();
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("trash.purge") }),
    );
    // Three distinct native operations complete before the aggregate notice.
    // Wait for that observable completion, preserving the exact counts.
    await waitFor(
      () =>
        expect(
          screen.getByText(
            text("trash.batchPartial", {
              completed: "3",
              protected: "2",
              failed: "0",
              cleanup: "0",
            }),
          ),
        ).toBeVisible(),
      { timeout: 5_000 },
    );
    expect(purged).toBe(true);
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "asset_trash_purge" &&
          command.input.assets.includes(assetId),
      ),
    ).toBe(true);
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "template_purge" &&
          command.input.templates.includes(templateId),
      ),
    ).toBe(true);
    expect(fixture.transport.assetInspection.trash).toHaveLength(1);
    expect(fixture.transport.assetInspection.deletedTemplates).toHaveLength(1);
  });

  it("휴지통 비우기는 100개를 넘는 승인 집합을 새 검사 토큰으로 나눠 모두 처리한다", async () => {
    const fixture = transportFixture();
    fixture.transport.assetInspection = {
      ...fixture.transport.assetInspection,
      rows: [],
      usedAssets: 0,
      unusedAssets: 0,
      trash: Array.from({ length: 101 }, (_, index) => ({
        id: `${String(index).padStart(8, "0")}-1111-4111-8111-111111111111`,
        name: `대량 리소스 ${String(index).padStart(3, "0")}.txt`,
        size: 1,
        reclaimableSize: 1,
        movedAtUtc: "2026-09-21T00:00:00.000Z",
        protected: false,
        reason: null,
      })),
      deletedTemplates: [],
    };
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.trash") }),
    );
    await screen.findByText("대량 리소스 100.txt");
    fireEvent.click(screen.getByRole("button", { name: text("trash.empty") }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("trash.purge") }),
    );
    expect(
      await screen.findByText(text("trash.batchComplete", { count: "101" })),
    ).toBeVisible();
    const purges = fixture.transport.commands.filter(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "asset_trash_purge",
    );
    expect(purges).toHaveLength(2);
    expect(
      purges.map((command) =>
        command.action === "submit" &&
        command.input.kind === "asset_trash_purge"
          ? command.input.assets.length
          : 0,
      ),
    ).toEqual([100, 1]);
    expect(fixture.transport.assetInspection.trash).toHaveLength(0);
  });

  it("혼합 복원은 리소스·Template·부모 문서를 먼저 처리하고 사용할 수 없는 원 부모는 루트 끝으로 보낸다", async () => {
    const fixture = transportFixture();
    const assetId = "12121212-1212-4212-8212-121212121212";
    const templateId = "13131313-1313-4313-8313-131313131313";
    const parentId = "14141414-1414-4414-8414-141414141414";
    const childId = "15151515-1515-4515-8515-151515151515";
    const unselectedParentId = "16161616-1616-4616-8616-161616161616";
    const fallbackId = "17171717-1717-4717-8717-171717171717";
    fixture.transport.assetInspection = {
      ...fixture.transport.assetInspection,
      rows: [],
      usedAssets: 0,
      unusedAssets: 0,
      trash: [
        {
          id: assetId,
          name: "먼저 복원할 리소스.txt",
          size: 1,
          reclaimableSize: 1,
          movedAtUtc: "2026-09-21T00:00:00.000Z",
          protected: false,
          reason: null,
        },
      ],
      deletedTemplates: [
        {
          id: templateId,
          name: "먼저 복원할 Template",
          size: 1,
          removable: false,
          reason: "stored_reference",
        },
      ],
    };
    let revision = 1;
    const active = new Set<string>();
    const edits: import("../bridge/documents").LayoutEdit[] = [];
    const originalParents = new Map<string, string | null>([
      [parentId, null],
      [childId, parentId],
      [unselectedParentId, null],
      [fallbackId, unselectedParentId],
    ]);
    const makeList = (): import("../bridge/documents").DocumentList => {
      const nodes: import("../bridge/documents").Layout["nodes"] = {};
      for (const [id, originalParent] of originalParents) {
        const isActive = active.has(id);
        nodes[id] = {
          parentId: isActive
            ? id === fallbackId
              ? null
              : originalParent
            : null,
          childOrder: id === parentId && active.has(childId) ? [childId] : [],
          state: isActive ? "active" : "trashed",
          trash: isActive
            ? null
            : {
                parentId: originalParent,
                index: 0,
                trashedAtUtc: "2026-09-21T00:00:00.000Z",
              },
        };
      }
      return {
        kind: "list",
        fingerprint: "fixture",
        snapshot: `restore-${revision}`,
        layout: {
          revision,
          rootOrder: [parentId, fallbackId].filter((id) => active.has(id)),
          nodes,
        },
        initial: true,
        unplaced: [],
        documents: [
          { id: parentId, name: "부모 문서", template: templateId },
          { id: childId, name: "자식 문서", template: templateId },
          {
            id: unselectedParentId,
            name: "선택하지 않은 부모",
            template: "18181818-1818-4818-8818-181818181818",
          },
          {
            id: fallbackId,
            name: "루트 fallback 문서",
            template: "18181818-1818-4818-8818-181818181818",
          },
        ],
        problem: null,
      };
    };
    fixture.transport.workspaceResult = (input) => {
      if (input.kind !== "document_workspace") return undefined;
      if (input.request.action === "list")
        return { kind: "document_workspace", value: makeList() };
      if (input.request.action === "mutate") {
        edits.push(structuredClone(input.request.edit));
        if (input.request.edit.kind === "restore") {
          active.add(input.request.edit.document);
          revision += 1;
        }
        return {
          kind: "write",
          session: `restore-session-${revision}`,
          artifact:
            input.request.edit.kind === "restore"
              ? input.request.edit.document
              : null,
          disk: "committed",
          changed: true,
          warnings: [],
          cleanup_failed: false,
          recovery_required: false,
          error: null,
          diagnostic: {
            stage: "Commit",
            category: null,
            sessionState: "Released",
            lockCategory: null,
            nextAction: "",
          },
        };
      }
      return undefined;
    };
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.trash") }),
    );
    await screen.findByText("부모 문서");
    await waitFor(() => expect(shell.snapshot().busy).toBe(false));
    for (const key of [
      `resource:${assetId}`,
      `template:${templateId}`,
      `document:${parentId}`,
      `document:${childId}`,
      `document:${fallbackId}`,
    ])
      fireEvent.click(document.getElementById(`project-file-${key}`)!, {
        ctrlKey: true,
      });
    fireEvent.contextMenu(
      document
        .getElementById(`project-file-document:${fallbackId}`)!
        .closest(".project-file-menu-row")!,
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("health.restore") }),
    );
    expect(
      await screen.findByText(
        text("trash.restoreComplete"),
        {},
        { timeout: 5_000 },
      ),
    ).toBeVisible();
    expect(edits).toEqual([
      { kind: "restore", document: parentId, destination: null },
      {
        kind: "restore",
        document: fallbackId,
        destination: { parent: null, index: 1 },
      },
      { kind: "restore", document: childId, destination: null },
    ]);
    const submitted = fixture.transport.commands.filter(
      (command) => command.action === "submit",
    );
    const assetAt = submitted.findIndex(
      (command) => command.input.kind === "asset_trash_restore",
    );
    const templateAt = submitted.findIndex(
      (command) => command.input.kind === "template_restore",
    );
    const documentAt = submitted.findIndex(
      (command) =>
        command.input.kind === "document_workspace" &&
        command.input.request.action === "mutate",
    );
    expect(assetAt).toBeGreaterThanOrEqual(0);
    expect(templateAt).toBeGreaterThan(assetAt);
    expect(documentAt).toBeGreaterThan(templateAt);
    expect(active.has(unselectedParentId)).toBe(false);
  });

  it("Template 목록은 Ctrl/Shift 다중 선택을 열기와 분리하고 선택 항목을 순서대로 휴지통에 보낸다", async () => {
    const fixture = transportFixture();
    const templates = [
      {
        ...structuredClone(fixture.base),
        id: "template-one",
        name: "첫 템플릿",
      },
      {
        ...structuredClone(fixture.base),
        id: "template-two",
        name: "둘 템플릿",
      },
      {
        ...structuredClone(fixture.base),
        id: "template-three",
        name: "셋 템플릿",
      },
    ];
    for (const template of templates)
      fixture.transport.templates.set(template.id, template);
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    // Collaboration keeps template trash enabled; native checks the exact SVN target.
    shell.snapshot().project!.collaborative = true;
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.templates") }),
    );

    const first = await screen.findByRole("button", { name: "첫 템플릿" });
    const third = screen.getByRole("button", { name: "셋 템플릿" });
    const readsBefore = fixture.transport.commands.filter(
      (command) =>
        command.action === "submit" && command.input.kind === "read_template",
    ).length;
    fireEvent.click(first, { ctrlKey: true });
    fireEvent.click(third, { shiftKey: true });
    expect(
      screen.getByText(text("documents.selectedCount", { count: "3" })),
    ).toBeVisible();
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "read_template",
      ),
    ).toHaveLength(readsBefore);

    fireEvent.contextMenu(third);
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: text("documents.toTrash"),
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText(
        text("templates.batchTrashWarning", { count: "3" }),
      ),
    ).toBeVisible();
    fireEvent.click(
      within(dialog).getByRole("button", {
        name: text("documents.toTrash"),
      }),
    );
    await waitFor(() =>
      expect(
        fixture.transport.writes.filter(
          ({ input }) => input.kind === "tombstone_template",
        ),
      ).toHaveLength(3),
    );
    expect(
      templates.every(
        ({ id }) =>
          fixture.transport.templates.get(id)?.lifecycle === "Deleted",
      ),
    ).toBe(true);
  });

  it("Template 선택과 오래된 확인은 같은 ID의 새 프로젝트 세대로 넘어가지 않는다", async () => {
    const fixture = transportFixture();
    fixture.transport.templates.set(
      "template-one",
      structuredClone(fixture.base),
    );
    fixture.transport.templates.set("template-two", {
      ...structuredClone(fixture.base),
      id: "template-two",
      name: "두 번째",
    });
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\project-a"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.templates") }),
    );
    const first = await screen.findByRole("button", {
      name: fixture.base.name,
    });
    fireEvent.click(first, { ctrlKey: true });
    fireEvent.click(screen.getByRole("button", { name: "두 번째" }), {
      ctrlKey: true,
    });
    fireEvent.contextMenu(first);
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("documents.toTrash") }),
    );
    expect(screen.getByRole("dialog")).toBeVisible();

    const firstGeneration = shell.projectGeneration();
    fixture.transport.nextProjectId = "project-one";
    await act(async () => {
      await shell.navigate({ kind: "open_project", root: "C:\\project-b" });
    });
    await waitFor(() =>
      expect(shell.projectGeneration()).toBeGreaterThan(firstGeneration),
    );
    expect(shell.snapshot().projectId).toBe("project-one");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      screen.queryByText(text("documents.selectedCount", { count: "2" })),
    ).toBeNull();
    expect(
      fixture.transport.writes.some(
        ({ input }) => input.kind === "tombstone_template",
      ),
    ).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: fixture.base.name }), {
      ctrlKey: true,
    });
    fireEvent.click(screen.getByRole("button", { name: "두 번째" }), {
      ctrlKey: true,
    });
    expect(
      screen.getByText(text("documents.selectedCount", { count: "2" })),
    ).toBeVisible();
    fireEvent.contextMenu(screen.getByRole("button", { name: "두 번째" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("documents.toTrash") }),
    );
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: text("documents.toTrash"),
      }),
    );
    await waitFor(() =>
      expect(
        fixture.transport.writes.filter(
          ({ input }) => input.kind === "tombstone_template",
        ),
      ).toHaveLength(2),
    );
  });

  it("프로젝트 점검 조치는 문제의 정확한 리소스와 삭제된 Template 행을 선택해 이동한다", async () => {
    const fixture = transportFixture();
    const missingId = "33333333-3333-4333-8333-333333333333";
    const deletedTemplateId = "44444444-4444-4444-8444-444444444444";
    const documentId = "55555555-5555-4555-8555-555555555555";
    fixture.transport.assetInspection = {
      ...fixture.transport.assetInspection,
      complete: true,
      missingAssets: 1,
      rows: [
        ...fixture.transport.assetInspection.rows,
        {
          id: missingId,
          name: "missing-reference.bin",
          size: 0,
          status: "missing",
          reason: "stored_reference",
        },
      ],
      deletedTemplates: [
        {
          id: deletedTemplateId,
          name: "삭제된 지역 Template",
          size: 128,
          removable: false,
          reason: "stored_reference",
        },
      ],
      documentIssues: [
        {
          documentId,
          reasons: ["deleted_template"],
          relatedResourceIds: [],
          relatedTemplateIds: [deletedTemplateId],
        },
      ],
    };
    fixture.transport.workspaceResult = (input) =>
      input.kind === "document_workspace"
        ? {
            kind: "document_workspace",
            value: {
              kind: "list",
              fingerprint: "fixture",
              snapshot: "documents-with-problem",
              layout: { revision: 1, rootOrder: [], nodes: {} },
              initial: true,
              unplaced: [documentId],
              documents: [
                {
                  id: documentId,
                  name: "미등록 지역 문서",
                  template: deletedTemplateId,
                  templateName: "삭제된 지역 Template",
                  revision: 1,
                  updatedAtUtc: "2026-09-21T00:00:00.000Z",
                },
              ],
              problem: null,
            },
          }
        : undefined;
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValueOnce("C:\\project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));

    const openHealth = async () => {
      fireEvent.click(screen.getByRole("button", { name: "project" }));
      fireEvent.click(
        await screen.findByRole("menuitem", { name: text("health.menu") }),
      );
      await screen.findByText(
        text("health.documentsWithoutTemplate", { count: "1" }),
      );
    };
    await openHealth();
    expect(
      screen.queryByRole("button", { name: text("health.addDocuments") }),
    ).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: text("health.openResources") }),
    );
    await waitFor(() =>
      expect(
        document.getElementById(`project-file-resource:${missingId}`),
      ).not.toBeNull(),
    );
    const missing = document.getElementById(
      `project-file-resource:${missingId}`,
    )!;
    await waitFor(() =>
      expect(missing).toHaveAttribute("aria-selected", "true"),
    );
    expect(document.activeElement).toBe(missing);

    await openHealth();
    fireEvent.click(
      screen.getByRole("button", {
        name: text("common.confirm"),
      }),
    );
    await waitFor(() =>
      expect(
        document.getElementById(`project-file-template:${deletedTemplateId}`),
      ).not.toBeNull(),
    );
    const deletedTemplate = document.getElementById(
      `project-file-template:${deletedTemplateId}`,
    )!;
    await waitFor(() =>
      expect(deletedTemplate).toHaveAttribute("aria-selected", "true"),
    );
    expect(document.activeElement).toBe(deletedTemplate);
  });

  it("백업 목록은 검증 상태·구형 범위와 다음 페이지를 구별한다", async () => {
    const fixture = transportFixture();
    const rows: BackupRow[] = Array.from({ length: 101 }, (_, index) => ({
      id: `backup-${index}`,
      label: `백업 ${index}`,
      createdAtUtc: `2026-09-20T00:${String(index % 60).padStart(2, "0")}:00.000Z`,
      kind: "manual",
      size: "12",
      status:
        index === 1
          ? "corrupt"
          : index === 2
            ? "unsupported"
            : "verification_required",
      coverage: index === 3 ? "legacy_unknown" : "complete",
      unnamedOrdinal: null,
      locator: `C:\\backups\\worldbuild-backups\\fixture\\backup-${index}.worldbuild-backup`,
    }));
    fixture.transport.backups.push(...rows);
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\project")
      .mockResolvedValueOnce("C:\\backups");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fireEvent.click(screen.getByRole("button", { name: "project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("backup.manage") }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("backup.location") }),
    );

    expect(
      (await screen.findAllByText(text("backup.verificationRequired"))).length,
    ).toBeGreaterThan(0);
    expect(screen.getByText(text("backup.corruptState"))).toBeVisible();
    expect(screen.getByText(text("backup.unsupportedState"))).toBeVisible();
    expect(document.body.textContent).toContain(text("backup.legacyCoverage"));
    fireEvent.click(screen.getByRole("button", { name: /백업 0/u }));
    await waitFor(() =>
      expect(
        fixture.transport.commands.some(
          (command) =>
            command.action === "submit" &&
            command.input.kind === "backup_inspect",
        ),
      ).toBe(true),
    );
    expect(
      screen.getByRole("button", { name: text("backup.restoreCurrent") }),
    ).toBeEnabled();

    const thirdBackup = screen.getByRole("button", { name: "백업 3" });
    expect(thirdBackup.closest("tr")).toHaveTextContent(
      text("backup.verificationRequired"),
    );
    fireEvent.click(thirdBackup);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("backup.restoreCurrent") }),
      ).toBeDisabled(),
    );

    fireEvent.click(
      screen.getByRole("button", { name: text("backup.loadMore") }),
    );
    expect(
      await screen.findByRole("button", { name: /백업 100/u }),
    ).toBeVisible();
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "backup_list" &&
          command.input.cursor === "backup-99",
      ),
    ).toBe(true);
  }, 20_000);

  it("내구성 불확실 set은 실제 재읽기 값을 반영하고 성공으로 표시하지 않는다", async () => {
    const fixture = transportFixture();
    fixture.transport.settingsWriteUncertain = true;
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\current-project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));

    await act(() => shell.setCurrentAsDefault());
    expect(
      (
        await screen.findByText(text("project.defaultUncertainObserved"))
      ).closest(".floating-message-stack"),
    ).not.toBeNull();
    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: text("documents.modes") }),
      ).getByRole("button", { name: /실행 기록/ }),
    );
    expect(
      await screen.findByText(text("project.defaultUncertainObserved"), {
        selector: "summary",
      }),
    ).toBeVisible();
    expect(shell.snapshot().defaultProjectRoot).toBe("C:\\current-project");
    expect(shell.snapshot().defaultProjectObserved).toBe(true);
    expect(screen.queryByText(text("project.defaultSaved"))).toBeNull();
  });

  it("내구성 불확실 clear 뒤 readback 실패는 현재값을 확정하지 않는다", async () => {
    const fixture = transportFixture();
    fixture.transport.defaultProjectRoot = "C:\\old-default";
    fixture.transport.settingsWriteUncertain = true;
    fixture.transport.failSettingsReadback = true;
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));

    await act(() => shell.clearDefaultProject());
    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: text("documents.modes") }),
      ).getByRole("button", { name: /실행 기록/ }),
    );
    expect(
      await screen.findByText(text("project.defaultUncertainUnknown"), {
        selector: "summary",
      }),
    ).toBeVisible();
    expect(fixture.transport.defaultProjectRoot).toBeNull();
    expect(shell.snapshot().defaultProjectObserved).toBe(false);
    expect(screen.queryByText(text("project.defaultCleared"))).toBeNull();
    fireEvent.click(
      screen.getByText(text("project.defaultUncertainUnknown"), {
        selector: "summary",
      }),
    );
    expect(
      screen.getByRole("button", { name: text("project.retrySettings") }),
    ).toBeEnabled();
  });

  it("미적용 write의 readback 실패 뒤 명시 재읽기가 실제 관측 상태를 갱신한다", async () => {
    const fixture = transportFixture();
    fixture.transport.defaultProjectRoot = "C:\\old-default";
    fixture.transport.failSettingsWrite = true;
    fixture.transport.failSettingsReadback = true;
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));

    await act(() => shell.clearDefaultProject());
    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: text("documents.modes") }),
      ).getByRole("button", { name: /실행 기록/ }),
    );
    expect(
      await screen.findByText(text("project.defaultNotAppliedUnknown"), {
        selector: "summary",
      }),
    ).toBeVisible();
    expect(shell.snapshot().defaultProjectObserved).toBe(false);

    fixture.transport.failSettingsReadback = false;
    fixture.transport.failSettingsRead = false;
    fireEvent.click(
      screen.getByText(text("project.defaultNotAppliedUnknown"), {
        selector: "summary",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("project.retrySettings") }),
    );

    expect(
      await screen.findByText(text("project.defaultNotAppliedObserved"), {
        selector: "summary",
      }),
    ).toBeVisible();
    expect(shell.snapshot().defaultProjectRoot).toBe("C:\\old-default");
    expect(shell.snapshot().defaultProjectObserved).toBe(true);
  });

  it("비어 있지 않은 폴더의 새 프로젝트 생성을 거절하고 기존 owner를 정리한다", async () => {
    const fixture = transportFixture();
    fixture.transport.projectNotEmpty = true;
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\projects"),
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("project.create") }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("project.create") }),
    );
    fireEvent.change(await screen.findByLabelText(text("project.name")), {
      target: { value: "occupied" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseLocation") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBeNull());
    expect(
      await screen.findByText(text("project.createNotEmpty")),
    ).toBeVisible();
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(true);
  });

  it("생성 실패 뒤 외부 항목 때문에 정리를 거절하면 보존 사실을 안내한다", async () => {
    const fixture = transportFixture();
    fixture.transport.initializationFailed = true;
    fixture.transport.initializationCleanupOutcome =
      "preserved_external_entries";
    fixture.transport.status = "Failed";
    fixture.transport.runtime = "Stopped";
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\projects"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    const create = await screen.findByRole("button", {
      name: text("project.create"),
    });
    await waitFor(() => expect(create).toBeEnabled());
    fireEvent.click(create);
    fireEvent.change(await screen.findByLabelText(text("project.name")), {
      target: { value: "cleanup-preserved" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseLocation") }),
    );

    await waitFor(() =>
      expect(
        screen.getByText(text("project.createCleanupPreserved")),
      ).toBeVisible(),
    );
    expect(shell.snapshot().projectId).toBeNull();
  });

  it("초기화 실패를 정리하고 폴더 선택기로 실제 작업 화면에서 다시 연다", async () => {
    const fixture = transportFixture();
    fixture.transport.initializationFailed = true;
    fixture.transport.status = "Failed";
    fixture.transport.runtime = "Stopped";
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\missing-project")
      .mockResolvedValueOnce("C:\\valid-project");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message07") }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() =>
      expect(shell.snapshot().project?.error?.code).toBe(
        "initialization_failed",
      ),
    );
    expect(
      screen.queryByText(text("project.initializationFailed")),
    ).not.toBeInTheDocument();
    expect(
      await screen.findByRole("button", {
        name: /실행 기록 · 확인할 오류 있음/u,
      }),
    ).toBeVisible();

    fixture.transport.initializationFailed = false;
    fixture.transport.projectStopped = false;
    fixture.transport.status = "Ready";
    fixture.transport.runtime = "Ready";
    fireEvent.click(
      screen.getByRole("button", { name: text("project.correctPath") }),
    );
    await waitFor(() =>
      expect(shell.snapshot().project?.runtime).toBe("Ready"),
    );
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" && command.input.kind === "close",
      ),
    ).toBe(true);
    expect(
      fixture.transport.commands.some(
        (command) => command.action === "acknowledge_shutdown",
      ),
    ).toBe(true);
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(true);
    expect(picker).toHaveBeenCalledTimes(2);
    expect(
      screen.queryByRole("button", { name: text("project.correctPath") }),
    ).toBeNull();
  });
});
