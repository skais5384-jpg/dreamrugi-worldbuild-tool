//! 한 프로젝트의 종료 정책. 실제 handle/P/R은 worker에, 원 종료 결과는 control owner에 남긴다.
use std::fmt;

use super::{
    recovery_handoff::SinkCapability,
    worker::{
        CleanupContext, Registration, SessionReleaseObservation, SessionReleaseRetry,
        WorkerCategory,
    },
};
use crate::data::{
    edit_session::EditSessionState,
    project_runtime::{RuntimeCloseError, RuntimeError, RuntimeSnapshot, RuntimeState},
};

/// 공개 숫자 입력이나 시간 상한이 아니다. 기본은 초기 end 이후 세션마다 추가 두 pass다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RetryPolicy {
    None,
    Once,
    #[default]
    Twice,
}
impl RetryPolicy {
    fn limit(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Once => 1,
            Self::Twice => 2,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownPhase {
    Idle,
    Draining,
    WaitingControl,
    Ending,
    Retrying,
    Blocked,
    ClosingRuntime,
    Stopping,
    Joined,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Custody {
    Empty,
    Active,
    Pending,
    Receipt,
    InProgress,
    Acknowledged,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownBlocker {
    Results,
    ReleaseBudget,
    ActiveDraft,
    CustodyDecision,
    PendingHandoff,
    Receipt,
    ValidationFailure,
    RuntimeRecovery,
    ClosedRecovery,
    RuntimeCloseFailed,
    CallerCustody,
    InitializationFailed,
    Unavailable,
}
impl fmt::Display for ShutdownBlocker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Results => "원 request ID 또는 종료 보고로 결과를 회수하세요",
            Self::ReleaseBudget => "원 진단을 확인하고 새 제한 라운드를 명시적으로 요청하세요",
            Self::ActiveDraft => {
                "편집 원본을 유지하고 명시적으로 반환하거나 가능한 보관 경로를 선택하세요"
            }
            Self::CustodyDecision => {
                "종료를 보류한 원본을 명시적으로 보관하거나 반환한 뒤 종료를 계속하세요"
            }
            Self::PendingHandoff => "실제 보관 기능을 연결해 원본을 명시적으로 인수하세요",
            Self::Receipt => "실제 인수 receipt를 명시적으로 ack하고 원 결과를 회수하세요",
            Self::ValidationFailure => "최초 재검증 실패를 원 session key로 회수하세요",
            Self::RuntimeRecovery => {
                "복구 자료를 유지하고 같은 runtime에서 명시적 recovery 결과를 확인하세요"
            }
            Self::RuntimeCloseFailed => {
                "닫기 원 오류를 보존하세요. 소비된 프로젝트 잠금은 재시도할 수 없습니다"
            }
            Self::ClosedRecovery => "복구 미완료 상태로 닫은 원 진단을 보존하세요. 다음 명시적 프로젝트 열기에서 복구를 확인해야 합니다",
            Self::CallerCustody => {
                "반환한 미저장 원본의 caller owner를 유지하세요. 앱 전체 인계는 아직 남아 있습니다"
            }
            Self::InitializationFailed => {
                "원 초기화 오류를 확인하세요. 자원 회수를 Ready 초기화 성공으로 취급하지 마세요"
            }
            Self::Unavailable => "worker 완료 여부가 미확정입니다. 원 결과와 진단을 보존하세요",
        })
    }
}
#[derive(Debug, Clone)]
pub(crate) struct SessionOwner {
    pub(crate) registration: Registration,
    pub(crate) release: SessionReleaseObservation,
    pub(crate) release_pending: bool,
    pub(crate) can_preserve_existing: bool,
    pub(crate) custody: Custody,
    pub(crate) capability: SinkCapability,
}
#[derive(Debug, Clone)]
pub(crate) struct SessionShutdownSnapshot {
    pub(crate) owner: SessionOwner,
    pub(crate) registered: bool,
    pub(crate) awaiting_custody: bool,
    pub(crate) first_failure: Option<SessionReleaseObservation>,
    pub(crate) round_passes: u8,
    pub(crate) total_passes: u64,
}
#[derive(Debug, Clone)]
pub(crate) enum CloseObservation {
    Closed(RuntimeSnapshot),
    Failed {
        snapshot: RuntimeSnapshot,
        previous: Option<RuntimeError>,
        release: RuntimeError,
    },
}
impl CloseObservation {
    pub(super) fn from_result(result: &Result<RuntimeSnapshot, Box<RuntimeCloseError>>) -> Self {
        match result {
            Ok(snapshot) => Self::Closed(*snapshot),
            Err(error) => Self::Failed {
                snapshot: error.snapshot,
                previous: error.previous_failure.clone(),
                release: error.release.clone(),
            },
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct ShutdownSnapshot {
    pub(crate) phase: ShutdownPhase,
    pub(crate) round: u64,
    pub(crate) sessions: Vec<SessionShutdownSnapshot>,
    pub(crate) blockers: Vec<ShutdownBlocker>,
    pub(crate) report_pending: bool,
    pub(crate) runtime: Option<RuntimeSnapshot>,
    pub(crate) close: Option<CloseObservation>,
    pub(crate) caller_custody: usize,
    pub(crate) stop_requested: bool,
    pub(crate) joined: bool,
    pub(crate) resources_complete: bool,
    pub(crate) normal_exit_allowed: bool,
}
impl Default for ShutdownSnapshot {
    fn default() -> Self {
        Self {
            phase: ShutdownPhase::Idle,
            round: 0,
            sessions: Vec::new(),
            blockers: Vec::new(),
            report_pending: false,
            runtime: None,
            close: None,
            caller_custody: 0,
            stop_requested: false,
            joined: false,
            resources_complete: false,
            normal_exit_allowed: false,
        }
    }
}
/// 원 Result를 이동한다. 관측/대기 취소로 소비하지 않고 별도의 take_shutdown_report만 회수한다.
#[derive(Debug)]
pub(crate) struct SessionShutdownReport {
    pub(crate) registration: Registration,
    pub(crate) end: Option<SessionReleaseRetry>,
    pub(crate) retries: Vec<SessionReleaseRetry>,
}
#[derive(Debug)]
#[must_use]
pub(crate) struct ShutdownReport {
    pub(crate) round: u64,
    pub(crate) sessions: Vec<SessionShutdownReport>,
    pub(crate) initialization_error: Option<RuntimeError>,
    pub(crate) close: Option<Result<RuntimeSnapshot, Box<RuntimeCloseError>>>,
    pub(crate) coordination_errors: Vec<WorkerCategory>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownRequestError {
    NotStarted,
    ResultsRemain,
    NoReleaseWork,
    Exhausted,
    Unavailable,
}
impl fmt::Display for ShutdownRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "종료 제어 요청을 수락하지 못했습니다 ({self:?}). 원 결과를 회수하고 현재 종료 상태를 확인하세요")
    }
}
impl std::error::Error for ShutdownRequestError {}

#[derive(Default)]
pub(super) struct ShutdownState {
    pub(super) view: ShutdownSnapshot,
    machine: Option<ShutdownMachine>,
    report: Option<ShutdownReport>,
    pub(super) pending: bool,
    pub(super) running: bool,
}
impl ShutdownState {
    pub(super) fn request(&mut self, policy: RetryPolicy) {
        if self.view.phase != ShutdownPhase::Idle {
            return;
        }
        self.view.phase = ShutdownPhase::Draining;
        self.view.round = 1;
        self.machine = Some(ShutdownMachine {
            policy,
            view: self.view.clone(),
            records: Vec::new(),
            initialized: false,
        });
        self.pending = true;
    }
    pub(super) fn notify(&mut self) {
        if !matches!(
            self.view.phase,
            ShutdownPhase::Idle
                | ShutdownPhase::Joined
                | ShutdownPhase::Unavailable
                | ShutdownPhase::Stopping
        ) && (self.machine.is_some() || self.running)
        {
            self.pending = true;
        }
    }
    pub(super) fn begin(&mut self) -> Option<ShutdownMachine> {
        if !self.pending || self.report.is_some() || self.running {
            return None;
        }
        let machine = self.machine.take()?;
        self.pending = false;
        self.running = true;
        Some(machine)
    }
    pub(super) fn waiting_control(&mut self) -> bool {
        if self.pending && self.report.is_none() && self.view.phase != ShutdownPhase::WaitingControl
        {
            self.view.phase = ShutdownPhase::WaitingControl;
            return true;
        }
        false
    }
    pub(super) fn finish(&mut self, machine: ShutdownMachine, report: Option<ShutdownReport>) {
        self.view = machine.view.clone();
        self.report = report;
        self.view.report_pending = self.report.is_some();
        if self.view.report_pending && !self.view.blockers.contains(&ShutdownBlocker::Results) {
            self.view.blockers.push(ShutdownBlocker::Results);
        }
        self.machine = Some(machine);
        self.running = false;
    }
    pub(super) fn has_report(&self) -> bool {
        self.report.is_some()
    }
    pub(super) fn take_report(&mut self) -> Option<ShutdownReport> {
        let report = self.report.take()?;
        self.view.report_pending = false;
        if self.machine.is_none() {
            self.view
                .blockers
                .retain(|b| *b != ShutdownBlocker::Results);
            self.view.resources_complete =
                self.view.joined && self.view.phase != ShutdownPhase::Unavailable;
        }
        self.notify();
        Some(report)
    }
    pub(super) fn release_round(&mut self) -> Result<(), ShutdownRequestError> {
        if self.running || (self.pending && self.view.phase == ShutdownPhase::Draining) {
            return Ok(());
        }
        if self.view.phase == ShutdownPhase::Idle {
            return Err(ShutdownRequestError::NotStarted);
        }
        if self.view.phase == ShutdownPhase::Unavailable {
            return Err(ShutdownRequestError::Unavailable);
        }
        if self.report.is_some() {
            return Err(ShutdownRequestError::ResultsRemain);
        }
        let m = self
            .machine
            .as_mut()
            .ok_or(ShutdownRequestError::NoReleaseWork)?;
        if !m.records.iter().any(|r| r.current.owner.release_pending) {
            return Err(ShutdownRequestError::NoReleaseWork);
        }
        m.view.round = m
            .view
            .round
            .checked_add(1)
            .ok_or(ShutdownRequestError::Exhausted)?;
        // 명시적 새 라운드에도 제품의 고정 상한을 적용한다. 최초 정책 0은 자동 retry만 끈다.
        m.policy = RetryPolicy::default();
        for r in &mut m.records {
            r.current.round_passes = 0;
        }
        self.view.round = m.view.round;
        self.view.phase = ShutdownPhase::Draining;
        self.pending = true;
        Ok(())
    }
    pub(super) fn unavailable(&mut self) {
        self.view.phase = ShutdownPhase::Unavailable;
        self.view.normal_exit_allowed = false;
        self.view.resources_complete = false;
        if !self.view.blockers.contains(&ShutdownBlocker::Unavailable) {
            self.view.blockers.push(ShutdownBlocker::Unavailable);
        }
        self.pending = false;
    }
    pub(super) fn absent_runtime(&mut self, error: Option<RuntimeError>) {
        self.report = Some(ShutdownReport {
            round: 1,
            sessions: Vec::new(),
            initialization_error: error,
            close: None,
            coordination_errors: Vec::new(),
        });
        self.view.report_pending = true;
        self.view.blockers = vec![
            ShutdownBlocker::InitializationFailed,
            ShutdownBlocker::Results,
        ];
        self.view.phase = ShutdownPhase::Blocked;
        self.pending = false;
        self.machine = None;
    }
    pub(super) fn mark_stop(&mut self) {
        self.view.stop_requested = true;
        self.view.phase = ShutdownPhase::Stopping;
    }
    pub(super) fn joined(&mut self, available: bool) {
        if self.view.phase == ShutdownPhase::Idle {
            return;
        }
        self.view.joined = true;
        if !available {
            self.unavailable();
            return;
        }
        self.view.phase = ShutdownPhase::Joined;
        self.view.resources_complete = !self.view.report_pending;
        self.view.normal_exit_allowed = self.view.blockers.is_empty()
            && !self.view.report_pending
            && matches!(self.view.close, Some(CloseObservation::Closed(s)) if s.state == RuntimeState::Ready);
    }
}

struct Record {
    current: SessionShutdownSnapshot,
    end_done: bool,
}
pub(super) struct ShutdownMachine {
    policy: RetryPolicy,
    view: ShutdownSnapshot,
    records: Vec<Record>,
    initialized: bool,
}
pub(super) struct DrainObservation {
    pub(super) ordinary_results: usize,
    pub(super) validation_failures: usize,
    pub(super) initialization_error: Option<RuntimeError>,
}
impl ShutdownMachine {
    fn refresh<P, R>(&mut self, ctx: &mut CleanupContext<'_, P, R>) {
        let owners = ctx.shutdown_sessions();
        for record in &mut self.records {
            record.current.registered = owners
                .iter()
                .any(|owner| owner.registration == record.current.owner.registration);
            if !record.current.registered {
                // 명시적 cleanup에서 제거할 때도 ReadOnly/custody 방어를 통과해야 한다.
                // 마지막 원 관측은 유지하되 이미 없는 등록을 retry 대상으로 만들지 않는다.
                record.current.owner.release_pending = false;
            }
        }
        for owner in owners {
            if let Some(record) = self
                .records
                .iter_mut()
                .find(|r| r.current.owner.registration == owner.registration)
            {
                if record.current.first_failure.is_none()
                    && (owner.release.release.is_some() || owner.release.acquire_error.is_some())
                {
                    record.current.first_failure = Some(owner.release.clone());
                }
                record.current.owner = owner;
            } else {
                let first_failure = (owner.release.release.is_some()
                    || owner.release.acquire_error.is_some())
                .then(|| owner.release.clone());
                self.records.push(Record {
                    current: SessionShutdownSnapshot {
                        owner,
                        registered: true,
                        awaiting_custody: false,
                        first_failure,
                        round_passes: 0,
                        total_passes: 0,
                    },
                    end_done: false,
                });
            }
        }
        self.view.sessions = self.records.iter().map(|r| r.current.clone()).collect();
        self.view.runtime = ctx.shutdown_runtime();
        self.view.close = ctx.runtime_close_observation();
        self.view.caller_custody = ctx.caller_custody();
    }
    pub(super) fn execute<P, R>(
        mut self,
        ctx: &mut CleanupContext<'_, P, R>,
        drain: DrainObservation,
        mut publish: impl FnMut(ShutdownSnapshot),
    ) -> (Self, Option<ShutdownReport>) {
        self.refresh(ctx);
        self.view.report_pending = false;
        let mut report = ShutdownReport {
            round: self.view.round,
            sessions: Vec::new(),
            initialization_error: None,
            close: None,
            coordination_errors: Vec::new(),
        };
        if !self.initialized {
            report.initialization_error = drain.initialization_error;
            self.initialized = true;
        }
        // 보관 결정을 기다리는 등록을 제외한 최초 end부터 진행한다. 다른 등록은 계속 정리한다.
        for i in 0..self.records.len() {
            let r = &mut self.records[i];
            if r.end_done {
                continue;
            }
            if r.current.owner.release_pending
                && r.current.owner.custody == Custody::Active
                && (r.current.awaiting_custody || r.current.owner.can_preserve_existing)
            {
                // 실패한 preserve나 runtime recovery만으로 원본 처리 결정이 완료되지는 않는다.
                r.current.awaiting_custody = true;
                continue;
            }
            r.current.awaiting_custody = false;
            r.end_done = true;
            if !r.current.owner.release_pending
                || r.current.owner.release.snapshot.state() == EditSessionState::ReleaseFailed
            {
                continue;
            }
            let registration = r.current.owner.registration.clone();
            self.view.phase = ShutdownPhase::Ending;
            publish(self.view.clone());
            match ctx.end_session_observed(&registration) {
                Ok(end) => report.sessions.push(SessionShutdownReport {
                    registration,
                    end: Some(end),
                    retries: Vec::new(),
                }),
                Err(error) => report.coordination_errors.push(error),
            }
            self.refresh(ctx);
        }
        // pass를 바깥 반복으로 두어 남은 각 세션에 한 번씩 기회를 준다.
        for _ in 0..self.policy.limit() {
            for i in 0..self.records.len() {
                let r = &mut self.records[i];
                if !r.current.owner.release_pending
                    || r.current.owner.release.snapshot.state() != EditSessionState::ReleaseFailed
                    || r.current.round_passes >= self.policy.limit()
                {
                    continue;
                }
                r.current.round_passes += 1;
                r.current.total_passes = r.current.total_passes.saturating_add(1);
                let registration = r.current.owner.registration.clone();
                self.view.sessions = self.records.iter().map(|r| r.current.clone()).collect();
                self.view.phase = ShutdownPhase::Retrying;
                publish(self.view.clone());
                match ctx.retry_session_release(&registration) {
                    Ok(retry) => {
                        if let Some(entry) = report
                            .sessions
                            .iter_mut()
                            .find(|r| r.registration == registration)
                        {
                            entry.retries.push(retry);
                        } else {
                            report.sessions.push(SessionShutdownReport {
                                registration,
                                end: None,
                                retries: vec![retry],
                            });
                        }
                    }
                    Err(error) => report.coordination_errors.push(error),
                }
                self.refresh(ctx);
            }
        }
        self.view.blockers.clear();
        for owner in ctx.shutdown_sessions() {
            if self.records.iter().any(|r| {
                r.current.owner.registration == owner.registration && r.current.awaiting_custody
            }) {
                self.view.blockers.push(ShutdownBlocker::CustodyDecision);
            } else if owner.release_pending {
                self.view.blockers.push(ShutdownBlocker::ReleaseBudget);
            }
            match owner.custody {
                Custody::Active => self.view.blockers.push(ShutdownBlocker::ActiveDraft),
                Custody::Pending | Custody::InProgress => {
                    self.view.blockers.push(ShutdownBlocker::PendingHandoff)
                }
                Custody::Receipt => self.view.blockers.push(ShutdownBlocker::Receipt),
                Custody::Empty | Custody::Acknowledged if !owner.release_pending => {
                    if let Err(error) = ctx.remove_session(&owner.registration) {
                        self.view.blockers.push(ShutdownBlocker::ValidationFailure);
                        report.coordination_errors.push(error);
                    }
                }
                _ => {}
            }
        }
        if drain.ordinary_results != 0 {
            self.view.blockers.push(ShutdownBlocker::Results);
        }
        if drain.validation_failures != 0 {
            self.view.blockers.push(ShutdownBlocker::ValidationFailure);
        }
        if self
            .view
            .runtime
            .is_some_and(|r| r.state != RuntimeState::Ready)
        {
            self.view.blockers.push(ShutdownBlocker::RuntimeRecovery);
        }
        // 원 종료 결과를 먼저 회수한다. caller custody는 자원 회수와 앱 종료를 구분한다.
        if !report.sessions.is_empty()
            || report.initialization_error.is_some()
            || !report.coordination_errors.is_empty()
        {
            self.view.blockers.push(ShutdownBlocker::Results);
        }
        let can_close = self.view.blockers.is_empty() && ctx.shutdown_sessions().is_empty();
        if can_close && self.view.runtime.is_some() {
            self.view.phase = ShutdownPhase::ClosingRuntime;
            publish(self.view.clone());
            match ctx.close_runtime() {
                Ok(original) => report.close = Some(original),
                Err(error) => report.coordination_errors.push(error),
            }
        }
        self.refresh(ctx);
        if self.view.caller_custody != 0 {
            self.view.blockers.push(ShutdownBlocker::CallerCustody);
        }
        if matches!(self.view.close, Some(CloseObservation::Failed { .. })) {
            self.view.blockers.push(ShutdownBlocker::RuntimeCloseFailed);
        }
        if matches!(self.view.close, Some(CloseObservation::Closed(s)) if s.state != RuntimeState::Ready)
        {
            self.view.blockers.push(ShutdownBlocker::ClosedRecovery);
        }
        self.view.blockers.dedup();
        self.view.phase = ShutdownPhase::Blocked;
        let has_report = report.initialization_error.is_some()
            || report.close.is_some()
            || !report.sessions.is_empty()
            || !report.coordination_errors.is_empty();
        (self, has_report.then_some(report))
    }
}
