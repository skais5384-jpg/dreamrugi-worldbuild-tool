import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, afterEach, describe, it, expect, vi } from "vitest";
import { svnClient } from "./svnClient";
import { useSyncExternalStore } from "react";
import { SvnToolbar } from "./SvnToolbar";
import { render } from "../test/render";
import { DocumentWorkspace } from "./DocumentWorkspace";
import { DocumentTree, DOCUMENT_TREE_ROW_HEIGHT } from "./DocumentTree";
import { DocumentEditor } from "./DocumentEditor";
import { DocumentController } from "./documentController";
import { collectDocumentIssues } from "./DocumentIssues";
import { WorkspaceController } from "./workspaceController";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";
import type {
  Creation,
  DocumentList,
  DocumentResponse,
  DocumentEditing,
} from "../bridge/documents";
import type { Template, ResultDto } from "../bridge/types";
import { text } from "../strings";
import { youtubeConsentStore } from "./youtubeConsent";
beforeEach(() => {
  localStorage.clear();
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
});
afterEach(() => vi.restoreAllMocks());

it("resumes an owner-protected inspection after confirmed editor release without opening health", async () => {
  const f = await setup();
  await f.controller.open("a");
  await f.controller.beginEdit("a");
  const shell = f.controller.shell;
  const run = shell.operations.run.bind(shell.operations);
  let protectedScan = true;
  let scans = 0;
  vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
    const reply = await run(...args);
    if (
      args[0].kind === "asset_inspect" &&
      reply.result.kind === "asset_maintenance"
    ) {
      ++scans;
      reply.result.inspection.complete = !protectedScan;
    }
    return reply;
  });
  f.controller.edits.update("a", (body) => ({
    ...body,
    name: { intent: "set", value: "saved" },
  }));
  await f.controller.edits.submit("a");
  await waitFor(() => expect(scans).toBe(1));
  expect(shell.snapshot().inspectionPendingDocuments).toEqual(["a"]);
  protectedScan = false;
  await f.controller.endEdit("a");
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual([]),
  );
  expect(scans).toBe(2);
  expect(shell.snapshot().health).toBeNull();
});

it("queues a confirmed release during an in-flight scan and rejects its stale protected result", async () => {
  const f = await setup();
  const shell = f.controller.shell;
  const run = shell.operations.run.bind(shell.operations);
  const entered = healthGate(),
    release = healthGate();
  let scans = 0;
  vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
    const reply = await run(...args);
    if (
      args[0].kind === "asset_inspect" &&
      reply.result.kind === "asset_maintenance"
    ) {
      if (++scans === 1) {
        reply.result.inspection.complete = false;
        reply.result.inspection.ownerProtected = true;
        entered.resolve();
        await release.promise;
      }
    }
    return reply;
  });
  shell.invalidateDocumentInspection(["a"]);
  await entered.promise;
  shell.resumeDocumentInspection(shell.snapshot().projectId!);
  release.resolve();
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual([]),
  );
  expect(scans).toBe(2);
  expect(shell.snapshot().assetInspection?.complete).toBe(true);
});

it("waits for the last owner and retains actual incomplete or failed scans without polling", async () => {
  const f = await setup();
  const shell = f.controller.shell;
  const run = shell.operations.run.bind(shell.operations);
  let owners = 2,
    actualFailure = false,
    scans = 0;
  vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
    const reply = await run(...args);
    if (
      args[0].kind === "asset_inspect" &&
      reply.result.kind === "asset_maintenance"
    ) {
      ++scans;
      reply.result.inspection.complete = !owners && !actualFailure;
      reply.result.inspection.ownerProtected = !!owners && !actualFailure;
    }
    return reply;
  });
  shell.invalidateDocumentInspection(["a"]);
  await waitFor(() =>
    expect(shell.snapshot().inspectionRefreshState).toBe("waiting"),
  );
  owners = 1;
  shell.resumeDocumentInspection(shell.snapshot().projectId!);
  await waitFor(() => expect(scans).toBe(2));
  expect(shell.snapshot().inspectionPendingDocuments).toEqual(["a"]);
  owners = 0;
  actualFailure = true;
  shell.resumeDocumentInspection(shell.snapshot().projectId!);
  await waitFor(() =>
    expect(shell.snapshot().inspectionRefreshState).toBe("failed"),
  );
  await Promise.resolve();
  expect(scans).toBe(3);
  expect(shell.snapshot().inspectionPendingDocuments).toEqual(["a"]);
  actualFailure = false;
  shell.resumeDocumentInspection("different-project");
  expect(scans).toBe(3);
  shell.resumeDocumentInspection(shell.snapshot().projectId!);
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual([]),
  );
  expect(scans).toBe(4);
});

it("saves through the visible document owner before autosave and keeps read-only Ctrl+S inert", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue({
    length: 1,
  } as DOMRectList);
  render(<DocumentWorkspace controller={controller} />);
  await screen.findByRole("button", { name: text("common.save") });
  const saves = () =>
    transport.commands.filter(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "edit_draft" &&
        command.input.request.save,
    );
  vi.useFakeTimers();
  try {
    act(() =>
      controller.edits.update("a", (body) => ({
        ...body,
        name: { intent: "set", value: "명시 단축키 저장" },
      })),
    );
    expect(saves()).toHaveLength(0);
    await act(async () => {
      fireEvent.keyDown(document, { key: "s", ctrlKey: true });
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(saves()).toHaveLength(1);
    expect(controller.edits.entries.a.status.read.name).toBe(
      "명시 단축키 저장",
    );
    fireEvent.keyUp(document, { key: "s", ctrlKey: true });
    await act(async () => controller.edits.close("a"));
    const before = transport.commands.length;
    await act(async () => {
      fireEvent.keyDown(document, { key: "s", ctrlKey: true });
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(transport.commands).toHaveLength(before);
  } finally {
    vi.useRealTimers();
  }
});
async function setup(kind: "Number" | "Url" = "Number") {
  const transport = new TestTransport();
  const template: Template = {
    id: "t",
    name: "시험 템플릿",
    revision: "1",
    lifecycle: "Active",
    presentation: null,
    fieldOrder: ["n"],
    fields: [
      {
        id: "n",
        label: "필수 숫자",
        kind,
        lifecycle: "Active",
        required: true,
        presentation: null,
        default: { kind: "unset" },
        initialDefault: { kind: "unset" },
        introducedRevision: "1",
        options: [],
        optionOrder: [],
      },
    ],
  };
  transport.templates.set(template.id, template);
  const list: DocumentList = {
    kind: "list",
    fingerprint: "fixture",
    snapshot: "snapshot",
    initial: true,
    unplaced: [],
    problem: null,
    documents: [
      { id: "a", template: "t", name: "첫 문서" },
      { id: "b", template: "t", name: "둘째 문서" },
    ],
    layout: {
      revision: 1,
      rootOrder: ["a", "b"],
      nodes: Object.fromEntries(
        ["a", "b"].map((id) => [
          id,
          { parentId: null, childOrder: [], state: "active", trash: null },
        ]),
      ),
    },
  };
  let draft: Creation = {
    kind: "draft",
    owner: "owner",
    generation: "1",
    body: { name: "", parent: null, fields: [], composing: false },
    template,
    deposited: false,
    outcome: null,
    problem: null,
    field: null,
  };
  let saves = 0;
  const editors = new Map<string, DocumentEditing>();
  let outcome: ResultDto | null = null;
  transport.workspaceResult = (input) => {
    if (input.kind !== "document_workspace") return undefined;
    const q = input.request;
    let value: DocumentResponse;
    switch (q.action) {
      case "format_inspect":
        value = {
          kind: "format",
          schema: q.kind === "template" ? 5 : 4,
          source: "format-source",
          history: [
            {
              digest: "format-history-one",
              schema: q.kind === "template" ? 4 : 3,
              content_updated_at: "2026-09-21T00:00:00.000Z",
            },
          ],
        };
        break;
      case "format_change":
        return {
          kind: "write",
          session: "format-session",
          artifact: q.artifact,
          disk: "committed",
          recovery_required: false,
          cleanup_failed: false,
          error: null,
          diagnostic: {
            stage: "complete",
            category: null,
            sessionState: "ReadOnly",
            lockCategory: null,
            nextAction: "",
          },
          changed: true,
          warnings: [],
        };
      case "edit_begin": {
        const editor: DocumentEditing = {
          kind: "editing",
          owner: "edit-" + q.document,
          document: q.document,
          generation: "1",
          saved_generation: "1",
          body: { name: { intent: "keep" }, fields: [], composing: false },
          read: {
            kind: "read",
            id: q.document,
            name: list.documents.find((d) => d.id === q.document)!.name,
            template,
            fields: [
              {
                id: "n",
                label: "필수 숫자",
                state: "Active",
                value:
                  kind === "Url"
                    ? { kind: "url", value: "https://youtu.be/M7lc1UVf-VE?t=5" }
                    : { kind: "number", value: "1" },
              },
            ],
            warnings: [],
          },
          editable: ["n"],
          source: "source",
          deposited: false,
          outcome: null,
          problem: null,
          field: null,
        };
        editors.set(editor.owner, editor);
        value = editor;
        break;
      }
      case "edit_draft":
      case "edit_deposit": {
        const old = editors.get(q.owner)!;
        const invalidEnglishName =
          q.body.englishName?.intent === "set" &&
          /[\n\v\f\r\u0085\u2028\u2029]/u.test(q.body.englishName.value);
        const invalidGlossarySummary =
          q.body.glossarySummary?.intent === "set" &&
          /[\n\v\f\r\u0085\u2028\u2029]/u.test(q.body.glossarySummary.value);
        const next = {
          ...old,
          source:
            q.action === "edit_draft" &&
            !invalidEnglishName &&
            !invalidGlossarySummary
              ? `source-${q.generation}`
              : old.source,
          body: q.body,
          generation: q.generation,
          saved_generation:
            q.action === "edit_draft" ? q.generation : old.saved_generation,
          deposited: q.action === "edit_deposit",
          problem:
            q.action === "edit_draft" &&
            (invalidEnglishName || invalidGlossarySummary)
              ? "SingleLineRequired"
              : null,
          field:
            q.action === "edit_draft" && invalidEnglishName
              ? "englishName"
              : q.action === "edit_draft" && invalidGlossarySummary
                ? "glossarySummary"
                : null,
        };
        if (q.body.name.intent === "set")
          next.read = { ...next.read, name: q.body.name.value };
        editors.set(q.owner, next);
        value = next;
        break;
      }
      case "edit_release":
        value = { kind: "released" };
        break;
      case "list":
        value = structuredClone(list);
        break;
      case "search": {
        const rows = [
          {
            id: "a",
            template: "t",
            templateName: template.name,
            name: "첫 문서",
            path: [],
            titleMatch: q.query.includes("첫"),
            excerpt: q.query.includes("필드")
              ? { label: "설명", text: "필드에서 찾은 저장 본문" }
              : null,
          },
          {
            id: "b",
            template: "t",
            templateName: template.name,
            name: "둘째 문서",
            path: ["첫 문서"],
            titleMatch: q.query.includes("둘째"),
            excerpt: null,
          },
        ].filter(
          (row) =>
            (!q.template || row.template === q.template) &&
            (!q.query || row.titleMatch || !!row.excerpt),
        );
        value = {
          kind: "search",
          generation: "search-generation",
          offset: q.offset,
          total: rows.length,
          hasMore: false,
          missingDocuments: 0,
          templateNames: { t: template.name },
          results: rows.slice(q.offset, q.offset + q.limit),
        };
        break;
      }
      case "references":
        value = {
          kind: "references",
          generation: "reference-generation",
          document: q.document,
          incomplete: 0,
          unavailable: [],
          incoming: [],
        };
        break;
      case "read":
      case "search_read":
        value = {
          kind: "read",
          id: q.document,
          name: list.documents.find((document) => document.id === q.document)!
            .name,
          template,
          fields: [
            {
              id: "n",
              label: "빈 숫자",
              state: "Active",
              value:
                kind === "Url"
                  ? { kind: "url", value: "https://youtu.be/M7lc1UVf-VE?t=5" }
                  : { kind: "unset" },
            },
            {
              id: "old",
              label: "보관 본문",
              state: "Archived",
              value: {
                kind: "rich_text",
                content: {
                  kind: "root",
                  children: [
                    {
                      kind: "paragraph",
                      children: [
                        { kind: "text", text: "강조된 원문", marks: ["bold"] },
                      ],
                    },
                  ],
                },
              },
            },
            {
              id: "orphan",
              label: "정의 없는 값",
              state: "Orphan",
              value: { kind: "single_line_text", value: "보존 원문" },
            },
          ],
          warnings: ["UnknownBinding"],
        };
        break;
      case "begin":
        value = draft;
        break;
      case "restore":
        draft = {
          ...draft,
          body: { ...draft.body, name: "복원 문서" },
          problem: "RestoredChooseParent",
        };
        value = draft;
        break;
      case "draft": {
        saves++;
        const invalidEnglishName = /[\n\v\f\r\u0085\u2028\u2029]/u.test(
          q.body.englishName ?? "",
        );
        const invalidGlossarySummary = /[\n\v\f\r\u0085\u2028\u2029]/u.test(
          q.body.glossarySummary ?? "",
        );
        const artifact =
          outcome?.kind === "write" && outcome.disk === "committed"
            ? outcome.artifact
            : null;
        draft = {
          ...draft,
          body: q.body,
          generation: q.generation,
          outcome,
          problem: outcome
            ? null
            : invalidEnglishName || invalidGlossarySummary
              ? "SingleLineRequired"
              : "Required",
          field: outcome
            ? null
            : invalidEnglishName
              ? "englishName"
              : invalidGlossarySummary
                ? "glossarySummary"
                : "n",
          commit: artifact
            ? {
                fingerprint: list.fingerprint,
                snapshot: "created-snapshot",
                layoutRevision: list.layout.revision + 1,
                parent: q.body.parent,
                changedDocuments: [
                  { id: artifact, template: "t", name: q.body.name },
                ],
                removedDocuments: [],
                unplaced: [],
                documentCount: list.documents.length + 1,
                read: {
                  kind: "read",
                  id: artifact,
                  name: q.body.name,
                  template,
                  fields: [],
                  warnings: [],
                },
              }
            : null,
        };
        value = draft;
        break;
      }
      case "deposit":
        draft = {
          ...draft,
          body: q.body,
          generation: q.generation,
          deposited: true,
        };
        value = draft;
        break;
      case "release":
      case "replace_discard":
        value = { kind: "released" };
        break;
      default:
        throw new Error(q.action);
    }
    return { kind: "document_workspace", value };
  };
  const shell = new TemplateController(new GuardedClient(transport));
  shell.start();
  await waitFor(() => expect(shell.snapshot().ready).toBe(true));
  shell.setRoot("fixture");
  await shell.open();
  const workspace = new WorkspaceController(shell);
  const controller = workspace.documents;
  await controller.load();
  return {
    workspace,
    controller,
    transport,
    list,
    template,
    get saves() {
      return saves;
    },
    setOutcome: (o: ResultDto) => {
      outcome = o;
    },
  };
}

it("저장된 문서 참조만 점검에서 갱신하고 실패한 저장은 기존 경고를 유지한다", async () => {
  const f = await setup();
  const shell = f.controller.shell;
  const resource = "resource-r";
  f.template.fields[0].kind = "File";
  let savedReferences = [resource, "resource-s"];
  const originalWorkspace = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    const response = originalWorkspace(input);
    if (
      input.kind !== "document_workspace" ||
      response?.kind !== "document_workspace"
    )
      return response;
    const value = response.value;
    if (input.request.action === "edit_draft" && value.kind === "editing") {
      const reference = input.request.body.fields.find(
        (field) => field.field === "n",
      )?.value;
      if (
        value.saved_generation === input.request.generation &&
        !value.problem &&
        reference?.intent === "set" &&
        reference.value.kind === "file"
      )
        savedReferences = [...reference.value.value];
    }
    if (value.kind === "editing" || value.kind === "read") {
      const read = value.kind === "editing" ? value.read : value;
      const updated = {
        ...read,
        fields: read.fields.map((field) =>
          field.id === "n"
            ? {
                ...field,
                value: { kind: "file" as const, value: savedReferences },
              }
            : field,
        ),
      };
      return {
        ...response,
        value: value.kind === "editing" ? { ...value, read: updated } : updated,
      };
    }
    return response;
  };
  f.transport.assetInspection.documentIssues = [
    {
      documentId: "a",
      reasons: ["resource_in_trash", "resource_missing"],
      targets: [
        {
          reason: "resource_in_trash",
          resourceIds: [resource],
          templateIds: [],
        },
        {
          reason: "resource_missing",
          resourceIds: ["resource-s"],
          templateIds: [],
        },
      ],
    },
    {
      documentId: "b",
      reasons: ["resource_in_trash"],
      relatedResourceIds: [resource],
    },
  ];
  shell.showHealth();
  await waitFor(() => expect(shell.snapshot().health?.phase).toBe("ready"));
  const issues = () =>
    collectDocumentIssues(
      f.controller.snapshot().list,
      shell.snapshot().rows,
      shell.snapshot().assetInspection,
      f.controller.snapshot().validationIssues,
      f.controller.snapshot().read,
      shell.snapshot().inspectionPendingDocuments,
    );
  expect(
    issues()
      .get("a")
      ?.some((issue) => issue.reason === "resource_in_trash"),
  ).toBe(true);
  await f.controller.beginEdit("a");
  f.controller.edits.update("a", (body) => ({
    ...body,
    englishName: { intent: "set", value: "invalid\nname" },
  }));
  await f.controller.edits.submit("a");
  expect(shell.snapshot().inspectionPendingDocuments).toEqual([]);
  expect(
    issues()
      .get("a")
      ?.some((issue) => issue.reason === "resource_in_trash"),
  ).toBe(true);

  f.transport.assetInspection.documentIssues = [
    {
      documentId: "a",
      reasons: ["resource_missing"],
      relatedResourceIds: ["resource-s"],
    },
    f.transport.assetInspection.documentIssues[1],
  ];
  f.controller.edits.update("a", (body) => ({
    ...body,
    englishName: { intent: "set", value: "valid" },
  }));
  f.controller.edits.field("a", "n", {
    intent: "set",
    value: { kind: "file", value: ["resource-s"] },
  });
  await f.controller.edits.submit("a");
  await f.controller.load();
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual([]),
  );
  expect(
    issues()
      .get("a")
      ?.some((issue) => issue.reason === "resource_in_trash"),
  ).toBe(false);
  expect(
    issues()
      .get("a")
      ?.some((issue) => issue.reason === "resource_missing"),
  ).toBe(true);
  expect(
    issues()
      .get("b")
      ?.some((issue) => issue.reason === "resource_in_trash"),
  ).toBe(true);
  // A background refresh cannot certify a new explicit project check.
  expect(shell.snapshot().health?.check).toBeUndefined();
  expect(shell.snapshot().health?.inspection?.complete).toBe(false);
  expect(savedReferences).toEqual(["resource-s"]);
  expect(
    f.transport.commands.some(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "edit_draft" &&
        command.input.request.body.fields.some(
          (field) =>
            field.field === "n" &&
            field.value.intent === "set" &&
            field.value.value.kind === "file" &&
            field.value.value.value.join(",") === "resource-s",
        ),
    ),
  ).toBe(true);
});

it("저장 이전 검사의 늦은 응답과 재검사 실패는 최신 문서를 정상으로 덮지 않는다", async () => {
  const f = await setup();
  const shell = f.controller.shell;
  const oldIssue = {
    documentId: "a",
    reasons: ["resource_in_trash"],
    relatedResourceIds: ["resource-r"],
  };
  f.transport.assetInspection.documentIssues = [oldIssue];
  shell.showHealth();
  await waitFor(() => expect(shell.snapshot().health?.phase).toBe("ready"));
  await f.controller.beginEdit("a");

  f.transport.hold = "asset_inspect";
  const oldInspection = shell.inspectAssets();
  await waitFor(() =>
    expect(
      [...f.transport.results.values()].some((row) => row.state === "pending"),
    ).toBe(true),
  );
  f.transport.hold = null;
  f.transport.assetInspection.documentIssues = [];
  f.controller.edits.update("a", (body) => ({
    ...body,
    name: { intent: "set", value: "new source" },
  }));
  await f.controller.edits.submit("a");
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual([]),
  );
  expect(shell.snapshot().assetInspection?.documentIssues).toEqual([]);
  f.transport.assetInspection.documentIssues = [oldIssue];
  f.transport.completeHeld();
  await oldInspection;
  expect(shell.snapshot().assetInspection?.documentIssues).toEqual([]);
  expect(shell.snapshot().health?.inspection).toBeNull();
  expect(shell.snapshot().health?.check).toBeUndefined();

  const original = f.transport.workspaceResult;
  f.transport.workspaceResult = (input) =>
    input.kind === "asset_inspect"
      ? {
          kind: "rejected",
          error: { code: "unavailable", nextAction: "" },
          input_retained: false,
        }
      : original?.(input);
  f.controller.edits.update("a", (body) => ({
    ...body,
    name: { intent: "set", value: "newer source" },
  }));
  await f.controller.edits.submit("a");
  await waitFor(() =>
    expect(shell.snapshot().inspectionPendingDocuments).toEqual(["a"]),
  );
  expect(shell.snapshot().assetInspection?.complete).toBe(false);
  expect(shell.snapshot().assetInspection?.documentIssues).toEqual([]);
  f.transport.workspaceResult = original;
  f.transport.assetInspection.documentIssues = [];
  await shell.inspectAssets();
  expect(shell.snapshot().inspectionPendingDocuments).toEqual([]);
  expect(shell.snapshot().health?.inspection?.complete).toBe(true);
});

it("문서를 열지 않아도 전체 검증 결과의 경고를 트리에 표시한다", async () => {
  const f = await setup();
  const original = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    if (input.kind === "document_workspace" && input.request.action === "list")
      return {
        kind: "document_workspace",
        value: {
          ...f.list,
          issues: [{ document: "a", warnings: ["RequiredValueUnset"] }],
        },
      };
    return original(input);
  };
  await f.controller.load();
  render(<DocumentWorkspace controller={f.controller} />);

  expect(f.controller.snapshot().read).toBeNull();
  const warning = screen.getByRole("img", {
    name: text("documents.warning.RequiredValueUnset"),
  });
  expect(warning).toHaveClass("document-tree-warning");
  expect(
    warning.closest(".document-tree-row")?.querySelector(".tree-name"),
  ).toHaveTextContent("첫 문서");
});

it.each([
  ["complete", ["template_missing"], [], "template_missing"],
  ["partial", [], ["a"], "validation_unknown"],
] as const)(
  "선택 전 %s 검사 결과를 문서 트리에 구분해 표시한다",
  async (issueStatus, reasons, unverifiedDocuments, messageReason) => {
    const f = await setup();
    const original = f.transport.workspaceResult!;
    f.transport.workspaceResult = (input) => {
      if (
        input.kind === "document_workspace" &&
        input.request.action === "list"
      )
        return {
          kind: "document_workspace",
          value: {
            ...f.list,
            issueStatus,
            unverifiedDocuments: [...unverifiedDocuments],
            issues: reasons.length
              ? [{ document: "a", warnings: [], reasons: [...reasons] }]
              : [],
          },
        };
      return original(input);
    };
    await f.controller.load();
    render(<DocumentWorkspace controller={f.controller} />);

    expect(f.controller.snapshot().read).toBeNull();
    const warning = screen.getByRole("img", {
      name: text(`documents.issue.${messageReason}`),
    });
    expect(
      warning.closest(".document-tree-row")?.querySelector(".tree-name"),
    ).toHaveTextContent("첫 문서");
  },
);

it("이전 열린 세대의 늦은 목록 응답이 현재 문서 경고를 덮지 않는다", async () => {
  const f = await setup();
  const original = f.transport.workspaceResult!;
  let listReplies = 0;
  f.transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "list"
    ) {
      listReplies += 1;
      return {
        kind: "document_workspace",
        value: {
          ...f.list,
          issueStatus: "complete",
          unverifiedDocuments: [],
          issues:
            listReplies === 1
              ? [{ document: "a", warnings: [], reasons: ["template_missing"] }]
              : [],
        },
      };
    }
    return original(input);
  };
  f.transport.hold = "document_workspace";
  const pending = f.controller.load();
  await waitFor(() =>
    expect(
      [...f.transport.results.values()].some((r) => r.state === "pending"),
    ).toBe(true),
  );
  // A new project open assigns the shell request generation before accepting
  // any delayed old List; advance that exact counter without touching storage.
  const shell = f.controller.shell as unknown as {
    projectRequestGeneration: number;
  };
  shell.projectRequestGeneration += 1;
  await f.controller.load();
  f.transport.hold = null;
  f.transport.completeHeld();
  await pending;
  await waitFor(() => expect(listReplies).toBe(2));
  expect(f.controller.snapshot().validationIssues).toEqual([]);
  expect(f.controller.snapshot().list?.issues).toEqual([]);
});

it("버전 기록은 문서 헤더에서 현재 형식과 보존본을 팝업으로 보여주고 선택 복원을 실행한다", async () => {
  const f = await setup();
  await f.controller.open("a");
  render(<DocumentWorkspace controller={f.controller} />);

  fireEvent.click(
    await screen.findByRole("button", { name: text("format.title") }),
  );
  const dialog = await screen.findByRole("dialog", {
    name: text("format.title"),
  });
  expect(
    within(dialog).getByRole("heading", { name: text("format.current") }),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByRole("heading", {
      name: text("format.savedVersions"),
    }),
  ).toBeInTheDocument();

  fireEvent.change(
    within(dialog).getByRole("combobox", { name: text("format.restore") }),
    { target: { value: "format-history-one" } },
  );
  fireEvent.click(
    within(dialog).getByRole("button", { name: text("format.restore") }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("dialog", { name: text("format.title") }),
    ).not.toBeInTheDocument(),
  );
  expect(
    f.transport.commands.some(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "format_change" &&
        command.input.request.restore === "format-history-one",
    ),
  ).toBe(true);
});

it("현재 선택형 반복 목록의 저장 key 누락은 필드 안에서 자동 복구 예정으로만 안내한다", async () => {
  const f = await setup();
  const group = {
    ...f.template.fields[0],
    id: "characters",
    label: "캐릭터 목록",
    kind: "Group" as const,
    required: false,
    members: [],
    memberOrder: [],
  };
  f.template.fieldOrder = [group.id];
  f.template.fields = [group];
  const original = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    if (input.kind === "document_workspace" && input.request.action === "read")
      return {
        kind: "document_workspace",
        value: {
          kind: "read",
          id: input.request.document,
          name: "첫 문서",
          template: f.template,
          fields: [
            {
              id: group.id,
              label: group.label,
              state: "Active",
              value: null,
              problem: "MissingKnownFieldValue",
            },
          ],
          warnings: ["MissingKnownFieldValue"],
        },
      };
    return original(input);
  };

  await f.controller.open("a");
  render(<DocumentWorkspace controller={f.controller} />);
  expect(
    await screen.findByText(
      text("documents.valueProblem.MissingOptionalGroup"),
    ),
  ).toBeInTheDocument();
  expect(
    screen
      .queryAllByRole("alert")
      .some((alert) =>
        alert.textContent?.includes(
          text("documents.warning.MissingKnownFieldValue"),
        ),
      ),
  ).toBe(false);
  const treeWarning = screen.getByRole("img", {
    name: text("documents.warning.MissingKnownFieldValue"),
  });
  expect(treeWarning).toHaveClass("document-tree-warning");
  expect(
    treeWarning.closest(".document-tree-row")?.querySelector(".tree-name"),
  ).toHaveTextContent("첫 문서");
});

it("fix002 uses the cache generation's Template name in the search filter", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) => {
    const result = original(input);
    if (result?.kind === "document_workspace" && result.value.kind === "search")
      return {
        ...result,
        value: { ...result.value, templateNames: { t: "외부 새 이름" } },
      };
    return result;
  };
  render(<DocumentWorkspace controller={controller} searchMode />);
  fireEvent.change(
    await screen.findByLabelText(text("documents.searchInput")),
    { target: { value: "첫" } },
  );
  expect(
    await screen.findByRole("option", { name: "외부 새 이름" }),
  ).toHaveValue("t");
});

it("fix002 rejects a stale target before flushing another dirty editor", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) =>
    input.kind === "document_workspace" &&
    input.request.action === "search_read"
      ? {
          kind: "document_workspace",
          value: { kind: "search_unavailable", document: "b" },
        }
      : original(input);
  vi.useFakeTimers();
  try {
    controller.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "dirty A" },
    }));
    const before = structuredClone(controller.edits.entries.a);
    await controller.openSearchResult("b");
    expect(
      transport.commands.filter(
        (c) =>
          c.action === "submit" &&
          c.input.kind === "document_workspace" &&
          c.input.request.action === "edit_draft" &&
          c.input.request.save,
      ),
    ).toHaveLength(0);
    expect(controller.edits.entries.a).toEqual(before);
    expect(controller.snapshot().ui.active).toBe("a");
  } finally {
    vi.useRealTimers();
  }
});

it("역참조의 출처 수정은 다른 문서를 몰래 저장하지 않고 정확한 관계 입력을 연다", async () => {
  const f = await setup();
  Object.assign(f.template.fields[0], {
    label: "소속",
    kind: "Relation",
    multiple: true,
    allowedTemplates: [],
    reciprocalNotice: true,
  });
  const original = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      ["search_read", "edit_begin"].includes(input.request.action) &&
      "document" in input.request &&
      input.request.document === "a"
    ) {
      const read = {
        kind: "read" as const,
        id: "a",
        name: "첫 문서",
        template: f.template,
        fields: [
          {
            id: "n",
            label: "소속",
            state: "Active",
            value: {
              kind: "relation" as const,
              links: [
                {
                  id: "connection-a-b",
                  document: "b",
                  oneWay: false,
                  name: "친구",
                },
              ],
            },
          },
        ],
        warnings: [],
      };
      return {
        kind: "document_workspace",
        value:
          input.request.action === "edit_begin"
            ? {
                kind: "editing" as const,
                owner: "edit-a-reference",
                document: "a",
                generation: "1",
                saved_generation: "1",
                body: {
                  name: { intent: "keep" as const },
                  fields: [],
                  composing: false,
                },
                read,
                editable: ["n"],
                source: "source-a-reference",
                deposited: false,
                outcome: null,
                problem: null,
                field: null,
              }
            : read,
      };
    }
    if (
      input.kind === "document_workspace" &&
      input.request.action === "references" &&
      input.request.document === "b"
    ) {
      const value: Extract<DocumentResponse, { kind: "references" }> = {
        kind: "references",
        generation: "relation-generation",
        document: "b",
        incomplete: 1,
        unavailable: [
          {
            source: "c",
            sourceName: "휴지통 출처 문서",
            reason: "trashed",
          },
        ],
        incoming: [
          {
            kind: "relation",
            source: "a",
            sourceName: "첫 문서",
            sourceTemplate: "시험 템플릿",
            path: [],
            target: "b",
            field: "n",
            fieldLabel: "소속",
            instance: null,
            connection: "connection-a-b",
            relationName: "친구",
            oneWay: false,
            missingReciprocal: true,
          },
          {
            kind: "document_link",
            source: "a",
            sourceName: "첫 문서",
            sourceTemplate: "시험 템플릿",
            path: [],
            target: "b",
            field: "link",
            fieldLabel: "관련 문서",
            instance: null,
          },
        ],
      };
      return {
        kind: "document_workspace",
        value,
      };
    }
    return original(input);
  };
  render(<DocumentWorkspace controller={f.controller} />);
  await waitFor(() => expect(f.controller.snapshot().busy).toBe(false));
  await act(() => f.controller.open("b"));
  await waitFor(() =>
    expect(f.controller.snapshot().references).toMatchObject({
      document: "b",
      incoming: [
        { kind: "relation", source: "a" },
        { kind: "document_link", source: "a" },
      ],
    }),
  );
  const incoming = [...document.querySelectorAll(".incoming-reference-item")];
  expect(incoming).toHaveLength(2);
  expect(incoming[0]).toHaveAttribute("data-kind", "relation");
  expect(incoming[0]).toHaveTextContent("관계첫 문서친구");
  expect(incoming[1]).toHaveAttribute("data-kind", "document_link");
  expect(incoming[1]).toHaveTextContent("링크첫 문서");
  expect(
    screen.getByRole("button", { name: "관계: 첫 문서 – 친구" }),
  ).toHaveClass("incoming-reference-source");
  expect(screen.getByRole("button", { name: "링크: 첫 문서" })).toHaveClass(
    "incoming-reference-source",
  );
  expect(
    incoming.every((item) => item.querySelector(".incoming-reference-heading")),
  ).toBe(true);
  expect(
    incoming[0].querySelector(".incoming-reference-followup"),
  ).toContainElement(incoming[0].querySelector(".relation-warning"));
  expect(incoming[0]).toHaveTextContent(text("relation.missingReciprocal"));
  const unavailable = screen
    .getByText("휴지통 출처 문서")
    .closest('[role="alert"]');
  expect(unavailable).not.toBeNull();
  expect(unavailable).toHaveTextContent("휴지통 출처 문서");
  expect(unavailable).toHaveTextContent(text("relation.unavailable.trashed"));
  expect(unavailable!.querySelector("svg")).not.toBeNull();
  const editSources = await screen.findAllByRole("button", {
    name: text("relation.editSource"),
  });
  expect(editSources).toHaveLength(1);
  expect(editSources[0]).toHaveClass("incoming-edit-source");
  expect(editSources[0].querySelector("svg")).not.toBeNull();
  fireEvent.click(editSources[0]);
  await waitFor(() => expect(f.controller.snapshot().ui.active).toBe("a"));
  await waitFor(() =>
    expect(document.activeElement).toHaveAttribute("id", "edit-a-n"),
  );
  expect(
    f.transport.commands.filter(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "edit_draft" &&
        command.input.request.save,
    ),
  ).toHaveLength(0);
});

it("역참조 출처의 연결 대상이 달라지면 이동·저장 전에 거절하고 현재 초안을 보존한다", async () => {
  const f = await setup();
  await f.controller.open("b");
  await f.controller.beginEdit("b");
  f.controller.edits.update("b", (body) => ({
    ...body,
    name: { intent: "set", value: "보존할 초안" },
  }));
  const before = structuredClone(f.controller.edits.entries.b);
  const original = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "search_read" &&
      input.request.document === "a"
    )
      return {
        kind: "document_workspace",
        value: {
          kind: "read" as const,
          id: "a",
          name: "첫 문서",
          template: f.template,
          fields: [
            {
              id: "n",
              label: "소속",
              state: "Active" as const,
              value: {
                kind: "relation" as const,
                links: [
                  {
                    id: "connection-a-b",
                    document: "c",
                    oneWay: false,
                    name: "친구",
                  },
                ],
              },
            },
          ],
          warnings: [],
        },
      };
    return original(input);
  };

  await f.controller.editReferenceSource({
    source: "a",
    target: "b",
    field: "n",
    instance: null,
    connection: "connection-a-b",
  });

  expect(f.controller.snapshot().ui.active).toBe("b");
  expect(f.controller.edits.entries.b).toEqual(before);
  expect(f.controller.snapshot().error).toBe(text("relation.sourceChanged"));
  expect(
    f.transport.commands.filter(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "edit_draft" &&
        command.input.request.save,
    ),
  ).toHaveLength(0);
});

it("그룹 역참조 행이 편집 시작 사이에 사라지면 다른 관계 입력에 포커스하지 않는다", async () => {
  const f = await setup();
  const relation = {
    ...f.template.fields[0],
    id: "relation-child",
    label: "행 관계",
    kind: "Relation" as const,
    required: false,
    multiple: true,
    allowedTemplates: [],
    reciprocalNotice: true,
  };
  const group = {
    ...f.template.fields[0],
    id: "group",
    label: "관계 그룹",
    kind: "Group" as const,
    required: false,
    members: [relation],
    memberOrder: [relation.id],
  };
  f.template.fieldOrder = [group.id];
  f.template.fields = [group];
  const canonical = {
    kind: "read" as const,
    id: "a",
    name: "첫 문서",
    template: f.template,
    fields: [
      {
        id: group.id,
        label: group.label,
        state: "Active" as const,
        value: {
          kind: "group" as const,
          instances: [
            {
              id: "row-a",
              source: null,
              fields: [
                {
                  field: relation.id,
                  value: {
                    intent: "set" as const,
                    value: {
                      kind: "relation" as const,
                      links: [
                        {
                          id: "group-connection-a-b",
                          document: "b",
                          oneWay: false,
                          name: "친구",
                        },
                      ],
                    },
                  },
                },
              ],
            },
          ],
        },
      },
    ],
    warnings: [],
  };
  const original = f.transport.workspaceResult!;
  f.transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "search_read" &&
      input.request.document === "a"
    )
      return { kind: "document_workspace", value: canonical };
    if (
      input.kind === "document_workspace" &&
      input.request.action === "edit_begin" &&
      input.request.document === "a"
    )
      return {
        kind: "document_workspace",
        value: {
          kind: "editing" as const,
          owner: "edit-a-group-stale",
          document: "a",
          generation: "1",
          saved_generation: "1",
          body: {
            name: { intent: "keep" as const },
            fields: [],
            composing: false,
          },
          read: {
            ...canonical,
            fields: [
              {
                ...canonical.fields[0],
                value: { kind: "group" as const, instances: [] },
              },
            ],
          },
          editable: [group.id],
          source: "source-a-group-stale",
          deposited: false,
          outcome: null,
          problem: null,
          field: null,
        },
      };
    return original(input);
  };

  await f.controller.open("b");
  await f.controller.editReferenceSource({
    source: "a",
    target: "b",
    field: relation.id,
    instance: "row-a",
    connection: "group-connection-a-b",
  });

  expect(f.controller.snapshot().ui.active).toBe("a");
  expect(f.controller.snapshot().referenceFocus).toBeNull();
  expect(f.controller.snapshot().error).toBe(text("relation.sourceChanged"));
});

it("fix002 cancels pending search before ownerless project close", async () => {
  const { controller, transport } = await setup();
  const workspace = new WorkspaceController(controller.shell);
  await workspace.documents.load();
  transport.hold = "document_workspace";
  workspace.documents.searchDocuments("첫", null);
  await waitFor(() =>
    expect(
      [...transport.results.values()].some((r) => r.state === "pending"),
    ).toBe(true),
  );
  await workspace.navigate({ kind: "close_project" });
  expect(
    transport.commands.some(
      (c) => c.action === "document_progress" && c.cancel,
    ),
  ).toBe(true);
  transport.completeHeld();
  await controller.shell.operations.queryAll();
});

it("fix002 waits for a late search reservation before sending project close", async () => {
  const { controller, workspace, transport } = await setup();
  let release!: () => void;
  transport.gateReserve = new Promise<void>((r) => {
    release = r;
  });
  controller.searchDocuments("첫", null);
  await waitFor(() =>
    expect(transport.commands[transport.commands.length - 1]?.action).toBe(
      "reserve",
    ),
  );
  const closing = workspace.navigate({ kind: "close_project" });
  await Promise.resolve();
  expect(
    transport.commands.some(
      (c) => c.action === "submit" && c.input.kind === "close",
    ),
  ).toBe(false);
  transport.gateReserve = null;
  release();
  await closing;
  const cancel = transport.commands.findIndex(
    (c) => c.action === "document_progress" && c.cancel,
  );
  const close = transport.commands.findIndex(
    (c) => c.action === "submit" && c.input.kind === "close",
  );
  expect(cancel).toBeGreaterThan(-1);
  expect(close).toBeGreaterThan(cancel);
});

it.each(["explicit", "autosave"])(
  "fix002 cancels pending search before %s save",
  async (mode) => {
    const { controller, transport } = await setup();
    await controller.open("a");
    await controller.beginEdit("a");
    transport.hold = "document_workspace";
    controller.searchDocuments("첫", null);
    await waitFor(() =>
      expect(
        [...transport.results.values()].some((r) => r.state === "pending"),
      ).toBe(true),
    );
    transport.hold = null;
    vi.useFakeTimers();
    try {
      controller.edits.update("a", (b) => ({
        ...b,
        name: { intent: "set", value: "saved A" },
      }));
      if (mode === "explicit") await controller.edits.submit("a");
      else await vi.advanceTimersByTimeAsync(1000);
      const cancel = transport.commands.findIndex(
        (c) => c.action === "document_progress" && c.cancel,
      );
      const save = transport.commands.findIndex(
        (c) =>
          c.action === "submit" &&
          c.input.kind === "document_workspace" &&
          c.input.request.action === "edit_draft" &&
          c.input.request.save,
      );
      expect(cancel).toBeGreaterThan(-1);
      expect(save).toBeGreaterThan(cancel);
      transport.completeHeld();
      await controller.shell.operations.queryAll();
    } finally {
      vi.useRealTimers();
    }
  },
);

it("fix002 validates a normal target before saving and revalidates before switching", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  vi.useFakeTimers();
  try {
    controller.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "valid A" },
    }));
    await controller.openSearchResult("b");
    const actions = transport.commands.flatMap((c) =>
      c.action === "submit" &&
      c.input.kind === "document_workspace" &&
      c.input.request.action !== "references"
        ? [c.input.request.action]
        : [],
    );
    expect(actions.slice(-3)).toEqual([
      "search_read",
      "edit_draft",
      "search_read",
    ]);
    expect(controller.snapshot().ui.active).toBe("b");
  } finally {
    vi.useRealTimers();
  }
});

it("fix002 discards a target validation after the search is cleared", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  transport.hold = "document_workspace";
  const opening = controller.openSearchResult("b");
  await waitFor(() =>
    expect(
      [...transport.results.values()].some((r) => r.state === "pending"),
    ).toBe(true),
  );
  controller.clearSearch();
  transport.hold = null;
  transport.completeHeld();
  await controller.shell.operations.queryAll();
  await opening;
  expect(controller.snapshot().ui.active).toBe("a");
  expect(
    transport.commands.some(
      (c) =>
        c.action === "submit" &&
        c.input.kind === "document_workspace" &&
        c.input.request.action === "edit_draft",
    ),
  ).toBe(false);
});

it("fix002 continues close after target validation without activating the late target", async () => {
  const { controller, workspace, transport } = await setup();
  await controller.open("a");
  transport.hold = "document_workspace";
  const opening = controller.openSearchResult("b");
  await waitFor(() =>
    expect(
      [...transport.results.values()].some((r) => r.state === "pending"),
    ).toBe(true),
  );
  await workspace.navigate({ kind: "close_project" });
  transport.hold = null;
  transport.completeHeld();
  await controller.shell.operations.queryAll();
  await opening;
  await waitFor(() =>
    expect(
      transport.commands.some(
        (c) => c.action === "submit" && c.input.kind === "close",
      ),
    ).toBe(true),
  );
  expect(controller.snapshot().ui.active).not.toBe("b");
});

it.each([false, true])(
  "fix002 cancels a pending search before app close (editor=%s)",
  async (owner) => {
    const { controller, workspace, transport } = await setup();
    if (owner) {
      await controller.open("a");
      await controller.beginEdit("a");
    }
    transport.hold = "document_workspace";
    controller.searchDocuments("첫", null);
    await waitFor(() =>
      expect(
        [...transport.results.values()].some((r) => r.state === "pending"),
      ).toBe(true),
    );
    transport.hold = null;
    transport.nativeClose(false);
    await workspace.navigate({
      kind: "close_app",
      attempt: transport.attempt!,
    });
    await waitFor(() =>
      expect(
        transport.commands.some(
          (c) => c.action === "ui_close_decision" && c.proceed,
        ),
      ).toBe(true),
    );
    const cancel = transport.commands.findIndex(
      (c) => c.action === "document_progress" && c.cancel,
    );
    const closing = transport.commands.findIndex(
      (c) => c.action === "ui_close_decision" && c.proceed,
    );
    expect(cancel).toBeGreaterThan(-1);
    expect(closing).toBeGreaterThan(cancel);
    transport.completeHeld();
    await controller.shell.operations.queryAll();
  },
);

it("fix002 flushes only the latest raw after a delayed valid target check", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  transport.hold = "document_workspace";
  const opening = controller.openSearchResult("b");
  await waitFor(() =>
    expect(
      [...transport.results.values()].some((r) => r.state === "pending"),
    ).toBe(true),
  );
  vi.useFakeTimers();
  try {
    controller.edits.update("a", (b) => ({
      ...b,
      name: { intent: "set", value: "new raw while verifying" },
    }));
    transport.hold = null;
    transport.completeHeld();
    await controller.shell.operations.queryAll();
    await opening;
    const saves = transport.commands.flatMap((c) =>
      c.action === "submit" &&
      c.input.kind === "document_workspace" &&
      c.input.request.action === "edit_draft" &&
      c.input.request.save
        ? [c.input.request.body.name]
        : [],
    );
    expect(saves).toEqual([
      { intent: "set", value: "new raw while verifying" },
    ]);
    expect(controller.snapshot().ui.active).toBe("b");
  } finally {
    vi.useRealTimers();
  }
});

it("searches saved fields, opens the existing document path, and Escape restores the tree", async () => {
  const { controller } = await setup();
  render(<DocumentWorkspace controller={controller} searchMode />);

  const input = await screen.findByLabelText(text("documents.searchInput"));
  fireEvent.change(input, { target: { value: "필드" } });

  const result = await screen.findByRole("button", {
    name: "첫 문서",
  });
  expect(result).toHaveClass("search-document-command");
  expect(result).toHaveAccessibleDescription(
    /시험 템플릿.*설명: 필드에서 찾은 저장 본문/,
  );
  expect(result.closest("table")).not.toBeNull();
  fireEvent.click(result);
  await waitFor(() => expect(controller.snapshot().ui.active).toBe("a"));

  fireEvent.keyDown(input, { key: "Escape" });
  expect(controller.snapshot().search).toBeNull();
  expect(controller.snapshot().ui.active).toBe("a");
  expect(screen.getByLabelText(text("documents.searchInput"))).toHaveValue("");
});

it("shows a Template-only flat result list without changing open tabs", async () => {
  const { controller } = await setup();
  render(<DocumentWorkspace controller={controller} searchMode />);
  const before = controller.snapshot().ui.tabs;

  fireEvent.change(
    await screen.findByLabelText(text("documents.searchTemplate")),
    { target: { value: "t" } },
  );

  await screen.findByText(`${text("documents.searchResults")} 2`);
  expect(controller.snapshot().ui.tabs).toEqual(before);
  expect(screen.getByRole("button", { name: /둘째 문서/ })).not.toBeNull();
});

it("moves search into its own area and previews attribute-only replacement without applying on Enter", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "replace_preview"
    ) {
      expect(input.request.scopes).toEqual({
        title: false,
        body: false,
        englishName: true,
        glossarySummary: false,
      });
      return {
        kind: "document_workspace",
        value: {
          kind: "replace_preview",
          preview: "preview-one",
          offset: 0,
          totalDocuments: 1,
          totalChanges: 1,
          hasMore: false,
          blockers: [],
          changes: [
            {
              document: "a",
              documentName: "첫 문서",
              templateName: "시험 템플릿",
              path: [],
              scope: "english_name",
              label: "영어 이름",
              before: "Old Name",
              after: "New Name",
              beforePrefix: "",
              beforeMatch: "Old",
              beforeSuffix: " Name",
              afterPrefix: "",
              afterMatch: "New",
              afterSuffix: " Name",
            },
          ],
        },
      };
    }
    return original(input);
  };
  const view = render(<DocumentWorkspace controller={controller} searchMode />);
  fireEvent.change(
    await screen.findByLabelText(text("documents.searchInput")),
    {
      target: { value: "Old" },
    },
  );
  fireEvent.click(
    screen.getByRole("tab", { name: text("documents.replaceTab") }),
  );
  expect(screen.getByLabelText(text("documents.replaceFind"))).toHaveValue(
    "Old",
  );
  const scopeToggle = screen.getByRole("button", {
    name: new RegExp(text("documents.replaceScope")),
  });
  expect(scopeToggle).toHaveAttribute("aria-expanded", "false");
  fireEvent.click(scopeToggle);
  expect(scopeToggle).toHaveAttribute("aria-expanded", "true");
  expect(scopeToggle.querySelector("svg")).toHaveAttribute(
    "aria-hidden",
    "true",
  );
  for (const label of [
    "documents.replaceTitle",
    "documents.replaceBody",
    "documents.replaceEnglishName",
    "documents.replaceSummary",
  ] as const)
    expect(screen.getByRole("checkbox", { name: text(label) })).toBeChecked();

  fireEvent.click(
    screen.getByRole("checkbox", { name: text("documents.replaceTitle") }),
  );
  fireEvent.click(
    screen.getByRole("checkbox", { name: text("documents.replaceBody") }),
  );
  fireEvent.click(
    screen.getByRole("checkbox", { name: text("documents.replaceSummary") }),
  );
  fireEvent.change(screen.getByLabelText(text("documents.replaceWith")), {
    target: { value: "New" },
  });
  const find = screen.getByRole("button", {
    name: text("documents.replacePreview"),
  });
  fireEvent.keyDown(screen.getByLabelText(text("documents.replaceWith")), {
    key: "Enter",
  });
  expect(
    transport.commands.filter(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "replace_apply",
    ),
  ).toHaveLength(0);
  fireEvent.click(find);
  expect(await screen.findByText("Old")).toHaveClass(
    "replace-change-highlight",
  );
  expect(screen.getByText("New")).toHaveClass("replace-change-highlight");
  expect(screen.getByText("시험 템플릿 · 영어 이름")).toBeVisible();
  const findAgain = screen.getByRole("button", {
    name: text("documents.replaceFindAgain"),
  });
  const apply = screen.getByRole("button", {
    name: text("documents.replaceAll"),
  });
  expect(findAgain.parentElement).toBe(apply.parentElement);
  expect(view.container.querySelector(".replace-document-link")).not.toBeNull();
  expect(screen.getByText(/대상 문서.*1/)).toBeVisible();
  expect(screen.getByText(/변경 위치.*1/)).toBeVisible();
});

it("discards the native replacement capability when Escape clears a preview", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "replace_preview"
    )
      return {
        kind: "document_workspace",
        value: {
          kind: "replace_preview",
          preview: "preview-escape",
          offset: 0,
          totalDocuments: 1,
          totalChanges: 1,
          hasMore: false,
          blockers: [],
          changes: [],
        },
      };
    return original(input);
  };
  render(<DocumentWorkspace controller={controller} searchMode />);
  fireEvent.click(
    screen.getByRole("tab", { name: text("documents.replaceTab") }),
  );
  const findInput = screen.getByLabelText(text("documents.replaceFind"));
  fireEvent.change(findInput, { target: { value: "old" } });
  fireEvent.click(
    screen.getByRole("button", { name: text("documents.replacePreview") }),
  );
  await waitFor(() =>
    expect(controller.snapshot().replace.preview?.preview).toBe(
      "preview-escape",
    ),
  );
  fireEvent.keyDown(findInput, { key: "Escape" });
  await waitFor(() => expect(controller.snapshot().replace.busy).toBe(false));
  expect(controller.snapshot().replace.preview).toBeNull();
  expect(
    transport.commands.some(
      (command) =>
        command.action === "submit" &&
        command.input.kind === "document_workspace" &&
        command.input.request.action === "replace_discard",
    ),
  ).toBe(true);
});

it.each(["allocation", "running"] as const)(
  "cancels replacement work from the %s boundary before discarding its capability",
  async (boundary) => {
    const { controller, transport } = await setup();
    const original = transport.workspaceResult!;
    transport.workspaceResult = (input) =>
      input.kind === "document_workspace" &&
      input.request.action === "replace_preview"
        ? {
            kind: "document_workspace",
            value: {
              kind: "replace_preview",
              preview: `preview-${boundary}`,
              offset: 0,
              totalDocuments: 0,
              totalChanges: 0,
              hasMore: false,
              blockers: [],
              changes: [],
            },
          }
        : original(input);
    render(<DocumentWorkspace controller={controller} searchMode />);
    await waitFor(() =>
      expect(controller.shell.operations.notices()).toEqual([]),
    );
    fireEvent.click(
      screen.getByRole("tab", { name: text("documents.replaceTab") }),
    );
    const findInput = screen.getByLabelText(text("documents.replaceFind"));
    fireEvent.change(findInput, { target: { value: "old" } });
    let releaseReserve = () => {};
    if (boundary === "allocation")
      transport.gateReserve = new Promise<void>((resolve) => {
        releaseReserve = resolve;
      });
    else transport.hold = "document_workspace";
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.replacePreview") }),
    );
    if (boundary === "running")
      await waitFor(() =>
        expect(
          transport.commands.some(
            (command) =>
              command.action === "submit" &&
              command.input.kind === "document_workspace" &&
              command.input.request.action === "replace_preview",
          ),
        ).toBe(true),
      );
    transport.hold = null;
    fireEvent.change(findInput, { target: { value: "new condition" } });
    if (boundary === "allocation") {
      transport.gateReserve = null;
      releaseReserve();
    }
    await waitFor(
      () => expect(controller.snapshot().replace.busy).toBe(false),
      { timeout: 5_000 },
    );
    expect(controller.snapshot().replace.preview).toBeNull();
    await waitFor(
      () => expect(controller.shell.operations.notices()).toEqual([]),
      { timeout: 5_000 },
    );
    expect(
      transport.commands.some(
        (command) =>
          (command.action === "document_progress" && command.cancel) ||
          command.action === "abandon_reservation",
      ),
    ).toBe(true);
    expect(
      transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "document_workspace" &&
          command.input.request.action === "replace_discard",
      ),
    ).toBe(true);
  },
);

it("waits for a running replacement to become terminal before project close", async () => {
  const { controller, workspace, transport } = await setup();
  controller.updateReplace({ find: "old" });
  transport.hold = "document_workspace";
  const running = controller.previewReplace();
  await waitFor(() =>
    expect(
      transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "document_workspace" &&
          command.input.request.action === "replace_preview",
      ),
    ).toBe(true),
  );
  transport.hold = null;
  await workspace.navigate({ kind: "close_project" });
  await running;
  expect(controller.snapshot().replace.preview).toBeNull();
  expect(controller.snapshot().replace.busy).toBe(false);
  expect(controller.shell.operations.notices()).toEqual([]);
  expect(
    transport.commands.some(
      (command) =>
        command.action === "submit" && command.input.kind === "close",
    ),
  ).toBe(true);
});

it("keeps the next-page action visible above a full result page and appends it", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "search"
    ) {
      const request = input.request;
      const rows = Array.from({ length: 101 }, (_, index) => ({
        id: `search-${index + 1}`,
        template: "t",
        templateName: "시험 템플릿",
        name: `검색 문서 ${index + 1}`,
        path: [],
        titleMatch: false,
        excerpt: null,
      }));
      const results = rows.slice(
        request.offset,
        request.offset + request.limit,
      );
      return {
        kind: "document_workspace",
        value: {
          kind: "search",
          generation: "paged-generation",
          offset: request.offset,
          total: rows.length,
          hasMore: request.offset + results.length < rows.length,
          missingDocuments: 0,
          templateNames: {},
          results,
        },
      };
    }
    return original(input);
  };

  render(<DocumentWorkspace controller={controller} searchMode />);
  fireEvent.change(
    await screen.findByLabelText(text("documents.searchTemplate")),
    { target: { value: "t" } },
  );

  const more = await screen.findByRole("button", {
    name: text("documents.searchMore"),
  });
  expect(controller.snapshot().search?.results).toHaveLength(100);
  fireEvent.click(more);

  await screen.findByText("검색 문서 101");
  expect(controller.snapshot().search?.results).toHaveLength(101);
  expect(
    screen.queryByRole("button", { name: text("documents.searchMore") }),
  ).toBeNull();
  const offsets = transport.commands.flatMap((command) =>
    command.action === "submit" &&
    command.input.kind === "document_workspace" &&
    command.input.request.action === "search"
      ? [command.input.request.offset]
      : [],
  );
  expect(offsets).toEqual([0, 100]);
});

it("revalidates a stale search result and preserves the already open edit", async () => {
  const { controller, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  controller.edits.update("a", (body) => ({
    ...body,
    name: { intent: "set", value: "저장 전 입력" },
    composing: true,
  }));

  const original = transport.workspaceResult!;
  let unavailable = false;
  transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "search_read"
    ) {
      unavailable = true;
      return {
        kind: "document_workspace",
        value: { kind: "search_unavailable", document: input.request.document },
      };
    }
    if (
      unavailable &&
      input.kind === "document_workspace" &&
      input.request.action === "search"
    ) {
      return {
        kind: "document_workspace",
        value: {
          kind: "search",
          generation: "refreshed-generation",
          offset: 0,
          total: 0,
          hasMore: false,
          missingDocuments: 0,
          templateNames: {},
          results: [],
        },
      };
    }
    return original(input);
  };

  render(<DocumentWorkspace controller={controller} searchMode />);
  fireEvent.change(
    await screen.findByLabelText(text("documents.searchInput")),
    {
      target: { value: "첫" },
    },
  );
  const searchTable = await screen.findByRole("table");
  fireEvent.click(
    await within(searchTable).findByRole("button", { name: "첫 문서" }),
  );

  await waitFor(() =>
    expect(controller.snapshot().error).toBe(
      text("documents.searchTargetUnavailable"),
    ),
  );
  expect(
    screen.queryByText(text("documents.searchTargetUnavailable")),
  ).not.toBeInTheDocument();
  await screen.findByText(`${text("documents.searchResults")} 0`);
  expect(controller.snapshot().ui.active).toBe("a");
  expect(controller.snapshot().ui.tabs).toContain("a");
  expect(controller.edits.entries.a.body.name).toEqual({
    intent: "set",
    value: "저장 전 입력",
  });
});

it("discards a late search response from the previous query", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) => {
    if (
      input.kind === "document_workspace" &&
      input.request.action === "search"
    ) {
      const query = input.request.query;
      return {
        kind: "document_workspace",
        value: {
          kind: "search",
          generation: "generation",
          offset: 0,
          total: 1,
          hasMore: false,
          missingDocuments: 0,
          templateNames: {},
          results: [
            {
              id: query,
              template: "t",
              templateName: "시험 템플릿",
              name: query,
              path: [],
              titleMatch: true,
              excerpt: null,
            },
          ],
        },
      };
    }
    return original(input);
  };

  transport.hold = "document_workspace";
  controller.searchDocuments("A", null);
  await waitFor(() =>
    expect(
      [...transport.results.values()].some(
        (result) => result.state === "pending",
      ),
    ).toBe(true),
  );
  transport.hold = null;
  controller.searchDocuments("B", null);
  await waitFor(() =>
    expect(
      transport.commands.some(
        (command) => command.action === "document_progress" && command.cancel,
      ),
    ).toBe(true),
  );
  await waitFor(() =>
    expect(controller.snapshot().search?.results[0]?.name).toBe("B"),
  );

  await act(async () => {
    transport.completeHeld();
    await controller.shell.operations.queryAll();
  });
  await waitFor(() => expect(controller.snapshot().searchBusy).toBe(false));
  expect(controller.snapshot().search?.results[0]?.name).toBe("B");
});

it("imports again after autosave without resending the cleaned body at the saved generation", async () => {
  const { controller, transport } = await setup();
  const original = transport.workspaceResult!;
  let synced: { generation: string; body: string } | null = null;
  let imports = 0;
  transport.workspaceResult = (input) => {
    if (input.kind === "document_workspace") {
      const q = input.request;
      if (q.action === "edit_draft") {
        const body = JSON.stringify(q.body);
        if (synced?.generation === q.generation && synced.body !== body)
          throw new Error("wrong_binding");
        synced = { generation: q.generation, body };
      }
      if (q.action === "asset_import") {
        imports++;
        return { kind: "document_workspace", value: { kind: "asset_done" } };
      }
    }
    return original(input);
  };
  await controller.beginEdit("a");
  controller.edits.field("a", "n", {
    intent: "set",
    value: { kind: "number", value: "2" },
  });
  await controller.importAsset("n", true, "a");
  await controller.edits.submit("a");
  expect(controller.edits.entries.a.body.fields).toEqual([]);
  await expect(controller.importAsset("n", true, "a")).resolves.toBeNull();
  expect(imports).toBe(2);
  await controller.endEdit("a");
});

it("rejects stale previews before reserving work when the project is reopened", async () => {
  const { controller, transport } = await setup();
  const shell = controller.shell;
  const before = shell.snapshot();
  const snapshot = vi.spyOn(shell, "snapshot");
  snapshot.mockReturnValue({
    ...before,
    projectId: "reopened-project",
    project: { ...before.project!, project: "reopened-project" },
  });
  const commands = transport.commands.length;
  await expect(
    controller.media({ action: "asset_read", asset: "old-asset" }),
  ).rejects.toThrow();
  await expect(
    controller.media({
      action: "asset_chunk",
      asset: "old-asset",
      digest: "digest",
      offset: 0,
    }),
  ).rejects.toThrow();
  expect(transport.commands).toHaveLength(commands);
  expect(shell.operations.notices()).toEqual([]);
  await controller.load();
  const original = transport.workspaceResult!;
  transport.workspaceResult = (input) =>
    input.kind === "document_workspace" && input.request.action === "asset_read"
      ? { kind: "document_workspace", value: { kind: "asset_done" } }
      : original(input);
  await expect(
    controller.media({ action: "asset_read", asset: "new-asset" }),
  ).resolves.toEqual({ kind: "asset_done" });
  expect(
    transport.commands.filter((c) => c.action === "submit").slice(-1)[0],
  ).toMatchObject({
    input: {
      project: "reopened-project",
      request: { action: "asset_read", asset: "new-asset" },
    },
  });
  snapshot.mockRestore();
});

it("keeps rollback protection with short guidance and leaves technical details for the log", async () => {
  const { controller } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  const entry = structuredClone(controller.edits.entries.a);
  entry.paused = true;
  entry.status.problem = "SaveFailed";
  entry.status.outcome = {
    kind: "write",
    session: "s",
    artifact: "a",
    disk: "rolled_back",
    changed: null,
    warnings: [],
    cleanup_failed: false,
    recovery_required: false,
    error: { code: "save_rejected", nextAction: "" },
    diagnostic: {
      stage: "Commit",
      category: null,
      sessionState: "Editing",
      lockCategory: null,
      nextAction: "",
      operationId: "op-fixture",
      observedAtUtc: "2026-09-16T00:00:00.000Z",
      failures: [
        {
          role: "Primary",
          stage: "Commit(WriteCommittedMarker)",
          category: "Io",
          transactionId: "txn-fixture",
          ioKind: "PermissionDenied",
          osCode: 5,
          context: "[]",
          secondary: [],
        },
      ],
    },
  };
  render(
    <DocumentEditor
      controller={controller}
      id="a"
      entry={entry}
      hidden={false}
      locked={false}
    />,
  );
  expect(
    screen.getByRole("button", { name: text("documentEdit.save") }),
  ).toBeDisabled();
  expect(
    screen.getByRole("button", { name: text("documentEdit.retry") }),
  ).toBeEnabled();
  expect(
    screen.queryByText(text("documentEdit.diagnostic")),
  ).not.toBeInTheDocument();
  expect(screen.queryByText("txn-fixture")).not.toBeInTheDocument();
  expect(
    screen.queryByText(/Commit\(WriteCommittedMarker\)/),
  ).not.toBeInTheDocument();
  expect(
    screen
      .getAllByText(text("documentEdit.saveStopped"))
      .some((node) => node.closest('[role="alert"]')),
  ).toBe(true);
  expect(
    screen.getByText(text("documentEdit.inputNotDeposited")),
  ).toBeVisible();
  expect(
    screen.queryByText(text("documentEdit.deposited")),
  ).not.toBeInTheDocument();
});

it.each([
  "success",
  "failed",
  "navigate",
  "hidden",
  "cancel",
  "admission-failed",
  "admission-pending",
  "observation-pending",
  "observation-failed",
] as const)(
  "force lock keeps current edit intent and admission guards: %s",
  async (scenario) => {
    const f = await setup();
    f.controller.shell.snapshot().project!.collaborative = true;
    await f.controller.open("a");
    let observed!: () => void;
    const observation = new Promise<void>((resolve) => {
      observed = resolve;
    });
    vi.spyOn(svnClient, "localDocumentStatus").mockImplementation(async () => {
      if (scenario === "observation-pending") await observation;
      if (scenario === "observation-failed")
        throw new Error("svn_status_unknown");
      return {
        path: "C:\\project\\documents\\a.json",
        local: "normal",
        properties: "none",
        remote: null,
        remoteProperties: null,
        lockOwner: "mine",
        wcLocked: true,
        workingCopyLocked: false,
        needsLock: true,
        remoteOnly: false,
      };
    });
    const originalBegin = f.controller.beginEdit.bind(f.controller);
    let admit!: () => void;
    const admission = new Promise<void>((resolve) => {
      admit = resolve;
    });
    const begin = vi
      .spyOn(f.controller, "beginEdit")
      .mockResolvedValueOnce(false)
      .mockImplementation(async (id) => {
        if (scenario === "admission-failed") return false;
        if (scenario === "admission-pending") await admission;
        return originalBegin(id);
      });
    vi.spyOn(svnClient, "documentLockOwner").mockResolvedValue({
      locked: true,
      owner: "other",
      observation: "server-token",
      localTokenPresent: false,
    } as Awaited<ReturnType<typeof svnClient.documentLockOwner>>);
    vi.spyOn(svnClient, "session").mockResolvedValue({
      connected: true,
      username: "mine",
    } as Awaited<ReturnType<typeof svnClient.session>>);
    let complete!: () => void;
    let reject!: (error: unknown) => void;
    const force = vi.spyOn(svnClient, "forceDocumentLock").mockImplementation(
      () =>
        new Promise((resolve, fail) => {
          complete = resolve;
          reject = fail;
        }),
    );
    const view = render(
      <DocumentWorkspace controller={f.controller} collaborative />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("documentEdit.begin") }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("documentEdit.begin") }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: text("svn.lockBlockedTitle"),
    });
    if (scenario === "cancel") {
      fireEvent.click(
        within(dialog).getByRole("button", { name: text("common.close") }),
      );
      expect(force).not.toHaveBeenCalled();
      expect(begin).toHaveBeenCalledTimes(1);
      return;
    }
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("svn.forceLock") }),
    );
    fireEvent.change(
      within(dialog).getByRole("textbox", { name: text("svn.forceReason") }),
      { target: { value: "owned test force" } },
    );
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("svn.forceConfirm") }),
    );
    await waitFor(() => expect(force).toHaveBeenCalledTimes(1));
    const observedBefore =
      f.controller.snapshot().svnLocalObservation?.sequence ?? 0;
    if (scenario === "navigate") await act(() => f.controller.open("b"));
    if (scenario === "hidden")
      view.rerender(
        <DocumentWorkspace controller={f.controller} collaborative hidden />,
      );
    await act(async () => {
      if (scenario === "failed") reject("svn_lock_unverified");
      else complete();
    });
    if (scenario === "observation-pending") {
      expect(begin).toHaveBeenCalledTimes(1);
      expect(f.controller.edits.entries.a).toBeUndefined();
      await act(async () => {
        observed();
      });
    }
    if (scenario === "observation-failed") {
      expect(begin).toHaveBeenCalledTimes(1);
      expect(f.controller.edits.entries.a).toBeUndefined();
      expect(
        screen.getByRole("dialog", { name: text("svn.lockBlockedTitle") }),
      ).toBeVisible();
      return;
    }
    if (scenario === "admission-pending" || scenario === "admission-failed") {
      expect(begin).toHaveBeenCalledTimes(2);
      expect(f.controller.edits.entries.a).toBeUndefined();
      expect(
        f.controller.snapshot().svnLocalObservation!.sequence,
      ).toBeGreaterThan(observedBefore);
      if (scenario === "admission-failed") return;
      await act(async () => {
        admit();
      });
    }
    if (
      scenario === "success" ||
      scenario === "admission-pending" ||
      scenario === "observation-pending"
    ) {
      await waitFor(() => expect(f.controller.edits.entries.a).toBeDefined());
      expect(begin).toHaveBeenCalledTimes(2);
      expect(
        f.controller.snapshot().svnLocalObservation!.sequence,
      ).toBeGreaterThan(observedBefore);
      expect(
        screen.queryByRole("dialog", { name: text("svn.lockBlockedTitle") }),
      ).toBeNull();
    } else {
      expect(begin).toHaveBeenCalledTimes(1);
      expect(f.controller.edits.entries.a).toBeUndefined();
      if (scenario === "failed")
        expect(f.controller.snapshot().svnLocalObservation?.sequence ?? 0).toBe(
          observedBefore,
        );
      else
        expect(
          f.controller.snapshot().svnLocalObservation!.sequence,
        ).toBeGreaterThan(observedBefore);
    }
  },
);

it("observes force ownership once before edit admission with the real SVN toolbar", async () => {
  const f = await setup();
  f.controller.shell.snapshot().project!.collaborative = true;
  await f.controller.open("a");
  const root = f.controller.shell.snapshot().root!;
  const row = {
    path: `${root}\\documents\\a.json`,
    local: "normal",
    properties: "none",
    remote: null,
    remoteProperties: null,
    lockOwner: "mine",
    wcLocked: true,
    workingCopyLocked: false,
    needsLock: true,
    remoteOnly: false,
  };
  vi.spyOn(svnClient, "status").mockResolvedValue({
    info: {
      root,
      wcRoot: root,
      url: "file:///C:/owned/project",
      repository: "file:///C:/owned",
      revision: "2",
    },
    entries: [{ ...row, lockOwner: "other", wcLocked: false }],
    serverRevision: "2",
    serverError: null,
    recovery: null,
    updateBlock: null,
  });
  vi.spyOn(svnClient, "session").mockResolvedValue({
    connected: true,
    username: "mine",
  } as Awaited<ReturnType<typeof svnClient.session>>);
  vi.spyOn(svnClient, "documentLockOwner").mockResolvedValue({
    locked: true,
    owner: "other",
    observation: "token",
    localTokenPresent: false,
  } as Awaited<ReturnType<typeof svnClient.documentLockOwner>>);
  vi.spyOn(svnClient, "forceDocumentLock").mockResolvedValue();
  let finishObservation!: () => void;
  let observationComplete = false;
  const local = vi
    .spyOn(svnClient, "localDocumentStatus")
    .mockImplementation(async () => {
      await new Promise<void>((resolve) => {
        finishObservation = resolve;
      });
      observationComplete = true;
      return row;
    });
  let finishAdmission!: () => void;
  const originalBegin = f.controller.beginEdit.bind(f.controller);
  const begin = vi
    .spyOn(f.controller, "beginEdit")
    .mockResolvedValueOnce(false)
    .mockImplementation(async (id) => {
      // Native Manager refuses a second operation while its local query owns the slot.
      if (!observationComplete) throw new Error("svn_busy");
      await new Promise<void>((resolve) => {
        finishAdmission = resolve;
      });
      return originalBegin(id);
    });
  const published = vi.fn();
  function Combined() {
    const state = useSyncExternalStore(
      f.controller.subscribe,
      f.controller.snapshot,
    );
    return (
      <>
        <SvnToolbar
          root={root}
          collaborative
          generation={f.controller.shell.projectGeneration()}
          controller={f.workspace}
          openConnection={vi.fn()}
          onUpdatingChange={vi.fn()}
          onConnectionVerified={vi.fn()}
          connectionFailed={false}
          sessionRevision={0}
          blocked={false}
          localObservation={state.svnLocalObservation}
          onStatus={published}
        />
        <DocumentWorkspace controller={f.controller} collaborative />
      </>
    );
  }
  render(<Combined />);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("documentEdit.begin") }),
    ).toBeEnabled(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: text("documentEdit.begin") }),
  );
  const dialog = await screen.findByRole("dialog", {
    name: text("svn.lockBlockedTitle"),
  });
  fireEvent.click(
    within(dialog).getByRole("button", { name: text("svn.forceLock") }),
  );
  fireEvent.change(
    within(dialog).getByRole("textbox", { name: text("svn.forceReason") }),
    { target: { value: "owned serialized force" } },
  );
  fireEvent.click(
    within(dialog).getByRole("button", { name: text("svn.forceConfirm") }),
  );
  await waitFor(() => expect(local).toHaveBeenCalledTimes(1));
  expect(begin).toHaveBeenCalledTimes(1);
  await act(async () => {
    finishObservation();
  });
  await waitFor(() => expect(begin).toHaveBeenCalledTimes(2));
  expect(f.controller.edits.entries.a).toBeUndefined();
  expect(
    published.mock.calls[published.mock.calls.length - 1]?.[0]?.entries.some(
      (entry: typeof row) => entry.wcLocked && entry.lockOwner === "mine",
    ),
  ).toBe(true);
  expect(local).toHaveBeenCalledTimes(1);
  await act(async () => {
    finishAdmission();
  });
  await waitFor(() => expect(f.controller.edits.entries.a).toBeDefined());
  expect(
    screen.queryByRole("dialog", { name: text("svn.lockBlockedTitle") }),
  ).toBeNull();
});

it("blocks preview remount before actual edit owner release during project close", async () => {
  const { controller, workspace, transport } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  let remount: Promise<unknown> | undefined;
  let once = false;
  const unsubscribe = controller.subscribe(() => {
    if (!once && !controller.hasOwners()) {
      once = true;
      remount = controller
        .media({ action: "asset_read", asset: "sample" })
        .catch(() => "stale");
    }
  });
  await workspace.navigate({ kind: "close_project" });
  await waitFor(() => expect(once).toBe(true));
  await expect(remount).resolves.toBe("stale");
  expect(
    transport.commands.some(
      (c) =>
        c.action === "submit" &&
        c.input.kind === "document_workspace" &&
        c.input.request.action === "asset_read",
    ),
  ).toBe(false);
  await waitFor(() =>
    expect(controller.shell.operations.notices()).toEqual([]),
  );
  unsubscribe();
});

it("cancelled project close retains dirty raw and restores the preview lifetime", async () => {
  const { controller, workspace } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  controller.edits.update("a", (body) => ({
    ...body,
    fields: [
      {
        field: "n",
        value: { intent: "set", value: { kind: "number", value: "-" } },
      },
    ],
  }));
  await workspace.navigate({ kind: "close_project" });
  await waitFor(() => expect(controller.snapshot().editPrompt).toBe("a"));
  expect(controller.snapshot().previewClosing).toBe(true);
  controller.cancelEditClose();
  expect(controller.snapshot().previewClosing).toBe(false);
  expect(controller.edits.entries.a.body.fields[0].value).toEqual({
    intent: "set",
    value: { kind: "number", value: "-" },
  });
});

it("closing only a creation tab keeps project previews available", async () => {
  const { controller, transport } = await setup();
  await controller.begin("t");
  controller.requestCreationClose();
  await controller.resolveClose("discard");
  expect(controller.snapshot().draft).toBeNull();
  expect(controller.snapshot().previewClosing).toBe(false);
  const previous = transport.workspaceResult;
  transport.workspaceResult = (input) =>
    input.kind === "document_workspace" && input.request.action === "asset_read"
      ? { kind: "document_workspace", value: { kind: "asset_done" } }
      : previous?.(input);
  await expect(
    controller.media({ action: "asset_read", asset: "sample" }),
  ).resolves.toEqual({ kind: "asset_done" });
});

it("removes hidden editing players across tabs, trash and parent screens while preserving input DOM", async () => {
  youtubeConsentStore().choose(true);
  const { controller } = await setup("Url");
  await controller.open("a");
  await controller.beginEdit("a");
  const { container, rerender } = render(
    <DocumentWorkspace controller={controller} />,
  );
  const input = container.querySelector('input[type="url"]')!;
  const player = container.querySelector("iframe");
  expect(player).not.toBeNull();
  await waitFor(() => expect(controller.snapshot().busy).toBe(false));
  await act(() => controller.open("b"));
  expect(controller.snapshot().ui.active).toBe("b");
  expect(container.querySelector(".document-editor iframe")).toBeNull();
  expect(player!.isConnected).toBe(false);
  expect(container.querySelector('input[type="url"]')).toBe(input);
  await act(() => controller.open("a"));
  expect(container.querySelector("iframe")).not.toBe(player);
  rerender(<DocumentWorkspace controller={controller} hidden />);
  expect(container.querySelector("iframe")).toBeNull();
  rerender(<DocumentWorkspace controller={controller} trash />);
  expect(container.querySelector("iframe")).toBeNull();
  rerender(<DocumentWorkspace controller={controller} />);
  expect(container.querySelector("iframe")).not.toBeNull();
  expect(container.querySelector('input[type="url"]')).toBe(input);
  await act(async () => controller.suspendPreviews());
  expect(container.querySelector("iframe")).toBeNull();
  expect(controller.edits.entries.a.status.owner).toBe("edit-a");
});

it("recreates the read player when two documents use the same URL", async () => {
  youtubeConsentStore().choose(true);
  const { controller } = await setup("Url");
  await controller.open("a");
  const { container } = render(<DocumentWorkspace controller={controller} />);
  await waitFor(() => expect(controller.snapshot().busy).toBe(false));
  const first = container.querySelector("iframe");
  expect(first).not.toBeNull();
  await act(() => controller.open("b"));
  const second = container.querySelector("iframe");
  expect(second).not.toBeNull();
  expect(second).not.toBe(first);
});

it("keeps editor DOM and invalid raw across tabs, then saves without remounting focused input", async () => {
  const { controller } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  render(<DocumentWorkspace controller={controller} />);
  const name = screen.getByRole("textbox", {
    name: text("documentEdit.name"),
  }) as HTMLInputElement;
  name.focus();
  name.setSelectionRange(1, 1);
  fireEvent.change(name, { target: { value: "수정 이름" } });
  fireEvent.compositionStart(name);
  await new Promise((r) => setTimeout(r, 1100));
  expect(controller.edits.entries.a.status.saved_generation).toBe("1");
  fireEvent.compositionEnd(name);
  await waitFor(
    () => expect(controller.edits.entries.a.status.read.name).toBe("수정 이름"),
    { timeout: 2500 },
  );
  expect(screen.getByRole("textbox", { name: text("documentEdit.name") })).toBe(
    name,
  );
  expect(document.activeElement).toBe(name);
  const number = document.getElementById("edit-a-n") as HTMLInputElement;
  fireEvent.change(number, { target: { value: "-" } });
  await act(() => controller.open("b"));
  await act(() => controller.beginEdit("b"));
  await act(() => controller.open("a"));
  expect(document.getElementById("edit-a-n")).toBe(number);
  expect(number.value).toBe("-");
  await act(() => controller.endEdit("a"));
  expect(controller.snapshot().editPrompt).toBe("a");
  await act(() => controller.depositEditClose());
  expect(controller.edits.entries.a).toBeUndefined();
});
it("restores each tab scroll and text selection after the read/active transition", async () => {
  const { controller } = await setup();
  await controller.open("a");
  await controller.beginEdit("a");
  await controller.open("b");
  await controller.beginEdit("b");
  await controller.open("a");
  const view = render(<DocumentWorkspace controller={controller} />);
  const body = view.container.querySelector(".document-body") as HTMLElement;
  const names = () =>
    screen.getByRole("textbox", {
      name: text("documentEdit.name"),
    }) as HTMLInputElement;
  const a = names();
  a.focus();
  a.setSelectionRange(1, 3, "backward");
  body.scrollTop = 180;
  fireEvent.scroll(body);
  const bTab = document.getElementById("document-tab-b")!;
  await waitFor(() => expect(bTab).not.toBeDisabled());
  bTab.focus();
  fireEvent.click(bTab);
  await waitFor(() => expect(controller.snapshot().ui.active).toBe("b"));
  expect(body.scrollTop).toBe(0);
  const b = names();
  b.focus();
  b.setSelectionRange(2, 2);
  body.scrollTop = 40;
  fireEvent.scroll(body);
  const aTab = document.getElementById("document-tab-a")!;
  aTab.focus();
  fireEvent.click(aTab);
  await waitFor(() => expect(controller.snapshot().ui.active).toBe("a"));
  expect(document.activeElement).toBe(a);
  expect([a.selectionStart, a.selectionEnd, a.selectionDirection]).toEqual([
    1,
    3,
    "backward",
  ]);
  expect(body.scrollTop).toBe(180);
  bTab.focus();
  fireEvent.click(bTab);
  await waitFor(() => expect(document.activeElement).toBe(b));
  expect(b.selectionStart).toBe(2);
  expect(body.scrollTop).toBe(40);
});
function healthGate() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("project check lifetime", () => {
  it("revalidates on opening, retry and reopening; one completed side never reports success", async () => {
    const f = await setup();
    f.list.issueStatus = "complete";
    await f.controller.load();
    const shell = f.controller.shell;
    const run = shell.operations.run.bind(shell.operations);
    const entered = healthGate();
    const release = healthGate();
    let hold = true;
    const original = f.transport.workspaceResult!;
    f.transport.workspaceResult = (input) => {
      const result = original(input);
      if (
        input.kind === "document_workspace" &&
        input.request.action === "list" &&
        result?.kind === "document_workspace" &&
        result.value.kind === "list"
      )
        return {
          ...result,
          value: {
            ...result.value,
            problem: "document_invalid",
            issueStatus: "unavailable",
          },
        };
      return result;
    };
    vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
      const result = await run(...args);
      if (
        hold &&
        args[0].kind === "document_workspace" &&
        args[0].request.action === "list"
      ) {
        entered.resolve();
        await release.promise;
      }
      return result;
    });
    shell.showHealth();
    await entered.promise;
    expect(shell.snapshot().health).toMatchObject({
      phase: "checking",
      message: null,
      inspection: null,
      check: { documents: "checking" },
    });
    release.resolve();
    await waitFor(() => expect(shell.snapshot().health?.phase).toBe("ready"));
    expect(shell.snapshot().health?.check?.documents).toBe("unverified");
    expect(shell.snapshot().health?.message).toBe(text("health.checkPartial"));
    hold = false;
    await shell.inspectAssets();
    expect(shell.snapshot().health?.check?.documents).toBe("unverified");
    shell.closeHealth();
    shell.showHealth();
    await waitFor(() => expect(shell.snapshot().health?.phase).toBe("ready"));
    expect(shell.snapshot().health?.check?.documents).toBe("unverified");
    f.transport.workspaceResult = original;
    await shell.inspectAssets();
    expect(shell.snapshot().health?.check?.documents).toBe("verified");
    expect(shell.snapshot().health?.message).toBe(text("health.checkComplete"));
  });

  it.each(["complete", "incomplete", "failed"] as const)(
    "waits for resources and preserves their %s result",
    async (outcome) => {
      const f = await setup();
      f.list.issueStatus = "complete";
      const shell = f.controller.shell;
      const run = shell.operations.run.bind(shell.operations);
      const entered = healthGate();
      const release = healthGate();
      f.transport.assetInspection.complete = outcome !== "incomplete";
      vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
        const result = await run(...args);
        if (args[0].kind === "asset_inspect") {
          entered.resolve();
          await release.promise;
          if (outcome === "failed")
            throw new Error("synthetic inspection failure");
        }
        return result;
      });
      shell.showHealth();
      await entered.promise;
      expect(shell.snapshot().health?.phase).toBe("checking");
      expect(shell.snapshot().health?.message).toBeNull();
      release.resolve();
      await waitFor(() =>
        expect(shell.snapshot().health?.phase).toBe(
          outcome === "failed" ? "failed" : "ready",
        ),
      );
      expect(shell.snapshot().health?.message).toBe(
        outcome === "complete"
          ? text("health.checkComplete")
          : outcome === "incomplete"
            ? text("health.checkPartial")
            : null,
      );
    },
  );

  it.each([
    [true, "list"],
    [false, "list"],
    [true, "assets"],
    [false, "assets"],
  ] as const)(
    "discards a closed check without reading A or overwriting B (old first=%s, delayed=%s)",
    async (oldFirst, delayed) => {
      const f = await setup();
      f.list.issueStatus = "complete";
      await f.controller.open("a");
      const shell = f.controller.shell;
      const run = shell.operations.run.bind(shell.operations);
      const entered = [healthGate(), healthGate()];
      const release = [healthGate(), healthGate()];
      let listIndex = 0;
      const requests: string[] = [];
      vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
        const result = await run(...args);
        if (args[0].kind === "document_workspace") {
          requests.push(args[0].request.action);
        }
        if (
          (delayed === "list" &&
            args[0].kind === "document_workspace" &&
            args[0].request.action === "list") ||
          (delayed === "assets" && args[0].kind === "asset_inspect")
        ) {
          const index = listIndex++;
          entered[index].resolve();
          await release[index].promise;
        }
        return result;
      });
      const oldCheck = shell.showHealth();
      await entered[0].promise;
      shell.closeHealth();
      await f.controller.open("b");
      const selected = f.controller.snapshot().read;
      const newCheck = shell.showHealth();
      await entered[1].promise;
      const currentId = shell.snapshot().health?.check?.id;
      release[oldFirst ? 0 : 1].resolve();
      if (oldFirst) {
        await Promise.resolve();
        expect(shell.snapshot().health?.phase).toBe("checking");
      } else
        await waitFor(() =>
          expect(shell.snapshot().health?.phase).toBe("ready"),
        );
      release[oldFirst ? 1 : 0].resolve();
      await Promise.all([oldCheck, newCheck]);
      await waitFor(() => expect(shell.snapshot().health?.phase).toBe("ready"));
      expect(shell.snapshot().health?.check?.id).toBe(currentId);
      expect(f.controller.snapshot().ui.active).toBe("b");
      expect(f.controller.snapshot().read).toBe(selected);
      expect(f.controller.snapshot().read?.id).toBe("b");
      expect(requests.filter((action) => action === "read")).toHaveLength(1);
    },
  );

  it.each([true, false])(
    "does not publish a pending list after an editor lifetime change (owner remains=%s)",
    async (remains) => {
      const f = await setup();
      f.list.issueStatus = "complete";
      await f.controller.open("a");
      const shell = f.controller.shell;
      const run = shell.operations.run.bind(shell.operations);
      const entered = healthGate();
      const release = healthGate();
      vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
        const result = await run(...args);
        if (
          args[0].kind === "document_workspace" &&
          args[0].request.action === "list"
        ) {
          entered.resolve();
          await release.promise;
        }
        return result;
      });
      const checking = f.controller.refreshForHealth();
      await entered.promise;
      await f.controller.beginEdit("a");
      if (!remains) await f.controller.endEdit("a");
      const before = f.controller.snapshot();
      const owner = f.controller.edits.entries.a;
      release.resolve();
      expect(await checking).toBe(false);
      expect(f.controller.snapshot().list).toBe(before.list);
      expect(f.controller.snapshot().read).toBe(before.read);
      expect(f.controller.edits.entries.a).toBe(owner);
    },
  );

  it("rejects a previous project's list after the next project is loaded", async () => {
    const f = await setup();
    await f.controller.open("a");
    const shell = f.controller.shell;
    const run = shell.operations.run.bind(shell.operations);
    const entered = healthGate();
    const release = healthGate();
    let hold = true;
    vi.spyOn(shell.operations, "run").mockImplementation(async (...args) => {
      const result = await run(...args);
      if (
        hold &&
        args[0].kind === "document_workspace" &&
        args[0].request.action === "list"
      ) {
        entered.resolve();
        await release.promise;
      }
      return result;
    });
    const checking = f.controller.refreshForHealth();
    await entered.promise;
    hold = false;
    const state = shell.snapshot();
    vi.spyOn(shell, "snapshot").mockImplementation(() => ({
      ...state,
      projectId: "next-project",
    }));
    vi.spyOn(shell, "projectGeneration").mockReturnValue(9);
    await f.controller.load();
    const next = f.controller.snapshot();
    release.resolve();
    expect(await checking).toBe(false);
    expect(f.controller.snapshot().list).toBe(next.list);
    expect(f.controller.snapshot().read).toBe(next.read);
    expect(f.controller.snapshot().ui).toBe(next.ui);
  });
});

describe("document workspace", () => {
  it.each(["committed", "retry", "reload", "unavailable"])(
    "생성 성공 뒤 목록 검증 실패를 보존하고 %s 경로에서 복구한다",
    async (recovery) => {
      const f = await setup();
      const original = f.transport.workspaceResult;
      f.transport.workspaceResult = (input) => {
        const result = original!(input);
        if (
          input.kind === "document_workspace" &&
          input.request.action === "draft"
        ) {
          f.list.documents.push({
            id: "created",
            template: "t",
            name: "즉시 공개",
          });
          f.list.layout.rootOrder.push("created");
          f.list.layout.nodes.created = {
            parentId: null,
            childOrder: [],
            state: "active",
            trash: null,
          };
          f.list.issueStatus = "complete";
          f.list.unverifiedDocuments = [];
        }
        return result;
      };
      await f.controller.begin("t");
      f.controller.edit((body) => ({ ...body, name: "즉시 공개" }));
      f.setOutcome({
        kind: "write",
        session: "session",
        artifact: "created",
        disk: "committed",
        recovery_required: false,
        cleanup_failed: false,
        error: null,
        diagnostic: {
          stage: "complete",
          category: null,
          sessionState: "ReadOnly",
          lockCategory: null,
          nextAction: "",
        },
        changed: true,
        warnings: [],
      });
      const before = f.transport.commands.length;
      const run = f.controller.shell.operations.run.bind(
        f.controller.shell.operations,
      );
      const failing =
        recovery === "committed"
          ? null
          : vi
              .spyOn(f.controller.shell.operations, "run")
              .mockImplementation(async (...args) => {
                if (
                  args[0].kind === "document_workspace" &&
                  args[0].request.action === "list"
                ) {
                  if (recovery === "unavailable") {
                    const result = await run(...args);
                    return {
                      ...result,
                      result: {
                        kind: "document_workspace",
                        value: {
                          ...f.list,
                          documents: [],
                          issueStatus: "unavailable",
                          problem: "document_invalid",
                        },
                      },
                    };
                  }
                  throw new Error(
                    "synthetic list failure after committed create",
                  );
                }
                return run(...args);
              });
      await f.controller.submit();
      if (failing) {
        expect(f.controller.snapshot().list?.issueStatus).toBe("partial");
        expect(f.controller.snapshot().list?.unverifiedDocuments).toContain(
          "created",
        );
        expect(f.controller.snapshot().read?.id).toBe("created");
        expect(f.controller.snapshot().ui.active).toBe("created");
        expect(f.controller.snapshot().error).toBeNull();
        failing.mockRestore();
        if (recovery === "reload") await f.controller.load();
        else expect(await f.controller.refreshForHealth()).toBe(true);
      }
      const requests = f.transport.commands
        .slice(before)
        .flatMap((command) =>
          command.action === "submit" &&
          command.input.kind === "document_workspace"
            ? [command.input.request.action]
            : [],
        );
      expect(requests).toEqual(
        recovery === "unavailable"
          ? ["draft", "release", "list", "list"]
          : recovery === "retry"
            ? ["draft", "release", "list"]
            : ["draft", "release", "list", "read"],
      );
      const documents = f.controller.snapshot().list?.documents ?? [];
      expect(documents[documents.length - 1]?.id).toBe("created");
      expect(f.controller.snapshot().read?.id).toBe("created");
      expect(f.controller.snapshot().ui.active).toBe("created");
      expect(f.controller.snapshot().list?.issueStatus).toBe("complete");
      expect(f.controller.snapshot().list?.unverifiedDocuments).toEqual([]);
    },
  );
  it("복원 후 이름 수정·오류 응답에도 부모 선택을 유지하고 오류 요약을 한 번 표시한다", async () => {
    const f = await setup();
    render(<DocumentWorkspace controller={f.controller} />);
    await waitFor(() => expect(f.controller.snapshot().busy).toBe(false));
    await act(() =>
      f.controller.restore({
        row: {
          locatorFingerprint: "fixture",
          key: {
            projectFingerprint: "fixture",
            draftId: "draft",
            generation: "1",
          },
          depositId: "deposit",
          payloadKind: "new_document",
          payloadDigest: "digest",
          error: null,
        },
        version: "version",
        createdAtUtc: null,
        artifact: null,
      }),
    );
    const name = screen.getByLabelText(text("documents.name") + " *");
    const create = screen.getByRole("button", {
      name: text("documents.create"),
    });
    // backend 복원 DTO는 새 입력 세션을 composing:false로 보낸다.
    expect(create).toBeEnabled();
    fireEvent.compositionStart(name);
    expect(create).toBeDisabled();
    fireEvent.compositionEnd(name);
    expect(create).toBeEnabled();
    fireEvent.keyDown(name, { key: "Enter" });
    expect(f.saves).toBe(0);
    expect(screen.getByLabelText(text("documents.parent"))).toBeInTheDocument();
    fireEvent.change(name, {
      target: { value: "수정한 이름" },
    });
    fireEvent.change(screen.getByLabelText(text("documents.parent")), {
      target: { value: "a" },
    });
    await act(() => f.controller.submit());
    expect(screen.getByLabelText(text("documents.parent"))).toHaveValue("a");
    expect(screen.getAllByText(text("documents.invalid"))).toHaveLength(1);
  });
  it("문서 ID 탭 재사용·순서·닫기·UI 복원 및 깨진 개인 상태를 분리한다", async () => {
    const f = await setup();
    await f.controller.open("a");
    await f.controller.open("b");
    await f.controller.open("a");
    expect(f.controller.snapshot().ui.tabs).toEqual(["a", "b"]);
    f.controller.reorderTab("a", 1);
    expect(f.controller.snapshot().ui.tabs).toEqual(["b", "a"]);
    await f.controller.closeTab("a");
    expect(f.controller.snapshot().ui.active).toBe("b");
    const reopened = new DocumentController(f.controller.shell);
    await reopened.load();
    expect(reopened.snapshot().ui.tabs).toEqual(["b"]);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    await reopened.open("a");
    expect(reopened.snapshot().uiError).toBe(true);
    expect(reopened.snapshot().ui.active).toBe("a");
    expect(reopened.snapshot().error).toBeNull();
  });
  it("문서 선택 중에는 장기 작업 안내 대신 탐색 상태로 분류한다", async () => {
    const f = await setup();
    f.transport.hold = "document_workspace";
    const opening = f.controller.open("a");
    await waitFor(() =>
      expect(
        [...f.transport.results.values()].some((r) => r.state === "pending"),
      ).toBe(true),
    );
    expect(f.controller.snapshot().busy).toBe(true);
    expect(f.controller.snapshot().navigationBusy).toBe(true);
    f.transport.completeHeld();
    await opening;
    expect(f.controller.snapshot().navigationBusy).toBe(false);
  });
  it("외부 파일 갱신 뒤 열린 문서 이름을 다시 읽고 탭과 선택을 유지한다", async () => {
    const f = await setup();
    await f.controller.open("a");
    await f.controller.open("b");
    await f.controller.open("a");
    const before = f.controller.snapshot().ui;
    f.list.documents[0].name = "서버에서 갱신된 문서";
    await f.controller.load();
    expect(f.controller.snapshot().ui.tabs).toEqual(before.tabs);
    expect(f.controller.snapshot().ui.active).toBe(before.active);
    expect(f.controller.snapshot().read?.name).toBe("서버에서 갱신된 문서");
  });
  it("깨진 개인 상태의 경고는 안전한 기본 상태를 다시 저장해도 유지한다", async () => {
    const f = await setup();
    localStorage.setItem("worldbuild.document-ui.fixture", "{broken");
    const restored = new DocumentController(f.controller.shell);
    await restored.load();
    expect(restored.snapshot().ui.tabs).toEqual([]);
    expect(restored.snapshot().uiError).toBe(true);
    expect(restored.snapshot().list?.documents).toHaveLength(2);
  });
  it("탐색 패널 폭·접힘을 프로젝트별로 복원하고 편집 DOM과 focus를 보존한다", async () => {
    const f = await setup();
    await f.controller.open("a");
    await f.controller.beginEdit("a");
    const view = render(<DocumentWorkspace controller={f.controller} />);
    const input = screen.getByRole("textbox", {
      name: text("documentEdit.name"),
    });
    input.focus();
    const separator = screen.getByRole("separator", {
      name: text("documents.navigationResize"),
    });
    Object.defineProperties(separator, {
      setPointerCapture: { configurable: true, value: vi.fn() },
      releasePointerCapture: { configurable: true, value: vi.fn() },
    });
    const initialWidth = f.controller.snapshot().ui.navigationWidth;
    fireEvent.pointerDown(separator, { pointerId: 1, clientX: 280 });
    fireEvent.pointerMove(separator, { pointerId: 1, clientX: 316 });
    fireEvent.pointerUp(separator, { pointerId: 1, clientX: 316 });
    expect(f.controller.snapshot().ui.navigationWidth).toBe(initialWidth + 36);
    fireEvent.keyDown(separator, { key: "End" });
    const maximum = Number(separator.getAttribute("aria-valuemax"));
    expect(f.controller.snapshot().ui.navigationWidth).toBe(maximum);
    separator.focus();
    expect(document.activeElement).toBe(separator);
    const collapse = screen.getByRole("button", {
      name: text("documents.navigationCollapse"),
    });
    collapse.focus();
    fireEvent.click(collapse);
    expect(f.controller.snapshot().ui.navigationCollapsed).toBe(true);
    expect(view.container.querySelector(".document-tree")).toHaveAttribute(
      "hidden",
    );
    expect(
      screen.getByRole("textbox", { name: text("documentEdit.name") }),
    ).toBe(input);
    const expand = screen.getByRole("button", {
      name: text("documents.navigationExpand"),
    });
    expect(document.activeElement).toBe(expand);
    fireEvent.click(expand);
    expect(f.controller.snapshot().ui.navigationCollapsed).toBe(false);
    const restored = new DocumentController(f.controller.shell);
    await restored.load();
    expect(restored.snapshot().ui.navigationWidth).toBe(maximum);
    expect(restored.snapshot().ui.navigationCollapsed).toBe(false);
  });
  it("글로서리는 처음 접혀 있고 키보드 폭·필터를 복원하며 편집 owner와 DOM을 보존한다", async () => {
    const f = await setup();
    await f.controller.open("a");
    await f.controller.beginEdit("a");
    const owner = f.controller.edits.entries.a.status.owner;
    const view = render(<DocumentWorkspace controller={f.controller} />);
    const input = screen.getByRole("textbox", {
      name: text("documentEdit.name"),
    });
    fireEvent.change(input, { target: { value: "작성 중 이름" } });
    input.focus();
    expect(f.controller.snapshot().ui.glossaryCollapsed).toBe(true);

    act(() => f.controller.toggleGlossary());
    expect(f.controller.snapshot().ui.glossaryCollapsed).toBe(false);
    const glossary = screen.getByRole("complementary", {
      name: text("glossary.title"),
    });
    expect(glossary).toBeVisible();
    expect(
      within(glossary).getByRole("button", { name: /첫 문서/ }),
    ).toBeVisible();
    expect(
      within(glossary).getByRole("button", { name: /둘째 문서/ }),
    ).toBeVisible();
    expect(
      screen.getByRole("textbox", { name: text("documentEdit.name") }),
    ).toBe(input);
    expect(input).toHaveValue("작성 중 이름");
    expect(f.controller.edits.entries.a.status.owner).toBe(owner);

    const separator = screen.getByRole("separator", {
      name: text("glossary.resize"),
    });
    Object.assign(separator, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });
    fireEvent.pointerDown(separator, { pointerId: 1, clientX: 280 });
    expect(document.activeElement).toBe(separator);
    fireEvent.pointerUp(separator, { pointerId: 1, clientX: 280 });
    fireEvent.keyDown(separator, { key: "Home" });
    expect(f.controller.snapshot().ui.glossaryWidth).toBe(
      Number(separator.getAttribute("aria-valuemin")),
    );
    fireEvent.keyDown(separator, { key: "ArrowLeft" });
    expect(f.controller.snapshot().ui.glossaryWidth).toBe(
      Number(separator.getAttribute("aria-valuemin")) + 12,
    );
    fireEvent.change(
      screen.getByRole("combobox", { name: text("glossary.templateFilter") }),
      { target: { value: "t" } },
    );
    expect(f.controller.snapshot().ui.glossaryTemplate).toBe("t");

    fireEvent.click(
      screen.getByRole("button", { name: text("glossary.collapse") }),
    );
    expect(f.controller.snapshot().ui.glossaryCollapsed).toBe(true);
    expect(view.container.querySelector(".document-glossary")).toBeNull();
    expect(
      screen.getByRole("button", { name: text("glossary.open") }),
    ).toBeVisible();
    const restored = new DocumentController(f.controller.shell);
    await restored.load();
    expect(restored.snapshot().ui.glossaryWidth).toBe(232);
    expect(restored.snapshot().ui.glossaryCollapsed).toBe(true);
    expect(restored.snapshot().ui.glossaryTemplate).toBe("t");
  });
  it("이전 개인 화면 상태는 새 패널 필드 없이도 안전한 기본값으로 이행한다", async () => {
    const f = await setup();
    localStorage.setItem(
      "worldbuild.document-ui.fixture",
      JSON.stringify({ tabs: ["a"], active: "a", collapsed: [], scroll: 7 }),
    );
    const restored = new DocumentController(f.controller.shell);
    await restored.load();
    expect(restored.snapshot().ui.navigationCollapsed).toBe(false);
    expect(restored.snapshot().ui.navigationWidth).toBeGreaterThanOrEqual(220);
    expect(restored.snapshot().ui.glossaryCollapsed).toBe(true);
    expect(restored.snapshot().ui.glossaryWidth).toBeGreaterThanOrEqual(220);
    expect(restored.snapshot().ui.glossaryTemplate).toBeNull();
    expect(restored.snapshot().uiError).toBe(false);
  });
  it("10k 트리는 표시 행만 유지하고 스크롤 뒤 stable ID 대상에 접근한다", async () => {
    const f = await setup();
    for (let index = 2; index < 10_000; index++) {
      const id = `document-${index.toString().padStart(5, "0")}`;
      f.list.documents.push({ id, template: "t", name: `문서 ${index}` });
      f.list.layout.rootOrder.push(id);
      f.list.layout.nodes[id] = {
        parentId: null,
        childOrder: [],
        state: "active",
        trash: null,
      };
    }
    await f.controller.load();
    expect(f.controller.snapshot().busy).toBe(false);
    expect(f.controller.snapshot().list?.problem).toBeNull();
    expect(f.controller.hasOwners()).toBe(false);
    expect(f.controller.snapshot().prompt).toBe(false);
    expect(f.controller.snapshot().editPrompt).toBeNull();
    expect(f.controller.shell.snapshot().closing).toBe(false);
    const view = render(<DocumentWorkspace controller={f.controller} />);
    expect(view.container.querySelector(".document-tree-list")).toHaveStyle({
      "--document-tree-row-height": `${DOCUMENT_TREE_ROW_HEIGHT}px`,
    });
    expect(view.container.querySelectorAll(".tree-name").length).toBeLessThan(
      100,
    );
    expect(document.getElementById("tree-name-document-09999")).toBeNull();
    const tree = view.container.querySelector(".document-tree")!;
    Object.defineProperty(tree, "scrollTop", {
      configurable: true,
      value: 9_999 * DOCUMENT_TREE_ROW_HEIGHT,
      writable: true,
    });
    fireEvent.scroll(tree);
    await waitFor(() =>
      expect(
        document.getElementById("tree-name-document-09999"),
      ).toBeInTheDocument(),
    );
    expect(view.container.querySelectorAll(".tree-name").length).toBeLessThan(
      100,
    );
  });
  it.each([{ unplaced: [] }, { unplaced: ["unregistered"] }])(
    "배치 목록에서 빠진 문서는 미배치 응답 $unplaced에서도 트리에 표시한다",
    async ({ unplaced }) => {
      const f = await setup();
      f.list.documents.push({
        id: "unregistered",
        template: "deleted-template",
        name: "삭제 Template 보호 확인",
      });
      f.list.unplaced = unplaced;
      await f.controller.load();
      const openDocument = vi.fn();
      expect(
        f.controller.snapshot().list?.documents.map((item) => item.id),
      ).toEqual(["a", "b", "unregistered"]);
      const view = render(
        <DocumentTree
          controller={f.controller}
          locked={false}
          structureLocked={false}
          createChild={() => {}}
          openDocument={openDocument}
          viewportHeight={640}
        />,
      );
      expect(f.controller.snapshot().list?.unplaced).toEqual(unplaced);
      expect(f.controller.snapshot().list?.layout.rootOrder).toEqual([
        "a",
        "b",
      ]);
      expect(f.controller.snapshot().list?.layout.nodes).not.toHaveProperty(
        "unregistered",
      );
      expect(
        [...view.container.querySelectorAll(".tree-name")].map(
          (item) => item.id,
        ),
      ).toEqual(["tree-name-a", "tree-name-b", "tree-name-unregistered"]);
      expect(
        document.getElementById("tree-name-unregistered"),
      ).toHaveTextContent("삭제 Template 보호 확인");
      expect(
        document
          .getElementById("tree-name-unregistered")
          ?.closest(".document-tree-row")
          ?.querySelector(".tree-menu"),
      ).toBeNull();
      fireEvent.click(document.getElementById("tree-name-unregistered")!);
      expect(openDocument).toHaveBeenCalledWith("unregistered");
    },
  );
  it("가상 트리 드래그 원본은 overscan 밖 스크롤 중에도 한 행만 보존한다", async () => {
    const f = await setup();
    for (let index = 2; index < 1_000; index++) {
      const id = `drag-${index.toString().padStart(4, "0")}`;
      f.list.documents.push({ id, template: "t", name: `드래그 ${index}` });
      f.list.layout.rootOrder.push(id);
      f.list.layout.nodes[id] = {
        parentId: null,
        childOrder: [],
        state: "active",
        trash: null,
      };
    }
    await f.controller.load();
    const tree = () => (
      <DocumentTree
        controller={f.controller}
        locked={false}
        structureLocked={false}
        createChild={() => {}}
        openDocument={() => {}}
        viewportHeight={640}
      />
    );
    const view = render(tree());
    const source = document
      .getElementById("tree-name-a")!
      .closest(".document-tree-row")!;
    const setData = vi.fn();
    fireEvent.dragStart(source, {
      dataTransfer: { effectAllowed: "none", setData },
    });
    expect(setData).toHaveBeenCalledWith("text/plain", "a");
    act(() => f.controller.scroll(999 * DOCUMENT_TREE_ROW_HEIGHT));
    view.rerender(tree());
    await waitFor(() =>
      expect(
        document.getElementById("tree-name-a")!.closest(".drag-source-pinned"),
      ).not.toBeNull(),
    );
    expect(view.container.querySelectorAll(".drag-source-pinned")).toHaveLength(
      1,
    );
    fireEvent.dragEnd(
      document.getElementById("tree-name-a")!.closest(".document-tree-row")!,
    );
    await waitFor(() =>
      expect(document.getElementById("tree-name-a")).toBeNull(),
    );
  });
  it("문서 트리는 Ctrl/Shift 선택을 열기와 분리하고 보이는 순서로 일괄 휴지통 이동한다", async () => {
    const f = await setup();
    const open = vi.fn();
    const exportPdf = vi.fn();
    const mutate = vi.spyOn(f.controller, "mutate").mockResolvedValue(true);
    render(
      <DocumentTree
        controller={f.controller}
        locked={false}
        structureLocked={false}
        createChild={() => {}}
        openDocument={open}
        exportPdf={exportPdf}
        viewportHeight={640}
        issues={
          new Map([
            [
              "a",
              [
                {
                  reason: "template_in_trash" as const,
                  message: "사용 중인 템플릿이 휴지통에 있습니다.",
                },
              ],
            ],
          ])
        }
      />,
    );
    const warning = screen.getByRole("img", {
      name: "사용 중인 템플릿이 휴지통에 있습니다.",
    });
    expect(warning).toHaveClass("document-tree-warning");
    const menu = warning
      .closest(".document-tree-row")!
      .querySelector(".tree-menu")!;
    expect(
      warning.compareDocumentPosition(menu) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0);
    fireEvent.click(document.getElementById("tree-name-a")!, {
      ctrlKey: true,
    });
    fireEvent.click(document.getElementById("tree-name-b")!, {
      shiftKey: true,
    });
    expect(open).not.toHaveBeenCalled();
    expect(
      screen.getByText(text("documents.selectedCount", { count: "2" })),
    ).toBeVisible();
    fireEvent.contextMenu(
      document.getElementById("tree-name-b")!.closest(".document-tree-row")!,
    );
    expect(
      screen.queryByRole("menuitem", { name: text("pdf.command") }),
    ).toBeNull();
    fireEvent.click(
      await screen.findByRole("menuitem", {
        name: text("documents.toTrash"),
      }),
    );
    await waitFor(() => expect(mutate).toHaveBeenCalledTimes(2));
    expect(mutate.mock.calls.map(([edit]) => edit)).toEqual([
      { kind: "trash", document: "a" },
      { kind: "trash", document: "b" },
    ]);
    expect(exportPdf).not.toHaveBeenCalled();
  });
  it("다른 문서 탭을 보고 있어도 트리 단일 메뉴는 연 문서의 PDF를 대상으로 한다", async () => {
    const f = await setup();
    const exportPdf = vi.fn();
    render(
      <DocumentTree
        controller={f.controller}
        locked={false}
        structureLocked={false}
        createChild={() => {}}
        openDocument={() => {}}
        exportPdf={exportPdf}
        viewportHeight={640}
      />,
    );
    fireEvent.contextMenu(
      document.getElementById("tree-name-b")!.closest(".document-tree-row")!,
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("pdf.command") }),
    );
    expect(exportPdf).toHaveBeenCalledExactlyOnceWith("b");
  });
  it("가상 트리 dragend가 누락돼도 Escape는 상태를 버리고 늦은 drop을 저장하지 않는다", async () => {
    const f = await setup();
    for (let index = 2; index < 1_000; index++) {
      const id = `cancel-${index.toString().padStart(4, "0")}`;
      f.list.documents.push({ id, template: "t", name: `취소 ${index}` });
      f.list.layout.rootOrder.push(id);
      f.list.layout.nodes[id] = {
        parentId: null,
        childOrder: [],
        state: "active",
        trash: null,
      };
    }
    await f.controller.load();
    const mutate = vi.spyOn(f.controller, "mutate");
    const view = render(
      <DocumentTree
        controller={f.controller}
        locked={false}
        structureLocked={false}
        createChild={() => {}}
        openDocument={() => {}}
        viewportHeight={640}
      />,
    );
    const source = document
      .getElementById("tree-name-a")!
      .closest(".document-tree-row")!;
    const target = document
      .getElementById("tree-name-b")!
      .closest(".document-tree-row")!;
    fireEvent.dragStart(source, {
      dataTransfer: { effectAllowed: "none", setData: vi.fn() },
    });
    expect(source.closest("li")).toHaveClass("drag-source-active");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(source.closest("li")).not.toHaveClass("drag-source-active");
    fireEvent.drop(target);
    expect(mutate).not.toHaveBeenCalled();
    expect(view.container.querySelector(".drop-before")).toBeNull();
    expect(view.container.querySelector(".drop-after")).toBeNull();
    expect(view.container.querySelector(".drop-inside")).toBeNull();
  });
  it("빈 값·보관·고아·경고와 rich marks를 실제 읽기 화면에 표시한다", async () => {
    const f = await setup();
    render(<DocumentWorkspace controller={f.controller} />);
    await waitFor(() => expect(f.controller.snapshot().busy).toBe(false));
    await act(() => f.controller.open("a"));
    expect(screen.getByText("빈 숫자")).toBeInTheDocument();
    expect(screen.getByText(text("field.unsetValue"))).toBeInTheDocument();
    expect(screen.getByText(text("field.archived"))).toBeInTheDocument();
    expect(screen.getByText(text("documents.orphan"))).toBeInTheDocument();
    expect(screen.getByText("보존 원문")).toBeInTheDocument();
    expect(screen.getByText("강조된 원문").closest("strong")).not.toBeNull();
    const alerts = screen.getAllByRole("alert");
    expect(
      alerts.some((alert) =>
        alert.textContent?.includes(text("documents.warning")),
      ),
    ).toBe(true);
    expect(
      screen.getByText(text("documents.orphan")).closest('[role="alert"]'),
    ).not.toBeNull();
    expect(
      alerts.every((alert) => !alert.textContent?.includes("UnknownBinding")),
    ).toBe(true);
    expect(
      f.transport.writes.filter((w) => w.input.kind === "save_document"),
    ).toHaveLength(0);
  });
  it("삭제된 템플릿과 누락 필드 값은 내부 분류 대신 보존 상태를 설명한다", async () => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    const f = await setup();
    f.template.lifecycle = "Deleted";
    const original = f.transport.workspaceResult!;
    f.transport.workspaceResult = (input) => {
      const result = original(input);
      if (result?.kind === "document_workspace" && result.value.kind === "read")
        return {
          ...result,
          value: {
            ...result.value,
            template: { ...result.value.template, lifecycle: "Deleted" },
            fields: [
              {
                id: "n",
                label: "캐릭터 목록",
                state: "Active",
                value: null,
                problem: "MissingKnownFieldValue",
              },
              {
                id: "orphan",
                label: "삭제된 필드",
                state: "Orphan",
                value: { kind: "single_line_text", value: "보존 원문" },
              },
            ],
            warnings: ["MissingKnownFieldValue"],
          },
        };
      return result;
    };
    render(<DocumentWorkspace controller={f.controller} />);
    await waitFor(() => expect(f.controller.snapshot().busy).toBe(false));
    await act(() => f.controller.open("a"));
    expect(
      screen
        .getAllByRole("alert")
        .some((alert) =>
          alert.textContent?.includes(text("documents.deletedTemplate")),
        ),
    ).toBe(true);
    expect(
      screen.getByText(text("documents.valueProblem.MissingKnownFieldValue")),
    ).toBeVisible();
    expect(
      screen.queryByText(text("documents.warning.MissingKnownFieldValue")),
    ).toBeNull();
    expect(screen.getByText("보존 원문")).toBeVisible();
    expect(screen.queryByText("MissingKnownFieldValue")).toBeNull();
  });
  it("조합·Enter는 생성하지 않고 raw 값과 오류 focus·닫기 취소 원문을 보존한다", async () => {
    const f = await setup();
    render(<DocumentWorkspace controller={f.controller} />);
    await waitFor(() => expect(f.controller.snapshot().busy).toBe(false));
    await act(() => f.controller.begin("t"));
    const name = screen.getByLabelText(text("documents.name") + " *");
    fireEvent.compositionStart(name);
    fireEvent.change(name, { target: { value: "한글" } });
    fireEvent.keyDown(name, { key: "Enter" });
    expect(f.saves).toBe(0);
    expect(
      screen.getByRole("button", { name: text("documents.create") }),
    ).toBeDisabled();
    fireEvent.compositionEnd(name);
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.create") }),
    );
    await waitFor(() => expect(f.saves).toBe(1));
    await waitFor(() =>
      expect(document.activeElement).toHaveAttribute("id", "creation-n"),
    );
    fireEvent.change(document.getElementById("creation-n")!, {
      target: { value: "-" },
    });
    expect(f.controller.snapshot().draft?.body.fields[0].value).toEqual({
      intent: "set",
      value: { kind: "number", value: "-" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.closeDraft") }),
    );
    await act(() => f.controller.cancelClose());
    expect(f.controller.snapshot().draft?.body.name).toBe("한글");
    expect(f.controller.snapshot().draft?.body.fields[0].value).toEqual({
      intent: "set",
      value: { kind: "number", value: "-" },
    });
  });
  it("용어 단일행 오류는 생성·편집의 정확한 입력을 표시하고 수정 원문을 보존한다", async () => {
    const creation = await setup();
    const creationView = render(
      <DocumentWorkspace controller={creation.controller} />,
    );
    await waitFor(() =>
      expect(creation.controller.snapshot().busy).toBe(false),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.new") }),
    );
    fireEvent.change(await screen.findByLabelText(text("documents.template")), {
      target: { value: "t" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.begin") }),
    );
    const creationName = await screen.findByLabelText(
      text("documents.name") + " *",
    );
    fireEvent.change(creationName, {
      target: { value: "용어 문서" },
    });
    const creationEnglish = screen.getByLabelText(text("glossary.englishName"));
    const creationSummary = screen.getByLabelText(text("glossary.summary"));
    fireEvent.change(creationEnglish, {
      target: { value: "line\u2028break" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.create") }),
    );
    await waitFor(() => expect(creation.saves).toBe(1));
    await waitFor(() => expect(document.activeElement).toBe(creationEnglish));
    expect(creationEnglish).toHaveAttribute("aria-invalid", "true");
    expect(creationEnglish).toHaveValue("line\u2028break");

    fireEvent.change(creationEnglish, { target: { value: "Allowed" } });
    fireEvent.change(creationSummary, {
      target: { value: "앞\u2029뒤" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.create") }),
    );
    await waitFor(() => expect(creation.saves).toBe(2));
    await waitFor(() => expect(document.activeElement).toBe(creationSummary));
    expect(creationSummary).toHaveAttribute("aria-invalid", "true");
    expect(creationSummary).toHaveValue("앞\u2029뒤");

    fireEvent.change(creationSummary, { target: { value: "정상 요약" } });
    fireEvent.click(
      screen.getByRole("button", { name: text("documents.create") }),
    );
    await waitFor(() => expect(creation.saves).toBe(3));
    await waitFor(() =>
      expect(document.activeElement).toHaveAttribute("id", "creation-n"),
    );
    expect(creationEnglish).toHaveAttribute("aria-invalid", "false");
    expect(creationSummary).toHaveAttribute("aria-invalid", "false");
    creationView.unmount();

    const editing = await setup();
    await editing.controller.open("a");
    await editing.controller.beginEdit("a");
    render(<DocumentWorkspace controller={editing.controller} />);
    const editEnglish = screen.getByLabelText(text("glossary.englishName"));
    const editSummary = screen.getByLabelText(text("glossary.summary"));
    fireEvent.change(editEnglish, { target: { value: "앞\u0085뒤" } });
    await act(() => editing.controller.edits.submit("a"));
    expect(editEnglish).toHaveAttribute("aria-invalid", "true");
    expect(editEnglish).toHaveValue("앞\u0085뒤");

    fireEvent.change(editEnglish, { target: { value: "Allowed" } });
    fireEvent.change(editSummary, { target: { value: "앞\v뒤" } });
    await act(() => editing.controller.edits.submit("a"));
    expect(editSummary).toHaveAttribute("aria-invalid", "true");
    expect(editSummary).toHaveValue("앞\v뒤");

    fireEvent.change(editSummary, { target: { value: "정상 요약" } });
    await act(() => editing.controller.edits.submit("a"));
    expect(editEnglish).toHaveAttribute("aria-invalid", "false");
    expect(editSummary).toHaveAttribute("aria-invalid", "false");
  });
  it("늦은 생성 응답은 새 입력을 덮지 않고 보관 후 닫는다", async () => {
    const f = await setup();
    await f.controller.begin("t");
    f.controller.edit((b) => ({ ...b, name: "제출" }));
    f.setOutcome({
      kind: "write",
      session: "s",
      artifact: "a",
      disk: "committed",
      recovery_required: false,
      cleanup_failed: false,
      error: null,
      diagnostic: {
        stage: "complete",
        category: null,
        sessionState: "ReadOnly",
        lockCategory: null,
        nextAction: "",
      },
      changed: true,
      warnings: [],
    });
    f.transport.hold = "document_workspace";
    const pending = f.controller.submit();
    await waitFor(() =>
      expect(
        [...f.transport.results.values()].some((r) => r.state === "pending"),
      ).toBe(true),
    );
    f.controller.edit((b) => ({ ...b, name: "제출 뒤 입력" }));
    f.transport.hold = null;
    f.transport.completeHeld();
    await pending;
    expect(f.controller.snapshot().draft?.body.name).toBe("제출 뒤 입력");
    expect(f.controller.snapshot().draft?.deposited).toBe(false);
    f.controller.requestClose();
    await f.controller.resolveClose("deposit");
    expect(f.controller.snapshot().draft).toBeNull();
    const deposits = f.transport.commands.filter(
      (w) =>
        w.action === "submit" &&
        w.input.kind === "document_workspace" &&
        w.input.request.action === "deposit",
    );
    expect(deposits).toHaveLength(1);
  });
});

it.each(["delete", "owner"])(
  "group import keeps its cell address and rejects late completion after %s",
  async (change) => {
    const { controller } = await setup();
    await controller.beginEdit("a");
    let resolve!: (r: Awaited<ReturnType<DocumentController["media"]>>) => void;
    const media = vi.spyOn(controller, "media").mockImplementationOnce(
      () =>
        new Promise((yes) => {
          resolve = yes;
        }),
    );
    const pending = controller.importAsset("group", false, "a", {
      instance: "card",
      child: "file",
    });
    const rejected = expect(pending).rejects.toThrow();
    await waitFor(() =>
      expect(media).toHaveBeenCalledWith({
        action: "asset_import",
        owner: "edit-a",
        generation: "1",
        field: "group",
        image: false,
        cell: { instance: "card", child: "file" },
      }),
    );
    if (change === "delete")
      controller.edits.field("a", "group", {
        intent: "set",
        value: { kind: "group", instances: [] },
      });
    else
      controller.edits.entries.a = {
        ...controller.edits.entries.a,
        status: { ...controller.edits.entries.a.status, owner: "new-owner" },
      };
    const current = structuredClone(controller.edits.entries.a.body);
    resolve({ kind: "asset_done" });
    await rejected;
    expect(controller.edits.entries.a.body).toEqual(current);
    // 후속 autosave를 시험 범위 밖으로 흘리지 않는다.
    controller.edits.entries.a.paused = true;
  },
);
