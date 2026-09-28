import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tauriConfig from "./src-tauri/tauri.conf.json";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    react(),
    {
      name: "app-style-nonce",
      // CSS 추출이 끝난 뒤 남기는 style은 Tauri의 공식 HTML nonce 삽입 대상이다.
      transformIndexHtml: {
        order: "post",
        handler: (_html, context) => [
          // HTTP devUrl은 Tauri asset protocol을 거치지 않는다. 같은 devCsp를 실제 문서에 적용한다.
          ...(context.server
            ? [
                {
                  tag: "meta",
                  attrs: {
                    "http-equiv": "Content-Security-Policy",
                    content: Object.entries(tauriConfig.app.security.devCsp)
                      .map(([directive, sources]) => `${directive} ${sources}`)
                      .join("; "),
                  },
                  injectTo: "head-prepend" as const,
                },
              ]
            : []),
          {
            tag: "style",
            attrs: { id: "app-style-nonce" },
            children: "/* Tauri runtime style nonce */",
            injectTo: "head",
          },
        ],
      },
    },
  ],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // native 빌드와 시험 자산/로그는 프런트 소스가 아니다. Windows의 파일 인수 중
      // 배타 handle을 watcher가 다시 열거나 fixture 저장으로 화면을 reload하지 않게 한다.
      ignored: ["**/src-tauri/**", "**/logs/**"],
    },
  },
}));
