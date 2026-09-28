import type { FocusEvent } from "react";

type TextControl = HTMLInputElement | HTMLTextAreaElement;
const textControl = (node: HTMLElement): node is TextControl =>
  node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement;

/** 확인창의 트리거가 focus를 가져가기 전 마지막 입력 위치를 별도로 보존한다. */
export class EditingFocus {
  private last: HTMLElement | null = null;
  private input: HTMLElement | null = null;
  private selection: [number, number, "forward" | "backward" | "none"] | null =
    null;
  private richSelection: Range | null = null;
  private pending: (() => void) | null = null;

  capture = (event: FocusEvent<HTMLElement>) => {
    this.last = event.target;
    if (
      event.target.matches("input, textarea, select, [contenteditable=true]")
    ) {
      this.input = event.target;
      this.rememberSelection();
    }
  };
  rememberSelection = () => {
    if (this.input?.isContentEditable) {
      const range = window.getSelection()?.rangeCount
        ? window.getSelection()!.getRangeAt(0)
        : null;
      if (range && this.input.contains(range.commonAncestorContainer))
        this.richSelection = range.cloneRange();
    }
    if (
      this.input &&
      textControl(this.input) &&
      this.input.selectionStart !== null
    )
      this.selection = [
        this.input.selectionStart,
        this.input.selectionEnd!,
        this.input.selectionDirection ?? "none",
      ];
  };
  prepare() {
    this.rememberSelection();
    const node = this.input?.isConnected ? this.input : this.last;
    const selection = this.selection;
    const richSelection = this.richSelection?.cloneRange();
    const position = { left: window.scrollX, top: window.scrollY };
    const ancestors: [HTMLElement, number, number][] = [];
    for (let parent = node; parent; parent = parent.parentElement)
      ancestors.push([parent, parent.scrollLeft, parent.scrollTop]);
    this.pending = () => {
      if (!node?.isConnected) return;
      node.focus({ preventScroll: true });
      if (textControl(node) && selection && node.selectionStart !== null)
        node.setSelectionRange(...selection);
      if (node.isContentEditable && richSelection) {
        const current = window.getSelection();
        current?.removeAllRanges();
        current?.addRange(richSelection);
      }
      for (const [element, left, top] of ancestors) {
        element.scrollLeft = left;
        element.scrollTop = top;
      }
      window.scrollTo(position);
    };
  }
  restore = () => {
    const restore = this.pending;
    this.pending = null;
    // Fluent의 modal/스크롤 잠금과 닫기 모션이 끝난 다음 한 번만 복원한다.
    if (restore) requestAnimationFrame(restore);
  };
}
