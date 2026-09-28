use super::*;
mod groups;
mod media;
#[path = "workspace_restore.rs"]
mod restore;
#[path = "m36_verify.rs"]
mod verify;

fn begin(h: &Harness, p: &str, view: Value) -> Value {
    let result = h.work(json!({"kind":"begin_template_draft","project":p,"view":view}));
    assert_eq!(result["kind"], "template_draft", "{result}");
    result["status"].clone()
}
fn content(h: &Harness, p: &str, s: &Value) -> Value {
    let mut offset = Value::String("0".into());
    let mut text = String::new();
    loop {
        let r=h.work(json!({"kind":"template_draft_content","project":p,"session":s["owner"],"snapshot":s["snapshot"],"offset":offset}));
        assert_eq!(r["kind"], "template_draft_content", "{r}");
        let c = &r["content"];
        text.push_str(c["text"].as_str().unwrap());
        offset = c["next"].clone();
        if offset.is_null() {
            break;
        }
    }
    serde_json::from_str(&text).unwrap()
}
fn submit(h: &Harness, p: &str, s: &Value, g: &str, body: &Value, action: &str) -> Value {
    let input = json!({"kind":"template_draft","project":p,"session":s["owner"],"generation":g,"body":body,"action":action});
    let r = if action == "deposit" {
        h.control(input)
    } else {
        h.work(input)
    };
    assert_eq!(r["kind"], "template_draft", "{r}");
    r["status"].clone()
}
fn release(h: &Harness, p: &str, s: &Value, discard: bool) -> Value {
    h.control(json!({"kind":"release_template_draft","project":p,"session":s["owner"],"generation":s["generation"],"body":null,"discard":discard}))
}
fn number() -> Value {
    json!({"id":"new:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","label":"수치","configuration":{"kind":"number"},"required":false,"presentation":{"intent":"keep"},"default":{"intent":"set","value":{"kind":"number","value":"7"}},"archived":false})
}

#[test]
fn workspace_existing_committed_save_release_and_bound_view_lifetime() {
    let h = Harness::new();
    let p = h.open();
    let (id, _) = h.template(&p);
    let view = h.read_template(&p, &id);
    let s = begin(&h, &p, view["view"].clone());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "명시적 저장 한 번".into();
    body["fields"] = json!([number()]);
    let saved = submit(&h, &p, &s, "147", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    assert_eq!(saved["outcome"]["disk"], "committed");
    let path = h.root.join(format!("templates/{id}.json"));
    let bytes = fs::read(&path).unwrap();
    // UI의 읽기 갱신과 초안 session은 서로 다른 view 수명을 가진다.
    let refreshed = h.read_template(&p, &id);
    let refusal = h.ipc(json!({"action":"release_view","project":p,"view":view["view"]}));
    assert!(
        refusal.is_err(),
        "초안이 보유한 원본 view를 조기 해제하면 안 된다"
    );
    let released = h.control(json!({"kind":"release_template_draft","project":p,"session":s["owner"],"generation":"147","body":body,"discard":false}));
    assert_eq!(released["kind"], "control", "{released}");
    assert!(released["error"].is_null(), "{released}");
    h.call(json!({"action":"release_view","project":p,"view":view["view"]}));
    let again = begin(&h, &p, refreshed["view"].clone());
    assert_eq!(content(&h, &p, &again)["base"]["name"], "명시적 저장 한 번");
    assert!(release(&h, &p, &again, false)["error"].is_null());
    assert_eq!(fs::read(path).unwrap(), bytes);
    h.close_clean();
}

#[test]
fn workspace_create_one_candidate_update_once_noop_and_fixed_new_identity() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    assert_eq!(
        fs::read_dir(h.root.join("templates"))
            .map(|d| d.count())
            .unwrap_or(0),
        0
    );
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "전체 생성".into();
    body["fields"] = json!([number()]);
    let saved = submit(&h, &p, &s, "7", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    assert_eq!(saved["baseRevision"], "1");
    assert_eq!(saved["savedGeneration"], "7");
    let id = saved["artifact"].as_str().unwrap();
    let path = h.root.join(format!("templates/{id}.json"));
    let first: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let field = saved["identities"][number()["id"].as_str().unwrap()]
        .as_str()
        .unwrap();
    assert_eq!(first["fields"][field]["initialDefaultValue"]["value"], "7");
    body["name"] = "이름과 수치 동시 변경".into();
    body["fields"][0]["default"]["value"]["value"] = "8".into();
    let updated = submit(&h, &p, &saved, "8", &body, "save");
    assert_eq!(updated["baseRevision"], "2", "{updated}");
    let second = fs::read(&path).unwrap();
    let document: Value = serde_json::from_slice(&second).unwrap();
    assert_eq!(
        document["fields"][field]["initialDefaultValue"]["value"],
        "7"
    );
    assert_eq!(document["fields"][field]["introducedRevision"], 1);
    let no_write = submit(&h, &p, &updated, "9", &body, "save");
    assert_eq!(no_write["outcome"]["disk"], "no_write");
    assert_eq!(second, fs::read(&path).unwrap());
    assert!(release(&h, &p, &no_write, false)["error"].is_null());
    h.close_clean();
}

#[test]
fn workspace_invalid_normal_deposit_exact_generation_retry_and_native_close() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "PRIVATE - . 원문".into();
    body["fields"] = json!([number()]);
    body["fields"][0]["default"]["value"]["value"] = "-".into();
    let failed = submit(&h, &p, &s, "7", &body, "save");
    assert!(!failed["error"].is_null());
    assert_eq!(failed["problems"][0]["property"], "default");
    assert_eq!(
        failed["problems"][0]["field"],
        failed["identities"][number()["id"].as_str().unwrap()]
    );
    assert!(!h
        .root
        .join(format!(
            "templates/{}.json",
            s["artifact"].as_str().unwrap()
        ))
        .exists());
    let store = h.state.recovery.connect().unwrap();
    store.lock().unwrap().fault = Some(crate::data::edit_recovery::error::Stage::Reopen);
    let failed_deposit = submit(&h, &p, &s, "7", &body, "deposit");
    assert!(failed_deposit["receipt"].is_null());
    h.call(json!({"action":"app_shutdown"}));
    assert!(!h.state.lock().closed);
    store.lock().unwrap().fault = None;
    let deposited = submit(&h, &p, &s, "7", &body, "deposit");
    assert_eq!(deposited["receipt"]["key"]["generation"], "7");
    let repeated = submit(&h, &p, &s, "7", &body, "deposit");
    assert_eq!(deposited["receipt"], repeated["receipt"]);
    let mut newer = body.clone();
    newer["fields"][0]["default"]["value"]["value"] = ".".into();
    let release_new=h.control(json!({"kind":"release_template_draft","project":p,"session":s["owner"],"generation":"8","body":newer,"discard":false}));
    assert_eq!(release_new["error"]["code"], "no_receipt");
    let later = submit(&h, &p, &s, "8", &newer, "deposit");
    assert_eq!(later["receipt"]["key"]["generation"], "8");
    let rows = store.lock().unwrap().list().unwrap();
    assert_eq!(rows.entries.len(), 2);
    assert!(release(&h, &p, &later, false)["error"].is_null());
    drop(store);
    h.close_clean();
}

#[test]
fn workspace_center_without_project_read_restore_conflict_reapply_and_discard() {
    let h = Harness::new();
    let empty = h.work(json!({"kind":"recovery_page","cursor":null}));
    assert_eq!(empty["kind"], "recovery_page");
    let p = h.open();
    let (id, _) = h.template(&p);
    let view = h.read_template(&p, &id);
    let s = begin(&h, &p, view["view"].clone());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "복원할 이름".into();
    let deposited = submit(&h, &p, &s, "2", &body, "deposit");
    assert!(release(&h, &p, &deposited, false)["error"].is_null());
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &page["page"]["entries"][0];
    let r = &row["row"];
    let read=h.work(json!({"kind":"recovery_read","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"]}));
    assert_eq!(read["kind"], "recovery_selection");
    let before = fs::read(h.root.join(format!("templates/{id}.json"))).unwrap();
    let restored=h.work(json!({"kind":"recovery_restore","project":p,"snapshot":read["selection"]["snapshot"],"reapply":null}));
    assert_eq!(restored["kind"], "template_draft", "{restored}");
    let restored = &restored["status"];
    assert_ne!(restored["owner"], s["owner"]);
    assert_eq!(restored["draftId"], s["draftId"]);
    assert_eq!(content(&h, &p, restored)["body"], body);
    assert_eq!(
        before,
        fs::read(h.root.join(format!("templates/{id}.json"))).unwrap()
    );
    let blocked =
        h.control(json!({"kind":"recovery_discard","key":r["key"],"version":row["version"]}));
    assert_eq!(blocked["error"]["code"], "owners_remain");
    assert!(release(&h, &p, restored, true)["error"].is_null());
    // revision과 논리적 값이 같아도 외부 bytes 변경은 자동 덮어쓰기를 허용하지 않는다.
    let path = h.root.join(format!("templates/{id}.json"));
    let mut external = before.clone();
    external.extend_from_slice(b"\n ");
    fs::write(&path, &external).unwrap();
    let conflict=h.work(json!({"kind":"recovery_restore","project":p,"snapshot":read["selection"]["snapshot"],"reapply":null}));
    assert_eq!(conflict["selection"]["phase"], "conflict", "{conflict}");
    assert_eq!(external, fs::read(&path).unwrap());
    let applied=h.work(json!({"kind":"recovery_restore","project":p,"snapshot":conflict["selection"]["snapshot"],"reapply":[{"kind":"name"}]}));
    assert_eq!(applied["kind"], "template_draft", "{applied}");
    assert_eq!(external, fs::read(&path).unwrap());
    let s = &applied["status"];
    assert_eq!(content(&h, &p, s)["body"]["name"], "복원할 이름");
    assert!(release(&h, &p, s, true)["error"].is_null());
    assert_eq!(
        h.control(json!({"kind":"recovery_discard","key":r["key"],"version":row["version"]}))
            ["kind"],
        "control"
    );
    assert_eq!(external, fs::read(&path).unwrap());
    h.close_clean();
}

#[test]
fn workspace_stale_save_and_source_snapshot_chunk_binding() {
    let h = Harness::new();
    let p = h.open();
    let (id, _) = h.template(&p);
    let view = h.read_template(&p, &id);
    let s = begin(&h, &p, view["view"].clone());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "가".repeat(40000).into();
    let deposited = submit(&h, &p, &s, "2", &body, "deposit");
    let part=h.work(json!({"kind":"template_draft_content","project":p,"session":s["owner"],"snapshot":deposited["snapshot"],"offset":"0"}));
    assert!(part["content"]["text"].as_str().unwrap().len() <= 65536);
    assert!(!part["content"]["next"].is_null());
    let old=h.work(json!({"kind":"template_draft_content","project":p,"session":s["owner"],"snapshot":s["snapshot"],"offset":"0"}));
    assert_eq!(old["error"]["code"], "wrong_binding");
    let path = h.root.join(format!("templates/{id}.json"));
    let mut external = fs::read(&path).unwrap();
    external.push(b' ');
    fs::write(&path, &external).unwrap();
    let failed = submit(&h, &p, &s, "3", &body, "save");
    assert_eq!(failed["phase"], "conflict", "{failed}");
    assert_eq!(external, fs::read(&path).unwrap());
    assert!(release(&h, &p, &failed, true)["error"].is_null());
    h.close_clean();
}

#[test]
fn workspace_restart_restores_raw_new_owner_then_explicitly_saves_once() {
    let mut first = Harness::new();
    let p = first.open();
    let s = begin(&first, &p, Value::Null);
    let mut body = content(&first, &p, &s)["body"].clone();
    body["name"] = "재시작 전체 초안".into();
    body["fields"] = json!([number()]);
    body["fields"][0]["default"]["value"]["value"] = "-".into();
    let deposited = submit(&first, &p, &s, "7", &body, "deposit");
    let identity = deposited["identities"].clone();
    assert_eq!(release(&first, &p, &deposited, false)["kind"], "control");
    first.close_clean();
    first.remove_on_drop = false;
    let base = first.base.clone();
    drop(first);
    // AppState/worker/store/controller 권한은 모두 새로 생성한다. OS GUI 재시작 시험은 아니다.
    let second = Harness::at(base, backend::provider(), true);
    let page = second.work(json!({"kind":"recovery_page","cursor":null}));
    let r = &page["page"]["entries"][0]["row"];
    let selected = second.work(json!({"kind":"recovery_read","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"]}));
    let p2 = second.open();
    assert_ne!(p, p2);
    let restored = second.work(json!({"kind":"recovery_restore","project":p2,"snapshot":selected["selection"]["snapshot"],"reapply":null}));
    assert_eq!(restored["kind"], "template_draft", "{restored}");
    let s2 = &restored["status"];
    assert_ne!(s["owner"], s2["owner"]);
    assert_eq!(content(&second, &p2, s2)["body"], body);
    assert_eq!(s2["identities"], identity);
    let path = second.root.join(format!(
        "templates/{}.json",
        s2["artifact"].as_str().unwrap()
    ));
    assert!(!path.exists());
    // 복원 자체는 무효 원문을 보존한다. 수정 후 최초 canonical 공개만 한 번 발생한다.
    body["fields"][0]["default"]["value"]["value"] = "20".into();
    let saved = submit(&second, &p2, s2, "9", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    assert_eq!(saved["baseRevision"], "1");
    assert!(path.exists());
    assert_eq!(
        second
            .state
            .recovery
            .connect()
            .unwrap()
            .lock()
            .unwrap()
            .list()
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_eq!(release(&second, &p2, &saved, false)["kind"], "control");
    second.close_clean();
}

#[test]
fn workspace_lock_lost_old_pending_payload_is_deposited_before_new_generation() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (id, _) = h.template(&p);
    let view = h.read_template(&p, &id);
    let s = begin(&h, &p, view["view"].clone());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "세대 7".into();
    let before = fs::read(h.root.join(format!("templates/{id}.json"))).unwrap();
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = submit(&h, &p, &s, "7", &body, "save");
    assert!(!failed["error"].is_null());
    body["name"] = "세대 8 원문".into();
    let deposited = submit(&h, &p, &s, "8", &body, "deposit");
    assert_eq!(
        deposited["receipt"]["key"]["generation"], "8",
        "{deposited}"
    );
    let store = h.state.recovery.connect().unwrap();
    let rows = store.lock().unwrap().list().unwrap();
    assert_eq!(rows.entries.len(), 2);
    let generations: std::collections::BTreeSet<_> = rows
        .entries
        .iter()
        .map(|e| e.key.as_ref().unwrap().generation)
        .collect();
    assert_eq!(generations, [7, 8].into_iter().collect());
    assert_eq!(
        before,
        fs::read(h.root.join(format!("templates/{id}.json"))).unwrap()
    );
    assert_eq!(release(&h, &p, &deposited, false)["kind"], "control");
    drop(store);
    h.close_clean();
}

#[test]
fn workspace_unpublished_deposit_has_no_acquired_edit_session_or_empty_file() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    assert!(provider.threads.lock().unwrap().is_empty());
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "미공개".into();
    let deposited = submit(&h, &p, &s, "2", &body, "deposit");
    assert!(!deposited["receipt"].is_null(), "{deposited}");
    assert!(provider.threads.lock().unwrap().is_empty());
    assert!(!h
        .root
        .join(format!(
            "templates/{}.json",
            s["artifact"].as_str().unwrap()
        ))
        .exists());
    assert_eq!(release(&h, &p, &deposited, false)["kind"], "control");
    h.close_clean();
}

#[test]
fn m36_guarded_rich_unknown_bounds_sections_deposit_restore_then_save_close() {
    let base = std::env::temp_dir().join(format!("worldbuild-m36-{}", uuid::Uuid::new_v4()));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "M3-6".into();
    let mut numeric = number();
    numeric["default"] = json!({"intent":"unset"});
    numeric["configuration"] = json!({"kind":"number","minimum":"-1","maximum":"1"});
    let mut rich = numeric.clone();
    rich["id"] = "new:cccccccc-cccc-4ccc-8ccc-cccccccccccc".into();
    rich["label"] = "본문".into();
    rich["configuration"] = json!({"kind":"rich_text"});
    body["fields"] = json!([rich, numeric]);
    body["sections"] = json!([{"id":"dddddddd-dddd-4ddd-8ddd-dddddddddddd","title":"상황","beforeField":"new:cccccccc-cccc-4ccc-8ccc-cccccccccccc"}]);
    let saved = submit(&h, &p, &s, "2", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    let t = saved["artifact"].as_str().unwrap();
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let template = h.read_template(&p, t);
    let fields = template["content"]["fieldOrder"].as_array().unwrap();
    let rich_id = fields[0].clone();
    let number_id = fields[1].clone();
    let request = |r: Value| h.work(json!({"kind":"document_workspace","project":p,"request":r}));
    let draft = request(json!({"action":"begin","template":t}))["value"].clone();
    let ast = json!({"kind":"root","children":[{"kind":"heading","level":2,"children":[{"kind":"text","text":"상황","marks":["bold"]}]},{"kind":"taskList","children":[{"kind":"taskItem","checked":true,"children":[{"kind":"paragraph","children":[{"kind":"text","text":"완료"}]}]}]}]});
    let fields = json!([{"field":rich_id,"value":{"intent":"set","value":{"kind":"rich_text","content":ast}}},{"field":number_id,"value":{"intent":"set","value":{"kind":"number_unknown","previous_raw":"-"}}}]);
    let create = json!({"name":"새 문서","parent":null,"fields":fields,"composing":false});
    let saved = request(
        json!({"action":"draft","owner":draft["owner"],"generation":"2","body":create,"save":true}),
    )["value"]
        .clone();
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let id = saved["outcome"]["artifact"].clone();
    assert_eq!(
        request(
            json!({"action":"release","owner":draft["owner"],"generation":"2","discard":false})
        )["value"]["kind"],
        "released"
    );
    let d = request(json!({"action":"edit_begin","document":id}))["value"].clone();
    assert!(d["editable"].as_array().unwrap().contains(&rich_id));
    let mut raw = d["body"].clone();
    raw["fields"] = fields;
    raw["fields"][1]["value"]["value"] = json!({"kind":"number","value":"2"});
    let before = fs::read(
        h.root
            .join(format!("documents/{}.json", id.as_str().unwrap())),
    )
    .unwrap();
    let rejected = request(
        json!({"action":"edit_draft","owner":d["owner"],"generation":"3","body":raw,"save":true}),
    )["value"]
        .clone();
    assert!(!rejected["problem"].is_null(), "{rejected}");
    assert_eq!(
        fs::read(
            h.root
                .join(format!("documents/{}.json", id.as_str().unwrap()))
        )
        .unwrap(),
        before
    );
    raw["fields"][1]["value"]["value"]["value"] = "-".into();
    let dep =
        request(json!({"action":"edit_deposit","owner":d["owner"],"generation":"4","body":raw}));
    assert_eq!(dep["value"]["deposited"], true, "{dep}");
    assert_eq!(
        request(json!({"action":"edit_release","owner":d["owner"],"generation":"4"}))["value"]
            ["kind"],
        "released"
    );
    // 새 AST와 invalid raw를 같은 Store에 남긴 뒤 AppState/worker를 종료하고 다시 연다.
    h.close_clean();
    drop(h);
    let h = Harness::at(base, backend::provider(), true);
    let p = h.open();
    let request = |r: Value| h.work(json!({"kind":"document_workspace","project":p,"request":r}));
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &rows["page"]["entries"][0]["row"];
    let restored=request(json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}))["value"].clone();
    let admitted: crate::commands::document_workspace::EditBody =
        serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(restored["body"], serde_json::to_value(admitted).unwrap());
    assert_eq!(
        fs::read(
            h.root
                .join(format!("documents/{}.json", id.as_str().unwrap()))
        )
        .unwrap(),
        before
    );
    raw["fields"][1]["value"]["value"] = json!({"kind":"number","value":"1"});
    let next = (restored["generation"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        + 1)
    .to_string();
    let fixed=request(json!({"action":"edit_draft","owner":restored["owner"],"generation":next,"body":raw,"save":true}))["value"].clone();
    assert_eq!(fixed["outcome"]["disk"], "committed", "{fixed}");
    let persisted: Value = serde_json::from_slice(
        &fs::read(
            h.root
                .join(format!("documents/{}.json", id.as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        persisted["fieldValues"][rich_id.as_str().unwrap()]["document"]["content"],
        ast
    );
    assert_eq!(
        persisted["fieldValues"][number_id.as_str().unwrap()]["value"],
        "1"
    );
    assert_eq!(
        request(json!({"action":"edit_release","owner":restored["owner"],"generation":next}))
            ["value"]["kind"],
        "released"
    );
    h.close_clean();
}
