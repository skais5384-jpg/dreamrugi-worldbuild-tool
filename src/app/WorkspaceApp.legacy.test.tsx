import { text } from "../strings";
import { invoke } from "@tauri-apps/api/core";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { GuardedClient } from "../bridge/client";
import { TemplateController } from "./controller";
import WorkspaceApp from "./WorkspaceApp";
import { WorkspaceController } from "./workspaceController";
import { transportFixture } from "./WorkspaceApp.testHarness";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (name) => {
    if (name === "diagnostic_recent_events")
      return { events: [], droppedEvents: 0 };
    if (name === "legacy_handoff_status")
      return { state: "failed", summary: null, error: null };
    if (name === "legacy_handoff_retry")
      return {
        state: "preserved",
        summary: {
          found: 1,
          imported: 1,
          alreadyPresent: 0,
          needsAttention: 0,
        },
        error: null,
      };
    throw new Error(`unexpected command: ${name}`);
  });
});

it("홈에서 구 보관본 인계 실패를 숨기지 않고 재확인 뒤 경고를 해소한다", async () => {
  const fixture = transportFixture();
  const shell = new TemplateController(
    new GuardedClient(fixture.transport),
    vi.fn(),
  );
  render(<WorkspaceApp controller={new WorkspaceController(shell)} />);

  expect(
    await screen.findByText(
      /이전 설치본의 보관 입력을 모두 외부에 확인하지 못했습니다/,
    ),
  ).toBeVisible();
  expect(screen.getByText(/이전 설치본을 제거하지 말고/)).toBeVisible();
  expect(
    screen.getByRole("button", { name: text("update.details") }),
  ).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: text("update.details") }));
  fireEvent.click(await screen.findByRole("button", { name: "보관 재확인" }));
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: "보관 재확인" })).toBeNull(),
  );
  expect(
    within(screen.getByRole("table")).getAllByText(
      /이전 설치본을 제거하지 말고/,
    ).length,
  ).toBeGreaterThan(0);
  fireEvent.click(
    within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }),
  );
  await waitFor(() =>
    expect(screen.queryByText(/이전 설치본을 제거하지 말고/)).toBeNull(),
  );
  expect(invoke).toHaveBeenCalledWith("legacy_handoff_retry");
});
