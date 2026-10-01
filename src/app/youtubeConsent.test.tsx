import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MediaActiveContext, MediaContext, UrlRead } from "./MediaValue";
import { PrivacyDialog } from "./PrivacyDialog";
import {
  createYoutubeConsentStore,
  youtubeConsentStore,
  YOUTUBE_CONSENT_KEY,
  YOUTUBE_CONSENT_VERSION,
} from "./youtubeConsent";
import { text } from "../strings";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  localStorage.removeItem(YOUTUBE_CONSENT_KEY);
  youtubeConsentStore().refresh();
});
const first = "https://youtu.be/aqz-KE-bpKQ";
const second = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
describe("YouTube privacy choices", () => {
  it("never creates remote elements without valid current consent, including unknown or old records", () => {
    for (const raw of [
      null,
      "{broken",
      JSON.stringify({
        choice: "allowed",
        version: "old",
        changedAt: new Date().toISOString(),
      }),
    ]) {
      if (raw === null) localStorage.removeItem(YOUTUBE_CONSENT_KEY);
      else localStorage.setItem(YOUTUBE_CONSENT_KEY, raw);
      youtubeConsentStore().refresh();
      const view = render(<UrlRead value={first} />);
      expect(
        view.container.querySelector("iframe,img,video,script,link"),
      ).toBeNull();
      expect(screen.getByRole("link")).toHaveAttribute("href", first);
      view.unmount();
    }
  });
  it("opens a denied link externally without granting consent and keeps images and video behavior", () => {
    const media = vi.fn().mockResolvedValue({ kind: "ok" });
    const { container } = render(
      <MediaContext.Provider value={{ media, pollProgress: vi.fn() }}>
        <UrlRead value={first} />
        <UrlRead value="https://example.com/photo.png" />
        <UrlRead value="https://example.com/clip.mp4" />
      </MediaContext.Provider>,
    );
    fireEvent.click(screen.getByRole("link"));
    expect(media).toHaveBeenCalledWith({ action: "url_open", url: first });
    expect(youtubeConsentStore().snapshot().choice).not.toBe("allowed");
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      "https://example.com/photo.png",
    );
    expect(container.querySelector("video")).toHaveAttribute(
      "preload",
      "metadata",
    );
    expect(container.querySelector("video")).not.toHaveAttribute("autoplay");
  });
  it("loads different displayed players after one consent, stops inactive players, and revokes every mounted copy", () => {
    const view = render(
      <>
        <UrlRead value={first} />
        <UrlRead value={second} />
      </>,
    );
    act(() => {
      youtubeConsentStore().choose(true);
    });
    expect(view.container.querySelectorAll("iframe")).toHaveLength(2);
    for (const frame of view.container.querySelectorAll("iframe")) {
      expect(frame.src).toMatch(/^https:\/\/www.youtube-nocookie.com\/embed\//);
      expect(new URL(frame.src).searchParams.get("autoplay")).toBeNull();
      expect(frame.getAttribute("allow")).not.toContain("autoplay");
    }
    view.rerender(
      <MediaActiveContext.Provider value={false}>
        <UrlRead value={first} />
        <UrlRead value={second} />
      </MediaActiveContext.Provider>,
    );
    expect(view.container.querySelectorAll("iframe")).toHaveLength(0);
    view.rerender(
      <MediaActiveContext.Provider value>
        <UrlRead value={first} />
        <UrlRead value={second} />
      </MediaActiveContext.Provider>,
    );
    expect(view.container.querySelectorAll("iframe")).toHaveLength(2);
    act(() => {
      youtubeConsentStore().choose(false);
    });
    expect(
      view.container.querySelectorAll("iframe,img,script,link"),
    ).toHaveLength(0);
    expect(view.container.querySelectorAll("a")).toHaveLength(2);
    act(() => {
      youtubeConsentStore().choose(true);
    });
    expect(view.container.querySelectorAll("iframe")).toHaveLength(2);
  });
  it("remembers choice across a fresh store without project contents or addresses", () => {
    const store = createYoutubeConsentStore();
    store.choose(true);
    const fresh = createYoutubeConsentStore();
    expect(fresh.snapshot().choice).toBe("allowed");
    const record = JSON.parse(localStorage.getItem(YOUTUBE_CONSENT_KEY)!);
    expect(Object.keys(record).sort()).toEqual([
      "changedAt",
      "choice",
      "version",
    ]);
    expect(record.version).toBe(YOUTUBE_CONSENT_VERSION);
    fresh.choose(false);
    expect(createYoutubeConsentStore().snapshot().choice).toBe("denied");
  });
  it("fails closed on unavailable storage and withdraws immediately when persistence fails", () => {
    const store = createYoutubeConsentStore();
    store.choose(true);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(store.choose(false)).toBe(false);
    expect(store.snapshot().choice).toBe("denied");
    expect(store.choose(true)).toBe(false);
    expect(store.snapshot().storageFailed).toBe(true);
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    expect(createYoutubeConsentStore().snapshot().choice).toBe("unset");
  });
  it("requires separate policy acceptance and loading permission; closing is refusal and policy works offline", () => {
    const close = vi.fn();
    render(<PrivacyDialog open close={close} />);
    const allow = screen.getByRole("button", { name: text("privacy.allow") });
    expect(allow).toBeDisabled();
    fireEvent.click(
      screen.getByRole("button", { name: text("privacy.policy") }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Dreamrugi Worldbuild Tool 개인정보 처리방침",
      }),
    ).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: text("privacy.close") }),
    );
    fireEvent.click(
      screen.getByRole("checkbox", { name: text("privacy.acceptPolicy") }),
    );
    expect(allow).toBeDisabled();
    fireEvent.click(
      screen.getByRole("checkbox", { name: text("privacy.allowYoutube") }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("privacy.allow") }),
    );
    expect(youtubeConsentStore().snapshot().choice).toBe("allowed");
    expect(close).toHaveBeenCalledOnce();
  });
});
