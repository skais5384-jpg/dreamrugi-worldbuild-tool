import type { DocumentController } from "./documentController";
import type { SpellFinding, SpellSurface } from "./spellcheck";
import { collectSurfaces } from "./spellcheck";
import { groupDraft } from "./groups";
import type { Value } from "../bridge/types";
import type { RichSpellRequest, RichSpellPatch } from "./rich/RichEditor";

interface AppliedChange {
  key: string;
  before: string;
  after: string;
  patches: { start: number; end: number; replacement: string }[];
}

export interface SpellApplyResult {
  applied: number;
  skipped: number;
  appliedIds: string[];
  skippedIds: string[];
  changes: AppliedChange[];
}

/** Keep untouched cards usable after earlier replacements shift their UTF-16 offsets. */
export function remainingSpellFindings(
  findings: readonly SpellFinding[],
  result: SpellApplyResult,
): SpellFinding[] {
  const applied = new Set(result.appliedIds);
  const skipped = new Set(result.skippedIds);
  const changes = new Map(result.changes.map((change) => [change.key, change]));
  return findings.flatMap((finding) => {
    if (applied.has(finding.id)) return [];
    const selected = skipped.has(finding.id) ? null : finding.selected;
    const change = changes.get(finding.surface.key);
    if (!change || finding.surface.text !== change.before)
      return [{ ...finding, selected }];
    let shift = 0;
    for (const patch of change.patches) {
      if (patch.end <= finding.start)
        shift += patch.replacement.length - (patch.end - patch.start);
      else if (patch.start < finding.end)
        return [{ ...finding, selected: null }];
    }
    const start = finding.start + shift;
    const end = finding.end + shift;
    if (change.after.slice(start, end) !== finding.original)
      return [{ ...finding, selected: null }];
    return [
      {
        ...finding,
        selected,
        surface: { ...finding.surface, text: change.after },
        start,
        end,
      },
    ];
  });
}

function replacement(surface: SpellSurface, findings: readonly SpellFinding[]) {
  let next = surface.text;
  let previousStart = Infinity;
  for (const finding of [...findings].sort((a, b) => b.start - a.start)) {
    if (
      !finding.selected ||
      finding.end > previousStart ||
      next.slice(finding.start, finding.end) !== finding.original
    )
      return null;
    next =
      next.slice(0, finding.start) + finding.selected + next.slice(finding.end);
    previousStart = finding.start;
  }
  return next;
}

function updatePlain(
  controller: DocumentController,
  document: string,
  surface: SpellSurface,
  next: string,
) {
  controller.edits.update(document, (body) => {
    if (surface.kind === "name")
      return { ...body, name: { intent: "set", value: next } };
    if (surface.kind === "summary")
      return { ...body, glossarySummary: { intent: "set", value: next } };
    const value: Value = { kind: "single_line_text", value: next };
    if (surface.kind === "field")
      return {
        ...body,
        fields: [
          ...body.fields.filter((field) => field.field !== surface.field),
          { field: surface.field!, value: { intent: "set", value } },
        ],
      };
    const entry = controller.edits.entries[document];
    const baseline = entry?.status.read.fields.find(
      (field) => field.id === surface.field,
    )?.value;
    const field = body.fields.find((item) => item.field === surface.field);
    const current =
      field?.value.intent === "set" && field.value.value.kind === "group"
        ? field.value.value
        : groupDraft(baseline?.kind === "group" ? baseline : undefined);
    return {
      ...body,
      fields: [
        ...body.fields.filter((item) => item.field !== surface.field),
        {
          field: surface.field!,
          value: {
            intent: "set" as const,
            value: {
              ...current,
              instances: current.instances.map((instance) =>
                instance.id !== surface.instance
                  ? instance
                  : {
                      ...instance,
                      fields: [
                        ...instance.fields.filter(
                          (item) => item.field !== surface.child,
                        ),
                        {
                          field: surface.child!,
                          value: { intent: "set" as const, value },
                        },
                      ],
                    },
              ),
            },
          },
        },
      ],
    };
  });
}

function replaceRich(
  elementId: string,
  patches: RichSpellPatch[],
  active: () => boolean,
) {
  return new Promise<boolean>((resolve) => {
    const element = window.document.getElementById(elementId);
    if (!element) return resolve(false);
    let completed = false;
    const timer = window.setTimeout(() => {
      if (!completed) {
        completed = true;
        resolve(false);
      }
    }, 2000);
    const request: RichSpellRequest = {
      patches,
      active: () => !completed && active(),
      resolve: (applied) => {
        if (completed) return;
        completed = true;
        window.clearTimeout(timer);
        resolve(applied);
      },
    };
    element.dispatchEvent(
      new CustomEvent("spell-replace", { detail: request }),
    );
  });
}

async function waitForIdle(
  controller: DocumentController,
  document: string,
  active: () => boolean,
) {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (!active()) return false;
    const entry = controller.edits.entries[document];
    if (!entry) return false;
    if (!entry.busy) return true;
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  return false;
}

export async function applySpellFindings(
  controller: DocumentController,
  document: string,
  owner: string,
  projectScope: string,
  findings: readonly SpellFinding[],
  active: () => boolean,
): Promise<SpellApplyResult> {
  const grouped = new Map<string, SpellFinding[]>();
  for (const finding of findings) {
    const list = grouped.get(finding.surface.key) ?? [];
    list.push(finding);
    grouped.set(finding.surface.key, list);
  }
  let applied = 0;
  let skipped = 0;
  const appliedIds: string[] = [];
  const skippedIds: string[] = [];
  const changes: AppliedChange[] = [];
  for (const group of grouped.values()) {
    if (!(await waitForIdle(controller, document, active))) {
      skipped += group.length;
      skippedIds.push(...group.map((finding) => finding.id));
      continue;
    }
    const entry = controller.edits.entries[document];
    const scope = `${controller.shell.projectGeneration()}:${controller.shell.snapshot().projectId ?? ""}`;
    if (
      !entry ||
      entry.status.owner !== owner ||
      entry.body.composing ||
      entry.paused ||
      entry.busy ||
      scope !== projectScope
    ) {
      skipped += group.length;
      skippedIds.push(...group.map((finding) => finding.id));
      continue;
    }
    const surface = collectSurfaces(document, entry).find(
      (candidate) => candidate.key === group[0].surface.key,
    );
    if (!surface || surface.text !== group[0].surface.text) {
      skipped += group.length;
      skippedIds.push(...group.map((finding) => finding.id));
      continue;
    }
    const next = replacement(surface, group);
    if (next === null) {
      skipped += group.length;
      skippedIds.push(...group.map((finding) => finding.id));
      continue;
    }
    if (surface.rich) {
      const patches = group.map((finding) => ({
        block: surface.block!,
        start: finding.start,
        end: finding.end,
        expected: finding.original,
        replacement: finding.selected!,
      }));
      if (
        !surface.elementId ||
        !(await replaceRich(surface.elementId, patches, active))
      ) {
        skipped += group.length;
        skippedIds.push(...group.map((finding) => finding.id));
        continue;
      }
    } else updatePlain(controller, document, surface, next);
    applied += group.length;
    appliedIds.push(...group.map((finding) => finding.id));
    changes.push({
      key: surface.key,
      before: surface.text,
      after: next,
      patches: group
        .map((finding) => ({
          start: finding.start,
          end: finding.end,
          replacement: finding.selected!,
        }))
        .sort((a, b) => a.start - b.start),
    });
  }
  return { applied, skipped, appliedIds, skippedIds, changes };
}
