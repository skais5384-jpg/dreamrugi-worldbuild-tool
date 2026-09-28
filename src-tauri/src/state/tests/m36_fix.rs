//! FIX-001: 실제 기존 문서 저장의 rollback 진단과 최신 raw/제출 attempt를 구별한다.
use super::*;

#[test]
fn m36_fix_rollback_diagnostic_deposit_retry_and_next_save() {
    use crate::data::transaction::test_support::{boundary::Point, CommitTestPoint as C};
    use std::sync::atomic::{AtomicBool, Ordering};
    let h = Harness::new();
    let armed = Arc::new(AtomicBool::new(false));
    let arm = armed.clone();
    gates::observe_io(
        h.root.clone(),
        Box::new(move |point, _| {
            if arm.load(Ordering::SeqCst) && point == Point::Commit(C::CommittedMarkerWrite) {
                return Err(std::io::Error::from_raw_os_error(5));
            }
            Ok(())
        }),
    );
    let p = h.open();
    let (t, _) = h.template(&p);
    let id = empty_document(&h, &p, &t, "before");
    let path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&path).unwrap();
    let d = request(&h, &p, json!({"action":"edit_begin","document":id}))["value"].clone();
    let mut raw = d["body"].clone();
    raw["name"] = json!({"intent":"set","value":"PRIVATE_SUBMITTED_15"});
    armed.store(true, Ordering::SeqCst);
    let failed = request(
        &h,
        &p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":"15","body":raw,"save":true}),
    )["value"]
        .clone();
    assert_eq!(failed["outcome"]["disk"], "rolled_back", "{failed}");
    assert_eq!(failed["problem"], "SaveFailed");
    assert_eq!(fs::read(&path).unwrap(), before);
    let diagnostic = &failed["outcome"]["diagnostic"];
    let primary = &diagnostic["failures"][0];
    assert_eq!(primary["role"], "Primary");
    assert_eq!(primary["stage"], "Commit(WriteCommittedMarker)");
    assert_eq!(primary["osCode"], 5);
    assert!(primary["ioKind"].is_string());
    assert!(primary["transactionId"]
        .as_str()
        .unwrap()
        .starts_with("txn-"));
    assert!(diagnostic["operationId"].is_string());
    assert!(diagnostic["observedAtUtc"].as_str().unwrap().ends_with('Z'));
    assert!(!diagnostic.to_string().contains("PRIVATE_"));
    assert!(!diagnostic
        .to_string()
        .contains(&h.root.to_string_lossy().to_string()));
    // 오류 뒤 최신 raw는 제출된15세대 attempt를 덮어쓰지 않는다.
    raw["name"] = json!({"intent":"set","value":"PRIVATE_LATEST_16"});
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"16","body":raw}),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true);
    assert_eq!(deposited["outcome"]["diagnostic"], *diagnostic);
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let store = h.state.recovery.connect().unwrap();
    let rows = rows["page"]["entries"].as_array().unwrap();
    let row = &rows
        .iter()
        .find(|r| r["row"]["key"]["generation"] == "16")
        .unwrap()["row"];
    let key: Key = serde_json::from_value(row["key"].clone()).unwrap();
    let bytes = store
        .lock()
        .unwrap()
        .read(&key, row["depositId"].as_str().unwrap())
        .unwrap()
        .bytes()
        .to_vec();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    let e = &envelope["envelope"];
    assert_eq!(e["draft"]["name"]["value"], "PRIVATE_LATEST_16");
    assert_eq!(e["attempt"]["submittedGeneration"], "15");
    assert_eq!(e["attempt"]["operationId"], diagnostic["operationId"]);
    assert_eq!(e["attempt"]["transactionId"], primary["transactionId"]);
    armed.store(false, Ordering::SeqCst);
    let resumed = request(
        &h,
        &p,
        json!({"action":"edit_retry","owner":d["owner"],"generation":"16","body":raw}),
    )["value"]
        .clone();
    assert!(resumed["problem"].is_null(), "{resumed}");
    assert_eq!(resumed["body"], raw);
    let saved = request(&h, &p, json!({"action":"edit_draft","owner":d["owner"],"generation":resumed["generation"],"body":raw,"save":true}))["value"].clone();
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert!(saved["problem"].is_null());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["name"],
        "PRIVATE_LATEST_16"
    );
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":saved["generation"]})
        )["value"]["kind"],
        "released"
    );
    h.close_clean();
}
