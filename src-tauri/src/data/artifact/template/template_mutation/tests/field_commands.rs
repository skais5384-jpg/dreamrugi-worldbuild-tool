use std::{collections::BTreeSet, error::Error};

use crate::data::artifact::{
    ArtifactChoiceValueLocation, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
    ArtifactValidationErrorCategory, FieldKind, FieldLifecycle,
};
use crate::data::field_engine::{
    choice::ChoiceValidationErrorCategory,
    rich_text::{
        normalize_rich_text, NormalizedRichText, RichTextValidationErrorCategory,
        RICH_TEXT_SCHEMA_VERSION,
    },
    scalar::ScalarValueErrorCategory,
};

use super::*;

const FIELD_NEW_TEXT: &str = "b1000000-0000-4000-8000-000000000001";
const FIELD_NEW_RICH: &str = "b1000000-0000-4000-8000-000000000002";
const FIELD_NEW_NUMBER: &str = "b1000000-0000-4000-8000-000000000003";
const FIELD_NEW_DATE: &str = "b1000000-0000-4000-8000-000000000004";
const FIELD_NEW_TIME: &str = "b1000000-0000-4000-8000-000000000005";
const FIELD_NEW_DURATION: &str = "b1000000-0000-4000-8000-000000000006";
const FIELD_NEW_SINGLE: &str = "b1000000-0000-4000-8000-000000000007";
const FIELD_NEW_MULTI: &str = "b1000000-0000-4000-8000-000000000008";
const FIELD_UNKNOWN: &str = "b1000000-0000-4000-8000-000000000099";
const OPTION_NEW_A: &str = "c1000000-0000-4000-8000-000000000001";
const OPTION_NEW_B: &str = "c2000000-0000-4000-8000-000000000002";

fn provenance_fixture(payload: &str, text: &str, owner: &str) -> TemplateArtifact {
    let mut value = fixture_value();
    value["templateId"] = json!(owner);
    value["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"text", "value":text, "payload":"__PAYLOAD__"});
    value["fields"][FIELD_NEW_TEXT] = value["fields"][FIELD_TEXT].clone();
    value["fieldOrder"]
        .as_array_mut()
        .unwrap()
        .push(json!(FIELD_NEW_TEXT));
    value["stableDraftSibling"] = json!("__SIBLING__");
    let raw = serde_json::to_string(&value)
        .unwrap()
        .replace("\"__PAYLOAD__\"", payload)
        .replace("\"__SIBLING__\"", "9E109");
    decode_template(raw.as_bytes()).unwrap()
}

#[test]
fn provenance_free_donors_never_inherit_destination_number_lexemes() {
    let cases = [
        ("[1E100,1e100]", "[1e100,1E100]"),
        (
            r#"[{"n":1E100},{"n":1e100}]"#,
            r#"[{"n":1e100},{"n":1E100}]"#,
        ),
        ("[-0E100,0]", "[0,-0E100]"),
        (r#"{"n":1E100}"#, r#"{"n":1e100}"#),
        (r#"[{"n":1E100}]"#, r#"[{"n":1e100}]"#),
        (r#"{"b":[1E100,1e100]}"#, r#"{"b":[1e100,1E100]}"#),
        (r#"{"from":1E100}"#, r#"{"to":1e100}"#),
        (
            r#"[{"id":"a","n":1E100},{"id":"b","n":1e100}]"#,
            r#"[{"id":"b","n":1e100},{"id":"a","n":1E100}]"#,
        ),
    ];
    for (original, replacement) in cases {
        let source = provenance_fixture(original, "old", TEMPLATE_ID);
        let donor = provenance_fixture(replacement, "new", OTHER_TEMPLATE_ID);
        let before = encode_template(&source).unwrap();
        let donor_before = encode_template(&donor).unwrap();
        assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::set_current_default(
                id(FIELD_TEXT),
                FieldValueDraft::provenance_free_replacement(
                    donor.fields()[&id(FIELD_TEXT)].default_value().clone(),
                ),
            ),
            TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
            Some(id(FIELD_TEXT)),
        );
        assert_eq!(encode_template(&donor).unwrap(), donor_before);
        // source-owned로 표시해도 다른 Template의 증표는 이전 목적지에 사용할 수 없다.
        assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::set_current_default(
                id(FIELD_TEXT),
                donor.current_default_draft(id(FIELD_TEXT)).unwrap(),
            ),
            TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch,
            Some(id(FIELD_TEXT)),
        );
    }
}

#[test]
fn source_owned_drafts_bind_field_and_immutable_snapshot_without_equality_inference() {
    let source = provenance_fixture("[1E100,1e100]", "old", TEMPLATE_ID);
    let before = encode_template(&source).unwrap();
    let field_id = id(FIELD_TEXT);
    let owned = source.current_default_draft(field_id).unwrap();
    let immutable_clone = source.clone();
    // Cloneは同じimmutable snapshotなので再利用してもvalue/tokenの所有者は変わらない。
    for target in [&source, &immutable_clone] {
        assert!(run(
            target,
            LATER_AT,
            TemplateMutationCommand::set_current_default(field_id, owned.clone())
        )
        .unwrap()
        .is_unchanged());
        assert_eq!(encode_template(target).unwrap(), before);
    }
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(id(FIELD_NEW_TEXT), owned.clone()),
        TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch,
        Some(id(FIELD_NEW_TEXT)),
    );
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(
            id(FIELD_NEW_TEXT),
            FieldValueDraft::provenance_free_replacement(
                source.fields()[&field_id].default_value().clone(),
            ),
        ),
        TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
        Some(id(FIELD_NEW_TEXT)),
    );
    let separately_decoded = decode_template(&before).unwrap();
    assert_eq!(source, separately_decoded); // domain equalityは成立しても別snapshotは承認しない。
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(
            field_id,
            separately_decoded.current_default_draft(field_id).unwrap(),
        ),
        TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch,
        Some(field_id),
    );
    let fresh = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            field_id,
            FieldValueDraft::single_line_text("new".into()),
        ),
    )
    .unwrap()
    .into_changed()
    .unwrap();
    let fresh_bytes = encode_template(&fresh).unwrap();
    let text = std::str::from_utf8(&fresh_bytes).unwrap();
    assert!(text.contains("1E100") && text.contains("1e100") && text.contains("9E109"));
    assert_eq!(
        fresh.revision(),
        source.revision().checked_increment().unwrap()
    );
    assert_eq!(fresh.updated_at_utc(), LATER_AT);
    assert_command_error(
        &fresh,
        &fresh_bytes,
        TemplateMutationCommand::set_current_default(field_id, owned),
        TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch,
        Some(field_id),
    );
    assert!(run(
        &fresh,
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            field_id,
            fresh.current_default_draft(field_id).unwrap()
        )
    )
    .unwrap()
    .is_unchanged());
    assert!(run(
        &fresh,
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            field_id,
            FieldValueDraft::single_line_text("new".into())
        )
    )
    .unwrap()
    .is_unchanged());
    let error = apply_template_mutation(
        &fresh,
        source.revision(),
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            field_id,
            fresh.current_default_draft(field_id).unwrap(),
        ),
    )
    .unwrap_err();
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    let error = run(
        &fresh,
        "invalid",
        TemplateMutationCommand::set_current_default(
            field_id,
            fresh.current_default_draft(field_id).unwrap(),
        ),
    )
    .unwrap_err();
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::InvalidTimestamp
    );
    assert_eq!(encode_template(&source).unwrap(), before);
    assert_round_trip(&fresh);
    // 새 Field의 initial default에도 current 전용 증표를 재사용할 수 없다.
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            draft(
                id(FIELD_NEW_NUMBER),
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                source.current_default_draft(field_id).unwrap(),
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(id(FIELD_NEW_NUMBER)),
    );
}

#[test]
fn normalized_rich_text_equality_cannot_issue_unknown_metadata_ownership() {
    let mut value = fixture_value();
    value["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["futureSame"] =
        json!({"n":1});
    let source = fixture_from_value(&value);
    let before = encode_template(&source).unwrap();
    let field_id = id(FIELD_RICH_TEXT);
    let reconstructed = normalized_rich_text_draft(
        value["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"].clone(),
    );
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(field_id, reconstructed),
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(field_id),
    );
    assert!(run(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            field_id,
            source.current_default_draft(field_id).unwrap()
        )
    )
    .unwrap()
    .is_unchanged());
}

fn run(
    source: &TemplateArtifact,
    timestamp: &str,
    command: TemplateMutationCommand,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    apply_template_mutation(source, source.revision(), timestamp, command)
}

fn draft(
    field_id: FieldId,
    kind: FieldKind,
    configuration: NewFieldConfiguration,
    first_default: FieldValueDraft,
) -> NewFieldDraft {
    NewFieldDraft::new(
        field_id,
        format!("new {kind:?}"),
        kind,
        configuration,
        true,
        Some("new-field-token".to_owned()),
        first_default,
    )
}

fn simple_draft(field_id: FieldId) -> NewFieldDraft {
    draft(
        field_id,
        FieldKind::SingleLineText,
        NewFieldConfiguration::single_line_text(),
        FieldValueDraft::single_line_text("first value".to_owned()),
    )
}

fn normalized_rich_text_draft(content: Value) -> FieldValueDraft {
    let content = content
        .as_object()
        .expect("rich-text command fixture must be an object");
    FieldValueDraft::from_normalized_rich_text(
        normalize_rich_text(RICH_TEXT_SCHEMA_VERSION, content)
            .expect("rich-text command fixture must normalize"),
    )
}

fn existing_default_from_value(value: &Value, field_id: FieldId) -> FieldValue {
    fixture_from_value(value).fields()[&field_id]
        .default_value()
        .clone()
}

fn assert_command_error(
    source: &TemplateArtifact,
    before: &[u8],
    command: TemplateMutationCommand,
    category: TemplateMutationErrorCategory,
    field_id: Option<FieldId>,
) -> TemplateMutationError {
    let error = run(source, LATER_AT, command).expect_err("command must fail");
    assert_eq!(error.category(), category);
    assert_eq!(error.field_id(), field_id);
    assert_eq!(error.option_id(), None);
    assert_eq!(
        encode_template(source).expect("failed command must not mutate source"),
        before
    );
    error
}

fn assert_round_trip(template: &TemplateArtifact) {
    let encoded = encode_template(template).expect("changed Template must encode");
    let decoded = decode_template(&encoded).expect("changed Template must decode");
    assert_eq!(
        encode_template(&decoded).expect("decoded Template must re-encode"),
        encoded
    );
}

fn assert_source_unknown_extras_preserved(candidate: &TemplateArtifact) {
    let source = fixture_value();
    let candidate = template_wire_value(candidate);
    for path in [
        &["futureRoot"][..],
        &["presentation", "futurePresentation"][..],
        &["fields", FIELD_TEXT, "futureField"][..],
        &["fields", FIELD_TEXT, "configuration", "futureConfig"][..],
        &["fields", FIELD_TEXT, "defaultValue", "futureValue"][..],
        &["fields", FIELD_TEXT, "initialDefaultValue", "futureInitial"][..],
        &[
            "fields",
            FIELD_TEXT,
            "presentation",
            "futureFieldPresentation",
        ][..],
    ] {
        let expected = path.iter().fold(&source, |value, member| &value[*member]);
        let actual = path
            .iter()
            .fold(&candidate, |value, member| &value[*member]);
        assert_eq!(actual, expected, "unknown extra path {path:?} must survive");
    }
}

#[test]
fn creates_all_eight_field_kinds_with_engine_owned_history_and_order() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let option_a = option_id(OPTION_NEW_A);
    let option_b = option_id(OPTION_NEW_B);
    let rich_value = FieldValueDraft::from_normalized_rich_text(
        normalize_rich_text(
            RICH_TEXT_SCHEMA_VERSION,
            json!({
                "kind": "root",
                "children": [{
                    "kind": "paragraph",
                    "children": [
                        { "kind": "text", "text": " 새 본문 한글 ", "marks": ["bold"] },
                        { "kind": "hardBreak" },
                        { "kind": "text", "text": "second line", "marks": ["italic"] }
                    ]
                }]
            })
            .as_object()
            .expect("fresh rich-text fixture must be an object"),
        )
        .expect("fresh rich-text fixture must normalize"),
    );
    let single_options = vec![NewChoiceOptionDraft::new(option_a, "A".to_owned())];
    let multi_options = vec![
        NewChoiceOptionDraft::new(option_a, "A".to_owned()),
        NewChoiceOptionDraft::new(option_b, "B".to_owned()),
    ];
    let cases = vec![
        (
            id(FIELD_NEW_TEXT),
            FieldKind::SingleLineText,
            NewFieldConfiguration::single_line_text(),
            FieldValueDraft::single_line_text("created".to_owned()),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_RICH),
            FieldKind::RichText,
            NewFieldConfiguration::rich_text(),
            rich_value,
            NewFieldInsertion::Append,
        ),
        (
            id(FIELD_NEW_NUMBER),
            FieldKind::Number,
            NewFieldConfiguration::number(),
            FieldValueDraft::number("12.5".to_owned()),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_DATE),
            FieldKind::Date,
            NewFieldConfiguration::date(),
            FieldValueDraft::date("2026-09-05".to_owned()),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_TIME),
            FieldKind::Time,
            NewFieldConfiguration::time(),
            FieldValueDraft::time("09:30:00.000".to_owned()),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_DURATION),
            FieldKind::Duration,
            NewFieldConfiguration::duration(),
            FieldValueDraft::duration("1000".to_owned()),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_SINGLE),
            FieldKind::SingleChoice,
            NewFieldConfiguration::single_choice(vec![option_a], single_options),
            FieldValueDraft::single_choice(option_a),
            NewFieldInsertion::At(1),
        ),
        (
            id(FIELD_NEW_MULTI),
            FieldKind::MultiChoice,
            // 표시 순서는 caller 입력을 따르고 값 배열만 canonical OptionId 순서를 사용한다.
            NewFieldConfiguration::multi_choice(vec![option_b, option_a], multi_options),
            FieldValueDraft::multi_choice(vec![option_a, option_b]),
            NewFieldInsertion::At(1),
        ),
    ];

    for (field_id, kind, configuration, first_default, insertion) in cases {
        let expected_default = first_default.clone().into_value();
        let changed = run(
            &source,
            LATER_AT,
            TemplateMutationCommand::create_field(
                draft(field_id, kind, configuration, first_default),
                insertion,
            ),
        )
        .expect("valid Field draft must succeed")
        .into_changed()
        .expect("Field creation must change the Template");
        let created = &changed.fields()[&field_id];

        assert_eq!(changed.revision().get(), source.revision().get() + 1);
        assert_eq!(changed.updated_at_utc(), LATER_AT);
        assert_eq!(created.lifecycle(), FieldLifecycle::Active);
        assert_eq!(created.kind(), kind);
        assert_eq!(created.introduced_revision(), changed.revision());
        assert_eq!(created.default_value(), &expected_default);
        assert_eq!(created.initial_default_value(), &expected_default);
        assert!(created.required());
        assert_eq!(created.presentation().token(), Some("new-field-token"));
        for (persisted_id, persisted) in source.fields() {
            assert_eq!(&changed.fields()[persisted_id], persisted);
        }
        let expected_index = match insertion {
            NewFieldInsertion::Append => source.field_order().len(),
            NewFieldInsertion::At(index) => index,
        };
        assert_eq!(changed.field_order()[expected_index], field_id);
        if kind == FieldKind::MultiChoice {
            assert_eq!(
                created.configuration().option_order(),
                Some([option_b, option_a].as_slice())
            );
            assert!(created
                .configuration()
                .options()
                .unwrap()
                .values()
                .all(|option| option.lifecycle() == OptionLifecycle::Active));
        }
        assert_eq!(
            template_wire_value(&changed)["futureRoot"],
            fixture_value()["futureRoot"]
        );
        assert_source_unknown_extras_preserved(&changed);
        assert_round_trip(&changed);
        assert_eq!(
            encode_template(&source).expect("creation must not mutate source"),
            before
        );
    }
}

#[test]
fn create_field_rejects_id_position_kind_value_and_choice_contract_violations() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    for existing in [id(FIELD_TEXT), id(FIELD_ARCHIVED)] {
        assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::create_field(
                simple_draft(existing),
                NewFieldInsertion::Append,
            ),
            TemplateMutationErrorCategory::FieldAlreadyExists,
            Some(existing),
        );
    }

    let new_field = id(FIELD_NEW_TEXT);
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            simple_draft(new_field),
            NewFieldInsertion::At(source.field_order().len() + 1),
        ),
        TemplateMutationErrorCategory::InvalidInsertionPosition,
        Some(new_field),
    );

    for (invalid, validation_category) in [
        (
            draft(
                new_field,
                FieldKind::Number,
                NewFieldConfiguration::single_line_text(),
                FieldValueDraft::number("1".to_owned()),
            ),
            None,
        ),
        (
            draft(
                new_field,
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                FieldValueDraft::number("1".to_owned()),
            ),
            Some(ArtifactValidationErrorCategory::FieldValueKindMismatch),
        ),
        (
            draft(
                new_field,
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                FieldValueDraft::single_line_text("line one\nline two".to_owned()),
            ),
            Some(ArtifactValidationErrorCategory::InvalidScalarValue),
        ),
        (
            draft(
                new_field,
                FieldKind::MultiChoice,
                NewFieldConfiguration::multi_choice(Vec::new(), Vec::new()),
                FieldValueDraft::multi_choice(Vec::new()),
            ),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue),
        ),
    ] {
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::create_field(invalid, NewFieldInsertion::Append),
            TemplateMutationErrorCategory::InvalidFieldDraft,
            Some(new_field),
        );
        assert_eq!(error.validation_category(), validation_category);
        if validation_category == Some(ArtifactValidationErrorCategory::InvalidScalarValue) {
            assert_eq!(
                error.validation_scalar_location(),
                Some(ArtifactScalarValueLocation::CurrentDefault)
            );
        }
        if validation_category == Some(ArtifactValidationErrorCategory::InvalidChoiceValue) {
            assert_eq!(
                error.validation_choice_location(),
                Some(ArtifactChoiceValueLocation::CurrentDefault)
            );
        }
    }

    let mut invalid_rich_source = source.clone();
    invalid_rich_source.replace_rich_text_default_content_for_test(
        id(FIELD_RICH_TEXT),
        serde_json::Map::from_iter([
            ("kind".to_owned(), json!("root")),
            ("children".to_owned(), json!([])),
        ]),
        false,
    );
    let invalid_rich = invalid_rich_source.fields()[&id(FIELD_RICH_TEXT)]
        .default_value()
        .clone();
    let error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            draft(
                new_field,
                FieldKind::RichText,
                NewFieldConfiguration::rich_text(),
                FieldValueDraft::provenance_free_replacement(invalid_rich),
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(new_field),
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidRichTextValue)
    );
    assert_eq!(
        error.validation_rich_text_location(),
        Some(ArtifactRichTextValueLocation::CurrentDefault)
    );
    assert_eq!(
        error.validation_rich_text_category(),
        Some(RichTextValidationErrorCategory::SemanticEmpty)
    );

    let copied_extra = source.fields()[&id(FIELD_TEXT)].default_value().clone();
    let copied_extra_error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            draft(
                new_field,
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                FieldValueDraft::provenance_free_replacement(copied_extra),
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(new_field),
    );
    assert_eq!(copied_extra_error.validation_category(), None);
}

#[test]
fn create_choice_field_rejects_archived_duplicate_foreign_and_non_bijective_options() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let field_id = id(FIELD_NEW_SINGLE);
    let option_a = option_id(OPTION_NEW_A);
    let option_b = option_id(OPTION_NEW_B);

    let cases = [
        (
            NewFieldConfiguration::single_choice(
                vec![option_a],
                vec![NewChoiceOptionDraft::archived_for_test(
                    option_a,
                    "archived".to_owned(),
                )],
            ),
            None,
        ),
        (
            NewFieldConfiguration::single_choice(
                vec![option_a],
                vec![
                    NewChoiceOptionDraft::new(option_a, "first".to_owned()),
                    NewChoiceOptionDraft::new(option_a, "duplicate".to_owned()),
                ],
            ),
            None,
        ),
        (
            NewFieldConfiguration::single_choice(
                vec![option_id(OPTION_ACTIVE_A)],
                vec![NewChoiceOptionDraft::new(
                    option_id(OPTION_ACTIVE_A),
                    "collision".to_owned(),
                )],
            ),
            Some(ArtifactValidationErrorCategory::DuplicateOptionId),
        ),
        (
            NewFieldConfiguration::single_choice(
                vec![option_a],
                vec![
                    NewChoiceOptionDraft::new(option_a, "A".to_owned()),
                    NewChoiceOptionDraft::new(option_b, "B".to_owned()),
                ],
            ),
            Some(ArtifactValidationErrorCategory::OptionOrderMismatch),
        ),
    ];
    for (configuration, validation_category) in cases {
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::create_field(
                draft(
                    field_id,
                    FieldKind::SingleChoice,
                    configuration,
                    FieldValueDraft::single_choice(option_a),
                ),
                NewFieldInsertion::Append,
            ),
            TemplateMutationErrorCategory::InvalidFieldDraft,
            Some(field_id),
        );
        assert_eq!(error.validation_category(), validation_category);
    }

    for default in [
        FieldValueDraft::single_choice(option_id(OPTION_ACTIVE_A)),
        FieldValueDraft::single_choice(option_b),
    ] {
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::create_field(
                draft(
                    field_id,
                    FieldKind::SingleChoice,
                    NewFieldConfiguration::single_choice(
                        vec![option_a],
                        vec![NewChoiceOptionDraft::new(option_a, "A".to_owned())],
                    ),
                    default,
                ),
                NewFieldInsertion::Append,
            ),
            TemplateMutationErrorCategory::InvalidFieldDraft,
            Some(field_id),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(
            error.validation_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
    }
}

#[test]
fn field_property_commands_have_changed_and_semantic_noop_paths() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let text = id(FIELD_TEXT);
    let initial_before =
        template_wire_value(&source)["fields"][FIELD_TEXT]["initialDefaultValue"].clone();
    let changed_cases = [
        TemplateMutationCommand::set_field_label(text, "renamed Field".to_owned()),
        TemplateMutationCommand::set_field_required(text, true),
        TemplateMutationCommand::set_field_presentation_token(text, Some("long".to_owned())),
        TemplateMutationCommand::set_current_default(
            text,
            FieldValueDraft::single_line_text("changed current".to_owned()),
        ),
        TemplateMutationCommand::set_current_default(text, FieldValueDraft::unset()),
    ];
    for command in changed_cases {
        let changed = run(&source, LATER_AT, command)
            .expect("valid active Field update must succeed")
            .into_changed()
            .expect("different Field property must change Template");
        assert_eq!(changed.revision().get(), source.revision().get() + 1);
        assert_eq!(changed.updated_at_utc(), LATER_AT);
        assert_eq!(
            template_wire_value(&changed)["fields"][FIELD_TEXT]["initialDefaultValue"],
            initial_before
        );
        assert_eq!(
            template_wire_value(&changed)["fields"][FIELD_TEXT]["futureField"],
            fixture_value()["fields"][FIELD_TEXT]["futureField"]
        );
        assert_source_unknown_extras_preserved(&changed);
        for (field_id, definition) in source.fields() {
            if *field_id != text {
                assert_eq!(&changed.fields()[field_id], definition);
            }
        }
        assert_round_trip(&changed);
        assert_eq!(encode_template(&source).unwrap(), before);
    }

    let source_field = &source.fields()[&text];
    let noop_cases = [
        TemplateMutationCommand::set_field_label(text, source_field.label().to_owned()),
        TemplateMutationCommand::set_field_required(text, source_field.required()),
        TemplateMutationCommand::set_field_presentation_token(
            text,
            source_field.presentation().token().map(str::to_owned),
        ),
        TemplateMutationCommand::set_current_default(
            text,
            source
                .current_default_draft(text)
                .expect("source owns this current default"),
        ),
    ];
    for command in noop_cases {
        assert!(run(&source, LATER_AT, command)
            .expect("same value must be a semantic no-op")
            .is_unchanged());
        assert_eq!(encode_template(&source).unwrap(), before);
    }
    let stale = apply_template_mutation(
        &source,
        revision(source.revision().get() - 1),
        LATER_AT,
        TemplateMutationCommand::set_field_label(text, source_field.label().to_owned()),
    )
    .expect_err("stale semantic no-op must still fail");
    assert_eq!(
        stale.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
}

#[test]
fn field_property_commands_reject_unknown_archived_and_invalid_defaults_atomically() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let archived = id(FIELD_ARCHIVED);
    let unknown = id(FIELD_UNKNOWN);

    for (target, expected) in [
        (archived, TemplateMutationErrorCategory::FieldIsArchived),
        (unknown, TemplateMutationErrorCategory::FieldNotFound),
    ] {
        for command in [
            TemplateMutationCommand::set_field_label(target, "blocked".to_owned()),
            TemplateMutationCommand::set_field_required(target, true),
            TemplateMutationCommand::set_field_presentation_token(
                target,
                Some("blocked".to_owned()),
            ),
            TemplateMutationCommand::set_current_default(target, FieldValueDraft::unset()),
        ] {
            assert_command_error(&source, &before, command, expected, Some(target));
        }
    }

    let text = id(FIELD_TEXT);
    for (value, scalar_category) in [
        ("", ScalarValueErrorCategory::NonCanonicalEmptyText),
        (
            "invalid\ntext",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
    ] {
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::set_current_default(
                text,
                FieldValueDraft::single_line_text(value.to_owned()),
            ),
            TemplateMutationErrorCategory::InvalidCurrentDefault,
            Some(text),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidScalarValue)
        );
        assert_eq!(
            error.validation_scalar_location(),
            Some(ArtifactScalarValueLocation::CurrentDefault)
        );
        assert_eq!(error.validation_scalar_category(), Some(scalar_category));
    }

    let choice = id(FIELD_CHOICE_A);
    for (invalid_option, choice_category) in [
        (
            option_id(OPTION_ARCHIVED_A),
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ),
        (
            option_id(OPTION_NEW_B),
            ChoiceValidationErrorCategory::UnknownSelectedOption,
        ),
    ] {
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::set_current_default(
                choice,
                FieldValueDraft::single_choice(invalid_option),
            ),
            TemplateMutationErrorCategory::InvalidCurrentDefault,
            Some(choice),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(
            error.validation_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
        assert_eq!(error.validation_choice_category(), Some(choice_category));
    }
}

#[test]
fn current_default_keeps_rich_metadata_fail_closed_and_redacts_payloads() {
    let source = fixture_from_value(&rich_text_unknown_fixture_value());
    let before = encode_template(&source).expect("metadata source must encode");
    let rich = id(FIELD_RICH_TEXT);
    let error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(rich, FieldValueDraft::unset()),
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(rich),
    );
    assert_eq!(error.validation_category(), None);

    let secret = "credential=command-secret path=C:\\private\\command.json /home/private/command";
    let redacted = assert_command_error(
        &fixture(),
        &encode_template(&fixture()).unwrap(),
        TemplateMutationCommand::set_current_default(
            id(FIELD_TEXT),
            FieldValueDraft::single_line_text(format!("{secret}\ninvalid")),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefault,
        Some(id(FIELD_TEXT)),
    );
    let mut rendered = format!("{redacted:?}\n{redacted}");
    let mut current = redacted.source();
    while let Some(source_error) = current {
        rendered.push_str(&format!("\n{source_error:?}\n{source_error}"));
        current = source_error.source();
    }
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("command.json"));

    let invalid_draft = NewFieldDraft::new(
        id(FIELD_NEW_TEXT),
        secret.to_owned(),
        FieldKind::Number,
        NewFieldConfiguration::single_line_text(),
        false,
        Some(secret.to_owned()),
        FieldValueDraft::number("1".to_owned()),
    );
    let draft_error = assert_command_error(
        &fixture(),
        &encode_template(&fixture()).unwrap(),
        TemplateMutationCommand::create_field(invalid_draft, NewFieldInsertion::Append),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(id(FIELD_NEW_TEXT)),
    );
    let draft_rendered = format!("{draft_error:?}\n{draft_error}");
    assert!(!draft_rendered.contains(secret));
    assert!(!draft_rendered.contains("command.json"));
}

#[test]
fn field_command_validation_error_keeps_only_safe_categories_and_one_field_id() {
    let sentinel =
        "option-label credential=choice-secret C:\\private\\choice.json /home/private/choice";
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_B]["configuration"]["options"][OPTION_ACTIVE_B1]["label"] =
        json!(sentinel);
    let source = fixture_from_value(&value);
    let before = encode_template(&source).unwrap();
    let target = id(FIELD_CHOICE_A);
    let foreign_option = option_id(OPTION_ACTIVE_B1);
    let error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(
            target,
            FieldValueDraft::single_choice(foreign_option),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefault,
        Some(target),
    );

    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        error.validation_choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );
    assert_eq!(
        error.validation_choice_location(),
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
    assert_eq!(error.option_id(), None);
    assert!(error.source().is_none());

    let rendered = format!("{error:?}\n{error}");
    for forbidden in [
        OPTION_ACTIVE_B1,
        OPTION_ACTIVE_B2,
        sentinel,
        "choice-secret",
        "choice.json",
        "C:\\private",
        "/home/private",
    ] {
        assert!(!rendered.contains(forbidden));
    }
    assert!(rendered.contains("InvalidCurrentDefault"));
    assert!(rendered.contains(&target.to_string()));

    let new_field = id(FIELD_NEW_SINGLE);
    let local_option = option_id(OPTION_NEW_A);
    let draft_error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            draft(
                new_field,
                FieldKind::SingleChoice,
                NewFieldConfiguration::single_choice(
                    vec![local_option],
                    vec![NewChoiceOptionDraft::new(local_option, sentinel.to_owned())],
                ),
                FieldValueDraft::single_choice(foreign_option),
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(new_field),
    );
    assert_eq!(
        draft_error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        draft_error.validation_choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );
    assert_eq!(
        draft_error.validation_choice_location(),
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
    assert!(draft_error.source().is_none());
    let draft_rendered = format!("{draft_error:?}\n{draft_error}");
    for forbidden in [OPTION_ACTIVE_B1, sentinel, "choice-secret", "choice.json"] {
        assert!(!draft_rendered.contains(forbidden));
    }
}

#[test]
fn current_default_rejects_provenance_free_values_and_preserves_fresh_source_metadata() {
    let source = fixture();
    let before = encode_template(&source).expect("metadata-free source must encode");
    let rich = id(FIELD_RICH_TEXT);

    let mut changed_body = fixture_value();
    changed_body["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["children"][0]
        ["children"][0]["text"] = json!("candidate body");
    changed_body["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureCandidateOuter"] =
        json!({"credential": "candidate-only-secret"});

    let mut same_body = fixture_value();
    same_body["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureCandidateOuter"] =
        json!({"credential": "candidate-only-secret"});

    for value in [&changed_body, &same_body] {
        let candidate = existing_default_from_value(value, rich);
        let error = assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::set_current_default(
                rich,
                FieldValueDraft::provenance_free_replacement(candidate),
            ),
            TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
            Some(rich),
        );
        assert_eq!(error.validation_category(), None);
        assert!(!format!("{error:?}\n{error}").contains("candidate-only-secret"));
    }

    let mut source_with_outer = fixture_value();
    let outer_number: Value = serde_json::from_str(ARBITRARY_NUMBER).unwrap();
    let outer_extra = json!({"keep": ["exact", outer_number]});
    source_with_outer["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureOuterPolicy"] =
        outer_extra.clone();
    let source_with_outer = fixture_from_value(&source_with_outer);
    let source_with_outer_before = encode_template(&source_with_outer).unwrap();

    let fresh_changed = run(
        &source_with_outer,
        LATER_AT,
        TemplateMutationCommand::set_current_default(
            rich,
            normalized_rich_text_draft(json!({
                "kind": "root",
                "children": [{
                    "kind": "paragraph",
                    "children": [{"kind": "text", "text": "fresh replacement"}]
                }]
            })),
        ),
    )
    .expect("fresh typed draft must inherit source envelope metadata")
    .into_changed()
    .expect("different fresh content must change the Template");
    assert_eq!(
        template_wire_value(&fresh_changed)["fields"][FIELD_RICH_TEXT]["defaultValue"]
            ["futureOuterPolicy"],
        outer_extra
    );

    let mut same_extra_candidate = template_wire_value(&source_with_outer);
    same_extra_candidate["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]
        ["children"][0]["children"][0]["text"] = json!("existing replacement");
    let same_extra_candidate = existing_default_from_value(&same_extra_candidate, rich);
    assert_command_error(
        &source_with_outer,
        &source_with_outer_before,
        TemplateMutationCommand::set_current_default(
            rich,
            FieldValueDraft::provenance_free_replacement(same_extra_candidate),
        ),
        TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
        Some(rich),
    );

    let mut different_extra_candidate = template_wire_value(&source_with_outer);
    different_extra_candidate["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureOuterPolicy"] =
        json!({"keep": ["different", 2]});
    let different_extra_candidate = existing_default_from_value(&different_extra_candidate, rich);
    assert_command_error(
        &source_with_outer,
        &source_with_outer_before,
        TemplateMutationCommand::set_current_default(
            rich,
            FieldValueDraft::provenance_free_replacement(different_extra_candidate),
        ),
        TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
        Some(rich),
    );

    let mut text_source_value = fixture_value();
    text_source_value["fields"][FIELD_TEXT]["defaultValue"]
        .as_object_mut()
        .unwrap()
        .remove("futureValue");
    let text_source = fixture_from_value(&text_source_value);
    let text_before = encode_template(&text_source).unwrap();
    let mut text_candidate_value = text_source_value;
    text_candidate_value["fields"][FIELD_TEXT]["defaultValue"]["value"] = json!("changed");
    text_candidate_value["fields"][FIELD_TEXT]["defaultValue"]["futureCandidateOuter"] =
        json!("candidate-only");
    let text_candidate = existing_default_from_value(&text_candidate_value, id(FIELD_TEXT));
    assert_command_error(
        &text_source,
        &text_before,
        TemplateMutationCommand::set_current_default(
            id(FIELD_TEXT),
            FieldValueDraft::provenance_free_replacement(text_candidate),
        ),
        TemplateMutationErrorCategory::ProvenanceFreeDefaultReplacement,
        Some(id(FIELD_TEXT)),
    );

    let inner_metadata = normalized_rich_text_draft(json!({
        "kind": "root",
        "children": [{
            "kind": "paragraph",
            "futureNode": {"keep": true},
            "children": [{"kind": "text", "text": "candidate inner metadata"}]
        }]
    }));
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(rich, inner_metadata),
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(rich),
    );
    assert_eq!(
        encode_template(&source_with_outer).unwrap(),
        source_with_outer_before
    );
}

#[test]
fn fresh_rich_text_construction_normalizes_empty_and_rejects_invalid_or_unknown_input() {
    let source = fixture();
    let before = encode_template(&source).unwrap();
    let field_id = id(FIELD_NEW_RICH);

    let empty = normalize_rich_text(
        RICH_TEXT_SCHEMA_VERSION,
        json!({"kind": "root", "children": []}).as_object().unwrap(),
    )
    .expect("semantic-empty editor input must normalize to unset");
    assert_eq!(empty, NormalizedRichText::Unset);
    let changed = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::create_field(
            draft(
                field_id,
                FieldKind::RichText,
                NewFieldConfiguration::rich_text(),
                FieldValueDraft::from_normalized_rich_text(empty),
            ),
            NewFieldInsertion::Append,
        ),
    )
    .expect("normalized empty rich text must create an unset default")
    .into_changed()
    .unwrap();
    assert!(changed.fields()[&field_id].default_value().is_unset());
    assert_eq!(
        changed.fields()[&field_id].default_value(),
        changed.fields()[&field_id].initial_default_value()
    );
    assert_round_trip(&changed);

    for (raw, category) in [
        (
            json!({"kind": "futureRoot", "children": []}),
            RichTextValidationErrorCategory::UnknownNodeType,
        ),
        (
            json!({
                "kind": "root",
                "children": [{
                    "kind": "paragraph",
                    "children": [{"kind": "text", "text": "x", "marks": ["futureMark"]}]
                }]
            }),
            RichTextValidationErrorCategory::UnknownMarkType,
        ),
        (
            json!({
                "kind": "root",
                "children": [{
                    "kind": "paragraph",
                    "children": [{"kind": "paragraph", "children": []}]
                }]
            }),
            RichTextValidationErrorCategory::InvalidChildNode,
        ),
    ] {
        let error = normalize_rich_text(RICH_TEXT_SCHEMA_VERSION, raw.as_object().unwrap())
            .expect_err("invalid rich-text input must fail before command construction");
        assert_eq!(error.category(), category);
    }

    let unknown_number: Value = serde_json::from_str(ARBITRARY_NUMBER).unwrap();
    let unknown = normalized_rich_text_draft(json!({
        "kind": "root",
        "futureMetadata": {"number": unknown_number, "credential": "new-rich-secret"},
        "children": [{
            "kind": "paragraph",
            "children": [{"kind": "text", "text": "non-empty"}]
        }]
    }));
    let error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            draft(
                field_id,
                FieldKind::RichText,
                NewFieldConfiguration::rich_text(),
                unknown,
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidFieldDraft,
        Some(field_id),
    );
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains(ARBITRARY_NUMBER));
    assert!(!rendered.contains("new-rich-secret"));
}

#[test]
fn reorder_requires_exact_active_permutation_and_preserves_caller_order() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let mut reversed = source.field_order().to_vec();
    reversed.reverse();
    let changed = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::reorder_fields(reversed.clone()),
    )
    .expect("exact active permutation must succeed")
    .into_changed()
    .expect("different order must change Template");
    assert_eq!(changed.field_order(), reversed);
    assert_eq!(changed.fields(), source.fields());
    assert_eq!(changed.revision().get(), source.revision().get() + 1);
    assert_round_trip(&changed);
    assert!(run(
        &source,
        LATER_AT,
        TemplateMutationCommand::reorder_fields(source.field_order().to_vec())
    )
    .expect("same order must succeed")
    .is_unchanged());

    let active = source.field_order();
    let invalid_orders = [
        vec![active[0], active[0], active[2], active[3]],
        active[..active.len() - 1].to_vec(),
        vec![active[0], active[1], active[2], id(FIELD_UNKNOWN)],
        vec![active[0], active[1], active[2], id(FIELD_ARCHIVED)],
        vec![
            active[0],
            active[1],
            active[2],
            active[3],
            id(FIELD_UNKNOWN),
        ],
    ];
    for order in invalid_orders {
        assert_command_error(
            &source,
            &before,
            TemplateMutationCommand::reorder_fields(order),
            TemplateMutationErrorCategory::InvalidFieldOrder,
            None,
        );
    }
    assert_eq!(encode_template(&source).unwrap(), before);
}

#[test]
fn reorder_defines_zero_and_single_active_field_boundaries() {
    let mut empty_value = fixture_value();
    empty_value["fields"] = json!({});
    empty_value["fieldOrder"] = json!([]);
    let empty = fixture_from_value(&empty_value);
    let empty_before = encode_template(&empty).unwrap();
    assert!(run(
        &empty,
        LATER_AT,
        TemplateMutationCommand::reorder_fields(Vec::new())
    )
    .expect("empty active set has exactly one valid permutation")
    .is_unchanged());
    assert_command_error(
        &empty,
        &empty_before,
        TemplateMutationCommand::reorder_fields(vec![id(FIELD_UNKNOWN)]),
        TemplateMutationErrorCategory::InvalidFieldOrder,
        None,
    );

    let mut archived_only_value = fixture_value();
    let archived_definition = archived_only_value["fields"][FIELD_ARCHIVED].clone();
    archived_only_value["fields"] = json!({(FIELD_ARCHIVED): archived_definition});
    archived_only_value["fieldOrder"] = json!([]);
    let archived_only = fixture_from_value(&archived_only_value);
    assert!(run(
        &archived_only,
        LATER_AT,
        TemplateMutationCommand::reorder_fields(Vec::new())
    )
    .expect("archived-only Template also has an empty active permutation")
    .is_unchanged());

    let mut single_value = fixture_value();
    let single_definition = single_value["fields"][FIELD_TEXT].clone();
    single_value["fields"] = json!({(FIELD_TEXT): single_definition});
    single_value["fieldOrder"] = json!([FIELD_TEXT]);
    let single = fixture_from_value(&single_value);
    let single_before = encode_template(&single).unwrap();
    assert!(run(
        &single,
        LATER_AT,
        TemplateMutationCommand::reorder_fields(vec![id(FIELD_TEXT)])
    )
    .expect("single active Field exact order must be a no-op")
    .is_unchanged());
    assert_command_error(
        &single,
        &single_before,
        TemplateMutationCommand::reorder_fields(Vec::new()),
        TemplateMutationErrorCategory::InvalidFieldOrder,
        None,
    );
    assert_eq!(encode_template(&empty).unwrap(), empty_before);
    assert_eq!(encode_template(&single).unwrap(), single_before);
}

#[test]
fn archive_is_one_way_preserves_definition_and_handles_noop_and_failures() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let choice = id(FIELD_CHOICE_A);
    let source_field_wire = template_wire_value(&source)["fields"][FIELD_CHOICE_A].clone();
    let changed = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::archive_field(choice),
    )
    .expect("active Field archive must succeed")
    .into_changed()
    .expect("archive must change Template");
    assert_eq!(
        changed.fields()[&choice].lifecycle(),
        FieldLifecycle::Archived
    );
    assert!(!changed.field_order().contains(&choice));
    let mut source_without_lifecycle = source_field_wire;
    let mut changed_without_lifecycle =
        template_wire_value(&changed)["fields"][FIELD_CHOICE_A].clone();
    let source_lifecycle = source_without_lifecycle
        .as_object_mut()
        .unwrap()
        .remove("lifecycle");
    let changed_lifecycle = changed_without_lifecycle
        .as_object_mut()
        .unwrap()
        .remove("lifecycle");
    assert_eq!(source_lifecycle, Some(json!("active")));
    assert_eq!(changed_lifecycle, Some(json!("archived")));
    assert_eq!(source_without_lifecycle, changed_without_lifecycle);
    for (field_id, definition) in source.fields() {
        if *field_id != choice {
            assert_eq!(changed.fields()[field_id], *definition);
        }
    }
    assert_eq!(changed.revision().get(), source.revision().get() + 1);
    assert_round_trip(&changed);
    assert_eq!(encode_template(&source).unwrap(), before);

    let archived = id(FIELD_ARCHIVED);
    assert!(run(
        &source,
        LATER_AT,
        TemplateMutationCommand::archive_field(archived)
    )
    .expect("already archived contract is a semantic no-op")
    .is_unchanged());
    let stale_archived = apply_template_mutation(
        &source,
        revision(source.revision().get() - 1),
        LATER_AT,
        TemplateMutationCommand::archive_field(archived),
    )
    .expect_err("stale revision must precede archived no-op detection");
    assert_eq!(
        stale_archived.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    let malformed_archived = apply_template_mutation(
        &source,
        source.revision(),
        "2026-09-03T03:04:05Z",
        TemplateMutationCommand::archive_field(archived),
    )
    .expect_err("malformed timestamp must precede archived no-op detection");
    assert_eq!(
        malformed_archived.category(),
        TemplateMutationErrorCategory::InvalidTimestamp
    );
    let regressed_archived = apply_template_mutation(
        &source,
        source.revision(),
        CREATED_AT,
        TemplateMutationCommand::archive_field(archived),
    )
    .expect_err("regressed timestamp must precede archived no-op detection");
    assert_eq!(
        regressed_archived.category(),
        TemplateMutationErrorCategory::TimestampRegression
    );
    assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::archive_field(id(FIELD_UNKNOWN)),
        TemplateMutationErrorCategory::FieldNotFound,
        Some(id(FIELD_UNKNOWN)),
    );

    let stale = apply_template_mutation(
        &source,
        revision(source.revision().get() - 1),
        LATER_AT,
        TemplateMutationCommand::archive_field(choice),
    )
    .expect_err("stale archive must fail before mutation");
    assert_eq!(
        stale.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    let regressing = apply_template_mutation(
        &source,
        source.revision(),
        CREATED_AT,
        TemplateMutationCommand::archive_field(choice),
    )
    .expect_err("regressing timestamp must fail before mutation");
    assert_eq!(
        regressing.category(),
        TemplateMutationErrorCategory::TimestampRegression
    );
    assert_eq!(encode_template(&source).unwrap(), before);
}

#[test]
fn field_commands_share_timestamp_admission_and_application() {
    let source = fixture();
    let before = encode_template(&source).unwrap();

    let equal_timestamp = apply_template_mutation(
        &source,
        source.revision(),
        UPDATED_AT,
        TemplateMutationCommand::create_field(
            simple_draft(id(FIELD_NEW_TEXT)),
            NewFieldInsertion::Append,
        ),
    )
    .expect("equal timestamp is valid for a changed add command")
    .into_changed()
    .unwrap();
    assert_eq!(equal_timestamp.updated_at_utc(), UPDATED_AT);
    assert_eq!(
        equal_timestamp.revision().get(),
        source.revision().get() + 1
    );

    let updated = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_field_required(id(FIELD_TEXT), true),
    )
    .expect("advanced timestamp must apply to update")
    .into_changed()
    .unwrap();
    assert_eq!(updated.updated_at_utc(), LATER_AT);

    let regressed = apply_template_mutation(
        &source,
        source.revision(),
        CREATED_AT,
        TemplateMutationCommand::reorder_fields(source.field_order().to_vec()),
    )
    .expect_err("reorder no-op must not bypass timestamp regression");
    assert_eq!(
        regressed.category(),
        TemplateMutationErrorCategory::TimestampRegression
    );
    let malformed = apply_template_mutation(
        &source,
        source.revision(),
        "not-a-timestamp",
        TemplateMutationCommand::archive_field(id(FIELD_TEXT)),
    )
    .expect_err("archive must use the common timestamp admission");
    assert_eq!(
        malformed.category(),
        TemplateMutationErrorCategory::InvalidTimestamp
    );
    assert_eq!(encode_template(&source).unwrap(), before);
}

#[test]
fn changed_field_commands_reject_revision_overflow_without_source_mutation() {
    let mut value = fixture_value();
    value["revision"] = json!(u32::MAX);
    let source = fixture_from_value(&value);
    let before = encode_template(&source).expect("MAX source must encode");
    for command in [
        TemplateMutationCommand::archive_field(id(FIELD_TEXT)),
        TemplateMutationCommand::set_field_label(id(FIELD_TEXT), "changed".to_owned()),
        TemplateMutationCommand::reorder_fields({
            let mut order = source.field_order().to_vec();
            order.reverse();
            order
        }),
    ] {
        assert_command_error(
            &source,
            &before,
            command,
            TemplateMutationErrorCategory::RevisionOverflow,
            None,
        );
    }
    let create_error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::create_field(
            simple_draft(id(FIELD_NEW_TEXT)),
            NewFieldInsertion::Append,
        ),
        TemplateMutationErrorCategory::RevisionOverflow,
        None,
    );
    assert_eq!(create_error.field_id(), None);
}

#[test]
fn multi_choice_constructor_does_not_normalize_or_reorder_command_input() {
    let source = fixture();
    let before = encode_template(&source).unwrap();
    let choice = id(FIELD_CHOICE_B);
    let noncanonical = FieldValueDraft::multi_choice(vec![
        option_id(OPTION_ACTIVE_B2),
        option_id(OPTION_ACTIVE_B1),
    ]);
    let error = assert_command_error(
        &source,
        &before,
        TemplateMutationCommand::set_current_default(choice, noncanonical),
        TemplateMutationErrorCategory::InvalidCurrentDefault,
        Some(choice),
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        error.validation_choice_location(),
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
    assert_eq!(
        error.validation_choice_category(),
        Some(ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder)
    );

    let active_set = source
        .field_order()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(active_set.len(), source.field_order().len());
}
