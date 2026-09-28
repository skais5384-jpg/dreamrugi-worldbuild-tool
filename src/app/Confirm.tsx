import { useEffect, useRef } from "react";
import {
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogContent,
  DialogActions,
} from "@fluentui/react-components";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import type { TemplateController } from "./controller";
import { text } from "../strings";

export function Confirm({ controller }: { controller: TemplateController }) {
  const state = controller.snapshot();
  const keep = useRef<HTMLButtonElement>(null);
  const surface = useRef<HTMLDivElement>(null);
  const previousPrompt = useRef<typeof state.prompt>(null);
  useEffect(() => {
    // 하단 순서와 무관하게 최초/새 닫기 시도의 focus는 안전 행동에 둔다.
    // Tab 순환은 Fluent가 맡는다.
    if (previousPrompt.current !== state.prompt) keep.current?.focus();
    previousPrompt.current = state.prompt;
  }, [state.prompt]);
  return (
    <Dialog
      open={!!state.prompt}
      modalType="alert"
      onOpenChange={(event, data) => {
        // 바깥 click은 결정을 만들지 않는다. Escape는 기존의 '계속 편집' 의미다.
        event.preventDefault();
        if (data.type === "escapeKeyDown" && !state.deciding)
          void controller.decide(false);
      }}
    >
      <DialogSurface
        ref={surface}
        className="confirm"
        aria-describedby="confirm-description"
        backdrop={{
          onPointerDown: (event) => {
            // backdrop은 Escape를 받는 surface의 형제다. 기본 focus 이탈을 막고
            // 현재 확인창 안으로 돌려준다. 결정 대기 중에는 disabled 버튼 대신 본체를 쓴다.
            event.preventDefault();
            const target = keep.current?.disabled
              ? surface.current
              : keep.current;
            target?.focus();
          },
        }}
      >
        <DialogBody>
          <DialogTitle>{text("common.unsaved")}</DialogTitle>
          <DialogContent className="confirm-content">
            <p id="confirm-description">
              {state.prompt?.attempt
                ? text("common.discardClose")
                : text(
                    state.prompt?.navigation?.kind === "archive_field"
                      ? "archive.discardField"
                      : "common.discardNavigate",
                  )}{" "}
              {text("common.retainedSeparate")}
            </p>
            {state.error && (
              <InlineNotice kind="error">{state.error}</InlineNotice>
            )}
          </DialogContent>
          <DialogActions className="actions">
            <Button
              type="button"
              danger
              disabled={state.deciding}
              onClick={() => void controller.decide(true)}
            >
              {text("common.discard")}
            </Button>
            {state.prompt?.attempt && (
              <Button
                type="button"
                onClick={() => void controller.checkStatus()}
              >
                {text("common.checkClose")}
              </Button>
            )}
            <Button
              type="button"
              ref={keep}
              disabled={state.deciding}
              onClick={() => void controller.decide(false)}
            >
              {text("common.continue")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
