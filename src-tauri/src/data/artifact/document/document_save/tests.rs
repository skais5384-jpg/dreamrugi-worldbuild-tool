use std::{error::Error, str::FromStr};

use serde_json::{json, Value};

use super::*;
use crate::data::{
    artifact::{
        create_document, decode_document, decode_template, encode_document, encode_template,
        materialize_document, reconcile_document, DocumentId, DocumentMaterializationErrorCategory,
        DocumentMaterializationOutcomeKind, OptionId,
    },
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::normalize_rich_text,
        validation::{FieldValidationErrorCategory, FieldValidationLocation},
    },
    json::{parse_strict_lossless_json_object, LosslessJsonValue},
};

type Category = DocumentSaveErrorCategory;
type Issue = DocumentReconciliationIssueCategory;
const EARLY: &str = "2026-09-01T00:00:00.000Z";
const NOW: &str = "2026-09-02T00:00:00.000Z";
const LATER: &str = "2026-09-03T00:00:00.000Z";
const SECRET: &str = "g1-private-payload-canary";
const KINDS: [&str; 8] = [
    "singleLineText",
    "richText",
    "number",
    "date",
    "time",
    "duration",
    "singleChoice",
    "multiChoice",
];

fn key(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}

fn fid(n: u32) -> FieldId {
    FieldId::from_str(&key(n)).expect("valid fixture FieldId")
}

fn oid(n: u32) -> OptionId {
    OptionId::from_str(&key(n)).expect("valid fixture OptionId")
}

fn rev(n: u32) -> TemplateRevision {
    TemplateRevision::try_from(n).expect("valid fixture revision")
}

fn rich(text: &str) -> Value {
    json!({"kind":"richText","document":{"schemaVersion":1,"content":{
        "kind":"root","children":[{"kind":"paragraph","children":[{
            "kind":"text","text":text,"marks":["bold"]
        }]}]
    }}})
}

fn typed_rich(text: &str) -> DocumentValueEdit {
    let value = rich(text);
    let normalized = normalize_rich_text(
        1,
        value["document"]["content"]
            .as_object()
            .expect("fixture content"),
    )
    .expect("valid rich-text fixture");
    DocumentValueEdit::from_normalized_rich_text(normalized)
}

fn values() -> [Value; 8] {
    [
        json!({"kind":"text","value":SECRET}),
        rich(SECRET),
        json!({"kind":"number","value":"12.5"}),
        json!({"kind":"date","value":"2024-02-29"}),
        json!({"kind":"time","value":"23:59:59.123"}),
        json!({"kind":"duration","milliseconds":"-123"}),
        json!({"kind":"singleChoice","optionId":key(71)}),
        json!({"kind":"multiChoice","optionIds":[key(81),key(82)]}),
    ]
}

fn template_raw() -> Value {
    let mut fields = serde_json::Map::new();
    for (index, (kind, value)) in KINDS.into_iter().zip(values()).enumerate() {
        let id = (index + 1) as u32;
        let mut configuration = json!({"kind":kind});
        if id >= 7 {
            configuration["optionOrder"] = json!([key(id * 10 + 1), key(id * 10 + 2)]);
            configuration["options"] = json!({
                (key(id * 10 + 1)):{"label":"a","lifecycle":"active"},
                (key(id * 10 + 2)):{"label":"b","lifecycle":"active"},
                (key(id * 10 + 3)):{"label":"old","lifecycle":"archived"}
            });
        }
        fields.insert(
            key(id),
            json!({
                "label":SECRET,"kind":kind,"lifecycle":"active","required":false,
                "introducedRevision":1,"defaultValue":value,"initialDefaultValue":{"kind":"unset"},
                "configuration":configuration,"presentation":{}
            }),
        );
    }
    json!({
        "schemaVersion":1,"artifactType":"template","templateId":key(100),"revision":3,
        "name":SECRET,"lifecycle":"active","fieldOrder":(1..=8).map(key).collect::<Vec<_>>(),
        "fields":fields,"presentation":{},"createdAtUtc":EARLY,"updatedAtUtc":NOW
    })
}

fn document_raw() -> Value {
    let fields = (1..=8)
        .map(key)
        .zip(values())
        .collect::<serde_json::Map<_, _>>();
    json!({
        "schemaVersion":1,"artifactType":"document","documentId":key(200),
        "templateId":key(100),"templateRevision":3,"name":SECRET,
        "fieldValues":fields,"orphanedFieldDefinitions":{},
        "createdAtUtc":EARLY,"updatedAtUtc":NOW
    })
}

fn raw_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_string(value)
        .expect("fixture serialization")
        .replace(
            "\"__lexemes__\"",
            "[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]",
        )
        .into_bytes()
}

fn template(value: &Value) -> TemplateArtifact {
    decode_template(&raw_bytes(value)).expect("valid fixture Template")
}

fn document(value: &Value) -> DocumentArtifact {
    decode_document(&raw_bytes(value)).expect("valid fixture Document")
}

fn set(id: u32, value: DocumentValueEdit) -> DocumentEditSet {
    DocumentEditSet::new(vec![DocumentEdit::SetValue(fid(id), value)])
}

fn save(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
    edits: &DocumentEditSet,
) -> Result<DocumentSaveOutcome, DocumentSaveError> {
    prepare_document_save(template, template.revision(), document, edits, LATER)
}

fn snapshot(kind: &str, options: Value) -> Value {
    json!({"kind":kind,"label":SECRET,"options":options})
}

fn archive_field(raw: &mut Value, n: u32) {
    raw["fields"][key(n)]["lifecycle"] = json!("archived");
    raw["fieldOrder"]
        .as_array_mut()
        .expect("fixture order")
        .retain(|id| id != &json!(key(n)));
}

fn error(result: Result<DocumentSaveOutcome, DocumentSaveError>) -> DocumentSaveError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected preparation failure"),
    }
}

/// 실제 실패 API 호출마다 source와 edit의 전체 equality/codec 결과를 함께 검사한다.
fn failure(
    template: &TemplateArtifact,
    document: &DocumentArtifact,
    edits: &DocumentEditSet,
    category: Category,
) -> DocumentSaveError {
    let before = (template.clone(), document.clone(), edits.clone());
    let bytes = (encode_template(template), encode_document(document));
    let result = error(save(template, document, edits));
    assert_eq!(result.category(), category);
    assert!(*template == before.0, "Template changed on failure");
    assert!(*document == before.1, "Document changed on failure");
    assert!(*edits == before.2, "edits changed on failure");
    assert!(
        encode_template(template) == bytes.0,
        "Template codec result changed"
    );
    assert!(
        encode_document(document) == bytes.1,
        "Document codec result changed"
    );
    assert!(result.source().is_none(), "unexpected raw error chain");
    assert!(
        !format!("{result} {result:?} {edits:?}").contains(SECRET),
        "diagnostic disclosure"
    );
    result
}

fn assert_idempotent(
    template: &TemplateArtifact,
    outcome: &DocumentSaveOutcome,
    edits: &DocumentEditSet,
) {
    assert!(outcome.document().validate_storage().is_ok());
    let view = reconcile_document(template, outcome.document()).expect("final bound validation");
    assert!(view.blocking_issues().is_empty());
    assert!(!view.materialization_required());
    let bytes = encode_document(outcome.document()).expect("candidate must encode");
    let decoded = decode_document(&bytes).expect("candidate must decode");
    assert!(
        encode_document(&decoded).expect("round-trip encode") == bytes,
        "round-trip changed"
    );
    for edits in [DocumentEditSet::default(), edits.clone()] {
        let repeated = prepare_document_save(
            template,
            template.revision(),
            &decoded,
            &edits,
            "2026-09-04T00:00:00.000Z",
        )
        .expect("repeat must succeed");
        assert_eq!(repeated.kind(), DocumentSaveOutcomeKind::Unchanged);
        assert!(repeated.document().updated_at_utc() == outcome.document().updated_at_utc());
        assert!(
            encode_document(repeated.document()).expect("repeat encode") == bytes,
            "repeat bytes changed"
        );
        assert!(
            repeated.warnings() == outcome.warnings(),
            "final warnings changed"
        );
    }
}

#[test]
fn historical_required_unset_is_filled_by_edit_before_final_validation() {
    let mut raw = template_raw();
    raw["fields"][key(1)]["introducedRevision"] = json!(2);
    raw["fields"][key(1)]["required"] = json!(true);
    let template = template(&raw);
    let mut raw = document_raw();
    raw["templateRevision"] = json!(1);
    raw["fieldValues"]
        .as_object_mut()
        .expect("values")
        .remove(&key(1));
    let document = document(&raw);
    let empty = DocumentEditSet::default();
    let blank = save(&template, &document, &empty).expect("required unset is saveable");
    assert!(blank.document().field_values()[&fid(1)].is_unset());
    let materialized =
        materialize_document(&template, template.revision(), &document, LATER.to_owned())
            .expect("historical required unset is materializable");
    assert!(materialized
        .document()
        .expect("inserted field")
        .field_values()[&fid(1)]
        .is_unset());
    let edits = set(1, DocumentValueEdit::single_line_text("filled".to_owned()));
    let outcome = save(&template, &document, &edits).expect("edit fills final required value");
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
    assert!(outcome.document().field_values()[&fid(1)].text() == Some("filled"));
    assert_idempotent(&template, &outcome, &edits);
    let unset = DocumentEditSet::new(vec![DocumentEdit::Unset(fid(1))]);
    let cleared = save(&template, &document, &unset).expect("required unset remains saveable");
    assert!(cleared.document().field_values()[&fid(1)].is_unset());
}

#[test]
fn historical_initial_and_existing_values_are_independent_of_current_default() {
    let mut tr = template_raw();
    tr["fields"][key(1)]["introducedRevision"] = json!(2);
    tr["fields"][key(1)]["initialDefaultValue"] =
        json!({"kind":"text","value":"initial","future":"__lexemes__"});
    let template = template(&tr);
    let mut dr = document_raw();
    dr["templateRevision"] = json!(1);
    dr["fieldValues"]
        .as_object_mut()
        .expect("values")
        .remove(&key(1));
    dr["fieldValues"][key(3)] = json!({"kind":"unset","future":"__lexemes__"});
    let document = document(&dr);
    let no_edit = save(&template, &document, &DocumentEditSet::default()).expect("historical save");
    assert!(no_edit.document().field_values()[&fid(1)].text() == Some("initial"));
    for id in 2..=8 {
        assert!(
            no_edit.document().field_values().get(&fid(id)).is_some(),
            "missing target"
        );
        assert!(
            no_edit.document().field_values()[&fid(id)] == document.field_values()[&fid(id)],
            "existing value changed"
        );
    }
    let edits = set(
        1,
        DocumentValueEdit::single_line_text("replacement".to_owned()),
    );
    let edited = save(&template, &document, &edits).expect("historical edit");
    let source = lossless(&encode_template(&template).expect("source encode"));
    let target = lossless(&encode_document(edited.document()).expect("target encode"));
    assert_preserved(
        &source,
        &["fields", &key(1), "initialDefaultValue", "future"],
        &target,
        &["fieldValues", &key(1), "future"],
    );
    assert_idempotent(&template, &edited, &edits);
}

#[test]
fn current_missing_cannot_be_repaired_by_default_or_explicit_edit() {
    let template = template(&template_raw());
    let mut dr = document_raw();
    dr["fieldValues"]
        .as_object_mut()
        .expect("values")
        .remove(&key(1));
    let document = document(&dr);
    for edits in [
        DocumentEditSet::default(),
        set(1, DocumentValueEdit::single_line_text("repair".into())),
    ] {
        let error = failure(
            &template,
            &document,
            &edits,
            Category::MissingKnownFieldValue,
        );
        assert_eq!(error.stage(), DocumentSaveStage::Preconditions);
        assert_eq!(error.field_id(), Some(fid(1)));
    }
}

#[test]
fn closed_edits_cover_name_all_eight_kinds_unset_and_repeat() {
    let template = template(&template_raw());
    let document = document(&document_raw());
    let payloads = [
        DocumentValueEdit::single_line_text(" new 값 ".into()),
        typed_rich("new rich"),
        DocumentValueEdit::number("-0.5".into()),
        DocumentValueEdit::date("2000-02-29".into()),
        DocumentValueEdit::time("00:01:02.003".into()),
        DocumentValueEdit::duration("9223372036854775807".into()),
        DocumentValueEdit::single_choice(oid(72)),
        DocumentValueEdit::multi_choice(vec![oid(82)]),
    ];
    let mut operations = vec![DocumentEdit::Rename("\n 새 이름 \r\n".into())];
    operations.extend(
        payloads
            .into_iter()
            .enumerate()
            .map(|(i, value)| DocumentEdit::SetValue(fid(i as u32 + 1), value)),
    );
    let edits = DocumentEditSet::new(operations);
    let outcome = save(&template, &document, &edits).expect("eight-kind edit");
    let mut reversed = edits.clone();
    reversed.edits.reverse();
    let reordered = save(&template, &document, &reversed).expect("independent edit order");
    assert!(
        encode_document(reordered.document()).expect("reordered encode")
            == encode_document(outcome.document()).expect("original encode")
    );
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
    assert!(outcome.document().name() == "\n 새 이름 \r\n");
    for edit in &edits.edits {
        if let DocumentEdit::SetValue(id, value) = edit {
            assert!(
                outcome.document().field_values().get(id).is_some(),
                "missing edited value"
            );
            assert!(
                outcome.document().field_values()[id] == value.value,
                "wrong edited payload"
            );
        }
    }
    assert_eq!(outcome.document().document_id(), document.document_id());
    assert_eq!(outcome.document().template_id(), document.template_id());
    assert!(outcome.document().created_at_utc() == EARLY);
    assert_idempotent(&template, &outcome, &edits);
    let unset = DocumentEditSet::new((1..=8).map(|i| DocumentEdit::Unset(fid(i))).collect());
    let cleared = save(&template, outcome.document(), &unset).expect("explicit unset");
    assert_eq!(cleared.document().field_values().len(), 8);
    assert!(cleared
        .document()
        .field_values()
        .values()
        .all(FieldValue::is_unset));
    assert_idempotent(&template, &cleared, &unset);
}

#[test]
fn glossary_exclusion_defaults_false_and_saves_without_schema_transition() {
    let template = template(&template_raw());
    let source = document(&document_raw());
    assert!(!source.glossary_excluded());
    let schema = source.schema_version();

    let excluded = save(
        &template,
        &source,
        &DocumentEditSet::new(vec![DocumentEdit::SetGlossaryExcluded(true)]),
    )
    .expect("glossary exclusion edit");
    assert!(excluded.document().glossary_excluded());
    assert_eq!(excluded.document().schema_version(), schema);
    let encoded = String::from_utf8(encode_document(excluded.document()).expect("encode"))
        .expect("utf8 JSON");
    let encoded: Value = serde_json::from_str(&encoded).expect("encoded JSON");
    assert_eq!(encoded["glossaryExcluded"], json!(true));

    let unrelated = save(
        &template,
        excluded.document(),
        &DocumentEditSet::new(vec![DocumentEdit::Rename("새 이름".into())]),
    )
    .expect("unrelated rename");
    assert!(unrelated.document().glossary_excluded());
}

#[test]
fn glossary_term_info_is_optional_persistent_clearable_and_schema_neutral() {
    let template = template(&template_raw());
    let source = document(&document_raw());
    assert_eq!(source.english_name(), "");
    assert_eq!(source.glossary_summary(), "");
    let schema = source.schema_version();
    let original_values = source.field_values().clone();

    let named = save(
        &template,
        &source,
        &DocumentEditSet::new(vec![
            DocumentEdit::SetEnglishName("Arin".into()),
            DocumentEdit::SetGlossarySummary("북부 왕국의 기록관".into()),
        ]),
    )
    .expect("term info edit");
    assert_eq!(named.document().english_name(), "Arin");
    assert_eq!(named.document().glossary_summary(), "북부 왕국의 기록관");
    assert_eq!(named.document().schema_version(), schema);
    assert_eq!(named.document().field_values(), &original_values);
    let encoded =
        String::from_utf8(encode_document(named.document()).expect("encode")).expect("utf8 JSON");
    let encoded: Value = serde_json::from_str(&encoded).expect("encoded JSON");
    assert_eq!(encoded["englishName"], json!("Arin"));
    assert_eq!(encoded["glossarySummary"], json!("북부 왕국의 기록관"));

    let cleared = save(
        &template,
        named.document(),
        &DocumentEditSet::new(vec![
            DocumentEdit::SetEnglishName(String::new()),
            DocumentEdit::Rename("제목만 변경".into()),
        ]),
    )
    .expect("clear term info");
    assert_eq!(cleared.document().english_name(), "");
    assert_eq!(cleared.document().glossary_summary(), "북부 왕국의 기록관");
    assert_eq!(cleared.document().name(), "제목만 변경");
    let encoded =
        String::from_utf8(encode_document(cleared.document()).expect("encode")).expect("utf8 JSON");
    let encoded: Value = serde_json::from_str(&encoded).expect("encoded JSON");
    assert!(encoded.get("englishName").is_none());
    assert_eq!(encoded["glossarySummary"], json!("북부 왕국의 기록관"));

    for separator in [
        '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0085}', '\u{2028}', '\u{2029}',
    ] {
        for edit in [
            DocumentEdit::SetEnglishName(format!("앞{separator}뒤")),
            DocumentEdit::SetGlossarySummary(format!("앞{separator}뒤")),
        ] {
            let source_before = named.document().clone();
            let error = save(
                &template,
                named.document(),
                &DocumentEditSet::new(vec![edit]),
            )
            .expect_err("term info hard separators must fail without mutating source");
            assert_eq!(
                error.category(),
                DocumentSaveErrorCategory::InvalidCandidate
            );
            assert_eq!(named.document(), &source_before);
        }
    }

    let preserved = save(
        &template,
        named.document(),
        &DocumentEditSet::new(vec![
            DocumentEdit::SetEnglishName("  한글\t🙂  ".into()),
            DocumentEdit::SetGlossarySummary(String::new()),
        ]),
    )
    .expect("allowed Unicode and an explicit empty value should persist canonically");
    assert_eq!(preserved.document().english_name(), "  한글\t🙂  ");
    assert_eq!(preserved.document().glossary_summary(), "");
    let encoded = encode_document(preserved.document()).expect("encode allowed term info");
    let encoded: Value = serde_json::from_slice(&encoded).expect("encoded JSON");
    assert_eq!(encoded["englishName"], json!("  한글\t🙂  "));
    assert!(encoded.get("glossarySummary").is_none());
}

#[test]
fn invalid_values_wrong_kind_and_noncanonical_choice_fail_without_normalization() {
    let template = template(&template_raw());
    let document = document(&document_raw());
    let cases = [
        (
            1,
            DocumentValueEdit::single_line_text("bad\ntext".into()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            1,
            DocumentValueEdit::single_line_text(String::new()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            1,
            DocumentValueEdit::number("1".into()),
            FieldValidationErrorCategory::FieldValueKindMismatch,
        ),
        (
            3,
            DocumentValueEdit::number("1e2".into()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            4,
            DocumentValueEdit::date("2023-02-29".into()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            5,
            DocumentValueEdit::time("24:00:00.000".into()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            6,
            DocumentValueEdit::duration("9223372036854775808".into()),
            FieldValidationErrorCategory::InvalidScalarValue,
        ),
        (
            8,
            DocumentValueEdit::multi_choice(vec![]),
            FieldValidationErrorCategory::InvalidChoiceValue,
        ),
        (
            8,
            DocumentValueEdit::multi_choice(vec![oid(82), oid(81)]),
            FieldValidationErrorCategory::InvalidChoiceValue,
        ),
        (
            8,
            DocumentValueEdit::multi_choice(vec![oid(81), oid(81)]),
            FieldValidationErrorCategory::InvalidChoiceValue,
        ),
    ];
    for (id, value, expected) in cases {
        let error = failure(
            &template,
            &document,
            &set(id, value),
            Category::InvalidEditValue,
        );
        let issue = error.issue().expect("validation issue");
        assert_eq!(issue.validation_category(), Some(expected));
        assert_eq!(
            issue.validation_location(),
            Some(FieldValidationLocation::ExistingDocumentValue)
        );
    }
}

#[test]
fn unknown_archived_and_duplicate_edit_targets_are_explicitly_rejected() {
    let mut tr = template_raw();
    archive_field(&mut tr, 1);
    let template = template(&tr);
    let document = document(&document_raw());
    for operation in [
        DocumentEdit::Unset(fid(1)),
        DocumentEdit::SetValue(fid(1), DocumentValueEdit::single_line_text("x".into())),
    ] {
        failure(
            &template,
            &document,
            &DocumentEditSet::new(vec![operation]),
            Category::ArchivedField,
        );
    }
    failure(
        &template,
        &document,
        &set(999, DocumentValueEdit::number("1".into())),
        Category::UnknownField,
    );
    for operations in [
        vec![DocumentEdit::Unset(fid(3)), DocumentEdit::Unset(fid(3))],
        vec![
            DocumentEdit::Unset(fid(3)),
            DocumentEdit::SetValue(fid(3), DocumentValueEdit::number("1".into())),
        ],
    ] {
        failure(
            &template,
            &document,
            &DocumentEditSet::new(operations),
            Category::DuplicateFieldEdit,
        );
    }
    for names in [("same", "same"), ("one", "two")] {
        failure(
            &template,
            &document,
            &DocumentEditSet::new(vec![
                DocumentEdit::Rename(names.0.into()),
                DocumentEdit::Rename(names.1.into()),
            ]),
            Category::DuplicateNameEdit,
        );
    }
}

#[test]
fn memory_revision_and_timestamp_preconditions_apply_to_no_op() {
    let template = template(&template_raw());
    let document = document(&document_raw());
    let edits = DocumentEditSet::default();
    for (expected, timestamp, category) in [
        (rev(2), "invalid", Category::RevisionMismatch),
        (rev(3), "invalid", Category::InvalidTimestamp),
        (rev(3), "2026-09-02T00:00:00Z", Category::InvalidTimestamp),
        (rev(3), EARLY, Category::TimestampRegression),
    ] {
        let result = error(prepare_document_save(
            &template, expected, &document, &edits, timestamp,
        ));
        assert_eq!(result.category(), category);
        assert_eq!(result.stage(), DocumentSaveStage::Preconditions);
    }
    let outcome = save(&template, &document, &edits).expect("no-op");
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert!(outcome.document().updated_at_utc() == NOW);
    assert!(
        !std::ptr::eq(outcome.document(), &document),
        "candidate must be independently owned"
    );
    assert_idempotent(&template, &outcome, &edits);
}

#[test]
fn snapshot_creation_covers_archived_fields_single_multi_mixed_and_unset() {
    let mut tr = template_raw();
    for id in [7, 8] {
        tr["fields"][key(id)]["futureDefinition"] = json!("__lexemes__");
        tr["fields"][key(id)]["configuration"]["options"][key(id * 10 + 3)]["futureOption"] =
            json!("__lexemes__");
    }
    for id in [1, 3, 7, 8] {
        archive_field(&mut tr, id);
    }
    let template = template(&tr);
    let mut dr = document_raw();
    dr["fieldValues"][key(3)] = json!({"kind":"unset"});
    dr["fieldValues"][key(7)] = json!({"kind":"singleChoice","optionId":key(73)});
    dr["fieldValues"][key(8)] = json!({"kind":"multiChoice","optionIds":[key(81),key(83)]});
    let document = document(&dr);
    let before = encode_document(&document).expect("source encode");
    let view = reconcile_document(&template, &document).expect("readable without snapshots");
    assert!(view.materialization_required());
    assert!(view.can_materialize());
    assert_eq!(view.orphan_fields().len(), 4);
    let edits = DocumentEditSet::default();
    let outcome = save(&template, &document, &edits).expect("snapshot creation");
    assert_eq!(outcome.document().orphaned_field_definitions().len(), 4);
    for id in [1, 3, 7, 8] {
        let snapshot = outcome
            .document()
            .orphaned_field_definitions()
            .get(&fid(id))
            .expect("snapshot created");
        assert!(snapshot.label() == template.fields()[&fid(id)].label());
        assert_eq!(snapshot.kind(), template.fields()[&fid(id)].kind());
        assert_eq!(
            snapshot.options().keys().copied().collect::<BTreeSet<_>>(),
            selected_options(&document.field_values()[&fid(id)])
        );
        assert!(!snapshot.contains_unknown_storage_data());
    }
    assert_eq!(outcome.warnings().len(), 2);
    assert!(encode_document(&document).expect("source unchanged") == before);
    let legacy = materialize_document(&template, template.revision(), &document, LATER.into())
        .expect("legacy snapshots");
    assert!(
        encode_document(legacy.document().expect("changed")).expect("legacy encode")
            == encode_document(outcome.document()).expect("new encode")
    );
    assert_idempotent(&template, &outcome, &edits);
}

#[test]
fn creation_finalization_and_no_edit_save_share_snapshot_requirements() {
    let mut tr = template_raw();
    tr["fields"][key(2)]["defaultValue"]["future"] = json!("__lexemes__");
    for id in [1, 7, 8] {
        archive_field(&mut tr, id);
    }
    let template = template(&tr);
    let created = create_document(
        &template,
        template.revision(),
        DocumentId::from_str(&key(200)).expect("id"),
        SECRET.into(),
        NOW.into(),
    )
    .expect("creation");
    let document = created.document();
    assert!(document.field_values().values().all(|v| v.is_unset()));
    assert!(template.fields()[&fid(2)].default_value().kind().is_some());
    assert_eq!(document.orphaned_field_definitions().len(), 3);
    for id in [1, 7, 8] {
        assert!(document.field_values()[&fid(id)].is_unset());
        assert!(document.orphaned_field_definitions()[&fid(id)]
            .options()
            .is_empty());
    }
    let outcome =
        save(&template, document, &DocumentEditSet::default()).expect("save after creation");
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_idempotent(&template, &outcome, &DocumentEditSet::default());
    let legacy = materialize_document(&template, template.revision(), document, LATER.into())
        .expect("legacy unchanged");
    assert_eq!(legacy.kind(), DocumentMaterializationOutcomeKind::Unchanged);
}

#[test]
fn new_choice_ids_must_be_active_but_existing_archived_members_can_remain() {
    let template = template(&template_raw());
    let current = document(&document_raw());
    failure(
        &template,
        &current,
        &set(7, DocumentValueEdit::single_choice(oid(73))),
        Category::ArchivedOptionAdded,
    );
    failure(
        &template,
        &current,
        &set(8, DocumentValueEdit::multi_choice(vec![oid(81), oid(83)])),
        Category::ArchivedOptionAdded,
    );
    failure(
        &template,
        &current,
        &set(7, DocumentValueEdit::single_choice(oid(999))),
        Category::UnknownOption,
    );
    failure(
        &template,
        &current,
        &set(7, DocumentValueEdit::single_choice(oid(81))),
        Category::UnknownOption,
    );
    let mut raw = document_raw();
    raw["fieldValues"][key(7)] = json!({"kind":"singleChoice","optionId":key(73)});
    raw["fieldValues"][key(8)] = json!({"kind":"multiChoice","optionIds":[key(81),key(83)]});
    let source = document(&raw);
    let edits = DocumentEditSet::new(vec![
        DocumentEdit::SetValue(fid(7), DocumentValueEdit::single_choice(oid(73))),
        DocumentEdit::SetValue(
            fid(8),
            DocumentValueEdit::multi_choice(vec![oid(82), oid(83)]),
        ),
    ]);
    let outcome = save(&template, &source, &edits).expect("preserve archived and add active");
    assert_eq!(outcome.warnings().len(), 2);
    assert_eq!(outcome.document().orphaned_field_definitions().len(), 2);
    assert_idempotent(&template, &outcome, &edits);
    let clear = DocumentEditSet::new(vec![
        DocumentEdit::Unset(fid(7)),
        DocumentEdit::SetValue(fid(8), DocumentValueEdit::multi_choice(vec![oid(82)])),
    ]);
    let cleared = save(&template, outcome.document(), &clear).expect("remove archived selections");
    assert!(
        cleared.warnings().is_empty(),
        "intermediate warning leaked into final outcome"
    );
    assert!(cleared.document().orphaned_field_definitions().is_empty());
    assert_idempotent(&template, &cleared, &clear);
}

#[test]
fn snapshot_membership_keeps_history_and_rejects_metadata_loss() {
    let mut tr = template_raw();
    tr["fields"][key(8)]["label"] = json!("renamed field");
    tr["fields"][key(8)]["configuration"]["options"][key(81)]["label"] = json!("renamed option");
    let template = template(&tr);
    let mut raw = document_raw();
    raw["fieldValues"][key(8)] = json!({"kind":"multiChoice","optionIds":[key(81),key(83)]});
    raw["orphanedFieldDefinitions"][key(8)] = snapshot(
        "multiChoice",
        json!({
            (key(81)):{"label":"historical a"},
            (key(83)):{"label":"historical archived"}
        }),
    );
    let source = document(&raw);
    let before = encode_document(&source).expect("source before rename cases");
    let keep =
        save(&template, &source, &DocumentEditSet::default()).expect("keep archived history");
    assert_eq!(keep.warnings().len(), 1);
    assert!(keep.document().orphaned_field_definitions() == source.orphaned_field_definitions());
    assert_idempotent(&template, &keep, &DocumentEditSet::default());
    let removal = set(8, DocumentValueEdit::multi_choice(vec![oid(81)]));
    let cleared =
        save(&template, &source, &removal).expect("last archived selection removed after rename");
    assert!(cleared.document().orphaned_field_definitions().is_empty());
    assert!(cleared.warnings().is_empty());
    assert_idempotent(&template, &cleared, &removal);
    assert!(encode_document(&source).expect("source unchanged") == before);
    // 제거할 Option뿐 아니라 재결합으로 없어질 Field/남은 Option extra도 차단한다.
    for remaining_option in [false, true] {
        let mut extra = raw.clone();
        if remaining_option {
            extra["orphanedFieldDefinitions"][key(8)]["options"][key(81)]["future"] =
                json!("__lexemes__");
        } else {
            extra["orphanedFieldDefinitions"][key(8)]["future"] = json!("__lexemes__");
        }
        assert_eq!(
            failure(
                &template,
                &document(&extra),
                &removal,
                Category::SnapshotBlocked
            )
            .issue()
            .expect("loss issue")
            .category(),
            Issue::LossyOrphanReattachment
        );
    }
    raw["orphanedFieldDefinitions"][key(8)]["options"][key(83)]["future"] = json!("__lexemes__");
    raw["orphanedFieldDefinitions"][key(8)]["future"] = json!("__lexemes__");
    let source = document(&raw);
    let edits = set(8, DocumentValueEdit::multi_choice(vec![oid(82), oid(83)]));
    let outcome = save(&template, &source, &edits).expect("safe membership update");
    let snap = outcome
        .document()
        .orphaned_field_definitions()
        .get(&fid(8))
        .expect("retained snapshot");
    assert_eq!(
        snap.options().keys().copied().collect::<Vec<_>>(),
        vec![oid(82), oid(83)]
    );
    assert!(snap.options()[&oid(82)].label() == "b");
    assert!(snap.options()[&oid(83)].label() == "historical archived");
    let old = lossless(&encode_document(&source).expect("old"));
    let new = lossless(&encode_document(outcome.document()).expect("new"));
    for path in [
        vec![
            "orphanedFieldDefinitions".to_owned(),
            key(8),
            "future".to_owned(),
        ],
        vec![
            "orphanedFieldDefinitions".to_owned(),
            key(8),
            "options".to_owned(),
            key(83),
        ],
    ] {
        let path = path.iter().map(String::as_str).collect::<Vec<_>>();
        assert_preserved(&old, &path, &new, &path);
    }
    assert_idempotent(&template, &outcome, &edits);
    let removal = set(8, DocumentValueEdit::multi_choice(vec![oid(81)]));
    assert_eq!(
        failure(&template, &source, &removal, Category::SnapshotBlocked)
            .issue()
            .expect("issue")
            .category(),
        Issue::LossySnapshotMembership
    );
    raw["orphanedFieldDefinitions"][key(8)]["options"][key(81)]["future"] = json!("__lexemes__");
    failure(
        &template,
        &document(&raw),
        &edits,
        Category::SnapshotBlocked,
    );
}

#[test]
fn active_reattachment_allows_label_renames_but_requires_compatible_kind_and_no_unknown_data() {
    let tr = template_raw();
    let template = template(&tr);
    let mut raw = document_raw();
    raw["orphanedFieldDefinitions"][key(7)] =
        snapshot("singleChoice", json!({(key(71)):{"label":"a"}}));
    // Field와 Option label을 각각 바꿔 어느 쪽도 재결합의 필수 조건이 아님을 확인한다.
    for label_case in 0..4 {
        let mut changed = raw.clone();
        match label_case {
            0 => {}
            1 => changed["orphanedFieldDefinitions"][key(7)]["label"] = json!("historical field"),
            2 => {
                changed["orphanedFieldDefinitions"][key(7)]["options"][key(71)]["label"] =
                    json!("historical option")
            }
            3 => {
                changed["orphanedFieldDefinitions"][key(1)] =
                    json!({"kind":"singleLineText","label":"historical text","options":{}})
            }
            _ => unreachable!("closed label cases"),
        }
        let source = document(&changed);
        let before = (
            encode_template(&template).expect("Template"),
            encode_document(&source).expect("source"),
        );
        let view = reconcile_document(&template, &source).expect("label-only reconciliation");
        assert!(view.blocking_issues().is_empty());
        let edits = DocumentEditSet::default();
        let outcome = save(&template, &source, &edits).expect("label-only reattachment");
        assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
        assert!(outcome.document().orphaned_field_definitions().is_empty());
        let materialized =
            materialize_document(&template, template.revision(), &source, LATER.into())
                .expect("label-only materialization")
                .into_changed()
                .expect("snapshot removed");
        assert!(
            encode_document(materialized.document()).expect("materialized")
                == encode_document(outcome.document()).expect("saved")
        );
        assert_idempotent(&template, &outcome, &edits);
        assert!(encode_template(&template).expect("Template unchanged") == before.0);
        assert!(encode_document(&source).expect("source unchanged") == before.1);
    }
    for option_extra in [false, true] {
        let mut changed = raw.clone();
        if option_extra {
            changed["orphanedFieldDefinitions"][key(7)]["options"][key(71)]["future"] =
                json!("__lexemes__");
        } else {
            changed["orphanedFieldDefinitions"][key(7)]["future"] = json!("__lexemes__");
        }
        let result = failure(
            &template,
            &document(&changed),
            &DocumentEditSet::default(),
            Category::SnapshotBlocked,
        );
        assert_eq!(
            result.issue().expect("issue").category(),
            Issue::LossyOrphanReattachment
        );
    }
    let mut raw = document_raw();
    raw["fieldValues"][key(7)] = json!({"kind":"unset"});
    raw["orphanedFieldDefinitions"][key(7)] = snapshot("number", json!({}));
    assert_eq!(
        failure(
            &template,
            &document(&raw),
            &DocumentEditSet::default(),
            Category::SnapshotBlocked
        )
        .issue()
        .expect("issue")
        .category(),
        Issue::ReattachmentSnapshotConflict
    );
}

#[test]
fn unknown_field_requires_snapshot_but_remains_readable_with_id_fallback() {
    let template = template(&template_raw());
    let mut raw = document_raw();
    raw["fieldValues"][key(99)] = json!({"kind":"text","value":SECRET,"future":"__lexemes__"});
    let source = document(&raw);
    let view = reconcile_document(&template, &source).expect("unknown readable");
    assert!(view.orphan_fields()[0].uses_id_fallback());
    assert_eq!(
        view.blocking_issues()[0].category(),
        Issue::OrphanSnapshotMissing
    );
    assert_eq!(
        failure(
            &template,
            &source,
            &DocumentEditSet::default(),
            Category::SnapshotBlocked
        )
        .issue()
        .expect("issue")
        .category(),
        Issue::OrphanSnapshotMissing
    );
    raw["orphanedFieldDefinitions"][key(99)] = snapshot("singleLineText", json!({}));
    raw["orphanedFieldDefinitions"][key(99)]["future"] = json!("__lexemes__");
    let source = document(&raw);
    let outcome =
        save(&template, &source, &DocumentEditSet::default()).expect("complete unknown preserved");
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_idempotent(&template, &outcome, &DocumentEditSet::default());
}

#[test]
fn no_label_evidence_blocks_snapshot_creation_without_inventing_labels() {
    let mut raw = template_raw();
    archive_field(&mut raw, 8);
    let template = template(&raw);
    let mut raw = document_raw();
    raw["fieldValues"][key(8)] = json!({"kind":"multiChoice","optionIds":[key(81),key(999)]});
    let source = document(&raw);
    let view = reconcile_document(&template, &source).expect("unknown option readable");
    assert!(view
        .blocking_issues()
        .iter()
        .any(|issue| issue.category() == Issue::OptionSnapshotUnavailable));
    let result = failure(
        &template,
        &source,
        &DocumentEditSet::default(),
        Category::SnapshotBlocked,
    );
    assert_eq!(
        result.issue().expect("snapshot issue").category(),
        Issue::OptionSnapshotUnavailable
    );
}

#[test]
fn source_admission_binding_lifecycle_priority_and_final_validation_are_separate() {
    let template = template(&template_raw());
    let document = document(&document_raw());
    let edits = set(1, DocumentValueEdit::single_line_text("fixed".into()));
    let mut invalid = document.clone();
    invalid.corrupt_scalar_value_for_test(fid(1), "bad\ntext");
    assert_eq!(
        failure(&template, &invalid, &edits, Category::InvalidDocument)
            .storage_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    let mut invalid_template = template.clone();
    invalid_template.corrupt_field_order_for_test();
    failure(
        &invalid_template,
        &invalid,
        &edits,
        Category::InvalidTemplate,
    );
    let mut mismatch = document.clone();
    mismatch.template_id = crate::data::artifact::TemplateId::from_str(&key(999)).expect("id");
    let result = error(prepare_document_save(
        &template,
        rev(1),
        &mismatch,
        &edits,
        "bad",
    ));
    assert_eq!(result.category(), Category::TemplateIdMismatch);
    let mut raw = template_raw();
    raw["lifecycle"] = json!("deleted");
    failure(
        &super::tests::template(&raw),
        &document,
        &edits,
        Category::TemplateIsTombstoned,
    );
    let mut future = document.clone();
    future.template_revision = rev(4);
    failure(&template, &future, &edits, Category::FutureDocumentRevision);
    let mut invalid_final = document.clone();
    invalid_final.corrupt_extra_for_test("parentId", json!(SECRET));
    let result = error(finalize_save(
        &template,
        invalid_final,
        DocumentSaveOutcomeKind::Changed,
    ));
    assert_eq!(result.category(), Category::InvalidCandidate);
    assert_eq!(result.stage(), DocumentSaveStage::FinalStorage);
    let mut bound_invalid = document.clone();
    bound_invalid
        .field_values
        .insert(fid(7), FieldValue::from_single_choice(oid(999)));
    assert!(bound_invalid.validate_storage().is_ok());
    let result = error(finalize_save(
        &template,
        bound_invalid,
        DocumentSaveOutcomeKind::Changed,
    ));
    assert_eq!(result.category(), Category::BlockingIssues);
    assert_eq!(
        result.issue().expect("issue").choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );
    let mut incomplete = document.clone();
    incomplete.template_revision = rev(2);
    assert!(incomplete.validate_storage().is_ok());
    let result = error(finalize_save(
        &template,
        incomplete,
        DocumentSaveOutcomeKind::Changed,
    ));
    assert_eq!(result.category(), Category::IncompleteCandidate);
    assert_eq!(result.stage(), DocumentSaveStage::FinalReconciliation);
}

fn lossless(bytes: &[u8]) -> LosslessJsonValue {
    parse_strict_lossless_json_object(bytes).expect("strict lossless fixture")
}

fn assert_preserved(
    source: &LosslessJsonValue,
    source_path: &[&str],
    target: &LosslessJsonValue,
    target_path: &[&str],
) {
    let source = source
        .object_path(source_path)
        .expect("missing source subtree");
    let target = target
        .object_path(target_path)
        .expect("missing target subtree");
    assert!(source == target, "preserved subtree mismatch");
}

mod preservation;
