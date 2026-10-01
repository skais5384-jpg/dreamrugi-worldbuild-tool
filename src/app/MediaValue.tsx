import { createContext, useContext, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogContent,
  DialogActions,
  Tooltip,
} from "@fluentui/react-components";
import {
  Add20Regular,
  Delete20Regular,
  Open20Regular,
  ZoomIn20Regular,
  ReOrderDotsVertical20Regular,
} from "@fluentui/react-icons";
import { Button } from "../ui/Controls";
import { HelpText } from "../ui/HelpText";
import { InlineNotice } from "../ui/InlineNotice";
import type { DocumentController } from "./documentController";
import type { AssetMetadata } from "../bridge/documents";
import { text } from "../strings";
import { useDraftReorder } from "./useDraftReorder";
import "./MediaValue.css";

export const MediaContext = createContext<Pick<
  DocumentController,
  "media" | "pollProgress"
> | null>(null);
// 편집 DOM은 유지하되 숨은 화면의 플레이어만 해제한다.
export const MediaActiveContext = createContext(true);
export const MediaPreviewContext = createContext(true);
const YOUTUBE_APP_ORIGIN = "https://com.dreamrugi.worldbuildtool";
function mediaUrl(raw: string): URL | null {
  if (
    raw.length > 4096 ||
    raw.trim() !== raw ||
    Array.from(raw).some(
      (c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127 || c === "\\",
    )
  )
    return null;
  try {
    const u = new URL(raw);
    if (
      u.protocol !== "https:" ||
      u.username ||
      u.password ||
      u.port ||
      raw.includes("#") ||
      !u.hostname.includes(".") ||
      u.hostname.endsWith(".") ||
      /(?:\.local|\.localhost)$/.test(u.hostname) ||
      /^[\d.]+$/.test(u.hostname) ||
      u.hostname.startsWith("[")
    )
      return null;
    return u;
  } catch {
    return null;
  }
}
function youtubeStart(u: URL): number | null | undefined {
  const values = [...u.searchParams]
    .filter(([key]) => key === "t" || key === "start")
    .map(([, value]) => value);
  if (!values.length) return undefined;
  if (values.length !== 1) return null;
  const raw = values[0];
  if (/^\d+$/.test(raw)) {
    const seconds = Number(raw);
    return Number.isSafeInteger(seconds) && seconds <= 86_400 ? seconds : null;
  }
  const match = /^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$/.exec(raw);
  if (!match || !match.slice(1).some(Boolean)) return null;
  const seconds =
    Number(match[1] ?? 0) * 3600 +
    Number(match[2] ?? 0) * 60 +
    Number(match[3] ?? 0);
  return Number.isSafeInteger(seconds) && seconds <= 86_400 ? seconds : null;
}
function youtubeId(u: URL): string | null {
  let id: string | null = null;
  const path = u.pathname.split("/").filter(Boolean);
  if (u.hostname === "youtu.be" && path.length === 1) id = path[0];
  else if (
    ["youtube.com", "www.youtube.com", "m.youtube.com"].includes(u.hostname)
  ) {
    if (path.length === 1 && path[0] === "watch") {
      const values = u.searchParams.getAll("v");
      if (values.length === 1) id = values[0];
    } else if (
      path.length === 2 &&
      (path[0] === "shorts" || path[0] === "embed")
    )
      id = path[1];
  }
  return id && /^[A-Za-z0-9_-]{11}$/.test(id) ? id : null;
}
export function youtubeEmbed(raw: string): string | null {
  const u = mediaUrl(raw);
  if (!u) return null;
  const id = youtubeId(u);
  const start = youtubeStart(u);
  if (!id || start === null) return null;
  const embed = new URL(`https://www.youtube.com/embed/${id}`);
  embed.searchParams.set("enablejsapi", "1");
  embed.searchParams.set("playsinline", "1");
  embed.searchParams.set("origin", YOUTUBE_APP_ORIGIN);
  if (start) embed.searchParams.set("start", String(start));
  return embed.toString();
}
export function mediaKind(raw: string): "image" | "video" | "youtube" | null {
  const u = mediaUrl(raw);
  if (!u) return null;
  if (youtubeEmbed(raw)) return "youtube";
  if (/\.(?:png|apng|jpe?g|jpe|jfif|webp|gif|bmp)$/i.test(u.pathname))
    return "image";
  if (/\.(?:mp4|webm)$/i.test(u.pathname)) return "video";
  return null;
}
export function UrlRead({ value }: { value: string }) {
  const active = useContext(MediaActiveContext);
  const kind = mediaKind(value);
  // 정지 이미지의 크기/로드 상태는 편집 DOM과 함께 보존한다.
  const player = kind === "video" || kind === "youtube";
  return !player || active ? <UrlAttempt key={value} value={value} /> : null;
}
function UrlAttempt({ value }: { value: string }) {
  const controller = useContext(MediaContext);
  // URL을 떠나거나 화면이 숨겨지면 이 요청 인스턴스도 끝난다.
  const [failed, setFailed] = useState(false);
  const kind = mediaKind(value);
  const youtube = youtubeEmbed(value);
  return (
    <div className="media-url">
      {kind === "image" ? (
        <img
          key={value}
          src={value}
          alt={text("field.image")}
          referrerPolicy="no-referrer"
          onError={() => setFailed(true)}
          onLoad={() => setFailed(false)}
        />
      ) : kind === "video" ? (
        <video
          key={value}
          src={value}
          controls
          preload="metadata"
          onError={() => setFailed(true)}
          onCanPlay={() => setFailed(false)}
        />
      ) : kind === "youtube" && youtube ? (
        <iframe
          key={value}
          src={youtube}
          title={text("media.youtube")}
          referrerPolicy="strict-origin-when-cross-origin"
          sandbox="allow-scripts allow-same-origin allow-presentation"
          allow="encrypted-media; picture-in-picture"
          allowFullScreen
          onError={() => setFailed(true)}
        />
      ) : null}
      {kind === "youtube" && !failed && (
        <p className="media-youtube-help">{text("media.youtubeHelp")}</p>
      )}
      {(!kind || failed) && <p role="status">{text("media.urlFailed")}</p>}
      <span className="media-url-address">{value}</span>
      {kind && (
        <Button
          type="button"
          appearance="subtle"
          icon={<Open20Regular />}
          onClick={() =>
            void controller
              ?.media({ action: "url_open", url: value })
              .then((result) => {
                if (result?.kind === "asset_error") setFailed(true);
              })
              .catch(() => setFailed(true))
          }
        >
          {text("media.external")}
        </Button>
      )}
    </div>
  );
}
function Asset({
  id,
  image,
  remove,
  disabled,
  handle,
}: {
  id: string;
  image: boolean;
  remove?: () => void;
  disabled?: boolean;
  handle?: React.ReactNode;
}) {
  const controller = useContext(MediaContext);
  const previewAllowed = useContext(MediaPreviewContext);
  const [meta, setMeta] = useState<AssetMetadata | null>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [failure, setFailure] = useState<{
    name: string | null;
    state: string | null;
  } | null>(null);
  const [zoom, setZoom] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let alive = true;
    let objectUrl: string | undefined;
    if (controller && previewAllowed)
      void (async () => {
        try {
          const r = await controller.media({ action: "asset_read", asset: id });
          if (r.kind === "asset_error") {
            if (!alive) return;
            setFailure({ name: r.assetName, state: r.assetState });
            return;
          }
          if (r.kind !== "asset" || (image && !r.metadata.image))
            throw new Error("asset mismatch");
          if (!alive) return;
          setMeta(r.metadata);
          if (image) {
            const parts: Uint8Array<ArrayBuffer>[] = [];
            let offset = 0;
            while (alive && offset < r.metadata.size) {
              const chunk = await controller.media({
                action: "asset_chunk",
                asset: id,
                digest: r.metadata.sha256,
                offset,
              });
              if (chunk.kind !== "asset_chunk")
                throw new Error("asset response");
              const bytes = Uint8Array.from(atob(chunk.data), (c) =>
                c.charCodeAt(0),
              );
              if (!bytes.length) throw new Error("empty asset chunk");
              parts.push(bytes);
              offset += bytes.length;
            }
            if (!alive) return;
            objectUrl = URL.createObjectURL(
              new Blob(parts, {
                type: r.contentType ?? "application/octet-stream",
              }),
            );
            setSrc(objectUrl);
          }
          setFailure(null);
        } catch {
          if (alive) setFailure({ name: null, state: "unavailable" });
        }
      })();
    return () => {
      alive = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [controller, id, image, retry, previewAllowed]);
  return (
    <>
      <div className="media-item-body">
        {handle}
        {src && (
          <img
            loading="lazy"
            src={src}
            alt={meta?.name ?? ""}
            onError={() =>
              setFailure({ name: meta?.name ?? null, state: "unreadable" })
            }
          />
        )}
        <span className="media-filename">
          {meta?.name ??
            failure?.name ??
            (failure ? text("media.unknownAttachment") : text("media.loading"))}
        </span>
        {meta && (
          <small>
            {(meta.size / 1024).toFixed(1)} KiB · {meta.name.split(".").pop()}
          </small>
        )}
        {failure && (
          <>
            <InlineNotice kind="warning" className="media-failure">
              <strong>
                {text("media.failedNamed", {
                  name: failure.name ?? text("media.unknownAttachment"),
                })}
              </strong>
              <small>{mediaFailureText(failure.state)}</small>
            </InlineNotice>
            <Button type="button" onClick={() => setRetry((v) => v + 1)}>
              {text("media.retry")}
            </Button>
          </>
        )}
        <div className="media-actions">
          {src && (
            <Tooltip content={text("media.zoom")} relationship="label">
              <Button
                type="button"
                appearance="subtle"
                icon={<ZoomIn20Regular />}
                onClick={() => setZoom(true)}
              />
            </Tooltip>
          )}
          {!image && meta && (
            <Tooltip content={text("media.open")} relationship="label">
              <Button
                type="button"
                appearance="subtle"
                icon={<Open20Regular />}
                onClick={() =>
                  void controller
                    ?.media({ action: "asset_open", asset: id })
                    .then((result) => {
                      if (result?.kind === "asset_error")
                        setFailure({
                          name: result.assetName,
                          state: result.assetState,
                        });
                    })
                    .catch(() =>
                      setFailure({
                        name: meta?.name ?? null,
                        state: "open_failed",
                      }),
                    )
                }
              />
            </Tooltip>
          )}
          {remove && (
            <Tooltip content={text("media.remove")} relationship="label">
              <Button
                type="button"
                appearance="subtle"
                disabled={disabled}
                icon={<Delete20Regular />}
                onClick={remove}
              />
            </Tooltip>
          )}
        </div>
      </div>
      <Dialog open={zoom} onOpenChange={(_, data) => setZoom(data.open)}>
        <DialogSurface className="media-zoom">
          <DialogBody>
            <DialogTitle>{meta?.name}</DialogTitle>
            <DialogContent>
              {src && <img src={src} alt={meta?.name ?? ""} />}
            </DialogContent>
            <DialogActions>
              <Button onClick={() => setZoom(false)}>
                {text("media.close")}
              </Button>
            </DialogActions>
          </DialogBody>
        </DialogSurface>
      </Dialog>
    </>
  );
}
function mediaFailureText(state: string | null) {
  switch (state) {
    case "trashed":
      return text("media.state.trashed");
    case "missing":
      return text("media.state.missing");
    case "corrupt":
      return text("media.state.corrupt");
    case "uncertain":
      return text("media.state.uncertain");
    case "unreadable":
      return text("media.state.unreadable");
    case "open_failed":
      return text("media.state.open_failed");
    default:
      return text("media.state.unavailable");
  }
}

export function AssetList({
  ids,
  image,
  change,
  importAsset,
  disabled = false,
  id = "media",
  label,
  invalid,
  describedBy,
}: {
  ids: string[];
  image: boolean;
  change?: (ids: string[]) => void;
  importAsset?: () => Promise<string | null>;
  disabled?: boolean;
  id?: string;
  label?: string;
  invalid?: boolean;
  describedBy?: string;
}) {
  const controller = useContext(MediaContext);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const epoch = useRef(0);
  useEffect(
    () => () => {
      epoch.current++;
    },
    [],
  );
  const reorder = useDraftReorder(
    id,
    JSON.stringify(ids),
    !!change && ids.length > 1 && !disabled && !busy,
    (_, next) => change?.(next),
    setMessage,
  );
  const add = async () => {
    if (!importAsset || busy || ids.length >= 32) return;
    const token = ++epoch.current;
    setBusy(true);
    setMessage("");
    try {
      const asset = await importAsset();
      if (token === epoch.current && asset) change?.([...ids, asset]);
    } catch {
      if (token === epoch.current) setMessage(text("media.importFailed"));
    } finally {
      if (token === epoch.current) setBusy(false);
    }
  };
  return (
    <div className="media-field" {...reorder.surface}>
      <ul className={image ? "media-gallery" : "media-files"}>
        {ids.map((asset, index) => (
          <li key={asset} {...reorder.card("assets", asset, ids)}>
            <Asset
              id={asset}
              image={image}
              disabled={disabled || busy}
              remove={
                change
                  ? () => change(ids.filter((v) => v !== asset))
                  : undefined
              }
              handle={
                change && ids.length > 1 && !disabled && !busy ? (
                  <Button
                    type="button"
                    appearance="subtle"
                    disabled={disabled || busy}
                    icon={<ReOrderDotsVertical20Regular />}
                    aria-label={text("media.reorder")}
                    onKeyDown={(e) => {
                      if (
                        e.altKey &&
                        ["ArrowUp", "ArrowDown"].includes(e.key)
                      ) {
                        e.preventDefault();
                        const next = [...ids];
                        const to = index + (e.key === "ArrowUp" ? -1 : 1);
                        if (to >= 0 && to < ids.length) {
                          [next[index], next[to]] = [next[to], next[index]];
                          change(next);
                          setMessage(text("whole.reordered"));
                        }
                      }
                    }}
                  />
                ) : undefined
              }
            />
          </li>
        ))}
      </ul>
      {importAsset && (
        <>
          <Tooltip content={text("media.add")} relationship="description">
            <Button
              id={id}
              type="button"
              aria-invalid={invalid}
              aria-describedby={describedBy}
              aria-label={
                label ? `${label}: ${text("media.add")}` : text("media.add")
              }
              disabled={disabled || busy || ids.length >= 32}
              icon={<Add20Regular />}
              onClick={() => void add()}
            >
              {text("media.add")}
            </Button>
          </Tooltip>
          {busy && (
            <Button
              type="button"
              onClick={() => {
                epoch.current++;
                setBusy(false);
                void controller?.pollProgress(true);
              }}
            >
              {text("media.cancel")}
            </Button>
          )}
          <HelpText>
            {text(image ? "media.imageHelp" : "media.fileHelp")}
          </HelpText>
        </>
      )}
      {(busy || message) && (
        <p role="status">{busy ? text("media.loading") : message}</p>
      )}
    </div>
  );
}
