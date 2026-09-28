import { describe, expect, it, vi } from "vitest";
import { GuardedClient } from "./client";
import type { Transport } from "./client";
import type { Command, Response, RetainedRef, Work } from "./types";

type Operation = Extract<Response, { kind: "operation" }>;
const key = (id: string): RetainedRef => ({
  id,
  generation: `generation-${id}`,
});
const edit = (
  name = "original",
): Extract<Work, { kind: "update_template" }> => ({
  kind: "update_template",
  project: "project",
  session: "session",
  view: "view",
  revision: "1",
  edit: { kind: "name", name },
});
function complete(
  operation: string,
  retained: RetainedRef | null = null,
): Operation {
  return {
    kind: "operation",
    operation,
    state: "complete",
    retained,
    result: { kind: "control", error: null },
  };
}
function mockClient() {
  const calls: Command[] = [];
  const responses = new Map<string, Operation>();
  const invoke = vi.fn<Transport["invoke"]>(async (_name, bytes) => {
    const command = JSON.parse(new TextDecoder().decode(bytes)) as Command;
    calls.push(command);
    if (command.action === "submit")
      return { kind: "submitted", operation: command.operation };
    if (command.action === "operation")
      return responses.get(command.operation) ?? complete(command.operation);
    return { kind: "acknowledged" };
  });
  const listen = vi
    .fn<Transport["listen"]>()
    .mockResolvedValue(() => undefined);
  return {
    client: new GuardedClient({ invoke, listen }),
    invoke,
    calls,
    responses,
  };
}
async function fill(client: GuardedClient) {
  for (let n = 0; n < 40; n++)
    await client.submit(`input-${n}`, edit(`original-${n}`));
}

describe("G13 FIX-002 bounded client ownership", () => {
  it("keeps ordinary40 separate from control8 and reclaims existing controls without another slot", async () => {
    const { client, calls } = mockClient();
    await fill(client);
    await expect(client.submit("overflow", edit())).rejects.toMatchObject({
      category: "protocol",
    });
    await expect(
      client.submit("flag", { ...edit(), control: true } as Work),
    ).rejects.toMatchObject({ category: "protocol" });
    await expect(
      client.resumeRetained("resume", key("input-0"), "project", {
        kind: "create_template",
      }),
    ).rejects.toMatchObject({ category: "protocol" });
    await expect(
      client.sessionControl("new-p", {
        project: "project",
        session: "session",
        control: "return_active",
      }),
    ).rejects.toMatchObject({ category: "protocol" });
    expect(calls).toHaveLength(40);
    for (let n = 0; n < 8; n++) await client.close(`control-${n}`, "project");
    await expect(
      client.close("control-overflow", "project"),
    ).rejects.toMatchObject({ category: "protocol" });
    expect(client.retainedInput("control-overflow")).toBeUndefined();
    await client.close("control-0", "project");
    await expect(client.close("control-0", "other")).rejects.toMatchObject({
      category: "protocol",
    });
    await client.result("control-0");
    await client.acknowledgeTransport("control-0");
    expect(client.retainedInput("control-0")).toBeUndefined();
    await client.close("control-overflow", "project");
    for (let n = 0; n < 40; n++)
      expect(client.retainedInput(`input-${n}`)).toEqual(edit(`original-${n}`));
    await expect(client.submit("still-full", edit())).rejects.toMatchObject({
      category: "protocol",
    });
  });

  it("allows only closed recovery roles to use the reserve while return-active keeps its payload slot", async () => {
    const { client, responses } = mockClient();
    await client.sessionControl("p", {
      project: "project",
      session: "session",
      control: "return_active",
    });
    responses.set("p", complete("p", key("p")));
    await client.result("p");
    await client.acknowledgeTransport("p");
    expect(client.retainedReference("p")).toEqual(key("p"));
    for (let n = 0; n < 39; n++) await client.submit(`input-${n}`, edit());
    const controls: Work[] = [
      { kind: "recover", project: "project" },
      { kind: "retire_project", project: "project" },
      ...(
        [
          "end",
          "retry_release",
          "revalidate",
          "preserve",
          "accept",
          "acknowledge_recovery",
        ] as const
      ).map((control) => ({
        kind: "session_control" as const,
        project: "project",
        session: "session",
        control,
      })),
    ];
    for (const [n, input] of controls.entries()) {
      await client.submit(`recovery-${n}`, input);
      await client.result(`recovery-${n}`);
      await client.acknowledgeTransport(`recovery-${n}`);
    }
    expect(client.retainedInput("p")).toMatchObject({
      control: "return_active",
    });
    await expect(
      client.submit("ordinary-overflow", edit()),
    ).rejects.toMatchObject({ category: "protocol" });
  });

  it("keeps originals through handoff submit and ack loss and frees the control only after reconciliation", async () => {
    const { client, invoke, responses } = mockClient();
    await fill(client);
    responses.set("input-0", complete("input-0", key("input-0")));
    await client.result("input-0");
    await client.acknowledgeTransport("input-0");
    invoke.mockRejectedValueOnce(new Error("lost submit response"));
    await expect(
      client.handoffRetained("handoff", key("input-0")),
    ).rejects.toMatchObject({ category: "transport" });
    expect(client.retainedInput("input-0")).toEqual(edit("original-0"));
    responses.set("handoff", {
      ...complete("handoff", key("input-0")),
      result: {
        kind: "retained_handled",
        retained: key("input-0"),
        action: "handed_off",
      },
    });
    await client.result("handoff");
    expect(client.retainedInput("input-0")).toBeDefined();
    invoke.mockRejectedValueOnce(new Error("lost ack response"));
    await expect(client.acknowledgeTransport("handoff")).rejects.toMatchObject({
      category: "transport",
    });
    expect(client.retainedInput("handoff")).toBeDefined();
    expect(client.retainedInput("input-0")).toBeDefined();
    invoke.mockRejectedValueOnce({
      code: "unknown_id",
      nextAction: "check state",
    });
    await client.acknowledgeTransport("handoff");
    expect(client.retainedInput("handoff")).toBeUndefined();
    expect(client.retainedInput("input-0")).toBeUndefined();
    expect(client.retainedInput("input-1")).toEqual(edit("original-1"));
    await client.submit("replacement", edit());
    for (let n = 0; n < 8; n++) await client.close(`recovery-${n}`, "project");
  });

  it("observes and acknowledges wrong-reference rejections without leaking control slots or changing originals", async () => {
    const { client, invoke, responses, calls } = mockClient();
    await fill(client);
    for (let n = 0; n < 8; n++) {
      invoke.mockRejectedValueOnce({
        code: "unknown_id",
        nextAction: "check reference",
      });
      await expect(
        client.abandonRetained(`bad-${n}`, key("wrong")),
      ).rejects.toMatchObject({ category: "boundary" });
      responses.set(`bad-${n}`, {
        ...complete(`bad-${n}`),
        state: "rejected",
        result: {
          kind: "rejected",
          error: { code: "unknown_id", nextAction: "check reference" },
          input_retained: false,
        },
      });
    }
    await expect(client.close("ninth", "project")).rejects.toMatchObject({
      category: "protocol",
    });
    for (let n = 0; n < 8; n++) {
      expect((await client.result(`bad-${n}`)).state).toBe("rejected");
      await client.acknowledgeTransport(`bad-${n}`);
      expect(client.retainedInput(`bad-${n}`)).toBeUndefined();
    }
    await client.close("ninth", "project");
    expect(client.retainedInput("input-0")).toEqual(edit("original-0"));
    expect(
      calls.filter(
        (c) => c.action === "submit" && c.input.kind === "update_template",
      ),
    ).toHaveLength(40);
    invoke.mockRejectedValueOnce({
      code: "unknown_id",
      nextAction: "reserve again",
    });
    await expect(client.result("ninth")).rejects.toMatchObject({
      category: "boundary",
    });
    expect(client.retainedInput("ninth")).toBeUndefined();
    for (let n = 0; n < 8; n++) {
      invoke.mockRejectedValueOnce(new Error("not delivered"));
      await expect(
        client.close(`unsent-${n}`, "project"),
      ).rejects.toMatchObject({ category: "transport" });
    }
    responses.set("unsent-1", {
      ...complete("unsent-1"),
      state: "reserved",
      result: null,
    });
    await client.result("unsent-1");
    expect(client.retainedInput("unsent-1")).toBeDefined();
    invoke.mockRejectedValueOnce({
      code: "unknown_id",
      nextAction: "reserve again",
    });
    await expect(client.result("unsent-0")).rejects.toMatchObject({
      category: "boundary",
    });
    await client.close("replacement-control", "project");
    await expect(client.close("still-eight", "project")).rejects.toMatchObject({
      category: "protocol",
    });
    invoke.mockRejectedValueOnce({
      code: "unknown_id",
      nextAction: "check state",
    });
    await expect(client.result("input-0")).rejects.toMatchObject({
      category: "boundary",
    });
    expect(client.retainedInput("input-0")).toEqual(edit("original-0"));
    await client.result("replacement-control");
    invoke.mockRejectedValueOnce({
      code: "unknown_id",
      nextAction: "check state",
    });
    await expect(client.result("replacement-control")).rejects.toMatchObject({
      category: "boundary",
    });
    expect(client.retainedInput("replacement-control")).toBeDefined();
  });

  it("rejects mismatched retained generations and actions before they can delete another input", async () => {
    const { client, responses } = mockClient();
    await client.submit("original", edit());
    responses.set("original", complete("original", key("original")));
    await client.result("original");
    await client.abandonRetained("abandon", key("original"));
    for (const result of [
      {
        kind: "retained_handled" as const,
        retained: { ...key("original"), generation: "other" },
        action: "abandoned" as const,
      },
      {
        kind: "retained_handled" as const,
        retained: key("original"),
        action: "handed_off" as const,
      },
    ]) {
      responses.set("abandon", { ...complete("abandon"), result });
      await expect(client.result("abandon")).rejects.toMatchObject({
        category: "protocol",
      });
      expect(client.retainedInput("original")).toEqual(edit());
    }
    responses.set("abandon", {
      ...complete("abandon"),
      result: {
        kind: "retained_handled",
        retained: key("original"),
        action: "abandoned",
      },
    });
    await client.result("abandon");
    await client.acknowledgeTransport("abandon");
    expect(client.retainedInput("original")).toBeUndefined();
    await client.handoffRetained("handoff-owner", key("original"));
    responses.set("handoff-owner", {
      ...complete("handoff-owner", key("different")),
      result: {
        kind: "retained_handled",
        retained: key("original"),
        action: "handed_off",
      },
    });
    await expect(client.result("handoff-owner")).rejects.toMatchObject({
      category: "protocol",
    });
  });

  it("does not turn pending or uncertain edits into disposable controls or implicit abandonment", async () => {
    const { client, responses, calls } = mockClient();
    await client.submit("pending", edit());
    responses.set("pending", { ...complete("pending"), state: "pending" });
    await client.result("pending");
    await client.acknowledgeTransport("pending");
    expect(client.retainedInput("pending")).toEqual(edit());
    responses.set("pending", {
      ...complete("pending", key("pending")),
      result: {
        kind: "write",
        session: "session",
        artifact: null,
        disk: "uncertain",
        changed: null,
        warnings: [],
        cleanup_failed: true,
        recovery_required: true,
        error: null,
        diagnostic: {
          stage: "write",
          category: null,
          sessionState: "Blocked",
          lockCategory: null,
          nextAction: "preserve",
        },
      },
    });
    await client.result("pending");
    await client.acknowledgeTransport("pending");
    const unsubscribe = client.subscribe(vi.fn(), vi.fn());
    unsubscribe();
    await client.shutdown();
    expect(client.retainedInput("pending")).toEqual(edit());
    expect(
      calls.some(
        (c) => c.action === "submit" && c.input.kind === "abandon_retained",
      ),
    ).toBe(false);
  });

  it("ignores late result and acknowledgement bookkeeping for a replaced local entry", async () => {
    const { client, invoke, responses } = mockClient();
    await client.close("id", "old");
    await client.result("id");
    let resolveAck: ((r: Response) => void) | undefined;
    invoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveAck = resolve;
        }),
    );
    const lateAck = client.acknowledgeTransport("id");
    await client.acknowledgeTransport("id");
    await client.submit("id", edit("new"));
    resolveAck?.({ kind: "acknowledged" });
    await lateAck;
    expect(client.retainedInput("id")).toEqual(edit("new"));
    let resolveResult: ((r: Response) => void) | undefined;
    invoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveResult = resolve;
        }),
    );
    const lateResult = client.result("id");
    await client.result("id");
    await client.acknowledgeTransport("id");
    await client.submit("id", edit("latest"));
    resolveResult?.(complete("id", key("stale")));
    await lateResult;
    expect(client.retainedReference("id")).toBeUndefined();
    expect(client.retainedInput("id")).toEqual(edit("latest"));
    responses.set("id", complete("id", key("shared")));
    await client.result("id");
    const handled = (operation: string): Operation => ({
      ...complete(operation),
      result: {
        kind: "retained_handled",
        retained: key("shared"),
        action: "abandoned",
      },
    });
    await client.abandonRetained("old-transfer", key("shared"));
    responses.set("old-transfer", handled("old-transfer"));
    await client.result("old-transfer");
    invoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveAck = resolve;
        }),
    );
    const transferAck = client.acknowledgeTransport("old-transfer");
    await client.abandonRetained("new-transfer", key("shared"));
    responses.set("new-transfer", handled("new-transfer"));
    await client.result("new-transfer");
    await client.acknowledgeTransport("new-transfer");
    await client.submit("id", edit("replacement after transfer"));
    await client.result("id");
    resolveAck?.({ kind: "acknowledged" });
    await transferAck;
    expect(client.retainedInput("id")).toEqual(
      edit("replacement after transfer"),
    );
  });
});
