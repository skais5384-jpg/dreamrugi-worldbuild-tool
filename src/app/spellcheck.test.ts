import { describe, expect, it } from "vitest";
import {
  exceptionRanges,
  glossaryTerms,
  tokens,
  type SpellSurface,
} from "./spellcheck";
import type { DocumentList } from "../bridge/documents";

describe("local spelling boundaries", () => {
  it("protects complete glossary names with known particles without hiding adjacent errors", () => {
    const surface: SpellSurface = {
      key: "body",
      label: "본문",
      kind: "field",
      text: "아린은 됬다. 고요한 항구에서 좋읍니다. 마린은 왔습니다.",
    };
    expect(
      tokens([surface], ["아린", "고요한 항구"]).map((item) => item.word),
    ).toEqual(["됬다", "좋읍니다", "마린은", "왔습니다"]);
    expect(exceptionRanges("아린됬다", ["아린"])).toEqual([]);
  });

  it("maps NFC words back to the exact UTF-16 source range after an emoji", () => {
    const decomposed = "좋읍니다".normalize("NFD");
    const surface: SpellSurface = {
      key: "x",
      label: "본문",
      kind: "field",
      text: `😀 ${decomposed}`,
    };
    expect(tokens([surface], [])).toEqual([
      {
        surface,
        start: 3,
        end: 3 + decomposed.length,
        word: "좋읍니다",
        original: decomposed,
      },
    ]);
  });

  it("uses the full active layout and template set rather than visible glossary rows", () => {
    const list = {
      problem: null,
      layout: {
        nodes: {
          a: { state: "active" },
          b: { state: "trashed" },
          c: { state: "active" },
        },
      },
      documents: [
        {
          id: "a",
          template: "t",
          name: "고요한 항구",
          englishName: "Quiet Harbor",
          glossaryExcluded: false,
        },
        {
          id: "b",
          template: "t",
          name: "버려진 이름",
          glossaryExcluded: false,
        },
        {
          id: "c",
          template: "x",
          name: "제외된 분류",
          glossaryExcluded: false,
        },
      ],
    } as unknown as DocumentList;
    expect(
      glossaryTerms(list, [
        {
          id: "t",
          name: "장소",
          revision: "1",
          lifecycle: "Active",
          glossaryExcluded: false,
        },
        {
          id: "x",
          name: "미표시",
          revision: "1",
          lifecycle: "Active",
          glossaryExcluded: true,
        },
      ]),
    ).toEqual(["Quiet Harbor", "고요한 항구"]);
  });
});
