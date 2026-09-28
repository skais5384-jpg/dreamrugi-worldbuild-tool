import { fireEvent, screen } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { render } from "../test/render";
import type { Field, Value } from "../bridge/types";
import { CreationValue, plain } from "./CreationValue";

const field: Field = {
  id: "n",
  label: "인구",
  kind: "Number",
  required: false,
  lifecycle: "Active",
  presentation: null,
  writingGuide: "예: 1000명",
  default: { kind: "number", value: "123" },
  initialDefault: { kind: "number", value: "5" },
  introducedRevision: "1",
  options: [],
  optionOrder: [],
};
describe("new document values", () => {
  it("가이드와 기존 기본값은 값이나 dirty intent가 되지 않으며 원시 숫자 입력을 유지한다", () => {
    const change = vi.fn();
    render(
      <CreationValue
        field={field}
        intent={{ intent: "keep" }}
        change={change}
        invalid={false}
        disabled={false}
      />,
    );
    const input = screen.getByRole("textbox", { name: "인구" });
    expect(input).toHaveValue("");
    expect(input).toHaveAttribute("placeholder", "예: 1000명");
    expect(screen.queryByText("예: 1000명")).toBeNull();
    expect(input).toHaveAccessibleDescription("예: 1000명");
    expect(screen.queryByText("123")).toBeNull();
    expect(change).not.toHaveBeenCalled();
    for (const raw of ["-", ".", "0", ""]) {
      fireEvent.change(input, { target: { value: raw } });
      if (raw)
        expect(change).toHaveBeenLastCalledWith({
          intent: "set",
          value: { kind: "number", value: raw },
        });
    }
  });
  it("오류 필드만 aria 연결과 오류 표시를 가지며 선택지의 첫 값을 자동 선택하지 않는다", () => {
    const choice = {
      ...field,
      kind: "SingleChoice",
      options: [{ id: "a", label: "A", lifecycle: "Active" }],
      optionOrder: ["a"],
    };
    const change = vi.fn();
    const view = render(
      <CreationValue
        field={choice}
        intent={{ intent: "keep" }}
        change={change}
        invalid
        disabled={false}
      />,
    );
    const input = screen.getByRole("combobox");
    expect(input).toHaveValue("");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("예: 1000명")).toBeInTheDocument();
    expect(input.getAttribute("aria-describedby")).toContain(
      "creation-n-error",
    );
    fireEvent.change(input, { target: { value: "a" } });
    expect(change).toHaveBeenCalledWith({
      intent: "set",
      value: { kind: "single_choice", option: "a" },
    });
    view.rerender(
      <CreationValue
        field={choice}
        intent={{
          intent: "set",
          value: { kind: "single_choice", option: "a" },
        }}
        change={change}
        invalid={false}
        disabled={false}
      />,
    );
    fireEvent.change(input, { target: { value: "" } });
    expect(change).toHaveBeenLastCalledWith({
      intent: "set",
      value: { kind: "single_choice", option: "" },
    });
  });
  it("줄바꿈과 복구된 체크되지 않은 task 값은 편집 전까지 그대로 남는다", () => {
    const change = vi.fn();
    const rich: Value = {
      kind: "rich_text",
      content: {
        kind: "root",
        children: [
          {
            kind: "taskList",
            children: [
              {
                kind: "taskItem",
                checked: false,
                children: [{ kind: "text", text: "미완료" }],
              },
            ],
          },
        ],
      },
    };
    const view = render(
      <CreationValue
        field={{ ...field, kind: "RichText" }}
        intent={{ intent: "set", value: rich }}
        change={change}
        invalid={false}
        disabled={false}
      />,
    );
    expect(screen.getByText("미완료")).toBeInTheDocument();
    expect(change).not.toHaveBeenCalled();
    view.rerender(
      <CreationValue
        key="new-rich-owner"
        field={{ ...field, kind: "RichText" }}
        intent={{ intent: "set", value: plain("앞\n뒤") }}
        change={change}
        invalid={false}
        disabled={false}
      />,
    );
    const input = screen.getByRole("textbox");
    expect(input).toHaveTextContent("앞뒤");
    expect(input).toHaveAttribute("contenteditable", "true");
    expect(change).not.toHaveBeenCalled();
  });
});
