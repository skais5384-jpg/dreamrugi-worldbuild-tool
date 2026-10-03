import type { DraftField } from "../bridge/workspace";
import type { Field } from "../bridge/types";
import { Button } from "../ui/Controls";
import { text } from "../strings";
import { restoreDefinition, changeGroupMembers } from "./archiveDefinition";

export function ArchivedDefinitionActions({
  field,
  original,
  disabled,
  canonical,
  change,
}: {
  field: DraftField;
  original?: Field;
  disabled: boolean;
  canonical: (id: string) => string;
  change: (value: DraftField) => void;
}) {
  const restoreParent = (value: DraftField) => ({
    ...value,
    archived: false,
    restore: original?.lifecycle === "Archived",
  });
  const configuration = field.configuration;
  return (
    <div className="actions">
      {configuration.kind === "group" &&
        configuration.members
          .filter((member) => member.archived)
          .map((member) => (
            <Button
              key={member.id}
              type="button"
              disabled={disabled}
              onClick={() =>
                change(
                  restoreParent(
                    changeGroupMembers(
                      field,
                      restoreDefinition(
                        configuration.members,
                        member.id,
                        original?.members?.find(
                          (item) => item.id === canonical(member.id),
                        )?.lifecycle === "Archived",
                      ),
                      original?.cardTitleField,
                      canonical,
                    ),
                  ),
                )
              }
            >
              {text("archive.restoreParentChild")} ·{" "}
              {member.label || text("field.emptyLabel")}
            </Button>
          ))}
      {(configuration.kind === "single_choice" ||
        configuration.kind === "multi_choice") &&
        configuration.options
          .filter((option) => option.archived)
          .map((option) => (
            <Button
              key={option.id}
              type="button"
              disabled={disabled}
              onClick={() =>
                change(
                  restoreParent({
                    ...field,
                    configuration: {
                      ...configuration,
                      options: restoreDefinition(
                        configuration.options,
                        option.id,
                        original?.options.find(
                          (item) => item.id === canonical(option.id),
                        )?.lifecycle === "Archived",
                      ),
                    },
                  }),
                )
              }
            >
              {text("archive.restoreParentOption")} ·{" "}
              {option.label || text("field.emptyLabel")}
            </Button>
          ))}
    </div>
  );
}
