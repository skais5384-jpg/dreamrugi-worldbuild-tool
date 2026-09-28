import { useState } from "react";
import {
  Field,
  MessageBar,
  MessageBarBody,
  Tab,
  TabList,
} from "@fluentui/react-components";
import {
  ChevronDown16Regular,
  ChevronRight16Regular,
} from "@fluentui/react-icons";
import type { TemplateSummary } from "../bridge/types";
import { text } from "../strings";
import { Button, Checkbox, Input, Select } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import type { DocumentController } from "./documentController";
import {
  DocumentSearchControls,
  DocumentSearchResults,
} from "./DocumentSearch";

export function DocumentSearchArea({
  controller,
  templates,
  openDocument,
}: {
  controller: DocumentController;
  templates: TemplateSummary[];
  openDocument: (id: string) => void;
}) {
  const state = controller.snapshot();
  const [tab, setTab] = useState<"search" | "replace">("search");
  const [scopeOpen, setScopeOpen] = useState(false);
  const form = state.replace;
  const scopes = form.scopes;
  const valid =
    !!form.find && Object.values(scopes).some(Boolean) && !form.busy;
  const scopeEntries = [
    ["title", "documents.replaceTitle"],
    ["body", "documents.replaceBody"],
    ["englishName", "documents.replaceEnglishName"],
    ["glossarySummary", "documents.replaceSummary"],
  ] as const;
  const selectedScopeCount = scopeEntries.filter(([key]) => scopes[key]).length;
  const changeScope = (key: keyof typeof scopes, checked: boolean) =>
    controller.updateReplace({ scopes: { ...scopes, [key]: checked } });

  return (
    <section
      className="document-search-area"
      aria-label={text("documents.searchArea")}
    >
      <TabList
        selectedValue={tab}
        onTabSelect={(_, data) => {
          const next = data.value === "replace" ? "replace" : "search";
          setTab(next);
          if (next === "replace") controller.startReplaceFromSearch();
        }}
      >
        <Tab value="search">{text("documents.searchTab")}</Tab>
        <Tab value="replace">{text("documents.replaceTab")}</Tab>
      </TabList>
      {tab === "search" ? (
        <>
          <DocumentSearchControls
            controller={controller}
            state={state}
            templates={templates}
          />
          <DocumentSearchResults
            controller={controller}
            state={state}
            openDocument={openDocument}
          />
        </>
      ) : (
        <section
          className="document-replace"
          aria-label={text("documents.replaceTab")}
          onKeyDown={(event) => {
            if (event.key === "Escape" && form.preview) {
              event.preventDefault();
              controller.updateReplace({});
            }
          }}
        >
          <Field label={text("documents.replaceFind")}>
            <Input
              value={form.find}
              aria-label={text("documents.replaceFind")}
              onChange={(_, data) =>
                controller.updateReplace({ find: data.value })
              }
            />
          </Field>
          <Field label={text("documents.replaceWith")}>
            <Input
              value={form.replacement}
              aria-label={text("documents.replaceWith")}
              onChange={(_, data) =>
                controller.updateReplace({ replacement: data.value })
              }
            />
          </Field>
          <Select
            value={form.template ?? ""}
            aria-label={text("documents.searchTemplate")}
            onChange={(event) =>
              controller.updateReplace({
                template: event.currentTarget.value || null,
              })
            }
          >
            <option value="">{text("documents.searchAllTemplates")}</option>
            {templates
              .filter((template) => template.lifecycle === "Active")
              .map((template) => (
                <option key={template.id} value={template.id}>
                  {template.name}
                </option>
              ))}
          </Select>
          <section className="replace-options replace-scope-options">
            <Button
              type="button"
              appearance="transparent"
              className="replace-scope-toggle"
              aria-expanded={scopeOpen}
              aria-controls="document-replace-scope-list"
              icon={
                scopeOpen ? (
                  <ChevronDown16Regular aria-hidden="true" />
                ) : (
                  <ChevronRight16Regular aria-hidden="true" />
                )
              }
              onClick={() => setScopeOpen((open) => !open)}
            >
              <span>{text("documents.replaceScope")}</span>
              <small>
                {text("documents.replaceScopeSelected", {
                  selected: String(selectedScopeCount),
                  total: String(scopeEntries.length),
                })}
              </small>
            </Button>
            {scopeOpen && (
              <div
                id="document-replace-scope-list"
                className="replace-option-list"
              >
                {scopeEntries.map(([key, label]) => (
                  <Checkbox
                    key={key}
                    checked={scopes[key]}
                    label={text(label)}
                    onChange={(_, data) =>
                      changeScope(key, data.checked === true)
                    }
                  />
                ))}
              </div>
            )}
          </section>
          <fieldset className="replace-options">
            <legend>{text("documents.replaceMatching")}</legend>
            <Checkbox
              checked={form.caseSensitive}
              label={text("documents.replaceCase")}
              onChange={(_, data) =>
                controller.updateReplace({
                  caseSensitive: data.checked === true,
                })
              }
            />
            <Checkbox
              checked={form.wholeWord}
              label={text("documents.replaceWholeWord")}
              onChange={(_, data) =>
                controller.updateReplace({ wholeWord: data.checked === true })
              }
            />
          </fieldset>
          <div className="document-replace-command">
            <div className="document-replace-primary-actions">
              <Button
                type="button"
                appearance="primary"
                disabled={!valid}
                onClick={() => void controller.previewReplace()}
              >
                {form.preview
                  ? text("documents.replaceFindAgain")
                  : text("documents.replacePreview")}
              </Button>
              {form.preview && (
                <Button
                  type="button"
                  appearance="primary"
                  disabled={
                    !form.preview.totalChanges ||
                    !!form.preview.blockers.length ||
                    form.busy
                  }
                  onClick={() => void controller.applyReplace()}
                >
                  {text("documents.replaceAll")}
                </Button>
              )}
            </div>
            {!Object.values(scopes).some(Boolean) && (
              <InlineNotice kind="error">
                {text("documents.replaceScopeRequired")}
              </InlineNotice>
            )}
          </div>
          {form.error && (
            <MessageBar intent="error" layout="multiline">
              <MessageBarBody>
                {text("documents.replaceFailed")} {form.error}
              </MessageBarBody>
            </MessageBar>
          )}
          {form.busy && (
            <div className="document-replace-command" role="status">
              <span>
                {text(
                  form.cancelling
                    ? "documents.replaceCancelling"
                    : "documents.replaceWorking",
                )}
              </span>
              <Button
                type="button"
                disabled={form.cancelling}
                onClick={() => controller.cancelReplace()}
              >
                {text("documents.cancel")}
              </Button>
            </div>
          )}
          {form.preview && (
            <section
              className="replace-preview"
              aria-label={text("documents.replaceReview")}
            >
              <div className="replace-summary" role="status">
                <strong>
                  {text("documents.replaceDocuments")}{" "}
                  {form.preview.totalDocuments}
                </strong>
                <span>
                  {text("documents.replaceLocations")}{" "}
                  {form.preview.totalChanges}
                </span>
              </div>
              {form.preview.blockers.length > 0 && (
                <MessageBar intent="error" layout="multiline">
                  <MessageBarBody>
                    <strong>{text("documents.replaceBlocked")}</strong>
                    <ul>
                      {form.preview.blockers.map((blocker, index) => (
                        <li key={`${blocker.document ?? "project"}-${index}`}>
                          {[blocker.documentName, blocker.label]
                            .filter(Boolean)
                            .join(" · ")}
                          : {blocker.reason}
                        </li>
                      ))}
                    </ul>
                  </MessageBarBody>
                </MessageBar>
              )}
              <ul className="replace-change-list">
                {form.preview.changes.map((change, index) => (
                  <li key={`${change.document}-${index}`}>
                    <Button
                      type="button"
                      appearance="subtle"
                      className="replace-document-link"
                      onClick={() => openDocument(change.document)}
                    >
                      {change.documentName}
                    </Button>
                    <small>
                      {[change.templateName, change.label, ...change.path].join(
                        " · ",
                      )}
                    </small>
                    <div className="replace-before-after">
                      <span>
                        <b>{text("documents.replaceBefore")}</b>
                        <span className="replace-change-text">
                          {change.beforePrefix}
                          <em className="replace-change-highlight">
                            {change.beforeMatch}
                          </em>
                          {change.beforeSuffix}
                        </span>
                      </span>
                      <span>
                        <b>{text("documents.replaceAfter")}</b>
                        <span className="replace-change-text">
                          {change.afterPrefix}
                          <em className="replace-change-highlight">
                            {change.afterMatch}
                          </em>
                          {change.afterSuffix}
                        </span>
                      </span>
                    </div>
                  </li>
                ))}
              </ul>
              {form.preview.hasMore && (
                <Button
                  type="button"
                  disabled={form.busy}
                  onClick={() => void controller.moreReplaceResults()}
                >
                  {text("documents.searchMore")}
                </Button>
              )}
              {!form.preview.totalChanges && (
                <p>{text("documents.replaceEmpty")}</p>
              )}
            </section>
          )}
        </section>
      )}
    </section>
  );
}
