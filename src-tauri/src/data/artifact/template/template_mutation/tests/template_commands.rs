use std::error::Error;

use crate::data::{
    artifact::{
        decode_document, ArtifactCodecErrorCategory, ArtifactCodecStage,
        ArtifactScalarValueLocation, DocumentArtifact, FieldKind,
    },
    field_engine::scalar::ScalarValueErrorCategory,
};

use super::*;

const DOCUMENT_TARGET_EMPTY: &str = "e1000000-0000-4000-8000-000000000001";
const DOCUMENT_TARGET_ORPHAN: &str = "e1000000-0000-4000-8000-000000000002";
const DOCUMENT_OTHER: &str = "e1000000-0000-4000-8000-000000000003";
const DOCUMENT_OTHER_A: &str = "e1000000-0000-4000-8000-000000000004";
const DOCUMENT_OTHER_B: &str = "e1000000-0000-4000-8000-000000000005";
const DOCUMENT_INVALID: &str = "e1000000-0000-4000-8000-000000000006";
const FIELD_TOMBSTONE_NEW: &str = "e2000000-0000-4000-8000-000000000001";
const OPTION_TOMBSTONE_NEW: &str = "e3000000-0000-4000-8000-000000000001";
const AFTER_TOMBSTONE: &str = "2026-09-03T04:05:06.007Z";
const DOCUMENT_SECRET: &str =
    "credential=document-secret path=C:\\private\\document.json /home/private/document.json";

fn run(
    source: &TemplateArtifact,
    timestamp: &str,
    command: TemplateMutationCommand,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    apply_template_mutation(source, source.revision(), timestamp, command)
}

fn complete_snapshot(documents: &[DocumentArtifact]) -> CompleteDocumentSnapshot<'_> {
    CompleteDocumentSnapshot::from_complete_enumeration_for_test(documents)
}

fn assessment(
    source: &TemplateArtifact,
    documents: &[DocumentArtifact],
) -> TemplateReferenceAssessment {
    assess_template_references(source, complete_snapshot(documents))
        .expect("no-reference snapshot must produce a tombstone assessment")
}

fn changed(
    source: &TemplateArtifact,
    timestamp: &str,
    command: TemplateMutationCommand,
) -> TemplateArtifact {
    run(source, timestamp, command)
        .expect("valid Template command must succeed")
        .into_changed()
        .expect("Template command must return a changed candidate")
}

fn assert_round_trip(template: &TemplateArtifact) {
    template
        .validate_storage()
        .expect("Template command candidate must be storage-valid");
    let encoded = encode_template(template).expect("Template command candidate must encode");
    let decoded = decode_template(&encoded).expect("Template command candidate must decode");
    assert_eq!(decoded, *template);
    assert_eq!(
        encode_template(&decoded).expect("decoded candidate must re-encode"),
        encoded
    );
}

fn assert_only_root_members_changed(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    members: &[&str],
) {
    let mut source_wire = template_wire_value(source);
    let mut candidate_wire = template_wire_value(candidate);
    for member in members {
        let source_member = source_wire
            .as_object_mut()
            .expect("source Template wire must be an object")
            .remove(*member)
            .expect("source changed member must exist");
        let candidate_member = candidate_wire
            .as_object_mut()
            .expect("candidate Template wire must be an object")
            .remove(*member)
            .expect("candidate changed member must exist");
        assert_ne!(source_member, candidate_member);
    }
    assert_eq!(
        candidate_wire, source_wire,
        "every non-target persisted member must remain exact"
    );
}

fn assert_only_presentation_token_changed(source: &TemplateArtifact, candidate: &TemplateArtifact) {
    let mut source_presentation = template_wire_value(source)["presentation"].clone();
    let mut candidate_presentation = template_wire_value(candidate)["presentation"].clone();
    let source_token = source_presentation
        .as_object_mut()
        .expect("source presentation must be an object")
        .remove("token");
    let candidate_token = candidate_presentation
        .as_object_mut()
        .expect("candidate presentation must be an object")
        .remove("token");
    assert_ne!(source_token, candidate_token);
    assert_eq!(
        candidate_presentation, source_presentation,
        "presentation unknown metadata must remain exact"
    );
}

fn document_value(document_id: &str, template_id: &str) -> Value {
    json!({
        "artifactType": "document",
        "createdAtUtc": CREATED_AT,
        "documentId": document_id,
        "fieldValues": {},
        "name": DOCUMENT_SECRET,
        "orphanedFieldDefinitions": {},
        "schemaVersion": 1,
        "templateId": template_id,
        "templateRevision": 1,
        "updatedAtUtc": UPDATED_AT,
        "futureDocument": {
            "credential": DOCUMENT_SECRET,
            "content": "empty fieldValues still references its Template"
        }
    })
}

fn document_from_value(value: &Value) -> DocumentArtifact {
    decode_document(
        &to_deterministic_json_bytes(value)
            .expect("Document fixture must encode deterministically"),
    )
    .expect("Document fixture must decode")
}

fn document(document_id: &str, template_id: &str) -> DocumentArtifact {
    document_from_value(&document_value(document_id, template_id))
}

fn orphan_document() -> DocumentArtifact {
    let mut value = document_value(DOCUMENT_TARGET_ORPHAN, TEMPLATE_ID);
    value["fieldValues"][FIELD_ARCHIVED] = json!({
        "kind": "text",
        "value": "historical orphan value"
    });
    value["orphanedFieldDefinitions"][FIELD_ARCHIVED] = json!({
        "kind": "singleLineText",
        "label": DOCUMENT_SECRET,
        "options": {},
        "futureOrphan": {"path": "C:\\private\\orphan.json"}
    });
    document_from_value(&value)
}

fn template_with_name(name: &str) -> TemplateArtifact {
    let mut value = fixture_value();
    value["name"] = json!(name);
    fixture_from_value(&value)
}

fn template_with_presentation_token(token: Option<&str>) -> TemplateArtifact {
    let mut value = fixture_value();
    value["presentation"]["token"] = match token {
        Some(token) => json!(token),
        None => Value::Null,
    };
    if token.is_none() {
        value["presentation"]
            .as_object_mut()
            .expect("Template presentation must be an object")
            .remove("token");
    }
    fixture_from_value(&value)
}

fn other_template() -> TemplateArtifact {
    let mut value = fixture_value();
    value["templateId"] = json!(OTHER_TEMPLATE_ID);
    fixture_from_value(&value)
}

fn tombstoned_fixture() -> TemplateArtifact {
    let source = fixture();
    let proof = assessment(&source, &[]);
    changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::tombstone_template(proof),
    )
}

fn assert_template_error(
    source: &TemplateArtifact,
    timestamp: &str,
    command: TemplateMutationCommand,
    category: TemplateMutationErrorCategory,
) -> TemplateMutationError {
    let before = encode_template(source).expect("source must encode before rejected command");
    let error = run(source, timestamp, command).expect_err("Template command must fail");
    assert_eq!(error.category(), category);
    assert_eq!(error.template_id(), Some(source.template_id()));
    assert_eq!(error.field_id(), None);
    assert_eq!(error.option_id(), None);
    assert_eq!(error.validation_category(), None);
    assert!(error.source().is_none());
    assert_eq!(
        encode_template(source).expect("rejected command must preserve source"),
        before
    );
    error
}

fn assert_error_redacted(error: &TemplateMutationError, hidden_values: &[&str]) {
    let rendered = render_mutation_error_chain(error);
    for value in [
        SECRET,
        METADATA_SECRET,
        ARBITRARY_NUMBER,
        DOCUMENT_SECRET,
        "C:\\private",
        "/home/private",
    ]
    .into_iter()
    .chain(hidden_values.iter().copied())
    {
        assert!(
            !rendered.contains(value),
            "Template command error must not expose protected payload"
        );
    }
}

fn assert_reference_error(
    source: &TemplateArtifact,
    documents: &[DocumentArtifact],
    expected_count: u16,
    expected_truncated: bool,
    hidden_values: &[&str],
) -> TemplateMutationError {
    let before = encode_template(source).expect("reference scan source must encode");
    let result = assess_template_references(source, complete_snapshot(documents));
    let error = result.expect_err("references must return neither proof nor tombstone candidate");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::TemplateHasDocuments
    );
    assert_eq!(error.template_id(), Some(source.template_id()));
    assert_eq!(error.field_id(), None);
    assert_eq!(error.option_id(), None);
    assert_eq!(error.reference_count(), Some(expected_count));
    assert_eq!(error.reference_count_truncated(), expected_truncated);
    assert_eq!(error.validation_category(), None);
    assert!(error.source().is_none());
    assert_error_redacted(&error, hidden_values);
    assert_eq!(
        encode_template(source).expect("reference scan must preserve source"),
        before
    );
    error
}

fn scalar_invalid_document() -> DocumentArtifact {
    let mut value = document_value(DOCUMENT_INVALID, OTHER_TEMPLATE_ID);
    value["fieldValues"][FIELD_TEXT] = json!({"kind": "text", "value": "valid"});
    let mut invalid = document_from_value(&value);
    invalid
        .validate_structure()
        .expect("scalar fixture must be valid before its only corruption");
    invalid.corrupt_scalar_value_for_test(id(FIELD_TEXT), &format!("line one\n{DOCUMENT_SECRET}"));
    let validation = invalid
        .validate_structure()
        .expect_err("scalar corruption must have one exact validation failure");
    assert_eq!(
        validation.category(),
        ArtifactValidationErrorCategory::InvalidScalarValue
    );
    assert_eq!(
        validation.scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        validation.scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    invalid
}

fn depth_invalid_document() -> DocumentArtifact {
    let mut invalid = document(DOCUMENT_INVALID, OTHER_TEMPLATE_ID);
    let mut nested = json!(DOCUMENT_SECRET);
    for _ in 0..MAX_JSON_NESTING_DEPTH {
        nested = json!([nested]);
    }
    invalid.corrupt_extra_depth_for_test(nested);
    invalid
        .validate_structure()
        .expect("subtree depth must remain valid before whole private-wire validation");
    invalid
}

fn assert_scalar_invalid_snapshot(
    source: &TemplateArtifact,
    documents: &[DocumentArtifact],
) -> TemplateMutationError {
    let source_before = source.clone();
    let bytes_before = encode_template(source).expect("invalid-order source must encode");
    let documents_before = documents.to_vec();
    let result = assess_template_references(source, complete_snapshot(documents));
    let error = result.expect_err("invalid snapshot must return neither assessment nor proof");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::InvalidDocumentSnapshot
    );
    assert_ne!(
        error.category(),
        TemplateMutationErrorCategory::TemplateHasDocuments
    );
    assert_eq!(error.template_id(), None);
    assert_eq!(error.field_id(), None);
    assert_eq!(error.option_id(), None);
    assert_eq!(error.reference_count(), None);
    assert!(!error.reference_count_truncated());
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        error.validation_scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        error.validation_scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    assert!(error.source().is_none());
    assert_error_redacted(&error, &[DOCUMENT_INVALID, "line one"]);
    assert_eq!(source, &source_before);
    assert_eq!(documents, documents_before);
    assert_eq!(
        encode_template(source).expect("invalid snapshot must preserve source"),
        bytes_before
    );
    error
}

fn assert_depth_invalid_snapshot(
    source: &TemplateArtifact,
    documents: &[DocumentArtifact],
) -> TemplateMutationError {
    let source_before = source.clone();
    let bytes_before = encode_template(source).expect("depth-order source must encode");
    let documents_before = documents.to_vec();
    let result = assess_template_references(source, complete_snapshot(documents));
    let error = result.expect_err("whole-wire-invalid snapshot must not produce an assessment");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::InvalidDocumentSnapshot
    );
    assert_ne!(
        error.category(),
        TemplateMutationErrorCategory::TemplateHasDocuments
    );
    assert_eq!(error.template_id(), None);
    assert_eq!(error.field_id(), None);
    assert_eq!(error.option_id(), None);
    assert_eq!(error.reference_count(), None);
    assert!(!error.reference_count_truncated());
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_eq!(error.validation_scalar_category(), None);
    assert_eq!(error.validation_scalar_location(), None);
    assert!(error.source().is_none());
    assert_error_redacted(&error, &[DOCUMENT_INVALID]);
    assert_eq!(source, &source_before);
    assert_eq!(documents, documents_before);
    assert_eq!(
        encode_template(source).expect("depth-invalid snapshot must preserve source"),
        bytes_before
    );
    error
}

fn assert_zero_reference_input_is_admissible(
    source: &TemplateArtifact,
    documents: &[DocumentArtifact],
) {
    let proof = assess_template_references(source, complete_snapshot(documents))
        .expect("valid unrelated Documents must form an admissible zero-reference input");
    assert_eq!(proof.template_id(), source.template_id());
    assert_eq!(proof.source_revision(), source.revision());
}

#[test]
fn template_name_and_presentation_are_closed_mutable_metadata() {
    let source = fixture();
    let before = encode_template(&source).expect("metadata source must encode");
    let name = "  opaque\r\nUnicode\u{0085}\u{2028}\u{2029}🙂  ";
    let candidate = changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_template_name(name.to_owned()),
    );

    assert_eq!(candidate.name(), name);
    assert_eq!(candidate.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(candidate.revision(), revision(3));
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    assert_only_root_members_changed(&source, &candidate, &["name", "revision", "updatedAtUtc"]);
    assert_round_trip(&candidate);
    assert_eq!(
        encode_template(&source).expect("metadata command must not mutate source"),
        before
    );

    let presentation_candidate = changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_template_presentation_token(Some(
            "  opaque\r\nUnicode\u{2028}🙂  ".to_owned(),
        )),
    );
    assert_eq!(
        presentation_candidate.presentation().token(),
        Some("  opaque\r\nUnicode\u{2028}🙂  ")
    );
    assert_eq!(
        presentation_candidate.lifecycle(),
        TemplateLifecycle::Active
    );
    assert_only_root_members_changed(
        &source,
        &presentation_candidate,
        &["presentation", "revision", "updatedAtUtc"],
    );
    assert_only_presentation_token_changed(&source, &presentation_candidate);
    assert_round_trip(&presentation_candidate);

    let cleared = changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_template_presentation_token(None),
    );
    assert_eq!(cleared.presentation().token(), None);
    assert_only_root_members_changed(
        &source,
        &cleared,
        &["presentation", "revision", "updatedAtUtc"],
    );
    assert_only_presentation_token_changed(&source, &cleared);
    assert_round_trip(&cleared);
}

#[test]
fn exact_opaque_template_names_are_noops_after_revision_precondition() {
    for name in [
        "",
        "line one\nline two",
        "line one\rline two",
        "next\u{0085}line",
        "next\u{2028}line",
        "next\u{2029}line",
        "  공백과 Unicode 🙂  ",
    ] {
        let source = template_with_name(name);
        let before = encode_template(&source).expect("opaque-name source must encode");
        let outcome = run(
            &source,
            LATER_AT,
            TemplateMutationCommand::set_template_name(name.to_owned()),
        )
        .expect("exact opaque name must be a valid no-op");
        assert!(outcome.is_unchanged());
        assert!(outcome.changed().is_none());
        assert_eq!(source.revision(), revision(2));
        assert_eq!(source.updated_at_utc(), UPDATED_AT);

        let stale = apply_template_mutation(
            &source,
            revision(1),
            LATER_AT,
            TemplateMutationCommand::set_template_name(name.to_owned()),
        )
        .expect_err("stale revision must precede metadata no-op detection");
        assert_eq!(
            stale.category(),
            TemplateMutationErrorCategory::RevisionMismatch
        );
        assert_eq!(stale.template_id(), None);
        assert_eq!(
            encode_template(&source).expect("metadata no-op must preserve source"),
            before
        );
    }

    for token in [
        None,
        Some(""),
        Some("line one\nline two"),
        Some("line one\rline two"),
        Some("next\u{0085}\u{2028}\u{2029}line"),
        Some("  토큰 🙂  "),
    ] {
        let source = template_with_presentation_token(token);
        let before = encode_template(&source).expect("opaque-token source must encode");
        let outcome = run(
            &source,
            LATER_AT,
            TemplateMutationCommand::set_template_presentation_token(token.map(str::to_owned)),
        )
        .expect("exact opaque presentation token must be a valid no-op");
        assert!(outcome.is_unchanged());

        let stale = apply_template_mutation(
            &source,
            revision(1),
            LATER_AT,
            TemplateMutationCommand::set_template_presentation_token(token.map(str::to_owned)),
        )
        .expect_err("stale revision must precede presentation no-op detection");
        assert_eq!(
            stale.category(),
            TemplateMutationErrorCategory::RevisionMismatch
        );
        assert_eq!(stale.template_id(), None);
        assert_eq!(
            encode_template(&source).expect("presentation no-op must preserve source"),
            before
        );
    }
}

#[test]
fn metadata_revision_and_timestamp_contract_is_unchanged() {
    let source = fixture();
    let equal = changed(
        &source,
        UPDATED_AT,
        TemplateMutationCommand::set_template_name("equal timestamp".to_owned()),
    );
    assert_eq!(equal.revision(), revision(3));
    assert_eq!(equal.updated_at_utc(), UPDATED_AT);

    for (timestamp, category) in [
        (
            "2026-09-03T02:03:04.004Z",
            TemplateMutationErrorCategory::TimestampRegression,
        ),
        (
            "2026-09-03T03:04:05Z",
            TemplateMutationErrorCategory::InvalidTimestamp,
        ),
        (
            "not-a-timestamp",
            TemplateMutationErrorCategory::InvalidTimestamp,
        ),
    ] {
        let result = run(
            &source,
            timestamp,
            TemplateMutationCommand::set_template_name("must fail".to_owned()),
        )
        .expect_err("invalid metadata timestamp must fail");
        assert_eq!(result.category(), category);
        assert_eq!(result.template_id(), None);
    }

    let mut max_value = fixture_value();
    max_value["revision"] = json!(u32::MAX);
    let max_source = fixture_from_value(&max_value);
    let max_before = encode_template(&max_source).expect("MAX revision source must encode");
    assert!(run(
        &max_source,
        LATER_AT,
        TemplateMutationCommand::set_template_name(max_source.name().to_owned()),
    )
    .expect("MAX revision metadata no-op must succeed")
    .is_unchanged());
    let overflow = run(
        &max_source,
        LATER_AT,
        TemplateMutationCommand::set_template_name("changed at MAX".to_owned()),
    )
    .expect_err("changed metadata at MAX revision must fail");
    assert_eq!(
        overflow.category(),
        TemplateMutationErrorCategory::RevisionOverflow
    );
    assert_eq!(overflow.template_id(), None);
    assert_eq!(
        encode_template(&max_source).expect("overflow must preserve source"),
        max_before
    );
}

#[test]
fn reference_scan_uses_template_binding_only_for_every_document_shape() {
    let source = fixture();
    let target_empty = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    assert!(target_empty.field_values().is_empty());
    assert!(target_empty.orphaned_field_definitions().is_empty());
    assert_eq!(target_empty.template_revision(), revision(1));
    let target_orphan = orphan_document();
    assert!(!target_orphan.orphaned_field_definitions().is_empty());
    let other = document(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);

    let none = assessment(&source, &[]);
    assert_eq!(none.template_id(), source.template_id());
    assert_eq!(none.source_revision(), source.revision());

    let other_only = assessment(&source, std::slice::from_ref(&other));
    assert_eq!(other_only.template_id(), source.template_id());
    assert_eq!(other_only.source_revision(), source.revision());

    let mixed = [other.clone(), target_empty.clone(), target_orphan.clone()];
    let mixed_reversed = [target_orphan, target_empty, other];
    let forward = assess_template_references(&source, complete_snapshot(&mixed))
        .expect_err("mixed snapshot must report target references");
    let reverse = assess_template_references(&source, complete_snapshot(&mixed_reversed))
        .expect_err("scan order must not hide target references");
    for error in [forward, reverse] {
        assert_eq!(
            error.category(),
            TemplateMutationErrorCategory::TemplateHasDocuments
        );
        assert_eq!(error.template_id(), Some(source.template_id()));
        assert_eq!(error.reference_count(), Some(2));
        assert!(!error.reference_count_truncated());
        assert!(error.source().is_none());
    }

    let snapshot_debug = format!("{:?}", complete_snapshot(&mixed));
    let assessment_debug = format!("{other_only:?}");
    for rendered in [&snapshot_debug, &assessment_debug] {
        assert!(rendered.contains("documents_redacted"));
        assert!(!rendered.contains(DOCUMENT_TARGET_EMPTY));
        assert!(!rendered.contains(DOCUMENT_TARGET_ORPHAN));
        assert!(!rendered.contains(DOCUMENT_OTHER));
        assert!(!rendered.contains(DOCUMENT_SECRET));
    }
}

#[test]
fn zero_references_issue_a_bound_assessment_and_allow_tombstone() {
    let source = fixture();
    let before = encode_template(&source).expect("zero-reference source must encode");
    let proof = assess_template_references(&source, complete_snapshot(&[]))
        .expect("zero references must issue a tombstone assessment");
    assert_eq!(proof.template_id(), source.template_id());
    assert_eq!(proof.source_revision(), source.revision());
    let candidate = run(
        &source,
        LATER_AT,
        TemplateMutationCommand::tombstone_template(proof),
    )
    .expect("zero-reference proof must authorize tombstone")
    .into_changed()
    .expect("tombstone must return a changed candidate");
    assert_eq!(candidate.lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(
        encode_template(&source).expect("tombstone must preserve source"),
        before
    );
}

#[test]
fn one_reference_reports_exact_untruncated_count_without_proof() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    let error = assert_reference_error(
        &source,
        std::slice::from_ref(&target),
        1,
        false,
        &[DOCUMENT_TARGET_EMPTY],
    );
    assert!(format!("{error}").contains("Document references 1"));
}

#[test]
fn exactly_1024_references_report_the_exact_untruncated_boundary() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    // Duplicate admission policy belongs to M2-6. The pure scanner remains conservative:
    // every admitted entry counts and duplicate input cannot hide a reference.
    let documents = vec![target; usize::from(MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES)];
    let error = assert_reference_error(
        &source,
        &documents,
        MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES,
        false,
        &[DOCUMENT_TARGET_EMPTY],
    );
    assert!(format!("{error}").contains("Document references 1024"));
    assert!(!format!("{error}").contains("1024+"));
}

#[test]
fn exactly_1025_references_report_the_bounded_truncated_boundary() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    let documents = vec![target; usize::from(MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES) + 1];
    let error = assert_reference_error(
        &source,
        &documents,
        MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES,
        true,
        &[DOCUMENT_TARGET_EMPTY],
    );
    assert!(format!("{error}").contains("Document references 1024+"));
}

#[test]
fn target_reference_at_slice_end_is_not_hidden_by_many_other_templates() {
    let source = fixture();
    let other = document(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);
    let mut documents = vec![other; 64];
    documents.push(document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID));
    assert_reference_error(
        &source,
        &documents,
        1,
        false,
        &[DOCUMENT_OTHER, DOCUMENT_TARGET_EMPTY],
    );
}

#[test]
fn invalid_document_cannot_be_admitted_or_issue_a_zero_reference_proof() {
    let source = fixture();
    let mut invalid_wire = document_value(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);
    invalid_wire["fieldValues"][FIELD_TEXT] = json!({
        "kind": "text",
        "value": format!("line one\n{DOCUMENT_SECRET}")
    });
    let bytes = to_deterministic_json_bytes(&invalid_wire)
        .expect("invalid Document wire fixture must encode deterministically");
    let codec_error = decode_document(&bytes)
        .expect_err("invalid Document bytes must not become a DocumentArtifact");
    assert_eq!(
        codec_error.category(),
        ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure
    );
    assert_eq!(codec_error.stage(), ArtifactCodecStage::SemanticValidation);
    assert_eq!(
        codec_error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        codec_error.scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        codec_error.scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    let codec_rendered = format!("{codec_error:?}\n{codec_error}");
    for hidden in [
        DOCUMENT_SECRET,
        DOCUMENT_OTHER,
        "C:\\private",
        "/home/private",
    ] {
        assert!(!codec_rendered.contains(hidden));
    }
    assert!(codec_error.source().is_none());

    // Test-only corruption bypasses codec construction so the production assessment boundary's
    // defense-in-depth validation is exercised directly.
    let mut valid_value = document_value(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);
    valid_value["fieldValues"][FIELD_TEXT] = json!({"kind": "text", "value": "valid"});
    let mut invalid = document_from_value(&valid_value);
    invalid.corrupt_scalar_value_for_test(id(FIELD_TEXT), &format!("line one\n{DOCUMENT_SECRET}"));
    let invalid_before = invalid.clone();
    let source_before = encode_template(&source).expect("invalid admission source must encode");
    let invalid_documents = [invalid.clone()];
    let result = assess_template_references(&source, complete_snapshot(&invalid_documents));
    let error = result.expect_err("invalid Document must not issue a zero-reference proof");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::InvalidDocumentSnapshot
    );
    assert_eq!(error.template_id(), None);
    assert_eq!(error.reference_count(), None);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        error.validation_scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        error.validation_scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    assert!(error.source().is_none());
    assert_error_redacted(&error, &[DOCUMENT_OTHER]);
    assert_eq!(invalid, invalid_before);
    assert_eq!(
        encode_template(&source).expect("invalid Document admission must preserve source"),
        source_before
    );

    let mut storage_invalid = document(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);
    let mut nested = json!(DOCUMENT_SECRET);
    for _ in 0..MAX_JSON_NESTING_DEPTH {
        nested = json!([nested]);
    }
    storage_invalid.corrupt_extra_depth_for_test(nested);
    let storage_before = storage_invalid.clone();
    let storage_documents = [storage_invalid.clone()];
    let storage_error = assess_template_references(&source, complete_snapshot(&storage_documents))
        .expect_err("whole-wire-invalid Document must not issue a zero-reference proof");
    assert_eq!(
        storage_error.category(),
        TemplateMutationErrorCategory::InvalidDocumentSnapshot
    );
    assert_eq!(
        storage_error.validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_eq!(storage_error.validation_scalar_category(), None);
    assert_eq!(storage_error.validation_scalar_location(), None);
    assert!(storage_error.source().is_none());
    assert_error_redacted(&storage_error, &[DOCUMENT_OTHER]);
    assert_eq!(storage_invalid, storage_before);
    assert_eq!(
        encode_template(&source).expect("whole-wire admission must preserve source"),
        source_before
    );
}

#[test]
fn scalar_invalid_document_at_the_first_position_fails_snapshot_admission() {
    let source = fixture();
    let valid = [
        document(DOCUMENT_OTHER_A, OTHER_TEMPLATE_ID),
        document(DOCUMENT_OTHER_B, OTHER_TEMPLATE_ID),
    ];
    assert_zero_reference_input_is_admissible(&source, &valid);
    let documents = [
        scalar_invalid_document(),
        valid[0].clone(),
        valid[1].clone(),
    ];
    assert_scalar_invalid_snapshot(&source, &documents);
}

#[test]
fn scalar_invalid_document_at_the_middle_position_fails_snapshot_admission() {
    let source = fixture();
    let valid = [
        document(DOCUMENT_OTHER_A, OTHER_TEMPLATE_ID),
        document(DOCUMENT_OTHER_B, OTHER_TEMPLATE_ID),
    ];
    assert_zero_reference_input_is_admissible(&source, &valid);
    let documents = [
        valid[0].clone(),
        scalar_invalid_document(),
        valid[1].clone(),
    ];
    assert_scalar_invalid_snapshot(&source, &documents);
}

#[test]
fn scalar_invalid_document_at_the_last_position_fails_snapshot_admission() {
    let source = fixture();
    let valid = [
        document(DOCUMENT_OTHER_A, OTHER_TEMPLATE_ID),
        document(DOCUMENT_OTHER_B, OTHER_TEMPLATE_ID),
    ];
    assert_zero_reference_input_is_admissible(&source, &valid);
    let documents = [
        valid[0].clone(),
        valid[1].clone(),
        scalar_invalid_document(),
    ];
    assert_scalar_invalid_snapshot(&source, &documents);
}

#[test]
fn scalar_invalid_document_after_a_valid_target_precedes_reference_reporting() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    assert_eq!(target.template_id(), source.template_id());
    assert_reference_error(
        &source,
        std::slice::from_ref(&target),
        1,
        false,
        &[DOCUMENT_TARGET_EMPTY],
    );
    let documents = [target, scalar_invalid_document()];
    assert_scalar_invalid_snapshot(&source, &documents);
}

#[test]
fn scalar_invalid_document_after_1024_valid_targets_precedes_the_count_cap() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    assert_eq!(target.template_id(), source.template_id());
    // Duplicate DocumentId admission is an M2-6 authority policy. At this pure scanner boundary,
    // each already-admitted entry is deliberately counted and cannot hide a later invalid entry.
    let valid_targets = vec![target; usize::from(MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES)];
    assert_reference_error(
        &source,
        &valid_targets,
        MAX_REPORTED_TEMPLATE_DOCUMENT_REFERENCES,
        false,
        &[DOCUMENT_TARGET_EMPTY],
    );
    let mut documents = valid_targets;
    documents.push(scalar_invalid_document());
    assert_scalar_invalid_snapshot(&source, &documents);
}

#[test]
fn whole_wire_depth_invalid_document_after_a_valid_target_precedes_reference_reporting() {
    let source = fixture();
    let target = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    assert_eq!(target.template_id(), source.template_id());
    assert_reference_error(
        &source,
        std::slice::from_ref(&target),
        1,
        false,
        &[DOCUMENT_TARGET_EMPTY],
    );
    let documents = [target, depth_invalid_document()];
    assert_depth_invalid_snapshot(&source, &documents);
}

#[test]
fn tombstone_preserves_the_complete_persisted_snapshot_except_lifecycle_clock_and_revision() {
    let mut value = rich_text_unknown_fixture_value();
    value["futureTombstoneNumber"] = serde_json::from_str::<Value>(ARBITRARY_NUMBER)
        .expect("arbitrary-precision number fixture must parse");
    let source = fixture_from_value(&value);
    let before = encode_template(&source).expect("tombstone source must encode");
    assert!(String::from_utf8_lossy(&before).contains(ARBITRARY_NUMBER));
    let unrelated = document(DOCUMENT_OTHER, OTHER_TEMPLATE_ID);
    let proof = assessment(&source, std::slice::from_ref(&unrelated));
    assert_eq!(proof.template_id(), source.template_id());

    let candidate = changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::tombstone_template(proof),
    );
    assert_eq!(candidate.lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(candidate.revision(), revision(3));
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    assert_only_root_members_changed(
        &source,
        &candidate,
        &["lifecycle", "revision", "updatedAtUtc"],
    );
    let encoded = encode_template(&candidate).expect("tombstone candidate must encode");
    assert!(String::from_utf8_lossy(&encoded).contains(ARBITRARY_NUMBER));
    assert_round_trip(&candidate);
    assert_eq!(
        encode_template(&source).expect("tombstone must not mutate source"),
        before
    );
}

#[test]
fn references_mismatch_and_stale_assessments_fail_before_candidate_escape() {
    let source = fixture();
    let target_document = document(DOCUMENT_TARGET_EMPTY, TEMPLATE_ID);
    let before = encode_template(&source).expect("reference scan source must encode");
    let reference_error = assess_template_references(
        &source,
        complete_snapshot(std::slice::from_ref(&target_document)),
    )
    .expect_err("a reference must not produce a tombstone proof");
    assert_eq!(
        reference_error.category(),
        TemplateMutationErrorCategory::TemplateHasDocuments
    );
    assert_eq!(reference_error.template_id(), Some(source.template_id()));
    assert_eq!(reference_error.field_id(), None);
    assert_eq!(reference_error.option_id(), None);
    assert_eq!(reference_error.validation_category(), None);
    assert!(reference_error.source().is_none());
    assert_eq!(reference_error.reference_count(), Some(1));
    assert!(!reference_error.reference_count_truncated());
    assert_error_redacted(
        &reference_error,
        &[DOCUMENT_TARGET_EMPTY, target_document.name()],
    );
    assert_eq!(
        encode_template(&source).expect("reference scan must preserve source"),
        before
    );

    let other = other_template();
    let mismatch = assert_template_error(
        &source,
        LATER_AT,
        TemplateMutationCommand::tombstone_template(assessment(&other, &[])),
        TemplateMutationErrorCategory::ReferenceAssessmentMismatch,
    );
    assert_eq!(mismatch.reference_count(), None);
    assert!(!format!("{mismatch:?}\n{mismatch}").contains(OTHER_TEMPLATE_ID));

    let stale_proof = assessment(&source, &[]);
    let renamed = changed(
        &source,
        LATER_AT,
        TemplateMutationCommand::set_template_name("new source revision".to_owned()),
    );
    let stale = assert_template_error(
        &renamed,
        AFTER_TOMBSTONE,
        TemplateMutationCommand::tombstone_template(stale_proof),
        TemplateMutationErrorCategory::ReferenceAssessmentStale,
    );
    assert_eq!(stale.reference_count(), None);
    assert_error_redacted(&stale, &[]);
}

#[test]
fn tombstone_obeys_stale_timestamp_overflow_and_repeat_precedence() {
    let source = fixture();
    let stale = apply_template_mutation(
        &source,
        revision(1),
        LATER_AT,
        TemplateMutationCommand::tombstone_template(assessment(&source, &[])),
    )
    .expect_err("stale expected revision must precede assessment use");
    assert_eq!(
        stale.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    assert_eq!(stale.template_id(), None);

    for (timestamp, category) in [
        (
            "2026-09-03T02:03:04.004Z",
            TemplateMutationErrorCategory::TimestampRegression,
        ),
        (
            "2026-09-03T03:04:05Z",
            TemplateMutationErrorCategory::InvalidTimestamp,
        ),
    ] {
        let error = run(
            &source,
            timestamp,
            TemplateMutationCommand::tombstone_template(assessment(&source, &[])),
        )
        .expect_err("invalid tombstone timestamp must fail");
        assert_eq!(error.category(), category);
        assert_eq!(error.template_id(), None);
    }

    let mut max_value = fixture_value();
    max_value["revision"] = json!(u32::MAX);
    let max_source = fixture_from_value(&max_value);
    let max_before = encode_template(&max_source).expect("MAX revision source must encode");
    let overflow = run(
        &max_source,
        LATER_AT,
        TemplateMutationCommand::tombstone_template(assessment(&max_source, &[])),
    )
    .expect_err("tombstone at MAX revision must fail atomically");
    assert_eq!(
        overflow.category(),
        TemplateMutationErrorCategory::RevisionOverflow
    );
    assert_eq!(
        encode_template(&max_source).expect("overflow must preserve source"),
        max_before
    );

    let deleted = tombstoned_fixture();
    let before = encode_template(&deleted).expect("tombstoned source must encode");
    let repeated = assert_template_error(
        &deleted,
        AFTER_TOMBSTONE,
        TemplateMutationCommand::tombstone_template(assessment(&deleted, &[])),
        TemplateMutationErrorCategory::TemplateIsTombstoned,
    );
    assert_error_redacted(&repeated, &[]);

    let stale_repeat = apply_template_mutation(
        &deleted,
        revision(2),
        AFTER_TOMBSTONE,
        TemplateMutationCommand::tombstone_template(assessment(&deleted, &[])),
    )
    .expect_err("stale revision must precede repeated-tombstone state");
    assert_eq!(
        stale_repeat.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    assert_eq!(
        encode_template(&deleted).expect("repeat attempts must preserve source"),
        before
    );
}

#[test]
fn invalid_tombstoned_source_precedes_the_tombstone_command_gate() {
    let mut source = tombstoned_fixture();
    source.corrupt_field_order_for_test();
    let source_before = source.clone();
    let bytes_before = raw_snapshot_bytes(&source);
    let result = run(
        &source,
        AFTER_TOMBSTONE,
        TemplateMutationCommand::set_template_name(format!("changed {DOCUMENT_SECRET}")),
    );
    let error = result.expect_err("invalid tombstoned source must fail admission");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::InvalidSource
    );
    assert_ne!(
        error.category(),
        TemplateMutationErrorCategory::TemplateIsTombstoned
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_eq!(error.validation_scalar_category(), None);
    assert_eq!(error.validation_scalar_location(), None);
    assert_eq!(error.template_id(), None);
    assert_eq!(error.reference_count(), None);
    assert!(error.source().is_none());
    assert_error_redacted(&error, &[]);
    assert_eq!(source, source_before);
    assert_eq!(raw_snapshot_bytes(&source), bytes_before);
}

#[test]
fn tombstoned_template_blocks_every_metadata_field_and_option_command_family() {
    let source = tombstoned_fixture();
    let before = encode_template(&source).expect("tombstoned source must encode");
    let new_field = id(FIELD_TOMBSTONE_NEW);
    let new_option = option_id(OPTION_TOMBSTONE_NEW);
    let mut reversed_fields = source.field_order().to_vec();
    reversed_fields.reverse();
    let commands = vec![
        TemplateMutationCommand::set_template_name("blocked metadata".to_owned()),
        TemplateMutationCommand::set_template_presentation_token(Some(
            "blocked presentation".to_owned(),
        )),
        TemplateMutationCommand::create_field(
            NewFieldDraft::new(
                new_field,
                "blocked Field".to_owned(),
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                false,
                None,
                FieldValueDraft::unset(),
            ),
            NewFieldInsertion::Append,
        ),
        TemplateMutationCommand::set_field_label(id(FIELD_TEXT), "blocked".to_owned()),
        TemplateMutationCommand::set_field_required(id(FIELD_TEXT), true),
        TemplateMutationCommand::set_field_presentation_token(
            id(FIELD_TEXT),
            Some("blocked".to_owned()),
        ),
        TemplateMutationCommand::set_current_default(id(FIELD_TEXT), FieldValueDraft::unset()),
        TemplateMutationCommand::reorder_fields(reversed_fields),
        TemplateMutationCommand::archive_field(id(FIELD_TEXT)),
        TemplateMutationCommand::add_option(
            id(FIELD_CHOICE_A),
            NewChoiceOptionDraft::new(new_option, "blocked".to_owned()),
            NewOptionInsertion::Append,
        ),
        TemplateMutationCommand::rename_option(
            id(FIELD_CHOICE_A),
            option_id(OPTION_ACTIVE_A),
            "blocked".to_owned(),
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
        TemplateMutationCommand::tombstone_template(assessment(&source, &[])),
    ];

    for command in commands {
        let error = assert_template_error(
            &source,
            AFTER_TOMBSTONE,
            command,
            TemplateMutationErrorCategory::TemplateIsTombstoned,
        );
        assert_error_redacted(
            &error,
            &[
                "blocked metadata",
                "blocked presentation",
                "blocked Field",
                "blocked",
            ],
        );
    }
    assert_eq!(
        encode_template(&source).expect("blocked commands must preserve source"),
        before
    );
}
