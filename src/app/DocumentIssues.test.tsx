import { describe, expect, it } from "vitest";
import type { AssetInspection } from "../bridge/types";
import type { DocumentList } from "../bridge/documents";
import { text } from "../strings";
import { collectDocumentIssues } from "./DocumentIssues";

const document = "11111111-1111-4111-8111-111111111111";
const template = "22222222-2222-4222-8222-222222222222";

function list(
  issueStatus: DocumentList["issueStatus"],
  reasons: string[] = [],
  unverifiedDocuments: string[] = [],
): DocumentList {
  return {
    kind: "list",
    fingerprint: "project",
    snapshot: "snapshot",
    initial: false,
    layout: {
      revision: 1,
      rootOrder: [document],
      nodes: {
        [document]: {
          parentId: null,
          childOrder: [],
          state: "active",
          trash: null,
        },
      },
    },
    documents: [{ id: document, template, name: "문서" }],
    unplaced: [],
    issues: reasons.length ? [{ document, warnings: [], reasons }] : [],
    issueStatus,
    unverifiedDocuments,
    problem: null,
  };
}

describe("shared document issues", () => {
  it("shows confirmed missing Template even when the Template list is empty", () => {
    const missing = list("complete", ["template_missing"]);
    const issues = collectDocumentIssues(missing, [], null, missing.issues);
    expect(issues.get(document)?.map((issue) => issue.reason)).toEqual([
      "template_missing",
    ]);
    expect(collectDocumentIssues(list("complete"), [], null).size).toBe(0);
  });

  it("marks an unverified document without guessing that its Template is missing", () => {
    const issues = collectDocumentIssues(
      list("partial", [], [document]),
      [],
      null,
    );
    expect(issues.get(document)?.map((issue) => issue.reason)).toEqual([
      "validation_unknown",
    ]);
  });

  it("keeps pending inspection alongside existing warnings until a verified retry clears it", () => {
    const partial = list("partial", ["resource_missing"]);
    const pending = collectDocumentIssues(
      partial,
      [],
      null,
      partial.issues,
      null,
      [document],
    );
    expect(pending.get(document)?.map((issue) => issue.reason)).toEqual([
      "inspection_pending",
      "resource_missing",
    ]);
    const complete = collectDocumentIssues(list("complete"), [], null);
    expect(complete.get(document)).toBeUndefined();
    const unknown = collectDocumentIssues(
      list("complete", ["future_unknown_reason"]),
      [],
      null,
      [{ document, warnings: [], reasons: ["future_unknown_reason"] }],
    );
    expect(unknown.size).toBe(0);
  });

  it("uses each resource reason's IDs for its own warning text", () => {
    const inspection = {
      rows: [
        { id: "trash-id", name: "휴지통.txt" },
        { id: "broken-id", name: "손상.txt" },
      ],
      documentIssues: [
        {
          documentId: document,
          reasons: ["resource_in_trash", "resource_corrupt"],
          relatedResourceIds: ["trash-id", "broken-id"],
          targets: [
            {
              reason: "resource_in_trash",
              resourceIds: ["trash-id"],
              templateIds: [],
            },
            {
              reason: "resource_corrupt",
              resourceIds: ["broken-id"],
              templateIds: [],
            },
          ],
        },
      ],
    } as unknown as AssetInspection;
    const issues = collectDocumentIssues(list("complete"), [], inspection);
    expect(
      issues
        .get(document)
        ?.find((issue) => issue.reason === "resource_in_trash")?.message,
    ).toBe(
      text("documents.issue.resource_in_trash_named", {
        names: "휴지통.txt",
      }),
    );
    expect(
      issues.get(document)?.find((issue) => issue.reason === "resource_corrupt")
        ?.message,
    ).toBe(text("documents.issue.resource_corrupt"));
  });
});
