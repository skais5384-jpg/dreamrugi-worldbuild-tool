import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { text } from "../strings";
import type { DocumentController } from "./documentController";
import { SvnCommitDialog } from "./SvnCommitDialog";
import { svnClient } from "./svnClient";

afterEach(() => vi.restoreAllMocks());

it("keeps an uncertain commit from being sent a second time", async () => {
  vi.spyOn(svnClient, "commitCandidates").mockResolvedValue([
    {
      path: "templates/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json",
      name: "시험 템플릿",
      local: "modified",
      properties: "none",
      held: true,
      newPath: false,
      canScheduleDelete: false,
      changed: true,
      eligible: true,
      blockedReason: null,
      required: [],
    },
  ]);
  const commit = vi
    .spyOn(svnClient, "commit")
    .mockRejectedValue("svn_commit_unverified");
  const snapshot = { editors: {} };
  const controller = {
    subscribe: () => () => {},
    snapshot: () => snapshot,
  } as unknown as DocumentController;
  render(
    <SvnCommitDialog
      root="C:\\owned\\working-copy"
      document={null}
      controller={controller}
      close={() => {}}
      committed={async () => []}
    />,
  );
  fireEvent.click(
    await screen.findByRole("checkbox", { name: /시험 템플릿/u }),
  );
  fireEvent.change(
    screen.getByRole("textbox", { name: text("svn.commitMessage") }),
    {
      target: { value: "owned uncertain commit" },
    },
  );
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.commitAction") }),
  );
  await waitFor(() => expect(commit).toHaveBeenCalledTimes(1));
  expect(
    await screen.findByText(text("svn.commitResultUnknownHelp")),
  ).toBeVisible();
  const submit = screen.getByRole("button", { name: text("svn.commitAction") });
  expect(submit).toBeDisabled();
  fireEvent.click(submit);
  expect(commit).toHaveBeenCalledTimes(1);
});

it("keeps a confirmed revision through postcheck failure and only rechecks state", async () => {
  const path = "templates/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb.json";
  const root = "C:\\owned\\postcheck-fixture";
  vi.spyOn(svnClient, "commitCandidates").mockResolvedValue([
    {
      path,
      name: "후속 확인 시험",
      local: "modified",
      properties: "none",
      held: true,
      newPath: false,
      canScheduleDelete: false,
      changed: true,
      eligible: true,
      blockedReason: null,
      required: [],
    },
  ]);
  const outcome = {
    revision: "42",
    paths: [path],
    deleted: [],
    unlockPending: [],
    verificationUnknown: [
      { path, check: "remoteLock", reason: "owned_postcheck_fault" },
    ],
  };
  const commit = vi.spyOn(svnClient, "commit").mockResolvedValue(outcome);
  const recheck = vi.spyOn(svnClient, "commitRecheck").mockResolvedValue({
    ...outcome,
    verificationUnknown: [],
  });
  const snapshot = { editors: {} };
  const controller = {
    subscribe: () => () => {},
    snapshot: () => snapshot,
  } as unknown as DocumentController;
  const props = {
    root,
    document: null,
    controller,
    close: () => {},
    committed: vi.fn(async () => []),
  };
  const rendered = render(<SvnCommitDialog {...props} />);
  fireEvent.click(
    await screen.findByRole("checkbox", { name: /후속 확인 시험/u }),
  );
  fireEvent.change(
    screen.getByRole("textbox", { name: text("svn.commitMessage") }),
    {
      target: { value: "owned confirmed commit" },
    },
  );
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.commitAction") }),
  );
  expect(
    await screen.findByText(`${text("svn.commitRevision")} 42`),
  ).toBeVisible();
  expect(screen.getByText(text("svn.commitLockUnknown"))).toBeVisible();
  expect(
    screen
      .getByText(text("svn.commitLockUnknown"))
      .closest(".inline-notice")
      ?.querySelector("svg"),
  ).not.toBeNull();
  expect(
    screen.queryByText(text("svn.commitStatusUnknown")),
  ).not.toBeInTheDocument();
  expect(commit).toHaveBeenCalledTimes(1);
  expect(props.committed).not.toHaveBeenCalled();
  rendered.unmount();
  render(<SvnCommitDialog {...props} />);
  expect(screen.getByText(`${text("svn.commitRevision")} 42`)).toBeVisible();
  expect(
    screen.queryByRole("button", { name: text("svn.commitAction") }),
  ).not.toBeInTheDocument();
  fireEvent.click(
    screen.getByRole("button", { name: text("svn.commitRecheck") }),
  );
  await waitFor(() => expect(recheck).toHaveBeenCalledTimes(1));
  expect(commit).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(props.committed).toHaveBeenCalledWith([path]));
});
