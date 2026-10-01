import { useEffect, useRef, type RefObject } from "react";

/** The visible editor invokes its existing save action; no edit/commit entry. */
export function useSaveShortcut({
  root,
  enabled,
  blocked,
  composing,
  save,
}: {
  root: RefObject<HTMLElement | null>;
  enabled: boolean;
  blocked: boolean;
  composing: boolean;
  save: () => unknown | Promise<unknown>;
}) {
  const latest = useRef({ enabled, blocked, composing, save });
  const held = useRef(false);
  const pending = useRef(false);
  const settling = useRef(false);
  useEffect(() => {
    latest.current = { enabled, blocked, composing, save };
  }, [enabled, blocked, composing, save]);
  useEffect(() => {
    const visible = () =>
      !!root.current &&
      !root.current.closest("[hidden], [inert]") &&
      root.current.getClientRects().length > 0;
    const keydown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        !event.ctrlKey ||
        event.altKey ||
        event.metaKey ||
        event.shiftKey ||
        event.key.toLowerCase() !== "s" ||
        !visible()
      )
        return;
      // Never save through a modal/menu or a suspended workspace owner.
      event.preventDefault();
      if (
        [
          ...document.querySelectorAll<HTMLElement>(
            '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]',
          ),
        ].some(
          (popup) =>
            !popup.closest('[hidden], [inert], [aria-hidden="true"]') &&
            popup.getClientRects().length > 0,
        )
      )
        return;
      const state = latest.current;
      const duplicate = held.current || pending.current || event.repeat;
      held.current = true;
      if (
        !state.enabled ||
        state.blocked ||
        state.composing ||
        event.isComposing ||
        event.keyCode === 229 ||
        settling.current ||
        duplicate
      )
        return;
      pending.current = true;
      // Controllers own save errors, policy checks and the retained draft.
      void Promise.resolve()
        .then(() => {
          const current = latest.current;
          if (
            visible() &&
            current.enabled &&
            !current.blocked &&
            !current.composing &&
            !settling.current
          )
            return current.save();
        })
        .catch(() => undefined)
        .finally(() => {
          pending.current = false;
        });
    };
    const keyup = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === "s") held.current = false;
      if (!latest.current.composing) settling.current = false;
    };
    const start = (event: CompositionEvent) => {
      if (root.current?.contains(event.target as Node)) settling.current = true;
    };
    const end = (event: CompositionEvent) => {
      if (root.current?.contains(event.target as Node)) settling.current = true;
    };
    const blur = () => {
      held.current = false;
    };
    document.addEventListener("keydown", keydown);
    document.addEventListener("keyup", keyup);
    document.addEventListener("compositionstart", start);
    document.addEventListener("compositionend", end);
    window.addEventListener("blur", blur);
    return () => {
      document.removeEventListener("keydown", keydown);
      document.removeEventListener("keyup", keyup);
      document.removeEventListener("compositionstart", start);
      document.removeEventListener("compositionend", end);
      window.removeEventListener("blur", blur);
    };
  }, [root]);
}
