import type { DocumentRead } from "../bridge/documents";
import type { ReferenceContext } from "./DocumentReferenceValue";
import { requiredWarnings, missingRequired } from "./requiredWarnings";
import { blockField, PropertyRow } from "./PropertyRow";
import { presentationClass, SectionTitles } from "./Presentation";
import { MediaTargetContext } from "./MediaValue";
import { ValueRead } from "./FieldValue";
import { InlineNotice } from "../ui/InlineNotice";
import { Button } from "../ui/Controls";
import { numberInBounds } from "./numberBounds";
import { text } from "../strings";

export function fieldProblemMessage(
  reason: string | null | undefined,
  optionalGroup = false,
) {
  switch (reason) {
    case "RequiredValueUnset":
      return text("documents.valueProblem.RequiredValueUnset");
    case "MissingKnownFieldValue":
      return text(
        optionalGroup
          ? "documents.valueProblem.MissingOptionalGroup"
          : "documents.valueProblem.MissingKnownFieldValue",
      );
    case "InvalidKnownFieldValue":
      return text("documents.valueProblem.InvalidKnownFieldValue");
    case "UnknownSelectedOption":
      return text("documents.valueProblem.UnknownSelectedOption");
    default:
      return text("documents.valueUnavailable");
  }
}

/** Current reading and confirmed-version preview use the same field rendering. */
export function DocumentContent({
  read,
  reference,
  locked = false,
  onRestoreField,
}: {
  read: DocumentRead;
  reference?: ReferenceContext;
  locked?: boolean;
  onRestoreField?: (template: string) => void;
}) {
  const warnings = requiredWarnings(read);
  return (
    <MediaTargetContext.Provider
      value={{ kind: "document", artifact: read.id }}
    >
      {!!warnings.length && (
        <InlineNotice kind="warning">
          {text("required.summary")} ({warnings.length})
        </InlineNotice>
      )}
      {read.fields.map((row) => {
        const field = read.template.fields.find((field) => field.id === row.id);
        const autoRepair =
          row.problem === "MissingKnownFieldValue" &&
          field?.kind === "Group" &&
          !field.required &&
          field.lifecycle === "Active";
        return (
          <PropertyRow
            key={row.id}
            label={row.label}
            block={blockField(field?.kind)}
            complex
            before={<SectionTitles template={read.template} before={row.id} />}
            presentation={presentationClass(
              read.template.presentation,
              field?.presentation,
            )}
          >
            {field && missingRequired(field, row.value) && (
              <InlineNotice kind="warning">
                {text("required.missing")}
              </InlineNotice>
            )}
            {row.state !== "Active" && (
              <InlineNotice kind="warning">
                {text(
                  row.state === "Archived"
                    ? "field.archived"
                    : "documents.orphan",
                )}
              </InlineNotice>
            )}
            {row.state !== "Active" &&
              onRestoreField &&
              field?.lifecycle === "Archived" && (
                <Button
                  type="button"
                  disabled={locked}
                  onClick={() => onRestoreField(read.template.id)}
                >
                  {text("archive.openDefinitions")}
                </Button>
              )}
            {row.value?.kind === "number" &&
              !numberInBounds(
                row.value.value,
                field?.minimum,
                field?.maximum,
              ) && <p role="status">{text("field.outsideBounds")}</p>}
            {row.value ? (
              <ValueRead
                value={row.value}
                field={field}
                options={field?.options ?? []}
                reference={reference}
              />
            ) : (
              <InlineNotice kind={autoRepair ? "info" : "warning"}>
                {fieldProblemMessage(row.problem, autoRepair)}
              </InlineNotice>
            )}
          </PropertyRow>
        );
      })}
      <SectionTitles template={read.template} before={null} />
    </MediaTargetContext.Provider>
  );
}
