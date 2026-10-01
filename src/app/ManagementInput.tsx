import { Button, Checkbox } from "../ui/Controls";
import { ReOrderDotsVertical20Regular } from "@fluentui/react-icons";
import { HelpText } from "../ui/HelpText";
import { useRef, useState } from "react";
import type { TemplateController } from "./controller";
import {
  fieldKind,
  orderIds,
  referencesOption,
  type FieldDraft,
} from "./fieldEditing";
import { emptyValue, ValueInput, ValueRead } from "./FieldValue";
import { text } from "../strings";

const dragType = "application/x-worldbuild-order";

/** drag 중에는 디스크나 초안을 바꾸지 않는다. 유효한 같은 목록 drop만 키보드와 같은 이동을 적용한다. */
export function OrderInput({
  controller,
  draft,
}: {
  controller: TemplateController;
  draft: FieldDraft;
}) {
  const state = controller.snapshot();
  const ids = orderIds(draft.edit) ?? [];
  const drag = useRef<{
    id: string;
    token: string;
    generation: number;
    view: string;
    selection: string;
    order: string[];
  } | null>(null);
  const [announcement, setAnnouncement] = useState<{
    message: string;
    initial: FieldDraft["initial"];
    property: FieldDraft["property"];
    generation: number | undefined;
  } | null>(null);
  function announce(message: string) {
    setAnnouncement({
      message,
      initial: draft.initial,
      property: draft.property,
      generation: state.fieldEditor?.generation,
    });
  }
  // 확정 저장은 initial을 새로 만든다. 단순 재조회/다른 속성 저장은 원 기준을 유지하므로 안내도 보존한다.
  // 저장 뒤 조회가 실패한 경우에는 committed만으로 미저장 안내를 숨기고 폼의 후속 안내를 남긴다.
  const currentAnnouncement =
    !draft.committed &&
    announcement?.initial === draft.initial &&
    announcement.property === draft.property &&
    announcement.generation === state.fieldEditor?.generation
      ? announcement.message
      : "";
  const field = draft.source.content.fields.find(
    (f) => f.id === ("field" in draft.edit ? draft.edit.field : null),
  );
  const names =
    draft.edit.kind === "reorder_fields"
      ? draft.source.content.fields
      : (field?.options ?? []);
  const movable = controller.orderMovable(draft.property);
  function move(id: string, index: number) {
    controller.moveOrder(draft.property, id, index);
    announce(
      text("order.position", {
        name: names.find((n) => n.id === id)?.label || text("field.emptyLabel"),
        position: String(index + 1),
        count: String(ids.length),
      }),
    );
  }
  function validDrag() {
    const owner = drag.current;
    return (
      owner &&
      movable &&
      owner.generation === state.fieldEditor?.generation &&
      owner.view === draft.source.view &&
      owner.selection === state.selection?.view &&
      JSON.stringify(owner.order) === JSON.stringify(ids)
    );
  }
  return (
    <div
      className="order-input"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          drag.current = null;
          announce(text("order.dragCancelled"));
        }
      }}
    >
      <HelpText>{text("order.help")}</HelpText>
      <ol aria-label={text("order.preview")}>
        {ids.map((id, index) => {
          const name =
            names.find((n) => n.id === id)?.label || text("field.emptyLabel");
          return (
            <li
              key={id}
              tabIndex={-1}
              draggable={movable}
              data-order-id={id}
              onDragStart={(e) => {
                if (!movable || !state.fieldEditor || !state.selection) {
                  e.preventDefault();
                  return;
                }
                const token = crypto.randomUUID();
                drag.current = {
                  id,
                  token,
                  generation: state.fieldEditor.generation,
                  view: draft.source.view,
                  selection: state.selection.view,
                  order: [...ids],
                };
                e.dataTransfer.setData(dragType, token);
                e.dataTransfer.effectAllowed = "move";
                announce(text("order.dragStarted", { name }));
              }}
              onDragEnter={(e) => {
                if (validDrag() && e.dataTransfer.types.includes(dragType)) {
                  e.preventDefault();
                }
              }}
              onDragOver={(e) => {
                if (validDrag() && e.dataTransfer.types.includes(dragType)) {
                  e.preventDefault();
                  e.dataTransfer.dropEffect = "move";
                }
              }}
              onDrop={(e) => {
                const owner = drag.current;
                if (
                  validDrag() &&
                  owner &&
                  e.dataTransfer.getData(dragType) === owner.token &&
                  !e.dataTransfer.files.length
                ) {
                  e.preventDefault();
                  move(owner.id, index);
                  const row = Array.from(
                    e.currentTarget.parentElement?.children ?? [],
                  ).find(
                    (item) =>
                      item instanceof HTMLElement &&
                      item.dataset.orderId === owner.id,
                  );
                  if (row instanceof HTMLElement) row.focus();
                }
                drag.current = null;
              }}
              onDragEnd={() => {
                if (drag.current) announce(text("order.dragCancelled"));
                drag.current = null;
              }}
            >
              <div className="order-card-heading">
                {movable && (
                  <ReOrderDotsVertical20Regular
                    className="order-drag-handle"
                    aria-hidden="true"
                  />
                )}
                <span className="literal">{name}</span>
              </div>
              <span className="identifier">{id}</span>
              <span>
                {text("order.ordinal", {
                  position: String(index + 1),
                  count: String(ids.length),
                })}
              </span>
              <div className="actions">
                <Button
                  type="button"
                  disabled={!movable || index === 0}
                  aria-label={text("order.upName", { name })}
                  onClick={(e) => {
                    move(id, index - 1);
                    e.currentTarget.closest("li")?.focus();
                  }}
                >
                  {text("order.up")}
                </Button>
                <Button
                  type="button"
                  disabled={!movable || index === ids.length - 1}
                  aria-label={text("order.downName", { name })}
                  onClick={(e) => {
                    move(id, index + 1);
                    e.currentTarget.closest("li")?.focus();
                  }}
                >
                  {text("order.down")}
                </Button>
              </div>
            </li>
          );
        })}
      </ol>
      {!ids.length && <p>{text("order.empty")}</p>}
      {!movable && !draft.submitted && <p>{text("order.membershipChanged")}</p>}
      <p role="status" aria-live="polite">
        {currentAnnouncement}
      </p>
      <Button
        type="button"
        disabled={draft.submitted}
        onClick={() => {
          drag.current = null;
          controller.cancelOrder(draft.property);
          announce(text("order.cancelled"));
        }}
      >
        {text("order.cancel")}
      </Button>
    </div>
  );
}

export function ArchiveInput({
  controller,
  draft,
}: {
  controller: TemplateController;
  draft: FieldDraft;
}) {
  const edit = draft.edit;
  if (edit.kind !== "archive_field" && edit.kind !== "archive_option")
    return null;
  const field = draft.source.content.fields.find((f) => f.id === edit.field);
  if (!field) return null;
  const required =
    edit.kind === "archive_option" && referencesOption(field, edit.option);
  const id = `archive-${draft.property}`;
  return (
    <div className="archive-input">
      <p>
        {text(
          edit.kind === "archive_field"
            ? "archive.fieldImpact"
            : "archive.optionImpact",
        )}
      </p>
      <p className="literal">
        {edit.kind === "archive_field"
          ? field.label
          : field.options.find((o) => o.id === edit.option)?.label ||
            text("field.emptyLabel")}
      </p>
      <strong>{text("archive.storedDefault")}</strong>
      <div>
        <ValueRead value={field.default} options={field.options} />
      </div>
      {edit.kind === "archive_option" && (
        <>
          <p>
            {text(required ? "archive.repairRequired" : "archive.noRepair")}
          </p>
          {required ? (
            edit.repair === null ? (
              <div className="actions">
                <Button
                  type="button"
                  onClick={() =>
                    controller.setField(draft.property, {
                      ...edit,
                      repair: emptyValue(fieldKind(field)),
                    })
                  }
                >
                  {text("archive.replace")}
                </Button>
                <Button
                  type="button"
                  onClick={() =>
                    controller.setField(draft.property, {
                      ...edit,
                      repair: { kind: "unset" },
                    })
                  }
                >
                  {text("archive.unset")}
                </Button>
              </div>
            ) : (
              <ValueInput
                id={id}
                kind={fieldKind(field)}
                value={edit.repair}
                keep={false}
                options={field.options.filter((o) => o.id !== edit.option)}
                onChange={(repair) =>
                  controller.setField(draft.property, { ...edit, repair })
                }
              />
            )
          ) : (
            edit.repair !== null && (
              <>
                <p>{text("archive.unnecessaryRepair")}</p>
                <ValueRead value={edit.repair} options={field.options} />
                <Button
                  type="button"
                  onClick={() =>
                    controller.setField(draft.property, {
                      ...edit,
                      repair: null,
                    })
                  }
                >
                  {text("archive.clearRepair")}
                </Button>
              </>
            )
          )}
        </>
      )}
      <Checkbox
        label={text("archive.confirm")}
        checked={draft.confirmation ?? false}
        onChange={(e) =>
          controller.confirmArchive(draft.property, e.target.checked)
        }
      />
    </div>
  );
}
