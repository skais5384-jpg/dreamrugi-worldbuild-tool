import { useRef } from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useSaveShortcut } from "./useSaveShortcut";

function Editor({
  save,
  enabled = true,
  blocked = false,
  composing = false,
  hidden = false,
}: {
  save: () => unknown | Promise<unknown>;
  enabled?: boolean;
  blocked?: boolean;
  composing?: boolean;
  hidden?: boolean;
}) {
  const root = useRef<HTMLElement>(null);
  useSaveShortcut({ root, save, enabled, blocked, composing });
  return (
    <article ref={root} hidden={hidden}>
      <input aria-label="draft" />
    </article>
  );
}
beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockImplementation(
    function (this: HTMLElement) {
      return (this.closest("[hidden], [inert]")
        ? []
        : [new DOMRect(0, 0, 100, 40)]) as unknown as DOMRectList;
    },
  );
});
const down = (extra: Partial<KeyboardEvent> = {}) =>
  fireEvent.keyDown(document, { key: "s", ctrlKey: true, ...extra });
const up = () => fireEvent.keyUp(document, { key: "s", ctrlKey: true });
const flush = () =>
  act(async () => {
    await Promise.resolve();
  });

it("saves only the active visible editor once per key press and in-flight request", async () => {
  let finish!: () => void;
  const active = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const other = vi.fn();
  render(
    <>
      <Editor save={other} hidden />
      <Editor save={active} />
    </>,
  );
  expect(down()).toBe(false);
  await flush();
  down({ repeat: true });
  up();
  down();
  await flush();
  expect(active).toHaveBeenCalledTimes(1);
  expect(other).not.toHaveBeenCalled();
  await act(async () => finish());
  up();
  down();
  await flush();
  expect(active).toHaveBeenCalledTimes(2);
});

it.each(["blocked", "composing", "disabled"])(
  "does not save a %s owner",
  async (condition) => {
    const save = vi.fn();
    render(
      <Editor
        save={save}
        blocked={condition === "blocked"}
        composing={condition === "composing"}
        enabled={condition !== "disabled"}
      />,
    );
    down();
    await flush();
    expect(save).not.toHaveBeenCalled();
  },
);

it.each(["dialog", "alertdialog", "menu", "listbox"])(
  "protects the visible %s and resumes after it closes",
  async (role) => {
    const save = vi.fn();
    const view = render(
      <>
        <Editor save={save} />
        <div role={role}>popup</div>
      </>,
    );
    down();
    await flush();
    expect(save).not.toHaveBeenCalled();
    view.rerender(<Editor save={save} />);
    up();
    down();
    await flush();
    expect(save).toHaveBeenCalledTimes(1);
  },
);

it("ignores hidden popups and saves the current controlled draft", async () => {
  const save = vi.fn();
  render(
    <>
      <Editor save={save} />
      <div role="dialog" hidden />
    </>,
  );
  down();
  await flush();
  expect(save).toHaveBeenCalledTimes(1);
});

it("suppresses composition keys and the composition-end chord until release", async () => {
  const save = vi.fn();
  const view = render(<Editor save={save} />);
  const input = view.getByRole("textbox");
  fireEvent.compositionStart(input);
  down({ isComposing: true });
  fireEvent.compositionEnd(input);
  down();
  await flush();
  expect(save).not.toHaveBeenCalled();
  up();
  down();
  await flush();
  expect(save).toHaveBeenCalledTimes(1);
});

it("rechecks owner protection before dispatching a queued save", async () => {
  const save = vi.fn();
  const view = render(<Editor save={save} />);
  down();
  view.rerender(<Editor save={save} blocked />);
  await flush();
  expect(save).not.toHaveBeenCalled();
});

it("leaves alternate key combinations to their existing handlers", async () => {
  const save = vi.fn();
  render(<Editor save={save} />);
  down({ shiftKey: true });
  down({ altKey: true });
  down({ metaKey: true });
  down({ ctrlKey: false });
  await flush();
  expect(save).not.toHaveBeenCalled();
});
