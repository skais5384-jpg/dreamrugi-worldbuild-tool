import { describe, expect, it } from "vitest";
import { formatLocalDateTime } from "./localDateTime";

describe("formatLocalDateTime", () => {
  it("UTC 저장 시각을 현재 실행 환경의 locale과 시간대로 표시한다", () => {
    const value = "2026-09-21T00:00:00.000Z";
    expect(formatLocalDateTime(value)).toBe(
      new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "medium",
      }).format(new Date(value)),
    );
  });

  it("해석할 수 없는 기존 값은 숨기지 않는다", () => {
    expect(formatLocalDateTime("unknown-time")).toBe("unknown-time");
    expect(formatLocalDateTime(null)).toBe("—");
  });
});
