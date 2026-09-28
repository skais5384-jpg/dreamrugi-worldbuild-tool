import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { render } from "../test/render";
import { text } from "../strings";
import { AboutDialog } from "./AboutDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "about_version") return "0.1.0";
    if (command === "about_channel") return "GitHub 로컬 시험본";
    return undefined;
  });
});

it("shows the bundled notices and installed version without changing the workspace", async () => {
  const close = vi.fn();
  render(<AboutDialog open close={close} />);
  expect(
    screen.getByRole("img", { name: text("about.worldbuildLogo") }),
  ).toHaveAttribute("src", "/brand/worldbuild-tool-logo.png");
  expect(
    screen.getByRole("img", { name: text("about.dreamrugiLogo") }),
  ).toHaveAttribute("src", "/brand/dreamrugi-logo.png");
  expect(await screen.findByText("0.1.0")).toBeVisible();
  expect(await screen.findByText("GitHub 로컬 시험본")).toBeVisible();
  fireEvent.click(
    screen.getByRole("button", { name: text("about.notice.app") }),
  );
  expect(screen.getByText(/GNU GENERAL PUBLIC LICENSE/u)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: text("about.back") }));
  fireEvent.click(
    screen.getByRole("button", { name: text("about.notice.dictionary") }),
  );
  expect(screen.getByText(/hunspell-dict-ko/u)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: text("about.back") }));
  fireEvent.click(
    screen.getByRole("button", { name: "https://dreamrugi.tistory.com/" }),
  );
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("about_open_link", { target: "blog" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "skais5384@naver.com" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("about_open_link", { target: "email" }),
  );
  fireEvent.click(screen.getByRole("button", { name: text("about.close") }));
  expect(close).toHaveBeenCalledOnce();
});

it("keeps contact text available if the default app cannot open", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "about_open_link") throw new Error("private OS path");
    return "0.1.0";
  });
  render(<AboutDialog open close={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "skais5384@naver.com" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    text("about.linkFailed"),
  );
  expect(screen.queryByText("private OS path")).toBeNull();
  expect(
    screen.getByRole("button", { name: "skais5384@naver.com" }),
  ).toBeVisible();
});
