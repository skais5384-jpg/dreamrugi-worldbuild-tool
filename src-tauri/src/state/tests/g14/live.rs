//! 살아 있는 AppState의 실제 원 결과와 공식 후속 동작을 함께 관측한다.
use super::*;
use crate::data::{
    application::{
        diagnostics::ApplicationError,
        templates::{TemplateMutationExecution, UpdateTemplateInput},
    },
    transaction::PrepareFailPoint as P,
};
use std::{fmt, sync::atomic::AtomicUsize};

#[derive(Debug)]
struct Cause(Arc<()>);
impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.0;
        f.write_str("G14_PRIVATE_CAUSE_CANARY")
    }
}
impl std::error::Error for Cause {}
type Original = (
    UpdateTemplateInput,
    Result<TemplateMutationExecution, ApplicationError>,
);

fn update(p: &str, s: &str, v: &Value, name: &str) -> Value {
    json!({"kind":"update_template","project":p,"session":s,"view":v["view"],"revision":v["content"]["revision"],"edit":{"kind":"name","name":name}})
}
fn owner(h: &Harness, op: &str) -> (Arc<Work>, *const (), Value) {
    let state = h.state.lock();
    let operation = &state.operations[&op.to_owned().try_into().unwrap()];
    let result = operation.result.as_ref().unwrap();
    let original = result
        .original
        .downcast_ref::<Original>()
        .expect("actual G6 result owner");
    assert!(
        original.1.as_ref().unwrap().candidate().is_some(),
        "candidate remains owned independently of disk"
    );
    (
        operation.input.as_ref().unwrap().clone(),
        &*result.original as *const _ as *const (),
        json!(format!(
            "{:?}",
            original.1.as_ref().unwrap().execution.diagnostic()
        )),
    )
}

#[test]
fn g14_guarded_outcomes_recovery_and_retained_owner_lifecycle() {
    for (fault, expected, cleanup, pending, new) in [
        ("prepare", "not_applied", false, false, false),
        ("prepare_cleanup", "not_applied", true, true, false),
        ("not_applied", "not_applied", false, true, false),
        ("rollback", "rolled_back", false, false, false),
        ("rollback_cleanup", "rolled_back", true, true, false),
        ("committed_cleanup", "committed", true, true, true),
        ("recovery_required", "uncertain", false, true, true),
    ] {
        let h = Harness::new();
        let armed = Arc::new(AtomicBool::new(false));
        let commits = Arc::new(AtomicUsize::new(0));
        let first = Arc::new(());
        let second = Arc::new(());
        let (a, c, one, two) = (
            armed.clone(),
            commits.clone(),
            first.clone(),
            second.clone(),
        );
        gates::observe_io(
            h.root.clone(),
            Box::new(move |point, _| {
                if !a.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if point == Point::Commit(C::ManifestRevalidation) {
                    c.fetch_add(1, Ordering::SeqCst);
                }
                let primary = match fault {
                    "prepare" | "prepare_cleanup" => point == Point::Prepare(P::StagedWrite),
                    "not_applied" => point == Point::Commit(C::ManifestRevalidation),
                    "rollback" | "rollback_cleanup" | "recovery_required" => {
                        point == Point::Commit(C::CommittedMarkerWrite)
                    }
                    "committed_cleanup" => point == Point::Commit(C::Cleanup),
                    _ => false,
                };
                let secondary = match fault {
                    "prepare_cleanup" => point == Point::Prepare(P::Cleanup),
                    "rollback_cleanup" => point == Point::Recovery(R::Cleanup),
                    "recovery_required" => point == Point::Recovery(R::RestoreExisting),
                    _ => false,
                };
                if primary {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        Cause(one.clone()),
                    ))
                } else if secondary {
                    Err(io::Error::other(Cause(two.clone())))
                } else {
                    Ok(())
                }
            }),
        );
        let p = h.open();
        let (id, _) = h.template(&p);
        let v = h.read_template(&p, &id);
        let s = h.session(&p, vec![v["view"].clone()], "template");
        let path = h.root.join(format!("templates/{id}.json"));
        let old = fs::read(&path).unwrap();
        let input = update(&p, &s, &v, "G14_PRIVATE_EDIT_CANARY");
        armed.store(true, Ordering::SeqCst);
        let op = h.submit(input.clone());
        let result = h.result(&op);
        assert_eq!(result["disk"], expected, "fixed outcome contract");
        assert_eq!(result["cleanup_failed"], cleanup);
        assert_eq!(result["recovery_required"], pending);
        let (original_input, original_ptr, diagnostic) = owner(&h, &op);
        assert!(
            Arc::strong_count(&first) >= 3,
            "original primary I/O owner retained"
        );
        if matches!(
            fault,
            "prepare_cleanup" | "rollback_cleanup" | "recovery_required"
        ) {
            assert!(
                Arc::strong_count(&second) >= 3,
                "secondary cause separately retained"
            );
        }
        assert!(
            !result.to_string().contains("G14_PRIVATE"),
            "write diagnostics must redact canaries"
        );
        assert!(
            !diagnostic.to_string().contains("G14_PRIVATE"),
            "lower diagnostic must redact canaries"
        );
        let actual = fs::read(&path).unwrap();
        let typed = artifact::decode_template(&actual).unwrap();
        assert_eq!(typed.revision().get(), if new { 2 } else { 1 });
        if !new {
            assert!(actual == old, "confirmed old bytes");
        } else {
            assert!(
                typed.name() == "G14_PRIVATE_EDIT_CANARY",
                "committed candidate name is preserved"
            );
        }
        assert!(h.result(&op) == result, "query does not retry store");
        assert_eq!(
            commits.load(Ordering::SeqCst),
            if fault.starts_with("prepare") { 0 } else { 1 }
        );
        let key = h.call(json!({"action":"operation","operation":op}))["retained"].clone();
        if pending {
            assert_eq!(
                h.call(json!({"action":"project_status","project":p}))["runtime"],
                "Pending"
            );
            let read_blocked = h.read_template(&p, &id);
            assert_eq!(
                read_blocked["kind"], "rejected",
                "pending artifact read is denied"
            );
            let count = commits.load(Ordering::SeqCst);
            let blocked=h.submit(json!({"kind":"create_template","project":p,"name":"G14 next write","presentation":null}));
            assert_eq!(h.result(&blocked)["kind"], "rejected");
            assert_eq!(
                commits.load(Ordering::SeqCst),
                count,
                "pending rejects before M1"
            );
            let blocked_key =
                h.call(json!({"action":"operation","operation":blocked}))["retained"].clone();
            h.ack(&blocked);
            super::super::fix::abandon(&h, &blocked_key);
        }
        armed.store(false, Ordering::SeqCst);
        if pending {
            let recovery = h.control(json!({"kind":"recover","project":p}));
            assert_eq!(recovery["kind"], "control");
            assert!(recovery["error"].is_null());
            assert_eq!(
                h.call(json!({"action":"project_status","project":p}))["runtime"],
                "Ready"
            );
        }
        let recovered = fs::read(&path).unwrap();
        if fault == "recovery_required" {
            assert!(
                recovered == old,
                "official recovery restores uncertain old generation"
            );
        } else {
            assert!(recovered == actual, "recovery preserves confirmed outcome");
        }
        h.ack(&op);
        if key.is_object() {
            let read = h.call(json!({"action":"retained_read","retained":key}));
            assert!(
                read["intent"]
                    .to_string()
                    .contains("G14_PRIVATE_EDIT_CANARY"),
                "explicit typed intent read is permitted"
            );
            let state = h.state.lock();
            let (kept, r) = state
                .retained
                .iter()
                .find(|(_, r)| serde_json::to_value(r.retained).unwrap() == key)
                .unwrap();
            assert!(Arc::ptr_eq(kept, &original_input));
            assert_eq!(&*r.original as *const _ as *const (), original_ptr);
            drop(state);
            if read["g6_clearable"] == true {
                super::super::fix::abandon(&h, &key);
            } else {
                let denied = h.reserve("control");
                let error=h.ipc(json!({"action":"submit","operation":denied,"input":{"kind":"abandon_retained","retained":key}})).unwrap_err();
                assert_eq!(error["code"], "owners_remain");
                assert_eq!(h.result(&denied)["error"]["code"], "owners_remain");
                h.ack(&denied);
                assert!(h.call(json!({"action":"retained_read","retained":key})) == read);
                h.call(json!({"action":"app_shutdown"}));
                super::super::fix::joined(&h, &p);
                assert!(
                    !h.state.poll(),
                    "uncertain/cleanup original prevents normal exit"
                );
                assert!(h.call(json!({"action":"retained_read","retained":key})) == read);
                record(
                    "LIVE",
                    &json!({"fault":fault,"disk":result["disk"],"cleanup":result["cleanup_failed"],"pending":result["recovery_required"],"normal_exit":false,"retained":true,"commits":commits.load(Ordering::SeqCst)}),
                );
                continue;
            }
        }
        if new {
            let stale = h.submit(update(&p, &s, &v, "stale retry"));
            assert_ne!(h.result(&stale)["disk"], "committed");
            let k = h.call(json!({"action":"operation","operation":stale}))["retained"].clone();
            h.ack(&stale);
            super::super::fix::abandon(&h, &k);
            assert!(fs::read(&path).unwrap() == recovered);
        }
        let fresh = h.read_template(&p, &id);
        let fresh_session = h.session(&p, vec![fresh["view"].clone()], "template");
        assert_eq!(
            h.work(update(&p, &fresh_session, &fresh, "explicit fresh update"))["disk"],
            "committed"
        );
        h.close_clean();
        record(
            "LIVE",
            &json!({"fault":fault,"disk":result["disk"],"cleanup":result["cleanup_failed"],"pending":result["recovery_required"],"normal_exit":true,"retained":false,"commits":commits.load(Ordering::SeqCst)}),
        );
    }
}
