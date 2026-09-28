import type { ReactNode } from "react";
import { Info16Regular } from "@fluentui/react-icons";
import "./HelpText.css";

/** 보이는 도움말은 입력의 설명으로 연결한다. 아이콘은 중복 낭독·Tab 정지점을 만들지 않는다. */
export function HelpText({
  id,
  children,
}: {
  id?: string;
  children: ReactNode;
}) {
  return (
    <span id={id} className="input-help">
      <Info16Regular aria-hidden="true" focusable="false" />
      <span>{children}</span>
    </span>
  );
}
