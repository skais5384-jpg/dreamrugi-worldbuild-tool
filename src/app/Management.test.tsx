import { render } from "../test/render";
import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";
import { Fields } from "./Fields";
import { GuardedClient } from "../bridge/client";
import { TemplateController } from "./controller";
import { TestTransport } from "./testTransport";
import { optionKey, type FieldProperty } from "./fieldEditing";
import type { Field, Value } from "../bridge/types";
import { text } from "../strings";

async function setup(multi = false) {
  const transport = new TestTransport();
  const field: Field = {
    id: "choice",
    label: "긴 선택 이름\n둘째 줄",
    kind: multi ? "MultiChoice" : "SingleChoice",
    lifecycle: "Active",
    required: true,
    presentation: "<b>token</b>",
    introducedRevision: "1",
    default: multi
      ? { kind: "multi_choice", options: ["a", "b"] }
      : { kind: "single_choice", option: "a" },
    initialDefault: { kind: "single_choice", option: "a" },
    options: [
      { id: "a", label: "첫 선택", lifecycle: "Active" },
      { id: "b", label: "둘째 선택", lifecycle: "Active" },
      { id: "old", label: "보관 선택", lifecycle: "Archived" },
    ],
    optionOrder: ["a", "b"],
  };
  if (multi) field.initialDefault = { kind: "multi_choice", options: ["a"] };
  transport.templates.set("template", {
    id: "template",
    name: "관리",
    revision: "10",
    lifecycle: "Active",
    presentation: null,
    fieldOrder: ["choice", "other"],
    fields: [
      field,
      {
        ...field,
        id: "other",
        label: "다른 Field",
        kind: "SingleLineText",
        required: false,
        presentation: null,
        options: [],
        optionOrder: [],
        default: { kind: "unset" },
        initialDefault: { kind: "unset" },
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
    await controller.navigate({ kind: "field", id: "choice" });
  });
  return { controller, transport, view };
}
const form = (name: string) => within(screen.getByRole("form", { name }));
const draft = (c: TemplateController, p: FieldProperty) =>
  c.snapshot().fieldEditor!.drafts.find((d) => d.property === p)!;
const idle = async (c: TemplateController) =>
  waitFor(() => expect(c.snapshot().busy).toBe(false));
const labels = (c: TemplateController) =>
  c.snapshot().selection!.content.fields[0];
function dataTransfer() {
  const data = new Map<string, string>();
  return {
    types: ["application/x-worldbuild-order"],
    files: [],
    effectAllowed: "",
    dropEffect: "",
    setData: (k: string, v: string) => data.set(k, v),
    getData: (k: string) => data.get(k) ?? "",
  };
}

describe("Option·보관·순서 UI 연결", () => {
  it("Option 이름 textbox는 선택지 label과 연결되고 같은 owner만 저장한다", async () => {
    const { controller: c, transport: t } = await setup();
    act(() => c.startOption("b"));
    const f = form(text("option.rename"));
    expect(
      f.getByRole("heading", { name: text("option.rename") }),
    ).toBeVisible();
    const input = f.getByRole("textbox", { name: text("option.label") });
    expect(f.getByLabelText(text("option.label"))).toBe(input);
    expect(f.queryByRole("textbox", { name: text("field.label") })).toBeNull();
    expect(
      form(text("field.label")).getByRole("textbox", {
        name: text("field.label"),
      }),
    ).toHaveValue(labels(c).label);
    const before = structuredClone(labels(c));
    const raw = "\n 이름 🌿 \n";
    fireEvent.change(input, { target: { value: raw } });
    fireEvent.click(f.getByRole("button", { name: "선택지 이름 변경 적용" }));
    await idle(c);
    expect(labels(c)).toEqual({
      ...before,
      options: before.options.map((o) =>
        o.id === "b" ? { ...o, label: raw } : o,
      ),
    });
    expect(t.writes).toHaveLength(1);
    expect(f.getByRole("textbox", { name: text("option.label") })).toHaveValue(
      raw,
    );
  });
  it.each(["fields", "options"] as const)(
    "%s 순서의 확정/Unchanged 뒤 안내를 지우고 새 편집과 취소를 표시한다",
    async (kind) => {
      const { controller: c } = await setup();
      await act(async () => {
        if (kind === "fields") await c.navigate({ kind: "fields_order" });
        else c.startOrder();
      });
      const f = form(text(`order.${kind}`));
      const name = kind === "fields" ? labels(c).label : "첫 선택";
      const down = () =>
        f.getByRole("button", { name: text("order.downName", { name }) });
      const up = () =>
        f.getByRole("button", { name: text("order.upName", { name }) });
      const apply = () =>
        f.getByRole("button", { name: `${text(`order.${kind}`)} 적용` });
      fireEvent.click(down());
      expect(f.getByText(/아직 저장하지 않았습니다/)).toHaveAttribute(
        "role",
        "status",
      );
      fireEvent.click(apply());
      await idle(c);
      expect(c.snapshot().selection!.content.revision).toBe("11");
      expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
      expect(f.getByText(text("field.saved"))).toHaveAttribute(
        "role",
        "status",
      );
      fireEvent.click(up());
      expect(f.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
      fireEvent.click(f.getByRole("button", { name: text("order.cancel") }));
      expect(f.getByText(text("order.cancelled"))).toBeVisible();
      expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
      fireEvent.click(up());
      fireEvent.click(down());
      expect(f.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
      fireEvent.click(apply());
      await idle(c);
      expect(c.snapshot().selection!.content.revision).toBe("11");
      expect(f.getByText(text("field.unchanged"))).toHaveAttribute(
        "role",
        "status",
      );
      expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
    },
  );
  it("다른 속성 저장과 명시적 재조회는 미제출 순서의 안내를 확정으로 바꾸지 않는다", async () => {
    const { controller: c } = await setup();
    act(() => {
      c.startOrder();
      c.startOption("b");
    });
    const f = form(text("order.options"));
    fireEvent.click(
      f.getByRole("button", {
        name: text("order.downName", { name: "첫 선택" }),
      }),
    );
    const original = draft(c, "options_order").source;
    const rename = form(text("option.rename"));
    fireEvent.change(
      rename.getByRole("textbox", { name: text("option.label") }),
      { target: { value: "새 이름" } },
    );
    fireEvent.click(
      rename.getByRole("button", { name: "선택지 이름 변경 적용" }),
    );
    await idle(c);
    expect(draft(c, "options_order").source).toBe(original);
    expect(f.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
    await act(async () => c.reconfirmField("options_order"));
    expect(draft(c, "options_order").source).not.toBe(original);
    expect(f.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
    fireEvent.click(f.getByRole("button", { name: "선택지 순서 편집 적용" }));
    await idle(c);
    expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
  });
  it("미수락·응답 유실은 안내와 다른 입력을 보존하고 확정 뒤 읽기 실패만 별도로 남긴다", async () => {
    const { controller: c, transport: t, view } = await setup();
    act(() => {
      c.startOrder();
      c.startOption("b");
    });
    const other = optionKey("choice", "b", "rename");
    const rename = form(text("option.rename"));
    fireEvent.change(
      rename.getByRole("textbox", { name: text("option.label") }),
      { target: { value: " 보존 \n" } },
    );
    const originalOther = draft(c, other);
    const f = form(text("order.options"));
    fireEvent.click(
      f.getByRole("button", {
        name: text("order.downName", { name: "첫 선택" }),
      }),
    );
    t.refuseWriteOnce = true;
    t.loseSubmit = true;
    t.failRead = true;
    fireEvent.click(f.getByRole("button", { name: "선택지 순서 편집 적용" }));
    fireEvent.click(
      await screen.findByRole("button", { name: text("followUp.openActions") }),
    );
    await screen.findByText("아직 수락되지 않은 예약");
    view.rerender(<App controller={c} />);
    expect(f.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
    expect(draft(c, "options_order").committed).toBe(false);
    expect(draft(c, other)).toBe(originalOther);
    fireEvent.click(
      screen.getByRole("button", { name: "같은 ID로 제출 재시도" }),
    );
    await idle(c);
    expect(draft(c, "options_order").committed).toBe(true);
    expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
    expect(f.getByText(text("field.saved"))).toHaveAttribute("role", "status");
    const detailsButton = await screen.findByRole("button", {
      name: text("update.details"),
    });
    detailsButton.focus();
    fireEvent.click(detailsButton);
    expect(
      within(await screen.findByRole("dialog"))
        .getAllByText(text("field.followup"))
        .every((item) => item.closest("td")),
    ).toBe(true);
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "닫기" }),
    );
    expect(draft(c, other)).toBe(originalOther);
    expect(t.writes).toHaveLength(1);
    t.failRead = false;
    await act(async () => c.reconfirmField("options_order"));
    expect(f.queryByText(/아직 저장하지 않았습니다/)).toBeNull();
    const currentForm = within(
      await screen.findByRole("form", { name: text("order.options") }),
    );
    await waitFor(() =>
      expect(
        currentForm.getByRole("button", {
          name: text("order.upName", { name: "첫 선택" }),
        }),
      ).toBeEnabled(),
    );
    fireEvent.click(
      currentForm.getByRole("button", {
        name: text("order.upName", { name: "첫 선택" }),
      }),
    );
    expect(currentForm.getByText(/아직 저장하지 않았습니다/)).toBeVisible();
    expect(draft(c, other)).toBe(originalOther);
  }, 10_000);
  it("활성 읽기 전용도 저장 속성을 표시하고 관리 form과 mutation을 열지 않는다", async () => {
    const { controller: c, transport: t } = await setup();
    t.status = "ReadOnly";
    await act(async () => {
      await c.checkStatus();
      await c.navigate({ kind: "field", id: "choice" });
    });
    expect(screen.getByText(text("field.requiredTrue"))).toBeInTheDocument();
    expect(screen.queryByText("<b>token</b>")).toBeNull();
    expect(screen.queryByRole("form")).toBeNull();
    await act(async () => {
      c.startOption();
      c.startOrder();
      c.startArchive("a");
      await c.saveField("default");
    });
    expect(c.snapshot().fieldEditor!.drafts).toEqual([]);
    expect(t.writes).toHaveLength(0);
  });
  it("Option별 이름 owner와 기본값 원 입력을 분리하고 빈/여러 줄 이름을 보존한다", async () => {
    const { controller: c, transport: t } = await setup();
    await act(async () => {
      c.startOption("a");
      c.startOption("b");
      c.startOrder();
    });
    const a = optionKey("choice", "a", "rename"),
      b = optionKey("choice", "b", "rename");
    const old = draft(c, b).source;
    await act(async () => {
      c.setField(a, {
        kind: "rename_option",
        field: "choice",
        option: "a",
        label: "",
      });
      c.setField(b, {
        kind: "rename_option",
        field: "choice",
        option: "b",
        label: "\n 보존 🌿 \n",
      });
      c.setField("default", {
        kind: "default",
        field: "choice",
        value: { kind: "unset" },
      });
      await c.saveField(a);
    });
    expect(labels(c).options[0].label).toBe("");
    expect(draft(c, b).source).toBe(old);
    expect(draft(c, b).edit).toMatchObject({ label: "\n 보존 🌿 \n" });
    expect(draft(c, "default").source).toBe(old);
    expect(draft(c, "options_order").source).toBe(old);
    expect(t.writes).toHaveLength(1);
    expect(labels(c).default).toEqual({ kind: "single_choice", option: "a" });
    await act(async () => {
      c.setField(b, {
        kind: "rename_option",
        field: "choice",
        option: "a",
        label: "wrong owner",
      });
    });
    expect(draft(c, b).edit).toMatchObject({
      option: "b",
      label: "\n 보존 🌿 \n",
    });
  });
  it("추가 ID는 Full·응답 유실과 remount에서 유지하고 확정 후 읽기만 재시도한다", async () => {
    const { controller: c, transport: t, view } = await setup();
    act(() => c.startOption());
    const add = c
      .snapshot()
      .fieldEditor!.drafts.find((d) => d.edit.kind === "add_option")!;
    t.refuseWriteOnce = true;
    t.loseSubmit = true;
    t.failRead = true;
    fireEvent.click(
      form(text("option.add")).getByRole("button", {
        name: "선택지 추가 적용",
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: text("followUp.openActions") }),
    );
    await screen.findByText("아직 수락되지 않은 예약");
    view.unmount();
    render(<App controller={c} />);
    fireEvent.click(
      screen.getByRole("button", { name: text("followUp.openActions") }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "같은 ID로 제출 재시도" }),
    );
    await idle(c);
    expect(t.writes).toHaveLength(1);
    expect(draft(c, add.property).committed).toBe(true);
    expect(draft(c, add.property).edit).toEqual(add.edit);
    await act(async () => c.saveField(add.property));
    expect(t.writes).toHaveLength(1);
    t.failRead = false;
    await act(async () => c.reconfirmField(add.property));
    expect(
      c.snapshot().fieldEditor!.drafts.some((d) => d.property === add.property),
    ).toBe(false);
    expect(t.writes).toHaveLength(1);
  }, 10_000);
  it("키보드와 drag가 같은 미리보기만 바꾸며 Escape·외부 drop은 이전 미리보기를 보존한다", async () => {
    const { controller: c, transport: t } = await setup();
    act(() => c.startOrder());
    const f = form(text("order.options"));
    fireEvent.click(
      f.getByRole("button", {
        name: text("order.downName", { name: "첫 선택" }),
      }),
    );
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["b", "a"],
    });
    expect(document.activeElement).toHaveAttribute("data-order-id", "a");
    const rows = f.getAllByRole("listitem");
    const dt = dataTransfer();
    fireEvent.dragStart(rows[1], { dataTransfer: dt });
    expect(
      f.getByText(text("order.dragStarted", { name: "첫 선택" })),
    ).toBeVisible();
    fireEvent.keyDown(rows[1], { key: "Escape" });
    fireEvent.drop(rows[0], { dataTransfer: dt });
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["b", "a"],
    });
    fireEvent.dragStart(rows[1], { dataTransfer: dt });
    fireEvent.dragEnd(rows[1], { dataTransfer: dt });
    expect(f.getByText(text("order.dragCancelled"))).toBeVisible();
    fireEvent.drop(rows[0], { dataTransfer: dt });
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["b", "a"],
    });
    fireEvent.dragStart(rows[1], { dataTransfer: dt });
    expect(fireEvent.dragEnter(rows[0], { dataTransfer: dt })).toBe(false);
    fireEvent.drop(rows[0], { dataTransfer: dt });
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["a", "b"],
    });
    expect(t.writes).toHaveLength(0);
    fireEvent.click(f.getByRole("button", { name: "선택지 순서 편집 적용" }));
    await idle(c);
    expect(c.snapshot().selection!.content.revision).toBe("10");
  });
  it("순서 전체 취소는 원 순서를 복원하며 membership 변경과 늦은 drag는 입력을 합성하지 않는다", async () => {
    const { controller: c, transport: t } = await setup();
    act(() => c.startOrder());
    const f = form(text("order.options"));
    const rows = f.getAllByRole("listitem");
    const dt = dataTransfer();
    fireEvent.dragStart(rows[0], { dataTransfer: dt });
    const field = t.templates.get("template")!.fields[0];
    field.optionOrder.push("new");
    field.options.push({ id: "new", label: "새 항목", lifecycle: "Active" });
    t.templates.get("template")!.revision = "11";
    await act(async () => c.refresh());
    fireEvent.drop(rows[1], { dataTransfer: dt });
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["a", "b"],
    });
    await act(async () => {
      await c.reconfirmField("options_order");
      await c.saveField("options_order");
    });
    expect(draft(c, "options_order").source.content.revision).toBe("10");
    expect(t.writes).toHaveLength(0);
    act(() => c.cancelOrder("options_order"));
    expect(draft(c, "options_order").edit).toMatchObject({
      options: ["a", "b"],
    });
  });
  it.each([0, 1, 2])(
    "Field 전체 순서 %i개는 정확한 permutation이고 변경 전 저장하지 않는다",
    async (count) => {
      const { controller: c, transport: t } = await setup();
      const template = t.templates.get("template")!;
      template.fields = template.fields.slice(0, count);
      template.fieldOrder = template.fieldOrder.slice(0, count);
      await act(async () => {
        await c.refresh();
        await c.navigate({ kind: "fields_order" });
      });
      const f = form(text("order.fields"));
      expect(f.queryAllByRole("listitem")).toHaveLength(count);
      if (count === 2) {
        fireEvent.click(
          f.getByRole("button", {
            name: text("order.downName", { name: template.fields[0].label }),
          }),
        );
        expect(t.writes).toHaveLength(0);
        fireEvent.click(f.getByRole("button", { name: text("order.cancel") }));
      }
      fireEvent.click(f.getByRole("button", { name: "필드 순서 편집 적용" }));
      await idle(c);
      expect(c.snapshot().selection!.content.revision).toBe("10");
      expect(t.writes).toHaveLength(1);
    },
  );
  it.each([false, true])(
    "저장된 %s choice의 repair는 보관과 한 요청이고 최초값·다른 미제출 입력은 유지한다",
    async (multi) => {
      const { controller: c, transport: t } = await setup(multi);
      act(() => {
        c.setField("default", {
          kind: "default",
          field: "choice",
          value: { kind: "unset" },
        });
        c.startArchive("a");
      });
      const key = optionKey("choice", "a", "archive");
      const old = draft(c, "default");
      const f = form(text("archive.option"));
      expect(f.getByText(text("archive.repairRequired"))).toBeInTheDocument();
      fireEvent.click(f.getByRole("button", { name: "선택지 보관 적용" }));
      await idle(c);
      expect(t.writes).toHaveLength(0);
      const repair: Value = multi
        ? { kind: "multi_choice", options: ["b"] }
        : { kind: "single_choice", option: "b" };
      await act(async () => {
        c.setField(key, {
          kind: "archive_option",
          field: "choice",
          option: "a",
          repair,
        });
        c.confirmArchive(key, true);
        await c.saveField(key);
      });
      expect(t.writes).toHaveLength(1);
      expect(t.writes[0].input).toMatchObject({
        edit: { kind: "archive_option", repair },
      });
      expect(labels(c).default).toEqual(repair);
      expect(labels(c).initialDefault).toEqual(
        multi
          ? { kind: "multi_choice", options: ["a"] }
          : { kind: "single_choice", option: "a" },
      );
      expect(draft(c, "default")).toBe(old);
      expect(c.snapshot().selection!.content.revision).toBe("11");
    },
  );
  it.each([
    null,
    { kind: "multi_choice", options: [] },
    { kind: "multi_choice", options: ["a"] },
    { kind: "multi_choice", options: ["old"] },
    { kind: "multi_choice", options: ["foreign"] },
  ] satisfies (Value | null)[])(
    "잘못된 repair %j는 UI 입력과 원 자료를 보존하고 전송하지 않는다",
    async (repair) => {
      const { controller: c, transport: t } = await setup(true);
      const key = optionKey("choice", "a", "archive");
      await act(async () => {
        c.startArchive("a");
        c.setField(key, {
          kind: "archive_option",
          field: "choice",
          option: "a",
          repair,
        });
        c.confirmArchive(key, true);
        await c.saveField(key);
      });
      expect(t.writes).toHaveLength(0);
      expect(draft(c, key).edit).toMatchObject({ repair });
      expect(labels(c).optionOrder).toEqual(["a", "b"]);
    },
  );
  it("initial만 참조하는 Option은 repair null이며 불필요한 repair를 막는다", async () => {
    const { controller: c, transport: t } = await setup();
    t.templates.get("template")!.fields[0].default = {
      kind: "single_choice",
      option: "b",
    };
    await act(async () => c.refresh());
    const key = optionKey("choice", "a", "archive");
    await act(async () => {
      c.startArchive("a");
      c.setField(key, {
        kind: "archive_option",
        field: "choice",
        option: "a",
        repair: { kind: "unset" },
      });
      c.confirmArchive(key, true);
      await c.saveField(key);
    });
    expect(t.writes).toHaveLength(0);
    expect(
      screen.getAllByText(text("archive.unnecessaryRepair")).length,
    ).toBeGreaterThan(0);
    await act(async () => {
      c.setField(key, {
        kind: "archive_option",
        field: "choice",
        option: "a",
        repair: null,
      });
      c.confirmArchive(key, true);
      await c.saveField(key);
    });
    expect(labels(c).default).toEqual({ kind: "single_choice", option: "b" });
    expect(labels(c).initialDefault).toEqual({
      kind: "single_choice",
      option: "a",
    });
    expect(t.writes).toHaveLength(1);
  });
  it("stale archive는 입력을 남기고 최신 저장값 조회와 새 동의를 요구하며 IME Enter는 제출하지 않는다", async () => {
    const { controller: c, transport: t } = await setup();
    const key = optionKey("choice", "a", "archive");
    act(() => {
      c.startArchive("a");
      c.setField(key, {
        kind: "archive_option",
        field: "choice",
        option: "a",
        repair: { kind: "unset" },
      });
      c.confirmArchive(key, true);
    });
    t.templates.get("template")!.revision = "11";
    await act(async () => c.refresh());
    await act(async () => c.saveField(key));
    expect(t.writes).toHaveLength(0);
    expect(draft(c, key).confirmation).toBe(false);
    await act(async () => c.reconfirmField(key));
    expect(draft(c, key).edit).toMatchObject({ repair: { kind: "unset" } });
    const f = screen.getByRole("form", { name: text("archive.option") });
    fireEvent.compositionStart(f);
    act(() => c.confirmArchive(key, true));
    // keydown과 독립적인 조합 중 submit 방어를 먼저 확인한다.
    fireEvent.submit(f);
    expect(t.writes).toHaveLength(0);
    expect(fireEvent.keyDown(f, { key: "Enter", isComposing: true })).toBe(
      false,
    );
    fireEvent.compositionEnd(f);
    fireEvent.keyUp(f, { key: "Enter" });
    fireEvent.click(
      within(f).getByRole("button", {
        name: text("field.apply", { property: text("archive.option") }),
      }),
      { detail: 1 },
    );
    await idle(c);
    expect(t.writes).toHaveLength(1);
  });
  it("Field 보관의 dirty 취소는 모든 입력을 유지하고 명시적 포기 후 보관은 조회 전용이다", async () => {
    const { controller: c } = await setup();
    act(() => {
      c.startOption();
      c.startArchive();
    });
    expect(screen.getByRole("alertdialog")).toHaveTextContent(
      text("archive.discardField"),
    );
    const before = c.snapshot().fieldEditor;
    fireEvent.click(
      screen.getByRole("button", { name: text("common.continue") }),
    );
    await idle(c);
    expect(c.snapshot().fieldEditor).toBe(before);
    act(() => c.startArchive());
    fireEvent.click(
      screen.getByRole("button", { name: text("common.discard") }),
    );
    await idle(c);
    act(() => c.confirmArchive("archive_field", true));
    fireEvent.click(
      form(text("archive.field")).getByRole("button", {
        name: "필드 보관 적용",
      }),
    );
    await idle(c);
    expect(c.snapshot().fieldEditor!.drafts).toEqual([]);
    expect(screen.getByText(text("field.requiredTrue"))).toBeInTheDocument();
    expect(screen.queryByText("<b>token</b>")).toBeNull();
    expect(
      screen.queryByRole("button", { name: text("option.add") }),
    ).toBeNull();
  });
  it.each([true, false])(
    "0005 required=%s는 archived/읽기 전용에 남고 presentation 메타데이터는 노출하지 않는다",
    async (required) => {
      const { controller: c, transport: t, view } = await setup();
      view.unmount();
      for (const presentation of [null, "", "<img src=x onerror=alert(1)>"]) {
        const field = t.templates.get("template")!.fields[0];
        field.required = required;
        field.presentation = presentation;
        field.lifecycle = "Archived";
        t.templates.get("template")!.fieldOrder = ["other"];
        await act(async () => {
          await c.refresh();
          await c.navigate({ kind: "field", id: "choice" });
        });
        const v = render(
          <Fields controller={c} canEdit={false} locked={false} />,
        );
        expect(
          screen.getByText(
            text(required ? "field.requiredTrue" : "field.requiredFalse"),
          ),
        ).toBeInTheDocument();
        expect(screen.queryByText(text("field.noToken"))).toBeNull();
        expect(screen.queryByText(text("field.emptyToken"))).toBeNull();
        if (presentation) expect(screen.queryByText(presentation)).toBeNull();
        expect(screen.queryByRole("form")).toBeNull();
        expect(v.container.querySelector("img")).toBeNull();
        act(() => {
          c.startOption();
          c.startArchive("a");
          c.startOrder();
        });
        expect(c.snapshot().fieldEditor!.drafts).toEqual([]);
        v.unmount();
      }
    },
  );
});
