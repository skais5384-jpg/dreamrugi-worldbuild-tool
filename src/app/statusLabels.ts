import type { Response } from "../bridge/types";
import { text } from "../strings";

export function lifecycleLabel(lifecycle: string): string {
  return text(
    lifecycle === "Active"
      ? "template.active"
      : lifecycle === "Deleted"
        ? "template.readonly"
        : "template.unknown",
  );
}
export function projectLabel(
  project: Extract<Response, { kind: "project" }>,
  closing: boolean,
): string {
  // Stopped라도 초기화 실패가 있으면 정상 종료 문구로 가리지 않는다. 제어 판단에는 사용하지 않는다.
  if (
    project.error?.code === "initialization_failed" ||
    project.shutdown.reports.some((r) => r.initializationFailed)
  )
    return text("project.initializationFailed");
  if (project.status === "Stopped") return text("project.stopped");
  if (closing || project.status === "Closing") return text("project.closing");
  if (project.status === "Ready" && project.runtime === "Ready")
    return text("project.ready");
  if (!project.runtime) return text("project.initialization");
  return text("project.unavailable");
}
