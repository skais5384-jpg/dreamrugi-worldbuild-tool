import { describe, expect, it } from "vitest";
import { createEditor } from "lexical";
import { $exportRich, $installRich, richHasText, richNodes } from "./adapter";
import type { RichNode } from "../../bridge/types";

describe("project rich AST", () => {
  it("preserves all supported blocks, nested lists, empty paragraphs, breaks and marks", () => {
    const paragraph: RichNode = {
      kind: "paragraph",
      children: [
        {
          kind: "text",
          text: "한글  ",
          marks: ["bold", "italic", "underline", "strikethrough"],
        },
        { kind: "hardBreak" },
        { kind: "text", text: "끝" },
      ],
    };
    const root: RichNode = {
      kind: "root",
      children: [
        paragraph,
        { kind: "paragraph", children: [] },
        ...[1, 2, 3, 4, 5, 6].map((level) => ({
          kind: "heading" as const,
          level,
          children: [{ kind: "text" as const, text: "제목" }],
        })),
        {
          kind: "blockquote",
          children: [
            paragraph,
            {
              kind: "bulletList",
              children: [
                {
                  kind: "listItem",
                  children: [
                    paragraph,
                    {
                      kind: "orderedList",
                      children: [{ kind: "listItem", children: [paragraph] }],
                    },
                  ],
                },
              ],
            },
          ],
        },
        {
          kind: "taskList",
          children: [false, true].map((checked) => ({
            kind: "taskItem",
            checked,
            children: [paragraph],
          })),
        },
      ],
    };
    const editor = createEditor({
      namespace: "roundtrip",
      nodes: richNodes,
      onError: (e) => {
        throw e;
      },
    });
    editor.update(() => $installRich(root), { discrete: true });
    expect(editor.getEditorState().read($exportRich)).toEqual(root);
    const json = editor.getEditorState().toJSON();
    editor.setEditorState(editor.parseEditorState(json));
    expect(editor.getEditorState().read($exportRich)).toEqual(root);
  });
  it("distinguishes whitespace text from structure-only empty content", () => {
    expect(
      richHasText({
        kind: "heading",
        level: 2,
        children: [{ kind: "hardBreak" }],
      }),
    ).toBe(false);
    expect(richHasText({ kind: "text", text: " " })).toBe(true);
  });
});
