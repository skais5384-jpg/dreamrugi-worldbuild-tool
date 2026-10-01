import { expect, it } from "vitest";
import { BridgeFailure } from "../bridge/client";
import { text } from "../strings";
import { safeFailure } from "./operations";

it("keeps native boundary codes and untrusted diagnostics out of the primary action guidance", () => {
  const failure = new BridgeFailure("boundary", "local-test", {
    code: "save_rejected",
    nextAction: "untrusted-secret-detail",
  });
  expect(safeFailure(failure)).toBe(text("error.reviewRetry"));
  expect(safeFailure(failure)).not.toContain("save_rejected");
  expect(safeFailure(failure)).not.toContain(failure.boundary!.nextAction);
  expect(failure.boundary!.code).toBe("save_rejected");
});

it.each([
  ["session_rejected", "StaleLockToken", "documentEdit.staleLockToken"],
  ["session_rejected", "AlreadyLocked", "documentEdit.entryRejected"],
  ["recovery_store_busy", "StaleLockToken", "documentEdit.recoveryStoreBusy"],
  ["sink_unavailable", "StaleLockToken", "documentEdit.recoveryUnavailable"],
] as const)(
  "preserves confirmed failure cause %s / %s",
  (code, category, key) => {
    const failure = new BridgeFailure("boundary", "test", {
      code,
      nextAction: "untrusted technical detail",
      diagnostic: {
        stage: "Acquire",
        category,
        outcome: "rejected",
        cleanupOutcome: "released",
      },
    });
    expect(safeFailure(failure)).toBe(text(key));
    expect(safeFailure(failure)).not.toContain(failure.boundary!.nextAction);
  },
);
