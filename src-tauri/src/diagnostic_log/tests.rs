use super::*;
#[cfg(windows)]
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[test]
fn diagnostic_identifiers_are_closed_and_paths_are_rejected() {
    assert!(safe_id("operation_01-safe"));
    assert!(!safe_id("C:\\Users\\private"));
    assert!(!safe_id("https://example.test/?token=secret"));
    assert!(valid_fingerprint(&"a".repeat(64)));
    assert!(!valid_fingerprint("project-name"));
}

#[test]
fn owned_log_and_marker_names_are_strict() {
    let id = uuid::Uuid::new_v4();
    assert!(valid_marker_name(&format!("session-{id}.active")));
    assert!(valid_event_name(&format!("events-{id}-000001.jsonl")));
    assert!(!valid_marker_name("session-..\\outside.active"));
    assert!(!valid_event_name("events-private.jsonl"));
}

#[test]
fn export_contains_only_closed_aggregates_and_not_the_destination_path() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-private-user-path-{}",
        uuid::Uuid::new_v4()
    )));
    fs::create_dir(&fixture.0).unwrap();
    let destination = fixture.0.join("diagnostic-export.json");
    let fingerprint = "a".repeat(64);
    let result = export(
        &destination,
        Some(ProjectSnapshot {
            fingerprint: fingerprint.clone(),
            observed_at_utc: "2026-09-21T00:00:00.000Z".to_owned(),
            inspection_complete: true,
            scanned_files: 4,
            used_assets: 3,
            unused_assets: 2,
            missing_assets: 1,
            corrupt_assets: 0,
            uncertain_assets: 0,
            trash_assets: 1,
        }),
        None,
    )
    .unwrap();
    let bytes = fs::read(&destination).unwrap();
    let body = String::from_utf8(bytes.clone()).unwrap();
    assert_eq!(result.outcome, "published");
    assert_eq!(result.size, Some(bytes.len() as u64));
    assert_eq!(
        result.sha256,
        Some(crate::data::edit_recovery::model::digest(&bytes))
    );
    assert!(body.contains(&fingerprint));
    assert!(body.contains("\"documentContent\"") || body.contains("document_content"));
    assert!(!body.contains("worldbuild-private-user-path"));
    assert!(!body.contains(destination.to_string_lossy().as_ref()));
    assert!(!body.contains("secret document body"));
    assert!(!body.contains("https://example.test/?token=secret"));
}

#[test]
fn export_faults_preserve_destination_and_report_cleanup_truthfully() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    for point in [
        ExportTestPoint::Write,
        ExportTestPoint::Sync,
        ExportTestPoint::Readback,
        ExportTestPoint::Compare,
        ExportTestPoint::Publish,
    ] {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "worldbuild-diagnostic-export-fault-{}",
            uuid::Uuid::new_v4()
        )));
        fs::create_dir(&fixture.0).unwrap();
        let destination = fixture.0.join("report.json");
        let result =
            with_export_test_failures(&[point], || export(&destination, None, None).unwrap());
        assert_eq!(result.outcome, "not_applied");
        assert!(!result.cleanup_required);
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
    }

    let fixture = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-diagnostic-export-cleanup-{}",
        uuid::Uuid::new_v4()
    )));
    fs::create_dir(&fixture.0).unwrap();
    let destination = fixture.0.join("report.json");
    let result = with_export_test_failures(
        &[ExportTestPoint::Compare, ExportTestPoint::Cleanup],
        || export(&destination, None, None).unwrap(),
    );
    assert_eq!(result.outcome, "not_applied_cleanup_required");
    assert!(result.cleanup_required);
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}

#[test]
fn export_rejects_managed_alias_and_collision_without_touching_canaries() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-diagnostic-export-boundary-{}",
        uuid::Uuid::new_v4()
    )));
    let project = fixture.0.join("project");
    let outside = fixture.0.join("outside");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let managed_alias = project.join("..").join("project").join("report.json");
    assert_eq!(
        export(&managed_alias, None, Some(&project))
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    let destination = outside.join("report.json");
    fs::write(&destination, b"destination-canary").unwrap();
    let error = export(&destination, None, Some(&project)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&destination).unwrap(), b"destination-canary");
    assert!(project.exists());
}

#[test]
fn logger_write_rotation_and_marker_faults_only_degrade_diagnostics() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let base = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-diagnostic-logger-fault-{}",
        uuid::Uuid::new_v4()
    )));
    fs::create_dir(&base.0).unwrap();
    let make = |root: PathBuf| {
        let id = uuid::Uuid::new_v4();
        State {
            root,
            guard: None,
            session_id: id.to_string(),
            marker: None,
            marker_name: format!("session-{id}.active"),
            segment: 0,
            events: VecDeque::new(),
            dropped: 0,
            previous_exit_unconfirmed: 0,
            available: false,
        }
    };
    let event = |session_id: String| SafeEvent {
        schema_version: 1,
        session_id,
        observed_at_utc: None,
        feature: "asset_maintenance",
        stage: "complete",
        category: "io",
        outcome: "partial",
        correlation: Some("safe-correlation".into()),
        project_fingerprint: Some("a".repeat(64)),
    };

    let mut write_state = make(base.0.join("write"));
    initialize_inner(&mut write_state).unwrap();
    write_state.available = true;
    let write_event = event(write_state.session_id.clone());
    with_log_test_failures(&[LogTestPoint::Write], || {
        retain_event(&mut write_state, write_event);
    });
    assert!(!write_state.available);
    assert_eq!(write_state.dropped, 1);
    assert_eq!(write_state.events.len(), 1);

    let mut rotate_state = make(base.0.join("rotate"));
    initialize_inner(&mut rotate_state).unwrap();
    rotate_state.available = true;
    let current = rotate_state.root.join(log_name(&rotate_state));
    let file = fs::File::create(current).unwrap();
    file.set_len(MAX_LOG_BYTES).unwrap();
    drop(file);
    let rotate_event = event(rotate_state.session_id.clone());
    with_log_test_failures(&[LogTestPoint::Rotate], || {
        retain_event(&mut rotate_state, rotate_event);
    });
    assert!(!rotate_state.available);
    assert_eq!(rotate_state.dropped, 1);
    assert_eq!(rotate_state.events.len(), 1);

    let mut marker_state = make(base.0.join("marker"));
    initialize_inner(&mut marker_state).unwrap();
    marker_state.available = true;
    with_log_test_failures(&[LogTestPoint::MarkerCleanup], || {
        mark_clean_state(&mut marker_state);
    });
    assert!(!marker_state.available);
    assert_eq!(marker_state.dropped, 1);
}

#[cfg(windows)]
#[test]
fn live_session_is_not_misreported_and_closed_handle_is_detected_as_unclean() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-diagnostic-session-{}",
        uuid::Uuid::new_v4()
    )));
    let make = |id: uuid::Uuid| State {
        root: fixture.0.clone(),
        guard: None,
        session_id: id.to_string(),
        marker: None,
        marker_name: format!("session-{id}.active"),
        segment: 0,
        events: VecDeque::new(),
        dropped: 0,
        previous_exit_unconfirmed: 0,
        available: false,
    };
    let mut first = make(uuid::Uuid::new_v4());
    initialize_inner(&mut first).unwrap();
    let mut concurrent = make(uuid::Uuid::new_v4());
    initialize_inner(&mut concurrent).unwrap();
    assert_eq!(concurrent.previous_exit_unconfirmed, 0);

    // 강제 종료처럼 marker handle만 닫고 clean disposition은 적용하지 않는다.
    drop(first.marker.take());
    drop(first.guard.take());
    let mut next = make(uuid::Uuid::new_v4());
    initialize_inner(&mut next).unwrap();
    assert_eq!(next.previous_exit_unconfirmed, 1);

    for state in [&mut concurrent, &mut next] {
        if let Some(marker) = state.marker.take() {
            native::cleanup(&marker).unwrap();
        }
        state.guard.take();
    }
}

#[cfg(windows)]
#[test]
#[ignore = "controller kills this process after observing the live marker"]
fn diagnostic_marker_child() {
    let Some(root) = std::env::var_os("WB_M545_DIAGNOSTIC_ROOT") else {
        return;
    };
    initialize(PathBuf::from(root));
    let ready = PathBuf::from(std::env::var_os("WB_M545_DIAGNOSTIC_READY").unwrap());
    fs::write(ready, b"marker-live").unwrap();
    if std::env::var("WB_M545_DIAGNOSTIC_MODE").as_deref() == Ok("clean") {
        mark_clean();
        return;
    }
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(windows)]
#[test]
fn killed_session_is_unconfirmed_but_explicit_clean_shutdown_is_not() {
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn state(root: &Path) -> State {
        let id = uuid::Uuid::new_v4();
        State {
            root: root.to_path_buf(),
            guard: None,
            session_id: id.to_string(),
            marker: None,
            marker_name: format!("session-{id}.active"),
            segment: 0,
            events: VecDeque::new(),
            dropped: 0,
            previous_exit_unconfirmed: 0,
            available: false,
        }
    }
    let base = Fixture(std::env::temp_dir().join(format!(
        "worldbuild-diagnostic-process-{}",
        uuid::Uuid::new_v4()
    )));
    fs::create_dir(&base.0).unwrap();
    let test_name = "diagnostic_log::tests::diagnostic_marker_child";
    let crash_root = base.0.join("crash");
    let crash_ready = base.0.join("crash-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env("WB_M545_DIAGNOSTIC_ROOT", &crash_root)
        .env("WB_M545_DIAGNOSTIC_READY", &crash_ready)
        .env("WB_M545_DIAGNOSTIC_MODE", "crash")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !crash_ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("diagnostic child exited before marker observation: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(crash_ready.exists());
    child.kill().unwrap();
    child.wait().unwrap();
    let mut after_crash = state(&crash_root);
    initialize_inner(&mut after_crash).unwrap();
    assert_eq!(after_crash.previous_exit_unconfirmed, 1);
    if let Some(marker) = after_crash.marker.take() {
        native::cleanup(&marker).unwrap();
    }
    after_crash.guard.take();

    let clean_root = base.0.join("clean");
    let clean_ready = base.0.join("clean-ready");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env("WB_M545_DIAGNOSTIC_ROOT", &clean_root)
        .env("WB_M545_DIAGNOSTIC_READY", &clean_ready)
        .env("WB_M545_DIAGNOSTIC_MODE", "clean")
        .status()
        .unwrap();
    assert!(status.success());
    assert!(clean_ready.exists());
    let mut after_clean = state(&clean_root);
    initialize_inner(&mut after_clean).unwrap();
    assert_eq!(after_clean.previous_exit_unconfirmed, 0);
    if let Some(marker) = after_clean.marker.take() {
        native::cleanup(&marker).unwrap();
    }
    after_clean.guard.take();
}
