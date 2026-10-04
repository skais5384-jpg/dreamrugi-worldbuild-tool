use super::*;

fn set_old(path: &Path, version: u64) -> Value {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["schemaVersion"] = version.into();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    value
}

#[test]
fn policy_direct_document_and_template_reads_upgrade_before_issuing_editable_views() {
    for entry in ["document", "template", "edit", "versions"] {
        let h = Harness::new();
        let p = h.open();
        let (t, session) = h.template(&p);
        h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"accept"}),
        );
        h.control(json!({"kind":"session_control","project":p,"session":session,"control":"end"}));
        let d = create(&h, &p, &t, "unchanged canonical content");
        let tp = h.root.join(format!("templates/{t}.json"));
        let dp = h.root.join(format!("documents/{d}.json"));
        let mut old_t = set_old(&tp, 7);
        let mut old_d = set_old(&dp, 6);
        let response = match entry {
            "document" => h.work(json!({"kind":"read_document","project":p,"document":d})),
            "template" => h.read_template(&p, &t),
            "edit" => request(&h, &p, json!({"action":"edit_begin","document":d})),
            _ => request(
                &h,
                &p,
                json!({"action":"versions_list","kind":"document","artifact":d}),
            ),
        };
        assert!(response["error"].is_null(), "{entry}: {response}");
        old_t["schemaVersion"] = 8.into();
        old_d["schemaVersion"] = 7.into();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&tp).unwrap()).unwrap(),
            old_t
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&dp).unwrap()).unwrap(),
            old_d
        );
        if entry == "edit" {
            assert_eq!(response["value"]["kind"], "editing", "{response}");
            edit_release(&h, &p, &response["value"]);
        }
        h.close_clean();
    }
}

#[test]
fn policy_history_repairs_missing_or_corrupt_document_without_reopening_old_format() {
    for damage in ["missing", "corrupt"] {
        let h = Harness::new();
        let p = h.open();
        let (t, session) = h.template(&p);
        h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"accept"}),
        );
        h.control(json!({"kind":"session_control","project":p,"session":session,"control":"end"}));
        let d = create(&h, &p, &t, "historical body survives");
        let dp = h.root.join(format!("documents/{d}.json"));
        if damage == "missing" {
            fs::remove_file(&dp).unwrap();
        } else {
            fs::write(&dp, b"owned malformed source").unwrap();
        }
        let listed = request(
            &h,
            &p,
            json!({"action":"versions_list","kind":"document","artifact":d}),
        );
        assert!(
            !listed["value"]["versions"].as_array().unwrap().is_empty(),
            "{listed}"
        );
        let preview = request(
            &h,
            &p,
            json!({"action":"version_preview","kind":"document","artifact":d,"version":"1"}),
        );
        assert!(preview["error"].is_null(), "{preview}");
        let restored = request(
            &h,
            &p,
            json!({"action":"version_restore","kind":"document","artifact":d,"version":"1","source":preview["value"]["source"]}),
        );
        assert!(restored["error"].is_null(), "{restored}");
        let actual: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
        assert_eq!(actual["schemaVersion"], 7);
        assert_eq!(actual["documentId"], d);
        h.close_clean();
    }
}
