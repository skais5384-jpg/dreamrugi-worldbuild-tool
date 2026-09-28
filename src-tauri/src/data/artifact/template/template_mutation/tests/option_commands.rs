use crate::data::artifact::{
    ArtifactChoiceValueLocation, ArtifactValidationErrorCategory, FieldKind, FieldLifecycle,
    OptionLifecycle,
};
use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

use super::*;

const OPTION_NEW: &str = "c3000000-0000-4000-8000-000000000001";
const OPTION_SECOND_A: &str = "c3000000-0000-4000-8000-000000000002";
const OPTION_UNKNOWN: &str = "c3000000-0000-4000-8000-000000000099";
const OPTION_ARCHIVED_B: &str = "68686868-6868-4868-8868-686868686868";
const FIELD_NEW_CHOICE_LABEL: &str = "b2000000-0000-4000-8000-000000000001";

fn run(
    source: &TemplateArtifact,
    command: TemplateMutationCommand,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    apply_template_mutation(source, source.revision(), LATER_AT, command)
}

fn changed(source: &TemplateArtifact, command: TemplateMutationCommand) -> TemplateArtifact {
    run(source, command)
        .expect("valid Option command must succeed")
        .into_changed()
        .expect("Option command must change the candidate")
}

fn assert_command_error(
    source: &TemplateArtifact,
    command: TemplateMutationCommand,
    category: TemplateMutationErrorCategory,
    field_id: FieldId,
    option_id: Option<OptionId>,
) -> TemplateMutationError {
    let before = encode_template(source).expect("source must encode before rejected command");
    let error = run(source, command).expect_err("Option command must fail");
    assert_eq!(error.category(), category);
    assert_eq!(error.field_id(), Some(field_id));
    assert_eq!(error.option_id(), option_id);
    assert_eq!(
        encode_template(source).expect("rejected command source must remain encodable"),
        before
    );
    assert!(error.source().is_none());
    error
}

fn assert_error_is_redacted(error: &TemplateMutationError) {
    let rendered = render_mutation_error_chain(error);
    for sensitive in [
        SECRET,
        METADATA_SECRET,
        CHANGED_METADATA_SECRET,
        ARBITRARY_NUMBER,
        OTHER_ARBITRARY_NUMBER,
        "C:\\private",
        "/home/private",
    ] {
        assert!(
            !rendered.contains(sensitive),
            "Option mutation error must not expose sensitive payload"
        );
    }
}

fn assert_round_trip(candidate: &TemplateArtifact) {
    candidate
        .validate_storage()
        .expect("Option command candidate must be storage-valid");
    let bytes = encode_template(candidate).expect("Option command candidate must encode");
    assert_eq!(
        encode_template(&decode_template(&bytes).expect("candidate must decode"))
            .expect("decoded candidate must re-encode"),
        bytes
    );
}

fn choice_with_second_option_value() -> Value {
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"] =
        json!([OPTION_ACTIVE_A, OPTION_SECOND_A]);
    value["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_SECOND_A] = json!({
        "label": "second active option",
        "lifecycle": "active",
        "futureSecondOption": {"keep": true}
    });
    value
}

fn choice_without_current_reference_value() -> Value {
    let mut value = choice_with_second_option_value();
    value["fields"][FIELD_CHOICE_A]["defaultValue"] =
        json!({"kind": "singleChoice", "optionId": OPTION_SECOND_A});
    value
}

fn no_active_option_value() -> Value {
    let mut value = metadata_isolated_fixture_value();
    value["fields"][FIELD_CHOICE_A]["defaultValue"] = json!({"kind": "unset"});
    value["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"] = json!([]);
    value["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]["lifecycle"] =
        json!("archived");
    value
}

fn multi_with_local_archived_option_value() -> Value {
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_B]["configuration"]["options"][OPTION_ARCHIVED_B] = json!({
        "label": "local archived option",
        "lifecycle": "archived",
        "futureArchivedOption": {"keep": true}
    });
    value
}

fn option_label_fixture(label: &str) -> TemplateArtifact {
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]["label"] =
        json!(label);
    fixture_from_value(&value)
}

fn option_wire<'a>(value: &'a Value, field_id: &str, option_id: &str) -> &'a Value {
    &value["fields"][field_id]["configuration"]["options"][option_id]
}

fn field_wire_bytes(template: &TemplateArtifact, field_id: FieldId) -> Vec<u8> {
    to_deterministic_json_bytes(&template_wire_value(template)["fields"][field_id.to_string()])
        .expect("Field wire subtree must encode deterministically")
}

fn assert_unrelated_persisted_state_equal(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    field_id: FieldId,
    changed_members: &[&str],
) {
    let source_wire = template_wire_value(source);
    let candidate_wire = template_wire_value(candidate);
    let field_key = field_id.to_string();
    for (key, source_value) in source_wire["fields"][&field_key]
        .as_object()
        .expect("source Field must be an object")
    {
        if !changed_members.contains(&key.as_str()) {
            assert_eq!(
                &candidate_wire["fields"][&field_key][key], source_value,
                "non-target persisted member {key} must remain exact"
            );
        }
    }
    for (other_id, source_field) in source_wire["fields"]
        .as_object()
        .expect("source fields must be an object")
    {
        if other_id != &field_key {
            assert_eq!(&candidate_wire["fields"][other_id], source_field);
        }
    }
}

#[test]
fn add_option_supports_single_and_multi_append_and_exact_insertion() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let new_option = option_id(OPTION_NEW);

    let single = changed(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "new single option".to_owned()),
            NewOptionInsertion::At(0),
        ),
    );
    let single_wire = template_wire_value(&single);
    assert_eq!(
        single_wire["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"],
        json!([OPTION_NEW, OPTION_ACTIVE_A])
    );
    assert_eq!(
        option_wire(&single_wire, FIELD_CHOICE_A, OPTION_NEW),
        &json!({"label": "new single option", "lifecycle": "active"})
    );
    assert_eq!(
        single.fields()[&id(FIELD_CHOICE_A)].default_value(),
        source.fields()[&id(FIELD_CHOICE_A)].default_value()
    );
    assert_eq!(
        single.fields()[&id(FIELD_CHOICE_A)].initial_default_value(),
        source.fields()[&id(FIELD_CHOICE_A)].initial_default_value()
    );
    assert_unrelated_persisted_state_equal(
        &source,
        &single,
        id(FIELD_CHOICE_A),
        &["configuration"],
    );
    assert_round_trip(&single);

    let multi = changed(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_B),
            NewChoiceOptionDraft::new(new_option, "new multi option".to_owned()),
            NewOptionInsertion::Append,
        ),
    );
    let multi_wire = template_wire_value(&multi);
    assert_eq!(
        multi_wire["fields"][FIELD_CHOICE_B]["configuration"]["optionOrder"],
        json!([OPTION_ACTIVE_B2, OPTION_ACTIVE_B1, OPTION_NEW])
    );
    assert_eq!(
        option_wire(&multi_wire, FIELD_CHOICE_B, OPTION_NEW),
        &json!({"label": "new multi option", "lifecycle": "active"})
    );
    assert_eq!(
        multi.fields()[&id(FIELD_CHOICE_B)].default_value(),
        source.fields()[&id(FIELD_CHOICE_B)].default_value()
    );
    assert_eq!(
        encode_template(&source).expect("add must not mutate source"),
        before
    );
    assert_round_trip(&multi);

    let single_append = changed(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "single append".to_owned()),
            NewOptionInsertion::Append,
        ),
    );
    assert_eq!(
        template_wire_value(&single_append)["fields"][FIELD_CHOICE_A]["configuration"]
            ["optionOrder"],
        json!([OPTION_ACTIVE_A, OPTION_NEW])
    );

    let multi_at = changed(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_B),
            NewChoiceOptionDraft::new(new_option, "multi inserted".to_owned()),
            NewOptionInsertion::At(1),
        ),
    );
    assert_eq!(
        template_wire_value(&multi_at)["fields"][FIELD_CHOICE_B]["configuration"]["optionOrder"],
        json!([OPTION_ACTIVE_B2, OPTION_NEW, OPTION_ACTIVE_B1])
    );
}

#[test]
fn add_option_handles_empty_choice_and_rejects_collision_draft_and_bad_position() {
    let empty_source = fixture_from_value(&no_active_option_value());
    let new_option = option_id(OPTION_NEW);
    let added = changed(
        &empty_source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "first active".to_owned()),
            NewOptionInsertion::At(0),
        ),
    );
    assert_eq!(
        template_wire_value(&added)["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"],
        json!([OPTION_NEW])
    );

    let source = fixture();
    for collision in [
        option_id(OPTION_ACTIVE_A),
        option_id(OPTION_ARCHIVED_A),
        option_id(OPTION_ACTIVE_B1),
    ] {
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::add_option(
                id(FIELD_CHOICE_A),
                NewChoiceOptionDraft::new(collision, "collision".to_owned()),
                NewOptionInsertion::Append,
            ),
            TemplateMutationErrorCategory::OptionAlreadyExists,
            id(FIELD_CHOICE_A),
            Some(collision),
        );
        assert_error_is_redacted(&error);
    }

    let mut cross_field_archived_value = fixture_value();
    cross_field_archived_value["fields"][FIELD_CHOICE_B]["configuration"]["optionOrder"] =
        json!([OPTION_ACTIVE_B2]);
    cross_field_archived_value["fields"][FIELD_CHOICE_B]["configuration"]["options"]
        [OPTION_ACTIVE_B1]["lifecycle"] = json!("archived");
    cross_field_archived_value["fields"][FIELD_CHOICE_B]["defaultValue"]["optionIds"] =
        json!([OPTION_ACTIVE_B2]);
    let cross_field_archived = fixture_from_value(&cross_field_archived_value);
    assert_command_error(
        &cross_field_archived,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(option_id(OPTION_ACTIVE_B1), "collision".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationErrorCategory::OptionAlreadyExists,
        id(FIELD_CHOICE_A),
        Some(option_id(OPTION_ACTIVE_B1)),
    );

    assert_command_error(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::archived_for_test(new_option, "archived draft".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationErrorCategory::InvalidOptionDraft,
        id(FIELD_CHOICE_A),
        Some(new_option),
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "outside".to_owned()),
            NewOptionInsertion::At(2),
        ),
        TemplateMutationErrorCategory::InvalidOptionInsertionPosition,
        id(FIELD_CHOICE_A),
        Some(new_option),
    );
}

#[test]
fn opaque_option_labels_preserve_existing_exact_noops_and_stale_priority() {
    for label in [
        "",
        "first\nsecond",
        "first\rsecond",
        "first\u{2028}second",
        " 일반 Unicode\t🙂 ",
    ] {
        let source = option_label_fixture(label);
        let before = encode_template(&source).expect("opaque-label source must encode");
        let outcome = run(
            &source,
            TemplateMutationCommand::rename_option(
                id(FIELD_CHOICE_A),
                option_id(OPTION_ACTIVE_A),
                label.to_owned(),
            ),
        )
        .expect("exact opaque label rename must succeed");
        assert!(outcome.is_unchanged());
        assert!(outcome.changed().is_none());
        assert_eq!(source.revision(), revision(2));
        assert_eq!(source.updated_at_utc(), UPDATED_AT);
        assert_eq!(
            encode_template(&source).expect("opaque-label source must remain encodable"),
            before
        );

        let stale = apply_template_mutation(
            &source,
            revision(1),
            LATER_AT,
            TemplateMutationCommand::rename_option(
                id(FIELD_CHOICE_A),
                option_id(OPTION_ACTIVE_A),
                label.to_owned(),
            ),
        )
        .expect_err("stale revision must precede exact opaque label no-op");
        assert_eq!(
            stale.category(),
            TemplateMutationErrorCategory::RevisionMismatch
        );
        assert_eq!(stale.field_id(), None);
        assert_eq!(stale.option_id(), None);
        assert_eq!(
            encode_template(&source).expect("stale rename must not mutate source"),
            before
        );
    }
}

#[test]
fn add_rename_and_create_field_accept_opaque_empty_and_multiline_option_labels() {
    for label in ["", "first\r\nsecond"] {
        let source = fixture();
        let new_option = option_id(OPTION_NEW);
        let added = changed(
            &source,
            TemplateMutationCommand::add_option(
                id(FIELD_CHOICE_A),
                NewChoiceOptionDraft::new(new_option, label.to_owned()),
                NewOptionInsertion::Append,
            ),
        );
        assert_eq!(
            added.fields()[&id(FIELD_CHOICE_A)]
                .configuration()
                .options()
                .expect("Choice options")[&new_option]
                .label(),
            label
        );
        assert_round_trip(&added);

        let renamed = changed(
            &source,
            TemplateMutationCommand::rename_option(
                id(FIELD_CHOICE_A),
                option_id(OPTION_ACTIVE_A),
                label.to_owned(),
            ),
        );
        assert_eq!(
            renamed.fields()[&id(FIELD_CHOICE_A)]
                .configuration()
                .options()
                .expect("Choice options")[&option_id(OPTION_ACTIVE_A)]
                .label(),
            label
        );
        assert_round_trip(&renamed);

        let field_id = id(FIELD_NEW_CHOICE_LABEL);
        let created = changed(
            &source,
            TemplateMutationCommand::create_field(
                NewFieldDraft::new(
                    field_id,
                    "opaque-label choice".to_owned(),
                    FieldKind::SingleChoice,
                    NewFieldConfiguration::single_choice(
                        vec![new_option],
                        vec![NewChoiceOptionDraft::new(new_option, label.to_owned())],
                    ),
                    false,
                    None,
                    FieldValueDraft::unset(),
                ),
                NewFieldInsertion::Append,
            ),
        );
        assert_eq!(
            created.fields()[&field_id]
                .configuration()
                .options()
                .expect("new Choice options")[&new_option]
                .label(),
            label
        );
        assert_round_trip(&created);
    }
}

#[test]
fn option_commands_report_field_and_option_target_categories() {
    let source = fixture();
    let unknown_field = id(FIELD_NEW);
    let unknown_option = option_id(OPTION_UNKNOWN);
    let active_option = option_id(OPTION_ACTIVE_A);
    let archived_option = option_id(OPTION_ARCHIVED_A);

    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(unknown_field, unknown_option, "rename".to_owned()),
        TemplateMutationErrorCategory::FieldNotFound,
        unknown_field,
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(
            id(FIELD_ARCHIVED),
            unknown_option,
            "rename".to_owned(),
        ),
        TemplateMutationErrorCategory::FieldIsArchived,
        id(FIELD_ARCHIVED),
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(id(FIELD_TEXT), unknown_option, "rename".to_owned()),
        TemplateMutationErrorCategory::FieldIsNotChoice,
        id(FIELD_TEXT),
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::add_option(
            id(FIELD_TEXT),
            NewChoiceOptionDraft::new(unknown_option, "new".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationErrorCategory::FieldIsNotChoice,
        id(FIELD_TEXT),
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::reorder_options(id(FIELD_TEXT), Vec::new()),
        TemplateMutationErrorCategory::FieldIsNotChoice,
        id(FIELD_TEXT),
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(id(FIELD_TEXT), unknown_option, None),
        TemplateMutationErrorCategory::FieldIsNotChoice,
        id(FIELD_TEXT),
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(unknown_field, unknown_option, None),
        TemplateMutationErrorCategory::FieldNotFound,
        unknown_field,
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::add_option(
            unknown_field,
            NewChoiceOptionDraft::new(unknown_option, "new".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationErrorCategory::FieldNotFound,
        unknown_field,
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::reorder_options(unknown_field, Vec::new()),
        TemplateMutationErrorCategory::FieldNotFound,
        unknown_field,
        None,
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            unknown_option,
            "rename".to_owned(),
        ),
        TemplateMutationErrorCategory::OptionNotFound,
        id(FIELD_CHOICE_A),
        Some(unknown_option),
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_B1),
            "cross-field".to_owned(),
        ),
        TemplateMutationErrorCategory::OptionNotFound,
        id(FIELD_CHOICE_A),
        Some(option_id(OPTION_ACTIVE_B1)),
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            archived_option,
            "rename".to_owned(),
        ),
        TemplateMutationErrorCategory::OptionIsArchived,
        id(FIELD_CHOICE_A),
        Some(archived_option),
    );

    let archived_choice = archived_choice_field_fixture();
    assert_eq!(
        archived_choice.fields()[&id(FIELD_CHOICE_B)].lifecycle(),
        FieldLifecycle::Archived
    );
    assert_command_error(
        &archived_choice,
        TemplateMutationCommand::archive_option(
            id(FIELD_CHOICE_B),
            option_id(OPTION_ACTIVE_B1),
            Some(FieldValueDraft::unset()),
        ),
        TemplateMutationErrorCategory::FieldIsArchived,
        id(FIELD_CHOICE_B),
        None,
    );
    assert_command_error(
        &archived_choice,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_B),
            option_id(OPTION_ACTIVE_B1),
            "rename".to_owned(),
        ),
        TemplateMutationErrorCategory::FieldIsArchived,
        id(FIELD_CHOICE_B),
        None,
    );
    assert_command_error(
        &archived_choice,
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_B),
            NewChoiceOptionDraft::new(unknown_option, "new".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationErrorCategory::FieldIsArchived,
        id(FIELD_CHOICE_B),
        None,
    );
    assert_command_error(
        &archived_choice,
        TemplateMutationCommand::reorder_options(
            id(FIELD_CHOICE_B),
            vec![option_id(OPTION_ACTIVE_B2), option_id(OPTION_ACTIVE_B1)],
        ),
        TemplateMutationErrorCategory::FieldIsArchived,
        id(FIELD_CHOICE_B),
        None,
    );

    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(id(FIELD_CHOICE_A), archived_option, None),
        TemplateMutationErrorCategory::OptionIsArchived,
        id(FIELD_CHOICE_A),
        Some(archived_option),
    );
    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(id(FIELD_CHOICE_A), unknown_option, None),
        TemplateMutationErrorCategory::OptionNotFound,
        id(FIELD_CHOICE_A),
        Some(unknown_option),
    );

    // 같은 target 값이어도 active Option은 archive repair 필요 오류까지 도달한다.
    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(id(FIELD_CHOICE_A), active_option, None),
        TemplateMutationErrorCategory::CurrentDefaultRepairRequired,
        id(FIELD_CHOICE_A),
        Some(active_option),
    );
}

#[test]
fn rename_option_preserves_identity_order_lifecycle_extras_and_supports_exact_noop() {
    let source = fixture();
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let before = encode_template(&source).expect("source must encode");
    let source_wire = template_wire_value(&source);

    let candidate = changed(
        &source,
        TemplateMutationCommand::rename_option(field_id, target, "renamed active".to_owned()),
    );
    let candidate_wire = template_wire_value(&candidate);
    assert_eq!(
        candidate_wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]
            ["label"],
        json!("renamed active")
    );
    for member in ["lifecycle", "futureOption"] {
        assert_eq!(
            candidate_wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]
                [member],
            source_wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]
                [member]
        );
    }
    assert_eq!(
        candidate_wire["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"],
        source_wire["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"]
    );
    assert_eq!(
        candidate.fields()[&field_id].default_value(),
        source.fields()[&field_id].default_value()
    );
    assert_eq!(
        candidate.fields()[&field_id].initial_default_value(),
        source.fields()[&field_id].initial_default_value()
    );
    assert_round_trip(&candidate);
    assert_eq!(
        encode_template(&source).expect("rename must not mutate source"),
        before
    );

    let noop = run(
        &source,
        TemplateMutationCommand::rename_option(field_id, target, "active option".to_owned()),
    )
    .expect("same label must be a successful no-op");
    assert!(noop.is_unchanged());
    assert!(noop.changed().is_none());
    assert_eq!(source.revision(), revision(2));
    assert_eq!(source.updated_at_utc(), UPDATED_AT);
}

#[test]
fn reorder_options_changes_display_order_without_touching_canonical_selection() {
    let source = fixture();
    let field_id = id(FIELD_CHOICE_B);
    let before = encode_template(&source).expect("source must encode");
    let selection_before = source.fields()[&field_id].default_value().clone();
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::reorder_options(
            field_id,
            vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_B2)],
        ),
    );
    let wire = template_wire_value(&candidate);
    assert_eq!(
        wire["fields"][FIELD_CHOICE_B]["configuration"]["optionOrder"],
        json!([OPTION_ACTIVE_B1, OPTION_ACTIVE_B2])
    );
    assert_eq!(
        candidate.fields()[&field_id].default_value(),
        &selection_before
    );
    assert_eq!(
        candidate.fields()[&field_id].default_value().multi_choice(),
        Some(&[option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_B2)][..]),
        "selected OptionIds stay in canonical ID order, independent of display order"
    );
    assert_eq!(
        candidate.fields()[&field_id].initial_default_value(),
        &initial_before
    );
    assert_unrelated_persisted_state_equal(&source, &candidate, field_id, &["configuration"]);
    assert_round_trip(&candidate);
    assert_eq!(
        encode_template(&source).expect("reorder must not mutate source"),
        before
    );

    let noop = run(
        &source,
        TemplateMutationCommand::reorder_options(
            field_id,
            vec![option_id(OPTION_ACTIVE_B2), option_id(OPTION_ACTIVE_B1)],
        ),
    )
    .expect("same optionOrder must no-op");
    assert!(noop.is_unchanged());
}

#[test]
fn reorder_options_accepts_empty_and_single_boundaries_and_rejects_non_permutations() {
    let empty_source = fixture_from_value(&no_active_option_value());
    let empty = run(
        &empty_source,
        TemplateMutationCommand::reorder_options(id(FIELD_CHOICE_A), Vec::new()),
    )
    .expect("empty active set has one valid empty permutation");
    assert!(empty.is_unchanged());

    let source = fixture();
    let single = run(
        &source,
        TemplateMutationCommand::reorder_options(
            id(FIELD_CHOICE_A),
            vec![option_id(OPTION_ACTIVE_A)],
        ),
    )
    .expect("single active set has one valid permutation");
    assert!(single.is_unchanged());

    let field_id = id(FIELD_CHOICE_B);
    for invalid in [
        Vec::new(),
        vec![option_id(OPTION_ACTIVE_B1)],
        vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_B1)],
        vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_UNKNOWN)],
        vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_A)],
        vec![
            option_id(OPTION_ACTIVE_B1),
            option_id(OPTION_ACTIVE_B2),
            option_id(OPTION_UNKNOWN),
        ],
    ] {
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::reorder_options(field_id, invalid),
            TemplateMutationErrorCategory::InvalidOptionOrder,
            field_id,
            None,
        );
        assert_eq!(error.validation_category(), None);
        assert_error_is_redacted(&error);
    }

    let local_archived_source = fixture_from_value(&multi_with_local_archived_option_value());
    let local_archived = option_id(OPTION_ARCHIVED_B);
    let active_b1 = option_id(OPTION_ACTIVE_B1);
    let active_b2 = option_id(OPTION_ACTIVE_B2);
    let options = local_archived_source.fields()[&field_id]
        .configuration()
        .options()
        .expect("local archived fixture must be Choice");
    assert_eq!(options[&active_b1].lifecycle(), OptionLifecycle::Active);
    assert_eq!(options[&active_b2].lifecycle(), OptionLifecycle::Active);
    assert_eq!(
        options[&local_archived].lifecycle(),
        OptionLifecycle::Archived
    );
    assert_eq!(
        local_archived_source.fields()[&field_id]
            .configuration()
            .option_order(),
        Some(&[active_b2, active_b1][..])
    );
    let archived_only_problem = vec![active_b1, local_archived];
    assert_eq!(archived_only_problem.len(), 2);
    assert_ne!(archived_only_problem[0], archived_only_problem[1]);
    assert!(
        archived_only_problem
            .iter()
            .all(|option_id| options.contains_key(option_id)),
        "every reorder ID must belong to the target Field"
    );
    let error = assert_command_error(
        &local_archived_source,
        TemplateMutationCommand::reorder_options(field_id, archived_only_problem),
        TemplateMutationErrorCategory::InvalidOptionOrder,
        field_id,
        None,
    );
    assert_eq!(error.validation_category(), None);
    assert_error_is_redacted(&error);
}

#[test]
fn archive_single_option_and_explicit_unset_are_one_validated_mutation() {
    let source = fixture();
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let before = encode_template(&source).expect("source must encode");
    let initial_before = source.fields()[&field_id].initial_default_value().clone();
    let option_before = template_wire_value(&source)["fields"][FIELD_CHOICE_A]["configuration"]
        ["options"][OPTION_ACTIVE_A]
        .clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(field_id, target, Some(FieldValueDraft::unset())),
    );
    let wire = template_wire_value(&candidate);
    assert_eq!(
        wire["fields"][FIELD_CHOICE_A]["configuration"]["optionOrder"],
        json!([])
    );
    assert_eq!(
        wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]["lifecycle"],
        json!("archived")
    );
    assert_eq!(
        wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]["label"],
        option_before["label"]
    );
    assert_eq!(
        wire["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]["futureOption"],
        option_before["futureOption"]
    );
    assert_eq!(
        wire["fields"][FIELD_CHOICE_A]["defaultValue"],
        json!({"kind": "unset"})
    );
    assert_eq!(
        candidate.fields()[&field_id].initial_default_value(),
        &initial_before,
        "historical initial default must remain byte-for-byte represented"
    );
    assert_eq!(
        candidate.fields()[&field_id].lifecycle(),
        FieldLifecycle::Active
    );
    assert_eq!(candidate.revision().get(), source.revision().get() + 1);
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    assert_round_trip(&candidate);
    assert_eq!(
        encode_template(&source).expect("archive must not mutate source"),
        before
    );
}

#[test]
fn repeated_archived_option_rejects_every_repair_before_payload_application() {
    let source = fixture();
    let field_id = id(FIELD_CHOICE_A);
    let archived = option_id(OPTION_ARCHIVED_A);
    let active = option_id(OPTION_ACTIVE_A);
    let before = encode_template(&source).expect("archived Option source must encode");

    for repair in [
        None,
        Some(FieldValueDraft::single_choice(active)),
        Some(FieldValueDraft::unset()),
    ] {
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::archive_option(field_id, archived, repair),
            TemplateMutationErrorCategory::OptionIsArchived,
            field_id,
            Some(archived),
        );
        assert_eq!(error.validation_category(), None);
        assert_error_is_redacted(&error);
        assert_eq!(
            encode_template(&source).expect("repeated archive must preserve source"),
            before
        );
    }

    let stale = apply_template_mutation(
        &source,
        revision(1),
        LATER_AT,
        TemplateMutationCommand::archive_option(field_id, archived, Some(FieldValueDraft::unset())),
    )
    .expect_err("stale revision must precede archived Option target inspection");
    assert_eq!(
        stale.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    assert_eq!(stale.field_id(), None);
    assert_eq!(stale.option_id(), None);
    assert_eq!(
        encode_template(&source).expect("stale repeated archive must preserve source"),
        before
    );
}

#[test]
fn archive_single_option_can_replace_with_other_active_and_preserves_history() {
    let value = choice_with_second_option_value();
    let source = fixture_from_value(&value);
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let replacement = option_id(OPTION_SECOND_A);
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::single_choice(replacement)),
        ),
    );
    let field = &candidate.fields()[&field_id];
    assert_eq!(field.default_value().single_choice(), Some(replacement));
    assert_eq!(field.initial_default_value(), &initial_before);
    assert_eq!(
        field.configuration().option_order(),
        Some(&[replacement][..])
    );
    assert_eq!(
        field.configuration().options().expect("Choice options")[&target].lifecycle(),
        OptionLifecycle::Archived
    );
    assert_eq!(
        field.configuration().options().expect("Choice options")[&replacement].lifecycle(),
        OptionLifecycle::Active
    );
    assert_round_trip(&candidate);
}

#[test]
fn archive_multi_option_supports_canonical_remaining_selection_and_unset() {
    let source = fixture();
    let field_id = id(FIELD_CHOICE_B);
    let target = option_id(OPTION_ACTIVE_B1);
    let remaining = option_id(OPTION_ACTIVE_B2);
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let replaced = changed(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::multi_choice(vec![remaining])),
        ),
    );
    let replaced_field = &replaced.fields()[&field_id];
    assert_eq!(
        replaced_field.default_value().multi_choice(),
        Some(&[remaining][..])
    );
    assert_eq!(replaced_field.initial_default_value(), &initial_before);
    assert_eq!(
        replaced_field.configuration().option_order(),
        Some(&[remaining][..])
    );
    assert_eq!(
        replaced_field
            .configuration()
            .options()
            .expect("Choice options")[&target]
            .lifecycle(),
        OptionLifecycle::Archived
    );
    assert_round_trip(&replaced);

    let unset = changed(
        &source,
        TemplateMutationCommand::archive_option(field_id, target, Some(FieldValueDraft::unset())),
    );
    assert!(unset.fields()[&field_id].default_value().is_unset());
    assert_eq!(
        unset.fields()[&field_id].configuration().option_order(),
        Some(&[remaining][..])
    );
    assert_round_trip(&unset);
}

#[test]
fn archive_without_reference_preserves_current_and_rejects_unnecessary_repair() {
    let source = fixture_from_value(&choice_without_current_reference_value());
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let current_before = source.fields()[&field_id].default_value().clone();
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(field_id, target, None),
    );
    let field = &candidate.fields()[&field_id];
    assert_eq!(field.default_value(), &current_before);
    assert_eq!(field.initial_default_value(), &initial_before);
    assert_eq!(
        field.configuration().option_order(),
        Some(&[option_id(OPTION_SECOND_A)][..])
    );
    assert_round_trip(&candidate);

    let error = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(field_id, target, Some(FieldValueDraft::unset())),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        field_id,
        Some(target),
    );
    assert_eq!(error.validation_category(), None);
}

#[test]
fn archive_last_active_option_is_valid_when_current_is_already_unset() {
    let mut value = metadata_isolated_fixture_value();
    value["fields"][FIELD_CHOICE_A]["defaultValue"] = json!({"kind": "unset"});
    let source = fixture_from_value(&value);
    let target = option_id(OPTION_ACTIVE_A);
    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(id(FIELD_CHOICE_A), target, None),
    );
    let field = &candidate.fields()[&id(FIELD_CHOICE_A)];
    assert_eq!(field.configuration().option_order(), Some(&[][..]));
    assert_eq!(
        field.configuration().options().expect("Choice options")[&target].lifecycle(),
        OptionLifecycle::Archived
    );
    assert!(field.default_value().is_unset());
    assert_round_trip(&candidate);
}

#[test]
fn archive_preserves_initial_default_even_when_it_historically_references_target() {
    let mut value = choice_with_second_option_value();
    value["fields"][FIELD_CHOICE_A]["initialDefaultValue"] =
        json!({"kind": "singleChoice", "optionId": OPTION_ACTIVE_A});
    value["fields"][FIELD_CHOICE_A]["defaultValue"] =
        json!({"kind": "singleChoice", "optionId": OPTION_SECOND_A});
    let source = fixture_from_value(&value);
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(field_id, target, None),
    );
    assert_eq!(
        candidate.fields()[&field_id].initial_default_value(),
        &initial_before
    );
    assert_eq!(
        candidate.fields()[&field_id]
            .initial_default_value()
            .single_choice(),
        Some(target)
    );
    assert_round_trip(&candidate);
}

#[test]
fn archive_repairs_current_but_preserves_same_target_in_historical_initial() {
    let mut value = choice_with_second_option_value();
    value["fields"][FIELD_CHOICE_A]["initialDefaultValue"] =
        json!({"kind": "singleChoice", "optionId": OPTION_ACTIVE_A});
    let source = fixture_from_value(&value);
    let field_id = id(FIELD_CHOICE_A);
    let target = option_id(OPTION_ACTIVE_A);
    let replacement = option_id(OPTION_SECOND_A);
    let initial_before = source.fields()[&field_id].initial_default_value().clone();

    let candidate = changed(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::single_choice(replacement)),
        ),
    );
    assert_eq!(
        candidate.fields()[&field_id]
            .default_value()
            .single_choice(),
        Some(replacement)
    );
    assert_eq!(
        candidate.fields()[&field_id].initial_default_value(),
        &initial_before
    );
    assert_eq!(
        candidate.fields()[&field_id]
            .initial_default_value()
            .single_choice(),
        Some(target)
    );
    assert_round_trip(&candidate);
}

#[test]
fn archive_repair_rejects_wrong_kind_target_archived_unknown_and_noncanonical_values() {
    let source = fixture();
    let single_field = id(FIELD_CHOICE_A);
    let single_target = option_id(OPTION_ACTIVE_A);
    let multi_field = id(FIELD_CHOICE_B);
    let multi_target = option_id(OPTION_ACTIVE_B1);

    let wrong_kind = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            single_field,
            single_target,
            Some(FieldValueDraft::single_line_text("wrong kind".to_owned())),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        single_field,
        Some(single_target),
    );
    assert_eq!(
        wrong_kind.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldValueKindMismatch)
    );
    assert_eq!(wrong_kind.validation_choice_category(), None);
    assert_error_is_redacted(&wrong_kind);

    let still_target = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            single_field,
            single_target,
            Some(FieldValueDraft::single_choice(single_target)),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        single_field,
        Some(single_target),
    );
    assert_eq!(
        still_target.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        still_target.validation_choice_category(),
        Some(ChoiceValidationErrorCategory::ArchivedOptionNotSelectable)
    );
    assert_eq!(
        still_target.validation_choice_location(),
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
    assert_error_is_redacted(&still_target);

    for (replacement, expected_choice_category) in [
        (
            FieldValueDraft::single_choice(option_id(OPTION_ARCHIVED_A)),
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ),
        (
            FieldValueDraft::single_choice(option_id(OPTION_UNKNOWN)),
            ChoiceValidationErrorCategory::UnknownSelectedOption,
        ),
    ] {
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::archive_option(single_field, single_target, Some(replacement)),
            TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
            single_field,
            Some(single_target),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(
            error.validation_choice_category(),
            Some(expected_choice_category)
        );
        assert_eq!(
            error.validation_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
        if expected_choice_category == ChoiceValidationErrorCategory::UnknownSelectedOption {
            let rendered = render_mutation_error_chain(&error);
            assert!(!rendered.contains(OPTION_UNKNOWN));
            assert!(rendered.contains(OPTION_ACTIVE_A));
        }
        assert_error_is_redacted(&error);
    }

    let cross_field = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            single_field,
            single_target,
            Some(FieldValueDraft::single_choice(option_id(OPTION_ACTIVE_B1))),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        single_field,
        Some(single_target),
    );
    assert_eq!(
        cross_field.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        cross_field.validation_choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );
    assert_eq!(
        cross_field.validation_choice_location(),
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
    assert!(!render_mutation_error_chain(&cross_field).contains(OPTION_ACTIVE_B1));
    assert_error_is_redacted(&cross_field);

    for (replacement, expected_choice_category) in [
        (
            FieldValueDraft::multi_choice(Vec::new()),
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
        ),
        (
            FieldValueDraft::multi_choice(vec![
                option_id(OPTION_ACTIVE_B2),
                option_id(OPTION_ACTIVE_B2),
            ]),
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
        ),
        (
            FieldValueDraft::multi_choice(vec![
                option_id(OPTION_ACTIVE_B2),
                option_id(OPTION_ACTIVE_B1),
            ]),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
        (
            FieldValueDraft::multi_choice(vec![option_id(OPTION_ACTIVE_B1)]),
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ),
    ] {
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::archive_option(multi_field, multi_target, Some(replacement)),
            TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
            multi_field,
            Some(multi_target),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(
            error.validation_choice_category(),
            Some(expected_choice_category)
        );
        assert_eq!(
            error.validation_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
        assert_error_is_redacted(&error);
    }
}

#[test]
fn multi_repair_membership_matrix_uses_canonical_shapes_and_redacts_foreign_ids() {
    let source = fixture_from_value(&multi_with_local_archived_option_value());
    let field_id = id(FIELD_CHOICE_B);
    let target = option_id(OPTION_ACTIVE_B1);
    let active = option_id(OPTION_ACTIVE_B2);
    let foreign = option_id(OPTION_ACTIVE_A);
    let unknown = option_id(OPTION_UNKNOWN);
    let other_archived = option_id(OPTION_ARCHIVED_B);
    let cases = [
        (
            vec![foreign],
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            Some(foreign),
        ),
        (
            vec![unknown],
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            Some(unknown),
        ),
        (
            vec![other_archived],
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
            Some(other_archived),
        ),
        (
            vec![target],
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
            None,
        ),
        (
            vec![foreign, active],
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            Some(foreign),
        ),
        (
            vec![active, unknown],
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            Some(unknown),
        ),
        (
            vec![active, other_archived],
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
            Some(other_archived),
        ),
    ];

    for (replacement, expected_choice_category, hidden_option_id) in cases {
        assert!(!replacement.is_empty());
        assert!(
            replacement.windows(2).all(|pair| pair[0] < pair[1]),
            "membership fixture must be sorted and unique before validation"
        );
        let error = assert_command_error(
            &source,
            TemplateMutationCommand::archive_option(
                field_id,
                target,
                Some(FieldValueDraft::multi_choice(replacement)),
            ),
            TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
            field_id,
            Some(target),
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(
            error.validation_choice_category(),
            Some(expected_choice_category)
        );
        assert_eq!(
            error.validation_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
        let rendered = render_mutation_error_chain(&error);
        if let Some(hidden_option_id) = hidden_option_id {
            assert!(!rendered.contains(&hidden_option_id.to_string()));
        }
        assert!(rendered.contains(&target.to_string()));
        assert_error_is_redacted(&error);
    }
}

#[test]
fn archive_repair_preserves_fresh_metadata_and_rejects_provenance_free_values() {
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_B]["defaultValue"]["futureChoiceDefault"] = json!({
        "nested": [{"keep": true}],
        "number": serde_json::from_str::<Value>(ARBITRARY_NUMBER)
            .expect("arbitrary precision fixture number must parse")
    });
    let source = fixture_from_value(&value);
    let field_id = id(FIELD_CHOICE_B);
    let target = option_id(OPTION_ACTIVE_B1);
    let remaining = option_id(OPTION_ACTIVE_B2);
    let source_default_wire =
        template_wire_value(&source)["fields"][FIELD_CHOICE_B]["defaultValue"].clone();

    let fresh = changed(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::multi_choice(vec![remaining])),
        ),
    );
    let fresh_default = &template_wire_value(&fresh)["fields"][FIELD_CHOICE_B]["defaultValue"];
    assert_eq!(
        fresh_default["futureChoiceDefault"], source_default_wire["futureChoiceDefault"],
        "fresh repair must transport persisted outer metadata exactly"
    );
    assert_round_trip(&fresh);

    let mut matching_existing = value.clone();
    matching_existing["fields"][FIELD_CHOICE_B]["defaultValue"]["optionIds"] =
        json!([OPTION_ACTIVE_B2]);
    let matching_existing = current_default_from(&matching_existing, field_id);
    assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::provenance_free_replacement(
                matching_existing,
            )),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        field_id,
        Some(target),
    );

    let mut changed_metadata = value;
    changed_metadata["fields"][FIELD_CHOICE_B]["defaultValue"]["optionIds"] =
        json!([OPTION_ACTIVE_B2]);
    changed_metadata["fields"][FIELD_CHOICE_B]["defaultValue"]["futureChoiceDefault"] =
        json!({"credential": CHANGED_METADATA_SECRET});
    let changed_metadata = current_default_from(&changed_metadata, field_id);
    let error = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::provenance_free_replacement(
                changed_metadata,
            )),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        field_id,
        Some(target),
    );
    assert_eq!(error.validation_category(), None);
    assert_error_is_redacted(&error);

    let mut lost_metadata = fixture_value();
    lost_metadata["fields"][FIELD_CHOICE_B]["defaultValue"]["optionIds"] =
        json!([OPTION_ACTIVE_B2]);
    let lost_metadata = current_default_from(&lost_metadata, field_id);
    let lost_error = assert_command_error(
        &source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::provenance_free_replacement(lost_metadata)),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        field_id,
        Some(target),
    );
    assert_eq!(lost_error.validation_category(), None);
    assert_error_is_redacted(&lost_error);

    let metadata_free_source = fixture();
    let mut injected_metadata = fixture_value();
    injected_metadata["fields"][FIELD_CHOICE_B]["defaultValue"]["optionIds"] =
        json!([OPTION_ACTIVE_B2]);
    injected_metadata["fields"][FIELD_CHOICE_B]["defaultValue"]["candidateOnly"] =
        json!({"credential": CHANGED_METADATA_SECRET});
    let injected_metadata = current_default_from(&injected_metadata, field_id);
    let injected_error = assert_command_error(
        &metadata_free_source,
        TemplateMutationCommand::archive_option(
            field_id,
            target,
            Some(FieldValueDraft::provenance_free_replacement(
                injected_metadata,
            )),
        ),
        TemplateMutationErrorCategory::InvalidCurrentDefaultRepair,
        field_id,
        Some(target),
    );
    assert_eq!(injected_error.validation_category(), None);
    assert_error_is_redacted(&injected_error);
}

#[test]
fn option_commands_preserve_unrelated_rich_text_unknown_metadata_exactly() {
    let source = fixture_from_value(&rich_text_unknown_fixture_value());
    let before = encode_template(&source).expect("metadata source must encode");
    let rich_field = id(FIELD_RICH_TEXT);
    let rich_before = field_wire_bytes(&source, rich_field);
    let option_new = option_id(OPTION_NEW);

    let commands = [
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(option_new, "new option".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "renamed".to_owned(),
        ),
        TemplateMutationCommand::reorder_options(
            id(FIELD_CHOICE_B),
            vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_B2)],
        ),
        TemplateMutationCommand::archive_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            Some(FieldValueDraft::unset()),
        ),
    ];

    for command in commands {
        let candidate = changed(&source, command);
        assert_eq!(
            field_wire_bytes(&candidate, rich_field),
            rich_before,
            "unrelated rich-text subtree including occurrence and number lexeme must be exact"
        );
        assert_round_trip(&candidate);
        assert_eq!(
            encode_template(&source).expect("command must not mutate source"),
            before
        );
    }
}

#[test]
fn all_option_commands_preserve_template_field_configuration_option_and_default_extras() {
    let mut value = fixture_value();
    value["fields"][FIELD_CHOICE_A]["futureChoiceField"] = json!({"keep": [1, 2]});
    value["fields"][FIELD_CHOICE_A]["configuration"]["futureChoiceConfiguration"] =
        json!({"keep": true});
    value["fields"][FIELD_CHOICE_A]["defaultValue"]["futureChoiceCurrent"] =
        json!({"keep": [ARBITRARY_NUMBER]});
    value["fields"][FIELD_CHOICE_A]["initialDefaultValue"]["futureChoiceInitial"] =
        json!({"credential": METADATA_SECRET});
    let source = fixture_from_value(&value);
    let source_wire = template_wire_value(&source);
    let new_option = option_id(OPTION_NEW);
    let commands = [
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "new option".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "renamed".to_owned(),
        ),
        TemplateMutationCommand::reorder_options(
            id(FIELD_CHOICE_B),
            vec![option_id(OPTION_ACTIVE_B1), option_id(OPTION_ACTIVE_B2)],
        ),
        TemplateMutationCommand::archive_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            Some(FieldValueDraft::unset()),
        ),
    ];

    for command in commands {
        let candidate = changed(&source, command);
        let candidate_wire = template_wire_value(&candidate);
        for path in [
            &["futureRoot"][..],
            &["fields", FIELD_CHOICE_A, "futureChoiceField"][..],
            &[
                "fields",
                FIELD_CHOICE_A,
                "configuration",
                "futureChoiceConfiguration",
            ][..],
            &[
                "fields",
                FIELD_CHOICE_A,
                "configuration",
                "options",
                OPTION_ACTIVE_A,
                "futureOption",
            ][..],
            &[
                "fields",
                FIELD_CHOICE_A,
                "defaultValue",
                "futureChoiceCurrent",
            ][..],
            &[
                "fields",
                FIELD_CHOICE_A,
                "initialDefaultValue",
                "futureChoiceInitial",
            ][..],
        ] {
            let expected = path
                .iter()
                .fold(&source_wire, |current, member| &current[*member]);
            let actual = path
                .iter()
                .fold(&candidate_wire, |current, member| &current[*member]);
            assert_eq!(actual, expected, "metadata path {path:?} must remain exact");
        }
        assert_round_trip(&candidate);
    }
}

#[test]
fn option_commands_share_revision_timestamp_and_overflow_contract() {
    let source = fixture();
    let equal_timestamp = apply_template_mutation(
        &source,
        source.revision(),
        UPDATED_AT,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "changed once".to_owned(),
        ),
    )
    .expect("equal timestamp is valid for a changed Option command")
    .into_changed()
    .expect("rename must change");
    assert_eq!(
        equal_timestamp.revision().get(),
        source.revision().get() + 1
    );
    assert_eq!(equal_timestamp.updated_at_utc(), UPDATED_AT);

    for (timestamp, category) in [
        (
            CREATED_AT,
            TemplateMutationErrorCategory::TimestampRegression,
        ),
        (
            "not-a-timestamp",
            TemplateMutationErrorCategory::InvalidTimestamp,
        ),
    ] {
        let error = apply_template_mutation(
            &source,
            source.revision(),
            timestamp,
            TemplateMutationCommand::rename_option(
                id(FIELD_CHOICE_A),
                option_id(OPTION_ACTIVE_A),
                "changed".to_owned(),
            ),
        )
        .expect_err("invalid command timestamp must fail before target mutation");
        assert_eq!(error.category(), category);
        assert_eq!(error.field_id(), None);
        assert_eq!(error.option_id(), None);
    }

    let mut max_value = fixture_value();
    max_value["revision"] = json!(u32::MAX);
    let max_source = fixture_from_value(&max_value);
    let max_before = encode_template(&max_source).expect("MAX revision source must encode");
    let overflow = run(
        &max_source,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "changed".to_owned(),
        ),
    )
    .expect_err("changed Option command at MAX revision must fail");
    assert_eq!(
        overflow.category(),
        TemplateMutationErrorCategory::RevisionOverflow
    );
    assert_eq!(overflow.field_id(), None);
    assert_eq!(overflow.option_id(), None);
    assert_eq!(
        encode_template(&max_source).expect("overflow source must remain encodable"),
        max_before
    );
}

#[test]
fn large_option_set_reorders_without_sorting_caller_display_order() {
    const OPTION_COUNT: usize = 1_024;
    let mut value = metadata_isolated_fixture_value();
    let mut options = serde_json::Map::new();
    let mut ascending = Vec::with_capacity(OPTION_COUNT);
    for index in 0..OPTION_COUNT {
        let option = format!("d0000000-0000-4000-8000-{index:012x}");
        ascending.push(option.clone());
        options.insert(
            option,
            json!({"label": format!("Option {index}"), "lifecycle": "active"}),
        );
    }
    value["fields"][FIELD_CHOICE_B]["configuration"]["options"] = Value::Object(options);
    value["fields"][FIELD_CHOICE_B]["configuration"]["optionOrder"] = json!(ascending);
    value["fields"][FIELD_CHOICE_B]["defaultValue"] = json!({"kind": "unset"});
    let source = fixture_from_value(&value);
    let mut reversed = source.fields()[&id(FIELD_CHOICE_B)]
        .configuration()
        .option_order()
        .expect("large Choice must have optionOrder")
        .to_vec();
    reversed.reverse();

    let candidate = changed(
        &source,
        TemplateMutationCommand::reorder_options(id(FIELD_CHOICE_B), reversed.clone()),
    );
    assert_eq!(
        candidate.fields()[&id(FIELD_CHOICE_B)]
            .configuration()
            .option_order(),
        Some(reversed.as_slice())
    );
    assert_eq!(
        candidate.fields()[&id(FIELD_CHOICE_B)]
            .configuration()
            .options()
            .expect("large Choice options")
            .len(),
        OPTION_COUNT
    );
    assert_round_trip(&candidate);
}

#[test]
fn stale_revision_rejects_option_noop_before_command_application() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let error = apply_template_mutation(
        &source,
        revision(1),
        LATER_AT,
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "active option".to_owned(),
        ),
    )
    .expect_err("stale Option no-op must fail");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    assert_eq!(error.field_id(), None);
    assert_eq!(error.option_id(), None);
    assert_eq!(
        encode_template(&source).expect("stale command must not mutate source"),
        before
    );
}
