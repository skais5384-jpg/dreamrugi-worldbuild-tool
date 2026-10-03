use super::*;

fn field(kind: &str) -> Value {
    json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":kind,"configuration":{"kind":kind},"required":false,"presentation":{"intent":"keep"},"default":{"intent":"unset"},"archived":false})
}

#[test]
fn archive_recovery_all_scalar_kinds_restore_same_ids_and_save_again_in_same_owner() {
    let h = Harness::new();
    let p = h.open();
    let start = begin(&h, &p, Value::Null);
    let mut body = content(&h, &p, &start)["body"].clone();
    body["name"] = "ordinary restore".into();
    body["fields"]=Value::Array(["single_line_text","rich_text","number","date","time","image","file","url","duration","document_link","relation"].into_iter().map(|kind| {
        let mut f=field(kind); if kind=="number" {f["default"]=json!({"intent":"set","value":{"kind":"number","value":"1"}});} if kind=="relation" {f["configuration"]=json!({"kind":"relation","multiple":true,"allowedTemplates":[],"reciprocalNotice":false});} f
    }).collect());
    let saved = submit(&h, &p, &start, "2", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    let path = h.root.join(format!(
        "templates/{}.json",
        saved["artifact"].as_str().unwrap()
    ));
    let before: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut current = content(&h, &p, &saved)["body"].clone();
    for f in current["fields"].as_array_mut().unwrap() {
        f["archived"] = true.into();
    }
    let archived = submit(&h, &p, &saved, "3", &current, "save");
    assert_eq!(archived["phase"], "saved", "{archived}");
    assert!(archived["error"].is_null(), "{archived} body={current}");
    let mut current = content(&h, &p, &archived)["body"].clone();
    for f in current["fields"].as_array_mut().unwrap() {
        f["archived"] = false.into();
        f["restore"] = true.into();
    }
    let restored = submit(&h, &p, &archived, "4", &current, "save");
    assert_eq!(restored["phase"], "saved", "{restored}");
    assert!(restored["error"].is_null(), "{restored}");
    let mut again = content(&h, &p, &restored)["body"].clone();
    assert!(again["fields"]
        .as_array()
        .unwrap()
        .iter()
        .all(|field| field.get("restore").is_none()));
    again["fields"][0]["label"] = "continued edit".into();
    let twice = submit(&h, &p, &restored, "5", &again, "save");
    assert_eq!(twice["phase"], "saved", "{twice}");
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(before["fieldOrder"], after["fieldOrder"]);
    for (id, old) in before["fields"].as_object().unwrap() {
        let now = &after["fields"][id];
        assert_eq!(now["lifecycle"], "active");
        for key in [
            "introducedRevision",
            "initialDefaultValue",
            "configuration",
            "required",
        ] {
            assert_eq!(old[key], now[key]);
        }
    }
    assert!(release(&h, &p, &twice, false)["error"].is_null());
    h.close_clean();
}

#[test]
fn archive_recovery_single_and_multi_option_restore_is_explicit_and_keeps_other_archives() {
    for kind in ["single_choice", "multi_choice"] {
        let h = Harness::new();
        let p = h.open();
        let start = begin(&h, &p, Value::Null);
        let mut body = content(&h, &p, &start)["body"].clone();
        body["name"] = "choice recovery".into();
        let mut choice = field(kind);
        let options=["first","second"].map(|label|json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":label,"archived":false}));
        choice["configuration"]["options"] = json!(options);
        body["fields"] = json!([choice]);
        let saved = submit(&h, &p, &start, "2", &body, "save");
        assert_eq!(saved["phase"], "saved", "{saved}");
        let mut b = content(&h, &p, &saved)["body"].clone();
        for o in b["fields"][0]["configuration"]["options"]
            .as_array_mut()
            .unwrap()
        {
            o["archived"] = true.into();
        }
        let archived = submit(&h, &p, &saved, "3", &b, "save");
        assert_eq!(archived["phase"], "saved", "{archived}");
        let mut b = content(&h, &p, &archived)["body"].clone();
        b["fields"][0]["configuration"]["options"][0]["archived"] = false.into();
        let forged = submit(&h, &p, &archived, "4", &b, "save");
        assert!(forged["error"].is_object());
        b["fields"][0]["configuration"]["options"][0]["restore"] = true.into();
        let restored = submit(&h, &p, &forged, "5", &b, "save");
        assert_eq!(restored["phase"], "saved", "{restored}");
        assert!(restored["error"].is_null(), "{restored}");
        let mut b = content(&h, &p, &restored)["body"].clone();
        b["fields"][0]["label"] = "same owner next save".into();
        let final_save = submit(&h, &p, &restored, "6", &b, "save");
        assert_eq!(final_save["phase"], "saved", "{final_save}");
        let read = h.read_template(&p, final_save["artifact"].as_str().unwrap());
        let opts = &read["template"]["fields"][0]["options"];
        // The canonical artifact is the authority even if the read DTO shape evolves.
        let raw: Value = serde_json::from_slice(
            &fs::read(h.root.join(format!(
                "templates/{}.json",
                final_save["artifact"].as_str().unwrap()
            )))
            .unwrap(),
        )
        .unwrap();
        let fid = raw["fieldOrder"][0].as_str().unwrap();
        let configured = &raw["fields"][fid]["configuration"];
        assert_eq!(configured["optionOrder"].as_array().unwrap().len(), 1);
        assert_eq!(
            configured["options"]
                .as_object()
                .unwrap()
                .values()
                .filter(|option| option["lifecycle"] == "archived")
                .count(),
            1
        );
        let _ = opts;
        assert!(release(&h, &p, &final_save, false)["error"].is_null());
        h.close_clean();
    }
}

#[test]
fn archive_recovery_parent_only_keeps_archived_child_and_explicit_child_restores_title_same_ids() {
    let h = Harness::new();
    let p = h.open();
    let start = begin(&h, &p, Value::Null);
    let mut b = content(&h, &p, &start)["body"].clone();
    b["name"] = "group restore".into();
    let title = field("rich_text");
    let title_id = title["id"].clone();
    let mut group = field("group");
    group["configuration"] = json!({"kind":"group","members":[title,field("file"),field("image")],"cardTitleField":{"intent":"set","value":title_id}});
    b["fields"] = json!([group]);
    let saved = submit(&h, &p, &start, "2", &b, "save");
    assert!(saved["error"].is_null(), "{saved}");
    let id = saved["artifact"].as_str().unwrap();
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let view = h.read_template(&p, id);
    let editing = begin(&h, &p, view["view"].clone());
    let mut b = content(&h, &p, &editing)["body"].clone();
    let root = b["fields"][0]["id"].as_str().unwrap().to_owned();
    let child = b["fields"][0]["configuration"]["members"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = h.root.join(format!("templates/{id}.json"));
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    b["fields"][0]["archived"] = true.into();
    b["fields"][0]["configuration"]["members"][0]["archived"] = true.into();
    let archived = submit(&h, &p, &editing, "3", &b, "save");
    assert!(archived["error"].is_null(), "{archived}");
    let mut b = content(&h, &p, &archived)["body"].clone();
    b["fields"][0]["archived"] = false.into();
    b["fields"][0]["restore"] = true.into();
    let parent = submit(&h, &p, &archived, "4", &b, "save");
    assert!(parent["error"].is_null(), "{parent}");
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        raw["fields"][&root]["configuration"]["members"][&child]["lifecycle"],
        "archived"
    );
    let mut b = content(&h, &p, &parent)["body"].clone();
    b["fields"][0]["configuration"]["members"][0]["archived"] = false.into();
    b["fields"][0]["configuration"]["members"][0]["restore"] = true.into();
    let restored = submit(&h, &p, &parent, "5", &b, "save");
    assert!(restored["error"].is_null(), "{restored}");
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        raw["fields"][&root]["presentation"],
        original["fields"][&root]["presentation"]
    );
    assert_eq!(
        raw["fields"][&root]["configuration"]["members"][&child]["lifecycle"],
        "active"
    );
    assert!(release(&h, &p, &restored, false)["error"].is_null());
    h.close_clean();
}

#[test]
fn archive_recovery_changed_template_merges_new_fields_options_order_members_title_and_current_label(
) {
    let h = Harness::new();
    let p = h.open();
    let start = begin(&h, &p, Value::Null);
    let mut b = content(&h, &p, &start)["body"].clone();
    b["name"] = "structure merge".into();
    let title = field("rich_text");
    let mut group = field("group");
    group["configuration"] =
        json!({"kind":"group","members":[title],"cardTitleField":{"intent":"unset"}});
    let mut choice = field("single_choice");
    choice["configuration"]["options"] = json!([{"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"existing option","archived":false}]);
    b["fields"] = json!([group, choice, field("single_line_text")]);
    let saved = submit(&h, &p, &start, "2", &b, "save");
    assert!(saved["error"].is_null(), "{saved}");
    let t = saved["artifact"].as_str().unwrap().to_owned();
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let edit = begin(&h, &p, h.read_template(&p, &t)["view"].clone());
    let mut b = content(&h, &p, &edit)["body"].clone();
    let group_id = b["fields"][0]["id"].as_str().unwrap().to_owned();
    let title_id = b["fields"][0]["configuration"]["members"][0]["id"].clone();
    let untouched_id = b["fields"][2]["id"].as_str().unwrap().to_owned();
    b["fields"][0]["configuration"]["cardTitleField"] = json!({"intent":"set","value":title_id});
    b["fields"][0]["configuration"]["members"]
        .as_array_mut()
        .unwrap()
        .push(field("file"));
    b["fields"][1]["configuration"]["options"].as_array_mut().unwrap().push(json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"preserved new option","archived":false}));
    b["fields"].as_array_mut().unwrap().swap(0, 1);
    b["fields"].as_array_mut().unwrap().push(field("number"));
    let dep = submit(&h, &p, &edit, "2", &b, "deposit");
    assert!(release(&h, &p, &dep, false)["error"].is_null());
    let receipt = &dep["receipt"];
    let key = serde_json::from_value(receipt["key"].clone()).unwrap();
    let original = h
        .state
        .recovery
        .connect()
        .unwrap()
        .lock()
        .unwrap()
        .read(&key, receipt["depositId"].as_str().unwrap())
        .unwrap()
        .bytes()
        .to_vec();
    let path = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    raw["fields"][&untouched_id]["label"] = "independent current label".into();
    raw["revision"] = 2.into();
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let s=h.work(json!({"kind":"recovery_read","key":receipt["key"],"deposit_id":receipt["depositId"],"digest":receipt["digest"]}));
    let compared=h.work(json!({"kind":"recovery_restore","project":p,"snapshot":s["selection"]["snapshot"],"reapply":null}));
    assert_eq!(compared["kind"], "recovery_selection", "{compared}");
    let snapshot = compared["selection"]["snapshot"].clone();
    let mut text = String::new();
    let mut offset = json!("0");
    loop {
        let c = h.work(json!({"kind":"recovery_content","snapshot":snapshot,"offset":offset}));
        text.push_str(c["content"]["text"].as_str().unwrap());
        offset = c["content"]["next"].clone();
        if offset.is_null() {
            break;
        }
    }
    let comparison: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(
        comparison["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] != "blocked"),
        "{comparison}"
    );
    let choices: Vec<Value> = comparison["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"kind":"change","change":c["id"]}))
        .collect();
    let restored = h
        .work(json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":choices}));
    assert_eq!(restored["kind"], "template_draft", "{restored}");
    let status = &restored["status"];
    let b = content(&h, &p, status)["body"].clone();
    let saved = submit(&h, &p, status, "4", &b, "save");
    assert!(saved["error"].is_null(), "{saved}");
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(raw["fieldOrder"].as_array().unwrap().len(), 4);
    assert_eq!(
        raw["fields"][&untouched_id]["label"],
        "independent current label"
    );
    assert_eq!(
        raw["fields"][&group_id]["configuration"]["memberOrder"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        raw["fields"][&group_id]["presentation"]["cardTitleField"],
        title_id
    );
    assert_eq!(
        h.state
            .recovery
            .connect()
            .unwrap()
            .lock()
            .unwrap()
            .read(&key, receipt["depositId"].as_str().unwrap())
            .unwrap()
            .bytes(),
        original
    );
    h.close_clean();
}
