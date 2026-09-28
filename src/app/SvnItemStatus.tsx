import { Tooltip } from "@fluentui/react-components";
import type { SvnStatus, SvnStatusEntry } from "./svnClient";
import { overlayForEntry, type SvnOverlay } from "./svnOverlay";
import { OverlayIcon, overlayNames } from "./SvnToolbar";

const rank: SvnOverlay[] = [
  "conflicted",
  "modified",
  "deleted",
  "added",
  "locked",
  "unversioned",
  "unknown",
  "needsLock",
  "normal",
  "ignored",
];

export function sharedItemStatus(
  status: SvnStatus | null,
  paths: readonly string[],
): { overlay: SvnOverlay; incoming: boolean; owner: string | null } | null {
  if (!status) return null;
  const entries = status.entries.filter((entry) =>
    paths.some((path) => {
      const observed = entry.path.replace(/\\/gu, "/");
      return observed === path || observed.endsWith(`/${path}`);
    }),
  );
  if (!entries.length) return null;
  const overlays = entries.map((entry) =>
    entry.remoteOnly ? "unknown" : overlayForEntry(entry),
  );
  return {
    overlay: status.recovery
      ? "unknown"
      : (rank.find((value) => overlays.includes(value)) ?? "unknown"),
    incoming: entries.some(hasIncoming),
    owner: entries.find((entry) => entry.lockOwner)?.lockOwner ?? null,
  };
}

function hasIncoming(entry: SvnStatusEntry): boolean {
  return [entry.remote, entry.remoteProperties].some(
    (value) => value != null && !["none", "normal"].includes(value),
  );
}

export function SvnItemStatus({
  status,
  paths,
}: {
  status: SvnStatus | null;
  paths: readonly string[];
}) {
  const result = sharedItemStatus(status, paths);
  if (!result) return null;
  const description = [
    overlayNames[result.overlay],
    result.incoming && "서버 변경 있음",
    result.owner && `잠금 소유자: ${result.owner}`,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <Tooltip content={description} relationship="description">
      <span className="svn-item-status" role="img" aria-label={description}>
        <OverlayIcon overlay={result.overlay} />
        {result.incoming && (
          <span className="svn-item-incoming" aria-hidden="true" />
        )}
      </span>
    </Tooltip>
  );
}
