use super::*;
use crate::data::{
    application::{
        composite::tests as fixture,
        diagnostics::{ApplicationCategory, DiskState},
        templates::TemplateEditIntent,
        write::TransactionResult,
    },
    artifact::{self, DocumentEdit, DocumentSaveOutcomeKind, DocumentValueEdit},
    collaboration_lock::{LockProviderKind, NoLockService},
    edit_session::{EditSessionErrorCategory, RecoveryReason},
    transaction::test_support::{
        with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint,
    },
};
use fixture::{context, did, field, filled_edit, load_input, required_intent, seeded_historical};
use std::{
    cell::{Cell, RefCell},
    fs, io,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const SECRET: &str = "G10_PRIVATE_DRAFT_undo_selection_unfinished";
const RECEIPT: &str = "G10_PRIVATE_RECEIPT_custody_proof";

// Clone/Debug/Serialize/Default가 없는 실제 비어 있지 않은 원본과 receipt를 사용한다.
struct Payload {
    bytes: Vec<u8>,
    owner: Arc<()>,
    drops: Rc<Cell<usize>>,
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
struct Receipt {
    bytes: Vec<u8>,
    owner: Arc<()>,
    drops: Rc<Cell<usize>>,
}
impl Drop for Receipt {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
type Session = EditSessionService<Payload, Receipt>;
struct Proof {
    owner: Arc<()>,
    drops: Rc<Cell<usize>>,
    address: *const u8,
}
impl Proof {
    fn payload() -> (Payload, Self) {
        let owner = Arc::new(());
        let drops = Rc::new(Cell::new(0));
        let p = Payload {
            bytes: SECRET.as_bytes().to_vec(),
            owner: owner.clone(),
            drops: drops.clone(),
        };
        let proof = Self {
            owner,
            drops,
            address: p.bytes.as_ptr(),
        };
        (p, proof)
    }
    fn check(&self, p: &Payload) {
        assert!(p.bytes == SECRET.as_bytes());
        assert!(Arc::ptr_eq(&p.owner, &self.owner));
        assert_eq!(p.bytes.as_ptr(), self.address);
        assert_eq!(self.drops.get(), 0);
    }
    fn active(&self, draft: &DraftCustody<Payload>) {
        self.check(draft.active.as_ref().expect("active original"));
    }
}
#[derive(Default)]
struct SinkState {
    calls: usize,
    failures: Vec<RecoverySinkFailureCategory>,
    envelopes: Vec<RecoveryEnvelope>,
    accepted: Option<Payload>,
    receipt_owner: Option<Arc<()>>,
    receipt_drops: Rc<Cell<usize>>,
    payload_address: Option<*const u8>,
}
struct Sink(Rc<RefCell<SinkState>>);
impl DurableRecoverySink<Payload> for Sink {
    type Receipt = Receipt;
    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        payload: Payload,
    ) -> Result<Receipt, RecoverySinkFailure<Payload>> {
        let mut s = self.0.borrow_mut();
        s.calls += 1;
        s.envelopes.push(envelope.clone());
        assert!(payload.bytes == SECRET.as_bytes());
        if let Some(address) = s.payload_address {
            assert_eq!(payload.bytes.as_ptr(), address);
        }
        s.payload_address = Some(payload.bytes.as_ptr());
        if !s.failures.is_empty() {
            let category = s.failures.remove(0);
            return Err(RecoverySinkFailure::new(payload, category));
        }
        assert!(s.accepted.is_none());
        s.accepted = Some(payload);
        let owner = Arc::new(());
        s.receipt_owner = Some(owner.clone());
        Ok(Receipt {
            bytes: RECEIPT.as_bytes().to_vec(),
            owner,
            drops: s.receipt_drops.clone(),
        })
    }
}
fn backend() -> (RecoveryBackend<Payload, Receipt>, Rc<RefCell<SinkState>>) {
    let s = Rc::new(RefCell::new(SinkState::default()));
    (RecoveryBackend::connected(Sink(s.clone())), s)
}
fn begin(rt: &ProjectRuntime, targets: &[ProjectRelativePath]) -> Session {
    let mut s = Session::new(Arc::new(NoLockService::new()));
    s.begin_edit(rt.project_fingerprint(), targets.to_vec())
        .unwrap();
    s
}
fn draft(rt: &ProjectRuntime, s: &Session) -> (DraftCustody<Payload>, Proof) {
    let (p, proof) = Proof::payload();
    let d = DraftCustody::bind(rt, s, p).unwrap_or_else(|_| panic!("live identity binding"));
    (d, proof)
}
fn document_input(rt: &mut ProjectRuntime, edits: Vec<DocumentEdit>) -> SaveDocumentInput {
    let i = load_input(rt, required_intent(), edits);
    SaveDocumentInput {
        document: i.document,
        template: i.template,
        edits: i.edits,
        timestamp_utc: i.timestamp_utc,
    }
}
fn dtargets() -> Vec<ProjectRelativePath> {
    vec![ArtifactSourceId::Document(did()).path().unwrap()]
}
fn no_io(c: crate::data::transaction::test_support::PrepareCounts, k: usize) {
    assert_eq!((c.calls, c.allocations, k), (0, 0, 0));
}
fn domain<E>(e: &WriteExecution<(), E>) -> &E {
    let Some(BodyOutcome::Rejected(e)) = e.body() else {
        panic!("original domain result")
    };
    e.domain_cause().unwrap()
}
fn safe(value: impl fmt::Debug) {
    let text = format!("{value:?}");
    for secret in [
        SECRET,
        RECEIPT,
        "G10_PRIVATE_IO_CAUSE",
        "G9 private editor payload",
        "historical option",
        "1E100",
    ] {
        assert!(!text.contains(secret));
    }
}
fn safe_error(e: &HandoffError) {
    safe(e);
    safe(e.to_string());
    assert!(std::error::Error::source(e).is_none());
}
fn finish_pending(
    rt: &ProjectRuntime,
    s: &mut Session,
    d: &mut DraftCustody<Payload>,
    b: &mut RecoveryBackend<Payload, Receipt>,
    state: &Rc<RefCell<SinkState>>,
    proof: &Proof,
) {
    d.accept(rt, s, b).unwrap();
    proof.check(state.borrow().accepted.as_ref().unwrap());
    assert_eq!(
        s.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(state.borrow().receipt_drops.get(), 0);
    let ack = d.acknowledge(rt, s).unwrap();
    assert!(ack.receipt.bytes == RECEIPT.as_bytes());
    assert!(Arc::ptr_eq(
        &ack.receipt.owner,
        state.borrow().receipt_owner.as_ref().unwrap()
    ));
    assert_eq!(
        ack.before.handoff,
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(ack.after.handoff, Some(RecoveryHandoffStatus::Acknowledged));
    safe(&ack);
    drop(ack);
    assert_eq!(state.borrow().receipt_drops.get(), 1);
}

#[test]
fn h1_document_changed_then_fresh_no_write_retains_independent_candidate_warning_and_active_p() {
    let (f, mut rt) = seeded_historical();
    let input = document_input(&mut rt, vec![filled_edit()]);
    let mut s = begin(&rt, &dtargets());
    let snap = s.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (b, state) = backend();
    let before = fixture::pair(&f);
    let (r, c, k) = fixture::observe(|| {
        save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &input).unwrap()
    });
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
    let r = r.original.unwrap();
    assert_eq!(r.execution.diagnostic().disk, DiskState::Committed);
    let o = r.outcome().unwrap();
    fixture::warning(o);
    assert_eq!(o.kind(), DocumentSaveOutcomeKind::Changed);
    let actual = fixture::pair(&f);
    assert!(actual.document.0 == artifact::encode_document(o.document()).unwrap());
    fixture::preserved(&actual.template.0, &actual.document.0);
    let expected: serde_json::Value = serde_json::from_slice(&actual.document.0).unwrap();
    assert!(
        expected["fieldValues"][fixture::key(3)]
            == serde_json::json!({"kind":"text","value":"filled"})
    );
    assert_eq!(o.document().template_revision(), fixture::revision(4));
    assert!(actual.template == before.template && actual.sentinel == before.sentinel);
    p.active(&d);
    let fresh = document_input(&mut rt, vec![filled_edit()]);
    let (n, c, k) = fixture::observe(|| {
        save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &fresh).unwrap()
    });
    no_io(c, k);
    let n = n.original.unwrap();
    assert_eq!(n.execution.diagnostic().disk, DiskState::NoWrite);
    fixture::warning(n.outcome().unwrap());
    fixture::same_pair(&f, &actual);
    p.active(&d);
    assert_eq!(state.borrow().calls, 0);
    safe(&r);
    safe(&n);
    safe(&d);
    assert!(!d.snapshot(&rt, &s).unwrap().normal_exit_allowed);
    s.end_edit().unwrap();
    drop(d);
    assert_eq!(p.drops.get(), 1);
    rt.close().unwrap();
}

#[test]
fn h1_composite_single_plan_then_fresh_no_write_keeps_two_outcomes_and_p() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let request = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, request.session_targets());
    let snap = s.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (b, state) = backend();
    let before = fixture::pair(&f);
    let (r, c, k) = fixture::observe(|| {
        save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &request).unwrap()
    });
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
    let (tb, db) = fixture::candidate_pair(&r.original);
    let actual = fixture::pair(&f);
    assert!(
        actual.template.0 == tb && actual.document.0 == db && actual.sentinel == before.sentinel
    );
    p.active(&d);
    let fresh = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let req = CompositeWriteRequest::new(&fresh).unwrap();
    let (n, c, k) = fixture::observe(|| {
        save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap()
    });
    no_io(c, k);
    assert_eq!(n.original.execution.diagnostic().disk, DiskState::NoWrite);
    fixture::warning(n.original.document_outcome().unwrap());
    assert!(n.original.template_outcome().unwrap().changed().is_none());
    p.active(&d);
    fixture::same_pair(&f, &actual);
    assert_eq!(state.borrow().calls, 0);
    safe(&r.original);
    safe(&n);
    s.end_edit().unwrap();
    drop(d);
    assert_eq!(p.drops.get(), 1);
    rt.close().unwrap();
}

#[test]
fn h2_actual_g8_revision_raw_template_and_document_stale_preserve_before_prepare() {
    for case in ["revision", "raw-template", "raw-document"] {
        let (f, mut rt) = seeded_historical();
        let mut input = document_input(&mut rt, vec![filled_edit()]);
        if case == "revision" {
            input.template.expected_revision = fixture::revision(3);
        } else {
            let path = if case == "raw-template" {
                f.template_path()
            } else {
                f.document_path()
            };
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b' ');
            fs::write(path, bytes).unwrap();
        }
        let before = fixture::pair(&f);
        let mut s = begin(&rt, &dtargets());
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (mut b, state) = backend();
        let (r, c, k) = fixture::observe(|| {
            save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &input).unwrap()
        });
        no_io(c, k);
        assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
        let r = r.original.unwrap();
        assert!(matches!(
            (case, domain(&r.execution)),
            ("revision", DocumentUpdateError::TemplateRevisionMismatch)
                | ("raw-template", DocumentUpdateError::TemplateSourceMismatch)
                | ("raw-document", DocumentUpdateError::DocumentSourceMismatch)
        ));
        assert!(r.outcome().is_none());
        assert!(d.active.is_none());
        assert_eq!(p.drops.get(), 0);
        assert_eq!(state.borrow().calls, 0);
        assert_eq!(s.snapshot().state(), EditSessionState::RecoveryRequired);
        fixture::same_pair(&f, &before);
        finish_pending(&rt, &mut s, &mut d, &mut b, &state, &p);
        assert_eq!(
            state.borrow().envelopes[0].reason(),
            RecoveryReason::SaveRecoveryRequired
        );
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h2_actual_g9_stale_keeps_typed_inputs_and_source_cause_with_pending_p() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let request = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, request.session_targets());
    let snap = s.snapshot();
    let mut bytes = fs::read(f.template_path()).unwrap();
    bytes.push(b' ');
    fs::write(f.template_path(), bytes).unwrap();
    let (mut d, p) = draft(&rt, &s);
    let (mut b, state) = backend();
    let (r, c, k) = fixture::observe(|| {
        save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &request).unwrap()
    });
    no_io(c, k);
    assert!(matches!(
        domain(&r.original.execution),
        CompositeSaveError::Source(DocumentUpdateError::TemplateSourceMismatch)
    ));
    assert!(r.original.template_outcome().is_none() && r.original.document_outcome().is_none());
    assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
    assert!(matches!(
        input.intent,
        TemplateEditIntent::SetFieldRequired { required: true, .. }
    ));
    finish_pending(&rt, &mut s, &mut d, &mut b, &state, &p);
    s.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn h2_wrong_project_session_and_exact_targets_do_not_run_save_or_move_any_payload() {
    let (_f, mut rt) = seeded_historical();
    let (_other, other_rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let req = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, req.session_targets());
    let mut s2 = begin(&rt, req.session_targets());
    let mut ds = begin(&rt, &dtargets());
    let mut ts = begin(
        &rt,
        &[ArtifactSourceId::Template(fixture::tid()).path().unwrap()],
    );
    let snap = s.snapshot();
    let snap2 = s2.snapshot();
    let ds_snap = ds.snapshot();
    let ts_snap = ts.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (mut d2, p2) = draft(&rt, &ds);
    let (mut d3, p3) = draft(&rt, &ts);
    let (b, state) = backend();
    for case in 0..5 {
        let (r, c, k) = fixture::observe(|| match case {
            0 => save_dirty_composite(&mut rt, &mut s2, &b, &mut d, context(&snap), &req),
            1 => save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap2), &req),
            2 => save_dirty_composite(&mut rt, &mut ds, &b, &mut d2, context(&ds_snap), &req),
            3 => save_dirty_composite(&mut rt, &mut ts, &b, &mut d3, context(&ts_snap), &req),
            _ => save_dirty_composite(
                &mut rt,
                &mut s,
                &b,
                &mut d,
                TemplateWriteContext {
                    project: other_rt.project_fingerprint(),
                    session: snap.session_id().unwrap(),
                },
                &req,
            ),
        });
        no_io(c, k);
        assert!(
            matches!(r.unwrap_err(), HandoffError::Rejected(c) if c == match case {
                0 | 1 => HandoffCategory::SessionMismatch,
                2 | 3 => HandoffCategory::TargetsMismatch,
                _ => HandoffCategory::ProjectMismatch,
            })
        );
        p.active(&d);
        p2.active(&d2);
        p3.active(&d3);
        assert_eq!(s.snapshot(), snap);
        assert_eq!(s2.snapshot(), snap2);
    }
    assert!(d.preserve_existing(&other_rt, &mut s).is_err());
    assert!(d
        .accept(&rt, &mut s2, &mut RecoveryBackend::default())
        .is_err());
    assert!(d.acknowledge(&rt, &mut s2).is_err());
    assert_eq!(state.borrow().calls, 0);
    s.end_edit().unwrap();
    s2.end_edit().unwrap();
    ds.end_edit().unwrap();
    ts.end_edit().unwrap();
    rt.close().unwrap();
    other_rt.close().unwrap();
}

#[test]
fn h3_disconnected_blocks_both_new_dirty_saves_but_pending_runtime_can_preserve_existing() {
    for composite in [false, true] {
        let (_f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let req = CompositeWriteRequest::new(&input).unwrap();
        let document_targets = dtargets();
        let mut s = begin(
            &rt,
            if composite {
                req.session_targets()
            } else {
                &document_targets
            },
        );
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let mut b = RecoveryBackend::default();
        assert_eq!(b.capability(), SinkCapability::Unconnected);
        let di = document_input(&mut rt, vec![filled_edit()]);
        let (category, c, k) = fixture::observe(|| {
            if composite {
                save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap_err()
            } else {
                save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &di).unwrap_err()
            }
        });
        no_io(c, k);
        assert_eq!(
            category.sink_category(),
            Some(RecoverySinkFailureCategory::Unavailable)
        );
        safe_error(&category);
        p.active(&d);
        assert!(d.preserve_existing(&rt, &mut s).is_err());
        assert!(d.accept(&rt, &mut s, &mut b).is_err());
        assert!(d.acknowledge(&rt, &mut s).is_err());
        rt.invalidate_recovery();
        assert_eq!(
            d.preserve_existing(&rt, &mut s).unwrap(),
            CustodyTransition::Preserved
        );
        let e = d.accept(&rt, &mut s, &mut b).unwrap_err();
        assert_eq!(
            e.sink_category(),
            Some(RecoverySinkFailureCategory::Unavailable)
        );
        safe_error(&e);
        assert_eq!(
            s.snapshot().recovery_handoff(),
            Some(RecoveryHandoffStatus::Pending)
        );
        assert_eq!(p.drops.get(), 0);
        s.end_edit().unwrap();
        assert!(!d.snapshot(&rt, &s).unwrap().normal_exit_allowed);
        let (mut connected, state) = backend();
        finish_pending(&rt, &mut s, &mut d, &mut connected, &state, &p);
        rt.recover().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h4_sink_each_error_restores_same_p_explicit_retry_retains_r_until_one_ack() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let req = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, req.session_targets());
    let snap = s.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (mut b, state) = backend();
    let mut bytes = fs::read(f.document_path()).unwrap();
    bytes.push(b' ');
    fs::write(f.document_path(), bytes).unwrap();
    let r = save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap();
    assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
    let categories = [
        RecoverySinkFailureCategory::Unavailable,
        RecoverySinkFailureCategory::Rejected,
        RecoverySinkFailureCategory::DurabilityUncertain,
    ];
    state.borrow_mut().failures = categories.to_vec();
    for (n, cat) in categories.into_iter().enumerate() {
        let e = d.accept(&rt, &mut s, &mut b).unwrap_err();
        assert_eq!(e.sink_category(), Some(cat));
        safe_error(&e);
        assert_eq!(state.borrow().calls, n + 1);
        assert_eq!(state.borrow().payload_address, Some(p.address));
        assert_eq!(p.drops.get(), 0);
        assert_eq!(
            s.snapshot().recovery_handoff(),
            Some(RecoveryHandoffStatus::Pending)
        );
        assert!(d.acknowledge(&rt, &mut s).is_err());
        assert!(state.borrow().receipt_owner.is_none());
        assert!(d.active.is_none());
    }
    d.accept(&rt, &mut s, &mut b).unwrap();
    p.check(state.borrow().accepted.as_ref().unwrap());
    assert_eq!(state.borrow().calls, 4);
    assert_eq!(state.borrow().receipt_drops.get(), 0);
    assert!(d.accept(&rt, &mut s, &mut b).is_err());
    assert_eq!(state.borrow().calls, 4);
    let mut other = begin(&rt, req.session_targets());
    assert!(d.acknowledge(&rt, &mut other).is_err());
    let e = state.borrow();
    for env in &e.envelopes {
        assert!(env.project_fingerprint() == rt.project_fingerprint());
        assert_eq!(env.session_id(), snap.session_id().unwrap());
        assert_eq!(env.provider_kind(), LockProviderKind::None);
        assert_eq!(env.targets(), req.session_targets());
        assert_eq!(env.reason(), RecoveryReason::SaveRecoveryRequired);
    }
    drop(e);
    let ack = d.acknowledge(&rt, &mut s).unwrap();
    assert!(ack.receipt.bytes == RECEIPT.as_bytes());
    assert!(Arc::ptr_eq(
        &ack.receipt.owner,
        state.borrow().receipt_owner.as_ref().unwrap()
    ));
    assert!(!ack.after.normal_exit_allowed);
    assert!(d.acknowledge(&rt, &mut s).is_err());
    assert_eq!(state.borrow().receipt_drops.get(), 0);
    safe(&ack);
    drop(ack);
    assert_eq!(state.borrow().receipt_drops.get(), 1);
    s.end_edit().unwrap();
    other.end_edit().unwrap();
    drop(b);
    drop(state);
    assert_eq!(p.drops.get(), 1);
    rt.close().unwrap();
}

#[test]
fn h8_preserve_refusal_restores_p2_without_overwriting_pending_p1_or_first_result() {
    let (f, mut rt) = seeded_historical();
    let input = document_input(&mut rt, vec![filled_edit()]);
    let mut s = begin(&rt, &dtargets());
    let snap = s.snapshot();
    let (mut d1, p1) = draft(&rt, &s);
    let (mut b, state) = backend();
    let mut bytes = fs::read(f.document_path()).unwrap();
    bytes.push(b' ');
    fs::write(f.document_path(), bytes).unwrap();
    let original =
        save_dirty_document(&mut rt, &mut s, &b, &mut d1, context(&snap), &input).unwrap();
    assert!(matches!(
        domain(&original.original.as_ref().unwrap().execution),
        DocumentUpdateError::DocumentSourceMismatch
    ));
    let (mut d2, p2) = draft(&rt, &s);
    let error = d2.preserve_existing(&rt, &mut s).unwrap_err();
    let HandoffError::Preserve(e) = &error else {
        panic!("original preserve refusal owner")
    };
    assert_eq!(e.category(), EditSessionErrorCategory::InvalidState);
    safe_error(&error);
    p2.active(&d2);
    assert_eq!(p1.drops.get(), 0);
    assert!(d1.active.is_none());
    assert!(save_dirty_document(&mut rt, &mut s, &b, &mut d2, context(&snap), &input).is_err());
    assert!(d2.accept(&rt, &mut s, &mut b).is_err());
    assert!(d2.acknowledge(&rt, &mut s).is_err());
    assert_eq!(state.borrow().calls, 0);
    safe(&original);
    finish_pending(&rt, &mut s, &mut d1, &mut b, &state, &p1);
    p2.active(&d2);
    assert!(d2.preserve_existing(&rt, &mut s).is_err());
    p2.active(&d2);
    s.end_edit().unwrap();
    drop(d2);
    assert_eq!(p2.drops.get(), 1);
    rt.close().unwrap();
}

#[test]
fn h8_binding_refusal_returns_original_p_with_redacted_diagnostics_and_no_service_change() {
    let (_f, rt) = seeded_historical();
    let (_other, other_rt) = seeded_historical();
    let mut session = begin(&rt, &dtargets());
    let before = session.snapshot();
    let (payload, proof) = Proof::payload();
    let failure = DraftCustody::bind(&other_rt, &session, payload).unwrap_err();
    safe(&failure);
    safe(failure.to_string());
    assert!(std::error::Error::source(&failure).is_none());
    let (payload, error) = failure.into_parts();
    assert!(matches!(
        error,
        HandoffError::Rejected(HandoffCategory::ProjectMismatch)
    ));
    proof.check(&payload);
    assert_eq!(session.snapshot(), before);
    drop(payload);
    assert_eq!(proof.drops.get(), 1);
    session.end_edit().unwrap();
    rt.close().unwrap();
    other_rt.close().unwrap();
}

use crate::data::{
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockService,
    },
    edit_session::{RetainedFailure, ValidationFailureRef},
    transaction::{self, CommitResultState, PrepareFailPoint},
};
struct Provider {
    inner: NoLockService,
    calls: AtomicUsize,
    fail: AtomicUsize,
    releases: AtomicUsize,
}
impl Provider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: NoLockService::new(),
            calls: AtomicUsize::new(0),
            fail: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
        })
    }
}
impl LockService for Provider {
    fn provider_info(&self) -> LockProviderInfo {
        self.inner.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        self.inner.acquire(r)
    }
    fn validate(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.fail.load(Ordering::SeqCst) {
            Err(LockError::for_held(
                LockErrorCategory::LockLost,
                h.provider_kind(),
                LockOperation::Validate,
                h.project_fingerprint(),
                h.session_id(),
                h.target(),
            ))
        } else {
            self.inner.validate(h)
        }
    }
    fn release(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.inner.release(h)
    }
}

#[test]
fn h5_actual_provider_loss_before_body_and_during_prepare_retains_validation_cause_and_p() {
    for at in [1, 4] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let req = CompositeWriteRequest::new(&input).unwrap();
        let provider = Provider::new();
        let mut s = Session::new(provider.clone());
        s.begin_edit(rt.project_fingerprint(), req.session_targets().to_vec())
            .unwrap();
        provider.calls.store(0, Ordering::SeqCst);
        provider.fail.store(at, Ordering::SeqCst);
        let snap = s.snapshot();
        let before = fixture::pair(&f);
        let (mut d, p) = draft(&rt, &s);
        let (mut b, state) = backend();
        let (r, c, k) = fixture::observe(|| {
            save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap()
        });
        assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
        assert_eq!(
            r.original
                .execution
                .diagnostic()
                .permit
                .unwrap()
                .lock_category,
            Some(LockErrorCategory::LockLost)
        );
        if at == 1 {
            no_io(c, k);
            assert!(r.original.template_outcome().is_none());
        } else {
            assert_eq!((c.calls, c.allocations, k), (1, 0, 0));
            fixture::candidate_pair(&r.original);
        }
        let Some(RetainedFailure::Recovery {
            reason,
            validation_failure:
                Some(ValidationFailureRef::Save(
                    crate::data::collaboration_lock::WritePermitError::Validation(cause),
                )),
            ..
        }) = s.retained_failure()
        else {
            panic!("original validation retained in recovery")
        };
        assert_eq!(reason, RecoveryReason::LockLost);
        assert_eq!(cause.category(), LockErrorCategory::LockLost);
        assert_eq!(cause.session_id(), snap.session_id());
        assert!(cause.project_fingerprint() == Some(rt.project_fingerprint()));
        assert!(req.session_targets().contains(cause.target().unwrap()));
        let calls = provider.calls.load(Ordering::SeqCst);
        assert_eq!(calls, at);
        fixture::same_pair(&f, &before);
        safe(&r.original);
        finish_pending(&rt, &mut s, &mut d, &mut b, &state, &p);
        assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
        assert_eq!(
            state.borrow().envelopes[0].reason(),
            RecoveryReason::LockLost
        );
        s.end_edit().unwrap();
        assert_eq!(provider.releases.load(Ordering::SeqCst), 2);
        rt.close().unwrap();
    }
}

#[test]
fn h5_ordinary_g8_domain_errors_keep_active_payload_and_ready_without_handoff() {
    for case in ["time", "duplicate", "value"] {
        let (f, mut rt) = seeded_historical();
        let mut input = document_input(
            &mut rt,
            match case {
                "duplicate" => vec![filled_edit(), filled_edit()],
                "value" => vec![DocumentEdit::SetValue(
                    field(4),
                    DocumentValueEdit::single_choice(fixture::key(12).parse().unwrap()),
                )],
                _ => vec![filled_edit()],
            },
        );
        if case == "time" {
            input.timestamp_utc = "invalid".into();
        }
        let mut s = begin(&rt, &dtargets());
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (b, state) = backend();
        let before = fixture::pair(&f);
        let (r, c, k) = fixture::observe(|| {
            save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &input).unwrap()
        });
        no_io(c, k);
        assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
        let r = r.original.unwrap();
        let DocumentUpdateError::Save(e) = domain(&r.execution) else {
            panic!("pure save original")
        };
        assert_eq!(
            e.category(),
            match case {
                "time" => DocumentSaveErrorCategory::InvalidTimestamp,
                "duplicate" => DocumentSaveErrorCategory::DuplicateFieldEdit,
                _ => DocumentSaveErrorCategory::InvalidEditValue,
            }
        );
        p.active(&d);
        fixture::same_pair(&f, &before);
        assert_eq!(state.borrow().calls, 0);
        assert_eq!(s.snapshot().state(), EditSessionState::Editing);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h5_composite_partial_no_op_and_second_domain_failure_keep_first_and_dual_outcomes() {
    for partial in [false, true] {
        let f = fixture::Fixture::new();
        let (t, disk) = fixture::fixture_raw();
        f.seed(&t, &disk);
        let mut rt = f.runtime();
        let input = load_input(
            &mut rt,
            if partial {
                TemplateEditIntent::KeepCurrentDefault { field: field(4) }
            } else {
                TemplateEditIntent::SetName("changed".into())
            },
            if partial {
                vec![DocumentEdit::Rename("changed document".into())]
            } else {
                vec![DocumentEdit::SetValue(
                    field(4),
                    DocumentValueEdit::single_choice(fixture::key(12).parse().unwrap()),
                )]
            },
        );
        let req = CompositeWriteRequest::new(&input).unwrap();
        let mut s = begin(&rt, req.session_targets());
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (b, state) = backend();
        let before = fixture::pair(&f);
        let (r, c, k) = fixture::observe(|| {
            save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap()
        });
        no_io(c, k);
        assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
        let t = r.original.template_outcome().unwrap();
        if partial {
            assert!(t.changed().is_none());
            let doc = r.original.document_outcome().unwrap();
            fixture::warning(doc);
            assert_eq!(doc.kind(), DocumentSaveOutcomeKind::Changed);
            assert!(matches!(
                domain(&r.original.execution),
                CompositeSaveError::TemplateUnchangedDocumentChanged
            ));
        } else {
            assert!(t.changed().is_some());
            assert!(r.original.document_outcome().is_none());
            assert!(
                matches!(domain(&r.original.execution),CompositeSaveError::Document(e) if e.category()==DocumentSaveErrorCategory::InvalidEditValue)
            );
        }
        p.active(&d);
        assert_eq!(state.borrow().calls, 0);
        fixture::same_pair(&f, &before);
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h2_binding_and_invalid_artifact_are_not_guessed_to_be_stale() {
    for binding in [true, false] {
        let (f, mut rt) = seeded_historical();
        let mut input = document_input(&mut rt, vec![filled_edit()]);
        if binding {
            input.template.id = fixture::key(101).parse().unwrap();
        } else {
            fs::write(f.template_path(), b"invalid artifact").unwrap();
        }
        let mut s = begin(&rt, &dtargets());
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (b, state) = backend();
        let (r, c, k) = fixture::observe(|| {
            save_dirty_document(&mut rt, &mut s, &b, &mut d, context(&snap), &input).unwrap()
        });
        no_io(c, k);
        assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
        let original = r.original.unwrap();
        if binding {
            assert!(matches!(
                domain(&original.execution),
                DocumentUpdateError::TemplateBindingMismatch
            ));
        } else {
            assert_eq!(
                original.execution.diagnostic().category,
                Some(ApplicationCategory::RepositoryRejected)
            );
        }
        p.active(&d);
        assert_eq!(state.borrow().calls, 0);
        assert_eq!(s.snapshot().state(), EditSessionState::Editing);
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h2_late_actual_source_precondition_change_preserves_both_candidates_and_pending_p() {
    for missing in [false, true] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let req = CompositeWriteRequest::new(&input).unwrap();
        let mut s = begin(&rt, req.session_targets());
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (mut b, state) = backend();
        let path = f.template_path();
        let (r, c) = with_canonical_prepare_hooks(
            move |point, op| {
                if point == PrepareFailPoint::BeforeOriginalRead && op == Some(1) {
                    if missing {
                        fs::remove_file(&path)?;
                    } else {
                        let mut bytes = fs::read(&path)?;
                        bytes.push(b' ');
                        fs::write(&path, bytes)?;
                    }
                }
                Ok(())
            },
            || save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap(),
        );
        assert_eq!((c.calls, c.allocations), (1, 1));
        assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
        fixture::candidate_pair(&r.original);
        assert_eq!(
            r.original
                .execution
                .diagnostic()
                .artifact_write
                .unwrap()
                .category,
            if missing {
                ArtifactWriteCategory::SourceMissing
            } else {
                ArtifactWriteCategory::SourceMismatch
            }
        );
        assert_eq!(
            r.original.execution.diagnostic().disk,
            DiskState::NotApplied
        );
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        finish_pending(&rt, &mut s, &mut d, &mut b, &state, &p);
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[derive(Debug)]
struct OwnedCause(Arc<()>);
impl fmt::Display for OwnedCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("G10_PRIVATE_IO_CAUSE")
    }
}
impl std::error::Error for OwnedCause {}
fn io_owner(error: &io::Error, expected: &Arc<()>) {
    let owned = transaction::io_cause(error)
        .get_ref()
        .unwrap()
        .downcast_ref::<OwnedCause>()
        .unwrap();
    assert!(Arc::ptr_eq(&owned.0, expected));
}

#[test]
fn h6_clean_prepare_failure_retains_original_io_and_candidates_without_forcing_handoff() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let req = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, req.session_targets());
    let snap = s.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (b, state) = backend();
    let before = fixture::pair(&f);
    let cause = Arc::new(());
    let injected = cause.clone();
    let (r, c) = with_canonical_prepare_hooks(
        move |point, _| {
            if point == PrepareFailPoint::StagedWrite {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    OwnedCause(injected.clone()),
                ))
            } else {
                Ok(())
            }
        },
        || save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap(),
    );
    assert_eq!((c.calls, c.allocations), (1, 1));
    assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
    fixture::candidate_pair(&r.original);
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::PrepareFailed(error),
        ..
    }) = r.original.execution.body()
    else {
        panic!("prepare cause owner")
    };
    io_owner(
        error
            .implementation_original_prepare()
            .unwrap()
            .implementation_original_io()
            .unwrap(),
        &cause,
    );
    assert_eq!(
        r.original.execution.diagnostic().disk,
        DiskState::NotApplied
    );
    p.active(&d);
    fixture::same_pair(&f, &before);
    assert_eq!(state.borrow().calls, 0);
    safe(&r.original);
    drop(r.original);
    assert_eq!(Arc::strong_count(&cause), 1);
    s.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn h6_actual_partial_apply_confirmed_rollback_keeps_old_disk_original_io_and_active_p() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let req = CompositeWriteRequest::new(&input).unwrap();
    let mut s = begin(&rt, req.session_targets());
    let snap = s.snapshot();
    let (mut d, p) = draft(&rt, &s);
    let (b, state) = backend();
    let before = fixture::pair(&f);
    let cause = Arc::new(());
    let injected = cause.clone();
    let dp = f.document_path();
    let tp = f.template_path();
    let old = before.clone();
    let hits = Rc::new(Cell::new(0));
    let observed = hits.clone();
    let r = with_commit_io_factory(
        move |point, _| {
            if point == Some(CommitTestPoint::ProgressState) {
                assert!(fs::read(&dp)? != old.document.0 && fs::read(&tp)? == old.template.0);
                observed.set(observed.get() + 1);
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    OwnedCause(injected.clone()),
                ));
            }
            Ok(())
        },
        || save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap(),
    );
    assert_eq!(hits.get(), 1);
    assert_eq!(r.custody.unwrap(), CustodyTransition::Active);
    fixture::candidate_pair(&r.original);
    assert_eq!(
        r.original.execution.diagnostic().disk,
        DiskState::RolledBack
    );
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = r.original.execution.body()
    else {
        panic!("confirmed rollback original")
    };
    let (first, cleanup) = outcome
        .implementation_original()
        .rollback_failures()
        .unwrap();
    assert!(cleanup.is_none());
    let transaction::CommitFailureSource::Io(io) = first.source.as_ref() else {
        panic!("original I/O")
    };
    io_owner(io, &cause);
    fixture::same_old_bytes(&f, &before);
    p.active(&d);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(s.snapshot().state(), EditSessionState::Editing);
    assert_eq!(state.borrow().calls, 0);
    safe(&r.original);
    drop(r.original);
    assert_eq!(Arc::strong_count(&cause), 1);
    s.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn h6_h7_actual_cleanup_os32_retains_new_pair_pending_p_then_separate_recovery_and_receipt() {
    use std::os::windows::fs::OpenOptionsExt;
    for recover_first in [false, true] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let req = CompositeWriteRequest::new(&input).unwrap();
        let provider = Provider::new();
        let mut s = Session::new(provider.clone());
        s.begin_edit(rt.project_fingerprint(), req.session_targets().to_vec())
            .unwrap();
        let snap = s.snapshot();
        let (mut d, p) = draft(&rt, &s);
        let (mut b, state) = backend();
        let held = Rc::new(RefCell::new(None));
        let holder = held.clone();
        let root = f.root.join(".worldbuild/transactions");
        let hits = Rc::new(Cell::new(0));
        let seen = hits.clone();
        let r = with_commit_io_factory(
            move |point, _| {
                if point == Some(CommitTestPoint::Cleanup) {
                    let entries = fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
                    assert_eq!(entries.len(), 1);
                    let journal = entries[0].path();
                    assert!(journal.join("committed.json").exists());
                    *holder.borrow_mut() = Some(
                        fs::OpenOptions::new()
                            .read(true)
                            .share_mode(1)
                            .open(journal.join("manifest.json"))?,
                    );
                    seen.set(seen.get() + 1);
                }
                Ok(())
            },
            || save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req).unwrap(),
        );
        assert_eq!(hits.get(), 1);
        assert_eq!(r.custody.unwrap(), CustodyTransition::Preserved);
        assert_eq!(state.borrow().calls, 0);
        let (tb, db) = fixture::candidate_pair(&r.original);
        let actual = fixture::pair(&f);
        assert!(actual.template.0 == tb && actual.document.0 == db);
        let diag = r.original.execution.diagnostic();
        assert_eq!(diag.disk, DiskState::Committed);
        assert_eq!(
            diag.artifact_commit.unwrap().state,
            CommitResultState::CommittedCleanupFailed
        );
        let Some(BodyOutcome::Write {
            transaction: TransactionResult::Commit(Ok(o)),
            ..
        }) = r.original.execution.body()
        else {
            panic!("cleanup original")
        };
        let transaction::CommitOutcome::CommittedCleanupFailed {
            failure,
            cleanup_failure,
        } = o.implementation_original()
        else {
            panic!("confirmed new with cleanup failure")
        };
        let transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
            panic!("OS cause owner")
        };
        assert_eq!(
            transaction::artifact_diagnostics::IoDiagnostic::new(io).os_code,
            Some(32)
        );
        assert!(cleanup_failure.is_none());
        let io_address = io as *const _;
        assert_eq!(rt.snapshot().state, RuntimeState::Pending);
        assert!(!d.snapshot(&rt, &s).unwrap().normal_exit_allowed);
        let (blocked, c, k) = fixture::observe(|| {
            save_dirty_composite(&mut rt, &mut s, &b, &mut d, context(&snap), &req)
        });
        no_io(c, k);
        assert!(blocked.is_err());
        s.end_edit().unwrap();
        assert_eq!(provider.releases.load(Ordering::SeqCst), 2);
        assert_eq!(s.snapshot().state(), EditSessionState::RecoveryRequired);
        let released = d.snapshot(&rt, &s).unwrap();
        assert_eq!(released.handoff, Some(RecoveryHandoffStatus::Pending));
        assert!(!released.normal_exit_allowed);
        drop(held.borrow_mut().take());
        if recover_first {
            rt.recover().unwrap();
            assert_eq!(
                s.snapshot().recovery_handoff(),
                Some(RecoveryHandoffStatus::Pending)
            );
            assert_eq!(state.borrow().calls, 0);
        }
        d.accept(&rt, &mut s, &mut b).unwrap();
        assert_eq!(state.borrow().receipt_drops.get(), 0);
        p.check(state.borrow().accepted.as_ref().unwrap());
        assert_eq!(
            rt.snapshot().state,
            if recover_first {
                RuntimeState::Ready
            } else {
                RuntimeState::Pending
            }
        );
        let ack = d.acknowledge(&rt, &mut s).unwrap();
        assert!(ack.receipt.bytes == RECEIPT.as_bytes());
        assert_eq!(ack.after.normal_exit_allowed, recover_first);
        assert_eq!(ack.after.session, EditSessionState::ReadOnly);
        if !recover_first {
            assert_eq!(rt.snapshot().state, RuntimeState::Pending);
            rt.recover().unwrap();
        }
        fixture::same_pair(&f, &actual);
        assert_eq!(s.snapshot().state(), EditSessionState::ReadOnly);
        // 새 session과 실제 fresh source 요청은 별도 동작이다. old token은 G2 후에도 stale다.
        s.begin_edit(rt.project_fingerprint(), req.session_targets().to_vec())
            .unwrap();
        let fresh_snap = s.snapshot();
        let (stale, c, k) = fixture::observe(|| {
            composite::update_template_and_save_document(
                &mut rt,
                &mut s,
                context(&fresh_snap),
                &req,
            )
        });
        no_io(c, k);
        assert!(matches!(
            domain(&stale.execution),
            CompositeSaveError::Source(DocumentUpdateError::DocumentSourceMismatch)
        ));
        let fresh = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        assert!(
            fresh.template.token != input.template.token
                && fresh.document.token != input.document.token
        );
        let fresh_req = CompositeWriteRequest::new(&fresh).unwrap();
        let (new, c, k) = fixture::observe(|| {
            composite::update_template_and_save_document(
                &mut rt,
                &mut s,
                context(&fresh_snap),
                &fresh_req,
            )
        });
        no_io(c, k);
        assert_eq!(new.execution.diagnostic().disk, DiskState::NoWrite);
        fixture::warning(new.document_outcome().unwrap());
        fixture::same_pair(&f, &actual);
        fixture::candidate_pair(&r.original);
        let Some(BodyOutcome::Write {
            transaction: TransactionResult::Commit(Ok(o)),
            ..
        }) = r.original.execution.body()
        else {
            panic!("retained cleanup")
        };
        let transaction::CommitOutcome::CommittedCleanupFailed { failure, .. } =
            o.implementation_original()
        else {
            panic!("retained failure")
        };
        let transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
            panic!("retained io")
        };
        assert!(std::ptr::eq(io, io_address));
        safe(&r.original);
        safe(&ack);
        drop(ack);
        assert_eq!(state.borrow().receipt_drops.get(), 1);
        s.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn h3_actual_blocked_runtime_and_preexisting_lock_lost_preserve_without_connected_sink() {
    for blocked in [false, true] {
        let (f, mut rt) = seeded_historical();
        let provider = Provider::new();
        let mut s = Session::new(provider.clone());
        s.begin_edit(rt.project_fingerprint(), dtargets()).unwrap();
        let (mut d, p) = draft(&rt, &s);
        if blocked {
            fs::create_dir_all(f.root.join(".worldbuild/transactions/invalid-journal")).unwrap();
            assert!(rt.recover().is_err());
            assert_eq!(rt.snapshot().state, RuntimeState::Blocked);
        } else {
            provider.calls.store(0, Ordering::SeqCst);
            provider.fail.store(1, Ordering::SeqCst);
            assert!(s.revalidate().is_err());
            assert_eq!(s.snapshot().state(), EditSessionState::LockLost);
        }
        d.preserve_existing(&rt, &mut s).unwrap();
        let mut unavailable = RecoveryBackend::default();
        assert_eq!(
            d.accept(&rt, &mut s, &mut unavailable)
                .unwrap_err()
                .sink_category(),
            Some(RecoverySinkFailureCategory::Unavailable)
        );
        assert_eq!(p.drops.get(), 0);
        s.end_edit().unwrap();
        assert!(!d.snapshot(&rt, &s).unwrap().normal_exit_allowed);
        let (mut b, state) = backend();
        finish_pending(&rt, &mut s, &mut d, &mut b, &state, &p);
        rt.close().unwrap();
    }
}
