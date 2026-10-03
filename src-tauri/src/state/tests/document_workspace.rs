use super::*;
use crate::data::edit_recovery::model::Key;
mod archive_recovery;
mod fix;
mod groups;
mod replace_m44;
mod scale;
mod search_fix002;

#[test]
fn document_edit_duplicate_raw_field_is_visible_failure_and_keeps_file() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let id = create(&h, &p, &t, "base");
    let d = edit_begin(&h, &p, &id);
    let path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&path).unwrap();
    let mut body = d["body"].clone();
    body["fields"] = json!([{"field":"duplicate","value":{"intent":"keep"}},{"field":"duplicate","value":{"intent":"keep"}}]);
    let rejected = edit_save(&h, &p, &d, "2", body.clone());
    assert_eq!(rejected["problem"], "InvalidValue");
    assert_eq!(rejected["body"], body);
    assert_eq!(fs::read(path).unwrap(), before);
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"2","body":body}),
    );
    assert_eq!(dep["value"]["deposited"], true);
    edit_release(&h, &p, &dep["value"]);
    h.close_clean();
}

#[test]
fn document_edit_creation_followup_restores_confirmed_id_and_lost_response() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "created".into();
    let saved = save(&h, &p, &d, "2", body.clone());
    let id = saved["outcome"]["artifact"].clone();
    body["name"] = "later raw".into();
    body["composing"] = true.into();
    let response = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"3","body":body}),
    );
    assert_eq!(response["value"]["kind"], "draft", "{response}");
    let dep = response["value"].clone();
    assert_eq!(dep["deposited"], true, "{dep}");
    release(&h, &p, &dep, false);
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &rows["page"]["entries"][0]["row"];
    let restore = json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]});
    // 응답을 잃어도 같은 operation을 다시 조회하며 새 owner를 만들지 않는다.
    let op = h.submit(json!({"kind":"document_workspace","project":p,"request":restore}));
    let first = h.result(&op);
    let second = h.result(&op);
    assert_eq!(first, second);
    h.ack(&op);
    let restored = &second["value"];
    assert_eq!(restored["document"], id, "{second}");
    assert_eq!(restored["body"]["composing"], false);
    let saved = edit_save(&h, &p, restored, "4", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}

#[test]
fn document_edit_failed_proof_keeps_owner_then_lock_retry_acquires_new_session() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (t, _) = h.template(&p);
    let id = create(&h, &p, &t, "base");
    let d = edit_begin(&h, &p, &id);
    let mut body = d["body"].clone();
    body["name"] = json!({"intent":"set","value":"retained"});
    provider
        .lose
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = edit_save(&h, &p, &d, "2", body.clone());
    assert!(!failed["problem"].is_null(), "{failed}");
    let store = h.state.recovery.connect().unwrap();
    store.lock().unwrap().fault = Some(crate::data::edit_recovery::error::Stage::Reopen);
    let deposit = json!({"action":"edit_deposit","owner":d["owner"],"generation":"3","body":body});
    let rejected = request(&h, &p, deposit.clone());
    assert!(!rejected["error"].is_null(), "{rejected}");
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":"3"})
        )["error"]["code"],
        "no_receipt"
    );
    store.lock().unwrap().fault = None;
    let kept = request(&h, &p, deposit);
    assert_eq!(kept["value"]["deposited"], true, "{kept}");
    provider
        .lose
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let retry = request(
        &h,
        &p,
        json!({"action":"edit_retry","owner":d["owner"],"generation":"3","body":body}),
    );
    assert_eq!(retry["value"]["generation"], "4", "{retry}");
    let saved = edit_save(&h, &p, &retry["value"], "4", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn document_edit_admitted_and_composite_recovery_preserve_both_intents() {
    for (composite, unchanged) in [(false, false), (true, false), (true, true)] {
        let provider = Arc::new(Provider::new());
        let h = Harness::with_provider(provider.clone());
        let p = h.open();
        let (t, _) = h.template(&p);
        let id = create(&h, &p, &t, "base");
        let tv = h.read_template(&p, &t);
        let dv = h.work(json!({"kind":"read_document","project":p,"document":id}));
        let session = h.session(
            &p,
            vec![dv["view"].clone(), tv["view"].clone()],
            if composite { "composite" } else { "document" },
        );
        provider
            .lose
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let mut input = json!({"kind":"save_document","project":p,"session":session,"document":dv["view"],"template":tv["view"],"revision":"1","edits":[{"kind":"rename","name":"recovered"}]});
        if composite {
            input["kind"] = "save_composite".into();
            let wire: Value = serde_json::from_slice(
                &fs::read(h.root.join(format!("templates/{t}.json"))).unwrap(),
            )
            .unwrap();
            input["edit"] = json!({"kind":"name","name":if unchanged {wire["name"].as_str().unwrap()} else {"pending template"}});
        }
        let failed = h.work(input);
        assert_eq!(failed["custody"], "Preserved");
        assert!(h.control(
            json!({"kind":"session_control","project":p,"session":session,"control":"accept"})
        )["error"]
            .is_null());
        assert!(h.control(json!({"kind":"session_control","project":p,"session":session,"control":"acknowledge_recovery"}))["error"].is_null());
        provider
            .lose
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
        let row = &rows["page"]["entries"][0]["row"];
        let restored = request(
            &h,
            &p,
            json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}),
        );
        if composite && !unchanged {
            assert_eq!(
                restored["error"]["code"], "composite_intent_pending",
                "{restored}"
            );
        } else {
            let saved = edit_save(
                &h,
                &p,
                &restored["value"],
                "2",
                restored["value"]["body"].clone(),
            );
            assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
            edit_release(&h, &p, &saved);
        }
        assert_eq!(
            h.state
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
        h.close_clean();
    }
}

fn edit_begin(h: &Harness, p: &str, id: &str) -> Value {
    let r = request(h, p, json!({"action":"edit_begin","document":id}));
    assert_eq!(r["value"]["kind"], "editing", "{r}");
    r["value"].clone()
}
fn edit_save(h: &Harness, p: &str, d: &Value, g: &str, body: Value) -> Value {
    let r = request(
        h,
        p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":g,"body":body,"save":true}),
    );
    assert_eq!(r["value"]["kind"], "editing", "{r}");
    r["value"].clone()
}
fn edit_release(h: &Harness, p: &str, d: &Value) {
    let r = request(
        h,
        p,
        json!({"action":"edit_release","owner":d["owner"],"generation":d["generation"]}),
    );
    assert_eq!(r["value"]["kind"], "released", "{r}");
}

#[test]
fn m39_fix003_edit_commit_refreshes_current_snapshot_for_next_create() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "before edit");
    let listed = list(&h, &p);

    let editing = edit_begin(&h, &p, &existing);
    let mut edit_body = editing["body"].clone();
    edit_body["name"] = json!({"intent":"set","value":"after edit"});
    let saved = edit_save(&h, &p, &editing, "2", edit_body.clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    // 유실 응답 재조회에 해당하는 같은 세대 재요청도 cache 연결을 중복 변경하지 않는다.
    let repeated = edit_save(&h, &p, &saved, "2", edit_body);
    assert_eq!(repeated["outcome"]["disk"], "committed", "{repeated}");
    edit_release(&h, &p, &repeated);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut create_body = draft["body"].clone();
    create_body["name"] = "created after edit".into();
    let created = save(&h, &p, &draft, "2", create_body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);

    let current = list(&h, &p);
    assert_eq!(current["documents"].as_array().unwrap().len(), 2);
    assert!(current["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|summary| summary["id"] == existing && summary["name"] == "after edit"));
    h.close_clean();
}

#[test]
fn m39_fix003_open_creation_draft_survives_edit_and_list_refresh() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "existing");
    let listed = list(&h, &p);
    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut create_body = draft["body"].clone();
    create_body["name"] = "raw creation body survives".into();

    let editing = edit_begin(&h, &p, &existing);
    let mut edit_body = editing["body"].clone();
    edit_body["name"] = json!({"intent":"set","value":"existing edited"});
    let edited = edit_save(&h, &p, &editing, "2", edit_body);
    assert_eq!(edited["outcome"]["disk"], "committed", "{edited}");
    edit_release(&h, &p, &edited);
    let refreshed = list(&h, &p);
    assert_eq!(refreshed["documents"].as_array().unwrap().len(), 1);

    let created = save(&h, &p, &draft, "2", create_body.clone());
    assert_eq!(created["body"], create_body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    h.close_clean();
}

#[test]
fn m39_fix003_edit_commit_refreshes_current_snapshot_for_tree_write() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "tree target");
    let listed = list(&h, &p);

    let editing = edit_begin(&h, &p, &existing);
    let mut body = editing["body"].clone();
    body["name"] = json!({"intent":"set","value":"tree target edited"});
    let edited = edit_save(&h, &p, &editing, "2", body);
    assert_eq!(edited["outcome"]["disk"], "committed", "{edited}");
    edit_release(&h, &p, &edited);

    let moved = request(
        &h,
        &p,
        json!({"action":"mutate","snapshot":listed["snapshot"],"edit":{"kind":"trash","document":existing}}),
    );
    assert_eq!(moved["disk"], "committed", "{moved}");
    h.close_clean();
}

#[test]
fn m39_fix003_external_change_between_list_and_edit_keeps_old_snapshot_stale() {
    let h = Harness::new();
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "before external change");
    let listed = list(&h, &p);
    let path = h.root.join(format!("documents/{existing}.json"));
    let mut external: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    external["name"] = "external source".into();
    fs::write(&path, serde_json::to_vec(&external).unwrap()).unwrap();

    let editing = edit_begin(&h, &p, &existing);
    let mut edit_body = editing["body"].clone();
    edit_body["name"] = json!({"intent":"set","value":"own edit after external"});
    let edited = edit_save(&h, &p, &editing, "2", edit_body);
    assert_eq!(edited["outcome"]["disk"], "committed", "{edited}");
    edit_release(&h, &p, &edited);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "must not cross external source".into();
    let rejected = save(&h, &p, &draft, "2", body);
    assert_ne!(rejected["outcome"]["disk"], "committed", "{rejected}");
    release(&h, &p, &rejected, true);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}

fn fail_next_commit_cleanup(h: &Harness) -> Arc<std::sync::atomic::AtomicBool> {
    use crate::data::transaction::test_support::{boundary::Point, CommitTestPoint};
    use std::sync::atomic::{AtomicBool, Ordering};
    let armed = Arc::new(AtomicBool::new(false));
    let gate = armed.clone();
    gates::observe_io(
        h.root.clone(),
        Box::new(move |point, _| {
            if point == Point::Commit(CommitTestPoint::Cleanup)
                && gate.swap(false, Ordering::SeqCst)
            {
                return Err(std::io::Error::other("m39 fix004 delayed refresh"));
            }
            Ok(())
        }),
    );
    armed
}

fn recover_delayed_edit(
    h: &Harness,
    project: &str,
    editing: &Value,
    armed: &std::sync::atomic::AtomicBool,
    name: &str,
) -> Value {
    use std::sync::atomic::Ordering;
    let mut body = editing["body"].clone();
    body["name"] = json!({"intent":"set","value":name});
    armed.store(true, Ordering::SeqCst);
    let saved = edit_save(h, project, editing, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(saved["problem"], "SavedReadRequired", "{saved}");
    let failed = request(
        h,
        project,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    );
    assert_eq!(failed["error"]["code"], "runtime_rejected", "{failed}");
    let recovered = h.control(json!({"kind":"recover","project":project}));
    assert!(recovered["error"].is_null(), "{recovered}");
    saved
}

fn deposit_during_delayed_edit(
    h: &Harness,
    project: &str,
    editing: &Value,
    armed: &std::sync::atomic::AtomicBool,
    committed_name: &str,
    deposit_generation: &str,
    raw_name: &str,
) -> Value {
    use std::sync::atomic::Ordering;
    let mut committed = editing["body"].clone();
    committed["name"] = json!({"intent":"set","value":committed_name});
    armed.store(true, Ordering::SeqCst);
    let saved = edit_save(h, project, editing, "2", committed);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(saved["problem"], "SavedReadRequired", "{saved}");
    let failed = request(
        h,
        project,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    );
    assert_eq!(failed["error"]["code"], "runtime_rejected", "{failed}");

    let mut raw = saved["body"].clone();
    raw["name"] = json!({"intent":"set","value":raw_name});
    let deposited = request(
        h,
        project,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":deposit_generation,"body":raw}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    deposited
}

fn recover_refresh_retry_save(
    h: &Harness,
    project: &str,
    editing: &Value,
    deposited: &Value,
) -> Value {
    let recovered = h.control(json!({"kind":"recover","project":project}));
    assert!(recovered["error"].is_null(), "{recovered}");
    let refreshed = request(
        h,
        project,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    )["value"]
        .clone();
    assert!(refreshed["problem"].is_null(), "{refreshed}");
    assert_eq!(refreshed["body"], deposited["body"]);
    let retried = request(
        h,
        project,
        json!({"action":"edit_retry","owner":editing["owner"],"generation":refreshed["generation"],"body":refreshed["body"]}),
    )["value"]
        .clone();
    let generation = retried["generation"].as_str().unwrap().to_owned();
    let saved = edit_save(h, project, &retried, &generation, retried["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    saved
}

#[test]
fn m39_fix005_deposit_refresh_retry_preserves_lineage_for_next_create() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "base");
    let listed = list(&h, &p);
    let cached = request(
        &h,
        &p,
        json!({"action":"search","query":"base","template":null,
          "offset":0,"limit":100,"refresh":true}),
    );
    assert_eq!(cached["value"]["total"], 1, "{cached}");
    let editing = edit_begin(&h, &p, &existing);
    let deposited = deposit_during_delayed_edit(
        &h,
        &p,
        &editing,
        &armed,
        "committed S",
        "3",
        "later raw S+1",
    );
    let saved = recover_refresh_retry_save(&h, &p, &editing, &deposited);
    crate::data::repository::test_support::begin_global();
    let searched = request(
        &h,
        &p,
        json!({"action":"search","query":"later raw S+1","template":null,
          "offset":0,"limit":100,"refresh":false}),
    );
    let search_counts = crate::data::repository::test_support::take_global();
    assert_eq!(searched["value"]["total"], 1, "{searched}");
    assert_eq!(search_counts.document_scans, 0);
    assert_eq!(search_counts.read_calls, 0);
    assert_eq!(search_counts.decoded, 0);
    edit_release(&h, &p, &saved);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "created after deposit retry".into();
    let created = save(&h, &p, &draft, "2", body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    let current = list(&h, &p);
    assert!(current["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == existing && row["name"] == "later raw S+1"));
    h.close_clean();
}

#[test]
fn m39_fix005_deposited_generation_release_restore_preserves_lineage() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "base");
    let listed = list(&h, &p);
    let editing = edit_begin(&h, &p, &existing);
    let deposited = deposit_during_delayed_edit(
        &h,
        &p,
        &editing,
        &armed,
        "committed S",
        "3",
        "later raw S+1",
    );
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let entries = rows["page"]["entries"].as_array().unwrap();
    let row = entries
        .iter()
        .map(|entry| &entry["row"])
        .find(|row| row["key"]["generation"] == "3")
        .unwrap()
        .clone();
    edit_release(&h, &p, &deposited);

    let restored = request(
        &h,
        &p,
        json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}),
    )["value"]
        .clone();
    assert_eq!(restored["body"], deposited["body"]);
    assert_eq!(restored["read"]["name"], "committed S");
    let generation = restored["generation"].as_str().unwrap().to_owned();
    let saved = edit_save(&h, &p, &restored, &generation, restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "created after restored deposit".into();
    let created = save(&h, &p, &draft, "2", body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    h.close_clean();
}

#[test]
fn m39_fix005_deposit_retry_updates_tree_and_open_creation_draft() {
    for follow_up in ["tree", "draft"] {
        let h = Harness::new();
        let armed = fail_next_commit_cleanup(&h);
        let p = h.open();
        let (template, _) = h.template(&p);
        let existing = create(&h, &p, &template, "base");
        let listed = list(&h, &p);
        let draft = (follow_up == "draft").then(|| {
            request(
                &h,
                &p,
                json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
            )["value"]
                .clone()
        });
        let editing = edit_begin(&h, &p, &existing);
        let deposited = deposit_during_delayed_edit(
            &h,
            &p,
            &editing,
            &armed,
            "committed S",
            "3",
            "later raw S+1",
        );
        let saved = recover_refresh_retry_save(&h, &p, &editing, &deposited);
        edit_release(&h, &p, &saved);

        if let Some(draft) = draft {
            let mut body = draft["body"].clone();
            body["name"] = "open raw survives deposit retry".into();
            let created = save(&h, &p, &draft, "2", body.clone());
            assert_eq!(created["body"], body);
            assert_eq!(created["owner"], draft["owner"]);
            assert_eq!(created["outcome"]["disk"], "committed", "{created}");
            release(&h, &p, &created, false);
        } else {
            let moved = request(
                &h,
                &p,
                json!({"action":"mutate","snapshot":listed["snapshot"],"edit":{"kind":"trash","document":existing}}),
            );
            assert_eq!(moved["disk"], "committed", "{moved}");
        }
        h.close_clean();
    }
}

#[test]
fn m39_fix005_repeated_deposit_refresh_requery_and_followup_edit_are_ordered() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "base");
    let listed = list(&h, &p);
    let editing = edit_begin(&h, &p, &existing);
    let first = deposit_during_delayed_edit(
        &h,
        &p,
        &editing,
        &armed,
        "committed S",
        "3",
        "later raw S+1",
    );
    let mut latest_body = first["body"].clone();
    latest_body["name"] = json!({"intent":"set","value":"latest raw S+1"});
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":"4","body":latest_body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");

    let refresh = json!({"kind":"document_workspace","project":p,"request":{"action":"edit_refresh","owner":editing["owner"]}});
    let operation = h.submit(refresh);
    let first_response = h.result(&operation);
    let repeated = h.result(&operation);
    assert_eq!(first_response, repeated);
    h.ack(&operation);
    let refreshed = first_response["value"].clone();
    assert_eq!(refreshed["body"], deposited["body"]);
    let retried = request(
        &h,
        &p,
        json!({"action":"edit_retry","owner":editing["owner"],"generation":refreshed["generation"],"body":refreshed["body"]}),
    )["value"]
        .clone();
    assert_eq!(retried["generation"], "5", "{retried}");
    let mut saved = edit_save(&h, &p, &retried, "5", retried["body"].clone());
    let mut final_body = saved["body"].clone();
    final_body["name"] = json!({"intent":"set","value":"final S+2"});
    saved = edit_save(&h, &p, &saved, "6", final_body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);

    let moved = request(
        &h,
        &p,
        json!({"action":"mutate","snapshot":listed["snapshot"],"edit":{"kind":"trash","document":existing}}),
    );
    assert_eq!(moved["disk"], "committed", "{moved}");
    h.close_clean();
}

#[test]
fn m39_fix005_deposit_lineage_rejects_same_length_external_bytes() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "base");
    let listed = list(&h, &p);
    let editing = edit_begin(&h, &p, &existing);
    let deposited = deposit_during_delayed_edit(
        &h,
        &p,
        &editing,
        &armed,
        "committed S",
        "3",
        "later raw S+1",
    );
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");
    let path = h.root.join(format!("documents/{existing}.json"));
    let own = String::from_utf8(fs::read(&path).unwrap()).unwrap();
    let external = own.replace("committed S", "external S?").into_bytes();
    assert_eq!(own.len(), external.len());
    fs::write(&path, &external).unwrap();

    let rejected = request(
        &h,
        &p,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    );
    assert_eq!(rejected["error"]["code"], "wrong_binding", "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), external);
    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "must reject external source".into();
    let rejected = save(&h, &p, &draft, "2", body);
    assert_ne!(rejected["outcome"]["disk"], "committed", "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), external);
    release(&h, &p, &rejected, true);

    fs::write(&path, own.as_bytes()).unwrap();
    let refreshed = request(
        &h,
        &p,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    )["value"]
        .clone();
    assert_eq!(refreshed["body"], deposited["body"]);
    let retried = request(
        &h,
        &p,
        json!({"action":"edit_retry","owner":editing["owner"],"generation":refreshed["generation"],"body":refreshed["body"]}),
    )["value"]
        .clone();
    let saved = edit_save(&h, &p, &retried, "4", retried["body"].clone());
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m39_fix004_delayed_refresh_advances_same_snapshot_for_next_create() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "before delayed refresh");
    let listed = list(&h, &p);
    let cached = request(
        &h,
        &p,
        json!({"action":"search","query":"before delayed refresh","template":null,
          "offset":0,"limit":100,"refresh":true}),
    );
    assert_eq!(cached["value"]["total"], 1, "{cached}");
    let editing = edit_begin(&h, &p, &existing);
    recover_delayed_edit(&h, &p, &editing, &armed, "after delayed refresh");

    // 같은 operation 결과 재조회는 transition을 다시 만들거나 역행시키지 않는다.
    let refresh = json!({"kind":"document_workspace","project":p,"request":{"action":"edit_refresh","owner":editing["owner"]}});
    let operation = h.submit(refresh);
    let first = h.result(&operation);
    let repeated = h.result(&operation);
    assert_eq!(first, repeated);
    h.ack(&operation);
    let refreshed = first["value"].clone();
    assert!(refreshed["problem"].is_null(), "{refreshed}");
    assert_eq!(refreshed["read"]["name"], "after delayed refresh");
    crate::data::repository::test_support::begin_global();
    let searched = request(
        &h,
        &p,
        json!({"action":"search","query":"after delayed refresh","template":null,
          "offset":0,"limit":100,"refresh":false}),
    );
    let search_counts = crate::data::repository::test_support::take_global();
    assert_eq!(searched["value"]["total"], 1, "{searched}");
    assert_eq!(search_counts.document_scans, 0);
    assert_eq!(search_counts.read_calls, 0);
    assert_eq!(search_counts.decoded, 0);
    edit_release(&h, &p, &refreshed);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "created after recovered refresh".into();
    let created = save(&h, &p, &draft, "2", body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    let current = list(&h, &p);
    assert_eq!(current["documents"].as_array().unwrap().len(), 2);
    assert!(current["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == existing && row["name"] == "after delayed refresh"));
    h.close_clean();
}

#[test]
fn m39_fix004_delayed_refresh_updates_tree_and_open_creation_draft() {
    for follow_up in ["tree", "draft"] {
        let h = Harness::new();
        let armed = fail_next_commit_cleanup(&h);
        let p = h.open();
        let (template, _) = h.template(&p);
        let existing = create(&h, &p, &template, "delayed target");
        let listed = list(&h, &p);
        let draft = (follow_up == "draft").then(|| {
            request(
                &h,
                &p,
                json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
            )["value"]
                .clone()
        });
        let editing = edit_begin(&h, &p, &existing);
        recover_delayed_edit(&h, &p, &editing, &armed, "delayed target edited");
        let mut refreshed = request(
            &h,
            &p,
            json!({"action":"edit_refresh","owner":editing["owner"]}),
        )["value"]
            .clone();
        if follow_up == "tree" {
            let mut body = refreshed["body"].clone();
            body["name"] = json!({"intent":"set","value":"delayed target edited again"});
            refreshed = edit_save(&h, &p, &refreshed, "3", body);
            assert_eq!(refreshed["outcome"]["disk"], "committed", "{refreshed}");
        }
        edit_release(&h, &p, &refreshed);

        if let Some(draft) = draft {
            let mut body = draft["body"].clone();
            body["name"] = "open raw survives delayed refresh".into();
            let created = save(&h, &p, &draft, "2", body.clone());
            assert_eq!(created["body"], body);
            assert_eq!(created["owner"], draft["owner"]);
            assert_eq!(created["outcome"]["disk"], "committed", "{created}");
            release(&h, &p, &created, false);
        } else {
            let moved = request(
                &h,
                &p,
                json!({"action":"mutate","snapshot":listed["snapshot"],"edit":{"kind":"trash","document":existing}}),
            );
            assert_eq!(moved["disk"], "committed", "{moved}");
        }
        h.close_clean();
    }
}

#[test]
fn m39_fix004_restore_of_same_committed_proof_advances_snapshot() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "before restored proof");
    let listed = list(&h, &p);
    let editing = edit_begin(&h, &p, &existing);
    let saved = recover_delayed_edit(&h, &p, &editing, &armed, "after restored proof");
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":saved["generation"],"body":saved["body"]}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let entries = rows["page"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{rows}");
    let row = entries[0]["row"].clone();

    edit_release(&h, &p, &deposited);
    let restored = request(
        &h,
        &p,
        json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}),
    )["value"]
        .clone();
    assert_eq!(
        restored["read"]["name"], "after restored proof",
        "{restored}"
    );

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "created after restored proof".into();
    let created = save(&h, &p, &draft, "2", body);
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    release(&h, &p, &created, false);
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":restored["owner"],"generation":restored["generation"],"body":restored["body"]}),
    )["value"]
        .clone();
    edit_release(&h, &p, &deposited);
    h.close_clean();
}

#[test]
fn m39_fix004_delayed_refresh_does_not_adopt_external_same_length_bytes() {
    let h = Harness::new();
    let armed = fail_next_commit_cleanup(&h);
    let p = h.open();
    let (template, _) = h.template(&p);
    let existing = create(&h, &p, &template, "aaaa");
    let listed = list(&h, &p);
    let editing = edit_begin(&h, &p, &existing);
    recover_delayed_edit(&h, &p, &editing, &armed, "bbbb");
    let path = h.root.join(format!("documents/{existing}.json"));
    let own = String::from_utf8(fs::read(&path).unwrap()).unwrap();
    let external = own.replace("bbbb", "cccc").into_bytes();
    assert_eq!(own.len(), external.len());
    fs::write(&path, &external).unwrap();

    let rejected = request(
        &h,
        &p,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    );
    assert_eq!(rejected["error"]["code"], "wrong_binding", "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), external);

    let draft = request(
        &h,
        &p,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let mut body = draft["body"].clone();
    body["name"] = "must remain rejected".into();
    let rejected = save(&h, &p, &draft, "2", body);
    assert_ne!(rejected["outcome"]["disk"], "committed", "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), external);
    release(&h, &p, &rejected, true);

    // 외부 변경을 채택하지 않았다는 검증이 끝난 뒤에만 확정 후보를 되돌려
    // 보류 중인 편집 증명을 정상 완료하고 테스트 프로젝트를 닫는다.
    fs::write(&path, own.as_bytes()).unwrap();
    let refreshed = request(
        &h,
        &p,
        json!({"action":"edit_refresh","owner":editing["owner"]}),
    )["value"]
        .clone();
    assert!(refreshed["problem"].is_null(), "{refreshed}");
    edit_release(&h, &p, &refreshed);
    h.close_clean();
}
#[test]
fn document_edit_three_owners_save_noop_stale_and_preserve_exact_files() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let ids: Vec<_> = [
        "same",
        "same",
        "long document name repeated repeated repeated",
    ]
    .iter()
    .map(|name| create(&h, &p, &t, name))
    .collect();
    let path = h.root.join(format!("documents/{}.json", ids[0]));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["futureMetadata"] = json!({"nested":[1,{"keep":"original"}]});
    fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let before_template = fs::read(h.root.join(format!("templates/{t}.json"))).unwrap();
    let before_layout = fs::read(h.root.join("workspace/document-layout.json")).unwrap();
    let controls: Vec<_> = ids[1..]
        .iter()
        .map(|id| fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
        .collect();
    let mut drafts: Vec<_> = ids.iter().map(|id| edit_begin(&h, &p, id)).collect();
    let before_noop = fs::read(&path).unwrap();
    // compact source의 의미가 같아도 실제 bytes를 pretty-encode한 source로 추정하지 않는다.
    drafts[0] = edit_save(&h, &p, &drafts[0], "2", drafts[0]["body"].clone());
    assert_eq!(drafts[0]["outcome"]["disk"], "no_write");
    assert!(drafts[0]["problem"].is_null());
    assert_eq!(fs::read(&path).unwrap(), before_noop);
    assert_eq!(
        request(&h, &p, json!({"action":"edit_begin","document":ids[0]}))["error"]["code"],
        "duplicate_conflict"
    );
    let mut body = drafts[0]["body"].clone();
    body["name"] = json!({"intent":"set","value":"renamed"});
    drafts[0] = edit_save(&h, &p, &drafts[0], "3", body.clone());
    assert_eq!(drafts[0]["outcome"]["disk"], "committed", "{}", drafts[0]);
    assert!(drafts[0]["problem"].is_null());
    let saved = fs::read(&path).unwrap();
    let actual: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(actual["name"], "renamed");
    assert_eq!(actual["futureMetadata"], wire["futureMetadata"]);
    assert!(actual["fieldValues"].is_object());
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_draft","owner":drafts[0]["owner"],"generation":"2","body":body,"save":true})
        )["error"]["code"],
        "wrong_binding"
    );
    drafts[0] = edit_save(&h, &p, &drafts[0], "4", body);
    assert_eq!(drafts[0]["outcome"]["disk"], "no_write", "{}", drafts[0]);
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert_eq!(
        fs::read(h.root.join(format!("templates/{t}.json"))).unwrap(),
        before_template
    );
    assert_eq!(
        fs::read(h.root.join("workspace/document-layout.json")).unwrap(),
        before_layout
    );
    for (id, bytes) in ids[1..].iter().zip(controls) {
        assert_eq!(
            fs::read(h.root.join(format!("documents/{id}.json"))).unwrap(),
            bytes
        );
    }
    for d in drafts {
        edit_release(&h, &p, &d);
    }
    h.close_clean();
}
#[test]
fn document_edit_store_restart_composing_and_generations_resume_save() {
    let base = std::env::temp_dir().join(format!("worldbuild-m35-{}", uuid::Uuid::new_v4()));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let p = h.open();
    let (t, _) = h.template(&p);
    let id = create(&h, &p, &t, "original");
    let d = edit_begin(&h, &p, &id);
    let mut body = d["body"].clone();
    body["name"] = json!({"intent":"set","value":"raw recovery"});
    body["composing"] = true.into();
    let no_save = edit_save(&h, &p, &d, "2", body.clone());
    assert_eq!(no_save["problem"], "Composing");
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":"2"})
        )["error"]["code"],
        "no_receipt"
    );
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = rows["page"]["entries"][0]["row"].clone();
    body["name"] = json!({"intent":"set","value":"later generation"});
    let later = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"7","body":body}),
    )["value"]
        .clone();
    assert_eq!(later["deposited"], true);
    edit_release(&h, &p, &later);
    h.close_clean();
    drop(h);
    let h = Harness::at(base, backend::provider(), true);
    let p = h.open();
    let restore = json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]});
    let restored = request(&h, &p, restore.clone())["value"].clone();
    assert_eq!(restored["generation"], "8", "{restored}");
    assert_eq!(restored["body"]["composing"], false);
    assert_eq!(restored["body"]["name"]["value"], "raw recovery");
    assert_eq!(
        request(&h, &p, restore)["error"]["code"],
        "duplicate_conflict"
    );
    let saved = edit_save(&h, &p, &restored, "8", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let actual: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap())
            .unwrap();
    assert_eq!(actual["name"], "raw recovery");
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}
#[test]
fn document_edit_raw_fields_unknown_keep_and_external_source_denial() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut template: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let ids = [
        "aaaaaaaa-aaaa-4aaa-8aaa-000000000001",
        "aaaaaaaa-aaaa-4aaa-8aaa-000000000002",
        "aaaaaaaa-aaaa-4aaa-8aaa-000000000003",
    ];
    template["fieldOrder"] = json!(ids);
    for (id, kind) in ids.iter().zip(["number", "singleLineText", "richText"]) {
        template["fields"][id] = json!({"label":kind,"kind":kind,"required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":kind},"presentation":{}});
    }
    fs::write(&tp, serde_json::to_vec(&template).unwrap()).unwrap();
    let id = create(&h, &p, &t, "base");
    let dp = h.root.join(format!("documents/{id}.json"));
    let mut doc: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    doc["fieldValues"][ids[2]] = json!({"kind":"richText","document":{"schemaVersion":1,"content":{"kind":"root","children":[{"kind":"paragraph","custom":1,"children":[{"kind":"text","text":"keep"}]}]}}});
    let before = serde_json::to_vec(&doc).unwrap();
    fs::write(&dp, &before).unwrap();
    let d = edit_begin(&h, &p, &id);
    assert!(!d["editable"].as_array().unwrap().contains(&json!(ids[2])));
    let mut body = d["body"].clone();
    body["name"] = json!({"intent":"set","value":"multi field"});
    body["fields"] = json!([
        {"field":ids[0],"value":{"intent":"set","value":{"kind":"number","value":"-"}}},
        {"field":ids[1],"value":{"intent":"set","value":{"kind":"single_line_text","value":"hello"}}}
    ]);
    let invalid = edit_save(&h, &p, &d, "2", body.clone());
    assert!(invalid["outcome"].is_null());
    assert_eq!(invalid["body"], body);
    assert_eq!(fs::read(&dp).unwrap(), before);
    body["fields"][0]["value"]["value"]["value"] = "0".into();
    let saved = edit_save(&h, &p, &d, "3", body.clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let actual: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
    assert_eq!(
        actual["fieldValues"][ids[0]],
        json!({"kind":"number","value":"0"})
    );
    assert_eq!(
        actual["fieldValues"][ids[1]],
        json!({"kind":"text","value":"hello"})
    );
    assert_eq!(actual["fieldValues"][ids[2]], doc["fieldValues"][ids[2]]);
    let mut external = actual.clone();
    external["name"] = "external".into();
    let external = serde_json::to_vec(&external).unwrap();
    fs::write(&dp, &external).unwrap();
    body["name"] = json!({"intent":"set","value":"must not overwrite"});
    let denied = edit_save(&h, &p, &saved, "4", body.clone());
    assert_ne!(denied["outcome"]["disk"], "committed");
    assert_eq!(denied["body"], body);
    assert_eq!(fs::read(&dp).unwrap(), external);
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"4","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &p, &deposited);
    h.close_clean();
}

#[test]
#[ignore = "explicit read-only validation of the bounded native verification fixture"]
fn native_m34_fixture_artifacts_are_valid() {
    let root = std::path::PathBuf::from(
        std::env::var_os("WB_M34_FIXTURE_ROOT").expect("explicit fixture path"),
    );
    let mut counts = [0, 0];
    for (i, namespace) in ["templates", "documents"].iter().enumerate() {
        for entry in fs::read_dir(root.join(namespace)).unwrap() {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            if i == 0 {
                crate::data::artifact::decode_template(&bytes).unwrap();
            } else {
                crate::data::artifact::decode_document(&bytes).unwrap();
            }
            counts[i] += 1;
        }
    }
    assert_eq!(counts, [12, 50]);
}

#[test]
fn document_creation_guides_empty_values_required_validation_and_raw_recovery() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let path = h.root.join(format!("templates/{t}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let kinds = [
        "singleLineText",
        "richText",
        "number",
        "date",
        "time",
        "duration",
        "singleChoice",
        "multiChoice",
    ];
    let ids: Vec<String> = (1..=8)
        .map(|n| format!("aaaaaaaa-aaaa-4aaa-8aaa-{n:012x}"))
        .collect();
    let option = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    wire["fieldOrder"] = json!(ids);
    for (i, kind) in kinds.iter().enumerate() {
        let config = if i >= 6 {
            json!({"kind":kind,"optionOrder":[option],"options":{(option):{"label":"A","lifecycle":"active"}}})
        } else {
            json!({"kind":kind})
        };
        // Option IDs are globally unique; only the last two fields need separate options.
        let mut config = config;
        if i == 7 {
            let second = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
            config = json!({"kind":kind,"optionOrder":[second],"options":{(second):{"label":"B","lifecycle":"active"}}});
        }
        wire["fields"][&ids[i]] = json!({"label":kind,"kind":kind,"required":false,"lifecycle":"active","introducedRevision":1,"writingGuide":"입력 예시", "defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":config,"presentation":{}});
    }
    wire["fields"][&ids[2]]["required"] = true.into();
    wire["fields"][&ids[2]]["defaultValue"] = json!({"kind":"number","value":"99"});
    let before = serde_json::to_vec(&wire).unwrap();
    fs::write(&path, &before).unwrap();
    let d = begin(&h, &p, &t);
    assert_eq!(d["template"]["fields"][2]["writingGuide"], "입력 예시");
    assert_eq!(d["body"]["fields"], json!([]));
    let mut body = d["body"].clone();
    body["name"] = "빈 선택값 시험".into();
    let empty = [
        json!({"kind":"single_line_text","value":""}),
        json!({"kind":"rich_text","content":{"kind":"root","children":[{"kind":"paragraph","children":[]}]}}),
        json!({"kind":"number","value":""}),
        json!({"kind":"date","value":""}),
        json!({"kind":"time","value":""}),
        json!({"kind":"duration","milliseconds":""}),
        json!({"kind":"single_choice","option":""}),
        json!({"kind":"multi_choice","options":[]}),
    ];
    body["fields"] = json!(ids
        .iter()
        .zip(empty)
        .map(|(id, value)| json!({"field":id,"value":{"intent":"set","value":value}}))
        .collect::<Vec<_>>());
    let rejected = save(&h, &p, &d, "2", body.clone());
    assert_eq!(rejected["problem"], "RequiredValueUnset");
    assert_eq!(rejected["body"], body);
    assert!(!h.root.join("workspace/document-layout.json").exists());
    body["fields"][2]["value"]["value"]["value"] = "-".into();
    let invalid = save(&h, &p, &d, "3", body.clone());
    assert!(invalid["outcome"].is_null());
    assert_eq!(invalid["body"], body);
    let deposited = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"3","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true);
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &rows["page"]["entries"][0]["row"];
    let restore = json!({"action":"restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]});
    release(&h, &p, &deposited, false);
    let restored = request(&h, &p, restore)["value"].clone();
    assert_eq!(restored["body"]["fields"], body["fields"]);
    body["fields"][2]["value"]["value"]["value"] = "0".into();
    let blank_recovered = save(&h, &p, &restored, "5", body.clone());
    assert!(blank_recovered["outcome"].is_null());
    assert_eq!(blank_recovered["body"], body);
    // Recovery preserves raw Set/Unset. The user explicitly clears unsupported
    // blank scalar/choice tokens; ordinary new-document normalization above stays.
    for (index, field) in body["fields"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if index != 2 {
            field["value"] = json!({"intent":"unset"});
        }
    }
    let saved = save(&h, &p, &blank_recovered, "6", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let doc: Value = serde_json::from_slice(
        &fs::read(h.root.join(format!(
            "documents/{}.json",
            saved["outcome"]["artifact"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(
            doc["fieldValues"][id],
            if i == 2 {
                json!({"kind":"number","value":"0"})
            } else {
                json!({"kind":"unset"})
            }
        );
    }
    assert_eq!(fs::read(path).unwrap(), before);
    release(&h, &p, &saved, false);
    h.close_clean();
}
fn request(h: &Harness, p: &str, request: Value) -> Value {
    h.work(json!({"kind":"document_workspace","project":p,"request":request}))
}
fn list(h: &Harness, p: &str) -> Value {
    let r = request(h, p, json!({"action":"list"}));
    assert_eq!(r["kind"], "document_workspace", "{r}");
    r["value"].clone()
}
fn begin(h: &Harness, p: &str, t: &str) -> Value {
    let r = request(h, p, json!({"action":"begin","template":t}));
    assert_eq!(
        r["value"]["kind"],
        "draft",
        "{r} list={} read={}",
        list(h, p),
        h.read_template(p, t)
    );
    r["value"].clone()
}
fn save(h: &Harness, p: &str, d: &Value, g: &str, body: Value) -> Value {
    request(
        h,
        p,
        json!({"action":"draft","owner":d["owner"],"generation":g,"body":body,"save":true}),
    )["value"]
        .clone()
}
fn release(h: &Harness, p: &str, d: &Value, discard: bool) {
    let r = request(
        h,
        p,
        json!({"action":"release","owner":d["owner"],"generation":d["generation"],"discard":discard}),
    );
    assert_eq!(r["value"]["kind"], "released", "{r}");
}
fn create(h: &Harness, p: &str, t: &str, name: &str) -> String {
    let d = begin(h, p, t);
    let mut body = d["body"].clone();
    body["name"] = name.into();
    let saved = save(h, p, &d, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let id = saved["outcome"]["artifact"].as_str().unwrap().into();
    release(h, p, &saved, false);
    id
}
fn mutate(h: &Harness, p: &str, edit: Value) -> Value {
    let l = list(h, p);
    request(
        h,
        p,
        json!({"action":"mutate","snapshot":l["snapshot"],"edit":edit}),
    )
}

#[test]
fn m545_fix004_list_distinguishes_missing_template_from_unverified_source() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create(&h, &project, &template, "warning before selection");
    let template_path = h.root.join(format!("templates/{template}.json"));
    let original = fs::read(&template_path).unwrap();

    let healthy = list(&h, &project);
    assert_eq!(healthy["issueStatus"], "complete", "{healthy}");
    assert_eq!(healthy["issues"], json!([]), "{healthy}");

    fs::remove_file(&template_path).unwrap();
    let missing = list(&h, &project);
    assert_eq!(missing["issueStatus"], "complete", "{missing}");
    assert_eq!(missing["issues"][0]["document"], document);
    assert_eq!(missing["issues"][0]["reasons"], json!(["template_missing"]));

    fs::write(&template_path, b"{invalid template bytes").unwrap();
    let unreadable = list(&h, &project);
    assert_eq!(unreadable["issueStatus"], "partial", "{unreadable}");
    assert_eq!(unreadable["unverifiedDocuments"], json!([document]));
    assert_eq!(unreadable["issues"], json!([]));

    fs::write(&template_path, original).unwrap();
    assert_eq!(list(&h, &project)["issueStatus"], "complete");
    h.close_clean();
}

#[test]
fn m545_fix005_list_rechecks_initial_template_absence() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create(&h, &project, &template, "missing then created");
    let path = h.root.join(format!("templates/{template}.json"));
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    crate::commands::backend::install_issue_snapshot_hook(
        template.clone(),
        Box::new(move || fs::write(&path, original).unwrap()),
    );
    let changed = list(&h, &project);
    assert_eq!(changed["issueStatus"], "partial", "{changed}");
    assert_eq!(changed["unverifiedDocuments"], json!([document]));
    assert_eq!(changed["issues"], json!([]));
    assert_eq!(list(&h, &project)["issueStatus"], "complete");
    h.close_clean();
}

#[test]
fn m545_fix005_list_preserves_first_shared_template_source() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let first = create(&h, &project, &template, "shared first");
    let second = create(&h, &project, &template, "shared second");
    let path = h.root.join(format!("templates/{template}.json"));
    crate::commands::backend::install_issue_snapshot_hook(
        template,
        Box::new(move || {
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b' ');
            fs::write(path, bytes).unwrap();
        }),
    );
    let changed = list(&h, &project);
    assert_eq!(changed["issueStatus"], "partial", "{changed}");
    let unverified = changed["unverifiedDocuments"].as_array().unwrap();
    assert_eq!(unverified.len(), 2);
    assert!(unverified.contains(&json!(first)));
    assert!(unverified.contains(&json!(second)));
    assert_eq!(changed["issues"], json!([]));
    assert_eq!(list(&h, &project)["issueStatus"], "complete");
    h.close_clean();
}

#[test]
fn m545_fix004_native_restore_requires_current_active_template_even_for_root_destination() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create(&h, &project, &template, "restore dependency");
    let trashed = mutate(&h, &project, json!({"kind":"trash","document":document}));
    assert_eq!(trashed["disk"], "committed", "{trashed}");
    let before = list(&h, &project);
    let layout_path = h.root.join("workspace/document-layout.json");
    let layout_bytes = fs::read(&layout_path).unwrap();
    let document_path = h.root.join(format!("documents/{document}.json"));
    let document_bytes = fs::read(&document_path).unwrap();
    let template_path = h.root.join(format!("templates/{template}.json"));
    let original = fs::read(&template_path).unwrap();
    let mut deleted: Value = serde_json::from_slice(&original).unwrap();
    deleted["lifecycle"] = json!("deleted");
    fs::write(&template_path, serde_json::to_vec(&deleted).unwrap()).unwrap();
    let restore = |snapshot: &Value| {
        request(
            &h,
            &project,
            json!({"action":"mutate","snapshot":snapshot["snapshot"],"edit":{
                "kind":"restore","document":document,
                "destination":{"parent":null,"index":0}
            }}),
        )
    };
    let rejected = restore(&before);
    assert_eq!(rejected["disk"], "not_attempted", "{rejected}");
    assert_eq!(fs::read(&layout_path).unwrap(), layout_bytes);
    assert_eq!(fs::read(&document_path).unwrap(), document_bytes);

    fs::remove_file(&template_path).unwrap();
    let missing_base = list(&h, &project);
    let missing = restore(&missing_base);
    assert_eq!(missing["disk"], "not_attempted", "{missing}");
    assert_eq!(fs::read(&layout_path).unwrap(), layout_bytes);

    fs::write(&template_path, serde_json::to_vec(&deleted).unwrap()).unwrap();
    let template_restored = h.work(json!({
        "kind":"template_restore","project":project,"template":template
    }));
    assert_eq!(
        template_restored["disk"], "committed",
        "{template_restored}"
    );
    let new_template: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    assert_ne!(
        new_template["revision"],
        serde_json::from_slice::<Value>(&original).unwrap()["revision"]
    );
    let restored_base = list(&h, &project);
    let restored = restore(&restored_base);
    assert_eq!(restored["disk"], "committed", "{restored}");
    let current = list(&h, &project);
    assert_eq!(current["layout"]["nodes"][&document]["state"], "active");
    assert_eq!(fs::read(&document_path).unwrap(), document_bytes);
    let readable = request(&h, &project, json!({"action":"read","document":document}));
    assert_eq!(
        readable["value"]["name"], "restore dependency",
        "{readable}"
    );
    h.close_clean();
}

#[test]
fn m43_fix001_term_info_rejects_every_multiline_entry_path_and_preserves_drafts() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);

    let draft = begin(&h, &project, &template);
    let mut creation_body = draft["body"].clone();
    creation_body["name"] = "용어 문서".into();
    creation_body["englishName"] = "line\u{2028}break".into();
    creation_body["glossarySummary"] = "정상 요약".into();
    let rejected = save(&h, &project, &draft, "2", creation_body.clone());
    assert_eq!(rejected["problem"], "SingleLineRequired", "{rejected}");
    assert_eq!(rejected["field"], "englishName", "{rejected}");
    assert_eq!(rejected["body"], creation_body);
    assert!(!h.root.join("workspace/document-layout.json").exists());

    let deposited = request(
        &h,
        &project,
        json!({
            "action":"deposit",
            "owner":rejected["owner"],
            "generation":rejected["generation"],
            "body":creation_body
        }),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &page["page"]["entries"][0]["row"];
    let restore = json!({
        "action":"restore",
        "key":row["key"],
        "deposit_id":row["depositId"],
        "digest":row["payloadDigest"]
    });
    release(&h, &project, &deposited, false);
    let restored = request(&h, &project, restore)["value"].clone();
    assert_eq!(restored["body"], rejected["body"]);

    let next_generation = |entry: &Value| {
        (entry["generation"]
            .as_str()
            .expect("generation string")
            .parse::<u64>()
            .expect("generation integer")
            + 1)
        .to_string()
    };
    let mut summary_invalid_body = restored["body"].clone();
    summary_invalid_body["englishName"] = "  한글\t🙂  ".into();
    summary_invalid_body["glossarySummary"] = "앞\u{2029}뒤".into();
    let summary_rejected = save(
        &h,
        &project,
        &restored,
        &next_generation(&restored),
        summary_invalid_body.clone(),
    );
    assert_eq!(
        summary_rejected["problem"], "SingleLineRequired",
        "{summary_rejected}"
    );
    assert_eq!(summary_rejected["field"], "glossarySummary");
    assert_eq!(summary_rejected["body"], summary_invalid_body);

    let mut valid_body = summary_rejected["body"].clone();
    valid_body["glossarySummary"] = "복구 뒤 정상 요약".into();
    let created = save(
        &h,
        &project,
        &summary_rejected,
        &next_generation(&summary_rejected),
        valid_body,
    );
    assert_eq!(created["outcome"]["disk"], "committed", "{created}");
    let document = created["outcome"]["artifact"]
        .as_str()
        .expect("created document id")
        .to_owned();
    let document_path = h.root.join(format!("documents/{document}.json"));
    let stored: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
    assert_eq!(stored["englishName"], "  한글\t🙂  ");
    assert_eq!(stored["glossarySummary"], "복구 뒤 정상 요약");
    release(&h, &project, &created, false);

    let editing = edit_begin(&h, &project, &document);
    let source_before = fs::read(&document_path).unwrap();
    let mut edit_body = editing["body"].clone();
    edit_body["englishName"] = json!({"intent":"set","value":"앞\u{0085}뒤"});
    let edit_rejected = edit_save(&h, &project, &editing, "2", edit_body.clone());
    assert_eq!(
        edit_rejected["problem"], "SingleLineRequired",
        "{edit_rejected}"
    );
    assert_eq!(edit_rejected["field"], "englishName");
    assert_eq!(edit_rejected["body"], edit_body);
    assert_eq!(fs::read(&document_path).unwrap(), source_before);

    let mut summary_edit_body = edit_rejected["body"].clone();
    summary_edit_body["englishName"] = json!({"intent":"set","value":"Allowed"});
    summary_edit_body["glossarySummary"] = json!({"intent":"set","value":"앞\u{000B}뒤"});
    let summary_edit_rejected =
        edit_save(&h, &project, &edit_rejected, "3", summary_edit_body.clone());
    assert_eq!(
        summary_edit_rejected["problem"], "SingleLineRequired",
        "{summary_edit_rejected}"
    );
    assert_eq!(summary_edit_rejected["field"], "glossarySummary");
    assert_eq!(summary_edit_rejected["body"], summary_edit_body);
    assert_eq!(fs::read(&document_path).unwrap(), source_before);

    let mut valid_edit_body = summary_edit_rejected["body"].clone();
    valid_edit_body["glossarySummary"] = json!({"intent":"unset"});
    let edited = edit_save(&h, &project, &summary_edit_rejected, "4", valid_edit_body);
    assert_eq!(edited["outcome"]["disk"], "committed", "{edited}");
    let stored: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
    assert_eq!(stored["englishName"], "Allowed");
    assert!(stored.get("glossarySummary").is_none());
    edit_release(&h, &project, &edited);
    h.close_clean();
}

#[test]
fn m42_reference_targets_self_template_trash_and_existing_keep_are_project_validated() {
    const RELATION: &str = "71717171-7171-4171-8171-717171717171";
    const LINK: &str = "72727272-7272-4272-8272-727272727272";
    const CONNECTION: &str = "73737373-7373-4373-8373-737373737373";
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let (allowed_template, _) = h.template(&project);
    let template_path = h.root.join(format!("templates/{template}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    wire["fieldOrder"] = json!([RELATION, LINK]);
    wire["fields"][RELATION] = json!({
        "label":"관계",
        "kind":"relation",
        "required":false,
        "lifecycle":"active",
        "introducedRevision":1,
        "defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},
        "configuration":{
            "kind":"relation",
            "multiple":true,
            "allowedTemplates":[allowed_template],
            "reciprocalNotice":true
        },
        "presentation":{}
    });
    wire["fields"][LINK] = json!({
        "label":"문서 링크",
        "kind":"documentLink",
        "required":false,
        "lifecycle":"active",
        "introducedRevision":1,
        "defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},
        "configuration":{"kind":"documentLink"},
        "presentation":{}
    });
    fs::write(&template_path, serde_json::to_vec(&wire).unwrap()).unwrap();

    let source = create(&h, &project, &template, "source");
    let disallowed = create(&h, &project, &template, "disallowed");
    let allowed = create(&h, &project, &allowed_template, "allowed");

    let editing = edit_begin(&h, &project, &source);
    let body = |target: &str| {
        json!({
            "name":{"intent":"keep"},
            "fields":[
                {"field":RELATION,"value":{"intent":"set","value":{
                    "kind":"relation",
                    "links":[{"id":CONNECTION,"document":target,"oneWay":false}]
                }}},
                {"field":LINK,"value":{"intent":"set","value":{
                    "kind":"document_link","documents":[source]
                }}}
            ],
            "composing":false
        })
    };
    let self_body = body(&source);
    let parsed = serde_json::from_value::<crate::commands::document_workspace::Request>(json!({
        "action":"edit_draft",
        "owner":editing["owner"],
        "generation":"2",
        "body":self_body,
        "save":true
    }));
    assert!(parsed.is_ok(), "reference request DTO: {:?}", parsed.err());
    let self_denied = edit_save(&h, &project, &editing, "2", body(&source));
    assert_eq!(self_denied["problem"], "SelfRelation", "{self_denied}");
    let disallowed_denied = edit_save(&h, &project, &self_denied, "3", body(&disallowed));
    assert_eq!(
        disallowed_denied["problem"], "InvalidDocumentReference",
        "{disallowed_denied}"
    );
    let saved = edit_save(&h, &project, &disallowed_denied, "4", body(&allowed));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &project, &saved);

    let moved = mutate(&h, &project, json!({"kind":"trash","document":allowed}));
    assert!(moved["error"].is_null(), "{moved}");
    let editing = edit_begin(&h, &project, &source);
    let rename = json!({
        "name":{"intent":"set","value":"renamed with dangling Keep"},
        "fields":[],
        "composing":false
    });
    let kept = edit_save(&h, &project, &editing, "2", rename);
    assert_eq!(kept["outcome"]["disk"], "committed", "{kept}");
    edit_release(&h, &project, &kept);

    let editing = edit_begin(&h, &project, &source);
    let trashed_denied = edit_save(&h, &project, &editing, "2", body(&allowed));
    assert_eq!(
        trashed_denied["problem"], "InvalidDocumentReference",
        "{trashed_denied}"
    );
    let deposited = request(
        &h,
        &project,
        json!({
            "action":"edit_deposit",
            "owner":trashed_denied["owner"],
            "generation":"3",
            "body":trashed_denied["body"]
        }),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &project, &deposited);
    h.close_clean();
}

#[test]
fn m41_search_reads_active_display_values_and_excludes_trashed_documents() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let template_path = h.root.join(format!("templates/{template}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    let text = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";
    let rich = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2";
    let choice = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3";
    let option = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    wire["fieldOrder"] = json!([text, rich, choice]);
    wire["fields"][text] = json!({
        "label":"요약","kind":"singleLineText","required":false,"lifecycle":"active",
        "introducedRevision":1,"defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"singleLineText"},
        "presentation":{}
    });
    wire["fields"][rich] = json!({
        "label":"본문","kind":"richText","required":false,"lifecycle":"active",
        "introducedRevision":1,"defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},"configuration":{"kind":"richText"},
        "presentation":{}
    });
    wire["fields"][choice] = json!({
        "label":"분류","kind":"singleChoice","required":false,"lifecycle":"active",
        "introducedRevision":1,"defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},
        "configuration":{"kind":"singleChoice","optionOrder":[option],
          "options":{(option):{"label":"선택 표시 이름","lifecycle":"active"}}},
        "presentation":{}
    });
    fs::write(&template_path, serde_json::to_vec(&wire).unwrap()).unwrap();

    let draft = begin(&h, &project, &template);
    let mut body = draft["body"].clone();
    body["name"] = "검색 제목 Alpha".into();
    body["fields"] = json!([
        {"field":text,"value":{"intent":"set","value":{"kind":"single_line_text","value":"한글 필드값"}}},
        {"field":rich,"value":{"intent":"set","value":{"kind":"rich_text","content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":"강조된","marks":["bold"]},{"kind":"text","text":" 본문","marks":[]}]}]}}}},
        {"field":choice,"value":{"intent":"set","value":{"kind":"single_choice","option":option}}}
    ]);
    let saved = save(&h, &project, &draft, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let document = saved["outcome"]["artifact"].as_str().unwrap().to_owned();
    release(&h, &project, &saved, false);
    let listed = list(&h, &project);

    let search = |query: &str, filter: Option<&str>, refresh: bool| {
        request(
            &h,
            &project,
            json!({"action":"search","query":query,"template":filter,
              "offset":0,"limit":100,"refresh":refresh}),
        )["value"]
            .clone()
    };
    for query in [" alpha ", "한글 필드값", "강조된 본문", "선택 표시 이름"] {
        let result = search(query, Some(&template), false);
        assert_eq!(result["kind"], "search", "{result}");
        assert_eq!(result["total"], 1, "query={query} {result}");
        assert_eq!(result["results"][0]["id"], document);
    }
    let other_template = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    assert_eq!(search("", Some(other_template), false)["total"], 0);

    assert_eq!(
        request(
            &h,
            &project,
            json!({"action":"search_read","document":document})
        )["value"]["kind"],
        "read"
    );
    let editing = edit_begin(&h, &project, &document);
    let mut edited_body = editing["body"].clone();
    edited_body["name"] = json!({"intent":"set","value":"갱신 제목 Beta"});
    edited_body["fields"] = json!([
        {"field":text,"value":{"intent":"set","value":{"kind":"single_line_text","value":"갱신된 단일 문서 값"}}},
        {"field":rich,"value":{"intent":"keep"}},
        {"field":choice,"value":{"intent":"keep"}}
    ]);
    crate::data::repository::test_support::begin_global();
    let edited = edit_save(&h, &project, &editing, "2", edited_body);
    let save_counts = crate::data::repository::test_support::take_global();
    assert_eq!(edited["outcome"]["disk"], "committed", "{edited}");
    assert!(
        save_counts.decoded > 0,
        "save must retain its own verification"
    );

    crate::data::repository::test_support::begin_global();
    let refreshed_title = search("Beta", None, false);
    let search_counts = crate::data::repository::test_support::take_global();
    assert_eq!(refreshed_title["total"], 1, "{refreshed_title}");
    assert_eq!(search_counts.document_scans, 0, "no full search rebuild");
    assert_eq!(
        search_counts.read_calls, 0,
        "query reuses the updated entry"
    );
    assert_eq!(search_counts.decoded, 0, "query reuses the updated entry");
    assert_eq!(search("갱신된 단일 문서 값", None, false)["total"], 1);
    assert_eq!(search("한글 필드값", None, false)["total"], 0);
    edit_release(&h, &project, &edited);

    let trashed = request(
        &h,
        &project,
        json!({"action":"mutate","snapshot":listed["snapshot"],
          "edit":{"kind":"trash","document":document}}),
    );
    assert_eq!(trashed["disk"], "committed", "{trashed}");
    assert_eq!(
        search("Alpha", None, false)["total"],
        0,
        "committed layout mutation must invalidate the derived search cache"
    );
    assert_eq!(
        request(
            &h,
            &project,
            json!({"action":"search_read","document":document})
        )["value"]["kind"],
        "search_unavailable"
    );
    assert_eq!(
        request(&h, &project, json!({"action":"read","document":document}))["value"]["kind"],
        "read",
        "ordinary trash view keeps the existing read contract"
    );
    let after_trash =
        request(&h, &project, json!({"action":"list","refreshSearch":false}))["value"].clone();
    let restored = request(
        &h,
        &project,
        json!({"action":"mutate","snapshot":after_trash["snapshot"],
          "edit":{"kind":"restore","document":document,"destination":null}}),
    );
    assert_eq!(restored["disk"], "committed", "{restored}");
    crate::data::repository::test_support::begin_global();
    assert_eq!(search("Beta", None, false)["total"], 1);
    let restored_search_counts = crate::data::repository::test_support::take_global();
    assert_eq!(restored_search_counts.document_scans, 0);
    assert_eq!(restored_search_counts.read_calls, 0);
    assert_eq!(restored_search_counts.decoded, 0);
    h.close_clean();
}

#[test]
fn m41_search_cache_adds_a_committed_creation_without_a_full_rebuild() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    assert_eq!(
        request(
            &h,
            &project,
            json!({"action":"search","query":"새 검색 문서","template":null,
              "offset":0,"limit":100,"refresh":true})
        )["value"]["total"],
        0
    );
    let draft = begin(&h, &project, &template);
    let mut body = draft["body"].clone();
    body["name"] = "새 검색 문서".into();
    let saved = save(&h, &project, &draft, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    release(&h, &project, &saved, false);

    crate::data::repository::test_support::begin_global();
    let result = request(
        &h,
        &project,
        json!({"action":"search","query":"새 검색 문서","template":null,
          "offset":0,"limit":100,"refresh":false}),
    );
    let counts = crate::data::repository::test_support::take_global();
    assert_eq!(result["value"]["total"], 1, "{result}");
    assert_eq!(counts.document_scans, 0);
    assert_eq!(counts.read_calls, 0);
    assert_eq!(counts.decoded, 0);
    h.close_clean();
}

#[test]
fn m41_search_read_reports_a_deleted_result_without_hiding_other_read_failures() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create(&h, &project, &template, "삭제 전 검색 결과");
    assert_eq!(
        request(
            &h,
            &project,
            json!({"action":"search","query":"삭제 전","template":null,
              "offset":0,"limit":100,"refresh":true})
        )["value"]["total"],
        1
    );
    fs::remove_file(h.root.join(format!("documents/{document}.json"))).unwrap();
    assert_eq!(
        request(
            &h,
            &project,
            json!({"action":"search_read","document":document})
        )["value"]["kind"],
        "search_unavailable"
    );
    h.close_clean();
}

#[test]
fn m39_fix_creation_commit_publishes_list_read_and_reusable_layout_snapshot() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let draft = begin(&h, &project, &template);
    let mut body = draft["body"].clone();
    body["name"] = "즉시 공개 문서".into();
    let saved = save(&h, &project, &draft, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let document = saved["outcome"]["artifact"].as_str().unwrap();
    assert_eq!(saved["commit"]["read"]["id"], document);
    assert!(saved["commit"]["changedDocuments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|summary| summary["id"] == document));
    assert_eq!(saved["commit"]["documentCount"], 1);
    let snapshot = saved["commit"]["snapshot"].clone();
    release(&h, &project, &saved, false);
    let moved = request(
        &h,
        &project,
        json!({"action":"mutate","snapshot":snapshot,"edit":{"kind":"trash","document":document}}),
    );
    assert_eq!(moved["disk"], "committed", "{moved}");
    h.close_clean();
}

#[test]
fn m39_fix002_begin_accepts_the_bound_list_snapshot() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let listed = list(&h, &project);
    let draft = request(
        &h,
        &project,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    assert_eq!(draft["kind"], "draft", "{draft}");
    release(&h, &project, &draft, true);
    h.close_clean();
}

#[test]
fn m39_fix002_cached_scan_rejects_external_source_change_before_create_commit() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let existing = create(&h, &project, &template, "외부 변경 전");
    let listed = list(&h, &project);
    let draft = request(
        &h,
        &project,
        json!({"action":"begin","template":template,"snapshot":listed["snapshot"]}),
    )["value"]
        .clone();
    let path = h.root.join(format!("documents/{existing}.json"));
    let mut external: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    external["name"] = "외부에서 바뀜".into();
    let external = serde_json::to_vec(&external).unwrap();
    fs::write(&path, &external).unwrap();
    let mut body = draft["body"].clone();
    body["name"] = "새 문서".into();
    let rejected = save(&h, &project, &draft, "2", body);
    assert_ne!(rejected["outcome"]["disk"], "committed", "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), external);
    release(&h, &project, &rejected, true);
    assert_eq!(list(&h, &project)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}

#[test]
fn document_workspace_pair_tree_trash_restore_no_write_and_reopen() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    assert_eq!(list(&h, &p)["initial"], true);
    assert!(!h.root.join("workspace/document-layout.json").exists());
    let a = create(&h, &p, &t, "첫 문서");
    let b = create(&h, &p, &t, "둘째 문서");
    let c = create(&h, &p, &t, "긴 한글 이름 세 번째 문서");
    let originals: [Vec<u8>; 3] =
        [&a, &b, &c].map(|id| fs::read(h.root.join(format!("documents/{id}.json"))).unwrap());
    let moved = mutate(
        &h,
        &p,
        json!({"kind":"move","document":b,"parent":a,"index":0}),
    );
    assert_eq!(moved["disk"], "committed", "{moved}");
    let path = h.root.join("workspace/document-layout.json");
    let bytes = fs::read(&path).unwrap();
    let noop = mutate(
        &h,
        &p,
        json!({"kind":"move","document":b,"parent":a,"index":0}),
    );
    assert_eq!(noop["disk"], "no_write", "{noop}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    for edit in [
        json!({"kind":"move","document":a,"parent":b,"index":0}),
        json!({"kind":"trash","document":a}),
    ] {
        let r = mutate(&h, &p, edit);
        assert_eq!(r["disk"], "not_attempted", "{r}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    assert_eq!(
        mutate(&h, &p, json!({"kind":"trash","document":b}))["disk"],
        "committed"
    );
    assert_eq!(
        mutate(&h, &p, json!({"kind":"trash","document":a}))["disk"],
        "committed"
    );
    assert_eq!(
        mutate(
            &h,
            &p,
            json!({"kind":"restore","document":b,"destination":null})
        )["disk"],
        "not_attempted"
    );
    assert_eq!(
        mutate(
            &h,
            &p,
            json!({"kind":"restore","document":b,"destination":{"parent":null,"index":4294967295u32}})
        )["disk"],
        "committed"
    );
    assert_eq!(
        mutate(
            &h,
            &p,
            json!({"kind":"restore","document":a,"destination":null})
        )["disk"],
        "committed"
    );
    for (id, bytes) in [&a, &b, &c].into_iter().zip(originals) {
        assert_eq!(
            fs::read(h.root.join(format!("documents/{id}.json"))).unwrap(),
            bytes
        );
    }
    let read = request(&h, &p, json!({"action":"read","document":a}));
    assert_eq!(read["value"]["name"], "첫 문서");
    h.close_clean();
}

#[test]
fn creation_and_move_preserve_unplaced_until_explicit_adoption() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "A");
    let original = fs::read(h.root.join(format!("documents/{a}.json"))).unwrap();
    let mut external: Value = serde_json::from_slice(&original).unwrap();
    let external_id = uuid::Uuid::new_v4().to_string();
    external["documentId"] = external_id.clone().into();
    let external_bytes = serde_json::to_vec(&external).unwrap();
    let external_path = h.root.join(format!("documents/{external_id}.json"));
    fs::write(&external_path, &external_bytes).unwrap();
    assert_eq!(list(&h, &p)["unplaced"], json!([external_id]));
    let b = create(&h, &p, &t, "B");
    assert_eq!(list(&h, &p)["unplaced"], json!([external_id]));
    assert_eq!(
        mutate(
            &h,
            &p,
            json!({"kind":"move","document":b,"parent":a,"index":0})
        )["disk"],
        "committed"
    );
    assert_eq!(list(&h, &p)["unplaced"], json!([external_id]));
    assert_eq!(mutate(&h, &p, json!({"kind":"adopt"}))["disk"], "committed");
    assert_eq!(list(&h, &p)["unplaced"], json!([]));
    assert_eq!(fs::read(external_path).unwrap(), external_bytes);
    assert_eq!(
        fs::read(h.root.join(format!("documents/{a}.json"))).unwrap(),
        original
    );
    h.close_clean();
}

#[test]
fn document_create_acquire_failure_reports_error_and_preserves_draft() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "retained draft".into();
    provider
        .fail_acquire
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let failed = save(&h, &p, &d, "2", body.clone());
    assert_eq!(failed["kind"], "draft", "{failed}");
    assert_eq!(failed["problem"], "SessionRejected", "{failed}");
    assert_eq!(failed["body"], body);
    assert_ne!(failed["outcome"]["disk"], "committed");
    assert!(!h.root.join("workspace/document-layout.json").exists());
    assert!(
        !h.root.join("documents").exists()
            || fs::read_dir(h.root.join("documents")).unwrap().count() == 0
    );
    provider
        .fail_acquire
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let deposited = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    release(&h, &p, &deposited, false);
    h.close_clean();
}

#[test]
fn document_workspace_raw_deposit_restore_duplicate_owner_and_single_create() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "조합 중 원문".into();
    body["composing"] = true.into();
    let failed = save(&h, &p, &d, "2", body.clone());
    assert_eq!(failed["problem"], "Composing");
    assert!(!h.root.join("workspace/document-layout.json").exists());
    let deposited = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &page["page"]["entries"][0]["row"];
    let key: Key = serde_json::from_value(row["key"].clone()).unwrap();
    let stored_bytes = {
        let store = h.state.recovery.connect().unwrap();
        let record = store
            .lock()
            .unwrap()
            .read(&key, row["depositId"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            record.payload_digest(),
            row["payloadDigest"].as_str().unwrap()
        );
        assert_eq!(
            serde_json::to_value(&record.envelope().draft).unwrap()["composing"],
            true
        );
        assert!(!record.envelope().originals.is_empty());
        record.bytes().to_vec()
    };
    let restore = json!({"action":"restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]});
    let blocked = request(&h, &p, restore.clone());
    assert_eq!(blocked["error"]["code"], "duplicate_conflict", "{blocked}");
    release(&h, &p, &deposited, false);
    let restored = request(&h, &p, restore.clone())["value"].clone();
    assert_eq!(restored["body"]["name"], "조합 중 원문", "{restored}");
    // 보관한 raw 조합 증거는 그대로 두고, 복원 응답의 새 입력 세션만 비조합으로 시작한다.
    assert_eq!(restored["body"]["composing"], false, "{restored}");
    assert_eq!(restored["generation"], "3");
    let store = h.state.recovery.connect().unwrap();
    let stored_after_restore = store
        .lock()
        .unwrap()
        .read(&key, row["depositId"].as_str().unwrap())
        .unwrap();
    assert_eq!(stored_after_restore.bytes(), stored_bytes);
    assert_eq!(
        request(&h, &p, restore)["error"]["code"],
        "duplicate_conflict"
    );
    let mut live_composition = restored["body"].clone();
    live_composition["composing"] = true.into();
    let blocked = save(&h, &p, &restored, "4", live_composition);
    assert_eq!(blocked["problem"], "Composing", "{blocked}");
    // compositionend 뒤에는 복원 응답의 비조합 body가 다시 제출된다.
    let saved = save(&h, &p, &restored, "5", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    release(&h, &p, &saved, false);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}

#[test]
fn document_workspace_stale_layout_and_future_layout_preserve_files() {
    let h = Harness::new();
    let p = h.open();
    let (t, _) = h.template(&p);
    let a = create(&h, &p, &t, "A");
    let before = list(&h, &p);
    assert_eq!(
        mutate(&h, &p, json!({"kind":"trash","document":a}))["disk"],
        "committed"
    );
    let path = h.root.join("workspace/document-layout.json");
    let bytes = fs::read(&path).unwrap();
    let stale = request(
        &h,
        &p,
        json!({"action":"mutate","snapshot":before["snapshot"],"edit":{"kind":"adopt"}}),
    );
    assert_eq!(stale["kind"], "rejected");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
    v["schemaVersion"] = 2.into();
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let future = fs::read(&path).unwrap();
    assert!(list(&h, &p)["problem"].is_string());
    assert_eq!(
        request(&h, &p, json!({"action":"begin","template":t}))["kind"],
        "rejected"
    );
    assert_eq!(fs::read(path).unwrap(), future);
    h.close_clean();
}

#[test]
fn document_workspace_deposit_restart_old_generation_duplicate_and_close_guard() {
    let base =
        std::env::temp_dir().join(format!("worldbuild-m34-restart-{}", uuid::Uuid::new_v4()));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "재시작 원문".into();
    body["composing"] = true.into();
    let first = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"2","body":body}),
    )["value"]
        .clone();
    let page = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = page["page"]["entries"][0].clone();
    let discard =
        h.work(json!({"kind":"recovery_discard","key":row["row"]["key"],"version":row["version"]}));
    assert_eq!(discard["error"]["code"], "owners_remain");
    body["name"] = "최신 세대".into();
    let latest = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"7","body":body}),
    )["value"]
        .clone();
    assert_eq!(latest["deposited"], true);
    assert_eq!(first["deposited"], true);
    release(&h, &p, &latest, false);
    h.close_clean();
    drop(h);
    let h = Harness::at(base, backend::provider(), true);
    let p = h.open();
    let r = &row["row"];
    let restore = json!({"action":"restore","key":r["key"],"deposit_id":r["depositId"],"digest":r["payloadDigest"]});
    let restored = request(&h, &p, restore.clone())["value"].clone();
    assert_eq!(restored["generation"], "8");
    assert_eq!(restored["body"]["name"], "재시작 원문");
    assert_eq!(restored["body"]["composing"], false, "{restored}");
    assert_eq!(
        request(&h, &p, restore)["error"]["code"],
        "duplicate_conflict"
    );
    let saved = save(&h, &p, &restored, "9", restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed");
    release(&h, &p, &saved, false);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    assert!(
        h.work(json!({"kind":"recovery_page","cursor":null}))["page"]["entries"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    h.close_clean();
}
#[test]
fn document_workspace_cancel_signal_bypasses_queued_worker_and_terminal_is_requeryable() {
    let provider = Arc::new(Provider::new());
    let h = Harness::with_provider(provider.clone());
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "cancel".into();
    let (entered, release_gate) = provider.hold();
    let op=h.submit(json!({"kind":"document_workspace","project":p,"request":{"action":"draft","owner":d["owner"],"generation":"2","body":body,"save":true}}));
    entered.recv_timeout(LIMIT).unwrap();
    let signal = h.call(json!({"action":"document_progress","operation":op,"cancel":true}));
    assert_eq!(signal["requested"], true);
    assert_eq!(
        h.call(json!({"action":"operation","operation":op}))["state"],
        "pending"
    );
    release_gate.send(()).unwrap();
    let result = h.result(&op);
    assert_eq!(result["error"]["code"], "cancelled", "{result}");
    assert_eq!(h.result(&op), result);
    h.ack(&op);
    assert!(!h.root.join("workspace/document-layout.json").exists());
    let saved = save(&h, &p, &d, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    release(&h, &p, &saved, false);
    assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
    h.close_clean();
}
