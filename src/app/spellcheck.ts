import type { DocumentList } from "../bridge/documents";
import type { RichNode, TemplateSummary, Value } from "../bridge/types";
import { cellKey, cellValue, groupDraft } from "./groups";
import { fieldKind } from "./fieldEditing";
import type { EditEntry } from "./documentEdits";

export interface SpellSurface {
  key: string;
  label: string;
  text: string;
  kind: "name" | "summary" | "field" | "cell";
  field?: string;
  instance?: string;
  child?: string;
  block?: number;
  rich?: boolean;
  elementId?: string;
}
export interface SpellFinding {
  id: string;
  surface: SpellSurface;
  start: number;
  end: number;
  original: string;
  suggestions: string[];
  selected: string | null;
  ignored: boolean;
}

export function glossaryTerms(
  list: DocumentList | null,
  templates: TemplateSummary[],
): string[] | null {
  if (
    !list ||
    list.problem ||
    (list.issueStatus && list.issueStatus !== "complete") ||
    (list.unverifiedDocuments?.length ?? 0) > 0
  )
    return null;
  const active = new Set(
    templates
      .filter((row) => row.lifecycle === "Active" && !row.glossaryExcluded)
      .map((row) => row.id),
  );
  const names = new Set<string>();
  for (const row of list.documents) {
    if (
      list.layout.nodes[row.id]?.state !== "active" ||
      row.glossaryExcluded ||
      !active.has(row.template)
    )
      continue;
    if (row.name.trim()) names.add(row.name.trim());
    if (row.englishName?.trim()) names.add(row.englishName.trim());
  }
  return [...names].sort((a, b) => b.length - a.length);
}

const particles = new Set([
  "은",
  "는",
  "이",
  "가",
  "을",
  "를",
  "에",
  "에서",
  "에서부터",
  "에게",
  "에게서",
  "에게는",
  "에게도",
  "께",
  "께서",
  "와",
  "과",
  "와는",
  "과는",
  "이랑",
  "랑",
  "의",
  "도",
  "만",
  "까지",
  "부터",
  "로",
  "으로",
  "처럼",
  "보다",
  "마다",
  "이나",
  "나",
  "이라",
  "라고",
  "이라서",
  "이라고",
  "이라도",
  "라고는",
]);
function wordCharacter(character: string | undefined) {
  return !!character && /[\p{L}\p{N}]/u.test(character);
}
/** Return only the token ranges occupied by real glossary terms, including a known particle. */
function exceptionMatcher(terms: readonly string[]) {
  type Node = { next: Map<string, Node>; terminal: boolean };
  const root: Node = { next: new Map(), terminal: false };
  for (const raw of terms) {
    const term = raw.normalize("NFC");
    if (!term) continue;
    let node = root;
    for (let index = 0; index < term.length; index++) {
      const character = term[index];
      let next = node.next.get(character);
      if (!next) {
        next = { next: new Map(), terminal: false };
        node.next.set(character, next);
      }
      node = next;
    }
    node.terminal = true;
  }
  return (text: string): [number, number][] => {
    const ranges: [number, number][] = [];
    for (let from = 0; from < text.length; from++) {
      if (wordCharacter(text[from - 1])) continue;
      let node = root;
      for (let end = from; end < text.length; end++) {
        const next = node.next.get(text[end]);
        if (!next) break;
        node = next;
        if (node.terminal) {
          const after = end + 1;
          // Every accepted particle is short; a bounded lookahead avoids
          // copying a long document for each glossary match.
          const tail =
            text.slice(after, after + 8).match(/^[가-힣]+/u)?.[0] ?? "";
          if (!wordCharacter(text[after]) || particles.has(tail)) {
            ranges.push([
              from,
              after + (particles.has(tail) ? tail.length : 0),
            ]);
          }
        }
      }
    }
    ranges.sort((a, b) => a[0] - b[0] || b[1] - a[1]);
    const merged: [number, number][] = [];
    for (const range of ranges) {
      const last = merged[merged.length - 1];
      if (last && range[0] <= last[1]) last[1] = Math.max(last[1], range[1]);
      else merged.push(range);
    }
    return merged;
  };
}

export function exceptionRanges(
  text: string,
  terms: readonly string[],
): [number, number][] {
  return exceptionMatcher(terms)(text);
}

function normalizedOffsets(source: string) {
  const boundary = new Map<number, number>([[0, 0]]);
  let normalized = "";
  const segmenter = new Intl.Segmenter("ko", { granularity: "grapheme" });
  for (const { segment, index } of segmenter.segment(source)) {
    normalized += segment.normalize("NFC");
    boundary.set(normalized.length, index + segment.length);
  }
  return { normalized, boundary };
}

export function tokens(
  surfaces: readonly SpellSurface[],
  terms: readonly string[],
) {
  const occurrences: {
    surface: SpellSurface;
    start: number;
    end: number;
    word: string;
    original: string;
  }[] = [];
  const exceptions = exceptionMatcher(terms);
  for (const surface of surfaces) {
    const { normalized, boundary } = normalizedOffsets(surface.text);
    const protectedRanges = exceptions(normalized);
    let protectedIndex = 0;
    for (const match of normalized.matchAll(/[가-힣]+/gu)) {
      const start = match.index;
      const end = start + match[0].length;
      while (
        protectedIndex < protectedRanges.length &&
        protectedRanges[protectedIndex][1] <= start
      )
        protectedIndex++;
      const protectedRange = protectedRanges[protectedIndex];
      if (
        protectedRange &&
        start >= protectedRange[0] &&
        end <= protectedRange[1]
      )
        continue;
      const sourceStart = boundary.get(start);
      const sourceEnd = boundary.get(end);
      if (sourceStart === undefined || sourceEnd === undefined) continue;
      occurrences.push({
        surface,
        start: sourceStart,
        end: sourceEnd,
        word: match[0],
        original: surface.text.slice(sourceStart, sourceEnd),
      });
    }
  }
  return occurrences;
}

function richBlocks(value: RichNode): string[] {
  return richBlockNodes(value).map((node) =>
    "children" in node &&
    node.children.some((child) => child.kind === "hardBreak")
      ? ""
      : "children" in node
        ? node.children
            .map((child) => (child.kind === "text" ? child.text : ""))
            .join("")
        : "",
  );
}

function appendValue(
  result: SpellSurface[],
  report: { excluded: number } | undefined,
  key: string,
  label: string,
  kind: "field" | "cell",
  value: Value | null | undefined,
  elementId: string,
  field: string,
  instance?: string,
  child?: string,
) {
  if (value?.kind === "single_line_text") {
    result.push({
      key,
      label,
      kind,
      text: value.value,
      elementId,
      field,
      instance,
      child,
    });
  } else if (value?.kind === "rich_text") {
    const nodes = richBlockNodes(value.content);
    richBlocks(value.content).forEach((block, index) => {
      if (block)
        result.push({
          key: `${key}:block:${index}`,
          label: `${label} · 문단 ${index + 1}`,
          kind,
          text: block,
          rich: true,
          block: index,
          elementId,
          field,
          instance,
          child,
        });
      else if (report && value.content.kind === "root") {
        // Empty paragraphs have no words. A block with a hard break is excluded.
        const raw = nodes[index];
        if (
          raw &&
          "children" in raw &&
          raw.children.some((node) => node.kind === "hardBreak")
        )
          report.excluded++;
      }
    });
  }
}

function richBlockNodes(value: RichNode): RichNode[] {
  const blocks: RichNode[] = [];
  const visit = (node: RichNode) => {
    if (node.kind === "paragraph" || node.kind === "heading") {
      blocks.push(node);
      return;
    }
    if ("children" in node) node.children.forEach(visit);
  };
  visit(value);
  return blocks;
}

export function collectSurfaces(
  document: string,
  entry: EditEntry,
  report?: { excluded: number },
): SpellSurface[] {
  const result: SpellSurface[] = [];
  const read = entry.status.read;
  const name =
    entry.body.name.intent === "set" ? entry.body.name.value : read.name;
  result.push({
    key: "name",
    label: "문서명",
    text: name,
    kind: "name",
    elementId: `edit-name-${document}`,
  });
  const summary =
    entry.body.glossarySummary?.intent === "set"
      ? entry.body.glossarySummary.value
      : (read.glossarySummary ?? "");
  result.push({
    key: "summary",
    label: "한줄 설명",
    text: summary,
    kind: "summary",
    elementId: `edit-glossary-summary-${document}`,
  });
  for (const row of read.fields) {
    const definition = read.template.fields.find(
      (field) => field.id === row.id,
    );
    if (
      !definition ||
      definition.lifecycle !== "Active" ||
      !entry.status.editable.includes(row.id)
    )
      continue;
    const draft = entry.body.fields.find(
      (field) => field.field === row.id,
    )?.value;
    const value =
      draft?.intent === "set"
        ? draft.value
        : draft?.intent === "unset"
          ? null
          : row.value;
    if (definition.kind !== "Group") {
      if (["single_line_text", "rich_text"].includes(fieldKind(definition)))
        appendValue(
          result,
          report,
          `field:${row.id}`,
          definition.label,
          "field",
          value,
          `edit-${document}-${row.id}`,
          row.id,
        );
      continue;
    }
    const baseline = row.value?.kind === "group" ? row.value : undefined;
    const current = value?.kind === "group" ? value : groupDraft(baseline);
    current.instances.forEach((instance, index) => {
      const source = baseline?.instances.find(
        (candidate) => candidate.id === (instance.source ?? instance.id),
      );
      for (const child of definition.members ?? []) {
        if (
          child.lifecycle !== "Active" ||
          source?.protected?.includes(child.id) ||
          !["single_line_text", "rich_text"].includes(fieldKind(child))
        )
          continue;
        const cell = cellValue(instance, child.id, baseline);
        appendValue(
          result,
          report,
          `cell:${row.id}:${instance.id}:${child.id}`,
          `${definition.label} · 항목 ${index + 1} · ${child.label}`,
          "cell",
          cell.intent === "set" ? cell.value : null,
          `edit-${document}-${cellKey(row.id, instance.id, child.id)}`,
          row.id,
          instance.id,
          child.id,
        );
      }
    });
  }
  return result;
}
