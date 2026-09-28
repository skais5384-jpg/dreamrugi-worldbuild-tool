import { render } from "../test/render";
import {
  act,
  createEvent,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { dialogSurfaceClassNames } from "@fluentui/react-components";

import App from "./App";
import { GuardedClient } from "../bridge/client";
import { TemplateController } from "./controller";
import { TestTransport } from "./testTransport";
import { text } from "../strings";

async function setup(transport = new TestTransport()) {
  const controller = new TemplateController(new GuardedClient(transport));
  const view = render(<App controller={controller} />);
  await waitFor(() => expect(controller.snapshot().ready).toBe(true));
  fireEvent.change(screen.getByLabelText("기존 프로젝트 폴더 경로"), {
    target: { value: "test-project" },
  });
  fireEvent.click(screen.getByRole("button", { name: "프로젝트 열기" }));
  await waitFor(() => expect(controller.snapshot().busy).toBe(false));
  return { transport, controller, view };
}
async function create(name = "첫 이름") {
  fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
  const input = await screen.findByLabelText("템플릿 이름");
  await waitFor(() => expect(input).toBeEnabled());
  fireEvent.change(input, { target: { value: name } });
  fireEvent.click(screen.getByRole("button", { name: "저장" }));
  await screen.findByRole("heading", {
    name: name || "(빈 이름)",
  });
}

function pressTab(dialog: HTMLElement, backwards = false) {
  const current = document.activeElement as HTMLElement;
  const event = createEvent.keyDown(current, {
    key: "Tab",
    shiftKey: backwards,
  });
  fireEvent(current, event);
  // jsdom은 중간 요소의 native Tab 이동을 실행하지 않는다. Fluent의
  // 경계 순환은 그대로 두고, 취소되지 않은 기본 이동만 DOM 순서로 모델링한다.
  if (!event.defaultPrevented) {
    const buttons = Array.from(
      dialog.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"),
    );
    const index = buttons.indexOf(current as HTMLButtonElement);
    buttons[
      (index + (backwards ? -1 : 1) + buttons.length) % buttons.length
    ]?.focus();
  }
}

describe("App", () => {
  it("확인창 밖 pointer 후 실제 focus의 Escape는 원 초안과 caret을 보존해 한 번 취소한다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = (await screen.findByLabelText(
      "템플릿 이름",
    )) as HTMLTextAreaElement;
    await waitFor(() => expect(input).toBeEnabled());
    const raw = "  원문\n미저장 입력  ";
    fireEvent.change(input, { target: { value: raw } });
    input.focus();
    input.setSelectionRange(2, 5, "forward");
    const draft = controller.snapshot().form;
    await act(async () => {
      transport.nativeClose();
    });
    const dialog = await screen.findByRole("alertdialog");
    const attempt = transport.attempt;
    const keep = within(dialog).getByRole("button", { name: "계속 편집" });
    expect(keep).toHaveFocus();
    expect(dialog.closest("[inert]")).toBeNull();
    expect(input.closest("[inert]")).not.toBeNull();
    pressTab(dialog, true);
    expect(
      within(dialog).getByRole("button", { name: "닫기 상태 다시 확인" }),
    ).toHaveFocus();
    pressTab(dialog);
    expect(keep).toHaveFocus();
    const backdrop = document.querySelector(
      `.${dialogSurfaceClassNames.backdrop}`,
    )!;
    const down = createEvent.pointerDown(backdrop, {
      button: 0,
      bubbles: true,
      cancelable: true,
    });
    fireEvent(backdrop, down);
    // jsdom은 pointer 기본 focus 이동을 실행하지 않는다. 취소되지 않은 경로에서만
    // 비-focusable 배경으로 이동할 때의 blur를 모델링하고 Escape는 실제 activeElement에 보낸다.
    if (!down.defaultPrevented) (document.activeElement as HTMLElement).blur();
    fireEvent.pointerUp(backdrop);
    fireEvent.click(backdrop);
    expect(controller.snapshot().prompt?.attempt).toBe(attempt);
    expect(
      transport.commands.filter((c) => c.action === "ui_close_decision"),
    ).toHaveLength(0);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    await waitFor(() => expect(input).toHaveFocus());
    expect(input).toHaveValue(raw);
    expect([
      input.selectionStart,
      input.selectionEnd,
      input.selectionDirection,
    ]).toEqual([2, 5, "forward"]);
    expect(controller.snapshot().form).toBe(draft);
    expect(
      transport.commands.filter((c) => c.action === "ui_close_decision"),
    ).toEqual([{ action: "ui_close_decision", attempt, proceed: false }]);
    expect(transport.writes).toHaveLength(0);
    expect(transport.closing).toBe(false);
  });

  it("확인 결정 중 배경 pointer와 내부 Escape는 진행 중인 결정을 중복하지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: "결정 대기 원문" } });
    await act(async () => {
      transport.nativeClose();
    });
    const dialog = await screen.findByRole("alertdialog");
    const attempt = transport.attempt;
    const decision = transport.deferClose("ui_close_decision");
    const discard = within(dialog).getByRole("button", { name: "변경 버리기" });
    fireEvent.click(discard);
    await decision.received;
    expect(controller.snapshot().deciding).toBe(true);
    expect(discard).toBeDisabled();
    expect(
      within(dialog).getByRole("button", { name: "계속 편집" }),
    ).toBeDisabled();
    const backdrop = document.querySelector(
      `.${dialogSurfaceClassNames.backdrop}`,
    )!;
    fireEvent.pointerDown(backdrop);
    fireEvent.click(backdrop);
    expect(dialog).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    fireEvent.click(discard);
    expect(controller.snapshot().prompt?.attempt).toBe(attempt);
    expect(
      transport.commands.filter((c) => c.action === "ui_close_decision"),
    ).toEqual([{ action: "ui_close_decision", attempt, proceed: true }]);
    expect(input).toHaveValue("결정 대기 원문");
    expect(transport.writes).toHaveLength(0);
    await act(async () => {
      decision.release();
    });
    await waitFor(() => expect(controller.snapshot().closing).toBe(true));
  });

  it("A의 늦은 취소 응답과 조회는 열린 B의 선택 focus를 빼앗지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: "B가 보존할 원문" } });
    await act(async () => {
      transport.nativeClose();
    });
    await screen.findByRole("alertdialog");
    const a = transport.attempt;
    const oldStatus = transport.deferClose("ui_close_status");
    fireEvent.click(
      screen.getByRole("button", { name: "닫기 상태 다시 확인" }),
    );
    await oldStatus.received;
    const oldDecision = transport.deferClose("ui_close_decision");
    fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
    await oldDecision.received;
    await act(async () => {
      transport.nativeClose(false);
    });
    const b = transport.attempt;
    expect(b).not.toBe(a);
    fireEvent.click(
      screen.getByRole("button", { name: "닫기 상태 다시 확인" }),
    );
    await waitFor(() => expect(controller.snapshot().prompt?.attempt).toBe(b));
    expect(screen.getByRole("button", { name: "계속 편집" })).toHaveFocus();
    const chosen = screen.getByRole("button", { name: "변경 버리기" });
    chosen.focus();
    await act(async () => {
      oldDecision.release();
    });
    await act(async () => {
      oldStatus.release();
    });
    expect(controller.snapshot().prompt?.attempt).toBe(b);
    expect(chosen).toHaveFocus();
    expect(input).toHaveValue("B가 보존할 원문");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    expect(
      transport.commands.filter((c) => c.action === "ui_close_decision"),
    ).toEqual([
      { action: "ui_close_decision", attempt: a, proceed: false },
      { action: "ui_close_decision", attempt: b, proceed: false },
    ]);
    expect(transport.writes).toHaveLength(0);
  });

  it("FIX2 프로젝트 경로 조합 종료의 기본 submit은 막고 포인터 열기는 허용한다", async () => {
    const transport = new TestTransport();
    const controller = new TemplateController(new GuardedClient(transport));
    render(<App controller={controller} />);
    await waitFor(() => expect(controller.snapshot().ready).toBe(true));
    const input = screen.getByLabelText("기존 프로젝트 폴더 경로");
    const button = screen.getByRole("button", { name: "프로젝트 열기" });
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "한글 프로젝트ㄱ" } });
    fireEvent.compositionEnd(input);
    fireEvent.click(button, { detail: 0 });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(controller.snapshot().project).toBeNull();
    expect(input).toHaveValue("한글 프로젝트ㄱ");
    fireEvent.click(button, { detail: 1 });
    await screen.findByText("프로젝트를 열었습니다.");
    expect(
      transport.commands.filter(
        (c) => c.action === "submit" && c.input.kind === "open",
      ),
    ).toHaveLength(1);
  });
  it("FIX2 Template form 취소 뒤 재열기는 이전 조합 상태를 새 form에 넘기지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "이전 조합ㄱ" } });
    fireEvent.compositionEnd(input);
    fireEvent.click(screen.getByRole("button", { name: "저장" }), {
      detail: 0,
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(transport.writes).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    fireEvent.click(await screen.findByRole("button", { name: "변경 버리기" }));
    await waitFor(() => expect(controller.snapshot().form).toBeNull());
    // 해제 publish와 React commit 사이에는 버튼이 아직 비활성일 수 있다. 실제 조작 조건을 기다린다.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "새 템플릿" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const reopened = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(reopened).toBeEnabled());
    expect(reopened).not.toBe(input);
    fireEvent.change(reopened, { target: { value: "새 초안" } });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await screen.findByRole("heading", { name: "새 초안" });
    expect(transport.writes).toHaveLength(1);
  });
  it("F1 프로젝트 경로의 IME Enter는 열기를 막고 이후 일반 Enter는 한 번 연다", async () => {
    const transport = new TestTransport();
    const controller = new TemplateController(new GuardedClient(transport));
    render(<App controller={controller} />);
    await waitFor(() => expect(controller.snapshot().ready).toBe(true));
    const input = screen.getByLabelText("기존 프로젝트 폴더 경로");
    const button = screen.getByRole("button", { name: "프로젝트 열기" });
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "한글 프로젝트ㄱ" } });
    fireEvent.compositionEnd(input);
    const event = createEvent.keyDown(input, {
      key: "Enter",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    });
    fireEvent(input, event);
    // 취소되지 않은 implicit submit만 jsdom의 실제 버튼 click/submit 경로에 연결한다.
    if (!event.defaultPrevented) fireEvent.click(button);
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect({
      cancelled: event.defaultPrevented,
      project: controller.snapshot().project,
    }).toEqual({ cancelled: true, project: null });
    expect(input).toHaveValue("한글 프로젝트ㄱ");
    fireEvent.keyUp(input, { key: "Enter", code: "Enter" });
    expect(
      fireEvent.keyDown(input, { key: "Enter", code: "Enter", keyCode: 13 }),
    ).toBe(true);
    fireEvent.click(button);
    await screen.findByText("프로젝트를 열었습니다.");
    expect(
      transport.commands.filter(
        (c) => c.action === "submit" && c.input.kind === "open",
      ),
    ).toHaveLength(1);
  });
  it("F1 템플릿 이름의 IME Enter는 저장 버튼을 활성화하지 않고 명시적 클릭은 저장한다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "조합 이름ㄱ" } });
    fireEvent.compositionEnd(input);
    const button = screen.getByRole("button", { name: "저장" });
    const event = createEvent.keyDown(button, {
      key: "Enter",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    });
    fireEvent(button, event);
    if (!event.defaultPrevented) fireEvent.click(button);
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect({
      cancelled: event.defaultPrevented,
      writes: transport.writes.length,
    }).toEqual({ cancelled: true, writes: 0 });
    expect(input).toHaveValue("조합 이름ㄱ");
    fireEvent.click(button, { detail: 1 });
    await screen.findByRole("heading", { name: "조합 이름ㄱ" });
    expect(transport.writes).toHaveLength(1);
  });
  it("프로젝트 화면의 제목과 종료 보호 준비 뒤 열기 상태를 보여준다", async () => {
    await setup();

    expect(
      screen.getByRole("heading", {
        level: 1,
        name: "Dreamrugi Worldbuild Tool",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("프로젝트를 열었습니다.")).toBeInTheDocument();
  });

  it("listener 준비 전에 편집을 막고 remount에서도 한 번만 연결한다", async () => {
    let ready!: () => void;
    const transport = new TestTransport();
    transport.gateListener = new Promise<void>((resolve) => {
      ready = resolve;
    });
    const controller = new TemplateController(new GuardedClient(transport));
    const first = render(<App controller={controller} />);
    expect(screen.getByLabelText("기존 프로젝트 폴더 경로")).toBeDisabled();
    // 이전 backend 등록이 보여도 현재 listener가 준비되기 전에는 조회로 편집을 열 수 없다.
    transport.enabled = true;
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    expect(screen.getByLabelText("기존 프로젝트 폴더 경로")).toBeDisabled();
    expect(
      transport.commands.filter((c) => c.action === "ui_ready"),
    ).toHaveLength(0);
    first.unmount();
    render(<App controller={controller} />);
    await act(async () => {
      ready();
    });
    await waitFor(() =>
      expect(screen.getByLabelText("기존 프로젝트 폴더 경로")).toBeEnabled(),
    );
    expect(transport.listenerCount).toBe(1);
    expect(
      transport.commands.filter((c) => c.action === "ui_ready"),
    ).toHaveLength(1);
  });

  it("종료 보호 등록 수락 뒤 응답을 잃어도 화면 재확인으로 프로젝트를 한 번 연다", async () => {
    const transport = new TestTransport();
    const ready = transport.deferClose("ui_ready");
    const controller = new TemplateController(new GuardedClient(transport));
    const first = render(<App controller={controller} />);
    await ready.received;
    await act(async () => {
      ready.lose();
    });
    expect(
      screen.getByRole("button", { name: "프로젝트 열기" }),
    ).toBeDisabled();
    expect(transport.enabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    await waitFor(() =>
      expect(screen.getByLabelText("기존 프로젝트 폴더 경로")).toBeEnabled(),
    );
    await waitFor(() =>
      expect(controller.snapshot().startupLoading).toBe(false),
    );
    fireEvent.change(screen.getByLabelText("기존 프로젝트 폴더 경로"), {
      target: { value: "test-project" },
    });
    fireEvent.click(screen.getByRole("button", { name: "프로젝트 열기" }));
    await screen.findByText("프로젝트를 열었습니다.");
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: "복구 후 원 입력" } });
    first.unmount();
    render(<App controller={controller} />);
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    await act(async () => {
      await controller.checkStatus();
    });
    expect(screen.getByLabelText("템플릿 이름")).toHaveValue("복구 후 원 입력");
    expect(transport.listenerCount).toBe(1);
    expect(
      transport.commands.filter((c) => c.action === "ui_ready"),
    ).toHaveLength(1);
    expect(
      transport.commands.filter(
        (c) => c.action === "submit" && c.input.kind === "open",
      ),
    ).toHaveLength(1);
    expect(transport.writes).toHaveLength(0);
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
  });

  it.each([false, true])(
    "취소 A 응답 유실 뒤 hint 없는 B를 재확인하고 새 선택 proceed=%s를 적용한다",
    async (proceed) => {
      const { transport, controller } = await setup();
      fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
      const input = await screen.findByLabelText("템플릿 이름");
      await waitFor(() => expect(input).toBeEnabled());
      const name = "  보존할\n미제출 이름  ";
      fireEvent.change(input, { target: { value: name } });
      fireEvent.click(screen.getByRole("button", { name: "앱 종료" }));
      const first = await screen.findByRole("alertdialog");
      const a = transport.attempt;
      const decision = transport.deferClose("ui_close_decision");
      fireEvent.click(within(first).getByRole("button", { name: "계속 편집" }));
      expect((await decision.received).reply.attempt).toBeNull();
      transport.nativeClose(false);
      const b = transport.attempt;
      expect(b).not.toBe(a);
      // 첫 readback도 잃어 확인창 안의 복구 버튼이 실제로 접근 가능한지 검증한다.
      const readback = transport.deferClose("ui_close_status");
      await act(async () => {
        decision.lose();
      });
      await readback.received;
      await act(async () => {
        readback.lose();
      });
      const retry = within(screen.getByRole("alertdialog")).getByRole(
        "button",
        { name: "닫기 상태 다시 확인" },
      );
      expect(retry.closest("[inert]")).toBeNull();
      fireEvent.click(retry);
      await waitFor(() =>
        expect(controller.snapshot().prompt?.attempt).toBe(b),
      );
      expect(screen.getByLabelText("템플릿 이름")).toHaveValue(name);
      expect(transport.writes).toHaveLength(0);
      expect(transport.closing).toBe(false);
      fireEvent.click(
        within(screen.getByRole("alertdialog")).getByRole("button", {
          name: proceed ? "변경 버리기" : "계속 편집",
        }),
      );
      await waitFor(() =>
        expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument(),
      );
      expect(
        transport.commands.filter((c) => c.action === "ui_close_decision"),
      ).toEqual([
        { action: "ui_close_decision", attempt: a, proceed: false },
        { action: "ui_close_decision", attempt: b, proceed },
      ]);
      expect(
        transport.commands.some(
          (c) => c.action === "submit" && c.input.kind === "abandon_retained",
        ),
      ).toBe(false);
      if (proceed) {
        expect(controller.snapshot().form).toBeNull();
        expect(controller.snapshot().closing).toBe(true);
        expect(controller.snapshot().app?.normal_exit_allowed).toBe(false);
        expect(transport.writes).toHaveLength(0);
      } else {
        expect(input).toBeEnabled();
        fireEvent.change(input, { target: { value: name + "저장" } });
        fireEvent.click(screen.getByRole("button", { name: "저장" }));
        await waitFor(() =>
          expect(controller.snapshot().selection?.content.name).toBe(
            name + "저장",
          ),
        );
        expect(transport.writes).toHaveLength(1);
        expect(transport.writes[0].input).toMatchObject({
          kind: "create_template",
          name: name + "저장",
        });
      }
      expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
    },
  );

  it.each([false, true])(
    "늦은 A 응답과 이전 조회가 B의 새 선택 proceed=%s와 입력을 덮지 않는다",
    async (proceed) => {
      const { transport, controller } = await setup();
      fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
      const input = await screen.findByLabelText("템플릿 이름");
      await waitFor(() => expect(input).toBeEnabled());
      fireEvent.change(input, { target: { value: "원 입력" } });
      fireEvent.click(screen.getByRole("button", { name: "앱 종료" }));
      await screen.findByRole("alertdialog");
      const oldStatus = transport.deferClose("ui_close_status");
      fireEvent.click(
        screen.getByRole("button", { name: "닫기 상태 다시 확인" }),
      );
      await oldStatus.received;
      const oldDecision = transport.deferClose("ui_close_decision");
      fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
      await oldDecision.received;
      transport.nativeClose(false);
      const b = transport.attempt;
      fireEvent.click(
        screen.getByRole("button", { name: "닫기 상태 다시 확인" }),
      );
      await waitFor(() =>
        expect(controller.snapshot().prompt?.attempt).toBe(b),
      );
      expect(screen.getByRole("button", { name: "계속 편집" })).toBeEnabled();
      if (proceed) {
        fireEvent.click(screen.getByRole("button", { name: "변경 버리기" }));
        await waitFor(() => expect(controller.snapshot().closing).toBe(true));
      } else {
        // 실제 숨김 전환까지 진행된 Dialog를 닫아야 해제 수명 회귀도 빠짐없이 검증한다.
        await waitFor(() =>
          expect(input.closest('[aria-hidden="true"]')).not.toBeNull(),
        );
        fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
        await waitFor(() => expect(input).toBeEnabled());
        await waitFor(() => expect(input).toHaveFocus());
        fireEvent.change(input, { target: { value: "B 취소 후 새 입력" } });
      }
      await act(async () => {
        oldDecision.release();
      });
      await act(async () => {
        oldStatus.release();
      });
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
      expect(controller.snapshot().closing).toBe(proceed);
      expect(transport.writes).toHaveLength(0);
      if (proceed) {
        expect(controller.snapshot().form).toBeNull();
        expect(controller.snapshot().app?.normal_exit_allowed).toBe(false);
      } else {
        expect(input).toHaveValue("B 취소 후 새 입력");
        // Dialog 제거와 배경 접근성 복구는 별개다. 사용자가 접근할 수 있을 때 저장한다.
        fireEvent.click(await screen.findByRole("button", { name: "저장" }));
        await waitFor(() =>
          expect(controller.snapshot().selection?.content.name).toBe(
            "B 취소 후 새 입력",
          ),
        );
        expect(transport.writes).toHaveLength(1);
      }
    },
  );

  it("열기 결과 뒤 상태 응답을 잃어도 project owner를 보존하고 같은 프로젝트를 재조회한다", async () => {
    const transport = new TestTransport();
    transport.loseProjectStatus = true;
    const { controller } = await setup(transport);
    await waitFor(() =>
      expect(controller.snapshot().projectId).toBe(transport.project),
    );
    expect(
      screen.getByRole("button", { name: "프로젝트 열기" }),
    ).toBeDisabled();
    transport.loseProjectStatus = false;
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    await waitFor(() =>
      expect(controller.snapshot().project?.project).toBe(transport.project),
    );
    expect(
      transport.commands.filter(
        (c) => c.action === "submit" && c.input.kind === "open",
      ),
    ).toHaveLength(1);
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
  });

  it("초기화 실패 owner를 정상 인수한 뒤 입력 경로를 보존해 같은 세션에서 다시 연다", async () => {
    const transport = new TestTransport();
    transport.initializationFailed = true;
    const { controller } = await setup(transport);
    expect(controller.snapshot().project?.error?.code).toBe(
      "initialization_failed",
    );
    fireEvent.click(screen.getByRole("button", { name: "다른 프로젝트 선택" }));
    const input = await screen.findByLabelText("기존 프로젝트 폴더 경로");
    await waitFor(() => expect(input).toHaveFocus());
    expect(input).toHaveValue("test-project");
    expect(controller.snapshot().projectId).toBeNull();
    expect(
      transport.commands.some(
        (command) =>
          command.action === "submit" && command.input.kind === "close",
      ),
    ).toBe(true);
    expect(
      transport.commands.some(
        (command) => command.action === "acknowledge_shutdown",
      ),
    ).toBe(true);
    expect(
      transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(true);

    transport.initializationFailed = false;
    transport.projectStopped = false;
    fireEvent.change(input, { target: { value: "valid-project" } });
    fireEvent.click(screen.getByRole("button", { name: "프로젝트 열기" }));
    await waitFor(() =>
      expect(controller.snapshot().project?.runtime).toBe("Ready"),
    );
    expect(screen.queryByRole("button", { name: "경로 수정" })).toBeNull();
  });

  it("초기화 실패 뒤 앱 종료 보고를 확인하면 owner를 퇴역시켜 종료를 진행한다", async () => {
    const transport = new TestTransport();
    transport.initializationFailed = true;
    const { controller } = await setup(transport);
    fireEvent.click(screen.getByRole("button", { name: "앱 종료" }));
    await waitFor(() => expect(controller.snapshot().closing).toBe(true));
    transport.projectStopped = true;
    transport.reportPending = true;
    transport.shutdownBlockers = ["Results"];
    await act(async () => {
      await controller.checkStatus();
    });
    await act(async () => {
      await controller.acknowledgeShutdown();
    });
    await waitFor(() => expect(controller.snapshot().projectId).toBeNull());
    expect(
      transport.commands.some(
        (command) =>
          command.action === "submit" &&
          command.input.kind === "retire_project",
      ),
    ).toBe(true);
  });

  it("생성·이름 변경·Unchanged를 원문과 고정 name intent로 저장하고 다시 읽는다", async () => {
    const { transport, controller } = await setup();
    await create();
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    const name = "  한국어\n 원문  ";
    fireEvent.change(input, { target: { value: name } });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe(name),
    );
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(transport.writes).toHaveLength(2);
    expect(transport.writes[1].input).toMatchObject({
      kind: "update_template",
      revision: "1",
      edit: { kind: "name", name },
    });
    expect(Object.keys(transport.writes[1].input)).not.toContain("fields");
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "저장" })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await screen.findByText(/변경 없음/);
    expect(controller.snapshot().selection?.content.revision).toBe("2");
  });

  it("수락 뒤 응답 유실·중복 클릭·화면 재구성이 같은 operation owner를 보존한다", async () => {
    const { transport, controller, view } = await setup();
    transport.hold = "create_template";
    transport.loseSubmit = true;
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "응답 유실" },
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(controller.snapshot().operations).toHaveLength(1),
    );
    const id = controller.snapshot().operations[0].id;
    view.unmount();
    render(<App controller={controller} />);
    expect(screen.getByLabelText("템플릿 이름")).toHaveValue("응답 유실");
    await act(async () => {
      transport.loseResult = true;
      transport.completeHeld();
    });
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe("응답 유실"),
    );
    expect(transport.writes).toHaveLength(1);
    expect(transport.writes[0].operation).toBe(id);
    expect(
      transport.commands.filter(
        (c) => c.action === "submit" && c.input.kind === "create_template",
      ),
    ).toHaveLength(1);
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
  });

  it("dirty 이동의 계속 편집과 버리기를 구분하고 IME 중 제출하지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    const input = await screen.findByLabelText("템플릿 이름");
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: "미제출" } });
    fireEvent.compositionStart(input);
    fireEvent.submit(input.closest("form")!);
    expect(transport.writes).toHaveLength(0);
    fireEvent.compositionEnd(input);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "계속 편집" })).toHaveFocus();
    pressTab(screen.getByRole("alertdialog"), true);
    expect(screen.getByRole("button", { name: "변경 버리기" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    expect(input).toHaveValue("미제출");
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    fireEvent.click(await screen.findByRole("button", { name: "변경 버리기" }));
    await waitFor(() => expect(controller.snapshot().form).toBeNull());
    expect(transport.writes).toHaveLength(0);
  });

  it("예약 대기 중 입력을 backend 인수로 오인하지 않고 닫기 취소 후 같은 저장을 마친다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "예약 전 입력" },
    });
    let release!: () => void;
    transport.gateReserve = new Promise<void>((resolve) => {
      release = resolve;
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await act(async () => {
      transport.nativeClose();
    });
    await screen.findByRole("alertdialog");
    expect(transport.closing).toBe(false);
    expect(controller.snapshot().form?.handedOff).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    await act(async () => {
      transport.gateReserve = null;
      release();
    });
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe(
        "예약 전 입력",
      ),
    );
    expect(transport.writes).toHaveLength(1);
  });

  it("확정 결과 ack 응답 유실을 같은 ID로 인수 확인하고 저장을 재실행하지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    transport.loseAck = true;
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(controller.snapshot().operations[0]?.phase).toBe("acknowledging"),
    );
    const operation = controller.snapshot().operations[0].id;
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe(""),
    );
    expect(transport.writes).toHaveLength(1);
    expect(
      transport.commands.filter(
        (c) =>
          c.action === "acknowledge_transport" && c.operation === operation,
      ),
    ).toHaveLength(2);
  });

  it("늦은 상세 조회 동안 이동을 직렬화하고 dirty close 취소 뒤에도 원 편집 기준을 유지한다", async () => {
    const { controller, transport } = await setup();
    await create();
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "늦은 조회 중 입력" },
    });
    const original = controller.snapshot().form?.source;
    transport.hold = "read_template";
    fireEvent.click(screen.getByRole("button", { name: "새로고침" }));
    await waitFor(() =>
      expect(
        controller
          .snapshot()
          .operations.some((o) => o.label === "템플릿 상세 조회"),
      ).toBe(true),
    );
    expect(screen.getByRole("button", { name: "새 템플릿" })).toBeDisabled();
    await act(async () => {
      transport.nativeClose();
    });
    fireEvent.click(await screen.findByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    await act(async () => {
      transport.completeHeld();
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(controller.snapshot().form?.source).toBe(original);
    expect(screen.getByLabelText("템플릿 이름")).toHaveValue(
      "늦은 조회 중 입력",
    );
    expect(transport.released).not.toContain(original?.view);
  });

  it("정상 프로젝트 종료 보고를 인수하고 실제 retire 응답 뒤 홈으로 바로 돌아간다", async () => {
    const { controller, transport } = await setup();
    await create();
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "프로젝트 닫기" }));
    await screen.findByLabelText("기존 프로젝트 폴더 경로");
    expect(
      screen.queryByRole("button", { name: "프로젝트 선택으로 돌아가기" }),
    ).toBeNull();
    expect(controller.snapshot().project).toBeNull();
    expect(transport.views.size).toBe(0);
    expect(
      transport.commands.some(
        (c) => c.action === "submit" && c.input.kind === "retire_project",
      ),
    ).toBe(true);
  });

  it("native close 시도를 동결하고 취소 후 저장하며 늦은 확인으로 새 입력을 버리지 않는다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "취소 후 저장" },
    });
    await act(async () => {
      transport.nativeClose();
    });
    await screen.findByRole("alertdialog");
    expect(screen.getByLabelText("템플릿 이름")).toBeDisabled();
    const firstAttempt = controller.snapshot().prompt?.attempt;
    await act(async () => {
      transport.nativeClose();
    });
    expect(controller.snapshot().prompt?.attempt).toBe(firstAttempt);
    transport.loseDecision = true;
    fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    expect(transport.closing).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe(
        "취소 후 저장",
      ),
    );
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "버릴 변경" },
    });
    await act(async () => {
      transport.nativeClose();
    });
    fireEvent.click(await screen.findByRole("button", { name: "변경 버리기" }));
    await waitFor(() => expect(transport.closing).toBe(true));
    expect(transport.writes).toHaveLength(1);
  });

  it("stale 입력과 옛 revision은 최신 조회 뒤에도 유지하고 실제 보관 포기 후 새 편집한다", async () => {
    const { controller, transport } = await setup();
    await create();
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "보존할 입력" },
    });
    const stored = [...transport.templates.values()][0];
    transport.templates.set(stored.id, {
      ...stored,
      name: "외부 변경",
      revision: "2",
    });
    fireEvent.click(screen.getByRole("button", { name: "새로고침" }));
    await waitFor(() =>
      expect(controller.snapshot().selection?.content.name).toBe("외부 변경"),
    );
    expect(controller.snapshot().form?.source?.content.revision).toBe("1");
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    fireEvent.click(
      await screen.findByRole("button", { name: text("followUp.openActions") }),
    );
    await screen.findByRole("button", { name: "보관 입력 포기" });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(screen.getByLabelText("템플릿 이름")).toHaveValue("보존할 입력");
    expect(transport.retained.size).toBe(1);
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
    fireEvent.click(screen.getByRole("button", { name: "보관 입력 포기" }));
    await waitFor(() => expect(controller.snapshot().form).toBeNull());
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(transport.retained.size).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "이름 변경" }));
    await waitFor(() =>
      expect(controller.snapshot().form?.source?.content.revision).toBe("2"),
    );
    expect(transport.released.length).toBeGreaterThan(0);
  });

  it("확정 저장 뒤 cleanup 실패는 재저장 없이 실제 세션 제어로 정리한다", async () => {
    const { controller, transport } = await setup();
    transport.cleanup = true;
    transport.failRelease = true;
    await create("정리 필요");
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(screen.getByText(/다시 저장하지 마세요/)).toBeInTheDocument();
    expect(transport.writes).toHaveLength(1);
    transport.failRelease = false;
    fireEvent.click(
      screen.getByRole("button", { name: text("followUp.openActions") }),
    );
    fireEvent.click(screen.getByRole("button", { name: "잠금 해제 재시도" }));
    await waitFor(() => expect(controller.snapshot().sessions).toHaveLength(0));
    expect(transport.writes).toHaveLength(1);
  });

  it("불확정 owner는 포기를 막고 보관 조회 실패도 원 입력과 handle을 남긴다", async () => {
    const { controller, transport } = await setup();
    transport.uncertain = true;
    transport.loseRetainedRead = true;
    fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
    await waitFor(() =>
      expect(screen.getByLabelText("템플릿 이름")).toBeEnabled(),
    );
    fireEvent.change(screen.getByLabelText("템플릿 이름"), {
      target: { value: "불확정" },
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(controller.snapshot().retainedRefs).toHaveLength(1),
    );
    expect(screen.getByLabelText("템플릿 이름")).toHaveValue("불확정");
    transport.loseRetainedRead = false;
    fireEvent.click(screen.getByRole("button", { name: "상태 다시 확인" }));
    fireEvent.click(
      await screen.findByRole("button", { name: text("followUp.openActions") }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "보관 입력 포기" }),
      ).toBeDisabled(),
    );
    expect(transport.retained.size).toBe(1);
  });

  it("목록 실패를 빈 목록으로 표시하지 않고 Blocked 프로젝트의 편집을 막는다", async () => {
    const transport = new TestTransport();
    transport.status = "Blocked";
    transport.runtime = "Blocked";
    const { controller } = await setup(transport);
    expect(screen.getByRole("button", { name: "새 템플릿" })).toBeDisabled();
    expect(controller.snapshot().listState).toBe("idle");
    transport.status = "Ready";
    transport.runtime = "Ready";
    await act(async () => {
      await controller.checkStatus();
    });
    transport.failList = true;
    fireEvent.click(screen.getByRole("button", { name: "새로고침" }));
    await screen.findByText(/목록을 읽지 못했습니다/);
    expect(
      screen.queryByText(/아직 Template이 없습니다/),
    ).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
  });
});
