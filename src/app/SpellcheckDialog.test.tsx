import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { render } from "../test/render";
import { SpellcheckDialog } from "./SpellcheckDialog";
import type { DocumentController } from "./documentController";
import type { EditEntry } from "./documentEdits";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => vi.mocked(invoke).mockReset());

function editableName(text: string) {
  const entry = {
    status: {
      owner: "owner",
      read: {
        name: text,
        glossarySummary: "",
        template: { fields: [] },
        fields: [],
      },
      editable: [],
    },
    body: {
      name: { intent: "set", value: text },
      fields: [],
      composing: false,
    },
    generation: "1",
    busy: false,
    paused: false,
  } as unknown as EditEntry;
  const shell = {
    snapshot: () => ({ projectId: "project", rows: [], listState: "ready" }),
    projectGeneration: () => 1,
    refresh: vi.fn().mockResolvedValue(undefined),
  };
  const controller = {
    shell,
    load: vi.fn().mockResolvedValue(undefined),
    snapshot: () => ({
      list: { problem: null, layout: { nodes: {} }, documents: [] },
    }),
    edits: {
      entries: { doc: entry },
      update: vi.fn(
        (
          _id: string,
          update: (body: EditEntry["body"]) => EditEntry["body"],
        ) => {
          entry.body = update(entry.body);
          entry.generation = String(Number(entry.generation) + 1);
        },
      ),
    },
  } as unknown as DocumentController;
  return { entry, controller };
}

it("checks the current draft on demand and applies one selected correction through autosave draft update", async () => {
  const entry = {
    status: {
      owner: "owner",
      document: "doc",
      generation: "1",
      saved_generation: "1",
      read: {
        kind: "read",
        id: "doc",
        name: "저장된 이름",
        englishName: "",
        glossarySummary: "",
        glossaryExcluded: false,
        template: { id: "template", fields: [], name: "시험" },
        fields: [],
        warnings: [],
      },
      editable: [],
      deposited: false,
    },
    body: {
      name: { intent: "set", value: "좋읍니다" },
      fields: [],
      composing: false,
    },
    generation: "1",
    busy: false,
    paused: false,
    error: null,
  } as unknown as EditEntry;
  const project = { projectId: "project", rows: [], listState: "ready" };
  const shell = {
    snapshot: () => project,
    projectGeneration: () => 1,
    refresh: vi.fn().mockResolvedValue(undefined),
  };
  const controller = {
    shell,
    load: vi.fn().mockResolvedValue(undefined),
    snapshot: () => ({
      list: { problem: null, layout: { nodes: {} }, documents: [] },
    }),
    edits: {
      entries: { doc: entry },
      update: vi.fn(
        (
          _id: string,
          update: (body: EditEntry["body"]) => EditEntry["body"],
        ) => {
          entry.body = update(entry.body);
          entry.generation = "2";
        },
      ),
    },
  } as unknown as DocumentController;
  vi.mocked(invoke).mockResolvedValueOnce([
    { word: "좋읍니다", valid: false, suggestions: ["좋습니다"] },
  ]);
  render(
    <SpellcheckDialog
      controller={controller}
      document="doc"
      entry={entry}
      locked={false}
      close={vi.fn()}
    />,
  );
  expect(await screen.findByText("좋읍니다")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "적용" }));
  await waitFor(() =>
    expect(entry.body.name).toEqual({ intent: "set", value: "좋습니다" }),
  );
  expect(screen.getByText(/1건을 초안에 적용했습니다/u)).toBeVisible();
});

it("shows a safe native spell error instead of hiding it behind a generic retry message", async () => {
  const entry = {
    status: {
      owner: "owner",
      read: {
        name: "좋읍니다",
        glossarySummary: "",
        template: { fields: [] },
        fields: [],
      },
      editable: [],
    },
    body: {
      name: { intent: "set", value: "좋읍니다" },
      fields: [],
      composing: false,
    },
    paused: false,
  } as unknown as EditEntry;
  const shell = {
    snapshot: () => ({ projectId: "project", rows: [], listState: "ready" }),
    projectGeneration: () => 1,
    refresh: vi.fn().mockResolvedValue(undefined),
  };
  const controller = {
    shell,
    load: vi.fn().mockResolvedValue(undefined),
    snapshot: () => ({
      list: { problem: null, layout: { nodes: {} }, documents: [] },
    }),
    edits: { entries: { doc: entry } },
  } as unknown as DocumentController;
  vi.mocked(invoke).mockRejectedValueOnce("검사 범위가 올바르지 않습니다.");
  render(
    <SpellcheckDialog
      controller={controller}
      document="doc"
      entry={entry}
      locked={false}
      close={vi.fn()}
    />,
  );
  expect(
    await screen.findByText("검사 범위가 올바르지 않습니다."),
  ).toBeVisible();
});

it("keeps later cards actionable after a shorter individual replacement shifts their ranges", async () => {
  const { entry, controller } = editableName("가나다라 됬다");
  vi.mocked(invoke).mockResolvedValueOnce([
    { word: "가나다라", valid: false, suggestions: ["가"] },
    { word: "됬다", valid: false, suggestions: ["됐다"] },
  ]);
  render(
    <SpellcheckDialog
      controller={controller}
      document="doc"
      entry={entry}
      locked={false}
      close={vi.fn()}
    />,
  );
  expect(await screen.findAllByRole("button", { name: "적용" })).toHaveLength(
    2,
  );
  fireEvent.click(screen.getAllByRole("button", { name: "적용" })[0]);
  await waitFor(() =>
    expect(entry.body.name).toEqual({ intent: "set", value: "가 됬다" }),
  );
  expect(await screen.findAllByRole("button", { name: "적용" })).toHaveLength(
    1,
  );
  expect(screen.getByRole("button", { name: "적용" })).toBeEnabled();
  entry.busy = true;
  window.setTimeout(() => {
    entry.busy = false;
  }, 70);
  fireEvent.click(screen.getByRole("button", { name: "적용" }));
  await waitFor(() =>
    expect(entry.body.name).toEqual({ intent: "set", value: "가 됐다" }),
  );
  expect(
    await screen.findByText("모든 검사 항목이 처리됐습니다."),
  ).toBeVisible();
  expect(screen.getByText("완료")).toBeVisible();
});

it("hides ignored cards and leaves only unselected findings after bulk apply", async () => {
  const { entry, controller } = editableName("가나다라 됬다 틀렷다");
  vi.mocked(invoke).mockResolvedValueOnce([
    { word: "가나다라", valid: false, suggestions: ["가"] },
    { word: "됬다", valid: false, suggestions: ["됐다"] },
    {
      word: "틀렷다",
      valid: false,
      suggestions: ["틀렸다", "틀렸었다"],
    },
  ]);
  render(
    <SpellcheckDialog
      controller={controller}
      document="doc"
      entry={entry}
      locked={false}
      close={vi.fn()}
    />,
  );
  expect(
    await screen.findAllByRole("button", { name: "이번은 무시" }),
  ).toHaveLength(3);
  fireEvent.click(screen.getByRole("button", { name: "일괄 적용" }));
  await waitFor(() =>
    expect(entry.body.name).toEqual({ intent: "set", value: "가 됐다 틀렷다" }),
  );
  expect(
    await screen.findAllByRole("button", { name: "이번은 무시" }),
  ).toHaveLength(1);
  expect(screen.getByRole("button", { name: "적용" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "이번은 무시" }));
  expect(
    await screen.findByText("모든 검사 항목이 처리됐습니다."),
  ).toBeVisible();
});
