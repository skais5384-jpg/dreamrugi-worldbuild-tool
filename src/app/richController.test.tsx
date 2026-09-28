import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { $getRoot, getNearestEditorFromDOMNode } from "lexical";
import { render } from "../test/render";
import recorded from "../test/fixtures/m35-refresh-real-dto.json";
import { DocumentWorkspace } from "./DocumentWorkspace";
import { DocumentController } from "./documentController";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";
import type {
  DocumentEditing,
  DocumentList,
  DocumentRequest,
} from "../bridge/documents";
import { text } from "../strings";
afterEach(() => {
  Reflect.deleteProperty(Range.prototype, "getBoundingClientRect");
  Reflect.deleteProperty(Range.prototype, "getClientRects");
  cleanup();
  vi.useRealTimers();
  localStorage.clear();
});
it("rich DOM and unknown toggle flow through controller and GuardedClient with composition debounce", async () => {
  // jsdom은 Range 레이아웃을 제공하지 않는다. 실제 IME/caret의 통과 증거가 아니다.
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => new DOMRect(),
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  const transport = new TestTransport();
  const submitted: Extract<DocumentRequest, { action: "edit_draft" }>[] = [];
  const d = structuredClone(recorded.begin) as DocumentEditing;
  d.problem = null;
  d.field = null;
  d.body = { name: { intent: "keep" }, fields: [], composing: false };
  d.saved_generation = d.generation;
  const base = {
    lifecycle: "Active",
    required: false,
    presentation: null,
    default: { kind: "unset" as const },
    initialDefault: { kind: "unset" as const },
    introducedRevision: "1",
    optionOrder: [],
    options: [],
  };
  d.read.template.fields = [
    { ...base, id: "r", label: "본문", kind: "RichText" },
    {
      ...base,
      id: "n",
      label: "숫자",
      kind: "Number",
      minimum: "-1",
      maximum: "1",
    },
  ];
  d.read.template.fieldOrder = ["r", "n"];
  d.editable = ["r", "n"];
  d.read.fields = [
    {
      id: "r",
      label: "본문",
      state: "Active",
      value: {
        kind: "rich_text",
        content: {
          kind: "root",
          children: [
            { kind: "paragraph", children: [{ kind: "text", text: "원문" }] },
          ],
        },
      },
    },
    {
      id: "n",
      label: "숫자",
      state: "Active",
      value: { kind: "number", value: "0" },
    },
  ];
  const id = d.document;
  const list: DocumentList = {
    kind: "list",
    fingerprint: "m36",
    snapshot: "snap",
    initial: false,
    unplaced: [],
    problem: null,
    documents: [{ id, template: d.read.template.id, name: "M3-6" }],
    layout: {
      revision: 1,
      rootOrder: [id],
      nodes: {
        [id]: { parentId: null, childOrder: [], state: "active", trash: null },
      },
    },
  };
  transport.workspaceResult = (w) => {
    if (w.kind !== "document_workspace") return;
    const q = w.request;
    if (q.action === "list") return { kind: "document_workspace", value: list };
    if (q.action === "read")
      return { kind: "document_workspace", value: d.read };
    if (q.action === "edit_begin")
      return { kind: "document_workspace", value: structuredClone(d) };
    if (q.action === "edit_draft") {
      submitted.push(structuredClone(q));
      return {
        kind: "document_workspace",
        value: {
          ...structuredClone(d),
          body: structuredClone(q.body),
          generation: q.generation,
          saved_generation: q.generation,
        },
      };
    }
    throw new Error(q.action);
  };
  const shell = new TemplateController(new GuardedClient(transport));
  shell.start();
  await waitFor(() => expect(shell.snapshot().ready).toBe(true));
  shell.setRoot("fixture");
  await shell.open();
  const controller = new DocumentController(shell);
  await controller.load();
  await controller.open(id);
  await controller.beginEdit(id);
  render(<DocumentWorkspace controller={controller} />);
  await waitFor(() => expect(controller.snapshot().busy).toBe(false));
  const input = screen.getByRole("textbox", { name: "본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  await act(async () => {
    editor.update(() => $getRoot().selectEnd(), { discrete: true });
  });
  vi.useFakeTimers();
  fireEvent.compositionStart(input);
  fireEvent.paste(input, {
    clipboardData: { getData: () => " 조합중", types: ["text/plain"] },
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  const saves = () => submitted;
  expect(saves()).toHaveLength(0);
  fireEvent.compositionEnd(input);
  fireEvent.change(screen.getByRole("textbox", { name: "숫자" }), {
    target: { value: "-" },
  });
  fireEvent.click(
    screen.getByRole("checkbox", { name: text("field.numberUnknown") }),
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(999);
  });
  expect(saves()).toHaveLength(0);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1);
  });
  expect(saves()).toHaveLength(1);
  const saved = saves()[0];
  expect(saved).toMatchObject({
    action: "edit_draft",
    body: {
      composing: false,
      fields: expect.arrayContaining([
        {
          field: "n",
          value: {
            intent: "set",
            value: { kind: "number_unknown", previous_raw: "-" },
          },
        },
      ]),
    },
  });
  expect(input).toHaveTextContent("원문 조합중");
  fireEvent.click(
    screen.getByRole("checkbox", { name: text("field.numberUnknown") }),
  );
  expect(screen.getByRole("textbox", { name: "숫자" })).toHaveValue("-");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
  expect(saves()).toHaveLength(1);
});
