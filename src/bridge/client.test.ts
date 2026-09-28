import { describe, expect, it, vi } from "vitest";
import { BridgeFailure, GuardedClient } from "./client";
import type { Transport } from "./client";
import type { Command, Hint, Response, Work } from "./types";

const id = "99999999-9999-4999-8999-000000000001";
function transport() {
  const calls: Command[] = [];
  const invoke = vi.fn(
    async (_command: "guarded", bytes: Uint8Array): Promise<Response> => {
      const command = JSON.parse(new TextDecoder().decode(bytes)) as Command;
      calls.push(command);
      switch (command.action) {
        case "reserve":
          return { kind: "reserved", operation: id };
        case "submit":
          return { kind: "submitted", operation: command.operation };
        case "operation":
          return {
            kind: "operation",
            operation: command.operation,
            state: "complete",
            retained: null,
            result: { kind: "control", error: null },
          };
        default:
          return { kind: "acknowledged" };
      }
    },
  );
  const listen = vi
    .fn<Transport["listen"]>()
    .mockResolvedValue(() => undefined);
  return { transport: { invoke, listen }, calls };
}
describe("G13 thin client", () => {
  it("reclaims 41 no-draft responses using backend ownership without abandoning input", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    for (let n = 0; n < 41; n++) {
      const operation = `${id}-${n}`;
      await client.sessionControl(operation, {
        project: id,
        session: id,
        control: "return_active",
      });
      mock.transport.invoke.mockResolvedValueOnce({
        kind: "operation",
        operation,
        state: "complete",
        retained: null,
        result: {
          kind: "control",
          error: { code: "no_draft", nextAction: "check state" },
        },
      });
      await client.result(operation);
      await client.acknowledgeTransport(operation);
      expect(client.retainedInput(operation)).toBeUndefined();
    }
    expect(
      mock.calls.some(
        (c) => c.action === "submit" && c.input.kind === "abandon_retained",
      ),
    ).toBe(false);
  });
  it("keeps retained identity after acknowledgement and uses explicit typed handoff and abandonment", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    const retained = { id, generation: id };
    await client.updateTemplate(id, {
      project: id,
      session: id,
      view: id,
      revision: "1",
      edit: { kind: "name", name: "kept input" },
    });
    mock.transport.invoke.mockResolvedValueOnce({
      kind: "operation",
      operation: id,
      state: "complete",
      retained,
      result: {
        kind: "rejected",
        error: { code: "save_rejected", nextAction: "choose" },
        input_retained: true,
      },
    });
    await client.result(id);
    await client.acknowledgeTransport(id);
    expect(client.retainedReference(id)).toEqual(retained);
    mock.transport.invoke.mockResolvedValueOnce({
      kind: "retained_list",
      entries: [retained],
    });
    expect(await client.listRetained()).toEqual([retained]);
    const projection: Extract<Response, { kind: "retained" }> = {
      kind: "retained",
      retained,
      project: id,
      artifacts: [id],
      intent: {
        kind: "update_template",
        revision: "1",
        edit: { kind: "name", name: "kept input" },
      },
      result: { kind: "control", error: null },
      g6_clearable: true,
    };
    mock.transport.invoke.mockResolvedValueOnce(projection);
    expect(await client.readRetained(retained)).toEqual(projection);
    const stop = client.subscribe(vi.fn(), vi.fn());
    stop();
    expect(
      mock.calls.some(
        (c) => c.action === "submit" && c.input.kind === "abandon_retained",
      ),
    ).toBe(false);
    const handoff = `${id}-handoff`;
    mock.transport.invoke.mockRejectedValueOnce(
      new Error("lost handoff reply"),
    );
    await expect(
      client.handoffRetained(handoff, retained),
    ).rejects.toMatchObject({ category: "transport", operation: handoff });
    expect(client.retainedInput(id)).toBeDefined();
    mock.transport.invoke.mockResolvedValueOnce({
      kind: "operation",
      operation: handoff,
      state: "complete",
      retained,
      result: { kind: "retained_handled", retained, action: "handed_off" },
    });
    await client.result(handoff);
    await client.acknowledgeTransport(handoff);
    expect(client.retainedInput(id)).toBeUndefined();
    const abandon = `${id}-abandon`;
    await client.abandonRetained(abandon, retained);
    mock.transport.invoke.mockResolvedValueOnce({
      kind: "operation",
      operation: abandon,
      state: "complete",
      retained: null,
      result: { kind: "retained_handled", retained, action: "abandoned" },
    });
    await client.result(abandon);
    await client.acknowledgeTransport(abandon);
    expect(client.retainedInput(handoff)).toBeUndefined();
    expect(client.retainedInput(abandon)).toBeUndefined();
    expect(
      mock.transport.invoke.mock.calls.every(
        ([command]) => command === "guarded",
      ),
    ).toBe(true);
  });
  it("uses explicit current destination and narrow native retry without replay from hints", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    const retained = { id, generation: id };
    const destination = {
      kind: "update_template" as const,
      session: id,
      view: id,
      revision: "2",
    };
    await client.resumeRetained(id, retained, id, destination);
    await client.retireProject(`${id}-retire`, id);
    await client.retryNativeCleanup("2");
    expect(mock.calls).toEqual([
      {
        action: "submit",
        operation: id,
        input: { kind: "resume_retained", retained, project: id, destination },
      },
      {
        action: "submit",
        operation: `${id}-retire`,
        input: { kind: "retire_project", project: id },
      },
      { action: "retry_native_cleanup", generation: "2" },
    ]);
  });
  it("reserves before submit and retains exact number lexemes and edit-start revision", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    const operation = await client.reserve();
    const input: Work = {
      kind: "save_document",
      project: id,
      session: id,
      template: id,
      document: id,
      revision: "4294967295",
      edits: [
        {
          kind: "set",
          field: id,
          value: { kind: "number", value: "9007199254740993.0000000000000001" },
        },
      ],
    };
    await client.saveDocument(operation, input);
    expect(mock.calls).toEqual([
      { action: "reserve", lane: "ordinary" },
      { action: "submit", operation: id, input },
    ]);
    expect(
      mock.transport.invoke.mock.calls.every(
        ([command, bytes]) =>
          command === "guarded" &&
          ArrayBuffer.isView(bytes) &&
          bytes.BYTES_PER_ELEMENT === 1,
      ),
    ).toBe(true);
    input.revision = "1";
    expect(client.retainedInput(operation)).toMatchObject({
      revision: "4294967295",
    });
  });
  it("keeps the known ID and input after submit response loss without automatic resubmission", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    const operation = await client.reserve();
    mock.transport.invoke.mockRejectedValueOnce(
      new Error("PRIVATE_PATH_SENTINEL"),
    );
    const failure = await client
      .open(operation, "test-root")
      .catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(BridgeFailure);
    expect(failure).toMatchObject({ category: "transport", operation });
    expect(String(failure)).not.toContain("PRIVATE_PATH_SENTINEL");
    expect(client.retainedInput(operation)).toEqual({
      kind: "open",
      root: "test-root",
    });
    await client.result(operation);
    expect(mock.transport.invoke).toHaveBeenCalledTimes(3);
    expect(mock.calls[mock.calls.length - 1]).toEqual({
      action: "operation",
      operation,
    });
  });
  it("requeries a finished result after response loss and distinguishes domain rejection", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    await client.saveDocument(id, {
      project: id,
      session: id,
      template: id,
      document: id,
      revision: "1",
      edits: [{ kind: "rename", name: "retained" }],
    });
    mock.transport.invoke.mockRejectedValueOnce(new Error("dropped response"));
    await expect(client.result(id)).rejects.toMatchObject({
      category: "transport",
      operation: id,
    });
    mock.transport.invoke.mockResolvedValueOnce({
      kind: "operation",
      operation: id,
      state: "complete",
      retained: { id, generation: id },
      result: {
        kind: "rejected",
        error: { code: "sink_unavailable", nextAction: "keep input" },
        input_retained: true,
      },
    });
    expect((await client.result(id)).result).toMatchObject({
      kind: "rejected",
      error: { code: "sink_unavailable" },
    });
    await client.acknowledgeTransport(id);
    expect(client.retainedInput(id)).toBeDefined();
    mock.transport.invoke.mockRejectedValueOnce({
      code: "wrong_binding",
      nextAction: "keep input",
    });
    await expect(client.result(id)).rejects.toMatchObject({
      category: "boundary",
      boundary: { code: "wrong_binding" },
    });
  });
  it("rejects a changed duplicate locally while allowing explicit same-ID reconciliation", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    await client.open(id, "one");
    await expect(client.open(id, "two")).rejects.toMatchObject({
      category: "protocol",
    });
    expect(mock.transport.invoke).toHaveBeenCalledTimes(1);
    await client.open(id, "one");
    expect(mock.transport.invoke).toHaveBeenCalledTimes(2);
  });
  it("keeps transport acknowledgement distinct from durable acknowledgement and return-active", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    for (const control of [
      "preserve",
      "accept",
      "acknowledge_recovery",
      "return_active",
      "end",
      "retry_release",
      "revalidate",
    ] as const) {
      const operation = `${id}-${control}`;
      await client.sessionControl(operation, {
        project: id,
        session: id,
        control,
      });
    }
    await client.acknowledgeTransport(id);
    expect(mock.calls[mock.calls.length - 1]).toEqual({
      action: "acknowledge_transport",
      operation: id,
    });
    expect(
      mock.calls
        .slice(0, 7)
        .every(
          (c) => c.action === "submit" && c.input.kind === "session_control",
        ),
    ).toBe(true);
  });
  it("releases acknowledged non-edit control slots without accumulating a lifetime limit", async () => {
    const mock = transport();
    const client = new GuardedClient(mock.transport);
    for (let index = 0; index < 50; index++) {
      const operation = `${id}-${index}`;
      await client.sessionControl(operation, {
        project: id,
        session: id,
        control: "revalidate",
      });
      await client.result(operation);
      await client.acknowledgeTransport(operation);
      expect(client.retainedInput(operation)).toBeUndefined();
    }
  });
  it("ignores stale hint generations above 2^53 and never saves or refreshes sources", async () => {
    const mock = transport();
    let receive: ((hint: Hint) => void) | undefined;
    const unlisten = vi.fn();
    mock.transport.listen.mockImplementation(async (_event, callback) => {
      receive = callback;
      return unlisten;
    });
    const client = new GuardedClient(mock.transport);
    const onHint = vi.fn();
    const onError = vi.fn();
    const stop = client.subscribe(onHint, onError);
    await Promise.resolve();
    for (const generation of [
      "9007199254740993",
      "9007199254740992",
      "9007199254740993",
      "9007199254740994",
      "-1",
      "01",
    ])
      receive?.({ generation });
    expect(onHint.mock.calls).toEqual([
      ["9007199254740993"],
      ["9007199254740994"],
    ]);
    expect(mock.transport.invoke).not.toHaveBeenCalled();
    stop();
    stop();
    receive?.({ generation: "9007199254740995" });
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(onHint).toHaveBeenCalledTimes(2);
    expect(onError).not.toHaveBeenCalled();
  });
  it("unlistens once when disposed before asynchronous registration finishes", async () => {
    const mock = transport();
    let ready: ((stop: () => void) => void) | undefined;
    mock.transport.listen.mockImplementation(
      () =>
        new Promise((resolve) => {
          ready = resolve;
        }),
    );
    const client = new GuardedClient(mock.transport);
    const stop = client.subscribe(vi.fn(), vi.fn());
    stop();
    const unlisten = vi.fn();
    ready?.(unlisten);
    await Promise.resolve();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
  it("reports listener failure without automatic retries or operation loss", async () => {
    const mock = transport();
    mock.transport.listen.mockRejectedValueOnce(new Error("PRIVATE_LISTENER"));
    const client = new GuardedClient(mock.transport);
    await client.open(id, "one");
    const onError = vi.fn();
    client.subscribe(vi.fn(), onError);
    await Promise.resolve();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(String(onError.mock.calls[0][0])).not.toContain("PRIVATE_LISTENER");
    expect(client.retainedInput(id)).toEqual({ kind: "open", root: "one" });
  });
});
