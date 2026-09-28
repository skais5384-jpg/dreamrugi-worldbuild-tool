//! 실제 guarded IPC→worker→파일 sink→recovery ack→release를 연결한다.
use super::*;
use crate::data::edit_recovery::{error::Stage, model::SaveState};

pub(super) fn unavailable(h: &Harness) {
    fs::create_dir_all(h.base.join("locks")).unwrap();
    fs::write(
        h.base.join("locks/edit-recovery"),
        b"fixture: storage unavailable",
    )
    .unwrap();
}

#[test]
fn recovery_worker_production_sink_failure_retry_ack_release_and_files_survive() {
    for composite in [false, true] {
        let provider = Arc::new(Provider::new());
        let h = Harness::with_provider(provider.clone());
        let project = h.open();
        let (template, _) = h.template(&project);
        let t = h.read_template(&project, &template);
        let created = h.work(
            json!({"kind":"create_document","project":project,"view":t["view"],"name":"document"}),
        );
        let document = created["artifact"].as_str().unwrap();
        let d = h.work(json!({"kind":"read_document","project":project,"document":document}));
        let session = h.session(
            &project,
            vec![d["view"].clone(), t["view"].clone()],
            if composite { "composite" } else { "document" },
        );
        let status = h.call(json!({"action":"session_status","project":project,"session":session}));
        assert_eq!(status["sink_connected"], true);
        assert!(status["recovery_error"].is_null());
        // 정상 Editing을 LockLost로 속여 명시적 보관을 구현하지 않는다.
        let preserve = h.control(json!({"kind":"session_control","project":project,"session":session,"control":"preserve"}));
        assert_eq!(preserve["error"]["code"], "no_recovery_condition");
        let before = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
        provider
            .lose
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut input = json!({"kind":"save_document","project":project,"session":session,"document":d["view"],"template":t["view"],"revision":"1","edits":[{"kind":"rename","name":"PRIVATE 정확한 원문"}]});
        if composite {
            input["kind"] = "save_composite".into();
            input["edit"] = json!({"kind":"name","name":"PRIVATE Template 초안"});
        }
        let operation = h.submit(input.clone());
        let result = h.result(&operation);
        assert_eq!(result["custody"], "Preserved");
        h.ack(&operation);
        let store = h.state.recovery.connect().unwrap();
        store.lock().unwrap().fault = Some(Stage::Reopen);
        let accept = json!({"kind":"session_control","project":project,"session":session,"control":"accept"});
        assert_eq!(
            h.control(accept.clone())["error"]["code"],
            "sink_unavailable"
        );
        let status = h.call(json!({"action":"session_status","project":project,"session":session}));
        assert_eq!(status["custody"], "Pending");
        assert_eq!(status["recovery_error"]["category"], "durability_uncertain");
        assert!(!status.to_string().contains("PRIVATE"));
        assert!(!h.control(json!({"kind":"session_control","project":project,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
        assert!(!h.state.lock().retained.is_empty());
        store.lock().unwrap().fault = None;
        assert!(h.control(accept)["error"].is_null());
        assert_eq!(
            h.call(json!({"action":"session_status","project":project,"session":session}))
                ["custody"],
            "DurablyAccepted"
        );
        let listing = store.lock().unwrap().list().unwrap();
        assert_eq!(listing.entries.len(), 1);
        let row = &listing.entries[0];
        let deposit = store
            .lock()
            .unwrap()
            .read(row.key.as_ref().unwrap(), row.deposit_id.as_ref().unwrap())
            .unwrap();
        assert_eq!(
            deposit.envelope().attempt.as_ref().unwrap().operation_id,
            operation
        );
        assert_eq!(
            deposit.envelope().attempt.as_ref().unwrap().result,
            SaveState::NotAttempted
        );
        assert_eq!(
            fs::read(h.root.join(format!("documents/{document}.json"))).unwrap(),
            before
        );
        assert!(h.control(json!({"kind":"session_control","project":project,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
        assert!(h.state.lock().retained.is_empty());
        provider
            .lose
            .store(false, std::sync::atomic::Ordering::SeqCst);
        h.close_clean();
        assert_eq!(store.lock().unwrap().list().unwrap().entries.len(), 1);
        drop(store);
    }
}

#[test]
fn recovery_initialization_failure_is_observable_and_reconnects_on_explicit_accept() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    unavailable(&h);
    let project = h.open();
    let (template, _) = h.template(&project);
    let t = h.read_template(&project, &template);
    let created = h.work(
        json!({"kind":"create_document","project":project,"view":t["view"],"name":"document"}),
    );
    let d =
        h.work(json!({"kind":"read_document","project":project,"document":created["artifact"]}));
    let session = h.session(
        &project,
        vec![d["view"].clone(), t["view"].clone()],
        "document",
    );
    let status = h.call(json!({"action":"session_status","project":project,"session":session}));
    assert_eq!(status["sink_connected"], false);
    assert!(!status["recovery_error"].is_null());
    let work = json!({"kind":"save_document","project":project,"session":session,"document":d["view"],"template":t["view"],"revision":"1","edits":[{"kind":"rename","name":"retained"}]});
    assert_eq!(h.work(work)["error"]["code"], "sink_unavailable");
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // 실제 provider 재검증 실패 후 preserve할 수 있다. UI의 정상 명시 deposit과 구분한다.
    h.control(json!({"kind":"session_control","project":project,"session":session,"control":"revalidate"}));
    fs::remove_file(h.base.join("locks/edit-recovery")).unwrap();
    let deadline = Instant::now() + LIMIT;
    loop {
        let status = h.call(json!({"action":"session_status","project":project,"session":session}));
        if status["custody"] == "Pending" {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(h.control(
        json!({"kind":"session_control","project":project,"session":session,"control":"accept"})
    )["error"]
        .is_null());
    assert!(h.control(json!({"kind":"session_control","project":project,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
    provider
        .lose
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.close_clean();
}

#[test]
fn collaborative_edit_rejects_busy_recovery_before_lock_and_canonical_write() {
    let provider = Arc::new(Provider::new());
    let first_base =
        std::env::temp_dir().join(format!("worldbuild-m7-busy-{}", uuid::Uuid::new_v4()));
    let first = Harness::at(first_base.clone(), backend::provider(), false);
    let first_project = first.open();
    let (template, _) = first.template(&first_project);
    let t = first.read_template(&first_project, &template);
    let created = first.work(
        json!({"kind":"create_document","project":first_project,"view":t["view"],"name":"original"}),
    );
    let document = created["artifact"].as_str().unwrap();
    let second_base =
        std::env::temp_dir().join(format!("worldbuild-m7-second-{}", uuid::Uuid::new_v4()));
    let second_root = second_base.join("project");
    copy_project(&first.root, &second_root);
    let h = Harness::at_recovery(
        second_base,
        provider.clone(),
        true,
        Some(first_base.join("locks/edit-recovery")),
    );
    let project = h.open();
    let path = h.root.join(format!("documents/{document}.json"));
    let before = fs::read(&path).unwrap();
    {
        let project_id = dto::Id::try_from(project.clone()).unwrap();
        let mut state = h.state.lock();
        let opened = state.projects.get_mut(&project_id).unwrap();
        opened.collaborative = true;
        opened.svn_provider = Some(Arc::new(crate::svn::SvnLockService::new(
            crate::svn::Manager::new(h.base.join("svn-test-app")),
            h.root.clone(),
        )));
    }
    let lock_calls = provider.threads.lock().unwrap().len();
    let rejected = h.work(json!({
        "kind":"document_workspace", "project":project,
        "request":{"action":"edit_begin","document":document}
    }));
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    assert_eq!(
        rejected["error"]["code"], "recovery_store_busy",
        "{rejected}"
    );
    assert_eq!(provider.threads.lock().unwrap().len(), lock_calls);
    assert_eq!(fs::read(&path).unwrap(), before);
    first.close_clean();
    drop(first);
    let editing = h.work(json!({
        "kind":"document_workspace", "project":project,
        "request":{"action":"edit_begin","document":document}
    }));
    assert_eq!(editing["value"]["kind"], "editing", "{editing}");
    let released = h.work(json!({
        "kind":"document_workspace", "project":project,
        "request":{"action":"edit_release","owner":editing["value"]["owner"],
            "generation":editing["value"]["generation"]}
    }));
    assert_eq!(released["value"]["kind"], "released", "{released}");
    h.close_clean();
    drop(h);
    fs::remove_dir_all(first_base).unwrap();
}

#[test]
fn collaborative_lock_loss_can_deposit_latest_input_without_changing_canonical() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let project = h.open();
    let (template, _) = h.template(&project);
    let t = h.read_template(&project, &template);
    let created = h.work(json!({"kind":"create_document","project":project,
        "view":t["view"],"name":"original"}));
    let document = created["artifact"].as_str().unwrap();
    {
        let project_id = dto::Id::try_from(project.clone()).unwrap();
        let mut state = h.state.lock();
        let opened = state.projects.get_mut(&project_id).unwrap();
        opened.collaborative = true;
        opened.svn_provider = Some(Arc::new(crate::svn::SvnLockService::new(
            crate::svn::Manager::new(h.base.join("svn-test-app")),
            h.root.clone(),
        )));
    }
    let editing = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_begin","document":document}}));
    assert_eq!(editing["value"]["kind"], "editing", "{editing}");
    let project_id = dto::Id::try_from(project.clone()).unwrap();
    let owner = dto::Id::try_from(editing["value"]["owner"].as_str().unwrap().to_owned()).unwrap();
    let key = {
        let state = h.state.lock();
        state.projects[&project_id].sessions[&owner]
            .key
            .clone()
            .unwrap()
    };
    let canonical = h.root.join(format!("documents/{document}.json"));
    let before = fs::read(&canonical).unwrap();
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    {
        let state = h.state.lock();
        state.projects[&project_id]
            .client
            .trigger(&key, TriggerReason::Foreground)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let completed = h.state.lock().projects[&project_id]
            .control
            .revalidation(&key)
            .unwrap()
            .completed;
        if completed > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "revalidation did not complete");
        thread::sleep(Duration::from_millis(10));
    }
    let mut body = editing["value"]["body"].clone();
    body["name"] = json!({"intent":"set","value":"잠금 상실 뒤 입력"});
    let failed = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_draft","owner":editing["value"]["owner"],
            "generation":"2","body":body,"save":true}}));
    assert_eq!(failed["value"]["kind"], "editing", "{failed}");
    assert!(
        matches!(
            failed["value"]["problem"].as_str(),
            Some("SaveFailed" | "SessionRejected")
        ),
        "{failed}"
    );
    assert_ne!(failed["value"]["saved_generation"], "2", "{failed}");
    assert_eq!(h.state.lock().projects[&project_id].failures.len(), 1);
    assert_eq!(fs::read(&canonical).unwrap(), before);
    let deposited = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_deposit","owner":editing["value"]["owner"],
            "generation":"2","body":body}}));
    assert_eq!(deposited["value"]["deposited"], true, "{deposited}");
    assert_eq!(fs::read(&canonical).unwrap(), before);
    provider
        .lose
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let released = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_release","owner":editing["value"]["owner"],
            "generation":"2"}}));
    assert_eq!(released["value"]["kind"], "released", "{released}");
    assert!(!h.state.lock().projects[&project_id]
        .sessions
        .contains_key(&owner));
    h.close_clean();
}

#[test]
#[ignore = "run with owned M7_FIX2_LOCKLOSS_WC and installed SVN CLI"]
fn m7_owned_svn_lock_loss_deposits_latest_input() {
    owned_svn_lock_loss_deposit_and_release(false);
}

#[test]
#[ignore = "run with owned M7_FIX2_LOCKLOSS_WC and installed SVN CLI"]
fn m789_owned_svn_successor_lock_allows_deposited_release_and_shutdown() {
    owned_svn_lock_loss_deposit_and_release(true);
}

fn owned_svn_lock_loss_deposit_and_release(successor: bool) {
    let base = std::path::PathBuf::from(std::env::var_os("M7_FIX2_LOCKLOSS_WC").unwrap());
    let manager = crate::svn::Manager::new(base.join("app-svn"));
    assert!(
        crate::svn::probe(
            &manager,
            Some(std::env::var("M7_SVN_TEST_CLI").unwrap()),
            None,
        )
        .installed
    );
    let h = Harness::owned_svn(base, manager);
    let project = h.open();
    let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let editing = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_begin","document":document}}));
    assert_eq!(editing["value"]["kind"], "editing", "{editing}");
    let canonical = h.root.join(format!("documents/{document}.json"));
    let before = fs::read(&canonical).unwrap();
    let cli = std::env::var_os("M7_SVN_TEST_CLI").unwrap();
    let url = std::process::Command::new(&cli)
        .args(["info", "--show-item", "url", "--non-interactive"])
        .arg(&canonical)
        .output()
        .unwrap();
    assert!(url.status.success());
    let url = String::from_utf8(url.stdout).unwrap().trim().to_owned();
    let change = if successor {
        std::process::Command::new(&cli)
            .args([
                "lock",
                "--force",
                "--username",
                "u6-successor",
                "--non-interactive",
                &url,
            ])
            .output()
            .unwrap()
    } else {
        std::process::Command::new(&cli)
            .args(["unlock", "--force", "--non-interactive"])
            .arg(&canonical)
            .output()
            .unwrap()
    };
    assert!(
        change.status.success(),
        "{}",
        String::from_utf8_lossy(&change.stderr)
    );
    let server_info = || {
        let result = std::process::Command::new(&cli)
            .args(["info", "--xml", "--non-interactive", &url])
            .output()
            .unwrap();
        assert!(result.status.success());
        let xml = String::from_utf8(result.stdout).unwrap();
        xml.find("<lock>")
            .map(|start| xml[start..xml.find("</lock>").unwrap() + "</lock>".len()].to_owned())
    };
    let successor_before = server_info();
    if successor {
        assert!(successor_before
            .as_deref()
            .unwrap()
            .contains("<owner>u6-successor</owner>"));
    }
    let mut body = editing["value"]["body"].clone();
    body["name"] = json!({"intent":"set","value":"잠금 상실 뒤 입력"});
    let failed = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_draft","owner":editing["value"]["owner"],
            "generation":"2","body":body,"save":true}}));
    assert_eq!(fs::read(&canonical).unwrap(), before);
    assert_ne!(failed["value"]["saved_generation"], "2");
    let deposited = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_deposit","owner":editing["value"]["owner"],
            "generation":"2","body":body}}));
    assert_eq!(
        deposited["value"]["deposited"], true,
        "deposit error={:?}, failed error={:?}",
        deposited["error"]["code"], failed["error"]["code"]
    );
    let repeat = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_deposit","owner":editing["value"]["owner"],
            "generation":"2","body":body}}));
    assert_eq!(
        repeat["value"]["deposited"], true,
        "repeat error={:?}",
        repeat["error"]["code"]
    );
    assert_eq!(fs::read(&canonical).unwrap(), before);
    let released = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_release","owner":editing["value"]["owner"],
            "generation":"2"}}));
    assert_eq!(
        released["value"]["kind"], "released",
        "release error={:?}",
        released["error"]["code"]
    );
    assert_eq!(fs::read(&canonical).unwrap(), before);
    assert_eq!(server_info(), successor_before);
    let project_id = dto::Id::try_from(project).unwrap();
    let owner = dto::Id::try_from(editing["value"]["owner"].as_str().unwrap().to_owned()).unwrap();
    assert!(!h.state.lock().projects[&project_id]
        .sessions
        .contains_key(&owner));
    h.close_clean();
    assert_eq!(server_info(), successor_before);
}

fn copy_project(source: &std::path::Path, destination: &std::path::Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_project(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
#[ignore = "owned fresh FSFS fixtures: M7_FIX001_REENTRY_BASE and M7_SVN_TEST_CLI"]
fn m789_fix001_stale_token_preserves_session_error_and_requires_explicit_update() {
    let base = std::path::PathBuf::from(std::env::var_os("M7_FIX001_REENTRY_BASE").unwrap());
    let cli = std::env::var_os("M7_SVN_TEST_CLI").unwrap();
    for other_owner in [false, true] {
        let fixture = base.join(if other_owner {
            "native-reentry-other"
        } else {
            "native-reentry-none"
        });
        let manager = crate::svn::Manager::new(fixture.join("app-svn"));
        assert!(crate::svn::probe(&manager, Some(cli.to_string_lossy().into()), None).installed);
        let h = Harness::owned_svn(fixture.clone(), manager);
        let project = h.open();
        let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let path = h.root.join(format!("documents/{document}.json"));
        let before = fs::read(&path).unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new(&cli)
                .args(args)
                .arg("--non-interactive")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        let local = path.to_str().unwrap();
        let url = run(&["info", "--show-item", "url", local])
            .trim()
            .to_owned();
        run(&["lock", "--username", "fix001-local", local]);
        if other_owner {
            run(&["lock", "--force", "--username", "fix001-successor", &url]);
        } else {
            run(&["unlock", "--force", &url]);
        }
        let server_lock = || {
            let xml = run(&["info", "--xml", &url]);
            xml.find("<lock>")
                .map(|start| xml[start..xml.find("</lock>").unwrap() + "</lock>".len()].to_owned())
        };
        let server_before = server_lock();
        let begin = || {
            h.work(json!({"kind":"document_workspace","project":project,
            "request":{"action":"edit_begin","document":document}}))
        };
        let rejected = begin();
        assert_eq!(rejected["error"]["code"], "session_rejected", "{rejected}");
        assert_eq!(
            rejected["error"]["diagnostic"]["category"], "StaleLockToken",
            "{rejected}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(server_lock(), server_before);
        fs::write(
            fixture.join("stale-rejection.json"),
            serde_json::to_vec_pretty(&rejected).unwrap(),
        )
        .unwrap();
        run(&["update", local]);
        if other_owner {
            let still_blocked = begin();
            assert_eq!(
                still_blocked["error"]["code"], "session_rejected",
                "{still_blocked}"
            );
            assert_ne!(
                still_blocked["error"]["diagnostic"]["category"],
                "StaleLockToken"
            );
            assert_eq!(server_lock(), server_before);
            fs::write(
                fixture.join("owner-rejection-after-update.json"),
                serde_json::to_vec_pretty(&still_blocked).unwrap(),
            )
            .unwrap();
            // Only the owned test successor explicitly releases its server lock.
            run(&["unlock", "--force", &url]);
            run(&["update", local]);
        }
        let editing = begin();
        assert_eq!(editing["value"]["kind"], "editing", "{editing}");
        let released = h.work(json!({"kind":"document_workspace","project":project,
            "request":{"action":"edit_release","owner":editing["value"]["owner"],"generation":"1"}}));
        assert_eq!(released["value"]["kind"], "released", "{released}");
        assert_eq!(fs::read(&path).unwrap(), before);
        h.close_clean();
    }
}

const STORE_CHILD: &str = "state::tests::recovery::collaborative_store_first_process_child";

#[test]
#[ignore = "run only as the owned two-process integration helper"]
fn collaborative_store_first_process_child() {
    let base = std::path::PathBuf::from(std::env::var_os("WB_M7_STORE_TEST_ROOT").unwrap());
    let h = Harness::at(base.join("first"), backend::provider(), false);
    let project = h.open();
    let (template, _) = h.template(&project);
    let t = h.read_template(&project, &template);
    let created = h.work(json!({"kind":"create_document", "project":project,
        "view":t["view"], "name":"first original"}));
    let document = created["artifact"].as_str().unwrap();
    let editing = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_begin","document":document}}));
    assert_eq!(editing["value"]["kind"], "editing", "{editing}");
    let mut body = editing["value"]["body"].clone();
    body["name"] = json!({"intent":"set","value":"첫 창 정상 저장"});
    let saved = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_draft","owner":editing["value"]["owner"],
            "generation":"2","body":body,"save":true}}));
    assert_eq!(saved["value"]["kind"], "editing", "{saved}");
    assert_eq!(saved["value"]["saved_generation"], "2", "{saved}");
    let released = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_release","owner":editing["value"]["owner"],
            "generation":"2"}}));
    assert_eq!(released["value"]["kind"], "released", "{released}");
    fs::write(base.join("ready"), document).unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    while !base.join("finish").exists() {
        assert!(Instant::now() < deadline, "first process release deadline");
        thread::sleep(Duration::from_millis(10));
    }
    h.close_clean();
}

#[test]
fn collaborative_two_processes_share_store_without_second_write() {
    use std::process::{Command, Stdio};
    let base = std::env::temp_dir().join(format!("worldbuild-m7-two-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", STORE_CHILD, "--ignored", "--nocapture"])
        .env("WB_M7_STORE_TEST_ROOT", &base)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    while !base.join("ready").exists() {
        if child.try_wait().unwrap().is_some() {
            panic!("first app process ended before opening its Store");
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("first app process setup deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let document = fs::read_to_string(base.join("ready")).unwrap();
    let second_base = base.join("second");
    copy_project(&base.join("first/project"), &second_base.join("project"));
    let provider = Arc::new(Provider::new());
    let h = Harness::at_recovery(
        second_base.clone(),
        provider.clone(),
        false,
        Some(base.join("first/locks/edit-recovery")),
    );
    let project = h.open();
    {
        let project_id = dto::Id::try_from(project.clone()).unwrap();
        let mut state = h.state.lock();
        let opened = state.projects.get_mut(&project_id).unwrap();
        opened.collaborative = true;
        opened.svn_provider = Some(Arc::new(crate::svn::SvnLockService::new(
            crate::svn::Manager::new(base.join("svn-test-app")),
            h.root.clone(),
        )));
    }
    let canonical = h.root.join(format!("documents/{document}.json"));
    let before = fs::read(&canonical).unwrap();
    let lock_calls = provider.threads.lock().unwrap().len();
    let denied = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_begin","document":document}}));
    assert_eq!(denied["error"]["code"], "recovery_store_busy", "{denied}");
    assert_eq!(provider.threads.lock().unwrap().len(), lock_calls);
    assert_eq!(fs::read(&canonical).unwrap(), before);
    fs::write(base.join("finish"), b"close normally").unwrap();
    assert!(
        child.wait().unwrap().success(),
        "first app failed to close normally"
    );
    let editing = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_begin","document":document}}));
    assert_eq!(editing["value"]["kind"], "editing", "{editing}");
    let mut body = editing["value"]["body"].clone();
    body["name"] = json!({"intent":"set","value":"둘째 창 저장 확인"});
    let saved = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_draft","owner":editing["value"]["owner"],
            "generation":"2","body":body.clone(),"save":true}}));
    assert_eq!(saved["value"]["saved_generation"], "2", "{saved}");
    assert!(saved["value"]["problem"].is_null(), "{saved}");
    let deposited = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_deposit","owner":editing["value"]["owner"],
            "generation":"2","body":body}}));
    assert_eq!(deposited["value"]["deposited"], true, "{deposited}");
    let store = h.state.recovery.connect().unwrap();
    let listing = store.lock().unwrap().list().unwrap();
    assert!(listing.complete && !listing.entries.is_empty());
    assert!(listing.entries.iter().all(|entry| entry.error.is_none()));
    drop(store);
    let released = h.work(json!({"kind":"document_workspace","project":project,
        "request":{"action":"edit_release","owner":editing["value"]["owner"],
            "generation":"2"}}));
    assert_eq!(released["value"]["kind"], "released", "{released}");
    assert_ne!(fs::read(&canonical).unwrap(), before);
    h.close_clean();
    drop(h);
    let reopened = Harness::at_recovery(
        second_base,
        backend::provider(),
        false,
        Some(base.join("first/locks/edit-recovery")),
    );
    let project = reopened.open();
    let read = reopened.work(json!({"kind":"read_document","project":project,
        "document":document}));
    assert_eq!(read["content"]["name"], "둘째 창 저장 확인", "{read}");
    reopened.close_clean();
    drop(reopened);
    fs::remove_dir_all(base).unwrap();
}
