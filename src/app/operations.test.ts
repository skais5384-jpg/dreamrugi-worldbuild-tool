import { expect, it } from "vitest";
import { BridgeFailure } from "../bridge/client";
import { text } from "../strings";
import { safeFailure } from "./operations";

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
