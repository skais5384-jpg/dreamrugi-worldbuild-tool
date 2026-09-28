import { expect, it } from "vitest";
import { sharedItemStatus } from "./SvnItemStatus";
import type { SvnStatus, SvnStatusEntry } from "./svnClient";

const entry = (patch: Partial<SvnStatusEntry>): SvnStatusEntry => ({
  path: "templates/a.json",
  local: "normal",
  properties: "normal",
  remote: null,
  remoteProperties: null,
  lockOwner: null,
  wcLocked: false,
  workingCopyLocked: false,
  needsLock: true,
  remoteOnly: false,
  ...patch,
});
const status = (entries: SvnStatusEntry[]): SvnStatus => ({
  info: {
    root: "C:\\wc",
    wcRoot: "C:\\wc",
    url: "https://example.test/wc",
    repository: "https://example.test",
    revision: "3",
  },
  entries,
  serverRevision: "3",
  serverError: null,
  recovery: null,
  updateBlock: null,
});

it("maps only the actual shared backing path and preserves incoming state", () => {
  const current = status([
    entry({
      path: "C:\\wc\\templates\\a.json",
      remote: "modified",
      lockOwner: "another",
    }),
  ]);
  expect(sharedItemStatus(current, ["templates/a.json"])).toEqual({
    overlay: "needsLock",
    incoming: true,
    owner: "another",
  });
  expect(sharedItemStatus(current, ["templates/b.json"])).toBeNull();
});

it("never calls a remote-only or interrupted entry clean", () => {
  expect(
    sharedItemStatus(status([entry({ remoteOnly: true, remote: "added" })]), [
      "templates/a.json",
    ])?.overlay,
  ).toBe("unknown");
  const interrupted = status([entry({})]);
  interrupted.recovery = "cleanupRequired";
  expect(sharedItemStatus(interrupted, ["templates/a.json"])?.overlay).toBe(
    "unknown",
  );
});
