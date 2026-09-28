use super::*;

#[test]
fn m523_native_request_boundary_copies_backs_up_lists_and_restores_exact_set() {
    let h = Harness::new();
    let project = h.open();
    let (template, session) = h.template(&project);
    assert!(h.control(json!({
        "kind":"session_control",
        "project":project,
        "session":session,
        "control":"end"
    }))["error"]
        .is_null());

    let copies = h.base.join("copies");
    let storage = h.base.join("backups");
    fs::create_dir(&copies).unwrap();
    fs::create_dir(&storage).unwrap();
    let copied = h.work(json!({
        "kind":"project_copy",
        "project":project,
        "parent":copies,
        "name":"독립 사본"
    }));
    assert_eq!(copied["kind"], "project_data", "{copied}");
    assert_eq!(copied["action"], "copy");
    let source_path = h.root.join(format!("templates/{template}.json"));
    let copied_path = copies
        .join("독립 사본")
        .join(format!("templates/{template}.json"));
    assert_eq!(
        fs::read(&source_path).unwrap(),
        fs::read(&copied_path).unwrap()
    );

    let created = h.work(json!({
        "kind":"backup_create",
        "project":project,
        "storage":storage,
        "label":"첫 백업"
    }));
    assert_eq!(created["kind"], "project_data", "{created}");
    assert_eq!(created["action"], "backup_create");
    assert_eq!(created["backup"]["status"], "verified");
    let locator = created["backup"]["locator"].as_str().unwrap().to_owned();

    let listed = h.work(json!({
        "kind":"backup_list",
        "project":project,
        "storage":storage
    }));
    assert_eq!(listed["backups"].as_array().unwrap().len(), 1);
    assert_eq!(listed["backups"][0]["label"], "첫 백업");
    assert_eq!(listed["backups"][0]["status"], "verification_required");
    assert!(listed["nextCursor"].is_null());

    let extra = h.root.join("documents/extra.json");
    fs::create_dir_all(extra.parent().unwrap()).unwrap();
    fs::write(&extra, b"not in snapshot").unwrap();
    fs::write(&source_path, b"changed after backup").unwrap();
    fs::create_dir_all(h.root.join(".git")).unwrap();
    fs::write(h.root.join(".git/canary"), b"keep").unwrap();
    let restored = h.work(json!({
        "kind":"restore_current",
        "project":project,
        "locator":locator,
        "safety_storage":storage
    }));
    assert_eq!(restored["kind"], "project_data", "{restored}");
    assert_eq!(restored["action"], "restore_current");
    assert_eq!(restored["outcome"], "applied_verified", "{restored}");
    assert!(restored["safety"].is_object());
    assert!(!extra.exists());
    assert_eq!(
        fs::read(&source_path).unwrap(),
        fs::read(&copied_path).unwrap()
    );
    assert_eq!(fs::read(h.root.join(".git/canary")).unwrap(), b"keep");

    let status = h.call(json!({"action":"project_status","project":project}));
    assert_eq!(status["runtime"], "Pending");
    let stale_read = h.work(json!({"kind":"list_templates","project":project}));
    assert_eq!(stale_read["kind"], "rejected", "{stale_read}");
    assert_eq!(stale_read["error"]["code"], "runtime_rejected");
    assert!(h.control(json!({"kind":"close","project":project}))["error"].is_null());
    h.close_clean();
}

#[test]
fn m523_home_restore_revalidates_backup_and_rejects_occupied_target() {
    let h = Harness::new();
    let project = h.open();
    let storage = h.base.join("backups");
    let destination = h.base.join("restored");
    fs::create_dir(&storage).unwrap();
    fs::create_dir(&destination).unwrap();
    let created = h.work(json!({
        "kind":"backup_create",
        "project":project,
        "storage":storage,
        "label":null
    }));
    let locator = created["backup"]["locator"].as_str().unwrap();
    let inspected = h.work(json!({"kind":"backup_inspect","locator":locator}));
    assert_eq!(inspected["action"], "inspect", "{inspected}");
    fs::create_dir(destination.join("점유 대상")).unwrap();
    fs::write(destination.join("점유 대상/canary"), b"keep").unwrap();
    let rejected = h.work(json!({
        "kind":"restore_new",
        "locator":locator,
        "parent":destination,
        "name":"점유 대상"
    }));
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    assert_eq!(rejected["error"]["code"], "restore_rejected");
    assert_eq!(
        fs::read(destination.join("점유 대상/canary")).unwrap(),
        b"keep"
    );
    h.close_clean();
}

#[test]
fn m523_recovery_retry_success_returns_safety_only_in_public_dto() {
    let h = Harness::new();
    let project = h.open();
    let (template, session) = h.template(&project);
    assert!(h.control(json!({
        "kind":"session_control",
        "project":project,
        "session":session,
        "control":"end"
    }))["error"]
        .is_null());
    let storage = h.base.join("backups");
    fs::create_dir(&storage).unwrap();
    let created = h.work(json!({
        "kind":"backup_create",
        "project":project,
        "storage":storage,
        "label":"선택 백업"
    }));
    let selected_id = created["backup"]["id"].as_str().unwrap().to_owned();
    let locator = created["backup"]["locator"].as_str().unwrap().to_owned();
    fs::write(
        h.root.join(format!("templates/{template}.json")),
        b"changed before restore",
    )
    .unwrap();
    crate::data::project_backup::set_root_test_faults(
        &h.root,
        vec!["restore_cleanup_after_apply", "recover_pending_once"],
    );

    let restored = h.work(json!({
        "kind":"restore_current",
        "project":project,
        "locator":locator,
        "safety_storage":storage
    }));
    assert_eq!(restored["kind"], "project_data", "{restored}");
    assert_eq!(restored["action"], "restore_current");
    assert_eq!(restored["outcome"], "applied_recovered", "{restored}");
    assert!(restored["backup"].is_null(), "{restored}");
    assert_eq!(restored["safety"]["kind"], "pre_restore", "{restored}");
    assert_ne!(restored["safety"]["id"], selected_id, "{restored}");
    assert_eq!(
        h.call(json!({"action":"project_status","project":project}))["runtime"],
        "Pending"
    );
    assert!(h.control(json!({"kind":"close","project":project}))["error"].is_null());
    h.close_clean();
}
