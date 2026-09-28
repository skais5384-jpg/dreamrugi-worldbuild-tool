//! 실제 guarded registry/worker/storage 증거. mock runtime이므로 실제 Windows 창 증거는 아니다.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command as ProcessCommand, Stdio},
};

fn attempt(h: &Harness) -> Value {
    h.state.request_shutdown();
    h.call(json!({"action":"ui_close_status"}))["attempt"].clone()
}
fn decide(h: &Harness, attempt: &Value, proceed: bool) -> Value {
    h.call(json!({"action":"ui_close_decision","attempt":attempt,"proceed":proceed}))
}
fn confirmed_exit(h: &Harness) {
    let a = attempt(h);
    if !a.is_null() {
        decide(h, &a, true);
    }
    h.close_clean();
}

fn progress_wake(h: &Harness) -> mpsc::Receiver<()> {
    let state = Arc::downgrade(&h.state);
    let (tx, rx) = mpsc::channel();
    h.state.set_wake(Arc::new(move || {
        let state = state.upgrade().unwrap();
        // 잠금 안의 callback 호출은 즉시 실패시킨다. 재진입 조회도 안전해야 한다.
        assert!(
            state.inner.try_lock().is_ok(),
            "wake called under app mutex"
        );
        let caller_thread = thread::current().id();
        let tx = tx.clone();
        // 창 thread에서 빈 callback이 inline 실행되는 누락도 잡는다. 실제 창 exit는 GUI로 검증한다.
        crate::events::queue_wake(move || {
            assert_ne!(caller_thread, thread::current().id());
            state.poll();
            tx.send(()).unwrap();
        });
    }));
    rx
}

#[test]
fn m745_fix003_clean_native_close_report_exposes_results_until_acknowledged() {
    let h = Harness::new();
    let project = h.open();
    h.call(json!({"action":"app_shutdown"}));
    let deadline = Instant::now() + LIMIT;
    let report = loop {
        h.state.poll();
        let status = h.call(json!({"action":"project_status","project":project}));
        if status["shutdown"]["reports"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
        {
            break status;
        }
        assert!(Instant::now() < deadline, "native close report deadline");
        thread::yield_now();
    };
    let shutdown = &report["shutdown"];
    assert_eq!(shutdown["reportPending"], true);
    assert!(shutdown["blockers"]
        .as_array()
        .unwrap()
        .contains(&json!("Results")));
    assert_eq!(shutdown["reports"][0]["initializationFailed"], false);
    assert_eq!(shutdown["reports"][0]["closeFailed"], false);
    assert_eq!(shutdown["reports"][0]["releaseFailures"], "0");
    assert_eq!(shutdown["reports"][0]["coordinationErrors"], json!([]));
    h.call(json!({"action":"acknowledge_shutdown","project":project}));
    h.close_clean();
    assert!(h.state.take_exit_approval());
    assert!(!h.state.take_exit_approval());
}

#[test]
fn m745_fix003_exit_approval_can_retry_after_external_svn_guard_defers_it() {
    let h = Harness::new();
    h.state.native_registered();
    h.call(json!({"action":"app_shutdown"}));
    crate::events::cleanup_pass(&h.state);
    assert!(h.state.poll());
    assert!(h.state.take_exit_approval());
    assert!(!h.state.take_exit_approval());
    h.state.defer_exit();
    assert!(h.state.take_exit_approval());
    assert!(!h.state.take_exit_approval());
}

#[test]
fn m271_wake_report_and_last_result_ack_schedule_existing_exit_progress() {
    for result_remains in [false, true] {
        let h = Harness::new();
        let project = h.open();
        let operation = h.submit(json!({"kind":"list_templates","project":project}));
        h.result(&operation);
        if !result_remains {
            h.ack(&operation);
        }
        h.state.native_registered();
        h.call(json!({"action":"app_shutdown"}));
        let deadline = Instant::now() + LIMIT;
        while !h.state.lock().projects.values().all(|p| p.joined) {
            assert!(Instant::now() < deadline, "worker join deadline");
            h.state.poll();
            thread::yield_now();
        }
        assert!(!h
            .state
            .lock()
            .projects
            .values()
            .next()
            .unwrap()
            .reports
            .is_empty());
        assert!(!h.state.poll());
        // join 완료 후 새 신호 수신기를 설치해 과거 worker wake를 인수의 신호로 오인하지 않는다.
        let wake = progress_wake(&h);
        h.call(json!({"action":"acknowledge_shutdown","project":project}));
        wake.recv_timeout(Duration::from_secs(1))
            .expect("report acknowledgement must schedule progress");
        h.call(json!({"action":"acknowledge_shutdown","project":project}));
        assert!(
            wake.try_recv().is_err(),
            "empty duplicate ack must not schedule another pass"
        );
        crate::events::cleanup_pass(&h.state);
        if result_remains {
            assert!(!h.state.poll());
            assert_eq!(
                h.call(json!({"action":"app_status"}))["native_cleanup"]["attempts"],
                "0"
            );
            h.ack(&operation);
            wake.recv_timeout(Duration::from_secs(1))
                .expect("last result acknowledgement must schedule progress");
            crate::events::cleanup_pass(&h.state);
        }
        assert!(h.state.poll());
        assert_eq!(
            h.call(json!({"action":"app_status"}))["native_cleanup"]["attempts"],
            "1"
        );
        assert!(h.state.take_exit_approval());
        assert!(!h.state.take_exit_approval());
        assert!(
            wake.try_recv().is_err(),
            "status/poll must not create wake loops"
        );
    }
}

#[test]
fn m271_wake_close_consent_and_native_retry_keep_single_cleanup_and_exit() {
    for ui_ready in [false, true] {
        let h = Harness::new();
        h.state.native_registered();
        let wake = progress_wake(&h);
        if ui_ready {
            h.call(json!({"action":"ui_ready"}));
        }
        h.call(json!({"action":"app_shutdown"}));
        if ui_ready {
            assert!(wake.try_recv().is_err());
            let attempt = h.call(json!({"action":"ui_close_status"}))["attempt"].clone();
            decide(&h, &attempt, true);
        }
        wake.recv_timeout(Duration::from_secs(1))
            .expect("accepted shutdown must schedule progress");
        assert!(!h.state.take_exit_approval());
        assert!(h.state.begin_native_cleanup());
        h.state.event_failure(Code::PlatformRemoval);
        for _ in 0..3 {
            crate::events::cleanup_pass(&h.state);
            assert!(!h.state.poll());
        }
        let status = h.call(json!({"action":"app_status"}));
        assert_eq!(status["native_cleanup"]["attempts"], "1");
        let generation = status["native_cleanup"]["generation"].clone();
        for _ in 0..2 {
            h.call(json!({"action":"retry_native_cleanup","generation":generation}));
            wake.recv_timeout(Duration::from_secs(1))
                .expect("explicit native retry must schedule progress");
        }
        // 중복 wake가 있어도 기존 Pending/Running 병합과 명시적 한 pass 예산을 유지한다.
        crate::events::cleanup_pass(&h.state);
        crate::events::cleanup_pass(&h.state);
        assert_eq!(
            h.call(json!({"action":"app_status"}))["native_cleanup"]["attempts"],
            "2"
        );
        assert!(h.state.poll());
        assert!(h.state.take_exit_approval());
        assert!(!h.state.take_exit_approval());
        assert!(wake.try_recv().is_err());
    }
}

#[test]
fn m271_production_controller_client_real_guarded_storage_and_close_protocol() {
    run_controller("m271");
}

#[test]
fn m272_production_field_controller_real_guarded_save_and_reopen() {
    run_controller("m272");
}
#[test]
fn m274_production_management_controller_real_guarded_storage_and_reopen() {
    run_controller("m274");
}

fn run_controller(scenario: &str) {
    let h = Harness::new();
    if scenario == "m274" {
        super::m274::seed(&h);
    }
    let driver =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/state/tests/m271-controller.cjs");
    let mut child = ProcessCommand::new("node")
        .arg(driver)
        .env("WB_UI_PROJECT", &h.root)
        .env("WB_UI_SCENARIO", scenario)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let mut summary = None;
    for line in BufReader::new(output).lines() {
        let packet: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if packet["done"] == true {
            summary = Some(packet["summary"].clone());
            break;
        }
        h.state.poll();
        let response = if packet["nativeClose"] == true {
            h.state.request_shutdown();
            Ok(json!(true))
        } else if packet["awaitShutdown"] == true {
            let end = Instant::now() + LIMIT;
            loop {
                h.state.poll();
                if h.state.lock().projects.values().all(|p| p.joined) {
                    break;
                }
                assert!(Instant::now() < end, "worker join deadline");
                thread::yield_now();
            }
            Ok(json!(true))
        } else {
            let command = packet["command"].clone();
            let mut result = h.ipc(command.clone());
            if command["action"] == "operation"
                && result.as_ref().is_ok_and(|r| r["state"] == "pending")
            {
                h.result(command["operation"].as_str().unwrap());
                result = h.ipc(command);
            }
            result
        };
        let response = match response {
            Ok(value) => json!({"serial":packet["serial"],"ok":true,"value":value}),
            Err(value) => json!({"serial":packet["serial"],"ok":false,"value":value}),
        };
        writeln!(input, "{response}").unwrap();
        input.flush().unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "controller driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary = summary.expect("controller must complete all assertions");
    let path = h
        .root
        .join("templates")
        .join(format!("{}.json", summary["id"].as_str().unwrap()));
    let disk: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(disk["name"], summary["name"]);
    if scenario == "m274" {
        super::m274::verify_copies(&h, &summary);
    } else if scenario == "m271" {
        assert_eq!(disk["revision"], 2);
    } else {
        assert_eq!(
            disk["revision"].as_u64().unwrap().to_string(),
            summary["revision"]
        );
        assert_eq!(disk["fields"].as_object().unwrap().len(), 8);
        assert_eq!(summary["reopened"], true);
    }
    assert_eq!(
        fs::read_dir(h.root.join("templates")).unwrap().count(),
        if scenario == "m274" { 3 } else { 1 }
    );
    assert!(h.state.poll());
    assert!(h.state.lock().projects.values().all(|p| p.joined));
    if let Some(base) = std::env::var_os("UI_REPORT_DIR") {
        let base = PathBuf::from(base);
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join(if scenario == "m271" {
                "controller-storage.json"
            } else if scenario == "m274" {
                "management-controller-storage.json"
            } else {
                "field-controller-storage.json"
            }),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn m271_close_cancel_keeps_original_view_and_ordinary_save_admission() {
    let h = Harness::new();
    assert_eq!(h.call(json!({"action":"ui_ready"}))["enabled"], true);
    let p = h.open();
    let (id, _) = h.template(&p);
    let read = h.read_template(&p, &id);
    let session = h.session(&p, vec![read["view"].clone()], "template");
    let a = attempt(&h);
    assert!(!a.is_null());
    assert_eq!(attempt(&h), a);
    assert_eq!(h.call(json!({"action":"app_status"}))["closing"], false);
    assert!(!h.state.poll());
    assert_eq!(decide(&h, &a, false)["closing"], false);
    let name = "  취소 뒤 저장\n원문  ";
    let result = h.work(
        json!({"kind":"update_template","project":p,"session":session,
        "view":read["view"],"revision":"1","edit":{"kind":"name","name":name}}),
    );
    assert_eq!(result["disk"], "committed");
    let stored = h.read_template(&p, &id);
    assert_eq!(stored["content"]["name"], name);
    assert_eq!(stored["content"]["revision"], "2");
    let listed = h.work(json!({"kind":"list_templates","project":p}));
    assert_eq!(listed["templates"][0]["name"], name);
    confirmed_exit(&h);
}

#[test]
fn m271_current_attempt_caller_and_registration_are_required_without_timeout_consent() {
    let h = Harness::new();
    h.call(json!({"action":"ui_ready"}));
    let a = attempt(&h);
    decide(&h, &a, false);
    let b = attempt(&h);
    assert_ne!(a, b);
    assert_eq!(
        h.ipc(json!({"action":"ui_close_decision","attempt":a,"proceed":true}))
            .unwrap_err()["code"],
        "wrong_binding"
    );
    let foreign = Caller(Id::new());
    let command = dto::decode(
        &serde_json::to_vec(&json!({"action":"ui_close_decision","attempt":b,"proceed":true}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        h.state.dispatch(foreign, command).err().unwrap().code,
        Code::Forbidden
    );
    for _ in 0..4 {
        assert!(!h.state.poll());
        assert_eq!(h.call(json!({"action":"ui_close_status"}))["attempt"], b);
    }
    assert_eq!(h.call(json!({"action":"app_status"}))["closing"], false);
    decide(&h, &b, true);
    assert_eq!(
        h.ipc(json!({"action":"ui_ready"})).unwrap_err()["code"],
        "closed"
    );
    h.close_clean();
}

#[test]
fn m271_confirmed_close_preserves_pending_result_and_retained_blockers() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    h.call(json!({"action":"ui_ready"}));
    let p = h.open();
    let (entered, release) = provider.hold();
    let op = h
        .submit(json!({"kind":"create_template","project":p,"name":"pending","presentation":null}));
    entered.recv_timeout(LIMIT).unwrap();
    let a = attempt(&h);
    decide(&h, &a, true);
    assert!(!h.state.poll());
    release.send(()).unwrap();
    let result = h.result(&op);
    assert_eq!(result["disk"], "committed");
    assert!(!h.state.poll());
    h.ack(&op);
    h.close_clean();

    let h = Harness::new();
    h.call(json!({"action":"ui_ready"}));
    let p = h.open();
    let (id, _) = h.template(&p);
    let read = h.read_template(&p, &id);
    let s = h.session(&p, vec![read["view"].clone()], "template");
    let op = h.submit(json!({"kind":"update_template","project":p,"session":s,"view":read["view"],"revision":"2","edit":{"kind":"name","name":"retained original"}}));
    assert_eq!(h.result(&op)["kind"], "rejected");
    let owner = h.call(json!({"action":"operation","operation":op}))["retained"].clone();
    h.ack(&op);
    let retained = h.call(json!({"action":"retained_read","retained":owner}));
    assert_eq!(retained["intent"]["edit"]["name"], "retained original");
    assert_eq!(retained["g6_clearable"], true);
    let a = attempt(&h);
    decide(&h, &a, true);
    assert!(!h.state.poll());
    let abandon = h.reserve("control");
    h.call(json!({"action":"submit","operation":abandon,"input":{"kind":"abandon_retained","retained":owner}}));
    assert_eq!(h.result(&abandon)["action"], "abandoned");
    h.ack(&abandon);
    h.close_clean();
}

#[test]
fn m271_name_only_write_preserves_fields_archived_options_unknown_lexemes_and_opaque_names() {
    let h = Harness::new();
    let (template, _) = crate::data::application::composite::tests::guarded_fixture_bytes();
    let id = "99999999-9999-4999-8999-000000000064";
    fs::create_dir(h.root.join("templates")).unwrap();
    let path = h.root.join("templates").join(format!("{id}.json"));
    fs::write(&path, &template).unwrap();
    let p = h.open();
    let read = h.read_template(&p, id);
    let session = h.session(&p, vec![read["view"].clone()], "template");
    let r = h.work(json!({"kind":"update_template","project":p,"session":session,"view":read["view"],"revision":"3","edit":{"kind":"name","name":" 새 이름\n "}}));
    assert_eq!(r["disk"], "committed");
    let saved = fs::read(&path).unwrap();
    let mut before: Value = serde_json::from_slice(&template).unwrap();
    let mut after: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(after["name"], " 새 이름\n ");
    assert_eq!(after["revision"], 4);
    for key in ["name", "revision", "updatedAtUtc"] {
        before.as_object_mut().unwrap().remove(key);
        after.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(before, after);
    let before = crate::data::json::parse_strict_lossless_json_object(&template).unwrap();
    let after = crate::data::json::parse_strict_lossless_json_object(&saved).unwrap();
    // timestamp의 '-0' 같은 우연한 문자열이 아니라 숫자 token을 보존하는 실제 subtree를 비교한다.
    for path in ["fields", "future", "presentation", "fieldOrder"] {
        assert_eq!(before.object_path(&[path]), after.object_path(&[path]));
    }
    for name in ["", "   ", "한글\n원문"] {
        let r =
            h.work(json!({"kind":"create_template","project":p,"name":name,"presentation":null}));
        assert_eq!(r["disk"], "committed");
        assert_eq!(
            h.read_template(&p, r["artifact"].as_str().unwrap())["content"]["name"],
            name
        );
    }
    let reserved = h.reserve("ordinary");
    let rejected = h.ipc(json!({"action":"submit","operation":reserved,"input":{"kind":"create_template","project":p,"name":{"raw":"M271_RAW_ERROR_CANARY"},"presentation":null}})).unwrap_err();
    assert_eq!(rejected["code"], "invalid_input");
    assert!(!rejected.to_string().contains("M271_RAW_ERROR_CANARY"));
    h.close_clean();
}
