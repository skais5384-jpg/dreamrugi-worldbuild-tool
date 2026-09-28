import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { text } from "../strings";
import { SvnRegisterDialog } from "./SvnRegisterDialog";
import { svnClient } from "./svnClient";

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(svnClient, "session").mockResolvedValue({
    connected: true,
    url: "https://example.test/svn",
    username: "test-user",
    remembered: true,
  });
});

it("previews only the server selection and registers the exact confirmed fingerprint", async () => {
  const preview = vi.spyOn(svnClient, "registerPreview").mockResolvedValue({
    root: "C:\\owned\\Project",
    url: "https://example.test/svn/Project",
    directories: ["documents", "templates"],
    files: ["templates/one.json"],
    excludedFiles: 2,
    fingerprint: "candidate-fingerprint",
  });
  const register = vi.spyOn(svnClient, "register").mockResolvedValue({
    root: "C:\\owned\\Project",
    url: "https://example.test/svn/Project",
    setupRevision: "3",
    revision: "4",
    files: 1,
  });
  const openProject = vi.fn().mockResolvedValue(undefined);
  render(
    <SvnRegisterDialog
      root={"C:\\owned\\Project"}
      kind="new"
      sessionRevision={0}
      close={() => undefined}
      connect={() => undefined}
      openProject={openProject}
    />,
  );
  const url = await screen.findByRole("textbox", {
    name: /새 서버 프로젝트 URL/u,
  });
  expect(url).toHaveValue("https://example.test/svn/Project");
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerPreview") }),
  );
  await waitFor(() => expect(preview).toHaveBeenCalledTimes(1));
  expect(await screen.findByText("templates/one.json")).toBeVisible();
  expect(screen.getByText(/공유 제외 파일 2개/u)).toBeVisible();
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  );
  await waitFor(() =>
    expect(register).toHaveBeenCalledWith(
      expect.any(String),
      "C:\\owned\\Project",
      "https://example.test/svn/Project",
      text("svn.registerDefaultMessage", { name: "Project" }),
      "candidate-fingerprint",
      false,
    ),
  );
  fireEvent.click(
    await screen.findByRole("button", { name: text("svn.registerOpen") }),
  );
  expect(openProject).toHaveBeenCalledWith("C:\\owned\\Project");
});

it("requires an explicit choice before resuming an empty server child", async () => {
  vi.spyOn(svnClient, "registerPreview").mockResolvedValue({
    root: "C:\\owned\\Project",
    url: "https://example.test/svn/Project",
    directories: ["documents"],
    files: [],
    excludedFiles: 0,
    fingerprint: "candidate-fingerprint",
  });
  const register = vi
    .spyOn(svnClient, "register")
    .mockRejectedValueOnce("svn_register_checkout_unverified")
    .mockResolvedValueOnce({
      root: "C:\\owned\\Project",
      url: "https://example.test/svn/Project",
      setupRevision: "3",
      revision: "4",
      files: 0,
    });
  render(
    <SvnRegisterDialog
      root={"C:\\owned\\Project"}
      kind="copy"
      sessionRevision={0}
      close={() => undefined}
      connect={() => undefined}
      openProject={async () => undefined}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: text("svn.registerPreview") }),
  );
  await screen.findByText("documents");
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  );
  const resume = await screen.findByRole("checkbox", {
    name: text("svn.registerResumeEmpty"),
  });
  expect(resume).not.toBeChecked();
  fireEvent.click(resume);
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerPreview") }),
  );
  await screen.findByText("documents");
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  );
  await waitFor(() => expect(register).toHaveBeenCalledTimes(2));
  expect(register.mock.calls[0][5]).toBe(false);
  expect(register.mock.calls[1][5]).toBe(true);
});

it("invalidates the preview when the target URL changes", async () => {
  vi.spyOn(svnClient, "registerPreview").mockResolvedValue({
    root: "C:\\owned\\Project",
    url: "https://example.test/svn/Project",
    directories: [],
    files: ["templates/one.json"],
    excludedFiles: 0,
    fingerprint: "candidate-fingerprint",
  });
  const register = vi.spyOn(svnClient, "register");
  render(
    <SvnRegisterDialog
      root={"C:\\owned\\Project"}
      kind="copy"
      sessionRevision={0}
      close={() => undefined}
      connect={() => undefined}
      openProject={async () => undefined}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: text("svn.registerPreview") }),
  );
  expect(await screen.findByText("templates/one.json")).toBeVisible();
  fireEvent.change(
    screen.getByRole("textbox", { name: /새 서버 프로젝트 URL/u }),
    {
      target: { value: "https://example.test/svn/Other" },
    },
  );
  expect(screen.queryByText("templates/one.json")).toBeNull();
  expect(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  ).toBeDisabled();
  expect(register).not.toHaveBeenCalled();
});

it("keeps an authentication failure inline and requires a fresh preview", async () => {
  vi.spyOn(svnClient, "registerPreview").mockResolvedValue({
    root: "C:\\owned\\Project",
    url: "https://example.test/svn/Project",
    directories: ["documents"],
    files: [],
    excludedFiles: 0,
    fingerprint: "candidate-fingerprint",
  });
  const register = vi
    .spyOn(svnClient, "register")
    .mockRejectedValue("svn_auth_failed");
  render(
    <SvnRegisterDialog
      root="C:\\owned\\Project"
      kind="copy"
      sessionRevision={0}
      close={() => undefined}
      connect={() => undefined}
      openProject={async () => undefined}
    />,
  );
  fireEvent.click(
    await screen.findByRole("button", { name: text("svn.registerPreview") }),
  );
  await screen.findByText("documents");
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    text("svn.errorAuth"),
  );
  expect(register).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByRole("checkbox", { name: text("svn.registerResumeEmpty") }),
  ).toBeNull();
  expect(
    screen.getByRole("button", { name: text("svn.registerAction") }),
  ).toBeDisabled();
  expect(
    screen.getByRole("button", { name: text("svn.registerPreview") }),
  ).toBeEnabled();
});

it.each(["svn_register_path_exists", "svn_register_setup_unverified"])(
  "keeps %s inline without offering an unproven resume",
  async (reason) => {
    vi.spyOn(svnClient, "registerPreview").mockResolvedValue({
      root: "C:\\owned\\Project",
      url: "https://example.test/svn/Project",
      directories: ["documents"],
      files: [],
      excludedFiles: 0,
      fingerprint: "candidate-fingerprint",
    });
    vi.spyOn(svnClient, "register").mockRejectedValue(reason);
    render(
      <SvnRegisterDialog
        root="C:\\owned\\Project"
        kind="copy"
        sessionRevision={0}
        close={() => undefined}
        connect={() => undefined}
        openProject={async () => undefined}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("svn.registerPreview") }),
    );
    await screen.findByText("documents");
    fireEvent.click(
      screen.getByRole("button", { name: text("svn.registerAction") }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      text(
        reason === "svn_register_path_exists"
          ? "svn.registerPathExists"
          : "svn.registerSetupUnknown",
      ),
    );
    expect(
      screen.queryByRole("checkbox", { name: text("svn.registerResumeEmpty") }),
    ).toBeNull();
  },
);
