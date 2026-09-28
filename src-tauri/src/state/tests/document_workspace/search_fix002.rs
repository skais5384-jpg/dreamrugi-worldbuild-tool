use super::*;

#[test]
#[ignore = "explicit owned FIX-002 native GUI fixture"]
fn m41_fix002_gui_fixture() {
    let base = PathBuf::from(std::env::var_os("M41_FIX002_GUI_BASE").expect("owned GUI base"));
    assert!(
        !base.join("project").exists(),
        "never replace an existing fixture"
    );
    let h = Harness::at(base, backend::provider(), false);
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "FIX002 A 편집 보존");
    let b = create(&h, &p, &t, "FIX002 B 삭제 대상");
    let c = create(&h, &p, &t, "FIX002 C 정상 결과");
    println!("FIX002_GUI {}", json!({"a":a,"b":b,"c":c,"template":t}));
    h.close_clean();
}

fn search(h: &Harness, project: &str, refresh: bool) -> Value {
    request(
        h,
        project,
        json!({"action":"search","query":"","template":null,
        "offset":0,"limit":100,"refresh":refresh}),
    )
}

#[test]
fn m41_fix002_running_search_cancel_reaches_checkpoint_before_queued_save_and_close() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "A");
    let e = edit_begin(&h, &p, &a);
    let op = h.reserve("ordinary");
    let operation_id: Id = serde_json::from_value(json!(op)).unwrap();
    let (entered, release_gate) = h.state.lock().operations[&operation_id]
        .progress
        .hold_checkpoint();
    h.call(json!({"action":"submit","operation":op,"input":{"kind":"document_workspace","project":p,"request":{"action":"search","query":"A","template":null,"offset":0,"limit":100,"refresh":true}}}));
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"document_progress","operation":op,"cancel":true}))["requested"],
        true
    );
    let mut body = e["body"].clone();
    body["name"] = json!({"intent":"set","value":"saved after cancel"});
    let save_op = h.submit(json!({"kind":"document_workspace","project":p,"request":{"action":"edit_draft","owner":e["owner"],"generation":"2","body":body,"save":true}}));
    assert_eq!(
        h.call(json!({"action":"operation","operation":save_op}))["state"],
        "pending"
    );
    release_gate.send(()).unwrap();
    let cancelled = h.result(&op);
    assert_eq!(cancelled["error"]["code"], "cancelled", "{cancelled}");
    assert_eq!(h.result(&op), cancelled);
    h.ack(&op);
    let saved = h.result(&save_op)["value"].clone();
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    h.ack(&save_op);
    edit_release(&h, &p, &saved);
    assert_eq!(
        search(&h, &p, false)["value"]["results"][0]["name"],
        "saved after cancel"
    );
    h.close_clean();
}

#[test]
fn m41_fix002_cancelled_reservation_cannot_start_a_cold_search() {
    let h = Harness::new();
    let p = h.open();
    let op = h.reserve("ordinary");
    assert_eq!(
        h.call(json!({"action":"document_progress","operation":op,"cancel":true}))["requested"],
        true
    );
    h.call(json!({"action":"submit","operation":op,"input":{"kind":"document_workspace","project":p,"request":{"action":"search","query":"A","template":null,"offset":0,"limit":100,"refresh":true}}}));
    assert_eq!(h.result(&op)["error"]["code"], "cancelled");
    h.ack(&op);
    h.close_clean();
}

#[test]
fn m42_fix001_running_references_cancel_is_terminal_and_worker_remains_usable() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let target = create(&h, &p, &t, "reference target");
    let op = h.reserve("ordinary");
    let operation_id: Id = serde_json::from_value(json!(op)).unwrap();
    let (entered, release_gate) = h.state.lock().operations[&operation_id]
        .progress
        .hold_checkpoint();
    h.call(json!({
        "action":"submit",
        "operation":op,
        "input":{
            "kind":"document_workspace",
            "project":p,
            "request":{"action":"references","document":target}
        }
    }));
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"operation","operation":op}))["state"],
        "pending"
    );
    assert_eq!(
        h.call(json!({"action":"document_progress","operation":op,"cancel":true}))["requested"],
        true
    );
    release_gate.send(()).unwrap();
    let cancelled = h.result(&op);
    assert_eq!(cancelled["error"]["code"], "cancelled", "{cancelled}");
    assert_eq!(h.result(&op), cancelled, "terminal result must be stable");
    h.ack(&op);
    let created = create(&h, &p, &t, "write after cancelled references");
    assert!(!created.is_empty());
    let follow_up = list(&h, &p);
    assert_eq!(follow_up["documents"].as_array().unwrap().len(), 2);
    h.close_clean();
}

#[test]
fn m41_fix002_running_search_yields_to_ownerless_project_close() {
    let h = Harness::new();
    let p = h.open();
    let op = h.reserve("ordinary");
    let id: Id = serde_json::from_value(json!(op)).unwrap();
    let (entered, release) = h.state.lock().operations[&id].progress.hold_checkpoint();
    h.call(json!({"action":"submit","operation":op,"input":{"kind":"document_workspace","project":p,"request":{"action":"search","query":"A","template":null,"offset":0,"limit":100,"refresh":true}}}));
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"document_progress","operation":op,"cancel":true}))["requested"],
        true
    );
    let close = h.submit(json!({"kind":"close","project":p}));
    // close 수락은 제어 신호의 terminal이며 worker 종료 완료와 다르다.
    assert_eq!(h.result(&close)["kind"], "control");
    release.send(()).unwrap();
    assert_eq!(h.result(&op)["error"]["code"], "cancelled");
    h.ack(&op);
    let closed = h.result(&close);
    assert_ne!(closed["kind"], "rejected", "{closed}");
    h.ack(&close);
    h.close_clean();
}

#[test]
fn m41_fix002_template_source_change_rebuilds_all_dependent_entries() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "A");
    create(&h, &p, &t, "B");
    assert_eq!(search(&h, &p, true)["value"]["total"], 2);
    let path = h.root.join(format!("templates/{t}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["name"] = "외부 정의".into();
    fs::write(path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let e = edit_begin(&h, &p, &a);
    let mut body = e["body"].clone();
    body["name"] = json!({"intent":"set","value":"A saved"});
    let saved = edit_save(&h, &p, &e, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let result = search(&h, &p, false);
    assert_eq!(result["value"]["templateNames"][&t], "외부 정의");
    for row in result["value"]["results"].as_array().unwrap() {
        assert_eq!(row["templateName"], "외부 정의", "{result}");
    }
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m41_fix002_missing_document_search_refresh_preserves_strict_writes() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "A");
    let b = create(&h, &p, &t, "B");
    let before = list(&h, &p);
    let layout_path = h.root.join("workspace/document-layout.json");
    let layout = fs::read(&layout_path).unwrap();
    let path = h.root.join(format!("documents/{b}.json"));
    let bytes = fs::read(&path).unwrap();
    assert_eq!(search(&h, &p, true)["value"]["total"], 2);
    fs::remove_file(&path).unwrap();
    assert_eq!(
        request(&h, &p, json!({"action":"search_read","document":b}))["value"]["kind"],
        "search_unavailable"
    );
    let result = search(&h, &p, true);
    assert_eq!(result["value"]["total"], 1, "{result}");
    assert_eq!(result["value"]["missingDocuments"], 1, "{result}");
    assert_eq!(result["value"]["results"][0]["id"], a);
    assert_eq!(search(&h, &p, false)["value"]["total"], 1);
    let rejected = request(
        &h,
        &p,
        json!({"action":"mutate","snapshot":before["snapshot"],"edit":{"kind":"trash","document":a}}),
    );
    assert_eq!(rejected["kind"], "write", "{rejected}");
    assert_eq!(rejected["disk"], "not_applied", "{rejected}");
    assert_eq!(rejected["diagnostic"]["category"], "ArtifactRejected");
    assert_eq!(fs::read(&layout_path).unwrap(), layout);
    fs::write(&path, &bytes).unwrap();
    assert_eq!(search(&h, &p, true)["value"]["total"], 2);
    let template_path = h.root.join(format!("templates/{t}.json"));
    let template_bytes = fs::read(&template_path).unwrap();
    fs::remove_file(&template_path).unwrap();
    let missing_template = search(&h, &p, true);
    assert_eq!(missing_template["value"]["total"], 0, "{missing_template}");
    assert_eq!(
        missing_template["value"]["missingDocuments"], 2,
        "{missing_template}"
    );
    fs::write(&template_path, template_bytes).unwrap();
    fs::write(&path, b"broken JSON").unwrap();
    assert_eq!(search(&h, &p, true)["kind"], "rejected");
    fs::write(&path, &bytes).unwrap();
    h.close_clean();
}

#[test]
fn m41_fix002_choice_and_repeated_child_definition_changes_do_not_mix() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut template: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let choice = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
    let group = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
    let child = "aaaaaaaa-aaaa-4aaa-8aaa-000000000003";
    let option = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
    let instance = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
    let definition = |kind: &str| json!({"label":"옛 이름","kind":kind,"required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":kind},"presentation":{}});
    template["fieldOrder"] = json!([choice, group]);
    template["fields"][choice] = definition("singleChoice");
    template["fields"][choice]["configuration"] = json!({"kind":"singleChoice","optionOrder":[option],"options":{(option):{"label":"옛 선택","lifecycle":"active"}}});
    template["fields"][group] = definition("group");
    template["fields"][group]["configuration"] =
        json!({"kind":"group","memberOrder":[child],"members":{(child):definition("richText")}});
    artifact::decode_template(&serde_json::to_vec(&template).unwrap()).unwrap();
    fs::write(&tp, serde_json::to_vec(&template).unwrap()).unwrap();
    let mut documents = Vec::new();
    for name in ["A", "B"] {
        let d = begin(&h, &p, &t);
        let mut body = d["body"].clone();
        body["name"] = name.into();
        body["fields"] = json!([
            {"field":choice,"value":{"intent":"set","value":{"kind":"single_choice","option":option}}},
            {"field":group,"value":{"intent":"set","value":{"kind":"group","instances":[{"id":instance,"source":null,"fields":[{"field":child,"value":{"intent":"set","value":{"kind":"rich_text","content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":"반복 본문","marks":[]}]}]}}}}]}]}}}
        ]);
        let saved = save(&h, &p, &d, "2", body);
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        documents.push(saved["outcome"]["artifact"].as_str().unwrap().to_owned());
        release(&h, &p, &saved, false);
    }
    assert_eq!(search(&h, &p, true)["value"]["total"], 2);
    template["fields"][choice]["configuration"]["options"][option]["label"] = "새 선택".into();
    template["fields"][group]["configuration"]["members"][child]["label"] = "새 자식".into();
    fs::write(&tp, serde_json::to_vec(&template).unwrap()).unwrap();
    let e = edit_begin(&h, &p, &documents[0]);
    let mut body = e["body"].clone();
    body["name"] = json!({"intent":"set","value":"A saved"});
    let saved = edit_save(&h, &p, &e, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    for (query, total) in [("새 선택", 2), ("옛 선택", 0), ("반복 본문", 2)] {
        let r = request(
            &h,
            &p,
            json!({"action":"search","query":query,"template":t,"offset":0,"limit":100,"refresh":false}),
        );
        assert_eq!(r["value"]["total"], total, "{r}");
        if query == "반복 본문" {
            for row in r["value"]["results"].as_array().unwrap() {
                assert!(
                    row["excerpt"]["label"]
                        .as_str()
                        .unwrap()
                        .contains("새 자식"),
                    "{r}"
                );
            }
        }
    }
    edit_release(&h, &p, &saved);
    // 정의의 활성 경계도 같은 source 증거로 재구축된다.
    template["fields"][group]["configuration"]["members"][child]["lifecycle"] = "archived".into();
    template["fields"][group]["configuration"]["memberOrder"] = json!([]);
    artifact::decode_template(&serde_json::to_vec(&template).unwrap()).unwrap();
    fs::write(&tp, serde_json::to_vec(&template).unwrap()).unwrap();
    let e = edit_begin(&h, &p, &documents[0]);
    let mut body = e["body"].clone();
    body["name"] = json!({"intent":"set","value":"A final"});
    let saved = edit_save(&h, &p, &e, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let r = request(
        &h,
        &p,
        json!({"action":"search","query":"반복 본문","template":null,"offset":0,"limit":100,"refresh":false}),
    );
    assert_eq!(r["value"]["total"], 0, "{r}");
    edit_release(&h, &p, &saved);
    h.close_clean();
}
