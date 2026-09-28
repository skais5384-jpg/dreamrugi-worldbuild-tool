import { BridgeFailure, GuardedClient } from "../bridge/client";
import { text } from "../strings";
import type { Id, Lane, ResultDto, RetainedRef, Work } from "../bridge/types";

export interface Completion {
  result: ResultDto;
  retained: RetainedRef | null;
}
export interface OperationNotice {
  id: Id;
  label: string;
  phase: "pending" | "reserved" | "acknowledging";
  problem: string | null;
}
interface OwnedOperation extends OperationNotice {
  input: Work;
  busy: boolean;
  completion?: Completion;
  accepted?: () => void;
  previewCurrent?: () => boolean;
  previewRefused?: boolean;
  resolve: (result: Completion) => void;
}

export function safeFailure(error: unknown): string {
  if (error instanceof BridgeFailure) {
    const code = error.boundary?.code;
    const cleanup = error.boundary?.diagnostic?.cleanupOutcome;
    if (
      ["initialization_failed", "project_not_empty"].includes(code ?? "") &&
      cleanup === "preserved_external_entries"
    )
      return text("project.createCleanupPreserved");
    if (
      ["initialization_failed", "project_not_empty"].includes(code ?? "") &&
      cleanup === "failed"
    )
      return text("project.createCleanupFailed");
    if (code === "project_not_empty") return text("project.createNotEmpty");
    if (code === "settings_read_failed")
      return text("project.defaultReadFailed");
    if (code === "settings_write_failed")
      return text("project.defaultWriteFailed");
    if (code === "snapshot_rejected") return text("backup.snapshotRejected");
    if (code === "backup_rejected") return text("backup.failed");
    if (code === "backup_delete_rejected") return text("backup.deleteRejected");
    if (code === "backup_retention_full") return text("backup.retentionFull");
    if (code === "backup_corrupt") return text("backup.corrupt");
    if (code === "restore_rejected") return text("backup.restoreFailed");
    if (code === "restore_recovery_required")
      return text("backup.recoveryRequired");
    if (code === "asset_maintenance_stale") return text("health.staleFailure");
    if (code === "owners_remain") return text("error.ownersRemain");
    if (code === "recovery_rejected") return text("error.recoveryRejected");
    if (code === "recovery_store_busy")
      return text("documentEdit.recoveryStoreBusy");
    if (code === "sink_unavailable")
      return text("documentEdit.recoveryUnavailable");
    if (code === "session_rejected")
      return text(
        error.boundary?.diagnostic?.category === "StaleLockToken"
          ? "documentEdit.staleLockToken"
          : "documentEdit.entryRejected",
      );
    if (code === "asset_maintenance_rejected")
      return text("health.inspectFailed");
    if (code === "diagnostic_export_rejected")
      return text("health.exportFailed");
    if (code === "pdf_export_rejected") return text("pdf.failed");
    if (code === "pdf_template_unavailable")
      return text("pdf.templateUnavailable");
    if (code === "composite_intent_pending")
      return text("whole.documentDeferred");
    if (code === "collaboration_read_only") return text("svn.readOnly");
    // 경계 밖 문자열은 화면에 복사하지 않는다. 오류 코드도 닫힌 목록에서 선택한다.
    if (
      code &&
      [
        "cancelled",
        "closed",
        "forbidden",
        "full",
        "unavailable",
        "owners_remain",
        "wrong_binding",
        "initialization_failed",
        "runtime_rejected",
        "repository_rejected",
        "session_rejected",
        "preparation_rejected",
        "save_rejected",
        "release_rejected",
        "recovery_rejected",
        "invalid_input",
        "unknown_id",
        "not_terminal",
        "project_not_empty",
        "settings_read_failed",
        "settings_write_failed",
        "snapshot_rejected",
        "backup_rejected",
        "backup_delete_rejected",
        "backup_retention_full",
        "backup_corrupt",
        "restore_rejected",
        "restore_recovery_required",
        "asset_maintenance_rejected",
        "asset_maintenance_stale",
        "diagnostic_export_rejected",
      ].includes(code)
    ) {
      const action = ["release_rejected", "not_terminal"].includes(code)
        ? "error.finishPending"
        : ["full", "initialization_failed", "recovery_rejected"].includes(code)
          ? "error.checkRecovery"
          : [
                "wrong_binding",
                "unknown_id",
                "repository_rejected",
                "session_rejected",
              ].includes(code)
            ? "error.checkCurrent"
            : "error.reviewRetry";
      return text(action) + " " + text("error.boundary", { code });
    }
    return text("error.transport", { category: error.category });
  }
  return text("error.uiUnavailable");
}

/** React의 수명과 분리된 단일 owner. 응답 유실은 새 ID나 자동 write 재시도를 만들지 않는다. */
export class Operations {
  private readonly owned = new Map<Id, OwnedOperation>();
  constructor(
    private readonly client: GuardedClient,
    private readonly changed: () => void,
  ) {}
  notices(): OperationNotice[] {
    return [...this.owned.values()].map(({ id, label, phase, problem }) => ({
      id,
      label,
      phase,
      problem,
    }));
  }
  async run(
    input: Work,
    label: string,
    lane: Lane = "ordinary",
    accepted?: () => void,
    allocated?: (id: Id) => void,
    previewCurrent?: () => boolean,
  ): Promise<Completion> {
    if (
      previewCurrent &&
      (input.kind !== "document_workspace" ||
        ![
          "asset_read",
          "asset_chunk",
          "replace_preview",
          "replace_page",
          "replace_apply",
        ].includes(input.request.action))
    )
      throw new BridgeFailure("protocol");
    const id = await this.client.reserve(lane);
    allocated?.(id);
    return new Promise<Completion>((resolve) => {
      const task: OwnedOperation = {
        id,
        label,
        input: structuredClone(input),
        phase: "pending",
        problem: null,
        busy: true,
        resolve,
        accepted,
        previewCurrent,
      };
      this.owned.set(id, task);
      this.changed();
      void this.submit(task);
    });
  }
  private async submit(task: OwnedOperation) {
    try {
      if (task.previewCurrent && !task.previewCurrent()) {
        await this.abandonPreview(task);
        return;
      }
      await this.client.submit(task.id, task.input);
      this.accepted(task);
    } catch (error: unknown) {
      task.problem = safeFailure(error);
      if (
        task.previewCurrent &&
        error instanceof BridgeFailure &&
        error.boundary?.code === "closed"
      )
        task.previewRefused = true;
    } finally {
      task.busy = false;
    }
    await this.query(task);
  }
  private async abandonPreview(task: OwnedOperation) {
    await this.client.abandonPreviewReservation(task.id);
    this.owned.delete(task.id);
    task.resolve({
      result: {
        kind: "rejected",
        error: { code: "cancelled", nextAction: "" },
        input_retained: false,
      },
      retained: null,
    });
    this.changed();
  }
  async queryAll() {
    await Promise.all([...this.owned.values()].map((task) => this.query(task)));
  }
  private accepted(task: OwnedOperation) {
    task.accepted?.();
    task.accepted = undefined;
  }
  async resubmit(id: Id) {
    const task = this.owned.get(id);
    if (!task || task.busy || task.phase !== "reserved") return;
    task.busy = true;
    task.phase = "pending";
    this.changed();
    await this.submit(task);
  }
  private async query(task: OwnedOperation) {
    if (task.busy || !this.owned.has(task.id)) return;
    task.busy = true;
    try {
      if (
        !task.completion &&
        task.previewCurrent &&
        (task.previewRefused || !task.previewCurrent())
      ) {
        try {
          await this.abandonPreview(task);
          return;
        } catch (error) {
          // 이미 수락됐거나 응답만 잃었다면 native가 해제를 거부한다.
          // 그 작업은 일반 terminal 조회/ack로 끝내야 한다.
          if (
            !(error instanceof BridgeFailure) ||
            error.boundary?.code !== "not_terminal"
          )
            throw error;
        }
      }
      if (!task.completion) {
        const reply = await this.client.result(task.id);
        task.phase = reply.state === "reserved" ? "reserved" : "pending";
        if (
          reply.state === "reserved" &&
          task.previewCurrent &&
          (task.previewRefused || !task.previewCurrent())
        ) {
          await this.abandonPreview(task);
          return;
        }
        if (reply.state !== "reserved") this.accepted(task);
        if (!reply.result || !["complete", "rejected"].includes(reply.state))
          return;
        task.completion = { result: reply.result, retained: reply.retained };
      }
      task.phase = "acknowledging";
      // 확정 결과를 이 owner에 먼저 보존한다. ack 응답 유실에도 같은 ID로 인수 확인한다.
      await this.client.acknowledgeTransport(task.id);
      this.owned.delete(task.id);
      task.resolve(task.completion);
    } catch (error: unknown) {
      task.problem = safeFailure(error);
    } finally {
      task.busy = false;
      this.changed();
    }
  }
}
