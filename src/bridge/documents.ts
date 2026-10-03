import type { Id, Template, Value, ResultDto } from "./types";
import type { Intent, RecoveryKey } from "./workspace";
export interface LayoutNode {
  parentId: Id | null;
  childOrder: Id[];
  state: "active" | "trashed";
  trash: { parentId: Id | null; index: number; trashedAtUtc: string } | null;
}
export interface Layout {
  revision: number;
  rootOrder: Id[];
  nodes: Record<Id, LayoutNode>;
}
export interface CreationBody {
  name: string;
  englishName?: string;
  glossarySummary?: string;
  glossaryExcluded?: boolean;
  parent: Id | null;
  fields: { field: Id; value: Intent<Value> }[];
  composing: boolean;
}
export interface EditBody {
  name: Intent<string>;
  englishName?: Intent<string>;
  glossarySummary?: Intent<string>;
  glossaryExcluded?: Intent<boolean>;
  fields: { field: Id; value: Intent<Value> }[];
  composing: boolean;
}
export type LayoutEdit =
  | { kind: "adopt" }
  | { kind: "move"; document: Id; parent: Id | null; index: number }
  | { kind: "trash"; document: Id }
  | {
      kind: "restore";
      document: Id;
      destination: { parent: Id | null; index: number } | null;
    }
  | { kind: "purge"; document: Id };
export interface AssetMetadata {
  schemaVersion: number;
  id: string;
  name: string;
  size: number;
  sha256: string;
  image: boolean;
  width: number | null;
  height: number | null;
}
export interface DocumentSearchResult {
  id: Id;
  template: Id;
  templateName: string;
  name: string;
  path: string[];
  titleMatch: boolean;
  excerpt: { label: string; text: string } | null;
}
export interface ReplaceScopes {
  title: boolean;
  body: boolean;
  englishName: boolean;
  glossarySummary: boolean;
}
export interface DocumentReplaceChange {
  document: Id;
  documentName: string;
  templateName: string;
  path: string[];
  scope: "title" | "body" | "english_name" | "glossary_summary";
  label: string;
  before: string;
  after: string;
  beforePrefix: string;
  beforeMatch: string;
  beforeSuffix: string;
  afterPrefix: string;
  afterMatch: string;
  afterSuffix: string;
}
export interface DocumentReplaceBlocker {
  document: Id | null;
  documentName: string | null;
  label: string;
  reason: string;
}
export type DocumentRequest =
  | {
      action: "asset_import";
      cell?: import("./types").CellAddress;
      owner: Id;
      generation: string;
      field: Id;
      image: boolean;
    }
  | { action: "asset_read" | "asset_open"; asset: Id }
  | { action: "asset_chunk"; asset: Id; digest: string; offset: number }
  | { action: "url_open"; url: string }
  | { action: "format_inspect"; kind: "template" | "document"; artifact: Id }
  | {
      action: "format_change";
      kind: "template" | "document";
      artifact: Id;
      source: string;
      restore: string | null;
    }
  | { action: "edit_begin"; document: Id }
  | {
      action: "edit_draft";
      owner: Id;
      generation: string;
      body: EditBody;
      save: boolean;
    }
  | { action: "edit_deposit"; owner: Id; generation: string; body: EditBody }
  | { action: "edit_release"; owner: Id; generation: string }
  | { action: "edit_refresh"; owner: Id }
  | { action: "edit_retry"; owner: Id; generation: string; body: EditBody }
  | {
      action: "edit_restore";
      reapply?: import("./workspace").RecoveryApply;
      key: RecoveryKey;
      deposit_id: string;
      digest: string;
    }
  | {
      action: "search";
      query: string;
      template: Id | null;
      offset: number;
      limit: number;
      refresh: boolean;
    }
  | { action: "search_read"; document: Id }
  | {
      action: "replace_preview";
      find: string;
      replacement: string;
      template: Id | null;
      scopes: ReplaceScopes;
      caseSensitive: boolean;
      wholeWord: boolean;
      offset: number;
      limit: number;
    }
  | { action: "replace_page"; preview: Id; offset: number; limit: number }
  | { action: "replace_discard"; preview: Id | null }
  | { action: "replace_apply"; preview: Id }
  | { action: "references"; document: Id }
  | { action: "list"; refreshSearch?: boolean }
  | { action: "read"; document: Id }
  | { action: "pdf_inspect"; document: Id }
  | {
      action: "pdf_export";
      document: Id;
      source: string;
      destination: string;
      allow_missing_images: boolean;
    }
  | { action: "mutate"; snapshot: Id; edit: LayoutEdit }
  | { action: "begin"; template: Id; snapshot?: Id }
  | {
      action: "draft";
      owner: Id;
      generation: string;
      body: CreationBody;
      save: boolean;
    }
  | { action: "deposit"; owner: Id; generation: string; body: CreationBody }
  | { action: "release"; owner: Id; generation: string; discard: boolean }
  | {
      action: "restore";
      key: RecoveryKey;
      deposit_id: string;
      digest: string;
      reapply?: import("./workspace").RecoveryApply;
    };
export type DocumentResponse =
  | { kind: "asset"; metadata: AssetMetadata; contentType?: string | null }
  | { kind: "asset_chunk"; data: string; done: boolean }
  | { kind: "asset_done" }
  | {
      kind: "asset_error";
      error: { category: string; stage: string; nextAction: string };
      assetName: string | null;
      assetState: string | null;
    }
  | {
      kind: "format";
      schema: number;
      source: string;
      history: { digest: string; schema: number; content_updated_at: string }[];
    }
  | {
      kind: "editing";
      owner: Id;
      document: Id;
      generation: string;
      saved_generation: string | null;
      body: EditBody;
      read: DocumentRead;
      editable: Id[];
      source: string;
      deposited: boolean;
      outcome: ResultDto | null;
      problem: string | null;
      field: Id | null;
    }
  | {
      kind: "list";
      fingerprint: string;
      snapshot: Id;
      layout: Layout;
      initial: boolean;
      unplaced: Id[];
      documents: {
        id: Id;
        template: Id;
        name: string;
        englishName?: string;
        glossarySummary?: string;
        glossaryExcluded?: boolean;
      }[];
      issues?: { document: Id; warnings: string[]; reasons?: string[] }[];
      issueStatus?: "complete" | "partial" | "unavailable";
      unverifiedDocuments?: Id[];
      problem: string | null;
    }
  | {
      kind: "search";
      generation: Id;
      offset: number;
      total: number;
      hasMore: boolean;
      missingDocuments: number;
      templateNames: Record<string, string>;
      results: DocumentSearchResult[];
    }
  | { kind: "search_unavailable"; document: Id }
  | {
      kind: "replace_preview";
      preview: Id;
      offset: number;
      totalDocuments: number;
      totalChanges: number;
      hasMore: boolean;
      changes: DocumentReplaceChange[];
      blockers: DocumentReplaceBlocker[];
    }
  | {
      kind: "references";
      generation: Id;
      document: Id;
      incomplete: number;
      unavailable: {
        source: Id;
        sourceName: string;
        reason: "trashed" | "missing" | "corrupt" | "unreadable";
      }[];
      incoming: (
        | {
            kind: "relation";
            source: Id;
            sourceName: string;
            sourceTemplate: string;
            path: string[];
            target: Id;
            field: Id;
            fieldLabel: string;
            instance: Id | null;
            connection: Id;
            relationName: string;
            oneWay: boolean;
            missingReciprocal: boolean;
          }
        | {
            kind: "document_link";
            source: Id;
            sourceName: string;
            sourceTemplate: string;
            path: string[];
            target: Id;
            field: Id;
            fieldLabel: string;
            instance: Id | null;
          }
      )[];
    }
  | {
      kind: "read";
      schema?: number;
      id: Id;
      name: string;
      englishName?: string;
      glossarySummary?: string;
      glossaryExcluded?: boolean;
      template: Template;
      fields: {
        id: Id;
        label: string;
        state: string;
        provenance?: string | null;
        value: Value | null;
        problem?: string | null;
      }[];
      warnings: string[];
    }
  | {
      kind: "pdf_inspect";
      document: Id;
      name: string;
      source: string;
      missing_images: string[];
    }
  | {
      kind: "pdf_export";
      destination: string;
      size: number;
      sha256: string;
      cleanup_warning: boolean;
    }
  | {
      kind: "draft";
      owner: Id;
      generation: string;
      body: CreationBody;
      template: Template;
      deposited: boolean;
      outcome: ResultDto | null;
      problem: string | null;
      field: Id | null;
      commit?: {
        fingerprint: string;
        snapshot: Id;
        layoutRevision: number;
        parent: Id | null;
        changedDocuments: {
          id: Id;
          template: Id;
          name: string;
          englishName?: string;
          glossarySummary?: string;
          glossaryExcluded?: boolean;
        }[];
        removedDocuments: Id[];
        unplaced: Id[];
        documentCount: number;
        read: DocumentRead;
      } | null;
    }
  | { kind: "released" };
export type DocumentList = Extract<DocumentResponse, { kind: "list" }>;
export type DocumentSearch = Extract<DocumentResponse, { kind: "search" }>;
export type DocumentReplacePreview = Extract<
  DocumentResponse,
  { kind: "replace_preview" }
>;
export type DocumentReferences = Extract<
  DocumentResponse,
  { kind: "references" }
>;
export type DocumentValidationIssues = NonNullable<DocumentList["issues"]>;
export type DocumentRead = Extract<DocumentResponse, { kind: "read" }>;
export type Creation = Extract<DocumentResponse, { kind: "draft" }>;
export type DocumentEditing = Extract<DocumentResponse, { kind: "editing" }>;
