import { fireEvent, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { render } from "../test/render";
import { FollowUp } from "./FollowUp";
import type { TemplateController } from "./controller";
import { text } from "../strings";

it("closing details preserves the unresolved action and verified completion removes it", () => {
  const state = {
    operations: [],
    sessions: [],
    retainedRefs: [],
    retained: [],
    busy: false,
    project: {
      shutdown: {
        reportPending: false,
        reports: [],
        forceActive: true,
        forceResults: [],
        blockers: ["ForceTasks"],
      },
    },
    app: { native_cleanup: { phase: "idle" } },
  };
  const checkStatus = vi.fn();
  const controller = {
    snapshot: () => state,
    checkStatus,
  } as unknown as TemplateController;
  const view = render(<FollowUp controller={controller} />);
  fireEvent.click(
    screen.getByRole("button", { name: text("followUp.openActions") }),
  );
  const dialog = screen.getByRole("dialog");
  expect(
    within(dialog)
      .getAllByRole("button")
      .map((button) => button.textContent),
  ).toEqual([text("followUp.message06"), text("common.close")]);
  fireEvent.click(
    within(dialog).getByRole("button", { name: text("followUp.message06") }),
  );
  expect(checkStatus).toHaveBeenCalledOnce();
  fireEvent.click(
    within(dialog).getByRole("button", { name: text("common.close") }),
  );
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(
    screen.getByRole("button", { name: text("followUp.openActions") }),
  ).toBeVisible();
  state.project.shutdown.forceActive = false;
  state.project.shutdown.blockers = [];
  Object.assign(state.project.shutdown, {
    forceResults: [{ document: "owned", outcome: "verified", error: null }],
  });
  view.rerender(<FollowUp controller={controller} />);
  expect(
    screen.queryByRole("button", { name: text("followUp.openActions") }),
  ).toBeNull();
  Object.assign(state.project.shutdown, {
    forceResults: [
      { document: "owned", outcome: "unknown", error: "svn_unavailable" },
    ],
  });
  view.rerender(<FollowUp controller={controller} />);
  expect(
    screen.getByRole("button", { name: text("followUp.openActions") }),
  ).toBeVisible();
});
