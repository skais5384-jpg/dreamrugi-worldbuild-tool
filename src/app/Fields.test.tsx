import { render } from "../test/render";
import {
  act,
  createEvent,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "./App";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";
import { text } from "../strings";

async function setup() {
  const transport = new TestTransport();
  transport.templates.set("template", {
    id: "template",
    name: "테스트 정의",
    lifecycle: "Active",
    revision: "9007199254740993",
    presentation: null,
    fieldOrder: ["number", "duration"],
    fields: [
      {
        id: "number",
        label: "정확한 숫자",
        kind: "Number",
        lifecycle: "Active",
        required: false,
        presentation: "existing",
        default: { kind: "number", value: "12345678901234567890.123456789" },
        initialDefault: { kind: "number", value: "-0.1" },
        introducedRevision: "1",
        options: [],
        optionOrder: [],
      },
      {
        id: "duration",
        label: "기간 정의",
        kind: "Duration",
        lifecycle: "Active",
        required: false,
        presentation: null,
        default: { kind: "unset" },
        initialDefault: { kind: "unset" },
        introducedRevision: "2",
        options: [],
        optionOrder: [],
      },
    ],
  });
  const controller = new TemplateController(new GuardedClient(transport));
  const view = render(<App controller={controller} />);
  await waitFor(() => expect(controller.snapshot().ready).toBe(true));
  await act(async () => {
    controller.setRoot("fixture");
    await controller.open();
    await controller.navigate({ kind: "select", id: "template" });
    await controller.navigate({ kind: "field", id: "number" });
  });
  return { controller, transport, view };
}
const form = (name: string) => within(screen.getByRole("form", { name }));
// jsdom은 Enter의 implicit submit을 실행하지 않는다. 취소되지 않은 키만 기본 버튼 click으로 연결한다.
function enterWithDefaultClick(
  target: HTMLElement,
  button: HTMLElement,
  init: KeyboardEventInit = {},
) {
  const event = createEvent.keyDown(target, {
    key: "Enter",
    code: "Enter",
    keyCode: 13,
    ...init,
  });
  fireEvent(target, event);
  if (!event.defaultPrevented) fireEvent.click(button);
  return event;
}
async function idle(controller: TemplateController) {
  await waitFor(() => expect(controller.snapshot().busy).toBe(false));
}
function setDefault(value: string) {
  const f = form(text("field.default"));
  fireEvent.change(f.getByLabelText(text("field.default")), {
    target: { value: "set" },
  });
  fireEvent.change(f.getByLabelText(text("field.value")), {
    target: { value },
  });
}
describe("Field 정의 편집", () => {
  it.skip("제거된 표시 토큰 입력의 IME 회귀", async () => {
    const { controller, transport } = await setup();
    const input = form(text("field.presentation")).getByLabelText(
      text("field.presentation"),
    );
    const button = form(text("field.presentation")).getByRole("button", {
      name: "표시 토큰 적용",
    });
    const owner = input.closest("form")!;
    expect(input).toBeInstanceOf(HTMLInputElement);
    expect(input.id).toBe("field-presentation-token");
    expect((button as HTMLButtonElement).form).toBe(owner);
    input.focus();
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "IME 확인 대기ㄱ" } });
    const draft = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "presentation")!;
    // Enter로 식별되지 않는 IME 키는 문자 입력을 위해 허용한다. 실제 Windows 키 값의 기록은 아니다.
    expect(
      fireEvent.keyDown(input, {
        key: "Process",
        code: "",
        keyCode: 229,
        isComposing: true,
      }),
    ).toBe(true);
    fireEvent.compositionEnd(input);
    const submits: SubmitEvent[] = [];
    owner.addEventListener("submit", (event) => submits.push(event), {
      once: true,
    });
    // jsdom의 implicit submit은 수동 연결한다. 취소된 keydown 뒤 submit을 강제로 보내는 검사가 아니다.
    fireEvent.click(button, { detail: 0 });
    await idle(controller);
    expect(submits).toHaveLength(1);
    expect(submits[0].submitter).toBe(button);
    expect(submits[0].defaultPrevented).toBe(true);
    expect(transport.writes).toHaveLength(0);
    const kept = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "presentation")!;
    expect(kept.source).toBe(draft.source);
    expect(kept.edit).toEqual(draft.edit);
    expect(kept.submitted).toBe(false);
    expect(input).toHaveValue("IME 확인 대기ㄱ");
    expect(input).toHaveFocus();
    fireEvent.keyUp(input, { key: "Enter", code: "Enter" });
    // 명시적 포인터 click은 기본 버튼의 키보드 click(detail 0)과 구분한다.
    fireEvent.click(button, { detail: 1 });
    await idle(controller);
    expect(transport.writes).toHaveLength(1);
    expect(transport.writes[0].input).toMatchObject({
      kind: "update_template",
      project: draft.source.project,
      view: draft.source.view,
      revision: draft.source.content.revision,
      edit: {
        kind: "field_presentation",
        field: "number",
        token: "IME 확인 대기ㄱ",
      },
    });
    expect(
      controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "label")!.source,
    ).toBe(draft.source);
  });
  it.skip("제거된 표시 토큰 입력의 Enter 회귀", async () => {
    const { controller, transport } = await setup();
    const input = form(text("field.presentation")).getByLabelText(
      text("field.presentation"),
    );
    const button = form(text("field.presentation")).getByRole("button", {
      name: "표시 토큰 적용",
    });
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "조합 확정ㄱ" } });
    fireEvent.compositionEnd(input);
    const ending = enterWithDefaultClick(input, button, {
      isComposing: false,
      keyCode: 13,
    });
    await idle(controller);
    expect({
      cancelled: ending.defaultPrevented,
      writes: transport.writes.length,
    }).toEqual({ cancelled: true, writes: 0 });
    fireEvent.keyUp(input, { key: "Enter", code: "Enter" });
    expect(enterWithDefaultClick(input, button).defaultPrevented).toBe(false);
    await idle(controller);
    expect(transport.writes).toHaveLength(1);
    expect(transport.writes[0].input).toMatchObject({
      edit: { token: "조합 확정ㄱ" },
    });
  });
  it.skip.each([
    {
      boundary: "compositionStart ref",
      composing: true,
      key: "Enter",
      code: "Enter",
      keyCode: 13,
      isComposing: false,
    },
    {
      boundary: "native isComposing",
      composing: false,
      key: "Enter",
      code: "Enter",
      keyCode: 13,
      isComposing: true,
    },
    {
      boundary: "compositionEnd 뒤 Enter/229",
      composing: false,
      key: "Enter",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    },
    {
      boundary: "compositionEnd 뒤 Process/Enter/229",
      composing: false,
      key: "Process",
      code: "Enter",
      keyCode: 229,
      isComposing: false,
    },
    {
      boundary: "compositionEnd 뒤 Process/NumpadEnter/229",
      composing: false,
      key: "Process",
      code: "NumpadEnter",
      keyCode: 229,
      isComposing: false,
    },
  ])(
    "F1 단일행 token $boundary는 기본 제출을 막고 명시적 적용은 한 번 저장한다",
    async ({ composing, ...key }) => {
      const { controller, transport } = await setup();
      const input = form(text("field.presentation")).getByLabelText(
        text("field.presentation"),
      );
      expect(input).toBeInstanceOf(HTMLInputElement);
      expect(input.id).toBe("field-presentation-token");
      const button = form(text("field.presentation")).getByRole("button", {
        name: "표시 토큰 적용",
      });
      input.focus();
      fireEvent.compositionStart(input);
      fireEvent.change(input, { target: { value: "IME 확인 대기ㄱ" } });
      const draft = controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "presentation")!;
      if (!composing) fireEvent.compositionEnd(input);
      else {
        // keydown과 독립적인 submit 방어 검사다. 취소된 keydown 뒤 submit을 강제로 재생하지 않는다.
        fireEvent.submit(input.closest("form")!);
        expect(transport.writes).toHaveLength(0);
      }
      const event = enterWithDefaultClick(input, button, key);
      await idle(controller);
      expect({
        cancelled: event.defaultPrevented,
        writes: transport.writes.length,
        revision: controller.snapshot().selection!.content.revision,
      }).toEqual({
        cancelled: true,
        writes: 0,
        revision: draft.source.content.revision,
      });
      const kept = controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "presentation")!;
      expect(kept.source).toBe(draft.source);
      expect(kept.edit).toEqual(draft.edit);
      expect(kept.submitted).toBe(false);
      expect(input).toHaveValue("IME 확인 대기ㄱ");
      expect(input).toHaveFocus();
      if (composing) fireEvent.compositionEnd(input);
      fireEvent.keyUp(input, { key: key.key, code: key.code });
      fireEvent.click(button, { detail: 1 });
      await idle(controller);
      expect(transport.writes).toHaveLength(1);
      expect(transport.writes[0].input).toMatchObject({
        kind: "update_template",
        project: draft.source.project,
        view: draft.source.view,
        revision: draft.source.content.revision,
        edit: {
          kind: "field_presentation",
          field: "number",
          token: "IME 확인 대기ㄱ",
        },
      });
      expect(
        controller.snapshot().selection!.content.fields[0].presentation,
      ).toBe("IME 확인 대기ㄱ");
      expect(
        controller
          .snapshot()
          .fieldEditor!.drafts.find((d) => d.property === "label")!.source,
      ).toBe(draft.source);
    },
  );
  it.skip.each(["input Enter", "button Enter", "button Space"])(
    "F1 조합 후 %s는 차단 잔류 없이 원 경로로 한 번 저장한다",
    async (activation) => {
      const { controller, transport } = await setup();
      const input = form(text("field.presentation")).getByLabelText(
        text("field.presentation"),
      );
      const button = form(text("field.presentation")).getByRole("button", {
        name: "표시 토큰 적용",
      });
      fireEvent.compositionStart(input);
      expect(
        fireEvent.keyDown(input, {
          key: "Process",
          code: "KeyR",
          keyCode: 229,
          isComposing: true,
        }),
      ).toBe(true);
      fireEvent.change(input, { target: { value: "ㄱ" } });
      fireEvent.compositionEnd(input);
      expect(
        enterWithDefaultClick(input, button, { keyCode: 229 }).defaultPrevented,
      ).toBe(true);
      fireEvent.keyUp(input, { key: "Enter" });
      fireEvent.change(input, { target: { value: "ㄱ 정상 후속 입력" } });
      expect(fireEvent.keyDown(input, { key: "Tab" })).toBe(true);
      expect(fireEvent.keyDown(button, { key: "Tab", shiftKey: true })).toBe(
        true,
      );
      const target = activation === "input Enter" ? input : button;
      target.focus();
      if (activation === "button Space") {
        const down = fireEvent.keyDown(button, { key: " ", code: "Space" });
        const up = fireEvent.keyUp(button, { key: " ", code: "Space" });
        expect(down && up).toBe(true);
        if (down && up) fireEvent.click(button);
      } else
        expect(enterWithDefaultClick(target, button).defaultPrevented).toBe(
          false,
        );
      await idle(controller);
      expect(transport.writes).toHaveLength(1);
      expect(transport.writes[0].input).toMatchObject({
        edit: {
          kind: "field_presentation",
          field: "number",
          token: "ㄱ 정상 후속 입력",
        },
      });
    },
  );
  it("F1 textarea의 일반 개행과 IME 확정 뒤 닫기 취소는 원 초안과 focus를 보존한다", async () => {
    const { transport } = await setup();
    const input = form(text("field.label")).getByLabelText(text("field.label"));
    input.focus();
    expect(fireEvent.keyDown(input, { key: "Enter", code: "Enter" })).toBe(
      true,
    );
    // jsdom은 개행 삽입도 하지 않으므로 기본 키 허용과 실제 onChange 보존을 따로 확인한다.
    fireEvent.change(input, { target: { value: "첫 줄\n한글" } });
    fireEvent.compositionStart(input);
    expect(fireEvent.keyDown(input, { key: "Enter", isComposing: true })).toBe(
      false,
    );
    fireEvent.compositionEnd(input);
    expect(
      fireEvent.keyDown(input, {
        key: "Enter",
        code: "Enter",
        keyCode: 229,
        isComposing: false,
      }),
    ).toBe(false);
    act(() => transport.nativeClose());
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(input).toHaveValue("첫 줄\n한글");
    expect(input).toHaveFocus();
    expect(transport.writes).toHaveLength(0);
  });
  it("Field 이름 초안의 native close 취소는 같은 입력과 focus를 복원하며 적용 전 쓰지 않는다", async () => {
    const { controller, transport } = await setup();
    const label = form(text("field.label")).getByLabelText(text("field.label"));
    label.focus();
    fireEvent.change(label, { target: { value: "닫기 취소 후 유지할 이름" } });
    const draft = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "label")!;
    // jsdom에는 비활성 영역의 focus 해제 동작이 없어 실제 WebView의 blur를 재현한다.
    const setAttribute = HTMLElement.prototype.setAttribute;
    const inert = vi
      .spyOn(HTMLElement.prototype, "setAttribute")
      .mockImplementation(function (this: HTMLElement, name, value) {
        if (
          (name === "inert" || name === "disabled") &&
          this.contains(document.activeElement)
        )
          (document.activeElement as HTMLElement).blur();
        setAttribute.call(this, name, value);
      });
    act(() => transport.nativeClose());
    const dialog = await screen.findByRole("alertdialog");
    inert.mockRestore();
    expect(
      within(dialog).getByRole("button", { name: "계속 편집" }),
    ).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(form(text("field.label")).getByLabelText(text("field.label"))).toBe(
      label,
    );
    expect(label).toHaveValue("닫기 취소 후 유지할 이름");
    expect(label).toHaveFocus();
    expect(controller.snapshot().fieldEditor!.field).toBe("number");
    const kept = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "label")!;
    expect(kept.source).toBe(draft.source);
    expect(kept.edit).toEqual(draft.edit);
    expect(transport.writes).toHaveLength(0);
    fireEvent.click(
      form(text("field.label")).getByRole("button", {
        name: "필드 이름 적용",
      }),
    );
    await idle(controller);
    expect(transport.writes).toHaveLength(1);
    expect(controller.snapshot().selection!.content.fields[0].label).toBe(
      "닫기 취소 후 유지할 이름",
    );
  });
  it("미수락 Full은 예약 owner를 남기고 명시적 재시도도 같은 ID를 사용한다", async () => {
    const { controller, transport } = await setup();
    transport.refuseWriteOnce = true;
    fireEvent.change(
      form(text("field.label")).getByLabelText(text("field.label")),
      { target: { value: "미수락 입력" } },
    );
    fireEvent.click(
      form(text("field.label")).getByRole("button", {
        name: "필드 이름 적용",
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("followUp.openActions") }),
    );
    await screen.findByText("아직 수락되지 않은 예약");
    expect(transport.writes).toHaveLength(0);
    expect(
      controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "label")!.handedOff,
    ).toBe(false);
    const before = transport.commands.filter(
      (c) => c.action === "submit" && c.input.kind === "update_template",
    );
    fireEvent.click(
      screen.getByRole("button", { name: text("followUp.message07") }),
    );
    await idle(controller);
    const after = transport.commands.filter(
      (c) => c.action === "submit" && c.input.kind === "update_template",
    );
    expect(after).toHaveLength(2);
    expect(after[1]).toEqual(before[0]);
    expect(transport.writes).toHaveLength(1);
  });
  it("생성 저장 후 재조회 실패를 재생성 실패로 바꾸지 않고 확정 내용만 다시 읽는다", async () => {
    const { controller, transport } = await setup();
    await act(async () => controller.navigate({ kind: "new_field" }));
    fireEvent.change(
      form(text("field.add")).getByLabelText(text("field.label")),
      { target: { value: "이미 저장한 Field" } },
    );
    transport.failRead = true;
    fireEvent.click(
      form(text("field.add")).getByRole("button", { name: "필드 추가 적용" }),
    );
    await idle(controller);
    expect(controller.snapshot().fieldEditor!.drafts[0].committed).toBe(true);
    fireEvent.click(
      screen.getByRole("button", { name: text("update.details") }),
    );
    expect(
      within(await screen.findByRole("dialog"))
        .getAllByText(text("field.followup"))
        .every((item) => item.closest("td")),
    ).toBe(true);
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }),
    );
    await screen.findByRole("form", { name: text("field.add") });
    expect(
      form(text("field.add")).getByRole("button", { name: "필드 추가 적용" }),
    ).toBeDisabled();
    transport.failRead = false;
    fireEvent.click(
      screen.getByRole("button", { name: text("field.readSaved") }),
    );
    await idle(controller);
    expect(
      form(text("field.label")).getByLabelText(text("field.label")),
    ).toHaveValue("이미 저장한 Field");
    expect(transport.writes).toHaveLength(1);
  });
  it.each(["9223372036854775808", "-9223372036854775809"])(
    "기간 범위 밖 %s의 원 입력을 보존한다",
    async (value) => {
      const { controller, transport } = await setup();
      await act(async () =>
        controller.navigate({ kind: "field", id: "duration" }),
      );
      setDefault(value);
      fireEvent.click(
        form(text("field.default")).getByRole("button", {
          name: "현재 기본값 적용",
        }),
      );
      await idle(controller);
      expect(
        form(text("field.default")).getByLabelText(text("field.value")),
      ).toHaveValue(value);
      expect(transport.writes).toHaveLength(0);
      expect(form(text("field.default")).getByRole("status")).toHaveTextContent(
        text("field.validation.duration"),
      );
    },
  );
  it("보관된 정의도 읽기 전용 상세와 stable ID를 보여준다", async () => {
    const { controller, transport } = await setup();
    const template = transport.templates.get("template")!;
    template.fields.push({
      ...template.fields[0],
      id: "archived",
      label: "보관 Field",
      lifecycle: "Archived",
    });
    await act(async () => controller.refresh());
    fireEvent.click(screen.getByRole("button", { name: "보관 Field" }));
    await idle(controller);
    expect(controller.snapshot().fieldEditor!.field).toBe("archived");
    expect(controller.snapshot().fieldEditor!.drafts).toEqual([]);
    expect(screen.getByText("archived")).toBeInTheDocument();
    expect(
      screen.getByText("12345678901234567890.123456789"),
    ).toBeInTheDocument();
  });
  it("속성 저장 후 다른 원 입력과 문자열 revision을 보존하고 명시적으로 한 속성만 재확인한다", async () => {
    const { controller, transport } = await setup();
    setDefault("-123456789012345678901.23456789");
    const old = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "default")!;
    fireEvent.change(
      form(text("field.label")).getByLabelText(text("field.label")),
      { target: { value: "바꾼 이름" } },
    );
    fireEvent.click(
      form(text("field.label")).getByRole("button", {
        name: "필드 이름 적용",
      }),
    );
    await idle(controller);
    const remain = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "default")!;
    expect(remain.source).toBe(old.source);
    expect(remain.edit).toEqual(old.edit);
    expect(transport.writes[0].input).toMatchObject({
      revision: "9007199254740993",
      edit: { kind: "field_label", field: "number", label: "바꾼 이름" },
    });
    expect(controller.snapshot().selection!.content.revision).toBe(
      "9007199254740994",
    );
    fireEvent.click(
      form(text("field.default")).getByRole("button", {
        name: text("field.reconfirm"),
      }),
    );
    await idle(controller);
    const confirmed = controller
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.property === "default")!;
    expect(confirmed.source.content.revision).toBe("9007199254740994");
    expect(confirmed.edit).toEqual(old.edit);
    expect(
      controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "required")!.source,
    ).toBe(old.source);
  });
  it.each(["", "-", "1.", "01", "-0", "1.20", "1e3"])(
    "미완성/비정규 숫자 '%s'를 0으로 바꾸지 않고 저장을 막는다",
    async (value) => {
      const { controller, transport } = await setup();
      setDefault(value);
      fireEvent.click(
        form(text("field.default")).getByRole("button", {
          name: "현재 기본값 적용",
        }),
      );
      await idle(controller);
      expect(
        form(text("field.default")).getByLabelText(text("field.value")),
      ).toHaveValue(value);
      expect(form(text("field.default")).getByRole("status")).toHaveTextContent(
        text("field.validation.number"),
      );
      expect(transport.writes).toHaveLength(0);
    },
  );
  it("IME Enter와 native close 취소 뒤 원 입력이 편집 가능하고 remount 중 저장 응답 유실은 한 요청이다", async () => {
    const { controller, transport, view } = await setup();
    const label = form(text("field.label")).getByLabelText(text("field.label"));
    fireEvent.compositionStart(label);
    fireEvent.change(label, { target: { value: "한글 조합" } });
    expect(fireEvent.keyDown(label, { key: "Enter", isComposing: true })).toBe(
      false,
    );
    expect(transport.writes).toHaveLength(0);
    fireEvent.compositionEnd(label);
    act(() => transport.nativeClose());
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(label).toBeEnabled());
    expect(label).toHaveValue("한글 조합");
    transport.hold = "update_template";
    transport.loseSubmit = true;
    fireEvent.click(
      form(text("field.label")).getByRole("button", {
        name: "필드 이름 적용",
      }),
      { detail: 1 },
    );
    await waitFor(() =>
      expect(
        transport.commands.some(
          (c) => c.action === "submit" && c.input.kind === "update_template",
        ),
      ).toBe(true),
    );
    view.unmount();
    render(<App controller={controller} />);
    await act(async () => {
      await controller.saveField("label");
      await controller.navigate({ kind: "field", id: "duration" });
      transport.completeHeld();
      await controller.checkStatus();
    });
    await idle(controller);
    expect(transport.writes).toHaveLength(1);
    expect(controller.snapshot().fieldEditor!.field).toBe("number");
    expect(
      form(text("field.label")).getByLabelText(text("field.label")),
    ).toHaveValue("한글 조합");
    expect(transport.listenerCount).toBe(1);
  });
  it("stale 거부와 retained 포기 후에도 다른 미제출 입력을 유지한다", async () => {
    const { controller, transport } = await setup();
    setDefault("-0.1234567890123456789");
    fireEvent.change(
      form(text("field.label")).getByLabelText(text("field.label")),
      { target: { value: "원래 실패 이름" } },
    );
    transport.templates.get("template")!.revision = "9007199254740994";
    fireEvent.click(
      form(text("field.label")).getByRole("button", {
        name: "필드 이름 적용",
      }),
    );
    await idle(controller);
    expect(
      form(text("field.label")).getByLabelText(text("field.label")),
    ).toHaveValue("원래 실패 이름");
    expect(controller.snapshot().retained).toHaveLength(1);
    fireEvent.click(
      screen.getByRole("button", { name: text("followUp.openActions") }),
    );
    fireEvent.click(screen.getByRole("button", { name: "보관 입력 포기" }));
    await idle(controller);
    expect(
      form(text("field.default")).getByLabelText(text("field.value")),
    ).toHaveValue("-0.1234567890123456789");
    expect(
      controller
        .snapshot()
        .fieldEditor!.drafts.find((d) => d.property === "default")!.source
        .content.revision,
    ).toBe("9007199254740993");
    expect(
      form(text("field.label")).getByLabelText(text("field.label")),
    ).toBeEnabled();
    expect(document.body).not.toHaveTextContent("M271_RAW_ERROR_CANARY");
  });
  it("현재 기본값 유지와 명시적 unset을 서로 다른 닫힌 요청으로 보낸다", async () => {
    const { controller, transport } = await setup();
    fireEvent.click(
      form(text("field.default")).getByRole("button", {
        name: "현재 기본값 적용",
      }),
    );
    await idle(controller);
    expect(transport.writes[0].input).toMatchObject({
      edit: { kind: "keep_default", field: "number" },
    });
    expect(controller.snapshot().selection!.content.revision).toBe(
      "9007199254740993",
    );
    fireEvent.change(
      form(text("field.default")).getByLabelText(text("field.default")),
      { target: { value: "unset" } },
    );
    fireEvent.click(
      form(text("field.default")).getByRole("button", {
        name: "현재 기본값 적용",
      }),
    );
    await idle(controller);
    expect(transport.writes[1].input).toMatchObject({
      edit: { kind: "default", field: "number", value: { kind: "unset" } },
    });
    expect(
      controller.snapshot().selection!.content.fields[0].initialDefault,
    ).toEqual({ kind: "number", value: "-0.1" });
  });
  it("미제출 Field 전환을 취소하면 숫자 입력과 선택을 보존한다", async () => {
    const { controller } = await setup();
    setDefault("-");
    fireEvent.click(screen.getByRole("button", { name: "기간 정의" }));
    await screen.findByRole("alertdialog");
    fireEvent.click(screen.getByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(controller.snapshot().fieldEditor!.field).toBe("number");
    const restored = await screen.findByRole("form", {
      name: text("field.default"),
    });
    expect(within(restored).getByLabelText(text("field.value"))).toHaveValue(
      "-",
    );
  });
  it("정상 종료 null runtime과 초기화 미완료 문구를 구분한다", async () => {
    const { controller, transport } = await setup();
    transport.runtime = null;
    transport.projectStopped = true;
    await act(async () => controller.checkStatus());
    expect(screen.getByText(/프로젝트 종료됨/)).toBeInTheDocument();
    expect(screen.queryByText(/초기화 확인 필요/)).toBeNull();
    transport.projectStopped = false;
    transport.status = "InitializationFailed";
    await act(async () => controller.checkStatus());
    expect(screen.getByText(/초기화 확인 필요/)).toBeInTheDocument();
  });
});
