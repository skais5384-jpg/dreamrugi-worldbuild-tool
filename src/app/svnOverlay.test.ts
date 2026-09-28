import { describe, expect, it } from "vitest";
import {
  dirtyCount,
  incomingChange,
  overlayForEntry,
  projectOverlay,
} from "./svnOverlay";
import type { SvnStatus, SvnStatusEntry } from "./svnClient";

const entry = (
  local: string,
  remote: string | null = null,
): SvnStatusEntry => ({
  path: `C:\\project\\${local}`,
  local,
  properties: "none",
  remote,
  remoteProperties: "none",
  lockOwner: null,
  wcLocked: false,
  workingCopyLocked: false,
  needsLock: false,
  remoteOnly: false,
});
const status = (
  entries: SvnStatusEntry[],
  serverError: string | null = null,
): SvnStatus => ({
  info: {
    root: "C:\\project",
    wcRoot: "C:\\project",
    url: "https://example.test/svn/project",
    repository: "https://example.test/svn",
    revision: "4",
  },
  entries,
  serverRevision: serverError ? null : "5",
  serverError,
  recovery: null,
  updateBlock: serverError ? "svn_server_unconfirmed" : null,
});
describe("TortoiseSVN project overlay", () => {
  it("keeps local overlay separate from incoming changes and repository revision", () => {
    const normalIncoming = status([entry("normal", "modified")]);
    expect(projectOverlay(normalIncoming)).toBe("normal");
    expect(incomingChange(normalIncoming)).toBe(true);
    const modifiedIncoming = status([entry("modified", "modified")]);
    expect(projectOverlay(modifiedIncoming)).toBe("modified");
    expect(incomingChange(modifiedIncoming)).toBe(true);
    const outsideCommit = status([entry("normal", null)]);
    expect(outsideCommit.serverRevision).toBe("5");
    expect(incomingChange(outsideCommit)).toBe(false);
  });
  it("uses conflicted ahead of modified and preserves property/lock causes", () => {
    expect(
      projectOverlay(
        status([entry("normal"), entry("modified"), entry("obstructed")]),
      ),
    ).toBe("conflicted");
    expect(
      overlayForEntry({
        ...entry("normal"),
        properties: "modified",
        needsLock: true,
      }),
    ).toBe("modified");
    expect(overlayForEntry({ ...entry("normal"), needsLock: true })).toBe(
      "needsLock",
    );
    expect(overlayForEntry({ ...entry("normal"), wcLocked: true })).toBe(
      "locked",
    );
    expect(
      overlayForEntry({
        ...entry("modified"),
        wcLocked: true,
        lockOwner: "finn001",
      }),
    ).toBe("modified");
    expect(
      overlayForEntry({
        ...entry("conflicted"),
        wcLocked: true,
        lockOwner: "finn001",
      }),
    ).toBe("conflicted");
    expect(
      projectOverlay(
        status([entry("normal"), { ...entry("normal"), wcLocked: true }]),
      ),
    ).toBe("locked");
    expect(overlayForEntry(entry("missing"))).toBe("deleted");
    expect(overlayForEntry(entry("unversioned"))).toBe("unversioned");
    expect(
      projectOverlay(status([entry("normal"), entry("unversioned")])),
    ).toBe("unversioned");
  });
  it("does not call an unobserved local state normal", () => {
    expect(projectOverlay(null)).toBe("unknown");
    expect(projectOverlay(status([]))).toBe("unknown");
  });
  it("separates server additions and incomplete recovery from local edits", () => {
    const remoteOnly = {
      ...entry("none", "added"),
      remoteOnly: true,
    };
    const receiving = status([entry("normal"), remoteOnly]);
    expect(incomingChange(receiving)).toBe(true);
    expect(dirtyCount(receiving)).toBe(0);
    expect(projectOverlay(receiving)).toBe("normal");
    expect(dirtyCount(status([entry("unversioned", "added")]))).toBe(1);

    const interrupted = status([
      entry("normal"),
      {
        ...entry("incomplete", "modified"),
        workingCopyLocked: true,
      },
    ]);
    interrupted.recovery = "cleanupRequired";
    interrupted.updateBlock = "svn_working_copy_cleanup_required";
    expect(projectOverlay(interrupted)).toBe("unknown");
    expect(dirtyCount(interrupted)).toBe(0);
    expect(incomingChange(interrupted)).toBe(true);
  });
});
