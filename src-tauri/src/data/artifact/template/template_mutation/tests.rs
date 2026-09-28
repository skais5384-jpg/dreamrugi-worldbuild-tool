use std::{error::Error, str::FromStr};

use serde_json::{json, Value};

use super::*;
use crate::data::{
    artifact::{decode_template, encode_template, ArtifactType, FieldValue, TemplateId},
    json::{to_deterministic_json_bytes, MAX_JSON_NESTING_DEPTH},
    schema::SchemaVersion,
};

const TEMPLATE_ID: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_TEMPLATE_ID: &str = "12121212-1212-4212-8212-121212121212";
const FIELD_TEXT: &str = "22222222-2222-4222-8222-222222222222";
const FIELD_CHOICE_A: &str = "33333333-3333-4333-8333-333333333333";
const OPTION_ACTIVE_A: &str = "44444444-4444-4444-8444-444444444444";
const OPTION_ARCHIVED_A: &str = "55555555-5555-4555-8555-555555555555";
const OPTION_ACTIVE_B1: &str = "66666666-6666-4666-8666-666666666666";
const OPTION_ACTIVE_B2: &str = "67676767-6767-4767-8767-676767676767";
const FIELD_CHOICE_B: &str = "88888888-8888-4888-8888-888888888888";
const FIELD_RICH_TEXT: &str = "99999999-9999-4999-8999-999999999999";
const FIELD_ARCHIVED: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const FIELD_NEW: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const SECRET: &str =
    "credential=mutation-secret path=C:\\private\\template.json /home/private/template.json";
const CREATED_AT: &str = "2026-09-03T01:02:03.004Z";
const UPDATED_AT: &str = "2026-09-03T02:03:04.005Z";
const LATER_AT: &str = "2026-09-03T03:04:05.006Z";
const ARBITRARY_NUMBER: &str = "12345678901234567890123456789012345678901234567890";
const OTHER_ARBITRARY_NUMBER: &str = "12345678901234567890123456789012345678901234567890e+0";
const METADATA_SECRET: &str =
    "credential=isolated-metadata path=C:\\private\\isolated.json /home/private/isolated.json";
const CHANGED_METADATA_SECRET: &str = "credential=changed-isolated-metadata";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RichMetadataLayer {
    Outer,
    Envelope,
    Root,
    Container,
    Text,
    NestedObjectArray,
    ArbitraryNumber,
}

impl RichMetadataLayer {
    const ALL: [Self; 7] = [
        Self::Outer,
        Self::Envelope,
        Self::Root,
        Self::Container,
        Self::Text,
        Self::NestedObjectArray,
        Self::ArbitraryNumber,
    ];

    const fn key(self) -> &'static str {
        match self {
            Self::Outer => "futureOuterOnly",
            Self::Envelope => "futureEnvelopeOnly",
            Self::Root => "futureRootOnly",
            Self::Container => "futureContainerOnly",
            Self::Text => "futureTextOnly",
            Self::NestedObjectArray => "futureNestedOnly",
            Self::ArbitraryNumber => "futureNumberOnly",
        }
    }
}

fn id(value: &str) -> FieldId {
    FieldId::from_str(value).expect("fixture FieldId must be valid")
}

fn option_id(value: &str) -> OptionId {
    OptionId::from_str(value).expect("fixture OptionId must be valid")
}

fn fixture_value() -> Value {
    json!({
        "artifactType": "template",
        "createdAtUtc": CREATED_AT,
        "fieldOrder": [FIELD_TEXT, FIELD_CHOICE_A, FIELD_CHOICE_B, FIELD_RICH_TEXT],
        "fields": {
            (FIELD_TEXT): {
                "configuration": { "kind": "singleLineText", "futureConfig": "preserved" },
                "defaultValue": { "kind": "text", "value": SECRET, "futureValue": "preserved" },
                "initialDefaultValue": { "kind": "unset", "futureInitial": "preserved" },
                "introducedRevision": 1,
                "kind": "singleLineText",
                "label": SECRET,
                "lifecycle": "active",
                "presentation": { "futureFieldPresentation": "preserved" },
                "required": false,
                "futureField": "preserved"
            },
            (FIELD_CHOICE_A): {
                "configuration": {
                    "kind": "singleChoice",
                    "optionOrder": [OPTION_ACTIVE_A],
                    "options": {
                        (OPTION_ACTIVE_A): {
                            "label": "active option",
                            "lifecycle": "active",
                            "futureOption": "preserved"
                        },
                        (OPTION_ARCHIVED_A): {
                            "label": SECRET,
                            "lifecycle": "archived"
                        }
                    }
                },
                "defaultValue": { "kind": "singleChoice", "optionId": OPTION_ACTIVE_A },
                "initialDefaultValue": {
                    "kind": "singleChoice",
                    "optionId": OPTION_ARCHIVED_A
                },
                "introducedRevision": 1,
                "kind": "singleChoice",
                "label": "choice A",
                "lifecycle": "active",
                "presentation": {},
                "required": false
            },
            (FIELD_CHOICE_B): {
                "configuration": {
                    "kind": "multiChoice",
                    "optionOrder": [OPTION_ACTIVE_B2, OPTION_ACTIVE_B1],
                    "options": {
                        (OPTION_ACTIVE_B1): { "label": "B1", "lifecycle": "active" },
                        (OPTION_ACTIVE_B2): { "label": "B2", "lifecycle": "active" }
                    }
                },
                "defaultValue": {
                    "kind": "multiChoice",
                    "optionIds": [OPTION_ACTIVE_B1, OPTION_ACTIVE_B2]
                },
                "initialDefaultValue": { "kind": "unset" },
                "introducedRevision": 2,
                "kind": "multiChoice",
                "label": "choice B",
                "lifecycle": "active",
                "presentation": {},
                "required": false
            },
            (FIELD_RICH_TEXT): {
                "configuration": { "kind": "richText" },
                "defaultValue": {
                    "kind": "richText",
                    "document": {
                        "schemaVersion": 1,
                        "content": {
                            "kind": "root",
                            "children": [{
                                "kind": "paragraph",
                                "children": [{ "kind": "text", "text": SECRET }]
                            }]
                        }
                    }
                },
                "initialDefaultValue": { "kind": "unset" },
                "introducedRevision": 2,
                "kind": "richText",
                "label": "rich text",
                "lifecycle": "active",
                "presentation": {},
                "required": false
            },
            (FIELD_ARCHIVED): {
                "configuration": { "kind": "singleLineText" },
                "defaultValue": { "kind": "unset" },
                "initialDefaultValue": { "kind": "unset" },
                "introducedRevision": 1,
                "kind": "singleLineText",
                "label": "archived field",
                "lifecycle": "archived",
                "presentation": {},
                "required": false
            }
        },
        "lifecycle": "active",
        "name": SECRET,
        "presentation": { "token": "character", "futurePresentation": "preserved" },
        "revision": 2,
        "schemaVersion": 1,
        "templateId": TEMPLATE_ID,
        "updatedAtUtc": UPDATED_AT,
        "futureRoot": { "unknownExtraSentinel": SECRET }
    })
}

fn metadata_isolated_fixture_value() -> Value {
    let mut value = fixture_value();
    value
        .as_object_mut()
        .expect("Template fixture must be an object")
        .remove("futureRoot");
    value["presentation"]
        .as_object_mut()
        .expect("Template presentation must be an object")
        .remove("futurePresentation");

    let text = &mut value["fields"][FIELD_TEXT];
    text.as_object_mut()
        .expect("text Field must be an object")
        .remove("futureField");
    text["configuration"]
        .as_object_mut()
        .expect("text configuration must be an object")
        .remove("futureConfig");
    text["defaultValue"]
        .as_object_mut()
        .expect("text default must be an object")
        .remove("futureValue");
    text["initialDefaultValue"]
        .as_object_mut()
        .expect("text initial default must be an object")
        .remove("futureInitial");
    text["presentation"]
        .as_object_mut()
        .expect("text presentation must be an object")
        .remove("futureFieldPresentation");
    value["fields"][FIELD_CHOICE_A]["configuration"]["options"][OPTION_ACTIVE_A]
        .as_object_mut()
        .expect("choice Option must be an object")
        .remove("futureOption");
    value
}

fn fixture() -> TemplateArtifact {
    fixture_from_value(&fixture_value())
}

fn fixture_from_value(value: &Value) -> TemplateArtifact {
    decode_template(&to_deterministic_json_bytes(value).expect("fixture JSON must encode"))
        .expect("fixture Template must decode")
}

fn revision(value: u32) -> TemplateRevision {
    TemplateRevision::try_from(value).expect("test revision must be positive")
}

fn raw_snapshot_bytes(template: &TemplateArtifact) -> Vec<u8> {
    let wire = super::super::TemplateWire::from(template);
    to_deterministic_json_bytes(&wire).expect("test-only wire snapshot must encode")
}

fn template_wire_value(template: &TemplateArtifact) -> Value {
    serde_json::to_value(super::super::TemplateWire::from(template))
        .expect("test-only Template wire must materialize")
}

fn maximum_container_depth(value: &Value) -> usize {
    fn visit(value: &Value, parent_depth: usize) -> usize {
        match value {
            Value::Object(object) => {
                let depth = parent_depth + 1;
                object
                    .values()
                    .map(|child| visit(child, depth))
                    .max()
                    .unwrap_or(depth)
                    .max(depth)
            }
            Value::Array(items) => {
                let depth = parent_depth + 1;
                items
                    .iter()
                    .map(|child| visit(child, depth))
                    .max()
                    .unwrap_or(depth)
                    .max(depth)
            }
            _ => parent_depth,
        }
    }

    visit(value, 0)
}

fn nested_blockquote_content(levels: usize, with_marks: bool) -> serde_json::Map<String, Value> {
    let text = if with_marks {
        json!({"kind": "text", "text": "depth boundary", "marks": ["bold"]})
    } else {
        json!({"kind": "text", "text": "depth boundary"})
    };
    let mut block = json!({
        "kind": "paragraph",
        "children": [text]
    });
    for _ in 0..levels {
        block = json!({
            "kind": "blockquote",
            "children": [block]
        });
    }
    json!({
        "kind": "root",
        "children": [block]
    })
    .as_object()
    .expect("rich-text root must be an object")
    .clone()
}

/// wrapper 수를 가정하지 않고 실제 private Template wire를 측정해 경계 content를 찾는다.
fn rich_text_content_at_wire_depth(
    source: &TemplateArtifact,
    target_depth: usize,
) -> serde_json::Map<String, Value> {
    let rich = id(FIELD_RICH_TEXT);
    for levels in 0..=MAX_JSON_NESTING_DEPTH {
        for with_marks in [false, true] {
            let content = nested_blockquote_content(levels, with_marks);
            let mut probe = source.clone();
            probe.replace_rich_text_default_content_for_test(rich, content.clone(), false);
            if maximum_container_depth(&template_wire_value(&probe)) == target_depth {
                return content;
            }
        }
    }
    panic!("test fixture could not reach Template wire depth {target_depth}")
}

fn current_default_from(value: &Value, field_id: FieldId) -> FieldValue {
    fixture_from_value(value).fields[&field_id]
        .default_value
        .clone()
}

fn rich_metadata_owner_mut(
    value: &mut Value,
    layer: RichMetadataLayer,
) -> &mut serde_json::Map<String, Value> {
    let default = &mut value["fields"][FIELD_RICH_TEXT]["defaultValue"];
    let owner = match layer {
        RichMetadataLayer::Outer => default,
        RichMetadataLayer::Envelope => &mut default["document"],
        RichMetadataLayer::Root => &mut default["document"]["content"],
        RichMetadataLayer::Container | RichMetadataLayer::NestedObjectArray => {
            &mut default["document"]["content"]["children"][0]
        }
        RichMetadataLayer::Text | RichMetadataLayer::ArbitraryNumber => {
            &mut default["document"]["content"]["children"][0]["children"][0]
        }
    };
    owner
        .as_object_mut()
        .expect("rich-text metadata owner must be an object")
}

fn rich_metadata_value(layer: RichMetadataLayer, changed: bool) -> Value {
    match layer {
        RichMetadataLayer::NestedObjectArray => json!({
            "credential": if changed { CHANGED_METADATA_SECRET } else { METADATA_SECRET },
            "nested": [{"array": [1, {"keep": !changed}]}]
        }),
        RichMetadataLayer::ArbitraryNumber => serde_json::from_str(if changed {
            OTHER_ARBITRARY_NUMBER
        } else {
            ARBITRARY_NUMBER
        })
        .expect("arbitrary-precision metadata number must parse"),
        RichMetadataLayer::Outer
        | RichMetadataLayer::Envelope
        | RichMetadataLayer::Root
        | RichMetadataLayer::Container
        | RichMetadataLayer::Text => Value::String(
            if changed {
                CHANGED_METADATA_SECRET
            } else {
                METADATA_SECRET
            }
            .to_owned(),
        ),
    }
}

fn set_rich_metadata(value: &mut Value, layer: RichMetadataLayer, changed: bool) {
    rich_metadata_owner_mut(value, layer)
        .insert(layer.key().to_owned(), rich_metadata_value(layer, changed));
}

fn remove_rich_metadata(value: &mut Value, layer: RichMetadataLayer) {
    assert!(
        rich_metadata_owner_mut(value, layer)
            .remove(layer.key())
            .is_some(),
        "target metadata must exist before deletion"
    );
}

fn rich_unknown_occurrences(value: &Value) -> Vec<String> {
    fn unknown_keys(
        object: &serde_json::Map<String, Value>,
        known: &[&str],
        location: &str,
        output: &mut Vec<String>,
    ) {
        output.extend(
            object
                .keys()
                .filter(|key| !known.contains(&key.as_str()))
                .map(|key| format!("{location}:{key}")),
        );
    }

    let default = value["fields"][FIELD_RICH_TEXT]["defaultValue"]
        .as_object()
        .expect("rich-text FieldValue must be an object");
    let document = default["document"]
        .as_object()
        .expect("rich-text document must be an object");
    let content = document["content"]
        .as_object()
        .expect("rich-text content must be an object");
    let mut output = Vec::new();
    unknown_keys(default, &["kind", "document"], "outer", &mut output);
    unknown_keys(
        document,
        &["schemaVersion", "content"],
        "envelope",
        &mut output,
    );

    let mut pending = vec![("root".to_owned(), content)];
    while let Some((location, node)) = pending.pop() {
        let kind = node["kind"]
            .as_str()
            .expect("isolated rich-text node must have a string kind");
        let known = match kind {
            "text" => &["kind", "text", "marks"][..],
            "heading" => &["kind", "children", "level"][..],
            "taskItem" => &["kind", "children", "checked"][..],
            "hardBreak" => &["kind"][..],
            _ => &["kind", "children"][..],
        };
        unknown_keys(node, known, &location, &mut output);
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for (index, child) in children.iter().enumerate() {
                pending.push((
                    format!("{location}/{index}"),
                    child
                        .as_object()
                        .expect("isolated rich-text child must be an object"),
                ));
            }
        }
    }
    output.sort();
    output
}

fn assert_only_rich_metadata(value: &Value, layer: RichMetadataLayer) {
    let occurrences = rich_unknown_occurrences(value);
    assert_eq!(
        occurrences.len(),
        1,
        "{layer:?} fixture must contain exactly one unknown metadata occurrence: {occurrences:?}"
    );
    assert!(occurrences[0].ends_with(layer.key()));
    if layer != RichMetadataLayer::Outer {
        assert!(!occurrences[0].starts_with("outer:"));
    }
    if !matches!(
        layer,
        RichMetadataLayer::Outer | RichMetadataLayer::Envelope
    ) {
        assert!(!occurrences[0].starts_with("envelope:"));
    }
}

fn assert_no_rich_metadata(value: &Value) {
    assert!(
        rich_unknown_occurrences(value).is_empty(),
        "metadata-free fixture must have no rich-text unknown extras"
    );
}

fn assert_only_target_rich_metadata_differs(
    source: &Value,
    candidate: &Value,
    layer: RichMetadataLayer,
) {
    let mut source_without_target = source.clone();
    let mut candidate_without_target = candidate.clone();
    let source_target =
        rich_metadata_owner_mut(&mut source_without_target, layer).remove(layer.key());
    let candidate_target =
        rich_metadata_owner_mut(&mut candidate_without_target, layer).remove(layer.key());
    assert_ne!(source_target, candidate_target);
    assert_eq!(
        source_without_target["fields"][FIELD_RICH_TEXT]["defaultValue"],
        candidate_without_target["fields"][FIELD_RICH_TEXT]["defaultValue"],
        "known rich-text payload and every non-target metadata location must remain identical"
    );
}

fn assert_rich_non_metadata_invariants(source: &TemplateArtifact, candidate: &TemplateArtifact) {
    let rich = id(FIELD_RICH_TEXT);
    let source_field = &source.fields[&rich];
    let candidate_field = &candidate.fields[&rich];
    assert_eq!(candidate_field.kind, source_field.kind);
    assert_eq!(candidate_field.lifecycle, source_field.lifecycle);
    assert_eq!(
        candidate_field.introduced_revision,
        source_field.introduced_revision
    );
    assert_eq!(
        candidate_field.initial_default_value,
        source_field.initial_default_value
    );
    assert_eq!(candidate_field.label, source_field.label);
    assert_eq!(candidate_field.required, source_field.required);
    assert_eq!(candidate_field.configuration, source_field.configuration);
    assert_eq!(candidate_field.presentation, source_field.presentation);
}

fn render_mutation_error_chain(error: &TemplateMutationError) -> String {
    let mut rendered = format!("{error:?}\n{error}");
    let mut current = error.source();
    while let Some(source) = current {
        rendered.push_str(&format!("\n{source:?}\n{source}"));
        current = source.source();
    }
    rendered
}

fn assert_rich_metadata_error_and_unchanged(
    source: &TemplateArtifact,
    before: &[u8],
    result: Result<TemplateMutationOutcome, TemplateMutationError>,
) {
    let rich = id(FIELD_RICH_TEXT);
    let error = assert_error(
        result,
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(rich),
        None,
    );
    assert_eq!(error.validation_category(), None);
    let rendered = render_mutation_error_chain(&error);
    for secret in [
        METADATA_SECRET,
        CHANGED_METADATA_SECRET,
        ARBITRARY_NUMBER,
        OTHER_ARBITRARY_NUMBER,
        "isolated.json",
    ] {
        assert!(!rendered.contains(secret));
    }
    assert_eq!(
        encode_template(source).expect("metadata source must remain encodable"),
        before
    );
}

fn rich_text_unknown_fixture_value() -> Value {
    let mut value = fixture_value();
    let number: Value = serde_json::from_str(ARBITRARY_NUMBER)
        .expect("arbitrary-precision fixture number must parse");
    value["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureValueOuter"] =
        json!({"keep": ["outer", 1]});
    value["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"] = json!({
        "schemaVersion": 1,
        "futureEnvelope": {"credential": SECRET},
        "content": {
            "kind": "root",
            "futureRoot": {"path": "C:\\private\\raw-ast.json"},
            "children": [
                {
                    "kind": "paragraph",
                    "futureParagraph": {"nested": [{"keep": true}]},
                    "children": [{
                        "kind": "text",
                        "text": "first",
                        "futureText": {"occurrence": [1, 2, 3]}
                    }]
                },
                {
                    "kind": "bulletList",
                    "children": [{
                        "kind": "listItem",
                        "futureListItem": {"number": number},
                        "children": [{
                            "kind": "paragraph",
                            "children": [{"kind": "text", "text": "second"}]
                        }]
                    }]
                }
            ]
        }
    });
    value
}

fn archived_field_fixture() -> TemplateArtifact {
    let mut value = metadata_isolated_fixture_value();
    value["fields"][FIELD_ARCHIVED]["defaultValue"] =
        json!({"kind": "text", "value": "last archived current"});
    value["fields"][FIELD_ARCHIVED]["initialDefaultValue"] =
        json!({"kind": "text", "value": "archived initial"});
    fixture_from_value(&value)
}

fn archived_choice_field_fixture() -> TemplateArtifact {
    let mut value = metadata_isolated_fixture_value();
    value["fields"][FIELD_CHOICE_B]["lifecycle"] = json!("archived");
    value["fieldOrder"]
        .as_array_mut()
        .expect("fieldOrder must be an array")
        .retain(|field_id| field_id != FIELD_CHOICE_B);
    fixture_from_value(&value)
}

fn assert_only_archived_field_member_changed(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    field_id: FieldId,
    member: &str,
) {
    let mut source_field = template_wire_value(source)["fields"][field_id.to_string()].clone();
    let mut candidate_field =
        template_wire_value(candidate)["fields"][field_id.to_string()].clone();
    let source_member = source_field
        .as_object_mut()
        .expect("source Field wire must be an object")
        .remove(member)
        .expect("source target member must exist");
    let candidate_member = candidate_field
        .as_object_mut()
        .expect("candidate Field wire must be an object")
        .remove(member)
        .expect("candidate target member must exist");
    assert_ne!(source_member, candidate_member);
    assert_eq!(
        source_field, candidate_field,
        "only archived Field member {member} may differ"
    );
}

fn assert_archived_base_invariants(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    field_id: FieldId,
) {
    let source_field = &source.fields[&field_id];
    let candidate_field = &candidate.fields[&field_id];
    assert_eq!(source_field.kind, candidate_field.kind);
    assert_eq!(source_field.lifecycle, FieldLifecycle::Archived);
    assert_eq!(candidate_field.lifecycle, FieldLifecycle::Archived);
    assert_eq!(
        source_field.introduced_revision,
        candidate_field.introduced_revision
    );
    assert_eq!(
        source_field.initial_default_value,
        candidate_field.initial_default_value
    );
    assert_eq!(source_field.extra, candidate_field.extra);
}

fn assert_archived_history_error_and_source_unchanged(
    source: &TemplateArtifact,
    candidate: &TemplateArtifact,
    field_id: FieldId,
    before: &[u8],
) {
    candidate
        .validate_storage()
        .expect("target-only archived candidate must be storage-valid");
    let error = validate_historical_invariants(source, candidate, false)
        .expect_err("archived persisted definition change must fail history validation");
    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::ImmutableFieldChanged
    );
    assert_eq!(error.field_id(), Some(field_id));
    assert_eq!(error.option_id(), None);
    assert_eq!(error.validation_category(), None);
    assert_eq!(
        encode_template(source).expect("rejected archived source must remain encodable"),
        before
    );
}

fn apply(
    source: &TemplateArtifact,
    updated_at_utc: &str,
    operation: TestMutation,
) -> Result<TemplateMutationOutcome, TemplateMutationError> {
    apply_template_mutation(
        source,
        source.revision(),
        updated_at_utc,
        TemplateMutationCommand::test(operation),
    )
}

fn assert_error(
    result: Result<TemplateMutationOutcome, TemplateMutationError>,
    category: TemplateMutationErrorCategory,
    field_id: Option<FieldId>,
    option_id: Option<OptionId>,
) -> TemplateMutationError {
    let error = result.expect_err("mutation must fail");
    assert_eq!(error.category(), category);
    assert_eq!(error.field_id(), field_id);
    assert_eq!(error.option_id(), option_id);
    error
}

#[test]
fn changed_mutation_increments_once_and_preserves_identity_extras_and_source() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");

    let outcome = apply(
        &source,
        UPDATED_AT,
        TestMutation::RenameAndPresentation {
            name: "renamed".to_owned(),
            token: "renamed-token".to_owned(),
        },
    )
    .expect("valid mutation must succeed");
    let changed = outcome.changed().expect("mutation must be changed");

    assert_eq!(changed.revision(), revision(3));
    assert_eq!(changed.created_at_utc(), CREATED_AT);
    assert_eq!(changed.updated_at_utc(), UPDATED_AT);
    assert_eq!(changed.template_id(), source.template_id());
    assert_eq!(
        encode_template(&source).expect("source must still encode"),
        before
    );

    let encoded = encode_template(changed).expect("validated candidate must encode");
    assert_eq!(
        decode_template(&encoded).expect("candidate must decode"),
        *changed
    );
    let encoded_text = String::from_utf8(encoded).expect("JSON must be UTF-8");
    assert!(encoded_text.contains("unknownExtraSentinel"));
    assert!(encoded_text.contains("futureFieldPresentation"));
    assert!(encoded_text.contains("futureValue"));
}

#[test]
fn glossary_exclusion_defaults_false_and_mutates_without_schema_transition() {
    let source = fixture();
    assert!(!source.glossary_excluded());
    let schema = source.schema_version();
    let outcome = apply_template_mutation(
        &source,
        source.revision(),
        UPDATED_AT,
        TemplateMutationCommand::set_glossary_excluded(true),
    )
    .expect("glossary exclusion mutation");
    let changed = outcome.changed().expect("exclusion must change");
    assert!(changed.glossary_excluded());
    assert_eq!(changed.schema_version(), schema);
    let encoded = String::from_utf8(encode_template(changed).expect("encode")).expect("utf8 JSON");
    let encoded: Value = serde_json::from_str(&encoded).expect("encoded JSON");
    assert_eq!(encoded["glossaryExcluded"], json!(true));
}

#[test]
fn later_timestamp_is_applied_without_clock_or_locale_dependency() {
    let source = fixture();
    let changed = apply(&source, LATER_AT, TestMutation::Rename("later".to_owned()))
        .expect("later timestamp must succeed")
        .into_changed()
        .expect("rename must change the Template");
    assert_eq!(changed.updated_at_utc(), LATER_AT);
    assert_eq!(changed.revision(), revision(3));
}

#[test]
fn semantic_noop_keeps_revision_timestamp_and_canonical_bytes() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let outcome = apply(
        &source,
        LATER_AT,
        TestMutation::Rename(source.name().to_owned()),
    )
    .expect("same value must be a no-op");

    assert!(outcome.is_unchanged());
    assert!(outcome.changed().is_none());
    assert_eq!(source.revision(), revision(2));
    assert_eq!(source.updated_at_utc(), UPDATED_AT);
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
}

#[test]
fn stale_revision_rejects_noop_and_actual_change_before_candidate_creation() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let error = apply_template_mutation(
        &source,
        revision(1),
        LATER_AT,
        TemplateMutationCommand::test(TestMutation::Noop),
    )
    .expect_err("stale no-op must fail");

    assert_eq!(
        error.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    let changed_error = apply_template_mutation(
        &source,
        revision(1),
        LATER_AT,
        TemplateMutationCommand::test(TestMutation::Rename("stale change".to_owned())),
    )
    .expect_err("stale changed command must fail");
    assert_eq!(
        changed_error.category(),
        TemplateMutationErrorCategory::RevisionMismatch
    );
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
}

#[test]
fn maximum_revision_allows_certain_noop_but_rejects_change_atomically() {
    let mut source = fixture();
    source.revision = revision(u32::MAX);
    let before = encode_template(&source).expect("MAX revision source must encode");

    assert!(apply(&source, LATER_AT, TestMutation::Noop)
        .expect("certain no-op at MAX must succeed")
        .is_unchanged());
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::Rename("changed".to_owned()),
        ),
        TemplateMutationErrorCategory::RevisionOverflow,
        None,
        None,
    );
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
}

#[test]
fn source_admission_and_command_failure_do_not_expose_partial_candidate() {
    let mut invalid_source = fixture();
    invalid_source.corrupt_field_order_for_test();
    let invalid_before = raw_snapshot_bytes(&invalid_source);
    let error = assert_error(
        apply(
            &invalid_source,
            LATER_AT,
            TestMutation::Rename("must not apply".to_owned()),
        ),
        TemplateMutationErrorCategory::InvalidSource,
        None,
        None,
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_eq!(raw_snapshot_bytes(&invalid_source), invalid_before);

    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::RenameThenReject("partial-secret".to_owned()),
        ),
        TemplateMutationErrorCategory::UnexpectedMutationState,
        None,
        None,
    );
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
}

#[test]
fn timestamps_reject_malformed_and_regressing_values_without_source_changes() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    for malformed in [
        "2026-09-03T03:04:05Z",
        "2026-09-03T03:04:05.006+00:00",
        "not-a-timestamp",
    ] {
        assert_error(
            apply(&source, malformed, TestMutation::Noop),
            TemplateMutationErrorCategory::InvalidTimestamp,
            None,
            None,
        );
    }
    assert_error(
        apply(&source, "2026-09-03T02:03:04.004Z", TestMutation::Noop),
        TemplateMutationErrorCategory::TimestampRegression,
        None,
        None,
    );
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
}

#[test]
fn immutable_template_identity_and_engine_metadata_fail_closed() {
    let source = fixture();
    for operation in [
        TestMutation::ChangeTemplateId(
            TemplateId::from_str(OTHER_TEMPLATE_ID).expect("fixture ID must parse"),
        ),
        TestMutation::ChangeSchemaVersion(
            SchemaVersion::try_from(2).expect("fixture schema version must be valid"),
        ),
        TestMutation::ChangeArtifactType(ArtifactType::Document),
        TestMutation::ChangeCreatedAt("2026-09-03T00:00:00.000Z".to_owned()),
    ] {
        assert_error(
            apply(&source, LATER_AT, operation),
            TemplateMutationErrorCategory::ImmutableTemplateIdentityChanged,
            None,
            None,
        );
    }
    for operation in [
        TestMutation::ChangeRevision(revision(8)),
        TestMutation::ChangeUpdatedAt(LATER_AT.to_owned()),
        TestMutation::ChangeRootExtra,
    ] {
        assert_error(
            apply(&source, LATER_AT, operation),
            TemplateMutationErrorCategory::UnexpectedMutationState,
            None,
            None,
        );
    }
}

#[test]
fn persisted_field_removal_kind_revision_and_initial_default_are_rejected() {
    let source = fixture();
    let text = id(FIELD_TEXT);
    let cases = [
        (
            TestMutation::RemoveField(text),
            TemplateMutationErrorCategory::FieldRemoved,
        ),
        (
            TestMutation::ChangeFieldKind {
                field_id: text,
                kind: super::super::super::FieldKind::Number,
            },
            TemplateMutationErrorCategory::ImmutableFieldChanged,
        ),
        (
            TestMutation::ChangeIntroducedRevision {
                field_id: text,
                revision: revision(2),
            },
            TemplateMutationErrorCategory::ImmutableFieldChanged,
        ),
        (
            TestMutation::ReplaceInitialWithCurrent(text),
            TemplateMutationErrorCategory::ImmutableInitialDefaultChanged,
        ),
    ];
    for (operation, category) in cases {
        assert_error(
            apply(&source, LATER_AT, operation),
            category,
            Some(text),
            None,
        );
    }
}

#[test]
fn persisted_option_removal_move_and_archived_definition_change_are_rejected() {
    let source = fixture();
    let field_a = id(FIELD_CHOICE_A);
    let field_b = id(FIELD_CHOICE_B);
    let active = option_id(OPTION_ACTIVE_A);
    let archived = option_id(OPTION_ARCHIVED_A);

    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::RemoveOption {
                field_id: field_a,
                option_id: active,
            },
        ),
        TemplateMutationErrorCategory::OptionRemoved,
        None,
        Some(active),
    );
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::MoveOption {
                source_field_id: field_a,
                target_field_id: field_b,
                option_id: active,
            },
        ),
        TemplateMutationErrorCategory::OptionOwnerChanged,
        None,
        Some(active),
    );
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::RenameArchivedOption {
                field_id: field_a,
                option_id: archived,
            },
        ),
        TemplateMutationErrorCategory::ImmutableOptionChanged,
        None,
        Some(archived),
    );
}

#[test]
fn archived_field_option_and_tombstoned_template_cannot_reactivate() {
    let source = fixture();
    let archived_field = id(FIELD_ARCHIVED);
    let field_a = id(FIELD_CHOICE_A);
    let archived_option = option_id(OPTION_ARCHIVED_A);

    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::ReactivateField(archived_field),
        ),
        TemplateMutationErrorCategory::LifecycleReactivation,
        Some(archived_field),
        None,
    );
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::ReactivateOption {
                field_id: field_a,
                option_id: archived_option,
            },
        ),
        TemplateMutationErrorCategory::LifecycleReactivation,
        None,
        Some(archived_option),
    );

    let mut deleted = fixture();
    deleted.lifecycle = TemplateLifecycle::Deleted;
    assert_error(
        apply(&deleted, LATER_AT, TestMutation::ReactivateTemplate),
        TemplateMutationErrorCategory::TemplateIsTombstoned,
        None,
        None,
    );
}

#[test]
fn dedicated_restore_reactivates_only_the_template_and_advances_revision() {
    let mut deleted = fixture();
    deleted.lifecycle = TemplateLifecycle::Deleted;
    let original_revision = deleted.revision();
    let archived_field = id(FIELD_ARCHIVED);
    let before = encode_template(&deleted).expect("deleted source must encode");
    let restored = restore_tombstoned_template(&deleted, original_revision, LATER_AT)
        .expect("dedicated restore must accept a deleted template")
        .into_changed()
        .expect("restore must write a new revision");
    assert_eq!(restored.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(
        restored.revision(),
        original_revision.checked_increment().expect("revision")
    );
    assert_eq!(
        restored.fields()[&archived_field].lifecycle(),
        FieldLifecycle::Archived
    );
    assert_eq!(
        encode_template(&deleted).expect("restore must preserve source"),
        before
    );
}

#[test]
fn archived_field_preserves_its_complete_last_persisted_definition() {
    let source = archived_field_fixture();
    let before = encode_template(&source).expect("archived source must encode");
    let archived = id(FIELD_ARCHIVED);
    let active_text = id(FIELD_TEXT);
    let unset = current_default_from(&fixture_value(), archived);
    let replacement_current = source.fields[&active_text].default_value.clone();

    let cases = [
        (
            TestMutation::RenameField {
                field_id: archived,
                label: "changed archived label".to_owned(),
            },
            TemplateMutationErrorCategory::ImmutableFieldChanged,
        ),
        (
            TestMutation::ReplaceCurrentDefault {
                field_id: archived,
                value: replacement_current,
            },
            TemplateMutationErrorCategory::ImmutableFieldChanged,
        ),
        (
            TestMutation::ReplaceCurrentDefault {
                field_id: archived,
                value: unset,
            },
            TemplateMutationErrorCategory::ImmutableFieldChanged,
        ),
        (
            TestMutation::ReplaceInitialWithCurrent(archived),
            TemplateMutationErrorCategory::ImmutableInitialDefaultChanged,
        ),
        (
            TestMutation::ReactivateField(archived),
            TemplateMutationErrorCategory::LifecycleReactivation,
        ),
        (
            TestMutation::ReplaceFieldDefinition {
                field_id: archived,
                source_field_id: active_text,
            },
            TemplateMutationErrorCategory::ImmutableInitialDefaultChanged,
        ),
    ];

    for (operation, category) in cases {
        assert_error(
            apply(&source, LATER_AT, operation),
            category,
            Some(archived),
            None,
        );
        assert_eq!(
            encode_template(&source).expect("failed mutation must not alter source"),
            before
        );
    }
}

#[test]
fn archived_field_whole_equality_rejects_each_independent_mutable_member() {
    let source = archived_field_fixture();
    let archived = id(FIELD_ARCHIVED);
    let before = encode_template(&source).expect("archived source must encode");

    let mut current_value = template_wire_value(&source);
    current_value["fields"][FIELD_ARCHIVED]["defaultValue"]["value"] =
        json!("different valid archived current");
    let current_candidate = fixture_from_value(&current_value);
    assert_archived_base_invariants(&source, &current_candidate, archived);
    assert_eq!(
        source.fields[&archived].initial_default_value,
        current_candidate.fields[&archived].initial_default_value
    );
    assert_eq!(
        template_wire_value(&source)["fields"][FIELD_ARCHIVED]["defaultValue"]
            .as_object()
            .expect("source current default must be an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["kind", "value"]
    );
    assert_only_archived_field_member_changed(
        &source,
        &current_candidate,
        archived,
        "defaultValue",
    );
    assert_archived_history_error_and_source_unchanged(
        &source,
        &current_candidate,
        archived,
        &before,
    );

    let mut required_candidate = source.clone();
    required_candidate
        .fields
        .get_mut(&archived)
        .expect("archived Field must exist")
        .required = true;
    assert_archived_base_invariants(&source, &required_candidate, archived);
    assert_only_archived_field_member_changed(&source, &required_candidate, archived, "required");
    assert_archived_history_error_and_source_unchanged(
        &source,
        &required_candidate,
        archived,
        &before,
    );

    let mut presentation_candidate = source.clone();
    presentation_candidate
        .fields
        .get_mut(&archived)
        .expect("archived Field must exist")
        .presentation
        .token = Some("archived-known-token".to_owned());
    assert_archived_base_invariants(&source, &presentation_candidate, archived);
    assert_only_archived_field_member_changed(
        &source,
        &presentation_candidate,
        archived,
        "presentation",
    );
    assert_archived_history_error_and_source_unchanged(
        &source,
        &presentation_candidate,
        archived,
        &before,
    );

    let mut reinserted_candidate = source.clone();
    let mut reinserted_definition = reinserted_candidate
        .fields
        .remove(&archived)
        .expect("archived Field must exist before reinsert");
    reinserted_definition.default_value = current_candidate.fields[&archived].default_value.clone();
    assert!(
        reinserted_candidate
            .fields
            .insert(archived, reinserted_definition)
            .is_none(),
        "same ID must have been removed before reinsert"
    );
    assert_archived_base_invariants(&source, &reinserted_candidate, archived);
    assert_only_archived_field_member_changed(
        &source,
        &reinserted_candidate,
        archived,
        "defaultValue",
    );
    assert_archived_history_error_and_source_unchanged(
        &source,
        &reinserted_candidate,
        archived,
        &before,
    );
}

#[test]
fn archived_choice_configuration_order_change_reaches_whole_definition_equality() {
    let source = archived_choice_field_fixture();
    let archived = id(FIELD_CHOICE_B);
    let before = encode_template(&source).expect("archived choice source must encode");
    let mut candidate = source.clone();
    let source_field = &source.fields[&archived];
    let source_options = source_field
        .configuration
        .options()
        .expect("source choice options must exist")
        .clone();
    let (candidate_order, candidate_options) =
        test_choice_configuration_mut(&mut candidate, archived);
    candidate_order.reverse();

    assert_ne!(
        source_field.configuration.option_order(),
        Some(candidate_order.as_slice())
    );
    assert_eq!(&source_options, candidate_options);
    assert_eq!(
        source_field
            .configuration
            .option_order()
            .expect("source choice order must exist")
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        candidate_order
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
    );
    assert_archived_base_invariants(&source, &candidate, archived);
    assert_eq!(
        source_field.default_value,
        candidate.fields[&archived].default_value
    );
    assert_eq!(source_field.label, candidate.fields[&archived].label);
    assert_eq!(source_field.required, candidate.fields[&archived].required);
    assert_eq!(
        source_field.presentation,
        candidate.fields[&archived].presentation
    );
    assert_only_archived_field_member_changed(&source, &candidate, archived, "configuration");
    assert_archived_history_error_and_source_unchanged(&source, &candidate, archived, &before);
}

#[test]
fn final_snapshot_allows_atomic_option_archive_with_current_default_change_only() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let field_a = id(FIELD_CHOICE_A);
    let active = option_id(OPTION_ACTIVE_A);

    let invalid = assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::ArchiveOptionOnly {
                field_id: field_a,
                option_id: active,
            },
        ),
        TemplateMutationErrorCategory::InvalidCandidate,
        Some(field_a),
        Some(active),
    );
    assert_eq!(
        invalid.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );

    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::ArchiveOptionAndUnsetCurrent {
            field_id: field_a,
            option_id: active,
            unset_source_field_id: id(FIELD_ARCHIVED),
        },
    )
    .expect("archive and current default change must validate as one final snapshot")
    .into_changed()
    .expect("compound mutation must change the Template");
    let field = &changed.fields[&field_a];
    assert!(field.default_value.is_unset());
    assert_eq!(
        field.configuration.options().expect("choice options")[&active].lifecycle,
        OptionLifecycle::Archived
    );
    assert_eq!(changed.revision(), revision(3));
    assert_eq!(
        encode_template(&source).expect("source must encode"),
        before
    );
    encode_template(&changed).expect("final candidate must encode");
}

#[test]
fn final_candidate_rejects_order_and_template_wide_option_identity_corruption() {
    let source = fixture();
    let field_a = id(FIELD_CHOICE_A);
    let field_b = id(FIELD_CHOICE_B);
    let active = option_id(OPTION_ACTIVE_A);
    let cases = [
        (
            TestMutation::ClearFieldOrder,
            ArtifactValidationErrorCategory::FieldOrderMismatch,
            None,
        ),
        (
            TestMutation::ClearOptionOrder(field_a),
            ArtifactValidationErrorCategory::OptionOrderMismatch,
            Some(field_a),
        ),
        (
            TestMutation::DuplicateOptionInField {
                source_field_id: field_a,
                target_field_id: field_b,
                option_id: active,
            },
            ArtifactValidationErrorCategory::DuplicateOptionId,
            Some(field_b),
        ),
    ];
    for (operation, validation_category, field) in cases {
        let error = assert_error(
            apply(&source, LATER_AT, operation),
            TemplateMutationErrorCategory::InvalidCandidate,
            field,
            None,
        );
        assert_eq!(error.validation_category(), Some(validation_category));
    }
}

#[test]
fn final_candidate_reuses_scalar_choice_and_rich_text_validation() {
    let source = fixture();
    let text = id(FIELD_TEXT);
    let multi = id(FIELD_CHOICE_B);
    let rich = id(FIELD_RICH_TEXT);
    let b1 = option_id(OPTION_ACTIVE_B1);
    let cases = [
        (
            TestMutation::CorruptCurrentScalar {
                field_id: text,
                raw: "line one\nline two".to_owned(),
            },
            ArtifactValidationErrorCategory::InvalidScalarValue,
            text,
            None,
        ),
        (
            TestMutation::CorruptCurrentMultiChoice {
                field_id: multi,
                option_ids: vec![b1, b1],
            },
            ArtifactValidationErrorCategory::InvalidChoiceValue,
            multi,
            Some(b1),
        ),
        (
            TestMutation::ReplaceCurrentRichTextContent {
                field_id: rich,
                content: json!({
                    "kind": "root",
                    "children": []
                })
                .as_object()
                .expect("rich-text root must be an object")
                .clone(),
            },
            ArtifactValidationErrorCategory::InvalidRichTextValue,
            rich,
            None,
        ),
    ];
    for (operation, validation_category, field, option) in cases {
        let error = assert_error(
            apply(&source, LATER_AT, operation),
            TemplateMutationErrorCategory::InvalidCandidate,
            Some(field),
            option,
        );
        assert_eq!(error.validation_category(), Some(validation_category));
    }
}

#[test]
fn invalid_initial_default_is_rejected_on_a_correctly_revisioned_new_field() {
    let source = fixture();
    let error = assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::AddInvalidInitialText {
                source_field_id: id(FIELD_TEXT),
                new_field_id: id(FIELD_NEW),
            },
        ),
        TemplateMutationErrorCategory::InvalidCandidate,
        Some(id(FIELD_NEW)),
        None,
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
}

#[test]
fn new_field_cannot_inject_an_old_introduced_revision() {
    let source = fixture();
    let new_field = id(FIELD_NEW);
    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::AddFieldWithRevision {
                source_field_id: id(FIELD_TEXT),
                new_field_id: new_field,
                revision: source.revision(),
            },
        ),
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(new_field),
        None,
    );
}

#[test]
fn new_field_requires_exact_final_revision_and_exact_revision_succeeds() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let new_field = id(FIELD_NEW);

    assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::AddFieldWithRevision {
                source_field_id: id(FIELD_TEXT),
                new_field_id: new_field,
                revision: revision(4),
            },
        ),
        TemplateMutationErrorCategory::ImmutableFieldChanged,
        Some(new_field),
        None,
    );

    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::AddFieldWithRevision {
            source_field_id: id(FIELD_TEXT),
            new_field_id: new_field,
            revision: revision(3),
        },
    )
    .expect("exact final introducedRevision must succeed")
    .into_changed()
    .expect("new Field must change the Template");
    assert_eq!(changed.revision(), revision(3));
    assert_eq!(
        changed.fields()[&new_field].introduced_revision(),
        changed.revision()
    );
    let encoded =
        encode_template(&changed).expect("Changed snapshot must be immediately encodable");
    assert_eq!(
        decode_template(&encoded).expect("Changed bytes must decode"),
        changed
    );
    assert_eq!(
        encode_template(&source).expect("source must remain unchanged"),
        before
    );
}

#[test]
fn mutation_admission_and_final_validation_share_full_template_wire_depth_boundaries() {
    let source = fixture();
    let rich = id(FIELD_RICH_TEXT);
    let at_boundary = rich_text_content_at_wire_depth(&source, MAX_JSON_NESTING_DEPTH);
    let over_boundary = rich_text_content_at_wire_depth(&source, MAX_JSON_NESTING_DEPTH + 1);

    let mut source_at_boundary = source.clone();
    source_at_boundary.replace_rich_text_default_content_for_test(rich, at_boundary.clone(), false);
    assert_eq!(
        maximum_container_depth(&template_wire_value(&source_at_boundary)),
        MAX_JSON_NESTING_DEPTH
    );
    let source_boundary_bytes =
        encode_template(&source_at_boundary).expect("full wire depth 127 source must encode");
    assert!(apply(&source_at_boundary, LATER_AT, TestMutation::Noop)
        .expect("full wire depth 127 source no-op must pass admission")
        .is_unchanged());
    assert_eq!(
        encode_template(&source_at_boundary).expect("source must remain encodable"),
        source_boundary_bytes
    );

    let mut source_over_boundary = source.clone();
    source_over_boundary.replace_rich_text_default_content_for_test(
        rich,
        over_boundary.clone(),
        false,
    );
    assert_eq!(
        maximum_container_depth(&template_wire_value(&source_over_boundary)),
        MAX_JSON_NESTING_DEPTH + 1
    );
    let invalid_source_before = source_over_boundary.clone();
    let admission_error = assert_error(
        apply(&source_over_boundary, LATER_AT, TestMutation::Noop),
        TemplateMutationErrorCategory::InvalidSource,
        None,
        None,
    );
    assert_eq!(
        admission_error.validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_eq!(source_over_boundary, invalid_source_before);

    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::ReplaceCurrentRichTextContent {
            field_id: rich,
            content: at_boundary,
        },
    )
    .expect("full wire depth 127 candidate must pass final validation")
    .into_changed()
    .expect("new rich-text content must change the Template");
    assert_eq!(
        maximum_container_depth(&template_wire_value(&changed)),
        MAX_JSON_NESTING_DEPTH
    );
    let changed_bytes =
        encode_template(&changed).expect("every Changed outcome must be immediately encodable");
    assert_eq!(
        decode_template(&changed_bytes).expect("Changed bytes must decode"),
        changed
    );

    let source_before = encode_template(&source).expect("source must encode");
    let final_error = assert_error(
        apply(
            &source,
            LATER_AT,
            TestMutation::ReplaceCurrentRichTextContent {
                field_id: rich,
                content: over_boundary,
            },
        ),
        TemplateMutationErrorCategory::InvalidCandidate,
        None,
        None,
    );
    assert_eq!(
        final_error.validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_eq!(
        encode_template(&source).expect("rejected candidate must not alter source"),
        source_before
    );
}

#[test]
fn each_rich_text_metadata_layer_is_independently_fail_closed_from_the_source() {
    let clean_value = metadata_isolated_fixture_value();
    assert_no_rich_metadata(&clean_value);
    let rich = id(FIELD_RICH_TEXT);
    let unset = current_default_from(&clean_value, id(FIELD_ARCHIVED));
    let other_variant = current_default_from(&clean_value, id(FIELD_TEXT));

    for layer in RichMetadataLayer::ALL {
        let mut source_value = clean_value.clone();
        set_rich_metadata(&mut source_value, layer, false);
        assert_only_rich_metadata(&source_value, layer);
        let source = fixture_from_value(&source_value);
        let before = encode_template(&source).expect("isolated metadata source must encode");

        let mut deleted_value = source_value.clone();
        remove_rich_metadata(&mut deleted_value, layer);
        assert_no_rich_metadata(&deleted_value);
        assert_only_target_rich_metadata_differs(&source_value, &deleted_value, layer);
        let deleted_candidate = fixture_from_value(&deleted_value);
        assert_rich_non_metadata_invariants(&source, &deleted_candidate);
        deleted_candidate
            .validate_storage()
            .expect("metadata deletion candidate must otherwise be storage-valid");
        assert_rich_metadata_error_and_unchanged(
            &source,
            &before,
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: deleted_candidate.fields[&rich].default_value.clone(),
                },
            ),
        );

        let mut changed_value = source_value.clone();
        set_rich_metadata(&mut changed_value, layer, true);
        assert_only_rich_metadata(&changed_value, layer);
        assert_only_target_rich_metadata_differs(&source_value, &changed_value, layer);
        let changed_candidate = fixture_from_value(&changed_value);
        assert_rich_non_metadata_invariants(&source, &changed_candidate);
        changed_candidate
            .validate_storage()
            .expect("metadata value-change candidate must otherwise be storage-valid");
        assert_rich_metadata_error_and_unchanged(
            &source,
            &before,
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: changed_candidate.fields[&rich].default_value.clone(),
                },
            ),
        );

        for replacement in [unset.clone(), other_variant.clone()] {
            assert_rich_metadata_error_and_unchanged(
                &source,
                &before,
                apply(
                    &source,
                    LATER_AT,
                    TestMutation::ReplaceCurrentDefault {
                        field_id: rich,
                        value: replacement,
                    },
                ),
            );
        }

        let before_rich = to_deterministic_json_bytes(
            &template_wire_value(&source)["fields"][FIELD_RICH_TEXT]["defaultValue"],
        )
        .expect("isolated rich-text subtree must encode");
        let unrelated = apply(
            &source,
            LATER_AT,
            TestMutation::RenameField {
                field_id: id(FIELD_TEXT),
                label: format!("unrelated rename for {layer:?}"),
            },
        )
        .expect("exact metadata preservation must allow another Field mutation")
        .into_changed()
        .expect("unrelated Field rename must change the Template");
        assert_eq!(
            to_deterministic_json_bytes(
                &template_wire_value(&unrelated)["fields"][FIELD_RICH_TEXT]["defaultValue"]
            )
            .expect("preserved rich-text subtree must encode"),
            before_rich
        );
        encode_template(&unrelated).expect("metadata-preserving Changed must encode");
        assert_eq!(
            encode_template(&source).expect("source must remain byte-identical"),
            before
        );
    }
}

#[test]
fn candidate_only_rich_text_metadata_is_rejected_symmetrically() {
    let source_value = metadata_isolated_fixture_value();
    assert_no_rich_metadata(&source_value);
    let source = fixture_from_value(&source_value);
    let before = encode_template(&source).expect("metadata-free source must encode");
    let rich = id(FIELD_RICH_TEXT);

    for layer in [
        RichMetadataLayer::Envelope,
        RichMetadataLayer::Root,
        RichMetadataLayer::Container,
        RichMetadataLayer::ArbitraryNumber,
    ] {
        let mut candidate_value = source_value.clone();
        set_rich_metadata(&mut candidate_value, layer, false);
        assert_only_rich_metadata(&candidate_value, layer);
        assert_only_target_rich_metadata_differs(&source_value, &candidate_value, layer);
        let candidate = fixture_from_value(&candidate_value);
        assert_rich_non_metadata_invariants(&source, &candidate);
        candidate
            .validate_storage()
            .expect("candidate-only metadata must remain storage-valid");

        assert_rich_metadata_error_and_unchanged(
            &source,
            &before,
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: candidate.fields[&rich].default_value.clone(),
                },
            ),
        );
    }
}

#[test]
fn rich_text_unknown_storage_data_changes_and_lost_locations_fail_closed() {
    let source_value = rich_text_unknown_fixture_value();
    let source = fixture_from_value(&source_value);
    let before = encode_template(&source).expect("unknown metadata source must encode");
    assert!(String::from_utf8_lossy(&before).contains(ARBITRARY_NUMBER));
    let rich = id(FIELD_RICH_TEXT);
    let mut changed_values = Vec::new();

    let mut outer_deleted = source_value.clone();
    outer_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]
        .as_object_mut()
        .expect("FieldValue must be an object")
        .remove("futureValueOuter");
    changed_values.push(outer_deleted);

    let mut envelope_deleted = source_value.clone();
    envelope_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]
        .as_object_mut()
        .expect("document must be an object")
        .remove("futureEnvelope");
    changed_values.push(envelope_deleted);

    let mut root_deleted = source_value.clone();
    root_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]
        .as_object_mut()
        .expect("content root must be an object")
        .remove("futureRoot");
    changed_values.push(root_deleted);

    for path in ["futureParagraph", "futureText", "futureListItem"] {
        let mut nested_deleted = source_value.clone();
        let node = match path {
            "futureParagraph" => {
                &mut nested_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]
                    ["content"]["children"][0]
            }
            "futureText" => {
                &mut nested_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]
                    ["content"]["children"][0]["children"][0]
            }
            "futureListItem" => {
                &mut nested_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]
                    ["content"]["children"][1]["children"][0]
            }
            _ => unreachable!("fixture path is closed"),
        };
        node.as_object_mut()
            .expect("rich-text node must be an object")
            .remove(path);
        changed_values.push(nested_deleted);
    }

    let mut number_changed = source_value.clone();
    number_changed["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["children"]
        [1]["children"][0]["futureListItem"]["number"] =
        serde_json::from_str(OTHER_ARBITRARY_NUMBER)
            .expect("replacement arbitrary-precision number must parse");
    changed_values.push(number_changed);

    let mut number_deleted = source_value.clone();
    number_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["children"][1]
        ["children"][0]["futureListItem"]
        .as_object_mut()
        .expect("list item extra must be an object")
        .remove("number");
    changed_values.push(number_deleted);

    let mut extra_bearing_node_deleted = source_value.clone();
    extra_bearing_node_deleted["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]
        ["children"]
        .as_array_mut()
        .expect("root children must be an array")
        .pop();
    changed_values.push(extra_bearing_node_deleted);

    for changed_value in changed_values {
        let replacement = current_default_from(&changed_value, rich);
        let error = assert_error(
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: replacement,
                },
            ),
            TemplateMutationErrorCategory::ImmutableFieldChanged,
            Some(rich),
            None,
        );
        let rendered = format!("{error:?}\n{error}");
        for secret in [
            SECRET,
            ARBITRARY_NUMBER,
            OTHER_ARBITRARY_NUMBER,
            "raw-ast.json",
            "futureParagraph",
        ] {
            assert!(!rendered.contains(secret));
        }
        assert_eq!(
            encode_template(&source).expect("failed mutation must leave source bytes unchanged"),
            before
        );
    }
}

#[test]
fn rich_text_unknown_data_cannot_lose_its_variant_and_exact_preservation_succeeds() {
    let source_value = rich_text_unknown_fixture_value();
    let source = fixture_from_value(&source_value);
    let before = encode_template(&source).expect("unknown metadata source must encode");
    let rich = id(FIELD_RICH_TEXT);

    for replacement in [
        current_default_from(&fixture_value(), id(FIELD_ARCHIVED)),
        current_default_from(&fixture_value(), id(FIELD_TEXT)),
    ] {
        assert_error(
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: replacement,
                },
            ),
            TemplateMutationErrorCategory::ImmutableFieldChanged,
            Some(rich),
            None,
        );
    }

    let exact = source.fields[&rich].default_value.clone();
    assert!(apply(
        &source,
        LATER_AT,
        TestMutation::ReplaceCurrentDefault {
            field_id: rich,
            value: exact,
        },
    )
    .expect("exact metadata-preserving replacement must succeed")
    .is_unchanged());

    let before_rich = to_deterministic_json_bytes(
        &template_wire_value(&source)["fields"][FIELD_RICH_TEXT]["defaultValue"],
    )
    .expect("rich-text subtree must encode");
    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::RenameField {
            field_id: id(FIELD_TEXT),
            label: "unrelated Field rename".to_owned(),
        },
    )
    .expect("unrelated Field mutation must preserve rich-text metadata")
    .into_changed()
    .expect("Field rename must change the Template");
    let after_rich = to_deterministic_json_bytes(
        &template_wire_value(&changed)["fields"][FIELD_RICH_TEXT]["defaultValue"],
    )
    .expect("changed rich-text subtree must encode");
    assert_eq!(after_rich, before_rich);
    assert_eq!(
        encode_template(&source).expect("source must remain unchanged"),
        before
    );
    let changed_bytes = encode_template(&changed).expect("Changed outcome must encode");
    assert_eq!(
        decode_template(&changed_bytes).expect("Changed bytes must decode"),
        changed
    );
}

#[test]
fn rich_text_outer_extra_alone_can_be_preserved_across_known_content_change() {
    let mut source_value = fixture_value();
    let number: Value = serde_json::from_str(ARBITRARY_NUMBER).unwrap();
    source_value["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureValueOuter"] =
        json!({"nested": [number]});
    let source = fixture_from_value(&source_value);
    let before = encode_template(&source).expect("outer-extra source must encode");
    let rich = id(FIELD_RICH_TEXT);

    let mut changed_value = source_value.clone();
    changed_value["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["children"]
        [0]["children"][0]["text"] = json!("changed known content");
    let replacement = current_default_from(&changed_value, rich);
    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::ReplaceCurrentDefault {
            field_id: rich,
            value: replacement,
        },
    )
    .expect("identical outer metadata can accompany a known content change")
    .into_changed()
    .expect("different known content must change the Template");
    assert_eq!(
        template_wire_value(&changed)["fields"][FIELD_RICH_TEXT]["defaultValue"]
            ["futureValueOuter"],
        source_value["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureValueOuter"]
    );
    assert_eq!(
        encode_template(&source).expect("accepted mutation must preserve source bytes"),
        before
    );
}

#[test]
fn rich_text_outer_only_unset_transitions_keep_template_mutation_guards() {
    use crate::data::{
        field_engine::rich_text::normalize_rich_text, json::parse_strict_lossless_json_object,
    };
    let mut raw = fixture_value();
    raw["fields"][FIELD_RICH_TEXT]["defaultValue"]["futureValueOuter"] = json!("__outer_tokens__");
    let bytes = serde_json::to_string(&raw).expect("fixture JSON").replace(
        "\"__outer_tokens__\"",
        "[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]",
    );
    let source = decode_template(bytes.as_bytes()).expect("outer-only source");
    let before = encode_template(&source).expect("source encode");
    let rich = id(FIELD_RICH_TEXT);
    let unset = apply_template_mutation(
        &source,
        source.revision(),
        LATER_AT,
        TemplateMutationCommand::set_current_default(rich, FieldValueDraft::unset()),
    )
    .expect("outer-only unset")
    .into_changed()
    .expect("changed default");
    assert!(unset.fields()[&rich].default_value().is_unset());
    let unset_bytes = encode_template(&unset).expect("unset encode");
    let decoded = decode_template(&unset_bytes).expect("unset decode");
    let content = json!({"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":"fresh content"}]}]});
    let draft = FieldValueDraft::from_normalized_rich_text(
        normalize_rich_text(1, content.as_object().expect("content")).expect("normalized"),
    );
    let restored = apply_template_mutation(
        &decoded,
        decoded.revision(),
        LATER_AT,
        TemplateMutationCommand::set_current_default(rich, draft.clone()),
    )
    .expect("outer-only rich-text")
    .into_changed()
    .expect("changed default");
    let restored_bytes = encode_template(&restored).expect("restored encode");
    let expected = parse_strict_lossless_json_object(
        br#"{"n":[1E100,1e100,-0,0.12345678901234567890123456789,[2,-0]]}"#,
    )
    .expect("expected tokens");
    for bytes in [&before, &unset_bytes, &restored_bytes] {
        let tree = parse_strict_lossless_json_object(bytes).expect("lossless Template");
        let outer = tree
            .object_path(&[
                "fields",
                FIELD_RICH_TEXT,
                "defaultValue",
                "futureValueOuter",
            ])
            .expect("outer exists");
        assert!(
            outer == expected.object_path(&["n"]).expect("expected canary"),
            "outer tokens changed"
        );
    }
    assert!(apply_template_mutation(
        &restored,
        restored.revision(),
        LATER_AT,
        TemplateMutationCommand::set_current_default(rich, draft)
    )
    .expect("repeat")
    .is_unchanged());
    // 공유 helper의 허용 범위는 같은 outer를 가진 rich-text/unset에 한정한다.
    let missing_outer = FieldValue::unset();
    let text_with_outer = FieldValue::single_line_text("wrong kind".into())
        .preserve_outer_storage_extra_from(source.fields()[&rich].default_value());
    for replacement in [missing_outer, text_with_outer] {
        assert_error(
            apply(
                &source,
                LATER_AT,
                TestMutation::ReplaceCurrentDefault {
                    field_id: rich,
                    value: replacement,
                },
            ),
            TemplateMutationErrorCategory::ImmutableFieldChanged,
            Some(rich),
            None,
        );
    }
    assert!(encode_template(&source).expect("source unchanged") == before);
}

#[test]
fn rich_text_without_unknown_data_can_change_known_default_content() {
    let source = fixture();
    let before = encode_template(&source).expect("source must encode");
    let rich = id(FIELD_RICH_TEXT);
    let mut changed_value = fixture_value();
    changed_value["fields"][FIELD_RICH_TEXT]["defaultValue"]["document"]["content"]["children"]
        [0]["children"][0]["text"] = json!("updated known content");
    let replacement = current_default_from(&changed_value, rich);

    let changed = apply(
        &source,
        LATER_AT,
        TestMutation::ReplaceCurrentDefault {
            field_id: rich,
            value: replacement,
        },
    )
    .expect("known rich-text content without unknown metadata must remain mutable")
    .into_changed()
    .expect("new known content must change the Template");
    assert_eq!(changed.revision(), revision(3));
    encode_template(&changed).expect("Changed rich-text snapshot must encode");
    assert_eq!(
        encode_template(&source).expect("source must remain unchanged"),
        before
    );
}

#[test]
fn semantically_invalid_source_rejects_noop_without_mutation() {
    let mut source = fixture();
    source.corrupt_scalar_default_for_test(id(FIELD_TEXT), "line one\nline two", false);
    let before = source.clone();
    let error = assert_error(
        apply(&source, LATER_AT, TestMutation::Noop),
        TemplateMutationErrorCategory::InvalidSource,
        None,
        None,
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(source, before);
}

#[test]
fn error_source_chain_and_outcome_debug_redact_all_payloads() {
    let source = fixture();
    let error = apply(&source, LATER_AT, TestMutation::ClearFieldOrder)
        .expect_err("invalid candidate must fail");
    let mut rendered = format!("{error:?}\n{error}");
    let mut current = error.source();
    while let Some(source_error) = current {
        rendered.push_str(&format!("\n{source_error:?}\n{source_error}"));
        current = source_error.source();
    }
    assert!(!rendered.contains(SECRET));
    assert!(!rendered.contains("unknownExtraSentinel"));
    assert!(!rendered.contains("mutation-secret"));
    assert!(!rendered.contains("C:\\private"));
    assert!(!rendered.contains("/home/private"));

    let outcome = apply(
        &source,
        LATER_AT,
        TestMutation::Rename(format!("changed {SECRET}")),
    )
    .expect("valid secret-bearing candidate must succeed");
    let debug = format!("{outcome:?}");
    assert!(!debug.contains(SECRET));
    assert!(!debug.contains("mutation-secret"));
    assert!(debug.contains("template_redacted"));

    let rejected = apply(
        &source,
        LATER_AT,
        TestMutation::RenameThenReject(SECRET.to_owned()),
    )
    .expect_err("command error must fail");
    assert!(!format!("{rejected:?}\n{rejected}").contains(SECRET));
}

#[test]
fn validated_structural_mutation_rebases_owned_number_provenance() {
    let mut raw = fixture_value();
    raw["futureRoot"]["number"] = json!("__ROOT_NUMBER__");
    raw["fields"][FIELD_TEXT]["futureFieldNumber"] = json!("__FIELD_NUMBER__");
    raw["fields"][FIELD_TEXT]["defaultValue"]["futureValue"] =
        json!({"nested":["__VALUE_NUMBER__"]});
    let mut raw =
        String::from_utf8(to_deterministic_json_bytes(&raw).expect("fixture JSON must serialize"))
            .expect("fixture JSON is UTF-8");
    for (marker, token) in [
        ("__ROOT_NUMBER__", "1E9223372036854775808"),
        ("__FIELD_NUMBER__", "1e+0009223372036854775808"),
        ("__VALUE_NUMBER__", "-0E-999999999999999999999"),
    ] {
        raw = raw.replace(&format!("\"{marker}\""), token);
    }
    let source = decode_template(raw.as_bytes()).expect("fixture Template must decode");
    let source_bytes = encode_template(&source).expect("source must encode");

    let outcome = apply_template_mutation(
        &source,
        source.revision(),
        LATER_AT,
        TemplateMutationCommand::archive_field(id(FIELD_TEXT)),
    )
    .expect("archive must succeed");
    let changed = outcome.changed().expect("archive must change the Template");
    let encoded = encode_template(changed).expect("changed Template must encode");
    let text = std::str::from_utf8(&encoded).expect("output is UTF-8");
    for token in [
        "1E9223372036854775808",
        "1e+0009223372036854775808",
        "-0E-999999999999999999999",
    ] {
        assert!(text.contains(token), "owned lexeme changed: {token}");
    }
    assert_eq!(
        encode_template(&decode_template(&encoded).expect("output must decode"))
            .expect("output must re-encode"),
        encoded
    );
    assert_eq!(
        encode_template(&source).expect("source must remain encodable"),
        source_bytes
    );
}

mod field_commands;
mod option_commands;
mod template_commands;
