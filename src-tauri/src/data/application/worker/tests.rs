use super::*;
mod release_cleanup;
mod shutdown;

thread_local! {
    static AFTER_JOB: std::cell::RefCell<std::collections::VecDeque<Gate>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

// mailbox lock 밖에서 다음 loop만 지연한다. 상태나 owner를 주입하지 않는 worker별 gate다.
pub(super) fn after_job() {
    let gate = AFTER_JOB.with(|slot| slot.borrow_mut().pop_front());
    if let Some(gate) = gate {
        gate.block();
    }
}
use crate::data::{
    application::{
        composite::{tests as fixture, CompositeSaveInput},
        diagnostics::{ApplicationCategory, DiskState},
        documents::persistence::{DocumentUpdateError, SaveDocumentInput},
        recovery_handoff::{Acknowledged, RecoveryBackend},
        write::BodyOutcome,
    },
    artifact::{self, DocumentSaveOutcomeKind},
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockProviderKind, LockService, NoLockService,
    },
    edit_session::{
        DurableRecoverySink, EditSessionOperation, RecoveryEnvelope, RecoveryHandoffStatus,
        RecoverySinkFailure, RecoverySinkFailureCategory, ValidationErrorSource,
    },
    project_runtime::RuntimeState,
    repository::ArtifactSourceId,
};
use std::{
    cell::Cell,
    fs,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

const LIMIT: Duration = Duration::from_secs(20);
const INTERVAL: Duration = Duration::from_secs(3600);
const SECRET: &str = "G11_PRIVATE_undo_selection_unfinished";
const RECEIPT: &str = "G11_PRIVATE_receipt_original";

fn wait_for<P, R>(control: &WorkerControl<P, R>, predicate: impl Fn(&Mailbox<P, R>) -> bool) {
    let end = Instant::now() + LIMIT;
    let mut m = control.shared.lock();
    while !predicate(&m) {
        let left = end.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "worker event deadline");
        m = control.shared.wake.wait_timeout(m, left).unwrap().0;
    }
}
fn result<P: 'static, R: 'static, O: Send + 'static>(c: &WorkerControl<P, R>, t: &Ticket<O>) -> O {
    wait_for(c, |m| {
        let v = if t.id.control {
            m.control_result.as_ref().map(|(_, v)| v)
        } else {
            m.results.get(&t.id.serial)
        };
        matches!(v, Some(Stored::Complete(_))) || m.status == WorkerStatus::Unavailable
    });
    c.try_take(&t.id())
        .unwrap()
        .expect("actual completed result")
}
fn config(f: &fixture::Fixture, capacity: usize, max_sessions: usize) -> WorkerConfig {
    WorkerConfig {
        project_root: f.root.clone(),
        lock_root: f.root.parent().unwrap().join("locks"),
        initialize_empty: false,
        create_directory: false,
        capacity,
        max_sessions,
        interval: INTERVAL,
        automatic_revalidation: true,
    }
}
fn start<P: 'static, R: 'static>(
    capacity: usize,
    max_sessions: usize,
) -> (fixture::Fixture, WorkerControl<P, R>, WorkerClient<P, R>) {
    let f = fixture::Fixture::new();
    let (t, d) = fixture::fixture_raw();
    f.seed(&t, &d);
    let (control, client) = WorkerControl::start(config(&f, capacity, max_sessions)).unwrap();
    wait_for(&control, |m| m.status != WorkerStatus::Starting);
    assert_eq!(
        control.snapshot().runtime.unwrap().state,
        RuntimeState::Ready
    );
    (f, control, client)
}
fn finish<P: 'static, R: 'static>(c: &mut WorkerControl<P, R>, registrations: Vec<Registration>) {
    c.close_admission();
    let t = c
        .try_cleanup(registrations, |ctx, registrations| {
            for reg in registrations {
                ctx.end_session(&reg).unwrap().unwrap();
                ctx.remove_session(&reg).unwrap();
            }
            ctx.close_runtime().unwrap().unwrap();
        })
        .unwrap();
    result(c, &t);
    c.request_stop().unwrap();
    wait_for(c, |m| m.status == WorkerStatus::Stopped);
    let end = Instant::now() + LIMIT;
    while !c.try_join().unwrap() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}

struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
struct GateOwner {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}
fn gate() -> (Gate, GateOwner) {
    let (tx, rx) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    (
        Gate {
            entered: tx,
            release: wait,
        },
        GateOwner {
            entered: rx,
            release,
        },
    )
}
impl Gate {
    fn block(self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(LIMIT).unwrap();
    }
}
impl GateOwner {
    fn entered(&self) {
        self.entered.recv_timeout(LIMIT).unwrap();
    }
    fn release(self) {
        self.release.send(()).unwrap();
    }
}
#[derive(Default)]
struct ProbeState {
    events: Vec<(&'static str, thread::ThreadId)>,
    block: Option<Gate>,
    fail: bool,
    active: usize,
    max_active: usize,
}
type ProbeOwner = Arc<Mutex<ProbeState>>;
struct Probe {
    owner: ProbeOwner,
    inner: NoLockService,
    private_token: &'static str,
}
impl Probe {
    fn new(owner: ProbeOwner) -> Self {
        Self {
            owner,
            inner: NoLockService::new(),
            private_token: "provider-token-sentinel",
        }
    }
    fn record(&self, name: &'static str) {
        self.owner
            .lock()
            .unwrap()
            .events
            .push((name, thread::current().id()));
    }
}
impl LockService for Probe {
    fn provider_info(&self) -> LockProviderInfo {
        self.inner.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        self.record("acquire");
        self.inner.acquire(r)
    }
    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        assert!(self.private_token == "provider-token-sentinel");
        self.record("validate");
        let block = {
            let mut s = self.owner.lock().unwrap();
            s.active += 1;
            s.max_active = s.max_active.max(s.active);
            s.block.take()
        };
        if let Some(block) = block {
            block.block();
        }
        let fail = {
            let mut s = self.owner.lock().unwrap();
            s.active -= 1;
            s.fail
        };
        if fail {
            Err(LockError::for_held(
                LockErrorCategory::LockLost,
                LockProviderKind::None,
                LockOperation::Validate,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ))
        } else {
            self.inner.validate(held)
        }
    }
    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        self.record("release");
        self.inner.release(held)
    }
}
fn count(owner: &ProbeOwner, name: &str) -> usize {
    owner
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|(n, _)| *n == name)
        .count()
}
fn register<P, R>(
    ctx: &mut WorkerContext<P, R>,
    owner: ProbeOwner,
) -> context::SessionRegistration {
    let r = ctx
        .begin_session(
            Arc::new(Probe::new(owner)),
            vec![ArtifactSourceId::Document(fixture::did()).path().unwrap()],
        )
        .unwrap();
    assert!(r.original.is_ok());
    r
}
fn wait_validations<P, R>(c: &WorkerControl<P, R>, key: &SessionKey, completed: u64) {
    wait_for(c, |m| {
        m.scheduled.iter().any(|s| {
            s.key == *key && s.completed >= completed && s.reservation.phase == Phase::Idle
        })
    });
}
fn document_input<P, R>(ctx: &mut WorkerContext<P, R>, _: ()) -> SaveDocumentInput {
    let i = fixture::load_input(
        ctx.runtime.as_mut().unwrap(),
        fixture::required_intent(),
        vec![fixture::filled_edit()],
    );
    SaveDocumentInput {
        document: i.document,
        template: i.template,
        edits: i.edits,
        timestamp_utc: i.timestamp_utc,
    }
}

#[test]
fn w1_w7_actual_worker_serial_g8_disk_and_fresh_no_write() {
    let (f, mut c, client) = start::<(), ()>(4, 2);
    let owner = ProbeOwner::default();
    let setup = client
        .try_submit(owner.clone(), |ctx, owner| {
            assert_eq!(ctx.runtime_snapshot().unwrap().state, RuntimeState::Ready);
            fixture::add_historical(ctx.runtime.as_mut().unwrap());
            let reg = register(ctx, owner);
            let input = document_input(ctx, ());
            (reg, input, thread::current().id())
        })
        .unwrap();
    let (reg, input, thread_id) = result(&c, &setup);
    assert_ne!(thread_id, thread::current().id());
    assert_eq!(Some(thread_id), c.snapshot().thread);
    let key = reg.key.unwrap();
    let before = fixture::pair(&f);
    let (block, hold) = gate();
    owner.lock().unwrap().block = Some(block);
    let save = client
        .try_submit((key.clone(), input), |ctx, (key, input)| {
            let (saved, prepare, commits) =
                fixture::observe(|| ctx.session(&key).unwrap().save_document(&input));
            (input, saved, prepare.calls, commits, thread::current().id())
        })
        .unwrap();
    hold.entered();
    // 첫 저장이 provider 안에서 막혀도 두 번째 ingress와 trigger가 즉시 반환한다.
    let second = client
        .try_submit(key.clone(), |ctx, key| {
            let input = document_input(ctx, ());
            let saved = ctx.session(&key).unwrap().save_document(&input);
            (saved, thread::current().id())
        })
        .unwrap();
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap(),
        TriggerAccepted::Queued
    );
    assert!(client.try_take(&second).unwrap().is_none());
    assert_eq!(c.snapshot().queued, 2);
    assert_eq!(count(&owner, "validate"), 1);
    hold.release();
    let (_, saved, prepares, commits, observed_thread) = result(&c, &save);
    let saved = saved.unwrap();
    assert_eq!(saved.execution.diagnostic().disk, DiskState::Committed);
    assert_eq!((prepares, commits), (1, 1));
    assert_eq!(observed_thread, thread_id);
    let outcome = saved.outcome().unwrap();
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
    fixture::warning(outcome);
    // 정규화/fixture write 전에 실제 disk와 상대 artifact를 직접 확인한다.
    let after = fixture::pair(&f);
    assert!(after.document.0 == artifact::encode_document(outcome.document()).unwrap());
    assert!(after.template == before.template && after.sentinel == before.sentinel);
    fixture::preserved(&after.template.0, &after.document.0);
    let (again, again_thread) = result(&c, &second);
    let again = again.unwrap();
    assert!(matches!(
        again.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    assert_eq!(
        again.outcome().unwrap().kind(),
        DocumentSaveOutcomeKind::Unchanged
    );
    fixture::warning(again.outcome().unwrap());
    assert_eq!(again_thread, thread_id);
    wait_validations(&c, &key, 1);
    fixture::same_pair(&f, &after);
    assert!(owner
        .lock()
        .unwrap()
        .events
        .iter()
        .all(|(_, id)| *id == thread_id));
    assert_eq!(owner.lock().unwrap().max_active, 1);
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn w2_w3_queued_burst_running_follow_up_and_other_session_fairness() {
    let (_f, mut c, client) = start::<(), ()>(4, 2);
    let owner = ProbeOwner::default();
    let r1 = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let r2 = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let k1 = r1.key.unwrap();
    let k2 = r2.key.unwrap();
    let (blocking, hold) = gate();
    let blocked = client.try_submit(blocking, |_, gate| gate.block()).unwrap();
    hold.entered();
    assert_eq!(
        client.trigger(&k1, TriggerReason::Foreground).unwrap(),
        TriggerAccepted::Queued
    );
    for _ in 0..100 {
        assert_eq!(
            client.trigger(&k1, TriggerReason::OsResume).unwrap(),
            TriggerAccepted::Coalesced
        );
    }
    assert_eq!(
        client.trigger(&k2, TriggerReason::Foreground).unwrap(),
        TriggerAccepted::Queued
    );
    let (validation, held_validation) = gate();
    owner.lock().unwrap().block = Some(validation);
    hold.release();
    result(&c, &blocked);
    held_validation.entered();
    for _ in 0..100 {
        assert_eq!(
            client.trigger(&k1, TriggerReason::OsResume).unwrap(),
            TriggerAccepted::FollowUp
        );
    }
    let ordinary = client
        .try_submit(owner.clone(), |_, owner| {
            owner
                .lock()
                .unwrap()
                .events
                .push(("ordinary", thread::current().id()));
        })
        .unwrap();
    assert_eq!(c.snapshot().queued, 2);
    held_validation.release();
    result(&c, &ordinary);
    wait_validations(&c, &k1, 2);
    wait_validations(&c, &k2, 1);
    let events: Vec<_> = owner
        .lock()
        .unwrap()
        .events
        .iter()
        .map(|(n, _)| *n)
        .collect();
    assert!(events == ["acquire", "acquire", "validate", "validate", "ordinary", "validate"]);
    // completion 이전 trigger와 이후 trigger를 각각 고정한다. 이후 이벤트는 새 예약이다.
    assert_eq!(
        client.trigger(&k1, TriggerReason::Foreground).unwrap(),
        TriggerAccepted::Queued
    );
    wait_validations(&c, &k1, 3);
    assert_eq!(count(&owner, "validate"), 4);
    assert_eq!(count(&owner, "acquire"), 2);
    assert_eq!(owner.lock().unwrap().max_active, 1);
    finish(&mut c, vec![r1.registration, r2.registration]);
}

fn advance<P, R>(c: &WorkerControl<P, R>, now: Instant) {
    let _m = c.shared.lock();
    *c.shared.clock.lock().unwrap() = Some(now);
    c.shared.wake.notify_one();
}
#[test]
fn w3_monotonic_deadlines_actual_worker_drive_and_missed_tick_coalescing() {
    let now = Instant::now();
    assert!(matches!(
        Periodic::new(now, Duration::ZERO),
        Err(IntervalError::Zero)
    ));
    assert!(matches!(
        Periodic::new(now, Duration::MAX),
        Err(IntervalError::DeadlineOverflow)
    ));
    let mut timer = Periodic::new(now, Duration::from_secs(2)).unwrap();
    assert!(!timer.due(now + Duration::from_secs(1)).unwrap());
    assert!(timer.due(now + Duration::from_secs(2)).unwrap());
    assert!(timer.due(now + Duration::from_secs(1000)).unwrap());
    assert!(!timer.due(now + Duration::from_secs(1000)).unwrap());
    let (_f, mut c, client) = start::<(), ()>(3, 1);
    let base = Instant::now();
    advance(&c, base);
    let owner = ProbeOwner::default();
    let reg = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let key = reg.key.unwrap();
    result(&c, &client.try_submit((), |_, ()| ()).unwrap());
    assert_eq!(count(&owner, "validate"), 0);
    advance(&c, base + INTERVAL);
    wait_validations(&c, &key, 1);
    let (blocked, hold) = gate();
    owner.lock().unwrap().block = Some(blocked);
    client.trigger(&key, TriggerReason::OsResume).unwrap();
    hold.entered();
    advance(&c, base + INTERVAL * 100);
    for _ in 0..100 {
        client.trigger(&key, TriggerReason::Foreground).unwrap();
    }
    hold.release();
    wait_validations(&c, &key, 3);
    result(&c, &client.try_submit((), |_, ()| ()).unwrap());
    assert_eq!(count(&owner, "validate"), 3);
    assert_eq!(count(&owner, "acquire"), 1);
    c.close_admission();
    advance(&c, base + INTERVAL * 200);
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Closed
    );
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn collaborative_worker_does_not_revalidate_on_idle_deadline() {
    let f = fixture::Fixture::new();
    let mut config = config(&f, 3, 1);
    config.automatic_revalidation = false;
    let (mut control, client) = WorkerControl::<(), ()>::start(config).unwrap();
    wait_for(&control, |m| m.status != WorkerStatus::Starting);
    let owner = ProbeOwner::default();
    let reg = result(
        &control,
        &client.try_submit(owner.clone(), register).unwrap(),
    );
    let key = reg.key.unwrap();
    let base = Instant::now();
    advance(&control, base + INTERVAL * 3);
    result(&control, &client.try_submit((), |_, ()| ()).unwrap());
    assert_eq!(count(&owner, "validate"), 0);
    assert_eq!(control.revalidation(&key).unwrap().completed, 0);
    finish(&mut control, vec![reg.registration]);
}

struct Payload {
    bytes: Vec<u8>,
    owner: Arc<()>,
    drops: Arc<AtomicUsize>,
}
struct Receipt {
    bytes: Vec<u8>,
    owner: Arc<()>,
    drops: Arc<AtomicUsize>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct Proof {
    owner: Arc<()>,
    drops: Arc<AtomicUsize>,
    address: usize,
}
fn payload() -> (Payload, Proof) {
    let owner = Arc::new(());
    let drops = Arc::new(AtomicUsize::new(0));
    let p = Payload {
        bytes: SECRET.as_bytes().to_vec(),
        owner: owner.clone(),
        drops: drops.clone(),
    };
    let proof = Proof {
        owner,
        drops,
        address: p.bytes.as_ptr() as usize,
    };
    (p, proof)
}
impl Proof {
    fn check(&self, p: &Payload) {
        assert!(p.bytes == SECRET.as_bytes());
        assert!(Arc::ptr_eq(&p.owner, &self.owner));
        assert!(p.bytes.as_ptr() as usize == self.address);
        assert_eq!(self.drops.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn w5_full_closed_and_unclaimed_results_return_original_input_once() {
    let (_f, mut c, client) = start::<(), ()>(2, 1);
    let (g, hold) = gate();
    let (p, proof) = payload();
    let first = client
        .try_submit((g, p), |_, (g, p)| {
            g.block();
            p
        })
        .unwrap();
    let first_id = first.id();
    drop(first);
    hold.entered();
    let (p2, proof2) = payload();
    let calls = Arc::new(AtomicUsize::new(0));
    let second = client
        .try_submit((p2, calls.clone()), |_, (p, calls)| {
            calls.fetch_add(1, Ordering::SeqCst);
            p
        })
        .unwrap();
    let second_id = second.id();
    drop(second);
    let (p3, proof3) = payload();
    let denied = client.try_submit(p3, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Full);
    proof3.check(&denied.input);
    hold.release();
    wait_for(&c, |m| {
        m.results.values().all(|v| matches!(v, Stored::Complete(_)))
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // 완료된 미회수 결과도 두 칸을 계속 점유한다. 다른 타입의 조회도 owner를 소모하지 않는다.
    assert_eq!(
        client.try_submit((), |_, ()| ()).unwrap_err().category,
        WorkerCategory::Full
    );
    assert_eq!(
        c.try_take::<()>(&first_id).unwrap_err(),
        WorkerCategory::ResultType
    );
    let recovered: Payload = c.try_take(&first_id).unwrap().unwrap();
    proof.check(&recovered);
    let recovered2: Payload = c.try_take(&second_id).unwrap().unwrap();
    proof2.check(&recovered2);
    assert_eq!(
        c.try_take::<()>(&first_id).unwrap_err(),
        WorkerCategory::UnknownRequest
    );
    c.close_admission();
    let denied = client.try_submit(denied.input, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Closed);
    proof3.check(&denied.input);
    drop((recovered, recovered2, denied));
    for proof in [proof, proof2, proof3] {
        assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    }
    finish(&mut c, vec![]);
}

#[test]
fn w5_initialization_failure_does_not_accept_or_lose_payload() {
    let f = fixture::Fixture::new();
    let mut cfg = config(&f, 2, 1);
    cfg.project_root = f.root.join("missing");
    let (mut c, client) = WorkerControl::<(), ()>::start(cfg).unwrap();
    wait_for(&c, |m| m.status == WorkerStatus::InitializationFailed);
    let (p, proof) = payload();
    let rejected = client.try_submit(p, |_, p| p).unwrap_err();
    assert_eq!(rejected.category, WorkerCategory::InitializationFailed);
    proof.check(&rejected.input);
    assert!(c.snapshot().initialization_error.is_some());
    assert!(c.snapshot().runtime.is_none());
    c.request_stop().unwrap();
    let end = Instant::now() + LIMIT;
    while !c.try_join().unwrap() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    drop(rejected);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn w6_close_drains_jobs_and_reserved_follow_up_control_survives_clients() {
    let (_f, mut c, client) = start::<(), ()>(2, 1);
    let owner = ProbeOwner::default();
    let reg = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let key = reg.key.unwrap();
    let (g, hold) = gate();
    owner.lock().unwrap().block = Some(g);
    client.trigger(&key, TriggerReason::Foreground).unwrap();
    hold.entered();
    client.trigger(&key, TriggerReason::OsResume).unwrap();
    let (p, proof) = payload();
    let accepted = client.try_submit(p, |_, p| p).unwrap();
    let id = accepted.id();
    drop(accepted);
    let second = client.try_submit(41_u32, |_, value| value + 1).unwrap();
    let second_id = second.id();
    drop(second);
    assert_eq!(c.snapshot().ordinary_outstanding, 2);
    let (q, qproof) = payload();
    let (ingress, ingress_owner) = gate();
    let competing = client.clone();
    let (reply, received) = mpsc::channel();
    let submitter = thread::spawn(move || {
        ingress.block();
        reply.send(competing.try_submit(q, |_, p| p)).unwrap();
    });
    ingress_owner.entered();
    c.close_admission();
    ingress_owner.release();
    let rejected = received.recv_timeout(LIMIT).unwrap().unwrap_err();
    submitter.join().unwrap();
    assert_eq!(rejected.category, WorkerCategory::Closed);
    qproof.check(&rejected.input);
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Closed
    );
    drop(client);
    // 별도 제어 칸은 일반 결과/큐가 차 있어도 수락하고 이미 받은 재검증 뒤에만 실행한다.
    let cleanup = c
        .try_cleanup(reg.registration.clone(), |ctx, reg| {
            ctx.end_session(&reg).unwrap().unwrap();
            ctx.remove_session(&reg).unwrap();
        })
        .unwrap();
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    hold.release();
    result(&c, &cleanup);
    assert_eq!(count(&owner, "validate"), 2);
    let recovered: Payload = c.try_take(&id).unwrap().unwrap();
    proof.check(&recovered);
    assert_eq!(c.try_take::<u32>(&second_id).unwrap(), Some(42));
    assert_eq!(c.snapshot().queued, 0);
    assert!(c.snapshot().runtime.is_some());
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    drop((recovered, rejected));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    finish(&mut c, vec![]);
}

#[derive(Default)]
struct SinkObservation {
    calls: usize,
    fail_next: bool,
    accepted: Option<Payload>,
    seen_address: Option<usize>,
    receipt_owner: Option<Arc<()>>,
    receipt_drops: Arc<AtomicUsize>,
    reasons: Vec<crate::data::edit_session::RecoveryReason>,
}
type SinkOwner = Arc<Mutex<SinkObservation>>;
// Rc가 있어 이 adapter는 !Send다. worker job 안에서 생성하고 같은 worker에서만 호출한다.
struct Sink {
    owner: SinkOwner,
    local_calls: Rc<Cell<usize>>,
}
impl DurableRecoverySink<Payload> for Sink {
    type Receipt = Receipt;
    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        p: Payload,
    ) -> Result<Receipt, RecoverySinkFailure<Payload>> {
        self.local_calls.set(self.local_calls.get() + 1);
        let mut s = self.owner.lock().unwrap();
        s.calls += 1;
        assert_eq!(s.calls, self.local_calls.get());
        assert!(p.bytes == SECRET.as_bytes());
        let address = p.bytes.as_ptr() as usize;
        if let Some(old) = s.seen_address {
            assert!(old == address);
        }
        s.seen_address = Some(address);
        s.reasons.push(envelope.reason());
        if std::mem::take(&mut s.fail_next) {
            return Err(RecoverySinkFailure::new(
                p,
                RecoverySinkFailureCategory::DurabilityUncertain,
            ));
        }
        assert!(s.accepted.is_none());
        s.accepted = Some(p);
        let owner = Arc::new(());
        s.receipt_owner = Some(owner.clone());
        Ok(Receipt {
            bytes: RECEIPT.as_bytes().to_vec(),
            owner,
            drops: s.receipt_drops.clone(),
        })
    }
}
fn connect(ctx: &mut WorkerContext<Payload, Receipt>, (key, owner): (SessionKey, SinkOwner)) {
    let backend = RecoveryBackend::connected(Sink {
        owner,
        local_calls: Rc::new(Cell::new(0)),
    });
    assert!(ctx.session(&key).unwrap().connect_backend(backend).is_ok());
}
fn check_receipt(ack: &Acknowledged<Receipt>, owner: &SinkOwner) {
    assert!(ack.receipt.bytes == RECEIPT.as_bytes());
    let s = owner.lock().unwrap();
    assert!(Arc::ptr_eq(
        &ack.receipt.owner,
        s.receipt_owner.as_ref().unwrap()
    ));
    assert_eq!(s.receipt_drops.load(Ordering::SeqCst), 0);
}
fn clean_active(c: &WorkerControl<Payload, Receipt>, reg: Registration) -> Payload {
    c.close_admission();
    let t = c
        .try_cleanup(reg, |ctx, reg| ctx.return_active(&reg).unwrap())
        .unwrap();
    result(c, &t)
}

#[test]
fn w7_g9_g10_pair_execution_survives_wait_drop_and_closed_ingress() {
    let (f, mut c, client) = start::<Payload, Receipt>(3, 1);
    let sink = SinkOwner::default();
    let provider = ProbeOwner::default();
    let (p, proof) = payload();
    let init = client
        .try_submit(
            (provider.clone(), sink.clone(), p),
            |ctx, (provider, sink, p)| {
                fixture::add_historical(ctx.runtime.as_mut().unwrap());
                let input = fixture::load_input(
                    ctx.runtime.as_mut().unwrap(),
                    fixture::required_intent(),
                    vec![fixture::filled_edit()],
                );
                let targets =
                    crate::data::application::composite::CompositeWriteRequest::new(&input)
                        .unwrap()
                        .session_targets()
                        .to_vec();
                let reg = ctx
                    .begin_session(Arc::new(Probe::new(provider)), targets)
                    .unwrap();
                assert!(reg.original.is_ok());
                let key = reg.key.as_ref().unwrap().clone();
                connect(ctx, (key.clone(), sink));
                ctx.session(&key).unwrap().bind_draft(p).unwrap();
                (reg, input)
            },
        )
        .unwrap();
    let (reg, input) = result(&c, &init);
    let key = reg.key.unwrap();
    let before = fixture::pair(&f);
    let (g, hold) = gate();
    let save = client
        .try_submit((key.clone(), input, g), |ctx, (key, input, gate)| {
            use crate::data::transaction::test_support::{
                with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint,
            };
            let gate = std::cell::RefCell::new(Some(gate));
            let commits = Rc::new(Cell::new(0_usize));
            let seen = commits.clone();
            let (saved, prepare) = with_canonical_prepare_hooks(
                |_, _| Ok(()),
                || {
                    with_commit_io_factory(
                        move |point, _| {
                            if point == Some(CommitTestPoint::ManifestRevalidation) {
                                seen.set(seen.get() + 1);
                                if let Some(gate) = gate.borrow_mut().take() {
                                    gate.block();
                                }
                            }
                            Ok(())
                        },
                        || ctx.session(&key).unwrap().save_dirty_composite(&input),
                    )
                },
            );
            (input, saved, prepare.calls, commits.get())
        })
        .unwrap();
    let id = save.id();
    drop(save);
    hold.entered();
    // worker 안에 설치한 commit TLS observer다. final reread/prepare 뒤에도 다른 job은 진입하지 않는다.
    fixture::same_pair(&f, &before);
    let progressed = Arc::new(AtomicUsize::new(0));
    let observation = client
        .try_submit(progressed.clone(), |_, count| {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    client.trigger(&key, TriggerReason::Foreground).unwrap();
    assert_eq!(progressed.load(Ordering::SeqCst), 0);
    assert!(client.try_take(&observation).unwrap().is_none());
    c.close_admission();
    drop(client);
    hold.release();
    wait_for(&c, |m| {
        matches!(m.results.get(&id.serial), Some(Stored::Complete(_)))
    });
    type PairResult = (
        CompositeSaveInput,
        Result<
            crate::data::application::recovery_handoff::DirtySaveResult<
                crate::data::application::composite::CompositeSaveExecution,
            >,
            context::CompositeDispatchError,
        >,
        usize,
        usize,
    );
    let (_, saved, prepares, commits): PairResult = c.try_take(&id).unwrap().unwrap();
    let saved = saved.unwrap();
    assert_eq!(saved.custody.unwrap(), CustodyTransition::Active);
    assert_eq!(
        saved.original.execution.diagnostic().disk,
        DiskState::Committed
    );
    assert_eq!((prepares, commits), (1, 1));
    let (tb, db) = fixture::candidate_pair(&saved.original);
    let after = fixture::pair(&f);
    assert!(after.template.0 == tb && after.document.0 == db && after.sentinel == before.sentinel);
    assert!(after.template.0 != before.template.0 && after.document.0 != before.document.0);
    wait_validations(&c, &key, 1);
    assert_eq!(sink.lock().unwrap().calls, 0);
    result(&c, &observation);
    assert_eq!(progressed.load(Ordering::SeqCst), 1);
    let original = clean_active(&c, reg.registration.clone());
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn w7_admitted_source_is_not_refreshed_and_typed_input_remains_original() {
    let (f, mut c, client) = start::<(), ()>(3, 1);
    let setup = client
        .try_submit(ProbeOwner::default(), |ctx, provider| {
            fixture::add_historical(ctx.runtime.as_mut().unwrap());
            (register(ctx, provider), document_input(ctx, ()))
        })
        .unwrap();
    let (reg, input) = result(&c, &setup);
    let key = reg.key.unwrap();
    let token = input.document.token.clone();
    let template_token = input.template.token.clone();
    let revision = input.template.expected_revision;
    let input_time = input.timestamp_utc.clone();
    let (g, hold) = gate();
    let block = client.try_submit(g, |_, g| g.block()).unwrap();
    hold.entered();
    let save = client
        .try_submit((key, input), |ctx, (key, input)| {
            let saved = ctx.session(&key).unwrap().save_document(&input);
            (input, saved)
        })
        .unwrap();
    // admission 뒤 외부 변경. 원 bytes에 whitespace만 추가해 실제 source precondition을 바꾼다.
    let before = fixture::pair(&f);
    let mut changed = before.document.0.clone();
    changed.push(b' ');
    fs::write(f.document_path(), &changed).unwrap();
    let external = fixture::pair(&f);
    hold.release();
    result(&c, &block);
    let (input, saved) = result(&c, &save);
    let saved = saved.unwrap();
    assert!(input.document.token == token && input.template.token == template_token);
    assert_eq!(input.template.expected_revision, revision);
    assert!(input.timestamp_utc == input_time);
    assert!(input.edits == artifact::DocumentEditSet::new(vec![fixture::filled_edit()]));
    let Some(BodyOutcome::Rejected(error)) = saved.execution.body() else {
        panic!("stale domain rejection");
    };
    assert!(matches!(
        error.domain_cause(),
        Some(DocumentUpdateError::DocumentSourceMismatch)
    ));
    assert!(saved.outcome().is_none());
    fixture::same_pair(&f, &external);
    assert!(external.template == before.template && external.sentinel == before.sentinel);
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn w8_provider_loss_preserves_p_unconnected_gate_and_real_receipt_after_cancel() {
    let (f, mut c, client) = start::<Payload, Receipt>(4, 1);
    let provider = ProbeOwner::default();
    let sink = SinkOwner::default();
    let (p, proof) = payload();
    let init = client
        .try_submit((provider.clone(), p), |ctx, (provider, p)| {
            fixture::add_historical(ctx.runtime.as_mut().unwrap());
            let reg = register(ctx, provider);
            ctx.session(reg.key.as_ref().unwrap())
                .unwrap()
                .bind_draft(p)
                .unwrap();
            (reg, document_input(ctx, ()))
        })
        .unwrap();
    let (reg, input) = result(&c, &init);
    let key = reg.key.unwrap();
    let disk = fixture::pair(&f);
    let (p2, proof2) = payload();
    let occupied = client
        .try_submit((key.clone(), p2), |ctx, (key, p)| {
            ctx.session(&key).unwrap().bind_draft(p)
        })
        .unwrap();
    let occupied = result(&c, &occupied).unwrap_err();
    proof2.check(&occupied.input);
    assert_eq!(occupied.category, WorkerCategory::OwnersRemain);
    assert!(occupied.original.is_none());
    let text = format!("{occupied:?}");
    assert!(!text.contains(SECRET));
    drop(occupied);
    assert_eq!(proof2.drops.load(Ordering::SeqCst), 1);
    let denied = client
        .try_submit((key.clone(), input), |ctx, (key, input)| {
            let denied = ctx.session(&key).unwrap().save_dirty_document(&input);
            (input, denied)
        })
        .unwrap();
    let (input, denied) = result(&c, &denied);
    assert_eq!(
        denied.unwrap_err().sink_category(),
        Some(RecoverySinkFailureCategory::Unavailable)
    );
    fixture::same_pair(&f, &disk);
    provider.lock().unwrap().fail = true;
    client.trigger(&key, TriggerReason::OsResume).unwrap();
    wait_validations(&c, &key, 1);
    let failure = c.take_validation_failure(&key).unwrap();
    let diagnostic = format!("{failure:?} {}", failure.original);
    for secret in [
        SECRET,
        RECEIPT,
        "provider-token-sentinel",
        &f.root.to_string_lossy(),
    ] {
        assert!(!diagnostic.contains(secret));
    }
    assert_eq!(
        failure.original.operation(),
        EditSessionOperation::Revalidate
    );
    assert!(
        matches!(&failure.original, EditSessionError::Validation { source: ValidationErrorSource::Lock(e), .. } if e.category() == LockErrorCategory::LockLost)
    );
    assert_eq!(
        failure.preserve.unwrap().unwrap(),
        CustodyTransition::Preserved
    );
    assert!(failure.reasons.contains(TriggerReason::OsResume));
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Inactive
    );
    let checked = client
        .try_submit((key.clone(), input), |ctx, (key, input)| {
            let entry = ctx.current(&key).unwrap();
            assert!(matches!(
                entry.service.retained_failure(),
                Some(crate::data::edit_session::RetainedFailure::Recovery {
                    reason: crate::data::edit_session::RecoveryReason::LockLost,
                    validation_failure: Some(_),
                    ..
                })
            ));
            let mut work = ctx.session(&key).unwrap();
            assert_eq!(work.snapshot().state(), EditSessionState::RecoveryRequired);
            let saved = work.save_document(&input).unwrap();
            assert_eq!(
                saved.execution.result.unwrap_err().category(),
                ApplicationCategory::InvalidSessionState
            );
            assert!(work.accept().is_err());
        })
        .unwrap();
    result(&c, &checked);
    assert_eq!(count(&provider, "acquire"), 1);
    assert_eq!(count(&provider, "validate"), 1);
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Ready);
    result(
        &c,
        &client
            .try_submit((key.clone(), sink.clone()), connect)
            .unwrap(),
    );
    sink.lock().unwrap().fail_next = true;
    let failed = client
        .try_submit(key.clone(), |ctx, key| ctx.session(&key).unwrap().accept())
        .unwrap();
    assert_eq!(
        result(&c, &failed).unwrap_err().sink_category(),
        Some(RecoverySinkFailureCategory::DurabilityUncertain)
    );
    assert_eq!(sink.lock().unwrap().calls, 1);
    assert!(sink.lock().unwrap().accepted.is_none());
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    let accepted = client
        .try_submit(key.clone(), |ctx, key| ctx.session(&key).unwrap().accept())
        .unwrap();
    result(&c, &accepted).unwrap();
    proof.check(sink.lock().unwrap().accepted.as_ref().unwrap());
    assert_eq!(sink.lock().unwrap().calls, 2);
    c.close_admission();
    drop(client);
    let ack = c
        .try_cleanup(reg.registration.clone(), |ctx, reg| {
            ctx.end_session(&reg).unwrap().unwrap();
            ctx.acknowledge(&reg).unwrap().unwrap()
        })
        .unwrap();
    let id = ack.id();
    wait_for(&c, |m| {
        matches!(m.control_result, Some((_, Stored::Complete(_))))
    });
    drop(ack);
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 0);
    let ack: Acknowledged<Receipt> = c.try_take(&id).unwrap().unwrap();
    check_receipt(&ack, &sink);
    assert_eq!(
        ack.before.handoff,
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(ack.after.session, EditSessionState::ReadOnly);
    assert!(ack.after.receipt_returned);
    assert!(ack.after.normal_exit_allowed);
    // release 뒤 ack가 identity를 지웠다. 옛 key를 새 세션으로 취급하지 않는다.
    assert_eq!(c.revalidation(&key).unwrap_err(), WorkerCategory::Stale);
    fixture::same_pair(&f, &disk);
    let remove = c
        .try_cleanup(reg.registration, |ctx, reg| {
            ctx.remove_session(&reg).unwrap();
        })
        .unwrap();
    result(&c, &remove);
    drop(ack);
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 1);
    let accepted_p = sink.lock().unwrap().accepted.take().unwrap();
    proof.check(&accepted_p);
    drop(accepted_p);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    finish(&mut c, vec![]);
}

#[test]
fn w4_target_change_queued_old_key_and_foreign_worker_are_stale() {
    let (_f, mut c, client) = start::<(), ()>(3, 2);
    let owner = ProbeOwner::default();
    let reg = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let old = reg.key.unwrap();
    let (g, hold) = gate();
    let change = client
        .try_submit((old.clone(), g), |ctx, (old, g)| {
            g.block();
            let targets = ctx.session(&old).unwrap().snapshot().targets().to_vec();
            ctx.session(&old)
                .unwrap()
                .change_targets(targets)
                .unwrap()
                .unwrap();
            ctx.session_key(&old.registration).unwrap()
        })
        .unwrap();
    hold.entered();
    client.trigger(&old, TriggerReason::Foreground).unwrap();
    hold.release();
    let new = result(&c, &change);
    assert!(new != old);
    result(&c, &client.try_submit((), |_, ()| ()).unwrap());
    assert_eq!(count(&owner, "validate"), 0);
    assert_eq!(count(&owner, "acquire"), 2);
    assert_eq!(
        client.trigger(&old, TriggerReason::OsResume).unwrap_err(),
        WorkerCategory::Stale
    );
    client.trigger(&new, TriggerReason::Foreground).unwrap();
    wait_validations(&c, &new, 1);
    assert_eq!(count(&owner, "validate"), 1);
    let (_foreign_f, mut foreign, other) = start::<(), ()>(2, 1);
    assert_eq!(
        other.trigger(&new, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Stale
    );
    let stale = other
        .try_submit(new.clone(), |ctx, key| ctx.session(&key).err().unwrap())
        .unwrap();
    assert_eq!(result(&foreign, &stale), WorkerCategory::Stale);
    finish(&mut foreign, vec![]);
    finish(&mut c, vec![reg.registration]);
    let (mut reopened, new_client) = WorkerControl::<(), ()>::start(config(&_f, 2, 1)).unwrap();
    wait_for(&reopened, |m| m.status != WorkerStatus::Starting);
    let new_owner = ProbeOwner::default();
    let new_reg = result(
        &reopened,
        &new_client.try_submit(new_owner.clone(), register).unwrap(),
    );
    assert!(new_reg.key.as_ref().unwrap().project == new.project);
    assert_eq!(
        new_client
            .trigger(&new, TriggerReason::Foreground)
            .unwrap_err(),
        WorkerCategory::Stale
    );
    assert_eq!(count(&new_owner, "validate"), 0);
    finish(&mut reopened, vec![new_reg.registration]);
}

#[test]
fn w9_worker_local_non_send_payload_and_safe_observations() {
    let (f, mut c, client) = start::<Rc<Cell<usize>>, Rc<Cell<usize>>>(2, 1);
    let reg = result(
        &c,
        &client
            .try_submit(ProbeOwner::default(), |ctx, owner| {
                let reg = register(ctx, owner);
                ctx.session(reg.key.as_ref().unwrap())
                    .unwrap()
                    .bind_draft(Rc::new(Cell::new(17)))
                    .unwrap();
                reg
            })
            .unwrap(),
    );
    let text = format!(
        "{:?} {:?} {:?} {}",
        c.snapshot(),
        reg.key,
        reg.registration,
        WorkerCategory::Full
    );
    for secret in [
        SECRET,
        RECEIPT,
        "G9 private editor payload",
        "provider-token-sentinel",
        &f.root.to_string_lossy(),
    ] {
        assert!(!text.contains(secret));
    }
    c.close_admission();
    let t = c
        .try_cleanup(reg.registration.clone(), |ctx, reg| {
            let p = ctx.return_active(&reg).unwrap();
            assert_eq!(p.get(), 17);
            assert_eq!(Rc::strong_count(&p), 1);
            drop(p);
        })
        .unwrap();
    result(&c, &t);
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn w4_w8_lock_lost_without_draft_and_pending_runtime_never_auto_resume() {
    let (_f, mut c, client) = start::<(), ()>(3, 1);
    let owner = ProbeOwner::default();
    let reg = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let key = reg.key.unwrap();
    result(
        &c,
        &client
            .try_submit((), |ctx, ()| {
                ctx.runtime.as_mut().unwrap().invalidate_recovery()
            })
            .unwrap(),
    );
    client.trigger(&key, TriggerReason::Foreground).unwrap();
    wait_validations(&c, &key, 1);
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Pending);
    owner.lock().unwrap().fail = true;
    client.trigger(&key, TriggerReason::OsResume).unwrap();
    wait_validations(&c, &key, 2);
    let blocked = client
        .try_submit(key.clone(), |ctx, key| {
            let mut work = ctx.session(&key).unwrap();
            let targets = work.snapshot().targets().to_vec();
            work.change_targets(targets).unwrap_err()
        })
        .unwrap();
    assert_eq!(result(&c, &blocked), WorkerCategory::OwnersRemain);
    let failure = c.take_validation_failure(&key).unwrap();
    assert!(failure.preserve.is_none());
    let state = result(
        &c,
        &client
            .try_submit(reg.registration.clone(), |ctx, reg| {
                ctx.session_snapshot(&reg).unwrap()
            })
            .unwrap(),
    );
    assert_eq!(state.state(), EditSessionState::LockLost);
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Inactive
    );
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Pending);
    assert_eq!(count(&owner, "acquire"), 1);
    assert_eq!(count(&owner, "validate"), 2);
    finish(&mut c, vec![reg.registration]);
}

#[test]
fn w5_recovery_initialization_failure_retains_blocked_runtime_for_control() {
    let f = fixture::Fixture::new();
    fs::write(f.root.join(".worldbuild"), b"G11 recovery evidence").unwrap();
    let (mut c, client) = WorkerControl::<(), ()>::start(config(&f, 2, 1)).unwrap();
    wait_for(&c, |m| m.status != WorkerStatus::Starting);
    assert_eq!(c.snapshot().status, WorkerStatus::InitializationFailed);
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Blocked);
    let (p, proof) = payload();
    let denied = client.try_submit(p, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::InitializationFailed);
    proof.check(&denied.input);
    let cleanup = c
        .try_cleanup((), |ctx, ()| ctx.close_runtime().unwrap())
        .unwrap();
    assert_eq!(result(&c, &cleanup).unwrap().state, RuntimeState::Blocked);
    assert!(fs::read(f.root.join(".worldbuild")).unwrap() == b"G11 recovery evidence");
    c.request_stop().unwrap();
    wait_for(&c, |m| m.status == WorkerStatus::Stopped);
    let end = Instant::now() + LIMIT;
    while !c.try_join().unwrap() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    drop(denied);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn w5_w9_request_counter_exhaustion_returns_original_without_wrap() {
    let (_f, mut c, client) = start::<(), ()>(2, 1);
    // 검증용 private counter seam이다. 제품 입력에는 counter 지정 API가 없다.
    c.shared.lock().next_id = u64::MAX;
    let (p, proof) = payload();
    let denied = client.try_submit(p, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Exhausted);
    proof.check(&denied.input);
    assert_eq!(c.snapshot().ordinary_outstanding, 0);
    // G12 제어 경계도 wrap하지 않는다. 테스트 정리를 위해 실제 미사용 serial만 복원한다.
    c.shared.lock().next_id = 0;
    finish(&mut c, vec![]);
}

#[test]
fn w5_w6_unexpected_panic_is_uncertain_and_keeps_earlier_owned_result() {
    let (_f, mut c, client) = start::<(), ()>(3, 1);
    // 자원은 명시적으로 닫은 뒤 panic seam을 검사한다. 임의 panic의 P 무손실은 주장하지 않는다.
    let (p, proof) = payload();
    let saved = client.try_submit(p, |_, p| p).unwrap();
    let saved_id = saved.id();
    drop(saved);
    wait_for(&c, |m| {
        matches!(m.results.get(&saved_id.serial), Some(Stored::Complete(_)))
    });
    c.close_admission();
    let close = c
        .try_cleanup((), |ctx, ()| ctx.close_runtime().unwrap().unwrap())
        .unwrap();
    result(&c, &close);
    let panic = c
        .try_cleanup((), |_, ()| -> () {
            panic!("G11 controlled worker fault");
        })
        .unwrap();
    wait_for(&c, |m| m.status == WorkerStatus::Unavailable);
    assert_eq!(
        c.try_take::<()>(&panic.id()).unwrap_err(),
        WorkerCategory::Unavailable
    );
    assert_eq!(
        client.try_submit((), |_, ()| ()).unwrap_err().category,
        WorkerCategory::Unavailable
    );
    let original: Payload = c.try_take(&saved_id).unwrap().unwrap();
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    let end = Instant::now() + LIMIT;
    while !c.try_join().unwrap() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}

#[test]
fn w2_reason_union_does_not_consume_events_after_execution_start() {
    let mut s = Reservation::default();
    assert_eq!(
        s.trigger(TriggerReason::Foreground),
        TriggerAccepted::Queued
    );
    for _ in 0..100 {
        assert_eq!(
            s.trigger(TriggerReason::OsResume),
            TriggerAccepted::Coalesced
        );
    }
    let first = s.start();
    assert!(first.contains(TriggerReason::Foreground) && first.contains(TriggerReason::OsResume));
    assert!(!first.contains(TriggerReason::Periodic));
    assert_eq!(
        s.trigger(TriggerReason::Periodic),
        TriggerAccepted::FollowUp
    );
    assert_eq!(
        s.trigger(TriggerReason::OsResume),
        TriggerAccepted::FollowUp
    );
    assert!(s.finish());
    assert_eq!(s.phase, Phase::Queued);
    let second = s.start();
    assert!(second.contains(TriggerReason::Periodic) && second.contains(TriggerReason::OsResume));
    assert!(!second.contains(TriggerReason::Foreground));
    assert!(!s.finish());
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(
        s.trigger(TriggerReason::Foreground),
        TriggerAccepted::Queued
    );
    assert!(s.start().contains(TriggerReason::Foreground));
    assert!(!s.finish());
}
