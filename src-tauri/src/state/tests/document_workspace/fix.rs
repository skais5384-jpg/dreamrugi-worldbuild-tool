//! AUDIT-001의 실제 transaction 반례를 정상 보관·복구 assertion으로 연결한다.
use super::*;
use crate::data::transaction::test_support::{
    boundary::Point, CommitTestPoint as C, RecoveryTestPoint as R,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn creation_result_snapshot_survives_postcommit_read_failure_and_proof_retry() {
    for external in [false, true] {
        let h = Harness::new();
        let armed = Arc::new(AtomicBool::new(false));
        let arm = armed.clone();
        let root = h.root.clone();
        let captured = Arc::new(Mutex::new(None));
        let capture = captured.clone();
        gates::observe_io(
            h.root.clone(),
            Box::new(move |point, _| {
                if point == Point::Commit(C::Cleanup) && arm.swap(false, Ordering::SeqCst) {
                    let path = fs::read_dir(root.join("documents"))?
                        .next()
                        .unwrap()?
                        .path();
                    let original = fs::read(&path)?;
                    if external {
                        let mut doc: Value = serde_json::from_slice(&original).unwrap();
                        doc["name"] = "external after commit".into();
                        fs::write(&path, serde_json::to_vec(&doc).unwrap())?;
                    }
                    *capture.lock().unwrap() = Some((path, original));
                }
                Ok(())
            }),
        );
        let p = h.open();
        let (t, _) = h.template(&p);
        let d = begin(&h, &p, &t);
        let template_path = h.root.join(format!("templates/{t}.json"));
        let template_bytes = fs::read(&template_path).unwrap();
        let mut body = d["body"].clone();
        body["name"] = "submitted S".into();
        armed.store(true, Ordering::SeqCst);
        let op = h.submit(json!({"kind":"document_workspace","project":p,"request":{
            "action":"draft","owner":d["owner"],"generation":"2","body":body,"save":true}}));
        let original_response = h.result(&op);
        let saved = &original_response["value"];
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        assert_eq!(saved["outcome"]["cleanup_failed"], external);
        let id = saved["outcome"]["artifact"].as_str().unwrap();
        let (path, committed_bytes) = captured.lock().unwrap().take().unwrap();
        let current_bytes = fs::read(&path).unwrap();
        let layout_path = h.root.join("workspace/document-layout.json");
        let layout_bytes = fs::read(&layout_path).unwrap();
        body["name"] = "later raw S+1".into();
        body["composing"] = true.into();
        let input = json!({"action":"deposit","owner":d["owner"],"generation":"3","body":body});
        let store = h.state.recovery.connect().unwrap();
        store.lock().unwrap().fault = Some(crate::data::edit_recovery::error::Stage::Reopen);
        let failed = request(&h, &p, input.clone());
        assert_eq!(failed["kind"], "rejected");
        assert_eq!(failed["input_retained"], true);
        let rejected = request(
            &h,
            &p,
            json!({"action":"release","owner":d["owner"],"generation":"3","discard":false}),
        );
        assert_eq!(rejected["error"]["code"], "no_receipt");
        store.lock().unwrap().fault = None;
        // 파일이 아직 외부 값인 동안에도 S 증거와 S+1 raw 보관은 성공해야 한다.
        let deposit_op = h.submit(json!({"kind":"document_workspace","project":p,"request":input}));
        let deposited = h.result(&deposit_op);
        assert_eq!(deposited["value"]["deposited"], true, "{deposited}");
        assert_eq!(deposited["value"]["body"], body);
        assert_eq!(deposited["value"]["outcome"], saved["outcome"]);
        assert_eq!(
            h.result(&deposit_op),
            deposited,
            "lost response recovers the same result"
        );
        h.ack(&deposit_op);
        assert_eq!(
            h.result(&op),
            original_response,
            "original operation stays unchanged"
        );
        h.ack(&op);
        assert_eq!(
            fs::read(&path).unwrap(),
            current_bytes,
            "deposit never rebases canonical"
        );
        let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
        let row = &rows["page"]["entries"][0]["row"];
        let key: Key = serde_json::from_value(row["key"].clone()).unwrap();
        let record = store
            .lock()
            .unwrap()
            .read(&key, row["depositId"].as_str().unwrap())
            .unwrap();
        let envelope = serde_json::to_value(record.envelope()).unwrap();
        assert_eq!(
            record.payload_digest(),
            row["payloadDigest"].as_str().unwrap()
        );
        assert_eq!(envelope["key"]["generation"], "3");
        assert_eq!(envelope["attempt"]["submittedGeneration"], "2");
        assert_eq!(envelope["attempt"]["operationId"], op);
        assert_eq!(envelope["attempt"]["result"], "committed");
        assert_eq!(envelope["draft"]["document"], id);
        assert_eq!(envelope["draft"]["name"]["value"], "later raw S+1");
        assert_eq!(envelope["draft"]["composing"], true);
        let originals = envelope["originals"].as_array().unwrap();
        assert_eq!(originals.len(), 2);
        let doc = originals.iter().find(|o| o["kind"] == "document").unwrap();
        assert_eq!(doc["artifactId"], id);
        assert_eq!(
            doc["snapshot"].as_str().unwrap().as_bytes(),
            committed_bytes
        );
        assert_eq!(
            doc["sourceDigest"],
            crate::data::edit_recovery::model::digest(&committed_bytes)
        );
        assert_eq!(envelope["attempt"]["candidateDigest"], doc["sourceDigest"]);
        release(&h, &p, &deposited["value"], false);
        let restore = json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]});
        if external {
            assert_eq!(request(&h, &p, restore.clone())["kind"], "rejected");
            assert_eq!(fs::read(&path).unwrap(), current_bytes);
            // 감사 fixture의 외부 writer를 명시적으로 해소한 뒤 공식 recover를 호출한다.
            // 이 조작을 제품의 자동 복구로 구현하지 않는다.
            fs::write(&path, &committed_bytes).unwrap();
            let recovered = h.control(json!({"kind":"recover","project":p}));
            assert!(recovered["error"].is_null(), "{recovered}");
        }
        let restored = request(&h, &p, restore)["value"].clone();
        assert_eq!(restored["document"], id, "{restored}");
        assert_eq!(restored["body"]["composing"], false);
        assert_eq!(restored["body"]["name"]["value"], "later raw S+1");
        let mut next = restored["body"].clone();
        next["name"]["value"] = "normal input after restore".into();
        let result = edit_save(&h, &p, &restored, "5", next);
        assert_eq!(result["outcome"]["disk"], "committed", "{result}");
        edit_release(&h, &p, &result);
        assert_eq!(
            request(&h, &p, json!({"action":"read","document":id}))["value"]["name"],
            "normal input after restore"
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .read(&key, row["depositId"].as_str().unwrap())
                .unwrap()
                .bytes(),
            record.bytes()
        );
        assert_eq!(fs::read(template_path).unwrap(), template_bytes);
        assert_eq!(fs::read(layout_path).unwrap(), layout_bytes);
        assert_eq!(list(&h, &p)["documents"].as_array().unwrap().len(), 1);
        h.close_clean();
    }
}

#[test]
fn creation_uncertain_candidate_is_deposited_without_claiming_commit() {
    let h = Harness::new();
    let armed = Arc::new(AtomicBool::new(false));
    let arm = armed.clone();
    gates::observe_io(
        h.root.clone(),
        Box::new(move |point, _| {
            if arm.load(Ordering::SeqCst)
                && matches!(
                    point,
                    Point::Commit(C::CommittedMarkerWrite) | Point::Recovery(R::RemoveNew)
                )
            {
                return Err(std::io::Error::other("creation uncertain fixture"));
            }
            Ok(())
        }),
    );
    let p = h.open();
    let (t, _) = h.template(&p);
    let d = begin(&h, &p, &t);
    let mut body = d["body"].clone();
    body["name"] = "uncertain S".into();
    armed.store(true, Ordering::SeqCst);
    let saved = save(&h, &p, &d, "2", body.clone());
    assert_eq!(saved["outcome"]["disk"], "uncertain", "{saved}");
    let id = saved["outcome"]["artifact"].clone();
    body["name"] = "uncertain followup".into();
    let dep = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"3","body":body}),
    );
    assert_eq!(dep["value"]["deposited"], true, "{dep}");
    assert_eq!(dep["value"]["outcome"], saved["outcome"]);
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &rows["page"]["entries"][0]["row"];
    let key: Key = serde_json::from_value(row["key"].clone()).unwrap();
    let store = h.state.recovery.connect().unwrap();
    let record = store
        .lock()
        .unwrap()
        .read(&key, row["depositId"].as_str().unwrap())
        .unwrap();
    let envelope = serde_json::to_value(record.envelope()).unwrap();
    assert_eq!(envelope["attempt"]["result"], "uncertain");
    assert_eq!(envelope["attempt"]["submittedGeneration"], "2");
    assert_eq!(envelope["draft"]["document"], id);
    assert_eq!(envelope["originals"].as_array().unwrap().len(), 2);
    let repeat = request(
        &h,
        &p,
        json!({"action":"draft","owner":d["owner"],"generation":"4","body":body,"save":true}),
    );
    assert_eq!(repeat["error"]["code"], "recovery_rejected");
    assert_eq!(repeat["input_retained"], true);
    let dep = request(
        &h,
        &p,
        json!({"action":"deposit","owner":d["owner"],"generation":"4","body":dep["value"]["body"]}),
    );
    assert_eq!(dep["value"]["deposited"], true);
    release(&h, &p, &dep["value"], false);
    armed.store(false, Ordering::SeqCst);
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");
    assert!(!h
        .root
        .join(format!("documents/{}.json", id.as_str().unwrap()))
        .exists());
    let restore = request(
        &h,
        &p,
        json!({"action":"edit_restore","key":row["key"],"deposit_id":row["depositId"],"digest":row["payloadDigest"]}),
    );
    assert_eq!(
        restore["kind"], "rejected",
        "rolled back candidate is not an existing source"
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .read(&key, row["depositId"].as_str().unwrap())
            .unwrap()
            .bytes(),
        record.bytes()
    );
    h.close_clean();
}

#[test]
fn document_refresh_real_rejection_then_recovery_success_keeps_saved_evidence() {
    let h = Harness::new();
    let armed = Arc::new(AtomicBool::new(false));
    let arm = armed.clone();
    gates::observe_io(
        h.root.clone(),
        Box::new(move |point, _| {
            if point == Point::Commit(C::Cleanup) && arm.swap(false, Ordering::SeqCst) {
                return Err(std::io::Error::other(
                    "postcommit read blocked until explicit recovery",
                ));
            }
            Ok(())
        }),
    );
    let p = h.open();
    let (t, _) = h.template(&p);
    let id = create(&h, &p, &t, "base");
    let d = edit_begin(&h, &p, &id);
    let mut body = d["body"].clone();
    body["name"] = json!({"intent":"set","value":"submitted S"});
    armed.store(true, Ordering::SeqCst);
    let saved = edit_save(&h, &p, &d, "2", body.clone());
    assert_eq!(saved["outcome"]["disk"], "committed");
    assert_eq!(saved["problem"], "SavedReadRequired");
    let failed = request(&h, &p, json!({"action":"edit_refresh","owner":d["owner"]}));
    assert_eq!(failed["error"]["code"], "runtime_rejected");
    let recovered = h.control(json!({"kind":"recover","project":p}));
    assert!(recovered["error"].is_null(), "{recovered}");
    let canary = block_latest_target(&h, "document", &id);
    let unproven = request(&h, &p, json!({"action":"edit_refresh","owner":d["owner"]}));
    assert_eq!(unproven["error"]["code"], "recovery_rejected");
    assert_eq!(unproven["input_retained"], true);
    unblock_latest_target(&canary);
    let refreshed = request(&h, &p, json!({"action":"edit_refresh","owner":d["owner"]}));
    assert_eq!(refreshed["value"]["read"]["name"], "submitted S");
    assert_eq!(refreshed["value"]["outcome"], saved["outcome"]);
    assert_eq!(refreshed["value"]["body"], body);
    assert_eq!(refreshed["value"]["generation"], "2");
    assert!(refreshed["value"]["problem"].is_null());
    println!(
        "M35_FIX_REFRESH_DTO {}",
        json!({"begin":d,"saved":saved,"refresh":failed,"success":refreshed["value"]})
    );
    body["name"]["value"] = "normal after refresh".into();
    let next = edit_save(&h, &p, &refreshed["value"], "3", body);
    assert_eq!(next["saved_generation"], "3", "{next}");
    assert!(next["problem"].is_null(), "{next}");
    assert_eq!(next["outcome"]["disk"], "committed");
    edit_release(&h, &p, &next);
    h.close_clean();
}
