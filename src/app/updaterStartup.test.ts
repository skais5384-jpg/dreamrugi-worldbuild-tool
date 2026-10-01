import { expect, it } from "vitest";
import { waitFor } from "@testing-library/react";
import { GuardedClient } from "../bridge/client";
import { TemplateController } from "./controller";
import { TestTransport } from "./testTransport";
it("update choice precedes settings and default-project reopen", async () => {
  const transport = new TestTransport();
  transport.defaultProjectRoot = "test-project";
  const controller = new TemplateController(new GuardedClient(transport));
  let proceed: () => void = () => {};
  controller.setStartupGate(
    new Promise<void>((resolve) => {
      proceed = resolve;
    }),
  );
  controller.start();
  await waitFor(() => expect(controller.snapshot().ready).toBe(true));
  expect(controller.snapshot().startupLoading).toBe(true);
  expect(
    transport.commands.some(
      (c) =>
        c.action === "submit" &&
        ["project_settings_read", "open"].includes(c.input.kind),
    ),
  ).toBe(false);
  proceed();
  await waitFor(() => expect(controller.snapshot().projectId).not.toBeNull());
  const kinds = transport.commands.flatMap((c) =>
    c.action === "submit" ? [c.input.kind] : [],
  );
  expect(kinds.indexOf("project_settings_read")).toBeLessThan(
    kinds.indexOf("open"),
  );
  expect(kinds.filter((k) => k === "open")).toHaveLength(1);
});
