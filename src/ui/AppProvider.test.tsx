import { render, screen, fireEvent } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import {
  Dialog,
  DialogSurface,
  DialogBody,
  DialogTitle,
  DialogActions,
} from "@fluentui/react-components";
import { AppProvider } from "./AppProvider";
import {
  Button,
  Checkbox,
  Fieldset,
  Input,
  Select,
  Textarea,
} from "./Controls";

it("문서 nonce를 동적 규칙·테마·body portal에 전달한다", () => {
  const marker = document.createElement("style");
  marker.id = "app-style-nonce";
  marker.nonce = "test-only-document-nonce";
  document.head.append(marker);
  const before = new Set(document.querySelectorAll("style"));
  try {
    const view = render(
      <AppProvider>
        <Dialog open modalType="alert">
          <DialogSurface>
            <DialogBody>
              <DialogTitle>시험 확인</DialogTitle>
              <DialogActions>
                <Button type="button">계속 편집</Button>
              </DialogActions>
            </DialogBody>
          </DialogSurface>
        </Dialog>
      </AppProvider>,
    );
    const dialog = screen.getByRole("alertdialog");
    expect(view.container.contains(dialog)).toBe(false);
    const inserted = [...document.querySelectorAll("style")].filter(
      (style) => !before.has(style),
    );
    expect(inserted.length).toBeGreaterThan(1);
    for (const style of inserted) expect(style.nonce).toBe(marker.nonce);
    const rules = inserted
      .flatMap((style) =>
        [...(style.sheet?.cssRules ?? [])].map((rule) => rule.cssText),
      )
      .join("\n");
    expect(rules).toContain("Noto Sans");
    expect(rules).toContain("--fontWeightSemibold: 500");
    view.unmount();
  } finally {
    marker.remove();
    for (const style of document.querySelectorAll("style")) {
      if (!before.has(style)) style.remove();
    }
  }
});

it("상위 잠금을 실제 input slot에 전달하고 해제 후 원문·제출 경계를 유지한다", () => {
  const submitted = vi.fn();
  const form = (locked: boolean) => (
    <AppProvider>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submitted();
        }}
      >
        <Fieldset disabled={locked}>
          <Input aria-label="숫자" defaultValue="-" />
          <Textarea aria-label="이름" defaultValue={"  첫 줄\n둘째 줄  "} />
          <Checkbox label="필수" defaultChecked />
          <Select aria-label="선택" defaultValue="one">
            <option value="one">하나</option>
          </Select>
          <Button type="button">취소</Button>
          <Button type="submit">적용</Button>
        </Fieldset>
      </form>
    </AppProvider>
  );
  const view = render(form(true));
  for (const control of view.container.querySelectorAll(
    "input,textarea,select,button",
  ))
    expect(control).toHaveAttribute("disabled");
  fireEvent.click(screen.getByRole("button", { name: "적용" }));
  expect(submitted).not.toHaveBeenCalled();
  view.rerender(form(false));
  expect(screen.getByLabelText("숫자")).toHaveValue("-");
  expect(screen.getByLabelText("이름")).toHaveValue("  첫 줄\n둘째 줄  ");
  expect(screen.getByLabelText("필수")).toBeChecked();
  fireEvent.click(screen.getByRole("button", { name: "취소" }));
  expect(submitted).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "적용" }));
  expect(submitted).toHaveBeenCalledTimes(1);
});
