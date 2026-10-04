import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { useEffect, useRef } from "react";
import type { TemplateController } from "./controller";
import type { ResultDto } from "../bridge/types";
import { text } from "../strings";
import { useImeGuard } from "./ime";

export function deletionMessage(result: ResultDto | null): string | null {
  if (result?.kind !== "write" || !result.deletion) return null;
  const summary = result.deletion;
  if (summary.reason === "source_changed")
    return text("template.sourceChanged");
  if (summary.reason === "reference_check_failed")
    return text("template.checkFailed");
  if (summary.reason !== "template_has_documents")
    return text("template.checkFailed");
  if (
    !Number.isInteger(summary.count) ||
    summary.count < 1 ||
    summary.count > 1024 ||
    typeof summary.truncated !== "boolean" ||
    (summary.truncated && summary.count !== 1024)
  )
    return text("template.checkFailed");
  return summary.truncated
    ? text("template.referencesTruncated")
    : text("template.references", { count: String(summary.count) });
}

export function TemplateManagement({
  controller,
  locked,
}: {
  controller: TemplateController;
  locked: boolean;
}) {
  const state = controller.snapshot(),
    action = state.templateAction;
  const cancel = useRef<HTMLButtonElement>(null);
  const ime = useImeGuard();
  const focused = useRef(false);
  useEffect(() => {
    const previous = document.activeElement;
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected)
        previous.focus();
    };
  }, []);
  useEffect(() => {
    // 이동 action이 끝나 버튼이 활성화된 뒤 최초 초점을 준다. 결과 갱신마다 초점을 빼앗지는 않는다.
    if (
      !locked &&
      !focused.current &&
      cancel.current &&
      !cancel.current.disabled
    ) {
      cancel.current.focus();
      focused.current = true;
    }
  }, [locked]);
  if (!action) return null;
  const blocked =
    locked ||
    action.phase === "pending" ||
    (action.handedOff && state.retainedRefs.length > 0);
  const refusal = deletionMessage(action.result);
  return (
    <section
      {...ime.bind}
      className="editor template-management"
      aria-labelledby="management-title"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !blocked && !ime.isComposing()) {
          event.preventDefault();
          void controller.navigate({ kind: "cancel" });
        }
      }}
    >
      <h2 id="management-title">
        {text(
          action.kind === "duplicate"
            ? "template.duplicate"
            : "template.delete",
        )}
      </h2>
      <p className="literal">
        {text("template.target", {
          name: action.source.content.name || text("field.emptyLabel"),
        })}
      </p>
      <p>
        {text(
          action.kind === "duplicate"
            ? "template.duplicateImpact"
            : "template.deleteImpact",
        )}
      </p>
      <p role="status">{action.message}</p>
      {refusal && <InlineNotice kind="error">{refusal}</InlineNotice>}
      {action.result?.kind === "write" &&
        (action.result.cleanup_failed ||
          action.result.recovery_required ||
          action.result.warnings.length > 0) && (
          <p>{text("template.postSave")}</p>
        )}
      <div className="actions">
        <Button
          type="button"
          ref={cancel}
          disabled={blocked}
          onClick={() => void controller.navigate({ kind: "cancel" })}
        >
          {text("common.cancel")}
        </Button>
        {action.phase === "confirm" && (
          <Button
            type="button"
            appearance={action.kind === "delete" ? "secondary" : "primary"}
            danger={action.kind === "delete"}
            disabled={locked || action.source !== state.selection}
            onClick={(event) => {
              if (ime.allowAction(event))
                void controller.executeTemplateAction();
            }}
          >
            {text(
              action.kind === "duplicate"
                ? "template.confirmDuplicate"
                : "template.confirmDelete",
            )}
          </Button>
        )}
        {action.artifact && (
          <Button
            type="button"
            disabled={locked || state.closing}
            onClick={() => void controller.readTemplateAction()}
          >
            {text("template.readResult")}
          </Button>
        )}
      </div>
    </section>
  );
}
