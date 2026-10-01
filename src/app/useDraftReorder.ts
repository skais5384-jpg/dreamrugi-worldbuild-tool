import { useRef, useState, type HTMLAttributes } from "react";
import { text } from "../strings";

type Drag = {
  owner: string;
  generation: string;
  group: string;
  source: string;
  order: string[];
};
/** 미리보기는 CSS 배치만 바꾼다. 입력 DOM/원문/세대는 drop까지 그대로 유지한다. */
export function useDraftReorder(
  owner: string,
  generation: string,
  enabled: boolean,
  commit: (group: string, order: string[]) => void,
  announce: (message: string) => void,
) {
  const active = useRef<Drag | null>(null);
  const [preview, setPreview] = useState<Drag | null>(null);
  const visiblePreview =
    enabled && preview?.owner === owner && preview.generation === generation
      ? preview
      : null;
  const valid = () =>
    enabled &&
    active.current?.owner === owner &&
    active.current?.generation === generation;
  const clear = () => {
    active.current = null;
    setPreview(null);
  };
  const card = (
    group: string,
    id: string,
    order: string[],
    archived = false,
  ): HTMLAttributes<HTMLElement> => ({
    className: `whole-drag-card${visiblePreview?.group === group && visiblePreview.source === id ? " whole-drag-preview" : ""}`,
    style:
      visiblePreview?.group === group
        ? {
            order: archived
              ? order.length + 1
              : visiblePreview.order.indexOf(id) + 1,
          }
        : undefined,
    draggable: enabled && !archived && order.length > 1,
    onDragStart: (event) => {
      if (!enabled || archived || order.length <= 1) {
        event.preventDefault();
        return;
      }
      event.stopPropagation();
      active.current = {
        owner,
        generation,
        group,
        source: id,
        order: [...order],
      };
      if (event.dataTransfer) {
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("application/x-worldbuild-order", id);
      }
      setPreview(active.current);
      announce(text("whole.dragHelp"));
    },
    onDragOver: (event) => {
      const drag = active.current;
      if (!drag || drag.group !== group || archived || !valid()) return;
      event.preventDefault();
      event.stopPropagation();
      if (id === drag.source) return;
      const bounds = event.currentTarget.getBoundingClientRect();
      const horizontal =
        event.currentTarget.parentElement &&
        getComputedStyle(event.currentTarget.parentElement).flexDirection ===
          "row";
      const after = horizontal
        ? event.clientX > bounds.left + bounds.width / 2
        : event.clientY > bounds.top + bounds.height / 2;
      const next = drag.order.filter((item) => item !== drag.source);
      const index = next.indexOf(id);
      if (index < 0) return;
      next.splice(index + (after ? 1 : 0), 0, drag.source);
      if (next.every((item, i) => item === drag.order[i])) return;
      active.current = { ...drag, order: next };
      setPreview(active.current);
      announce(text("whole.dragSnap"));
    },
    onDrop: (event) => {
      const drag = active.current;
      if (!drag || drag.group !== group) return;
      event.preventDefault();
      event.stopPropagation();
      if (archived) {
        clear();
        return;
      }
      if (!valid()) announce(text("whole.staleDrag"));
      else if (!drag.order.every((item, i) => item === order[i])) {
        commit(group, drag.order);
        announce(text("whole.reordered"));
      }
      clear();
    },
    onDragEnd: clear,
  });
  const surface: HTMLAttributes<HTMLElement> = {
    onKeyDown: (event) => {
      if (event.key === "Escape" && active.current) {
        event.preventDefault();
        event.stopPropagation();
        clear();
      }
    },
    onDragOver: (event) => {
      if (active.current) event.preventDefault();
    },
    onDrop: (event) => {
      // 유효한 카드 drop은 전파를 멈춘다. 나머지는 원문 입력에 삽입하지 않고 취소한다.
      if (active.current) {
        event.preventDefault();
        clear();
      }
    },
  };
  return { card, preview: visiblePreview, surface };
}
