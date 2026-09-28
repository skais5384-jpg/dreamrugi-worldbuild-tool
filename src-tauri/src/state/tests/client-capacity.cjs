// Rust 정식 host 하나가 실행한다. 설치된 TypeScript로 제품 client 자체를 메모리에서 변환한다.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const crypto = require("node:crypto");
const root = path.resolve(__dirname, "../../../..");
const req = Module.createRequire(path.join(root, "package.json"));
const filename = path.join(root, "src/bridge/client.ts");
const bytes = fs.readFileSync(filename);
const ts = req("typescript");
const code = ts.transpileModule(bytes.toString("utf8"), {
  compilerOptions: {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.CommonJS,
  },
}).outputText;
const moduleUnderTest = new Module(filename);
moduleUnderTest.paths = Module._nodeModulePaths(root);
moduleUnderTest._compile(code, filename);
const { GuardedClient } = moduleUnderTest.exports;
const lines = require("node:readline").createInterface({
  input: process.stdin,
});
const replies = lines[Symbol.asyncIterator]();
let submits = 0;
let loseSubmit = false;
let loseAck = false;
let failBeforeSubmit = false;
const transport = {
  listen: async () => () => {},
  invoke: async (name, body) => {
    assert.equal(name, "guarded");
    const command = JSON.parse(new TextDecoder().decode(body));
    if (command.action === "submit" && failBeforeSubmit) {
      failBeforeSubmit = false;
      throw Error("test request not delivered");
    }
    if (command.action === "submit") submits++;
    process.stdout.write(JSON.stringify({ command }) + "\n");
    const line = await replies.next();
    assert.equal(line.done, false, "production host disconnected");
    const response = JSON.parse(line.value);
    if (!response.ok) throw response.value;
    // 실제 backend 처리 뒤 응답만 유실시킨다. 재전송도 반드시 같은 공식 client를 통한다.
    if (command.action === "submit" && loseSubmit) {
      loseSubmit = false;
      throw Error("test lost submit reply");
    }
    if (command.action === "acknowledge_transport" && loseAck) {
      loseAck = false;
      throw Error("test lost acknowledgement reply");
    }
    return response.value;
  },
};
const deadline = setTimeout(() => {
  console.error("actual client integration deadline");
  process.exit(1);
}, 60000);
async function main() {
  const count = Number(process.argv[2]);
  const action = process.argv[3];
  const closing = process.argv[4] === "closing";
  const input = JSON.parse(process.env.WB_CLIENT_INPUT);
  const client = new GuardedClient(transport);
  const originals = [];
  const keys = [];
  const snapshots = [];
  const localCount = () =>
    originals.filter((id) => client.retainedInput(id)).length;
  for (let i = 0; i < count; i++) {
    const id = await client.reserve();
    await client.submit(id, input);
    const result = await client.result(id);
    assert.notEqual(result.result.disk, "committed");
    assert.ok(result.retained);
    originals.push(id);
    keys.push(client.retainedReference(id));
    if (i < 32) await client.acknowledgeTransport(id);
    const snapshot = await client.readRetained(keys[i]);
    assert.deepEqual(snapshot.intent, {
      kind: "update_template",
      revision: "1",
      edit: input.edit,
    });
    assert.equal(snapshot.g6_clearable, true);
    snapshots.push(snapshot);
  }
  const before = await client.appStatus();
  assert.equal(before.retained_edits, "32");
  assert.equal(before.operations, String(count - 32));
  assert.equal(before.closing, false);
  assert.equal(localCount(), count);
  assert.equal(client.inputs.size, count); // 읽기 전용 점유 관찰이며 map 변경은 제품 client만 한다.
  assert.equal(client.controls.size, 0);
  async function unchanged(start = 0) {
    for (let i = start; i < count; i++) {
      assert.deepEqual(client.retainedInput(originals[i]), input);
      assert.deepEqual(await client.readRetained(keys[i]), snapshots[i]);
    }
  }
  // 잘못된 reference/generation, 없는 project의 제어도 terminal 거부를 인수하면 회수된다.
  for (const bad of [
    {
      kind: "abandon_retained",
      retained: { ...keys[0], generation: crypto.randomUUID() },
    },
    {
      kind: "retained_handoff",
      retained: { id: crypto.randomUUID(), generation: crypto.randomUUID() },
    },
    { kind: "close", project: crypto.randomUUID() },
  ]) {
    const id = await client.reserve("control");
    await assert.rejects(
      client.submit(id, bad),
      (e) => e.boundary?.code === "unknown_id",
    );
    await client.submit(id, bad);
    const rejected = await client.result(id);
    assert.equal(rejected.state, "rejected");
    assert.equal(rejected.result.error.code, "unknown_id");
    await client.acknowledgeTransport(id);
    assert.equal(client.retainedInput(id), undefined);
  }
  await unchanged();
  // 제어 전송 전 실패8 + 아홉 번째 예약의 실제 빈 ID eviction도 공식 조회로 회수한다.
  const unsent = [];
  const invalid = {
    kind: "abandon_retained",
    retained: { ...keys[0], generation: crypto.randomUUID() },
  };
  for (let n = 0; n < 8; n++) {
    const pending = await client.reserve("control");
    failBeforeSubmit = true;
    await assert.rejects(
      client.submit(pending, invalid),
      (e) => e.category === "transport",
    );
    unsent.push(pending);
  }
  const ninth = await client.reserve("control");
  const sentBefore = submits;
  await assert.rejects(
    client.submit(ninth, invalid),
    (e) => e.category === "protocol",
  );
  assert.equal(submits, sentBefore);
  let evicted = 0;
  for (const pending of unsent) {
    try {
      assert.equal((await client.result(pending)).state, "reserved");
      assert.ok(client.retainedInput(pending));
    } catch (error) {
      assert.equal(error.boundary?.code, "unknown_id");
      assert.equal(client.retainedInput(pending), undefined);
      evicted++;
    }
  }
  assert.equal(evicted, 1);
  for (const pending of [
    ...unsent.filter((id) => client.retainedInput(id)),
    ninth,
  ]) {
    await assert.rejects(
      client.submit(pending, invalid),
      (e) => e.boundary?.code === "unknown_id",
    );
    assert.equal((await client.result(pending)).state, "rejected");
    await client.acknowledgeTransport(pending);
  }
  assert.equal(client.controls.size, 0);
  await unchanged();
  async function join() {
    const end = Date.now() + 20000;
    for (;;) {
      const status = await client.projectStatus(input.project);
      await client.acknowledgeShutdown(input.project);
      if (status.shutdown.joined) return;
      assert.ok(Date.now() < end, "project join deadline");
      await new Promise((resolve) => setImmediate(resolve));
    }
  }
  if (closing) {
    const end = await client.reserve("control");
    await client.sessionControl(end, {
      project: input.project,
      session: input.session,
      control: "end",
    });
    assert.equal((await client.result(end)).result.error, null);
    await client.acknowledgeTransport(end);
    await client.shutdown();
    await join();
    await assert.rejects(
      client.reserve(),
      (e) => e.boundary?.code === "closed",
    );
    await unchanged();
    assert.equal(localCount(), 40);
  }
  const id = await client.reserve("control");
  const submit = () =>
    action === "abandon"
      ? client.abandonRetained(id, keys[0])
      : client.handoffRetained(id, keys[0]);
  const transmissions = submits;
  loseSubmit = true;
  await assert.rejects(submit(), (e) => e.category === "transport");
  assert.equal(localCount(), count);
  await submit();
  const handled = await client.result(id);
  assert.deepEqual(handled.result, {
    kind: "retained_handled",
    retained: keys[0],
    action: action === "abandon" ? "abandoned" : "handed_off",
  });
  assert.deepEqual((await client.result(id)).result, handled.result);
  assert.equal(submits - transmissions, 2);
  assert.equal(localCount(), count);
  if (action === "handoff")
    assert.deepEqual(await client.readRetained(keys[0]), snapshots[0]);
  else
    await assert.rejects(
      client.readRetained(keys[0]),
      (e) => e.boundary?.code === "unknown_id",
    );
  await unchanged(1);
  loseAck = true;
  await assert.rejects(
    client.acknowledgeTransport(id),
    (e) => e.category === "transport",
  );
  assert.equal(localCount(), count);
  assert.ok(client.retainedInput(id));
  await client.acknowledgeTransport(id);
  assert.equal(localCount(), count - 1);
  assert.equal(client.retainedInput(id), undefined);
  assert.equal(client.controls.size, 0);
  await unchanged(1);
  if (action === "handoff")
    assert.deepEqual(await client.readRetained(keys[0]), snapshots[0]);
  // 합성 fixture의 확정 미저장 G6만 명시적으로 포기한다. 종료/ack는 이를 대신하지 않는다.
  for (const retained of await client.listRetained()) {
    const cleanup = await client.reserve("control");
    await client.abandonRetained(cleanup, retained);
    assert.equal((await client.result(cleanup)).result.action, "abandoned");
    await client.acknowledgeTransport(cleanup);
  }
  for (const original of originals.slice(32))
    await client.acknowledgeTransport(original);
  await client.shutdown();
  await join();
  await client.releaseView(input.project, input.view);
  const retire = await client.reserve("control");
  await client.retireProject(retire, input.project);
  assert.equal((await client.result(retire)).result.kind, "project_retired");
  await client.acknowledgeTransport(retire);
  const final = await client.appStatus();
  assert.equal(final.projects, "0");
  assert.equal(final.operations, "0");
  assert.equal(final.retained_edits, "0");
  assert.equal(final.normal_exit_allowed, true);
  assert.equal(client.inputs.size, 0);
  assert.equal(client.controls.size, 0);
  assert.equal(client.results.size, 0);
  assert.equal(client.owners.size, 0);
  assert.equal(client.acknowledgements.size, 0);
  process.stdout.write(
    JSON.stringify({
      done: true,
      summary: {
        count,
        action,
        closing,
        client_source: {
          bytes: bytes.length,
          sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
        },
        before,
        main_submit_transmissions: 2,
        same_id_effects: 1,
        wrong_reference_rejections: 3,
        unsent_controls: 8,
        evicted_empty_reservations: 1,
        final,
      },
    }) + "\n",
  );
}
main()
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  })
  .finally(() => {
    clearTimeout(deadline);
    lines.close();
    process.stdin.destroy();
  });
