use super::*;

fn leaf(kind: &str) -> Value {
    json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"하위","configuration":{"kind":kind},"required":false,"presentation":{"intent":"unset"},"default":{"intent":"unset"},"archived":false})
}
#[test]
fn m38_whole_template_member_history_order_owner_and_duplicate() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut b = content(&h, &p, &s)["body"].clone();
    b["name"] = "그룹 Template".into();
    let mut group = leaf("group");
    group["configuration"] = json!({"kind":"group","members":[leaf("rich_text"),leaf("number"),leaf("image"),leaf("file")]});
    b["fields"] = json!([group]);
    let saved = submit(&h, &p, &s, "2", &b, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    let tid = saved["artifact"].as_str().unwrap();
    let path = h.root.join(format!("templates/{tid}.json"));
    let first: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        first["schemaVersion"],
        artifact::TEMPLATE_SCHEMA_VERSION.get()
    );
    assert_eq!(saved["identities"].as_object().unwrap().len(), 5);
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let view = h.read_template(&p, tid);
    let s = begin(&h, &p, view["view"].clone());
    let mut b = content(&h, &p, &s)["body"].clone();
    let group_id = b["fields"][0]["id"].as_str().unwrap().to_owned();
    let original_id = b["fields"][0]["configuration"]["members"][0]["id"].clone();
    b["fields"][0]["configuration"]["members"][0]["label"] = "새 이름".into();
    b["fields"][0]["configuration"]["members"][1]["archived"] = true.into();
    b["fields"][0]["configuration"]["members"]
        .as_array_mut()
        .unwrap()
        .swap(0, 3);
    let next = submit(&h, &p, &s, "3", &b, "save");
    assert_eq!(next["phase"], "saved", "{next}");
    let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(current["revision"], 2);
    assert_eq!(
        current["fields"][&group_id]["configuration"]["members"][original_id.as_str().unwrap()]
            ["introducedRevision"],
        1
    );
    let canonical = crate::data::artifact::decode_template(&fs::read(&path).unwrap()).unwrap();
    let dup =
        crate::data::artifact::duplicate_template(&canonical, "2026-09-17T00:00:00.000Z".into())
            .unwrap();
    let duplicate: Value =
        serde_json::from_slice(&crate::data::artifact::encode_template(&dup).unwrap()).unwrap();
    let new_group = duplicate["fieldOrder"][0].as_str().unwrap();
    assert_ne!(new_group, group_id);
    let new_members = duplicate["fields"][new_group]["configuration"]["members"]
        .as_object()
        .unwrap();
    assert!(new_members.keys().all(
        |id| current["fields"][&group_id]["configuration"]["members"]
            .get(id)
            .is_none()
    ));
    assert!(release(&h, &p, &next, false)["error"].is_null());
    h.close_clean();
}

#[test]
fn m38_whole_rejects_nested_and_foreign_member_without_writing() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut b = content(&h, &p, &s)["body"].clone();
    b["name"] = "반례".into();
    let mut g = leaf("group");
    let mut nested = leaf("group");
    nested["configuration"] = json!({"kind":"group","members":[]});
    g["configuration"] = json!({"kind":"group","members":[nested]});
    b["fields"] = json!([g]);
    let bad = submit(&h, &p, &s, "2", &b, "save");
    assert_ne!(bad["phase"], "saved", "{bad}");
    assert_eq!(
        fs::read_dir(h.root.join("templates"))
            .map(|x| x.count())
            .unwrap_or(0),
        0
    );
    let child = leaf("number");
    b["fields"][0]["configuration"]["members"] = json!([child.clone(), child]);
    let bad = submit(&h, &p, &s, "3", &b, "save");
    assert_ne!(bad["phase"], "saved");
    assert!(release(&h, &p, &bad, true)["error"].is_null());
    h.close_clean();
}
