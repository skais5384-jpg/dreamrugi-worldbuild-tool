import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";
import {
  archiveDefinition,
  restoreDefinition,
  changeGroupMembers,
} from "./archiveDefinition";
import { ArchivedDefinitionActions } from "./ArchivedDefinitionActions";
import { GroupDefinition } from "./GroupDefinition";
import {
  orderedTemplateItemIds,
  reorderTemplateItems,
  repairSectionAnchors,
} from "./SectionEditor";
import { useEffect, useRef, useState } from "react";
import { Badge } from "@fluentui/react-components";
import {
  CheckmarkCircle16Regular,
  Clock16Regular,
  ArrowSync20Regular,
  Archive20Regular,
  DoorArrowRight20Regular,
  Edit16Regular,
  ErrorCircle16Regular,
  ReOrderDotsVertical20Regular,
  Save20Regular,
} from "@fluentui/react-icons";
import { Button, Checkbox, Fieldset, Input, Select } from "../ui/Controls";
import type {
  DraftField,
  DraftProblem,
  Intent,
  TemplateBody,
} from "../bridge/workspace";
import { kinds } from "./fieldEditing";
import type { Field as FieldDefinition, Value } from "../bridge/types";
import { ValueInput, ValueRead } from "./FieldValue";
import { text } from "../strings";
import type { WholeDraft, WorkspaceController } from "./workspaceController";
import { problemHelp, problemTarget, revealProblem } from "./draftProblems";
import { useDraftReorder } from "./useDraftReorder";
import { PropertyRow } from "./PropertyRow";
import { HelpText } from "../ui/HelpText";
import { InlineNotice } from "../ui/InlineNotice";
import { boundsProblem } from "./numberBounds";
import { DraftConflictChoices } from "./DraftConflictChoices";
import { FormatControl } from "./FormatControl";
import { IconCommand } from "../ui/IconCommand";
import { MediaTargetContext } from "./MediaValue";
import { useSaveShortcut } from "./useSaveShortcut";

function token(intent: Intent<string>, original: string | null) {
  return intent.intent === "keep"
    ? (original ?? "")
    : intent.intent === "set"
      ? intent.value
      : "";
}
function move<T extends { id: string }>(items: T[], id: string, delta: number) {
  const i = items.findIndex((v) => v.id === id),
    j = i + delta;
  if (i < 0 || j < 0 || j >= items.length) return items;
  const next = [...items];
  [next[i], next[j]] = [next[j], next[i]];
  return next;
}

type DraftKind = DraftField["configuration"]["kind"];

function fieldKindBadgeColor(
  kind: DraftKind | "section",
): "brand" | "informative" | "success" | "warning" | "severe" | "subtle" {
  if (kind === "group") return "brand";
  if (kind === "section") return "subtle";
  if (kind === "image" || kind === "file") return "success";
  if (kind === "single_choice" || kind === "multi_choice") return "warning";
  if (kind === "relation" || kind === "document_link") return "severe";
  if (kind === "number") return "informative";
  return "brand";
}

function draftConfiguration(kind: DraftKind): DraftField["configuration"] {
  if (kind === "single_choice" || kind === "multi_choice")
    return { kind, options: [] };
  if (kind === "group") return { kind, members: [] };
  if (kind === "relation")
    return {
      kind,
      multiple: true,
      allowedTemplates: [],
      reciprocalNotice: true,
    };
  return { kind };
}

function newDraftField(
  id: string,
  label = "",
  kind: DraftKind = "single_line_text",
) {
  return {
    id,
    label,
    configuration: draftConfiguration(kind),
    required: false,
    presentation: { intent: "unset" as const },
    default: { intent: "unset" as const },
    archived: false,
  } satisfies DraftField;
}

export function WholeTemplate({
  controller,
  draft,
  locked,
}: {
  controller: WorkspaceController;
  draft: WholeDraft;
  locked: boolean;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState({
    message: "",
    generation: "0",
  });
  const notice =
    announcement.message === text("archive.restoredDraft") &&
    announcement.generation !== draft.generation
      ? ""
      : announcement.message;
  const setNotice = (message: string) =>
    setAnnouncement({
      message,
      generation: controller.snapshot().draft?.generation ?? "0",
    });
  const state = controller.snapshot();
  const templates = controller.shell
    .snapshot()
    .rows.filter((row) => row.lifecycle === "Active");
  const errors = useRef<HTMLElement>(null);
  const shortcutRoot = useRef<HTMLElement>(null);
  const archiveRoot = useRef<HTMLDetailsElement>(null);
  const revealed = useRef<string | null>(null);
  const problems =
    draft.status.generation === draft.generation ? draft.status.problems : [];
  const hasError = !!problems.length || !!state.error;
  useEffect(() => {
    if (
      state.busy ||
      locked ||
      !hasError ||
      draft.lastSave !== draft.generation
    )
      return;
    const attempt = draft.status.owner + ":" + draft.lastSave;
    if (revealed.current === attempt) return;
    revealed.current = attempt;
    const frame = requestAnimationFrame(() => revealProblem(errors.current));
    return () => cancelAnimationFrame(frame);
  }, [
    state.busy,
    locked,
    hasError,
    draft.lastSave,
    draft.generation,
    draft.status.owner,
  ]);
  const current = draft.body.fields.find((f) => f.id === selected);
  const currentSection = draft.body.sections?.find(
    (section) => section.id === selected,
  );
  const changed = (change: (body: TemplateBody) => TemplateBody) =>
    controller.edit((b) => repairSectionAnchors(b, change(b)));
  const editField = (id: string, edit: (f: DraftField) => DraftField) =>
    changed((b) => ({
      ...b,
      fields: b.fields.map((f) => (f.id === id ? edit(f) : f)),
    }));
  const canonical = (id: string) => draft.status.identities[id] ?? id;
  const original = draft.base.fields.find(
    (f) => f.id === canonical(selected ?? ""),
  );
  const newField = (id: string) =>
    !draft.base.fields.some((f) => f.id === canonical(id));
  const newSection = (id: string) =>
    !draft.base.sections?.some((section) => section.id === id);
  const newOption = (id: string) =>
    !original?.options.some((o) => o.id === canonical(id));
  const blocked = state.busy || locked || !draft.loaded;
  const actionBlocked =
    blocked ||
    ["saved_read_required", "uncertain"].includes(draft.status.phase);
  useSaveShortcut({
    root: shortcutRoot,
    enabled: true,
    blocked: actionBlocked,
    composing: draft.body.composing,
    save: () => controller.save(),
  });
  const active = draft.body.fields.filter((f) => !f.archived);
  const ordered = orderedTemplateItemIds(draft.body);
  const archived = draft.body.fields.filter((f) => f.archived);
  const saveState = state.pendingAction
    ? "busy"
    : hasError
      ? "error"
      : controller.dirty()
        ? "dirty"
        : "saved";
  const SaveStateIcon =
    saveState === "error"
      ? ErrorCircle16Regular
      : saveState === "busy"
        ? Clock16Regular
        : saveState === "dirty"
          ? Edit16Regular
          : CheckmarkCircle16Regular;
  const reorderItem = (id: string, delta: number) => {
    const index = ordered.indexOf(id);
    const target = index + delta;
    if (index < 0 || target < 0 || target >= ordered.length) return;
    const ids = [...ordered];
    [ids[index], ids[target]] = [ids[target], ids[index]];
    changed((body) => reorderTemplateItems(body, ids));
    setNotice(text("whole.reordered"));
  };
  const reorder = useDraftReorder(
    draft.status.owner,
    draft.generation,
    !blocked && !draft.body.composing,
    (group, order) => {
      if (group === "template-items")
        changed((body) => reorderTemplateItems(body, order));
      else
        editField(group, (f) =>
          "options" in f.configuration
            ? {
                ...f,
                configuration: {
                  ...f.configuration,
                  options: [
                    ...order.map((id) =>
                      ("options" in f.configuration
                        ? f.configuration.options
                        : []
                      ).find((o) => o.id === id)!,
                    ),
                    ...f.configuration.options.filter((o) => o.archived),
                  ],
                },
              }
            : f,
        );
    },
    setNotice,
  );
  const focusProblem = (problem: DraftProblem) => {
    const target = problemTarget(problem, draft);
    if (!target) return;
    setSelected(target.field);
    requestAnimationFrame(() =>
      revealProblem(document.getElementById(target.input)),
    );
  };
  if (draft.status.comparison)
    return (
      <section className="whole-template">
        <h1>{draft.base.name}</h1>
        {state.error && (
          <InlineNotice kind="warning">{state.error}</InlineNotice>
        )}
        <DraftConflictChoices
          key={JSON.stringify(draft.status.comparison)}
          changes={draft.status.comparison}
          template={draft.base}
          busy={locked}
          reference={{ list: null, templates, preview: true, open: () => {} }}
          apply={(selected) => controller.resume(selected)}
          cancel={() => controller.cancelComparison()}
        />
      </section>
    );
  return (
    <MediaTargetContext.Provider
      value={{ kind: "template", artifact: draft.base.id }}
    >
      <section
        ref={shortcutRoot}
        className="whole-template"
        {...reorder.surface}
        aria-label={text("whole.operation")}
        onCompositionStartCapture={() => controller.compose(true)}
        onCompositionEndCapture={() => controller.compose(false)}
      >
        <div className="whole-heading">
          <div>
            <h2>{draft.body.name || text("field.emptyLabel")}</h2>
          </div>
          <p
            className="document-save-state"
            data-state={saveState}
            role="status"
          >
            <SaveStateIcon aria-hidden />
            <span>
              {state.pendingAction
                ? text(
                    state.pendingAction === "save"
                      ? "whole.saving"
                      : "whole.depositing",
                  )
                : controller.dirty()
                  ? text("whole.dirty")
                  : text("whole.saved")}
            </span>
          </p>
          <div className="actions">
            <IconCommand
              label={text("whole.save")}
              icon={<Save20Regular />}
              disabled={actionBlocked || draft.body.composing}
              onClick={() => void controller.save()}
            />
            {draft.status.artifact && (
              <FormatControl
                shell={controller.shell}
                kind="template"
                artifact={draft.status.artifact}
                locked
                changed={() => controller.shell.refresh()}
              />
            )}
            {!draft.loaded && (
              <IconCommand
                label={text("whole.loadedRetry")}
                icon={<ArrowSync20Regular />}
                disabled={state.busy}
                onClick={() => void controller.reload()}
              />
            )}
            {draft.status.phase === "saved_read_required" && (
              <IconCommand
                label={text("whole.refreshSaved")}
                icon={<ArrowSync20Regular />}
                disabled={blocked}
                onClick={() => void controller.refreshSaved()}
              />
            )}
            <IconCommand
              label={text("whole.deposit")}
              icon={<Archive20Regular />}
              disabled={blocked}
              onClick={() => void controller.deposit()}
            />
            <IconCommand
              label={text("whole.endEditing")}
              icon={<DoorArrowRight20Regular />}
              disabled={blocked}
              onClick={() => void controller.navigate({ kind: "browse" })}
            />
          </div>
        </div>
        {!draft.loaded && !state.busy && (
          <FloatingNotice intent="warning">
            <FloatingNoticeContent>
              {text("whole.loadIncomplete")}
            </FloatingNoticeContent>
          </FloatingNotice>
        )}
        {draft.status.phase === "saved_read_required" && (
          <FloatingNotice intent="warning">
            <FloatingNoticeContent>
              {text("whole.readRequired")}
            </FloatingNoticeContent>
          </FloatingNotice>
        )}
        {draft.status.remainingInput && (
          <InlineNotice kind="warning">
            현재 구조에 적용할 수 없는 입력이 남아 있습니다. 입력은 이 템플릿에
            보존됩니다. 관련 정의를 복원한 뒤 편집을 다시 열어 확인해 주세요.
          </InlineNotice>
        )}
        {draft.status.savedGeneration === null &&
          draft.status.phase === "editing" && (
            <InlineNotice kind="info">{text("draft.resumed")}</InlineNotice>
          )}
        {draft.status.phase === "uncertain" && (
          <FloatingNotice intent="error">
            <FloatingNoticeContent>
              {text("whole.uncertain")}
            </FloatingNoticeContent>
          </FloatingNotice>
        )}
        {draft.status.phase === "conflict" && (
          <FloatingNotice intent="warning">
            <FloatingNoticeContent>
              {text("whole.conflict")}
            </FloatingNoticeContent>
          </FloatingNotice>
        )}
        {hasError && (
          <section
            ref={errors}
            tabIndex={-1}
            className="whole-errors"
            role="alert"
            aria-label={text("whole.errors")}
          >
            <h3>
              <ErrorCircle16Regular aria-hidden />
              {text("whole.errors")}
            </h3>
            <p>{text("whole.invalidHelp")}</p>
            {!problems.length && <p>{state.error}</p>}
            <ul>
              {problems.map((p, i) => (
                <li key={i}>
                  {problemTarget(p, draft) ? (
                    <Button type="button" onClick={() => focusProblem(p)}>
                      {problemHelp(p)}
                    </Button>
                  ) : (
                    <span>{problemHelp(p)}</span>
                  )}
                </li>
              ))}
            </ul>
          </section>
        )}
        <Fieldset disabled={locked || !draft.loaded} className="whole-inputs">
          <legend>{text("whole.basic")}</legend>
          <PropertyRow label={text("app.message23")} htmlFor="whole-name">
            <Input
              id="whole-name"
              value={draft.body.name}
              onChange={(e) => changed((b) => ({ ...b, name: e.target.value }))}
            />
          </PropertyRow>
          <PropertyRow
            label={text("glossary.exclude")}
            htmlFor="whole-glossary-excluded"
          >
            <Checkbox
              id="whole-glossary-excluded"
              aria-label={text("glossary.excludeTemplate")}
              checked={draft.body.glossaryExcluded ?? false}
              onChange={(_, data) =>
                changed((body) => ({
                  ...body,
                  glossaryExcluded: data.checked === true,
                }))
              }
            />
          </PropertyRow>
          <section aria-labelledby="whole-fields-title">
            <div className="panel-heading">
              <h3 id="whole-fields-title">{text("whole.fields")}</h3>
              {!!archived.length && (
                <Button
                  type="button"
                  onClick={() => {
                    const section = archiveRoot.current;
                    if (!section) return;
                    section.open = true;
                    section.scrollIntoView({ block: "nearest" });
                    section.querySelector("summary")?.focus();
                  }}
                >
                  {text("archive.fields")} ({archived.length})
                </Button>
              )}
            </div>
            <div className="whole-field-layout">
              <ul className="whole-field-list">
                {ordered.map((id, index) => {
                  const field = active.find((candidate) => candidate.id === id);
                  const section = draft.body.sections?.find(
                    (candidate) => candidate.id === id,
                  );
                  if (!field && !section) return null;
                  const card = reorder.card("template-items", id, ordered);
                  return (
                    <li
                      key={id}
                      {...card}
                      className={`${card.className} whole-field-card${selected === id ? " selected" : ""}`}
                    >
                      <Button
                        type="button"
                        appearance="subtle"
                        className="whole-field-name"
                        aria-label={
                          (field?.label || section?.title) ??
                          text("field.emptyLabel")
                        }
                        aria-pressed={selected === id}
                        aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                        aria-description={text("whole.fieldReorderHelp")}
                        title={text("whole.fieldReorderHelp")}
                        onClick={() => setSelected(id)}
                        onKeyDown={(event) => {
                          if (
                            blocked ||
                            draft.body.composing ||
                            !event.altKey ||
                            event.ctrlKey ||
                            event.metaKey ||
                            event.shiftKey ||
                            !["ArrowUp", "ArrowDown"].includes(event.key)
                          )
                            return;
                          event.preventDefault();
                          const delta = event.key === "ArrowUp" ? -1 : 1;
                          if (
                            index + delta >= 0 &&
                            index + delta < ordered.length
                          )
                            reorderItem(id, delta);
                        }}
                      >
                        {!blocked &&
                          !draft.body.composing &&
                          ordered.length > 1 && (
                            <span
                              className="whole-field-grip"
                              aria-hidden="true"
                            >
                              <ReOrderDotsVertical20Regular />
                            </span>
                          )}
                        <span className="whole-field-label">
                          {(field?.label || section?.title) ??
                            text("field.emptyLabel")}
                        </span>
                        <Badge
                          aria-hidden="true"
                          appearance="tint"
                          color={fieldKindBadgeColor(
                            section ? "section" : field!.configuration.kind,
                          )}
                          className="whole-field-kind-badge"
                        >
                          {text(
                            `whole.kind.${section ? "section" : field!.configuration.kind}`,
                          )}
                        </Badge>
                      </Button>
                      {field &&
                        problems.some(
                          (problem) => problem.field === canonical(field.id),
                        ) && <small>{text("whole.errors")}</small>}
                    </li>
                  );
                })}
                <li className="whole-field-add-card">
                  <Button
                    type="button"
                    appearance="subtle"
                    aria-label={text("whole.newField")}
                    title={text("whole.newField")}
                    onClick={() => {
                      const id = "new:" + crypto.randomUUID();
                      changed((body) => ({
                        ...body,
                        fields: [...body.fields, newDraftField(id)],
                      }));
                      setSelected(id);
                    }}
                  >
                    +
                  </Button>
                </li>
              </ul>
              <div className="whole-field-detail">
                {currentSection ? (
                  <>
                    <h4>{text("whole.kind.section")}</h4>
                    <PropertyRow
                      label={text("section.title")}
                      htmlFor={"whole-section-" + currentSection.id}
                    >
                      <Input
                        id={"whole-section-" + currentSection.id}
                        value={currentSection.title}
                        onChange={(event) =>
                          changed((body) => ({
                            ...body,
                            sections: body.sections?.map((section) =>
                              section.id === currentSection.id
                                ? { ...section, title: event.target.value }
                                : section,
                            ),
                          }))
                        }
                      />
                    </PropertyRow>
                    <PropertyRow
                      label={text("field.kind")}
                      htmlFor={"whole-section-kind-" + currentSection.id}
                    >
                      <Select
                        id={"whole-section-kind-" + currentSection.id}
                        value="section"
                        disabled={!newSection(currentSection.id)}
                        onChange={(event) => {
                          const kind = event.target.value as
                            DraftField["configuration"]["kind"] | "section";
                          if (kind === "section") return;
                          const id = "new:" + crypto.randomUUID();
                          const ids = ordered.map((item) =>
                            item === currentSection.id ? id : item,
                          );
                          changed((body) =>
                            reorderTemplateItems(
                              {
                                ...body,
                                sections: body.sections?.filter(
                                  (section) => section.id !== currentSection.id,
                                ),
                                fields: [
                                  ...body.fields,
                                  newDraftField(id, currentSection.title, kind),
                                ],
                              },
                              ids,
                            ),
                          );
                          setSelected(id);
                        }}
                      >
                        <option value="section">
                          {text("whole.kind.section")}
                        </option>
                        {([...Object.values(kinds), "group"] as const).map(
                          (kind) => (
                            <option key={kind} value={kind}>
                              {text(`whole.kind.${kind}`)}
                            </option>
                          ),
                        )}
                      </Select>
                      {!newSection(currentSection.id) && (
                        <HelpText>{text("field.kindLocked")}</HelpText>
                      )}
                    </PropertyRow>
                    <div className="whole-detail-actions">
                      <Button
                        type="button"
                        danger
                        onClick={() => {
                          changed((body) => ({
                            ...body,
                            sections: body.sections?.filter(
                              (section) => section.id !== currentSection.id,
                            ),
                          }));
                          setSelected(null);
                        }}
                      >
                        {text("section.remove")}
                      </Button>
                    </div>
                  </>
                ) : current && !current.archived ? (
                  <>
                    <h4>{text("whole.fieldBasic")}</h4>
                    <PropertyRow
                      label={text("field.label")}
                      htmlFor={"whole-field-" + current.id}
                      complex
                    >
                      <div className="field-name-controls">
                        <Input
                          id={"whole-field-" + current.id}
                          value={current.label}
                          onChange={(e) =>
                            editField(current.id, (f) => ({
                              ...f,
                              label: e.target.value,
                            }))
                          }
                        />
                        {current.configuration.kind !== "group" && (
                          <Checkbox
                            label={text("field.required")}
                            checked={current.required}
                            onChange={(_, data) =>
                              editField(current.id, (f) => ({
                                ...f,
                                required: data.checked === true,
                              }))
                            }
                          />
                        )}
                      </div>
                    </PropertyRow>
                    <div id={"whole-configuration-" + current.id} tabIndex={-1}>
                      <PropertyRow
                        label={text("field.kind")}
                        htmlFor={"whole-kind-" + current.id}
                      >
                        {problems
                          .filter(
                            (p) =>
                              p.field === canonical(current.id) &&
                              p.property === "configuration",
                          )
                          .map((p, i) => (
                            <InlineNotice key={i} kind="error">
                              {problemHelp(p)}
                            </InlineNotice>
                          ))}
                        <Select
                          id={"whole-kind-" + current.id}
                          value={current.configuration.kind}
                          disabled={!newField(current.id)}
                          onChange={(e) => {
                            const kind = e.target.value as
                              DraftKind | "section";
                            if (kind === "section") {
                              const id = crypto.randomUUID();
                              const ids = ordered.map((item) =>
                                item === current.id ? id : item,
                              );
                              changed((body) =>
                                reorderTemplateItems(
                                  {
                                    ...body,
                                    fields: body.fields.filter(
                                      (field) => field.id !== current.id,
                                    ),
                                    sections: [
                                      ...(body.sections ?? []),
                                      {
                                        id,
                                        title:
                                          current.label || text("section.new"),
                                        beforeField: null,
                                      },
                                    ],
                                  },
                                  ids,
                                ),
                              );
                              setSelected(id);
                              return;
                            }
                            editField(current.id, (field) => ({
                              ...field,
                              configuration: draftConfiguration(kind),
                              default: { intent: "unset" },
                            }));
                          }}
                        >
                          {(
                            [
                              ...Object.values(kinds),
                              "group",
                              "section",
                            ] as const
                          ).map((kind) => (
                            <option key={kind} value={kind}>
                              {text(`whole.kind.${kind}`)}
                            </option>
                          ))}
                        </Select>
                        {!newField(current.id) && (
                          <HelpText>{text("field.kindLocked")}</HelpText>
                        )}
                      </PropertyRow>
                    </div>
                    <PropertyRow
                      label={text(
                        current.configuration.kind === "group"
                          ? "group.writingGuide"
                          : "field.writingGuide",
                      )}
                      htmlFor={"whole-guide-" + current.id}
                      complex
                    >
                      <Input
                        id={"whole-guide-" + current.id}
                        value={token(
                          current.writingGuide ?? { intent: "keep" },
                          original?.writingGuide ?? null,
                        )}
                        placeholder={text("field.writingGuideHelp")}
                        aria-description={text("field.writingGuideHelp")}
                        onChange={(e) =>
                          editField(current.id, (f) => ({
                            ...f,
                            writingGuide:
                              e.target.value === ""
                                ? { intent: "unset" }
                                : { intent: "set", value: e.target.value },
                          }))
                        }
                      />
                    </PropertyRow>
                    {current.configuration.kind === "group" && (
                      <GroupDefinition
                        key={current.id}
                        members={current.configuration.members}
                        cardTitleField={
                          current.configuration.cardTitleField?.intent === "set"
                            ? current.configuration.cardTitleField.value
                            : current.configuration.cardTitleField?.intent ===
                                "unset"
                              ? null
                              : original?.cardTitleField
                        }
                        changeTitle={(cardTitleField) =>
                          editField(current.id, (f) => ({
                            ...f,
                            archiveTitle: undefined,
                            configuration:
                              f.configuration.kind === "group"
                                ? {
                                    ...f.configuration,
                                    cardTitleField: cardTitleField
                                      ? {
                                          intent: "set" as const,
                                          value: cardTitleField,
                                        }
                                      : { intent: "unset" as const },
                                  }
                                : f.configuration,
                          }))
                        }
                        original={original}
                        owner={draft.status.owner}
                        generation={draft.generation}
                        savedGeneration={draft.status.savedGeneration}
                        composing={draft.body.composing}
                        disabled={blocked}
                        canonical={canonical}
                        templates={templates}
                        change={(members) =>
                          editField(current.id, (f) => {
                            return changeGroupMembers(
                              f,
                              members,
                              original?.cardTitleField,
                              canonical,
                            );
                          })
                        }
                      />
                    )}
                    {current.configuration.kind === "relation" && (
                      <>
                        <PropertyRow label={text("relation.multiple")}>
                          <Checkbox
                            aria-label={text("relation.multiple")}
                            disabled={blocked}
                            checked={current.configuration.multiple}
                            onChange={(_, data) =>
                              editField(current.id, (field) => ({
                                ...field,
                                configuration:
                                  field.configuration.kind === "relation"
                                    ? {
                                        ...field.configuration,
                                        multiple: data.checked === true,
                                      }
                                    : field.configuration,
                              }))
                            }
                          />
                        </PropertyRow>
                        <PropertyRow
                          label={text("relation.allowedTemplates")}
                          htmlFor={"whole-relation-templates-" + current.id}
                        >
                          <Select
                            id={"whole-relation-templates-" + current.id}
                            multiple
                            disabled={blocked}
                            value={current.configuration.allowedTemplates}
                            onChange={(event) =>
                              editField(current.id, (field) => ({
                                ...field,
                                configuration:
                                  field.configuration.kind === "relation"
                                    ? {
                                        ...field.configuration,
                                        allowedTemplates: Array.from(
                                          event.target.selectedOptions,
                                          (option) => option.value,
                                        ),
                                      }
                                    : field.configuration,
                              }))
                            }
                          >
                            {templates.map((template) => (
                              <option key={template.id} value={template.id}>
                                {template.name}
                              </option>
                            ))}
                          </Select>
                          <HelpText>{text("relation.allowedHelp")}</HelpText>
                        </PropertyRow>
                        <PropertyRow label={text("relation.reciprocalNotice")}>
                          <Checkbox
                            aria-label={text("relation.reciprocalNotice")}
                            disabled={blocked}
                            checked={current.configuration.reciprocalNotice}
                            onChange={(_, data) =>
                              editField(current.id, (field) => ({
                                ...field,
                                configuration:
                                  field.configuration.kind === "relation"
                                    ? {
                                        ...field.configuration,
                                        reciprocalNotice: data.checked === true,
                                      }
                                    : field.configuration,
                              }))
                            }
                          />
                        </PropertyRow>
                      </>
                    )}
                    {current.configuration.kind === "number" &&
                      (["minimum", "maximum"] as const).map((bound) => (
                        <PropertyRow
                          key={bound}
                          label={text(`field.${bound}`)}
                          htmlFor={`whole-${bound}-${current.id}`}
                        >
                          <Input
                            id={`whole-${bound}-${current.id}`}
                            type="text"
                            value={
                              current.configuration.kind === "number"
                                ? (current.configuration[bound] ?? "")
                                : ""
                            }
                            aria-invalid={
                              current.configuration.kind === "number" &&
                              boundsProblem(
                                current.configuration.minimum,
                                current.configuration.maximum,
                              ) === bound
                            }
                            onChange={(event) =>
                              editField(current.id, (f) => ({
                                ...f,
                                configuration:
                                  f.configuration.kind === "number"
                                    ? {
                                        ...f.configuration,
                                        [bound]: event.target.value,
                                      }
                                    : f.configuration,
                              }))
                            }
                          />
                          {current.configuration.kind === "number" &&
                            boundsProblem(
                              current.configuration.minimum,
                              current.configuration.maximum,
                            ) === bound && (
                              <InlineNotice kind="error">
                                {text("field.invalidBounds")}
                              </InlineNotice>
                            )}
                        </PropertyRow>
                      ))}
                    {"options" in current.configuration && (
                      <>
                        <h4>{text("whole.value")}</h4>
                        <div className="whole-options">
                          {current.configuration.options.map((option) => (
                            <div
                              key={option.id}
                              {...reorder.card(
                                current.id,
                                option.id,
                                ("options" in current.configuration
                                  ? current.configuration.options
                                  : []
                                )
                                  .filter((o) => !o.archived)
                                  .map((o) => o.id),
                                option.archived,
                              )}
                              className={
                                "whole-option " +
                                reorder.card(current.id, option.id, [])
                                  .className
                              }
                            >
                              <PropertyRow
                                label={
                                  <span className="whole-option-label">
                                    {!blocked &&
                                      !draft.body.composing &&
                                      !option.archived &&
                                      ("options" in current.configuration
                                        ? current.configuration.options.filter(
                                            (candidate) => !candidate.archived,
                                          ).length
                                        : 0) > 1 && (
                                        <span
                                          className="whole-field-grip"
                                          aria-hidden="true"
                                        >
                                          <ReOrderDotsVertical20Regular />
                                        </span>
                                      )}
                                    {text("option.label")}
                                  </span>
                                }
                                htmlFor={"whole-option-" + option.id}
                                complex
                              >
                                <Input
                                  id={"whole-option-" + option.id}
                                  aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                                  aria-description={text(
                                    "whole.fieldReorderHelp",
                                  )}
                                  title={text("whole.fieldReorderHelp")}
                                  onKeyDown={(event) => {
                                    if (
                                      blocked ||
                                      draft.body.composing ||
                                      option.archived ||
                                      !event.altKey ||
                                      event.ctrlKey ||
                                      event.metaKey ||
                                      event.shiftKey ||
                                      !["ArrowUp", "ArrowDown"].includes(
                                        event.key,
                                      )
                                    )
                                      return;
                                    event.preventDefault();
                                    const delta =
                                      event.key === "ArrowUp" ? -1 : 1;
                                    editField(current.id, (f) =>
                                      "options" in f.configuration
                                        ? {
                                            ...f,
                                            configuration: {
                                              ...f.configuration,
                                              options: move(
                                                f.configuration.options,
                                                option.id,
                                                delta,
                                              ),
                                            },
                                          }
                                        : f,
                                    );
                                  }}
                                  value={option.label}
                                  disabled={option.archived}
                                  onChange={(e) =>
                                    editField(current.id, (f) =>
                                      "options" in f.configuration
                                        ? {
                                            ...f,
                                            configuration: {
                                              ...f.configuration,
                                              options:
                                                f.configuration.options.map(
                                                  (o) =>
                                                    o.id === option.id
                                                      ? {
                                                          ...o,
                                                          label: e.target.value,
                                                        }
                                                      : o,
                                                ),
                                            },
                                          }
                                        : f,
                                    )
                                  }
                                />
                                {problems
                                  .filter(
                                    (p) =>
                                      p.field === canonical(current.id) &&
                                      p.option === canonical(option.id) &&
                                      p.property === "option",
                                  )
                                  .map((p, i) => (
                                    <InlineNotice key={i} kind="error">
                                      {problemHelp(p)}
                                    </InlineNotice>
                                  ))}
                              </PropertyRow>
                              {option.archived ? (
                                <Button
                                  type="button"
                                  disabled={
                                    actionBlocked || draft.body.composing
                                  }
                                  onClick={() => {
                                    editField(current.id, (f) =>
                                      "options" in f.configuration
                                        ? {
                                            ...f,
                                            configuration: {
                                              ...f.configuration,
                                              options: restoreDefinition(
                                                f.configuration.options,
                                                option.id,
                                                original?.options.find(
                                                  (o) =>
                                                    o.id ===
                                                    canonical(option.id),
                                                )?.lifecycle === "Archived",
                                              ),
                                            },
                                          }
                                        : f,
                                    );
                                    setNotice(text("archive.restoredDraft"));
                                  }}
                                >
                                  {text(
                                    original?.options.find(
                                      (o) => o.id === canonical(option.id),
                                    )?.lifecycle === "Archived"
                                      ? "archive.restore"
                                      : "archive.undo",
                                  )}
                                </Button>
                              ) : (
                                <div className="actions">
                                  <Button
                                    type="button"
                                    danger
                                    onClick={() => {
                                      setNotice("");
                                      editField(current.id, (f) =>
                                        "options" in f.configuration
                                          ? {
                                              ...f,
                                              configuration: {
                                                ...f.configuration,
                                                options: newOption(option.id)
                                                  ? f.configuration.options.filter(
                                                      (o) => o.id !== option.id,
                                                    )
                                                  : archiveDefinition(
                                                      f.configuration.options,
                                                      option.id,
                                                    ),
                                              },
                                            }
                                          : f,
                                      );
                                    }}
                                  >
                                    {text(
                                      newOption(option.id)
                                        ? "whole.removeNew"
                                        : "whole.archive",
                                    )}
                                  </Button>
                                </div>
                              )}
                            </div>
                          ))}
                          <Button
                            type="button"
                            onClick={() =>
                              editField(current.id, (f) =>
                                "options" in f.configuration
                                  ? {
                                      ...f,
                                      configuration: {
                                        ...f.configuration,
                                        options: [
                                          ...f.configuration.options,
                                          {
                                            id: "new:" + crypto.randomUUID(),
                                            label: "",
                                            archived: false,
                                          },
                                        ],
                                      },
                                    }
                                  : f,
                              )
                            }
                          >
                            {text("whole.newOption")}
                          </Button>
                        </div>
                      </>
                    )}
                    {original && current.configuration.kind !== "group" && (
                      <>
                        <HelpText>{text("field.legacyDefaultHelp")}</HelpText>
                        <PropertyRow
                          label={text("field.default")}
                          htmlFor={
                            "whole-default-" +
                            current.id +
                            (current.configuration.kind === "rich_text"
                              ? ""
                              : "-mode")
                          }
                          complex
                        >
                          <WholeDefault
                            field={current}
                            original={original}
                            onChange={(value) =>
                              editField(current.id, (f) => ({
                                ...f,
                                default: value,
                              }))
                            }
                          />
                          {problems
                            .filter(
                              (p) =>
                                p.field === canonical(current.id) &&
                                p.property === "default",
                            )
                            .map((p, i) => (
                              <InlineNotice key={i} kind="error">
                                {problemHelp(p)}
                              </InlineNotice>
                            ))}
                        </PropertyRow>
                      </>
                    )}
                    <div className="whole-detail-actions">
                      <Button
                        type="button"
                        danger
                        onClick={() => {
                          if (newField(current.id))
                            changed((b) => ({
                              ...b,
                              fields: b.fields.filter(
                                (f) => f.id !== current.id,
                              ),
                            }));
                          else
                            changed((b) => ({
                              ...b,
                              fields: archiveDefinition(b.fields, current.id),
                            }));
                          setSelected(null);
                          setNotice("");
                        }}
                      >
                        {text(
                          newField(current.id)
                            ? "whole.removeNew"
                            : "archive.field",
                        )}
                      </Button>
                    </div>
                  </>
                ) : (
                  <p>{text("whole.chooseField")}</p>
                )}
              </div>
            </div>
          </section>
          {!!archived.length && (
            <details open ref={archiveRoot} className="archive-definitions">
              <summary>
                {text("whole.archived")} ({archived.length})
              </summary>
              <HelpText>{text("archive.restoreHelp")}</HelpText>
              {archived.map((f) => (
                <section key={f.id} className="archive-definition-row">
                  <div className="archive-definition-heading">
                    <h4>{f.label || text("field.emptyLabel")}</h4>
                    <p>{text(`whole.kind.${f.configuration.kind}`)}</p>
                  </div>
                  <div className="actions">
                    <ArchivedDefinitionActions
                      canonical={canonical}
                      field={f}
                      original={draft.base.fields.find(
                        (s) => s.id === canonical(f.id),
                      )}
                      disabled={actionBlocked || draft.body.composing}
                      change={(value) => {
                        changed((b) => ({
                          ...b,
                          fields: restoreDefinition(
                            b.fields,
                            f.id,
                            draft.base.fields.find(
                              (s) => s.id === canonical(f.id),
                            )?.lifecycle === "Archived",
                          ).map((item) =>
                            item.id === f.id
                              ? {
                                  ...value,
                                  archiveIndex: item.archiveIndex,
                                  archiveOrder: item.archiveOrder,
                                }
                              : item,
                          ),
                        }));
                        setSelected(f.id);
                        setNotice(text("archive.restoredDraft"));
                      }}
                    />
                    <Button
                      type="button"
                      disabled={actionBlocked || draft.body.composing}
                      onClick={() => {
                        changed((b) => ({
                          ...b,
                          fields: restoreDefinition(
                            b.fields,
                            f.id,
                            draft.base.fields.find(
                              (s) => s.id === canonical(f.id),
                            )?.lifecycle === "Archived",
                          ),
                        }));
                        setSelected(f.id);
                        setNotice(text("archive.restoredDraft"));
                      }}
                    >
                      {text(
                        draft.base.fields.find((s) => s.id === canonical(f.id))
                          ?.lifecycle === "Archived"
                          ? "archive.restore"
                          : "archive.undo",
                      )}
                    </Button>
                  </div>
                </section>
              ))}
            </details>
          )}
        </Fieldset>
        <p role="status" aria-live="polite">
          {notice === text("archive.restoredDraft") &&
          draft.status.savedGeneration !== null &&
          BigInt(draft.status.savedGeneration) >=
            BigInt(announcement.generation)
            ? text("archive.restoredSaved")
            : notice}
        </p>
      </section>
    </MediaTargetContext.Provider>
  );
}

function WholeDefault({
  field,
  original,
  onChange,
}: {
  field: DraftField;
  original: FieldDefinition | undefined;
  onChange: (value: Intent<Value>) => void;
}) {
  if (field.configuration.kind === "group") return null;
  const value =
    field.default.intent === "keep"
      ? null
      : field.default.intent === "unset"
        ? { kind: "unset" as const }
        : field.default.value;
  const options =
    "options" in field.configuration
      ? field.configuration.options.map((o) => ({
          ...o,
          lifecycle: o.archived ? "Archived" : "Active",
        }))
      : [];
  const change = (value: Value | null) =>
    onChange(
      value === null
        ? { intent: "keep" }
        : value.kind === "unset"
          ? { intent: "unset" }
          : { intent: "set", value },
    );
  if (field.configuration.kind === "rich_text")
    return (
      <div className="whole-default">
        <Select
          id={"whole-default-" + field.id}
          value={field.default.intent}
          onChange={(e) =>
            onChange(
              e.target.value === "keep"
                ? { intent: "keep" }
                : e.target.value === "unset"
                  ? { intent: "unset" }
                  : field.default,
            )
          }
        >
          {original && <option value="keep">{text("field.keep")}</option>}
          <option value="unset">{text("field.unset")}</option>
          {field.default.intent === "set" && (
            <option value="set">{text("field.set")}</option>
          )}
        </Select>
        <p>{text("whole.richDeferred")}</p>
        {value?.kind === "rich_text" && (
          <ValueRead value={value} options={options} />
        )}
        {original && field.default.intent === "keep" && (
          <ValueRead value={original.default} options={original.options} />
        )}
      </div>
    );
  return (
    <div className="whole-default">
      <ValueInput
        id={"whole-default-" + field.id}
        kind={field.configuration.kind}
        value={value}
        keep={!!original}
        options={options}
        onChange={change}
        showModeLabel={false}
      />
      {original && field.default.intent === "keep" && (
        <ValueRead value={original.default} options={original.options} />
      )}
    </div>
  );
}
