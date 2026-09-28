import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getNearestEditorFromDOMNode } from "lexical";
import type { RichNode } from "../../bridge/types";
import { text } from "../../strings";
import { render } from "../../test/render";
import { RichEditor } from "./RichEditor";
import type { RichAction } from "./RichToolbar";
import { $exportRich } from "./adapter";

const paragraph = (value: string): RichNode => ({
  kind: "paragraph",
  children: value ? [{ kind: "text", text: value }] : [],
});
const item = (value: string): RichNode => ({
  kind: "listItem",
  children: [paragraph(value)],
});
beforeEach(() => {
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => new DOMRect(),
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
});
afterEach(cleanup);

function mount(value: RichNode) {
  const change = vi.fn();
  render(
    <RichEditor
      id="block-command"
      label="변환 본문"
      value={value}
      change={change}
      disabled={false}
      invalid={false}
    />,
  );
  const input = screen.getByRole("textbox", { name: "변환 본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  const result = () => editor.getEditorState().read($exportRich);
  const button = (kind: RichAction) =>
    screen.getByRole("button", { name: text(`rich.${kind}`) });
  return { input, change, result, button };
}

async function selectText(
  input: HTMLElement,
  first: number,
  last = first,
  firstOffset = 0,
  lastOffset?: number,
) {
  await act(async () => {
    input.focus();
    const walker = document.createTreeWalker(input, NodeFilter.SHOW_TEXT);
    const texts: Text[] = [];
    for (let node = walker.nextNode(); node; node = walker.nextNode())
      texts.push(node as Text);
    window
      .getSelection()!
      .setBaseAndExtent(
        texts[first],
        firstOffset,
        texts[last],
        lastOffset ?? texts[last].length,
      );
    fireEvent(document, new Event("selectionchange"));
  });
}

it("converts an existing two-item list without nesting or empty remnants", async () => {
  const value: RichNode = {
    kind: "root",
    children: [
      { kind: "bulletList", children: [item("first"), item("second")] },
    ],
  };
  const { input, change, result, button } = mount(value);
  await selectText(input, 0, 1);
  expect(change).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(button("orderedList")));
  expect(result()).toEqual({
    kind: "root",
    children: [
      { kind: "orderedList", children: [item("first"), item("second")] },
    ],
  });
  expect(change).toHaveBeenLastCalledWith(result());
});

it("splits a partial list and preserves marks, breaks and a nested list", async () => {
  const middle: RichNode = {
    kind: "listItem",
    children: [
      {
        kind: "paragraph",
        children: [
          { kind: "text", text: "mid", marks: ["bold", "italic"] },
          { kind: "hardBreak" },
          { kind: "text", text: "tail" },
        ],
      },
      { kind: "orderedList", children: [item("nested")] },
    ],
  };
  const value: RichNode = {
    kind: "root",
    children: [
      {
        kind: "bulletList",
        children: [item("before"), middle, item("after")],
      },
    ],
  };
  const { input, result, button } = mount(value);
  await selectText(input, 1, 1, 1, 2);
  await act(async () => fireEvent.click(button("taskList")));
  expect(result()).toEqual({
    kind: "root",
    children: [
      { kind: "bulletList", children: [item("before")] },
      {
        kind: "taskList",
        children: [{ ...middle, kind: "taskItem", checked: false }],
      },
      { kind: "bulletList", children: [item("after")] },
    ],
  });
});

it("converts one checked item, preserves adjacent states, and undo/redo is atomic", async () => {
  const task = (value: string, checked: boolean): RichNode => ({
    kind: "taskItem",
    checked,
    children: [paragraph(value)],
  });
  const value: RichNode = {
    kind: "root",
    children: [
      {
        kind: "taskList",
        children: [
          task("before", true),
          task("middle", true),
          task("after", false),
        ],
      },
    ],
  };
  const converted: RichNode = {
    kind: "root",
    children: [
      { kind: "taskList", children: [task("before", true)] },
      { kind: "orderedList", children: [item("middle")] },
      { kind: "taskList", children: [task("after", false)] },
    ],
  };
  const { input, result, button } = mount(value);
  await selectText(input, 1, 1, 2, 2);
  await act(async () => fireEvent.click(button("orderedList")));
  expect(result()).toEqual(converted);
  await act(async () => fireEvent.click(button("undo")));
  expect(result()).toEqual(value);
  await act(async () => fireEvent.click(button("redo")));
  expect(result()).toEqual(converted);
});

it("keeps the same list kind as a no-op and does not produce a save change", async () => {
  const value: RichNode = {
    kind: "root",
    children: [{ kind: "bulletList", children: [item("same")] }],
  };
  const { input, change, result, button } = mount(value);
  await selectText(input, 0);
  await act(async () => fireEvent.click(button("bulletList")));
  expect(result()).toEqual(value);
  expect(change).not.toHaveBeenCalled();
});

it("quotes a multi-item selection as one intact list and keeps heading changes inside items", async () => {
  const value: RichNode = {
    kind: "root",
    children: [
      { kind: "bulletList", children: [item("first"), item("second")] },
    ],
  };
  const first = mount(value);
  await selectText(first.input, 0, 1);
  await act(async () => fireEvent.click(first.button("blockquote")));
  expect(first.result()).toEqual({
    kind: "root",
    children: [
      {
        kind: "blockquote",
        children: [
          { kind: "bulletList", children: [item("first"), item("second")] },
        ],
      },
    ],
  });
  cleanup();
  const second = mount(value);
  await selectText(second.input, 0);
  await act(async () => fireEvent.click(second.button("heading")));
  expect(second.result()).toEqual({
    kind: "root",
    children: [
      {
        kind: "bulletList",
        children: [
          {
            kind: "listItem",
            children: [
              {
                kind: "heading",
                level: 3,
                children: [{ kind: "text", text: "first" }],
              },
            ],
          },
          item("second"),
        ],
      },
    ],
  });
});

it("converts an outer list once while preserving a selected nested list and history", async () => {
  const first: RichNode = {
    kind: "listItem",
    children: [
      {
        kind: "paragraph",
        children: [
          { kind: "text", text: "outer", marks: ["bold"] },
          { kind: "hardBreak" },
          { kind: "text", text: "tail" },
        ],
      },
      { kind: "bulletList", children: [item("nested")] },
    ],
  };
  const value: RichNode = {
    kind: "root",
    children: [{ kind: "bulletList", children: [first, item("outer-second")] }],
  };
  const converted: RichNode = {
    kind: "root",
    children: [
      { kind: "orderedList", children: [first, item("outer-second")] },
    ],
  };
  const { input, change, result, button } = mount(value);
  await selectText(input, 0, 3);
  await act(async () => fireEvent.click(button("orderedList")));
  expect(result()).toEqual(converted);
  expect(change).toHaveBeenLastCalledWith(converted);
  await act(async () => fireEvent.click(button("undo")));
  expect(result()).toEqual(value);
  await act(async () => fireEvent.click(button("redo")));
  expect(result()).toEqual(converted);
  change.mockClear();
  await act(async () => fireEvent.click(button("orderedList")));
  expect(result()).toEqual(converted);
  expect(change).not.toHaveBeenCalled();
});

it("quotes an outer list once without reprocessing its selected descendants", async () => {
  const value: RichNode = {
    kind: "root",
    children: [
      {
        kind: "bulletList",
        children: [
          {
            kind: "listItem",
            children: [
              paragraph("outer-first"),
              { kind: "bulletList", children: [item("nested")] },
            ],
          },
          item("outer-second"),
        ],
      },
    ],
  };
  const { input, change, result, button } = mount(value);
  await selectText(input, 0, 2);
  await act(async () => fireEvent.click(button("blockquote")));
  expect(result()).toEqual({
    kind: "root",
    children: [{ kind: "blockquote", children: value.children }],
  });
  expect(change).toHaveBeenLastCalledWith(result());
});
