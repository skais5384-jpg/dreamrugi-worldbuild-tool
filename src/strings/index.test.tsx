import { render } from "../test/render";
import { screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import ko from "./ko.json";
import { formatResource, text, validateResources } from "./index";

afterEach(() => {
  vi.doUnmock("./ko.json");
  vi.resetModules();
});
it("전체 리소스의 형식·인자 누락을 개발 검사에서 거부하고 실행 조회는 안전하게 대체한다", () => {
  expect(() => validateResources(ko)).not.toThrow();
  for (const value of [
    null,
    12,
    "",
    "잘못된 {revision",
    "기준 {other}",
    "기준",
  ])
    expect(() => validateResources({ ...ko, "field.basis": value })).toThrow();
  expect(
    formatResource(
      { ...ko, "field.basis": "잘못된 {revision" },
      "field.basis",
      {},
    ),
  ).toBe(ko["resource.fallback"]);
  expect(
    formatResource(ko, "field.basis", { revision: 12 } as unknown as Record<
      string,
      string
    >),
  ).toBe(ko["resource.fallback"]);
});
it("사용자 이름의 markup과 토큰 문자열을 literal text로 한 번만 치환한다", () => {
  const name = "<img src=x onerror=alert(1)> {revision}";
  render(<p>{text("template.target", { name, revision: "7" })}</p>);
  expect(screen.getByText(`대상: ${name} · 기준 버전 7`)).toBeInTheDocument();
  expect(document.querySelector("img")).toBeNull();
});
it("이름 있는 인자와 키를 검사하고 내부 값 대신 안전한 fallback을 표시한다", () => {
  expect(text("field.basis", { revision: "9007199254740993" })).toBe(
    "편집 기준 버전 9007199254740993",
  );
  expect(formatResource(ko, "missing", {})).toBe(ko["resource.fallback"]);
  expect(formatResource(ko, "field.basis", {})).toBe(ko["resource.fallback"]);
  expect(
    formatResource(ko, "field.basis", { revision: "1", secret: "RAW_CANARY" }),
  ).not.toContain("RAW_CANARY");
  expect(
    formatResource(ko, "field.basis", {
      revision: "<script>alert(1)</script>",
    }),
  ).toContain("<script>");
  // @ts-expect-error 잘못된 키를 compile 단계에서도 차단한다.
  text("unknown.key");
  // @ts-expect-error 치환 인자는 revision이라는 이름으로 필요하다.
  text("field.basis", { value: "1" });
});
it("JSON의 문구값 하나만 바꿔 실제 Field 버튼과 접근성 이름에 반영한다", async () => {
  const changed =
    "새로운 Field 정의를 추가하고 속성별로 저장하기 — 긴 리소스 문구 확인";
  vi.doMock("./ko.json", () => ({ default: { ...ko, "field.add": changed } }));
  vi.resetModules();
  const { Fields } = await import("../app/Fields");
  const { TemplateController } = await import("../app/controller");
  const { GuardedClient } = await import("../bridge/client");
  const { TestTransport } = await import("../app/testTransport");
  const transport = new TestTransport();
  const controller = new TemplateController(new GuardedClient(transport));
  controller.start();
  await waitFor(() => expect(controller.snapshot().ready).toBe(true));
  await controller.checkStatus();
  controller.setRoot("fixture");
  await controller.open();
  await controller.navigate({ kind: "create" });
  controller.setName("test");
  await controller.save();
  render(<Fields controller={controller} locked={false} canEdit />);
  expect(screen.getByRole("button", { name: changed })).toBeInTheDocument();
});
