//! 프로젝트당 한 blocking thread. 이 내부 seam을 frontend command 목록으로 노출하지 않는다.
#[cfg(all(test, windows))]
pub(crate) mod bridge_test_support;
mod context;
pub(crate) use context::{
    CleanupContext, CompositeDispatchError, SessionRegistration, SessionReleaseObservation,
    SessionReleaseRetry, WorkerContext,
};

use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    fmt,
    marker::PhantomData,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::{
    recovery_handoff::{CustodyTransition, HandoffError},
    scheduler::{
        IntervalError, Periodic, Phase, Reasons, Reservation, TriggerAccepted, TriggerReason,
    },
    shutdown::{
        DrainObservation, RetryPolicy, ShutdownMachine, ShutdownPhase, ShutdownReport,
        ShutdownRequestError, ShutdownSnapshot, ShutdownState,
    },
};
use crate::data::{
    collaboration_lock::LockSessionId,
    edit_session::{EditSessionError, EditSessionState},
    project_runtime::{ProjectRuntime, RuntimeError, RuntimeSnapshot},
};

/// Arc의 수명이 옛 key와 함께 유지되어 주소 재사용도 다른 worker를 같은 세대로 만들지 않는다.
#[derive(Clone)]
pub(crate) struct Registration {
    worker: Arc<()>,
    serial: u64,
}
impl PartialEq for Registration {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.worker, &other.worker) && self.serial == other.serial
    }
}
impl Eq for Registration {}
impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Registration([redacted])")
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionKey {
    registration: Registration,
    project: String,
    session: LockSessionId,
}
impl fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionKey([redacted])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerCategory {
    Starting,
    InitializationFailed,
    Full,
    Closed,
    Unavailable,
    Stale,
    Inactive,
    Exhausted,
    OwnersRemain,
    MustCloseAdmission,
    ResultType,
    UnknownRequest,
}
impl fmt::Display for WorkerCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "worker 요청을 완료하지 못했습니다 ({self:?}). 원본과 제어 owner를 유지하고 상태를 확인하세요")
    }
}
impl std::error::Error for WorkerCategory {}

/// 거부는 아직 실행하지 않은 실제 입력을 반환한다. 함수 포인터에는 숨은 owned capture가 없다.
pub(crate) struct Rejected<I> {
    pub(crate) input: I,
    pub(crate) category: WorkerCategory,
}
impl<I> fmt::Debug for Rejected<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rejected")
            .field("category", &self.category)
            .finish_non_exhaustive()
    }
}

pub(crate) struct WorkerConfig {
    pub(crate) project_root: PathBuf,
    pub(crate) lock_root: PathBuf,
    pub(crate) initialize_empty: bool,
    pub(crate) create_directory: bool,
    /// 일반 요청은 실행 중/미회수 결과까지 이 상한을 점유한다.
    pub(crate) capacity: usize,
    /// 각 등록은 별도로 예약 한 칸, running follow-up 한 칸, 최초 실패 한 칸을 보장받는다.
    pub(crate) max_sessions: usize,
    pub(crate) interval: Duration,
    /// Collaborative SVN locks are checked by the edit/save operation, never
    /// by an idle timer. The generic worker keeps its periodic behavior.
    pub(crate) automatic_revalidation: bool,
}
#[derive(Debug)]
pub(crate) enum StartError {
    InvalidCapacity,
    Interval(IntervalError),
    Spawn(SpawnFailure),
}
pub(crate) struct SpawnFailure(std::io::Error);
impl fmt::Debug for SpawnFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SpawnFailure").field(&self.0.kind()).finish()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerStatus {
    Starting,
    Ready,
    InitializationFailed,
    Unavailable,
    Stopped,
}
#[derive(Debug, Clone)]
pub(crate) struct WorkerSnapshot {
    pub(crate) status: WorkerStatus,
    pub(crate) admission_closed: bool,
    pub(crate) ordinary_outstanding: usize,
    pub(crate) queued: usize,
    pub(crate) running: bool,
    pub(crate) sessions: usize,
    pub(crate) runtime: Option<RuntimeSnapshot>,
    pub(crate) initialization_error: Option<RuntimeError>,
    pub(crate) timer_error: Option<IntervalError>,
    pub(crate) unclaimed_validation_failures: usize,
    pub(crate) thread: Option<thread::ThreadId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationCompletion {
    Validated,
    Failed,
    Stale,
    Inactive,
}
#[derive(Debug)]
pub(crate) struct RevalidationFailure {
    pub(crate) key: SessionKey,
    pub(crate) reasons: Reasons,
    pub(crate) original: EditSessionError,
    pub(crate) preserve: Option<Result<CustodyTransition, HandoffError>>,
}
#[derive(Clone)]
pub(crate) struct SessionObservation {
    pub(crate) snapshot: crate::data::edit_session::EditSessionSnapshot,
    pub(crate) active_dirty: bool,
    pub(crate) capability: super::recovery_handoff::SinkCapability,
}
#[derive(Debug, Clone)]
pub(crate) struct RevalidationSnapshot {
    pub(crate) key: SessionKey,
    pub(crate) phase: Phase,
    pub(crate) last: Option<ValidationCompletion>,
    pub(crate) completed: u64,
    pub(crate) eligible: bool,
}
struct Scheduled {
    key: SessionKey,
    eligible: bool,
    reservation: Reservation,
    last: Option<ValidationCompletion>,
    completed: u64,
    failure: Option<RevalidationFailure>,
}

/// Ticket는 대기 구독일 뿐 결과 owner가 아니다. drop해도 request id로 다시 회수할 수 있다.
pub(crate) struct Ticket<O> {
    id: RequestId,
    output: PhantomData<fn() -> O>,
}
impl<O> Ticket<O> {
    pub(crate) fn id(&self) -> RequestId {
        self.id.clone()
    }
}
impl<O> fmt::Debug for Ticket<O> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ticket([redacted])")
    }
}
#[derive(Clone)]
pub(crate) struct RequestId {
    worker: Arc<()>,
    serial: u64,
    control: bool,
}
enum Stored {
    Pending,
    Complete(Box<dyn Any + Send>),
}
trait Job<P, R>: Send {
    fn run(self: Box<Self>, context: &mut WorkerContext<P, R>) -> Box<dyn Any + Send>;
}
struct OrdinaryJob<I, O, P, R> {
    input: I,
    run: fn(&mut WorkerContext<P, R>, I) -> O,
}
impl<I: Send + 'static, O: Send + 'static, P, R> Job<P, R> for OrdinaryJob<I, O, P, R> {
    fn run(self: Box<Self>, context: &mut WorkerContext<P, R>) -> Box<dyn Any + Send> {
        Box::new((self.run)(context, self.input))
    }
}
struct CleanupJob<I, O, P, R> {
    input: I,
    run: fn(&mut CleanupContext<'_, P, R>, I) -> O,
}
impl<I: Send + 'static, O: Send + 'static, P, R> Job<P, R> for CleanupJob<I, O, P, R> {
    fn run(self: Box<Self>, context: &mut WorkerContext<P, R>) -> Box<dyn Any + Send> {
        Box::new((self.run)(&mut CleanupContext(context), self.input))
    }
}
enum Queued<P, R> {
    Job(u64, Box<dyn Job<P, R>>),
    Revalidate(SessionKey),
    Shutdown(Box<ShutdownMachine>),
}
struct Mailbox<P, R> {
    status: WorkerStatus,
    closed: bool,
    stop: bool,
    running: bool,
    next_id: u64,
    capacity: usize,
    queue: VecDeque<Queued<P, R>>,
    results: BTreeMap<u64, Stored>,
    control: Option<(u64, Box<dyn Job<P, R>>)>,
    control_result: Option<(u64, Stored)>,
    scheduled: Vec<Scheduled>,
    runtime: Option<RuntimeSnapshot>,
    runtime_closed: bool,
    sessions: usize,
    session_observations: Vec<(Registration, SessionObservation)>,
    initialization_error: Option<RuntimeError>,
    timer_error: Option<IntervalError>,
    thread: Option<thread::ThreadId>,
    shutdown: ShutdownState,
}
struct Shared<P, R> {
    marker: Arc<()>,
    mailbox: Mutex<Mailbox<P, R>>,
    wake: Condvar,
    app_wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    clock: Mutex<Option<Instant>>,
}
impl<P, R> Shared<P, R> {
    fn notify_app(&self) {
        let callback = self
            .app_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }
    fn lock(&self) -> MutexGuard<'_, Mailbox<P, R>> {
        self.mailbox.lock().unwrap_or_else(|poison| {
            let mut mailbox = poison.into_inner();
            mailbox.status = WorkerStatus::Unavailable;
            mailbox.shutdown.unavailable();
            mailbox.closed = true;
            mailbox
        })
    }
    fn now(&self) -> Instant {
        #[cfg(test)]
        if let Some(now) = *self.clock.lock().expect("test clock lock") {
            return now;
        }
        Instant::now()
    }
}

pub(crate) struct WorkerClient<P, R> {
    shared: Arc<Shared<P, R>>,
}
impl<P, R> Clone for WorkerClient<P, R> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
/// G12/app owner가 보관한다. client/ticket의 수명과 독립적이며 명시적 정리와 try_join까지 유지한다.
#[must_use = "worker 제어 owner를 보관하고 명시적 정리 뒤 try_join으로 thread를 회수하세요"]
pub(crate) struct WorkerControl<P, R> {
    shared: Arc<Shared<P, R>>,
    thread: Option<JoinHandle<()>>,
}

fn ready<P, R>(mailbox: &Mailbox<P, R>) -> Result<(), WorkerCategory> {
    match mailbox.status {
        WorkerStatus::Starting => Err(WorkerCategory::Starting),
        WorkerStatus::InitializationFailed => Err(WorkerCategory::InitializationFailed),
        WorkerStatus::Unavailable | WorkerStatus::Stopped => Err(WorkerCategory::Unavailable),
        WorkerStatus::Ready => Ok(()),
    }
}
fn next_id<P, R>(m: &mut Mailbox<P, R>) -> Result<u64, WorkerCategory> {
    m.next_id = m.next_id.checked_add(1).ok_or(WorkerCategory::Exhausted)?;
    Ok(m.next_id)
}
impl<P: 'static, R: 'static> WorkerClient<P, R> {
    pub(crate) fn try_submit<I: Send + 'static, O: Send + 'static>(
        &self,
        input: I,
        run: fn(&mut WorkerContext<P, R>, I) -> O,
    ) -> Result<Ticket<O>, Rejected<I>> {
        let mut m = self.shared.lock();
        let admission = ready(&m).and_then(|()| {
            if m.closed {
                Err(WorkerCategory::Closed)
            } else if m.results.len() == m.capacity {
                Err(WorkerCategory::Full)
            } else {
                next_id(&mut m)
            }
        });
        let id = match admission {
            Ok(id) => id,
            Err(category) => return Err(Rejected { input, category }),
        };
        m.results.insert(id, Stored::Pending);
        m.queue
            .push_back(Queued::Job(id, Box::new(OrdinaryJob { input, run })));
        self.shared.wake.notify_one();
        Ok(Ticket {
            id: RequestId {
                worker: self.shared.marker.clone(),
                serial: id,
                control: false,
            },
            output: PhantomData,
        })
    }
    pub(crate) fn trigger(
        &self,
        key: &SessionKey,
        reason: TriggerReason,
    ) -> Result<TriggerAccepted, WorkerCategory> {
        let mut m = self.shared.lock();
        ready(&m)?;
        if m.closed {
            return Err(WorkerCategory::Closed);
        }
        let entry = m
            .scheduled
            .iter_mut()
            .find(|s| s.key == *key)
            .ok_or(WorkerCategory::Stale)?;
        if !entry.eligible {
            return Err(WorkerCategory::Inactive);
        }
        let accepted = entry.reservation.trigger(reason);
        if accepted == TriggerAccepted::Queued {
            m.queue.push_back(Queued::Revalidate(key.clone()));
        }
        self.shared.wake.notify_one();
        Ok(accepted)
    }
    pub(crate) fn try_take<O: Send + 'static>(
        &self,
        ticket: &Ticket<O>,
    ) -> Result<Option<O>, WorkerCategory> {
        take(&self.shared, &ticket.id)
    }
}
fn take<P, R, O: Send + 'static>(
    shared: &Shared<P, R>,
    id: &RequestId,
) -> Result<Option<O>, WorkerCategory> {
    if !Arc::ptr_eq(&shared.marker, &id.worker) {
        return Err(WorkerCategory::UnknownRequest);
    }
    let value = {
        let mut m = shared.lock();
        let stored = if id.control {
            m.control_result
                .as_ref()
                .filter(|(n, _)| *n == id.serial)
                .map(|(_, v)| v)
        } else {
            m.results.get(&id.serial)
        }
        .ok_or(WorkerCategory::UnknownRequest)?;
        match stored {
            Stored::Pending => {
                if m.status == WorkerStatus::Unavailable {
                    return Err(WorkerCategory::Unavailable);
                }
                return Ok(None);
            }
            Stored::Complete(value) if !value.is::<O>() => return Err(WorkerCategory::ResultType),
            Stored::Complete(_) => {}
        }
        let value = if id.control {
            m.control_result.take().map(|(_, v)| v)
        } else {
            m.results.remove(&id.serial)
        };
        m.shutdown.notify();
        shared.wake.notify_one();
        value
    };
    match value {
        Some(Stored::Complete(value)) => value
            .downcast::<O>()
            .map(|o| Some(*o))
            .map_err(|_| WorkerCategory::ResultType),
        _ => Err(WorkerCategory::UnknownRequest),
    }
}
impl<P: 'static, R: 'static> WorkerControl<P, R> {
    pub(crate) fn start(config: WorkerConfig) -> Result<(Self, WorkerClient<P, R>), StartError> {
        if config.capacity == 0 || config.max_sessions == 0 {
            return Err(StartError::InvalidCapacity);
        }
        let periodic =
            Periodic::new(Instant::now(), config.interval).map_err(StartError::Interval)?;
        let shared = Arc::new(Shared {
            marker: Arc::new(()),
            wake: Condvar::new(),
            app_wake: Mutex::new(None),
            #[cfg(test)]
            clock: Mutex::new(None),
            mailbox: Mutex::new(Mailbox {
                status: WorkerStatus::Starting,
                closed: false,
                stop: false,
                running: false,
                next_id: 0,
                capacity: config.capacity,
                queue: VecDeque::new(),
                results: BTreeMap::new(),
                control: None,
                control_result: None,
                scheduled: Vec::new(),
                runtime: None,
                runtime_closed: false,
                sessions: 0,
                session_observations: Vec::new(),
                initialization_error: None,
                timer_error: None,
                thread: None,
                shutdown: ShutdownState::default(),
            }),
        });
        let owner = shared.clone();
        // 초기화에는 다시 구성할 수 있는 경로와 설정만 넘긴다. 유일한 P/R은 초기화 뒤 입력으로 받는다.
        let thread = thread::Builder::new()
            .name("project-worker".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_worker(&owner, config, periodic)
                }));
                if result.is_err() {
                    let mut m = owner.lock();
                    m.status = WorkerStatus::Unavailable;
                    m.shutdown.unavailable();
                    m.closed = true;
                    // panic payload를 로그에 넣거나 저장 실패로 번역하지 않는다. 미회수 결과는 그대로 둔다.
                    owner.wake.notify_all();
                }
                owner.notify_app();
            })
            .map_err(|e| StartError::Spawn(SpawnFailure(e)))?;
        Ok((
            Self {
                shared: shared.clone(),
                thread: Some(thread),
            },
            WorkerClient { shared },
        ))
    }
    pub(crate) fn snapshot(&self) -> WorkerSnapshot {
        let mut m = self.shared.lock();
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished)
            && matches!(m.status, WorkerStatus::Starting | WorkerStatus::Ready)
        {
            m.status = WorkerStatus::Unavailable;
            m.shutdown.unavailable();
            m.closed = true;
        }
        WorkerSnapshot {
            status: m.status,
            admission_closed: m.closed,
            ordinary_outstanding: m.results.len(),
            queued: m.queue.len(),
            running: m.running,
            sessions: m.sessions,
            runtime: m.runtime,
            initialization_error: m.initialization_error.clone(),
            timer_error: m.timer_error,
            unclaimed_validation_failures: m
                .scheduled
                .iter()
                .filter(|s| s.failure.is_some())
                .count(),
            thread: m.thread,
        }
    }
    /// 등록된 app callback은 이벤트 루프 깨우기만 한다. worker lock 밖에서 호출한다.
    pub(crate) fn set_app_wake(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self
            .shared
            .app_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(callback);
    }
    pub(crate) fn clear_app_wake(&self) {
        self.shared
            .app_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
    pub(crate) fn close_admission(&self) {
        let mut m = self.shared.lock();
        m.closed = true;
        self.shared.wake.notify_one();
    }
    /// worker가 실제 업무 뒤 공개한 불변 관측이다. 상태 조회가 source/권한을 갱신하지 않는다.
    pub(crate) fn session_observation(
        &self,
        registration: &Registration,
    ) -> Option<SessionObservation> {
        self.shared
            .lock()
            .session_observations
            .iter()
            .find(|(r, _)| r == registration)
            .map(|(_, s)| s.clone())
    }
    /// 수락과 일반 admission 종료는 같은 mailbox lock에서 완료한다. 여기서 I/O를 하지 않는다.
    pub(crate) fn request_shutdown(&self) -> ShutdownSnapshot {
        self.request_shutdown_with_policy(RetryPolicy::default())
    }
    pub(in crate::data::application) fn request_shutdown_with_policy(
        &self,
        policy: RetryPolicy,
    ) -> ShutdownSnapshot {
        let mut m = self.shared.lock();
        let first = m.shutdown.view.phase == ShutdownPhase::Idle;
        m.closed = true;
        m.shutdown.request(policy);
        if m.status == WorkerStatus::Unavailable
            || (first && (m.stop || m.status == WorkerStatus::Stopped))
        {
            m.shutdown.unavailable();
        } else if first
            && m.status == WorkerStatus::InitializationFailed
            && m.runtime.is_none()
            && !m.runtime_closed
        {
            let error = m.initialization_error.clone();
            m.shutdown.absent_runtime(error);
        }
        self.shared.wake.notify_one();
        m.shutdown.view.clone()
    }
    pub(crate) fn shutdown_snapshot(&self) -> ShutdownSnapshot {
        self.shared.lock().shutdown.view.clone()
    }
    pub(crate) fn take_shutdown_report(&self) -> Option<ShutdownReport> {
        let mut m = self.shared.lock();
        let report = m.shutdown.take_report();
        self.shared.wake.notify_one();
        report
    }
    pub(crate) fn request_release_round(&self) -> Result<ShutdownSnapshot, ShutdownRequestError> {
        let mut m = self.shared.lock();
        m.shutdown.release_round()?;
        self.shared.wake.notify_one();
        Ok(m.shutdown.view.clone())
    }
    /// ticket이 사라져도 점유된 cleanup 결과의 실제 ID로 회수할 수 있다.
    pub(crate) fn pending_cleanup_id(&self) -> Option<RequestId> {
        self.shared
            .lock()
            .control_result
            .as_ref()
            .map(|(id, _)| RequestId {
                worker: self.shared.marker.clone(),
                serial: *id,
                control: true,
            })
    }
    pub(crate) fn try_take<O: Send + 'static>(
        &self,
        id: &RequestId,
    ) -> Result<Option<O>, WorkerCategory> {
        take(&self.shared, id)
    }
    pub(crate) fn revalidation(
        &self,
        key: &SessionKey,
    ) -> Result<RevalidationSnapshot, WorkerCategory> {
        self.shared
            .lock()
            .scheduled
            .iter()
            .find(|s| s.key == *key)
            .map(|s| RevalidationSnapshot {
                key: s.key.clone(),
                phase: s.reservation.phase,
                last: s.last,
                completed: s.completed,
                eligible: s.eligible,
            })
            .ok_or(WorkerCategory::Stale)
    }
    pub(crate) fn take_validation_failure(&self, key: &SessionKey) -> Option<RevalidationFailure> {
        let mut m = self.shared.lock();
        let failure = m
            .scheduled
            .iter_mut()
            .find(|s| s.key == *key)
            .and_then(|s| s.failure.take());
        m.shutdown.notify();
        self.shared.wake.notify_one();
        failure
    }
    /// 닫힌 일반 gate를 우회하는 job은 받지 않는다. cleanup view에는 저장/재획득 API가 없다.
    pub(crate) fn try_cleanup<I: Send + 'static, O: Send + 'static>(
        &self,
        input: I,
        run: fn(&mut CleanupContext<'_, P, R>, I) -> O,
    ) -> Result<Ticket<O>, Rejected<I>> {
        self.try_control_inner(input, run, false)
    }
    /// 앱의 명시적 세션 제어는 일반 큐 포화 중에도 별도 한 칸에 넣는다.
    /// 기존 cleanup의 closed 선행 조건은 유지하고, 이 view에도 쓰기/재획득 권한은 없다.
    pub(crate) fn try_control<I: Send + 'static, O: Send + 'static>(
        &self,
        input: I,
        run: fn(&mut CleanupContext<'_, P, R>, I) -> O,
    ) -> Result<Ticket<O>, Rejected<I>> {
        self.try_control_inner(input, run, true)
    }
    fn try_control_inner<I: Send + 'static, O: Send + 'static>(
        &self,
        input: I,
        run: fn(&mut CleanupContext<'_, P, R>, I) -> O,
        allow_open: bool,
    ) -> Result<Ticket<O>, Rejected<I>> {
        let mut m = self.shared.lock();
        let admission = (if m.status == WorkerStatus::InitializationFailed && m.runtime.is_some() {
            Ok(())
        } else {
            ready(&m)
        })
        .and_then(|()| {
            if m.stop {
                // stop 수락과 같은 lock에서 닫아, 수락한 cleanup이 loop 종료에 버려지지 않게 한다.
                Err(WorkerCategory::Closed)
            } else if !m.closed && !allow_open {
                Err(WorkerCategory::MustCloseAdmission)
            } else if m.control_result.is_some() || m.shutdown.running {
                Err(WorkerCategory::Full)
            } else {
                next_id(&mut m)
            }
        });
        let id = match admission {
            Ok(id) => id,
            Err(category) => return Err(Rejected { input, category }),
        };
        m.control_result = Some((id, Stored::Pending));
        m.control = Some((id, Box::new(CleanupJob { input, run })));
        self.shared.wake.notify_one();
        Ok(Ticket {
            id: RequestId {
                worker: self.shared.marker.clone(),
                serial: id,
                control: true,
            },
            output: PhantomData,
        })
    }
    pub(crate) fn request_stop(&self) -> Result<(), WorkerCategory> {
        let mut m = self.shared.lock();
        request_stop_locked(&mut m)?;
        if m.shutdown.view.phase != ShutdownPhase::Idle {
            m.shutdown.mark_stop();
        }
        self.shared.wake.notify_one();
        Ok(())
    }
    /// is_finished를 확인한 뒤에만 join한다. UI poll은 blocking join으로 바뀌지 않는다.
    pub(crate) fn try_join(&mut self) -> Result<bool, WorkerCategory> {
        if self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            return Ok(false);
        }
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                self.shared.lock().shutdown.unavailable();
                return Err(WorkerCategory::Unavailable);
            }
        }
        let mut m = self.shared.lock();
        let available = m.status != WorkerStatus::Unavailable;
        m.shutdown.joined(available);
        Ok(true)
    }
}

fn request_stop_locked<P, R>(m: &mut Mailbox<P, R>) -> Result<(), WorkerCategory> {
    if m.status != WorkerStatus::InitializationFailed {
        ready(m)?;
    }
    if !m.closed {
        return Err(WorkerCategory::MustCloseAdmission);
    }
    if m.running
        || !m.queue.is_empty()
        || !m.results.is_empty()
        || m.control_result.is_some()
        || m.sessions != 0
        || m.runtime.is_some()
        || m.scheduled.iter().any(|s| s.failure.is_some())
        || m.shutdown.has_report()
        || m.shutdown.pending
    {
        return Err(WorkerCategory::OwnersRemain);
    }
    m.stop = true;
    Ok(())
}

fn synchronize<P, R>(m: &mut Mailbox<P, R>, context: &WorkerContext<P, R>) {
    m.runtime = context.runtime.as_ref().map(ProjectRuntime::snapshot);
    m.runtime_closed = context.runtime_close.is_some();
    m.sessions = context.sessions.len();
    m.session_observations = context.observations();
    for old in &mut m.scheduled {
        old.eligible = context
            .current(&old.key)
            .is_ok_and(|e| e.service.snapshot().state() == EditSessionState::Editing);
    }
    // 사용하지 않은 옛 identity만 회수한다. 예약 또는 최초 실패가 남으면 별도 결과 owner다.
    m.scheduled.retain(|s| {
        context.current(&s.key).is_ok() || s.reservation.phase != Phase::Idle || s.failure.is_some()
    });
    for entry in &context.sessions {
        if let Some(key) = context.key(entry) {
            if !m.scheduled.iter().any(|s| s.key == key) {
                m.scheduled.push(Scheduled {
                    key,
                    eligible: entry.service.snapshot().state() == EditSessionState::Editing,
                    reservation: Reservation::default(),
                    last: None,
                    completed: 0,
                    failure: None,
                });
            }
        }
    }
}
fn run_worker<P: 'static, R: 'static>(
    shared: &Shared<P, R>,
    config: WorkerConfig,
    mut periodic: Periodic,
) {
    shared.lock().thread = Some(thread::current().id());
    #[cfg(all(test, windows))]
    let _io_observer = bridge_test_support::install_io(&config.project_root);
    #[cfg(all(test, windows))]
    bridge_test_support::before_init(&config.project_root);
    let runtime = match if config.initialize_empty {
        ProjectRuntime::acquire_empty(
            &config.project_root,
            &config.lock_root,
            config.create_directory,
        )
    } else {
        ProjectRuntime::acquire(&config.project_root, &config.lock_root)
    } {
        Ok(rt) => rt,
        Err(error) => {
            let mut m = shared.lock();
            m.initialization_error = Some(error);
            m.status = WorkerStatus::InitializationFailed;
            m.closed = true;
            if m.shutdown.view.phase != ShutdownPhase::Idle {
                let error = m.initialization_error.clone();
                m.shutdown.absent_runtime(error);
            }
            shared.wake.notify_all();
            return;
        }
    };
    let mut context = WorkerContext::new(runtime, shared.marker.clone(), config.max_sessions);
    // 복구 실패 시에도 실제 Pending/Blocked runtime은 제어 owner 아래 남겨 명시적으로 정리한다.
    let recovery = context.runtime.as_mut().map(ProjectRuntime::recover);
    {
        let mut m = shared.lock();
        m.initialization_error = recovery.and_then(Result::err);
        m.status = if m.initialization_error.is_some() {
            m.closed = true;
            WorkerStatus::InitializationFailed
        } else {
            WorkerStatus::Ready
        };
        synchronize(&mut m, &context);
        shared.wake.notify_all();
    }
    shared.notify_app();
    loop {
        let (job, control, reasons) = {
            let mut m = shared.lock();
            loop {
                if m.stop {
                    m.status = WorkerStatus::Stopped;
                    shared.wake.notify_all();
                    return;
                }
                if m.status == WorkerStatus::Unavailable {
                    return;
                }
                if !m.closed {
                    match periodic.due(shared.now()) {
                        Ok(true) => {
                            let mut queued = Vec::new();
                            for s in &mut m.scheduled {
                                if s.eligible
                                    && config.automatic_revalidation
                                    && s.reservation.trigger(TriggerReason::Periodic)
                                        == TriggerAccepted::Queued
                                {
                                    queued.push(s.key.clone());
                                }
                            }
                            m.queue.extend(queued.into_iter().map(Queued::Revalidate));
                        }
                        Ok(false) => {}
                        Err(error) => {
                            m.timer_error = Some(error);
                            m.closed = true;
                        }
                    }
                }
                if let Some(job) = m.queue.pop_front() {
                    m.running = true;
                    let reasons = if let Queued::Revalidate(key) = &job {
                        m.scheduled
                            .iter_mut()
                            .find(|s| s.key == *key)
                            .map(|s| s.reservation.start())
                            .unwrap_or_default()
                    } else {
                        Reasons::default()
                    };
                    break (job, false, reasons);
                }
                if let Some((id, job)) = m.control.take() {
                    m.running = true;
                    break (Queued::Job(id, job), true, Reasons::default());
                }
                // 사용자 cleanup 결과는 덮어쓰지 않는다. 회수 통지가 같은 종료를 다시 깨운다.
                if m.control_result.is_none() {
                    if let Some(machine) = m.shutdown.begin() {
                        m.running = true;
                        break (
                            Queued::Shutdown(Box::new(machine)),
                            true,
                            Reasons::default(),
                        );
                    }
                } else if m.shutdown.waiting_control() {
                    shared.wake.notify_all();
                }
                let wait = if m.closed {
                    Duration::from_secs(3600)
                } else {
                    periodic.wait(shared.now())
                };
                m = match shared.wake.wait_timeout(m, wait) {
                    Ok((guard, _)) => guard,
                    Err(poison) => {
                        let (mut guard, _) = poison.into_inner();
                        guard.status = WorkerStatus::Unavailable;
                        guard.shutdown.unavailable();
                        guard.closed = true;
                        guard
                    }
                };
            }
        };
        match job {
            Queued::Job(id, job) => {
                context.retained_failure_owners = shared
                    .lock()
                    .scheduled
                    .iter()
                    .filter(|s| s.failure.is_some())
                    .map(|s| s.key.registration.clone())
                    .collect();
                // SVN provider는 같은 작업 안의 반복 permit 검사에서 동일한
                // 서버 lock token을 재조회하지 않는다. 다음 작업/주기 재검사는 새 범위다.
                let output = crate::svn::with_validation_scope(|| job.run(&mut context));
                let mut m = shared.lock();
                if control {
                    m.control_result = Some((id, Stored::Complete(output)));
                } else {
                    m.results.insert(id, Stored::Complete(output));
                }
                synchronize(&mut m, &context);
                m.running = false;
                m.shutdown.notify();
            }
            Queued::Revalidate(key) => {
                let (completion, failure) = context.revalidate(&key, reasons);
                let mut m = shared.lock();
                // old key completion만 끝낸다. 새 identity의 reservation을 지우지 않는다.
                if let Some(s) = m.scheduled.iter_mut().find(|s| s.key == key) {
                    s.last = Some(completion);
                    s.completed = s.completed.saturating_add(1);
                    if let Some(failure) = failure {
                        s.failure = Some(failure);
                    }
                    if s.reservation.finish() {
                        m.queue.push_back(Queued::Revalidate(key));
                    }
                }
                synchronize(&mut m, &context);
                m.running = false;
                m.shutdown.notify();
            }
            Queued::Shutdown(machine) => {
                let drain = {
                    let m = shared.lock();
                    context.retained_failure_owners = m
                        .scheduled
                        .iter()
                        .filter(|s| s.failure.is_some())
                        .map(|s| s.key.registration.clone())
                        .collect();
                    DrainObservation {
                        ordinary_results: m.results.len(),
                        validation_failures: context.retained_failure_owners.len(),
                        initialization_error: m.initialization_error.clone(),
                    }
                };
                let (machine, report) =
                    machine.execute(&mut CleanupContext(&mut context), drain, |view| {
                        shared.lock().shutdown.view = view;
                        shared.wake.notify_all();
                    });
                let mut m = shared.lock();
                synchronize(&mut m, &context);
                m.running = false;
                m.shutdown.finish(machine, report);
                // 자기 보고도 먼저 회수해야 한다. 완료 보고와 사용자 cleanup 슬롯 간 순환은 없다.
                if context.runtime_close.is_some() && request_stop_locked(&mut m).is_ok() {
                    m.shutdown.mark_stop();
                }
            }
        }
        shared.wake.notify_all();
        shared.notify_app();
        #[cfg(all(test, windows))]
        bridge_test_support::after_publish();
        #[cfg(all(test, windows))]
        tests::after_job();
    }
}

#[cfg(all(test, windows))]
mod tests;
