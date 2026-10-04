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
import { text } from "../strings";
import { transportFixture } from "./WorkspaceApp.testHarness";
import type { Creation } from "../bridge/documents";

beforeEach(() => {
  // jsdom에는 스크롤 배치가 없다. 이동 요청만 검사하며 실제 가시성은 native에서 확인한다.
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.spyOn(window, "scrollBy").mockImplementation(() => {});
  window.localStorage.clear();
});

describe("whole template workspace", () => {
  it("offers minimal template creation retry only after native custody is safely released", async () => {
    const fixture = transportFixture();
    fixture.transport.rejectWrite = true;
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\fixture"),
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    const templates = await screen.findByRole("button", {
      name: text("documents.templates"),
    });
    await waitFor(() => expect(templates).toBeEnabled());
    fireEvent.click(templates);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message14") }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("app.message14") }),
    );
    const retry = await screen.findByRole("button", {
      name: text("whole.retryCreate"),
    });
    await waitFor(() => expect(retry).toBeEnabled());
    expect(shell.snapshot().retainedRefs).toHaveLength(0);
    expect(shell.snapshot().form?.submitted).toBe(false);
    expect(screen.getByText(text("whole.createFailed"))).toBeVisible();
    fixture.transport.rejectWrite = false;
    fireEvent.click(retry);
    await waitFor(() => expect(controller.snapshot().draft).not.toBeNull());
    expect(shell.snapshot().form).toBeNull();
    expect(fixture.transport.templates.size).toBe(1);
  });
  it("protects post-acquisition rejected template creation from an unsafe duplicate retry", async () => {
    const fixture = transportFixture();
    fixture.transport.rejectWrite = true;
    fixture.transport.creationRejectHasOwner = true;
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\fixture"),
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    const templates = await screen.findByRole("button", {
      name: text("documents.templates"),
    });
    await waitFor(() => expect(templates).toBeEnabled());
    fireEvent.click(templates);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message14") }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("app.message14") }),
    );
    const retry = await screen.findByRole("button", {
      name: text("whole.retryCreate"),
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(retry).toBeDisabled();
    expect(shell.snapshot().retainedRefs).toHaveLength(1);
    expect(shell.snapshot().form?.submitted).toBe(true);
    expect(fixture.transport.templates.size).toBe(0);
  });

  it("keeps a failed creation minimal and preserves its input for retry", async () => {
    const fixture = transportFixture();
    fixture.base.fields[0].required = true;
    const originalWork = fixture.transport.workspaceResult;
    let draft: Creation = {
      kind: "draft",
      owner: "creation-owner",
      generation: "1",
      body: {
        name: "recovered draft",
        parent: null,
        fields: [],
        composing: false,
      },
      template: fixture.base,
      deposited: false,
      outcome: null,
      problem: null,
      field: null,
    };
    fixture.transport.workspaceResult = (input) => {
      if (input.kind === "document_workspace") {
        const request = input.request;
        if (request.action === "begin")
          return { kind: "document_workspace", value: draft };
        if (request.action === "draft") {
          draft = {
            ...draft,
            body: request.body,
            generation: request.generation,
            problem: "InvalidNumber",
            field: "number-field",
          };
          return { kind: "document_workspace", value: draft };
        }
      }
      return originalWork?.(input);
    };
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\fixture"),
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    const start = await screen.findByRole("button", {
      name: text("documents.new"),
    });
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);
    await act(() => controller.documents.begin(fixture.base.id));
    const retry = await screen.findByRole("button", {
      name: text("project.retryDefault"),
    });
    await waitFor(() => expect(retry).toBeEnabled());
    expect(screen.getByText(text("documents.createFailed"))).toBeVisible();
    expect(
      screen.queryByRole("button", { name: text("documents.create") }),
    ).toBeNull();
    expect(screen.queryByText(text("documents.requiredValue"))).toBeNull();
    expect(controller.documents.snapshot().draft?.body.name).toBe(
      text("documents.newDocumentName"),
    );
    const generation = controller.documents.snapshot().draft?.generation;
    fireEvent.click(retry);
    await waitFor(() =>
      expect(controller.documents.snapshot().busy).toBe(false),
    );
    expect(controller.documents.snapshot().draft?.generation).toBe(generation);
    expect(screen.getAllByText(text("documents.createFailed"))).toHaveLength(1);
  });
  it("keeps local work available without YouTube consent and exposes privacy settings in Help", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn(),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message07") }),
      ).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: text("about.menu") }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("privacy.title") }),
    );
    expect(
      screen.getByRole("checkbox", { name: text("privacy.acceptPolicy") }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("button", { name: text("privacy.allow") }),
    ).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: text("privacy.deny") }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message07") }),
      ).toBeEnabled(),
    );
  });
  it("실제 홈에서 기존 프로젝트와 단일 SVN 연결에 접근할 수 있다", async () => {
    const fixture = transportFixture();
    const picker = vi.fn().mockResolvedValue("C:\\existing-project");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    const start = screen
      .getByRole("heading", { name: text("project.startTitle") })
      .closest("section");
    expect(start).not.toBeNull();
    expect(within(start!).getAllByRole("button")).toHaveLength(3);
    expect(
      within(start!).getByRole("button", { name: text("svn.checkout") }),
    ).toBeEnabled();
    fireEvent.click(
      within(start!).getByRole("button", { name: text("svn.checkout") }),
    );
    expect(
      await screen.findByRole("dialog", { name: text("svn.title") }),
    ).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: text("svn.close") }));
    expect(
      within(start!).queryByText(text("backup.restoreFromHome")),
    ).toBeNull();
    expect(screen.queryByLabelText(text("app.message06"))).toBeNull();

    const projectMenu = screen.getByRole("button", {
      name: text("documents.projectMenu"),
    });
    fireEvent.click(projectMenu);
    expect(
      await screen.findByRole("menuitem", {
        name: text("backup.restoreFromHome"),
      }),
    ).toBeVisible();
    fireEvent.keyDown(document, { key: "Escape" });

    fireEvent.click(open);
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    expect(picker).toHaveBeenCalledWith("open");
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "open" &&
          command.input.root === "C:\\existing-project",
      ),
    ).toBe(true);
  });

  it("프로젝트가 열리지 않아도 앱 진단 정보 화면을 열 수 있다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\fixture"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    const open = await screen.findByRole("button", {
      name: text("app.message07"),
    });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.projectMenu") }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("health.menu") }),
    );
    expect(await screen.findByText(text("health.title"))).toBeVisible();
    expect(screen.getByText(text("health.notChecked"))).toBeVisible();
    expect(screen.getByText(text("health.badge.unchecked"))).toBeVisible();
    expect(
      screen.getByRole("button", { name: text("health.export") }),
    ).toBeEnabled();
  });

  it("프로젝트 메뉴는 아이콘을 표시하고 현재 기본값 상태에 맞는 설정 동작 하나만 제공한다", async () => {
    const fixture = transportFixture();
    fixture.transport.canonicalizeSettingsWrite = true;
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
    const menu = await screen.findByRole("button", { name: "current-project" });
    fireEvent.click(menu);

    const setDefault = await screen.findByRole("menuitem", {
      name: text("project.setDefault"),
    });
    expect(
      screen.queryByRole("menuitem", { name: text("project.clearDefault") }),
    ).toBeNull();
    const menuItems = screen.getAllByRole("menuitem");
    expect(menuItems.map((item) => item.textContent?.trim())).toEqual([
      text("policy.title"),
      text("project.setDefault"),
      text("app.message07"),
      text("backup.copy"),
      text("svn.registerExisting"),
      text("app.message03"),
      text("health.menu"),
      text("backup.manage"),
      text("app.message10"),
      text("app.message04"),
    ]);
    for (const item of menuItems)
      expect(item.querySelector("svg")).not.toBeNull();

    fireEvent.click(setDefault);
    await waitFor(() =>
      expect(fixture.transport.defaultProjectRoot).toBe(
        "\\\\?\\C:\\current-project",
      ),
    );
    fireEvent.click(menu);
    expect(
      await screen.findByRole("menuitem", {
        name: text("project.clearDefault"),
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("menuitem", { name: text("project.setDefault") }),
    ).toBeNull();
  });

  it("글로서리 탭은 왼쪽 필터와 본문 목록을 제공하고 문서 탭의 빠른 글로서리도 유지한다", async () => {
    const fixture = transportFixture();
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

    const ribbon = await screen.findByRole("navigation", {
      name: text("documents.modes"),
    });
    fireEvent.click(
      within(ribbon).getByRole("button", { name: text("glossary.title") }),
    );
    const filterPanel = await screen.findByRole("complementary", {
      name: text("glossary.templateFilter"),
    });
    expect(
      within(filterPanel).getByLabelText(text("glossary.templateFilter")),
    ).toBeVisible();
    const listPanel = screen.getByRole("region", {
      name: text("glossary.title"),
    });
    expect(within(listPanel).getByText(text("glossary.empty"))).toBeVisible();

    fireEvent.click(
      within(ribbon).getByRole("button", { name: text("documents.title") }),
    );
    await waitFor(() =>
      expect(
        screen
          .getAllByRole("button", { name: text("glossary.open") })
          .find((button) => button.classList.contains("glossary-open-command")),
      ).toBeVisible(),
    );
  });

  it("정상 프로젝트 닫기는 성공 보고를 자동 인수하고 홈으로 바로 돌아간다", async () => {
    const fixture = transportFixture();
    fixture.transport.emptyShutdownReportReads = 1;
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
    fireEvent.click(
      await screen.findByRole("button", { name: "current-project" }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("app.message10") }),
    );

    expect(
      await screen.findByRole("heading", { name: text("project.startTitle") }),
    ).toBeVisible();
    expect(shell.snapshot().projectId).toBeNull();
    expect(shell.snapshot().error).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(
      screen.queryByRole("button", { name: text("app.message12") }),
    ).toBeNull();
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
  });

  it("퇴역한 프로젝트의 늦은 상태 오류를 홈 알림으로 표시하지 않는다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\current-project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await screen.findByRole("button", { name: "current-project" });

    const invoke = fixture.transport.invoke.bind(fixture.transport);
    let release!: () => void;
    let entered!: () => void;
    const observed = new Promise<void>((resolve) => (entered = resolve));
    let armed = true;
    let failRetiredAppStatus = false;
    fixture.transport.invoke = async (name, body) => {
      const command = JSON.parse(new TextDecoder().decode(body)) as {
        action: string;
      };
      if (failRetiredAppStatus && command.action === "app_status") {
        failRetiredAppStatus = false;
        throw { code: "unknown_id", nextAction: "" };
      }
      if (armed && command.action === "project_status") {
        armed = false;
        entered();
        await new Promise<void>((resolve) => (release = resolve));
        throw { code: "unknown_id", nextAction: "" };
      }
      return invoke(name, body);
    };
    const stale = shell.checkStatus();
    await observed;
    fireEvent.click(screen.getByRole("button", { name: "current-project" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("app.message10") }),
    );
    await screen.findByRole("heading", { name: text("project.startTitle") });
    release();
    await stale;
    expect(shell.snapshot().projectId).toBeNull();
    expect(shell.snapshot().error).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    failRetiredAppStatus = true;
    await shell.checkStatus();
    expect(shell.snapshot().error).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("명시적으로 닫는 동안 상태 조회가 프로젝트를 먼저 퇴역시키지 않는다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\current-project"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await screen.findByRole("button", { name: "current-project" });
    await waitFor(() => expect(shell.snapshot().busy).toBe(false));

    fixture.transport.hold = "close";
    const closing = shell.navigate({ kind: "close_project" });
    await waitFor(
      () =>
        expect(
          fixture.transport.commands.some(
            (command) =>
              command.action === "submit" && command.input.kind === "close",
          ),
        ).toBe(true),
      { timeout: 2000 },
    );
    fixture.transport.projectCloseRequested = true;
    fixture.transport.projectStopped = true;
    fixture.transport.reportPending = false;
    await shell.checkStatus();
    expect(shell.snapshot().projectId).toBe("project-one");
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(false);

    fixture.transport.hold = null;
    fixture.transport.completeHeld();
    await closing;
    await screen.findByRole("heading", { name: text("project.startTitle") });
    expect(shell.snapshot().error).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each(["0", "1"])(
    "앱 종료 보고의 해제 실패 %s건은 정상 보고만 자동 인수한다",
    async (releaseFailures) => {
      const fixture = transportFixture();
      const shell = new TemplateController(
        new GuardedClient(fixture.transport),
        vi.fn().mockResolvedValue("C:\\fixture"),
      );
      render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
      fireEvent.click(
        await screen.findByRole("button", { name: text("app.message07") }),
      );
      await waitFor(() =>
        expect(shell.snapshot().projectId).toBe("project-one"),
      );
      fixture.transport.projectStopped = true;
      fixture.transport.reportPending = true;
      // Actual native shutdown keeps the report itself as a Results blocker.
      fixture.transport.shutdownBlockers = ["Results"];
      fixture.transport.shutdownReleaseFailures = releaseFailures;
      fixture.transport.closing = true;
      await act(async () => shell.checkStatus());

      const acknowledged = fixture.transport.commands.some(
        (command) => command.action === "acknowledge_shutdown",
      );
      expect(acknowledged).toBe(releaseFailures === "0");
      expect(fixture.transport.reportPending).toBe(releaseFailures !== "0");
      expect(document.querySelector(".follow-up")).toBeNull();
      if (releaseFailures === "0") {
        expect(
          screen.queryByRole("button", { name: text("followUp.openActions") }),
        ).toBeNull();
      } else {
        fireEvent.click(
          screen.getByRole("button", { name: text("followUp.openActions") }),
        );
        expect(
          screen.getByRole("button", {
            name: text("followUp.confirmFailedReport"),
          }),
        ).toBeVisible();
      }
    },
  );

  it("정상 종료 보고라도 Results 외 차단 항목은 자동 인수하지 않는다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\fixture"),
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    fireEvent.click(
      await screen.findByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    fixture.transport.projectStopped = true;
    fixture.transport.reportPending = true;
    fixture.transport.shutdownBlockers = ["Results", "ActiveDraft"];
    fixture.transport.closing = true;
    await act(async () => shell.checkStatus());
    expect(
      fixture.transport.commands.some(
        (c) => c.action === "acknowledge_shutdown",
      ),
    ).toBe(false);
    expect(fixture.transport.reportPending).toBe(true);
  });

  it("프로젝트 메뉴에서 위치 선택 뒤 현재 프로젝트를 안전하게 닫고 다른 프로젝트를 연다", async () => {
    const fixture = transportFixture();
    const picker = vi
      .fn()
      .mockResolvedValueOnce("C:\\current-project")
      .mockResolvedValueOnce("C:\\next-project");
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
    fireEvent.click(
      await screen.findByRole("button", { name: "current-project" }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("app.message07") }),
    );

    expect(
      await screen.findByRole("button", { name: "next-project" }),
    ).toBeVisible();
    expect(shell.snapshot().root).toBe("C:\\next-project");
    expect(picker).toHaveBeenNthCalledWith(1, "open");
    expect(picker).toHaveBeenNthCalledWith(2, "open");
    const submitted = fixture.transport.commands.filter(
      (command) => command.action === "submit",
    );
    const close = submitted.findIndex(
      (command) => command.input.kind === "close",
    );
    const retire = submitted.findIndex(
      (command) => command.input.kind === "retire_project",
    );
    const opens = submitted
      .map((command, index) => ({ command, index }))
      .filter(({ command }) => command.input.kind === "open");
    expect(opens).toHaveLength(2);
    expect(close).toBeLessThan(retire);
    expect(retire).toBeLessThan(opens[1]!.index);
  });

  it("새 프로젝트 이름을 검증한 뒤 선택 위치 아래의 이름 폴더를 생성한다", async () => {
    const fixture = transportFixture();
    const picker = vi.fn().mockResolvedValue("C:\\Users\\tester\\Desktop");
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    const create = await screen.findByRole("button", {
      name: text("project.create"),
    });
    await waitFor(() => expect(create).toBeEnabled());
    fireEvent.click(screen.getByLabelText(text("project.openAsDefault")));
    fireEvent.click(create);
    const name = await screen.findByLabelText(text("project.name"));
    fireEvent.change(name, { target: { value: "CON" } });
    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseLocation") }),
    );
    expect(await screen.findByText(text("project.nameInvalid"))).toBeVisible();
    expect(picker).not.toHaveBeenCalled();

    fireEvent.change(name, { target: { value: "나의 세계" } });
    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseLocation") }),
    );
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    expect(picker).toHaveBeenCalledWith("create");
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "create_project" &&
          command.input.parent === "C:\\Users\\tester\\Desktop" &&
          command.input.name === "나의 세계",
      ),
    ).toBe(true);
    expect(shell.snapshot().root).toBe("C:\\Users\\tester\\Desktop\\나의 세계");
    await waitFor(() =>
      expect(fixture.transport.defaultProjectRoot).toBe(
        "C:\\Users\\tester\\Desktop\\나의 세계",
      ),
    );
  });

  it("새 프로젝트 위치 선택 취소는 이름 대화상자와 디스크 명령을 그대로 보존한다", async () => {
    const fixture = transportFixture();
    const picker = vi.fn().mockResolvedValue(null);
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    const create = await screen.findByRole("button", {
      name: text("project.create"),
    });
    await waitFor(() => expect(create).toBeEnabled());
    fireEvent.click(create);
    const name = await screen.findByLabelText(text("project.name"));
    fireEvent.change(name, { target: { value: "취소해도 남는 이름" } });
    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseLocation") }),
    );

    await waitFor(() => expect(picker).toHaveBeenCalledWith("create"));
    expect(screen.getByLabelText(text("project.name"))).toHaveValue(
      "취소해도 남는 이름",
    );
    expect(shell.snapshot().projectId).toBeNull();
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "create_project",
      ),
    ).toBe(false);
  });

  it("기본 프로젝트 설정을 시작당 한 번 읽고 같은 일반 열기 경로로 자동 연다", async () => {
    const fixture = transportFixture();
    fixture.transport.defaultProjectRoot = "C:\\fixture-default";
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);

    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    expect(shell.snapshot().root).toBe("C:\\fixture-default");
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "project_settings_read",
      ),
    ).toHaveLength(1);
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(1);
    await act(() => shell.checkStatus());
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(1);
  });

  it("기본 프로젝트 자동 열기 실패를 정리하고 재시도 가능한 홈에 머문다", async () => {
    const fixture = transportFixture();
    fixture.transport.defaultProjectRoot = "C:\\missing-default";
    fixture.transport.initializationFailed = true;
    fixture.transport.status = "Failed";
    fixture.transport.runtime = "Stopped";
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);

    await waitFor(() =>
      expect(shell.snapshot().startupFailureKind).toBe("project_open"),
    );
    const notice = await screen.findByRole("alert");
    expect(within(notice).getAllByRole("button")).toHaveLength(2);
    expect(
      within(notice).getByRole("button", { name: text("update.details") }),
    ).toContainHTML("<svg");
    expect(
      within(notice).queryByRole("button", {
        name: text("project.retryDefault"),
      }),
    ).toBeNull();
    expect(shell.snapshot().projectId).toBeNull();
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(1));
    expect(
      screen.getByRole("alert").closest(".floating-message-stack"),
    ).not.toBeNull();
    expect(screen.queryByLabelText(text("app.message06"))).toBeNull();
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(1);
    expect(
      fixture.transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(true);
    fireEvent.click(
      within(notice).getByRole("button", { name: text("update.details") }),
    );
    expect(
      await screen.findByRole("button", { name: text("project.retryDefault") }),
    ).toBeEnabled();
  });

  it("설정 읽기 실패는 root 없이 재읽기·해제·다른 폴더 선택을 모두 제공한다", async () => {
    const fixture = transportFixture();
    fixture.transport.failSettingsRead = true;
    const picker = vi.fn().mockResolvedValue(null);
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      picker,
    );
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    const notice = await screen.findByRole("alert");
    expect(within(notice).getAllByRole("button")).toHaveLength(2);
    fireEvent.click(
      within(notice).getByRole("button", { name: text("update.details") }),
    );
    const retry = await screen.findByRole("button", {
      name: text("project.retrySettings"),
    });
    expect(
      screen.getByRole("button", { name: text("project.chooseAnother") }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: text("project.clearDefault") }),
    ).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: text("project.retryDefault") }),
    ).toBeNull();

    fireEvent.click(
      screen.getByRole("button", { name: text("project.chooseAnother") }),
    );
    await waitFor(() => expect(picker).toHaveBeenCalledWith("open"));
    expect(
      screen
        .getAllByText(text("project.defaultReadFailed"))
        .some((node) => node.closest("table")),
    ).toBe(true);
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(0);

    fixture.transport.failSettingsRead = false;
    fixture.transport.defaultProjectRoot = "C:\\recovered-default";
    fireEvent.click(retry);
    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    expect(shell.snapshot().root).toBe("C:\\recovered-default");
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "project_settings_read",
      ),
    ).toHaveLength(2);
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(1);
  });

  it("설정 root를 읽지 못해도 사용자가 명시한 기본값 해제를 저장한다", async () => {
    const fixture = transportFixture();
    fixture.transport.failSettingsRead = true;
    fixture.transport.defaultProjectRoot = "C:\\unreadable-default";
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    const notice = await screen.findByRole("alert");
    fireEvent.click(
      within(notice).getByRole("button", { name: text("update.details") }),
    );
    const clear = await screen.findByRole("button", {
      name: text("project.clearDefault"),
    });
    fireEvent.click(clear);
    await waitFor(() =>
      expect(fixture.transport.defaultProjectRoot).toBeNull(),
    );
    expect(shell.snapshot().defaultProjectObserved).toBe(true);
    expect(shell.snapshot().startupFailure).toBeNull();
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "project_settings_write",
      ),
    ).toHaveLength(1);
  });

  it("앱 종료 뒤 늦게 도착한 설정 읽기는 상태나 자동 열기를 되살리지 않는다", async () => {
    const fixture = transportFixture();
    fixture.transport.hold = "project_settings_read";
    fixture.transport.defaultProjectRoot = "C:\\late-default";
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);
    await waitFor(() =>
      expect(
        fixture.transport.commands.some(
          (command) =>
            command.action === "submit" &&
            command.input.kind === "project_settings_read",
        ),
      ).toBe(true),
    );

    fixture.transport.nativeClose();
    await act(() => shell.checkStatus());
    await waitFor(() => expect(shell.snapshot().closing).toBe(true));
    fixture.transport.completeHeld();
    await act(() => shell.checkStatus());

    expect(shell.snapshot().defaultProjectObserved).toBe(false);
    expect(shell.snapshot().defaultProjectRoot).toBeNull();
    expect(
      fixture.transport.commands.filter(
        (command) =>
          command.action === "submit" && command.input.kind === "open",
      ),
    ).toHaveLength(0);
  });

  it("열기 성공 뒤에만 선택한 프로젝트를 기본값으로 확정한다", async () => {
    const fixture = transportFixture();
    const shell = new TemplateController(
      new GuardedClient(fixture.transport),
      vi.fn().mockResolvedValue("C:\\chosen-project"),
    );
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("app.message07") }),
      ).toBeEnabled(),
    );
    fireEvent.click(screen.getByLabelText(text("project.openAsDefault")));
    fireEvent.click(
      screen.getByRole("button", { name: text("app.message07") }),
    );
    await waitFor(() =>
      expect(fixture.transport.defaultProjectRoot).toBe("C:\\chosen-project"),
    );
    const submitted = fixture.transport.commands.filter(
      (command) => command.action === "submit",
    );
    expect(
      submitted.findIndex((command) => command.input.kind === "open"),
    ).toBeLessThan(
      submitted.findIndex(
        (command) => command.input.kind === "project_settings_write",
      ),
    );
  });

  it("기본 프로젝트 저장 실패가 열린 프로젝트를 닫거나 기존 기본값을 바꾸지 않는다", async () => {
    const fixture = transportFixture();
    fixture.transport.defaultProjectRoot = "C:\\old-default";
    fixture.transport.failSettingsWrite = true;
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    const controller = new WorkspaceController(shell);
    render(<WorkspaceApp controller={controller} />);

    await waitFor(() => expect(shell.snapshot().projectId).toBe("project-one"));
    await act(() => shell.setCurrentAsDefault());

    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: text("documents.modes") }),
      ).getByRole("button", { name: /실행 기록/ }),
    );

    expect(
      await screen.findByText(text("project.defaultNotAppliedObserved"), {
        selector: "summary",
      }),
    ).toBeVisible();
    expect(shell.snapshot().projectId).toBe("project-one");
    expect(fixture.transport.defaultProjectRoot).toBe("C:\\old-default");
    expect(shell.snapshot().defaultProjectRoot).toBe("C:\\old-default");
    expect(shell.snapshot().defaultProjectObserved).toBe(true);
  });

  it("시작 설정 read 실패 뒤 미적용 clear의 readback 성공은 과거 read 카드만 퇴역시킨다", async () => {
    const fixture = transportFixture();
    fixture.transport.failSettingsRead = true;
    fixture.transport.failSettingsWrite = true;
    const shell = new TemplateController(new GuardedClient(fixture.transport));
    render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

    expect(
      await screen.findByText(text("project.defaultReadFailed")),
    ).toBeVisible();
    fixture.transport.failSettingsRead = false;
    await act(() => shell.clearDefaultProject());

    expect(
      await screen.findByText(text("project.defaultNotAppliedObserved")),
    ).toBeVisible();
    expect(screen.queryByText(text("project.defaultReadFailed"))).toBeNull();
    expect(shell.snapshot().settingsNotice?.kind).toBe("not_applied");
    expect(shell.snapshot().startupFailureKind).toBeNull();
  });
});
