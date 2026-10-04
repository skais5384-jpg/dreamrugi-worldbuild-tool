//! Field 실제 저장·lossless owner·거부 회귀. 실제 Windows 창 검증과 구분한다.
use super::*;
use crate::data::json::parse_strict_lossless_json_object as lossless;

const TEMPLATE: &str = "99999999-9999-4999-8999-000000000064";
fn field(n: u32) -> String {
    format!("99999999-9999-4999-8999-{n:012x}")
}

#[test]
fn m273_atomic_archive_repair_preserves_exact_owners_and_documents() {
    for multi in [false, true] {
        let h = Harness::new();
        let (template, document) =
            crate::data::application::composite::tests::guarded_fixture_bytes();
        let decoded: Value = serde_json::from_slice(&template).unwrap();
        let mut choice = decoded["fields"][field(1)].clone();
        let wire_kind = if multi { "multiChoice" } else { "singleChoice" };
        choice["kind"] = json!(wire_kind);
        choice["required"] = json!(true);
        choice["presentation"] = json!({"token":"archive-readback"});
        choice["configuration"]["kind"] = json!(wire_kind);
        choice["configuration"]["optionOrder"] = json!([field(12), field(13)]);
        choice["configuration"]["options"][field(12)]["future"] =
            json!({"owner":"option-twelve","n":"M273_OPTION_NUMBER"});
        choice["configuration"]["options"][field(13)] =
            json!({"label":"remaining","lifecycle":"active"});
        choice["defaultValue"] = if multi {
            json!({"kind":wire_kind,"optionIds":[field(12),field(13)]})
        } else {
            json!({"kind":wire_kind,"optionId":field(12)})
        };
        choice["defaultValue"]["future"] =
            json!({"owner":"choice-current","n":"M273_CURRENT_NUMBER"});
        choice["initialDefaultValue"] = if multi {
            json!({"kind":wire_kind,"optionIds":[field(12)]})
        } else {
            json!({"kind":wire_kind,"optionId":field(12)})
        };
        choice["initialDefaultValue"]["future"] =
            json!({"owner":"choice-initial","n":"M273_INITIAL_NUMBER"});
        let replacement = serde_json::to_string(&choice)
            .unwrap()
            .replace("\"M273_OPTION_NUMBER\"", "73E+101")
            .replace("\"M273_CURRENT_NUMBER\"", "74e+102")
            .replace("\"M273_INITIAL_NUMBER\"", "75E103");
        let original = String::from_utf8(template).unwrap();
        // fixture의 첫 Field만 최초 준비 단계에서 교체한다. 일반 JSON 직렬화로 숫자 원문을 찾지 않는다.
        let marker = format!("\"{}\":", field(1));
        assert_eq!(original.matches(&marker).count(), 1);
        let start = original.find(&marker).unwrap() + marker.len();
        let end = start
            + original[start..]
                .find(&format!(",\"{}\":", field(2)))
                .unwrap();
        let template =
            format!("{}{}{}", &original[..start], replacement, &original[end..]).into_bytes();
        artifact::decode_template(&template).unwrap();
        let template = current_policy_fixture(&template, &format!("templates/{TEMPLATE}.json"));
        let document = current_policy_fixture(&document, &format!("documents/{}.json", field(200)));
        fs::create_dir(h.root.join("templates")).unwrap();
        fs::create_dir(h.root.join("documents")).unwrap();
        let path = h.root.join("templates").join(format!("{TEMPLATE}.json"));
        let doc = h
            .root
            .join("documents")
            .join(format!("{}.json", field(200)));
        fs::write(&path, &template).unwrap();
        fs::write(&doc, &document).unwrap();
        let doc_time = fs::metadata(&doc).unwrap().modified().unwrap();
        let before = lossless(&template).unwrap();
        let p = h.open();
        for edit in [
            json!({"kind":"add_option","field":field(1),"option":{"id":field(14),"label":""},"index":null}),
            json!({"kind":"rename_option","field":field(1),"option":field(14),"label":"\n선택 🌿 \n"}),
            json!({"kind":"reorder_options","field":field(1),"options":[field(14),field(13),field(12)]}),
        ] {
            assert_eq!(apply(&h, &p, TEMPLATE, edit)["disk"], "committed");
        }
        let before_archive = fs::read(&path).unwrap();
        let mut refusals = vec![
            json!({"kind":"archive_option","field":field(1),"option":field(12),"repair":null}),
            json!({"kind":"archive_option","field":field(1),"option":field(14),"repair":{"kind":"unset"}}),
            json!({"kind":"archive_option","field":field(1),"option":field(11),"repair":null}),
        ];
        for invalid in [field(12), field(11), field(99)] {
            let repair = if multi {
                json!({"kind":"multi_choice","options":[invalid]})
            } else {
                json!({"kind":"single_choice","option":invalid})
            };
            refusals.push(json!({"kind":"archive_option","field":field(1),"option":field(12),"repair":repair}));
        }
        if multi {
            refusals.push(json!({"kind":"archive_option","field":field(1),"option":field(12),"repair":{"kind":"multi_choice","options":[]}}));
        }
        for edit in refusals {
            let result = apply(&h, &p, TEMPLATE, edit);
            assert!(
                result["error"].is_object(),
                "invalid archive must return a closed error"
            );
            assert!(
                fs::read(&path).unwrap() == before_archive,
                "refused archive changed source bytes"
            );
        }
        let repair = if multi {
            json!({"kind":"multi_choice","options":[field(13)]})
        } else {
            json!({"kind":"single_choice","option":field(13)})
        };
        assert_eq!(
            apply(
                &h,
                &p,
                TEMPLATE,
                json!({"kind":"archive_option","field":field(1),"option":field(12),"repair":repair})
            )["disk"],
            "committed"
        );
        let saved = fs::read(&path).unwrap();
        let after = lossless(&saved).unwrap();
        let saved_json: Value = serde_json::from_slice(&saved).unwrap();
        let prior_json: Value = serde_json::from_slice(&before_archive).unwrap();
        assert!(
            saved_json["revision"].as_u64().unwrap()
                == prior_json["revision"].as_u64().unwrap() + 1,
            "archive and repair must be one revision"
        );
        for owner in [
            vec!["future"],
            vec!["fields", &field(2)],
            vec!["fields", &field(4)],
            vec!["fields", &field(1), "future"],
            vec!["fields", &field(1), "initialDefaultValue"],
            vec!["fields", &field(1), "defaultValue", "future"],
            vec!["fields", &field(1), "configuration", "future"],
            vec![
                "fields",
                &field(1),
                "configuration",
                "options",
                &field(12),
                "future",
            ],
        ] {
            assert!(
                before.object_path(&owner).is_some(),
                "expected original owner missing"
            );
            assert!(
                before.object_path(&owner) == after.object_path(&owner),
                "exact archive owner differs"
            );
        }
        assert!(
            saved_json["fields"][field(1)]["configuration"]["options"][field(12)]["lifecycle"]
                == "archived",
            "target must remain archived"
        );
        assert!(
            saved_json["fields"][field(1)]["configuration"]["optionOrder"]
                == json!([field(14), field(13)]),
            "active order must exclude only target"
        );
        assert_eq!(
            apply(
                &h,
                &p,
                TEMPLATE,
                json!({"kind":"archive_field","field":field(1)})
            )["disk"],
            "committed"
        );
        let archived = fs::read(&path).unwrap();
        let final_owner = lossless(&archived).unwrap();
        for member in [
            "label",
            "kind",
            "required",
            "presentation",
            "introducedRevision",
            "configuration",
            "defaultValue",
            "initialDefaultValue",
            "future",
        ] {
            assert!(
                after.object_path(&["fields", &field(1), member])
                    == final_owner.object_path(&["fields", &field(1), member]),
                "Field archive must preserve last definition and option lifecycle"
            );
        }
        assert_eq!(
            apply(
                &h,
                &p,
                TEMPLATE,
                json!({"kind":"archive_field","field":field(1)})
            )["disk"],
            "no_write"
        );
        assert!(
            fs::read(&path).unwrap() == archived,
            "repeated Field archive must be unchanged"
        );
        assert!(
            document == fs::read(&doc).unwrap()
                && doc_time == fs::metadata(&doc).unwrap().modified().unwrap(),
            "archive must not rewrite an existing Document"
        );
        h.close_clean();
    }
}
fn apply(h: &Harness, p: &str, id: &str, edit: Value) -> Value {
    let read = h.read_template(p, id);
    let s = h.session(p, vec![read["view"].clone()], "template");
    let op=h.submit(json!({"kind":"update_template","project":p,"session":s,"view":read["view"],"revision":read["content"]["revision"],"edit":edit}));
    let result = h.result(&op);
    let retained = h.call(json!({"action":"operation","operation":op}))["retained"].clone();
    h.ack(&op);
    if !retained.is_null() {
        let observed = h.call(json!({"action":"retained_read","retained":retained}));
        assert!(
            observed["intent"]["edit"] == edit,
            "retained must own the exact rejected intent"
        );
        assert_eq!(observed["g6_clearable"], true);
        let abandon = h.reserve("control");
        h.call(json!({"action":"submit","operation":abandon,"input":{"kind":"abandon_retained","retained":retained}}));
        assert_eq!(h.result(&abandon)["action"], "abandoned");
        h.ack(&abandon);
    }
    assert_eq!(
        h.control(json!({"kind":"session_control","project":p,"session":s,"control":"end"}))
            ["kind"],
        "control"
    );
    result
}

#[test]
fn m272_lossless_field_owners_keep_fresh_history_and_document_bytes() {
    let h = Harness::new();
    let (template, document) = crate::data::application::composite::tests::guarded_fixture_bytes();
    // 기존 token이 없는 rich Field만 최초 fixture 준비에서 교체한다. 저장 결과는 덮어쓰지 않는다.
    let decoded: Value = serde_json::from_slice(&template).unwrap();
    let old = serde_json::to_string(&decoded["fields"][field(2)]).unwrap();
    let mut rich = decoded["fields"][field(2)].clone();
    rich["defaultValue"] = json!({"kind":"richText","future":{"owner":"rich-current","n":"NUMBER_TOKEN"},"document":{"schemaVersion":1,"content":{"kind":"root","future":{"n":"NODE_TOKEN"},"children":[{"kind":"paragraph","children":[{"kind":"text","text":"fixed fixture"}]}]}}});
    let replacement = serde_json::to_string(&rich)
        .unwrap()
        .replace("\"NUMBER_TOKEN\"", "7E+109")
        .replace("\"NODE_TOKEN\"", "8e+108");
    let text = String::from_utf8(template).unwrap();
    assert_eq!(text.matches(&old).count(), 1);
    let template = text.replacen(&old, &replacement, 1).into_bytes();
    artifact::decode_template(&template).unwrap();
    let template = current_policy_fixture(&template, &format!("templates/{TEMPLATE}.json"));
    let document = current_policy_fixture(&document, &format!("documents/{}.json", field(200)));
    fs::create_dir(h.root.join("templates")).unwrap();
    fs::create_dir(h.root.join("documents")).unwrap();
    let path = h.root.join("templates").join(format!("{TEMPLATE}.json"));
    let doc = h
        .root
        .join("documents")
        .join(format!("{}.json", field(200)));
    fs::write(&path, &template).unwrap();
    fs::write(&doc, &document).unwrap();
    let before = lossless(&template).unwrap();
    let marker = lossless(br#"{"a":7E+109,"b":8e+108,"c":1E100}"#).unwrap();
    assert!(
        before.object_path(&["fields", &field(2), "defaultValue", "future", "n"])
            == marker.object_path(&["a"]),
        "rich default numeric owner must exist before writes"
    );
    assert!(
        before.object_path(&[
            "fields",
            &field(2),
            "defaultValue",
            "document",
            "content",
            "future",
            "n"
        ]) == marker.object_path(&["b"]),
        "AST numeric owner must exist before writes"
    );
    assert!(
        before.object_path(&[
            "fields",
            &field(4),
            "defaultValue",
            "future",
            "lexemes",
            "upper"
        ]) == marker.object_path(&["c"]),
        "other default owner must exist before writes"
    );
    let p = h.open();
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    for f in [field(2), field(4)] {
        let r = apply(&h, &p, TEMPLATE, json!({"kind":"keep_default","field":f}));
        assert_eq!(r["disk"], "no_write");
        assert_eq!(r["changed"], false);
        assert!(
            fs::read(&path).unwrap() == template,
            "Keep must preserve all native bytes"
        );
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), time);
    }
    for edit in [
        json!({"kind":"field_label","field":field(4),"label":"renamed"}),
        json!({"kind":"field_required","field":field(4),"required":true}),
        json!({"kind":"field_presentation","field":field(4),"token":"opaque-token"}),
        json!({"kind":"default","field":field(4),"value":{"kind":"single_line_text","value":"changed"}}),
    ] {
        assert_eq!(apply(&h, &p, TEMPLATE, edit)["disk"], "committed");
    }
    let saved = fs::read(&path).unwrap();
    let after = lossless(&saved).unwrap();
    for owner in [
        vec!["future"],
        vec!["fields", &field(1)],
        vec!["fields", &field(2)],
        vec!["fields", &field(4), "defaultValue", "future"],
        vec!["fields", &field(4), "initialDefaultValue"],
    ] {
        assert!(
            before.object_path(&owner).is_some(),
            "preserved owner must exist"
        );
        assert!(
            before.object_path(&owner) == after.object_path(&owner),
            "exact lossless owner changed"
        );
    }
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        apply(
            &h,
            &p,
            TEMPLATE,
            json!({"kind":"default","field":field(2),"value":{"kind":"unset"}})
        )["disk"],
        "not_attempted"
    );
    assert!(
        saved == fs::read(&path).unwrap(),
        "rich metadata must not be discarded by unset"
    );
    assert_eq!(
        apply(
            &h,
            &p,
            TEMPLATE,
            json!({"kind":"default","field":field(4),"value":{"kind":"single_line_text","value":"changed"}})
        )["disk"],
        "no_write"
    );
    assert!(
        saved == fs::read(&path).unwrap(),
        "Fresh equal must preserve bytes"
    );
    assert_eq!(mtime, fs::metadata(&path).unwrap().modified().unwrap());
    assert_eq!(
        apply(
            &h,
            &p,
            TEMPLATE,
            json!({"kind":"default","field":field(4),"value":{"kind":"unset"}})
        )["disk"],
        "committed"
    );
    let final_bytes = fs::read(&path).unwrap();
    let final_source = lossless(&final_bytes).unwrap();
    assert!(
        before.object_path(&["fields", &field(4), "initialDefaultValue"])
            == final_source.object_path(&["fields", &field(4), "initialDefaultValue"])
    );
    assert!(
        before.object_path(&["fields", &field(4), "defaultValue", "future"])
            == final_source.object_path(&["fields", &field(4), "defaultValue", "future"])
    );
    assert!(
        document == fs::read(doc).unwrap(),
        "existing Document bytes changed"
    );
    h.close_clean();
}

#[test]
fn m272_closed_field_value_rejections_and_required_unset_contract() {
    for (kind, valid, invalid) in [
        (
            "single_line_text",
            json!({"kind":"single_line_text","value":"text"}),
            json!({"kind":"single_line_text","value":"line\nbreak"}),
        ),
        (
            "number",
            json!({"kind":"number","value":"0"}),
            json!({"kind":"number","value":"-"}),
        ),
        (
            "date",
            json!({"kind":"date","value":"2024-02-29"}),
            json!({"kind":"date","value":"2025-02-29"}),
        ),
        (
            "time",
            json!({"kind":"time","value":"23:59:59.999"}),
            json!({"kind":"time","value":"24:00:00.000"}),
        ),
        (
            "duration",
            json!({"kind":"duration","milliseconds":"-9223372036854775808"}),
            json!({"kind":"duration","milliseconds":"9223372036854775808"}),
        ),
        (
            "single_choice",
            json!({"kind":"single_choice","option":field(11)}),
            json!({"kind":"single_choice","option":field(12)}),
        ),
        (
            "multi_choice",
            json!({"kind":"multi_choice","options":[field(11)]}),
            json!({"kind":"multi_choice","options":[]}),
        ),
        (
            "rich_text",
            json!({"kind":"unset"}),
            json!({"kind":"single_line_text","value":"wrong variant"}),
        ),
    ] {
        let h = Harness::new();
        let p = h.open();
        let (id, _) = h.template(&p);
        let config = if kind.ends_with("choice") {
            json!({"kind":kind,"options":[{"id":field(11),"label":"initial"}]})
        } else {
            json!({"kind":kind})
        };
        assert_eq!(
            apply(
                &h,
                &p,
                &id,
                json!({"kind":"create_field","field":field(1),"label":"field","configuration":config,"required":true,"presentation":null,"default":valid,"index":null})
            )["disk"],
            "committed"
        );
        let path = h.root.join("templates").join(format!("{id}.json"));
        let before = fs::read(&path).unwrap();
        let result = apply(
            &h,
            &p,
            &id,
            json!({"kind":"default","field":field(1),"value":invalid}),
        );
        assert_eq!(result["disk"], "not_attempted");
        assert!(result["error"].is_object());
        assert!(
            before == fs::read(&path).unwrap(),
            "rejected Field value changed disk"
        );
        // Template default와 최종 Document required 정책은 다르다. 허용된 unset을 거부로 바꾸지 않는다.
        let unset = apply(
            &h,
            &p,
            &id,
            json!({"kind":"default","field":field(1),"value":{"kind":"unset"}}),
        );
        assert!(unset["disk"] == "committed" || unset["disk"] == "no_write");
        assert_eq!(
            apply(
                &h,
                &p,
                &id,
                json!({"kind":"field_required","field":field(1),"required":true})
            )["disk"],
            "no_write"
        );
        h.close_clean();
    }
}
