import { describe, expect, it } from "vitest";
import { documentPlacement } from "./documentPlacement";
import type { Layout } from "../bridge/documents";
const layout: Layout = {
  revision: 1,
  rootOrder: ["a", "b", "c"],
  nodes: {
    a: { parentId: null, childOrder: ["d"], state: "active", trash: null },
    b: { parentId: null, childOrder: [], state: "active", trash: null },
    c: { parentId: null, childOrder: [], state: "active", trash: null },
    d: { parentId: "a", childOrder: [], state: "active", trash: null },
    x: { parentId: null, childOrder: [], state: "trashed", trash: null },
  },
};
describe("document placement", () => {
  it("같은 부모의 앞/뒤 이동은 제거 후 순서로 계산하며 원본을 바꾸지 않는다", () => {
    const before = JSON.stringify(layout);
    expect(documentPlacement(layout, "a", "c", "after")).toEqual({
      kind: "move",
      document: "a",
      parent: null,
      index: 2,
    });
    expect(documentPlacement(layout, "c", "a", "before")).toEqual({
      kind: "move",
      document: "c",
      parent: null,
      index: 0,
    });
    expect(documentPlacement(layout, "a", "b", "before")).toEqual({
      kind: "move",
      document: "a",
      parent: null,
      index: 0,
    });
    expect(JSON.stringify(layout)).toBe(before);
  });
  it("하위/루트 이동을 계산하고 자기 자신·자손·삭제된 대상을 거절한다", () => {
    expect(documentPlacement(layout, "b", "a", "inside")).toEqual({
      kind: "move",
      document: "b",
      parent: "a",
      index: 1,
    });
    expect(documentPlacement(layout, "d", null, "inside")).toEqual({
      kind: "move",
      document: "d",
      parent: null,
      index: 3,
    });
    for (const target of ["a", "d", "x", "missing"])
      expect(documentPlacement(layout, "a", target, "inside")).toBeNull();
    expect(documentPlacement(layout, "a", "d", "before")).toBeNull();
  });
});
