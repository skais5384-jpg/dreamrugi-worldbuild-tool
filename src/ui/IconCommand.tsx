import { Tooltip, useRestoreFocusTarget } from "@fluentui/react-components";
import type { MouseEventHandler, ReactElement } from "react";
import { Button } from "./Controls";

/** 헤더와 조밀한 도구 영역에서 같은 크기·설명 방식으로 쓰는 아이콘 명령이다. */
export function IconCommand({
  label,
  icon,
  disabled,
  onClick,
  className,
  restoreFocus = false,
}: {
  label: string;
  icon: ReactElement;
  disabled?: boolean;
  onClick?: MouseEventHandler<HTMLButtonElement>;
  className?: string;
  restoreFocus?: boolean;
}) {
  const focusAttributes = useRestoreFocusTarget();
  return (
    <Tooltip content={label} relationship="label">
      <Button
        {...(restoreFocus ? focusAttributes : {})}
        type="button"
        appearance="subtle"
        aria-label={label}
        icon={icon}
        disabled={disabled}
        onClick={onClick}
        className={["icon-command", className].filter(Boolean).join(" ")}
      />
    </Tooltip>
  );
}
