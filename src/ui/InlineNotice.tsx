import {
  ErrorCircle16Filled,
  Info16Regular,
  Warning16Filled,
} from "@fluentui/react-icons";
import "./InlineNotice.css";

export function InlineNotice({
  kind,
  children,
  className = "",
}: {
  kind: "info" | "warning" | "error";
  children: React.ReactNode;
  className?: string;
}) {
  const Icon =
    kind === "info"
      ? Info16Regular
      : kind === "warning"
        ? Warning16Filled
        : ErrorCircle16Filled;
  return (
    <div
      className={`inline-notice ${kind} ${className}`.trim()}
      role={kind === "info" ? "status" : "alert"}
    >
      <Icon aria-hidden />
      <div>{children}</div>
    </div>
  );
}
