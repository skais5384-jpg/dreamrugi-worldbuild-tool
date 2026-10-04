import ko from "./ko.json";

export type StringKey = keyof typeof ko;
type NamedArguments = {
  "svn.commitRequired": { paths: string };
  "svn.registerDefaultMessage": { name: string };
  "svn.registerSummary": {
    files: string;
    directories: string;
    excluded: string;
  };
  "svn.registerRevisions": { setup: string; content: string };
  "svn.lockOwner": { owner: string };
  "spell.scanningCount": { surfaces: string; words: string };
  "spell.excluded": { count: string };
  "spell.found": { count: string };
  "spell.applyPartial": { applied: string; skipped: string };
  "spell.applyDone": { applied: string };
  "spell.suggestionFor": { location: string; word: string };
  "spell.more": { count: string };
  "documents.searchMissing": { count: string };
  "documents.selectedCount": { count: string };
  "documents.replaceScopeSelected": { selected: string; total: string };
  "format.version": { version: string };
  "template.target": { name: string };
  "template.resultId": { id: string };
  "template.version": { revision: string; lifecycle: string };
  "template.references": { count: string };
  "template.savedWarnings": { count: string };
  "project.defaultPath": { path: string };
  "backup.copyDefaultName": { name: string };
  "backup.unnamed": { number: string };
  "backup.deletedUndo": { name: string };
  "followup.sessionProblem": { problem: string };
  "followup.shutdownReport": { count: string; failures: string };
  "followup.blockers": { count: string };
  "supportDiagnostics.youtube": {
    stage: string;
    category: string;
    cause: string;
  };
  "order.position": { name: string; position: string; count: string };
  "order.dragStarted": { name: string };
  "order.ordinal": { position: string; count: string };
  "order.upName": { name: string };
  "order.downName": { name: string };
  "error.boundary": { code: string };
  "error.transport": { category: string };
  "media.failedNamed": { name: string };
  "field.basis": { revision: string };
  "field.introduced": { revision: string };
  "field.apply": { property: string };
  "reference.count": { count: string };
  "glossary.count": { count: string };
  "glossary.collapseCategory": { name: string };
  "glossary.expandCategory": { name: string };
  "relation.incomplete": { count: string };
  "relation.sourceRole": { name: string };
  "documents.issue.resource_in_trash_named": { names: string };
  "health.resourceProblems": { count: string };
  "health.resourcesInTrash": { count: string };
  "health.documentsWithoutTemplate": { count: string };
  "health.unregisteredDocuments": { count: string };
  "health.badge.problems": { count: string };
  "health.deletedTemplateNotice": { count: string };
  "resources.selected": { count: string };
  "trash.batchComplete": { count: string };
  "trash.batchPartial": {
    completed: string;
    protected: string;
    failed: string;
    cleanup: string;
  };
  "trash.restoreResult": { completed: string; failed: string; cleanup: string };
  "trash.restoreBlockedByEditors": { names: string };
  "templates.batchTrashWarning": { count: string };
  "trash.purgeSummary": { count: string; protected: string };
};
// 문구 편집 때 인자 이름도 검사한다. 타입과 실행 검사에서 같은 닫힌 계약을 사용한다.
export const argumentNames: {
  [K in keyof NamedArguments]: readonly (keyof NamedArguments[K])[];
} = {
  "svn.commitRequired": ["paths"],
  "svn.registerDefaultMessage": ["name"],
  "svn.registerSummary": ["files", "directories", "excluded"],
  "svn.registerRevisions": ["setup", "content"],
  "svn.lockOwner": ["owner"],
  "spell.scanningCount": ["surfaces", "words"],
  "spell.excluded": ["count"],
  "spell.found": ["count"],
  "spell.applyPartial": ["applied", "skipped"],
  "spell.applyDone": ["applied"],
  "spell.suggestionFor": ["location", "word"],
  "spell.more": ["count"],
  "documents.searchMissing": ["count"],
  "documents.selectedCount": ["count"],
  "documents.replaceScopeSelected": ["selected", "total"],
  "format.version": ["version"],
  "template.target": ["name"],
  "template.resultId": ["id"],
  "template.version": ["revision", "lifecycle"],
  "template.references": ["count"],
  "template.savedWarnings": ["count"],
  "project.defaultPath": ["path"],
  "backup.copyDefaultName": ["name"],
  "backup.unnamed": ["number"],
  "backup.deletedUndo": ["name"],
  "followup.sessionProblem": ["problem"],
  "followup.shutdownReport": ["count", "failures"],
  "followup.blockers": ["count"],
  "supportDiagnostics.youtube": ["stage", "category", "cause"],
  "order.position": ["name", "position", "count"],
  "order.dragStarted": ["name"],
  "order.ordinal": ["position", "count"],
  "order.upName": ["name"],
  "order.downName": ["name"],
  "error.boundary": ["code"],
  "error.transport": ["category"],
  "media.failedNamed": ["name"],
  "field.basis": ["revision"],
  "field.introduced": ["revision"],
  "field.apply": ["property"],
  "reference.count": ["count"],
  "glossary.count": ["count"],
  "glossary.collapseCategory": ["name"],
  "glossary.expandCategory": ["name"],
  "relation.incomplete": ["count"],
  "relation.sourceRole": ["name"],
  "documents.issue.resource_in_trash_named": ["names"],
  "health.resourceProblems": ["count"],
  "health.resourcesInTrash": ["count"],
  "health.documentsWithoutTemplate": ["count"],
  "health.unregisteredDocuments": ["count"],
  "health.badge.problems": ["count"],
  "health.deletedTemplateNotice": ["count"],
  "resources.selected": ["count"],
  "trash.batchComplete": ["count"],
  "trash.batchPartial": ["completed", "protected", "failed", "cleanup"],
  "trash.restoreResult": ["completed", "failed", "cleanup"],
  "trash.restoreBlockedByEditors": ["names"],
  "templates.batchTrashWarning": ["count"],
  "trash.purgeSummary": ["count", "protected"],
};
export function validateResources(resource: Record<string, unknown>): void {
  const expected: Record<string, readonly string[]> = argumentNames;
  for (const key of new Set([
    ...Object.keys(ko),
    ...Object.keys(expected),
    ...Object.keys(resource),
  ])) {
    const value = resource[key];
    if (
      typeof value !== "string" ||
      !value.length ||
      /[{}]/.test(value.replace(/\{[a-zA-Z]+\}/g, ""))
    )
      throw new Error("Invalid string resource: " + key);
    const actual = [
      ...new Set([...value.matchAll(/\{([a-zA-Z]+)\}/g)].map((m) => m[1])),
    ].sort();
    if (
      JSON.stringify(actual) !==
      JSON.stringify([...(expected[key] ?? [])].sort())
    )
      throw new Error("Invalid string arguments: " + key);
  }
}
type Arguments<K extends StringKey> = K extends keyof NamedArguments
  ? [NamedArguments[K]]
  : [];
/** 리소스는 텍스트로만 치환한다. 누락된 키/인자는 원 오류를 노출하지 않는다. */
export function text<K extends StringKey>(
  key: K,
  ...args: Arguments<K>
): string {
  return formatResource(ko, key, args[0] ?? {});
}
export function formatResource(
  resource: Record<string, string>,
  key: string,
  args: Record<string, string>,
): string {
  const value = resource[key];
  if (
    typeof value !== "string" ||
    /[{}]/.test(value.replace(/\{[a-zA-Z]+\}/g, ""))
  )
    return ko["resource.fallback"];
  const names = [...value.matchAll(/\{([a-zA-Z]+)\}/g)].map((m) => m[1]);
  if (
    names.some((n) => !Object.prototype.hasOwnProperty.call(args, n)) ||
    Object.keys(args).some(
      (n) => !names.includes(n) || typeof args[n] !== "string",
    )
  )
    return ko["resource.fallback"];
  return value.replace(/\{([a-zA-Z]+)\}/g, (_, name: string) => args[name]);
}
