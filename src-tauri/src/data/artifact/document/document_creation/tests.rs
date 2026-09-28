use std::{collections::BTreeSet, error::Error, str::FromStr};

use serde_json::{json, Map, Value};

use super::*;
use crate::data::artifact::{
    document::DocumentWire, template::TemplateWire, value::FieldValueWire,
};
use crate::data::{
    artifact::{decode_document, decode_template, encode_document, encode_template},
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
        validation::{
            BoundDocumentValueContext, FieldValidationErrorCategory, FieldValidationLocation,
            FieldValidationOutcome,
        },
    },
    json::{to_deterministic_json_bytes, MAX_JSON_NESTING_DEPTH},
};

const TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000001";
const DOCUMENT_ID: &str = "90000000-0000-4000-8000-000000000001";
const TIMESTAMP: &str = "2026-09-05T12:34:56.789Z";

const FIELD_TEXT: &str = "20000000-0000-4000-8000-000000000001";
const FIELD_RICH: &str = "20000000-0000-4000-8000-000000000002";
const FIELD_NUMBER: &str = "20000000-0000-4000-8000-000000000003";
const FIELD_DATE: &str = "20000000-0000-4000-8000-000000000004";
const FIELD_TIME: &str = "20000000-0000-4000-8000-000000000005";
const FIELD_DURATION: &str = "20000000-0000-4000-8000-000000000006";
const FIELD_SINGLE: &str = "20000000-0000-4000-8000-000000000007";
const FIELD_MULTI: &str = "20000000-0000-4000-8000-000000000008";
const FIELD_ARCHIVED: &str = "20000000-0000-4000-8000-000000000009";
const FIELD_ARCHIVED_SCALAR: &str = "20000000-0000-4000-8000-00000000000a";
const FIELD_ARCHIVED_CHOICE: &str = "20000000-0000-4000-8000-00000000000b";

const OPTION_A: &str = "a0000000-0000-4000-8000-000000000001";
const OPTION_B: &str = "a0000000-0000-4000-8000-000000000002";
const OPTION_ARCHIVED: &str = "a0000000-0000-4000-8000-000000000003";
const OPTION_SINGLE_A: &str = "a1000000-0000-4000-8000-000000000001";
const OPTION_SINGLE_B: &str = "a1000000-0000-4000-8000-000000000002";
const OPTION_SINGLE_ARCHIVED: &str = "a1000000-0000-4000-8000-000000000003";
const OPTION_UNKNOWN: &str = "b0000000-0000-4000-8000-000000000001";

fn field(
    kind: &str,
    lifecycle: &str,
    required: bool,
    current: Value,
    initial: Value,
    configuration: Value,
) -> Value {
    json!({
        "label": format!("private-{kind}-label"),
        "lifecycle": lifecycle,
        "kind": kind,
        "required": required,
        "defaultValue": current,
        "initialDefaultValue": initial,
        "introducedRevision": 1,
        "configuration": configuration,
        "presentation": {}
    })
}

fn basic_configuration(kind: &str) -> Value {
    json!({"kind":kind})
}

fn choice_configuration(kind: &str) -> Value {
    json!({
        "kind":kind,
        "optionOrder":[OPTION_B,OPTION_A],
        "options":{
            (OPTION_A):{"label":"private-option-a","lifecycle":"active"},
            (OPTION_B):{"label":"private-option-b","lifecycle":"active"},
            (OPTION_ARCHIVED):{"label":"private-option-archived","lifecycle":"archived"}
        }
    })
}

fn single_choice_configuration() -> Value {
    json!({
        "kind":"singleChoice",
        "optionOrder":[OPTION_SINGLE_B,OPTION_SINGLE_A],
        "options":{
            (OPTION_SINGLE_A):{"label":"private-single-a","lifecycle":"active"},
            (OPTION_SINGLE_B):{"label":"private-single-b","lifecycle":"active"},
            (OPTION_SINGLE_ARCHIVED):{"label":"private-single-archived","lifecycle":"archived"}
        }
    })
}

fn rich_text(text: &str) -> Value {
    json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{
                "kind":"root",
                "children":[{"kind":"paragraph","children":[{"kind":"text","text":text}]}]
            }
        }
    })
}

fn template_value(
    fields: impl IntoIterator<Item = (&'static str, Value)>,
    active_order: &[&str],
    revision: u32,
    lifecycle: &str,
) -> Value {
    let fields = fields
        .into_iter()
        .map(|(id, field)| (id.to_owned(), field))
        .collect::<Map<_, _>>();
    json!({
        "schemaVersion":1,
        "artifactType":"template",
        "templateId":TEMPLATE_ID,
        "revision":revision,
        "name":"private-template-name",
        "lifecycle":lifecycle,
        "fieldOrder":active_order,
        "fields":fields,
        "presentation":{},
        "createdAtUtc":"2026-09-01T00:00:00.000Z",
        "updatedAtUtc":"2026-09-02T00:00:00.000Z"
    })
}

fn decode_template_value(value: &Value) -> TemplateArtifact {
    decode_template(&to_deterministic_json_bytes(value).expect("fixture JSON must encode"))
        .expect("fixture Template must be valid")
}

fn document_id() -> DocumentId {
    DocumentId::from_str(DOCUMENT_ID).expect("constant DocumentId must be valid")
}

fn field_id(raw: &str) -> FieldId {
    FieldId::from_str(raw).expect("constant FieldId must be valid")
}

fn option_id(raw: &str) -> crate::data::artifact::OptionId {
    crate::data::artifact::OptionId::from_str(raw).expect("constant OptionId must be valid")
}

fn create(
    source: &TemplateArtifact,
    name: &str,
) -> Result<DocumentCreationOutcome, DocumentCreationError> {
    create_document(
        source,
        source.revision(),
        document_id(),
        name.to_owned(),
        TIMESTAMP.to_owned(),
    )
}

fn template_snapshot_bytes(source: &TemplateArtifact) -> Vec<u8> {
    to_deterministic_json_bytes(&TemplateWire::from(source))
        .expect("test-only Template projection must encode")
}

fn unchecked_template_snapshot_bytes(source: &TemplateArtifact) -> Vec<u8> {
    serde_json::to_vec(&TemplateWire::from(source))
        .expect("test-only unchecked Template projection must serialize")
}

fn document_wire_value(document: &DocumentArtifact) -> Value {
    serde_json::to_value(DocumentWire::from(document))
        .expect("test-only Document projection must encode")
}

fn field_value_bytes(value: &FieldValue) -> Vec<u8> {
    to_deterministic_json_bytes(&FieldValueWire::from(value))
        .expect("test-only FieldValue projection must encode")
}

fn maximum_container_depth(value: &Value) -> usize {
    let mut maximum = 0;
    let mut pending = vec![(value, 0_usize)];
    while let Some((value, parent_depth)) = pending.pop() {
        match value {
            Value::Object(object) => {
                let depth = parent_depth + 1;
                maximum = maximum.max(depth);
                pending.extend(object.values().map(|child| (child, depth)));
            }
            Value::Array(items) => {
                let depth = parent_depth + 1;
                maximum = maximum.max(depth);
                pending.extend(items.iter().map(|child| (child, depth)));
            }
            _ => {}
        }
    }
    maximum
}

fn nested_array(container_depth: usize, leaf: Value) -> Value {
    (0..container_depth).fold(leaf, |value, _| Value::Array(vec![value]))
}

fn assert_source_unchanged(
    source: &TemplateArtifact,
    before: &TemplateArtifact,
    bytes_before: &[u8],
) {
    assert_eq!(source, before);
    assert_eq!(template_snapshot_bytes(source), bytes_before);
}

fn assert_error_redacted(error: &DocumentCreationError, forbidden: &[&str]) {
    for rendered in [error.to_string(), format!("{error:?}")] {
        for fragment in forbidden {
            assert!(
                !rendered.contains(fragment),
                "creation error leaked forbidden fragment {fragment}"
            );
        }
    }
    assert!(error.source().is_none());
}

#[test]
fn empty_template_creation_preserves_opaque_inputs_max_revision_and_determinism(
) -> Result<(), Box<dyn Error>> {
    let source = decode_template_value(&template_value([], &[], u32::MAX, "active"));
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;

    for name in [
        "",
        "line one\r\nline two\u{2028}세 번째",
        "\u{0085}",
        "\u{2029}",
        " \t  ",
    ] {
        let first = create(&source, name)?;
        let second = create(&source, name)?;
        assert_eq!(first.document(), second.document());
        assert_eq!(
            encode_document(first.document())?,
            encode_document(second.document())?
        );

        let debug = format!("{first:?}");
        for forbidden in [name, DOCUMENT_ID, TEMPLATE_ID] {
            if !forbidden.is_empty() {
                assert!(!debug.contains(forbidden));
            }
        }
        let document = first.into_document();
        assert_eq!(document.document_id(), document_id());
        assert_eq!(document.template_id(), source.template_id());
        assert_eq!(document.template_revision(), source.revision());
        assert_eq!(document.name(), name);
        assert_eq!(document.created_at_utc(), TIMESTAMP);
        assert_eq!(document.updated_at_utc(), TIMESTAMP);
        assert!(document.field_values().is_empty());
        assert!(document.orphaned_field_definitions().is_empty());
        document.validate_storage()?;

        let wire = document_wire_value(&document);
        assert_eq!(
            wire.as_object()
                .expect("Document wire must be an object")
                .len(),
            10
        );
        let encoded = encode_document(&document)?;
        let decoded = decode_document(&encoded)?;
        assert_eq!(encode_document(&decoded)?, encoded);
    }

    assert_eq!(encode_template(&source)?, source_bytes);
    assert_eq!(source, source_before);
    Ok(())
}

#[test]
fn explicit_eight_kind_values_are_bound_validated_without_reordering() -> Result<(), Box<dyn Error>>
{
    let fields = [
        (
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"text","value":"current text"}),
                json!({"kind":"text","value":"historical text"}),
                basic_configuration("singleLineText"),
            ),
        ),
        (
            FIELD_RICH,
            field(
                "richText",
                "active",
                true,
                rich_text("current rich"),
                rich_text("historical rich"),
                basic_configuration("richText"),
            ),
        ),
        (
            FIELD_NUMBER,
            field(
                "number",
                "active",
                true,
                json!({"kind":"number","value":"12345678901234567890.123"}),
                json!({"kind":"number","value":"-1"}),
                basic_configuration("number"),
            ),
        ),
        (
            FIELD_DATE,
            field(
                "date",
                "active",
                true,
                json!({"kind":"date","value":"2026-09-05"}),
                json!({"kind":"date","value":"2020-01-01"}),
                basic_configuration("date"),
            ),
        ),
        (
            FIELD_TIME,
            field(
                "time",
                "active",
                true,
                json!({"kind":"time","value":"12:34:56.789"}),
                json!({"kind":"time","value":"00:00:00.000"}),
                basic_configuration("time"),
            ),
        ),
        (
            FIELD_DURATION,
            field(
                "duration",
                "active",
                true,
                json!({"kind":"duration","milliseconds":"9223372036854775807"}),
                json!({"kind":"duration","milliseconds":"0"}),
                basic_configuration("duration"),
            ),
        ),
        (
            FIELD_SINGLE,
            field(
                "singleChoice",
                "active",
                true,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_A}),
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_B}),
                single_choice_configuration(),
            ),
        ),
        (
            FIELD_MULTI,
            field(
                "multiChoice",
                "active",
                true,
                json!({"kind":"multiChoice","optionIds":[OPTION_A,OPTION_B]}),
                json!({"kind":"multiChoice","optionIds":[OPTION_B]}),
                choice_configuration("multiChoice"),
            ),
        ),
    ];
    let order = [
        FIELD_TEXT,
        FIELD_RICH,
        FIELD_NUMBER,
        FIELD_DATE,
        FIELD_TIME,
        FIELD_DURATION,
        FIELD_SINGLE,
        FIELD_MULTI,
    ];
    let source = decode_template_value(&template_value(fields, &order, 7, "active"));
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;
    assert_eq!(
        create(&source, "blank-required").unwrap_err().category(),
        DocumentCreationErrorCategory::RequiredValueUnset
    );
    let values = source
        .fields()
        .iter()
        .map(|(id, f)| (*id, f.default_value().clone()))
        .collect();
    let document = create_document_with_values(
        &source,
        source.revision(),
        document_id(),
        "eight-kind".into(),
        false,
        TIMESTAMP.into(),
        &values,
    )?
    .into_document();

    assert_eq!(document.field_values().len(), source.fields().len());
    for (id, definition) in source.fields() {
        assert_eq!(
            document.field_values().get(id),
            Some(definition.default_value())
        );
        assert_eq!(
            field_value_bytes(&document.field_values()[id]),
            field_value_bytes(definition.default_value())
        );
    }
    assert_eq!(
        document.field_values()[&field_id(FIELD_MULTI)].multi_choice(),
        Some([option_id(OPTION_A), option_id(OPTION_B)].as_slice())
    );
    assert_eq!(
        source.fields()[&field_id(FIELD_MULTI)]
            .configuration()
            .option_order(),
        Some([option_id(OPTION_B), option_id(OPTION_A)].as_slice())
    );
    assert_ne!(
        source.fields()[&field_id(FIELD_TEXT)].default_value(),
        source.fields()[&field_id(FIELD_TEXT)].initial_default_value()
    );
    assert_eq!(
        document.field_values()[&field_id(FIELD_TEXT)],
        *source.fields()[&field_id(FIELD_TEXT)].default_value()
    );
    assert_source_unchanged(&source, &source_before, &source_bytes);

    let encoded = encode_document(&document)?;
    let decoded = decode_document(&encoded)?;
    assert_eq!(encode_document(&decoded)?, encoded);
    Ok(())
}

#[test]
fn archived_required_and_optional_unset_fields_materialize_canonical_unset_for_every_known_key(
) -> Result<(), Box<dyn Error>> {
    let optional_precision = serde_json::from_str::<Value>("123456789012345678901234567890")?;
    let active = field(
        "singleLineText",
        "active",
        false,
        json!({"kind":"unset","futureOptionalUnset":{"precision":optional_precision}}),
        json!({"kind":"text","value":"historical"}),
        basic_configuration("singleLineText"),
    );
    let mut archived = field(
        "richText",
        "archived",
        true,
        rich_text("private archived current"),
        rich_text("private archived initial"),
        basic_configuration("richText"),
    );
    archived["defaultValue"]["futureArchivedMetadata"] = json!({"credential":"not-copied"});
    let archived_scalar = field(
        "number",
        "archived",
        false,
        json!({"kind":"number","value":"12345678901234567890.123"}),
        json!({"kind":"number","value":"-7"}),
        basic_configuration("number"),
    );
    let archived_choice = field(
        "multiChoice",
        "archived",
        true,
        json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED]}),
        json!({"kind":"multiChoice","optionIds":[OPTION_A]}),
        choice_configuration("multiChoice"),
    );
    let source = decode_template_value(&template_value(
        [
            (FIELD_TEXT, active),
            (FIELD_ARCHIVED, archived),
            (FIELD_ARCHIVED_SCALAR, archived_scalar),
            (FIELD_ARCHIVED_CHOICE, archived_choice),
        ],
        &[FIELD_TEXT],
        2,
        "active",
    ));
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;
    let document = create(&source, "mixed")?.into_document();

    assert_eq!(
        document
            .field_values()
            .keys()
            .copied()
            .collect::<BTreeSet<_>>(),
        source.fields().keys().copied().collect()
    );
    assert!(document.field_values()[&field_id(FIELD_TEXT)].is_unset());
    assert_eq!(
        document.field_values()[&field_id(FIELD_TEXT)],
        FieldValue::unset()
    );
    for archived_field in [FIELD_ARCHIVED, FIELD_ARCHIVED_SCALAR, FIELD_ARCHIVED_CHOICE] {
        let archived_value = &document.field_values()[&field_id(archived_field)];
        assert!(archived_value.is_unset());
        assert_eq!(
            field_value_bytes(archived_value),
            b"{\n  \"kind\": \"unset\"\n}\n"
        );
    }
    // U1: archived unset도 최소 kind/label snapshot을 가지며 비선택 Option은 복사하지 않는다.
    assert_eq!(document.orphaned_field_definitions().len(), 3);
    for id in [FIELD_ARCHIVED, FIELD_ARCHIVED_SCALAR, FIELD_ARCHIVED_CHOICE] {
        let snapshot = &document.orphaned_field_definitions()[&field_id(id)];
        assert_eq!(snapshot.kind(), source.fields()[&field_id(id)].kind());
        assert!(snapshot.label() == source.fields()[&field_id(id)].label());
        assert!(snapshot.options().is_empty());
    }
    assert!(!String::from_utf8(encode_document(&document)?)?.contains("not-copied"));
    assert_source_unchanged(&source, &source_before, &source_bytes);
    Ok(())
}

#[test]
fn archived_creation_guard_rejects_storage_valid_non_unset_values_for_all_eight_kinds(
) -> Result<(), Box<dyn Error>> {
    let fields = [
        (
            FIELD_TEXT,
            field(
                "singleLineText",
                "archived",
                false,
                json!({"kind":"text","value":"credential=archived-text C:\\private\\value.txt"}),
                json!({"kind":"text","value":"initial text"}),
                basic_configuration("singleLineText"),
            ),
        ),
        (
            FIELD_RICH,
            field(
                "richText",
                "archived",
                false,
                rich_text("credential=archived-rich"),
                rich_text("initial rich"),
                basic_configuration("richText"),
            ),
        ),
        (
            FIELD_NUMBER,
            field(
                "number",
                "archived",
                false,
                json!({"kind":"number","value":"12345678901234567890.123"}),
                json!({"kind":"number","value":"-1"}),
                basic_configuration("number"),
            ),
        ),
        (
            FIELD_DATE,
            field(
                "date",
                "archived",
                false,
                json!({"kind":"date","value":"2026-09-05"}),
                json!({"kind":"date","value":"2020-01-01"}),
                basic_configuration("date"),
            ),
        ),
        (
            FIELD_TIME,
            field(
                "time",
                "archived",
                false,
                json!({"kind":"time","value":"12:34:56.789"}),
                json!({"kind":"time","value":"00:00:00.000"}),
                basic_configuration("time"),
            ),
        ),
        (
            FIELD_DURATION,
            field(
                "duration",
                "archived",
                false,
                json!({"kind":"duration","milliseconds":"9223372036854775807"}),
                json!({"kind":"duration","milliseconds":"0"}),
                basic_configuration("duration"),
            ),
        ),
        (
            FIELD_SINGLE,
            field(
                "singleChoice",
                "archived",
                false,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_A}),
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_B}),
                single_choice_configuration(),
            ),
        ),
        (
            FIELD_MULTI,
            field(
                "multiChoice",
                "archived",
                false,
                json!({"kind":"multiChoice","optionIds":[OPTION_A,OPTION_B]}),
                json!({"kind":"multiChoice","optionIds":[OPTION_A]}),
                choice_configuration("multiChoice"),
            ),
        ),
    ];
    let source = decode_template_value(&template_value(fields, &[], 4, "active"));
    source.validate_storage()?;
    let before = source.clone();
    let bytes = encode_template(&source)?;

    for (field_id, definition) in source.fields() {
        let override_value = definition.default_value().clone();
        assert!(!override_value.is_unset());
        assert_eq!(
            definition.validate_document_value(
                &override_value,
                BoundDocumentValueContext::ExistingDocumentValue,
            )?,
            FieldValidationOutcome::Valid,
            "fixture must prove ExistingDocumentValue alone accepts this archived value"
        );
        let error = create_document_with_field_value_override_for_test(
            &source,
            document_id(),
            *field_id,
            override_value,
        )
        .expect_err("creation-specific archived guard must reject non-unset values");
        assert_eq!(
            error.category(),
            DocumentCreationErrorCategory::InvalidBoundValue
        );
        assert_eq!(error.field_id(), Some(*field_id));
        assert_eq!(error.bound_validation_category(), None);
        assert_eq!(error.artifact_validation_category(), None);
        assert_error_redacted(
            &error,
            &[
                "credential=archived-text",
                "credential=archived-rich",
                "C:\\private\\value.txt",
                OPTION_A,
                OPTION_B,
                OPTION_SINGLE_A,
                "private-single-a",
            ],
        );
    }
    assert_source_unchanged(&source, &before, &bytes);
    Ok(())
}

#[test]
fn archived_creation_guard_rejects_extra_bearing_unset_and_initial_default_clone(
) -> Result<(), Box<dyn Error>> {
    let source = decode_template_value(&template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "archived",
                true,
                json!({
                    "kind":"unset",
                    "futureArchived":{"credential":"extra-secret","path":"/private/value.txt"}
                }),
                json!({"kind":"text","value":"credential=initial-secret"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[],
        4,
        "active",
    ));
    source.validate_storage()?;
    let before = source.clone();
    let bytes = encode_template(&source)?;
    let definition = &source.fields()[&field_id(FIELD_TEXT)];

    for override_value in [
        definition.default_value().clone(),
        definition.initial_default_value().clone(),
    ] {
        assert_eq!(
            definition.validate_document_value(
                &override_value,
                BoundDocumentValueContext::ExistingDocumentValue,
            )?,
            FieldValidationOutcome::Valid
        );
        let error = create_document_with_field_value_override_for_test(
            &source,
            document_id(),
            field_id(FIELD_TEXT),
            override_value,
        )
        .expect_err("current or initial default clone must not bypass archived creation guard");
        assert_eq!(
            error.category(),
            DocumentCreationErrorCategory::InvalidBoundValue
        );
        assert_eq!(error.field_id(), Some(field_id(FIELD_TEXT)));
        assert_eq!(error.bound_validation_category(), None);
        assert_eq!(error.artifact_validation_category(), None);
        assert_error_redacted(
            &error,
            &[
                "futureArchived",
                "extra-secret",
                "/private/value.txt",
                "initial-secret",
            ],
        );
    }
    assert_source_unchanged(&source, &before, &bytes);
    Ok(())
}

#[test]
fn legacy_defaults_and_metadata_stay_in_template_without_new_document_injection(
) -> Result<(), Box<dyn Error>> {
    const OUTER_VALUE: &str = "outer-value-canary-7ab31001";
    const ENVELOPE_VALUE: &str = "envelope-value-canary-7ab31002";
    const ROOT_VALUE: &str = "root-value-canary-7ab31003";
    const CONTAINER_VALUE: &str = "container-value-canary-7ab31004";
    const TEXT_VALUE: &str = "text-value-canary-7ab31005";
    const INTEGER: &str = "123456789012345678901234567890123456789";
    const DECIMAL: &str = "9.87654321098765432109876543210987654321e+210";

    let integer = serde_json::from_str::<Value>(INTEGER)?;
    let decimal = serde_json::from_str::<Value>(DECIMAL)?;
    let mut rich_default = rich_text("metadata body");
    rich_default["futureValueOuter"] = json!(OUTER_VALUE);
    rich_default["document"]["futureEnvelope"] = json!(ENVELOPE_VALUE);
    rich_default["document"]["content"]["futureRoot"] = json!(ROOT_VALUE);
    rich_default["document"]["content"]["children"][0]["futureContainer"] = json!(CONTAINER_VALUE);
    rich_default["document"]["content"]["children"][0]["children"][0]["futureText"] = json!({
        "sentinel":TEXT_VALUE,
        "nested":[{"integer":integer},["kept-order",{"decimal":decimal}]]
    });

    let mut rich_field = field(
        "richText",
        "active",
        false,
        rich_default,
        rich_text("historical body"),
        basic_configuration("richText"),
    );
    rich_field["futureFieldDefinition"] = json!("field-only-not-copied");
    rich_field["configuration"]["futureConfiguration"] = json!("configuration-only-not-copied");
    rich_field["presentation"]["futurePresentation"] = json!("presentation-only-not-copied");

    let mut choice_default = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_A,OPTION_B],
        "futureChoiceValue":{"kept":[1,2,3]}
    });
    choice_default["futureChoicePrecision"] =
        serde_json::from_str::<Value>("0.123456789012345678901234567890123456789")?;
    let mut choice_field = field(
        "multiChoice",
        "active",
        false,
        choice_default,
        json!({"kind":"multiChoice","optionIds":[OPTION_A]}),
        choice_configuration("multiChoice"),
    );
    choice_field["configuration"]["options"][OPTION_A]["futureOption"] =
        json!("option-only-not-copied");

    let mut raw = template_value(
        [(FIELD_RICH, rich_field), (FIELD_MULTI, choice_field)],
        &[FIELD_RICH, FIELD_MULTI],
        4,
        "active",
    );
    raw["futureTemplateRoot"] = json!("template-only-not-copied");
    let source = decode_template_value(&raw);
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;
    let outcome = create(&source, "private-metadata-document")?;
    let outcome_debug = format!("{outcome:?}");
    assert!(outcome_debug.contains("DocumentCreationOutcome"));
    assert!(outcome_debug.contains("field_value_count: 2"));
    for forbidden in [
        "private-metadata-document",
        TEMPLATE_ID,
        DOCUMENT_ID,
        "metadata body",
        OUTER_VALUE,
        ENVELOPE_VALUE,
        ROOT_VALUE,
        CONTAINER_VALUE,
        TEXT_VALUE,
        INTEGER,
        DECIMAL,
        OPTION_A,
        OPTION_B,
        "field-only-not-copied",
    ] {
        assert!(!outcome_debug.contains(forbidden));
    }
    let document = outcome.into_document();

    assert!(document.field_values().values().all(|v| v.is_unset()));
    let encoded = encode_document(&document)?;
    let encoded_text = std::str::from_utf8(&encoded)?;
    for preserved in [
        OUTER_VALUE,
        ENVELOPE_VALUE,
        ROOT_VALUE,
        CONTAINER_VALUE,
        TEXT_VALUE,
        INTEGER,
        DECIMAL,
        "futureChoiceValue",
        "0.123456789012345678901234567890123456789",
    ] {
        assert!(
            !encoded_text.contains(preserved),
            "legacy value leaked: {preserved}"
        );
    }
    for source_only in [
        "field-only-not-copied",
        "configuration-only-not-copied",
        "presentation-only-not-copied",
        "option-only-not-copied",
        "template-only-not-copied",
    ] {
        assert!(!encoded_text.contains(source_only));
    }
    assert_eq!(
        document.field_values()[&field_id(FIELD_MULTI)].multi_choice(),
        None
    );
    let decoded = decode_document(&encoded)?;
    assert_eq!(decoded, document);
    assert_eq!(encode_document(&decoded)?, encoded);
    assert_source_unchanged(&source, &source_before, &source_bytes);
    Ok(())
}

#[test]
fn admission_priority_is_source_then_revision_timestamp_and_tombstone() -> Result<(), Box<dyn Error>>
{
    let raw = template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"valid"}),
                json!({"kind":"unset"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[FIELD_TEXT],
        3,
        "active",
    );
    let mut invalid_source = decode_template_value(&raw);
    invalid_source.corrupt_field_order_for_test();
    let invalid_before = invalid_source.clone();
    let invalid_bytes = template_snapshot_bytes(&invalid_source);
    let error = create_document(
        &invalid_source,
        TemplateRevision::INITIAL,
        document_id(),
        "private-document-name".to_owned(),
        "malformed credential=timestamp-secret".to_owned(),
    )
    .expect_err("invalid source must win every later precondition");
    assert_eq!(
        error.category(),
        DocumentCreationErrorCategory::InvalidSource
    );
    assert_eq!(
        error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_error_redacted(
        &error,
        &["private-document-name", "credential=timestamp-secret"],
    );
    assert_source_unchanged(&invalid_source, &invalid_before, &invalid_bytes);

    let mut deleted_invalid_raw = template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"credential=deleted-invalid-value"}),
                json!({"kind":"unset"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[FIELD_TEXT],
        3,
        "deleted",
    );
    deleted_invalid_raw["futureDeletedMetadata"] = json!({
        "url":"https://private.example.invalid/token",
        "windowsPath":"C:\\private\\deleted.txt",
        "unixPath":"/private/deleted.txt"
    });
    let mut deleted_invalid = decode_template_value(&deleted_invalid_raw);
    deleted_invalid.corrupt_field_order_for_test();
    let deleted_invalid_before = deleted_invalid.clone();
    let deleted_invalid_bytes = unchecked_template_snapshot_bytes(&deleted_invalid);
    let deleted_invalid_error = create_document(
        &deleted_invalid,
        deleted_invalid.revision(),
        document_id(),
        "credential=deleted-invalid-name".to_owned(),
        TIMESTAMP.to_owned(),
    )
    .expect_err("invalid source admission must precede the deleted lifecycle gate");
    assert_eq!(
        deleted_invalid_error.category(),
        DocumentCreationErrorCategory::InvalidSource
    );
    assert_eq!(
        deleted_invalid_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_eq!(deleted_invalid_error.field_id(), None);
    assert_error_redacted(
        &deleted_invalid_error,
        &[
            "credential=deleted-invalid-name",
            "credential=deleted-invalid-value",
            "private-singleLineText-label",
            "https://private.example.invalid/token",
            "C:\\private\\deleted.txt",
            "/private/deleted.txt",
        ],
    );
    assert_eq!(deleted_invalid, deleted_invalid_before);
    assert_eq!(
        unchecked_template_snapshot_bytes(&deleted_invalid),
        deleted_invalid_bytes
    );

    let deleted = decode_template_value(&template_value([], &[], 3, "deleted"));
    let deleted_before = deleted.clone();
    let deleted_bytes = encode_template(&deleted)?;
    let stale = create_document(
        &deleted,
        TemplateRevision::INITIAL,
        document_id(),
        "private-name".to_owned(),
        "malformed".to_owned(),
    )
    .expect_err("revision mismatch must precede timestamp and lifecycle");
    assert_eq!(
        stale.category(),
        DocumentCreationErrorCategory::RevisionMismatch
    );

    let malformed = create_document(
        &deleted,
        deleted.revision(),
        document_id(),
        "private-name".to_owned(),
        "malformed credential=timestamp-secret".to_owned(),
    )
    .expect_err("timestamp must precede deleted lifecycle");
    assert_eq!(
        malformed.category(),
        DocumentCreationErrorCategory::InvalidTimestamp
    );
    assert_error_redacted(&malformed, &["private-name", "credential=timestamp-secret"]);

    let tombstoned = create_document(
        &deleted,
        deleted.revision(),
        document_id(),
        "private-name".to_owned(),
        TIMESTAMP.to_owned(),
    )
    .expect_err("deleted Template cannot create a Document");
    assert_eq!(
        tombstoned.category(),
        DocumentCreationErrorCategory::TemplateIsTombstoned
    );
    assert_source_unchanged(&deleted, &deleted_before, &deleted_bytes);
    Ok(())
}

#[test]
fn active_required_unset_returns_typed_bound_error_without_payload_or_candidate(
) -> Result<(), Box<dyn Error>> {
    let source = decode_template_value(&template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"unset","futureDefault":{"credential":"required-secret"}}),
                json!({"kind":"text","value":"historical fallback must not be used"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[FIELD_TEXT],
        2,
        "active",
    ));
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;
    let error = create(&source, "private-required-document")
        .expect_err("required current unset must fail rather than use the initial default");
    assert_eq!(
        error.category(),
        DocumentCreationErrorCategory::RequiredValueUnset
    );
    assert_eq!(error.field_id(), Some(field_id(FIELD_TEXT)));
    assert_eq!(
        error.bound_validation_category(),
        Some(FieldValidationErrorCategory::RequiredValueUnset)
    );
    assert_eq!(
        error.bound_validation_location(),
        Some(FieldValidationLocation::NewDocumentValue)
    );
    assert_error_redacted(
        &error,
        &[
            "private-required-document",
            "required-secret",
            "historical fallback must not be used",
            "futureDefault",
        ],
    );
    assert_source_unchanged(&source, &source_before, &source_bytes);
    Ok(())
}

#[test]
fn invalid_scalar_rich_text_and_choice_defaults_fail_during_source_admission(
) -> Result<(), Box<dyn Error>> {
    let scalar_raw = template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"valid"}),
                json!({"kind":"unset"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[FIELD_TEXT],
        1,
        "active",
    );
    let mut scalar_source = decode_template_value(&scalar_raw);
    scalar_source.corrupt_scalar_default_for_test(
        field_id(FIELD_TEXT),
        "line one\ncredential=scalar-secret C:\\private\\scalar.txt",
        false,
    );
    let scalar_before = scalar_source.clone();
    let scalar_bytes = template_snapshot_bytes(&scalar_source);
    let scalar_error = create(&scalar_source, "private-name")
        .expect_err("invalid scalar default must fail source admission");
    assert_eq!(
        scalar_error.category(),
        DocumentCreationErrorCategory::InvalidSource
    );
    assert_eq!(
        scalar_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        scalar_error.artifact_scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        scalar_error.artifact_scalar_location(),
        Some(ArtifactScalarValueLocation::CurrentDefault)
    );
    assert_error_redacted(
        &scalar_error,
        &[
            "scalar-secret",
            "C:\\private\\scalar.txt",
            "line one",
            "private-name",
        ],
    );
    assert_source_unchanged(&scalar_source, &scalar_before, &scalar_bytes);

    let rich_raw = template_value(
        [(
            FIELD_RICH,
            field(
                "richText",
                "active",
                false,
                rich_text("valid"),
                rich_text("historical"),
                basic_configuration("richText"),
            ),
        )],
        &[FIELD_RICH],
        1,
        "active",
    );
    let mut rich_source = decode_template_value(&rich_raw);
    rich_source.replace_rich_text_default_content_for_test(
        field_id(FIELD_RICH),
        json!({"kind":"root","children":[]})
            .as_object()
            .expect("fixture content must be an object")
            .clone(),
        false,
    );
    let rich_before = rich_source.clone();
    let rich_bytes = template_snapshot_bytes(&rich_source);
    let rich_error = create(&rich_source, "private-name")
        .expect_err("semantic-empty rich text must fail source admission");
    assert_eq!(
        rich_error.category(),
        DocumentCreationErrorCategory::InvalidSource
    );
    assert_eq!(
        rich_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidRichTextValue)
    );
    assert_eq!(
        rich_error.artifact_rich_text_category(),
        Some(RichTextValidationErrorCategory::SemanticEmpty)
    );
    assert_eq!(
        rich_error.artifact_rich_text_location(),
        Some(ArtifactRichTextValueLocation::CurrentDefault)
    );
    assert_eq!(
        rich_error.artifact_rich_text_structure_location(),
        Some(RichTextErrorLocation::Root)
    );
    assert_source_unchanged(&rich_source, &rich_before, &rich_bytes);

    let choice_raw = template_value(
        [(
            FIELD_MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[OPTION_A,OPTION_B]}),
                json!({"kind":"unset"}),
                choice_configuration("multiChoice"),
            ),
        )],
        &[FIELD_MULTI],
        1,
        "active",
    );
    for (selection, expected) in [
        (
            vec![option_id(OPTION_A), option_id(OPTION_A)],
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
        ),
        (
            vec![option_id(OPTION_ARCHIVED)],
            ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ),
        (
            vec![option_id(OPTION_UNKNOWN)],
            ChoiceValidationErrorCategory::UnknownSelectedOption,
        ),
        (
            vec![option_id(OPTION_B), option_id(OPTION_A)],
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
    ] {
        let mut source = decode_template_value(&choice_raw);
        source.corrupt_multi_choice_default_for_test(field_id(FIELD_MULTI), selection, false);
        let before = source.clone();
        let bytes = template_snapshot_bytes(&source);
        let error = create(&source, "private-name")
            .expect_err("invalid choice default must fail source admission");
        assert_eq!(
            error.category(),
            DocumentCreationErrorCategory::InvalidSource
        );
        assert_eq!(
            error.artifact_validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
        );
        assert_eq!(error.artifact_choice_category(), Some(expected));
        assert_eq!(
            error.artifact_choice_location(),
            Some(ArtifactChoiceValueLocation::CurrentDefault)
        );
        assert_error_redacted(
            &error,
            &[
                OPTION_A,
                OPTION_B,
                OPTION_ARCHIVED,
                OPTION_UNKNOWN,
                "private-name",
            ],
        );
        assert_source_unchanged(&source, &before, &bytes);
    }
    Ok(())
}

#[test]
fn actual_wire_depth_proves_valid_source_candidate_relation_and_rejects_source_at_128(
) -> Result<(), Box<dyn Error>> {
    let raw = template_value(
        [(
            FIELD_RICH,
            field(
                "richText",
                "active",
                false,
                rich_text("depth"),
                json!({"kind":"unset"}),
                basic_configuration("richText"),
            ),
        )],
        &[FIELD_RICH],
        1,
        "active",
    );
    let shallow = decode_template_value(&raw);
    let mut at_127 = None;
    let mut at_128 = None;
    for nested_depth in 1..=MAX_JSON_NESTING_DEPTH {
        let mut candidate = shallow.clone();
        candidate.corrupt_rich_text_default_depth_for_test(
            field_id(FIELD_RICH),
            nested_array(nested_depth, Value::String("depth-canary".to_owned())),
            false,
        );
        match maximum_container_depth(
            &serde_json::to_value(TemplateWire::from(&candidate))
                .expect("test-only Template projection must serialize"),
        ) {
            MAX_JSON_NESTING_DEPTH => at_127 = Some(candidate),
            depth if depth == MAX_JSON_NESTING_DEPTH + 1 => {
                at_128 = Some(candidate);
                break;
            }
            _ => {}
        }
    }

    let source = at_127.expect("fixture search must find actual whole-wire depth 127");
    source.validate_storage()?;
    let source_before = source.clone();
    let source_bytes = template_snapshot_bytes(&source);
    let document = create(&source, "depth")?.into_document();
    let document_depth = maximum_container_depth(&document_wire_value(&document));
    assert!(
        document_depth < 10,
        "Template default depth must not enter a blank document"
    );
    document.validate_storage()?;
    let encoded = encode_document(&document)?;
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    assert_source_unchanged(&source, &source_before, &source_bytes);

    let too_deep = at_128.expect("fixture search must find actual whole-wire depth 128");
    let too_deep_before = too_deep.clone();
    let too_deep_bytes = unchecked_template_snapshot_bytes(&too_deep);
    let error = create(&too_deep, "depth")
        .expect_err("depth-128 source must fail before candidate construction");
    assert_eq!(
        error.category(),
        DocumentCreationErrorCategory::InvalidSource
    );
    assert_eq!(
        error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_error_redacted(&error, &["depth-canary"]);
    assert_eq!(too_deep, too_deep_before);
    assert_eq!(unchecked_template_snapshot_bytes(&too_deep), too_deep_bytes);
    Ok(())
}

#[test]
fn creation_does_not_transfer_legacy_default_number_provenance() -> Result<(), Box<dyn Error>> {
    let mut text_default = json!({
        "kind":"text",
        "value":"created text",
        "futureOuter":{"nested":[0,{"value":0}]}
    });
    text_default["futureOuter"]["nested"][0] = json!("__TEXT_EXPONENT__");
    text_default["futureOuter"]["nested"][1]["value"] = json!("__TEXT_ZERO__");

    let mut rich_default = rich_text("created rich");
    rich_default["document"]["futureEnvelope"] = json!({"number":"__RICH_ENVELOPE__"});
    rich_default["document"]["content"]["children"][0]["futureBlock"] =
        json!({"number":"__RICH_BLOCK__"});
    rich_default["document"]["content"]["children"][0]["children"][0]["futureNode"] =
        json!({"number":"__RICH_NODE__"});

    let mut archived_default = json!({"kind":"text","value":"archived"});
    archived_default["archivedNumber"] = json!("__ARCHIVED_NUMBER__");
    let mut raw = template_value(
        [
            (
                FIELD_TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    text_default,
                    json!({"kind":"unset"}),
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                FIELD_RICH,
                field(
                    "richText",
                    "active",
                    false,
                    rich_default,
                    json!({"kind":"unset"}),
                    basic_configuration("richText"),
                ),
            ),
            (
                FIELD_ARCHIVED,
                field(
                    "singleLineText",
                    "archived",
                    false,
                    archived_default.clone(),
                    archived_default,
                    basic_configuration("singleLineText"),
                ),
            ),
        ],
        &[FIELD_TEXT, FIELD_RICH],
        1,
        "active",
    );
    raw["templateOnlyNumber"] = json!("__TEMPLATE_NUMBER__");
    let mut raw = String::from_utf8(to_deterministic_json_bytes(&raw)?)?;
    for (marker, token) in [
        ("__TEXT_EXPONENT__", "1E9223372036854775808"),
        ("__TEXT_ZERO__", "-0E-999999999999999999999"),
        ("__RICH_ENVELOPE__", "1e+0009223372036854775808"),
        ("__RICH_BLOCK__", "7E-100"),
        ("__RICH_NODE__", "6e+100"),
        ("__ARCHIVED_NUMBER__", "5E100"),
        ("__TEMPLATE_NUMBER__", "4E100"),
    ] {
        raw = raw.replace(&format!("\"{marker}\""), token);
    }
    let source = decode_template(raw.as_bytes())?;
    let source_before = source.clone();
    let source_bytes = encode_template(&source)?;

    let document = create(&source, "created")?.into_document();
    let encoded = encode_document(&document)?;
    let text = std::str::from_utf8(&encoded)?;
    for token in [
        "1E9223372036854775808",
        "-0E-999999999999999999999",
        "1e+0009223372036854775808",
        "7E-100",
        "6e+100",
    ] {
        assert!(
            !text.contains(token),
            "legacy default was injected: {token}"
        );
    }
    assert!(!text.contains("templateOnlyNumber"));
    assert!(!text.contains("archivedNumber"));
    let wire: Value = serde_json::from_slice(&encoded)?;
    assert_eq!(wire["fieldValues"][FIELD_ARCHIVED], json!({"kind":"unset"}));
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    assert_eq!(source, source_before);
    assert_eq!(encode_template(&source)?, source_bytes);
    Ok(())
}

#[test]
fn trusted_artifact_provenance_rejects_a_different_template_owner() -> Result<(), Box<dyn Error>> {
    const OTHER_TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000002";
    let definition = field(
        "singleLineText",
        "active",
        false,
        json!({"kind":"text","value":"same","future":"__NUMBER__"}),
        json!({"kind":"unset"}),
        basic_configuration("singleLineText"),
    );
    let source_value = template_value(
        [(FIELD_TEXT, definition.clone())],
        &[FIELD_TEXT],
        1,
        "active",
    );
    let source_raw = String::from_utf8(to_deterministic_json_bytes(&source_value)?)?
        .replace("\"__NUMBER__\"", "1E100");
    let source = decode_template(source_raw.as_bytes())?;

    let mut destination_value =
        template_value([(FIELD_TEXT, definition)], &[FIELD_TEXT], 1, "active");
    destination_value["templateId"] = json!(OTHER_TEMPLATE_ID);
    let destination = decode_template_value(&destination_value);
    let mut document = create(&destination, "different owner")?.into_document();
    let before = encode_document(&document)?;
    let provenance = source
        .current_default_provenance(field_id(FIELD_TEXT))
        .expect("decoded source owns current default provenance");

    assert!(!document.graft_current_default_provenance(provenance));
    assert_eq!(encode_document(&document)?, before);

    for mutation in ["missing", "different"] {
        let mut candidate = create(&source, "same owner")?.into_document();
        let field_id = field_id(FIELD_TEXT);
        if mutation == "missing" {
            candidate.field_values.remove(&field_id);
        } else {
            candidate.field_values.insert(field_id, FieldValue::unset());
        }
        let carrier_before = serde_json::to_string(&candidate.lossless_source())?;
        let provenance = source
            .current_default_provenance(field_id)
            .expect("decoded source owns current default provenance");
        assert!(!candidate.graft_current_default_provenance(provenance));
        assert_eq!(
            serde_json::to_string(&candidate.lossless_source())?,
            carrier_before
        );
    }
    Ok(())
}

#[test]
fn failed_artifact_rebase_keeps_the_owned_source_atomic() -> Result<(), Box<dyn Error>> {
    let source = decode_template_value(&template_value(
        [(
            FIELD_TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"same","future":1E100}),
                json!({"kind":"unset"}),
                basic_configuration("singleLineText"),
            ),
        )],
        &[FIELD_TEXT],
        1,
        "active",
    ));
    let document = create(&source, "atomic source")?.into_document();

    for invalid_extra in [
        (
            "$serde_json::private::forbidden".to_owned(),
            json!("reserved"),
        ),
        (
            "futureDepth".to_owned(),
            nested_array(MAX_JSON_NESTING_DEPTH, json!("depth")),
        ),
    ] {
        let mut candidate = document.clone();
        let before = serde_json::to_string(&candidate.lossless_source())?;
        candidate.extra.insert(invalid_extra.0, invalid_extra.1);
        assert!(candidate.rebase_lossless_source().is_err());
        assert_eq!(serde_json::to_string(&candidate.lossless_source())?, before);
    }
    Ok(())
}
