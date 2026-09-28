import { Button } from "../ui/Controls";
import { Edit16Regular, Warning12Regular } from "@fluentui/react-icons";
import { Badge, Link, Tooltip } from "@fluentui/react-components";
import type { DocumentController } from "./documentController";
import type { DocumentReferences } from "../bridge/documents";
import { text } from "../strings";
import { InlineNotice } from "../ui/InlineNotice";

export function IncomingRelations({
  controller,
  document,
  references,
  busy,
  error,
}: {
  controller: DocumentController;
  document: string;
  references: DocumentReferences | null;
  busy: boolean;
  error: string | null;
}) {
  const current = references?.document === document ? references : null;
  return (
    <section
      className="incoming-relations"
      aria-label={text("relation.incoming")}
    >
      <h3>{text("relation.incoming")}</h3>
      {busy && (
        <InlineNotice kind="info" className="incoming-message">
          {text("relation.loading")}
        </InlineNotice>
      )}
      {error && (
        <InlineNotice kind="error" className="incoming-message">
          <strong>{text("relation.loadFailed")}</strong>
          <small>{text("relation.loadFailedHelp")}</small>
        </InlineNotice>
      )}
      {current?.unavailable.map((reference) => (
        <InlineNotice
          key={`${reference.source}:${reference.reason}`}
          kind="warning"
          className="incoming-message"
        >
          <strong>{reference.sourceName}</strong>
          <small>{text(`relation.unavailable.${reference.reason}`)}</small>
        </InlineNotice>
      ))}
      {current && !current.incoming.length && !current.unavailable.length && (
        <InlineNotice kind="info" className="incoming-message">
          {text("relation.incomingEmpty")}
        </InlineNotice>
      )}
      <ul>
        {current?.incoming.map((reference) => (
          <li
            className="incoming-reference-item"
            data-kind={reference.kind}
            key={[
              reference.kind,
              reference.source,
              reference.field,
              reference.instance,
              reference.kind === "relation"
                ? reference.connection
                : reference.target,
            ].join(":")}
          >
            <div className="incoming-reference">
              <div className="incoming-reference-heading">
                <Badge
                  appearance="tint"
                  color="subtle"
                  shape="rounded"
                  size="small"
                  className="incoming-reference-kind"
                >
                  {reference.kind === "relation"
                    ? text("relation.kind")
                    : text("relation.linkKind")}
                </Badge>
                <Tooltip
                  content={(reference.kind === "relation"
                    ? [
                        reference.sourceName,
                        text("relation.sourceRole", {
                          name:
                            reference.relationName || text("relation.noName"),
                        }),
                        reference.sourceTemplate,
                        reference.path.join(" / "),
                        reference.fieldLabel,
                      ]
                    : [
                        reference.sourceName,
                        reference.sourceTemplate,
                        reference.path.join(" / "),
                        reference.fieldLabel,
                      ]
                  )
                    .filter(Boolean)
                    .join(" · ")}
                  relationship="description"
                >
                  <Link
                    as="button"
                    type="button"
                    className="incoming-reference-source"
                    aria-label={
                      reference.kind === "relation"
                        ? `${text("relation.kind")}: ${reference.sourceName} – ${reference.relationName || text("relation.noName")}`
                        : `${text("relation.linkKind")}: ${reference.sourceName}`
                    }
                    onClick={() =>
                      void controller.openReferenceTarget(reference.source)
                    }
                  >
                    {reference.sourceName}
                  </Link>
                </Tooltip>
                {reference.kind === "relation" && (
                  <span className="incoming-reference-role">
                    {reference.relationName || text("relation.noName")}
                  </span>
                )}
              </div>
            </div>
            {reference.kind === "relation" && (
              <div className="incoming-reference-followup">
                {reference.missingReciprocal && (
                  <span className="relation-warning" role="status">
                    <Warning12Regular aria-hidden />
                    <span>{text("relation.missingReciprocal")}</span>
                  </span>
                )}
                <Button
                  type="button"
                  appearance="secondary"
                  size="small"
                  icon={<Edit16Regular aria-hidden />}
                  className="incoming-edit-source"
                  onClick={() => void controller.editReferenceSource(reference)}
                >
                  {text("relation.editSource")}
                </Button>
              </div>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
