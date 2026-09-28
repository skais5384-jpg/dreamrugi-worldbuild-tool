import { GroupRead } from "./GroupValue";
import { AssetList, UrlRead } from "./MediaValue";
import { createElement, type ReactNode } from "react";
import { Label } from "@fluentui/react-components";
import { Input, Select } from "../ui/Controls";
import { HelpText } from "../ui/HelpText";
import type {
  Field,
  FieldConfiguration,
  RichNode,
  Value,
} from "../bridge/types";
import { text } from "../strings";
import { ReferenceRead, type ReferenceContext } from "./DocumentReferenceValue";

type Kind = FieldConfiguration["kind"];
type Option = Field["options"][number];
export function emptyValue(kind: Kind): Value {
  switch (kind) {
    case "image":
    case "file":
      return { kind, value: [] };
    case "duration":
      return { kind, milliseconds: "" };
    case "single_choice":
      return { kind, option: "" };
    case "multi_choice":
      return { kind, options: [] };
    case "relation":
      return { kind, links: [] };
    case "document_link":
      return { kind, documents: [] };
    case "rich_text":
      return { kind: "unset" };
    default:
      return { kind, value: "" };
  }
}
function RichRead({ node }: { node: RichNode }) {
  if (node.kind === "text") {
    let content: ReactNode = node.text;
    for (const mark of node.marks ?? []) {
      const tag = {
        bold: "strong",
        italic: "em",
        underline: "u",
        strikethrough: "s",
      }[mark];
      content = createElement(tag, null, content);
    }
    return <>{content}</>;
  }
  if (node.kind === "hardBreak") return <br />;
  const content = node.children.map((child, index) => (
    <RichRead key={index} node={child} />
  ));
  switch (node.kind) {
    case "root":
      return <div className="rich-read">{content}</div>;
    case "paragraph":
      return <p>{content}</p>;
    case "heading":
      return createElement(
        `h${Math.max(1, Math.min(6, node.level))}`,
        null,
        content,
      );
    case "blockquote":
      return <blockquote>{content}</blockquote>;
    case "bulletList":
    case "taskList":
      return <ul>{content}</ul>;
    case "orderedList":
      return <ol>{content}</ol>;
    case "listItem":
      return <li>{content}</li>;
    case "taskItem":
      return (
        <li>
          <span
            aria-label={text(
              node.checked
                ? "documents.taskChecked"
                : "documents.taskUnchecked",
            )}
          >
            {node.checked ? "☑" : "☐"}
          </span>
          {content}
        </li>
      );
  }
}
/** 읽기 전용 projection이다. 표시 결과를 Fresh 편집 값으로 만들지 않는다. */
export function ValueRead({
  value,
  options,
  field,
  reference,
}: {
  value: Value;
  options: Option[];
  field?: Field;
  reference?: ReferenceContext;
}) {
  switch (value.kind) {
    case "group":
      return <GroupRead value={value} field={field} reference={reference} />;
    case "image":
    case "file":
      return <AssetList ids={value.value} image={value.kind === "image"} />;
    case "url":
      return <UrlRead value={value.value} />;
    case "number_unknown":
      return <span>{text("field.numberUnknown")}</span>;
    case "unset":
      return <span>{text("field.unsetValue")}</span>;
    case "rich_text":
      return <RichRead node={value.content} />;
    case "duration":
      return <span>{value.milliseconds}</span>;
    case "single_choice":
      return (
        <span>
          {options.find((o) => o.id === value.option)?.label ?? value.option}
        </span>
      );
    case "multi_choice":
      return (
        <span>
          {value.options
            .map((id) => options.find((o) => o.id === id)?.label ?? id)
            .join(", ")}
        </span>
      );
    case "relation":
    case "document_link":
      return reference ? (
        <ReferenceRead value={value} context={reference} />
      ) : (
        <span>
          {text("reference.count", {
            count:
              value.kind === "relation"
                ? String(value.links.length)
                : String(value.documents.length),
          })}
        </span>
      );
    default:
      return <span>{value.value}</span>;
  }
}
export function ValueInput({
  id,
  kind,
  value,
  keep,
  options,
  onChange,
  showModeLabel = true,
}: {
  id: string;
  kind: Kind;
  value: Value | null;
  keep: boolean;
  options: Option[];
  onChange: (value: Value | null) => void;
  /** 상위 속성 행이 같은 label을 제공할 때 중복 표시만 생략한다. */
  showModeLabel?: boolean;
}) {
  if (value?.kind === "group") return <GroupRead value={value} />;
  const active = options.filter((o) => o.lifecycle === "Active");
  const mode =
    value === null ? "keep" : value.kind === "unset" ? "unset" : "set";
  const help =
    kind === "number"
      ? text("field.numberHelp")
      : kind === "duration"
        ? text("field.durationHelp")
        : kind === "date"
          ? text("field.dateHelp")
          : kind === "time"
            ? text("field.timeHelp")
            : kind === "single_line_text"
              ? text("field.textHelp")
              : null;
  return (
    <>
      {showModeLabel && (
        <Label htmlFor={id + "-mode"}>{text("field.default")}</Label>
      )}
      <Select
        id={id + "-mode"}
        value={mode}
        onChange={(e) =>
          onChange(
            e.target.value === "keep"
              ? null
              : e.target.value === "unset"
                ? { kind: "unset" }
                : emptyValue(kind),
          )
        }
      >
        {keep && <option value="keep">{text("field.keep")}</option>}
        {![
          "rich_text",
          "image",
          "file",
          "url",
          "relation",
          "document_link",
        ].includes(kind) && <option value="set">{text("field.set")}</option>}
        <option value="unset">{text("field.unset")}</option>
      </Select>
      {kind === "rich_text" && <p>{text("field.richUnsupported")}</p>}
      {help && <HelpText id={id + "-help"}>{help}</HelpText>}
      {value &&
        value.kind !== "unset" &&
        value.kind !== "rich_text" &&
        value.kind !== "number_unknown" &&
        value.kind !== "image" &&
        value.kind !== "relation" &&
        value.kind !== "document_link" &&
        value.kind !== "file" && (
          <>
            <Label htmlFor={id}>{text("field.value")}</Label>
            {value.kind === "single_choice" ? (
              <Select
                id={id}
                value={value.option}
                onChange={(e) =>
                  onChange({ kind: "single_choice", option: e.target.value })
                }
              >
                <option value="">{text("field.unsetValue")}</option>
                {active.map((o) => (
                  <option key={o.id} value={o.id}>
                    {o.label}
                  </option>
                ))}
              </Select>
            ) : value.kind === "multi_choice" ? (
              <Select
                id={id}
                multiple
                value={value.options}
                onChange={(e) =>
                  onChange({
                    kind: "multi_choice",
                    options: Array.from(
                      e.target.selectedOptions,
                      (o) => o.value,
                    ),
                  })
                }
              >
                {active.map((o) => (
                  <option key={o.id} value={o.id}>
                    {o.label}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                id={id}
                type="text"
                autoComplete="off"
                spellCheck={false}
                aria-describedby={help ? id + "-help" : undefined}
                value={
                  value.kind === "duration" ? value.milliseconds : value.value
                }
                onChange={(e) =>
                  onChange(
                    value.kind === "duration"
                      ? { ...value, milliseconds: e.target.value }
                      : { ...value, value: e.target.value },
                  )
                }
              />
            )}
            {(value.kind === "single_choice" ||
              value.kind === "multi_choice") &&
              !active.length && <p>{text("field.noOptions")}</p>}
          </>
        )}
    </>
  );
}
