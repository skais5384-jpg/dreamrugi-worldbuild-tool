use super::*;

fn preservation_fixture() -> (TemplateArtifact, DocumentArtifact) {
    let tr = template_raw();
    let mut raw = document_raw();
    raw["futureRoot"] = json!({"same":"__lexemes__","private":SECRET});
    for id in 1..=8 {
        raw["fieldValues"][key(id)]["future"] = json!({"same":"__lexemes__","private":SECRET});
    }
    raw["fieldValues"][key(2)]["document"]["futureEnvelope"] = json!("__lexemes__");
    raw["fieldValues"][key(2)]["document"]["content"]["futureRoot"] = json!("__lexemes__");
    raw["fieldValues"][key(2)]["document"]["content"]["children"][0]["futureBlock"] =
        json!("__lexemes__");
    raw["fieldValues"][key(2)]["document"]["content"]["children"][0]["children"][0]["futureNode"] =
        json!("__lexemes__");
    for field in [98, 99] {
        let a = field * 10 + 1;
        let b = field * 10 + 2;
        raw["fieldValues"][key(field)] = json!({
            "kind":"multiChoice","optionIds":[key(a),key(b)],"future":"__lexemes__"
        });
        raw["orphanedFieldDefinitions"][key(field)] = json!({
            "label":SECRET,"kind":"multiChoice","future":"__lexemes__",
            "options":{
                (key(a)):{"label":SECRET,"future":"__lexemes__"},
                (key(b)):{"label":SECRET,"future":"__lexemes__"}
            }
        });
    }
    (template(&tr), document(&raw))
}

/// owner를 고정한 계약 목록이다. 값이 같아도 다른 Field/Option 위치로 대체하지 않는다.
fn preservation_paths() -> Vec<Vec<String>> {
    let mut paths = vec![
        vec!["futureRoot".into()],
        vec!["fieldValues".into(), key(1), "future".into()],
    ];
    for id in 2..=8 {
        paths.push(vec!["fieldValues".into(), key(id)]);
    }
    for field in [98, 99] {
        paths.push(vec!["fieldValues".into(), key(field)]);
        paths.push(vec!["orphanedFieldDefinitions".into(), key(field)]);
        for option in [field * 10 + 1, field * 10 + 2] {
            paths.push(vec![
                "orphanedFieldDefinitions".into(),
                key(field),
                "options".into(),
                key(option),
            ]);
        }
    }
    paths
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OptionCase {
    Field98Option981,
    Field98Option982,
    Field99Option991,
    Field99Option992,
}

impl OptionCase {
    const ALL: [Self; 4] = [
        Self::Field98Option981,
        Self::Field98Option982,
        Self::Field99Option991,
        Self::Field99Option992,
    ];

    fn path(self) -> Vec<String> {
        // 전달받은 경로나 lookup 결과에서 case를 역산하지 않는 fixture 소유 계약이다.
        let (field, option) = match self {
            Self::Field98Option981 => (98, 981),
            Self::Field98Option982 => (98, 982),
            Self::Field99Option991 => (99, 991),
            Self::Field99Option992 => (99, 992),
        };
        vec![
            "orphanedFieldDefinitions".into(),
            key(field),
            "options".into(),
            key(option),
        ]
    }
}

#[derive(Clone)]
struct OptionCheck {
    case: OptionCase,
    source_path: Vec<String>,
    target_path: Vec<String>,
}

fn option_checks() -> Vec<OptionCheck> {
    OptionCase::ALL
        .into_iter()
        .map(|case| OptionCheck {
            case,
            source_path: case.path(),
            target_path: case.path(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerCategory {
    MissingCase,
    DuplicateCase,
    SourcePath,
    TargetPath,
    MissingSource,
    SourceCanary,
    MissingTarget,
    TargetCanary,
    SubtreeMismatch,
}

// 오류에는 안전한 case/category만 담고 canary나 비교 operand는 담지 않는다.
type OwnerError = (OptionCase, OwnerCategory);

fn check_option_paths(check: &OptionCheck) -> Result<(), OwnerError> {
    if check.source_path != check.case.path() {
        return Err((check.case, OwnerCategory::SourcePath));
    }
    if check.target_path != check.case.path() {
        return Err((check.case, OwnerCategory::TargetPath));
    }
    Ok(())
}

fn option_subtree<'a>(
    tree: &'a LosslessJsonValue,
    path: &[String],
    case: OptionCase,
    missing: OwnerCategory,
    mismatch: OwnerCategory,
) -> Result<&'a LosslessJsonValue, OwnerError> {
    let path = path.iter().map(String::as_str).collect::<Vec<_>>();
    let subtree = tree.object_path(&path).ok_or((case, missing))?;
    let canary = subtree.object_path(&["future"]).ok_or((case, mismatch))?;
    // 실제 source를 기대값으로 재사용하지 않는다. 숫자 표기를 포함한 독립 계약이다.
    let expected = lossless(br#"{"n":[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]}"#);
    if canary != expected.object_path(&["n"]).expect("fixed canary") {
        return Err((case, mismatch));
    }
    Ok(subtree)
}

fn verify_option_sources(
    source: &LosslessJsonValue,
    checks: &[OptionCheck],
) -> Result<(), OwnerError> {
    let mut seen = std::collections::BTreeSet::new();
    for check in checks {
        if !seen.insert(check.case) {
            return Err((check.case, OwnerCategory::DuplicateCase));
        }
        check_option_paths(check)?;
    }
    for case in OptionCase::ALL {
        if !seen.contains(&case) {
            return Err((case, OwnerCategory::MissingCase));
        }
    }
    for check in checks {
        option_subtree(
            source,
            &check.source_path,
            check.case,
            OwnerCategory::MissingSource,
            OwnerCategory::SourceCanary,
        )?;
    }
    Ok(())
}

fn verify_option_owner_preserved(
    source: &LosslessJsonValue,
    target: &LosslessJsonValue,
    check: &OptionCheck,
) -> Result<(), OwnerError> {
    check_option_paths(check)?;
    let old = option_subtree(
        source,
        &check.source_path,
        check.case,
        OwnerCategory::MissingSource,
        OwnerCategory::SourceCanary,
    )?;
    let new = option_subtree(
        target,
        &check.target_path,
        check.case,
        OwnerCategory::MissingTarget,
        OwnerCategory::TargetCanary,
    )?;
    if old != new {
        return Err((check.case, OwnerCategory::SubtreeMismatch));
    }
    // 정확한 owner/source 계약이 확인된 후 기존 전체 subtree 비교도 그대로 사용한다.
    let source_path = check
        .source_path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let target_path = check
        .target_path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_preserved(source, &source_path, target, &target_path);
    Ok(())
}

#[test]
fn editing_one_field_preserves_exact_unedited_subtrees_and_every_owner_lexeme() {
    let (template, document) = preservation_fixture();
    let source_bytes = encode_document(&document).expect("source encode");
    let source = lossless(&source_bytes);
    let source_template = encode_template(&template).expect("Template source encode");
    let paths = preservation_paths();
    let checks = option_checks();
    // 반드시 production save 전에 네 source canary와 완전한 case inventory를 검증한다.
    verify_option_sources(&source, &checks).expect("option source precondition");
    // None == None 회귀를 막기 위해 production 호출 전에 모든 source의 존재를 먼저 확인한다.
    for path in &paths {
        let path = path.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(
            source.object_path(&path).is_some(),
            "missing preservation source"
        );
    }
    let edits = DocumentEditSet::new(vec![
        DocumentEdit::Rename("renamed".into()),
        DocumentEdit::SetValue(fid(1), DocumentValueEdit::single_line_text("edited".into())),
    ]);
    let outcome = save(&template, &document, &edits).expect("lossless edit");
    let target_bytes = encode_document(outcome.document()).expect("target encode");
    let target = lossless(&target_bytes);
    for path in &paths {
        let path = path.iter().map(String::as_str).collect::<Vec<_>>();
        assert_preserved(&source, &path, &target, &path);
    }
    for check in &checks {
        verify_option_owner_preserved(&source, &target, check).expect("option target preservation");
    }
    // 최초 lexeme가 fixture 자체에 있었는지 독립적으로 확인한다. 같은 값의 다른 owner도 존재한다.
    let expected = lossless(br#"{"n":[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]}"#);
    for path in [
        vec!["futureRoot".to_owned(), "same".to_owned()],
        vec![
            "fieldValues".to_owned(),
            key(1),
            "future".to_owned(),
            "same".to_owned(),
        ],
    ] {
        let path = path.iter().map(String::as_str).collect::<Vec<_>>();
        assert_preserved(&expected, &["n"], &source, &path);
        assert_preserved(&expected, &["n"], &target, &path);
    }
    assert!(encode_document(&document).expect("source reencode") == source_bytes);
    assert!(encode_template(&template).expect("Template reencode") == source_template);
    assert_idempotent(&template, &outcome, &edits);
}

#[test]
fn rich_text_unset_transitions_preserve_only_the_outer_envelope_losslessly() {
    let template = template(&template_raw());
    for outer in [false, true] {
        let mut raw = document_raw();
        if outer {
            raw["fieldValues"][key(2)]["future"] = json!("__lexemes__");
        }
        let original = document(&raw);
        let before = encode_document(&original).expect("original encode");
        let source = lossless(&before);
        let template_before = encode_template(&template).expect("Template encode");
        let clear = DocumentEditSet::new(vec![DocumentEdit::Unset(fid(2))]);
        let unset = save(&template, &original, &clear).expect("outer-only rich-text to unset");
        assert!(unset.document().field_values()[&fid(2)].is_unset());
        let bytes = encode_document(unset.document()).expect("unset encode");
        let unset_tree = lossless(&bytes);
        assert!(unset_tree
            .object_path(&["fieldValues", &key(2), "document"])
            .is_none());
        assert_idempotent(&template, &unset, &clear);
        // 실제 decode를 거쳐 다시 채워도 이전 본문/provenance가 되살아나지 않아야 한다.
        let decoded = decode_document(&bytes).expect("unset decode");
        let edits = set(2, typed_rich("new known payload"));
        let restored = save(&template, &decoded, &edits).expect("outer-only unset to rich-text");
        let restored_tree =
            lossless(&encode_document(restored.document()).expect("restored encode"));
        let mut expected_raw = document_raw();
        expected_raw["fieldValues"][key(2)] = rich("new known payload");
        let expected_payload = lossless(&raw_bytes(&expected_raw));
        let payload_path = ["fieldValues", &key(2), "document"];
        assert_preserved(
            &expected_payload,
            &payload_path,
            &restored_tree,
            &payload_path,
        );
        let extra_path = ["fieldValues", &key(2), "future"];
        if outer {
            let expected =
                lossless(br#"{"n":[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]}"#);
            for tree in [&source, &unset_tree, &restored_tree] {
                assert_preserved(&expected, &["n"], tree, &extra_path);
            }
        } else {
            for tree in [&source, &unset_tree, &restored_tree] {
                assert!(tree.object_path(&extra_path).is_none());
            }
        }
        assert_idempotent(&template, &restored, &edits);
        assert!(encode_document(&original).expect("original unchanged") == before);
        assert!(encode_template(&template).expect("Template unchanged") == template_before);
        let mut required = template_raw();
        required["fields"][key(2)]["required"] = json!(true);
        let blank = save(&super::template(&required), &original, &clear)
            .expect("clearing a required value preserves its outer envelope");
        assert!(blank.document().field_values()[&fid(2)].is_unset());
    }
}

#[test]
fn rich_text_metadata_is_neither_injected_discarded_nor_reassigned_by_edits() {
    let (template, document) = preservation_fixture();
    for edits in [
        set(2, typed_rich("new text")),
        set(2, typed_rich(SECRET)),
        DocumentEditSet::new(vec![DocumentEdit::Unset(fid(2))]),
    ] {
        failure(&template, &document, &edits, Category::LossyValueEdit);
    }
    let no_edit =
        save(&template, &document, &DocumentEditSet::default()).expect("no-edit internal metadata");
    assert_eq!(no_edit.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert!(
        encode_document(no_edit.document()).expect("no-edit encode")
            == encode_document(&document).expect("source encode")
    );
    assert_idempotent(&template, &no_edit, &DocumentEditSet::default());
    // 각 내부 계층만 있어도 손실을 검출해야 다른 계층의 extra가 결함을 가리지 않는다.
    for layer in 0..3 {
        let mut raw = document_raw();
        let payload = &mut raw["fieldValues"][key(2)]["document"];
        match layer {
            0 => payload["future"] = json!("__lexemes__"),
            1 => payload["content"]["future"] = json!("__lexemes__"),
            2 => payload["content"]["children"][0]["children"][0]["future"] = json!("__lexemes__"),
            _ => unreachable!("closed metadata layers"),
        }
        let source = super::document(&raw);
        for edits in [
            set(2, typed_rich("replacement")),
            DocumentEditSet::new(vec![DocumentEdit::Unset(fid(2))]),
        ] {
            failure(&template, &source, &edits, Category::LossyValueEdit);
        }
    }
    let mut rich = rich(SECRET);
    rich["document"]["content"]["future"] = json!(SECRET);
    let normalized =
        normalize_rich_text(1, rich["document"]["content"].as_object().expect("content"))
            .expect("normalization allows retained metadata");
    let edits = set(2, DocumentValueEdit::from_normalized_rich_text(normalized));
    failure(&template, &document, &edits, Category::UnknownEditMetadata);
    // FieldValue outer extra는 원래 envelope에 운반할 수 있어 단독으로는 본문 편집을 막지 않는다.
    let mut raw = document_raw();
    raw["fieldValues"][key(2)]["future"] = json!("__lexemes__");
    let plain = super::document(&raw);
    let edits = set(2, typed_rich("known replacement"));
    let outcome = save(&template, &plain, &edits).expect("outer extra safely retained");
    let old = lossless(&encode_document(&plain).expect("old"));
    let new = lossless(&encode_document(outcome.document()).expect("new"));
    assert_preserved(
        &old,
        &["fieldValues", &key(2), "future"],
        &new,
        &["fieldValues", &key(2), "future"],
    );
    assert_idempotent(&template, &outcome, &edits);
}

#[test]
fn required_archived_snapshot_keeps_historical_labels_and_all_extras() {
    let mut tr = template_raw();
    archive_field(&mut tr, 7);
    let mut raw = document_raw();
    raw["orphanedFieldDefinitions"][key(7)] = snapshot(
        "singleChoice",
        json!({
            (key(71)):{"label":"historic option","future":"__lexemes__"}
        }),
    );
    raw["orphanedFieldDefinitions"][key(7)]["label"] = json!("historic field");
    raw["orphanedFieldDefinitions"][key(7)]["future"] = json!("__lexemes__");
    let template = template(&tr);
    let document = document(&raw);
    let edits = DocumentEditSet::new(vec![DocumentEdit::Rename("changed".into())]);
    let outcome = save(&template, &document, &edits).expect("archived history preserved");
    let source = lossless(&encode_document(&document).expect("source"));
    let target = lossless(&encode_document(outcome.document()).expect("target"));
    assert_preserved(
        &source,
        &["orphanedFieldDefinitions", &key(7)],
        &target,
        &["orphanedFieldDefinitions", &key(7)],
    );
    assert_idempotent(&template, &outcome, &edits);
    let mut active = tr;
    active["fields"][key(7)]["lifecycle"] = json!("active");
    active["fieldOrder"]
        .as_array_mut()
        .expect("order")
        .push(json!(key(7)));
    // Template에도 같은 extra가 있더라도 snapshot 소유 extra 제거의 근거가 되지 않는다.
    active["fields"][key(7)]["future"] = raw["orphanedFieldDefinitions"][key(7)]["future"].clone();
    let active = super::template(&active);
    let blocked = failure(
        &active,
        &document,
        &DocumentEditSet::default(),
        Category::SnapshotBlocked,
    );
    assert_eq!(
        blocked.issue().expect("issue").category(),
        Issue::LossyOrphanReattachment
    );
}

#[test]
fn full_storage_validation_rejects_option_id_collision_across_generated_snapshots() {
    let mut tr = template_raw();
    archive_field(&mut tr, 7);
    let template = template(&tr);
    let mut raw = document_raw();
    raw["fieldValues"][key(99)] = json!({"kind":"singleChoice","optionId":key(71)});
    raw["orphanedFieldDefinitions"][key(99)] =
        snapshot("singleChoice", json!({(key(71)):{"label":"unknown owner"}}));
    let source = document(&raw);
    assert!(source.validate_storage().is_ok());
    let result = failure(
        &template,
        &source,
        &DocumentEditSet::default(),
        Category::InvalidCandidate,
    );
    assert_eq!(result.stage(), DocumentSaveStage::FinalStorage);
    assert_eq!(
        result.storage_validation_category(),
        Some(ArtifactValidationErrorCategory::DuplicateOptionId)
    );
}

#[test]
fn final_blocker_count_is_bounded_and_payload_safe() {
    let mut tr = template_raw();
    tr["fields"] = json!({});
    tr["fieldOrder"] = json!([]);
    let base = template_raw()["fields"][key(1)].clone();
    let mut raw = document_raw();
    raw["fieldValues"] = json!({});
    for i in 1..=1_025 {
        let mut field = base.clone();
        field["required"] = json!(true);
        tr["fields"][key(i)] = field;
        tr["fieldOrder"]
            .as_array_mut()
            .expect("order")
            .push(json!(key(i)));
        raw["fieldValues"][key(i)] = json!({"kind":"number","value":"1"});
    }
    let result = failure(
        &template(&tr),
        &document(&raw),
        &DocumentEditSet::default(),
        Category::BlockingIssues,
    );
    assert_eq!(result.issue_count(), 1_024);
    assert!(result.issues_truncated());
    assert_eq!(result.field_id(), Some(fid(1)));
}

/// 실제 비교 helper가 누락/변경을 검출하는지 확인하는 작은 raw object 편집기다.
/// 조상 object만 파싱하여 다른 owner의 number token을 정규화하지 않는다.
fn change_member(bytes: &[u8], path: &[&str], replacement: Option<&str>) -> Vec<u8> {
    use serde_json::value::RawValue;
    let mut object: std::collections::BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(bytes).expect("raw object");
    let (key, rest) = path.split_first().expect("nonempty path");
    assert!(object.contains_key(*key), "missing probe target");
    if rest.is_empty() {
        if let Some(replacement) = replacement {
            object.insert(
                (*key).to_owned(),
                RawValue::from_string(replacement.to_owned()).expect("raw replacement"),
            );
        } else {
            object.remove(*key);
        }
    } else {
        let nested = change_member(object[*key].get().as_bytes(), rest, replacement);
        object.insert(
            (*key).to_owned(),
            RawValue::from_string(String::from_utf8(nested).expect("UTF-8")).expect("nested raw"),
        );
    }
    serde_json::to_vec(&object).expect("raw serialization")
}

#[test]
fn preservation_comparison_negative_cases_and_panic_output_are_isolated() {
    const CHILD: &str = "WORLDBUILD_G1_SAFE_ASSERTION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["data::artifact::document::document_save::tests::preservation::preservation_comparison_negative_cases_and_panic_output_are_isolated", "--exact", "--test-threads=1", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .expect("isolated assertion test");
        assert!(output.status.success(), "isolated assertion test failed");
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            rendered.contains("running 1 test") && rendered.contains("1 passed; 0 failed"),
            "child filter did not run"
        );
        assert!(
            rendered.matches("g1-safe-probes=21").count() == 1,
            "missing child probes"
        );
        assert!(!rendered.contains(SECRET), "assertion payload disclosure");
        return;
    }
    let (_, document) = preservation_fixture();
    let bytes = encode_document(&document).expect("source encode");
    let source = lossless(&bytes);
    let checks = option_checks();
    verify_option_sources(&source, &checks).expect("complete source inventory");
    let mut reversed = checks.clone();
    reversed.reverse();
    verify_option_sources(&source, &reversed).expect("order does not change case coverage");
    let mut probes = 0;
    let mut rejects = |result, case, category| {
        assert_eq!(
            result,
            Err((case, category)),
            "wrong owner failure category"
        );
        probes += 1;
    };
    for check in &checks {
        let mut extra_path = check.source_path.clone();
        extra_path.push("future".into());
        let path = extra_path.iter().map(String::as_str).collect::<Vec<_>>();
        let unchanged = change_member(
            &bytes,
            &path,
            Some("[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]"),
        );
        verify_option_owner_preserved(&source, &lossless(&unchanged), check)
            .expect("owner no-op control");
        for replacement in [
            None,
            Some("[1e100,1e100,-0,0.12345678901234567890123456789,[2,-0]]"),
        ] {
            let changed = lossless(&change_member(&bytes, &path, replacement));
            rejects(
                verify_option_owner_preserved(&source, &changed, check),
                check.case,
                OwnerCategory::TargetCanary,
            );
        }
    }
    let first = &checks[0];
    let absent = lossless(b"{}");
    rejects(
        verify_option_owner_preserved(&absent, &source, first),
        first.case,
        OwnerCategory::MissingSource,
    );
    rejects(
        verify_option_owner_preserved(&source, &absent, first),
        first.case,
        OwnerCategory::MissingTarget,
    );
    for (index, wrong_option) in [(0, 991), (2, 981)] {
        let mut wrong = checks[index].clone();
        wrong.source_path[3] = key(wrong_option);
        wrong.target_path = wrong.source_path.clone();
        rejects(
            verify_option_owner_preserved(&source, &source, &wrong),
            wrong.case,
            OwnerCategory::SourcePath,
        );
    }
    // 같은 값을 가진 유효한 다른 owner도 intended case를 대신할 수 없다.
    let mut wrong_target = first.clone();
    wrong_target.target_path = checks[2].target_path.clone();
    rejects(
        verify_option_owner_preserved(&source, &source, &wrong_target),
        first.case,
        OwnerCategory::TargetPath,
    );
    let first_path = first
        .source_path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let removed = lossless(&change_member(&bytes, &first_path, None));
    let mut wrong_both = wrong_target;
    wrong_both.source_path = checks[2].source_path.clone();
    rejects(
        verify_option_owner_preserved(&removed, &removed, &wrong_both),
        first.case,
        OwnerCategory::SourcePath,
    );
    let mut first_future = first.source_path.clone();
    first_future.push("future".into());
    let path = first_future.iter().map(String::as_str).collect::<Vec<_>>();
    let contaminated = lossless(&change_member(&bytes, &path, Some("[0]")));
    rejects(
        verify_option_owner_preserved(&contaminated, &contaminated, first),
        first.case,
        OwnerCategory::SourceCanary,
    );
    // 원래 보존 테스트의 production 호출 전 진입점과 정확히 같은 precondition을 검사한다.
    let mut second_future = checks[1].source_path.clone();
    second_future.push("future".into());
    let path = second_future.iter().map(String::as_str).collect::<Vec<_>>();
    let bad_982 = lossless(&change_member(&bytes, &path, Some("[0]")));
    rejects(
        verify_option_sources(&bad_982, &checks),
        checks[1].case,
        OwnerCategory::SourceCanary,
    );
    rejects(
        verify_option_sources(&source, &checks[..3]),
        checks[3].case,
        OwnerCategory::MissingCase,
    );
    let mut duplicated = checks.clone();
    duplicated.push(first.clone());
    rejects(
        verify_option_sources(&source, &duplicated),
        first.case,
        OwnerCategory::DuplicateCase,
    );
    let mut substituted = checks.clone();
    substituted[1] = checks[2].clone();
    rejects(
        verify_option_sources(&source, &substituted),
        checks[2].case,
        OwnerCategory::DuplicateCase,
    );
    // 일반 helper의 기존 missing-source/target panic도 정확한 안전한 이유만 허용한다.
    for (source, target, expected) in [
        (&absent, &source, "missing source subtree"),
        (&source, &absent, "missing target subtree"),
    ] {
        let panic = std::panic::catch_unwind(|| {
            assert_preserved(source, &["futureRoot"], target, &["futureRoot"])
        })
        .expect_err("missing subtree comparison must fail");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied());
        assert!(
            message == Some(expected),
            "unexpected subtree panic category"
        );
        probes += 1;
    }
    assert_eq!(probes, 21);
    println!("g1-safe-probes={probes}");
}

#[test]
fn success_and_failure_diagnostics_do_not_expose_payloads_or_edit_values() {
    let (template, document) = preservation_fixture();
    let edits = DocumentEditSet::new(vec![DocumentEdit::Rename(SECRET.to_owned())]);
    let result = save(&template, &document, &edits).expect("redacted success");
    for rendered in [
        format!("{result:?}"),
        format!("{:?}", result.document()),
        format!("{edits:?}"),
        format!("{:?}", edits.edits[0]),
        format!("{:?}", typed_rich(SECRET)),
    ] {
        assert!(!rendered.contains(SECRET), "success payload disclosure");
    }
    let bad = set(
        1,
        DocumentValueEdit::single_line_text(format!("{SECRET}\ninvalid")),
    );
    failure(&template, &document, &bad, Category::InvalidEditValue);
    let loss = set(2, typed_rich(SECRET));
    failure(&template, &document, &loss, Category::LossyValueEdit);
}
