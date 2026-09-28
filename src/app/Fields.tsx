import { Label } from "@fluentui/react-components";
import { HelpText } from "../ui/HelpText";
import { Button, Textarea, Select, Fieldset, Checkbox } from "../ui/Controls";
import { useEffect, useRef } from "react";
import type { TemplateController } from "./controller";
import {
  fieldKind,
  kinds,
  type FieldDraft,
  type FieldEdit,
  editOwner,
  fieldDirty,
} from "./fieldEditing";
import { OrderInput, ArchiveInput } from "./ManagementInput";
import { ValueInput, ValueRead } from "./FieldValue";
import type { FieldConfiguration } from "../bridge/types";
import { text } from "../strings";
import { useImeGuard } from "./ime";

function DraftForm({
  controller,
  draft,
  locked,
}: {
  controller: TemplateController;
  draft: FieldDraft;
  locked: boolean;
}) {
  const ime = useImeGuard();
  const heading = useRef<HTMLHeadingElement>(null);
  const management =
    draft.property.startsWith("option:") ||
    ["fields_order", "options_order", "archive_field"].includes(draft.property);
  useEffect(() => {
    if (management) heading.current?.focus();
  }, [management]);
  const state = controller.snapshot();
  const edit = draft.edit;
  const current = draft.source.content.fields.find(
    (f) => f.id === editOwner(edit),
  );
  const latest = state.selection?.content.fields.find(
    (f) => f.id === editOwner(edit),
  );
  const unavailable =
    edit.kind !== "create_field" &&
    edit.kind !== "reorder_fields" &&
    (latest?.lifecycle !== "Active" ||
      ((edit.kind === "rename_option" || edit.kind === "archive_option") &&
        !latest.options.some(
          (o) => o.id === edit.option && o.lifecycle === "Active",
        )));
  const kind =
    edit.kind === "create_field"
      ? edit.configuration.kind
      : current && fieldKind(current);
  const stale = state.selection?.view !== draft.source.view;
  const id = `field-${draft.property}`;
  const options =
    edit.kind === "create_field" && "options" in edit.configuration
      ? edit.configuration.options.map((o) => ({ ...o, lifecycle: "Active" }))
      : (current?.options ?? []);
  const change = (edit: FieldEdit) => controller.setField(draft.property, edit);
  const title =
    edit.kind === "add_option"
      ? text("option.add")
      : edit.kind === "rename_option"
        ? text("option.rename")
        : edit.kind === "reorder_fields"
          ? text("order.fields")
          : edit.kind === "reorder_options"
            ? text("order.options")
            : edit.kind === "archive_field"
              ? text("archive.field")
              : edit.kind === "archive_option"
                ? text("archive.option")
                : draft.property === "create"
                  ? text("field.add")
                  : text(
                      `field.${draft.property as "label" | "required" | "presentation" | "default"}`,
                    );
  return (
    <form
      {...ime.bind}
      className={
        management
          ? "editor field-editor management-form"
          : draft.property === "create"
            ? "editor field-editor field-create"
            : "editor field-editor"
      }
      aria-label={title}
      onSubmit={(e) => {
        e.preventDefault();
        // 기본 버튼 click에서 들어와도 저장 callback 직전의 조합 경계를 통과해야 한다.
        if (ime.allowAction(e)) void controller.saveField(draft.property);
      }}
    >
      {/* 단일 속성은 연결된 label로 구분하고, 관리 동작에만 진입 제목을 둔다. */}
      {(management || draft.property === "create") && (
        <h3 ref={heading} tabIndex={-1}>
          {title}
        </h3>
      )}
      <Fieldset
        disabled={locked || draft.submitted || unavailable}
        aria-describedby={draft.message ? id + "-message" : undefined}
      >
        {(edit.kind === "reorder_fields" ||
          edit.kind === "reorder_options") && (
          <OrderInput controller={controller} draft={draft} />
        )}
        {(edit.kind === "archive_field" || edit.kind === "archive_option") && (
          <ArchiveInput controller={controller} draft={draft} />
        )}
        {edit.kind === "add_option" && (
          <>
            <Label htmlFor={id + "-label"}>{text("option.label")}</Label>
            <Textarea
              id={id + "-label"}
              rows={2}
              value={edit.option.label}
              onChange={(e) =>
                change({
                  ...edit,
                  option: { ...edit.option, label: e.target.value },
                })
              }
            />
            <span className="identifier">{edit.option.id}</span>
          </>
        )}
        {"label" in edit && (
          <>
            <Label htmlFor={id + "-label"}>
              {text(
                edit.kind === "rename_option" ? "option.label" : "field.label",
              )}
            </Label>
            <Textarea
              id={id + "-label"}
              rows={2}
              value={edit.label}
              onChange={(e) => change({ ...edit, label: e.target.value })}
            />
          </>
        )}
        {edit.kind === "create_field" && (
          <>
            <Label htmlFor={id + "-kind"}>{text("field.kind")}</Label>
            <Select
              id={id + "-kind"}
              value={edit.configuration.kind}
              onChange={(e) => {
                const kind = e.target.value as FieldConfiguration["kind"];
                change({
                  ...edit,
                  configuration:
                    kind === "single_choice" || kind === "multi_choice"
                      ? { kind, options: [] }
                      : kind === "relation"
                        ? {
                            kind,
                            multiple: true,
                            allowedTemplates: [],
                            reciprocalNotice: true,
                          }
                        : { kind },
                  default: { kind: "unset" },
                });
              }}
            >
              {Object.values(kinds).map((kind) => (
                <option key={kind} value={kind}>
                  {text(`field.${kind}`)}
                </option>
              ))}
            </Select>
            {"options" in edit.configuration && (
              <>
                <Label htmlFor={id + "-options"}>
                  {text("field.initialOptions")}
                </Label>
                <Textarea
                  id={id + "-options"}
                  rows={3}
                  value={edit.configuration.options
                    .map((o) => o.label)
                    .join("\n")}
                  onChange={(e) => {
                    if (!("options" in edit.configuration)) return;
                    const previous = edit.configuration.options;
                    change({
                      ...edit,
                      configuration: {
                        ...edit.configuration,
                        options:
                          e.target.value === ""
                            ? []
                            : e.target.value
                                .split("\n")
                                .map((label, index) => ({
                                  id:
                                    previous[index]?.id ?? crypto.randomUUID(),
                                  label,
                                })),
                      },
                    });
                  }}
                />
                <HelpText>{text("field.optionsHelp")}</HelpText>
              </>
            )}
          </>
        )}
        {"required" in edit && (
          <>
            <Checkbox
              label={text("field.required")}
              checked={edit.required}
              onChange={(e) => change({ ...edit, required: e.target.checked })}
            />
            <HelpText>{text("field.requiredHelp")}</HelpText>
          </>
        )}
        {kind &&
          (edit.kind === "create_field" ||
            edit.kind === "default" ||
            edit.kind === "keep_default") && (
            <ValueInput
              id={id + "-value"}
              kind={kind}
              value={
                edit.kind === "create_field"
                  ? edit.default
                  : edit.kind === "default"
                    ? edit.value
                    : null
              }
              keep={edit.kind !== "create_field"}
              options={options}
              onChange={(value) =>
                change(
                  edit.kind === "create_field"
                    ? { ...edit, default: value ?? { kind: "unset" } }
                    : value === null
                      ? { kind: "keep_default", field: edit.field }
                      : { kind: "default", field: edit.field, value },
                )
              }
            />
          )}
      </Fieldset>
      <p className="draft-meta">
        {text("field.basis", { revision: draft.source.content.revision })}
      </p>
      {stale && <p>{text("field.stale")}</p>}
      {unavailable && <p>{text("field.unavailable")}</p>}
      {draft.message && (
        <p id={id + "-message"} role="status">
          {draft.message}
        </p>
      )}
      {draft.submitted && <p>{text("field.frozen")}</p>}
      <div className="actions">
        {(draft.property.startsWith("option:") ||
          draft.property === "archive_field") && (
          <Button
            type="button"
            disabled={locked || draft.submitted}
            onClick={() => controller.cancelFieldDraft(draft.property)}
          >
            {text("field.cancelDraft")}
          </Button>
        )}
        <Button
          type="submit"
          appearance="primary"
          disabled={locked || draft.submitted || unavailable}
        >
          {text("field.apply", { property: title })}
        </Button>
        {(stale || draft.committed) && (
          <Button
            type="button"
            disabled={locked || (draft.submitted && !draft.committed)}
            onClick={() => void controller.reconfirmField(draft.property)}
          >
            {text(draft.committed ? "field.readSaved" : "field.reconfirm")}
          </Button>
        )}
      </div>
    </form>
  );
}

export function Fields({
  controller,
  locked,
  canEdit,
}: {
  controller: TemplateController;
  locked: boolean;
  canEdit: boolean;
}) {
  const state = controller.snapshot();
  const source = state.selection;
  if (!source) return null;
  const editor = state.fieldEditor;
  const active = source.content.fieldOrder.flatMap((id) =>
    source.content.fields.filter((f) => f.id === id),
  );
  const archived = source.content.fields.filter(
    (f) => !source.content.fieldOrder.includes(f.id),
  );
  const selected = source.content.fields.find((f) => f.id === editor?.field);
  const submitted = !!editor?.drafts.some((d) => d.submitted && !d.committed);
  const editable = canEdit && source.content.lifecycle === "Active";
  // retained 후속 처리는 입력을 숨길 이유가 아니다. 실제 읽기 전용 정의만 form을 닫는다.
  const draftsVisible =
    state.project?.status === "Ready" &&
    state.project.runtime === "Ready" &&
    source.content.lifecycle === "Active" &&
    (!selected || selected.lifecycle === "Active");
  return (
    <section aria-label={text("field.title")}>
      <div className="panel-heading">
        <h3>{text("field.title")}</h3>
        {source.content.lifecycle === "Active" && (
          <div
            className="actions"
            role="group"
            aria-label={text("field.title")}
          >
            <Button
              type="button"
              disabled={locked || !editable || submitted}
              onClick={() => void controller.navigate({ kind: "fields_order" })}
            >
              {text("order.fields")}
            </Button>
            <Button
              type="button"
              disabled={locked || !editable || submitted}
              onClick={() => void controller.navigate({ kind: "new_field" })}
            >
              {text("field.add")}
            </Button>
          </div>
        )}
      </div>
      {!source.content.fields.length && <p>{text("field.empty")}</p>}
      <ul className="fields">
        {[...active, ...archived].map((field) => (
          <li key={field.id}>
            <Button
              type="button"
              aria-pressed={editor?.field === field.id}
              disabled={locked || submitted}
              onClick={() =>
                void controller.navigate({ kind: "field", id: field.id })
              }
            >
              {field.label || text("field.emptyLabel")}
            </Button>
            <span>
              {fieldKind(field)
                ? text(`field.${fieldKind(field)!}`)
                : text("field.unknown")}{" "}
              ·{" "}
              {field.lifecycle === "Active"
                ? text("field.active")
                : text("field.archived")}
            </span>
            <span className="identifier">{field.id}</span>
          </li>
        ))}
      </ul>
      {selected && (
        <div className="field-readback">
          <p>
            {text("field.introduced", {
              revision: selected.introducedRevision,
            })}
          </p>
          <p>
            {text(
              selected.required ? "field.requiredTrue" : "field.requiredFalse",
            )}
          </p>
          <strong>{text("field.default")}</strong>
          <div>
            <ValueRead value={selected.default} options={selected.options} />
          </div>
          <strong>{text("field.initial")}</strong>
          <div>
            <ValueRead
              value={selected.initialDefault}
              options={selected.options}
            />
          </div>
          {!!selected.options.length && (
            <>
              <strong>{text("field.options")}</strong>
              <ul>
                {[
                  ...selected.optionOrder.flatMap((id) =>
                    selected.options.filter((o) => o.id === id),
                  ),
                  ...selected.options.filter(
                    (o) => !selected.optionOrder.includes(o.id),
                  ),
                ].map((o) => (
                  <li className="option-row" key={o.id}>
                    <span className="literal">
                      {o.label || text("field.emptyLabel")}
                    </span>
                    <span>
                      {o.lifecycle === "Active"
                        ? text("field.active")
                        : text("field.archived")}
                    </span>
                    <span className="identifier">{o.id}</span>
                    <div className="actions">
                      {editable &&
                        selected.lifecycle === "Active" &&
                        o.lifecycle === "Active" && (
                          <Button
                            type="button"
                            disabled={locked || submitted}
                            onClick={() => controller.startOption(o.id)}
                          >
                            {text("option.rename")}
                          </Button>
                        )}
                      {editable &&
                        selected.lifecycle === "Active" &&
                        o.lifecycle === "Active" && (
                          <Button
                            type="button"
                            disabled={locked || submitted}
                            onClick={() => controller.startArchive(o.id)}
                            danger
                          >
                            {text("archive.option")}
                          </Button>
                        )}
                    </div>
                  </li>
                ))}
              </ul>
            </>
          )}
          <div
            className="actions definition-actions"
            role="group"
            aria-label={text("field.options")}
          >
            {editable &&
              selected.lifecycle === "Active" &&
              ["single_choice", "multi_choice"].includes(
                fieldKind(selected),
              ) && (
                <Button
                  type="button"
                  disabled={locked || submitted}
                  onClick={() => controller.startOption()}
                >
                  {text("option.add")}
                </Button>
              )}
            {editable &&
              selected.lifecycle === "Active" &&
              ["single_choice", "multi_choice"].includes(
                fieldKind(selected),
              ) && (
                <Button
                  type="button"
                  disabled={locked || submitted}
                  onClick={() => controller.startOrder()}
                >
                  {text("order.options")}
                </Button>
              )}
          </div>
          {editable && selected.lifecycle === "Active" && (
            <Button
              type="button"
              disabled={locked || submitted}
              onClick={() => controller.startArchive()}
              danger
            >
              {text("archive.field")}
            </Button>
          )}
        </div>
      )}
      {editor && (
        <>
          <div className="field-forms">
            {(draftsVisible ? editor.drafts : [])
              .filter((draft) => draft.property !== "presentation")
              .map((draft) => (
                <DraftForm
                  key={`${editor.generation}-${draft.property}`}
                  controller={controller}
                  draft={draft}
                  locked={locked || !editable}
                />
              ))}
          </div>
          {!draftsVisible && editor.drafts.some(fieldDirty) && (
            <p role="status">{text("field.readOnlyDrafts")}</p>
          )}
          <Button
            type="button"
            disabled={locked || submitted}
            onClick={() => void controller.navigate({ kind: "close_field" })}
          >
            {text("common.cancel")}
          </Button>
        </>
      )}
    </section>
  );
}
