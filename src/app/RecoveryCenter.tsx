import { recoveryText } from "./recoveryText";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { Checkbox, Button } from "../ui/Controls";
import { HelpText } from "../ui/HelpText";
import { InlineNotice } from "../ui/InlineNotice";
import type { ReapplyIntent, RecoveryContent } from "../bridge/workspace";
import type { Template } from "../bridge/types";
import type { WorkspaceController } from "./workspaceController";
import { RecoveryComparison } from "./RecoveryComparison";
import { ValueRead } from "./FieldValue";
import { text } from "../strings";
import { formatLocalDateTime } from "./localDateTime";

export function RecoveryCenter({
  controller,
}: {
  controller: WorkspaceController;
}) {
  const state = controller.snapshot();
  const documents = useSyncExternalStore(
    controller.documents.subscribe,
    controller.documents.snapshot,
  );
  const [choices, setChoices] = useState<ReapplyIntent[]>([]);
  const selected = state.selected;
  const content = state.recovery;
  const previousComparison = useRef<RecoveryContent["comparison"]>(undefined);
  const [copyState, setCopyState] = useState<string | null>(null);
  useEffect(() => {
    const previous = previousComparison.current;
    setChoices((choices) =>
      choices.filter(
        (choice) =>
          choice.kind !== "change" ||
          content?.comparison?.some(
            (item) =>
              item.id === choice.change &&
              item.status !== "blocked" &&
              (!previous ||
                previous.some(
                  (old) =>
                    old.id === item.id &&
                    JSON.stringify([
                      old.current,
                      old.original,
                      old.preserved,
                    ]) ===
                      JSON.stringify([
                        item.current,
                        item.original,
                        item.preserved,
                      ]),
                )),
          ),
      ),
    );
    previousComparison.current = content?.comparison;
  }, [content?.comparison]);
  const project = controller.shell.snapshot().project;
  const ready = project?.status === "Ready" && project.runtime === "Ready";
  return (
    <Dialog
      open
      modalType="modal"
      onOpenChange={(_, data) => {
        if (!data.open && !state.busy)
          void controller.navigate({ kind: "browse" });
      }}
    >
      <DialogSurface className="recovery-center-dialog">
        <DialogBody>
          <DialogTitle>{text("whole.center")}</DialogTitle>
          <DialogContent className="recovery-center-content">
            <section
              className="recovery-center"
              aria-label={text("whole.center")}
            >
              <div className="recovery-toolbar">
                <div className="actions">
                  <Button
                    type="button"
                    disabled={state.busy}
                    onClick={() => void controller.loadPage()}
                  >
                    {text("whole.list")}
                  </Button>
                  <Button
                    type="button"
                    disabled={state.busy || !state.page?.next}
                    onClick={() => void controller.loadPage(true)}
                  >
                    {text("whole.next")}
                  </Button>
                </div>
              </div>
              {documents.error && (
                <InlineNotice kind="error">{documents.error}</InlineNotice>
              )}
              {state.recoveryFailure && (
                <details>
                  <summary>{text("recovery.details")}</summary>
                  <p>
                    {state.recoveryFailure.category} /{" "}
                    {state.recoveryFailure.stage} /{" "}
                    {state.recoveryFailure.ioKind ?? "—"} /{" "}
                    {state.recoveryFailure.osCode ?? "—"} /{" "}
                    {state.recoveryFailure.boundaryReason ?? "—"}
                  </p>
                </details>
              )}
              {!ready && <p>{text("whole.noProjectRestore")}</p>}
              {state.page?.legacy &&
                (state.page.legacy.found > 0 ||
                  state.page.legacy.needsAttention > 0) && (
                  <InlineNotice
                    kind={state.page.legacy.needsAttention ? "error" : "info"}
                  >
                    이전 설치본의 보관 초안 {state.page.legacy.found}개 중{" "}
                    {state.page.legacy.imported}개를 확인해 인계했습니다.
                    {state.page.legacy.alreadyPresent > 0 &&
                      ` 이미 인계된 초안 ${state.page.legacy.alreadyPresent}개를 재확인했습니다.`}
                    {state.page.legacy.needsAttention > 0 &&
                      ` ${state.page.legacy.needsAttention}개는 인계되지 않았습니다. 이전 설치본을 제거하지 말고 목록을 다시 확인해 주세요.`}
                    {state.page.legacy.held.length > 0 &&
                      ` 보류 식별: ${state.page.legacy.held.join(", ")}.`}
                    {state.page.legacy.needsAttention === 0 &&
                      " 이전 보관본은 확인을 위해 그대로 남겨둡니다."}
                  </InlineNotice>
                )}
              <div className="recovery-center-layout">
                <div
                  className="recovery-list-pane"
                  role="region"
                  aria-label={text("recovery.listTitle")}
                  tabIndex={0}
                >
                  <h3>{text("recovery.listTitle")}</h3>
                  {state.page &&
                    !state.page.next &&
                    !state.page.entries.length && (
                      <p>{text("whole.noRecovery")}</p>
                    )}
                  <ul className="recovery-list">
                    {state.page?.entries.map((row) => (
                      <li key={row.row.locatorFingerprint}>
                        <div className="recovery-summary">
                          <strong>
                            {row.name ||
                              text(
                                row.row.payloadKind === "template"
                                  ? "recovery.unnamedTemplate"
                                  : "recovery.unnamedDocument",
                              )}
                          </strong>
                          <p>
                            {text(
                              row.row.error
                                ? "recovery.needsCheck"
                                : "recovery.preserved",
                            )}{" "}
                            · {formatLocalDateTime(row.createdAtUtc)}
                          </p>
                          {row.row.error && (
                            <p role="status">{text("whole.recoveryError")}</p>
                          )}
                          <details>
                            <summary>{text("recovery.details")}</summary>
                            <dl>
                              <dt>{text("whole.generation")}</dt>
                              <dd>{row.row.key?.generation}</dd>
                              <dt>{text("recovery.artifact")}</dt>
                              <dd className="identifier">{row.artifact}</dd>
                              <dt>{text("recovery.project")}</dt>
                              <dd className="identifier">
                                {row.row.key?.projectFingerprint}
                              </dd>
                              <dt>{text("recovery.payload")}</dt>
                              <dd>{row.row.payloadKind}</dd>
                              {row.row.error && (
                                <>
                                  <dt>{text("recovery.error")}</dt>
                                  <dd>
                                    {row.row.error.category} /{" "}
                                    {row.row.error.stage}
                                  </dd>
                                </>
                              )}
                            </dl>
                          </details>
                        </div>
                        <div className="actions">
                          <Button
                            type="button"
                            disabled={state.busy || !row.row.depositId}
                            onClick={() => {
                              setChoices([]);
                              void controller.inspect(row);
                            }}
                          >
                            {text("whole.inspect")}
                          </Button>
                          <Button
                            type="button"
                            disabled={state.busy || !row.row.depositId}
                            onClick={() => void controller.revalidate(row)}
                          >
                            {text("whole.revalidate")}
                          </Button>
                          <Button
                            type="button"
                            danger
                            disabled={
                              state.busy || !row.version || !row.row.key
                            }
                            onClick={() => controller.requestDiscard(row)}
                          >
                            {text("whole.discardFile")}
                          </Button>
                        </div>
                      </li>
                    ))}
                  </ul>
                </div>
                <div
                  className="recovery-detail-pane"
                  role="region"
                  aria-label={text("whole.recoveryDraft")}
                  tabIndex={0}
                >
                  <h3>{text("whole.recoveryDraft")}</h3>
                  {(!selected || !content) && (
                    <p>{text("recovery.selectToInspect")}</p>
                  )}
                  {selected && content && (
                    <section className="recovery-detail">
                      {content.attempt && (
                        <p>
                          {text("whole.generation")}{" "}
                          {content.attempt.submittedGeneration} ·{" "}
                          {content.attempt.result}
                        </p>
                      )}
                      <p>
                        {content.draft.kind === "template"
                          ? content.draft.name
                          : content.draft.kind === "document" &&
                              content.draft.name.intent === "set"
                            ? content.draft.name.value
                            : text("whole.recoveryDraft")}
                      </p>
                      <details>
                        <summary>{text("recovery.raw")}</summary>
                        <DraftRead content={content} />
                      </details>
                      <details open>
                        <summary>{text("recoveryCompare.copy")}</summary>
                        <HelpText>{text("recoveryCompare.copyHelp")}</HelpText>
                        <pre className="recovery-raw">
                          {recoveryText(content)}
                        </pre>
                        <Button
                          type="button"
                          onClick={() => {
                            void navigator.clipboard
                              .writeText(recoveryText(content))
                              .then(
                                () =>
                                  setCopyState(text("recoveryCompare.copied")),
                                () =>
                                  setCopyState(
                                    "클립보드에 복사하지 못했습니다. 표시된 내용을 직접 선택해 복사할 수 있습니다.",
                                  ),
                              );
                          }}
                        >
                          {text("recoveryCompare.copy")}
                        </Button>
                        {copyState && <p role="status">{copyState}</p>}
                      </details>
                      {content.draft.kind === "document" &&
                        content.draft.document === null && (
                          <Button
                            type="button"
                            disabled={
                              !ready ||
                              state.busy ||
                              documents.busy ||
                              !!documents.draft
                            }
                            onClick={() => {
                              const row = state.page?.entries.find(
                                (r) =>
                                  r.row.key?.draftId === selected.key.draftId &&
                                  r.row.key?.generation ===
                                    selected.key.generation &&
                                  r.row.key?.projectFingerprint ===
                                    selected.key.projectFingerprint,
                              );
                              if (row) void controller.restore();
                            }}
                          >
                            {text("documents.restoreCreation")}
                          </Button>
                        )}
                      {(content.draft.kind === "admitted_composite" ||
                        content.draft.kind === "admitted_document" ||
                        (content.draft.kind === "document" &&
                          content.draft.document !== null)) && (
                        <Button
                          type="button"
                          disabled={!ready || state.busy || documents.busy}
                          onClick={() => {
                            const row = state.page?.entries.find(
                              (r) =>
                                r.row.key?.draftId === selected.key.draftId &&
                                r.row.key?.generation ===
                                  selected.key.generation &&
                                r.row.key?.projectFingerprint ===
                                  selected.key.projectFingerprint,
                            );
                            if (row) void controller.restore();
                          }}
                        >
                          {text("documentEdit.restore")}
                        </Button>
                      )}
                      {content.draft.kind === "admitted_composite" && (
                        <div>
                          <p>{text("recoveryCompare.compositeHelp")}</p>
                          <Button
                            type="button"
                            disabled={state.busy || !ready}
                            onClick={() => {
                              setChoices([]);
                              void controller.restore([
                                { kind: "component_template" },
                              ]);
                            }}
                          >
                            {text("recoveryCompare.templatePart")}
                          </Button>
                        </div>
                      )}
                      {selected.canRestore &&
                        content.draft.kind === "template" && (
                          <div className="actions">
                            <Button
                              type="button"
                              appearance="primary"
                              disabled={state.busy || !ready}
                              onClick={() => void controller.restore()}
                            >
                              {text(
                                selected.phase === "unchecked"
                                  ? "whole.restore"
                                  : "whole.checkCurrent",
                              )}
                            </Button>
                          </div>
                        )}
                      {selected.phase !== "unchecked" &&
                        selected.phase !== "conflict" && (
                          <InlineNotice kind="error">
                            {text(
                              selected.phase === "uncertain"
                                ? "recoveryCompare.uncertain"
                                : selected.phase === "deleted"
                                  ? "whole.deletedSource"
                                  : "whole.unreadableSource",
                            )}
                          </InlineNotice>
                        )}
                      {selected.phase === "conflict" && (
                        <section>
                          <h3>{text("whole.selectIntents")}</h3>
                          <HelpText>{text("whole.reapplyHelp")}</HelpText>
                          <div className="recovery-comparison">
                            <TemplateRead
                              title={text("whole.original")}
                              template={content.original}
                            />
                            <TemplateRead
                              title={text("whole.current")}
                              template={content.current}
                            />
                          </div>
                          {content.comparison && (
                            <RecoveryComparison
                              content={content}
                              selected={choices
                                .filter((choice) => choice.kind === "change")
                                .map((choice) => choice.change)}
                              change={(ids) =>
                                setChoices(
                                  ids.map((change) => ({
                                    kind: "change",
                                    change,
                                  })),
                                )
                              }
                            />
                          )}
                          <div className="reapply-options">
                            {selected.intents
                              .filter((intent) => intent.kind !== "change")
                              .map((intent) => {
                                const key = JSON.stringify(intent);
                                const label =
                                  "field" in intent
                                    ? (content.original?.fields.find(
                                        (f) => f.id === intent.field,
                                      )?.label ?? intent.field)
                                    : "";
                                return (
                                  <Checkbox
                                    key={key}
                                    label={[label, intentLabel(intent)]
                                      .filter(Boolean)
                                      .join(" · ")}
                                    checked={choices.some(
                                      (i) => JSON.stringify(i) === key,
                                    )}
                                    onChange={(_, data) =>
                                      setChoices((items) =>
                                        data.checked === true
                                          ? [...items, intent]
                                          : items.filter(
                                              (i) => JSON.stringify(i) !== key,
                                            ),
                                      )
                                    }
                                  />
                                );
                              })}
                          </div>
                          <Button
                            type="button"
                            appearance="primary"
                            disabled={state.busy || !ready}
                            onClick={() => void controller.restore(choices)}
                          >
                            {text("whole.reapply")}
                          </Button>
                        </section>
                      )}
                    </section>
                  )}
                </div>
              </div>
            </section>
          </DialogContent>
          <DialogActions>
            <Button
              type="button"
              disabled={state.busy}
              onClick={() => void controller.navigate({ kind: "browse" })}
            >
              닫기
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
function intentLabel(intent: ReapplyIntent) {
  switch (intent.kind) {
    case "component_template":
      return text("recoveryCompare.templatePart");
    case "change":
      return text("recoveryCompare.title");
    case "name":
      return text("app.message23");
    case "presentation":
    case "field_presentation":
      return text("field.presentation");
    case "field_card_title":
      return text("group.cardTitleField");
    case "field_label":
      return text("field.label");
    case "field_required":
      return text("field.required");
    case "field_default":
      return text("field.default");
    case "field_writing_guide":
      return text("field.writingGuide");
  }
}
function TemplateRead({
  title,
  template,
}: {
  title: string;
  template: Template | null;
}) {
  return (
    <section>
      <h4>{title}</h4>
      {template && (
        <>
          <p>{template.name}</p>
          <p>
            {text("whole.base")} {template.revision}
          </p>
          <ul>
            {template.fields.map((f) => (
              <li key={f.id}>
                <strong>{f.label}</strong>
                <p>
                  {f.kind} · {f.lifecycle}
                </p>
                <ValueRead value={f.default} options={f.options} />
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
function DraftRead({ content }: { content: RecoveryContent }) {
  const draft = content.draft;
  if (draft.kind === "template")
    return (
      <div>
        <h4>{draft.name || text("field.emptyLabel")}</h4>
        <ul>
          {draft.fields.map((f) => (
            <li key={f.id}>
              <strong>{f.label || text("field.emptyLabel")}</strong>
              {f.archived && <span> · {text("whole.archived")}</span>}
              <p>
                {f.configuration.kind} · {f.default.intent}
              </p>
              {f.default.intent === "set" && (
                <ValueRead
                  value={f.default.value}
                  options={
                    "options" in f.configuration
                      ? f.configuration.options.map((o) => ({
                          ...o,
                          lifecycle: o.archived ? "Archived" : "Active",
                        }))
                      : []
                  }
                />
              )}
            </li>
          ))}
        </ul>
      </div>
    );
  // 복구 자료의 두 대상 의도와 raw 증거를 생략하지 않고 표시한다.
  return (
    <details open>
      <summary>{text("whole.recoveryDraft")}</summary>
      <pre className="recovery-raw">{JSON.stringify(draft, null, 2)}</pre>
    </details>
  );
}
