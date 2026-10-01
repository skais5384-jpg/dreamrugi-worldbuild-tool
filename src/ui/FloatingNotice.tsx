import {
  Children,
  createContext,
  isValidElement,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { DismissCircle20Regular, Info20Regular } from "@fluentui/react-icons";
import { Button, Fieldset, useControlsDisabled } from "./Controls";
import { IconCommand } from "./IconCommand";
import { text } from "../strings";
import type { ActivityEvent } from "../app/ActivityLog";
import "./FloatingNotice.css";

type Entry = {
  id: string;
  content: ReactNode;
  detail?: ReactNode;
  noticeKey?: string;
};
type NoticeEvent = {
  key: string;
  message: string;
  summary: string;
  intent: string;
};
const Registry = createContext<{
  put: (entry: Entry, event?: NoticeEvent) => void;
  remove: (id: string) => void;
  isCurrent: (id: string, key: string) => boolean;
} | null>(null);
const History = createContext<ActivityEvent[]>([]);
const Entries = createContext<Entry[]>([]);
const Host = createContext<HTMLElement | null>(null);

function readableText(node: ReactNode, summary = false): string {
  return Children.toArray(node)
    .map((child) => {
      if (typeof child === "string" || typeof child === "number")
        return String(child);
      if (isValidElement<{ children?: ReactNode }>(child)) {
        if (
          child.type === Button ||
          child.type === IconCommand ||
          (summary && child.type === "details")
        )
          return "";
        return readableText(child.props.children, summary);
      }
      return "";
    })
    .join(" ")
    .replace(/\s+/gu, " ")
    .trim();
}

/** All notifications share one scrollable lower-left area, including in dialogs. */
export function FloatingNoticeProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [history, setHistory] = useState<ActivityEvent[]>([]);
  const [dialog, setDialog] = useState<Element | null>(null);
  const [host, setHost] = useState<HTMLElement | null>(null);
  const currentDialog = useRef<Element | null>(null);
  const lastEvents = useRef(new Map<string, string>());
  const sequence = useRef(0);
  const pendingEntries = useRef(new Map<string, Entry>());
  const pendingEvents = useRef<ActivityEvent[]>([]);
  const timer = useRef<number | null>(null);
  const schedule = useCallback(() => {
    if (timer.current !== null) return;
    // Native operations can publish many states in one task. Batch notices in
    // the next task rather than adding nested renders to each protected action.
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setEntries([...pendingEntries.current.values()]);
      const events = pendingEvents.current;
      pendingEvents.current = [];
      if (events.length)
        setHistory((current) => [...current, ...events].slice(-50));
    }, 0);
  }, []);
  const put = useCallback(
    (entry: Entry, event?: NoticeEvent) => {
      pendingEntries.current.set(entry.id, entry);
      schedule();
      if (!event || lastEvents.current.get(entry.id) === event.key) return;
      lastEvents.current.set(entry.id, event.key);
      const notice: ActivityEvent = {
        sessionId: `notice-${++sequence.current}`,
        observedAtUtc: new Date().toISOString(),
        feature: "안내",
        stage: "notification",
        category: event.intent,
        outcome: event.intent === "error" ? "error" : "notice",
        correlation: null,
        projectFingerprint: null,
        summary: event.summary.slice(0, 100),
        detail: event.message,
        noticeId: entry.id,
        noticeKey: event.key,
      };
      pendingEvents.current.push(notice);
    },
    [schedule],
  );
  const remove = useCallback(
    (id: string) => {
      pendingEntries.current.delete(id);
      schedule();
    },
    [schedule],
  );
  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = null;
    },
    [],
  );
  const isCurrent = useCallback(
    (id: string, key: string) =>
      pendingEntries.current.get(id)?.noticeKey === key,
    [],
  );
  const registry = useMemo(
    () => ({ put, remove, isCurrent }),
    [put, remove, isCurrent],
  );
  useEffect(() => {
    const refresh = () => {
      const surfaces = document.querySelectorAll(".fui-DialogSurface");
      const next = surfaces.item(surfaces.length - 1);
      if (currentDialog.current === next) return;
      currentDialog.current = next;
      setDialog(next);
    };
    refresh();
    const observer = new MutationObserver(refresh);
    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, []);
  const stack = (
    <section
      ref={setHost}
      className={`floating-message-stack${dialog ? " floating-message-stack-dialog" : ""}`}
      aria-label="알림"
    >
      {entries
        .filter((entry) => entry.content != null)
        .map((entry) => (
          <div className="floating-message-entry" key={entry.id}>
            {entry.content}
          </div>
        ))}
    </section>
  );
  return (
    <Registry.Provider value={registry}>
      <History.Provider value={history}>
        <Entries.Provider value={entries}>
          <Host.Provider value={host}>
            {children}
            {dialog?.classList.contains("activity-log-dialog")
              ? null
              : dialog
                ? createPortal(stack, dialog)
                : stack}
          </Host.Provider>
        </Entries.Provider>
      </History.Provider>
    </Registry.Provider>
  );
}

export function useNoticeHistory() {
  return useContext(History);
}

/** Existing operation toasts retain their own buttons, timers and dismissal. */
export function FloatingMessage({ children }: { children: ReactNode }) {
  const registry = useContext(Registry);
  const host = useContext(Host);
  return host
    ? createPortal(
        <div className="floating-message-entry">{children}</div>,
        host,
      )
    : registry
      ? null
      : children;
}

export function FloatingNotice({
  children,
  intent = "info",
  eventId,
  scope = "",
  isCurrent,
}: {
  children: ReactNode;
  intent?: "info" | "success" | "warning" | "error";
  /** Stable request/result identity. Action notices supply their actual owner. */
  eventId?: string | number | object;
  scope?: string;
  isCurrent?: () => boolean;
}) {
  const registry = useContext(Registry);
  const id = useId();
  const disabled = useControlsDisabled();
  const message = readableText(children);
  const summary = readableText(children, true);
  const identity = eventId ?? `${intent}:${message}`;
  const [occurrence, setOccurrence] = useState({
    identity,
    scope,
    revision: 1,
  });
  let current = occurrence;
  if (!Object.is(occurrence.identity, identity) || occurrence.scope !== scope) {
    current = { identity, scope, revision: occurrence.revision + 1 };
    setOccurrence(current);
  }
  const key = `${id}:${current.revision}`;
  const live = useRef({ key, disabled, isCurrent });
  useLayoutEffect(() => {
    live.current = { key, disabled, isCurrent };
  }, [key, disabled, isCurrent]);
  const detail = useMemo(
    () => (
      <div
        onClickCapture={(event) => {
          // A retained DOM handler can be clicked before the batched history
          // render catches up. Check the current request again at dispatch.
          if (
            live.current.key !== key ||
            live.current.disabled ||
            (registry && !registry.isCurrent(id, key)) ||
            live.current.isCurrent?.() === false
          ) {
            event.preventDefault();
            event.stopPropagation();
          }
        }}
      >
        <Fieldset className="notice-control-scope" disabled={disabled}>
          {children}
        </Fieldset>
      </div>
    ),
    [children, disabled, id, key, registry],
  );
  const [dismissed, setDismissed] = useState<string | null>(null);
  const content = useMemo(
    () => (
      <div
        className={`feedback-toast floating-notice ${intent}`}
        role={intent === "warning" || intent === "error" ? "alert" : "status"}
      >
        <div className="floating-notice-content">{summary}</div>
        <IconCommand
          restoreFocus
          label={text("update.details")}
          icon={<Info20Regular />}
          className="feedback-toast-command"
          onClick={() =>
            window.dispatchEvent(
              new CustomEvent("open-activity-log", {
                detail: { noticeKey: key },
              }),
            )
          }
        />
        <IconCommand
          label={text("backup.dismiss")}
          icon={<DismissCircle20Regular />}
          className="feedback-toast-command"
          onClick={() => setDismissed(key)}
        />
      </div>
    ),
    [summary, intent, key],
  );
  useLayoutEffect(() => {
    registry?.put(
      {
        id,
        content: dismissed === key ? null : content,
        detail,
        noticeKey: key,
      },
      { key, message, summary, intent },
    );
  }, [registry, id, key, message, summary, intent, dismissed, content, detail]);
  useLayoutEffect(() => () => registry?.remove(id), [registry, id]);
  return registry || dismissed === key ? null : content;
}

export function FloatingNoticeContent({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

/** Current guidance and action buttons remain accessible after toast dismissal. */
export function NoticeEventDetail({ event }: { event: ActivityEvent }) {
  const current = useContext(Entries).find(
    (entry) =>
      entry.id === event.noticeId && entry.noticeKey === event.noticeKey,
  );
  return current?.detail ?? event.detail;
}
