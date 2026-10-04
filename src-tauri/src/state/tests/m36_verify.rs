//! VERIFY-001: 같은 guarded 요청/Store를 쓰는 호환·실패·수동 fixture 근거.
use super::*;
use crate::data::edit_recovery::model::{digest, Key};
use std::path::Path;

#[path = "m36_fix.rs"]
mod fix;

fn request(h: &Harness, p: &str, r: Value) -> Value {
    h.work(json!({"kind":"document_workspace","project":p,"request":r}))
}
fn write_export(name: &str, bytes: &[u8]) {
    if let Some(root) = std::env::var_os("WB_M36_VERIFY_EXPORT") {
        let path = PathBuf::from(root).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
fn export_tree(root: &Path, relative: &Path) {
    let current = root.join(relative);
    if !current.exists() {
        return;
    }
    for entry in fs::read_dir(current).unwrap() {
        let path = entry.unwrap().path();
        let rel = path.strip_prefix(root).unwrap();
        if path.is_dir() {
            export_tree(root, rel);
        } else {
            write_export(
                &format!("manual-project/{}", rel.to_string_lossy()),
                &fs::read(path).unwrap(),
            );
        }
    }
}
#[test]
fn m36_verify_export_current_product_samples_and_manual_project() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "M3-6 입력 확인".into();
    let mut n = number();
    n["label"] = "필수 숫자 10~20".into();
    n["required"] = true.into();
    n["default"] = json!({"intent":"unset"});
    n["configuration"] = json!({"kind":"number","minimum":"10","maximum":"20"});
    let mut r = n.clone();
    r["id"] = "new:cccccccc-cccc-4ccc-8ccc-cccccccccccc".into();
    r["label"] = "서식 본문".into();
    r["required"] = false.into();
    r["configuration"] = json!({"kind":"rich_text"});
    let mut optional = n.clone();
    optional["id"] = "new:eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".into();
    optional["label"] = "선택 숫자".into();
    optional["required"] = false.into();
    optional["configuration"] = json!({"kind":"number"});
    body["fields"] = json!([r, n, optional]);
    body["sections"] = json!([{"id":"dddddddd-dddd-4ddd-8ddd-dddddddddddd","title":"원페이지 컨셉","beforeField":"new:cccccccc-cccc-4ccc-8ccc-cccccccccccc"},{"id":"ffffffff-ffff-4fff-8fff-ffffffffffff","title":"배경 상황","beforeField":"new:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"}]);
    let saved = submit(&h, &p, &s, "2", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    let t = saved["artifact"].as_str().unwrap();
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let template = h.read_template(&p, t);
    let order = template["content"]["fieldOrder"].as_array().unwrap();
    let ast = json!({"kind":"root","children":[{"kind":"heading","level":2,"children":[{"kind":"text","text":"서식 샘플","marks":["bold"]}]},{"kind":"paragraph","children":[{"kind":"text","text":"문장 중간에 한글을 입력해 주세요."}]},{"kind":"bulletList","children":[{"kind":"listItem","children":[{"kind":"paragraph","children":[{"kind":"text","text":"목록 항목"}]}]}]},{"kind":"taskList","children":[{"kind":"taskItem","checked":true,"children":[{"kind":"paragraph","children":[{"kind":"text","text":"체크 항목"}]}]}]}]});
    let mut ids = vec![];
    for name in ["입력 시험 A", "탭 왕복 B"] {
        let d = request(&h, &p, json!({"action":"begin","template":t}))["value"].clone();
        let b = json!({"name":name,"parent":null,"composing":false,"fields":[{"field":order[0],"value":{"intent":"set","value":{"kind":"rich_text","content":ast}}},{"field":order[1],"value":{"intent":"set","value":{"kind":"number_unknown","previous_raw":"17"}}}]});
        let done = request(
            &h,
            &p,
            json!({"action":"draft","owner":d["owner"],"generation":"2","body":b,"save":true}),
        )["value"]
            .clone();
        assert_eq!(done["outcome"]["disk"], "committed", "{done}");
        ids.push(done["outcome"]["artifact"].as_str().unwrap().to_owned());
        assert_eq!(
            request(
                &h,
                &p,
                json!({"action":"release","owner":d["owner"],"generation":"2","discard":false})
            )["value"]["kind"],
            "released"
        );
    }
    write_export(
        "compat/template3.json",
        &fs::read(h.root.join(format!("templates/{t}.json"))).unwrap(),
    );
    write_export(
        "compat/document2.json",
        &fs::read(h.root.join(format!("documents/{}.json", ids[0]))).unwrap(),
    );
    let d = request(&h, &p, json!({"action":"edit_begin","document":ids[0]}))["value"].clone();
    let mut b = d["body"].clone();
    b["fields"] = json!([{"field":order[1],"value":{"intent":"set","value":{"kind":"number_unknown","previous_raw":"21"}}}]);
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":d["owner"],"generation":"2","body":b}),
    );
    assert_eq!(dep["value"]["deposited"], true, "{dep}");
    stage_latest_as_legacy_archive(&h, &dep["value"]["owner"]);
    let rows = h.work(json!({"kind":"recovery_page","cursor":null}));
    let row = &rows["page"]["entries"][0]["row"];
    let key: Key = serde_json::from_value(row["key"].clone()).unwrap();
    let store = h.state.recovery.connect().unwrap();
    let record = store
        .lock()
        .unwrap()
        .read(&key, row["depositId"].as_str().unwrap())
        .unwrap();
    write_export("compat/recovery2.json", record.bytes());
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":"2"})
        )["value"]["kind"],
        "released"
    );
    h.close_clean();
    for namespace in ["templates", "documents", "workspace"] {
        export_tree(&h.root, Path::new(namespace));
    }
    write_export(
        "fixture-ids.json",
        &serde_json::to_vec_pretty(&json!({"template":t,"documents":ids,"fields":order})).unwrap(),
    );
}

fn empty_document(h: &Harness, p: &str, t: &str, name: &str) -> String {
    let d = request(h, p, json!({"action":"begin","template":t}))["value"].clone();
    let done=request(h,p,json!({"action":"draft","owner":d["owner"],"generation":"2","body":{"name":name,"parent":null,"fields":[],"composing":false},"save":true}))["value"].clone();
    assert_eq!(done["outcome"]["disk"], "committed", "{done}");
    assert_eq!(
        request(
            h,
            p,
            json!({"action":"release","owner":d["owner"],"generation":"2","discard":false})
        )["value"]["kind"],
        "released"
    );
    done["outcome"]["artifact"].as_str().unwrap().into()
}
fn old_header(path: &Path, version: u64) -> Vec<u8> {
    let mut wire: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    wire["schemaVersion"] = version.into();
    wire.as_object_mut().unwrap().remove("sections");
    wire["future"] = json!("RAW_LEXEME");
    let raw = serde_json::to_string(&wire)
        .unwrap()
        .replace("\"RAW_LEXEME\"", "1E100")
        .into_bytes();
    fs::write(path, &raw).unwrap();
    raw
}
fn format(h: &Harness, p: &str, kind: &str, id: &str, restore: Option<&str>) -> Value {
    let info = request(
        h,
        p,
        json!({"action":"format_inspect","kind":kind,"artifact":id}),
    )["value"]
        .clone();
    assert_eq!(info["kind"], "format", "{info}");
    request(
        h,
        p,
        json!({"action":"format_change","kind":kind,"artifact":id,"source":info["source"],"restore":restore}),
    )
}
fn edit_name(h: &Harness, p: &str, id: &str, name: &str) {
    let d = request(h, p, json!({"action":"edit_begin","document":id}))["value"].clone();
    assert_eq!(d["kind"], "editing", "{d}");
    let generation = (d["generation"].as_str().unwrap().parse::<u64>().unwrap() + 1).to_string();
    let mut b = d["body"].clone();
    b["name"] = json!({"intent":"set","value":name});
    let saved = request(
        h,
        p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":generation,"body":b,"save":true}),
    );
    assert_eq!(saved["value"]["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        request(
            h,
            p,
            json!({"action":"edit_release","owner":d["owner"],"generation":generation})
        )["value"]["kind"],
        "released"
    );
    assert_eq!(
        request(h, p, json!({"action":"read","document":id}))["value"]["name"],
        name
    );
}
#[test]
fn m36_verify_format_uncertain_upgrade_restore_cold_recovery_then_edit_save() {
    use crate::data::transaction::test_support::{
        boundary::Point, CommitTestPoint as C, RecoveryTestPoint as R,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    if let Some(base) = std::env::var_os("WB_M36_UNCERTAIN_CHILD") {
        let base = PathBuf::from(base);
        let restore_case = std::env::var("WB_M36_RESTORE_CASE").unwrap() == "true";
        let h = Harness::at(base.clone(), backend::provider(), false);
        let armed = Arc::new(AtomicBool::new(false));
        let arm = armed.clone();
        let hits = Arc::new(Mutex::new(vec![]));
        let observed = hits.clone();
        gates::observe_io(
            h.root.clone(),
            Box::new(move |point, _| {
                if arm.load(Ordering::SeqCst)
                    && matches!(
                        point,
                        Point::Commit(C::CommittedMarkerWrite)
                            | Point::Recovery(R::RestoreExisting)
                    )
                {
                    observed.lock().unwrap().push(format!("{point:?}"));
                    return Err(std::io::Error::other(
                        "VERIFY deterministic commit/rollback failure",
                    ));
                }
                Ok(())
            }),
        );
        let p = h.open();
        let (t, _) = h.template(&p);
        let id = empty_document(&h, &p, &t, "전환 전");
        let path = h.root.join(format!("documents/{id}.json"));
        let original = old_header(&path, 1);
        let target = if restore_case {
            let up = format(&h, &p, "document", &id, None);
            assert_eq!(up["disk"], "committed", "{up}");
            edit_name(&h, &p, &id, "복원 전 최신 내용");
            let legacy = h
                .root
                .join(format!(".worldbuild/format-history/document-{id}"));
            fs::create_dir_all(&legacy).unwrap();
            fs::write(
                legacy.join(format!("{}.json", digest(&original))),
                &original,
            )
            .unwrap();
            current_policy_fixture(&original, &format!("documents/{id}.json"))
        } else {
            crate::data::artifact::transition_format(
                &original,
                &crate::data::project_relative_path::ProjectRelativePath::parse(&format!(
                    "documents/{id}.json"
                ))
                .unwrap(),
            )
            .unwrap()
        };
        let before = fs::read(&path).unwrap();
        armed.store(true, Ordering::SeqCst);
        let restore_hash = digest(&original);
        let failed = format(
            &h,
            &p,
            "document",
            &id,
            restore_case.then_some(restore_hash.as_str()),
        );
        assert_eq!(failed["disk"], "uncertain", "{failed}");
        assert_eq!(failed["recovery_required"], true, "{failed}");
        assert_eq!(fs::read(&path).unwrap(), target);
        let history = h
            .root
            .join(format!(".worldbuild/content-versions/document-{id}"));
        let preserved = fs::read_dir(&history)
            .unwrap()
            .map(|entry| {
                let value: Value =
                    serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap();
                value["content"].as_str().unwrap().as_bytes().to_vec()
            })
            .collect::<Vec<_>>();
        assert!(
            preserved.contains(&before),
            "exact pre-write content must be preserved"
        );
        assert!(preserved.len() <= 10);
        let journals = fs::read_dir(h.root.join(".worldbuild/transactions"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(journals.len(), 1);
        let journal = journals[0].path();
        assert!(journal.join("manifest.json").exists());
        assert!(journal.join("state.json").exists());
        let manifest: Value =
            serde_json::from_slice(&fs::read(journal.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["operations"].as_array().unwrap().len(),
            1,
            "single artifact format transaction"
        );
        let evidence = json!({"restore":restore_case,"before_sha256":digest(&before),"candidate_sha256":digest(&target),"response":failed,"points":hits.lock().unwrap().clone(),"manifest":manifest});
        write_export(
            &format!("v2/uncertain-{restore_case}.json"),
            &serde_json::to_vec_pretty(&evidence).unwrap(),
        );
        fs::write(
            base.join("verify-oracle.json"),
            serde_json::to_vec(&json!({"document":id,"before":before,"history":history})).unwrap(),
        )
        .unwrap();
        std::process::exit(0);
    }
    for restore_case in [false, true] {
        let base =
            std::env::temp_dir().join(format!("worldbuild-m36-cold-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","state::tests::workspace::verify::m36_verify_format_uncertain_upgrade_restore_cold_recovery_then_edit_save","--nocapture"])
            .env("WB_M36_UNCERTAIN_CHILD",&base).env("WB_M36_RESTORE_CASE",restore_case.to_string()).output().unwrap();
        assert!(
            child.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        let oracle: Value =
            serde_json::from_slice(&fs::read(base.join("verify-oracle.json")).unwrap()).unwrap();
        let id = oracle["document"].as_str().unwrap().to_owned();
        let before: Vec<u8> = serde_json::from_value(oracle["before"].clone()).unwrap();
        let path = base.join(format!("project/documents/{id}.json"));
        let h = Harness::at(base, backend::provider(), true);
        let p = h.open();
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "cold recovery rolls back undecided transaction"
        );
        // Cold rollback preserves the exact pre-write state in bounded versions.
        // Legacy format-history is not the new archive for every format write.
        let versions = fs::read_dir(
            h.root
                .join(format!(".worldbuild/content-versions/document-{id}")),
        )
        .unwrap()
        .map(|entry| {
            let value: Value =
                serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap();
            value["content"].as_str().unwrap().as_bytes().to_vec()
        })
        .collect::<Vec<_>>();
        assert!(versions.contains(&before));
        assert!(versions.len() <= 10);
        if !restore_case {
            let up = format(&h, &p, "document", &id, None);
            assert_eq!(up["disk"], "committed", "{up}");
        }
        edit_name(&h, &p, &id, "복구 뒤 정상 저장");
        assert!(String::from_utf8(fs::read(&path).unwrap())
            .unwrap()
            .contains("1E100"));
        h.close_clean();
    }
}

#[test]
fn m36_verify_mixed_versions_and_dirty_owner_format_gate_preserve_sources() {
    for old_template in [false, true] {
        let h = Harness::new();
        let p = h.open();
        let (t, creation_session) = h.template(&p);
        let ended = h.control(json!({"kind":"session_control","project":p,"session":creation_session,"control":"end"}));
        assert!(ended["error"].is_null(), "{ended}");
        let view = h.read_template(&p, &t);
        let whole = begin(&h, &p, view["view"].clone());
        let mut template_body = content(&h, &p, &whole)["body"].clone();
        let mut n = number();
        n["default"] = json!({"intent":"unset"});
        if !old_template {
            n["configuration"] = json!({"kind":"number","minimum":"10","maximum":"20"});
            template_body["sections"] = json!([{"id":"dddddddd-dddd-4ddd-8ddd-dddddddddddd","title":"혼합 구조","beforeField":n["id"]}]);
        }
        template_body["fields"] = json!([n]);
        let saved = submit(&h, &p, &whole, "2", &template_body, "save");
        assert_eq!(saved["phase"], "saved", "{saved}");
        assert!(release(&h, &p, &saved, false)["error"].is_null());
        let field = h.read_template(&p, &t)["content"]["fieldOrder"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let id = empty_document(&h, &p, &t, "혼합 버전");
        let editing =
            request(&h, &p, json!({"action":"edit_begin","document":id}))["value"].clone();
        let mut b = editing["body"].clone();
        b["fields"] = json!([{"field":field,"value":{"intent":"set","value":if old_template {json!({"kind":"number_unknown","previous_raw":"17"})} else {json!({"kind":"number","value":"17"})}}}]);
        let saved = request(
            &h,
            &p,
            json!({"action":"edit_draft","owner":editing["owner"],"generation":"2","body":b,"save":true}),
        );
        assert_eq!(saved["value"]["outcome"]["disk"], "committed", "{saved}");
        assert_eq!(
            request(
                &h,
                &p,
                json!({"action":"edit_release","owner":editing["owner"],"generation":"2"})
            )["value"]["kind"],
            "released"
        );
        let tp = h.root.join(format!("templates/{t}.json"));
        let dp = h.root.join(format!("documents/{id}.json"));
        let (kind, target, path, version) = if old_template {
            ("template", t.as_str(), tp.as_path(), 2)
        } else {
            ("document", id.as_str(), dp.as_path(), 1)
        };
        let legacy = old_header(path, version);
        let read = request(&h, &p, json!({"action":"read","document":id}));
        assert_eq!(read["value"]["kind"], "read", "{read}");
        assert_eq!(
            read["value"]["schema"],
            artifact::DOCUMENT_SCHEMA_VERSION.get()
        );
        let original = fs::read(path).unwrap();
        assert_eq!(
            original,
            current_policy_fixture(&legacy, &format!("{kind}s/{target}.json"))
        );
        let d = request(&h, &p, json!({"action":"edit_begin","document":id}))["value"].clone();
        let mut raw = d["body"].clone();
        raw["name"] = json!({"intent":"set","value":"보존할 dirty 초안"});
        let dirty = request(
            &h,
            &p,
            json!({"action":"edit_draft","owner":d["owner"],"generation":"2","body":raw,"save":false}),
        );
        assert_eq!(dirty["value"]["body"], raw, "{dirty}");
        let denied = format(&h, &p, kind, target, None);
        assert_eq!(denied["kind"], "rejected", "{denied}");
        assert_eq!(denied["error"]["code"], "owners_remain", "{denied}");
        assert_eq!(fs::read(path).unwrap(), original);
        let dep = request(
            &h,
            &p,
            json!({"action":"edit_deposit","owner":d["owner"],"generation":"2","body":raw}),
        );
        assert_eq!(dep["value"]["deposited"], true, "{dep}");
        assert_eq!(dep["value"]["body"], raw);
        assert_eq!(
            request(
                &h,
                &p,
                json!({"action":"edit_release","owner":d["owner"],"generation":"2"})
            )["value"]["kind"],
            "released"
        );
        let view = h.read_template(&p, &t);
        let template_owner = begin(&h, &p, view["view"].clone());
        let mut template_raw = content(&h, &p, &template_owner)["body"].clone();
        template_raw["name"] = "보존할 Template 초안".into();
        let template_deposit = submit(&h, &p, &template_owner, "2", &template_raw, "deposit");
        assert!(!template_deposit["receipt"].is_null(), "{template_deposit}");
        let template_denied = format(&h, &p, kind, target, None);
        assert_eq!(
            template_denied["error"]["code"], "owners_remain",
            "{template_denied}"
        );
        assert_eq!(content(&h, &p, &template_deposit)["body"], template_raw);
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(release(&h, &p, &template_deposit, false)["error"].is_null());
        let up = format(&h, &p, kind, target, None);
        // Normal admission already converted the header. An explicit duplicate
        // upgrade is rejected rather than manufacturing another format write.
        assert_eq!(up["disk"], "not_applied", "{up}");
        assert_eq!(up["error"]["code"], "save_rejected", "{up}");
        assert_eq!(fs::read(path).unwrap(), original);
        edit_name(&h, &p, &id, "혼합 버전 정상 저장");
        let persisted: Value = serde_json::from_slice(&fs::read(&dp).unwrap()).unwrap();
        assert_eq!(
            persisted["fieldValues"][&field]["kind"],
            if old_template {
                "numberUnknown"
            } else {
                "number"
            }
        );
        if !old_template {
            assert_eq!(persisted["fieldValues"][&field]["value"], "17");
        }
        assert!(String::from_utf8(fs::read(path).unwrap())
            .unwrap()
            .contains("1E100"));
        write_export(&format!("v2/mixed-{old_template}.json"),&serde_json::to_vec_pretty(&json!({"read":read,"owner_denial":denied,"upgrade":up,"old_sha256":digest(&original)})).unwrap());
        h.close_clean();
    }
}
