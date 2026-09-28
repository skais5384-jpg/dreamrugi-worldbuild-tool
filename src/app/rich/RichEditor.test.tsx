import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import {
  $getRoot,
  $isElementNode,
  $getSelection,
  $isRangeSelection,
  $setSelection,
  getNearestEditorFromDOMNode,
} from "lexical";
import type { RichNode } from "../../bridge/types";
import { render } from "../../test/render";
import { RichEditor } from "./RichEditor";
import type { RichSpellRequest } from "./RichEditor";
import { $exportRich } from "./adapter";
import { text } from "../../strings";
import { DocumentPositions } from "../documentPosition";
afterEach(cleanup);
it("applies a checked spelling range across rich runs without replacing the editor root", async () => {
  const change = vi.fn();
  render(
    <RichEditor
      id="spell-rich"
      label="맞춤법 본문"
      disabled={false}
      invalid={false}
      change={change}
      value={{
        kind: "root",
        children: [
          {
            kind: "paragraph",
            children: [
              { kind: "text", text: "앞 😀 " },
              { kind: "text", text: "좋읍", marks: ["bold"] },
              { kind: "text", text: "니다" },
              { kind: "text", text: " 뒤" },
            ],
          },
        ],
      }}
    />,
  );
  const input = screen.getByRole("textbox", { name: "맞춤법 본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  let accepted = false;
  await act(async () => {
    await new Promise<void>((resolve) => {
      const request: RichSpellRequest = {
        active: () => true,
        patches: [
          {
            block: 0,
            start: 5,
            end: 9,
            expected: "좋읍니다",
            replacement: "좋습니다",
          },
        ],
        resolve: (result) => {
          accepted = result;
          resolve();
        },
      };
      input.dispatchEvent(
        new CustomEvent("spell-replace", { detail: request }),
      );
    });
  });
  expect(accepted).toBe(true);
  const result = editor.getEditorState().read($exportRich);
  expect(
    result.kind === "root" &&
      result.children[0].kind === "paragraph" &&
      result.children[0].children
        .map((node) => (node.kind === "text" ? node.text : ""))
        .join(""),
  ).toBe("앞 😀 좋습니다 뒤");
  expect(change).toHaveBeenCalled();
});
it("icon toolbar follows caret formats and block ancestors without changing the document", async () => {
  const change = vi.fn();
  render(
    <RichEditor
      id="toolbar"
      label="서식 위치"
      disabled={false}
      invalid={false}
      change={change}
      value={{
        kind: "root",
        children: [
          { kind: "paragraph", children: [{ kind: "text", text: "일반" }] },
          {
            kind: "blockquote",
            children: [
              {
                kind: "heading",
                level: 2,
                children: [
                  {
                    kind: "text",
                    text: "인용 제목",
                    marks: ["bold", "italic"],
                  },
                ],
              },
            ],
          },
          {
            kind: "taskList",
            children: [
              {
                kind: "taskItem",
                checked: true,
                children: [
                  {
                    kind: "paragraph",
                    children: [{ kind: "text", text: "완료" }],
                  },
                ],
              },
            ],
          },
        ],
      }}
    />,
  );
  const button = (
    name:
      | "bold"
      | "italic"
      | "paragraph"
      | "heading"
      | "blockquote"
      | "taskList"
      | "toggleTask",
  ) => screen.getByRole("button", { name: text(`rich.${name}`) });
  const input = screen.getByRole("textbox", { name: "서식 위치" });
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => new DOMRect(),
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  const select = async (index: number) =>
    act(async () => {
      input.focus();
      const walker = document.createTreeWalker(
        input.children[index],
        NodeFilter.SHOW_TEXT,
      );
      const node = walker.nextNode()!;
      window.getSelection()!.setBaseAndExtent(node, 1, node, 1);
      fireEvent(document, new Event("selectionchange"));
    });
  await select(1);
  for (const name of ["bold", "italic", "heading", "blockquote"] as const)
    expect(button(name)).toHaveAttribute("aria-pressed", "true");
  expect(button("paragraph")).toHaveAttribute("aria-pressed", "false");
  expect(button("bold").textContent).toBe("");
  expect(button("bold").querySelector("svg")).not.toBeNull();
  await select(2);
  expect(button("taskList")).toHaveAttribute("aria-pressed", "true");
  expect(button("toggleTask")).toHaveAttribute("aria-pressed", "true");
  expect(button("blockquote")).toHaveAttribute("aria-pressed", "false");
  await select(0);
  expect(button("paragraph")).toHaveAttribute("aria-pressed", "true");
  expect(button("italic")).toHaveAttribute("aria-pressed", "false");
  expect(button("toggleTask")).toBeDisabled();
  expect(change).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(button("heading")));
  const editor = getNearestEditorFromDOMNode(input)!;
  const result = editor.getEditorState().read($exportRich);
  expect(result.kind === "root" && result.children[0]).toEqual({
    kind: "heading",
    level: 3,
    children: [{ kind: "text", text: "일반" }],
  });
  expect(input.querySelector("h3")?.textContent).toBe("일반");
  // 새 제목은 H3이며 기존 저장된 제목 단계는 자동 변환하지 않는다.
  expect(input.querySelector("blockquote h2")?.textContent).toBe("인용 제목");
  expect(button("heading")).toHaveAttribute("aria-pressed", "true");
});
it("restores rich caret after tab blur without changing content or composition", async () => {
  Object.defineProperty(Range.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => new DOMRect(),
  });
  Object.defineProperty(Range.prototype, "getClientRects", {
    configurable: true,
    value: () => [],
  });
  const change = vi.fn();
  const view = render(
    <div>
      <RichEditor
        id="position"
        label="위치 본문"
        value={{
          kind: "root",
          children: [
            {
              kind: "paragraph",
              children: [{ kind: "text", text: "앞뒤 문장" }],
            },
          ],
        }}
        disabled={false}
        invalid={false}
        change={change}
      />
      <button>다른 탭</button>
    </div>,
  );
  const input = screen.getByRole("textbox", { name: "위치 본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  const body = view.container;
  const positions = new DocumentPositions();
  await act(async () => {
    editor.update(
      () => {
        const selection = $getRoot().selectStart();
        selection.anchor.offset = 1;
        selection.focus.offset = 1;
      },
      { discrete: true },
    );
  });
  body.scrollTop = 180;
  positions.remember("a", input, body);
  await act(async () => {
    screen.getByRole("button", { name: "다른 탭" }).focus();
    editor.update(() => $setSelection(null), { discrete: true });
  });
  body.scrollTop = 0;
  await act(async () => positions.restore("a", body));
  expect(document.activeElement).toBe(input);
  expect(body.scrollTop).toBe(180);
  expect(editor.isComposing()).toBe(false);
  expect(change).not.toHaveBeenCalled();
  expect(
    editor.getEditorState().read(() => {
      const selection = $getSelection();
      return $isRangeSelection(selection) && selection.anchor.offset;
    }),
  ).toBe(1);
  fireEvent.paste(input, {
    clipboardData: {
      getData: (type: string) => (type === "text/plain" ? "삽입" : ""),
    },
  });
  await act(async () => {});
  expect(input).toHaveTextContent("앞삽입뒤 문장");
});
it("plain paste, toolbar selection, format undo/redo and same-owner rerender preserve content", async () => {
  const change = vi.fn();
  const value = {
    kind: "root" as const,
    children: [
      {
        kind: "paragraph" as const,
        children: [{ kind: "text" as const, text: "원문" }],
      },
    ],
  };

  const view = render(
    <RichEditor
      id="r"
      label="본문"
      value={value}
      change={change}
      disabled={false}
      invalid={false}
    />,
  );
  const input = screen.getByRole("textbox", { name: "본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  const select = async () =>
    act(async () => {
      editor.update(() => $getRoot().selectEnd(), { discrete: true });
    });

  await select();
  expect(change).not.toHaveBeenCalled();

  fireEvent.paste(input, {
    clipboardData: {
      types: ["text/plain", "text/html"],
      getData: (type: string) =>
        type === "text/plain"
          ? "붙임"
          : "<a href='https://invalid.test'>HTML</a>",
    },
  });
  await act(async () => {});
  expect(input).toHaveTextContent("원문붙임");
  expect(input.querySelector("a")).toBeNull();
  await act(async () => {
    editor.update(() => $getRoot().select(0, $getRoot().getChildrenSize()), {
      discrete: true,
    });
  });
  fireEvent.mouseDown(screen.getByRole("button", { name: text("rich.bold") }));
  fireEvent.click(screen.getByRole("button", { name: text("rich.bold") }));
  await act(async () => {});
  expect(editor.getEditorState().read($exportRich)).toMatchObject({
    children: [{ children: [{ marks: ["bold"] }] }],
  });
  fireEvent.click(screen.getByRole("button", { name: text("rich.undo") }));
  await act(async () => {});
  expect(editor.getEditorState().read($exportRich)).not.toMatchObject({
    children: [{ children: [{ marks: ["bold"] }] }],
  });
  fireEvent.click(screen.getByRole("button", { name: text("rich.redo") }));
  await act(async () => {});
  const before = editor.getEditorState().read($exportRich);
  view.rerender(
    <RichEditor
      id="r"
      label="본문"
      value={value}
      change={change}
      disabled={false}
      invalid={false}
    />,
  );
  expect(editor.getEditorState().read($exportRich)).toEqual(before);
  const calls = change.mock.calls.length;

  fireEvent.paste(input, {
    clipboardData: { types: ["text/html"], getData: () => "" },
  });
  await act(async () => {});
  expect(change.mock.calls.length).toBe(calls);
  expect(screen.getByRole("status")).toHaveTextContent(text("rich.plainOnly"));
  fireEvent.drop(input, {
    dataTransfer: { types: ["Files"], getData: () => "" },
  });
  expect(editor.getEditorState().read($exportRich)).toEqual(before);
});

it("list Enter and Backspace preserve sibling order", async () => {
  const paragraph = (value: string): RichNode => ({
    kind: "paragraph",
    children: value ? [{ kind: "text", text: value }] : [],
  });
  const value: RichNode = {
    kind: "root",
    children: [
      {
        kind: "bulletList",
        children: [
          { kind: "listItem", children: [paragraph("첫 항목")] },
          { kind: "listItem", children: [paragraph("다음 항목")] },
        ],
      },
    ],
  };
  const change = vi.fn();
  render(
    <RichEditor
      id="list"
      label="목록 본문"
      value={value}
      change={change}
      disabled={false}
      invalid={false}
    />,
  );
  const input = screen.getByRole("textbox", { name: "목록 본문" });
  const editor = getNearestEditorFromDOMNode(input)!;
  await act(async () => {
    editor.update(
      () => {
        const list = $getRoot().getFirstChild();
        const item = $isElementNode(list) ? list.getFirstChild() : null;
        const p = $isElementNode(item) ? item.getFirstChild() : null;
        if ($isElementNode(p)) p.selectEnd();
      },
      { discrete: true },
    );
  });
  fireEvent.keyDown(input, {
    key: "Enter",
    code: "Enter",
    keyCode: 13,
    which: 13,
  });
  await act(async () => {});
  expect(editor.getEditorState().read($exportRich)).toEqual({
    kind: "root",
    children: [
      {
        kind: "bulletList",
        children: [
          { kind: "listItem", children: [paragraph("첫 항목")] },
          { kind: "listItem", children: [paragraph("")] },
          { kind: "listItem", children: [paragraph("다음 항목")] },
        ],
      },
    ],
  });
  fireEvent.keyDown(input, {
    key: "Backspace",
    code: "Backspace",
    keyCode: 8,
    which: 8,
  });
  await act(async () => {});
  expect(editor.getEditorState().read($exportRich)).toEqual(value);
});
