import { archiveDefinition, restoreDefinition } from "./archiveDefinition";
import { useState } from "react";
import { Button, Checkbox, Input, Select, Textarea } from "../ui/Controls";
import { text } from "../strings";
import type { DraftField } from "../bridge/workspace";
import type { Field, TemplateSummary } from "../bridge/types";
import { PropertyRow } from "./PropertyRow";
import { useDraftReorder } from "./useDraftReorder";
import { HelpText } from "../ui/HelpText";
import "./Groups.css";

export function GroupDefinition({
  members,
  cardTitleField,
  changeTitle,
  original,
  owner,
  generation,
  composing,
  disabled,
  change,
  canonical,
  templates = [],
}: {
  members: DraftField[];
  cardTitleField?: string | null;
  changeTitle?: (id: string | null) => void;
  original?: Field;
  owner: string;
  generation: string;
  composing: boolean;
  disabled: boolean;
  change: (fields: DraftField[]) => void;
  canonical: (id: string) => string;
  templates?: TemplateSummary[];
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const active = members.filter((f) => !f.archived);
  const current = members.find((f) => f.id === selected);
  const old = original?.members?.find(
    (f) => f.id === canonical(selected ?? ""),
  );
  const update = (edit: (f: DraftField) => DraftField) =>
    change(members.map((f) => (f.id === selected ? edit(f) : f)));
  const order = active.map((f) => f.id);
  const reorder = useDraftReorder(
    owner,
    generation,
    !disabled && !composing,
    (_, ids) =>
      change([
        ...ids.map((id) => members.find((f) => f.id === id)!),
        ...members.filter((f) => f.archived),
      ]),
    setNotice,
  );
  const addMember = () => {
    const id = "new:" + crypto.randomUUID();
    change([
      ...members,
      {
        id,
        label: "",
        configuration: { kind: "rich_text" },
        required: false,
        archived: false,
        default: { intent: "unset" },
        presentation: { intent: "unset" },
      },
    ]);
    setSelected(id);
  };
  return (
    <section
      className="group-definition"
      aria-label={text("group.members")}
      {...reorder.surface}
    >
      <h4>{text("group.members")}</h4>
      {changeTitle && (
        <PropertyRow
          label={text("group.cardTitleField")}
          htmlFor={"group-title-" + (original?.id ?? owner)}
        >
          <Select
            id={"group-title-" + (original?.id ?? owner)}
            disabled={disabled || composing}
            value={cardTitleField ?? ""}
            onChange={(e) => changeTitle(e.target.value || null)}
          >
            <option value="">{text("group.noCardTitle")}</option>
            {members
              .filter(
                (f) => !f.archived && f.configuration.kind === "rich_text",
              )
              .map((f) => (
                <option key={f.id} value={f.id}>
                  {f.label || text("field.emptyLabel")}
                </option>
              ))}
            {cardTitleField &&
              !active.some(
                (f) =>
                  f.id === cardTitleField &&
                  f.configuration.kind === "rich_text",
              ) && (
                <option value={cardTitleField} disabled>
                  {text("group.archivedTitle")}
                </option>
              )}
          </Select>
        </PropertyRow>
      )}
      <div className="repeat-member-editor">
        <div
          className="repeat-members"
          role="tablist"
          aria-label={text("group.members")}
        >
          {active.map((f, index) => (
            <div
              key={f.id}
              className="repeat-member-tab"
              {...reorder.card(original?.id ?? "members", f.id, order)}
            >
              <Button
                type="button"
                appearance="subtle"
                id={`member-tab-${f.id}`}
                role="tab"
                aria-selected={selected === f.id}
                aria-controls={`member-panel-${f.id}`}
                aria-pressed={selected === f.id}
                disabled={disabled}
                title={text("whole.fieldReorderHelp")}
                aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                onClick={() => setSelected(f.id)}
                onKeyDown={(e) => {
                  if (
                    composing ||
                    !e.altKey ||
                    e.ctrlKey ||
                    e.metaKey ||
                    e.shiftKey ||
                    !["ArrowUp", "ArrowDown"].includes(e.key)
                  )
                    return;
                  e.preventDefault();
                  const target = index + (e.key === "ArrowUp" ? -1 : 1);
                  if (target < 0 || target >= active.length) return;
                  const next = [...active];
                  [next[index], next[target]] = [next[target], next[index]];
                  change([...next, ...members.filter((f) => f.archived)]);
                  setNotice(text("whole.reordered"));
                }}
              >
                {f.label || text("field.emptyLabel")}
              </Button>
            </div>
          ))}
          <div className="repeat-member-tab repeat-member-add-tab">
            <Button
              type="button"
              appearance="subtle"
              aria-label={text("group.newMember")}
              title={text("group.newMember")}
              disabled={disabled || composing}
              onClick={addMember}
            >
              +
            </Button>
          </div>
        </div>
        {current && !current.archived && (
          <div
            className="repeat-member-panel"
            role="tabpanel"
            id={`member-panel-${current.id}`}
            aria-labelledby={`member-tab-${current.id}`}
          >
            <PropertyRow
              label={text("field.label")}
              htmlFor={"whole-field-" + current.id}
              complex
            >
              <div className="field-name-controls">
                <Input
                  id={"whole-field-" + current.id}
                  value={current.label}
                  disabled={disabled}
                  onChange={(e) =>
                    update((f) => ({ ...f, label: e.target.value }))
                  }
                />
                <Checkbox
                  label={text("field.required")}
                  checked={current.required}
                  disabled={disabled}
                  onChange={(_, data) =>
                    update((f) => ({
                      ...f,
                      required: data.checked === true,
                    }))
                  }
                />
              </div>
            </PropertyRow>
            <PropertyRow
              label={text("field.kind")}
              htmlFor={"member-kind-" + current.id}
            >
              <Select
                id={"member-kind-" + current.id}
                disabled={disabled || !!old}
                value={current.configuration.kind}
                onChange={(e) => {
                  const kind = e.target.value as
                    | "rich_text"
                    | "number"
                    | "image"
                    | "file"
                    | "relation"
                    | "document_link";
                  update((f) => ({
                    ...f,
                    configuration:
                      kind === "relation"
                        ? {
                            kind,
                            multiple: true,
                            allowedTemplates: [],
                            reciprocalNotice: true,
                          }
                        : { kind },
                    default: { intent: "unset" },
                  }));
                }}
              >
                {(
                  [
                    "rich_text",
                    "number",
                    "image",
                    "file",
                    "relation",
                    "document_link",
                  ] as const
                ).map((kind) => (
                  <option key={kind} value={kind}>
                    {text(`whole.kind.${kind}`)}
                  </option>
                ))}
              </Select>
              {!!old && <HelpText>{text("field.kindLocked")}</HelpText>}
            </PropertyRow>
            <PropertyRow
              label={text("field.writingGuide")}
              htmlFor={"member-guide-" + current.id}
            >
              <Textarea
                id={"member-guide-" + current.id}
                disabled={disabled}
                value={
                  current.writingGuide?.intent === "set"
                    ? current.writingGuide.value
                    : current.writingGuide?.intent === "unset"
                      ? ""
                      : (old?.writingGuide ?? "")
                }
                onChange={(e) =>
                  update((f) => ({
                    ...f,
                    writingGuide: { intent: "set", value: e.target.value },
                  }))
                }
              />
            </PropertyRow>
            {current.configuration.kind === "number" &&
              (["minimum", "maximum"] as const).map((bound) => (
                <PropertyRow
                  key={bound}
                  label={text(`field.${bound}`)}
                  htmlFor={`member-${bound}-${current.id}`}
                >
                  <Input
                    id={`member-${bound}-${current.id}`}
                    disabled={disabled}
                    value={
                      current.configuration.kind === "number"
                        ? (current.configuration[bound] ?? "")
                        : ""
                    }
                    onChange={(e) =>
                      update((f) => ({
                        ...f,
                        configuration:
                          f.configuration.kind === "number"
                            ? { ...f.configuration, [bound]: e.target.value }
                            : f.configuration,
                      }))
                    }
                  />
                </PropertyRow>
              ))}
            {current.configuration.kind === "relation" && (
              <>
                <PropertyRow label={text("relation.multiple")}>
                  <Checkbox
                    aria-label={text("relation.multiple")}
                    disabled={disabled}
                    checked={current.configuration.multiple}
                    onChange={(_, data) =>
                      update((field) => ({
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
                <PropertyRow label={text("relation.allowedTemplates")} complex>
                  <Select
                    multiple
                    disabled={disabled}
                    value={current.configuration.allowedTemplates}
                    onChange={(event) =>
                      update((field) => ({
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
                    disabled={disabled}
                    checked={current.configuration.reciprocalNotice}
                    onChange={(_, data) =>
                      update((field) => ({
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
            <Button
              type="button"
              danger
              disabled={disabled || composing}
              onClick={() => {
                change(
                  old
                    ? archiveDefinition(members, current.id)
                    : members.filter((f) => f.id !== current.id),
                );
                setSelected(null);
              }}
            >
              {text(old ? "whole.archive" : "whole.removeNew")}
            </Button>
          </div>
        )}
      </div>
      {!!members.filter((f) => f.archived).length && (
        <details>
          <summary>{text("whole.archived")}</summary>
          {members
            .filter((f) => f.archived)
            .map((f) => (
              <div key={f.id}>
                <p>{f.label}</p>
                <Button
                  type="button"
                  disabled={disabled || composing}
                  onClick={() => {
                    change(
                      restoreDefinition(
                        members,
                        f.id,
                        original?.members?.find((m) => m.id === canonical(f.id))
                          ?.lifecycle === "Archived",
                      ),
                    );
                    setSelected(f.id);
                    setNotice(text("archive.restoredDraft"));
                  }}
                >
                  {text(
                    original?.members?.find((m) => m.id === canonical(f.id))
                      ?.lifecycle === "Archived"
                      ? "archive.restore"
                      : "archive.undo",
                  )}
                </Button>
              </div>
            ))}
        </details>
      )}
      <HelpText>{text("whole.fieldReorderHelp")}</HelpText>
      <span role="status">{notice}</span>
    </section>
  );
}
