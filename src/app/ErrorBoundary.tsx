import { Component, type ReactNode } from "react";
import { useFocusFinders } from "@fluentui/react-components";
import { text } from "../strings";
import { Button } from "../ui/Controls";

import "./ErrorBoundary.css";

interface ErrorBoundaryProps {
  children: ReactNode;
  fluent?: boolean;
}

interface ErrorBoundaryState {
  hasError: boolean;
}

class RenderErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = {
    hasError: false,
  };

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { hasError: true };
  }

  componentDidCatch() {
    // 렌더링 오류에 사용자 값이 섞일 수 있으므로 원 오류와 component stack은 기본 로그에 복사하지 않는다.
    console.error(text("errorBoundary.message01"));
  }

  render() {
    if (this.state.hasError) {
      return (
        <main className="error-boundary" role="alert">
          <h1>{text("errorBoundary.message02")}</h1>
          <p>{text("errorBoundary.message03")}</p>
          {this.props.fluent ? (
            <Button
              type="button"
              appearance="primary"
              onClick={() => window.location.reload()}
            >
              {text("errorBoundary.message04")}
            </Button>
          ) : (
            <button
              className="emergency-button"
              type="button"
              onClick={() => window.location.reload()}
            >
              {text("errorBoundary.message04")}
            </button>
          )}
        </main>
      );
    }

    return this.props.children;
  }
}

export default function ErrorBoundary(props: ErrorBoundaryProps) {
  // 이 공개 훅은 해당 문서의 Tabster 참조를 mount부터 cleanup까지 유지한다.
  // App/Dialog가 오류로 해제돼도 같은 root에 남는 fallback의 숨김 복구가
  // 끝나야 하므로, 교체되는 하위 화면이 아닌 오류 경계 자체가 수명을 소유한다.
  // 바깥 경계도 이 참조를 유지해 provider가 해제된 정적 fallback까지 보호한다.
  useFocusFinders();
  return <RenderErrorBoundary {...props} />;
}
