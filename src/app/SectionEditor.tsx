import { useState } from "react";
import type { TemplateBody } from "../bridge/workspace";
import { Button, Input } from "../ui/Controls";
import { text } from "../strings";
import { useDraftReorder } from "./useDraftReorder";

export function repairSectionAnchors(
  previous: TemplateBody,
  next: TemplateBody,
): TemplateBody {
  const active = next.fields.filter((f) => !f.archived).map((f) => f.id);
  return {
    ...next,
    sections: next.sections?.map((s) => {
      if (!s.beforeField || active.includes(s.beforeField)) return s;
      const index = previous.fields.findIndex((f) => f.id === s.beforeField);
      return {
        ...s,
        beforeField:
          previous.fields.slice(index + 1).find((f) => active.includes(f.id))
            ?.id ?? null,
      };
    }),
  };
}

/** 저장 형식은 section anchor를 유지하되 편집 화면에서는 필드와 부제목을 한 순서로 다룬다. */
export function orderedTemplateItemIds(body: TemplateBody) {
  const sections = body.sections ?? [];
  const fields = body.fields.filter((field) => !field.archived);
  const known = new Set(fields.map((field) => field.id));
  return [
    ...fields.flatMap((field) => [
      ...sections
        .filter((section) => section.beforeField === field.id)
        .map((section) => section.id),
      field.id,
    ]),
    ...sections
      .filter(
        (section) =>
          section.beforeField === null || !known.has(section.beforeField),
      )
      .map((section) => section.id),
  ];
}

export function reorderTemplateItems(body: TemplateBody, ids: string[]) {
  const active = body.fields.filter((field) => !field.archived);
  return {
    ...body,
    fields: [
      ...ids.flatMap((id) => active.filter((field) => field.id === id)),
      ...body.fields.filter((field) => field.archived),
    ],
    sections: ids.flatMap((id, index) =>
      (body.sections ?? [])
        .filter((section) => section.id === id)
        .map((section) => ({
          ...section,
          beforeField:
            ids
              .slice(index + 1)
              .find((next) => active.some((field) => field.id === next)) ??
            null,
        })),
    ),
  };
}
/** 제목과 필드의 표시 순서를 한 목록에서 바꾸고 저장할 때 anchor로 표현한다. */
export function SectionEditor({
  body,
  owner,
  generation,
  disabled,
  change,
}: {
  body: TemplateBody;
  owner: string;
  generation: string;
  disabled: boolean;
  change: (edit: (body: TemplateBody) => TemplateBody) => void;
}) {
  const [selected, select] = useState<string | null>(null);
  const [notice, announce] = useState("");
  const sections = body.sections ?? [];
  const fields = body.fields.filter((f) => !f.archived);
  const order = orderedTemplateItemIds(body);
  const reorder = (ids: string[]) =>
    change((b) => reorderTemplateItems(b, ids));
  const drag = useDraftReorder(
    owner,
    generation,
    !disabled,
    (_, ids) => reorder(ids),
    announce,
  );
  const current = sections.find((s) => s.id === selected);
  return (
    <details className="section-editor" {...drag.surface}>
      <summary>{text("section.manage")}</summary>
      <Button
        type="button"
        disabled={disabled}
        onClick={() => {
          const id = crypto.randomUUID();
          select(id);
          change((b) => ({
            ...b,
            sections: [
              ...(b.sections ?? []),
              { id, title: text("section.new"), beforeField: null },
            ],
          }));
        }}
      >
        {text("section.add")}
      </Button>
      <ul className="whole-field-list">
        {order.map((id, index) => {
          const section = sections.find((s) => s.id === id);
          return (
            <li key={id} {...drag.card("sections", id, order)}>
              <Button
                type="button"
                disabled={disabled}
                aria-label={
                  text("section.manage") +
                  ": " +
                  (section?.title ??
                    fields.find((f) => f.id === id)?.label ??
                    "")
                }
                aria-pressed={selected === id}
                onClick={() => select(id)}
                aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                aria-description={text("whole.fieldReorderHelp")}
                onKeyDown={(event) => {
                  if (
                    disabled ||
                    !event.altKey ||
                    !["ArrowUp", "ArrowDown"].includes(event.key)
                  )
                    return;
                  event.preventDefault();
                  const next = index + (event.key === "ArrowUp" ? -1 : 1);
                  if (next < 0 || next >= order.length) return;
                  const ids = [...order];
                  [ids[index], ids[next]] = [ids[next], ids[index]];
                  reorder(ids);
                }}
              >
                {section
                  ? section.title
                  : fields.find((f) => f.id === id)?.label}
              </Button>
            </li>
          );
        })}
      </ul>
      {current && (
        <div className="actions">
          <Input
            aria-label={text("section.title")}
            disabled={disabled}
            value={current.title}
            onChange={(event) =>
              change((b) => ({
                ...b,
                sections: b.sections?.map((s) =>
                  s.id === current.id ? { ...s, title: event.target.value } : s,
                ),
              }))
            }
          />
          <Button
            type="button"
            disabled={disabled}
            onClick={() =>
              change((b) => ({
                ...b,
                sections: b.sections?.filter((s) => s.id !== current.id),
              }))
            }
          >
            {text("section.remove")}
          </Button>
        </div>
      )}
      <span role="status">{notice}</span>
    </details>
  );
}
