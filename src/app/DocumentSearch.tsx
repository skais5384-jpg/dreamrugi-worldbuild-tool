import { MessageBar, MessageBarBody } from "@fluentui/react-components";
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
        <MessageBar intent="error" layout="multiline">
          <MessageBarBody>
            {text("documents.searchFailed")} {state.searchError}
          </MessageBarBody>
        </MessageBar>
      )}
      {state.searchBusy && !state.search && (
        <p role="status">{text("documents.searching")}</p>
      )}
      {state.search && (
        <>
          {state.search.missingDocuments > 0 && (
            <MessageBar intent="warning" layout="multiline">
              <MessageBarBody>
                {text("documents.searchMissing", {
                  count: String(state.search.missingDocuments),
                })}
              </MessageBarBody>
            </MessageBar>
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
          <ul>
            {state.search.results.map((result) => (
              <li key={result.id}>
                <Button
                  type="button"
                  appearance="subtle"
                  className="document-search-result-link"
                  onClick={() => openDocument(result.id)}
                >
                  <strong>{result.name}</strong>
                  <span>
                    {[result.templateName, ...result.path].join(" · ")}
                  </span>
                  {result.excerpt && (
                    <span className="document-search-excerpt">
                      {result.excerpt.label}: {result.excerpt.text}
                    </span>
                  )}
                </Button>
              </li>
            ))}
          </ul>
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
