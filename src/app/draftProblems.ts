import type { DraftProblem } from "../bridge/workspace";
import type { WholeDraft } from "./workspaceController";
import { text } from "../strings";

export const problemHelp = (problem: DraftProblem) =>
  text(
    problem.category === "GuideMetadataConflict"
      ? "whole.problem.guideConflict"
      : `whole.problem.${problem.property}`,
  );

export function problemTarget(problem: DraftProblem, draft: WholeDraft) {
  const raw = (id: string) =>
    Object.entries(draft.status.identities).find(
      ([, value]) => value === id,
    )?.[0] ?? id;
  const field = problem.field ? raw(problem.field) : null;
  const definition = draft.body.fields.find(
    (f) => f.id === field && !f.archived,
  );
  if (!definition || problem.property === "global") return null;
  if (problem.property === "default")
    return {
      field: definition.id,
      input: `whole-default-${definition.id}${definition.configuration.kind !== "rich_text" && definition.default.intent !== "set" ? "-mode" : ""}`,
    };
  if (problem.property === "option" && problem.option) {
    const option = raw(problem.option);
    if (
      "options" in definition.configuration &&
      definition.configuration.options.some(
        (o) => o.id === option && !o.archived,
      )
    )
      return { field: definition.id, input: "whole-option-" + option };
  }
  return {
    field: definition.id,
    input: "whole-configuration-" + definition.id,
  };
}

export function revealProblem(element: HTMLElement | null) {
  if (!element) return;
  for (
    let parent = element.parentElement;
    parent;
    parent = parent.parentElement
  )
    if (parent instanceof HTMLDetailsElement) parent.open = true;
  element.scrollIntoView({ block: "center" });
  element.focus({ preventScroll: true });
  const heading = element
    .closest(".whole-template")
    ?.querySelector(".whole-heading");
  if (heading) {
    const overlap =
      heading.getBoundingClientRect().bottom +
      12 -
      element.getBoundingClientRect().top;
    if (overlap > 0) window.scrollBy(0, -overlap);
  }
}
