import { fireEvent, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { render } from "../test/render";
import { ActivityLog, type ActivityEvent } from "./ActivityLog";

it("shows a short event summary and keeps diagnostic fields in row details", () => {
  const event: ActivityEvent = {
    sessionId: "session",
    observedAtUtc: "2026-09-27T00:00:00Z",
    feature: "svn",
    stage: "commit",
    category: "svn_commit_result_uncertain",
    outcome: "uncertain",
    correlation: null,
    projectFingerprint: null,
  };
  render(
    <ActivityLog events={[event]} droppedEvents={0} close={() => undefined} />,
  );
  expect(screen.getByRole("table")).toBeVisible();
  expect(screen.getByText("주의")).toBeVisible();
  expect(screen.getByText("작업 결과를 확인해 주세요")).toBeVisible();
  expect(screen.getByText("svn_commit_result_uncertain")).not.toBeVisible();
  fireEvent.click(screen.getByText("작업 결과를 확인해 주세요"));
  expect(screen.getByText("svn_commit_result_uncertain")).toBeVisible();
});
