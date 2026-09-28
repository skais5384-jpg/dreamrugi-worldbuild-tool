import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { text } from "../strings";
import type { TemplateController } from "./controller";
import { SvnDialog } from "./SvnDialog";
import { svnClient } from "./svnClient";

const chooseFile = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: chooseFile }));
const controller = {} as TemplateController;

beforeEach(() => {
  vi.restoreAllMocks();
  chooseFile.mockReset();
  vi.spyOn(svnClient, "probe").mockResolvedValue({
    installed: true,
    path: "C:\\Program Files\\TortoiseSVN\\bin\\svn.exe",
    guiInstalled: true,
    guiPath: "C:\\Program Files\\TortoiseSVN\\bin\\TortoiseProc.exe",
    version: "1.14.5",
    reason: null,
  });
  vi.spyOn(svnClient, "session").mockResolvedValue({
    connected: false,
    url: null,
    username: null,
    remembered: false,
  });
});

describe("SVN dialog request recovery", () => {
  it("receives an exact child project URL while keeping the connected account", async () => {
    vi.mocked(svnClient.session).mockResolvedValue({
      connected: true,
      url: "https://example.test/svn/project",
      username: "finn001",
      remembered: true,
    });
    const checkout = vi.spyOn(svnClient, "checkout").mockResolvedValue({
      root: "C:\\owned\\second-wc",
      wcRoot: "C:\\owned\\second-wc",
      url: "https://example.test/svn/project/child",
      repository: "https://example.test/svn",
      revision: "8",
    });
    const openDownloadedProject = vi.fn().mockResolvedValue(true);
    render(
      <SvnDialog
        open
        entry="checkout"
        onClose={() => undefined}
        controller={{ openDownloadedProject } as unknown as TemplateController}
        onSessionChanged={() => undefined}
      />,
    );
    fireEvent.change(await screen.findByLabelText(text("svn.checkoutUrl")), {
      target: { value: "https://example.test/svn/project/child" },
    });
    fireEvent.change(screen.getByLabelText(text("svn.destination")), {
      target: { value: "C:\\owned\\second-wc" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("svn.checkoutAction") }),
    );
    await waitFor(() =>
      expect(checkout).toHaveBeenCalledWith(
        expect.any(String),
        "https://example.test/svn/project/child",
        "C:\\owned\\second-wc",
      ),
    );
    expect(openDownloadedProject).toHaveBeenCalledWith("C:\\owned\\second-wc");
  });
  it("shows confirmed tool paths and connection without redundant inputs or actions", async () => {
    vi.mocked(svnClient.probe).mockResolvedValue({
      installed: true,
      path: "\\\\?\\C:\\Program Files\\TortoiseSVN\\bin\\svn.exe",
      guiInstalled: true,
      guiPath: "\\\\?\\C:\\Program Files\\TortoiseSVN\\bin\\TortoiseProc.exe",
      version: "1.14.5",
      reason: null,
    });
    vi.mocked(svnClient.session).mockResolvedValue({
      connected: true,
      url: "https://example.test/svn/project",
      username: "finn001",
      remembered: true,
    });
    render(
      <SvnDialog
        open
        entry="checkout"
        onClose={() => undefined}
        controller={controller}
        onSessionChanged={() => undefined}
      />,
    );
    expect(await screen.findByText(text("svn.installVerified"))).toBeVisible();
    expect(await screen.findByText(text("svn.connectionReady"))).toBeVisible();
    expect(
      screen.getAllByRole("button", { name: text("svn.changePath") }),
    ).toHaveLength(2);
    expect(
      screen.queryByRole("button", { name: text("svn.probe") }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: text("svn.login") }),
    ).toBeNull();
    expect(
      screen.getByRole("button", { name: text("svn.logout") }),
    ).toBeVisible();
    expect(screen.queryByLabelText(text("svn.password"))).toBeNull();
    expect(screen.getByText("https://example.test/svn/project")).toBeVisible();
    expect(screen.getByText("finn001")).toBeVisible();
    expect(
      screen.getByText("C:\\Program Files\\TortoiseSVN\\bin\\svn.exe"),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: text("svn.checkoutTitle") }),
    ).toBeVisible();
    expect(screen.getByLabelText(text("svn.checkoutUrl"))).toHaveValue(
      "https://example.test/svn/project",
    );

    fireEvent.click(
      screen.getAllByRole("button", { name: text("svn.changePath") })[0],
    );
    expect(screen.getByLabelText(text("svn.cliPath"))).toBeVisible();
    expect(
      screen.getByRole("button", { name: text("svn.chooseExecutable") }),
    ).toBeVisible();
    chooseFile.mockResolvedValue("C:\\Tools\\svn.exe");
    fireEvent.click(
      screen.getByRole("button", { name: text("svn.chooseExecutable") }),
    );
    await waitFor(() =>
      expect(screen.getByLabelText(text("svn.cliPath"))).toHaveValue(
        "C:\\Tools\\svn.exe",
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: text("svn.probe") }));
    await waitFor(() =>
      expect(svnClient.probe).toHaveBeenCalledWith(
        "C:\\Tools\\svn.exe",
        "C:\\Program Files\\TortoiseSVN\\bin\\TortoiseProc.exe",
      ),
    );
    await waitFor(() =>
      expect(screen.queryByLabelText(text("svn.cliPath"))).toBeNull(),
    );
  });

  it("failed login leaves a retry available and only successful login clears its error", async () => {
    const login = vi
      .spyOn(svnClient, "login")
      .mockRejectedValueOnce("svn_auth_failed")
      .mockResolvedValueOnce({
        root: "",
        wcRoot: "",
        url: "https://example.test/svn/project",
        repository: "https://example.test/svn",
        revision: "2",
      });
    const logout = vi.spyOn(svnClient, "logout").mockResolvedValue();
    render(
      <SvnDialog
        open
        entry="connect"
        onClose={() => undefined}
        controller={controller}
        onSessionChanged={() => undefined}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("svn.login") }),
      ).toBeDisabled(),
    );
    fireEvent.change(screen.getByLabelText(text("svn.url")), {
      target: { value: "https://example.test/svn/project" },
    });
    fireEvent.change(screen.getByLabelText(text("svn.username")), {
      target: { value: "finn001" },
    });
    fireEvent.change(screen.getByLabelText(text("svn.password")), {
      target: { value: "incorrect" },
    });
    fireEvent.click(screen.getByRole("button", { name: text("svn.login") }));
    expect(await screen.findByText(text("svn.errorAuth"))).toBeVisible();
    expect(login).toHaveBeenCalledTimes(1);

    fireEvent.change(screen.getByLabelText(text("svn.password")), {
      target: { value: "accepted" },
    });
    fireEvent.click(screen.getByRole("button", { name: text("svn.login") }));
    expect(await screen.findByText(text("svn.connectionReady"))).toBeVisible();
    expect(screen.queryByText(text("svn.errorAuth"))).toBeNull();
    expect(login).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole("button", { name: text("svn.logout") }));
    expect(await screen.findByText(text("svn.logoutDone"))).toBeVisible();
    expect(logout).toHaveBeenCalledTimes(1);
  });

  it("a late login result from a closed dialog cannot reconnect the next view", async () => {
    let finish:
      | ((value: {
          root: string;
          wcRoot: string;
          url: string;
          repository: string;
          revision: string;
        }) => void)
      | null = null;
    vi.spyOn(svnClient, "login").mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const view = render(
      <SvnDialog
        open
        entry="connect"
        onClose={() => undefined}
        controller={controller}
        onSessionChanged={() => undefined}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("svn.login") }),
      ).toBeDisabled(),
    );
    fireEvent.change(screen.getByLabelText(text("svn.url")), {
      target: { value: "https://example.test/svn/project" },
    });
    fireEvent.change(screen.getByLabelText(text("svn.username")), {
      target: { value: "finn001" },
    });
    fireEvent.change(screen.getByLabelText(text("svn.password")), {
      target: { value: "accepted" },
    });
    fireEvent.click(screen.getByRole("button", { name: text("svn.login") }));
    await waitFor(() => expect(finish).not.toBeNull());
    view.rerender(
      <SvnDialog
        open={false}
        entry="connect"
        onClose={() => undefined}
        controller={controller}
        onSessionChanged={() => undefined}
      />,
    );
    await act(async () => {
      finish!({
        root: "",
        wcRoot: "",
        url: "https://example.test/svn/project",
        repository: "https://example.test/svn",
        revision: "2",
      });
    });
    view.rerender(
      <SvnDialog
        open
        entry="connect"
        onClose={() => undefined}
        controller={controller}
        onSessionChanged={() => undefined}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("svn.login") }),
      ).toBeDisabled(),
    );
    expect(screen.queryByText(text("svn.loginReady"))).toBeNull();
  });
});
