use super::*;
use tauri::Manager as _;

#[test]
fn m8_startup_gate_blocks_project_work_but_keeps_local_recovery_and_close() {
    let h = Harness::new();
    ui(&h);
    h.state.hold_project_startup();
    let op = h.reserve("ordinary");
    assert!(h
        .ipc(json!({"action":"submit","operation":op,"input":{"kind":"open","root":h.root}}))
        .is_err());
    assert!(!h.state.update_work_allowed());
    assert!(h.state.update_check_allowed());
    assert!(h.state.verify_recovery_update_handoff().unwrap().is_empty());
    h.state.allow_project_startup();
    h.call(json!({"action":"submit","operation":op,"input":{"kind":"open","root":h.root}}));
    assert!(!h.result(&op)["project"].is_null());
    h.ack(&op);
    finish(&h);
}

#[test]
fn m8_broken_recovery_cannot_pass_handoff_proof() {
    let h = Harness::new();
    fs::create_dir_all(h.base.join("locks")).unwrap();
    fs::write(h.base.join("locks/edit-recovery"), b"unavailable").unwrap();
    assert!(h.state.verify_recovery_update_handoff().is_err());
    finish(&h);
}

#[cfg(feature = "updater-test")]
#[test]
#[ignore = "requires a new owned loopback fixture matching the compiled test key"]
fn m8_native_cancel_ready_retry_and_single_handoff() {
    let fixture = PathBuf::from(std::env::var("M8_UPDATER_FIXTURE").unwrap());
    fs::write(
        fixture.join("fixture-control.json"),
        json!({"scenario":"normal","slow":false}).to_string(),
    )
    .unwrap();
    let h = Harness::new();
    ui(&h);
    h.state.hold_project_startup();
    h.app
        .handle()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .unwrap();
    let settings = h.base.join("fix002-updater-settings");
    h.app
        .manage(crate::svn::Manager::new(h.base.join("svn-config")));
    h.app.manage(Arc::new(crate::updater::Manager::new(
        settings.clone(),
        "0.2.0".into(),
    )));
    let call = |command, body| {
        let mut request = Harness::request(command, Vec::new(), "http://tauri.localhost");
        request.body = tauri::ipc::InvokeBody::Json(body);
        tauri::test::get_ipc_response(&h.main, request)
    };
    let checked = call("updater_check", json!({"retry":false}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(checked["phase"], "available");
    let candidate = checked["candidate"].clone();
    let ready = call("updater_download", json!({"candidate":candidate}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(ready["phase"], "ready");
    let cancelled = call("updater_cancel", json!({"candidate":candidate}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(cancelled["phase"], "cancelled");
    let repeated = call("updater_cancel", json!({"candidate":candidate}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(cancelled["revision"], repeated["revision"]);
    assert!(call("updater_prepare", json!({"candidate":candidate})).is_err());
    crate::updater::handoff(h.app.handle(), &h.state);
    assert!(!settings.join("updater-test-receipt.json").exists());
    assert!(call("updater_download", json!({"candidate":"different"})).is_err());
    let ready = call("updater_download", json!({"candidate":candidate}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(ready["phase"], "ready");
    assert!(call("updater_prepare", json!({"candidate":"different"})).is_err());
    let preparing = call("updater_prepare", json!({"candidate":candidate}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(preparing["phase"], "preparing");
    assert!(call("updater_cancel", json!({"candidate":candidate})).is_err());
    decision(&h, true);
    let until = Instant::now() + Duration::from_secs(5);
    while !h.state.exit_approved() && Instant::now() < until {
        h.state.poll();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state.exit_approved());
    crate::updater::handoff(h.app.handle(), &h.state);
    crate::updater::handoff(h.app.handle(), &h.state);
    let receipt: Value =
        serde_json::from_slice(&fs::read(settings.join("updater-test-receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["candidate"], candidate);
    assert_eq!(receipt["boundaryCalls"], 1);
    assert_eq!(receipt["actualInstallerCalls"], 0);
    assert_eq!(receipt["nativeApprovalConsumed"], true);
}

#[cfg(feature = "updater-test")]
#[test]
#[ignore = "requires a separate owned loopback fixture matching the compiled test key"]
fn m8_native_startup_network_retry_timeout_and_continue() {
    let fixture = PathBuf::from(std::env::var("M8_UPDATER_FIXTURE").unwrap());
    let h = Harness::new();
    ui(&h);
    h.state.hold_project_startup();
    h.app
        .handle()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .unwrap();
    h.app.manage(Arc::new(crate::updater::Manager::new(
        h.base.join("updater-settings"),
        "0.2.0".into(),
    )));
    let count = || -> u64 {
        let value: Value =
            serde_json::from_slice(&fs::read(fixture.join("counters.json")).unwrap()).unwrap();
        value["checks"].as_u64().unwrap()
    };
    let before = if fixture.join("counters.json").exists() {
        count()
    } else {
        0
    };
    for (scenario, retry, expected) in [("network", false, "network"), ("timeout", true, "timeout")]
    {
        fs::write(
            fixture.join("fixture-control.json"),
            json!({"scenario":scenario,"slow":false}).to_string(),
        )
        .unwrap();
        let start = Instant::now();
        let mut request = Harness::request("updater_check", Vec::new(), "http://tauri.localhost");
        request.body = tauri::ipc::InvokeBody::Json(json!({"retry":retry}));
        let status = tauri::test::get_ipc_response(&h.main, request)
            .unwrap()
            .deserialize::<Value>()
            .unwrap();
        assert_eq!(status["phase"], "failed");
        assert_eq!(status["error"], expected);
        assert_eq!(status["startupComplete"], false);
        assert!(!h.state.update_work_allowed());
        if scenario == "timeout" {
            assert!(start.elapsed() >= Duration::from_secs(9));
            assert!(start.elapsed() < Duration::from_secs(12));
        }
    }
    assert_eq!(count(), before + 2);
    let status = tauri::test::get_ipc_response(
        &h.main,
        Harness::request("updater_continue", Vec::new(), "http://tauri.localhost"),
    )
    .unwrap()
    .deserialize::<Value>()
    .unwrap();
    assert_eq!(status["phase"], "skipped");
    assert_eq!(status["startupComplete"], true);
    assert!(h.state.update_work_allowed());
    let mut request = Harness::request("updater_check", Vec::new(), "http://tauri.localhost");
    request.body = tauri::ipc::InvokeBody::Json(json!({"retry":true}));
    let status = tauri::test::get_ipc_response(&h.main, request)
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(status["phase"], "skipped");
    assert_eq!(count(), before + 2);
    finish(&h);
}

#[test]
fn m8_updater_direct_plugin_and_foreign_commands_are_denied() {
    let h = Harness::new();
    h.app
        .handle()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .unwrap();
    h.app.manage(Arc::new(crate::updater::Manager::new(
        h.root.join("updater-settings"),
        "0.2.0".into(),
    )));
    for command in [
        "plugin:updater|check",
        "plugin:updater|download",
        "plugin:updater|install",
        "plugin:updater|download_and_install",
    ] {
        let mut request = Harness::request(command, Vec::new(), "http://tauri.localhost");
        request.body = tauri::ipc::InvokeBody::Json(
            json!({"rid":0,"endpoints":["http://127.0.0.1:1/arbitrary"]}),
        );
        let error = tauri::test::get_ipc_response(&h.main, request).unwrap_err();
        assert!(
            error.to_string().contains("not allowed"),
            "{command}: {error}"
        );
    }
    let main = tauri::test::get_ipc_response(
        &h.main,
        Harness::request("updater_status", Vec::new(), "http://tauri.localhost"),
    )
    .unwrap()
    .deserialize::<Value>()
    .unwrap();
    assert_eq!(main["currentVersion"], "0.2.0");
    let foreign = tauri::WebviewWindowBuilder::new(&h.app, "updater-foreign", Default::default())
        .build()
        .unwrap();
    assert!(tauri::test::get_ipc_response(
        &foreign,
        Harness::request("updater_status", Vec::new(), "http://tauri.localhost")
    )
    .is_err());
    finish(&h);
}

fn ui(h: &Harness) {
    h.call(json!({"action":"ui_ready"}));
}
fn decision(h: &Harness, proceed: bool) {
    let attempt = h.call(json!({"action":"ui_close_status"}))["attempt"].clone();
    h.call(json!({"action":"ui_close_decision","attempt":attempt,"proceed":proceed}));
}
fn finish(h: &Harness) {
    h.state.request_shutdown();
    if !h.call(json!({"action":"ui_close_status"}))["attempt"].is_null() {
        decision(h, true);
    }
    h.close_clean();
}

#[test]
fn m8_cancel_and_stale_approval_leave_work_available() {
    let h = Harness::new();
    ui(&h);
    h.state.prepare_update("candidate-a").unwrap();
    assert!(h.state.prepare_update("candidate-b").is_err());
    assert!(!h.state.consume_update_approval("candidate-a"));
    decision(&h, false);
    assert!(h.state.update_intent().is_none());
    assert!(!h.state.consume_update_approval("candidate-a"));
    assert!(h.state.update_work_allowed());
    let p = h.open();
    assert!(!p.is_empty());
    finish(&h);
}

#[test]
fn m8_final_approval_blocks_new_work_and_is_consumed_once() {
    let h = Harness::new();
    ui(&h);
    h.state.prepare_update("candidate-a").unwrap();
    decision(&h, true);
    assert!(h.state.poll());
    assert!(!h.state.take_exit_approval());
    assert!(!h.state.consume_update_approval("candidate-b"));
    assert!(h.state.consume_update_approval("candidate-a"));
    assert!(!h.state.consume_update_approval("candidate-a"));
    assert!(h
        .ipc(json!({"action":"reserve","lane":"ordinary"}))
        .is_err());
    assert!(h.ipc(json!({"action":"reserve","lane":"control"})).is_err());
}

#[test]
fn m8_pending_native_cleanup_and_retained_input_prevent_install() {
    let h = Harness::new();
    ui(&h);
    h.state.native_registered();
    h.state.prepare_update("candidate-a").unwrap();
    decision(&h, true);
    assert!(!h.state.poll());
    assert!(!h.state.consume_update_approval("candidate-a"));
    assert!(h.state.begin_native_cleanup());
    h.state.event_failure(Code::PlatformRemoval);
    assert!(!h.state.consume_update_approval("candidate-a"));
    h.state.native_removed();
    assert!(h.state.consume_update_approval("candidate-a"));
}

#[test]
fn m8_new_owner_between_prepare_and_close_keeps_installer_unreachable() {
    let h = Harness::new();
    ui(&h);
    h.state.prepare_update("candidate-a").unwrap();
    h.state
        .lock()
        .workspace_owners
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    decision(&h, true);
    assert!(!h.state.lock().closed);
    assert!(!h.state.consume_update_approval("candidate-a"));
    h.state
        .lock()
        .workspace_owners
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    h.state.request_shutdown();
    decision(&h, true);
    assert!(h.state.consume_update_approval("candidate-a"));
}

#[test]
fn m8_unrecovered_results_keep_shutdown_and_installer_blocked() {
    let h = Harness::new();
    ui(&h);
    let p = h.open();
    let operation=h.submit(json!({"kind":"create_template","project":p,"name":"보존할 미회수 결과","presentation":null}));
    h.state.prepare_update("candidate-a").unwrap();
    decision(&h, true);
    assert!(!h.state.consume_update_approval("candidate-a"));
    let result = h.result(&operation);
    assert_eq!(result["disk"], "committed");
    h.ack(&operation);
    h.close_clean();
    assert!(h.state.consume_update_approval("candidate-a"));
}

#[test]
#[ignore = "set M8_GUI_FIXTURE to a new owned directory"]
fn m8_prepare_gui_fixture() {
    let base = PathBuf::from(std::env::var("M8_GUI_FIXTURE").expect("owned new directory"));
    assert!(!base.exists());
    let h = Harness::at(base, backend::provider(), false);
    let p = h.open();
    let (template, _) = h.template(&p);
    for name in [
        "한글 입력·저장 시험",
        "설치 전 보관 시험",
        "읽기 전용 확인 문서",
    ] {
        let read = h.read_template(&p, &template);
        let result =
            h.work(json!({"kind":"create_document","project":p,"view":read["view"],"name":name}));
        assert_eq!(result["disk"], "committed");
    }
    h.close_clean();
    let rows = std::fs::read_dir(h.root.join("documents")).unwrap().count();
    assert_eq!(rows, 3);
}

#[test]
#[ignore = "set M8_FIX_GUI_BASE and M8_FIX_RECOVERY_ROOT to new owned directories"]
fn m8_fix_prepare_persistent_gui_fixture() {
    let base = PathBuf::from(std::env::var("M8_FIX_GUI_BASE").unwrap());
    let recovery = PathBuf::from(std::env::var("M8_FIX_RECOVERY_ROOT").unwrap());
    assert!(!base.exists() && !recovery.exists());
    fs::create_dir_all(&recovery).unwrap();
    let recovery = fs::canonicalize(recovery).unwrap();
    if let Ok(pending_root) = std::env::var("M8_FIX_PENDING_ROOT") {
        fs::create_dir_all(&pending_root).unwrap();
        let store = crate::updater::handoff::Store::new(fs::canonicalize(pending_root).unwrap());
        store.preserve(crate::updater::handoff::PendingSvn {
            operation_id:uuid::Uuid::new_v4().to_string(),project_fingerprint:"c".repeat(64),requested_paths:vec!["documents/fix001-owned-example.json".into()],outcome:"unknown".into(),
            observed:json!({"error":"svn_commit_unverified","fixture":true,"serverCommitExecuted":false}),
        }).unwrap();
        assert_eq!(store.verify().unwrap().len(), 1);
    }
    let provider = Arc::new(Provider::new());
    let h = Harness::at_recovery(base, provider.clone(), false, Some(recovery));
    let project = h.open();
    let (template, _) = h.template(&project);
    let t = h.read_template(&project, &template);
    let created = h.work(json!({"kind":"create_document","project":project,"view":t["view"],"name":"FIX001 소유 시험 문서"}));
    let document = created["artifact"].as_str().unwrap();
    let d = h.work(json!({"kind":"read_document","project":project,"document":document}));
    let session = h.session(
        &project,
        vec![d["view"].clone(), t["view"].clone()],
        "document",
    );
    let before = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let operation=h.submit(json!({"kind":"save_document","project":project,"session":session,"document":d["view"],"template":t["view"],"revision":"1","edits":[{"kind":"rename","name":"FIX001 다시 읽을 보관 입력"}]}));
    assert_eq!(h.result(&operation)["custody"], "Preserved");
    h.ack(&operation);
    assert!(h.control(
        json!({"kind":"session_control","project":project,"session":session,"control":"accept"})
    )["error"]
        .is_null());
    assert_eq!(h.state.verify_recovery_update_handoff().unwrap().len(), 1);
    assert!(h.control(json!({"kind":"session_control","project":project,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
    provider
        .lose
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.close_clean();
    assert_eq!(
        fs::read(h.root.join(format!("documents/{document}.json"))).unwrap(),
        before
    );
    assert_eq!(h.state.verify_recovery_update_handoff().unwrap().len(), 1);
}
