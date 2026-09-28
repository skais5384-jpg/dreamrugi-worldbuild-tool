import { fireEvent, screen, waitFor } from "@testing-library/react";
import { expect, vi } from "vitest";
import { render } from "../test/render";
import WorkspaceApp from "./WorkspaceApp";
import { WorkspaceController } from "./workspaceController";
import { TemplateController } from "./controller";
import { TestTransport } from "./testTransport";
import { GuardedClient } from "../bridge/client";
import type {
  DraftContent,
  DraftStatus,
  TemplateBody,
} from "../bridge/workspace";
import type { Template, Work } from "../bridge/types";
import { text } from "../strings";
export function transportFixture() {
  const transport = new TestTransport();
  const base: Template = {
    id: "template-one",
    revision: "1",
    name: "기준",
    lifecycle: "Active",
    presentation: null,
    fieldOrder: ["number-field"],
    fields: [
      {
        id: "number-field",
        label: "숫자 필드",
        kind: "Number",
        lifecycle: "Active",
        required: false,
        presentation: null,
        default: { kind: "unset" },
        initialDefault: { kind: "unset" },
        introducedRevision: "1",
        options: [],
        optionOrder: [],
      },
    ],
  };
  let body: TemplateBody = {
    name: base.name,
    presentation: { intent: "keep" },
    fields: [
      {
        id: "number-field",
        label: "숫자 필드",
        configuration: { kind: "number" },
        required: false,
        presentation: { intent: "keep" },
        default: { intent: "keep" },
        archived: false,
      },
    ],
    composing: false,
  };
  let serial = 0;
  let status: DraftStatus = {
    owner: "owner-one",
    draftId: "draft-one",
    projectFingerprint: "a".repeat(64),
    artifact: base.id,
    generation: "1",
    savedGeneration: "1",
    baseRevision: "1",
    sourceDigest: "b".repeat(64),
    snapshot: "snapshot-0",
    phase: "editing",
    receipt: null,
    error: null,
    problems: [],
    identities: {},
    outcome: null,
  };
  const snapshots = new Map<string, DraftContent>();
  const submitted: Extract<Work, { kind: "template_draft" }>[] = [];
  const changed = () => {
    status = { ...status, snapshot: `snapshot-${++serial}` };
    snapshots.set(status.snapshot, structuredClone({ base, body }));
    return { kind: "template_draft" as const, status: structuredClone(status) };
  };
  transport.workspaceResult = (input) => {
    switch (input.kind) {
      case "begin_template_draft":
        return changed();
      case "template_draft_content": {
        const snapshot = snapshots.get(input.snapshot);
        if (!snapshot) throw new Error("missing test snapshot");
        return {
          kind: "template_draft_content",
          content: {
            snapshot: input.snapshot,
            offset: "0",
            next: null,
            text: JSON.stringify(snapshot),
          },
        };
      }
      case "template_draft": {
        submitted.push(structuredClone(input));
        body = structuredClone(input.body);
        status = { ...status, generation: input.generation, error: null };
        if (input.action === "deposit")
          status.receipt = {
            key: {
              projectFingerprint: status.projectFingerprint,
              draftId: status.draftId,
              generation: input.generation,
            },
            depositId: "deposit-one",
            digest: "c".repeat(64),
          };
        else {
          const invalid = body.fields.some(
            (f) =>
              f.default.intent === "set" &&
              f.default.value.kind === "number" &&
              ["-", ".", ""].includes(f.default.value.value),
          );
          if (invalid)
            status = {
              ...status,
              error: { code: "save_rejected", nextAction: "" },
              problems: [
                {
                  category: "InvalidNumber",
                  property: "default",
                  field: "number-field",
                  option: null,
                },
              ],
            };
          else {
            status = {
              ...status,
              savedGeneration: input.generation,
              baseRevision: "2",
              phase: "saved",
            };
            base.name = body.name;
            base.revision = "2";
          }
        }
        return changed();
      }
      case "release_template_draft":
        return { kind: "control", error: null };
      case "document_workspace":
        return {
          kind: "document_workspace",
          value: {
            kind: "list",
            fingerprint: "fixture",
            snapshot: "documents",
            layout: { revision: 1, rootOrder: [], nodes: {} },
            initial: true,
            unplaced: [],
            documents: [],
            problem: null,
          },
        };
      case "recovery_page":
        return {
          kind: "recovery_page",
          page: { entries: [], next: null, bytesRead: 0, visitedNodes: 0 },
        };
      default:
        return undefined;
    }
  };
  return { transport, submitted, base };
}
export async function setup() {
  const fixture = transportFixture();
  const shell = new TemplateController(
    new GuardedClient(fixture.transport),
    vi.fn().mockResolvedValue("C:\\fixture"),
  );
  const controller = new WorkspaceController(shell);
  const view = render(<WorkspaceApp controller={controller} />);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("app.message07") }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: text("app.message07") }));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("documents.templates") }),
    ).toBeEnabled(),
  );
  fireEvent.click(
    screen.getByRole("button", { name: text("documents.templates") }),
  );
  // 다음 행동은 실제 DOM의 활성 상태에 동기화한다. controller publish와 React commit은 다른 시점이다.
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("app.message14") }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: text("app.message14") }));
  await waitFor(() =>
    expect(screen.getByLabelText(text("app.message23"))).toBeEnabled(),
  );
  return { ...fixture, controller, shell, view };
}
