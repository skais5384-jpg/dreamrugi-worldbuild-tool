use super::*;

const LEFT: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
const RIGHT: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2";

#[test]
#[ignore = "explicit synthetic record staging directory required; never alters an app store"]
fn archive_recovery_freeze_corrected_manual_records() {
    use crate::data::edit_recovery::model::{Deposit, Envelope};
    let root = std::path::PathBuf::from(
        std::env::var("WORLDBUILD_RECOVERY_CORRECTION_DIR").expect("explicit staging directory"),
    );
    for name in ["M11", "M07"] {
        let input = root.join(name).join("input.json");
        let output = root.join(name).join("frozen.json");
        assert!(!output.exists(), "preserve prior frozen fixture");
        let raw: Value = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
        let envelope: Envelope = serde_json::from_value(raw["envelope"].clone()).unwrap();
        let deposit = Deposit::freeze(envelope).unwrap();
        let mut frozen: Value = serde_json::from_slice(deposit.bytes()).unwrap();
        frozen["recoverySchemaVersion"] = raw["recoverySchemaVersion"].clone();
        let bytes = serde_json::to_vec(&frozen).unwrap();
        let admitted = Deposit::decode(&bytes).unwrap();
        assert_eq!(admitted.payload_digest(), deposit.payload_digest());
        fs::write(output, bytes).unwrap();
    }
}
fn seed(h: &Harness, p: &str) -> String {
    seed_with_session(h, p).0
}
fn seed_with_session(h: &Harness, p: &str) -> (String, String) {
    let (id, creation_session) = h.template(p);
    let path = h.root.join(format!("templates/{id}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["fieldOrder"] = json!([LEFT, RIGHT]);
    wire["fields"] = json!({});
    for (id, label) in [(LEFT, "왼쪽"), (RIGHT, "오른쪽")] {
        wire["fields"][id] = json!({"label":label,"kind":"singleLineText","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"text","value":"base"},"initialDefaultValue":{"kind":"text","value":"base"},"configuration":{"kind":"singleLineText"},"presentation":{}});
    }
    fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
    (id, creation_session)
}

#[test]
fn document_edit_known_snapshot_renders_once_before_and_after_definition_restore() {
    let h = Harness::new();
    let p = h.open();
    let (t, creation_session) = seed_with_session(&h, &p);
    assert!(h.control(
        json!({"kind":"session_control","project":p,"session":creation_session,"control":"end"})
    )["error"]
        .is_null());
    let id = create(&h, &p, &t, "snapshot input projection");
    let tp = h.root.join(format!("templates/{t}.json"));
    let dp = h.root.join(format!("documents/{id}.json"));
    let mut template: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let mut document: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    document["fieldValues"][LEFT] = json!({"kind":"text","value":"saved archived value"});
    document["orphanedFieldDefinitions"][LEFT] =
        json!({"label":"왼쪽","kind":"singleLineText","options":{}});
    fs::write(&dp, serde_json::to_vec(&document).unwrap()).unwrap();
    for archived in [true, false] {
        template["fields"][LEFT]["lifecycle"] = if archived { "archived" } else { "active" }.into();
        template["fieldOrder"] = if archived {
            json!([RIGHT])
        } else {
            json!([RIGHT, LEFT])
        };
        fs::write(&tp, serde_json::to_vec(&template).unwrap()).unwrap();
        let before = fs::read(&dp).unwrap();
        let editing = edit_begin(&h, &p, &id);
        let rows = editing["read"]["fields"].as_array().unwrap();
        assert_eq!(
            rows.iter().filter(|row| row["id"] == LEFT).count(),
            1,
            "{editing}"
        );
        assert_eq!(
            rows.iter().find(|row| row["id"] == LEFT).unwrap()["value"]["value"],
            "saved archived value"
        );
        assert_eq!(
            editing["body"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["field"] == LEFT)
                .count(),
            0,
            "unchanged fields stay implicit Keep in the input body"
        );
        assert_eq!(
            editing["editable"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|field| **field == LEFT)
                .count(),
            usize::from(!archived)
        );
        assert_eq!(
            fs::read(&dp).unwrap(),
            before,
            "projection must not modify canonical input"
        );
        if archived {
            let deposited = request(&h, &p, json!({"action":"edit_deposit","owner":editing["owner"],"generation":editing["generation"],"body":editing["body"]}))["value"].clone();
            edit_release(&h, &p, &deposited);
        } else {
            let mut body = editing["body"].clone();
            body["fields"] = json!([set(LEFT, "edited restored value")]);
            let saved = edit_save(&h, &p, &editing, "2", body);
            assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
            assert_eq!(
                saved["read"]["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|row| row["id"] == LEFT)
                    .count(),
                1
            );
            edit_release(&h, &p, &saved);
            let stored: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
            assert_eq!(
                stored["fieldValues"][LEFT]["value"],
                "edited restored value"
            );
        }
    }
    h.close_clean();
}
fn set(field: &str, value: &str) -> Value {
    json!({"field":field,"value":{"intent":"set","value":{"kind":"single_line_text","value":value}}})
}
fn row(h: &Harness, owner: &Value) -> Value {
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    page["page"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["row"]["key"]["draftId"] == *owner)
        .unwrap_or_else(|| panic!("{page} owner={owner}"))["row"]
        .clone()
}
fn stored(h: &Harness, row: &Value) -> Vec<u8> {
    h.state
        .recovery
        .connect()
        .unwrap()
        .lock()
        .unwrap()
        .read(
            &serde_json::from_value(row["key"].clone()).unwrap(),
            row["depositId"].as_str().unwrap(),
        )
        .unwrap()
        .bytes()
        .to_vec()
}
fn inspect(h: &Harness, row: &Value) -> Value {
    let result=h.work(json!({"kind":"recovery_read","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}));
    assert_eq!(result["kind"], "recovery_selection", "{result} row={row}");
    result["selection"]["snapshot"].clone()
}
fn compare(h: &Harness, p: &str, snapshot: &Value) -> (Value, Value) {
    let r =
        h.work(json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":null}));
    assert_eq!(r["kind"], "recovery_selection", "{r}");
    let snapshot = r["selection"]["snapshot"].clone();
    let mut offset = json!("0");
    let mut raw = String::new();
    loop {
        let r = h.work(json!({"kind":"recovery_content","snapshot":snapshot,"offset":offset}));
        assert_eq!(r["kind"], "template_draft_content", "{r}");
        raw.push_str(
            r["content"]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{r}")),
        );
        offset = r["content"]["next"].clone();
        if offset.is_null() {
            break;
        }
    }
    (snapshot, serde_json::from_str(&raw).unwrap())
}
fn proof(content: &Value) -> Value {
    let mut proof = content["basis"].clone();
    proof["selected"] = Value::Array(
        content["comparison"]
            .as_array()
            .unwrap_or_else(|| panic!("{content}"))
            .iter()
            .filter(|change| change["status"] != "blocked")
            .map(|change| change["id"].clone())
            .collect(),
    );
    proof
}

#[test]
fn archive_recovery_existing_document_merges_independent_fields_and_rejects_stale_compare() {
    let h = Harness::new();
    let p = h.open();
    let template = seed(&h, &p);
    let id = create(&h, &p, &template, "base document");
    let editing = edit_begin(&h, &p, &id);
    let mut b = editing["body"].clone();
    b["fields"] = json!([set(LEFT, "preserved")]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let listing = h.work(json!({"kind":"recovery_page","cursor":null}));
    let listed = listing["page"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["row"]["key"]["draftId"] == d["owner"])
        .unwrap();
    assert_eq!(listed["name"], "base document");
    let r = row(&h, &d["owner"]);
    let bytes = stored(&h, &r);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["fieldValues"][RIGHT] = json!({"kind":"text","value":"independent current"});
    fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let (snapshot, c) = compare(&h, &p, &inspect(&h, &r));
    let selected_proof = proof(&c);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(stored(&h, &r), bytes);
    wire["name"] = "later current name".into();
    fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let latest = fs::read(&path).unwrap();
    let stale = request(
        &h,
        &p,
        json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":selected_proof}),
    );
    assert_eq!(stale["error"]["code"], "wrong_binding", "{stale}");
    assert_eq!(fs::read(&path).unwrap(), latest);
    let (_, c) = compare(&h, &p, &snapshot);
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    assert_eq!(restored["kind"], "editing", "{restored}");
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["fieldValues"][LEFT]["value"], "preserved");
    assert_eq!(after["fieldValues"][RIGHT]["value"], "independent current");
    assert_eq!(after["name"], "later current name");
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_new_document_after_template_change_keeps_current_creation_policy_and_record() {
    let h = Harness::new();
    let p = h.open();
    let template = seed(&h, &p);
    let d = begin(&h, &p, &template);
    let mut b = d["body"].clone();
    b["name"] = "recovered new document".into();
    b["fields"] = json!([set(LEFT, "")]);
    let d = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    release(&h, &p, &d, false);
    let r = row(&h, &d["owner"]);
    let bytes = stored(&h, &r);
    let path = h.root.join(format!("templates/{template}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["fields"][RIGHT]["defaultValue"]["value"] = "new default".into();
    wire["revision"] = 2.into();
    fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 0);
    let restored=request(&h,&p,json!({"action":"restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    assert_eq!(restored["kind"], "draft", "{restored}");
    let empty = save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(empty["problem"], "InvalidBoundValue", "{empty}");
    assert_eq!(empty["body"]["fields"][0]["value"]["value"]["value"], "");
    let mut correction = empty["body"].clone();
    correction["fields"][0] = set(LEFT, "corrected recovered input");
    let saved = save(&h, &p, &empty, "5", correction);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    release(&h, &p, &saved, false);
    let id = saved["outcome"]["artifact"].as_str().unwrap();
    let after: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
            .unwrap();
    assert_eq!(
        after["fieldValues"][LEFT]["value"],
        "corrected recovered input"
    );
    assert_eq!(after["fieldValues"][RIGHT]["kind"], "unset");
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_group_from_unset_and_partial_cells_preserves_other_current_card() {
    let h = Harness::new();
    let p = h.open();
    let template = super::groups::template(&h, &p);
    let id = create(&h, &p, &template, "group merge");
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    let edit = edit_begin(&h, &p, &id);
    let mut input = edit["body"].clone();
    input["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"3"}}}]}]}}}]);
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":edit["owner"],"generation":"2","body":input}),
    )["value"]
        .clone();
    edit_release(&h, &p, &dep);
    stage_latest_as_legacy_archive(&h, &dep["owner"]);
    let r = row(&h, &dep["owner"]);
    let preserved = stored(&h, &r);
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    assert_eq!(restored["kind"], "editing", "{restored}");
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    assert_eq!(stored(&h, &r), preserved);
    let editing = edit_begin(&h, &p, &id);
    let mut b = editing["body"].clone();
    b["fields"] = saved["body"]["fields"].clone();
    b["fields"][0]["value"]["value"]["instances"][0]["source"] = A.into();
    b["fields"][0]["value"]["value"]["instances"][0]["fields"][0]["value"] =
        json!({"intent":"set","value":{"kind":"number","value":"7"}});
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &dep);
    stage_latest_as_legacy_archive(&h, &dep["owner"]);
    let r = row(&h, &dep["owner"]);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut second = raw["fieldValues"][G]["instances"][A].clone();
    second["values"][N] = json!({"kind":"number","value":"11"});
    raw["fieldValues"][G]["instances"][B] = second;
    raw["fieldValues"][G]["instanceOrder"] = json!([A, B]);
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        raw["fieldValues"][G]["instances"][A]["values"][N]["value"],
        "7"
    );
    assert_eq!(
        raw["fieldValues"][G]["instances"][B]["values"][N]["value"],
        "11"
    );
    h.close_clean();
}

#[test]
fn archive_recovery_clone_keeps_archived_child_and_unknown_source_metadata() {
    let h = Harness::new();
    let p = h.open();
    let t = super::groups::template(&h, &p);
    let id = create(&h, &p, &t, "clone archived child");
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    let e = edit_begin(&h, &p, &id);
    let mut body = e["body"].clone();
    body["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"3"}}}]}]}}}]);
    let saved = edit_save(&h, &p, &e, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    raw["fields"][G]["configuration"]["members"][N]["lifecycle"] = "archived".into();
    raw["fields"][G]["configuration"]["memberOrder"]
        .as_array_mut()
        .unwrap()
        .retain(|v| v != N);
    fs::write(tp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let dp = h.root.join(format!("documents/{id}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    raw["fieldValues"][G]["instances"][A]["futureSynthetic"] = "opaque retained".into();
    fs::write(&dp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let e = edit_begin(&h, &p, &id);
    let mut body = e["body"].clone();
    body["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":A,"fields":[]},{"id":B,"source":A,"fields":[{"field":N,"value":{"intent":"keep"}}]}]}}}]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let r = row(&h, &d["owner"]);
    let bytes = stored(&h, &r);
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    assert!(
        c["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] != "blocked"),
        "{c}"
    );
    let e=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &e, "4", e["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw: Value = serde_json::from_slice(&fs::read(dp).unwrap()).unwrap();
    assert_eq!(
        raw["fieldValues"][G]["instances"][B]["values"][N]["value"],
        "3"
    );
    assert_eq!(
        raw["fieldValues"][G]["instances"][B]["futureSynthetic"],
        "opaque retained"
    );
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_composite_stages_new_definition_then_document_same_record() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let t = seed(&h, &p);
    let id = create(&h, &p, &t, "composite stage");
    let tv = h.read_template(&p, &t);
    let dv = h.work(json!({"kind":"read_document","project":p,"document":id}));
    let session = h.session(
        &p,
        vec![dv["view"].clone(), tv["view"].clone()],
        "composite",
    );
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    const NEW: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3";
    let failed=h.work(json!({"kind":"save_composite","project":p,"session":session,"document":dv["view"],"template":tv["view"],"revision":"1","edit":{"kind":"create_field","field":NEW,"label":"composite new field","configuration":{"kind":"single_line_text"},"required":false,"presentation":null,"default":{"kind":"unset"},"index":null},"edits":[{"kind":"set","field":NEW,"value":{"kind":"single_line_text","value":"composite preserved input"}}]}));
    assert_eq!(failed["custody"], "Preserved", "{failed}");
    assert!(h.control(
        json!({"kind":"session_control","project":p,"session":session,"control":"accept"})
    )["error"]
        .is_null());
    assert!(h.control(json!({"kind":"session_control","project":p,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
    let listing = h.work(json!({"kind":"recovery_page","cursor":null}));
    let composite = listing["page"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["row"]["payloadKind"] == "admitted_composite")
        .unwrap();
    assert_eq!(composite["name"], "composite stage");
    provider
        .lose
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    let r = page["page"]["entries"][0]["row"].clone();
    let bytes = stored(&h, &r);
    let snapshot = inspect(&h, &r);
    let compare_template=h.work(json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":[{"kind":"component_template"}]}));
    assert_eq!(
        compare_template["kind"], "recovery_selection",
        "{compare_template}"
    );
    let snapshot = compare_template["selection"]["snapshot"].clone();
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
    let c: Value = serde_json::from_str(&text).unwrap();
    let choices = Value::Array(
        c["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["status"] != "blocked")
            .map(|c| json!({"kind":"change","change":c["id"]}))
            .collect(),
    );
    let resumed = h
        .work(json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":choices}));
    assert_eq!(resumed["kind"], "template_draft", "{resumed}");
    let status = resumed["status"].clone();
    let mut offset = json!("0");
    let mut text = String::new();
    loop {
        let c=h.work(json!({"kind":"template_draft_content","project":p,"session":status["owner"],"snapshot":status["snapshot"],"offset":offset}));
        text.push_str(c["content"]["text"].as_str().unwrap());
        offset = c["content"]["next"].clone();
        if offset.is_null() {
            break;
        }
    }
    let c: Value = serde_json::from_str(&text).unwrap();
    let saved=h.work(json!({"kind":"template_draft","project":p,"session":status["owner"],"generation":"3","body":c["body"],"action":"save"}));
    assert!(saved["status"]["error"].is_null(), "{saved}");
    assert_eq!(saved["status"]["outcome"]["disk"], "committed", "{saved}");
    let status = saved["status"].clone();
    assert!(h.control(json!({"kind":"release_template_draft","project":p,"session":status["owner"],"generation":status["generation"],"body":null,"discard":false}))["error"].is_null());
    let (_, c) = compare(&h, &p, &snapshot);
    assert_eq!(c["templatePart"], false, "{c}");
    assert!(
        c["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] != "blocked"),
        "{c}"
    );
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &restored, "5", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
            .unwrap();
    assert!(
        raw["fieldValues"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v["value"] == "composite preserved input"),
        "{raw}"
    );
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn startup_composite_transition_normal_template_resume_reconnects_document_input() {
    for concurrent_value in [false, true] {
        let provider = Arc::new(Provider::new());
        let h = Harness::with_provider(provider.clone());
        let p = h.open();
        let (t, creation_session) = seed_with_session(&h, &p);
        assert!(h.control(
            json!({"kind":"session_control","project":p,"session":creation_session,"control":"end"})
        )["error"]
            .is_null());
        let id = create(&h, &p, &t, "normal composite transition");
        let tv = h.read_template(&p, &t);
        let dv = h.work(json!({"kind":"read_document","project":p,"document":id}));
        let session = h.session(
            &p,
            vec![dv["view"].clone(), tv["view"].clone()],
            "composite",
        );
        provider
            .lose
            .store(true, std::sync::atomic::Ordering::SeqCst);
        const NEW: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3";
        let failed = h.work(json!({"kind":"save_composite","project":p,"session":session,"document":dv["view"],"template":tv["view"],"revision":"1","edit":{"kind":"create_field","field":NEW,"label":"linked composite field","configuration":{"kind":"single_line_text"},"required":false,"presentation":null,"default":{"kind":"unset"},"index":null},"edits":[{"kind":"set","field":NEW,"value":{"kind":"single_line_text","value":"linked preserved document value"}}]}));
        assert_eq!(failed["custody"], "Preserved", "{failed}");
        h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"accept"}),
        );
        h.control(json!({"kind":"session_control","project":p,"session":session,"control":"acknowledge_recovery"}));
        provider
            .lose
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let ended = h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"end"}),
        );
        assert!(ended["error"].is_null(), "{ended}");
        let listed = h.work(json!({"kind":"list_templates","project":p}));
        assert_eq!(listed["kind"], "templates", "{listed}");
        let view = h.read_template(&p, &t);
        let resumed = super::super::workspace::begin(&h, &p, view["view"].clone());
        let body = super::super::workspace::content(&h, &p, &resumed)["body"].clone();
        assert!(
            body["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["label"] == "linked composite field"),
            "{resumed} {body}"
        );
        let saved = super::super::workspace::submit(
            &h,
            &p,
            &resumed,
            resumed["generation"].as_str().unwrap(),
            &body,
            "save",
        );
        assert!(saved["error"].is_null(), "{saved}");
        assert!(super::super::workspace::release(&h, &p, &saved, false)["error"].is_null());
        let document_path = h.root.join(format!("documents/{id}.json"));
        if concurrent_value {
            let template: Value = serde_json::from_slice(
                &fs::read(h.root.join(format!("templates/{t}.json"))).unwrap(),
            )
            .unwrap();
            let field = template["fields"]
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, definition)| definition["label"] == "linked composite field")
                .unwrap()
                .0
                .clone();
            let mut current: Value =
                serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
            current["fieldValues"][field] =
                json!({"kind":"text","value":"different explicitly saved current value"});
            fs::write(&document_path, serde_json::to_vec(&current).unwrap()).unwrap();
        }
        let before_resume = fs::read(&document_path).unwrap();
        let mut doc = edit_begin(&h, &p, &id);
        assert_eq!(doc["remaining_input"], false, "{doc}");
        assert_eq!(
            fs::read(&document_path).unwrap(),
            before_resume,
            "Opening never overwrites canonical content"
        );
        if concurrent_value {
            assert_eq!(doc["problem"], "DraftConflict", "{doc}");
            assert!(doc["comparison"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["status"] == "conflict"));
            let selected: Vec<_> = doc["comparison"]
                .as_array()
                .unwrap()
                .iter()
                .map(|change| change["id"].clone())
                .collect();
            doc = request(
                &h,
                &p,
                json!({"action":"edit_resume","owner":doc["owner"],"selected":selected}),
            )["value"]
                .clone();
            assert_eq!(
                fs::read(&document_path).unwrap(),
                before_resume,
                "Selection is not a canonical save"
            );
        } else {
            assert!(doc["problem"].is_null(), "{doc}");
            assert!(doc.get("comparison").is_none(), "{doc}");
        }
        assert!(
            doc["body"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["value"]["value"]["value"] == "linked preserved document value"),
            "{doc}"
        );
        let generation =
            (doc["generation"].as_str().unwrap().parse::<u64>().unwrap() + 1).to_string();
        let saved = edit_save(&h, &p, &doc, &generation, doc["body"].clone());
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        edit_release(&h, &p, &saved);
        let raw: Value =
            serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
                .unwrap();
        assert!(raw["fieldValues"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v["value"] == "linked preserved document value"));
        h.close_clean();
    }
}

#[test]
fn archive_recovery_partial_missing_definition_keeps_original_and_restores_supported_field() {
    let h = Harness::new();
    let p = h.open();
    let t = seed(&h, &p);
    let id = create(&h, &p, &t, "partial salvage");
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["fields"] = json!([
        set(LEFT, "unsupported retained input"),
        set(RIGHT, "supported recovered input")
    ]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let r = row(&h, &d["owner"]);
    let bytes = stored(&h, &r);
    let path = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    raw["fields"].as_object_mut().unwrap().remove(LEFT);
    raw["fieldOrder"] = json!([RIGHT]);
    raw["revision"] = 2.into();
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let dp = h.root.join(format!("documents/{id}.json"));
    let mut doc: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    doc["orphanedFieldDefinitions"][LEFT] =
        json!({"label":"left historical","kind":"singleLineText","options":{}});
    fs::write(dp, serde_json::to_vec(&doc).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    assert!(
        c["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["status"] == "blocked"),
        "{c}"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
            .unwrap();
    assert_eq!(
        raw["fieldValues"][RIGHT]["value"],
        "supported recovered input"
    );
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_m11_seven_template_changes_save_reopen_and_preserve_current_label() {
    let h = Harness::new();
    let p = h.open();
    let t = super::groups::template(&h, &p);
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const R: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000003";
    const SC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000011";
    const O: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let scalar = json!({"label":"current label","kind":"singleLineText","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"singleLineText"},"presentation":{},"futureSynthetic":"retained"});
    raw["fields"][LEFT] = scalar;
    raw["fields"][SC] = json!({"label":"choice","kind":"singleChoice","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"singleChoice","optionOrder":[O],"options":{(O):{"label":"original option","lifecycle":"active"}}},"presentation":{}});
    raw["fields"][G]["configuration"]["members"][R] = json!({"label":"title","kind":"richText","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"richText"},"presentation":{}});
    raw["fields"][G]["configuration"]["memberOrder"] = json!([N, R]);
    raw["fields"][G]["configuration"]["members"]
        .as_object_mut()
        .unwrap()
        .retain(|id, _| id == N || id == R);
    raw["fields"][G]["presentation"]["cardTitleField"] = R.into();
    raw["fieldOrder"] = json!([LEFT, G, SC]);
    fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let tv = h.read_template(&p, &t);
    assert!(tv["error"].is_null(), "{tv}");
    let status = h.work(json!({"kind":"begin_template_draft","project":p,"view":tv["view"]}))
        ["status"]
        .clone();
    let read_body = |status: &Value| {
        let mut offset = json!("0");
        let mut text = String::new();
        loop {
            let c = h.work(json!({"kind":"template_draft_content","project":p,"session":status["owner"],"snapshot":status["snapshot"],"offset":offset}));
            text.push_str(c["content"]["text"].as_str().unwrap());
            offset = c["content"]["next"].clone();
            if offset.is_null() {
                break;
            }
        }
        serde_json::from_str::<Value>(&text).unwrap()["body"].clone()
    };
    let mut body = read_body(&status);
    body["name"] = "M11 recovered".into();
    let members = body["fields"][1]["configuration"]["members"]
        .as_array_mut()
        .unwrap();
    members.swap(0, 1);
    members.push(json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"M11 new child","configuration":{"kind":"rich_text"},"required":false,"presentation":{"intent":"keep"},"default":{"intent":"unset"},"archived":false}));
    body["fields"][1]["configuration"]["cardTitleField"] = json!({"intent":"unset"});
    body["fields"][2]["configuration"]["options"].as_array_mut().unwrap().push(json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"M11 new option","archived":false}));
    body["fields"].as_array_mut().unwrap().swap(0, 2);
    let deposited = h.control(json!({"kind":"template_draft","project":p,"session":status["owner"],"generation":"2","body":body,"action":"deposit"}));
    let d = &deposited["status"];
    assert!(h.control(json!({"kind":"release_template_draft","project":p,"session":d["owner"],"generation":d["generation"],"body":null,"discard":false}))["error"].is_null());
    stage_latest_as_legacy_archive(&h, &d["draftId"]);
    let r = row(&h, &d["draftId"]);
    let bytes = stored(&h, &r);
    raw["fields"][LEFT]["label"] = "independent current label".into();
    raw["revision"] = 2.into();
    fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let before = fs::read(&tp).unwrap();
    let (snapshot, c) = compare(&h, &p, &inspect(&h, &r));
    assert_eq!(c["comparison"].as_array().unwrap().len(), 7, "{c}");
    assert!(c["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v["status"] != "blocked"));
    assert_eq!(fs::read(&tp).unwrap(), before);
    let choices: Value = c["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"kind":"change","change":c["id"]}))
        .collect();
    let resumed = h
        .work(json!({"kind":"recovery_restore","project":p,"snapshot":snapshot,"reapply":choices}));
    assert_eq!(resumed["kind"], "template_draft", "{resumed}");
    let status = &resumed["status"];
    let recovered_body = read_body(status);
    let saved = h.work(json!({"kind":"template_draft","project":p,"session":status["owner"],"generation":"3","body":recovered_body,"action":"save"}));
    assert!(saved["status"]["error"].is_null(), "{saved}");
    assert_eq!(saved["status"]["outcome"]["disk"], "committed", "{saved}");
    let s = &saved["status"];
    assert!(h.control(json!({"kind":"release_template_draft","project":p,"session":s["owner"],"generation":s["generation"],"body":null,"discard":false}))["error"].is_null());
    let opened = h.read_template(&p, &t);
    assert!(opened["error"].is_null(), "{opened}");
    let after: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    assert_eq!(after["name"], "M11 recovered");
    assert_eq!(after["fields"][LEFT]["label"], "independent current label");
    assert_eq!(after["fields"][LEFT]["futureSynthetic"], "retained");
    assert!(after["fields"][G]["presentation"]
        .get("cardTitleField")
        .is_none());
    assert_eq!(after["fields"][G]["configuration"]["memberOrder"][0], R);
    assert_eq!(
        after["fields"][G]["configuration"]["members"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        after["fields"][SC]["configuration"]["options"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(after["fieldOrder"], json!([SC, G, LEFT]));
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_deleted_known_card_materializes_kept_original_cell() {
    let h = Harness::new();
    let p = h.open();
    let t = super::groups::template(&h, &p);
    let id = create(&h, &p, &t, "deleted known card");
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"7"}}}]}]}}}]);
    let saved = edit_save(&h, &p, &e, "2", b);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["name"] = json!({"intent":"set","value":"recovered known card"});
    b["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":A,"fields":[]},{"id":B,"source":A,"fields":[{"field":N,"value":{"intent":"keep"}}]}]}}}]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let r = row(&h, &d["owner"]);
    let dp = h.root.join(format!("documents/{id}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    raw["fieldValues"][G] = json!({"kind":"group","instanceOrder":[],"instances":{}});
    fs::write(&dp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    let restored=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw: Value = serde_json::from_slice(&fs::read(dp).unwrap()).unwrap();
    assert!(raw["fieldValues"][G]["instances"].get(A).is_none());
    assert_eq!(
        raw["fieldValues"][G]["instances"][B]["values"][N]["value"],
        "7"
    );
    h.close_clean();
}

#[test]
fn archive_recovery_corrupt_missing_snapshot_blocks_before_entry_and_keeps_readable_record() {
    let h = Harness::new();
    let p = h.open();
    let t = seed(&h, &p);
    let id = create(&h, &p, &t, "corrupt snapshot retained");
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["fields"] = json!([set(RIGHT, "readable retained input")]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let r = row(&h, &d["owner"]);
    let bytes = stored(&h, &r);
    let path = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    raw["fields"].as_object_mut().unwrap().remove(LEFT);
    raw["fieldOrder"] = json!([RIGHT]);
    raw["revision"] = 2.into();
    fs::write(path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let dp = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&dp).unwrap();
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    assert!(
        c["comparison"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] == "blocked"),
        "{c}"
    );
    assert!(serde_json::to_string(&c["draft"])
        .unwrap()
        .contains("readable retained input"));
    assert_eq!(fs::read(dp).unwrap(), before);
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
fn archive_recovery_reads_100_schema_four_record_from_store_and_saves_selected_input() {
    use crate::data::edit_recovery::model::Deposit;
    let h = Harness::new();
    let p = h.open();
    let t = seed(&h, &p);
    let id = create(&h, &p, &t, "legacy recovery");
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["fields"] = json!([set(LEFT, "1.0.0 retained input")]);
    let d = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    edit_release(&h, &p, &d);
    stage_latest_as_legacy_archive(&h, &d["owner"]);
    let r = row(&h, &d["owner"]);
    let mut envelope = Deposit::decode(&stored(&h, &r)).unwrap().envelope().clone();
    envelope.key.draft_id = uuid::Uuid::new_v4().to_string();
    envelope.deposit_id = uuid::Uuid::new_v4().to_string();
    envelope.app_version = "1.0.0".into();
    let record = Deposit::freeze(envelope).unwrap();
    let mut raw: Value = serde_json::from_slice(record.bytes()).unwrap();
    raw["recoverySchemaVersion"] = 4.into();
    let bytes = serde_json::to_vec(&raw).unwrap();
    let record = Deposit::decode(&bytes).unwrap();
    h.state
        .recovery
        .connect()
        .unwrap()
        .lock()
        .unwrap()
        .accept(&record)
        .unwrap();
    let r = row(&h, &json!(record.key().draft_id));
    assert_eq!(stored(&h, &r), bytes);
    let (_, c) = compare(&h, &p, &inspect(&h, &r));
    let e=request(&h,&p,json!({"action":"edit_restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"],"reapply":proof(&c)}))["value"].clone();
    let saved = edit_save(&h, &p, &e, "4", e["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    assert_eq!(stored(&h, &r), bytes);
    h.close_clean();
}

#[test]
#[ignore = "Explicit owned manual fixture generation; requires WB_ARCHIVE_FIXTURE_ROOT"]
fn archive_recovery_prepare_manual_fixtures() {
    use crate::data::edit_recovery::model::Deposit;
    let base = PathBuf::from(
        std::env::var("WB_ARCHIVE_FIXTURE_ROOT").expect("explicit owned path required"),
    );
    assert!(base.is_absolute() && base.to_string_lossy().contains("V1-101-복원시험"));
    assert!(
        !base.join("project/templates").exists(),
        "fixture path already initialized; preserve it"
    );
    let staging = PathBuf::from(
        std::env::var("WB_ARCHIVE_RECOVERY_STAGING").expect("explicit owned staging path required"),
    );
    let provider = Arc::new(Provider::new());
    let h = Harness::at_recovery(base.clone(), provider.clone(), false, Some(staging.clone()));
    let p = h.open();
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const R: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000003";
    const IM: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000004";
    const FI: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000005";
    const REL: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000006";
    const LINK: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000007";
    const SC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000011";
    const MC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000012";
    const O1: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    const O2: &str = "cccccccc-cccc-4ccc-8ccc-000000000002";
    const O3: &str = "cccccccc-cccc-4ccc-8ccc-000000000003";
    const O4: &str = "cccccccc-cccc-4ccc-8ccc-000000000004";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    let t = super::groups::template(&h, &p);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    raw["name"] = "M02–M06 복원 실습".into();
    let def = |kind: &str, label: &str| json!({"label":label,"kind":kind,"required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":kind},"presentation":{}});
    raw["fields"] = json!({G:raw["fields"][G].clone(),LEFT:def("singleLineText","관측 목적"),RIGHT:def("singleLineText","다른 작업 기록"),SC:def("singleChoice","단일 선택"),MC:def("multiChoice","복수 선택")});
    raw["fieldOrder"] = json!([LEFT, RIGHT, G, SC, MC]);
    raw["fields"][G]["label"] = "상세 섹션 반복 그룹".into();
    let mut relation = def("relation", "연계 관측 문서");
    relation["configuration"] =
        json!({"kind":"relation","multiple":true,"allowedTemplates":[],"reciprocalNotice":true});
    raw["fields"][G]["configuration"]["members"][REL] = relation;
    raw["fields"][G]["configuration"]["members"][LINK] = def("documentLink", "참고 문서");
    raw["fields"][G]["configuration"]["memberOrder"] = json!([R, N, IM, FI, REL, LINK]);
    for (id, label) in [
        (N, "관측 수치"),
        (R, "섹션 제목"),
        (IM, "관측 이미지"),
        (FI, "첨부 자료"),
    ] {
        raw["fields"][G]["configuration"]["members"][id]["label"] = label.into();
    }
    raw["fields"][G]["presentation"]["cardTitleField"] = R.into();
    for (id, kind) in [(SC, "singleChoice"), (MC, "multiChoice")] {
        raw["fields"][id]["configuration"] = json!({"kind":kind,"optionOrder":[O1,O2],"options":{O1:{"label":"준비","lifecycle":"active"},O2:{"label":"진행","lifecycle":"active"}}});
    }
    raw["fields"][MC]["configuration"] = json!({"kind":"multiChoice","optionOrder":[O3,O4],"options":{O3:{"label":"준비","lifecycle":"active"},O4:{"label":"진행","lifecycle":"active"}}});
    fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let mut png = vec![];
    {
        let mut encoder = png::Encoder::new(&mut png, 8, 8);
        encoder.set_color(png::ColorType::Rgb);
        let mut w = encoder.write_header().unwrap();
        w.write_image_data(&[60; 8 * 8 * 3]).unwrap();
    }
    let image_source = base.join("synthetic-card.png");
    fs::write(&image_source, png).unwrap();
    let file_source = base.join("synthetic-note.txt");
    fs::write(
        &file_source,
        "독립 합성 관측 자료입니다. 실제 사용자 문서가 아닙니다.",
    )
    .unwrap();
    let assets = crate::data::assets::Store::open(&h.root, true).unwrap();
    let image = assets
        .import(&image_source, true, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let file = assets
        .import(&file_source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(assets);
    let other = create(&h, &p, &t, "M03 이후 작업 확인 문서");
    let rich = |text: &str| json!({"kind":"rich_text","content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":text,"marks":[]}]}]}});
    let cards=[(A,"첫 관측","cccccccc-cccc-4ccc-8ccc-000000000101"),(B,"둘째 관측","cccccccc-cccc-4ccc-8ccc-000000000102")].map(|(id,title,connection)|json!({"id":id,"source":null,"fields":[{"field":R,"value":{"intent":"set","value":rich(title)}},{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"7"}}},{"field":IM,"value":{"intent":"set","value":{"kind":"image","value":[image.id]}}},{"field":FI,"value":{"intent":"set","value":{"kind":"file","value":[file.id]}}},{"field":REL,"value":{"intent":"set","value":{"kind":"relation","links":[{"id":connection,"document":other,"oneWay":true,"name":"합성 관측 연결"}]}}},{"field":LINK,"value":{"intent":"set","value":{"kind":"document_link","documents":[other]}}}]}));
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "M01 복원 기준 문서".into();
    body["fields"] = json!([set(LEFT,"원래 관측 목적"),set(RIGHT,"기존 다른 작업"),{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":cards}}},{"field":SC,"value":{"intent":"set","value":{"kind":"single_choice","option":O1}}},{"field":MC,"value":{"intent":"set","value":{"kind":"multi_choice","options":[O3,O4]}}}]);
    let saved = save(&h, &p, &d, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let ux_document = saved["outcome"]["artifact"].clone();
    release(&h, &p, &saved, false);
    let mut manifest = json!({"project":h.root,"stagingStore":staging,"uxTemplate":t,"uxDocument":ux_document,"otherDocument":other,"records":[]});
    for (manual, name) in [
        ("M07", "M07 기본 보관 입력"),
        ("M09", "M09 서로 다른 필드 회수"),
        ("M10", "M10 같은 필드 충돌"),
        ("M12", "M12 정의 복원 후 회수"),
        ("M14", "M14 일부 적용 불가 회수"),
    ] {
        let t = seed(&h, &p);
        let tp = h.root.join(format!("templates/{t}.json"));
        let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
        raw["name"] = format!("{manual} 회수 전용 템플릿").into();
        fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        let id = create(&h, &p, &t, name);
        let e = edit_begin(&h, &p, &id);
        let mut body = e["body"].clone();
        body["fields"] = json!([set(LEFT, &format!("{manual} 보관한 왼쪽 입력"))]);
        if manual == "M14" {
            body["fields"]
                .as_array_mut()
                .unwrap()
                .push(set(RIGHT, "M14 회수 가능한 오른쪽 입력"));
        }
        let d = request(
            &h,
            &p,
            json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":body}),
        )["value"]
            .clone();
        edit_release(&h, &p, &d);
        stage_latest_as_legacy_archive(&h, &d["owner"]);
        let r = row(&h, &d["owner"]);
        if manual == "M07" {
            let mut envelope = Deposit::decode(&stored(&h, &r)).unwrap().envelope().clone();
            envelope.key.draft_id = uuid::Uuid::new_v4().to_string();
            envelope.deposit_id = uuid::Uuid::new_v4().to_string();
            envelope.app_version = "1.0.0".into();
            let record = Deposit::freeze(envelope).unwrap();
            let mut raw: Value = serde_json::from_slice(record.bytes()).unwrap();
            raw["recoverySchemaVersion"] = 4.into();
            let record = Deposit::decode(&serde_json::to_vec(&raw).unwrap()).unwrap();
            h.state
                .recovery
                .connect()
                .unwrap()
                .lock()
                .unwrap()
                .accept(&record)
                .unwrap();
        }
        if manual == "M09" || manual == "M10" {
            let dp = h.root.join(format!("documents/{id}.json"));
            let mut raw: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
            raw["fieldValues"][if manual == "M09" { RIGHT } else { LEFT }] =
                json!({"kind":"text","value":format!("{manual} 현재 저장한 내용")});
            fs::write(dp, serde_json::to_vec(&raw).unwrap()).unwrap();
        }
        if manual == "M12" || manual == "M14" {
            let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
            raw["fields"][LEFT]["lifecycle"] = "archived".into();
            raw["fieldOrder"] = json!([RIGHT]);
            raw["revision"] = 2.into();
            fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        }
        manifest["records"].as_array_mut().unwrap().push(
            json!({"manual":manual,"name":name,"template":t,"document":id,"draftId":d["owner"]}),
        );
    }
    {
        let t = seed(&h, &p);
        let tp = h.root.join(format!("templates/{t}.json"));
        let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
        raw["name"] = "M08 새 문서 회수 템플릿".into();
        fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        let e = begin(&h, &p, &t);
        let mut body = e["body"].clone();
        body["name"] = "M08 보관 새 문서".into();
        body["fields"] = json!([set(LEFT, "M08 보관한 값")]);
        let d = request(
            &h,
            &p,
            json!({"action":"deposit","owner":e["owner"],"generation":"2","body":body}),
        )["value"]
            .clone();
        release(&h, &p, &d, false);
        raw["name"] = "M08 이름이 바뀐 템플릿".into();
        raw["fieldOrder"] = json!([RIGHT, LEFT]);
        raw["fields"][LEFT]["label"] = "M08 새 왼쪽 이름".into();
        raw["fields"][RIGHT]["required"] = true.into();
        raw["revision"] = 2.into();
        fs::write(tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        manifest["records"].as_array_mut().unwrap().push(
            json!({"manual":"M08","name":"M08 보관 새 문서","template":t,"draftId":d["owner"]}),
        );
    }
    {
        let tv = h.read_template(&p, &t);
        let status = h.work(json!({"kind":"begin_template_draft","project":p,"view":tv["view"]}))
            ["status"]
            .clone();
        let mut offset = json!("0");
        let mut text = String::new();
        loop {
            let c=h.work(json!({"kind":"template_draft_content","project":p,"session":status["owner"],"snapshot":status["snapshot"],"offset":offset}));
            text.push_str(c["content"]["text"].as_str().unwrap());
            offset = c["content"]["next"].clone();
            if offset.is_null() {
                break;
            }
        }
        let mut body: Value = serde_json::from_str::<Value>(&text).unwrap()["body"].clone();
        body["name"] = "M11 회수한 구조".into();
        body["fields"][2]["configuration"]["cardTitleField"] = json!({"intent":"unset"});
        body["fields"][2]["configuration"]["members"].as_array_mut().unwrap().push(json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"M11 새 하위 텍스트","configuration":{"kind":"rich_text"},"required":false,"presentation":{"intent":"keep"},"default":{"intent":"unset"},"archived":false}));
        body["fields"][3]["configuration"]["options"].as_array_mut().unwrap().push(json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":"M11 새 선택지","archived":false}));
        body["fields"].as_array_mut().unwrap().swap(0, 1);
        let deposited=h.control(json!({"kind":"template_draft","project":p,"session":status["owner"],"generation":"2","body":body,"action":"deposit"}));
        let d = &deposited["status"];
        assert!(h.control(json!({"kind":"release_template_draft","project":p,"session":d["owner"],"generation":d["generation"],"body":null,"discard":false}))["error"].is_null());
        let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
        raw["fields"][LEFT]["label"] = "M11 현재 별도 라벨 변경".into();
        raw["revision"] = 2.into();
        fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        manifest["records"].as_array_mut().unwrap().push(
            json!({"manual":"M11","name":"M11 회수한 구조","template":t,"draftId":d["draftId"]}),
        );
    }
    {
        let t = seed(&h, &p);
        let tp = h.root.join(format!("templates/{t}.json"));
        let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
        raw["name"] = "M13 복합 회수 템플릿".into();
        fs::write(tp, serde_json::to_vec(&raw).unwrap()).unwrap();
        let id = create(&h, &p, &t, "M13 복합 회수 문서");
        let tv = h.read_template(&p, &t);
        let dv = h.work(json!({"kind":"read_document","project":p,"document":id}));
        let session = h.session(
            &p,
            vec![dv["view"].clone(), tv["view"].clone()],
            "composite",
        );
        provider
            .lose
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let failed=h.work(json!({"kind":"save_composite","project":p,"session":session,"document":dv["view"],"template":tv["view"],"revision":"1","edit":{"kind":"create_field","field":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3","label":"M13 새 필드","configuration":{"kind":"single_line_text"},"required":false,"presentation":null,"default":{"kind":"unset"},"index":null},"edits":[{"kind":"set","field":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3","value":{"kind":"single_line_text","value":"M13 복합 보관 문서 입력"}}]}));
        assert_eq!(failed["custody"], "Preserved", "{failed}");
        assert!(h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"accept"})
        )["error"]
            .is_null());
        assert!(h.control(json!({"kind":"session_control","project":p,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
        provider
            .lose
            .store(false, std::sync::atomic::Ordering::SeqCst);
        manifest["records"]
            .as_array_mut()
            .unwrap()
            .push(json!({"manual":"M13","name":"M13 복합 회수 문서","template":t,"document":id}));
    }
    fs::write(
        base.join("fixture-manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    h.close_clean();
}

#[test]
fn archive_recovery_document_snapshot_reattaches_after_field_and_group_restore() {
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const SC: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000011";
    const O: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    let h = Harness::new();
    let p = h.open();
    let t = super::groups::template(&h, &p);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let def = json!({"label":"ordinary","kind":"singleLineText","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"singleLineText"},"presentation":{}});
    raw["fields"][LEFT] = def.clone();
    raw["fields"][RIGHT] = def;
    raw["fields"][SC] = json!({"label":"choice","kind":"singleChoice","lifecycle":"active","required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"singleChoice","optionOrder":[O],"options":{O:{"label":"selected","lifecycle":"active"}}},"presentation":{}});
    raw["fieldOrder"] = json!([LEFT, RIGHT, G, SC]);
    fs::write(&tp, serde_json::to_vec(&raw).unwrap()).unwrap();
    let id = create(&h, &p, &t, "reattachment");
    let e = edit_begin(&h, &p, &id);
    let mut b = e["body"].clone();
    b["fields"] = json!([set(LEFT,"original preserved value"),{"field":SC,"value":{"intent":"set","value":{"kind":"single_choice","option":O}}},{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"7"}}}]}]}}}]);
    let saved = edit_save(&h, &p, &e, "2", b);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    for restoring in [false, true] {
        let tv = h.read_template(&p, &t);
        let status = h.work(json!({"kind":"begin_template_draft","project":p,"view":tv["view"]}))
            ["status"]
            .clone();
        let mut offset = json!("0");
        let mut text = String::new();
        loop {
            let c=h.work(json!({"kind":"template_draft_content","project":p,"session":status["owner"],"snapshot":status["snapshot"],"offset":offset}));
            text.push_str(c["content"]["text"].as_str().unwrap());
            offset = c["content"]["next"].clone();
            if offset.is_null() {
                break;
            }
        }
        let mut b: Value = serde_json::from_str::<Value>(&text).unwrap()["body"].clone();
        for field in b["fields"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .filter(|f| f["id"] == LEFT || f["id"] == G)
        {
            field["archived"] = (!restoring).into();
            if restoring {
                field["restore"] = true.into();
            }
        }
        let option = &mut b["fields"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|f| f["id"] == SC)
            .unwrap()["configuration"]["options"][0];
        option["archived"] = (!restoring).into();
        if restoring {
            option["restore"] = true.into();
        }
        let saved=h.work(json!({"kind":"template_draft","project":p,"session":status["owner"],"generation":"2","body":b,"action":"save"}));
        assert!(saved["status"]["error"].is_null(), "{saved}");
        let status = &saved["status"];
        assert!(h.control(json!({"kind":"release_template_draft","project":p,"session":status["owner"],"generation":status["generation"],"body":null,"discard":false}))["error"].is_null());
        {
            let path = h.root.join(format!("documents/{id}.json"));
            let before = fs::read(&path).unwrap();
            let read = request(&h, &p, json!({"action":"read","document":id}));
            let fields = read["value"]["fields"].as_array().unwrap();
            for field in [LEFT, G, SC] {
                let rows: Vec<_> = fields.iter().filter(|row| row["id"] == field).collect();
                assert_eq!(rows.len(), 1, "snapshot field duplicated: {read}");
                assert_eq!(
                    rows[0]["state"],
                    if !restoring && field != SC {
                        "Archived"
                    } else {
                        "Active"
                    },
                    "{read}"
                );
            }
            assert_eq!(
                fs::read(&path).unwrap(),
                before,
                "reading must not remove snapshots"
            );
        }
        let e = edit_begin(&h, &p, &id);
        let mut b = e["body"].clone();
        b["fields"] = if restoring {
            json!([set(LEFT, "edited reattached original")])
        } else {
            json!([set(RIGHT, "independent edit while archived")])
        };
        let saved = edit_save(&h, &p, &e, "2", b);
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        edit_release(&h, &p, &saved);
        let read = request(&h, &p, json!({"action":"read","document":id}));
        for field in [LEFT, G, SC] {
            assert_eq!(
                read["value"]["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|row| row["id"] == field)
                    .count(),
                1,
                "preserved snapshot duplicated after save: {read}"
            );
        }
        let raw: Value =
            serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
                .unwrap();
        assert_eq!(
            raw["fieldValues"][G]["instances"][A]["values"][N]["value"],
            "7"
        );
        if !restoring {
            assert!(raw["orphanedFieldDefinitions"].get(LEFT).is_some());
            assert!(raw["orphanedFieldDefinitions"].get(G).is_some());
            assert!(raw["orphanedFieldDefinitions"].get(SC).is_some());
        } else {
            assert!(raw["orphanedFieldDefinitions"].get(LEFT).is_none());
            assert!(raw["orphanedFieldDefinitions"].get(G).is_none());
            assert!(raw["orphanedFieldDefinitions"].get(SC).is_none());
            assert_eq!(
                raw["fieldValues"][RIGHT]["value"],
                "independent edit while archived"
            );
            assert_eq!(
                raw["fieldValues"][LEFT]["value"],
                "edited reattached original"
            );
        }
    }
    let e = edit_begin(&h, &p, &id);
    assert!(e["read"]["warnings"].as_array().unwrap().is_empty(), "{e}");
    edit_release(&h, &p, &e);
    h.close_clean();
}

#[test]
fn read_snapshot_dedup_preserves_genuine_orphan_and_conflict_diagnostics() {
    for conflict in [false, true] {
        let h = Harness::new();
        let p = h.open();
        let t = seed(&h, &p);
        let id = create(&h, &p, &t, "snapshot boundary");
        let path = h.root.join(format!("documents/{id}.json"));
        let mut doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        doc["fieldValues"][LEFT] = json!({"kind":"text","value":"base"});
        doc["orphanedFieldDefinitions"][LEFT] = json!({
            "label":"retained historical field",
            "kind":"singleLineText",
            "options":{}
        });
        if conflict {
            doc["orphanedFieldDefinitions"][LEFT]["futureSynthetic"] =
                "must not disappear when reattaching".into();
        }
        fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        if !conflict {
            let tp = h.root.join(format!("templates/{t}.json"));
            let mut template: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
            template["fields"].as_object_mut().unwrap().remove(LEFT);
            template["fieldOrder"] = json!([RIGHT]);
            template["revision"] = 2.into();
            fs::write(tp, serde_json::to_vec(&template).unwrap()).unwrap();
        }
        let before = fs::read(&path).unwrap();
        let read = request(&h, &p, json!({"action":"read","document":id}));
        assert_eq!(read["value"]["kind"], "read", "{read}");
        let fields = read["value"]["fields"].as_array().unwrap();
        let orphan = fields
            .iter()
            .find(|row| row["id"] == LEFT && row["state"] == "Orphan")
            .unwrap_or_else(|| panic!("real orphan or conflict hidden: {read}"));
        assert_eq!(orphan["value"]["value"], "base", "{read}");
        if conflict {
            assert!(
                read["value"]["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|warning| warning == "LossyOrphanReattachment"),
                "{read}"
            );
            assert!(
                fields
                    .iter()
                    .any(|row| row["id"] == LEFT && !row["problem"].is_null()),
                "{read}"
            );
        } else {
            assert_eq!(fields.iter().filter(|row| row["id"] == LEFT).count(), 1);
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "read mutated snapshot storage"
        );
        let editing = edit_begin(&h, &p, &id);
        let editor_fields = editing["read"]["fields"].as_array().unwrap();
        assert!(
            editor_fields.iter().any(|row| row["id"] == LEFT
                && row["state"] == "Orphan"
                && row["value"]["value"] == "base"),
            "{editing}"
        );
        if conflict {
            assert!(
                editing["read"]["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|warning| warning == "LossyOrphanReattachment"),
                "{editing}"
            );
            assert!(
                editor_fields
                    .iter()
                    .any(|row| row["id"] == LEFT && !row["problem"].is_null()),
                "{editing}"
            );
        } else {
            assert_eq!(
                editor_fields.iter().filter(|row| row["id"] == LEFT).count(),
                1
            );
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "editor projection mutated retained snapshot"
        );
        edit_release(&h, &p, &editing);
        h.close_clean();
    }
}

#[test]
fn latest_partial_input_survives_normal_save_end_and_resumes_after_definition_restore() {
    let h = Harness::new();
    let p = h.open();
    let template = seed(&h, &p);
    let id = create(&h, &p, &template, "부분 입력 보존");
    let editor = edit_begin(&h, &p, &id);
    let mut body = editor["body"].clone();
    body["fields"] = json!([
        set(LEFT, "나중에 복원할 입력"),
        set(RIGHT, "지금 적용할 입력")
    ]);
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editor["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &p, &deposited);
    let template_path = h.root.join(format!("templates/{template}.json"));
    let mut source: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    source["fields"][LEFT]["lifecycle"] = "archived".into();
    source["fieldOrder"] = json!([RIGHT]);
    source["revision"] = 2.into();
    fs::write(&template_path, serde_json::to_vec(&source).unwrap()).unwrap();
    let document_path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&document_path).unwrap();
    for pass in 0..2 {
        let resumed = edit_begin(&h, &p, &id);
        assert!(resumed["problem"].is_null(), "{resumed}");
        assert!(
            resumed["comparison"].is_null(),
            "blocked-only input needs no choice: {resumed}"
        );
        assert_eq!(resumed["remaining_input"], true, "{resumed}");
        if pass == 0 {
            assert_eq!(fs::read(&document_path).unwrap(), before);
        }
        let saved = edit_save(
            &h,
            &p,
            &resumed,
            resumed["generation"].as_str().unwrap(),
            resumed["body"].clone(),
        );
        assert!(saved["problem"].is_null(), "{saved}");
        edit_release(&h, &p, &saved);
        let current: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
        assert_eq!(
            current["fieldValues"][RIGHT]["value"], "지금 적용할 입력",
            "{current}"
        );
        let entries = fs::read_dir(
            h.root
                .join(format!(".worldbuild/latest-drafts/document-{id}")),
        )
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        let record: Value = serde_json::from_slice(&fs::read(&entries[0]).unwrap()).unwrap();
        assert!(record["envelope"]["residual"]
            .to_string()
            .contains("나중에 복원할 입력"));
    }
    source["fields"][LEFT]["lifecycle"] = "active".into();
    source["fieldOrder"] = json!([LEFT, RIGHT]);
    source["revision"] = 3.into();
    fs::write(&template_path, serde_json::to_vec(&source).unwrap()).unwrap();
    let resumed = edit_begin(&h, &p, &id);
    assert!(resumed["problem"].is_null(), "{resumed}");
    assert_eq!(resumed["remaining_input"], false, "{resumed}");
    let saved = edit_save(
        &h,
        &p,
        &resumed,
        resumed["generation"].as_str().unwrap(),
        resumed["body"].clone(),
    );
    assert!(saved["problem"].is_null(), "{saved}");
    edit_release(&h, &p, &saved);
    let current: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
    assert_eq!(
        current["fieldValues"][LEFT]["value"], "나중에 복원할 입력",
        "{current}"
    );
    assert_eq!(
        current["fieldValues"][RIGHT]["value"], "지금 적용할 입력",
        "{current}"
    );
    assert_eq!(
        fs::read_dir(
            h.root
                .join(format!(".worldbuild/latest-drafts/document-{id}"))
        )
        .unwrap()
        .count(),
        0
    );
    h.close_clean();
}

#[test]
fn latest_residual_keeps_opaque_original_provenance_across_repeated_save_and_reopen() {
    let h = Harness::new();
    let p = h.open();
    let template = seed(&h, &p);
    let id = create(&h, &p, &template, "원문 메타데이터 보호");
    let editor = edit_begin(&h, &p, &id);
    let mut body = editor["body"].clone();
    body["fields"] = json!([set(LEFT, "7"), set(RIGHT, "기존")]);
    let saved = edit_save(&h, &p, &editor, "2", body);
    assert!(saved["problem"].is_null(), "{saved}");
    edit_release(&h, &p, &saved);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut source: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    source["fieldValues"][LEFT]["futureSynthetic"] = "preserve opaque original".into();
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let editor = edit_begin(&h, &p, &id);
    let mut body = editor["body"].clone();
    body["fields"] = json!([set(LEFT, "8"), set(RIGHT, "적용 가능한 입력")]);
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editor["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &p, &deposited);
    source["fieldValues"][LEFT] = json!({"kind":"unset"});
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    for _ in 0..3 {
        let resumed = edit_begin(&h, &p, &id);
        assert!(resumed["problem"].is_null(), "{resumed}");
        assert_eq!(resumed["remaining_input"], true, "{resumed}");
        assert!(resumed["comparison"].is_null());
        let saved = edit_save(
            &h,
            &p,
            &resumed,
            resumed["generation"].as_str().unwrap(),
            resumed["body"].clone(),
        );
        assert!(saved["problem"].is_null(), "{saved}");
        edit_release(&h, &p, &saved);
        let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(current["fieldValues"][LEFT]["kind"], "unset", "{current}");
        assert_eq!(current["fieldValues"][RIGHT]["value"], "적용 가능한 입력");
        let entries = fs::read_dir(
            h.root
                .join(format!(".worldbuild/latest-drafts/document-{id}")),
        )
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        let record: Value = serde_json::from_slice(&fs::read(&entries[0]).unwrap()).unwrap();
        assert!(
            record["envelope"]["residual"]
                .to_string()
                .contains("preserve opaque original"),
            "{record}"
        );
    }
    h.close_clean();
}

#[test]
fn own_committed_resolved_residual_advances_cached_scan_on_normal_begin() {
    use sha2::Digest;
    use std::sync::atomic::Ordering;
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let template = seed(&h, &p);
    let id = create(&h, &p, &template, "잔여 입력 저장 기준");
    let editor = edit_begin(&h, &p, &id);
    let mut body = editor["body"].clone();
    body["fields"] = json!([set(LEFT, "7"), set(RIGHT, "기존")]);
    let saved = edit_save(&h, &p, &editor, "2", body);
    assert!(saved["problem"].is_null(), "{saved}");
    edit_release(&h, &p, &saved);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut source: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    source["fieldValues"][LEFT]["futureSynthetic"] = "opaque original custody".into();
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let editor = edit_begin(&h, &p, &id);
    let mut body = editor["body"].clone();
    body["fields"] = json!([set(LEFT, "8"), set(RIGHT, "먼저 반영할 입력")]);
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editor["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &p, &deposited);
    source["fieldValues"][LEFT] = json!({"kind":"unset"});
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let editor = edit_begin(&h, &p, &id);
    assert_eq!(editor["remaining_input"], true, "{editor}");
    let saved = edit_save(
        &h,
        &p,
        &editor,
        editor["generation"].as_str().unwrap(),
        editor["body"].clone(),
    );
    assert!(saved["problem"].is_null(), "{saved}");
    edit_release(&h, &p, &saved);
    // Resolve the retained value through a real edit. Interrupt only cleanup,
    // keeping the actual committed candidate and its previous-source evidence.
    let editor = edit_begin(&h, &p, &id);
    assert_eq!(editor["remaining_input"], true, "{editor}");
    let listed = list(&h, &p);
    let generation = (editor["generation"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        + 1)
    .to_string();
    let mut body = editor["body"].clone();
    body["fields"] = json!([set(LEFT, "8"), set(RIGHT, "먼저 반영할 입력")]);
    body["name"] = json!({"intent":"set","value":"잔여 입력까지 반영됨"});
    armed.store(true, Ordering::SeqCst);
    let saved = edit_save(&h, &p, &editor, &generation, body.clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(saved["problem"], "SavedReadRequired", "{saved}");
    let failed = request(
        &h,
        &p,
        json!({"action":"edit_refresh","owner":editor["owner"]}),
    );
    assert_eq!(failed["error"]["code"], "runtime_rejected", "{failed}");
    let deposited = request(&h, &p, json!({"action":"edit_deposit","owner":editor["owner"],"generation":generation,"body":body}))["value"].clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let before = latest_target_deposit(&h, "document", &id);
    assert!(before.envelope().residual.is_some());
    let attempt = before.envelope().attempt.as_ref().unwrap();
    assert_eq!(
        attempt.result,
        crate::data::edit_recovery::model::SaveState::Committed
    );
    assert_eq!(
        attempt.candidate_digest.as_deref(),
        Some(format!("{:x}", sha2::Sha256::digest(fs::read(&path).unwrap())).as_str())
    );
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");
    edit_release(&h, &p, &deposited);
    let resumed = edit_begin(&h, &p, &id);
    assert!(resumed["problem"].is_null(), "{resumed}");
    assert!(resumed["comparison"].is_null(), "{resumed}");
    assert_eq!(
        resumed["generation"],
        (generation.parse::<u64>().unwrap() + 1).to_string()
    );
    assert_eq!(resumed["saved_generation"], resumed["generation"]);
    assert_eq!(resumed["body"]["fields"], json!([]), "{resumed}");
    edit_release(&h, &p, &resumed);
    // No new List/refresh: only normal begin could advance this stale snapshot.
    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "재개 후 새 문서".into();
    let created = save(&h, &p, &draft, "2", body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 2);
    h.close_clean();
}

#[test]
fn latest_group_residual_keeps_only_relevant_opaque_custody_across_save_and_reopen() {
    const G: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    const N: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    for level in ["child", "card", "group"] {
        let h = Harness::new();
        let p = h.open();
        let template = super::groups::template(&h, &p);
        let id = create(&h, &p, &template, "반복 입력 원문 보호");
        let e = edit_begin(&h, &p, &id);
        let mut body = e["body"].clone();
        body["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"3"}}}]},{"id":B,"source":null,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"5"}}}]}]}}}]);
        let saved = edit_save(&h, &p, &e, "2", body);
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        edit_release(&h, &p, &saved);
        let path = h.root.join(format!("documents/{id}.json"));
        let mut source: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match level {
            "child" => {
                source["fieldValues"][G]["instances"][A]["values"][N]["futureSynthetic"] =
                    "opaque original child".into()
            }
            "card" => {
                source["fieldValues"][G]["instances"][A]["futureSynthetic"] =
                    "opaque original card".into()
            }
            _ => source["fieldValues"][G]["futureSynthetic"] = "opaque original group".into(),
        };
        fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
        let e = edit_begin(&h, &p, &id);
        let mut body = e["body"].clone();
        body["fields"] = json!([{"field":G,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":A,"source":A,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"8"}}}]},{"id":B,"source":B,"fields":[{"field":N,"value":{"intent":"set","value":{"kind":"number","value":"11"}}}]}]}}}]);
        let deposited = request(
            &h,
            &p,
            json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":body}),
        )["value"]
            .clone();
        assert_eq!(deposited["deposited"], true, "{deposited}");
        edit_release(&h, &p, &deposited);
        match level {
            "child" => {
                source["fieldValues"][G]["instances"][A]["values"][N]
                    .as_object_mut()
                    .unwrap()
                    .remove("futureSynthetic");
            }
            "card" => {
                source["fieldValues"][G]["instances"][A]
                    .as_object_mut()
                    .unwrap()
                    .remove("futureSynthetic");
            }
            _ => {
                source["fieldValues"][G]
                    .as_object_mut()
                    .unwrap()
                    .remove("futureSynthetic");
            }
        }
        fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
        for cycle in 0..3 {
            eprintln!("synthetic group level={level}, cycle={cycle}");
            let e = edit_begin(&h, &p, &id);
            assert_eq!(e["remaining_input"], true, "{e}");
            assert!(e["comparison"].is_null());
            let saved = edit_save(
                &h,
                &p,
                &e,
                e["generation"].as_str().unwrap(),
                e["body"].clone(),
            );
            assert!(
                ["committed", "no_write"].contains(&saved["outcome"]["disk"].as_str().unwrap()),
                "{saved}"
            );
            edit_release(&h, &p, &saved);
            let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                current["fieldValues"][G]["instances"][A]["values"][N]["value"],
                "3"
            );
            assert_eq!(
                current["fieldValues"][G]["instances"][B]["values"][N]["value"],
                if level == "group" { "5" } else { "11" }
            );
            let entries = fs::read_dir(
                h.root
                    .join(format!(".worldbuild/latest-drafts/document-{id}")),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
            assert_eq!(entries.len(), 1);
            let raw = String::from_utf8(fs::read(entries[0].path()).unwrap()).unwrap();
            assert!(raw.contains(&format!("opaque original {level}")));
        }
        h.close_clean();
    }
}
