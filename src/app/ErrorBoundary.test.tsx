import { render } from "../test/render";
import {
  act,
  fireEvent,
  render as renderRoot,
  screen,
  waitFor,
} from "@testing-library/react";
import { StrictMode, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Dialog, DialogSurface, DialogTitle } from "@fluentui/react-components";

import App from "./App";
import ErrorBoundary from "./ErrorBoundary";
import { AppProvider } from "../ui/AppProvider";
import { TemplateController } from "./controller";
import { GuardedClient } from "../bridge/client";
import { TestTransport } from "./testTransport";

function ThrowingComponent(): never {
  throw new Error("Error Boundary 테스트용 오류");
}

function RenderFault({
  fail,
  children,
}: {
  fail: boolean;
  children: ReactNode;
}) {
  if (fail) throw new Error("Error Boundary 테스트용 오류");
  return children;
}

describe("ErrorBoundary", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("하위 화면 오류가 발생하면 사용자용 대체 화면을 보여준다", () => {
    const consoleErrorSpy = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);

    render(
      <ErrorBoundary>
        <ThrowingComponent />
      </ErrorBoundary>,
    );

    expect(
      screen.getByRole("heading", { name: "화면을 표시하지 못했습니다" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "다시 불러오기" }),
    ).toBeInTheDocument();
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      "[Dreamrugi Worldbuild Tool][ErrorBoundary] 화면 렌더링 실패",
    );
  });

  it.each([
    { boundary: "app", modalOpen: false },
    { boundary: "app", modalOpen: true },
    { boundary: "provider", modalOpen: false },
    { boundary: "provider", modalOpen: true },
  ])(
    "$boundary 오류 교체 후 같은 root의 복구 화면에 접근한다 (modalOpen=$modalOpen)",
    async ({ boundary, modalOpen }) => {
      vi.spyOn(console, "error").mockImplementation(() => undefined);
      const transport = new TestTransport();
      const controller = new TemplateController(new GuardedClient(transport));
      // main과 같은 두 경계와 StrictMode를 사용한다. provider 경로는 안쪽
      // 경계 바깥의 render 오류를 주입해 AppProvider까지 해제되도록 한다.
      const tree = (fail: boolean) => (
        <StrictMode>
          <ErrorBoundary>
            <AppProvider>
              <RenderFault fail={fail && boundary === "provider"}>
                <ErrorBoundary fluent>
                  <RenderFault fail={fail && boundary === "app"}>
                    <App controller={controller} />
                  </RenderFault>
                </ErrorBoundary>
              </RenderFault>
            </AppProvider>
          </ErrorBoundary>
        </StrictMode>
      );
      const view = renderRoot(tree(false));
      const root = view.container;
      await waitFor(() => expect(controller.snapshot().ready).toBe(true));
      fireEvent.change(screen.getByLabelText("기존 프로젝트 폴더 경로"), {
        target: { value: "test-project" },
      });
      fireEvent.click(screen.getByRole("button", { name: "프로젝트 열기" }));
      await waitFor(() => expect(controller.snapshot().busy).toBe(false));
      fireEvent.click(screen.getByRole("button", { name: "새 템플릿" }));
      const input = await screen.findByLabelText("템플릿 이름");
      await waitFor(() => expect(input).toBeEnabled());
      const raw = "  오류 전 초안\n보존  ";
      fireEvent.change(input, { target: { value: raw } });
      input.focus();
      const draft = controller.snapshot().form;
      if (modalOpen) {
        await act(async () => transport.nativeClose());
        await screen.findByRole("alertdialog");
        // 실제 라이브러리의 숨김을 먼저 관측해야 즉시 throw 대조와 구별된다.
        await waitFor(() =>
          expect(input.closest('[aria-hidden="true"]')).not.toBeNull(),
        );
      }

      view.rerender(tree(true));
      expect(view.container).toBe(root);
      expect(input).not.toBeInTheDocument();
      expect(document.querySelector("main.error-boundary")).toHaveTextContent(
        "다시 불러오기",
      );
      expect(transport.writes).toHaveLength(0);
      const reloadButton = await screen.findByRole("button", {
        name: "다시 불러오기",
      });
      expect(
        screen.getByRole("heading", { name: "화면을 표시하지 못했습니다" }),
      ).toBeInTheDocument();
      expect(screen.getByRole("alert")).toBeInTheDocument();
      expect(reloadButton.closest('[aria-hidden="true"], [inert]')).toBeNull();
      expect(reloadButton).toBeEnabled();
      reloadButton.focus();
      expect(reloadButton).toHaveFocus();
      expect(root.querySelector(".fui-FluentProvider") !== null).toBe(
        boundary === "app",
      );
      expect(reloadButton.classList.contains("emergency-button")).toBe(
        boundary === "provider",
      );
      // DOM/접근성 경로는 실제로 실행하고, jsdom이 지원하지 않는 문서
      // 재로드 호출만 가로채 기존 버튼의 복구 행동 연결을 확인한다.
      const reload = vi.fn();
      vi.stubGlobal(
        "window",
        new Proxy(window, {
          get(target, property) {
            return property === "location"
              ? { reload }
              : Reflect.get(target, property);
          },
        }),
      );
      try {
        fireEvent.click(reloadButton);
        expect(reload).toHaveBeenCalledTimes(1);
      } finally {
        vi.unstubAllGlobals();
      }
      expect(controller.snapshot().form).toBe(draft);
      expect(draft?.name).toBe(raw);
      expect(transport.writes).toHaveLength(0);

      // 복구 뒤 최종 root 해제도 같은 컨테이너를 남긴 채 확인한다.
      view.unmount();
      expect(root).toBeInTheDocument();
      expect(root).toBeEmptyDOMElement();
      expect(root.closest('[aria-hidden="true"]')).toBeNull();
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    },
  );

  it("다른 활성 modal의 숨김을 보존하고 정상 닫힘 뒤 복구 화면을 노출한다", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const tree = (fail: boolean, open: boolean) => (
      <ErrorBoundary>
        <AppProvider>
          <ErrorBoundary fluent>
            <RenderFault fail={fail}>
              <button type="button">배경 동작</button>
            </RenderFault>
          </ErrorBoundary>
          <Dialog open={open} modalType="alert">
            <DialogSurface>
              <DialogTitle>다른 확인창</DialogTitle>
              <button type="button">안전한 선택</button>
            </DialogSurface>
          </Dialog>
        </AppProvider>
      </ErrorBoundary>
    );
    const view = renderRoot(tree(false, true));
    const background = screen.getByText("배경 동작");
    await waitFor(() =>
      expect(background.closest('[aria-hidden="true"]')).not.toBeNull(),
    );
    view.rerender(tree(true, true));
    expect(document.querySelector("main.error-boundary")).toBeInTheDocument();
    expect(
      screen.getByRole("alertdialog", { name: "다른 확인창" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "다시 불러오기" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "안전한 선택" })).toHaveFocus();

    view.rerender(tree(true, false));
    expect(
      await screen.findByRole("button", { name: "다시 불러오기" }),
    ).toBeEnabled();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    view.unmount();
    expect(view.container.closest('[aria-hidden="true"]')).toBeNull();
  });
});
