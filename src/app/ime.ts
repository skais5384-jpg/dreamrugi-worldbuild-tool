import { useCallback, useRef } from "react";
import type { HTMLAttributes, SyntheticEvent } from "react";

const initial = () => ({ composing: false, ending: false, released: true });

/** 해당 form/동작 영역에서 조합 확정과 후속 저장의 경계를 함께 보호한다. */
export function useImeGuard() {
  const state = useRef(initial());
  // App의 form이 사라졌다가 다시 열려도 이전 조합 상태가 새 DOM에 넘어가지 않는다.
  const ref = useCallback(() => {
    state.current = initial();
  }, []);
  const events: HTMLAttributes<HTMLElement> = {
    onCompositionStartCapture() {
      state.current = { composing: true, ending: true, released: false };
    },
    onCompositionEndCapture() {
      // 종료 뒤의 기본 click/submit도 같은 확정 동작일 수 있어 즉시 저장을 허용하지 않는다.
      state.current = { composing: false, ending: true, released: false };
    },
    onKeyDownCapture(event) {
      const current = state.current;
      const native = event.nativeEvent;
      const enter =
        event.key === "Enter" ||
        native.code === "Enter" ||
        native.code === "NumpadEnter";
      const ime =
        current.composing || native.isComposing || native.keyCode === 229;
      if (ime) current.ending = true;
      else if (!enter || (current.released && !native.repeat))
        current.ending = false;
      current.released = false;
      // key/code가 식별 불가인 229 문자 키는 허용하고, 이어지는 저장 진입에서 다시 막는다.
      if (enter && (ime || current.ending)) event.preventDefault();
    },
    onKeyUpCapture() {
      if (!state.current.composing) state.current.released = true;
      // keyup 자체를 저장 의도로 보지 않는다. 다음 정상 keydown이 키보드 제출을 다시 연다.
    },
    onClickCapture(event) {
      // implicit submit도 click(detail 0)을 보낸다. 양수의 포인터 click만 새 적용 동작이다.
      if (event.detail > 0 && !state.current.composing)
        state.current = initial();
    },
    onBlurCapture(event) {
      if (!event.currentTarget.contains(event.relatedTarget))
        state.current = initial();
    },
  };
  return {
    bind: { ...events, ref },
    isComposing: () => state.current.composing,
    allowAction(event: SyntheticEvent) {
      if (!state.current.composing && !state.current.ending) return true;
      event.preventDefault();
      return false;
    },
  };
}
