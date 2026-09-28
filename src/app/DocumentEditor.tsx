import { Badge, Field as FluentField } from "@fluentui/react-components";
import {
  CheckmarkCircle16Regular,
  Archive20Regular,
  ArrowSync20Regular,
  Clock16Regular,
  Edit16Regular,
  DocumentPdf20Regular,
  TextGrammarCheckmark20Regular,
  ErrorCircle16Regular,
  Save20Regular,
  DoorArrowRight20Regular,
} from "@fluentui/react-icons";
import type { DocumentController } from "./documentController";
import type { EditEntry } from "./documentEdits";
import { editableProblem, editDirty } from "./documentEdits";
import { PropertyRow } from "./PropertyRow";
import { CreationValue } from "./CreationValue";
import { ValueRead } from "./FieldValue";
import { Checkbox, Input } from "../ui/Controls";
import { text } from "../strings";
import { presentationClass, SectionTitles } from "./Presentation";
import { useContext, useEffect, useRef, useState } from "react";
import { groupDraft } from "./groups";
import { MediaActiveContext } from "./MediaValue";
import type { ReferenceContext } from "./DocumentReferenceValue";
import { IncomingRelations } from "./IncomingRelations";
import { InlineNotice } from "../ui/InlineNotice";
import { FormatControl } from "./FormatControl";
import { IconCommand } from "../ui/IconCommand";
import { DocumentIssueNotices, type DocumentIssue } from "./DocumentIssues";
import { SpellcheckDialog } from "./SpellcheckDialog";
import { SvnCommitIcon } from "./SvnCommitIcon";

export function editLabel(e: EditEntry) {
  return text(
    e.busy
      ? "documentEdit.saving"
      : e.error || e.paused
        ? "documentEdit.error"
        : editableProblem(e) ||
            (e.status.generation === e.generation && e.status.problem)
          ? "documentEdit.invalid"
          : editDirty(e)
            ? "documentEdit.dirty"
            : "documentEdit.saved",
  );
}
export function DocumentEditor({
  controller,
  id,
  entry,
  hidden,
  locked,
  reference,
  issues = [],
  exportPdf,
  collaborative = false,
  onCommit,
}: {
  controller: DocumentController;
  id: string;
  entry: EditEntry;
  hidden: boolean;
  locked: boolean;
  reference?: ReferenceContext;
  issues?: readonly DocumentIssue[];
  exportPdf?: (id: string) => void;
  collaborative?: boolean;
  onCommit?: (id: string) => void;
}) {
  const read = entry.status.read;
  const references: ReferenceContext = reference ?? {
    list: controller.snapshot().list,
    templates: controller.shell.snapshot().rows,
    currentDocument: id,
    open: (document) => void controller.openReferenceTarget(document),
  };
  const active = useContext(MediaActiveContext);
  const article = useRef<HTMLElement>(null);
  const [spellOpen, setSpellOpen] = useState(false);
  useEffect(() => {
    if (!hidden) return;
    const frame = requestAnimationFrame(() => setSpellOpen(false));
    return () => cancelAnimationFrame(frame);
  }, [hidden]);
  const problem = editableProblem(entry);
  const fieldProblem =
    entry.status.generation === entry.generation ? entry.status.field : null;
  const invalidEnglishName =
    entry.status.problem === "SingleLineRequired" &&
    fieldProblem === "englishName";
  const invalidGlossarySummary =
    entry.status.problem === "SingleLineRequired" &&
    fieldProblem === "glossarySummary";
  const referenceFocus = controller.snapshot().referenceFocus;
  const statusKind =
    entry.error || entry.paused || editableProblem(entry)
      ? "error"
      : entry.busy
        ? "busy"
        : editDirty(entry)
          ? "dirty"
          : "saved";
  const StatusIcon =
    statusKind === "error"
      ? ErrorCircle16Regular
      : statusKind === "busy"
        ? Clock16Regular
        : statusKind === "dirty"
          ? Edit16Regular
          : CheckmarkCircle16Regular;
  useEffect(() => {
    if (hidden || referenceFocus?.document !== id) return;
    let focusFrame = 0;
    const frame = requestAnimationFrame(() => {
      const editors = article.current?.querySelectorAll<HTMLElement>(
        ".reference-editor[data-reference-field]",
      );
      const target = [...(editors ?? [])].find((editor) => {
        const address = editor.dataset.referenceField;
        if (!address) return false;
        if (!referenceFocus.instance)
          return !referenceFocus.group && address === referenceFocus.field;
        try {
          const parsed: unknown = JSON.parse(address);
          return (
            Array.isArray(parsed) &&
            parsed.length === 3 &&
            parsed[0] === referenceFocus.group &&
            parsed[1] === referenceFocus.instance &&
            parsed[2] === referenceFocus.field
          );
        } catch {
          return false;
        }
      });
      if (!target) {
        controller.rejectReferenceFocus(id, referenceFocus.request);
        return;
      }
      const card = target.closest(".repeat-card");
      const toggle = card?.querySelector<HTMLButtonElement>(
        'button[aria-expanded="false"]',
      );
      const focus = () => {
        const control =
          target.querySelector<HTMLElement>('[role="combobox"]') ??
          target.querySelector<HTMLElement>("input, select, button");
        control?.focus();
        target.scrollIntoView?.({ block: "center" });
        controller.consumeReferenceFocus(id, referenceFocus.request);
      };
      if (toggle) {
        toggle.click();
        focusFrame = requestAnimationFrame(focus);
      } else focus();
    });
    return () => {
      cancelAnimationFrame(frame);
      if (focusFrame) cancelAnimationFrame(focusFrame);
    };
  }, [controller, hidden, id, referenceFocus]);
  return (
    <MediaActiveContext.Provider value={active && !hidden}>
      <article
        ref={article}
        hidden={hidden}
        className="document-editor"
        onCompositionStartCapture={() =>
          controller.edits.update(id, (b) => ({ ...b, composing: true }))
        }
        onCompositionEndCapture={() =>
          controller.edits.update(id, (b) => ({ ...b, composing: false }))
        }
      >
        <header className="document-edit-header">
          <Badge appearance="tint" color="informative">
            {text("documents.template")}: {read.template.name}
          </Badge>
          <div
            className="document-save-state"
            data-state={statusKind}
            role="status"
          >
            <StatusIcon aria-hidden />
            <span>{editLabel(entry)}</span>
          </div>
          <div className="actions">
            <IconCommand
              label={text("documentEdit.save")}
              icon={<Save20Regular />}
              disabled={locked || entry.busy || !!problem || entry.paused}
              onClick={() => void controller.edits.submit(id)}
            />
            {collaborative && onCommit && (
              <IconCommand
                label={text("svn.commit")}
                icon={<SvnCommitIcon />}
                disabled={locked || entry.busy || editDirty(entry)}
                onClick={() => onCommit(id)}
              />
            )}
            <IconCommand
              label="맞춤법 검사"
              icon={<TextGrammarCheckmark20Regular />}
              disabled={locked || entry.busy || entry.body.composing}
              onClick={() => setSpellOpen(true)}
            />
            {exportPdf && (
              <IconCommand
                label={text("pdf.command")}
                icon={<DocumentPdf20Regular />}
                disabled={locked || entry.busy}
                onClick={() => exportPdf(id)}
              />
            )}
            <FormatControl
              shell={controller.shell}
              kind="document"
              artifact={id}
              locked
              changed={() => controller.load()}
            />
            {(entry.error || entry.paused) &&
              entry.status.problem === "SavedReadRequired" && (
                <IconCommand
                  label={text("documentEdit.refresh")}
                  icon={<ArrowSync20Regular />}
                  disabled={entry.busy}
                  onClick={() => void controller.edits.refresh(id)}
                />
              )}
            {(entry.error || entry.paused) &&
              entry.status.problem !== "SavedReadRequired" &&
              entry.status.problem !== "Uncertain" && (
                <IconCommand
                  label={text("documentEdit.retry")}
                  icon={<ArrowSync20Regular />}
                  disabled={entry.busy || locked}
                  onClick={() => void controller.edits.retry(id)}
                />
              )}
            {(entry.error || entry.paused) && (
              <IconCommand
                label={text("documentEdit.deposit")}
                icon={<Archive20Regular />}
                disabled={entry.busy || locked}
                onClick={() => void controller.edits.submit(id, true)}
              />
            )}
            <IconCommand
              label={text("documentEdit.exit")}
              icon={<DoorArrowRight20Regular />}
              disabled={locked}
              onClick={() => void controller.endEdit(id)}
            />
          </div>
        </header>
        <DocumentIssueNotices issues={issues} />
        {(entry.error || entry.paused) && (
          <div>
            <InlineNotice kind="error">
              {text(
                entry.status.problem === "SavedReadRequired"
                  ? "documentEdit.readRequired"
                  : entry.status.problem === "OptionalGroupRepairRejected"
                    ? "documentEdit.groupRepairFailed"
                    : entry.status.problem === "RecoveryUnavailable"
                      ? "documentEdit.recoveryUnavailable"
                      : entry.status.problem === "SourceChanged"
                        ? "documentEdit.sourceChanged"
                        : entry.status.problem === "Uncertain"
                          ? "documentEdit.resultUnknown"
                          : entry.status.problem === "SessionRejected"
                            ? "documentEdit.lockCheckFailed"
                            : "documentEdit.saveStopped",
              )}
            </InlineNotice>
          </div>
        )}
        {entry.status.deposited &&
          entry.status.generation === entry.generation && (
            <InlineNotice kind="info">
              {text("documentEdit.deposited")}
            </InlineNotice>
          )}
        {(entry.error || entry.paused) &&
          !(
            entry.status.deposited &&
            entry.status.generation === entry.generation
          ) && (
            <InlineNotice kind="warning">
              {text("documentEdit.inputNotDeposited")}
            </InlineNotice>
          )}
        {!entry.paused &&
          entry.status.problem &&
          entry.status.problem !== "Composing" &&
          entry.status.generation === entry.generation && (
            <InlineNotice kind="error">
              {text("documentEdit.validation")}
            </InlineNotice>
          )}
        <PropertyRow
          label={text("documentEdit.name")}
          htmlFor={"edit-name-" + id}
        >
          <FluentField
            validationState={problem === "name" ? "error" : "none"}
            validationMessage={
              problem === "name" ? text("documents.requiredValue") : undefined
            }
          >
            <Input
              id={"edit-name-" + id}
              aria-label={text("documentEdit.name")}
              disabled={locked}
              value={
                entry.body.name.intent === "set"
                  ? entry.body.name.value
                  : read.name
              }
              onChange={(event) =>
                controller.edits.update(id, (b) => ({
                  ...b,
                  name: { intent: "set", value: event.target.value },
                }))
              }
            />
          </FluentField>
        </PropertyRow>
        <PropertyRow
          label={text("glossary.englishName")}
          htmlFor={"edit-english-name-" + id}
        >
          <FluentField
            validationState={invalidEnglishName ? "error" : "none"}
            validationMessage={
              invalidEnglishName ? text("documents.invalidValue") : undefined
            }
          >
            <Input
              id={"edit-english-name-" + id}
              aria-label={text("glossary.englishName")}
              aria-invalid={invalidEnglishName}
              disabled={locked}
              value={
                entry.body.englishName?.intent === "set"
                  ? entry.body.englishName.value
                  : (read.englishName ?? "")
              }
              onChange={(event) =>
                controller.edits.update(id, (body) => ({
                  ...body,
                  englishName: { intent: "set", value: event.target.value },
                }))
              }
            />
          </FluentField>
        </PropertyRow>
        <PropertyRow
          label={text("glossary.summary")}
          htmlFor={"edit-glossary-summary-" + id}
        >
          <FluentField
            validationState={invalidGlossarySummary ? "error" : "none"}
            validationMessage={
              invalidGlossarySummary
                ? text("documents.invalidValue")
                : undefined
            }
          >
            <Input
              id={"edit-glossary-summary-" + id}
              aria-label={text("glossary.summary")}
              aria-invalid={invalidGlossarySummary}
              disabled={locked}
              value={
                entry.body.glossarySummary?.intent === "set"
                  ? entry.body.glossarySummary.value
                  : (read.glossarySummary ?? "")
              }
              onChange={(event) =>
                controller.edits.update(id, (body) => ({
                  ...body,
                  glossarySummary: {
                    intent: "set",
                    value: event.target.value,
                  },
                }))
              }
            />
          </FluentField>
        </PropertyRow>
        <PropertyRow label={text("glossary.exclude")}>
          <div>
            <Checkbox
              aria-label={text("glossary.excludeDocument")}
              disabled={locked}
              checked={
                entry.body.glossaryExcluded?.intent === "set"
                  ? entry.body.glossaryExcluded.value
                  : (read.glossaryExcluded ?? false)
              }
              onChange={(_, data) =>
                controller.edits.update(id, (body) => ({
                  ...body,
                  glossaryExcluded: {
                    intent: "set",
                    value: data.checked === true,
                  },
                }))
              }
            />
            {!!read.template.glossaryExcluded && (
              <p className="property-help">
                {text("glossary.templateExcludedNotice")}
              </p>
            )}
          </div>
        </PropertyRow>
        {read.fields.map((f) => {
          const field = read.template.fields.find((t) => t.id === f.id);
          const intent = entry.body.fields.find((v) => v.field === f.id)?.value;
          const current =
            intent && intent.intent !== "keep"
              ? intent
              : f.value
                ? { intent: "set" as const, value: f.value }
                : { intent: "keep" as const };
          return (
            <PropertyRow
              key={f.id}
              before={<SectionTitles template={read.template} before={f.id} />}
              presentation={presentationClass(
                read.template.presentation,
                field?.presentation,
              )}
              label={f.label}
              complex
              htmlFor={"edit-" + id + "-" + f.id}
            >
              {field && entry.status.editable.includes(f.id) ? (
                <CreationValue
                  group={{
                    owner: entry.status.owner,
                    generation: entry.generation,
                    composing: entry.body.composing,
                    baseline: f.value?.kind === "group" ? f.value : undefined,
                    problem: fieldProblem,
                    importCell: (address, image) =>
                      controller.importAsset(f.id, image, id, address),
                    reference: references,
                  }}
                  importAsset={() =>
                    controller.importAsset(f.id, field?.kind === "Image", id)
                  }
                  key={entry.status.owner + f.id}
                  field={field}
                  reference={references}
                  intent={
                    field.kind === "Group" &&
                    (!intent || intent.intent === "keep")
                      ? {
                          intent: "set",
                          value: groupDraft(
                            f.value?.kind === "group" ? f.value : undefined,
                          ),
                        }
                      : current
                  }
                  change={(v) => controller.edits.field(id, f.id, v)}
                  invalid={problem === f.id || fieldProblem === f.id}
                  disabled={locked}
                  prefix={"edit-" + id + "-"}
                />
              ) : (
                <>
                  {f.value && (
                    <ValueRead
                      value={f.value}
                      options={field?.options ?? []}
                      field={field}
                      reference={references}
                    />
                  )}
                  <p>{text("documentEdit.kept")}</p>
                </>
              )}
            </PropertyRow>
          );
        })}
        <SectionTitles template={read.template} before={null} />
        <IncomingRelations
          controller={controller}
          document={id}
          references={controller.snapshot().references}
          busy={controller.snapshot().referencesBusy}
          error={controller.snapshot().referencesError}
        />
        {spellOpen && (
          <SpellcheckDialog
            controller={controller}
            document={id}
            entry={entry}
            locked={locked || hidden}
            close={() => setSpellOpen(false)}
          />
        )}
      </article>
    </MediaActiveContext.Provider>
  );
}
