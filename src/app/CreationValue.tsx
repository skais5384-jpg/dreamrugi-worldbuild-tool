import { GroupEditor } from "./GroupValue";
import { groupDraft, type GroupContext } from "./groups";
import { AssetList, UrlRead } from "./MediaValue";
import { Field as FluentField } from "@fluentui/react-components";
import { Checkbox, Input, Select } from "../ui/Controls";
import { HelpText } from "../ui/HelpText";
import type { Field, Value } from "../bridge/types";
import type { Intent } from "../bridge/workspace";
import { RichEditor } from "./rich/RichEditor";
import { kinds } from "./fieldEditing";
import { text } from "../strings";
import {
  ReferenceEditor,
  type ReferenceContext,
} from "./DocumentReferenceValue";

export function plain(value: string): Value {
  return {
    kind: "rich_text",
    content: {
      kind: "root",
      children: value.split("\n").map((line) => ({
        kind: "paragraph",
        children: line ? [{ kind: "text", text: line }] : [],
      })),
    },
  };
}
/** 가이드는 표시만 한다. 복구된 값은 사용자가 바꾸기 전까지 원시 intent 그대로 둔다. */
export function CreationValue({
  field,
  intent,
  change,
  invalid,
  disabled,
  prefix = "creation-",
  importAsset,
  group,
  reference,
}: {
  field: Field;
  intent: Intent<Value>;
  change: (i: Intent<Value>) => void;
  invalid: boolean;
  disabled: boolean;
  prefix?: string;
  importAsset?: () => Promise<string | null>;
  group?: GroupContext;
  reference?: ReferenceContext;
}) {
  if (field.kind === "Group" && group)
    return (
      <GroupEditor
        field={field}
        value={
          intent.intent === "set" && intent.value.kind === "group"
            ? intent.value
            : groupDraft(group.baseline)
        }
        change={(value) => change({ intent: "set", value })}
        context={group}
        disabled={disabled}
        prefix={prefix}
      />
    );
  const kind = kinds[field.kind as keyof typeof kinds];
  const id = prefix + field.id;
  const value = intent.intent === "set" ? intent.value : undefined;
  const set = (v: Value) => change({ intent: "set", value: v });
  const guide = field.writingGuide || undefined;
  // 입력칸 안에 표시한 가이드를 아래에 반복하지 않는다. 선택/복합 값만 아래 안내를 쓴다.
  const guideBelow =
    kind === "single_choice" ||
    kind === "multi_choice" ||
    kind === "rich_text" ||
    kind === "image" ||
    kind === "file";
  const help = invalid
    ? text(
        field.required ? "documents.requiredValue" : "documents.invalidValue",
      )
    : undefined;
  if (kind === "relation" || kind === "document_link") {
    if (!reference) return <p>{text("documents.unknown")}</p>;
    const current =
      value?.kind === kind
        ? value
        : kind === "relation"
          ? ({ kind, links: [] } satisfies Extract<Value, { kind: "relation" }>)
          : ({ kind, documents: [] } satisfies Extract<
              Value,
              { kind: "document_link" }
            >);
    return (
      <FluentField
        validationState={invalid ? "error" : "none"}
        validationMessage={help && { id: id + "-error", children: help }}
      >
        <ReferenceEditor
          id={id}
          value={current}
          field={field}
          context={reference}
          disabled={disabled}
          change={(next) => {
            const empty =
              next.kind === "relation"
                ? next.links.length === 0
                : next.documents.length === 0;
            change(
              empty ? { intent: "unset" } : { intent: "set", value: next },
            );
          }}
        />
        {guide && <HelpText id={id + "-guide"}>{guide}</HelpText>}
      </FluentField>
    );
  }
  const shared = {
    id,
    disabled,
    "aria-label": field.label,
    "aria-invalid": invalid,
    "aria-description": guide && !guideBelow ? guide : undefined,
    "aria-describedby":
      [guide && guideBelow && id + "-guide", invalid && id + "-error"]
        .filter(Boolean)
        .join(" ") || undefined,
  };
  if (!kind) return <p>{text("documents.unknown")}</p>;
  return (
    <FluentField
      validationState={invalid ? "error" : "none"}
      validationMessage={help && { id: id + "-error", children: help }}
    >
      {kind === "image" || kind === "file" ? (
        <AssetList
          id={id}
          label={field.label}
          invalid={invalid}
          describedBy={shared["aria-describedby"]}
          image={kind === "image"}
          ids={value?.kind === kind ? value.value : []}
          disabled={disabled}
          importAsset={importAsset}
          change={(ids) =>
            change(
              ids.length
                ? { intent: "set", value: { kind, value: ids } }
                : { intent: "unset" },
            )
          }
        />
      ) : kind === "url" ? (
        <>
          <Input
            {...shared}
            type="url"
            value={value?.kind === "url" ? value.value : ""}
            placeholder={guide || text("media.urlHelp")}
            onChange={(e) => set({ kind: "url", value: e.target.value })}
          />
          {value?.kind === "url" && value.value && (
            <UrlRead value={value.value} />
          )}
        </>
      ) : kind === "rich_text" ? (
        <RichEditor
          id={id}
          label={field.label}
          disabled={disabled}
          invalid={invalid}
          guide={guide}
          value={
            value?.kind === "rich_text"
              ? value.content
              : { kind: "root", children: [] }
          }
          change={(content) => set({ kind: "rich_text", content })}
        />
      ) : kind === "number" ? (
        <>
          <Input
            {...shared}
            disabled={disabled || value?.kind === "number_unknown"}
            type="text"
            placeholder={guide}
            value={
              value?.kind === "number"
                ? value.value
                : value?.kind === "number_unknown"
                  ? (value.previous_raw ?? "")
                  : ""
            }
            onChange={(e) => set({ kind: "number", value: e.target.value })}
          />
          {(field.minimum || field.maximum) && (
            <p>
              {text("field.minimum")}: {field.minimum || "—"} ·{" "}
              {text("field.maximum")}: {field.maximum || "—"}
            </p>
          )}
          <Checkbox
            label={text("field.numberUnknown")}
            disabled={disabled}
            checked={value?.kind === "number_unknown"}
            onChange={(_, data) =>
              set(
                data.checked === true
                  ? {
                      kind: "number_unknown",
                      previous_raw: value?.kind === "number" ? value.value : "",
                    }
                  : {
                      kind: "number",
                      value:
                        value?.kind === "number_unknown"
                          ? (value.previous_raw ?? "")
                          : "",
                    },
              )
            }
          />
        </>
      ) : kind === "single_choice" ? (
        <Select
          {...shared}
          value={value?.kind === kind ? value.option : ""}
          onChange={(e) => set({ kind, option: e.target.value })}
        >
          <option value="">{text("documents.noSelection")}</option>
          {value?.kind === "single_choice" &&
            value.option &&
            !field.options.some((o) => o.id === value.option) && (
              <option value={value.option}>{value.option}</option>
            )}
          {field.options
            .filter(
              (o) =>
                o.lifecycle === "Active" ||
                (value?.kind === "single_choice" && value.option === o.id),
            )
            .map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
        </Select>
      ) : kind === "multi_choice" ? (
        <Select
          {...shared}
          multiple
          value={value?.kind === kind ? value.options : []}
          onChange={(e) =>
            set({
              kind,
              options: Array.from(e.target.selectedOptions, (o) => o.value),
            })
          }
        >
          {value?.kind === "multi_choice" &&
            value.options
              .filter((id) => !field.options.some((o) => o.id === id))
              .map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
          {field.options
            .filter(
              (o) =>
                o.lifecycle === "Active" ||
                (value?.kind === "multi_choice" &&
                  value.options.includes(o.id)),
            )
            .map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
        </Select>
      ) : (
        <Input
          {...shared}
          type="text"
          placeholder={guide}
          value={
            value?.kind === "duration"
              ? value.milliseconds
              : value && "value" in value && typeof value.value === "string"
                ? value.value
                : ""
          }
          onChange={(e) =>
            set(
              kind === "duration"
                ? { kind, milliseconds: e.target.value }
                : { kind, value: e.target.value },
            )
          }
        />
      )}
      {guide && guideBelow && <HelpText id={id + "-guide"}>{guide}</HelpText>}
    </FluentField>
  );
}
