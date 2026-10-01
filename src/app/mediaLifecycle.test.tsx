import { afterEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { DocumentController } from "./documentController";
import { TemplateController } from "./controller";
import { WorkspaceController } from "./workspaceController";
import { TestTransport } from "./testTransport";
import { GuardedClient } from "../bridge/client";
import type { Command } from "../bridge/types";
import { MediaActiveContext, UrlRead, mediaKind } from "./MediaValue";
import { youtubeConsentStore, YOUTUBE_CONSENT_KEY } from "./youtubeConsent";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  localStorage.removeItem(YOUTUBE_CONSENT_KEY);
  youtubeConsentStore().refresh();
});
async function fixture() {
  const transport = new TestTransport();
  transport.workspaceResult = (input) =>
    input.kind !== "document_workspace"
      ? undefined
      : {
          kind: "document_workspace",
          value:
            input.request.action === "list"
              ? {
                  kind: "list",
                  fingerprint: "preview-race",
                  snapshot: "s",
                  initial: true,
                  unplaced: [],
                  problem: null,
                  documents: [],
                  layout: { revision: 1, rootOrder: [], nodes: {} },
                }
              : { kind: "asset_done" },
        };
  const shell = new TemplateController(new GuardedClient(transport));
  shell.start();
  await waitFor(() => expect(shell.snapshot().ready).toBe(true));
  shell.setRoot("fixture");
  await shell.open();
  const controller = new DocumentController(shell);
  await controller.load();
  return { controller, shell, transport };
}

it("completes normal preview and rejects close-before-reserve without allocating", async () => {
  const { controller, shell, transport } = await fixture();
  await expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).resolves.toEqual({ kind: "asset_done" });
  expect(shell.operations.notices()).toEqual([]);
  controller.suspendPreviews();
  const count = transport.commands.length;
  await expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).rejects.toThrow();
  expect(transport.commands).toHaveLength(count);
  controller.resumePreviews();
  await expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).resolves.toEqual({ kind: "asset_done" });
});

it.each([false, true])(
  "ends a delayed reservation after close, including cancel/reopen=%s",
  async (resume) => {
    const { controller, shell, transport } = await fixture();
    let release = () => {};
    transport.gateReserve = new Promise<void>((r) => {
      release = r;
    });
    const pending = controller.media({ action: "asset_read", asset: "sample" });
    const rejected = expect(pending).rejects.toThrow();
    await waitFor(() =>
      expect(transport.commands[transport.commands.length - 1]?.action).toBe(
        "reserve",
      ),
    );
    controller.suspendPreviews();
    if (resume) controller.resumePreviews();
    const before = transport.commands.length;
    release();
    await rejected;
    expect(
      transport.commands.slice(before).some((c) => c.action === "submit"),
    ).toBe(false);
    const abandoned = [...transport.commands]
      .reverse()
      .find((c) => c.action === "abandon_reservation");
    expect(abandoned?.action).toBe("abandon_reservation");
    expect(shell.operations.notices()).toEqual([]);
    if (abandoned?.action === "abandon_reservation") {
      expect(shell.client.retainedInput(abandoned.operation)).toBeUndefined();
      await expect(
        shell.client.result(abandoned.operation),
      ).rejects.toMatchObject({ boundary: { code: "unknown_id" } });
    }
  },
);

it("consumes pre-admission closed rather than retaining an ordinary reservation", async () => {
  const { controller, shell, transport } = await fixture();
  const original = transport.invoke.bind(transport);
  vi.spyOn(transport, "invoke").mockImplementation(async (command, body) => {
    const c = JSON.parse(new TextDecoder().decode(body)) as Command;
    if (c.action === "submit" && c.input.kind === "document_workspace")
      throw { code: "closed", nextAction: "" };
    return original(command, body);
  });
  await expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).rejects.toThrow();
  expect(shell.operations.notices()).toEqual([]);
  expect(
    transport.commands.some((c) => c.action === "abandon_reservation"),
  ).toBe(true);
});

it("confirms a lost abandon reply without leaking its reservation or promise", async () => {
  const { controller, shell, transport } = await fixture();
  let release = () => {};
  transport.gateReserve = new Promise<void>((r) => {
    release = r;
  });
  const original = transport.invoke.bind(transport);
  let lost = false;
  vi.spyOn(transport, "invoke").mockImplementation(async (command, body) => {
    const c = JSON.parse(new TextDecoder().decode(body)) as Command;
    const result = await original(command, body);
    if (c.action === "abandon_reservation" && !lost) {
      lost = true;
      throw new Error("lost abandon reply");
    }
    return result;
  });
  const rejected = expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).rejects.toThrow();
  controller.suspendPreviews();
  release();
  await rejected;
  expect(lost).toBe(true);
  expect(shell.operations.notices()).toEqual([]);
});

it("never enrolls an editing-owner request in preview abandonment", async () => {
  const { shell, transport } = await fixture();
  const count = transport.commands.length;
  await expect(
    shell.operations.run(
      {
        kind: "document_workspace",
        project: shell.snapshot().projectId!,
        request: { action: "edit_release", owner: "owner", generation: "2" },
      },
      "edit",
      "ordinary",
      undefined,
      undefined,
      () => false,
    ),
  ).rejects.toThrow();
  expect(transport.commands).toHaveLength(count);
});

it.each([false, true])(
  "accepted preview preserves terminal/ack across close, lost-submit=%s",
  async (lost) => {
    const { controller, shell, transport } = await fixture();
    transport.hold = "document_workspace";
    const original = transport.invoke.bind(transport);
    vi.spyOn(transport, "invoke").mockImplementation(async (command, body) => {
      const c = JSON.parse(new TextDecoder().decode(body)) as Command;
      const result = await original(command, body);
      if (
        lost &&
        c.action === "submit" &&
        c.input.kind === "document_workspace"
      )
        throw new Error("lost response");
      return result;
    });
    const pending = controller.media({
      action: "asset_chunk",
      asset: "sample",
      digest: "digest",
      offset: 0,
    });
    const rejected = expect(pending).rejects.toThrow();
    await waitFor(() =>
      expect(transport.commands[transport.commands.length - 1]?.action).toBe(
        "operation",
      ),
    );
    controller.suspendPreviews();
    transport.completeHeld();
    await shell.operations.queryAll();
    await rejected;
    expect(shell.operations.notices()).toEqual([]);
    expect(
      transport.commands.some((c) => c.action === "acknowledge_transport"),
    ).toBe(true);
  },
);

it("workspace closes previews before releasing an editing owner and resumes on cancellation", async () => {
  const { shell } = await fixture();
  const workspace = new WorkspaceController(shell);
  await workspace.documents.load();
  vi.spyOn(workspace.documents, "hasOwners").mockReturnValue(true);
  vi.spyOn(workspace.documents, "requestClose").mockImplementation(
    (_action, cancel) => {
      expect(workspace.documents.snapshot().previewClosing).toBe(true);
      cancel?.();
      return false;
    },
  );
  await workspace.navigate({ kind: "close_project" });
  expect(workspace.documents.snapshot().previewClosing).toBe(false);
  await expect(
    workspace.documents.media({ action: "asset_read", asset: "sample" }),
  ).resolves.toEqual({ kind: "asset_done" });
});

it.each(["png", "mp4"])(
  "A failure/B success/A success uses a new %s request and ignores old events",
  (ext) => {
    const a = `https://example.com/a.${ext}`,
      b = `https://example.com/b.${ext}`;
    const selector = ext === "png" ? "img" : "video";
    const { container, rerender } = render(<UrlRead value={a} />);
    const old = container.querySelector(selector)!;
    fireEvent.error(old);
    expect(screen.getByRole("status")).toBeInTheDocument();
    rerender(<UrlRead value={b} />);
    const middle = container.querySelector(selector)!;
    if (ext === "png") fireEvent.load(middle);
    else fireEvent.canPlay(middle);
    expect(screen.queryByRole("status")).toBeNull();
    rerender(<UrlRead value={a} />);
    const current = container.querySelector(selector)!;
    expect(current).not.toBe(old);
    fireEvent.error(current);
    fireEvent.load(old);
    fireEvent.canPlay(old);
    expect(screen.getByRole("status")).toBeInTheDocument();
    if (ext === "png") fireEvent.load(current);
    else fireEvent.canPlay(current);
    fireEvent.error(old);
    expect(screen.queryByRole("status")).toBeNull();
  },
);

it.each(["https://example.com/a.mp4", "https://youtu.be/M7lc1UVf-VE?t=5"])(
  "unmounts only the hidden player and returns without autoplay: %s",
  (value) => {
    youtubeConsentStore().choose(true);
    const view = (active: boolean) => (
      <MediaActiveContext.Provider value={active}>
        <input defaultValue="raw" />
        <UrlRead value={value} />
      </MediaActiveContext.Provider>
    );
    const { container, rerender } = render(view(true));
    const input = container.querySelector("input");
    const player = container.querySelector("iframe,video");
    rerender(view(false));
    expect(container.querySelector("iframe,video")).toBeNull();
    expect(container.querySelector("input")).toBe(input);
    rerender(view(true));
    expect(container.querySelector("iframe,video")).not.toBe(player);
    expect(container.querySelector("video")?.autoplay ?? false).toBe(false);
  },
);

it("matches native rejection of empty YouTube paths, fragments and signed times", () => {
  for (const url of [
    "https://youtu.be",
    "https://youtu.be/",
    "https://youtu.be///?t=5",
    "https://youtu.be/invalid",
    "https://youtu.be/M7lc1UVf-VE?t=%2B5",
    "https://example.com/a.png#",
    "https://youtu.be/M7lc1UVf-VE?t=86401",
  ])
    expect(mediaKind(url)).toBeNull();
  expect(mediaKind("https://youtu.be/M7lc1UVf-VE?t=86400")).toBe("youtube");
});

it("preserves static image loading state while the editing screen is hidden", () => {
  const view = (active: boolean) => (
    <MediaActiveContext.Provider value={active}>
      <UrlRead value="https://example.com/a.png" />
    </MediaActiveContext.Provider>
  );
  const { container, rerender } = render(view(true));
  const image = container.querySelector("img");
  rerender(view(false));
  expect(container.querySelector("img")).toBe(image);
});
