import type { SvnStatus, SvnStatusEntry } from "./svnClient";

// TortoiseSVN's overlay priority is shared by the project summary and future
// per-path displays. Incoming server changes are deliberately separate.
export type SvnOverlay =
  | "conflicted"
  | "modified"
  | "deleted"
  | "added"
  | "normal"
  | "needsLock"
  | "locked"
  | "ignored"
  | "unversioned"
  | "unknown";

export function overlayForEntry(entry: SvnStatusEntry): SvnOverlay {
  if (entry.workingCopyLocked || entry.local === "incomplete") return "unknown";
  switch (entry.local) {
    case "conflicted":
    case "obstructed":
      return "conflicted";
    case "modified":
    case "merged":
    case "replaced":
      return "modified";
    case "deleted":
    case "missing":
      return "deleted";
    case "added":
      return "added";
    case "normal":
      if (entry.properties && !["none", "normal"].includes(entry.properties))
        return "modified";
      if (entry.wcLocked) return "locked";
      if (entry.needsLock) return "needsLock";
      return "normal";
    case "ignored":
      return "ignored";
    case "unversioned":
      return "unversioned";
    default:
      return "unknown";
  }
}

const priority: SvnOverlay[] = [
  "conflicted",
  "modified",
  "deleted",
  "added",
  "locked",
  "unversioned",
  "normal",
  "needsLock",
  "ignored",
  "unknown",
];
export function projectOverlay(status: SvnStatus | null): SvnOverlay {
  if (!status || !status.entries.length) return "unknown";
  if (status.recovery) return "unknown";
  const overlays = status.entries.map(overlayForEntry);
  return priority.find((item) => overlays.includes(item)) ?? "normal";
}
export function incomingChange(status: SvnStatus): boolean {
  return status.entries.some((entry) =>
    [entry.remote, entry.remoteProperties].some(
      (value) => value != null && !["none", "normal"].includes(value),
    ),
  );
}
export function dirtyCount(status: SvnStatus): number {
  return status.entries.filter((entry) => {
    if (entry.remoteOnly && entry.local === "none" && entry.remote === "added")
      return false;
    // An interrupted update is reported through recovery, not local edits.
    if (entry.local === "incomplete") return false;
    if (
      entry.properties != null &&
      !["none", "normal"].includes(entry.properties)
    )
      return true;
    return !["normal", "ignored"].includes(entry.local);
  }).length;
}
