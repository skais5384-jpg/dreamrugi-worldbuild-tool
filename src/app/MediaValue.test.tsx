import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AssetList,
  MediaContext,
  mediaKind,
  UrlRead,
  youtubeEmbed,
} from "./MediaValue";
import { CreationValue } from "./CreationValue";
import type { Field } from "../bridge/types";
import { text } from "../strings";

afterEach(cleanup);
describe("M3-7 media", () => {
  it("accepts direct HTTPS media and rejects credentials, local addresses and active pages", () => {
    expect(mediaKind("https://example.com/clip.MP4?quality=low")).toBe("video");
    expect(mediaKind("https://example.com/photo.webp")).toBe("image");
    for (const extension of ["GIF", "apng", "bmp", "jfif"])
      expect(mediaKind(`https://example.com/photo.${extension}`)).toBe("image");
    for (const url of [
      "http://example.com/a.mp4",
      "https://user:secret@example.com/a.mp4",
      "https://127.1/a.mp4",
      "https://[::1]/a.mp4",
      "https://intranet/a.png",
      "https://site.local/a.png",
      "https://example.com:8443/a.mp4",
      "https://example.com/a.svg",
      "https://youtube.com/watch?v=test",
      "https://example.com/a.mp4#fragment",
      "https://example.com/a.mp4\n",
    ])
      expect(mediaKind(url)).toBeNull();
  });
  it("normalizes official YouTube URLs without forwarding tracking parameters", () => {
    for (const url of [
      "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1m30s&si=secret",
      "https://youtu.be/dQw4w9WgXcQ?t=90",
      "https://youtube.com/shorts/dQw4w9WgXcQ?start=90",
      "https://m.youtube.com/embed/dQw4w9WgXcQ?t=90",
    ]) {
      const embed = new URL(youtubeEmbed(url)!);
      expect(embed.origin).toBe("https://www.youtube.com");
      expect(embed.pathname).toBe("/embed/dQw4w9WgXcQ");
      expect(embed.searchParams.get("start")).toBe("90");
      expect(embed.searchParams.get("si")).toBeNull();
      expect(mediaKind(url)).toBe("youtube");
    }
    for (const url of [
      "https://youtube.example/watch?v=dQw4w9WgXcQ",
      "https://youtube.com.example/watch?v=dQw4w9WgXcQ",
      "https://youtube.com/watch?v=short",
      "https://youtube.com/watch?v=dQw4w9WgXcQ&v=aqz-KE-bpKQ",
      "https://youtube.com/watch?v=dQw4w9WgXcQ&t=forever",
    ])
      expect(youtubeEmbed(url)).toBeNull();
  });
  it("isolates the YouTube player and keeps the original address visible", () => {
    const url = "https://youtu.be/dQw4w9WgXcQ?si=kept-in-original";
    const { container } = render(<UrlRead value={url} />);
    const frame = container.querySelector("iframe")!;
    expect(frame.getAttribute("sandbox")).toContain("allow-scripts");
    expect(frame.getAttribute("sandbox")).not.toContain("allow-top-navigation");
    expect(frame.src).not.toContain("kept-in-original");
    expect(screen.getByText(url)).toBeTruthy();
    expect(screen.getByText(text("media.youtubeHelp"))).toBeTruthy();
  });
  it("keeps a failed URL visible and uses actual media elements", () => {
    const url = "https://example.com/clip.mp4";
    const { container } = render(<UrlRead value={url} />);
    const video = container.querySelector("video")!;
    expect(video.controls).toBe(true);
    fireEvent.error(video);
    expect(screen.getByText(text("media.urlFailed"))).toBeTruthy();
    expect(screen.getByText(url)).toBeTruthy();
  });
  it.each([
    { image: false, surface: "attachment" },
    { image: true, surface: "gallery" },
  ])(
    "shows the retained $surface filename and the actual trash reason",
    async ({ image }) => {
      const media = vi.fn().mockResolvedValue({
        kind: "asset_error",
        error: { category: "not_found", stage: "asset_read", nextAction: "" },
        assetName: "attachment.txt",
        assetState: "trashed",
      });
      render(
        <MediaContext.Provider
          value={{ media, pollProgress: vi.fn().mockResolvedValue(undefined) }}
        >
          <AssetList ids={["asset-one"]} image={image} />
        </MediaContext.Provider>,
      );
      const reason = await screen.findByText(text("media.state.trashed"));
      const warning = reason.closest('[role="alert"]');
      expect(warning).not.toBeNull();
      expect(warning).toHaveTextContent("attachment.txt");
      expect(warning?.querySelector("svg")).not.toBeNull();
      expect(media).toHaveBeenCalledWith({
        action: "asset_read",
        asset: "asset-one",
      });
    },
  );
  it("does not attach a late result after cancel or unmount", async () => {
    let finish: (id: string) => void = () => {};
    const importAsset = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const change = vi.fn();
    const { unmount } = render(
      <AssetList ids={[]} image importAsset={importAsset} change={change} />,
    );
    fireEvent.click(screen.getByRole("button", { name: text("media.add") }));
    expect(
      screen.getByRole("button", { name: text("media.add") }),
    ).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: text("media.cancel") }));
    finish("late");
    await waitFor(() => expect(importAsset).toHaveBeenCalledTimes(1));
    expect(change).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: text("media.add") }));
    unmount();
    finish("later");
    await Promise.resolve();
    expect(change).not.toHaveBeenCalled();
  });
  it("reorders by ID from the keyboard and removes only the selected reference", () => {
    const change = vi.fn();
    render(<AssetList ids={["a", "b", "c"]} image change={change} />);
    fireEvent.keyDown(
      screen.getAllByRole("button", { name: text("media.reorder") })[1],
      { key: "ArrowUp", altKey: true },
    );
    expect(change).toHaveBeenLastCalledWith(["b", "a", "c"]);
    fireEvent.click(
      screen.getAllByRole("button", { name: text("media.remove") })[1],
    );
    expect(change).toHaveBeenLastCalledWith(["a", "c"]);
  });
  it("exposes required errors and writing guide on the actual add control and unsets the last item", () => {
    const field: Field = {
      id: "asset",
      label: "gallery",
      kind: "Image",
      lifecycle: "Active",
      required: true,
      presentation: null,
      default: { kind: "unset" },
      initialDefault: { kind: "unset" },
      introducedRevision: "1",
      optionOrder: [],
      options: [],
      writingGuide: "Choose a reference image",
    };
    const change = vi.fn();
    render(
      <CreationValue
        field={field}
        intent={{ intent: "set", value: { kind: "image", value: ["a"] } }}
        change={change}
        invalid
        disabled={false}
        importAsset={async () => null}
      />,
    );
    const add = screen.getByRole("button", {
      name: `gallery: ${text("media.add")}`,
    });
    expect(add).toHaveAttribute("aria-invalid", "true");
    expect(add).toHaveAttribute(
      "aria-describedby",
      expect.stringContaining("-error"),
    );
    expect(screen.getByText(field.writingGuide!)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: text("media.remove") }));
    expect(change).toHaveBeenCalledWith({ intent: "unset" });
  });
});
