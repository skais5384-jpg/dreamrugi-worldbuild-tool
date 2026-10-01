import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";

import { Search20Regular } from "@fluentui/react-icons";
import type { TemplateSummary } from "../bridge/types";
import { text } from "../strings";
import { Button, Input, Select } from "../ui/Controls";
import type { DocumentController } from "./documentController";

type SearchState = Pick<
  ReturnType<DocumentController["snapshot"]>,
  "searchQuery" | "searchTemplate" | "search" | "searchBusy" | "searchError"
>;

export function DocumentSearchControls({
  controller,
  state,
  templates,
}: {
  controller: DocumentController;
  state: SearchState;
  templates: TemplateSummary[];
}) {
  const active = !!(state.searchQuery.trim() || state.searchTemplate);
  return (
    <section
      className="document-search-controls"
      aria-label={text("documents.search")}
      onKeyDown={(event) => {
        if (event.key === "Escape" && active) {
          event.preventDefault();
          controller.clearSearch();
        }
      }}
    >
      <Input
        value={state.searchQuery}
        aria-label={text("documents.searchInput")}
        placeholder={text("documents.searchPlaceholder")}
        contentBefore={<Search20Regular />}
        onChange={(_, data) =>
          controller.searchDocuments(data.value, state.searchTemplate)
        }
      />
      <Select
        value={state.searchTemplate ?? ""}
        aria-label={text("documents.searchTemplate")}
        onChange={(event) =>
          controller.searchDocuments(
            state.searchQuery,
            event.currentTarget.value || null,
          )
        }
      >
        <option value="">{text("documents.searchAllTemplates")}</option>
        {templates
          .filter((row) => row.lifecycle === "Active")
          .map((row) => (
            <option key={row.id} value={row.id}>
              {state.search?.templateNames[row.id] ?? row.name}
            </option>
          ))}
      </Select>
      <div className="document-search-actions">
        <span>{text("documents.searchScope")}</span>
        {active && (
          <>
            <Button
              type="button"
              appearance="subtle"
              disabled={state.searchBusy}
              onClick={() => void controller.refreshSearch(true)}
            >
              {text("documents.searchRefresh")}
            </Button>
            <Button
              type="button"
              appearance="subtle"
              onClick={() => controller.clearSearch()}
            >
              {text("documents.searchClear")}
            </Button>
          </>
        )}
      </div>
    </section>
  );
}

export function DocumentSearchResults({
  controller,
  state,
  openDocument,
}: {
  controller: DocumentController;
  state: SearchState;
  openDocument: (id: string) => void;
}) {
  return (
    <section className="document-search-results">
      {state.searchError && (
        <FloatingNotice intent="error">
          <FloatingNoticeContent>
            {text("documents.searchFailed")} {state.searchError}
          </FloatingNoticeContent>
        </FloatingNotice>
      )}
      {state.searchBusy && !state.search && (
        <p role="status">{text("documents.searching")}</p>
      )}
      {state.search && (
        <>
          {state.search.missingDocuments > 0 && (
            <FloatingNotice intent="warning">
              <FloatingNoticeContent>
                {text("documents.searchMissing", {
                  count: String(state.search.missingDocuments),
                })}
              </FloatingNoticeContent>
            </FloatingNotice>
          )}
          <div className="document-search-summary">
            <p className="document-search-count" role="status">
              {text("documents.searchResults")} {state.search.total}
            </p>
            {state.search.hasMore && (
              <Button
                type="button"
                disabled={state.searchBusy}
                onClick={() => void controller.moreSearchResults()}
              >
                {text("documents.searchMore")}
              </Button>
            )}
          </div>
          <div className="search-results-scroll">
            <table className="management-table">
              <thead>
                <tr>
                  <th scope="col">문서</th>
                  <th scope="col">유형 / 위치</th>
                  <th scope="col">일치한 내용</th>
                </tr>
              </thead>
              <tbody>
                {state.search.results.map((result) => (
                  <tr key={result.id}>
                    <td>
                      <Button
                        type="button"
                        appearance="subtle"
                        className="search-document-command"
                        aria-describedby={`search-type-${result.id} search-excerpt-${result.id}`}
                        onClick={() => openDocument(result.id)}
                      >
                        {result.name}
                      </Button>
                    </td>
                    <td id={`search-type-${result.id}`}>
                      {[result.templateName, ...result.path].join(" · ")}
                    </td>
                    <td id={`search-excerpt-${result.id}`}>
                      {result.excerpt && (
                        <>
                          <span className="search-excerpt-label">
                            {result.excerpt.label}:
                          </span>{" "}
                          {result.excerpt.text}
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {!state.search.total && !state.searchBusy && (
            <p>{text("documents.searchEmpty")}</p>
          )}
          {state.searchBusy && (
            <p role="status">{text("documents.searchUpdating")}</p>
          )}
        </>
      )}
    </section>
  );
}
