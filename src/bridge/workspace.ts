import type {
  BoundaryError,
  DocumentEdit,
  Id,
  ResultDto,
  Template,
  TemplateEdit,
  Value,
} from "./types";

export type Intent<T> =
  { intent: "keep" } | { intent: "unset" } | { intent: "set"; value: T };
export interface DraftOption {
  id: string;
  label: string;
  archived: boolean;
}
export type DraftConfiguration =
  | { kind: "group"; members: DraftField[]; cardTitleField?: Intent<string> }
  | { kind: "number"; minimum?: string; maximum?: string }
  | {
      kind:
        | "single_line_text"
        | "rich_text"
        | "date"
        | "time"
        | "duration"
        | "image"
        | "file"
        | "url";
    }
  | { kind: "single_choice" | "multi_choice"; options: DraftOption[] }
  | {
      kind: "relation";
      multiple: boolean;
      allowedTemplates: string[];
      reciprocalNotice: boolean;
    }
  | { kind: "document_link" };
export interface DraftField {
  id: string;
  label: string;
  configuration: DraftConfiguration;
  required: boolean;
  presentation: Intent<string>;
  writingGuide?: Intent<string>;
  default: Intent<Value>;
  archived: boolean;
}
export interface TemplateBody {
  sections?: import("./types").Section[];
  name: string;
  glossaryExcluded?: boolean;
  presentation: Intent<string>;
  fields: DraftField[];
  composing: boolean;
}
export interface RecoveryKey {
  projectFingerprint: string;
  draftId: string;
  generation: string;
}
export interface Receipt {
  key: RecoveryKey;
  depositId: string;
  digest: string;
}
export interface DraftProblem {
  category: string;
  field: string | null;
  option: string | null;
  property: "global" | "default" | "configuration" | "option";
}
export interface DraftStatus {
  owner: Id;
  draftId: string;
  projectFingerprint: string;
  artifact: string;
  generation: string;
  savedGeneration: string | null;
  baseRevision: string;
  sourceDigest: string | null;
  snapshot: Id;
  phase: string;
  receipt: Receipt | null;
  error: BoundaryError | null;
  problems: DraftProblem[];
  identities: Record<string, string>;
  outcome: ResultDto | null;
}
export interface DraftContent {
  base: Template;
  body: TemplateBody;
}
export interface ContentChunk {
  snapshot: Id;
  offset: string;
  next: string | null;
  text: string;
}
export interface RecoveryError {
  category: string;
  stage: string;
  ioKind?: string | null;
  osCode?: number | null;
  boundaryReason?: string | null;
  published: boolean;
  nextAction: string;
  cleanupFailed: boolean;
}
export interface RecoveryRow {
  row: {
    locatorFingerprint: string;
    key: RecoveryKey | null;
    depositId: string | null;
    payloadKind: string | null;
    payloadDigest: string | null;
    error: RecoveryError | null;
  };
  version: string | null;
  createdAtUtc: string | null;
  name?: string | null;
  artifact: string | null;
}
export interface RecoveryPage {
  entries: RecoveryRow[];
  next: string | null;
  visitedNodes: number;
  bytesRead: number;
  legacy?: {
    found: number;
    imported: number;
    alreadyPresent: number;
    needsAttention: number;
    held: string[];
  };
}
export type ReapplyIntent =
  | { kind: "name" | "presentation" }
  | {
      kind:
        | "field_label"
        | "field_required"
        | "field_writing_guide"
        | "field_presentation"
        | "field_card_title"
        | "field_default";
      field: string;
    };
export interface RecoverySelection {
  snapshot: Id;
  key: RecoveryKey;
  phase: string;
  canRestore: boolean;
  intents: ReapplyIntent[];
}
export type RecoveryDraft =
  | (TemplateBody & { kind: "template"; template: string | null })
  | {
      kind: "admitted_document";
      document: string;
      template: string;
      revision: string;
      edits: DocumentEdit[];
    }
  | {
      kind: "admitted_composite";
      document: string;
      template: string;
      revision: string;
      edit: TemplateEdit;
      edits: DocumentEdit[];
    }
  | {
      kind: "document";
      document: string | null;
      template: string;
      name: Intent<string>;
      fields: { field: string; value: Intent<Value> }[];
      composing: boolean;
    };
export interface RecoveryContent {
  draft: RecoveryDraft;
  original: Template | null;
  current: Template | null;
  attempt: {
    submittedGeneration: string;
    operationId: string;
    result: string;
    candidateDigest: string | null;
    transactionId: string | null;
  } | null;
}
export type WorkspaceWork =
  | { kind: "begin_template_draft"; project: Id; view: Id | null }
  | {
      kind: "template_draft";
      project: Id;
      session: Id;
      generation: string;
      body: TemplateBody;
      action: "save" | "deposit";
    }
  | {
      kind: "template_draft_content";
      project: Id;
      session: Id;
      snapshot: Id;
      offset: string;
    }
  | { kind: "refresh_template_draft"; project: Id; session: Id }
  | {
      kind: "release_template_draft";
      project: Id;
      session: Id;
      generation: string;
      body: TemplateBody | null;
      discard: boolean;
    }
  | { kind: "recovery_page"; cursor: string | null }
  | { kind: "recovery_close_cursor"; cursor: string }
  | {
      kind: "recovery_read" | "recovery_revalidate";
      key: RecoveryKey;
      deposit_id: string;
      digest: string;
    }
  | { kind: "recovery_content"; snapshot: Id; offset: string }
  | { kind: "recovery_release_selection"; snapshot: Id }
  | { kind: "recovery_discard"; key: RecoveryKey; version: string }
  | {
      kind: "recovery_restore";
      project: Id;
      snapshot: Id;
      reapply: ReapplyIntent[] | null;
    };
export type WorkspaceResult =
  | { kind: "template_draft"; status: DraftStatus }
  | { kind: "template_draft_content"; content: ContentChunk }
  | { kind: "recovery_page"; page: RecoveryPage }
  | { kind: "recovery_selection"; selection: RecoverySelection }
  | { kind: "recovery_receipt"; receipt: Receipt }
  | { kind: "recovery_failure"; failure: RecoveryError };
