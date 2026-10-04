//! 삭제 DTO는 실제 guarded 실행의 원 거부에서만 나와야 한다.
use super::*;

pub(super) fn seed(h: &Harness) {
    let (mut template, document) = crate::data::application::composite::tests::fixture_raw();
    let field = |n: u32| format!("99999999-9999-4999-8999-{n:012x}");
    template["fields"][field(1)]["defaultValue"] = json!({"kind":"singleChoice","optionId":field(12),"future":{"owner":"m274_current","number":"M274_CURRENT_TOKEN"}});
    template["fields"][field(1)]["initialDefaultValue"] = json!({"kind":"singleChoice","optionId":field(11),"future":{"owner":"m274_initial","number":"M274_INITIAL_TOKEN"}});
    template["fields"][field(4)]["lifecycle"] = json!("archived");
    template["fieldOrder"] = json!([field(1), field(2)]);
    let bytes = |value: &Value| {
        let mut raw = serde_json::to_string(value).unwrap();
        for (i, owner) in [
            "template",
            "field",
            "configuration",
            "option",
            "default",
            "document",
            "value",
            "snapshot",
            "snapshot_option",
            "rich_outer",
            "rich_inner",
        ]
        .iter()
        .enumerate()
        {
            raw = raw.replace(
                &format!("\"__{owner}__\""),
                &format!(r#"{{"owner":"{owner}","number":{}E+101}}"#, i + 70),
            );
        }
        raw.replace("\"M274_CURRENT_TOKEN\"", "81E+102")
            .replace("\"M274_INITIAL_TOKEN\"", "82e+103")
            .into_bytes()
    };
    let template = bytes(&template);
    let document = bytes(&document);
    let template = current_policy_fixture(&template, &format!("templates/{}.json", field(100)));
    let document = current_policy_fixture(&document, &format!("documents/{}.json", field(200)));
    artifact::decode_template(&template).unwrap();
    artifact::decode_document(&document).unwrap();
    fs::create_dir(h.root.join("templates")).unwrap();
    fs::create_dir(h.root.join("documents")).unwrap();
    fs::write(
        h.root.join(format!("templates/{}.json", field(100))),
        &template,
    )
    .unwrap();
    fs::write(
        h.root.join(format!("documents/{}.json", field(200))),
        &document,
    )
    .unwrap();
    fs::write(h.base.join("m274-original-template.json"), &template).unwrap();
    fs::write(h.base.join("m274-original-document.json"), &document).unwrap();
}

pub(super) fn verify_copies(h: &Harness, summary: &Value) {
    use crate::data::json::parse_strict_lossless_json_object as lossless;
    let read = |id: &str| fs::read(h.root.join(format!("templates/{id}.json"))).unwrap();
    let original = summary["original"].as_str().unwrap();
    let first = summary["first"].as_str().unwrap();
    let second = summary["id"].as_str().unwrap();
    assert!(
        read(original) == fs::read(h.base.join("m274-original-template.json")).unwrap(),
        "original bytes preserved"
    );
    assert!(
        fs::read(
            h.root
                .join("documents/99999999-9999-4999-8999-0000000000c8.json")
        )
        .unwrap()
            == fs::read(h.base.join("m274-original-document.json")).unwrap(),
        "Document bytes preserved"
    );
    for (source, copy, map) in [(original, first, "mapping1"), (first, second, "mapping2")] {
        let before = lossless(&read(source)).unwrap();
        let after = lossless(&read(copy)).unwrap();
        assert!(
            before.object_path(&["future"]) == after.object_path(&["future"]),
            "Template metadata owner preserved"
        );
        let mappings = summary[map].as_array().unwrap();
        for pair in mappings {
            let old = pair[0].as_str().unwrap();
            let new = pair[1].as_str().unwrap();
            if before.object_path(&["fields", old]).is_none() {
                continue;
            }
            for suffix in [
                vec!["future"],
                vec!["configuration", "future"],
                vec!["defaultValue", "future"],
                vec!["initialDefaultValue", "future"],
            ] {
                let a = [vec!["fields", old], suffix.clone()].concat();
                let b = [vec!["fields", new], suffix].concat();
                assert!(
                    before.object_path(&a) == after.object_path(&b),
                    "Field/default/configuration metadata owner and lexeme preserved"
                );
            }
            for option in mappings {
                let a = [
                    "fields",
                    old,
                    "configuration",
                    "options",
                    option[0].as_str().unwrap(),
                    "future",
                ];
                let b = [
                    "fields",
                    new,
                    "configuration",
                    "options",
                    option[1].as_str().unwrap(),
                    "future",
                ];
                assert!(
                    before.object_path(&a) == after.object_path(&b),
                    "Option metadata owner and lexeme preserved"
                );
            }
        }
    }
}

fn release_retained(h: &Harness) {
    let entries = h.call(json!({"action":"retained_list"}))["entries"]
        .as_array()
        .unwrap()
        .clone();
    for retained in entries {
        let id = h.reserve("control");
        h.call(json!({"action":"submit","operation":id,"input":{"kind":"abandon_retained","retained":retained}}));
        assert_eq!(h.result(&id)["action"], "abandoned");
        h.ack(&id);
    }
}

#[test]
fn m274_tombstone_actual_zero_one_and_truncated_references() {
    for count in [0, 1, 1025] {
        let h = Harness::new();
        let p = h.open();
        let (id, _) = h.template(&p);
        let read = h.read_template(&p, &id);
        if count > 0 {
            let doc = h.work(json!({"kind":"create_document","project":p,"view":read["view"],"name":"synthetic"}));
            let doc_path = h.root.join(format!(
                "documents/{}.json",
                doc["artifact"].as_str().unwrap()
            ));
            let bytes = fs::read(doc_path).unwrap();
            for _ in 1..count {
                let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                let id = uuid::Uuid::new_v4().to_string();
                value["documentId"] = json!(id);
                fs::write(
                    h.root.join(format!("documents/{id}.json")),
                    serde_json::to_vec(&value).unwrap(),
                )
                .unwrap();
            }
        }
        let path = h.root.join(format!("templates/{id}.json"));
        let before = fs::read(&path).unwrap();
        let session = h.session(&p, vec![read["view"].clone()], "template");
        let result = h.work(json!({"kind":"tombstone_template","project":p,"session":session,"view":read["view"],"revision":"1"}));
        if count == 0 {
            assert_eq!(result["disk"], "committed");
            assert!(result.get("deletion").is_none());
            assert_eq!(h.read_template(&p, &id)["content"]["lifecycle"], "Deleted");
        } else {
            assert_eq!(result["disk"], "not_attempted");
            assert_eq!(result["error"]["code"], "save_rejected");
            assert_eq!(
                result["deletion"],
                json!({"reason":"template_has_documents","count":count.min(1024),"truncated":count>1024})
            );
            assert!(
                fs::read(&path).unwrap() == before,
                "reference rejection must preserve source bytes"
            );
            release_retained(&h);
        }
        h.close_clean();
    }
}

#[test]
fn m274_tombstone_scan_failure_and_stale_are_not_reference_counts() {
    for stale in [false, true] {
        let h = Harness::new();
        let p = h.open();
        let (id, _) = h.template(&p);
        let read = h.read_template(&p, &id);
        let session = h.session(&p, vec![read["view"].clone()], "template");
        if stale {
            let result=h.work(json!({"kind":"update_template","project":p,"session":session,"view":read["view"],"revision":"1","edit":{"kind":"name","name":"new basis"}}));
            assert_eq!(result["disk"], "committed");
            assert!(result.get("deletion").is_none());
        } else {
            fs::create_dir_all(h.root.join("documents")).unwrap();
            fs::write(
                h.root
                    .join(format!("documents/{}.json", uuid::Uuid::new_v4())),
                b"{M274_PRIVATE_SCAN_CANARY",
            )
            .unwrap();
        }
        let path = h.root.join(format!("templates/{id}.json"));
        let before = fs::read(&path).unwrap();
        let result=h.work(json!({"kind":"tombstone_template","project":p,"session":session,"view":read["view"],"revision":"1"}));
        assert_eq!(result["disk"], "not_attempted");
        assert_eq!(
            result["deletion"],
            json!({"reason":if stale {"source_changed"} else {"reference_check_failed"}})
        );
        assert!(!result.to_string().contains("PRIVATE_SCAN_CANARY"));
        assert!(
            fs::read(path).unwrap() == before,
            "failed check must preserve source bytes"
        );
        release_retained(&h);
        h.close_clean();
    }
}

#[test]
fn m274_dialog_minimum_acl_and_actual_dispatcher_registration() {
    let h = Harness::new();
    let foreign = tauri::WebviewWindowBuilder::new(&h.app, "dialog-foreign", Default::default())
        .build()
        .unwrap();
    let body = serde_json::to_vec(&json!({"options":{"directory":[],"multiple":false}})).unwrap();
    // 잘못된 닫힌 옵션 타입으로 native 창 생성 전에 멈춘다. main은 plugin decoder에 도달한다.
    let main = tauri::test::get_ipc_response(
        &h.main,
        Harness::request("plugin:dialog|open", body.clone(), "http://tauri.localhost"),
    )
    .unwrap_err();
    assert!(main.to_string().contains("invalid args"));
    for command in [
        "plugin:dialog|save",
        "plugin:dialog|message",
        "plugin:fs|read_file",
        "plugin:path|join",
    ] {
        assert!(tauri::test::get_ipc_response(
            &h.main,
            Harness::request(command, body.clone(), "http://tauri.localhost")
        )
        .is_err());
    }
    assert!(tauri::test::get_ipc_response(
        &foreign,
        Harness::request("plugin:dialog|open", body.clone(), "http://tauri.localhost")
    )
    .is_err());
    let desktop_request = |window: &tauri::WebviewWindow<tauri::test::MockRuntime>| {
        let mut request = Harness::request(
            "plugin:path|resolve_directory",
            Vec::new(),
            "http://tauri.localhost",
        );
        request.body = tauri::ipc::InvokeBody::Json(json!({"directory":18}));
        tauri::test::get_ipc_response(window, request)
    };
    // Windows의 실제 Known Folder API로 독립적으로 구한 Desktop과 IPC 결과를
    // 비교한다. 리다이렉트/현지화된 Desktop도 현재 계정의 정확한 경로여야 한다.
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };
    let known_desktop = unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) };
    match known_desktop {
        Ok(raw) => {
            let expected = std::path::PathBuf::from(unsafe { raw.to_string().unwrap() });
            unsafe { CoTaskMemFree(Some(raw.0.cast())) };
            let actual = desktop_request(&h.main)
                .expect("main window must resolve Desktop")
                .deserialize::<std::path::PathBuf>()
                .unwrap();
            assert_eq!(actual, expected);
        }
        Err(_) => {
            let error = desktop_request(&h.main).unwrap_err();
            assert!(error.to_string().contains("unknown path"));
        }
    }
    let desktop_foreign = desktop_request(&foreign).unwrap_err();
    let denial = desktop_foreign.to_string();
    assert!(denial.contains("resolve_directory") && denial.contains("not allowed"));
    assert!(tauri::test::get_ipc_response(
        &h.main,
        Harness::request("plugin:dialog|open", body, "https://untrusted.example")
    )
    .is_err());
    h.close_clean();
}
