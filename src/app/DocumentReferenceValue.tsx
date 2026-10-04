import { useMemo, useState } from "react";
import {
  Checkbox,
  Combobox,
  Link,
  Option,
  Tooltip,
} from "@fluentui/react-components";
import { Dismiss12Regular } from "@fluentui/react-icons";
import { Button, Input } from "../ui/Controls";
import type { DocumentList } from "../bridge/documents";
import type { Field, TemplateSummary, Value } from "../bridge/types";
import { text } from "../strings";

export interface ReferenceContext {
  list: DocumentList | null;
  templates: TemplateSummary[];
  index?: ReadonlyMap<string, ReferenceDisplay>;
  currentDocument?: string;
  preview?: boolean;
  open: (document: string) => void;
}

type ReferenceDisplay = { name: string; detail: string; active: boolean };

const missingDisplay = (): ReferenceDisplay => ({
  name: text("reference.missing"),
  detail: text("reference.missingHelp"),
  active: false,
});

/** One index per document-list generation, shared by read rows and candidate search. */
export function buildReferenceIndex(
  list: DocumentList | null,
  templates: readonly TemplateSummary[],
): ReadonlyMap<string, ReferenceDisplay> {
  const byId = new Map((list?.documents ?? []).map((row) => [row.id, row]));
  const templateNames = new Map(templates.map((row) => [row.id, row.name]));
  const result = new Map<string, ReferenceDisplay>();
  for (const summary of list?.documents ?? []) {
    const node = list?.layout.nodes[summary.id];
    const parents: string[] = [];
    let parent = node?.parentId ?? null;
    while (parent && parents.length < 32) {
      const row = byId.get(parent);
      if (row) parents.unshift(row.name);
      parent = list?.layout.nodes[parent]?.parentId ?? null;
    }
    result.set(summary.id, {
      name: summary.name,
      detail: [
        templateNames.get(summary.template) ??
          text("reference.missingTemplate"),
        parents.length ? parents.join(" / ") : text("documents.root"),
        node?.state === "trashed" ? text("reference.trashed") : "",
      ]
        .filter(Boolean)
        .join(" · "),
      active: node?.state !== "trashed",
    });
  }
  return result;
}

function ReferenceRows({
  value,
  context,
  disabled,
  change,
}: {
  value: Extract<Value, { kind: "relation" | "document_link" }>;
  context: ReferenceContext;
  disabled?: boolean;
  change?: (
    value: Extract<Value, { kind: "relation" | "document_link" }>,
  ) => void;
}) {
  const index = useMemo(
    () => context.index ?? buildReferenceIndex(context.list, context.templates),
    [context.index, context.list, context.templates],
  );
  const documents =
    value.kind === "relation"
      ? value.links.map((link) => link.document)
      : value.documents;
  return (
    <ul className="reference-values">
      {documents.map((document, position) => {
        const shown = index.get(document) ?? missingDisplay();
        const tooltip = [shown.name, shown.detail].filter(Boolean).join(" · ");
        if (value.kind === "relation") {
          const relation = value.links[position];
          return (
            <li
              className={`relation-reference-value${change ? " is-editing" : ""}`}
              key={relation.id}
            >
              <div className="relation-entry-heading">
                <div className="relation-tag">
                  <Tooltip content={tooltip} relationship="description">
                    <Button
                      type="button"
                      appearance="transparent"
                      className="relation-tag-name"
                      disabled={!shown.active || context.preview}
                      onClick={() => context.open(document)}
                    >
                      {shown.name}
                    </Button>
                  </Tooltip>
                  {relation.name && (
                    <span className="relation-tag-role">
                      <span aria-hidden="true">–</span> {relation.name}
                    </span>
                  )}
                  {(change || context.preview) && (
                    <span className="relation-tag-direction">
                      {relation.oneWay ? "단방향 →" : "양방향 ↔"}
                    </span>
                  )}
                </div>
              </div>
              {change && (
                <div className="relation-edit-controls">
                  <label
                    className="relation-name-control"
                    htmlFor={`relation-name-${relation.id}`}
                  >
                    <span>{text("reference.relationName")}</span>
                    <Input
                      id={`relation-name-${relation.id}`}
                      className="relation-name-input"
                      aria-label={`${shown.name}: ${text("reference.relationName")}`}
                      aria-description={text("reference.relationNameHelp")}
                      placeholder={text("reference.relationName")}
                      disabled={disabled}
                      value={relation.name}
                      onKeyDown={(event) => {
                        if (
                          ["Enter", "Escape"].includes(event.key) ||
                          (event.nativeEvent.isComposing &&
                            event.key === "Backspace")
                        )
                          event.stopPropagation();
                      }}
                      onChange={(event) =>
                        change({
                          kind: "relation",
                          links: value.links.map((link) =>
                            link.id === relation.id
                              ? { ...link, name: event.target.value }
                              : link,
                          ),
                        })
                      }
                    />
                  </label>
                  <div className="relation-edit-actions">
                    <Checkbox
                      label={text("reference.oneWay")}
                      disabled={disabled}
                      checked={relation.oneWay}
                      onChange={(_, data) =>
                        change({
                          kind: "relation",
                          links: value.links.map((link) =>
                            link.id === relation.id
                              ? { ...link, oneWay: data.checked === true }
                              : link,
                          ),
                        })
                      }
                    />
                    <Button
                      type="button"
                      appearance="subtle"
                      className="reference-remove"
                      icon={<Dismiss12Regular />}
                      aria-label={`${shown.name}: ${text("reference.remove")}`}
                      disabled={disabled}
                      onClick={() =>
                        change({
                          kind: "relation",
                          links: value.links.filter(
                            (link) => link.id !== relation.id,
                          ),
                        })
                      }
                    >
                      <span className="sr-only">
                        {text("reference.remove")}
                      </span>
                    </Button>
                  </div>
                </div>
              )}
            </li>
          );
        }
        return (
          <li className="document-link-value" key={document}>
            <Tooltip content={tooltip} relationship="description">
              <Link
                as="button"
                type="button"
                className="reference-link"
                disabled={!shown.active || context.preview}
                onClick={() => context.open(document)}
              >
                {shown.name}
              </Link>
            </Tooltip>
            {change && (
              <Button
                type="button"
                appearance="subtle"
                className="reference-remove"
                icon={<Dismiss12Regular />}
                aria-label={`${shown.name}: ${text("reference.remove")}`}
                disabled={disabled}
                onClick={() =>
                  change({
                    kind: "document_link",
                    documents: value.documents.filter((id) => id !== document),
                  })
                }
              >
                <span className="sr-only">{text("reference.remove")}</span>
              </Button>
            )}
          </li>
        );
      })}
      {!documents.length && (
        <li className="reference-empty">{text("field.unsetValue")}</li>
      )}
    </ul>
  );
}

export function ReferenceRead({
  value,
  context,
}: {
  value: Extract<Value, { kind: "relation" | "document_link" }>;
  context: ReferenceContext;
}) {
  return <ReferenceRows value={value} context={context} />;
}

export function ReferenceEditor({
  id,
  value,
  field,
  context,
  disabled,
  change,
}: {
  id: string;
  value: Extract<Value, { kind: "relation" | "document_link" }>;
  field: Field;
  context: ReferenceContext;
  disabled: boolean;
  change: (
    value: Extract<Value, { kind: "relation" | "document_link" }>,
  ) => void;
}) {
  const [query, setQuery] = useState("");
  const [composing, setComposing] = useState(false);
  const index = useMemo(
    () => context.index ?? buildReferenceIndex(context.list, context.templates),
    [context.index, context.list, context.templates],
  );
  const candidates = useMemo(() => {
    const used = new Set(
      value.kind === "relation"
        ? value.links.map((link) => link.document)
        : value.documents,
    );
    const needle = query.trim().toLocaleLowerCase();
    return (context.list?.documents ?? [])
      .filter((row) => context.list?.layout.nodes[row.id]?.state !== "trashed")
      .filter((row) => !used.has(row.id))
      .filter(
        (row) =>
          value.kind !== "relation" || row.id !== context.currentDocument,
      )
      .filter(
        (row) =>
          value.kind !== "relation" ||
          !field.allowedTemplates?.length ||
          field.allowedTemplates.includes(row.template),
      )
      .filter((row) => {
        if (!needle) return true;
        const shown = index.get(row.id) ?? missingDisplay();
        return (
          !needle ||
          shown.name.toLocaleLowerCase().includes(needle) ||
          shown.detail.toLocaleLowerCase().includes(needle)
        );
      })
      .sort((left, right) =>
        left.name.localeCompare(right.name, "ko", { sensitivity: "base" }),
      );
  }, [
    context.list,
    context.currentDocument,
    field.allowedTemplates,
    index,
    query,
    value,
  ]);
  const selectionFull =
    value.kind === "relation" &&
    field.multiple === false &&
    value.links.length > 0;
  const add = (document: string) => {
    if (!document || disabled || selectionFull) return;
    change(
      value.kind === "relation"
        ? {
            kind: "relation",
            links: [
              ...value.links,
              {
                id: crypto.randomUUID(),
                document,
                oneWay: false,
                name: "",
              },
            ],
          }
        : {
            kind: "document_link",
            documents: [...value.documents, document],
          },
    );
    setQuery("");
  };
  return (
    <div className="reference-editor" data-reference-field={field.id}>
      <ReferenceRows
        value={value}
        context={context}
        disabled={disabled}
        change={change}
      />
      <Combobox
        id={id}
        value={query}
        disabled={disabled || selectionFull}
        aria-label={text("reference.search")}
        placeholder={text("reference.search")}
        freeform
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !event.nativeEvent.isComposing) {
            event.stopPropagation();
            setQuery("");
          }
        }}
        onChange={(event) => {
          setQuery(event.target.value);
        }}
        onOptionSelect={(_, data) => {
          if (!composing && data.optionValue) add(data.optionValue);
        }}
      >
        {candidates.map((row) => {
          const shown = index.get(row.id) ?? missingDisplay();
          const label = [shown.name, shown.detail].filter(Boolean).join(" · ");
          return (
            <Tooltip key={row.id} content={label} relationship="description">
              <Option value={row.id} text={label} aria-label={label}>
                {shown.name}
              </Option>
            </Tooltip>
          );
        })}
      </Combobox>
      {!candidates.length && <small>{text("reference.noCandidates")}</small>}
    </div>
  );
}
