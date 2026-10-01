import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { Button } from "../ui/Controls";
import { NoticeEventDetail, useNoticeHistory } from "../ui/FloatingNotice";
import "./ActivityLog.css";

export interface ActivityEvent {
  sessionId: string;
  observedAtUtc: string | null;
  feature: string;
  stage: string;
  category: string;
  outcome: string;
  correlation: string | null;
  projectFingerprint: string | null;
  summary?: string;
  detail?: string;
  noticeId?: string;
  noticeKey?: string;
}
interface RecentEvents {
  events: ActivityEvent[];
  droppedEvents: number;
}

function isAttention(event: ActivityEvent) {
  return /fail|error|uncertain|abnormal|partial|warning/i.test(
    `${event.outcome} ${event.category}`,
  );
}

function eventKey(event: ActivityEvent) {
  return `${event.sessionId}:${event.observedAtUtc}:${event.feature}:${event.stage}:${event.outcome}`;
}

export function useActivityLog(enabled: boolean) {
  const noticeHistory = useNoticeHistory();
  const [snapshot, setSnapshot] = useState<RecentEvents>({
    events: [],
    droppedEvents: 0,
  });
  const [readThrough, setReadThrough] = useState<string | null>(null);
  const [localEvents, setLocalEvents] = useState<ActivityEvent[]>([]);
  const localSequence = useRef(0);
  const record = useCallback(
    (
      event: Omit<
        ActivityEvent,
        "sessionId" | "observedAtUtc" | "correlation" | "projectFingerprint"
      >,
    ) => {
      setLocalEvents((events) => [
        ...events.slice(-49),
        {
          ...event,
          sessionId: `ui-${++localSequence.current}`,
          observedAtUtc: new Date().toISOString(),
          correlation: null,
          projectFingerprint: null,
        },
      ]);
    },
    [],
  );
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const refresh = () => {
      try {
        void invoke<RecentEvents>("diagnostic_recent_events").then(
          (value) => {
            if (active) setSnapshot(value);
          },
          () => {},
        );
      } catch {
        // Browser-only tests have no Tauri transport.
      }
    };
    refresh();
    const timer = window.setInterval(refresh, 5000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [enabled]);
  const events = [...snapshot.events, ...localEvents, ...noticeHistory].sort(
    (a, b) => (a.observedAtUtc ?? "").localeCompare(b.observedAtUtc ?? ""),
  );
  const latestAttention = [...events].reverse().find(isAttention);
  const latestAttentionKey = latestAttention ? eventKey(latestAttention) : null;
  const attention = !!latestAttentionKey && latestAttentionKey !== readThrough;
  return {
    ...snapshot,
    events,
    attention,
    markRead: () => setReadThrough(latestAttentionKey),
    record,
  };
}

function eventLevel(event: ActivityEvent) {
  return isAttention(event) ? "주의" : "정보";
}

function eventSummary(event: ActivityEvent) {
  if (event.summary) return event.summary;
  if (event.outcome === "started") return "작업을 시작했습니다";
  if (/success|completed|done|ok/i.test(event.outcome))
    return "작업을 마쳤습니다";
  if (isAttention(event)) return "작업 결과를 확인해 주세요";
  return "상태를 기록했습니다";
}

const featureLabels: Record<string, string> = {
  project: "프로젝트",
  project_backup: "프로젝트 백업",
  project_restore: "프로젝트 복원",
  recovery: "복구",
  diagnostics: "진단",
  asset_maintenance: "자료 관리",
  editor: "문서 / 템플릿 저장",
  svn: "SVN 협업",
  updater: "앱 업데이트",
  알림: "알림",
  안내: "안내",
  프로젝트: "프로젝트",
};

export function ActivityLog({
  events,
  droppedEvents,
  close,
  noticeKey,
}: RecentEvents & { close: () => void; noticeKey?: string }) {
  const previousDialog = useRef(
    Array.from(
      document.querySelectorAll<HTMLElement>(".fui-DialogSurface"),
    ).pop(),
  );
  const previousFocus = useRef(document.activeElement);
  useEffect(
    () => () => {
      // A notice trigger is remounted when the floating stack changes portals.
      // If it disappeared, Fluent has no connected trigger to reactivate the
      // underlying modal. Restore only that still-current owned surface.
      requestAnimationFrame(() => {
        const previous = previousDialog.current;
        const current = Array.from(
          document.querySelectorAll<HTMLElement>(".fui-DialogSurface"),
        ).pop();
        if (current && current !== previous) return;
        const focused = previousFocus.current;
        if (
          focused instanceof HTMLElement &&
          focused.isConnected &&
          focused !== document.body
        ) {
          focused.focus({ preventScroll: true });
        } else if (previous?.isConnected) {
          previous.focus({ preventScroll: true });
        } else {
          // The lower-left trigger can move portals while the log opens.
          // Return keyboard control to a current app button when it vanished.
          document
            .querySelector<HTMLButtonElement>(
              ".app-shell button:not(:disabled)",
            )
            ?.focus({ preventScroll: true });
        }
      });
    },
    [],
  );
  return (
    <Dialog
      open
      modalType="modal"
      onOpenChange={(_, data) => {
        if (!data.open) close();
      }}
    >
      <DialogSurface className="activity-log-dialog">
        <DialogBody className="activity-log-body">
          <DialogTitle>실행 기록</DialogTitle>
          <DialogContent className="activity-log-content">
            <section className="activity-log" aria-label="실행 기록">
              <p>작업 결과와 안내를 시간순으로 확인할 수 있습니다.</p>
              {droppedEvents > 0 && (
                <p role="status">
                  오래된 기록 {droppedEvents}건은 보관 범위를 넘어 목록에서
                  제외됐습니다.
                </p>
              )}
              <div className="activity-log-table-wrap">
                <table>
                  <colgroup>
                    <col className="activity-log-time-column" />
                    <col className="activity-log-level-column" />
                    <col className="activity-log-work-column" />
                    <col />
                  </colgroup>
                  <thead>
                    <tr>
                      <th scope="col">시간</th>
                      <th scope="col">수준</th>
                      <th scope="col">작업</th>
                      <th scope="col">요약</th>
                    </tr>
                  </thead>
                  <tbody>
                    {[...events].reverse().map((event, index) => (
                      <tr key={`${eventKey(event)}:${index}`}>
                        <td>
                          <time>
                            {event.observedAtUtc
                              ? new Date(event.observedAtUtc).toLocaleString()
                              : "시간 확인 필요"}
                          </time>
                        </td>
                        <td>{eventLevel(event)}</td>
                        <td>{featureLabels[event.feature] ?? "기타 작업"}</td>
                        <td>
                          <details
                            open={
                              (!!noticeKey && event.noticeKey === noticeKey) ||
                              undefined
                            }
                          >
                            <summary>{eventSummary(event)}</summary>
                            <dl>
                              <dt>작업 코드</dt>
                              <dd>{event.feature}</dd>
                              <dt>단계</dt>
                              <dd>{event.stage}</dd>
                              <dt>결과</dt>
                              <dd>{event.outcome}</dd>
                              <dt>분류</dt>
                              <dd>{event.category}</dd>
                              {event.correlation && (
                                <>
                                  <dt>연결</dt>
                                  <dd>{event.correlation}</dd>
                                </>
                              )}
                              {event.projectFingerprint && (
                                <>
                                  <dt>프로젝트 식별</dt>
                                  <dd>
                                    {event.projectFingerprint.slice(0, 12)}
                                  </dd>
                                </>
                              )}
                              {event.detail && (
                                <>
                                  <dt>안내</dt>
                                  <dd>
                                    <NoticeEventDetail event={event} />
                                  </dd>
                                </>
                              )}
                            </dl>
                          </details>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {!events.length && <p>아직 표시할 기록이 없습니다.</p>}
            </section>
          </DialogContent>
          <DialogActions>
            <Button type="button" onClick={close}>
              닫기
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
