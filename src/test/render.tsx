import {
  render as testingRender,
  type RenderOptions,
} from "@testing-library/react";
import type { ReactElement } from "react";
import { AppProvider } from "../ui/AppProvider";

/** 실제 앱과 동일한 provider/portal을 사용하여 입력과 focus 회귀를 확인한다. */
export function render(ui: ReactElement, options?: RenderOptions) {
  return testingRender(ui, { wrapper: AppProvider, ...options });
}
