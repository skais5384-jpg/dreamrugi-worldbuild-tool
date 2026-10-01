import { useSyncExternalStore } from "react";

// App-user preference only. Never put this record in a project or SVN payload.
export const YOUTUBE_CONSENT_VERSION = "1.0.0-2026-10-01";
export const YOUTUBE_CONSENT_KEY = "worldbuild.youtube-consent";
export interface YoutubeConsent {
  choice: "unset" | "denied" | "allowed";
  version: string;
  changedAt: string | null;
  storageFailed: boolean;
}
function read(): YoutubeConsent {
  const empty: YoutubeConsent = {
    choice: "unset",
    version: YOUTUBE_CONSENT_VERSION,
    changedAt: null,
    storageFailed: false,
  };
  try {
    const raw = window.localStorage.getItem(YOUTUBE_CONSENT_KEY);
    if (!raw) return empty;
    const saved = JSON.parse(raw);
    if (
      saved.version !== YOUTUBE_CONSENT_VERSION ||
      !["allowed", "denied"].includes(saved.choice) ||
      typeof saved.changedAt !== "string" ||
      !Number.isFinite(Date.parse(saved.changedAt))
    )
      return empty;
    return { ...empty, choice: saved.choice, changedAt: saved.changedAt };
  } catch {
    return { ...empty, storageFailed: true };
  }
}
export function createYoutubeConsentStore() {
  let value = read();
  const listeners = new Set<() => void>();
  const notify = () => listeners.forEach((listener) => listener());
  return {
    snapshot: () => value,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    refresh: () => {
      value = read();
      notify();
    },
    choose: (allowed: boolean) => {
      // Revoke synchronously, before any persistence attempt or continuation.
      const next: YoutubeConsent = {
        choice: allowed ? "allowed" : "denied",
        version: YOUTUBE_CONSENT_VERSION,
        changedAt: new Date().toISOString(),
        storageFailed: false,
      };
      value = { ...next, choice: "denied" };
      notify();
      try {
        window.localStorage.setItem(
          YOUTUBE_CONSENT_KEY,
          JSON.stringify({
            choice: next.choice,
            version: next.version,
            changedAt: next.changedAt,
          }),
        );
        if (
          window.localStorage.getItem(YOUTUBE_CONSENT_KEY) !==
          JSON.stringify({
            choice: next.choice,
            version: next.version,
            changedAt: next.changedAt,
          })
        )
          throw new Error("consent persistence unconfirmed");
        value = next;
      } catch {
        value = { ...next, choice: "denied", storageFailed: true };
      }
      notify();
      return !value.storageFailed;
    },
  };
}
let store: ReturnType<typeof createYoutubeConsentStore> | undefined;
export function youtubeConsentStore() {
  if (!store) {
    store = createYoutubeConsentStore();
    window.addEventListener("storage", (event) => {
      if (event.key === YOUTUBE_CONSENT_KEY || event.key === null)
        store?.refresh();
    });
  }
  return store;
}
export function useYoutubeConsent() {
  const consent = youtubeConsentStore();
  return useSyncExternalStore(consent.subscribe, consent.snapshot);
}
