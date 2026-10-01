import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { render } from "../test/render";
import { text } from "../strings";
import { UpdateDialog } from "./UpdateDialog";
import { UpdateController, type UpdateStatus } from "./updaterClient";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
beforeEach(() => vi.mocked(invoke).mockReset());

it("FIX002 regression: cancel racing with native download completion still offers a retry", async () => {
  const status: UpdateStatus = {
    revision: 1,
    testMode: false,
    distribution: "github",
    currentVersion: "0.2.0",
    channel: "stable",
    phase: "available",
    candidate: "candidate-1",
    version: "1.0.0",
    notes: "",
    downloaded: 0,
    total: null,
    error: null,
    notify: true,
    startupComplete: false,
    releaseUrl: null,
    handoff: null,
  };
  vi.mocked(invoke).mockResolvedValue(status);
  const c = new UpdateController();
  await c.initialize();
  let finish: (value: UpdateStatus) => void = () => {};
  vi.mocked(invoke).mockImplementation((command) => {
    if (command === "updater_download")
      return new Promise((resolve) => {
        finish = resolve as typeof finish;
      });
    // Native completes first. updater_cancel rejects because phase is now ready.
    if (command === "updater_cancel") return Promise.reject("target");
    return Promise.resolve({
      ...status,
      revision: 4,
      phase: "ready",
      downloaded: 1024,
      total: 1024,
    });
  });
  const flow = c.update();
  await c.cancel();
  finish({
    ...status,
    revision: 4,
    phase: "ready",
    downloaded: 1024,
    total: 1024,
  });
  await flow;
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
  expect(c.snapshot()?.phase).toBe("ready");
  render(<UpdateDialog controller={c} open onClose={vi.fn()} />);
  expect(
    screen.queryByRole("button", { name: text("update.retry") }) ??
      screen.queryByRole("button", { name: text("update.install") }),
  ).not.toBeNull();
  vi.mocked(invoke).mockImplementation((command) =>
    Promise.resolve({
      ...status,
      revision: command === "updater_prepare" ? 6 : 5,
      phase: command === "updater_prepare" ? "preparing" : "ready",
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: text("update.install") }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("updater_prepare", {
      candidate: "candidate-1",
    }),
  );
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_prepare"),
  ).toHaveLength(1);
});

function available(): UpdateStatus {
  return {
    revision: 1,
    testMode: true,
    distribution: "dev",
    currentVersion: "0.2.0",
    channel: "test",
    phase: "available",
    candidate: "candidate-1",
    version: "0.3.0",
    notes: "",
    downloaded: 0,
    total: null,
    error: null,
    notify: true,
    startupComplete: false,
    releaseUrl: null,
    handoff: null,
  };
}

it("cancel first and late completion cannot consume a new explicit flow or prepare twice", async () => {
  const initial = available();
  vi.mocked(invoke).mockResolvedValue(initial);
  const controller = new UpdateController();
  await controller.initialize();
  const completions: ((value: UpdateStatus) => void)[] = [];
  let current = initial;
  vi.mocked(invoke).mockImplementation((command) => {
    if (command === "updater_download")
      return new Promise((resolve) =>
        completions.push(resolve as (value: UpdateStatus) => void),
      );
    if (command === "updater_cancel")
      current = {
        ...initial,
        revision: 3,
        phase: "cancelled",
        error: "cancelled",
      };
    if (command === "updater_prepare")
      current = { ...current, revision: 7, phase: "preparing" };
    return Promise.resolve(current);
  });
  const oldFlow = controller.update();
  await controller.cancel();
  render(<UpdateDialog controller={controller} open onClose={vi.fn()} />);
  expect(
    screen.getByRole("button", { name: text("update.retry") }),
  ).toBeEnabled();
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
  const newFlow = controller.update();
  await act(async () => {
    completions[0]({ ...initial, revision: 4, phase: "ready" });
    await oldFlow;
  });
  expect(controller.snapshot()?.phase).toBe("cancelled");
  expect(controller.update()).toBe(newFlow);
  expect(completions).toHaveLength(2);
  await act(async () => {
    current = { ...initial, revision: 6, phase: "ready" };
    completions[1](current);
    await newFlow;
  });
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_prepare"),
  ).toEqual([["updater_prepare", { candidate: "candidate-1" }]]);
});

it("replacement candidate during cancel ignores a late success with a newer revision", async () => {
  const initial = available();
  vi.mocked(invoke).mockResolvedValue(initial);
  const controller = new UpdateController();
  await controller.initialize();
  let finish: (value: UpdateStatus) => void = () => {};
  const next = {
    ...initial,
    revision: 10,
    candidate: "candidate-2",
    phase: "ready" as const,
  };
  vi.mocked(invoke).mockImplementation((command) => {
    if (command === "updater_download")
      return new Promise((resolve) => {
        finish = resolve as typeof finish;
      });
    if (command === "updater_cancel") return Promise.reject("target");
    return Promise.resolve(next);
  });
  const oldFlow = controller.update();
  await controller.cancel();
  finish({ ...initial, revision: 11, phase: "ready" });
  await oldFlow;
  expect(controller.snapshot()?.candidate).toBe("candidate-2");
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
  vi.mocked(invoke).mockImplementation((command) =>
    Promise.resolve({
      ...next,
      revision: command === "updater_prepare" ? 13 : 12,
      phase: command === "updater_prepare" ? "preparing" : "ready",
    }),
  );
  await controller.update();
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_prepare"),
  ).toEqual([["updater_prepare", { candidate: "candidate-2" }]]);
});

it("duplicate cancel coalesces and ready re-consent stays disabled until cancellation settles", async () => {
  const initial = available();
  vi.mocked(invoke).mockResolvedValue(initial);
  const controller = new UpdateController();
  await controller.initialize();
  let finish: (value: UpdateStatus) => void = () => {};
  let finishCancel: (value: UpdateStatus) => void = () => {};
  vi.mocked(invoke).mockImplementation((command) => {
    if (command === "updater_download")
      return new Promise((resolve) => {
        finish = resolve as typeof finish;
      });
    if (command === "updater_cancel")
      return new Promise((resolve) => {
        finishCancel = resolve as typeof finishCancel;
      });
    return Promise.resolve({ ...initial, revision: 4, phase: "ready" });
  });
  const flow = controller.update();
  const cancel = controller.cancel();
  expect(controller.cancel()).toBe(cancel);
  render(<UpdateDialog controller={controller} open onClose={vi.fn()} />);
  await waitFor(() => expect(controller.snapshot()?.phase).toBe("ready"));
  expect(
    screen.getByRole("button", { name: text("update.install") }),
  ).toBeDisabled();
  await controller.update();
  expect(invoke).not.toHaveBeenCalledWith("updater_prepare", expect.anything());
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_cancel"),
  ).toHaveLength(1);
  await act(async () => {
    finishCancel({
      ...initial,
      revision: 5,
      phase: "cancelled",
      error: "cancelled",
    });
    await cancel;
    finish({ ...initial, revision: 4, phase: "ready" });
    await flow;
  });
  expect(
    screen.getByRole("button", { name: text("update.retry") }),
  ).toBeEnabled();
  vi.mocked(invoke).mockImplementation((command) =>
    Promise.resolve({
      ...initial,
      revision: command === "updater_prepare" ? 7 : 6,
      phase: command === "updater_prepare" ? "preparing" : "ready",
    }),
  );
  await act(() => controller.update());
  expect(
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "updater_prepare"),
  ).toHaveLength(1);
});
