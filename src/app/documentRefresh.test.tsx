import {
  act,
  cleanup,
  fireEvent,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { render } from "../test/render";
import recordedJson from "../test/fixtures/m35-refresh-real-dto.json";
import { DocumentWorkspace } from "./DocumentWorkspace";
import { DocumentController } from "./documentController";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";
import type { DocumentEditing, DocumentList } from "../bridge/documents";
import type { ResultDto } from "../bridge/types";
import { editDirty } from "./documentEdits";
import { text } from "../strings";

// AUDIT-001의 실제 guarded 응답을 보존한다. transport 도착 시점만 지연하며
// 제품 owner/raw 상태를 시험이 대신 복구하지 않는다.
const recorded = recordedJson as {
  begin: DocumentEditing;
  saved: DocumentEditing;
  refresh: ResultDto;
  success: DocumentEditing;
};
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  localStorage.clear();
});

for (const success of [false, true])
  for (const newInput of [false, true]) {
    it(`refresh response preserves rendered raw: success=${success}, input=${newInput}`, async () => {
      HTMLElement.prototype.scrollIntoView = vi.fn();
      vi.spyOn(window, "scrollTo").mockImplementation(() => {});
      const transport = new TestTransport();
      const d = structuredClone(recorded.begin);
      // 실제 이름 거부 DTO에 같은 raw 규칙을 쓰는 숫자 필드 대조를 추가한다.
      d.read.template.fields = [
        {
          id: "n",
          label: "number",
          kind: "Number",
          lifecycle: "Active",
          required: false,
          presentation: null,
          default: { kind: "unset" },
          initialDefault: { kind: "unset" },
          introducedRevision: "1",
          optionOrder: [],
          options: [],
        },
      ];
      d.read.template.fieldOrder = ["n"];
      d.read.fields = [
        {
          id: "n",
          label: "number",
          state: "Active",
          value: { kind: "number", value: "0" },
        },
      ];
      d.editable = ["n"];
      const id = d.document;
      const saved = {
        ...structuredClone(recorded.saved),
        read: d.read,
        editable: d.editable,
      };
      const refreshed: DocumentEditing = {
        ...structuredClone(recorded.success),
        editable: d.editable,
        read: { ...d.read, name: recorded.success.read.name },
      };
      const list: DocumentList = {
        kind: "list",
        fingerprint: "fixture",
        snapshot: "snap",
        initial: false,
        unplaced: [],
        problem: null,
        documents: [{ id, template: d.read.template.id, name: d.read.name }],
        layout: {
          revision: 1,
          rootOrder: [id],
          nodes: {
            [id]: {
              parentId: null,
              childOrder: [],
              state: "active",
              trash: null,
            },
          },
        },
      };
      transport.workspaceResult = (w) => {
        if (w.kind !== "document_workspace") return undefined;
        const q = w.request;
        if (q.action === "list")
          return { kind: "document_workspace", value: list };
        if (q.action === "read")
          return { kind: "document_workspace", value: d.read };
        if (q.action === "edit_begin")
          return { kind: "document_workspace", value: d };
        if (q.action === "edit_draft")
          return { kind: "document_workspace", value: saved };
        if (q.action === "edit_refresh")
          return success
            ? { kind: "document_workspace", value: refreshed }
            : structuredClone(recorded.refresh);
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
      const name = screen.getByRole("textbox", {
        name: text("documentEdit.name"),
      });
      const number = screen.getByRole("textbox", { name: "number" });
      fireEvent.change(name, { target: { value: "submitted S" } });
      await act(() => controller.edits.submit(id));
      expect(controller.edits.entries[id].status.problem).toBe(
        "SavedReadRequired",
      );
      transport.hold = "document_workspace";
      fireEvent.click(
        screen.getByRole("button", { name: text("documentEdit.refresh") }),
      );
      await waitFor(() =>
        expect(
          [...transport.results.values()].some((r) => r.state === "pending"),
        ).toBe(true),
      );
      expect(name).toBeEnabled();
      expect(number).toBeEnabled();
      if (newInput) {
        fireEvent.change(name, { target: { value: "NEW RAW DURING REFRESH" } });
        fireEvent.change(number, { target: { value: "-" } });
      }
      const body = structuredClone(controller.edits.entries[id].body);
      const generation = controller.edits.entries[id].generation;
      await act(async () => {
        transport.hold = null;
        transport.completeHeld();
        await shell.operations.queryAll();
      });
      await waitFor(() =>
        expect(controller.edits.entries[id].busy).toBe(false),
      );
      const latest = controller.edits.entries[id];
      expect(latest.body).toEqual(body);
      expect(latest.generation).toBe(generation);
      expect(latest.status.outcome).toEqual(saved.outcome);
      expect(latest.paused).toBe(!success);
      expect(editDirty(latest)).toBe(newInput);
      expect(name).toHaveValue(
        newInput ? "NEW RAW DURING REFRESH" : "submitted S",
      );
      expect(number).toHaveValue(newInput ? "-" : "0");
      expect(latest.error === null).toBe(success);
    });
  }
