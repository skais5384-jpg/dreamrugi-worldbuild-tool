import {
  $getSelection,
  $isRangeSelection,
  $setSelection,
  getNearestEditorFromDOMNode,
  SKIP_SCROLL_INTO_VIEW_TAG,
} from "lexical";

type Position = { top: number; restore?: () => void };

/** 탭의 표시 상태만 기억한다. 조합 상태와 저장 초안의 소유권은 복원하지 않는다. */
export class DocumentPositions {
  private positions = new Map<string, Position>();

  scroll(pane: string, top: number) {
    this.positions.set(pane, { ...this.positions.get(pane), top });
  }

  remember(pane: string, target: EventTarget, body: HTMLElement) {
    if (
      !(target instanceof HTMLElement) ||
      !body.contains(target) ||
      target.closest("[hidden]")
    )
      return;
    let restore: (() => void) | undefined;
    if (
      (target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement) &&
      target.selectionStart !== null
    ) {
      const start = target.selectionStart;
      const end = target.selectionEnd;
      const direction = target.selectionDirection ?? "none";
      restore = () => {
        target.focus({ preventScroll: true });
        target.setSelectionRange(start, end, direction);
      };
    } else if (target.matches("[contenteditable=true]")) {
      const editor = getNearestEditorFromDOMNode(target);
      const selection = editor?.getEditorState().read(() => {
        const current = $getSelection();
        return $isRangeSelection(current) ? current.clone() : null;
      });
      if (editor && selection)
        restore = () => {
          target.focus({ preventScroll: true });
          // DOM Range 대신 편집기 선택을 복원해 다음 입력과 실행 취소도 같은 위치를 쓴다.
          editor.update(() => $setSelection(selection.clone()), {
            discrete: true,
            tag: SKIP_SCROLL_INTO_VIEW_TAG,
          });
        };
    }
    if (restore) {
      const apply = restore;
      this.positions.set(pane, {
        top: body.scrollTop,
        restore: () => {
          if (target.isConnected && !target.closest("[hidden]")) apply();
        },
      });
    }
  }

  restore(pane: string, body: HTMLElement) {
    const position = this.positions.get(pane);
    position?.restore?.();
    // 포커스를 돌려준 뒤 본문 위치를 적용해 브라우저의 자동 스크롤을 남기지 않는다.
    body.scrollTop = position?.top ?? 0;
  }
}
