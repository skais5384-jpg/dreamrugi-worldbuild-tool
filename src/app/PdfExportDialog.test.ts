import { createElement, useEffect, useState } from "react";
import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { save } from "@tauri-apps/plugin-dialog";
import { render } from "../test/render";
import { text } from "../strings";
import { ActivityLog, useActivityLog } from "./ActivityLog";
import type { DocumentController } from "./documentController";
import { PdfExportDialog, pdfFilename } from "./PdfExportDialog";

function TestActivityLog() {
  const log = useActivityLog(false);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const show = () => setOpen(true);
    window.addEventListener("open-activity-log", show);
    return () => window.removeEventListener("open-activity-log", show);
  }, []);
  return open
    ? createElement(ActivityLog, { ...log, close: () => setOpen(false) })
    : null;
}

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});
afterEach(() => vi.unstubAllGlobals());

describe("PDF save filename", () => {
  it("keeps the document name while removing reserved path syntax and duplicate extension", () => {
    expect(pdfFilename("세계/설정:최종.pdf")).toBe("세계_설정_최종.pdf");
    expect(pdfFilename("CON.txt")).toBe("문서.pdf");
    expect(pdfFilename("  ...  ")).toBe("문서.pdf");
    expect(pdfFilename("이름.pdf. ")).toBe("이름.pdf");
    expect(pdfFilename("이름 ".repeat(80)).length).toBeLessThanOrEqual(124);
  });
});

describe("PDF export decision", () => {
  it("owns the picker synchronously and allows a fresh attempt after its failure", async () => {
    let rejectPicker!: (error: Error) => void;
    vi.mocked(save)
      .mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            rejectPicker = reject;
          }),
      )
      .mockResolvedValueOnce(null);
    const controller = {
      pdfInspect: vi.fn().mockResolvedValue({
        kind: "pdf_inspect",
        document: "document-a",
        name: "시험 문서",
        source: "saved-source",
        missing_images: [],
      }),
      pdfExport: vi.fn(),
    } as unknown as DocumentController;
    const close = vi.fn();
    render(
      createElement(PdfExportDialog, {
        controller,
        document: "document-a",
        dirty: false,
        close,
        completed: vi.fn(),
      }),
    );
    const choose = await screen.findByRole("button", {
      name: text("pdf.choose"),
    });
    fireEvent.click(choose);
    fireEvent.click(choose);
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    await act(async () => rejectPicker(new Error("picker unavailable")));
    fireEvent.click(screen.getByRole("button", { name: text("pdf.choose") }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(controller.pdfExport).not.toHaveBeenCalled();
  });

  it("states the saved basis and requires an explicit choice for missing images", async () => {
    const controller = {
      pdfInspect: vi.fn().mockResolvedValue({
        kind: "pdf_inspect",
        document: "document-a",
        name: "시험 문서",
        source: "saved-source",
        missing_images: ["삽화"],
      }),
    } as unknown as DocumentController;
    render(
      createElement(
        "div",
        null,
        createElement(PdfExportDialog, {
          controller,
          document: "document-a",
          dirty: true,
          close: vi.fn(),
          completed: vi.fn(),
        }),
        createElement(TestActivityLog),
      ),
    );
    await screen.findByText(text("pdf.missing"), { exact: false });
    fireEvent.click(
      screen
        .getAllByRole("button", { name: text("update.details") })
        .slice(-1)[0],
    );
    expect(await screen.findByText("삽화")).toBeTruthy();
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "실행 기록" })).getByRole(
        "button",
        { name: "닫기" },
      ),
    );
    expect(screen.getByText(text("pdf.purpose"))).toBeTruthy();
    expect(await screen.findByText(text("pdf.savedOnly"))).toBeTruthy();
    expect(
      screen.getByText(text("pdf.missing"), { exact: false }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: text("pdf.continue") }),
    ).toBeTruthy();
  });

  it("returns to the document without exporting when the OS save picker is cancelled", async () => {
    vi.mocked(save).mockResolvedValueOnce(null);
    const close = vi.fn();
    const controller = {
      pdfInspect: vi.fn().mockResolvedValue({
        kind: "pdf_inspect",
        document: "document-a",
        name: "시험 문서",
        source: "saved-source",
        missing_images: [],
      }),
      pdfExport: vi.fn(),
    } as unknown as DocumentController;
    render(
      createElement(PdfExportDialog, {
        controller,
        document: "document-a",
        dirty: false,
        close,
        completed: vi.fn(),
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("pdf.choose") }),
    );
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(controller.pdfExport).not.toHaveBeenCalled();
  });

  it("reports a published PDF and distinguishes a later cleanup warning", async () => {
    vi.mocked(save).mockResolvedValueOnce("C:\\export\\document.pdf");
    const completed = vi.fn();
    const close = vi.fn();
    const controller = {
      pdfInspect: vi.fn().mockResolvedValue({
        kind: "pdf_inspect",
        document: "document-a",
        name: "시험 문서",
        source: "saved-source",
        missing_images: [],
      }),
      pdfExport: vi.fn().mockResolvedValue({
        kind: "pdf_export",
        destination: "C:\\export\\document.pdf",
        size: 1024,
        sha256: "digest",
        cleanup_warning: true,
      }),
    } as unknown as DocumentController;
    render(
      createElement(PdfExportDialog, {
        controller,
        document: "document-a",
        dirty: false,
        close,
        completed,
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("pdf.choose") }),
    );
    await waitFor(() =>
      expect(completed).toHaveBeenCalledExactlyOnceWith(true),
    );
    expect(close).toHaveBeenCalledOnce();
  });

  it("keeps the native operation as the cancel target and releases ownership after failure", async () => {
    vi.mocked(save)
      .mockResolvedValueOnce("C:\\export\\one.pdf")
      .mockResolvedValueOnce("C:\\export\\two.pdf");
    let rejectExport!: (error: Error) => void;
    const controller = {
      pdfInspect: vi.fn().mockResolvedValue({
        kind: "pdf_inspect",
        document: "document-a",
        name: "시험 문서",
        source: "saved-source",
        missing_images: [],
      }),
      pdfExport: vi
        .fn()
        .mockImplementationOnce(
          () =>
            new Promise((_, reject) => {
              rejectExport = reject;
            }),
        )
        .mockResolvedValueOnce({ kind: "pdf_export", cleanup_warning: false }),
      cancelPdfExport: vi.fn().mockResolvedValue(true),
    } as unknown as DocumentController;
    const completed = vi.fn();
    render(
      createElement(PdfExportDialog, {
        controller,
        document: "document-a",
        dirty: false,
        close: vi.fn(),
        completed,
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("pdf.choose") }),
    );
    const cancel = await screen.findByRole("button", {
      name: text("pdf.cancel"),
    });
    fireEvent.click(cancel);
    expect(controller.cancelPdfExport).toHaveBeenCalledOnce();
    await act(async () => rejectExport(new Error("render failed")));
    fireEvent.click(
      await screen.findByRole("button", { name: text("pdf.choose") }),
    );
    await waitFor(() => expect(controller.pdfExport).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(completed).toHaveBeenCalledExactlyOnceWith(false),
    );
  });
});
