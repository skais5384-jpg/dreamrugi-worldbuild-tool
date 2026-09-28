import { render } from "../test/render";
import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import App from "./App";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";
import { deletionMessage } from "./TemplateManagement";
import type { ResultDto } from "../bridge/types";
import { text } from "../strings";

async function setup(picker?: () => Promise<string | null>) {
  const transport = new TestTransport();
  const controller = new TemplateController(
    new GuardedClient(transport),
    picker,
  );
  controller.start();
  await waitFor(() => expect(controller.snapshot().ready).toBe(true));
  await controller.checkStatus();
  return { controller, transport };
}
async function saved() {
  const { controller: c, transport: t } = await setup();
  c.setRoot("fixture");
  await c.open();
  await c.navigate({ kind: "create" });
  c.setName(" 같은 이름\n🌿 ");
  await c.save();
  return { c, t, source: c.snapshot().selection! };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((r, j) => {
    resolve = r;
    reject = j;
  });
  return { promise, resolve, reject };
}

it("Active와 Deleted 복제는 저장된 원 view 하나를 제출하고 확정 결과를 별도로 조회한다", async () => {
  for (const lifecycle of ["Active", "Deleted"]) {
    const { c, t, source } = await saved();
    t.templates.set(source.content.id, { ...source.content, lifecycle });
    await c.refresh();
    await c.navigate({ kind: "duplicate" });
    const view = c.snapshot().selection!.view;
    expect(t.writes).toHaveLength(1);
    await Promise.all([c.executeTemplateAction(), c.executeTemplateAction()]);
    expect(t.writes).toHaveLength(2);
    expect(t.writes[1].input).toEqual({
      kind: "duplicate_template",
      project: t.project,
      view,
    });
    expect(c.snapshot().selection!.content).toMatchObject({
      name: source.content.name,
      lifecycle: "Active",
    });
    expect(c.snapshot().templateAction!.artifact).not.toBe(source.content.id);
    expect(t.templates.get(source.content.id)!.lifecycle).toBe(lifecycle);
    expect(t.sessions.size).toBe(0);
  }
});
it.each(["full", "lost"])(
  "복제 %s에서 같은 operation만 유지하고 자동 복제를 만들지 않는다",
  async (mode) => {
    const { c, t } = await saved();
    await c.navigate({ kind: "duplicate" });
    if (mode === "full") t.refuseWriteOnce = true;
    else {
      t.loseSubmit = true;
      t.loseResult = true;
    }
    const pending = c.executeTemplateAction();
    await waitFor(() => expect(c.snapshot().operations).toHaveLength(1));
    if (mode === "full") {
      await waitFor(() =>
        expect(c.snapshot().operations[0].phase).toBe("reserved"),
      );
      expect(t.writes).toHaveLength(1);
      await c.executeTemplateAction();
      await c.operations.resubmit(c.snapshot().operations[0].id);
    } else await c.checkStatus();
    await pending;
    expect(t.writes).toHaveLength(2);
    expect(t.templates.size).toBe(2);
  },
);
it("확정 복제 뒤 읽기 실패는 ID와 성공 사실을 보존하고 조회만 재시도한다", async () => {
  const { c, t, source } = await saved();
  await c.navigate({ kind: "duplicate" });
  t.failRead = true;
  t.failList = true;
  await c.executeTemplateAction();
  const result = c.snapshot().templateAction!;
  expect(result.result).toMatchObject({ kind: "write", disk: "committed" });
  expect(result.artifact).toBeTruthy();
  expect(c.snapshot().selection!.content.id).toBe(source.content.id);
  await c.executeTemplateAction();
  expect(t.writes).toHaveLength(2);
  t.failRead = false;
  t.failList = false;
  await c.readTemplateAction();
  expect(c.snapshot().selection!.content.id).toBe(result.artifact);
  expect(t.writes).toHaveLength(2);
});
it("늦은 복제 결과는 종료 중 선택을 덮지 않고 결과 owner만 인수한다", async () => {
  const { c, t, source } = await saved();
  await c.navigate({ kind: "duplicate" });
  t.hold = "duplicate_template";
  const pending = c.executeTemplateAction();
  await waitFor(() => expect(c.snapshot().operations).toHaveLength(1));
  await c.requestClose();
  expect(c.snapshot().closing).toBe(true);
  t.completeHeld();
  await c.checkStatus();
  await pending;
  expect(c.snapshot().selection!.content.id).toBe(source.content.id);
  expect(c.snapshot().templateAction!.artifact).toBeTruthy();
  expect(t.writes).toHaveLength(2);
});
it("삭제 세션의 늦은 인수 뒤 종료 중에는 새 tombstone을 제출하지 않는다", async () => {
  const { c, t, source } = await saved();
  await c.navigate({ kind: "delete" });
  t.hold = "begin_session";
  const pending = c.executeTemplateAction();
  await waitFor(() => expect(c.snapshot().operations).toHaveLength(1));
  await c.requestClose();
  t.completeHeld();
  await c.checkStatus();
  await pending;
  expect(t.writes).toHaveLength(1);
  expect(t.templates.get(source.content.id)!.lifecycle).toBe("Active");
  expect(t.sessions.size).toBe(0);
  expect(c.snapshot().templateAction!.phase).toBe("complete");
});
it("미제출 이름에서 삭제 이동 취소는 입력을 보존하고 새 view는 원 삭제 확인을 무효화한다", async () => {
  const { c, t } = await saved();
  await c.rename();
  c.setName("아직 제출하지 않은 이름");
  await c.navigate({ kind: "delete" });
  expect(c.snapshot().prompt).toBeTruthy();
  await c.decide(false);
  expect(c.snapshot().form!.name).toBe("아직 제출하지 않은 이름");
  await c.navigate({ kind: "delete" });
  await c.decide(true);
  expect(c.snapshot().templateAction!.source.content.name).not.toBe(
    "아직 제출하지 않은 이름",
  );
  await c.refresh();
  await c.executeTemplateAction();
  expect(t.writes).toHaveLength(1);
  expect(c.snapshot().templateAction!.message).toBe(
    text("template.sourceChanged"),
  );
});
it("렌더링 중 이동으로 열린 확인은 busy 해제 뒤 취소에 초점을 주고 Escape로 닫힌다", async () => {
  const { c, t } = await saved();
  render(<App controller={c} />);
  fireEvent.click(
    screen.getByRole("button", { name: text("template.delete") }),
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: text("common.cancel") }),
    ).toHaveFocus(),
  );
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  await waitFor(() => expect(c.snapshot().templateAction).toBeNull());
  expect(t.writes).toHaveLength(1);
});
it("FIX2 조합 종료 뒤 click은 직접 삭제 callback도 막고 다음 포인터 확정은 허용한다", async () => {
  const { c, t } = await saved();
  await c.navigate({ kind: "delete" });
  render(<App controller={c} />);
  const confirm = await screen.findByRole("button", { name: "삭제 확정" });
  await waitFor(() => expect(confirm).toBeEnabled());
  fireEvent.compositionStart(confirm);
  expect(
    fireEvent.keyDown(confirm, {
      key: "Process",
      code: "",
      keyCode: 229,
      isComposing: true,
    }),
  ).toBe(true);
  fireEvent.compositionEnd(confirm);
  fireEvent.click(confirm, { detail: 0 });
  await waitFor(() => expect(c.snapshot().busy).toBe(false));
  expect(t.writes).toHaveLength(1);
  fireEvent.click(confirm, { detail: 1 });
  await waitFor(() => expect(t.writes).toHaveLength(2));
  expect(t.writes[1].input.kind).toBe("tombstone_template");
});
it("F1 삭제 확인의 Escape/IME Enter는 저장하지 않고 확정만 tombstone을 제출한다", async () => {
  const { c, t } = await saved();
  await c.navigate({ kind: "delete" });
  render(<App controller={c} />);
  const confirm = screen.getByRole("button", {
    name: text("template.confirmDelete"),
  });
  fireEvent.compositionStart(confirm);
  fireEvent.keyDown(confirm, { key: "Enter", isComposing: true, keyCode: 229 });
  fireEvent.click(confirm);
  expect(t.writes).toHaveLength(1);
  fireEvent.compositionEnd(confirm);
  expect(
    fireEvent.keyDown(confirm, {
      key: "Enter",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    }),
  ).toBe(false);
  expect(
    fireEvent.keyDown(confirm, {
      key: "Process",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    }),
  ).toBe(false);
  fireEvent.keyDown(confirm, { key: "Escape" });
  await waitFor(() => expect(c.snapshot().templateAction).toBeNull());
  expect(t.writes).toHaveLength(1);
  await act(() => c.navigate({ kind: "delete" }));
  fireEvent.click(
    screen.getByRole("button", { name: text("template.confirmDelete") }),
  );
  await waitFor(() =>
    expect(c.snapshot().selection!.content.lifecycle).toBe("Deleted"),
  );
  expect(
    screen.queryByRole("button", {
      name: text("template.delete"),
    }),
  ).not.toBeInTheDocument();
  expect(t.writes[1].input.kind).toBe("tombstone_template");
  expect(
    screen.queryByRole("button", { name: text("field.add") }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: text("order.fields") }),
  ).not.toBeInTheDocument();
});
it("참조 거부는 대상·retained를 유지하고 안전한 count만 표시한다", async () => {
  const { c, t, source } = await saved();
  t.deletion = { reason: "template_has_documents", count: 1, truncated: false };
  await c.navigate({ kind: "delete" });
  await c.executeTemplateAction();
  expect(c.snapshot().retainedRefs).toHaveLength(1);
  expect(c.snapshot().templateAction!.source).toBe(source);
  expect(t.templates.get(source.content.id)!.lifecycle).toBe("Active");
  render(<App controller={c} />);
  expect(
    screen.getByText(text("template.references", { count: "1" })),
  ).toBeInTheDocument();
  expect(document.body.textContent).not.toContain("RAW_CANARY");
  await act(() => c.abandon(c.snapshot().retainedRefs[0].id));
  await act(() => c.navigate({ kind: "cancel" }));
  expect(c.snapshot().templateAction).toBeNull();
  expect(c.snapshot().retainedRefs).toHaveLength(0);
});
it("상한·누락·잘못된 참조 수와 scan/stale는 정확한 총수 0으로 보정하지 않는다", () => {
  const write = { kind: "write" } as Extract<ResultDto, { kind: "write" }>;
  for (const count of [0, -1, 1025, NaN, undefined])
    expect(
      deletionMessage({
        ...write,
        deletion: {
          reason: "template_has_documents",
          count: count as number,
          truncated: false,
        },
      }),
    ).toBe(text("template.checkFailed"));
  expect(
    deletionMessage({
      ...write,
      deletion: {
        reason: "template_has_documents",
        count: 1024,
        truncated: true,
      },
    }),
  ).toBe(text("template.referencesTruncated"));
  expect(
    deletionMessage({
      ...write,
      deletion: { reason: "reference_check_failed" },
    }),
  ).toBe(text("template.checkFailed"));
  expect(
    deletionMessage({ ...write, deletion: { reason: "source_changed" } }),
  ).toBe(text("template.sourceChanged"));
});
it("picker는 중복·동시 open을 막고 선택만 반영하며 취소와 실패 뒤 입력을 유지한다", async () => {
  const gate = deferred<string | null>(),
    picker = vi.fn(() => gate.promise);
  const { controller: c, transport: t } = await setup(picker);
  c.setRoot("기존 경로");
  const pending = c.pickFolder();
  await c.pickFolder();
  await c.open();
  expect(picker).toHaveBeenCalledTimes(1);
  expect(
    t.commands.some(
      (x) =>
        x.action === "submit" &&
        (x.input.kind === "open" || x.input.kind === "create_project"),
    ),
  ).toBe(false);
  gate.resolve("한글 공백 폴더");
  expect(await pending).toBe(true);
  expect(c.snapshot().root).toBe("한글 공백 폴더");
  expect(c.snapshot().projectId).toBeNull();
  picker.mockResolvedValueOnce(null);
  await c.pickFolder();
  expect(c.snapshot().root).toBe("한글 공백 폴더");
  picker.mockRejectedValueOnce(Error("PRIVATE_PATH_CANARY"));
  await c.pickFolder();
  expect(c.snapshot().error).toBe(text("picker.failed"));
  expect(c.snapshot().picking).toBe(false);
});
it.each(["input", "close", "error"])(
  "picker 대기 중 %s 뒤 늦은 응답은 현재 입력·오류를 덮지 않는다",
  async (mode) => {
    const gate = deferred<string | null>();
    const { controller: c } = await setup(() => gate.promise);
    c.setRoot("이전 입력");
    const pending = c.pickFolder();
    if (mode === "close") await c.requestClose();
    else c.setRoot("새 입력");
    const before = c.snapshot();
    if (mode === "error") gate.reject(Error("PRIVATE_CANARY"));
    else gate.resolve("늦은 선택");
    expect(await pending).toBe(false);
    expect(c.snapshot().root).toBe(before.root);
    expect(c.snapshot().error).toBe(before.error);
    expect(c.snapshot().picking).toBe(false);
  },
);
it("정상 Stopped 보고와 초기화 실패 Stopped를 같은 UI에서 구분한다", async () => {
  const { c, t } = await saved();
  t.runtime = null;
  t.projectStopped = true;
  t.reportPending = true;
  await c.checkStatus();
  render(<App controller={c} />);
  expect(screen.getByText(text("project.stopped"))).toBeInTheDocument();
  expect(
    screen.queryByText(text("project.initializationFailed")),
  ).not.toBeInTheDocument();
  t.initializationFailed = true;
  await act(() => c.checkStatus());
  expect(
    screen.getByText(text("project.initializationFailed")),
  ).toBeInTheDocument();
  expect(screen.queryByText(text("project.stopped"))).not.toBeInTheDocument();
});
