import { missingRequired } from "./requiredWarnings";
import { InlineNotice } from "../ui/InlineNotice";
import { useState } from "react";
import { Tooltip } from "@fluentui/react-components";
import {
  Add20Regular,
  Copy20Regular,
  Delete20Regular,
  ChevronDown20Regular,
  ChevronRight20Regular,
  ReOrderDotsVertical20Regular,
} from "@fluentui/react-icons";
import { Button } from "../ui/Controls";
import { text } from "../strings";
import type { Field } from "../bridge/types";
import { CreationValue } from "./CreationValue";
import { ValueRead } from "./FieldValue";
import { blockField, PropertyRow } from "./PropertyRow";
import { HelpText } from "../ui/HelpText";
import { useDraftReorder } from "./useDraftReorder";
import {
  cellInvalid,
  cellKey,
  cellValue,
  copyCard,
  type GroupContext,
  type GroupValue,
  type GroupInstance,
} from "./groups";
import "./Groups.css";

export function GroupEditor({
  field,
  value,
  change,
  context,
  disabled,
  prefix,
}: {
  field: Field;
  value: GroupValue;
  change: (v: GroupValue) => void;
  context: GroupContext;
  disabled: boolean;
  prefix: string;
}) {
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  const [notice, setNotice] = useState("");
  const order = value.instances.map((i) => i.id);
  const canReorder = order.length > 1 && !disabled && !context.composing;
  const reorder = useDraftReorder(
    context.owner,
    context.generation,
    canReorder,
    (_, ids) =>
      change({
        kind: "group",
        instances: ids.map((id) => value.instances.find((i) => i.id === id)!),
      }),
    setNotice,
  );
  const replace = (id: string, next: GroupInstance) =>
    change({
      ...value,
      instances: value.instances.map((i) => (i.id === id ? next : i)),
    });
  const members = [
    ...(field.memberOrder ?? []),
    ...(field.members ?? [])
      .filter((f) => f.lifecycle !== "Active")
      .map((f) => f.id),
  ]
    .map((id) => field.members?.find((f) => f.id === id))
    .filter((f): f is Field => !!f);
  return (
    <section
      className="repeat-group"
      aria-label={field.label}
      {...reorder.surface}
    >
      <Button
        type="button"
        icon={<Add20Regular />}
        disabled={disabled || context.composing}
        onClick={() =>
          change({
            ...value,
            instances: [
              ...value.instances,
              { id: crypto.randomUUID(), source: null, fields: [] },
            ],
          })
        }
      >
        {text("group.add")}
      </Button>
      {!order.length && <p>{text("group.empty")}</p>}
      <div className="repeat-cards">
        {value.instances.map((card, index) => {
          const source = context.baseline?.instances.find(
            (i) => i.id === (card.source ?? card.id),
          );
          const invalid = members.find(
            (f) =>
              f.lifecycle === "Active" &&
              (cellInvalid(f, cellValue(card, f.id, context.baseline)) ||
                context.problem === cellKey(field.id, card.id, f.id)),
          );
          const drag = reorder.card(field.id, card.id, order);
          return (
            <section
              key={card.id}
              {...drag}
              draggable={false}
              className={`repeat-card ${drag.className ?? ""}`}
            >
              <div className="repeat-header">
                {canReorder && (
                  <Tooltip
                    content={text("whole.fieldReorderHelp")}
                    relationship="description"
                  >
                    <Button
                      type="button"
                      appearance="subtle"
                      className="repeat-drag-handle"
                      icon={<ReOrderDotsVertical20Regular />}
                      aria-label={text("group.reorder")}
                      aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                      draggable
                      onDragStart={drag.onDragStart}
                      onDragEnd={drag.onDragEnd}
                      onKeyDown={(e) => {
                        if (
                          !e.altKey ||
                          e.ctrlKey ||
                          e.metaKey ||
                          e.shiftKey ||
                          !["ArrowUp", "ArrowDown"].includes(e.key)
                        )
                          return;
                        e.preventDefault();
                        const target = index + (e.key === "ArrowUp" ? -1 : 1);
                        if (target < 0 || target >= order.length) return;
                        const instances = [...value.instances];
                        [instances[index], instances[target]] = [
                          instances[target],
                          instances[index],
                        ];
                        change({ ...value, instances });
                        setNotice(text("whole.reordered"));
                      }}
                    />
                  </Tooltip>
                )}
                <Button
                  type="button"
                  appearance="subtle"
                  icon={
                    folded[card.id] ? (
                      <ChevronRight20Regular />
                    ) : (
                      <ChevronDown20Regular />
                    )
                  }
                  aria-expanded={!folded[card.id]}
                  onClick={() =>
                    setFolded({ ...folded, [card.id]: !folded[card.id] })
                  }
                >
                  {text("group.card")} {index + 1}
                </Button>
                <Tooltip content={text("group.duplicate")} relationship="label">
                  <Button
                    type="button"
                    icon={<Copy20Regular />}
                    aria-label={text("group.duplicate")}
                    disabled={disabled || context.composing}
                    onClick={() => {
                      const copy = copyCard(
                        card,
                        crypto.randomUUID(),
                        context.baseline,
                      );
                      change({
                        ...value,
                        instances: [
                          ...value.instances.slice(0, index + 1),
                          copy,
                          ...value.instances.slice(index + 1),
                        ],
                      });
                    }}
                  />
                </Tooltip>
                <Tooltip content={text("group.delete")} relationship="label">
                  <Button
                    type="button"
                    danger
                    icon={<Delete20Regular />}
                    aria-label={text("group.delete")}
                    disabled={disabled || context.composing}
                    onClick={() =>
                      change({
                        ...value,
                        instances: value.instances.filter(
                          (i) => i.id !== card.id,
                        ),
                      })
                    }
                  />
                </Tooltip>
                {invalid && (
                  <Button
                    type="button"
                    onClick={() => {
                      setFolded({ ...folded, [card.id]: false });
                      requestAnimationFrame(() =>
                        document
                          .getElementById(
                            prefix + cellKey(field.id, card.id, invalid.id),
                          )
                          ?.focus(),
                      );
                    }}
                  >
                    {text("group.error")} · {invalid.label}
                  </Button>
                )}
              </div>
              {/* 접기는 보기 상태뿐이다. 입력 DOM과 진행 중 첨부/IME owner를 보존한다. */}
              <div hidden={!!folded[card.id]}>
                {members.map((child) => {
                  const key = cellKey(field.id, card.id, child.id);
                  const intent = cellValue(card, child.id, context.baseline);
                  const readOnly =
                    child.lifecycle !== "Active" ||
                    source?.protected?.includes(child.id);
                  return (
                    <PropertyRow
                      key={child.id}
                      requiredAnchor={key}
                      label={
                        (child.lifecycle === "Active"
                          ? child.label
                          : (source?.labels?.[child.id] ?? child.label)) +
                        (child.required ? " *" : "")
                      }
                      htmlFor={prefix + key}
                      complex
                      block={blockField(child.kind)}
                    >
                      {missingRequired(
                        child,
                        intent.intent === "set" ? intent.value : null,
                      ) && (
                        <InlineNotice kind="warning">
                          {text("required.missing")}
                        </InlineNotice>
                      )}
                      {readOnly ? (
                        <>
                          <ValueRead
                            value={
                              intent.intent === "set"
                                ? intent.value
                                : { kind: "unset" }
                            }
                            options={child.options}
                            reference={context.reference}
                          />
                          <HelpText>{text("documentEdit.kept")}</HelpText>
                        </>
                      ) : (
                        <CreationValue
                          field={{ ...child, id: key }}
                          prefix={prefix}
                          intent={intent}
                          disabled={disabled}
                          invalid={
                            cellInvalid(child, intent) ||
                            context.problem === key
                          }
                          importAsset={
                            context.importCell
                              ? () =>
                                  context.importCell!(
                                    { instance: card.id, child: child.id },
                                    child.kind === "Image",
                                  )
                              : undefined
                          }
                          reference={context.reference}
                          change={(value) =>
                            replace(card.id, {
                              ...card,
                              fields: [
                                ...card.fields.filter(
                                  (f) => f.field !== child.id,
                                ),
                                { field: child.id, value },
                              ],
                            })
                          }
                        />
                      )}
                    </PropertyRow>
                  );
                })}
              </div>
            </section>
          );
        })}
      </div>
      <HelpText>{text("whole.fieldReorderHelp")}</HelpText>
      <span className="sr-only" role="status">
        {notice}
      </span>
    </section>
  );
}

function hasTitleContent(node: import("../bridge/types").RichNode): boolean {
  if (node.kind === "taskItem") return true;
  return node.kind === "text"
    ? !!node.text.trim()
    : node.kind === "hardBreak"
      ? false
      : node.children.some(hasTitleContent);
}
export function GroupRead({
  value,
  field,
  reference,
}: {
  value: GroupValue;
  field?: Field;
  reference?: import("./DocumentReferenceValue").ReferenceContext;
}) {
  return (
    <div className="repeat-cards">
      {value.instances.map((card, index) => {
        const titleField = field?.members?.find(
          (f) =>
            f.id === field.cardTitleField &&
            f.lifecycle === "Active" &&
            f.kind === "RichText",
        );
        const title =
          titleField && card.fields.find((c) => c.field === titleField.id);
        const titleValue =
          title?.value.intent === "set" ? title.value.value : null;
        const promoted =
          !!titleField &&
          !card.protected?.includes(titleField.id) &&
          (titleValue?.kind === "rich_text" ||
            titleValue?.kind === "unset" ||
            title?.value.intent === "unset");
        return (
          <section
            className="repeat-card"
            key={card.id}
            aria-label={`${text("group.card")} ${index + 1}`}
          >
            {promoted &&
              titleValue?.kind === "rich_text" &&
              hasTitleContent(titleValue.content) && (
                <div className="repeat-card-title">
                  <ValueRead
                    value={titleValue}
                    options={[]}
                    reference={reference}
                  />
                </div>
              )}
            {promoted &&
              titleField &&
              missingRequired(titleField, titleValue) && (
                <InlineNotice kind="warning">
                  {text("required.missing")}
                </InlineNotice>
              )}
            {[
              ...(field?.memberOrder ?? []).map(
                (id) =>
                  card.fields.find((c) => c.field === id) ?? {
                    field: id,
                    value: { intent: "unset" as const },
                  },
              ),
              ...card.fields.filter(
                (c) => !field?.memberOrder?.includes(c.field),
              ),
            ]
              .filter(
                (c): c is GroupInstance["fields"][number] =>
                  !!c && !(promoted && c.field === titleField?.id),
              )
              .map((cell) => {
                const child = field?.members?.find((f) => f.id === cell.field);
                return (
                  <PropertyRow
                    key={cell.field}
                    label={
                      child?.lifecycle === "Active"
                        ? child.label
                        : (card.labels?.[cell.field] ??
                          child?.label ??
                          "이름을 확인할 수 없는 하위 필드")
                    }
                    complex
                    block={blockField(child?.kind)}
                  >
                    {child &&
                      missingRequired(
                        child,
                        cell.value.intent === "set" ? cell.value.value : null,
                      ) && (
                        <InlineNotice kind="warning">
                          {text("required.missing")}
                        </InlineNotice>
                      )}
                    <ValueRead
                      value={
                        cell.value.intent === "set"
                          ? cell.value.value
                          : { kind: "unset" }
                      }
                      options={child?.options ?? []}
                      field={child}
                      reference={reference}
                    />
                  </PropertyRow>
                );
              })}
          </section>
        );
      })}
    </div>
  );
}
