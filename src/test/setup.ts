import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";

function hasHiddenAncestor(element: HTMLElement | null): boolean {
  return (
    !!element &&
    (getComputedStyle(element).display === "none" ||
      hasHiddenAncestor(element.parentElement))
  );
}

beforeEach(() => {
  // jsdom에는 layout이 없어 Tabster가 body를 숨은 iframe으로 판정한다.
  // 가시성 판정에 필요한 크기/부모만 제공하며 실제 배치·DPI는 Windows에서 별도로 검증한다.
  vi.spyOn(document.body, "getBoundingClientRect").mockReturnValue(
    new DOMRect(0, 0, 1024, 640),
  );
  vi.spyOn(HTMLElement.prototype, "offsetParent", "get").mockImplementation(
    function (this: HTMLElement) {
      if (!this.isConnected || this.closest("[hidden], [inert]")) return null;
      if (hasHiddenAncestor(this)) return null;
      return this.parentElement;
    },
  );
});

// 테스트 사이에 렌더링된 화면을 정리해 서로의 결과가 영향을 주지 않게 한다.
afterEach(() => {
  try {
    cleanup();
  } finally {
    try {
      vi.useRealTimers();
    } finally {
      vi.restoreAllMocks();
    }
  }
});
