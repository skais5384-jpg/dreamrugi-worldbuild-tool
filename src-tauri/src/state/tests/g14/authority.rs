//! source/scan 실패와 협조적 writer 순서를 실제 registry 및 project worker에서 확인한다.
use super::*;
use std::sync::atomic::AtomicUsize;

fn count_prepare(h: &Harness) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    gates::observe_io(
        h.root.clone(),
        Box::new(move |point, _| {
            if point
                == Point::Prepare(
                    crate::data::transaction::PrepareFailPoint::TransactionDirectoryAllocated,
                )
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }),
    );
    count
}
fn finish_original(h: &Harness, project: &str, op: &str) {
    let key = h.call(json!({"action":"operation","operation":op}))["retained"].clone();
    h.ack(op);
    if key.is_object() {
        let read = h.call(json!({"action":"retained_read","retained":key}));
        if read["g6_clearable"] == true {
            super::super::fix::abandon(h, &key);
        } else {
            let denied = h.reserve("control");
            assert_eq!(h.ipc(json!({"action":"submit","operation":denied,"input":{"kind":"abandon_retained","retained":key}})).unwrap_err()["code"],"owners_remain");
            assert_eq!(h.result(&denied)["error"]["code"], "owners_remain");
            h.ack(&denied);
            h.call(json!({"action":"app_shutdown"}));
            super::super::fix::joined(h, project);
            assert!(
                !h.state.poll(),
                "G7 intent is not eligible for G6 explicit abandonment"
            );
            assert!(
                h.call(json!({"action":"retained_read","retained":key})) == read,
                "original survives query/ack/end/join"
            );
            return;
        }
    }
    h.close_clean();
}

#[test]
fn g14_guarded_corrupt_future_and_scan_io_errors_do_not_become_no_reference() {
    for mode in ["corrupt", "future", "wrong_id", "directory"] {
        let h = Harness::new();
        let count = count_prepare(&h);
        let p = h.open();
        let (id, _) = h.template(&p);
        let read = h.read_template(&p, &id);
        let session = h.session(&p, vec![read["view"].clone()], "template");
        fs::create_dir_all(h.root.join("documents")).unwrap();
        let did = "77777777-7777-4777-8777-000000000001";
        let path = h.root.join(format!("documents/{did}.json"));
        match mode {
            "directory" => fs::create_dir(&path).unwrap(),
            "corrupt" => fs::write(&path, b"{G14_PRIVATE_CORRUPT_CANARY").unwrap(),
            _ => {
                let (_, bytes) =
                    crate::data::application::composite::tests::guarded_fixture_bytes();
                let mut raw = String::from_utf8(bytes).unwrap();
                if mode == "future" {
                    raw = raw.replace("\"schemaVersion\":1", "\"schemaVersion\":99");
                }
                // filename/body ID 불일치는 다른 source를 추측하여 선택하지 않아야 한다.
                fs::write(&path, raw).unwrap();
            }
        }
        let before = disk(&h.base);
        let baseline = count.load(Ordering::SeqCst);
        let invalid = h.work(json!({"kind":"read_document","project":p,"document":did}));
        assert_eq!(invalid["kind"], "rejected");
        let op=h.submit(json!({"kind":"tombstone_template","project":p,"session":session,"view":read["view"],"revision":"1"}));
        let result = h.result(&op);
        assert_eq!(result["disk"], "not_attempted");
        assert_eq!(result["diagnostic"]["category"], "RepositoryRejected");
        assert_eq!(
            count.load(Ordering::SeqCst),
            baseline,
            "no candidate admission reaches M1 prepare"
        );
        assert!(
            disk(&h.base) == before,
            "failure evidence and source bytes preserved"
        );
        assert!(
            !result.to_string().contains("G14_PRIVATE"),
            "guarded error is redacted"
        );
        finish_original(&h, &p, &op);
    }
}

#[test]
fn g14_guarded_document_create_and_tombstone_are_serialized_in_both_orders() {
    for document_first in [true, false] {
        let provider = Arc::new(Provider::new());
        let h = Harness::with_provider(provider.clone());
        let count = count_prepare(&h);
        let p = h.open();
        let (id, _) = h.template(&p);
        let read = h.read_template(&p, &id);
        let session = h.session(&p, vec![read["view"].clone()], "template");
        let create = json!({"kind":"create_document","project":p,"view":read["view"],"name":"ordered document"});
        let tombstone = json!({"kind":"tombstone_template","project":p,"session":session,"view":read["view"],"revision":"1"});
        let baseline = count.load(Ordering::SeqCst);
        let (entered, release) = provider.hold();
        let first = h.submit(if document_first {
            create.clone()
        } else {
            tombstone.clone()
        });
        entered.recv_timeout(LIMIT).unwrap();
        let second = h.submit(if document_first { tombstone } else { create });
        assert!(h.call(json!({"action":"operation","operation":second}))["state"] == "pending");
        release.send(()).unwrap();
        let result = h.result(&first);
        assert_eq!(result["disk"], "committed");
        let rejected = h.result(&second);
        assert_ne!(rejected["disk"], "committed");
        assert_eq!(
            count.load(Ordering::SeqCst),
            baseline + 1,
            "later request must recheck without preparing"
        );
        let current = h.read_template(&p, &id);
        if document_first {
            assert_eq!(current["content"]["lifecycle"], "Active");
            let doc =
                h.work(json!({"kind":"read_document","project":p,"document":result["artifact"]}));
            assert_eq!(doc["content"]["template"], id);
            assert_eq!(rejected["disk"], "not_attempted");
        } else {
            assert_eq!(current["content"]["lifecycle"], "Deleted");
            assert!(
                !h.root.join("documents").exists(),
                "inactive Template must not create Document namespace"
            );
        }
        h.ack(&first);
        finish_original(&h, &p, &second);
    }
}

#[test]
fn g14_guarded_external_template_change_after_preparation_is_preserved() {
    for lifecycle in [false, true] {
        let provider = Arc::new(Provider::new());
        let h = Harness::with_provider(provider.clone());
        let count = count_prepare(&h);
        let p = h.open();
        let (id, _) = h.template(&p);
        let read = h.read_template(&p, &id);
        let path = h.root.join(format!("templates/{id}.json"));
        let old = fs::read(&path).unwrap();
        let baseline = count.load(Ordering::SeqCst);
        let (entered, release) = provider.hold();
        let op=h.submit(json!({"kind":"create_document","project":p,"view":read["view"],"name":"external race"}));
        entered.recv_timeout(LIMIT).unwrap();
        // 고정된 최소 fixture만 외부 writer로 변경한다. 실제 새 bytes를 관측하기 전에 되돌리지 않는다.
        let text = String::from_utf8(old)
            .unwrap()
            .replace("\"revision\": 1", "\"revision\": 2");
        let text = if lifecycle {
            text.replace("\"lifecycle\": \"active\"", "\"lifecycle\": \"deleted\"")
        } else {
            text
        };
        let external = text.into_bytes();
        let typed = artifact::decode_template(&external).unwrap();
        assert_eq!(typed.revision().get(), 2);
        fs::write(&path, &external).unwrap();
        release.send(()).unwrap();
        let rejected = h.result(&op);
        assert_ne!(rejected["disk"], "committed");
        assert_eq!(count.load(Ordering::SeqCst), baseline);
        assert!(
            fs::read(&path).unwrap() == external,
            "external bytes observed unchanged before teardown"
        );
        finish_original(&h, &p, &op);
    }
}
