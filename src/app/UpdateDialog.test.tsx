import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { render } from "../test/render";
import { text } from "../strings";
import { UpdateDialog } from "./UpdateDialog";
import { UpdateController, type UpdateStatus } from "./updaterClient";
const events = vi.hoisted(() => ({
  deliver: null as ((event: { payload: UpdateStatus }) => void) | null,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => {
    events.deliver = callback;
    return () => {};
  }),
}));
let status: UpdateStatus;
beforeEach(() => {
  status = {
    revision: 1,
    testMode: false,
    distribution: "github",
    currentVersion: "0.2.0",
    channel: "stable",
    phase: "available",
    candidate: "candidate-1",
    version: "1.0.0",
    notes: "긴 한글 변경 내용\n<script>external code</script>",
    downloaded: 0,
    total: null,
    error: null,
    notify: true,
    startupComplete: false,
    releaseUrl:
      "https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool/releases/tag/v1.0.0",
    handoff: null,
  };
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "updater_download")
      status = { ...status, revision: status.revision + 1, phase: "ready" };
    if (command === "updater_prepare")
      status = { ...status, revision: status.revision + 1, phase: "preparing" };
    if (command === "updater_continue")
      status = {
        ...status,
        revision: status.revision + 1,
        phase: "skipped",
        startupComplete: true,
        notify: false,
      };
    return status;
  });
});
it("one consent downloads and prepares, without a second install choice or direct plugin invocation", async () => {
  const controller = new UpdateController();
  await controller.initialize();
  render(<UpdateDialog controller={controller} open onClose={vi.fn()} />);
  expect(screen.getByText(/<script>external code<\/script>/u)).toBeVisible();
  expect(document.querySelector("script")).toBeNull();
  expect(screen.getByText(text("update.protect"))).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: text("update.install") }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("updater_prepare", {
      candidate: "candidate-1",
    }),
  );
  const calls = vi.mocked(invoke).mock.calls.map(([c]) => c);
  expect(calls.indexOf("updater_download")).toBeLessThan(
    calls.indexOf("updater_prepare"),
  );
  expect(
    calls.some((c) => c.includes("plugin:updater") || c === "updater_install"),
  ).toBe(false);
});
it("later releases only this startup and never stores a deferral or install consent", async () => {
  const c = new UpdateController();
  await c.initialize();
  render(<UpdateDialog controller={c} open onClose={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: text("update.later") }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("updater_continue"));
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
  expect(
    vi.mocked(invoke).mock.calls.some(([c]) => c === "updater_defer"),
  ).toBe(false);
  expect(screen.getAllByRole("button").slice(-1)[0]).toHaveTextContent(
    text("update.close"),
  );
});
it("signature rejection shows no retry or installer and passes only the native candidate to official link", async () => {
  status.phase = "failed";
  status.error = "signature";
  const c = new UpdateController();
  await c.initialize();
  render(<UpdateDialog controller={c} open onClose={vi.fn()} />);
  expect(screen.getByText(text("update.signature"))).toBeVisible();
  expect(
    screen.queryByRole("button", { name: text("update.retry") }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("update.install") }),
  ).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: text("update.release") }));
  expect(invoke).toHaveBeenCalledWith("updater_release", {
    candidate: "candidate-1",
  });
});
it("late download completion after continue cannot prepare or replace its newer status", async () => {
  const c = new UpdateController();
  await c.initialize();
  let finish: (s: UpdateStatus) => void = () => {};
  vi.mocked(invoke).mockImplementation((command) =>
    command === "updater_download"
      ? new Promise((resolve) => {
          finish = resolve as typeof finish;
        })
      : Promise.resolve({
          ...status,
          revision: 20,
          phase: "skipped",
          startupComplete: true,
        }),
  );
  const flow = c.update();
  await c.continue();
  finish({ ...status, revision: 5, phase: "ready" });
  await flow;
  expect(c.snapshot()?.phase).toBe("skipped");
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
});
it("startup check coalesces, waits for choice, and never checks again after personal continuation", async () => {
  const c = new UpdateController();
  let released = false;
  const first = c.start().then(() => {
    released = true;
  });
  c.start();
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("updater_check", { retry: false }),
  );
  expect(released).toBe(false);
  await c.continue();
  await first;
  await c.start();
  await c.retry();
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_check"),
  ).toHaveLength(1);
});
it.each(["dev", "store"] as const)(
  "%s has no operating query at startup or later details access",
  async (distribution) => {
    status.distribution = distribution;
    status.phase = "unavailable";
    const c = new UpdateController();
    await c.start();
    await c.refresh();
    expect(
      vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_check"),
    ).toHaveLength(0);
    expect(c.snapshot()?.startupComplete).toBe(true);
  },
);
it("Store help keeps its channel guidance after startup continues from idle", async () => {
  status = {
    ...status,
    distribution: "store",
    currentVersion: "1.0.0",
    phase: "idle",
    candidate: null,
    version: null,
    notes: "",
    releaseUrl: null,
    notify: false,
  };
  const controller = new UpdateController();
  await controller.start();
  expect(controller.snapshot()?.phase).toBe("skipped");
  render(<UpdateDialog controller={controller} open onClose={vi.fn()} />);
  expect(screen.getByText("1.0.0")).toBeVisible();
  expect(screen.getByText(text("update.store"))).toBeVisible();
  expect(screen.queryByText(text("update.skipped"))).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("update.install") }),
  ).toBeNull();
  expect(
    screen.queryByRole("button", { name: text("update.release") }),
  ).toBeNull();
  expect(
    vi
      .mocked(invoke)
      .mock.calls.some(([command]) =>
        ["updater_check", "updater_download", "updater_prepare"].includes(
          command,
        ),
      ),
  ).toBe(false);
});
it("duplicate consent coalesces and cancel prevents late ready from preparing", async () => {
  const c = new UpdateController();
  await c.initialize();
  let finish: (s: UpdateStatus) => void = () => {};
  vi.mocked(invoke).mockImplementation((command) =>
    command === "updater_download"
      ? new Promise((resolve) => {
          finish = resolve as typeof finish;
        })
      : Promise.resolve({ ...status, revision: 10, phase: "cancelled" }),
  );
  const a = c.update(),
    b = c.update();
  await c.cancel();
  finish({ ...status, revision: 4, phase: "ready" });
  await Promise.all([a, b]);
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_download"),
  ).toHaveLength(1);
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
});
it("known and unknown totals show actual bytes and a separate cancel button", async () => {
  status.phase = "downloading";
  status.downloaded = 1048576;
  const c = new UpdateController();
  await c.initialize();
  render(<UpdateDialog controller={c} open onClose={vi.fn()} />);
  expect(screen.getByText("1 MB")).toBeVisible();
  expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
  act(() =>
    events.deliver?.({ payload: { ...status, revision: 10, total: 2097152 } }),
  );
  expect(screen.getByText("1 MB / 2 MB")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: text("update.cancel") }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("updater_cancel", {
      candidate: "candidate-1",
    }),
  );
});
it.each(["preparing", "installing"] as const)(
  "%s cannot dismiss the handoff dialog with Escape",
  async (phase) => {
    status.phase = phase;
    const c = new UpdateController();
    await c.initialize();
    const onClose = vi.fn();
    render(<UpdateDialog controller={c} open onClose={onClose} />);
    expect(
      screen.getByRole("button", { name: text("update.close") }),
    ).toBeDisabled();
    fireEvent.keyDown(screen.getByRole("dialog"), {
      key: "Escape",
      code: "Escape",
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("updater_continue");
  },
);
