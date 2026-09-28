import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { text } from "../strings";
import type { WorkspaceController } from "./workspaceController";
import { svnClient, type SvnStatus } from "./svnClient";
import { SvnToolbar } from "./SvnToolbar";

const root = "C:\\owned project\\project";
const status = (remote: string | null): SvnStatus => ({
  info: {
    root,
    wcRoot: root,
    url: "https://example.test/svn/project",
    repository: "https://example.test/svn",
    revision: "4",
  },
  entries: [
    {
      path: `${root}\\documents\\a.json`,
      local: "normal",
      properties: "none",
      remote,
      remoteProperties: "none",
      lockOwner: null,
      wcLocked: false,
      workingCopyLocked: false,
      needsLock: false,
      remoteOnly: false,
    },
  ],
  serverRevision: "5",
  serverError: null,
  recovery: null,
  updateBlock: null,
});
const session = {
  connected: true,
  url: "https://example.test/svn/project",
  username: "finn001",
  remembered: true,
};
function controller() {
  let projectId: string | null = "project-one";
  const navigate = vi.fn(async () => {
    projectId = null;
  });
  const openDownloadedProject = vi.fn(async () => {
    projectId = "project-two";
    return true;
  });
  const refreshAfterExternalFiles = vi.fn(async () => undefined);
  const load = vi.fn(async () => undefined);
  return {
    navigate,
    shell: {
      snapshot: () => ({ projectId, root }),
      projectGeneration: () => 1,
      openDownloadedProject,
      refreshAfterExternalFiles,
    },
    documents: { load },
  } as unknown as WorkspaceController;
}
beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(svnClient, "session").mockResolvedValue(session);
});
describe("common SVN toolbar", () => {
  it("keeps home hidden and personal project free of server queries", () => {
    const query = vi.spyOn(svnClient, "status");
    const props = {
      root: "",
      collaborative: false,
      generation: 0,
      controller: controller(),
      openConnection: vi.fn(),
      onUpdatingChange: vi.fn(),
      onConnectionVerified: vi.fn(),
      connectionFailed: false,
      sessionRevision: 0,
      blocked: false,
    };
    const view = render(<SvnToolbar {...props} />);
    expect(screen.queryByText(text("svn.personalProject"))).toBeNull();
    view.rerender(<SvnToolbar {...props} root={root} />);
    expect(screen.getByText(text("svn.personalProject"))).toBeVisible();
    expect(query).not.toHaveBeenCalled();
  });
  it("shows incoming separately, then refreshes the same open project after GUI closure", async () => {
    const query = vi
      .spyOn(svnClient, "status")
      .mockResolvedValueOnce(status("modified"))
      .mockResolvedValue(status(null));
    const update = vi
      .spyOn(svnClient, "update")
      .mockResolvedValue(status(null));
    const owner = controller();
    const changing = vi.fn();
    render(
      <SvnToolbar
        root={root}
        collaborative
        generation={1}
        controller={owner}
        openConnection={vi.fn()}
        onUpdatingChange={changing}
        onConnectionVerified={vi.fn()}
        connectionFailed={false}
        sessionRevision={0}
        blocked={false}
      />,
    );
    expect(await screen.findByText(text("svn.remoteIncoming"))).toBeVisible();
    expect(screen.getByText("로컬 변경 없음")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: text("svn.update") }));
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith(expect.any(String), root),
    );
    await waitFor(() =>
      expect(owner.shell.refreshAfterExternalFiles).toHaveBeenCalledOnce(),
    );
    expect(owner.documents.load).toHaveBeenCalledOnce();
    expect(owner.navigate).not.toHaveBeenCalled();
    expect(owner.shell.openDownloadedProject).not.toHaveBeenCalled();
    expect(owner.shell.snapshot().projectId).toBe("project-one");
    await waitFor(() => expect(query).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(text("svn.remoteCurrent"))).toBeVisible();
    expect(changing.mock.calls).toEqual([[true], [false]]);
  });
  it("keeps a verified local conflict visible when the server check fails", async () => {
    vi.spyOn(svnClient, "status").mockResolvedValue({
      ...status(null),
      entries: [{ ...status(null).entries[0], local: "conflicted" }],
      serverRevision: null,
      serverError: "svn_connection_failed",
      updateBlock: "svn_server_unconfirmed",
    });
    render(
      <SvnToolbar
        root={root}
        collaborative
        generation={1}
        controller={controller()}
        openConnection={vi.fn()}
        onUpdatingChange={vi.fn()}
        onConnectionVerified={vi.fn()}
        connectionFailed={false}
        sessionRevision={0}
        blocked={false}
      />,
    );
    expect(await screen.findByText("충돌")).toBeVisible();
    expect(screen.getByText(text("svn.remoteUnknown"))).toBeVisible();
    expect(
      screen.getByRole("button", { name: text("svn.update") }),
    ).toBeDisabled();
  });
  it("shows the active account on a local working copy without a false red failure", async () => {
    vi.spyOn(svnClient, "status").mockResolvedValue({
      ...status(null),
      info: {
        ...status(null).info,
        url: "file:///C:/owned-repository/project",
        repository: "file:///C:/owned-repository",
      },
    });
    const view = render(
      <SvnToolbar
        root={root}
        collaborative
        generation={1}
        controller={controller()}
        openConnection={vi.fn()}
        onUpdatingChange={vi.fn()}
        onConnectionVerified={vi.fn()}
        connectionFailed={false}
        sessionRevision={0}
        blocked={false}
      />,
    );
    await waitFor(() =>
      expect(view.container.querySelector(".svn-connection-ok")).not.toBeNull(),
    );
    expect(view.container.querySelector(".svn-connection-fail")).toBeNull();
  });
  it("replaces a confirmed saved document row without a new server scan", async () => {
    const query = vi.spyOn(svnClient, "status").mockResolvedValue(status(null));
    const local = vi.spyOn(svnClient, "localDocumentStatus").mockResolvedValue({
      ...status(null).entries[0],
      local: "modified",
      wcLocked: true,
      lockOwner: "finn001",
    });
    const base = {
      root,
      collaborative: true,
      generation: 1,
      controller: controller(),
      openConnection: vi.fn(),
      onUpdatingChange: vi.fn(),
      onConnectionVerified: vi.fn(),
      connectionFailed: false,
      sessionRevision: 0,
      blocked: false,
    };
    const view = render(<SvnToolbar {...base} />);
    expect(await screen.findByText("로컬 변경 없음")).toBeVisible();
    view.rerender(
      <SvnToolbar
        {...base}
        localObservation={{ document: "a", sequence: 1 }}
      />,
    );
    await waitFor(() => expect(local).toHaveBeenCalledWith(root, "a"));
    expect(await screen.findByText("수정됨")).toBeVisible();
    expect(query).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: text("svn.update") }),
    ).toBeDisabled();
  });
  it("allows an actual server-only addition while marking incoming red", async () => {
    const addition = {
      ...status("added").entries[0],
      path: `${root}\\new-folder\\new.txt`,
      local: "none",
      remoteOnly: true,
    };
    const incoming = {
      ...status(null),
      entries: [...status(null).entries, addition],
    };
    vi.spyOn(svnClient, "status").mockResolvedValue(incoming);
    const view = render(
      <SvnToolbar
        root={root}
        collaborative
        generation={1}
        controller={controller()}
        openConnection={vi.fn()}
        onUpdatingChange={vi.fn()}
        onConnectionVerified={vi.fn()}
        connectionFailed={false}
        sessionRevision={0}
        blocked={false}
      />,
    );
    expect(await screen.findByText(text("svn.remoteIncoming"))).toBeVisible();
    expect(view.container.querySelector(".svn-remote-incoming")).not.toBeNull();
    expect(
      screen.getByRole("button", { name: text("svn.update") }),
    ).toBeEnabled();
  });
  it("shows cleanup details on demand until status permits the next update", async () => {
    const locked: SvnStatus = {
      ...status("modified"),
      info: { ...status("modified").info, wcRoot: `\\\\?\\${root}` },
      entries: [
        {
          ...status("modified").entries[0],
          local: "incomplete",
          workingCopyLocked: true,
        },
        status(null).entries[0],
      ],
      recovery: "cleanupRequired",
      updateBlock: "svn_working_copy_cleanup_required",
    };
    const resumable: SvnStatus = {
      ...locked,
      entries: locked.entries.map((entry) => ({
        ...entry,
        workingCopyLocked: false,
      })),
      recovery: "resumeRequired",
      updateBlock: null,
    };
    vi.spyOn(svnClient, "status")
      .mockResolvedValueOnce(locked)
      .mockResolvedValue(resumable);
    const cleanup = vi.spyOn(svnClient, "cleanup").mockResolvedValue(resumable);
    const view = render(
      <SvnToolbar
        root={root}
        collaborative
        generation={1}
        controller={controller()}
        openConnection={vi.fn()}
        onUpdatingChange={vi.fn()}
        onConnectionVerified={vi.fn()}
        connectionFailed={false}
        sessionRevision={0}
        blocked={false}
      />,
    );
    const cleanupButton = await screen.findByRole("button", {
      name: text("svn.cleanupAction"),
    });
    await waitFor(() => expect(cleanupButton).toBeEnabled());
    expect(
      screen.getByRole("button", { name: text("svn.update") }),
    ).toBeDisabled();
    expect(view.container.querySelector(".svn-recovery")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(view.container.querySelector(".svn-remote-incoming")).not.toBeNull();
    fireEvent.click(cleanupButton);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent(text("svn.cleanupRequired"));
    expect(dialog).toHaveTextContent(text("svn.cleanupLocksHelp"));
    expect(dialog.querySelector(".svn-recovery-path")).toHaveTextContent(root);
    expect(dialog.querySelector(".svn-recovery-path")).not.toHaveTextContent(
      "\\\\?\\",
    );
    expect(cleanup).not.toHaveBeenCalled();
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("svn.cleanupAction") }),
    );
    await waitFor(() => expect(cleanup).toHaveBeenCalledOnce());
    await waitFor(() => expect(cleanupButton).toBeDisabled());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      await screen.findByText(text("svn.localResumeRequired")),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: text("svn.update") }),
    ).toBeEnabled();
  });
});
