import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { render } from "../test/render";
import { text } from "../strings";
import type { TemplateController } from "./controller";
import type { DocumentController } from "./documentController";
import { ProjectFiles } from "./ProjectFiles";

describe("project file manager", () => {
  it("empties the confirmed whole trash while the visible list is filtered", async () => {
    const shellState = {
      health: null,
      fileManagerTarget: null,
      assetInspection: {
        token: "inspection",
        observedAtUtc: "2026-09-23T00:00:00.000Z",
        complete: true,
        scannedFiles: 3,
        referencedAssets: 0,
        usedAssets: 0,
        unusedAssets: 0,
        missingAssets: 0,
        corruptAssets: 0,
        uncertainAssets: 0,
        rows: [],
        trash: [{ id: "asset", name: "one.txt", size: 2, protected: false }],
        deletedTemplates: [
          { id: "template", name: "모형", size: 3, removable: true },
        ],
      },
    };
    const setHealthTrash = vi.fn();
    const setHealthTemplates = vi.fn();
    const shell = {
      snapshot: () => shellState,
      subscribe: () => () => {},
      showFileManager: vi.fn(),
      clearFileManagerTarget: vi.fn(),
      inspectAssets: vi.fn(),
      setHealthTrash,
      setHealthTemplates,
      showHealthPurgeConfirm: vi.fn(),
      purgeHealthSelection: vi.fn().mockResolvedValue({
        completedCount: 1,
        failures: [],
        cleanupRequired: [],
      }),
    } as unknown as TemplateController;
    const mutate = vi.fn().mockResolvedValue(true);
    const documentState = {
      editors: {},
      list: {
        documents: [{ id: "document", name: "문서", template: "template" }],
        layout: {
          revision: 1,
          rootOrder: [],
          nodes: { document: { state: "trashed" } },
        },
      },
    };
    const documents = {
      snapshot: () => documentState,
      subscribe: () => () => {},
      mutate,
    } as unknown as DocumentController;
    render(<ProjectFiles mode="trash" shell={shell} documents={documents} />);
    fireEvent.change(
      screen.getByRole("combobox", { name: text("resources.typeFilter") }),
      {
        target: { value: "resource" },
      },
    );
    fireEvent.change(
      screen.getByRole("textbox", { name: text("resources.search") }),
      {
        target: { value: "one" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: text("trash.empty") }));
    expect(
      screen.getByText(
        text("trash.purgeSummary", { count: "3", protected: "0" }),
      ),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: text("trash.purge") }));
    await waitFor(() => {
      expect(mutate).toHaveBeenCalledWith({
        kind: "purge",
        document: "document",
      });
      expect(setHealthTrash).toHaveBeenCalledWith(["asset"]);
      expect(setHealthTemplates).toHaveBeenCalledWith(["template"]);
    });
  });

  it("shows readable, type-colored extension badges including unknown files", () => {
    const shellState = {
      health: null,
      fileManagerTarget: null,
      assetInspection: {
        token: "inspection",
        observedAtUtc: "2026-09-22T00:00:00.000Z",
        complete: true,
        scannedFiles: 3,
        referencedAssets: 0,
        usedAssets: 0,
        unusedAssets: 3,
        missingAssets: 0,
        corruptAssets: 0,
        uncertainAssets: 0,
        rows: [
          {
            id: "txt",
            name: "attachment.txt",
            size: 34,
            status: "unused",
            reason: null,
          },
          {
            id: "png",
            name: "logo.png",
            size: 95,
            status: "unused",
            reason: null,
          },
          {
            id: "unknown",
            name: null,
            size: null,
            status: "unused",
            reason: null,
          },
        ],
        trash: [],
        deletedTemplates: [],
      },
    };
    const shell = {
      snapshot: () => shellState,
      subscribe: () => () => {},
      showFileManager: vi.fn(),
      clearFileManagerTarget: vi.fn(),
    } as unknown as TemplateController;
    const documentState = { editors: {}, list: null };
    const documents = {
      snapshot: () => documentState,
      subscribe: () => () => {},
    } as unknown as DocumentController;

    render(
      <ProjectFiles mode="resources" shell={shell} documents={documents} />,
    );

    expect(screen.getByText("TXT")).toHaveClass("resource-extension-text");
    expect(screen.getByText("PNG")).toHaveClass("resource-extension-image");
    expect(screen.getByText(text("resources.unknownExtension"))).toHaveClass(
      "resource-extension-other",
    );
  });

  it("explains the active editor owner before attempting a trash restore", async () => {
    const shellState = {
      health: null,
      fileManagerTarget: null,
      assetInspection: {
        token: "inspection",
        observedAtUtc: "2026-09-22T00:00:00.000Z",
        complete: true,
        scannedFiles: 1,
        referencedAssets: 0,
        usedAssets: 0,
        unusedAssets: 0,
        missingAssets: 0,
        corruptAssets: 0,
        uncertainAssets: 0,
        rows: [],
        trash: [
          {
            id: "asset-one",
            name: "attachment.txt",
            size: 34,
            reclaimableSize: 34,
            movedAtUtc: "2026-09-22T00:00:00.000Z",
            protected: false,
            reason: null,
          },
        ],
        deletedTemplates: [],
      },
    };
    const restoreAsset = vi.fn();
    const shell = {
      snapshot: () => shellState,
      subscribe: () => () => {},
      showFileManager: vi.fn(),
      clearFileManagerTarget: vi.fn(),
      inspectAssets: vi.fn(),
      restoreAsset,
    } as unknown as TemplateController;
    const documentState = {
      editors: { "document-one": {} },
      list: {
        documents: [
          {
            id: "document-one",
            name: "FIX003 복원 순서 부모 문서",
            template: "template-one",
          },
        ],
        layout: { revision: 1, rootOrder: [], nodes: {} },
      },
    };
    const documents = {
      snapshot: () => documentState,
      subscribe: () => () => {},
    } as unknown as DocumentController;

    render(<ProjectFiles mode="trash" shell={shell} documents={documents} />);
    fireEvent.click(
      screen.getByRole("button", {
        name: `attachment.txt · ${text("documents.menu")}`,
      }),
    );
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("health.restore") }),
    );

    const warning = await screen.findByText(
      text("trash.restoreBlockedByEditors", {
        names: "FIX003 복원 순서 부모 문서",
      }),
    );
    expect(warning.closest('[role="alert"]')).not.toBeNull();
    expect(restoreAsset).not.toHaveBeenCalled();
  });
});
