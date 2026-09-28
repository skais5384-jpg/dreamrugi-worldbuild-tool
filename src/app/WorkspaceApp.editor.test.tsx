import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, it, expect, beforeEach, vi } from "vitest";
import { render } from "../test/render";
import WorkspaceApp from "./WorkspaceApp";
import { nextGeneration } from "./workspaceController";
import { text } from "../strings";
import { problemTarget } from "./draftProblems";
import { setup } from "./WorkspaceApp.testHarness";

beforeEach(() => {
  // jsdom에는 스크롤 배치가 없다. 이동 요청만 검사하며 실제 가시성은 native에서 확인한다.
  HTMLElement.prototype.scrollIntoView = vi.fn();
  vi.spyOn(window, "scrollTo").mockImplementation(() => {});
  vi.spyOn(window, "scrollBy").mockImplementation(() => {});
  window.localStorage.clear();
});

describe("whole template workspace", () => {
  it("선택 기능 등록 실패를 안전한 지원 진단으로 표시한다", async () => {
    const { transport } = await setup();
    transport.supportDiagnosticChanged([
      {
        feature: "youtube_handler",
        stage: "request_filter",
        category: "webview2_com",
        causeId: "0x80070005",
        observedAtUtc: "2026-09-17T00:00:00.000Z",
      },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "fixture" }));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: text("health.menu") }),
    );
    fireEvent.click(screen.getByText(text("health.diagnostics")));
    expect(screen.getByText(/request_filter/)).toHaveTextContent("0x80070005");
    expect(screen.getByText(/request_filter/)).not.toHaveTextContent(
      "https://",
    );
  });
  it("Template 카드는 삭제된 항목을 숨기고 알 수 없는 상태는 읽기 전용으로 유지한다", async () => {
    const { transport, base, shell } = await setup();
    const definitions = [
      { id: "deleted-a", name: "삭제 정의 A", lifecycle: "Deleted" },
      { id: "active-a", name: "사용 정의 A", lifecycle: "Active" },
      { id: "deleted-b", name: "삭제 정의 B", lifecycle: "Deleted" },
      { id: "unknown", name: "확인할 정의", lifecycle: "FutureState" },
      { id: "active-b", name: "사용 정의 B", lifecycle: "Active" },
    ];
    for (const definition of definitions)
      transport.templates.set(definition.id, {
        ...base,
        ...definition,
        revision: "9",
      });
    await act(() => shell.refresh());
    const list = screen.getByRole("region", { name: text("app.message13") });
    const heading = within(list)
      .getByRole("heading", { name: text("app.message13") })
      .closest("header")!;
    const headingButtons = within(heading).getAllByRole("button");
    expect(
      headingButtons.map((button) => button.getAttribute("aria-label")),
    ).toEqual([
      text("navigation.panelCollapse"),
      text("app.message15"),
      text("app.message14"),
    ]);
    expect(headingButtons.every((button) => button.querySelector("svg"))).toBe(
      true,
    );
    const cards = within(list).getAllByRole("listitem");
    expect(
      cards.map((card) => card.querySelector("strong")?.textContent),
    ).toEqual(["사용 정의 A", "확인할 정의", "사용 정의 B"]);
    expect(within(list).queryByText("삭제 정의 A")).toBeNull();
    expect(within(list).queryByText("삭제 정의 B")).toBeNull();
    expect(list).not.toHaveTextContent("revision");
    expect(
      within(list).getByText(text("template.unknown")),
    ).toBeInTheDocument();
    expect(shell.snapshot().rows.map((row) => row.id)).toEqual(
      definitions.map((row) => row.id),
    );
    const unknown = within(list).getByRole("button", {
      name: /확인할 정의.*확인 필요/,
    });
    fireEvent.click(unknown);
    await waitFor(() =>
      expect(shell.snapshot().selection?.content.id).toBe("unknown"),
    );
    expect(unknown).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.getByRole("region", { name: text("app.message20") }),
    ).toHaveTextContent("상태 확인 필요");
    expect(screen.getByText(text("field.label"))).toBeInTheDocument();
    expect(screen.getByText(text("field.initial"))).toBeInTheDocument();
    expect(screen.getAllByText("숫자 필드").length).toBeGreaterThan(1);
    expect(
      screen.queryByLabelText(text("app.message23")),
    ).not.toBeInTheDocument();
  }, 15_000);

  it("Enter·조합 확정은 저장하지 않고 포커스·caret·동일 DOM을 보존한다", async () => {
    const { submitted } = await setup();
    const input = screen.getByLabelText(
      text("app.message23"),
    ) as HTMLTextAreaElement;
    input.focus();
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "가나다\n초안" } });
    input.setSelectionRange(2, 2);
    fireEvent.keyDown(input, {
      key: "Enter",
      code: "Enter",
      keyCode: 229,
      isComposing: true,
    });
    fireEvent.compositionEnd(input);
    fireEvent.keyUp(input, { key: "Enter" });
    expect(input).toBe(screen.getByLabelText(text("app.message23")));
    await waitFor(() => expect(input).toHaveFocus());
    expect(input.selectionStart).toBe(2);
    expect(submitted).toHaveLength(0);
    fireEvent.keyDown(input, { key: "Enter", code: "Enter" });
    expect(submitted).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: text("whole.save") }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    expect(submitted[0].body.name).toBe("가나다초안");
  });
  it("표현 설정은 숨기고 새 필드를 저장 전 부제목으로 바꿀 수 있다", async () => {
    await setup();
    expect(screen.queryByText(text("whole.presentation"))).toBeNull();
    expect(screen.queryByText(text("field.presentation"))).toBeNull();
    expect(screen.queryByText(text("section.manage"))).toBeNull();

    const fieldList = document.querySelector(".whole-field-list")!;
    const addField = screen.getByRole("button", {
      name: text("whole.newField"),
    });
    expect(addField.closest("li")).toBe(fieldList.lastElementChild);

    fireEvent.click(addField);
    expect(addField.closest("li")).toBe(fieldList.lastElementChild);
    const kind = screen.getByLabelText(text("field.kind"));
    expect(kind).toBeEnabled();
    fireEvent.change(kind, { target: { value: "section" } });

    expect(
      await screen.findByRole("heading", {
        name: text("whole.kind.section"),
        level: 4,
      }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(text("field.kind"))).toHaveValue("section");
  });
  it("무효 숫자 원문을 보관하고 늦은 보관 응답은 이후 입력을 정리하지 않는다", async () => {
    const { transport, controller, submitted } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "숫자 필드" }));
    fireEvent.change(screen.getByLabelText(text("field.default")), {
      target: { value: "set" },
    });
    const input = screen.getByLabelText(text("field.value"));
    fireEvent.change(input, { target: { value: "-" } });
    fireEvent.click(screen.getByRole("button", { name: text("whole.save") }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: text("whole.errors") }),
      ).toBeInTheDocument(),
    );
    expect(input).toHaveValue("-");
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: text("whole.deposit") }),
      ).toBeEnabled(),
    );
    transport.hold = "template_draft";
    fireEvent.click(
      screen.getByRole("button", { name: text("whole.deposit") }),
    );
    await waitFor(() => expect(controller.snapshot().busy).toBe(true));
    const before = controller.snapshot().draft!.generation;
    fireEvent.change(input, { target: { value: "." } });
    await act(async () => {
      transport.completeHeld();
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(input).toHaveValue(".");
    expect(controller.snapshot().draft!.status.receipt!.key.generation).toBe(
      before,
    );
    expect(controller.snapshot().draft!.generation).toBe(
      nextGeneration(before),
    );
    expect(controller.dirty()).toBe(true);
    expect(submitted[submitted.length - 1]?.body.fields[0].default).toEqual({
      intent: "set",
      value: { kind: "number", value: "-" },
    });
  });
  it("저장 중 최신 입력과 앱 수명 초안을 remount 후에도 보존한다", async () => {
    const { transport, controller, view } = await setup();
    const input = screen.getByLabelText(text("app.message23"));
    fireEvent.change(input, { target: { value: "세대 7" } });
    transport.hold = "template_draft";
    fireEvent.click(screen.getByRole("button", { name: text("whole.save") }));
    await waitFor(() => expect(controller.snapshot().busy).toBe(true));
    fireEvent.change(input, { target: { value: "세대 8 원문" } });
    await act(async () => {
      transport.completeHeld();
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(controller.dirty()).toBe(true);
    view.unmount();
    render(<WorkspaceApp controller={controller} />);
    expect(screen.getByLabelText(text("app.message23"))).toHaveValue(
      "세대 8 원문",
    );
    expect(transport.listenerCount).toBe(1);
  });
  it("이동·native close에서 보관/버리기/취소를 같은 owner에 적용한다", async () => {
    const { controller, transport } = await setup();
    const input = screen.getByLabelText(text("app.message23"));
    input.focus();
    fireEvent.change(input, { target: { value: "미저장 원문" } });
    fireEvent.click(screen.getByRole("button", { name: "fixture" }));
    fireEvent.click(
      screen.getByRole("menuitem", { name: text("whole.center") }),
    );
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByRole("button", { name: text("whole.keepEditing") }),
    ).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(input).toHaveFocus());
    await act(async () => {
      transport.nativeClose();
    });
    const closing = await screen.findByRole("alertdialog");
    fireEvent.click(
      within(closing).getByRole("button", { name: text("whole.keepEditing") }),
    );
    await waitFor(() => expect(controller.snapshot().prompt).toBeNull());
    expect(input).toHaveValue("미저장 원문");
    expect(transport.closing).toBe(false);
    // 상태의 취소와 Fluent Dialog의 aria-hidden 해제는 다른 렌더 경계다.
    // 사용자에게 메뉴가 다시 접근 가능해진 뒤 다음 전환을 시작한다.
    fireEvent.click(await screen.findByRole("button", { name: "fixture" }));
    fireEvent.click(
      screen.getByRole("menuitem", { name: text("whole.center") }),
    );
    fireEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: text("whole.deposit"),
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: text("whole.center") }),
      ).toBeInTheDocument(),
    );
    expect(controller.snapshot().draft).toBeNull();
  }, 15_000);

  it("저장 후 재읽기의 늦은 응답도 최신 원문을 덮지 않는다", async () => {
    const { transport, controller } = await setup();
    const previous = transport.workspaceResult!;
    transport.workspaceResult = (input) =>
      input.kind === "refresh_template_draft"
        ? {
            kind: "template_draft",
            status: {
              ...controller.snapshot().draft!.status,
              phase: "saved",
              savedGeneration: "1",
            },
          }
        : previous(input);
    transport.hold = "refresh_template_draft";
    let pending!: Promise<void>;
    act(() => {
      pending = controller.refreshSaved();
    });
    await waitFor(() => expect(controller.snapshot().busy).toBe(true));
    const input = screen.getByLabelText(text("app.message23"));
    fireEvent.change(input, { target: { value: "재읽기 도중 새 원문" } });
    await act(async () => {
      transport.completeHeld();
      await pending;
    });
    expect(input).toHaveValue("재읽기 도중 새 원문");
    expect(controller.dirty()).toBe(true);
  });
  it("유실된 저장 결과를 같은 operation으로 조회하고 닫기 대화상자에서도 회수한다", async () => {
    const { transport, controller, submitted } = await setup();
    fireEvent.change(screen.getByLabelText(text("app.message23")), {
      target: { value: "한 번만 저장" },
    });
    transport.loseResult = true;
    fireEvent.click(screen.getByRole("button", { name: text("whole.save") }));
    await waitFor(() => expect(submitted).toHaveLength(1));
    await act(async () => {
      transport.nativeClose();
    });
    const dialog = await screen.findByRole("alertdialog");
    transport.loseResult = false;
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("app.message03") }),
    );
    await waitFor(() => expect(controller.snapshot().busy).toBe(false));
    expect(submitted).toHaveLength(1);
    const submits = transport.commands.filter(
      (c) => c.action === "submit" && c.input.kind === "template_draft",
    );
    expect(submits).toHaveLength(1);
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("whole.keepEditing") }),
    );
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });
  it("요청 wrapper가 1 MiB를 넘으면 예약 없이 원문을 유지하고 수정 후 다시 저장한다", async () => {
    const { transport, controller, submitted } = await setup();
    const before = transport.commands.length;
    const raw = "한".repeat(350000);
    act(() => controller.edit((b) => ({ ...b, name: raw })));
    await act(async () => {
      await controller.save();
    });
    expect(controller.snapshot().error).toBe(text("whole.requestTooLarge"));
    expect(controller.snapshot().draft!.body.name).toBe(raw);
    expect(controller.snapshot().busy).toBe(false);
    expect(
      transport.commands.slice(before).filter((c) => c.action === "reserve"),
    ).toHaveLength(0);
    act(() => controller.edit((b) => ({ ...b, name: "크기를 줄인 원문" })));
    await act(async () => {
      await controller.save();
    });
    expect(submitted).toHaveLength(1);
  });
  it("복구 센터는 분리된 목록·내용 대화상자에서 닫으면 원래 탐색으로 돌아온다", async () => {
    const { controller } = await setup();
    await act(async () => {
      await controller.showCenter();
    });
    const dialog = screen.getByRole("dialog", { name: text("whole.center") });
    expect(
      within(dialog).getByRole("region", { name: text("recovery.listTitle") }),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByRole("region", { name: text("whole.recoveryDraft") }),
    ).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "닫기" }));
    await waitFor(() => expect(controller.snapshot().center).toBe(false));
    expect(
      screen.getByRole("button", { name: text("app.message14") }),
    ).toBeEnabled();
  });
  it("u64 세대의 상위 값도 Number로 바꾸지 않고 overflow를 거부한다", () => {
    expect(nextGeneration("9007199254740993")).toBe("9007199254740994");
    expect(() => nextGeneration("18446744073709551615")).toThrow();
  });

  it("원본 view는 전체 저장/목록 갱신 동안 보존하고 실제 종료 응답 후 회수한다", async () => {
    const { controller, shell, transport, base } = await setup();
    transport.templates.set(base.id, structuredClone(base));
    await act(async () => {
      await controller.navigate({ kind: "select", id: base.id });
    });
    const original = shell.snapshot().selection!.view;
    act(() => controller.edit((b) => ({ ...b, name: "명시적 변경" })));
    await act(async () => {
      await controller.save();
    });
    expect(shell.snapshot().selection!.view).not.toBe(original);
    expect(transport.released).not.toContain(original);
    const work = transport.workspaceResult!;
    transport.workspaceResult = (input) =>
      input.kind === "release_template_draft"
        ? { kind: "control", error: { code: "owners_remain", nextAction: "" } }
        : work(input);
    await act(async () => {
      await controller.navigate({ kind: "browse" });
    });
    expect(controller.snapshot().draft).not.toBeNull();
    expect(transport.released).not.toContain(original);
    transport.workspaceResult = work;
    await act(async () => {
      await controller.navigate({ kind: "browse" });
    });
    expect(controller.snapshot().draft).toBeNull();
    expect(transport.released).toContain(original);
  });

  it("실제 버튼 focus를 거친 취소도 마지막 입력의 선택과 스크롤을 복원한다", async () => {
    await setup();
    const input = screen.getByLabelText(
      text("app.message23"),
    ) as HTMLTextAreaElement;
    input.focus();
    fireEvent.change(input, { target: { value: "가나다\n끝" } });
    input.setSelectionRange(1, 3, "backward");
    input.scrollTop = 21;
    vi.spyOn(window, "scrollY", "get").mockReturnValue(700);
    const leave = screen.getByRole("button", {
      name: text("whole.endEditing"),
    });
    leave.focus();
    fireEvent.click(leave);
    const dialog = await screen.findByRole("alertdialog");
    input.scrollTop = 0;
    fireEvent.click(
      within(dialog).getByRole("button", { name: text("whole.keepEditing") }),
    );
    await waitFor(() => expect(input).toHaveFocus());
    expect(input.selectionStart).toBe(1);
    expect(input.selectionEnd).toBe(3);
    expect(input.selectionDirection).toBe("backward");
    expect(input.scrollTop).toBe(21);
    expect(window.scrollTo).toHaveBeenLastCalledWith({ left: 0, top: 700 });
  });

  it("숫자 오류 요약은 실제 기본값으로 이동하고 수정 입력에는 focus를 다시 빼앗지 않는다", async () => {
    const { controller } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "숫자 필드" }));
    fireEvent.change(screen.getByLabelText(text("field.default")), {
      target: { value: "set" },
    });
    const input = screen.getByLabelText(
      text("field.value"),
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "-" } });
    await act(async () => {
      await controller.save();
    });
    await waitFor(() =>
      expect(
        screen.getByRole("alert", { name: text("whole.errors") }),
      ).toHaveFocus(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: /현재 기본값 확인 및 수정/ }),
    );
    await waitFor(() => expect(input).toHaveFocus());
    expect(input).toHaveValue("-");
    fireEvent.change(input, { target: { value: "-25" } });
    input.setSelectionRange(2, 2);
    expect(
      screen.queryByRole("heading", { name: text("whole.errors") }),
    ).toBeNull();
    expect(input).toHaveFocus();
    expect(input.selectionStart).toBe(2);
    const draft = controller.snapshot().draft!;
    expect(
      problemTarget(
        { category: "Unknown", property: "global", field: null, option: null },
        draft,
      ),
    ).toBeNull();
    const withOption = {
      ...draft,
      body: {
        ...draft.body,
        fields: [
          {
            ...draft.body.fields[0],
            configuration: {
              kind: "single_choice" as const,
              options: [{ id: "option-one", label: "A", archived: false }],
            },
          },
        ],
      },
    };
    expect(
      problemTarget(
        {
          category: "InvalidOptionDraft",
          property: "option",
          field: "number-field",
          option: "option-one",
        },
        withOption,
      )?.input,
    ).toBe("whole-option-option-one");
  }, 10_000);

  it("축소된 필드 카드는 버튼 없이 키보드 순서 이동과 focus를 유지하고 경계에서는 초안을 바꾸지 않는다", async () => {
    const { controller } = await setup();
    act(() =>
      controller.edit((b) => ({
        ...b,
        fields: [
          ...b.fields,
          { ...b.fields[0], id: "second", label: "둘째 필드" },
          { ...b.fields[0], id: "third", label: "셋째 필드" },
        ],
      })),
    );
    const button = screen.getByRole("button", { name: "숫자 필드" });
    button.focus();
    const generation = controller.snapshot().draft!.generation;
    fireEvent.keyDown(button, { key: "ArrowUp", altKey: true });
    expect(controller.snapshot().draft!.generation).toBe(generation);
    fireEvent.keyDown(button, { key: "ArrowDown", altKey: true });
    expect(controller.snapshot().draft!.body.fields.map((f) => f.id)).toEqual([
      "second",
      "number-field",
      "third",
    ]);
    expect(document.activeElement).toBe(button);
    expect(
      screen.queryByRole("button", { name: "숫자 필드 위로 이동" }),
    ).not.toBeInTheDocument();
    act(() => controller.compose(true));
    const composingGeneration = controller.snapshot().draft!.generation;
    fireEvent.keyDown(button, { key: "ArrowDown", altKey: true });
    expect(controller.snapshot().draft!.generation).toBe(composingGeneration);
  }, 10_000);

  it("드래그 스냅은 DOM과 원문 세대를 유지하고 drop만 순서를 반영하며 취소/오래된 drag는 반영하지 않는다", async () => {
    const { controller } = await setup();
    act(() =>
      controller.edit((b) => ({
        ...b,
        fields: [
          ...b.fields,
          { ...b.fields[0], id: "second", label: "둘째 필드" },
        ],
      })),
    );
    const first = screen
      .getByRole("button", { name: "숫자 필드" })
      .closest("li")!;
    const second = screen
      .getByRole("button", { name: "둘째 필드" })
      .closest("li")!;
    vi.spyOn(second, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, 100, 100, 100),
    );
    const before = structuredClone(controller.snapshot().draft!);
    fireEvent.dragStart(first);
    fireEvent(
      second,
      new MouseEvent("dragover", { bubbles: true, clientY: 199, clientX: 99 }),
    );
    expect(first).toHaveClass("whole-drag-preview");
    expect(first.style.order).toBe("2");
    expect(controller.snapshot().draft!.generation).toBe(before.generation);
    expect(controller.snapshot().draft!.body).toEqual(before.body);
    expect(
      screen.getByRole("button", { name: "숫자 필드" }).closest("li"),
    ).toBe(first);
    fireEvent.dragEnd(first);
    expect(first.style.order).toBe("");
    expect(controller.snapshot().draft!.body).toEqual(before.body);
    fireEvent.dragStart(first);
    const outside = new MouseEvent("drop", { bubbles: true, cancelable: true });
    fireEvent(screen.getByLabelText(text("app.message23")), outside);
    expect(outside.defaultPrevented).toBe(true);
    expect(controller.snapshot().draft!.body).toEqual(before.body);
    expect(first).not.toHaveClass("whole-drag-preview");
    fireEvent.dragStart(first);
    fireEvent(
      second,
      new MouseEvent("dragover", { bubbles: true, clientY: 199, clientX: 99 }),
    );
    fireEvent.drop(second);
    expect(controller.snapshot().draft!.body.fields.map((f) => f.id)).toEqual([
      "second",
      "number-field",
    ]);
    fireEvent.dragStart(first);
    act(() => controller.edit((b) => ({ ...b, name: "더 최신 원문" })));
    expect(first).not.toHaveClass("whole-drag-preview");
    fireEvent.drop(second);
    expect(controller.snapshot().draft!.body.fields.map((f) => f.id)).toEqual([
      "second",
      "number-field",
    ]);
    expect(screen.getByText(text("whole.staleDrag"))).toBeInTheDocument();
  }, 10_000);
});
