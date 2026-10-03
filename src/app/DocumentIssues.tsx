import type { AssetInspection, TemplateSummary } from "../bridge/types";
import type {
  DocumentList,
  DocumentRead,
  DocumentValidationIssues,
} from "../bridge/documents";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";

export type DocumentIssueReason =
  | "resource_in_trash"
  | "resource_missing"
  | "resource_corrupt"
  | "resource_uncertain"
  | "template_in_trash"
  | "template_missing"
  | "validation_unknown"
  | "inspection_waiting"
  | "inspection_pending"
  | "unregistered"
  | "document_validation";

export interface DocumentIssue {
  reason: DocumentIssueReason;
  code?: string;
  message: string;
  /** 필드 내부에 이미 같은 안내가 있는 경우 트리에서만 표시한다. */
  showInBody?: boolean;
}

export function documentValidationMessage(reason: string): string {
  switch (reason) {
    case "RequiredValueUnset":
      return text("documents.warning.RequiredValueUnset");
    case "MissingKnownFieldValue":
      return text("documents.warning.MissingKnownFieldValue");
    case "InvalidKnownFieldValue":
      return text("documents.warning.InvalidKnownFieldValue");
    case "UnknownSelectedOption":
      return text("documents.warning.UnknownSelectedOption");
    case "OrphanSnapshotMissing":
      return text("documents.warning.OrphanSnapshotMissing");
    case "ReattachmentSnapshotConflict":
      return text("documents.warning.ReattachmentSnapshotConflict");
    case "LossyOrphanReattachment":
      return text("documents.warning.LossyOrphanReattachment");
    case "OptionSnapshotUnavailable":
      return text("documents.warning.OptionSnapshotUnavailable");
    case "LossySnapshotMembership":
      return text("documents.warning.LossySnapshotMembership");
    default:
      return text("documents.warning");
  }
}

export function documentIssueMessage(reason: string): string {
  switch (reason) {
    case "resource_in_trash":
      return text("documents.issue.resource_in_trash");
    case "resource_missing":
      return text("documents.issue.resource_missing");
    case "resource_corrupt":
      return text("documents.issue.resource_corrupt");
    case "resource_uncertain":
      return text("documents.issue.resource_uncertain");
    case "deleted_template":
    case "template_in_trash":
      return text("documents.issue.template_in_trash");
    case "missing_template":
    case "template_missing":
      return text("documents.issue.template_missing");
    case "unregistered":
      return text("documents.issue.unregistered");
    case "validation_unknown":
      return text("documents.issue.validation_unknown");
    case "inspection_waiting":
      return text("documents.issue.inspection_waiting");
    case "inspection_pending":
      return text("documents.issue.inspection_pending");
    default:
      return text("documents.warning");
  }
}

function detailedIssueMessage(
  reason: DocumentIssueReason,
  resourceNames: readonly string[],
) {
  if (reason === "resource_in_trash" && resourceNames.length)
    return text("documents.issue.resource_in_trash_named", {
      names: resourceNames.join(", "),
    });
  return documentIssueMessage(reason);
}

function normalizeReason(reason: string): DocumentIssueReason | null {
  switch (reason) {
    case "resource_in_trash":
    case "resource_missing":
    case "resource_corrupt":
    case "resource_uncertain":
    case "unregistered":
    case "validation_unknown":
    case "inspection_waiting":
    case "inspection_pending":
      return reason;
    case "deleted_template":
    case "template_in_trash":
      return "template_in_trash";
    case "missing_template":
    case "template_missing":
      return "template_missing";
    default:
      return null;
  }
}

export function collectDocumentIssues(
  list: DocumentList | null,
  templates: readonly TemplateSummary[],
  inspection: AssetInspection | null,
  validationIssues: DocumentValidationIssues = [],
  currentRead: DocumentRead | null = null,
  inspectionPendingDocuments: readonly string[] = [],
  inspectionWaiting = false,
): ReadonlyMap<string, DocumentIssue[]> {
  const result = new Map<string, DocumentIssue[]>();
  const add = (
    document: string,
    rawReason: string,
    resourceNames: readonly string[] = [],
    code?: string,
  ) => {
    const reason = normalizeReason(rawReason);
    if (!reason) return;
    const current = result.get(document) ?? [];
    if (
      !current.some((issue) => issue.reason === reason && issue.code === code)
    )
      current.push({
        reason,
        code,
        message: detailedIssueMessage(reason, resourceNames),
      });
    result.set(document, current);
  };
  const addValidation = (document: string, code: string) => {
    const current = result.get(document) ?? [];
    if (
      !current.some(
        (issue) =>
          issue.reason === "document_validation" && issue.code === code,
      )
    )
      current.push({
        reason: "document_validation",
        code,
        message: documentValidationMessage(code),
        // 저장값 미형성은 해당 필드 안에서 복구 안내를 이미 제공한다.
        showInBody: code !== "MissingKnownFieldValue",
      });
    result.set(document, current);
  };

  const resourceNames = new Map(
    (inspection?.rows ?? [])
      .filter((row) => !!row.name)
      .map((row) => [row.id, row.name!] as const),
  );
  for (const issue of inspection?.documentIssues ?? []) {
    for (const reason of issue.reasons) {
      const target = issue.targets?.find((item) => item.reason === reason);
      const ids = target
        ? target.resourceIds
        : issue.reasons.length === 1
          ? (issue.relatedResourceIds ?? [])
          : [];
      const names = ids
        .map((id) => resourceNames.get(id))
        .filter((name): name is string => !!name);
      add(issue.documentId, reason, names);
    }
  }
  for (const document of inspectionPendingDocuments)
    add(
      document,
      inspectionWaiting ? "inspection_waiting" : "inspection_pending",
    );

  const templateLifecycle = new Map(
    templates.map((template) => [template.id, template.lifecycle] as const),
  );
  if (list) {
    for (const document of list.documents) {
      if (!list.issueStatus) {
        const lifecycle = templateLifecycle.get(document.template);
        if (lifecycle === "Deleted") add(document.id, "template_in_trash");
        else if (templates.length && lifecycle === undefined)
          add(document.id, "template_missing");
      }
    }
    for (const document of list.unplaced) add(document, "unregistered");
    for (const document of list.unverifiedDocuments ?? [])
      add(document, "validation_unknown");
    if (list.issueStatus === "unavailable" && !list.unverifiedDocuments?.length)
      for (const document of list.documents)
        if (list.layout.nodes[document.id]?.state !== "trashed")
          add(document.id, "validation_unknown");
  }
  for (const issue of validationIssues) {
    for (const reason of issue.reasons ?? []) add(issue.document, reason);
    for (const code of issue.warnings) addValidation(issue.document, code);
  }
  // A read result is authoritative for the document currently on screen. Keeping
  // this rule in the shared collector prevents the tree and both read/edit bodies
  // from drifting when a template lifecycle changes between list refreshes.
  if (currentRead?.template.lifecycle === "Deleted")
    add(currentRead.id, "template_in_trash");
  if (currentRead) {
    const validationCodes = new Set([
      ...currentRead.warnings,
      ...currentRead.fields
        .map((field) => field.problem)
        .filter((problem): problem is string => !!problem),
    ]);
    for (const code of validationCodes) addValidation(currentRead.id, code);
  }
  return result;
}

export function DocumentIssueNotices({
  issues,
}: {
  issues: readonly DocumentIssue[];
}) {
  return issues
    .filter((issue) => issue.showInBody !== false)
    .map((issue) => (
      <InlineNotice
        key={`${issue.reason}:${issue.code ?? ""}`}
        kind={issue.reason === "inspection_waiting" ? "info" : "warning"}
      >
        {issue.message}
      </InlineNotice>
    ));
}
