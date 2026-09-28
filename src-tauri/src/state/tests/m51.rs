use super::*;
use std::io;

fn retire_project(h: &Harness, project: &str) {
    assert!(h.control(json!({"kind":"close","project":project}))["error"].is_null());
    let end = Instant::now() + LIMIT;
    loop {
        h.state.poll();
        let status = h.call(json!({"action":"project_status","project":project}));
        if status["shutdown"]["reportPending"] == true {
            h.call(json!({"action":"acknowledge_shutdown","project":project}));
        }
        if status["shutdown"]["joined"] == true && status["shutdown"]["reportPending"] == false {
            break;
        }
        assert!(Instant::now() < end, "project retirement deadline");
        thread::yield_now();
    }
    let operation = h.reserve("control");
    h.call(json!({
        "action":"submit",
        "operation":operation,
        "input":{"kind":"retire_project","project":project}
    }));
    let retired = h.result(&operation);
    assert_eq!(retired["kind"], "project_retired");
    h.ack(&operation);
}

#[test]
fn m51_project_name_is_one_safe_directory_component() {
    let parent = if cfg!(windows) {
        "C:\\projects"
    } else {
        "/projects"
    };
    assert_eq!(
        new_project_root(parent, "나의 세계").unwrap(),
        PathBuf::from(parent).join("나의 세계")
    );
    for invalid in [
        "", "..", " 앞", "뒤 ", "CON", "com1.txt", "a/b", "a\\b", "a:",
    ] {
        assert_eq!(new_project_root(parent, invalid), Err(Code::InvalidInput));
    }
}

#[test]
fn m51_empty_folder_creation_and_default_setting_use_real_state_boundaries() {
    let h = Harness::new();
    let created_root = h.base.join("새 세계");
    let created = h.work(json!({
        "kind":"create_project",
        "parent":h.base,
        "name":"새 세계"
    }));
    assert_eq!(created["kind"], "open");
    assert_eq!(created["created"], true);
    assert_eq!(created["error"], Value::Null);
    assert_eq!(fs::read_dir(&created_root).unwrap().count(), 0);

    let saved = h.work(json!({
        "kind":"project_settings_write",
        "default_root":created_root
    }));
    assert_eq!(saved["kind"], "project_settings_write");
    assert_eq!(saved["outcome"], "applied");
    assert_eq!(saved["observed"], true);
    let read = h.work(json!({"kind":"project_settings_read"}));
    assert_eq!(read["kind"], "project_settings");
    assert_eq!(read["defaultRoot"], saved["defaultRoot"]);
    assert_eq!(fs::read_dir(&created_root).unwrap().count(), 0);

    let project = created["project"].as_str().unwrap();
    let searched = h.work(json!({
        "kind":"document_workspace",
        "project":project,
        "request":{
            "action":"search",
            "query":"",
            "template":null,
            "offset":0,
            "limit":100,
            "refresh":true
        }
    }));
    assert_eq!(searched["value"]["kind"], "search");
    let project_id = h
        .state
        .lock()
        .projects
        .keys()
        .copied()
        .find(|id| String::from(*id) == project)
        .unwrap();
    assert!(h
        .state
        .workspace
        .lock()
        .unwrap()
        .has_project_cache(project_id));
    retire_project(&h, project);
    assert!(!h
        .state
        .workspace
        .lock()
        .unwrap()
        .has_project_cache(project_id));
    h.close_clean();
}

#[test]
fn m51_non_empty_creation_and_invalid_default_write_preserve_existing_data() {
    let h = Harness::new();
    let marker = h.root.join("keep.txt");
    fs::write(&marker, b"keep").unwrap();

    let rejected = h.work(json!({
        "kind":"create_project",
        "parent":h.base,
        "name":"project"
    }));
    assert_eq!(rejected["kind"], "open");
    assert_eq!(rejected["created"], true);
    assert_eq!(rejected["error"]["code"], "project_not_empty");
    assert_eq!(fs::read(&marker).unwrap(), b"keep");

    let saved = h.work(json!({
        "kind":"project_settings_write",
        "default_root":h.root
    }));
    let invalid = h.work(json!({
        "kind":"project_settings_write",
        "default_root":h.base.join("missing")
    }));
    assert_eq!(invalid["kind"], "project_settings_write");
    assert_eq!(invalid["outcome"], "not_applied");
    assert_eq!(invalid["observed"], true);
    assert_eq!(invalid["error"]["code"], "settings_write_failed");
    assert_eq!(invalid["defaultRoot"], saved["defaultRoot"]);
    let read = h.work(json!({"kind":"project_settings_read"}));
    assert_eq!(read["defaultRoot"], saved["defaultRoot"]);
    assert_eq!(fs::read(&marker).unwrap(), b"keep");

    retire_project(&h, rejected["project"].as_str().unwrap());
    h.close_clean();
}

#[test]
fn m51_initialization_cleanup_diagnostic_reaches_boundary_without_paths() {
    let error = crate::data::project_runtime::RuntimeError::m51_initialization_with_cleanup(
        io::Error::new(io::ErrorKind::Other, "C:/private/primary"),
        Err(io::Error::new(
            io::ErrorKind::DirectoryNotEmpty,
            "C:/private/cleanup",
        )),
    );
    let value = serde_json::to_value(initialization_error(&error)).unwrap();
    assert_eq!(value["code"], "initialization_failed");
    assert_eq!(
        value["diagnostic"]["cleanupOutcome"],
        "preserved_external_entries"
    );
    assert_eq!(value["diagnostic"]["cleanupIoKind"], "DirectoryNotEmpty");
    let rendered = value.to_string();
    assert!(!rendered.contains("private"));
}
