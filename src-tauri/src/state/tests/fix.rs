use super::*;

pub(super) fn submit(h: &Harness, lane: &str, input: Value) -> String {
    let op = h.reserve(lane);
    h.call(json!({"action":"submit","operation":op,"input":input}));
    op
}
fn retained(h: &Harness, op: &str) -> Value {
    h.call(json!({"action":"operation","operation":op}))["retained"].clone()
}
fn read(h: &Harness, key: &Value) -> Value {
    h.call(json!({"action":"retained_read","retained":key}))
}
pub(super) fn abandon(h: &Harness, key: &Value) {
    let op = submit(
        h,
        "control",
        json!({"kind":"abandon_retained","retained":key}),
    );
    assert_eq!(h.result(&op)["action"], "abandoned");
    h.ack(&op);
}
pub(super) fn joined(h: &Harness, project: &str) {
    let end = Instant::now() + LIMIT;
    loop {
        h.state.poll();
        let status = h.call(json!({"action":"project_status","project":project}));
        h.call(json!({"action":"acknowledge_shutdown","project":project}));
        if status["shutdown"]["joined"] == true {
            break;
        }
        assert!(Instant::now() < end, "join deadline");
        thread::yield_now();
    }
}
fn release_views(h: &Harness, p: &str) {
    let views = h.state.lock().projects[&p.to_owned().try_into().unwrap()]
        .views
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for view in views {
        h.call(json!({"action":"release_view","project":p,"view":String::from(view)}));
    }
}
fn retire(h: &Harness, p: &str) {
    release_views(h, p);
    let id = submit(h, "control", json!({"kind":"retire_project","project":p}));
    let result = h.result(&id);
    assert_eq!(result["kind"], "project_retired");
    assert_eq!(result["shutdown"]["normalExitAllowed"], true);
    assert_eq!(result["shutdown"]["joined"], true);
    assert!(h.result(&id) == result);
    h.ack(&id);
}
struct Stale {
    project: String,
    template: String,
    session: String,
    operation: String,
    key: Value,
    intent: Value,
}
fn stale(h: &Harness) -> Stale {
    let p = h.open();
    let (t, _) = h.template(&p);
    let view = h.read_template(&p, &t);
    let s = h.session(&p, vec![view["view"].clone()], "template");
    let make = |name: &str| json!({"kind":"update_template","project":p,"session":s,"view":view["view"],"revision":"1","edit":{"kind":"name","name":name}});
    assert_eq!(h.work(make("committed base"))["disk"], "committed");
    let intent = json!({"kind":"update_template","revision":"1","edit":{"kind":"name","name":"FIX retained intent"}});
    let operation = h.submit(make("FIX retained intent"));
    assert_ne!(h.result(&operation)["disk"], "committed");
    let key = retained(h, &operation);
    assert!(key.is_object());
    Stale {
        project: p,
        template: t,
        session: s,
        operation,
        key,
        intent,
    }
}

#[test]
fn f1_retained_read_handoff_abandon_after_ack_end_and_join_keeps_exact_owners() {
    let h = Harness::new();
    let s = stale(&h);
    let (input, source, original) = {
        let state = h.state.lock();
        let op = &state.operations[&s.operation.clone().try_into().unwrap()];
        let r = op.result.as_ref().unwrap();
        (
            op.input.clone().unwrap(),
            r.sources[0].clone(),
            &*r.original as *const _ as *const (),
        )
    };
    let before = read(&h, &s.key);
    assert!(before["intent"] == s.intent);
    assert_eq!(before["artifacts"][0], s.template);
    assert!(!before.to_string().contains("sourceToken"));
    let bad = json!({"id":s.key["id"],"generation":String::from(Id::new())});
    assert_eq!(
        h.ipc(json!({"action":"retained_read","retained":bad}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    let key: RetainedRef = serde_json::from_value(s.key.clone()).unwrap();
    assert!(h
        .state
        .dispatch(Caller(Id::new()), Command::RetainedRead { retained: key })
        .is_err());
    h.ack(&s.operation);
    assert!(h.call(json!({"action":"retained_list"}))["entries"]
        .as_array()
        .unwrap()
        .contains(&s.key));
    assert!(read(&h, &s.key) == before);
    assert_eq!(
        h.ipc(json!({"action":"operation","operation":s.operation}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    assert!(h.control(
        json!({"kind":"session_control","project":s.project,"session":s.session,"control":"end"})
    )["error"]
        .is_null());
    h.call(json!({"action":"app_shutdown"}));
    joined(&h, &s.project);
    assert!(!h.state.poll());
    assert!(read(&h, &s.key) == before);
    let transfer = json!({"kind":"retained_handoff","retained":s.key});
    let op = submit(&h, "control", transfer.clone()); // 응답을 사용하지 않아도 예약 ID로 인수 결과를 재조회한다.
    h.call(json!({"action":"submit","operation":op,"input":transfer}));
    assert_eq!(h.result(&op)["action"], "handed_off");
    assert!(read(&h, &s.key) == before);
    {
        let state = h.state.lock();
        let draft = state.operations[&op.clone().try_into().unwrap()]
            .draft
            .as_ref()
            .unwrap();
        assert!(Arc::ptr_eq(&input, &draft.0));
        assert!(Arc::ptr_eq(&source, &draft.1.sources[0]));
        assert!(std::ptr::eq(
            original,
            &*draft.1.original as *const _ as *const ()
        ));
        assert!(state.retained.is_empty());
    }
    assert!(!h.state.poll());
    h.ack(&op);
    assert!(read(&h, &s.key) == before);
    let second = submit(
        &h,
        "control",
        json!({"kind":"retained_handoff","retained":s.key}),
    );
    let abandon_op = submit(
        &h,
        "control",
        json!({"kind":"abandon_retained","retained":s.key}),
    );
    let observation = h.result(&abandon_op);
    h.call(json!({"action":"submit","operation":abandon_op,"input":{"kind":"abandon_retained","retained":s.key}}));
    assert!(h.result(&abandon_op) == observation);
    assert_eq!(
        h.ipc(json!({"action":"retained_read","retained":s.key}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    assert!(!h.state.poll());
    h.ack(&second);
    h.ack(&abandon_op);
    drop(input);
    drop(source);
    h.close_clean();
}

#[test]
fn f1_explicit_resume_revalidates_current_target_and_owns_failure_then_commits_once() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let s = stale(&h);
    h.ack(&s.operation);
    let previous = read(&h, &s.key);
    let old_view = h.state.lock().retained[0].0.clone();
    let Work::UpdateTemplate { view, .. } = &*old_view else {
        panic!("fixture kind");
    };
    let destination = json!({"kind":"update_template","session":s.session,"view":String::from(*view),"revision":"1"});
    let failed = submit(
        &h,
        "ordinary",
        json!({"kind":"resume_retained","retained":s.key,"project":s.project,"destination":destination}),
    );
    assert_ne!(h.result(&failed)["disk"], "committed");
    let next = retained(&h, &failed);
    assert!(read(&h, &next)["intent"] == previous["intent"]);
    let reserved = h.reserve("ordinary");
    assert_eq!(h.ipc(json!({"action":"submit","operation":reserved,"input":{"kind":"resume_retained","retained":next,"project":s.project,"destination":destination}})).unwrap_err()["code"],"owners_remain");
    {
        let state = h.state.lock();
        let result = state.operations[&failed.clone().try_into().unwrap()]
            .result
            .as_ref()
            .unwrap();
        assert!(Arc::ptr_eq(&result.previous.as_ref().unwrap().0, &old_view));
        assert!(result.previous.as_ref().unwrap().1.previous.is_none());
    }
    h.ack(&failed);
    let (other, _) = h.template(&s.project);
    let wrong = h.read_template(&s.project, &other);
    let reserved = h.reserve("ordinary");
    assert_eq!(h.ipc(json!({"action":"submit","operation":reserved,"input":{"kind":"resume_retained","retained":next,"project":s.project,"destination":{"kind":"update_template","session":s.session,"view":wrong["view"],"revision":"1"}}})).unwrap_err()["code"],"wrong_binding");
    assert!(read(&h, &next)["intent"] == previous["intent"]);
    let view = h.read_template(&s.project, &s.template);
    let session = h.session(&s.project, vec![view["view"].clone()], "template");
    let resume = json!({"kind":"resume_retained","retained":next,"project":s.project,"destination":{"kind":"update_template","session":session,"view":view["view"],"revision":"2"}});
    let (entered, release) = provider.hold();
    let op = h.reserve("ordinary");
    h.main.as_ref().clone().on_message(
        Harness::request(
            "guarded",
            serde_json::to_vec(&json!({"action":"submit","operation":op,"input":resume})).unwrap(),
            "http://tauri.localhost",
        ),
        Box::new(|_, _, _, _, _| {}),
    );
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"operation","operation":op}))["state"],
        "pending"
    );
    assert!(read(&h, &next)["intent"] == previous["intent"]);
    let abandon_op = h.reserve("control");
    assert_eq!(h.ipc(json!({"action":"submit","operation":abandon_op,"input":{"kind":"abandon_retained","retained":next}})).unwrap_err()["code"],"owners_remain");
    assert_eq!(h.result(&abandon_op)["error"]["code"], "owners_remain");
    h.ack(&abandon_op);
    assert_eq!(
        h.ipc(json!({"action":"acknowledge_transport","operation":op}))
            .unwrap_err()["code"],
        "not_terminal"
    );
    h.call(json!({"action":"submit","operation":op,"input":resume}));
    release.send(()).unwrap();
    assert_eq!(h.result(&op)["disk"], "committed");
    let bytes = fs::read(h.root.join(format!("templates/{}.json", s.template))).unwrap();
    let disk = artifact::decode_template(&bytes).unwrap();
    assert!(disk.name() == "FIX retained intent");
    assert_eq!(disk.revision().get(), 3);
    assert!(retained(&h, &op).is_null());
    assert!(!h.state.poll());
    h.ack(&op);
    assert!(h.state.lock().retained.is_empty());
    h.close_clean();
}

#[test]
fn f2_no_draft_32_then_8_acknowledgements_recover_control_capacity() {
    let h = Harness::new();
    let p = h.open();
    let (_, s) = h.template(&p);
    let input = json!({"kind":"session_control","project":p,"session":s,"control":"return_active"});
    for _ in 0..ORDINARY {
        assert_eq!(h.control(input.clone())["error"]["code"], "no_draft");
    }
    assert!(h.state.lock().retained.is_empty());
    let mut operations = Vec::new();
    for _ in 0..CONTROL {
        let (op, r) = h.control_result(input.clone());
        assert_eq!(r["error"]["code"], "no_draft");
        operations.push(op);
    }
    assert_eq!(
        h.ipc(json!({"action":"reserve","lane":"control"}))
            .unwrap_err()["code"],
        "full"
    );
    for op in operations {
        h.ack(&op);
    }
    assert_eq!(h.control(input)["error"]["code"], "no_draft");
    assert!(h.state.lock().retained.is_empty());
    assert_eq!(
        h.call(json!({"action":"session_status","project":p,"session":s}))["active_dirty"],
        false
    );
    h.close_clean();
}

#[test]
fn f2_real_returned_p_and_g6_owner_survive_data_free_errors_and_g6_abandonment() {
    let h = Harness::new();
    super::recovery::unavailable(&h);
    let s = stale(&h);
    h.ack(&s.operation);
    let t = h.read_template(&s.project, &s.template);
    let created=h.work(json!({"kind":"create_document","project":s.project,"view":t["view"],"name":"payload document"}));
    let d =
        h.work(json!({"kind":"read_document","project":s.project,"document":created["artifact"]}));
    let session = h.session(
        &s.project,
        vec![d["view"].clone(), t["view"].clone()],
        "document",
    );
    let work = json!({"kind":"save_document","project":s.project,"session":session,"document":d["view"],"template":t["view"],"revision":"2","edits":[{"kind":"rename","name":"FIX exact P"}]});
    let dirty = h.submit(work);
    assert_eq!(h.result(&dirty)["error"]["code"], "sink_unavailable");
    let proof = h.state.lock().operations[&dirty.clone().try_into().unwrap()]
        .input
        .clone()
        .unwrap();
    let dirty_key = retained(&h, &dirty);
    h.ack(&dirty);
    for _ in 0..ORDINARY + CONTROL + 1 {
        assert_eq!(h.control(json!({"kind":"session_control","project":s.project,"session":s.session,"control":"return_active"}))["error"]["code"],"no_draft");
    }
    assert!(read(&h, &s.key)["intent"] == s.intent);
    let (returned,result)=h.control_result(json!({"kind":"session_control","project":s.project,"session":session,"control":"return_active"}));
    assert!(result["error"].is_null());
    {
        let state = h.state.lock();
        let result = state.operations[&returned.clone().try_into().unwrap()]
            .result
            .as_ref()
            .unwrap();
        let original = result
            .original
            .downcast_ref::<Result<PendingEdit, WorkerCategory>>()
            .unwrap();
        let payload = original.as_ref().ok().unwrap();
        assert!(Arc::ptr_eq(&proof, &payload.input));
        assert!(
            matches!(&*payload.input,Work::SaveDocument { edits, .. } if matches!(&edits[0],DocumentEdit::Rename { name } if name=="FIX exact P"))
        );
    }
    let pkey = retained(&h, &returned);
    h.ack(&returned);
    assert!(!read(&h, &pkey)["g6_clearable"].as_bool().unwrap());
    for key in [&pkey, &dirty_key] {
        let id = h.reserve("control");
        assert_eq!(h.ipc(json!({"action":"submit","operation":id,"input":{"kind":"abandon_retained","retained":key}})).unwrap_err()["code"],"owners_remain");
        assert_eq!(h.result(&id)["error"]["code"], "owners_remain");
        h.ack(&id);
    }
    abandon(&h, &s.key);
    assert_eq!(h.state.lock().retained.len(), 2);
    assert_eq!(h.control(json!({"kind":"session_control","project":s.project,"session":session,"control":"return_active"}))["error"]["code"],"no_draft");
    h.call(json!({"action":"app_shutdown"}));
    joined(&h, &s.project);
    assert!(!h.state.poll());
    assert_eq!(
        h.call(json!({"action":"project_status","project":s.project}))["shutdown"]
            ["normalExitAllowed"],
        false
    );
    // 실제 반환 P와 CallerCustody의 정상 exit 차단이 최종 assertion이다. 강제 clear를 하지 않는다.
}

#[test]
fn f3_project_slots_retire_after_real_join_and_unclaimed_result_blocks_removal() {
    let h = Harness::new();
    let mut ids = Vec::new();
    for _ in 0..MAX_PROJECTS + 1 {
        let p = h.open();
        assert!(!ids.contains(&p));
        let (close, _) = h.control_result(json!({"kind":"close","project":p}));
        joined(&h, &p);
        let release = h.reserve("control");
        assert_eq!(h.ipc(json!({"action":"submit","operation":release,"input":{"kind":"retire_project","project":p}})).unwrap_err()["code"],"owners_remain");
        assert_eq!(h.result(&release)["error"]["code"], "owners_remain");
        h.ack(&release);
        h.ack(&close);
        retire(&h, &p);
        assert_eq!(
            h.ipc(json!({"action":"project_status","project":p}))
                .unwrap_err()["code"],
            "unknown_id"
        );
        assert_eq!(h.state.lock().projects.len(), 0);
        ids.push(p);
    }
    for index in 0..MAX_PROJECTS {
        let root = h.base.join(format!("live-{index}"));
        fs::create_dir(&root).unwrap();
        let result = h.work(json!({"kind":"open","root":root}));
        assert_eq!(result["status"], "Ready");
    }
    let ninth = h.reserve("ordinary");
    assert_eq!(
        h.ipc(json!({"action":"submit","operation":ninth,"input":{"kind":"open","root":h.root}}))
            .unwrap_err()["code"],
        "full"
    );
    assert_eq!(h.state.lock().projects.len(), MAX_PROJECTS);
    h.call(json!({"action":"app_shutdown"}));
    let end = Instant::now() + LIMIT;
    loop {
        let ids = h.state.lock().projects.keys().copied().collect::<Vec<_>>();
        for id in &ids {
            h.call(json!({"action":"acknowledge_shutdown","project":String::from(*id)}));
        }
        if h.state.poll() {
            break;
        }
        if Instant::now() >= end {
            let safe = ids
                .iter()
                .map(|id| h.call(json!({"action":"project_status","project":String::from(*id)})))
                .collect::<Vec<_>>();
            panic!(
                "normal exit deadline: {} {:?}",
                h.call(json!({"action":"app_status"})),
                safe
            );
        }
        thread::yield_now();
    }
    h.close_clean();
}

#[test]
fn f4_intentional_close_before_stop_after_join_and_retired_are_deterministic() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (_, session) = h.template(&p);
    let (entered, release) = provider.hold();
    crate::events::window_event(&h.state, "main", &tauri::WindowEvent::Focused(true));
    entered.recv_timeout(LIMIT).unwrap();
    h.call(json!({"action":"app_shutdown"}));
    let input =
        json!({"kind":"session_control","project":p,"session":session,"control":"revalidate"});
    assert_eq!(h.control(input.clone())["error"]["code"], "closed");
    assert!(
        !h.state.lock().projects[&p.clone().try_into().unwrap()]
            .control
            .shutdown_snapshot()
            .stop_requested
    );
    release.send(()).unwrap();
    joined(&h, &p);
    {
        let state = h.state.lock();
        let project = &state.projects[&p.clone().try_into().unwrap()];
        assert_eq!(
            project.control.request_stop(),
            Err(WorkerCategory::Unavailable)
        );
        assert!(request_project_stop(project).is_ok());
    }
    assert_eq!(h.control(input.clone())["error"]["code"], "closed");
    // status는 아직 registry owner의 정상 join을 설명한다. retirement 이후와 구분한다.
    let status = h.call(json!({"action":"project_status","project":p}));
    assert_eq!(status["shutdown"]["joined"], true);
    retire(&h, &p);
    let id = h.reserve("control");
    assert_eq!(
        h.ipc(json!({"action":"submit","operation":id,"input":input}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    assert_eq!(h.result(&id)["error"]["code"], "unknown_id");
    h.ack(&id);
    h.close_clean();
}

#[test]
fn f4_actual_worker_fault_keeps_unavailable_and_original_terminal_diagnostic() {
    let h = Harness::new();
    let p = h.open();
    let (_, session) = h.template(&p);
    let id: Id = p.clone().try_into().unwrap();
    let (client, registration) = {
        let state = h.state.lock();
        let project = &state.projects[&id];
        (
            project.client.clone(),
            project.sessions[&session.clone().try_into().unwrap()]
                .registration
                .clone(),
        )
    };
    // 실제 자원 해제 후 좁은 worker panic을 유발한다. app 종료/장애 flag를 주입하지 않는다.
    let cleanup = {
        let state = h.state.lock();
        state.projects[&id]
            .control
            .try_control(registration, |ctx, r| {
                assert!(ctx.end_session_observed(&r).unwrap().original.is_ok());
                ctx.remove_session(&r).unwrap();
                ctx.close_runtime().unwrap().unwrap();
            })
            .unwrap()
    };
    let end = Instant::now() + LIMIT;
    while client.try_take(&cleanup).unwrap().is_none() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    let ticket = {
        let state = h.state.lock();
        state.projects[&id]
            .control
            .try_control((), |_, ()| panic!("controlled worker fault"))
            .unwrap()
    };
    loop {
        if h.state.lock().projects[&id].control.snapshot().status == WorkerStatus::Unavailable {
            break;
        }
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    drop(ticket);
    h.call(json!({"action":"app_shutdown"}));
    assert_eq!(
        request_project_stop(&h.state.lock().projects[&id]),
        Err(WorkerCategory::Unavailable)
    );
    let op = h.submit(
        json!({"kind":"session_control","project":p,"session":session,"control":"revalidate"}),
    );
    let result = h.result(&op);
    assert_eq!(result["error"]["code"], "unavailable");
    assert!(h.result(&op) == result);
    h.ack(&op);
    loop {
        if h.state
            .lock()
            .projects
            .get_mut(&id)
            .unwrap()
            .control
            .try_join()
            .unwrap()
        {
            break;
        }
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    assert!(!h.state.poll());
}

fn hook(h: &Harness) -> windows::Win32::Foundation::HWND {
    use windows::{
        core::w,
        Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WINDOW_STYLE},
    };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("G13 FIX hook fixture"),
            WINDOW_STYLE::default(),
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    crate::platform::install(hwnd, h.state.clone()).unwrap();
    hwnd
}
fn native_finish(
    h: &Harness,
    hwnd: windows::Win32::Foundation::HWND,
    project_session: Option<(&str, &str)>,
) {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{DestroyWindow, SendMessageW},
    };
    let before = Arc::strong_count(&h.state);
    crate::platform::fail_removals(2);
    crate::events::cleanup_pass(&h.state);
    assert_eq!(crate::platform::removal_attempts(), 1);
    assert_eq!(Arc::strong_count(&h.state), before);
    assert!(!h.state.poll());
    let status = h.call(json!({"action":"app_status"}));
    assert_eq!(status["native_cleanup"]["phase"], "failed");
    assert_eq!(status["native_cleanup"]["firstError"], "platform_removal");
    assert_eq!(
        status["native_cleanup"]["nextAction"],
        "retry_native_cleanup"
    );
    for _ in 0..20 {
        h.state.poll();
        h.call(json!({"action":"app_shutdown"}));
        crate::events::cleanup_pass(&h.state);
    }
    assert_eq!(crate::platform::removal_attempts(), 1);
    let generation = status["native_cleanup"]["generation"].clone();
    let mut full = Vec::new();
    if let Some((p, s)) = project_session {
        for _ in 0..CONTROL {
            let op = h.submit(
                json!({"kind":"session_control","project":p,"session":s,"control":"revalidate"}),
            );
            assert_eq!(h.result(&op)["error"]["code"], "closed");
            full.push(op);
        }
        assert_eq!(
            h.ipc(json!({"action":"reserve","lane":"control"}))
                .unwrap_err()["code"],
            "full"
        );
    }
    h.call(json!({"action":"retry_native_cleanup","generation":generation}));
    h.call(json!({"action":"retry_native_cleanup","generation":generation}));
    if !full.is_empty() {
        crate::events::cleanup_pass(&h.state);
        assert_eq!(crate::platform::removal_attempts(), 1);
        for op in full {
            h.ack(&op);
        }
    }
    assert!(h.state.begin_native_cleanup());
    h.call(json!({"action":"retry_native_cleanup","generation":generation}));
    assert!(!h.state.begin_native_cleanup());
    assert!(crate::platform::remove().is_err());
    assert_eq!(crate::platform::removal_attempts(), 2);
    assert_eq!(Arc::strong_count(&h.state), before);
    let status = h.call(json!({"action":"app_status"}));
    assert_eq!(status["native_cleanup"]["firstError"], "platform_removal");
    assert_eq!(
        h.ipc(json!({"action":"retry_native_cleanup","generation":generation}))
            .unwrap_err()["code"],
        "wrong_binding"
    );
    h.call(json!({"action":"retry_native_cleanup","generation":status["native_cleanup"]["generation"]}));
    crate::events::cleanup_pass(&h.state);
    assert_eq!(crate::platform::removal_attempts(), 3);
    assert_eq!(Arc::strong_count(&h.state), before - 1);
    let status = h.call(json!({"action":"app_status"}));
    assert_eq!(status["native_cleanup"]["phase"], "complete");
    assert_eq!(status["native_cleanup"]["firstError"], "platform_removal");
    assert!(status["event_error"].is_null());
    assert!(h.state.poll());
    let generation = h.state.generation();
    unsafe {
        SendMessageW(hwnd, 0x218, Some(WPARAM(0x12)), Some(LPARAM(0)));
    }
    crate::platform::remove().unwrap();
    crate::events::cleanup_pass(&h.state);
    assert_eq!(h.state.generation(), generation);
    assert_eq!(crate::platform::removal_attempts(), 3);
    assert!(h.state.take_exit_approval());
    assert!(!h.state.take_exit_approval());
    unsafe { DestroyWindow(hwnd) }.unwrap();
}

#[test]
fn f5_native_failure_explicit_retry_preserves_hook_arc_and_first_error_until_real_remove() {
    let h = Harness::new();
    let p = h.open();
    let (_, s) = h.template(&p);
    let weak = Arc::downgrade(&h.state);
    let hwnd = hook(&h);
    for _ in 0..ORDINARY {
        h.reserve("ordinary");
    }
    for _ in 0..CONTROL {
        h.reserve("control");
    }
    h.call(json!({"action":"app_shutdown"}));
    joined(&h, &p);
    native_finish(&h, hwnd, Some((&p, &s)));
    assert!(weak.upgrade().is_some());
    // mock Runtime의 별도 managed AppState 참조는 hook 제거 수명이 아니다.
    // 별도 native 등록만 남긴 실제 Arc로 마지막 strong owner의 제거도 확인한다.
    let isolated = Arc::new(AppState::new(h.base.join("isolated-locks")));
    let isolated_weak = Arc::downgrade(&isolated);
    use windows::{
        core::w,
        Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
        },
    };
    let second = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("G13 owner fixture"),
            WINDOW_STYLE::default(),
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    crate::platform::install(second, isolated.clone()).unwrap();
    drop(isolated);
    assert!(isolated_weak.upgrade().is_some());
    crate::platform::remove().unwrap();
    assert!(isolated_weak.upgrade().is_none());
    unsafe { DestroyWindow(second) }.unwrap();
}

#[test]
fn f6_stale_input_no_draft_project_retirement_and_native_retry_allow_normal_exit() {
    let h = Harness::new();
    let hwnd = hook(&h);
    let s = stale(&h);
    h.ack(&s.operation);
    assert_eq!(h.control(json!({"kind":"session_control","project":s.project,"session":s.session,"control":"return_active"}))["error"]["code"],"no_draft");
    h.control(json!({"kind":"close","project":s.project}));
    joined(&h, &s.project);
    release_views(&h, &s.project);
    let release = h.reserve("control");
    assert_eq!(h.ipc(json!({"action":"submit","operation":release,"input":{"kind":"retire_project","project":s.project}})).unwrap_err()["code"],"owners_remain");
    assert_eq!(h.result(&release)["error"]["code"], "owners_remain");
    h.ack(&release);
    assert!(read(&h, &s.key)["intent"] == s.intent);
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.poll());
    abandon(&h, &s.key);
    retire(&h, &s.project);
    assert!(h.state.lock().projects.is_empty());
    native_finish(&h, hwnd, None);
}

#[test]
fn m42_fix001_terminal_initialization_failure_can_retire_without_normalizing_it() {
    let h = Harness::new();
    let healthy = h.open();
    let alias = h.root.join(".");
    let opened = h.work(json!({"kind":"open","root":alias}));
    assert_eq!(opened["status"], "InitializationFailed");
    let failed = opened["project"].as_str().unwrap();

    let closed = h.control(json!({"kind":"close","project":failed}));
    assert!(closed["error"].is_null());
    let end = Instant::now() + LIMIT;
    let terminal = loop {
        h.state.poll();
        let status = h.call(json!({"action":"project_status","project":failed}));
        if status["shutdown"]["reportPending"] == true {
            h.call(json!({"action":"acknowledge_shutdown","project":failed}));
        }
        if status["shutdown"]["joined"] == true && status["shutdown"]["reportPending"] == false {
            break status;
        }
        assert!(Instant::now() < end, "initialization failure join deadline");
        thread::yield_now();
    };
    assert_eq!(terminal["error"]["code"], "initialization_failed");
    assert_eq!(terminal["shutdown"]["normalExitAllowed"], false);
    assert_eq!(terminal["shutdown"]["resourcesComplete"], true);

    let retirement = submit(
        &h,
        "control",
        json!({"kind":"retire_project","project":failed}),
    );
    let retired = h.result(&retirement);
    assert_eq!(retired["kind"], "project_retired");
    assert_eq!(retired["shutdown"]["normalExitAllowed"], false);
    h.ack(&retirement);
    assert_eq!(h.state.lock().projects.len(), 1);
    assert!(h
        .state
        .lock()
        .projects
        .keys()
        .any(|id| String::from(*id) == healthy));
    h.close_clean();
}
