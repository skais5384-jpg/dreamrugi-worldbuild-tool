import type { Field, Template } from "../bridge/types";
import { text } from "../strings";
import { fieldKind } from "./fieldEditing";
import { ValueRead } from "./FieldValue";
import { PropertyRow } from "./PropertyRow";
import { lifecycleLabel } from "./statusLabels";
import type { ReactNode } from "react";
import { Fragment } from "react";
import { SectionTitles } from "./Presentation";
import { Badge } from "@fluentui/react-components";

function orderedFields(template: Template) {
  const listed = template.fieldOrder.flatMap((id) =>
    template.fields.filter((field) => field.id === id),
  );
  return [
    ...listed,
    ...template.fields.filter(
      (field) => !template.fieldOrder.includes(field.id),
    ),
  ];
}

function orderedOptions(field: Field) {
  const listed = field.optionOrder.flatMap((id) =>
    field.options.filter((option) => option.id === id),
  );
  return [
    ...listed,
    ...field.options.filter((option) => !field.optionOrder.includes(option.id)),
  ];
}

function ReadonlyField({ field }: { field: Field }) {
  const kind = field.kind === "Group" ? "group" : fieldKind(field);
  return (
    <section className="template-read-field">
      <h3>{field.label || text("field.emptyLabel")}</h3>
      <PropertyRow label={text("field.label")}>
        <span>{field.label || text("field.emptyLabel")}</span>
      </PropertyRow>
      <PropertyRow label={text("field.kind")}>
        <span>{kind ? text(`whole.kind.${kind}`) : text("field.unknown")}</span>
      </PropertyRow>
      <PropertyRow label={text("field.status")}>
        <Badge
          appearance="tint"
          color={field.lifecycle === "Active" ? "success" : "informative"}
        >
          {text(
            field.lifecycle === "Active" ? "field.active" : "field.archived",
          )}
        </Badge>
      </PropertyRow>
      <PropertyRow label={text("field.required")}>
        <span>
          {text(field.required ? "field.requiredTrue" : "field.requiredFalse")}
        </span>
      </PropertyRow>
      <PropertyRow label={text("field.default")}>
        <ValueRead value={field.default} options={field.options} />
      </PropertyRow>
      <PropertyRow label={text("field.initial")}>
        <ValueRead value={field.initialDefault} options={field.options} />
      </PropertyRow>
      {!!field.members?.length && (
        <details>
          <summary>{text("group.members")}</summary>
          {[
            ...(field.memberOrder ?? []),
            ...field.members
              .filter((f) => f.lifecycle !== "Active")
              .map((f) => f.id),
          ]
            .map((id) => field.members?.find((f) => f.id === id))
            .filter((f): f is Field => !!f)
            .map((child) => (
              <ReadonlyField key={child.id} field={child} />
            ))}
        </details>
      )}
      {!!field.options.length && (
        <PropertyRow label={text("field.options")}>
          <ul className="template-read-options">
            {orderedOptions(field).map((option) => (
              <li key={option.id}>
                <span>{option.label || text("field.emptyLabel")}</span>
                <span>
                  {option.lifecycle === "Active"
                    ? text("field.active")
                    : text("field.archived")}
                </span>
              </li>
            ))}
          </ul>
        </PropertyRow>
      )}
    </section>
  );
}

/** 삭제된 Template도 수정 가능한 입력처럼 보이지 않게 같은 속성 대응만 제공한다. */
export function ReadonlyTemplate({
  template,
  actions,
}: {
  template: Template;
  actions?: ReactNode;
}) {
  return (
    <section className="template-read-detail">
      <header className="template-detail-header">
        <div>
          <h2>{template.name || text("app.message19")}</h2>
          <Badge
            appearance="tint"
            color={template.lifecycle === "Active" ? "success" : "warning"}
          >
            {lifecycleLabel(template.lifecycle)}
          </Badge>
        </div>
        {actions && <div className="actions">{actions}</div>}
      </header>
      {!!template.fields.length && (
        <section aria-label={text("whole.fields")}>
          <h2>{text("whole.fields")}</h2>
          {orderedFields(template).map((field) => (
            <Fragment key={field.id}>
              <SectionTitles template={template} before={field.id} />
              <ReadonlyField field={field} />
            </Fragment>
          ))}
          <SectionTitles template={template} before={null} />
        </section>
      )}
    </section>
  );
}
