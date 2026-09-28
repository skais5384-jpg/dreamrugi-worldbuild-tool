use super::*;

fn typed_rich(text: &str) -> DocumentValueEdit {
    let mut raw = rich();
    raw["document"]["content"]["children"][0]["children"][0]["text"] = text.into();
    let normalized = crate::data::field_engine::rich_text::normalize_rich_text(
        1,
        raw["document"]["content"].as_object().unwrap(),
    )
    .unwrap();
    DocumentValueEdit::from_normalized_rich_text(normalized)
}
fn committed_pair(f: &Fixture, result: &CompositeSaveExecution, before: &Pair) -> Pair {
    assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
    let t = result.template_outcome().unwrap().changed().unwrap();
    let d = result.document_outcome().unwrap();
    assert_eq!(d.kind(), DocumentSaveOutcomeKind::Changed);
    assert_eq!(d.document().template_revision(), t.revision());
    let actual = pair(f);
    assert!(actual.template.0 == artifact::encode_template(t).unwrap());
    assert!(actual.document.0 == artifact::encode_document(d.document()).unwrap());
    assert!(actual.template.0 != before.template.0 && actual.document.0 != before.document.0);
    assert!(actual.sentinel == before.sentinel);
    actual
}

#[test]
fn g9_label_only_snapshot_rejoins_but_field_or_option_extra_loss_blocks_after_template_candidate() {
    for case in ["label-only", "field-extra", "option-extra"] {
        let f = Fixture::new();
        let (mut t, mut d) = fixture_raw();
        t["fields"][key(1)]["configuration"]["options"][key(11)]["lifecycle"] = "active".into();
        t["fields"][key(1)]["configuration"]["optionOrder"] = json!([key(11), key(12)]);
        d["orphanedFieldDefinitions"][key(1)] = json!({"label":"historical field","kind":"singleChoice","options":{(key(11)):{"label":"historical option"}}});
        if case == "field-extra" {
            d["orphanedFieldDefinitions"][key(1)]["future"] = "__snapshot__".into();
        }
        if case == "option-extra" {
            d["orphanedFieldDefinitions"][key(1)]["options"][key(11)]["future"] =
                "__snapshot_option__".into();
        }
        f.seed(&t, &d);
        let mut rt = f.runtime();
        let before = pair(&f);
        owner(
            &before.document.0,
            &["orphanedFieldDefinitions", &key(1)],
            &d["orphanedFieldDefinitions"][key(1)],
        );
        let input = load_input(
            &mut rt,
            TemplateEditIntent::SetName("changed".into()),
            vec![],
        );
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        let tc = result.template_outcome().unwrap().changed().unwrap();
        assert_eq!(tc.revision(), revision(4));
        if case == "label-only" {
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
            let actual = committed_pair(&f, &result, &before);
            let tree = parse_strict_lossless_json_object(&actual.document.0).unwrap();
            assert!(tree
                .object_path(&["orphanedFieldDefinitions", &key(1)])
                .is_none());
            assert!(result.document_outcome().unwrap().warnings().is_empty());
            owner(
                &actual.document.0,
                &["fieldValues", &key(1)],
                &d["fieldValues"][key(1)],
            );
            metadata_owner(&actual.document.0, &["future"], "document");
            metadata_owner(
                &actual.template.0,
                &[
                    "fields",
                    &key(1),
                    "configuration",
                    "options",
                    &key(11),
                    "future",
                ],
                "option",
            );
        } else {
            no_io(c, k);
            let CompositeSaveError::Document(cause) = domain(&result) else {
                panic!("snapshot original error")
            };
            assert_eq!(cause.category(), DocumentSaveErrorCategory::SnapshotBlocked);
            assert_eq!(
                cause.issue().unwrap().category(),
                artifact::DocumentReconciliationIssueCategory::LossyOrphanReattachment
            );
            assert!(result.document_outcome().is_none());
            same_pair(&f, &before);
        }
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_outer_rich_metadata_survives_unset_and_fresh_rich_without_reviving_old_content() {
    let f = Fixture::new();
    let (t, mut d) = fixture_raw();
    d["fieldValues"][key(2)]["document"]
        .as_object_mut()
        .unwrap()
        .remove("future");
    f.seed(&t, &d);
    let mut rt = f.runtime();
    let mut before = pair(&f);
    metadata_owner(
        &before.document.0,
        &["fieldValues", &key(2), "future"],
        "rich_outer",
    );
    for (n, edit) in [
        (4, DocumentEdit::Unset(field(2))),
        (
            5,
            DocumentEdit::SetValue(field(2), typed_rich("fresh rich content")),
        ),
    ] {
        let mut input = load_input(
            &mut rt,
            TemplateEditIntent::SetName(format!("changed {n}")),
            vec![edit],
        );
        input.timestamp_utc = if n == 4 { SAVE } else { AGAIN }.into();
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
        let actual = committed_pair(&f, &result, &before);
        assert_eq!(
            result
                .document_outcome()
                .unwrap()
                .document()
                .template_revision(),
            revision(n)
        );
        warning(result.document_outcome().unwrap());
        let expected = if n == 4 {
            json!({"kind":"unset","future":"__rich_outer__"})
        } else {
            let mut value = rich();
            value["document"].as_object_mut().unwrap().remove("future");
            value["document"]["content"]["children"][0]["children"][0]["text"] =
                "fresh rich content".into();
            value
        };
        owner(&actual.document.0, &["fieldValues", &key(2)], &expected);
        metadata_owner(
            &actual.document.0,
            &["fieldValues", &key(2), "future"],
            "rich_outer",
        );
        assert!(!String::from_utf8(actual.document.0.clone())
            .unwrap()
            .contains(PRIVATE));
        before = actual;
        session.end_edit().unwrap();
    }
    rt.close().unwrap();
}

#[test]
fn g9_internal_rich_metadata_loss_is_rejected_with_first_candidate_and_original_pair() {
    for location in ["envelope", "root", "node"] {
        let f = Fixture::new();
        let (t, mut d) = fixture_raw();
        if location != "envelope" {
            d["fieldValues"][key(2)]["document"]
                .as_object_mut()
                .unwrap()
                .remove("future");
            if location == "root" {
                d["fieldValues"][key(2)]["document"]["content"]["future"] = "__rich_inner__".into();
            } else {
                d["fieldValues"][key(2)]["document"]["content"]["children"][0]["future"] =
                    "__rich_inner__".into();
            }
        }
        f.seed(&t, &d);
        let mut rt = f.runtime();
        let before = pair(&f);
        owner(
            &before.document.0,
            &["fieldValues", &key(2)],
            &d["fieldValues"][key(2)],
        );
        let input = load_input(
            &mut rt,
            TemplateEditIntent::SetName("changed".into()),
            vec![DocumentEdit::Unset(field(2))],
        );
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        no_io(c, k);
        let CompositeSaveError::Document(cause) = domain(&result) else {
            panic!("rich loss error")
        };
        assert_eq!(
            (cause.category(), cause.stage()),
            (
                DocumentSaveErrorCategory::LossyValueEdit,
                DocumentSaveStage::Edits
            )
        );
        assert_eq!(
            result
                .template_outcome()
                .unwrap()
                .changed()
                .unwrap()
                .revision(),
            revision(4)
        );
        assert!(result.document_outcome().is_none());
        same_pair(&f, &before);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}
