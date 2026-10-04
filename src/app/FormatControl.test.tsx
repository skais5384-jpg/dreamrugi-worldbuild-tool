import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FormatControl } from "./FormatControl";
import type { TemplateController } from "./controller";
import { text } from "../strings";
import type { Template } from "../bridge/types";

it("finishes historical attachment inspection across operation notifications without repeating requests", async () => {
  let state = { projectId: "owned-project", notification: 0 };
  const listeners = new Set<() => void>();
  let finish!: (value: unknown) => void;
  const assetResult = new Promise((resolve) => {
    finish = resolve;
  });
  const template = {
    id: "owned-template",
    name: "합성 템플릿",
    lifecycle: "Active",
    fieldOrder: ["file"],
    fields: [
      {
        id: "file",
        label: "첨부",
        kind: "File",
        lifecycle: "Active",
        required: false,
        default: { kind: "file", value: ["owned-asset"] },
        options: [],
        optionOrder: [],
      },
    ],
  } as unknown as Template;
  const run = vi.fn(async (input: { request: { action: string } }) => {
    const action = input.request.action;
    if (action === "asset_read") return assetResult;
    return {
      result: {
        kind: "document_workspace",
        value:
          action === "versions_list"
            ? {
                kind: "versions",
                versions: [
                  {
                    version: "7",
                    recorded_at_utc: "2026-10-04T00:00:00Z",
                    available: true,
                  },
                ],
              }
            : {
                kind: "version_preview",
                version: "7",
                source: "owned-source",
                asset_names: { "owned-asset": "합성 자료.txt" },
                template,
                document: null,
              },
      },
    };
  });
  const shell = {
    snapshot: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    projectGeneration: () => 1,
    operations: { run, queryAll: vi.fn(async () => {}) },
  } as unknown as TemplateController;
  render(
    <FormatControl
      shell={shell}
      kind="template"
      artifact="owned-template"
      locked={false}
      changed={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: text("format.title") }));
  await waitFor(() =>
    expect(
      run.mock.calls.filter(([input]) => input.request.action === "asset_read"),
    ).toHaveLength(1),
  );
  act(() => {
    state = { ...state, notification: 1 };
    listeners.forEach((listener) => listener());
  });
  await act(async () =>
    finish({
      result: {
        kind: "document_workspace",
        value: {
          kind: "asset_error",
          assetName: null,
          assetState: "missing",
          error: { category: "not_found", stage: "asset_read", nextAction: "" },
        },
      },
    }),
  );
  expect(await screen.findByText(text("media.state.missing"))).toBeTruthy();
  expect(screen.getByRole("alert")).toHaveTextContent("합성 자료.txt");
  expect(
    run.mock.calls.filter(([input]) => input.request.action === "asset_read"),
  ).toHaveLength(1);
  expect(
    run.mock.calls.find(
      ([input]) => input.request.action === "asset_read",
    )?.[0],
  ).toMatchObject({
    request: {
      action: "asset_read",
      asset: "owned-asset",
      target: { kind: "template", artifact: "owned-template" },
    },
  });
});

describe("content versions project boundary", () => {
  it("ignores the previous project's delayed list even when both projects share an artifact ID", async () => {
    let state = { projectId: "first-project" };
    let generation = 1;
    const listeners = new Set<() => void>();
    let finish!: (value: unknown) => void;
    const result = new Promise((resolve) => {
      finish = resolve;
    });
    const run = vi.fn((_request: unknown, _label: string) => result);
    const changed = vi.fn(async () => {});
    const shell = {
      snapshot: () => state,
      subscribe: (listener: () => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      projectGeneration: () => generation,
      operations: { run, queryAll: vi.fn(async () => {}) },
    } as unknown as TemplateController;
    render(
      <FormatControl
        shell={shell}
        kind="document"
        artifact="shared-artifact"
        locked={false}
        changed={changed}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: text("format.title") }));
    await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    expect(run.mock.calls[0][0]).toMatchObject({ project: "first-project" });
    act(() => {
      state = { projectId: "second-project" };
      ++generation;
      listeners.forEach((listener) => listener());
    });
    await act(async () =>
      finish({
        result: {
          kind: "document_workspace",
          value: {
            kind: "versions",
            versions: [
              {
                version: "99",
                recorded_at_utc: "2026-10-04T00:00:00.000Z",
                available: true,
              },
            ],
          },
        },
      }),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText("99")).toBeNull();
    expect(changed).not.toHaveBeenCalled();
    expect(run).toHaveBeenCalledTimes(1);
  });
});

it("explains backup recovery when all retained versions are unreadable and never offers restoration", async () => {
  const state = { projectId: "owned-project" };
  const run = vi.fn(async () => ({
    result: {
      kind: "document_workspace",
      value: {
        kind: "versions",
        versions: [{ version: "1", recorded_at_utc: "", available: false }],
      },
    },
  }));
  const shell = {
    snapshot: () => state,
    subscribe: () => () => {},
    projectGeneration: () => 1,
    operations: { run, queryAll: vi.fn(async () => {}) },
  } as unknown as TemplateController;
  render(
    <FormatControl
      shell={shell}
      kind="template"
      artifact="owned-template"
      locked={false}
      changed={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: text("format.title") }));
  await screen.findByText(text("format.backupNeeded"));
  expect(
    screen.getByRole("button", { name: text("format.restore") }),
  ).toBeDisabled();
  expect(run).toHaveBeenCalledTimes(1);
});
