import { Tooltip } from "@fluentui/react-components";
import {
  ChevronDown16Regular,
  ChevronRight16Regular,
} from "@fluentui/react-icons";
import { useEffect, useMemo, useState } from "react";
import type { DocumentList } from "../bridge/documents";
import type { TemplateSummary } from "../bridge/types";
import { Button, Select } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";

const PAGE_SIZE = 100;

function parentPath(
  list: DocumentList,
  byId: ReadonlyMap<string, DocumentList["documents"][number]>,
  id: string,
) {
  const names: string[] = [];
  const visited = new Set<string>([id]);
  let parent = list.layout.nodes[id]?.parentId ?? null;
  while (parent && !visited.has(parent)) {
    visited.add(parent);
    names.unshift(byId.get(parent)?.name ?? parent);
    parent = list.layout.nodes[parent]?.parentId ?? null;
  }
  return names;
}

export function DocumentGlossary({
  list,
  templates,
  template,
  selectTemplate,
  open,
  filterOnly = false,
  listOnly = false,
}: {
  list: DocumentList | null;
  templates: TemplateSummary[];
  template: string | null;
  selectTemplate: (template: string | null) => void;
  open: (id: string) => void;
  filterOnly?: boolean;
  listOnly?: boolean;
}) {
  const [limit, setLimit] = useState(PAGE_SIZE);
  const [collapsedTemplates, setCollapsedTemplates] = useState<
    ReadonlySet<string>
  >(() => new Set());
  const visibleTemplates = useMemo(
    () =>
      templates
        .filter((row) => row.lifecycle === "Active" && !row.glossaryExcluded)
        .sort(
          (left, right) =>
            left.name.localeCompare(right.name, "ko") ||
            left.id.localeCompare(right.id),
        ),
    [templates],
  );
  const byTemplate = useMemo(
    () => new Map(visibleTemplates.map((row) => [row.id, row])),
    [visibleTemplates],
  );
  const byDocument = useMemo(
    () =>
      new Map(
        (list?.documents ?? []).map((document) => [document.id, document]),
      ),
    [list],
  );
  useEffect(() => {
    if (template && !byTemplate.has(template)) selectTemplate(null);
  }, [byTemplate, selectTemplate, template]);
  const categories = useMemo(() => {
    if (!list || list.problem) return [];
    const names = new Map<string, number>();
    const grouped = new Map<
      string,
      { document: DocumentList["documents"][number]; path: string[] }[]
    >();
    for (const document of list.documents) {
      if (
        list.layout.nodes[document.id]?.state !== "active" ||
        document.glossaryExcluded ||
        !byTemplate.has(document.template) ||
        (template && document.template !== template)
      )
        continue;
      names.set(document.name, (names.get(document.name) ?? 0) + 1);
      const rows = grouped.get(document.template) ?? [];
      rows.push({
        document,
        path: parentPath(list, byDocument, document.id),
      });
      grouped.set(document.template, rows);
    }
    return visibleTemplates
      .filter((row) => !template || row.id === template)
      .map((category) => {
        const rows = grouped.get(category.id) ?? [];
        rows.sort(
          (left, right) =>
            left.document.name.localeCompare(right.document.name, "ko") ||
            left.path
              .join("\u0000")
              .localeCompare(right.path.join("\u0000"), "ko") ||
            left.document.id.localeCompare(right.document.id),
        );
        return {
          template: category,
          entries: rows.map((entry) => ({
            ...entry,
            duplicate: (names.get(entry.document.name) ?? 0) > 1,
          })),
        };
      })
      .filter((category) => category.entries.length > 0);
  }, [byDocument, byTemplate, list, template, visibleTemplates]);
  const entryCount = categories.reduce(
    (count, category) => count + category.entries.length,
    0,
  );
  const { visibleCategories, hasMoreEntries } = useMemo(() => {
    const visible: (typeof categories)[number][] = [];
    let remaining = limit;
    let hasMore = false;
    for (const category of categories) {
      if (collapsedTemplates.has(category.template.id)) {
        visible.push({ ...category, entries: [] });
        continue;
      }
      if (remaining <= 0) {
        hasMore = true;
        continue;
      }
      const entries = category.entries.slice(0, remaining);
      visible.push({ ...category, entries });
      remaining -= entries.length;
      if (entries.length < category.entries.length) hasMore = true;
    }
    return { visibleCategories: visible, hasMoreEntries: hasMore };
  }, [categories, collapsedTemplates, limit]);
  const toggleCategory = (id: string) =>
    setCollapsedTemplates((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  return (
    <div
      className={`document-glossary-content${filterOnly ? " glossary-filter-only" : ""}${listOnly ? " glossary-list-only" : ""}`}
    >
      {!listOnly && (
        <Select
          aria-label={text("glossary.templateFilter")}
          value={template ?? ""}
          onChange={(event) => selectTemplate(event.target.value || null)}
        >
          <option value="">{text("glossary.allTemplates")}</option>
          {visibleTemplates.map((row) => (
            <option key={row.id} value={row.id}>
              {row.name}
            </option>
          ))}
        </Select>
      )}
      {!filterOnly &&
        (list?.problem ? (
          <InlineNotice kind="error">
            {text("glossary.unavailable")}
          </InlineNotice>
        ) : entryCount ? (
          <>
            <p className="glossary-count">
              {text("glossary.count", { count: String(entryCount) })}
            </p>
            <div className="glossary-table-scroll">
              <table className="glossary-table">
                <colgroup>
                  <col className="glossary-name-column" />
                  <col className="glossary-english-column" />
                  <col className="glossary-summary-column" />
                </colgroup>
                <thead>
                  <tr>
                    <th scope="col">{text("glossary.documentName")}</th>
                    <th scope="col">{text("glossary.englishName")}</th>
                    <th scope="col">{text("glossary.summary")}</th>
                  </tr>
                </thead>
                {visibleCategories.map((category) => (
                  <tbody
                    key={category.template.id}
                    aria-label={category.template.name}
                  >
                    <tr className="glossary-category">
                      <th scope="rowgroup" colSpan={3}>
                        <Button
                          type="button"
                          appearance="transparent"
                          className="glossary-category-toggle"
                          aria-expanded={
                            !collapsedTemplates.has(category.template.id)
                          }
                          aria-label={text(
                            collapsedTemplates.has(category.template.id)
                              ? "glossary.expandCategory"
                              : "glossary.collapseCategory",
                            { name: category.template.name },
                          )}
                          icon={
                            collapsedTemplates.has(category.template.id) ? (
                              <ChevronRight16Regular />
                            ) : (
                              <ChevronDown16Regular />
                            )
                          }
                          onClick={() => toggleCategory(category.template.id)}
                        >
                          {category.template.name}
                        </Button>
                      </th>
                    </tr>
                    {category.entries.map(({ document, path, duplicate }) => {
                      const pathLabel = path.join(" · ");
                      const detail = [category.template.name, pathLabel]
                        .filter(Boolean)
                        .join(" · ");
                      return (
                        <tr key={document.id}>
                          <td>
                            <Tooltip
                              content={[
                                document.name,
                                document.englishName,
                                document.glossarySummary,
                                detail,
                                document.id,
                              ]
                                .filter(Boolean)
                                .join(" · ")}
                              relationship="description"
                            >
                              <Button
                                type="button"
                                appearance="transparent"
                                className="glossary-document-button"
                                onClick={() => open(document.id)}
                              >
                                <span className="glossary-cell-text">
                                  {document.name}
                                </span>
                                {duplicate && pathLabel && (
                                  <small className="glossary-entry-detail">
                                    {pathLabel}
                                  </small>
                                )}
                              </Button>
                            </Tooltip>
                          </td>
                          <td title={document.englishName || undefined}>
                            <span className="glossary-cell-text">
                              {document.englishName ?? ""}
                            </span>
                          </td>
                          <td title={document.glossarySummary || undefined}>
                            <span className="glossary-cell-text">
                              {document.glossarySummary ?? ""}
                            </span>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                ))}
              </table>
            </div>
            {hasMoreEntries && (
              <Button type="button" onClick={() => setLimit(limit + PAGE_SIZE)}>
                {text("glossary.more")}
              </Button>
            )}
          </>
        ) : (
          <p>{text("glossary.empty")}</p>
        ))}
    </div>
  );
}
