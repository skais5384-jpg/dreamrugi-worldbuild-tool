//! 복원 응답뿐 아니라 실제 파일·receipt·owner 수명을 guarded IPC로 대조한다.
use super::*;
use crate::data::edit_recovery::model::{Deposit, Key};
use std::sync::atomic::{AtomicBool, Ordering};

fn owners(h: &Harness) -> usize {
    h.state.lock().workspace_owners.load(Ordering::Acquire)
}
fn select(h: &Harness, s: &Value) -> Value {
    let r = &s["receipt"];
    let selected = h.work(json!({"kind":"recovery_read","key":r["key"],"deposit_id":r["depositId"],"digest":r["digest"]}));
    assert_eq!(selected["kind"], "recovery_selection", "{selected}");
    selected["selection"]["snapshot"].clone()
}
fn restore_input(p: &str, snapshot: &Value) -> Value {
    json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":null})
}
fn deposited_bytes(h: &Harness, s: &Value) -> Vec<u8> {
    let r = &s["receipt"];
    let key: Key = serde_json::from_value(r["key"].clone()).unwrap();
    let store = h.state.recovery.connect().unwrap();
    let record = store
        .lock()
        .unwrap()
        .read(&key, r["depositId"].as_str().unwrap())
        .unwrap();
    assert_eq!(record.payload_digest(), r["digest"].as_str().unwrap());
    record.bytes().to_vec()
}
fn invalid_body(h: &Harness, p: &str, s: &Value) -> Value {
    let mut body = content(h, p, s)["body"].clone();
    let mut choice = number();
    choice["id"] = "new:cccccccc-cccc-4ccc-8ccc-cccccccccccc".into();
    choice["configuration"] = json!({"kind":"single_choice","options":[{"id":"new:dddddddd-dddd-4ddd-8ddd-dddddddddddd","label":"선택","archived":false}]});
    choice["default"] = json!({"intent":"unset"});
    body["name"] = "과거 초안".into();
    body["fields"] = json!([number(), choice]);
    body["fields"][0]["default"]["value"]["value"] = "-".into();
    body
}

#[test]
fn past_latest_repeated_restore_redeposit_preserves_raw_identities_and_all_files() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let old_body = invalid_body(&h, &p, &s);
    let d7 = submit(&h, &p, &s, "7", &old_body, "deposit");
    let mut newer = old_body.clone();
    newer["name"] = "새 초안".into();
    let d8 = submit(&h, &p, &s, "8", &newer, "deposit");
    let preserved = [
        (&d7, deposited_bytes(&h, &d7)),
        (&d8, deposited_bytes(&h, &d8)),
    ];
    assert_eq!(release(&h, &p, &d8, false)["kind"], "control");
    for (selected, expected, generation) in [
        (&d7, &old_body, "9"),
        (&d8, &newer, "10"),
        (&d7, &old_body, "11"),
    ] {
        let snapshot = select(&h, selected);
        let result = h.work(restore_input(&p, &snapshot));
        assert_eq!(result["kind"], "template_draft", "{result}");
        let restored = &result["status"];
        assert_eq!(owners(&h), 1);
        assert_eq!(restored["generation"], generation);
        assert_eq!(restored["draftId"], s["draftId"]);
        assert_eq!(restored["artifact"], s["artifact"]);
        assert_eq!(restored["identities"], d7["identities"]);
        assert_eq!(&content(&h, &p, restored)["body"], expected);
        let deposited = submit(&h, &p, restored, generation, expected, "deposit");
        let raw = deposited_bytes(&h, &deposited);
        let record = Deposit::decode(&raw).unwrap();
        assert_eq!(record.key().generation.to_string(), generation);
        assert_eq!(
            serde_json::to_value(&record.envelope().draft).unwrap()["fields"],
            expected["fields"]
        );
        assert_eq!(release(&h, &p, &deposited, false)["kind"], "control");
        assert_eq!(owners(&h), 0);
    }
    for (s, bytes) in preserved {
        assert_eq!(deposited_bytes(&h, s), bytes);
    }
    assert_eq!(
        h.state
            .recovery
            .connect()
            .unwrap()
            .lock()
            .unwrap()
            .list()
            .unwrap()
            .entries
            .len(),
        5
    );
    assert!(!h
        .root
        .join(format!(
            "templates/{}.json",
            s["artifact"].as_str().unwrap()
        ))
        .exists());
    h.close_clean();
}

#[test]
fn duplicate_operations_snapshots_generations_keep_one_owner_until_successful_release() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let body = invalid_body(&h, &p, &s);
    let d7 = submit(&h, &p, &s, "7", &body, "deposit");
    let d8 = submit(&h, &p, &s, "8", &body, "deposit");
    let snapshot = select(&h, &d7);
    // 복원 owner뿐 아니라 처음 만든 정상 편집 owner도 보호한다.
    assert_eq!(
        h.work(restore_input(&p, &snapshot))["error"]["code"],
        "duplicate_conflict"
    );
    assert_eq!(release(&h, &p, &d8, false)["kind"], "control");
    let input = restore_input(&p, &snapshot);
    let operation = h.submit(input.clone());
    let first = h.result(&operation);
    assert_eq!(first["kind"], "template_draft");
    h.call(json!({"action":"submit","operation":operation,"input":input}));
    assert_eq!(h.result(&operation), first);
    h.ack(&operation);
    for snapshot in [snapshot, select(&h, &d7), select(&h, &d8)] {
        assert_eq!(
            h.work(restore_input(&p, &snapshot))["error"]["code"],
            "duplicate_conflict"
        );
        assert_eq!(owners(&h), 1);
        assert_eq!(content(&h, &p, &first["status"])["body"], body);
    }
    let rejected_release = release(&h, &p, &first["status"], false);
    assert_eq!(rejected_release["error"]["code"], "no_receipt");
    assert_eq!(
        h.work(restore_input(&p, &select(&h, &d8)))["error"]["code"],
        "duplicate_conflict"
    );
    assert_eq!(release(&h, &p, &first["status"], true)["kind"], "control");
    let again = h.work(restore_input(&p, &select(&h, &d8)));
    assert_eq!(again["kind"], "template_draft");
    assert_eq!(release(&h, &p, &again["status"], true)["kind"], "control");
    assert_eq!(owners(&h), 0);
    h.close_clean();
}

#[test]
fn generation_scan_ignores_ui_page_cutoff_and_preserves_exact_u64_overflow() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let body = invalid_body(&h, &p, &s);
    let first = submit(&h, &p, &s, "7", &body, "deposit");
    for generation in 8..137 {
        submit(&h, &p, &s, &generation.to_string(), &body, "deposit");
    }
    let high = submit(&h, &p, &s, "18446744073709551614", &body, "deposit");
    let high_bytes = deposited_bytes(&h, &high);
    assert_eq!(release(&h, &p, &high, false)["kind"], "control");
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    assert_eq!(page["page"]["entries"].as_array().unwrap().len(), 128);
    assert!(!page["page"]["next"].is_null());
    let snapshot = select(&h, &first);
    let restored = h.work(restore_input(&p, &snapshot));
    assert_eq!(restored["status"]["generation"], "18446744073709551615");
    let max = submit(
        &h,
        &p,
        &restored["status"],
        "18446744073709551615",
        &body,
        "deposit",
    );
    let max_bytes = deposited_bytes(&h, &max);
    assert_eq!(release(&h, &p, &max, false)["kind"], "control");
    let before = {
        let state = h.state.lock();
        let project = &state.projects[&p.clone().try_into().unwrap()];
        (project.views.len(), project.sessions.len())
    };
    for selected in [&first, &max] {
        let rejected = h.work(restore_input(&p, &select(&h, selected)));
        assert_eq!(rejected["error"]["code"], "full", "{rejected}");
        assert_eq!(owners(&h), 0);
    }
    let after = {
        let state = h.state.lock();
        let project = &state.projects[&p.clone().try_into().unwrap()];
        (project.views.len(), project.sessions.len())
    };
    assert_eq!(before, after);
    assert_eq!(deposited_bytes(&h, &high), high_bytes);
    assert_eq!(deposited_bytes(&h, &max), max_bytes);
    assert!(!h
        .root
        .join(format!(
            "templates/{}.json",
            s["artifact"].as_str().unwrap()
        ))
        .exists());
    h.close_clean();
}

#[test]
fn failed_generation_scan_leaves_no_owner_and_can_be_retried() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let body = invalid_body(&h, &p, &s);
    let deposited = submit(&h, &p, &s, "7", &body, "deposit");
    let bytes = deposited_bytes(&h, &deposited);
    assert_eq!(release(&h, &p, &deposited, false)["kind"], "control");
    let snapshot = select(&h, &deposited);
    let store = h.state.recovery.connect().unwrap();
    store.lock().unwrap().fault = Some(crate::data::edit_recovery::error::Stage::List);
    let result = h.work(restore_input(&p, &snapshot));
    assert_eq!(result["kind"], "recovery_failure", "{result}");
    assert_eq!(owners(&h), 0);
    store.lock().unwrap().fault = None;
    let restored = h.work(restore_input(&p, &snapshot));
    assert_eq!(restored["status"]["generation"], "8");
    assert_eq!(content(&h, &p, &restored["status"])["body"], body);
    assert_eq!(
        release(&h, &p, &restored["status"], true)["kind"],
        "control"
    );
    assert_eq!(deposited_bytes(&h, &deposited), bytes);
    h.close_clean();
}

struct RestoreProvider {
    original: NoLockService,
    fail_acquire: AtomicBool,
    fail_release: AtomicBool,
    acquire_gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}
impl RestoreProvider {
    fn new() -> Self {
        Self {
            original: NoLockService::new(),
            fail_acquire: AtomicBool::new(false),
            fail_release: AtomicBool::new(false),
            acquire_gate: Mutex::new(None),
        }
    }
}
impl LockService for RestoreProvider {
    fn provider_info(&self) -> LockProviderInfo {
        self.original.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        if let Some((entered, resume)) = self.acquire_gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            resume.recv_timeout(LIMIT).unwrap();
        }
        if self.fail_acquire.load(Ordering::Acquire) {
            return Err(LockError::for_request(
                crate::data::collaboration_lock::LockErrorCategory::LockAcquireFailed,
                self.provider_info().kind,
                crate::data::collaboration_lock::LockOperation::Acquire,
                &r,
            ));
        }
        self.original.acquire(r)
    }
    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        self.original.validate(held)
    }
    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        if self.fail_release.load(Ordering::Acquire) {
            return Err(LockError::for_held(
                crate::data::collaboration_lock::LockErrorCategory::LockReleaseFailed,
                held.provider_kind(),
                crate::data::collaboration_lock::LockOperation::Release,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ));
        }
        self.original.release(held)
    }
}

#[test]
fn concurrent_restore_and_acquire_release_failures_preserve_owner_and_canonical() {
    let provider = Arc::new(RestoreProvider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (id, _) = h.template(&p);
    let canonical_path = h.root.join(format!("templates/{id}.json"));
    let canonical = fs::read(&canonical_path).unwrap();
    let view = h.read_template(&p, &id);
    let s = begin(&h, &p, view["view"].clone());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "복원 중 원문".into();
    let deposited = submit(&h, &p, &s, "7", &body, "deposit");
    stage_latest_as_legacy_archive(&h, &deposited["draftId"]);
    let bytes = deposited_bytes(&h, &deposited);
    assert_eq!(release(&h, &p, &deposited, false)["kind"], "control");
    let snapshot = select(&h, &deposited);
    let input = restore_input(&p, &snapshot);
    // begin_edit 실패도 worker가 반환한 등록/실패 원인을 소유한다. ghost가 아닌
    // 오류가 보이는 owner이며 명시적으로 해제하기 전에는 중복 등록을 막아야 한다.
    provider.fail_acquire.store(true, Ordering::Release);
    let failed = h.work(input.clone());
    assert_eq!(failed["kind"], "template_draft", "{failed}");
    assert_eq!(failed["status"]["error"]["code"], "session_rejected");
    assert_eq!(owners(&h), 1);
    assert_eq!(h.work(input.clone())["error"]["code"], "duplicate_conflict");
    provider.fail_acquire.store(false, Ordering::Release);
    assert_eq!(release(&h, &p, &failed["status"], true)["kind"], "control");
    assert_eq!(owners(&h), 0);
    // 첫 worker 등록이 진행 중인 동안 두 번째 요청을 실제 IPC로 접수한다.
    let (entered_tx, entered) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    *provider.acquire_gate.lock().unwrap() = Some((entered_tx, resume_rx));
    let first_op = h.submit(input.clone());
    entered.recv_timeout(LIMIT).unwrap();
    let second_op = h.submit(input.clone());
    resume.send(()).unwrap();
    let first = h.result(&first_op);
    let second = h.result(&second_op);
    assert_eq!(first["kind"], "template_draft", "{first}");
    assert_eq!(second["error"]["code"], "duplicate_conflict", "{second}");
    h.ack(&first_op);
    h.ack(&second_op);
    assert_eq!(owners(&h), 1);
    assert_eq!(content(&h, &p, &first["status"])["body"], body);
    let again = submit(&h, &p, &first["status"], "8", &body, "deposit");
    stage_latest_as_legacy_archive(&h, &again["draftId"]);
    deposited_bytes(&h, &again);
    provider.fail_release.store(true, Ordering::Release);
    let failed_release = release(&h, &p, &again, false);
    assert_eq!(
        failed_release["status"]["error"]["code"], "release_rejected",
        "{failed_release}"
    );
    assert_eq!(owners(&h), 1);
    assert_eq!(h.work(input.clone())["error"]["code"], "duplicate_conflict");
    provider.fail_release.store(false, Ordering::Release);
    assert_eq!(release(&h, &p, &again, false)["kind"], "control");
    let last = h.work(input);
    assert_eq!(last["status"]["generation"], "9");
    assert_eq!(release(&h, &p, &last["status"], true)["kind"], "control");
    assert_eq!(owners(&h), 0);
    assert_eq!(fs::read(canonical_path).unwrap(), canonical);
    assert_eq!(deposited_bytes(&h, &deposited), bytes);
    h.close_clean();
}

#[test]
fn distinct_drafts_and_same_draft_id_in_another_project_restore_independently() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let body = invalid_body(&h, &p, &s);
    let first = submit(&h, &p, &s, "7", &body, "deposit");
    let s2 = begin(&h, &p, Value::Null);
    let body2 = invalid_body(&h, &p, &s2);
    let second = submit(&h, &p, &s2, "30", &body2, "deposit");
    // Keep the first owner until both legacy namespace fixtures exist; a
    // normal owner-free begin would correctly migrate and retire the first.
    assert_eq!(release(&h, &p, &first, false)["kind"], "control");
    assert_eq!(release(&h, &p, &second, false)["kind"], "control");
    let root2 = h.base.join("other-project");
    fs::create_dir(&root2).unwrap();
    let opened = h.work(json!({"kind":"open","root":root2}));
    let p2 = opened["project"].as_str().unwrap();
    let other_seed = begin(&h, p2, Value::Null);
    // 프로젝트 namespace 분리만을 위한 실제 store fixture. 복원/중복 검사는 IPC다.
    let source = Deposit::decode(&deposited_bytes(&h, &first)).unwrap();
    let mut envelope = source.envelope().clone();
    envelope.key.project_fingerprint = other_seed["projectFingerprint"].as_str().unwrap().into();
    envelope.deposit_id = uuid::Uuid::new_v4().to_string();
    let other = Deposit::freeze(envelope).unwrap();
    h.state
        .recovery
        .connect()
        .unwrap()
        .lock()
        .unwrap()
        .accept(&other)
        .unwrap();
    let third = json!({"receipt":{"key":other.key(),"depositId":other.envelope().deposit_id,"digest":other.payload_digest()}});
    assert_eq!(release(&h, p2, &other_seed, true)["kind"], "control");
    let a = h.work(restore_input(&p, &select(&h, &first)));
    let b = h.work(restore_input(&p, &select(&h, &second)));
    let c = h.work(restore_input(p2, &select(&h, &third)));
    assert_eq!(a["status"]["generation"], "8");
    assert_eq!(b["status"]["generation"], "31");
    assert_eq!(c["status"]["generation"], "8");
    assert_eq!(a["status"]["draftId"], c["status"]["draftId"]);
    assert_ne!(
        a["status"]["projectFingerprint"],
        c["status"]["projectFingerprint"]
    );
    assert_eq!(owners(&h), 3);
    for (project, result) in [(&*p, a), (&*p, b), (p2, c)] {
        assert_eq!(
            release(&h, project, &result["status"], true)["kind"],
            "control"
        );
    }
    assert_eq!(owners(&h), 0);
    h.close_clean();
}
