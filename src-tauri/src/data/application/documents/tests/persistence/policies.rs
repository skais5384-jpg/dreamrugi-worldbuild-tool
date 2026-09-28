use super::*;

pub(super) fn typed_rich(text: &str) -> DocumentValueEdit {
    let mut raw = rich();
    raw["document"]["content"]["children"][0]["children"][0]["text"] = text.into();
    let normalized = crate::data::field_engine::rich_text::normalize_rich_text(
        1,
        raw["document"]["content"].as_object().unwrap(),
    )
    .unwrap();
    DocumentValueEdit::from_normalized_rich_text(normalized)
}

#[test]
fn g8_historical_initial_provenance_does_not_overwrite_existing_or_unset_values() {
    for case in ["missing", "unset", "existing"] {
        let f = Fixture::new();
        let (mut t, mut d) = fixture_raw();
        t["revision"] = 4.into();
        t["fieldOrder"].as_array_mut().unwrap().push(key(3).into());
        t["fields"][key(3)] = json!({"label":"historical","kind":"singleLineText","lifecycle":"active","required":false,
            "introducedRevision":4,"defaultValue":{"kind":"text","value":"current","future":"__numbers__"},
            "initialDefaultValue":{"kind":"text","value":"initial","future":"__numbers__"},"presentation":{},"configuration":{"kind":"singleLineText"}});
        if case == "unset" {
            d["fieldValues"][key(3)] = json!({"kind":"unset","future":"__numbers__"});
        }
        if case == "existing" {
            d["fieldValues"][key(3)] =
                json!({"kind":"text","value":"existing","future":"__numbers__"});
        }
        assert_owner(
            &raw_bytes(&t),
            &["fields", &key(3), "initialDefaultValue"],
            &json!({"kind":"text","value":"initial","future":"__numbers__"}),
        );
        assert_owner(
            &raw_bytes(&t),
            &["fields", &key(3), "defaultValue"],
            &json!({"kind":"text","value":"current","future":"__numbers__"}),
        );
        assert_numbers(&raw_bytes(&d));
        if case != "missing" {
            assert_owner(
                &raw_bytes(&d),
                &["fieldValues", &key(3)],
                &if case == "unset" {
                    json!({"kind":"unset","future":"__numbers__"})
                } else {
                    json!({"kind":"text","value":"existing","future":"__numbers__"})
                },
            );
        }
        let (tid, did) = seed_raw(&f, &t, &d);
        let mut rt = f.runtime();
        let input = save_input(&mut rt, tid, did, vec![]);
        let original = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let mut session: Session = begin(&rt, targets.session_targets());
        let snap = session.snapshot();
        let (result, c, k) =
            observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
        assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
        assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
        let owner = result.outcome().unwrap();
        assert_warning(owner.warnings());
        let bytes = artifact::encode_document(owner.document()).unwrap();
        assert_numbers(&bytes);
        let tree = parse_strict_lossless_json_object(&bytes).unwrap();
        let expected = raw_bytes(&match case {
            "missing" => json!({"kind":"text","value":"initial","future":"__numbers__"}),
            "unset" => json!({"kind":"unset","future":"__numbers__"}),
            _ => json!({"kind":"text","value":"existing","future":"__numbers__"}),
        });
        let expected = parse_strict_lossless_json_object(&expected).unwrap();
        assert!(
            tree.object_path(&["fieldValues", &key(3)])
                .expect("historical owner exists")
                == &expected,
            "initial provenance or prior value remains at field 3"
        );
        assert!(fs::read(document_path(&f, did)).unwrap() == bytes);
        assert_disk(&template_path(&f, tid), &original);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g8_snapshot_membership_label_rejoin_and_metadata_loss_use_final_candidate_policy() {
    use artifact::DocumentReconciliationIssueCategory as Issue;
    for case in [
        "rejoin",
        "keep-archived",
        "field-extra",
        "retained-option-extra",
        "removed-option-extra",
    ] {
        let f = Fixture::new();
        let (mut t, mut d) = fixture_raw();
        t["fields"][key(1)]["kind"] = "multiChoice".into();
        t["fields"][key(1)]["configuration"]["kind"] = "multiChoice".into();
        d["fieldValues"][key(1)] =
            json!({"kind":"multiChoice","optionIds":[key(11),key(12)],"future":"__numbers__"});
        d["orphanedFieldDefinitions"][key(1)] = json!({"kind":"multiChoice","label":"historical field",
            "options":{(key(11)):{"label":"historical archived"},(key(12)):{"label":"historical active"}}});
        match case {
            "field-extra" | "keep-archived" => {
                d["orphanedFieldDefinitions"][key(1)]["future"] = "__numbers__".into()
            }
            "retained-option-extra" => {
                d["orphanedFieldDefinitions"][key(1)]["options"][key(12)]["future"] =
                    "__numbers__".into()
            }
            "removed-option-extra" => {
                d["orphanedFieldDefinitions"][key(1)]["options"][key(11)]["future"] =
                    "__numbers__".into()
            }
            _ => {}
        }
        let source_bytes = raw_bytes(&d);
        assert_owner(
            &source_bytes,
            &["fieldValues", &key(1)],
            &json!({"kind":"multiChoice","optionIds":[key(11),key(12)],"future":"__numbers__"}),
        );
        for (id, label, has_extra) in [
            (11, "historical archived", case == "removed-option-extra"),
            (12, "historical active", case == "retained-option-extra"),
        ] {
            let expected = if has_extra {
                json!({"label":label,"future":"__numbers__"})
            } else {
                json!({"label":label})
            };
            assert_owner(
                &source_bytes,
                &["orphanedFieldDefinitions", &key(1), "options", &key(id)],
                &expected,
            );
        }
        if case == "field-extra" || case == "keep-archived" {
            assert_owner(
                &source_bytes,
                &["orphanedFieldDefinitions", &key(1), "future"],
                &json!("__numbers__"),
            );
        }
        let (tid, did) = seed_raw(&f, &t, &d);
        let mut rt = f.runtime();
        let edit = DocumentEdit::SetValue(
            field(1),
            DocumentValueEdit::multi_choice(vec![option(if case == "keep-archived" {
                11
            } else {
                12
            })]),
        );
        let input = save_input(&mut rt, tid, did, vec![edit.clone()]);
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        let db = disk(&document_path(&f, did));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let mut session = begin::<Payload>(&rt, targets.session_targets());
        let snap = session.snapshot();
        let (result, c, k) =
            observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
        if case.ends_with("extra") {
            no_io(c, k);
            assert!(result.outcome().is_none());
            let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
                panic!("snapshot policy rejection")
            };
            let Some(DocumentUpdateError::Save(error)) = error.domain_cause() else {
                panic!("pure snapshot cause")
            };
            assert_eq!(error.category(), DocumentSaveErrorCategory::SnapshotBlocked);
            assert_eq!(
                error.issue().unwrap().category(),
                if case == "removed-option-extra" {
                    Issue::LossySnapshotMembership
                } else {
                    Issue::LossyOrphanReattachment
                }
            );
            assert_disk(&document_path(&f, did), &db);
        } else {
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
            assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
            let owner = result.outcome().unwrap();
            let bytes = artifact::encode_document(owner.document()).unwrap();
            let raw: Value = serde_json::from_slice(&bytes).unwrap();
            if case == "keep-archived" {
                assert_warning(owner.warnings());
                let snapshot = &raw["orphanedFieldDefinitions"][key(1)];
                assert!(
                    snapshot["label"] == "historical field"
                        && snapshot["options"][key(11)]["label"] == "historical archived"
                );
                assert!(
                    snapshot["options"].as_object().unwrap().len() == 1
                        && snapshot["options"].get(key(12)).is_none()
                );
                let tree = parse_strict_lossless_json_object(&bytes).unwrap();
                let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
                assert!(
                    tree.object_path(&["orphanedFieldDefinitions", &key(1), "future"])
                        .expect("snapshot owner")
                        == &expected
                );
            } else {
                assert!(owner.warnings().is_empty());
                assert!(
                    raw["orphanedFieldDefinitions"]
                        .as_object()
                        .unwrap()
                        .is_empty(),
                    "label differences allow safe active rejoin"
                );
            }
            assert!(fs::read(document_path(&f, did)).unwrap() == bytes);
            let before = disk(&document_path(&f, did));
            let mut fresh = save_input(&mut rt, tid, did, vec![edit]);
            fresh.timestamp_utc = AGAIN.into();
            let (again, c, k) =
                observe(|| save_document(&mut rt, &mut session, context(&snap), &fresh).unwrap());
            no_io(c, k);
            assert!(matches!(
                again.execution.body(),
                Some(BodyOutcome::NoWrite(()))
            ));
            assert!(again.outcome().unwrap().warnings() == owner.warnings());
            assert_disk(&document_path(&f, did), &before);
        }
        assert_disk(&template_path(&f, tid), &tb);
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g8_rich_text_outer_extra_survives_unset_and_new_typed_payload_without_reviving_old_data() {
    let f = Fixture::new();
    let (t, mut d) = fixture_raw();
    d["fieldValues"][key(2)]["document"]
        .as_object_mut()
        .unwrap()
        .remove("future");
    assert_owner(
        &raw_bytes(&d),
        &["fieldValues", &key(2), "future"],
        &json!("__numbers__"),
    );
    let (tid, did) = seed_raw(&f, &t, &d);
    let mut rt = f.runtime();
    let template_before = disk(&template_path(&f, tid));
    let targets = doc_targets(did);
    let mut session: Session = begin(&rt, targets.session_targets());
    let snap = session.snapshot();
    for unset in [true, false] {
        let input = save_input(
            &mut rt,
            tid,
            did,
            vec![if unset {
                DocumentEdit::Unset(field(2))
            } else {
                DocumentEdit::SetValue(field(2), typed_rich("fresh payload"))
            }],
        );
        let (result, c, k) =
            observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
        assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
        assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
        let owner = result.outcome().unwrap();
        assert_warning(owner.warnings());
        let bytes = artifact::encode_document(owner.document()).unwrap();
        let tree = parse_strict_lossless_json_object(&bytes).unwrap();
        let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
        assert!(
            tree.object_path(&["fieldValues", &key(2), "future"])
                .expect("outer envelope remains")
                == &expected
        );
        let raw: Value = serde_json::from_slice(&bytes).unwrap();
        let value = &raw["fieldValues"][key(2)];
        if unset {
            assert!(value["kind"] == "unset" && value.get("document").is_none());
        } else {
            let expected = json!({"schemaVersion":1,"content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":"fresh payload","marks":["bold"]}]}]}});
            assert!(
                value["document"] == expected,
                "only new known rich-text payload remains"
            );
        }
        assert!(fs::read(document_path(&f, did)).unwrap() == bytes);
    }
    assert_disk(&template_path(&f, tid), &template_before);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g8_each_unknown_rich_text_internal_owner_blocks_replacement_and_unset() {
    for layer in ["document", "root", "node"] {
        for unset in [false, true] {
            let f = Fixture::new();
            let (t, mut d) = fixture_raw();
            let rich = &mut d["fieldValues"][key(2)]["document"];
            rich.as_object_mut().unwrap().remove("future");
            match layer {
                "document" => rich["future"] = "__numbers__".into(),
                "root" => rich["content"]["future"] = "__numbers__".into(),
                _ => rich["content"]["children"][0]["children"][0]["future"] = "__numbers__".into(),
            }
            let source_tree = parse_strict_lossless_json_object(&raw_bytes(&d)).unwrap();
            let rich_owner = source_tree
                .object_path(&["fieldValues", &key(2), "document"])
                .expect("rich document owner");
            let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
            if layer == "document" {
                assert!(
                    rich_owner
                        .object_path(&["future"])
                        .expect("document metadata owner")
                        == &expected
                );
            } else if layer == "root" {
                assert!(
                    rich_owner
                        .object_path(&["content", "future"])
                        .expect("root metadata owner")
                        == &expected
                );
            } else {
                // array 내부 owner는 독립 기대 rich document 전체와 비교해 다른 text node로 대체하지 않는다.
                let expected = json!({"schemaVersion":1,"content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":PRIVATE,"marks":["bold"],"future":"__numbers__"}]}]}});
                assert_owner(
                    &raw_bytes(&d),
                    &["fieldValues", &key(2), "document"],
                    &expected,
                );
            }
            let (tid, did) = seed_raw(&f, &t, &d);
            let mut rt = f.runtime();
            let input = save_input(
                &mut rt,
                tid,
                did,
                vec![if unset {
                    DocumentEdit::Unset(field(2))
                } else {
                    DocumentEdit::SetValue(field(2), typed_rich("replacement"))
                }],
            );
            let payload = Payload::new();
            let proof = RequestProof::capture(&input, &payload);
            let db = disk(&document_path(&f, did));
            let tb = disk(&template_path(&f, tid));
            let targets = doc_targets(did);
            let mut session = begin::<Payload>(&rt, targets.session_targets());
            let snap = session.snapshot();
            let (result, c, k) =
                observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
            no_io(c, k);
            assert!(result.outcome().is_none());
            let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
                panic!("rich-text loss rejection")
            };
            let Some(DocumentUpdateError::Save(error)) = error.domain_cause() else {
                panic!("original loss cause")
            };
            assert_eq!(error.category(), DocumentSaveErrorCategory::LossyValueEdit);
            assert_eq!(error.field_id(), Some(field(2)));
            assert_disk(&document_path(&f, did), &db);
            assert_disk(&template_path(&f, tid), &tb);
            proof.assert(&input, &payload);
            proof.drop_payload(payload);
            assert_eq!(rt.snapshot().state, RuntimeState::Ready);
            assert_eq!(session.snapshot().state(), EditSessionState::Editing);
            session.end_edit().unwrap();
            rt.close().unwrap();
        }
    }
}
