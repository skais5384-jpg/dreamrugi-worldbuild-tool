import { useState, type ReactNode } from "react";
import {
  createDOMRenderer,
  FluentProvider,
  RendererProvider,
  webLightTheme,
} from "@fluentui/react-components";

export const appTheme = {
  ...webLightTheme,
  fontFamilyBase:
    '"Noto Sans", "Malgun Gothic", "Apple SD Gothic Neo", sans-serif',
  fontWeightSemibold: 500,
};
function ThemedApp({ children }: { children: ReactNode }) {
  return (
    <FluentProvider theme={appTheme} className="app-provider">
      {children}
    </FluentProvider>
  );
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [renderer] = useState(() => {
    // 배포 HTML의 빈 style에 Tauri가 문서마다 부여한 nonce를 재사용한다.
    // getAttribute는 브라우저의 nonce 숨김 때문에 빈 문자열을 돌려줄 수 있다.
    const nonce =
      document.querySelector<HTMLStyleElement>("#app-style-nonce")?.nonce;
    return createDOMRenderer(document, {
      styleElementAttributes: nonce ? { nonce } : undefined,
    });
  });
  return (
    <RendererProvider renderer={renderer}>
      <ThemedApp>{children}</ThemedApp>
    </RendererProvider>
  );
}
