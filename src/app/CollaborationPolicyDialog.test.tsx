import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { CollaborationPolicyDialog } from "./CollaborationPolicyDialog";
import { svnClient, type PolicySnapshot } from "./svnClient";
import { text } from "../strings";
afterEach(() => vi.restoreAllMocks());
const policy = (version: string) => ({
  formatVersion: 1,
  repository: "https://example.org/svn",
  repositoryId: "b726ba7c-6f1b-4cc3-98e1-316203527a4a",
  project: "https://example.org/svn/team",
  minimumAppVersion: version,
  supportFloor: "0.2.0",
});
const snapshot = (change: Partial<PolicySnapshot> = {}): PolicySnapshot => ({
  currentAppVersion: "0.3.0",
  server: policy("0.2.0"),
  local: policy("0.2.0"),
  localModified: false,
  revision: "5",
  issue: null,
  availableVersions: ["0.3.0"],
  ...change,
});
const props = () => ({
  root: "owned",
  canSave: true,
  close: vi.fn(),
  commit: vi.fn(),
  openProject: vi.fn(),
  home: vi.fn(),
  update: vi.fn(),
});
it("distinguishes a local policy save from a server commit", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockResolvedValue(snapshot());
  const save = vi
    .spyOn(svnClient, "policySave")
    .mockResolvedValue(
      snapshot({ local: policy("0.3.0"), localModified: true }),
    );
  const p = props();
  render(<CollaborationPolicyDialog {...p} />);
  await screen.findByRole("button", { name: text("policy.save") });
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("policy.save") }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: text("policy.save") }));
  await screen.findByText(text("policy.uncommitted"));
  expect(save).toHaveBeenCalledWith("owned", "0.3.0", "5");
  expect(within(screen.getByRole("table")).getAllByText("0.3.0")).toHaveLength(
    2,
  );
  expect(p.commit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: text("policy.commit") }));
  expect(p.commit).toHaveBeenCalledOnce();
});
it("does not label an unchanged stale working copy as an uncommitted edit", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockResolvedValue(
    snapshot({
      server: policy("0.3.0"),
      local: policy("0.2.0"),
      localModified: false,
      currentAppVersion: "0.2.0",
      issue: "svn_policy_app_too_old:0.3.0",
    }),
  );
  render(<CollaborationPolicyDialog {...props()} />);
  await screen.findByText(text("policy.outdated"));
  expect(screen.queryByText(text("policy.uncommitted"))).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("policy.commit") }),
  ).toBeNull();
  expect(within(screen.getByRole("table")).getAllByText("0.2.0")).toHaveLength(
    2,
  );
});
it("offers explicit policy-only initialization before the project opens", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockResolvedValue(
    snapshot({ server: null, local: null, issue: "svn_policy_missing" }),
  );
  const initialize = vi
    .spyOn(svnClient, "policyInitialize")
    .mockResolvedValue(
      snapshot({ server: policy("0.3.0"), local: policy("0.3.0") }),
    );
  render(<CollaborationPolicyDialog {...props()} canSave={false} />);
  const button = await screen.findByRole("button", {
    name: text("policy.initialize"),
  });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await screen.findByRole("button", { name: text("policy.open") });
  expect(initialize).toHaveBeenCalledWith("owned", expect.any(String), "0.3.0");
});
it("a low app keeps home/update reachable and offers no save/initialize bypass", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockResolvedValue(
    snapshot({
      server: policy("0.4.0"),
      issue: "svn_policy_app_too_old:0.4.0",
    }),
  );
  const p = props();
  render(<CollaborationPolicyDialog {...p} />);
  await screen.findByText(text("policy.tooOld"));
  expect(
    screen.queryByRole("button", { name: text("policy.save") }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("policy.initialize") }),
  ).toBeNull();
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("policy.update") }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: text("policy.update") }));
  expect(p.update).toHaveBeenCalledOnce();
});
it("late responses from a replaced project cannot overwrite the new owner", async () => {
  let resolve: (value: PolicySnapshot) => void = () => {};
  vi.spyOn(svnClient, "policySnapshot").mockImplementation((path) =>
    path === "old"
      ? new Promise((r) => {
          resolve = r;
        })
      : Promise.resolve(snapshot({ currentAppVersion: "0.5.0" })),
  );
  const p = props();
  const view = render(<CollaborationPolicyDialog {...p} root="old" />);
  view.rerender(<CollaborationPolicyDialog {...p} root="new" />);
  await screen.findByText("0.5.0");
  resolve(snapshot({ currentAppVersion: "0.1.0" }));
  await waitFor(() => expect(screen.queryByText("0.1.0")).toBeNull());
});

it("server failure keeps home/update and installed version without claiming an applied policy", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockRejectedValue(
    "svn_connection_failed",
  );
  const p = props();
  render(
    <CollaborationPolicyDialog {...p} canSave={false} currentVersion="0.3.0" />,
  );
  await screen.findByText("0.3.0");
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("policy.home") }),
    ).toBeEnabled(),
  );
  expect(screen.queryByText(text("policy.applied"))).toBeNull();
  expect(screen.queryByText(text("policy.same"))).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("policy.initialize") }),
  ).toBeNull();
  expect(
    screen.getByRole("button", { name: text("policy.update") }),
  ).toBeEnabled();
});

it("a previous project's lookup failure cannot label a successful new project as unconfirmed", async () => {
  vi.spyOn(svnClient, "policySnapshot").mockImplementation((root) =>
    root === "old"
      ? Promise.reject("svn_connection_failed")
      : Promise.resolve(snapshot({ currentAppVersion: "0.5.0" })),
  );
  const view = render(
    <CollaborationPolicyDialog
      {...props()}
      root="old"
      currentVersion="0.3.0"
    />,
  );
  await screen.findAllByText(text("policy.unconfirmed"));
  view.rerender(
    <CollaborationPolicyDialog
      {...props()}
      root="new"
      currentVersion="0.5.0"
    />,
  );
  await screen.findByText("0.5.0");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText(text("policy.applied"))).toBeVisible();
});
