use std::{error::Error, str::FromStr};

use serde_json::{json, Map, Value};

use super::*;
use crate::data::{
    artifact::{
        decode_document, decode_template, document::DocumentWire, encode_document, encode_template,
        template::TemplateWire, ArtifactRichTextValueLocation, ArtifactScalarValueLocation,
        DocumentId, FieldKind, OptionId,
    },
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        rich_text::{RichTextErrorLocation, RichTextValidationErrorCategory},
        scalar::ScalarValueErrorCategory,
        validation::{FieldValidationErrorCategory, FieldValidationLocation},
    },
    json::{to_deterministic_json_bytes, MAX_JSON_NESTING_DEPTH},
};

const TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000001";
const OTHER_TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000002";
const DOCUMENT_ID: &str = "90000000-0000-4000-8000-000000000001";
const CREATED_AT: &str = "2026-09-01T00:00:00.000Z";
const UPDATED_AT: &str = "2026-09-02T00:00:00.000Z";
const LATER_AT: &str = "2026-09-03T00:00:00.000Z";

const TEXT: &str = "20000000-0000-4000-8000-000000000001";
const RICH: &str = "20000000-0000-4000-8000-000000000002";
const NUMBER: &str = "20000000-0000-4000-8000-000000000003";
const DATE: &str = "20000000-0000-4000-8000-000000000004";
const TIME: &str = "20000000-0000-4000-8000-000000000005";
const DURATION: &str = "20000000-0000-4000-8000-000000000006";
const SINGLE: &str = "20000000-0000-4000-8000-000000000007";
const MULTI: &str = "20000000-0000-4000-8000-000000000008";
const ARCHIVED: &str = "20000000-0000-4000-8000-000000000009";
const ORPHAN: &str = "20000000-0000-4000-8000-00000000000a";
const ORPHAN_OTHER: &str = "20000000-0000-4000-8000-00000000000b";

const SINGLE_A: &str = "a0000000-0000-4000-8000-000000000001";
const SINGLE_B: &str = "a0000000-0000-4000-8000-000000000002";
const SINGLE_ARCHIVED: &str = "a0000000-0000-4000-8000-000000000003";
const MULTI_A: &str = "b0000000-0000-4000-8000-000000000001";
const MULTI_B: &str = "b0000000-0000-4000-8000-000000000002";
const MULTI_ARCHIVED: &str = "b0000000-0000-4000-8000-000000000003";
const UNKNOWN_OPTION: &str = "c0000000-0000-4000-8000-000000000001";
const UNKNOWN_OPTION_TWO: &str = "c0000000-0000-4000-8000-000000000002";
const UNKNOWN_OPTION_THREE: &str = "c0000000-0000-4000-8000-000000000003";
const EXPONENT_NUMBER: &str = "9.87654321098765432109876543210987654321e+210";
const FIELD_INTEGER_NUMBER: &str = "123456789012345678901234567890";
const ORPHAN_INTEGER_NUMBER: &str = "123456789012345678901234567891";
const ORPHAN_DECIMAL_NUMBER: &str = "12345678901234567890.0002";
const DOCUMENT_DECIMAL_NUMBER: &str = "987654321098765432109876543210.0001";
const INITIAL_RICH_PAYLOAD: &str = "초기 rich";
const BASE_SCALAR_PAYLOAD: &str = "private scalar payload";
const INVALID_SCALAR_PAYLOAD: &str = "private\nscalar payload";
const INVALID_SCALAR_ESCAPED: &str = "private\\nscalar payload";
const WINDOWS_CANARY: &str = r"C:\M2_5D\redaction-only\secret.txt";
const UNIX_CANARY: &str = "/srv/m2_5d/redaction-only/secret.txt";
const RICH_WINDOWS_CANARY: &str = r"C:\M2_5D\secret.txt";

fn revision(value: u32) -> TemplateRevision {
    TemplateRevision::try_from(value).expect("fixture revision must be non-zero")
}

fn field_id(value: &str) -> FieldId {
    FieldId::from_str(value).expect("fixture FieldId must be valid")
}

fn option_id(value: &str) -> OptionId {
    OptionId::from_str(value).expect("fixture OptionId must be valid")
}

fn exact_number(value: &str) -> Value {
    serde_json::from_str(value).expect("fixture number must use a valid JSON lexeme")
}

fn basic_configuration(kind: &str) -> Value {
    json!({"kind":kind})
}

fn single_configuration() -> Value {
    json!({
        "kind":"singleChoice",
        "optionOrder":[SINGLE_A,SINGLE_B],
        "options":{
            (SINGLE_A):{"label":"single-a","lifecycle":"active"},
            (SINGLE_B):{"label":"single-b","lifecycle":"active"},
            (SINGLE_ARCHIVED):{"label":"credential=single-archived","lifecycle":"archived"}
        }
    })
}

fn multi_configuration() -> Value {
    json!({
        "kind":"multiChoice",
        "optionOrder":[MULTI_B,MULTI_A],
        "options":{
            (MULTI_A):{"label":"multi-a","lifecycle":"active"},
            (MULTI_B):{"label":"multi-b","lifecycle":"active"},
            (MULTI_ARCHIVED):{"label":"credential=multi-archived","lifecycle":"archived"}
        }
    })
}

fn rich_text(text: &str, metadata: bool) -> Value {
    let mut value = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{
                "kind":"root",
                "children":[{
                    "kind":"paragraph",
                    "children":[
                        {"kind":"text","text":text,"marks":["bold"]},
                        {"kind":"hardBreak"},
                        {"kind":"text","text":"끝"}
                    ]
                }]
            }
        }
    });
    if metadata {
        value["futureValueOuter"] = json!({"credential":"outer-secret"});
        value["document"]["futureEnvelope"] =
            json!({"decimal":exact_number("12345678901234567890.0001")});
        value["document"]["content"]["futureRoot"] = json!([{"nested":[1,2,3]}]);
        value["document"]["content"]["children"][0]["futureContainer"] =
            json!({"path":r"C:\M2_5D\secret.txt"});
        value["document"]["content"]["children"][0]["children"][0]["futureText"] =
            json!({"url":"https://example.invalid/private?credential=rich"});
    }
    value
}

fn field(
    kind: &str,
    lifecycle: &str,
    required: bool,
    current: Value,
    initial: Value,
    introduced_revision: u32,
    configuration: Value,
) -> Value {
    json!({
        "label":format!("credential={kind}-label"),
        "lifecycle":lifecycle,
        "kind":kind,
        "required":required,
        "defaultValue":current,
        "initialDefaultValue":initial,
        "introducedRevision":introduced_revision,
        "configuration":configuration,
        "presentation":{"token":format!("private-{kind}-token")}
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
        .map(|(id, value)| (id.to_owned(), value))
        .collect::<Map<_, _>>();
    json!({
        "schemaVersion":1,
        "artifactType":"template",
        "templateId":TEMPLATE_ID,
        "revision":revision,
        "name":"credential=template-name",
        "lifecycle":lifecycle,
        "fieldOrder":active_order,
        "fields":fields,
        "presentation":{"token":"private-template-token"},
        "createdAtUtc":CREATED_AT,
        "updatedAtUtc":UPDATED_AT,
        "futureTemplate":{"path":"/home/private/template.json"}
    })
}

fn document_value(
    field_values: impl IntoIterator<Item = (&'static str, Value)>,
    snapshots: impl IntoIterator<Item = (&'static str, Value)>,
    template_revision: u32,
    template_id: &str,
) -> Value {
    let field_values = field_values
        .into_iter()
        .map(|(id, value)| (id.to_owned(), value))
        .collect::<Map<_, _>>();
    let snapshots = snapshots
        .into_iter()
        .map(|(id, value)| (id.to_owned(), value))
        .collect::<Map<_, _>>();
    let mut value = json!({
        "schemaVersion":1,
        "artifactType":"document",
        "documentId":DOCUMENT_ID,
        "templateId":template_id,
        "templateRevision":template_revision,
        "name":"credential=document-name",
        "fieldValues":field_values,
        "orphanedFieldDefinitions":snapshots,
        "createdAtUtc":CREATED_AT,
        "updatedAtUtc":UPDATED_AT,
        "futureDocument":{
            "url":"https://example.invalid/document?credential=secret",
            "windows":r"C:\M2_5D\document.json",
            "unix":"/home/private/document.json",
            "number":0
        }
    });
    value["futureDocument"]["number"] = exact_number("987654321098765432109876543210.0001");
    value
}

fn orphan_snapshot(label: &str, kind: &str, options: Value) -> Value {
    json!({"label":label,"kind":kind,"options":options})
}

fn decode_template_value(value: &Value) -> TemplateArtifact {
    decode_template(&to_deterministic_json_bytes(value).expect("Template fixture must encode"))
        .expect("Template fixture must decode")
}

fn decode_document_value(value: &Value) -> DocumentArtifact {
    decode_document(&to_deterministic_json_bytes(value).expect("Document fixture must encode"))
        .expect("Document fixture must decode")
}

fn template_bytes(template: &TemplateArtifact) -> Vec<u8> {
    to_deterministic_json_bytes(&TemplateWire::from(template))
        .expect("Template snapshot must encode")
}

fn document_bytes(document: &DocumentArtifact) -> Vec<u8> {
    to_deterministic_json_bytes(&DocumentWire::from(document))
        .expect("Document snapshot must encode")
}

fn document_wire_value(document: &DocumentArtifact) -> Value {
    serde_json::to_value(DocumentWire::from(document)).expect("Document wire must serialize")
}

fn template_wire_value(template: &TemplateArtifact) -> Value {
    serde_json::to_value(TemplateWire::from(template)).expect("Template wire must serialize")
}

fn changed(outcome: DocumentMaterializationOutcome) -> DocumentArtifact {
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    let changed = outcome
        .into_changed()
        .expect("Changed outcome must own a candidate and warnings");
    assert!(
        changed.warnings().is_empty(),
        "helper only accepts warning-free outcomes"
    );
    changed.document
}

fn assert_sources_unchanged(
    template: &TemplateArtifact,
    template_before: &TemplateArtifact,
    template_snapshot: &[u8],
    document: &DocumentArtifact,
    document_before: &DocumentArtifact,
    document_snapshot: &[u8],
) {
    assert_eq!(template, template_before);
    assert_eq!(document, document_before);
    assert_eq!(template_bytes(template), template_snapshot);
    assert_eq!(document_bytes(document), document_snapshot);
}

fn assert_error_redacted(error: &DocumentMaterializationError, forbidden: &[&str]) {
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert_rendering_redacted(&rendered, forbidden, "materialization error");
    }
    assert!(error.source().is_none());
}

fn assert_base_scalar_redacted(error: &DocumentMaterializationError) {
    assert_error_redacted(error, &[BASE_SCALAR_PAYLOAD, "field-value-secret"]);
}

fn first_redaction_leak<'a>(rendered: &str, forbidden: &'a [&str]) -> Option<&'a str> {
    forbidden
        .iter()
        .copied()
        .find(|fragment| rendered.contains(fragment))
}

fn assert_rendering_redacted(rendered: &str, forbidden: &[&str], context: &str) {
    if let Some(fragment) = first_redaction_leak(rendered, forbidden) {
        panic!("{context} leaked {fragment}");
    }
}

fn assert_redaction_fragments_are_observable(forbidden: &[&str]) {
    for fragment in forbidden {
        assert_eq!(
            first_redaction_leak(fragment, &[*fragment]),
            Some(*fragment),
            "leak predicate must identify the exact supplied canary"
        );
    }
    assert_eq!(
        first_redaction_leak(
            "DocumentMaterializationWarning { count: 1, truncated: false }",
            forbidden,
        ),
        None,
        "safe category/count/boolean/type information must remain renderable"
    );
}

#[test]
fn redaction_leak_predicate_detects_raw_and_escaped_multiline_scalar_exactly() {
    assert_redaction_fragments_are_observable(&[INVALID_SCALAR_PAYLOAD, INVALID_SCALAR_ESCAPED]);
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

fn historical_template_value(revision: u32) -> Value {
    let fields = [
        (
            TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"current text"}),
                json!({"kind":"text","value":"  초기 값 Ω  ","futureText":{"n":exact_number(FIELD_INTEGER_NUMBER)}}),
                2,
                basic_configuration("singleLineText"),
            ),
        ),
        (
            RICH,
            field(
                "richText",
                "active",
                false,
                rich_text("current rich", false),
                rich_text(INITIAL_RICH_PAYLOAD, true),
                2,
                basic_configuration("richText"),
            ),
        ),
        (
            NUMBER,
            field(
                "number",
                "active",
                false,
                json!({"kind":"number","value":"1"}),
                json!({"kind":"number","value":"123456789012345678901234567890.0001"}),
                2,
                basic_configuration("number"),
            ),
        ),
        (
            DATE,
            field(
                "date",
                "active",
                false,
                json!({"kind":"date","value":"2026-09-05"}),
                json!({"kind":"date","value":"2025-01-02"}),
                2,
                basic_configuration("date"),
            ),
        ),
        (
            TIME,
            field(
                "time",
                "active",
                false,
                json!({"kind":"time","value":"12:34:00.000"}),
                json!({"kind":"time","value":"01:02:03.004"}),
                2,
                basic_configuration("time"),
            ),
        ),
        (
            DURATION,
            field(
                "duration",
                "active",
                false,
                json!({"kind":"duration","milliseconds":"1"}),
                json!({"kind":"duration","milliseconds":"1234567890123456789"}),
                2,
                basic_configuration("duration"),
            ),
        ),
        (
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"singleChoice","optionId":SINGLE_B}),
                2,
                single_configuration(),
            ),
        ),
        (
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                json!({"kind":"multiChoice","optionIds":[MULTI_A,MULTI_B]}),
                2,
                multi_configuration(),
            ),
        ),
    ];
    template_value(
        fields,
        &[TEXT, RICH, NUMBER, DATE, TIME, DURATION, SINGLE, MULTI],
        revision,
        "active",
    )
}

fn multi_materialization_values(reverse: bool) -> (Value, Value) {
    let mut fields = vec![
        (
            TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"current text"}),
                json!({"kind":"text","value":"historical text","futureText":{"nested":[3,2,1]}}),
                2,
                basic_configuration("singleLineText"),
            ),
        ),
        (
            RICH,
            field(
                "richText",
                "active",
                false,
                rich_text("current rich", false),
                rich_text("historical rich", true),
                2,
                basic_configuration("richText"),
            ),
        ),
        (
            NUMBER,
            field(
                "number",
                "active",
                false,
                json!({"kind":"number","value":"1"}),
                json!({
                    "kind":"number",
                    "value":"12345678901234567890.0001",
                    "futureExponent":exact_number(EXPONENT_NUMBER)
                }),
                2,
                basic_configuration("number"),
            ),
        ),
        (
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"unset"}),
                1,
                single_configuration(),
            ),
        ),
        (
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                json!({"kind":"unset"}),
                1,
                multi_configuration(),
            ),
        ),
        (
            ARCHIVED,
            field(
                "singleLineText",
                "archived",
                true,
                json!({"kind":"text","value":"archived current"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("singleLineText"),
            ),
        ),
        (
            DURATION,
            field(
                "duration",
                "active",
                false,
                json!({"kind":"duration","milliseconds":"1"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("duration"),
            ),
        ),
        (
            TIME,
            field(
                "time",
                "active",
                false,
                json!({"kind":"time","value":"12:00:00.000"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("time"),
            ),
        ),
    ];
    let mut active_order = vec![TIME, NUMBER, SINGLE, TEXT, DURATION, RICH, MULTI];
    let mut field_values = vec![
        (
            SINGLE,
            json!({
                "kind":"singleChoice",
                "optionId":SINGLE_ARCHIVED,
                "futureValue":{"nested":["single",2,1]}
            }),
        ),
        (
            MULTI,
            json!({"kind":"unset","futureUnset":{"nested":[3,1,2]}}),
        ),
        (
            ARCHIVED,
            json!({"kind":"text","value":"archived existing","futureValue":"keep"}),
        ),
        (
            DURATION,
            json!({"kind":"duration","milliseconds":"987654321","futureValue":"existing"}),
        ),
        (TIME, json!({"kind":"unset","futureUnset":"existing-unset"})),
        (
            ORPHAN,
            json!({
                "kind":"multiChoice",
                "optionIds":[UNKNOWN_OPTION,UNKNOWN_OPTION_TWO,UNKNOWN_OPTION_THREE],
                "futureValue":{"nested":[{"order":[3,1,2]}]}
            }),
        ),
    ];
    let mut snapshots = vec![
        (
            SINGLE,
            orphan_snapshot(
                "reattach single",
                "singleChoice",
                json!({(SINGLE_ARCHIVED):{"label":"archived selected"}}),
            ),
        ),
        (
            MULTI,
            orphan_snapshot("reattach unset multi", "multiChoice", json!({})),
        ),
        (
            ARCHIVED,
            orphan_snapshot("reattach archived", "singleLineText", json!({})),
        ),
        (
            ORPHAN,
            json!({
                "label":"unknown choice orphan",
                "kind":"multiChoice",
                "options":{
                    (UNKNOWN_OPTION):{
                        "label":"unknown option one",
                        "futureOption":{"nested":[1,{"integer":123456789012345678901234567890u128}]}
                    },
                    (UNKNOWN_OPTION_TWO):{
                        "label":"unknown option two",
                        "futureOption":{"nested":[2,{"decimal":exact_number("12345678901234567890.0001")}]}
                    },
                    (UNKNOWN_OPTION_THREE):{
                        "label":"unknown option three",
                        "futureOption":{"nested":[3,{"exponent":exact_number(EXPONENT_NUMBER)}]}
                    }
                },
                "futureSnapshot":{"array":[3,1,2]}
            }),
        ),
    ];
    if reverse {
        fields.reverse();
        active_order.reverse();
        field_values.reverse();
        snapshots.reverse();
    }
    (
        template_value(fields, &active_order, 3, "active"),
        document_value(field_values, snapshots, 1, TEMPLATE_ID),
    )
}

fn raw_order_document_bytes(reverse: bool) -> Vec<u8> {
    let text_value = format!(r#""{TEXT}":{{"kind":"text","value":"reattached value"}}"#);
    let orphan_value = format!(
        r#""{ORPHAN}":{{"kind":"text","value":"preserved orphan","futureValueOuter":{{"array":[3,1,2]}}}}"#
    );
    let field_values = if reverse {
        format!("{orphan_value},{text_value}")
    } else {
        format!("{text_value},{orphan_value}")
    };

    let text_snapshot = format!(
        r#""{TEXT}":{{"label":"metadata-free reattachment","kind":"singleLineText","options":{{}}}}"#
    );
    let orphan_snapshot = format!(
        r#""{ORPHAN}":{{"label":"preserved orphan snapshot","kind":"singleLineText","options":{{}},"futureSnapshot":{{"array":[3,1,2]}}}}"#
    );
    let snapshots = if reverse {
        format!("{orphan_snapshot},{text_snapshot}")
    } else {
        format!("{text_snapshot},{orphan_snapshot}")
    };
    let nested_extra = if reverse {
        format!(r#""rawOmega":[3,1,2],"rawNumber":{EXPONENT_NUMBER},"rawAlpha":"first""#)
    } else {
        format!(r#""rawAlpha":"first","rawNumber":{EXPONENT_NUMBER},"rawOmega":[3,1,2]"#)
    };

    format!(
        concat!(
            r#"{{"schemaVersion":1,"artifactType":"document","documentId":"{DOCUMENT_ID}","#,
            r#""templateId":"{TEMPLATE_ID}","templateRevision":1,"name":"raw order source","#,
            r#""fieldValues":{{{field_values}}},"orphanedFieldDefinitions":{{{snapshots}}},"createdAtUtc":"{CREATED_AT}","#,
            r#""updatedAtUtc":"{UPDATED_AT}","futureRawOrder":{{{nested_extra}}}}}"#
        ),
        DOCUMENT_ID = DOCUMENT_ID,
        TEMPLATE_ID = TEMPLATE_ID,
        field_values = field_values,
        snapshots = snapshots,
        CREATED_AT = CREATED_AT,
        UPDATED_AT = UPDATED_AT,
        nested_extra = nested_extra,
    )
    .into_bytes()
}

fn assert_raw_member_precedes(raw: &[u8], first: &str, second: &str) {
    let raw = std::str::from_utf8(raw).expect("raw-order fixture must be UTF-8");
    let first_position = raw.find(first).expect("first member must exist");
    let second_position = raw.find(second).expect("second member must exist");
    assert!(
        first_position < second_position,
        "{first} must precede {second} in the raw wire fixture"
    );
}

fn assert_object_member_precedes(raw: &[u8], object: &str, first: &str, second: &str) {
    let raw = std::str::from_utf8(raw).expect("raw-order fixture must be UTF-8");
    let object_marker = format!("\"{object}\":{{");
    let object_start = raw
        .find(&object_marker)
        .expect("target object must exist in the raw fixture");
    let object_members = &raw[object_start + object_marker.len()..];
    let first_marker = format!("\"{first}\":");
    let second_marker = format!("\"{second}\":");
    let first_position = object_members
        .find(&first_marker)
        .expect("first object member must exist");
    let second_position = object_members
        .find(&second_marker)
        .expect("second object member must exist");
    assert!(
        first_position < second_position,
        "{first} must precede {second} inside {object}"
    );
}

fn full_redaction_values() -> (Value, Value) {
    let mut template = historical_template_value(3);
    template["futureTemplate"] = json!({
        "credential":"template-root-secret",
        "windows":WINDOWS_CANARY
    });
    template["fields"][TEXT]["futureField"] = json!({
        "integer":exact_number(FIELD_INTEGER_NUMBER),
        "credential":"field-definition-secret"
    });
    template["fields"][TEXT]["presentation"]["futurePresentation"] = json!({"unix":UNIX_CANARY});
    template["fields"][TEXT]["configuration"]["futureConfiguration"] =
        json!({"exponent":exact_number(EXPONENT_NUMBER)});
    template["fields"][SINGLE]["configuration"]["options"][SINGLE_A]["futureKnownOption"] =
        json!({"url":"https://example.invalid/known?credential=option"});

    let mut document = document_value(
        [
            (
                TEXT,
                json!({
                    "kind":"text",
                    "value":BASE_SCALAR_PAYLOAD,
                    "futureValueOuter":{"credential":"field-value-secret"}
                }),
            ),
            (
                ORPHAN,
                json!({
                    "kind":"multiChoice",
                    "optionIds":[UNKNOWN_OPTION,UNKNOWN_OPTION_TWO,UNKNOWN_OPTION_THREE],
                    "futureValueOuter":{"nested":[3,1,2]}
                }),
            ),
        ],
        [(
            ORPHAN,
            json!({
                "label":"credential=orphan-field-label",
                "kind":"multiChoice",
                "options":{
                    (UNKNOWN_OPTION):{
                        "label":"credential=orphan-option-one",
                        "futureOption":{"integer":exact_number(ORPHAN_INTEGER_NUMBER)}
                    },
                    (UNKNOWN_OPTION_TWO):{
                        "label":"credential=orphan-option-two",
                        "futureOption":{"decimal":exact_number(ORPHAN_DECIMAL_NUMBER)}
                    },
                    (UNKNOWN_OPTION_THREE):{
                        "label":"credential=orphan-option-three",
                        "futureOption":{"exponent":exact_number(EXPONENT_NUMBER)}
                    }
                },
                "futureSnapshot":{"url":"https://example.invalid/orphan?credential=snapshot"}
            }),
        )],
        1,
        TEMPLATE_ID,
    );
    document["futureDocument"] = json!({
        "credential":"document-root-secret",
        "windows":WINDOWS_CANARY,
        "unix":UNIX_CANARY,
        "number":exact_number(DOCUMENT_DECIMAL_NUMBER)
    });
    (template, document)
}

fn assert_full_redaction_canaries(template: &Value, document: &Value) {
    assert_eq!(
        template["futureTemplate"]["credential"],
        json!("template-root-secret")
    );
    assert_eq!(
        document["futureDocument"]["credential"],
        json!("document-root-secret")
    );
    assert_eq!(
        template["fields"][TEXT]["futureField"]["credential"],
        json!("field-definition-secret")
    );
    assert_eq!(
        template["fields"][TEXT]["futureField"]["integer"]
            .as_number()
            .expect("Field metadata integer must be a Number")
            .to_string(),
        FIELD_INTEGER_NUMBER
    );
    assert_eq!(
        document["fieldValues"][TEXT]["futureValueOuter"]["credential"],
        json!("field-value-secret")
    );
    assert_eq!(
        document["fieldValues"][TEXT]["value"],
        json!(BASE_SCALAR_PAYLOAD)
    );
    assert_eq!(
        template["fields"][SINGLE]["configuration"]["options"][SINGLE_A]["futureKnownOption"]
            ["url"],
        json!("https://example.invalid/known?credential=option")
    );
    assert_eq!(
        template["fields"][SINGLE]["configuration"]["options"][SINGLE_A]["label"],
        json!("single-a")
    );
    assert_eq!(
        template["fields"][TEXT]["presentation"]["token"],
        json!("private-singleLineText-token")
    );
    assert_eq!(
        template["fields"][TEXT]["presentation"]["futurePresentation"]["unix"],
        json!(UNIX_CANARY)
    );
    assert_eq!(
        template["fields"][TEXT]["configuration"]["futureConfiguration"]["exponent"]
            .as_number()
            .expect("configuration exponent must be a Number")
            .to_string(),
        EXPONENT_NUMBER
    );
    let rich = &template["fields"][RICH]["initialDefaultValue"];
    assert_eq!(
        rich["document"]["content"]["children"][0]["children"][0]["text"],
        json!(INITIAL_RICH_PAYLOAD)
    );
    assert_eq!(
        rich["futureValueOuter"]["credential"],
        json!("outer-secret")
    );
    assert_eq!(
        rich["document"]["futureEnvelope"]["decimal"]
            .as_number()
            .expect("rich-text decimal must be a Number")
            .to_string(),
        "12345678901234567890.0001"
    );
    assert!(rich["document"]["content"]["futureRoot"].is_array());
    assert_eq!(
        rich["document"]["content"]["children"][0]["futureContainer"]["path"],
        json!(RICH_WINDOWS_CANARY)
    );
    assert_eq!(
        rich["document"]["content"]["children"][0]["children"][0]["futureText"]["url"],
        json!("https://example.invalid/private?credential=rich")
    );
    assert_eq!(
        document["orphanedFieldDefinitions"][ORPHAN]["label"],
        json!("credential=orphan-field-label")
    );
    assert_eq!(
        document["orphanedFieldDefinitions"][ORPHAN]["futureSnapshot"]["url"],
        json!("https://example.invalid/orphan?credential=snapshot")
    );
    for (option, label) in [
        (UNKNOWN_OPTION, "credential=orphan-option-one"),
        (UNKNOWN_OPTION_TWO, "credential=orphan-option-two"),
        (UNKNOWN_OPTION_THREE, "credential=orphan-option-three"),
    ] {
        assert_eq!(
            document["orphanedFieldDefinitions"][ORPHAN]["options"][option]["label"],
            json!(label)
        );
    }
    assert_eq!(
        document["orphanedFieldDefinitions"][ORPHAN]["options"][UNKNOWN_OPTION]["futureOption"]
            ["integer"]
            .as_number()
            .expect("orphan metadata integer must be a Number")
            .to_string(),
        ORPHAN_INTEGER_NUMBER
    );
    assert_eq!(
        document["orphanedFieldDefinitions"][ORPHAN]["options"][UNKNOWN_OPTION_TWO]["futureOption"]
            ["decimal"]
            .as_number()
            .expect("orphan metadata decimal must be a Number")
            .to_string(),
        ORPHAN_DECIMAL_NUMBER
    );
    assert_eq!(
        document["orphanedFieldDefinitions"][ORPHAN]["options"][UNKNOWN_OPTION_THREE]
            ["futureOption"]["exponent"]
            .as_number()
            .expect("orphan exponent must be a Number")
            .to_string(),
        EXPONENT_NUMBER
    );
    assert_eq!(
        document["fieldValues"][ORPHAN]["optionIds"],
        json!([UNKNOWN_OPTION, UNKNOWN_OPTION_TWO, UNKNOWN_OPTION_THREE])
    );
    assert_eq!(document["futureDocument"]["windows"], json!(WINDOWS_CANARY));
    assert_eq!(document["futureDocument"]["unix"], json!(UNIX_CANARY));
    assert_eq!(
        document["futureDocument"]["number"]
            .as_number()
            .expect("Document metadata decimal must be a Number")
            .to_string(),
        DOCUMENT_DECIMAL_NUMBER
    );

    let template_bytes =
        to_deterministic_json_bytes(template).expect("Template canaries must encode");
    let document_bytes =
        to_deterministic_json_bytes(document).expect("Document canaries must encode");
    for lexeme in [FIELD_INTEGER_NUMBER, EXPONENT_NUMBER] {
        assert!(template_bytes
            .windows(lexeme.len())
            .any(|window| window == lexeme.as_bytes()));
    }
    for lexeme in [
        ORPHAN_INTEGER_NUMBER,
        ORPHAN_DECIMAL_NUMBER,
        DOCUMENT_DECIMAL_NUMBER,
        EXPONENT_NUMBER,
    ] {
        assert!(document_bytes
            .windows(lexeme.len())
            .any(|window| window == lexeme.as_bytes()));
    }
}

#[test]
fn historical_absence_materializes_all_eight_initial_defaults_losslessly_and_is_idempotent(
) -> Result<(), Box<dyn Error>> {
    let raw_template = historical_template_value(3);
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let outcome = materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert!(outcome.warnings().is_empty());
    let candidate = outcome.document().expect("Changed must expose candidate");
    assert_eq!(candidate.template_revision(), revision(3));
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    assert_eq!(candidate.created_at_utc(), CREATED_AT);
    assert_eq!(candidate.document_id(), document.document_id());
    assert_eq!(candidate.template_id(), document.template_id());
    assert_eq!(candidate.name(), document.name());
    let candidate_wire = document_wire_value(candidate);
    for field in [TEXT, RICH, NUMBER, DATE, TIME, DURATION, SINGLE, MULTI] {
        assert_ne!(
            raw_template["fields"][field]["defaultValue"],
            raw_template["fields"][field]["initialDefaultValue"]
        );
        assert_eq!(
            candidate_wire["fieldValues"][field],
            raw_template["fields"][field]["initialDefaultValue"]
        );
    }
    assert_eq!(
        candidate_wire["fieldValues"][MULTI]["optionIds"],
        json!([MULTI_A, MULTI_B])
    );
    assert_eq!(
        candidate_wire["fieldValues"][RICH]["document"]["content"]["children"][0]["children"][1]
            ["kind"],
        json!("hardBreak")
    );
    assert_eq!(
        candidate_wire["fieldValues"][RICH]["document"]["futureEnvelope"]["decimal"]
            .as_number()
            .expect("arbitrary precision metadata must remain a Number")
            .to_string(),
        "12345678901234567890.0001"
    );
    candidate.validate_storage()?;
    let encoded = encode_document(candidate)?;
    let decoded = decode_document(&encoded)?;
    assert_eq!(encode_document(&decoded)?, encoded);

    let same_inputs =
        materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(
        same_inputs.kind(),
        DocumentMaterializationOutcomeKind::Changed
    );
    assert_eq!(
        document_bytes(same_inputs.document().expect("deterministic candidate")),
        encoded
    );

    let second = materialize_document(&template, template.revision(), candidate, LATER_AT.into())?;
    assert_eq!(second.kind(), DocumentMaterializationOutcomeKind::Unchanged);
    assert!(second.document().is_none());
    assert!(second.warnings().is_empty());
    assert_eq!(document_bytes(candidate), encoded);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn no_op_revision_timestamp_and_max_revision_follow_fixed_precedence() -> Result<(), Box<dyn Error>>
{
    let current_template = decode_template_value(&template_value([], &[], 1, "active"));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let bytes_before = document_bytes(&document);

    let unchanged = materialize_document(
        &current_template,
        current_template.revision(),
        &document,
        LATER_AT.into(),
    )?;
    assert_eq!(
        unchanged.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(unchanged.document().is_none());
    assert_eq!(document.updated_at_utc(), UPDATED_AT);
    assert_eq!(document_bytes(&document), bytes_before);

    let stale = materialize_document(&current_template, revision(2), &document, LATER_AT.into())
        .expect_err("stale expected revision must beat certain no-op");
    assert_eq!(
        stale.category(),
        DocumentMaterializationErrorCategory::RevisionMismatch
    );

    for (timestamp, category) in [
        (
            "2026-09-03T00:00:00Z",
            DocumentMaterializationErrorCategory::InvalidTimestamp,
        ),
        (
            CREATED_AT,
            DocumentMaterializationErrorCategory::TimestampRegression,
        ),
    ] {
        let error = materialize_document(
            &current_template,
            current_template.revision(),
            &document,
            timestamp.into(),
        )
        .expect_err("timestamp validation must run even for no-op");
        assert_eq!(error.category(), category);
    }

    let max_template = decode_template_value(&template_value([], &[], u32::MAX, "active"));
    let changed = materialize_document(
        &max_template,
        max_template.revision(),
        &document,
        UPDATED_AT.into(),
    )?;
    let candidate = changed.document().expect("revision drift must change");
    assert_eq!(candidate.template_revision().get(), u32::MAX);
    assert_eq!(candidate.updated_at_utc(), UPDATED_AT);
    assert_eq!(candidate.created_at_utc(), CREATED_AT);
    let candidate_bytes = document_bytes(candidate);
    let repeated = materialize_document(
        &max_template,
        max_template.revision(),
        candidate,
        LATER_AT.into(),
    )?;
    assert_eq!(
        repeated.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert_eq!(document_bytes(candidate), candidate_bytes);
    Ok(())
}

#[test]
fn admission_errors_obey_source_revision_timestamp_binding_lifecycle_order(
) -> Result<(), Box<dyn Error>> {
    let raw_template = template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"text","value":"initial"}),
                1,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        2,
        "active",
    );
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&document_value(
        [(TEXT, json!({"kind":"text","value":"existing"}))],
        [],
        1,
        TEMPLATE_ID,
    ));
    let mut invalid_template = template.clone();
    invalid_template.corrupt_field_order_for_test();
    let mut invalid_document = document.clone();
    invalid_document.corrupt_scalar_value_for_test(field_id(TEXT), "invalid\nscalar");
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);
    let invalid_template_before = invalid_template.clone();
    let invalid_document_before = invalid_document.clone();
    let invalid_template_snapshot = template_bytes(&invalid_template);
    let invalid_document_snapshot = document_bytes(&invalid_document);

    let invalid_template_error = materialize_document(
        &invalid_template,
        revision(99),
        &invalid_document,
        "malformed".into(),
    )
    .expect_err("invalid Template must have first priority");
    assert_eq!(
        invalid_template_error.category(),
        DocumentMaterializationErrorCategory::InvalidTemplate
    );
    assert_eq!(
        invalid_template_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_eq!(invalid_template_error.artifact_scalar_category(), None);
    assert_error_redacted(
        &invalid_template_error,
        &[
            "credential=",
            "invalid\nscalar",
            r"C:\M2_5D",
            "/home/private",
        ],
    );

    let invalid_document_error = materialize_document(
        &template,
        revision(99),
        &invalid_document,
        "malformed".into(),
    )
    .expect_err("invalid Document must beat preconditions");
    assert_eq!(
        invalid_document_error.category(),
        DocumentMaterializationErrorCategory::InvalidDocument
    );
    assert_eq!(
        invalid_document_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        invalid_document_error.artifact_scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        invalid_document_error.artifact_scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    assert_error_redacted(
        &invalid_document_error,
        &[
            "credential=",
            "invalid\nscalar",
            r"C:\M2_5D",
            "/home/private",
        ],
    );

    let stale = materialize_document(&template, revision(1), &document, "malformed".into())
        .expect_err("revision mismatch must beat timestamp");
    assert_eq!(
        stale.category(),
        DocumentMaterializationErrorCategory::RevisionMismatch
    );

    let tombstone = decode_template_value(&template_value([], &[], 2, "deleted"));
    let empty = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let malformed =
        materialize_document(&tombstone, tombstone.revision(), &empty, "malformed".into())
            .expect_err("malformed timestamp must beat tombstone");
    assert_eq!(
        malformed.category(),
        DocumentMaterializationErrorCategory::InvalidTimestamp
    );
    let regressed =
        materialize_document(&tombstone, tombstone.revision(), &empty, CREATED_AT.into())
            .expect_err("timestamp regression must beat tombstone");
    assert_eq!(
        regressed.category(),
        DocumentMaterializationErrorCategory::TimestampRegression
    );

    let other_document = decode_document_value(&document_value([], [], 3, OTHER_TEMPLATE_ID));
    let mismatch = materialize_document(
        &tombstone,
        tombstone.revision(),
        &other_document,
        LATER_AT.into(),
    )
    .expect_err("identity mismatch must beat tombstone and future revision");
    assert_eq!(
        mismatch.category(),
        DocumentMaterializationErrorCategory::TemplateIdMismatch
    );
    let tombstoned =
        materialize_document(&tombstone, tombstone.revision(), &empty, LATER_AT.into())
            .expect_err("tombstone must reject materialization");
    assert_eq!(
        tombstoned.category(),
        DocumentMaterializationErrorCategory::TemplateIsTombstoned
    );

    let active_empty = decode_template_value(&template_value([], &[], 2, "active"));
    let future = decode_document_value(&document_value([], [], 3, TEMPLATE_ID));
    let future_error = materialize_document(
        &active_empty,
        active_empty.revision(),
        &future,
        LATER_AT.into(),
    )
    .expect_err("future Document revision must be fatal");
    assert_eq!(
        future_error.category(),
        DocumentMaterializationErrorCategory::FutureDocumentRevision
    );

    for error in [
        invalid_template_error,
        invalid_document_error,
        stale,
        malformed,
        regressed,
        mismatch,
        tombstoned,
        future_error,
    ] {
        assert_error_redacted(
            &error,
            &[
                "credential=",
                "invalid\nscalar",
                "C:\\M2_5D",
                "/home/private",
            ],
        );
    }
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    assert_eq!(invalid_template, invalid_template_before);
    assert_eq!(template_bytes(&invalid_template), invalid_template_snapshot);
    assert_eq!(invalid_document, invalid_document_before);
    assert_eq!(document_bytes(&invalid_document), invalid_document_snapshot);
    Ok(())
}

#[test]
fn existing_unset_archived_orphan_and_choice_order_are_preserved_during_revision_update(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"new default"}),
                    json!({"kind":"text","value":"old initial"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                SINGLE,
                field(
                    "singleChoice",
                    "active",
                    false,
                    json!({"kind":"singleChoice","optionId":SINGLE_A}),
                    json!({"kind":"unset"}),
                    1,
                    single_configuration(),
                ),
            ),
            (
                MULTI,
                field(
                    "multiChoice",
                    "active",
                    false,
                    json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                    json!({"kind":"unset"}),
                    1,
                    multi_configuration(),
                ),
            ),
            (
                ARCHIVED,
                field(
                    "singleLineText",
                    "archived",
                    true,
                    json!({"kind":"text","value":"archived default"}),
                    json!({"kind":"unset"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            ),
        ],
        &[MULTI, TEXT, SINGLE],
        2,
        "active",
    ));
    let raw_document = document_value(
        [
            (
                TEXT,
                json!({"kind":"text","value":"existing private payload"}),
            ),
            (SINGLE, json!({"kind":"unset"})),
            (
                MULTI,
                json!({"kind":"multiChoice","optionIds":[MULTI_A,MULTI_B]}),
            ),
            (ARCHIVED, json!({"kind":"text","value":"archived existing"})),
            (
                ORPHAN,
                json!({"kind":"text","value":"unknown orphan value","futureValue":"keep"}),
            ),
        ],
        [(
            ORPHAN,
            json!({
                "label":"unknown orphan label",
                "kind":"singleLineText",
                "options":{},
                "futureSnapshot":{"nested":[123456789012345678901234567890u128]}
            }),
        )],
        1,
        TEMPLATE_ID,
    );
    let document = decode_document_value(&raw_document);
    let source_wire = document_wire_value(&document);

    let candidate = changed(materialize_document(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?);
    let candidate_wire = document_wire_value(&candidate);
    for field in [TEXT, SINGLE, MULTI, ARCHIVED, ORPHAN] {
        assert_eq!(
            candidate_wire["fieldValues"][field],
            source_wire["fieldValues"][field]
        );
    }
    assert_eq!(
        candidate_wire["orphanedFieldDefinitions"][ORPHAN],
        source_wire["orphanedFieldDefinitions"][ORPHAN]
    );
    assert_eq!(
        candidate_wire["fieldValues"][MULTI]["optionIds"],
        json!([MULTI_A, MULTI_B])
    );
    assert_eq!(candidate_wire["name"], source_wire["name"]);
    assert_eq!(
        candidate_wire["futureDocument"],
        source_wire["futureDocument"]
    );
    assert_eq!(candidate_wire["createdAtUtc"], source_wire["createdAtUtc"]);
    Ok(())
}

#[test]
fn blockers_return_exact_first_issue_without_candidate_or_payload() -> Result<(), Box<dyn Error>> {
    let required_template = decode_template_value(&template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"text","value":"default"}),
                json!({"kind":"text","value":"initial"}),
                1,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        2,
        "active",
    ));
    let explicit_unset = decode_document_value(&document_value(
        [(
            TEXT,
            json!({"kind":"unset","futureUnset":"credential=unset"}),
        )],
        [],
        1,
        TEMPLATE_ID,
    ));
    let error = materialize_document(
        &required_template,
        required_template.revision(),
        &explicit_unset,
        LATER_AT.into(),
    )
    .expect_err("required explicit unset must block");
    assert_eq!(
        error.category(),
        DocumentMaterializationErrorCategory::BlockingIssues
    );
    assert_eq!(
        error.issue_category(),
        Some(DocumentReconciliationIssueCategory::RequiredValueUnset)
    );
    assert_eq!(error.field_id(), Some(field_id(TEXT)));
    assert_eq!(error.issue_count(), 1);
    assert!(!error.issues_truncated());
    assert_eq!(
        error.issue_validation_category(),
        Some(FieldValidationErrorCategory::RequiredValueUnset)
    );
    assert_eq!(
        error.issue_validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );
    assert_error_redacted(&error, &["credential=unset", "credential=document-name"]);

    let missing = decode_document_value(&document_value([], [], 2, TEMPLATE_ID));
    let missing_error = materialize_document(
        &required_template,
        required_template.revision(),
        &missing,
        LATER_AT.into(),
    )
    .expect_err("non-historical missing key must block");
    assert_eq!(
        missing_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::MissingKnownFieldValue)
    );
    assert!(missing_error.source().is_none());
    Ok(())
}

#[test]
fn optional_group_missing_key_repairs_to_canonical_empty_without_inferring_other_fields(
) -> Result<(), Box<dyn Error>> {
    let group = "20000000-0000-4000-8000-00000000000c";
    let mut raw_template = template_value(
        [
            (
                group,
                field(
                    "group",
                    "active",
                    false,
                    json!({"kind":"unset"}),
                    json!({"kind":"unset"}),
                    1,
                    json!({"kind":"group","memberOrder":[],"members":{}}),
                ),
            ),
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"default"}),
                    json!({"kind":"text","value":"initial"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            ),
        ],
        &[group, TEXT],
        1,
        "active",
    );
    raw_template["schemaVersion"] = json!(6);
    let template = decode_template_value(&raw_template);
    let mut raw_document = document_value([], [], 1, TEMPLATE_ID);
    raw_document["schemaVersion"] = json!(6);
    let document = decode_document_value(&raw_document);

    let outcome = repair_missing_optional_group_values(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?;
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    let candidate = outcome
        .document()
        .expect("optional missing group must be repaired");
    let wire = document_wire_value(candidate);
    assert_eq!(
        wire["fieldValues"][group],
        json!({"kind":"group","instanceOrder":[],"instances":{}})
    );
    assert!(wire["fieldValues"].get(TEXT).is_none());

    let second = repair_missing_optional_group_values(
        &template,
        template.revision(),
        candidate,
        LATER_AT.into(),
    )?;
    assert_eq!(second.kind(), DocumentMaterializationOutcomeKind::Unchanged);
    Ok(())
}

#[test]
fn automatic_empty_group_repair_does_not_touch_required_or_scalar_missing_values(
) -> Result<(), Box<dyn Error>> {
    let required_group = "20000000-0000-4000-8000-00000000000c";
    let mut raw_template = template_value(
        [
            (
                required_group,
                field(
                    "group",
                    "active",
                    true,
                    json!({"kind":"unset"}),
                    json!({"kind":"unset"}),
                    1,
                    json!({"kind":"group","memberOrder":[],"members":{}}),
                ),
            ),
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"default"}),
                    json!({"kind":"text","value":"initial"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            ),
        ],
        &[required_group, TEXT],
        1,
        "active",
    );
    raw_template["schemaVersion"] = json!(6);
    let template = decode_template_value(&raw_template);
    let mut raw_document = document_value([], [], 1, TEMPLATE_ID);
    raw_document["schemaVersion"] = json!(6);
    let document = decode_document_value(&raw_document);

    let outcome = repair_missing_optional_group_values(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?;
    assert_eq!(
        outcome.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(outcome.document().is_none());
    Ok(())
}

#[test]
fn blocking_issue_summary_is_bounded_at_1024_without_hiding_failure() -> Result<(), Box<dyn Error>>
{
    for (count, truncated) in [(1_024_usize, false), (1_025, true)] {
        let mut raw_template = template_value([], &[], 1, "active");
        let mut fields = Map::new();
        let mut order = Vec::new();
        for index in 0..count {
            let id = format!("e0000000-{:04x}-4000-8000-{:012x}", index / 65_536, index);
            fields.insert(
                id.clone(),
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"current"}),
                    json!({"kind":"text","value":"initial"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            );
            order.push(Value::String(id));
        }
        raw_template["fields"] = Value::Object(fields);
        raw_template["fieldOrder"] = Value::Array(order);
        let template = decode_template_value(&raw_template);
        let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
        let error =
            materialize_document(&template, template.revision(), &document, LATER_AT.into())
                .expect_err("all non-historical missing keys must remain blocking");
        assert_eq!(
            error.category(),
            DocumentMaterializationErrorCategory::BlockingIssues
        );
        assert_eq!(
            error.issue_category(),
            Some(DocumentReconciliationIssueCategory::MissingKnownFieldValue)
        );
        assert_eq!(error.issue_count(), 1_024);
        assert_eq!(error.issues_truncated(), truncated);
        assert!(error.field_id().is_some());
        assert!(error.source().is_none());
    }
    Ok(())
}

#[test]
fn historical_unset_respects_active_required_and_archived_non_retroactivity(
) -> Result<(), Box<dyn Error>> {
    let active_optional = "21000000-0000-4000-8000-000000000001";
    let archived_unset = "21000000-0000-4000-8000-000000000002";
    let archived_value = "21000000-0000-4000-8000-000000000003";
    let template = decode_template_value(&template_value(
        [
            (
                active_optional,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"current"}),
                    json!({"kind":"unset","futureInitial":"keep-unset"}),
                    2,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                archived_unset,
                field(
                    "singleLineText",
                    "archived",
                    true,
                    json!({"kind":"text","value":"current archived"}),
                    json!({"kind":"unset","futureInitial":"keep-archived-unset"}),
                    2,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                archived_value,
                field(
                    "singleLineText",
                    "archived",
                    true,
                    json!({"kind":"unset"}),
                    json!({"kind":"text","value":"archived historical","futureInitial":"keep-value"}),
                    2,
                    basic_configuration("singleLineText"),
                ),
            ),
        ],
        &[active_optional],
        3,
        "active",
    ));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let candidate = changed(materialize_document(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?);
    let wire = document_wire_value(&candidate);
    assert_eq!(
        wire["fieldValues"][active_optional],
        json!({"kind":"unset","futureInitial":"keep-unset"})
    );
    assert_eq!(
        wire["fieldValues"][archived_unset],
        json!({"kind":"unset","futureInitial":"keep-archived-unset"})
    );
    assert_eq!(
        wire["fieldValues"][archived_value],
        json!({"kind":"text","value":"archived historical","futureInitial":"keep-value"})
    );

    let required = decode_template_value(&template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"unset"}),
                2,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        3,
        "active",
    ));
    let error = materialize_document(&required, required.revision(), &document, LATER_AT.into())
        .expect_err("active required historical unset must block before candidate creation");
    assert_eq!(
        error.issue_category(),
        Some(DocumentReconciliationIssueCategory::RequiredValueUnset)
    );
    assert_eq!(error.field_id(), Some(field_id(TEXT)));
    assert!(error.issue_validation_category().is_none());
    Ok(())
}

#[test]
fn metadata_free_reattachment_removes_only_its_snapshot_and_is_revision_independent(
) -> Result<(), Box<dyn Error>> {
    for (revision_value, lifecycle) in
        [(1, "active"), (2, "active"), (3, "active"), (3, "archived")]
    {
        let order = if lifecycle == "active" {
            vec![TEXT]
        } else {
            Vec::new()
        };
        let template = decode_template_value(&template_value(
            [(
                TEXT,
                field(
                    "singleLineText",
                    lifecycle,
                    false,
                    json!({"kind":"text","value":"current"}),
                    json!({"kind":"unset"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            )],
            &order,
            3,
            "active",
        ));
        let document = decode_document_value(&document_value(
            [
                (
                    TEXT,
                    json!({"kind":"text","value":"reattached value","futureValue":"keep"}),
                ),
                (ORPHAN_OTHER, json!({"kind":"number","value":"999.0001"})),
            ],
            [
                (
                    TEXT,
                    orphan_snapshot("historical label", "singleLineText", json!({})),
                ),
                (
                    ORPHAN_OTHER,
                    json!({
                        "label":"unrelated orphan",
                        "kind":"number",
                        "options":{},
                        "futureSnapshot":{"number":123456789012345678901234567890u128}
                    }),
                ),
            ],
            revision_value,
            TEMPLATE_ID,
        ));
        let source_wire = document_wire_value(&document);
        let outcome =
            materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
        // U1: archived 정의의 snapshot은 재결합으로 제거하지 않는다.
        let candidate = if lifecycle == "archived" {
            assert_eq!(
                outcome.kind(),
                DocumentMaterializationOutcomeKind::Unchanged
            );
            document.clone()
        } else {
            changed(outcome)
        };
        let wire = document_wire_value(&candidate);
        assert_eq!(wire["fieldValues"][TEXT], source_wire["fieldValues"][TEXT]);
        if lifecycle == "archived" {
            assert!(
                wire["orphanedFieldDefinitions"][TEXT]
                    == source_wire["orphanedFieldDefinitions"][TEXT]
            );
        } else {
            assert!(wire["orphanedFieldDefinitions"].get(TEXT).is_none());
        }
        assert_eq!(
            wire["orphanedFieldDefinitions"][ORPHAN_OTHER],
            source_wire["orphanedFieldDefinitions"][ORPHAN_OTHER]
        );
        let bytes = document_bytes(&candidate);
        let repeated =
            materialize_document(&template, template.revision(), &candidate, LATER_AT.into())?;
        assert_eq!(
            repeated.kind(),
            DocumentMaterializationOutcomeKind::Unchanged
        );
        assert_eq!(document_bytes(&candidate), bytes);
    }
    Ok(())
}

#[test]
fn reattachment_metadata_unknown_option_and_kind_conflict_fail_closed() -> Result<(), Box<dyn Error>>
{
    let template = decode_template_value(&template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"unset"}),
                1,
                single_configuration(),
            ),
        )],
        &[SINGLE],
        3,
        "active",
    ));
    let metadata_cases = [
        json!({
            "label":"private snapshot",
            "kind":"singleChoice",
            "options":{(SINGLE_A):{"label":"old option"}},
            "futureSnapshot":{"nested":[123456789012345678901234567890u128]}
        }),
        json!({
            "label":"private snapshot",
            "kind":"singleChoice",
            "options":{(SINGLE_A):{
                "label":"old option",
                "futureOption":{"credential":"option-extra"}
            }}
        }),
    ];
    for snapshot in metadata_cases {
        let document = decode_document_value(&document_value(
            [(SINGLE, json!({"kind":"singleChoice","optionId":SINGLE_A}))],
            [(SINGLE, snapshot)],
            1,
            TEMPLATE_ID,
        ));
        let before = document.clone();
        let bytes = document_bytes(&document);
        let error =
            materialize_document(&template, template.revision(), &document, LATER_AT.into())
                .expect_err("unknown snapshot metadata must block lossy removal");
        assert_eq!(
            error.category(),
            DocumentMaterializationErrorCategory::BlockingIssues
        );
        assert_eq!(
            error.issue_category(),
            Some(DocumentReconciliationIssueCategory::LossyOrphanReattachment)
        );
        assert_eq!(document, before);
        assert_eq!(document_bytes(&document), bytes);
        assert_error_redacted(
            &error,
            &[
                "futureSnapshot",
                "futureOption",
                "123456789012345678901234567890",
                "option-extra",
            ],
        );
    }

    let unknown = decode_document_value(&document_value(
        [(
            SINGLE,
            json!({"kind":"singleChoice","optionId":UNKNOWN_OPTION}),
        )],
        [(
            SINGLE,
            orphan_snapshot(
                "unknown selected option",
                "singleChoice",
                json!({(UNKNOWN_OPTION):{"label":"unknown"}}),
            ),
        )],
        1,
        TEMPLATE_ID,
    ));
    let unknown_error =
        materialize_document(&template, template.revision(), &unknown, LATER_AT.into())
            .expect_err("unknown selected Option must block");
    assert_eq!(
        unknown_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::UnknownSelectedOption)
    );
    assert_eq!(
        unknown_error.issue_choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );

    let mixed_unknown = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[MULTI_ARCHIVED,UNKNOWN_OPTION]}),
        )],
        [(
            MULTI,
            orphan_snapshot(
                "mixed archived and unknown",
                "multiChoice",
                json!({
                    (MULTI_ARCHIVED):{"label":"archived"},
                    (UNKNOWN_OPTION):{"label":"unknown"}
                }),
            ),
        )],
        1,
        TEMPLATE_ID,
    ));
    let multi_template = decode_template_value(&template_value(
        [(
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                json!({"kind":"unset"}),
                1,
                multi_configuration(),
            ),
        )],
        &[MULTI],
        3,
        "active",
    ));
    let view = crate::data::artifact::reconcile_document(&multi_template, &mixed_unknown)?;
    assert!(view.warnings().is_empty());
    assert_eq!(
        view.blocking_issues()[0].category(),
        DocumentReconciliationIssueCategory::UnknownSelectedOption
    );
    let mixed_error = materialize_document(
        &multi_template,
        multi_template.revision(),
        &mixed_unknown,
        LATER_AT.into(),
    )
    .expect_err("unknown Option blocker must take precedence over archived warning");
    assert_eq!(
        mixed_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::UnknownSelectedOption)
    );

    let conflict = decode_document_value(&document_value(
        [(SINGLE, json!({"kind":"unset"}))],
        [(SINGLE, orphan_snapshot("wrong kind", "number", json!({})))],
        1,
        TEMPLATE_ID,
    ));
    let conflict_error =
        materialize_document(&template, template.revision(), &conflict, LATER_AT.into())
            .expect_err("snapshot kind mismatch must block");
    assert_eq!(
        conflict_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict)
    );
    Ok(())
}

#[test]
fn archived_option_warnings_survive_changed_and_unchanged_outcomes() -> Result<(), Box<dyn Error>> {
    let warning_metadata_key = "futureWarningMetadata";
    let warning_metadata_value = "credential=warning-metadata-value";
    let mut raw_template = template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"unset"}),
                1,
                single_configuration(),
            ),
        )],
        &[SINGLE],
        2,
        "active",
    );
    raw_template["fields"][SINGLE]["configuration"]["options"][SINGLE_ARCHIVED]
        [warning_metadata_key] = json!({
        "value":warning_metadata_value,
        "path":WINDOWS_CANARY,
        "url":"https://example.invalid/warning?credential=secret",
        "integer":exact_number(FIELD_INTEGER_NUMBER),
        "decimal":exact_number(ORPHAN_DECIMAL_NUMBER),
        "exponent":exact_number(EXPONENT_NUMBER)
    });
    raw_template["fields"][RICH] = field(
        "richText",
        "active",
        false,
        rich_text("current warning rich", false),
        json!({"kind":"unset"}),
        1,
        basic_configuration("richText"),
    );
    raw_template["fieldOrder"] = json!([SINGLE, RICH]);
    let template = decode_template_value(&raw_template);
    let mut raw_document = document_value(
        [
            (
                SINGLE,
                json!({"kind":"singleChoice","optionId":SINGLE_ARCHIVED}),
            ),
            (RICH, rich_text("warning rich payload", true)),
        ],
        [],
        2,
        TEMPLATE_ID,
    );
    raw_document["futureDocument"]["windows"] = json!(WINDOWS_CANARY);
    raw_document["futureDocument"]["unix"] = json!(UNIX_CANARY);
    raw_document["fieldValues"][SINGLE]["futureValueOuter"] =
        json!({"payload":"warning-payload-secret"});
    assert_eq!(
        raw_template["fields"][SINGLE]["configuration"]["options"][SINGLE_ARCHIVED]
            [warning_metadata_key]["value"],
        json!(warning_metadata_value)
    );
    assert_eq!(
        raw_template["fields"][SINGLE]["configuration"]["options"][SINGLE_ARCHIVED]["label"],
        json!("credential=single-archived")
    );
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["optionId"],
        json!(SINGLE_ARCHIVED)
    );
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["futureValueOuter"]["payload"],
        json!("warning-payload-secret")
    );
    assert_eq!(
        raw_document["futureDocument"]["windows"],
        json!(WINDOWS_CANARY)
    );
    assert_eq!(raw_document["futureDocument"]["unix"], json!(UNIX_CANARY));
    assert_eq!(
        raw_document["futureDocument"]["number"]
            .as_number()
            .expect("warning source decimal must be a Number")
            .to_string(),
        DOCUMENT_DECIMAL_NUMBER
    );
    assert_eq!(
        raw_document["fieldValues"][RICH]["document"]["content"]["children"][0]["children"][0]
            ["text"],
        json!("warning rich payload")
    );
    assert_eq!(
        raw_document["fieldValues"][RICH]["document"]["futureEnvelope"]["decimal"]
            .as_number()
            .expect("warning rich decimal must be a Number")
            .to_string(),
        "12345678901234567890.0001"
    );
    assert_eq!(
        raw_document["fieldValues"][RICH]["futureValueOuter"]["credential"],
        json!("outer-secret")
    );
    assert_eq!(
        raw_document["fieldValues"][RICH]["document"]["content"]["children"][0]["futureContainer"]
            ["path"],
        json!(RICH_WINDOWS_CANARY)
    );
    assert_eq!(
        raw_document["fieldValues"][RICH]["document"]["content"]["children"][0]["children"][0]
            ["futureText"]["url"],
        json!("https://example.invalid/private?credential=rich")
    );
    for (key, lexeme) in [
        ("integer", FIELD_INTEGER_NUMBER),
        ("decimal", ORPHAN_DECIMAL_NUMBER),
        ("exponent", EXPONENT_NUMBER),
    ] {
        assert_eq!(
            raw_template["fields"][SINGLE]["configuration"]["options"][SINGLE_ARCHIVED]
                [warning_metadata_key][key]
                .as_number()
                .expect("warning metadata precision canary must be a Number")
                .to_string(),
            lexeme
        );
    }
    let document = decode_document_value(&raw_document);
    let escaped_windows = WINDOWS_CANARY.escape_debug().to_string();
    let escaped_rich_windows = RICH_WINDOWS_CANARY.escape_debug().to_string();
    let warning_forbidden = [
        SINGLE_ARCHIVED,
        "a0000000-0000",
        "credential=",
        "credential=single-archived",
        warning_metadata_key,
        warning_metadata_value,
        "warning-payload-secret",
        "warning rich payload",
        "outer-secret",
        "futureEnvelope",
        "futureRoot",
        "futureContainer",
        "futureText",
        FIELD_INTEGER_NUMBER,
        ORPHAN_DECIMAL_NUMBER,
        EXPONENT_NUMBER,
        DOCUMENT_DECIMAL_NUMBER,
        "12345678901234567890.0001",
        "https://example.invalid/warning",
        "https://example.invalid/private?credential=rich",
        WINDOWS_CANARY,
        escaped_windows.as_str(),
        RICH_WINDOWS_CANARY,
        escaped_rich_windows.as_str(),
        "redaction-only",
        UNIX_CANARY,
        "srv/m2_5d",
    ];
    assert_redaction_fragments_are_observable(&warning_forbidden);
    // U1: 최초 pass에서 snapshot을 만든 뒤 Unchanged warning 계약을 검사한다.
    let snapshot_outcome =
        materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(snapshot_outcome.warnings().len(), 1);
    assert_eq!(snapshot_outcome.warnings()[0].field_id(), field_id(SINGLE));
    let snapshot_bundle = snapshot_outcome
        .into_changed()
        .expect("initial snapshot change");
    let snapshotted = snapshot_bundle.document();
    assert!(snapshotted
        .orphaned_field_definitions()
        .contains_key(&field_id(SINGLE)));
    let unchanged =
        materialize_document(&template, template.revision(), snapshotted, LATER_AT.into())?;
    assert_eq!(
        unchanged.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert_eq!(unchanged.warnings().len(), 1);
    let warning = &unchanged.warnings()[0];
    assert_eq!(
        warning.category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(warning.field_id(), field_id(SINGLE));
    assert_eq!(warning.count(), 1);
    assert!(!warning.truncated());
    for rendered in [
        format!("{unchanged:?}"),
        format!("{:?}", unchanged.warnings()),
        format!("{:?}", unchanged.warnings().as_slice()),
        format!("{warning:?}"),
    ] {
        assert_rendering_redacted(&rendered, &warning_forbidden, "Unchanged warning outcome");
    }
    let unchanged_warnings = unchanged
        .into_changed()
        .expect_err("Unchanged must not synthesize a Document clone");
    assert_eq!(unchanged_warnings.len(), 1);
    assert_eq!(
        unchanged_warnings[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(unchanged_warnings[0].field_id(), field_id(SINGLE));
    assert_eq!(unchanged_warnings[0].count(), 1);
    assert!(!unchanged_warnings[0].truncated());
    assert_rendering_redacted(
        &format!("{unchanged_warnings:?}"),
        &warning_forbidden,
        "consumed Unchanged warnings",
    );

    let mut raw_older = raw_document;
    raw_older["templateRevision"] = json!(1);
    let older = decode_document_value(&raw_older);
    let changed_outcome =
        materialize_document(&template, template.revision(), &older, LATER_AT.into())?;
    assert_eq!(
        changed_outcome.kind(),
        DocumentMaterializationOutcomeKind::Changed
    );
    assert_eq!(changed_outcome.warnings().len(), 1);
    assert_eq!(
        changed_outcome.warnings()[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(changed_outcome.warnings()[0].field_id(), field_id(SINGLE));
    assert_eq!(changed_outcome.warnings()[0].count(), 1);
    assert!(!changed_outcome.warnings()[0].truncated());
    for rendered in [
        format!("{changed_outcome:?}"),
        format!("{:?}", changed_outcome.warnings()),
        format!("{:?}", changed_outcome.warnings().as_slice()),
        format!("{:?}", changed_outcome.warnings()[0]),
    ] {
        assert_rendering_redacted(&rendered, &warning_forbidden, "Changed warning outcome");
    }
    let changed = changed_outcome
        .into_changed()
        .expect("Changed consuming accessor must preserve candidate and warnings");
    assert_eq!(changed.warnings().len(), 1);
    assert_eq!(
        changed.warnings()[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(changed.warnings()[0].field_id(), field_id(SINGLE));
    assert_eq!(changed.warnings()[0].count(), 1);
    assert!(!changed.warnings()[0].truncated());
    assert_eq!(
        changed.document().field_values()[&field_id(SINGLE)],
        older.field_values()[&field_id(SINGLE)]
    );
    for rendered in [
        format!("{changed:?}"),
        format!("{:?}", changed.warnings()),
        format!("{:?}", changed.warnings().as_slice()),
        format!("{:?}", changed.warnings()[0]),
    ] {
        assert_rendering_redacted(&rendered, &warning_forbidden, "Changed bundle warnings");
    }
    Ok(())
}

#[test]
fn historical_and_reattached_archived_options_keep_warning_semantics() -> Result<(), Box<dyn Error>>
{
    let historical_template = decode_template_value(&template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"singleChoice","optionId":SINGLE_ARCHIVED}),
                2,
                single_configuration(),
            ),
        )],
        &[SINGLE],
        3,
        "active",
    ));
    let missing = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let historical = materialize_document(
        &historical_template,
        historical_template.revision(),
        &missing,
        LATER_AT.into(),
    )?;
    assert_eq!(
        historical.kind(),
        DocumentMaterializationOutcomeKind::Changed
    );
    assert_eq!(historical.warnings().len(), 1);
    assert_eq!(historical.warnings()[0].count(), 1);
    assert_eq!(
        document_wire_value(historical.document().expect("Changed candidate"))["fieldValues"]
            [SINGLE]["optionId"],
        json!(SINGLE_ARCHIVED)
    );

    let reattach_template = decode_template_value(&template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":SINGLE_A}),
                json!({"kind":"unset"}),
                1,
                single_configuration(),
            ),
        )],
        &[SINGLE],
        3,
        "active",
    ));
    let reattach_document = decode_document_value(&document_value(
        [(
            SINGLE,
            json!({"kind":"singleChoice","optionId":SINGLE_ARCHIVED}),
        )],
        [(
            SINGLE,
            orphan_snapshot(
                "historical choice",
                "singleChoice",
                json!({(SINGLE_ARCHIVED):{"label":"historical archived"}}),
            ),
        )],
        3,
        TEMPLATE_ID,
    ));
    let reattached = materialize_document(
        &reattach_template,
        reattach_template.revision(),
        &reattach_document,
        LATER_AT.into(),
    )?;
    assert_eq!(
        reattached.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert_eq!(reattached.warnings().len(), 1);
    assert_eq!(reattached.warnings()[0].count(), 1);
    assert!(reattached.document().is_none());
    let candidate = &reattach_document;
    assert_eq!(
        candidate.field_values()[&field_id(SINGLE)],
        reattach_document.field_values()[&field_id(SINGLE)]
    );
    assert!(candidate
        .orphaned_field_definitions()
        .contains_key(&field_id(SINGLE)));
    Ok(())
}

#[test]
fn mixed_active_and_archived_multi_choice_reports_only_archived_count_without_reordering(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [(
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"unset"}),
                json!({"kind":"unset"}),
                1,
                multi_configuration(),
            ),
        )],
        &[MULTI],
        2,
        "active",
    ));
    let document = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[MULTI_A,MULTI_ARCHIVED]}),
        )],
        [],
        1,
        TEMPLATE_ID,
    ));
    let source_value = document.field_values()[&field_id(MULTI)].clone();
    let outcome = materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert_eq!(outcome.warnings().len(), 1);
    assert_eq!(outcome.warnings()[0].count(), 1);
    assert!(!outcome.warnings()[0].truncated());
    assert_eq!(
        outcome
            .document()
            .expect("revision update candidate")
            .field_values()[&field_id(MULTI)],
        source_value
    );
    assert_eq!(
        document_wire_value(outcome.document().expect("candidate"))["fieldValues"][MULTI]
            ["optionIds"],
        json!([MULTI_A, MULTI_ARCHIVED])
    );
    Ok(())
}

#[test]
fn combined_materialization_is_atomic_deterministic_and_idempotent() -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"current must not be used"}),
                    json!({"kind":"text","value":"historical inserted","futureInitial":{"n":123456789012345678901234567890u128}}),
                    2,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                SINGLE,
                field(
                    "singleChoice",
                    "active",
                    false,
                    json!({"kind":"singleChoice","optionId":SINGLE_A}),
                    json!({"kind":"unset"}),
                    1,
                    single_configuration(),
                ),
            ),
            (
                MULTI,
                field(
                    "multiChoice",
                    "active",
                    false,
                    json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                    json!({"kind":"unset"}),
                    1,
                    multi_configuration(),
                ),
            ),
        ],
        &[MULTI, TEXT, SINGLE],
        3,
        "active",
    ));
    let document = decode_document_value(&document_value(
        [
            (
                SINGLE,
                json!({"kind":"singleChoice","optionId":SINGLE_ARCHIVED,"futureValue":"preserve"}),
            ),
            (MULTI, json!({"kind":"unset","futureUnset":"preserve"})),
            (
                ORPHAN,
                json!({"kind":"number","value":"999.0001","futureValue":"orphan"}),
            ),
        ],
        [
            (
                SINGLE,
                orphan_snapshot(
                    "historical choice label",
                    "singleChoice",
                    json!({(SINGLE_ARCHIVED):{"label":"historical archived label"}}),
                ),
            ),
            (
                ORPHAN,
                json!({
                    "label":"unknown orphan label",
                    "kind":"number",
                    "options":{},
                    "futureSnapshot":{"nested":[123456789012345678901234567890u128]}
                }),
            ),
        ],
        1,
        TEMPLATE_ID,
    ));
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let first = materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(first.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert_eq!(first.warnings().len(), 1);
    let candidate = first.document().expect("combined candidate");
    let wire = document_wire_value(candidate);
    assert_eq!(
        wire["fieldValues"][TEXT]["value"],
        json!("historical inserted")
    );
    assert_eq!(
        wire["fieldValues"][SINGLE],
        document_wire_value(&document)["fieldValues"][SINGLE]
    );
    assert_eq!(
        wire["fieldValues"][MULTI],
        document_wire_value(&document)["fieldValues"][MULTI]
    );
    assert!(
        wire["orphanedFieldDefinitions"]
            .get(SINGLE)
            .expect("archived selection snapshot must remain")
            == document_wire_value(&document)["orphanedFieldDefinitions"]
                .get(SINGLE)
                .expect("source has the selected snapshot")
    );
    assert_eq!(
        wire["orphanedFieldDefinitions"][ORPHAN],
        document_wire_value(&document)["orphanedFieldDefinitions"][ORPHAN]
    );
    assert_eq!(candidate.template_revision(), template.revision());
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    candidate.validate_storage()?;
    let final_view = crate::data::artifact::reconcile_document(&template, candidate)?;
    assert!(final_view.can_materialize());
    assert!(!final_view.materialization_required());
    assert_eq!(final_view.warnings().len(), 1);
    let encoded = encode_document(candidate)?;
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);

    let second = materialize_document(&template, template.revision(), candidate, LATER_AT.into())?;
    assert_eq!(second.kind(), DocumentMaterializationOutcomeKind::Unchanged);
    assert_eq!(second.warnings().len(), 1);
    assert_eq!(document_bytes(candidate), encoded);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn outcome_and_blocking_error_debug_redact_aggregate_metadata_and_payloads(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&historical_template_value(3));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let outcome = materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    let rendered = format!("{outcome:?}");
    let forbidden = [
        "credential=",
        "초기 rich",
        "outer-secret",
        "futureEnvelope",
        "12345678901234567890.0001",
        "https://example.invalid",
        r"C:\M2_5D",
        "/home/private",
        SINGLE_B,
        MULTI_A,
    ];
    assert_rendering_redacted(&rendered, &forbidden, "materialization outcome");
    assert!(rendered.contains("Changed"));
    assert!(rendered.contains("document_redacted"));

    let required_template = decode_template_value(&template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"text","value":"initial"}),
                1,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        2,
        "active",
    ));
    let blocked = decode_document_value(&document_value(
        [(TEXT, json!({"kind":"unset","secret":"credential=payload"}))],
        [],
        1,
        TEMPLATE_ID,
    ));
    let error = materialize_document(
        &required_template,
        required_template.revision(),
        &blocked,
        LATER_AT.into(),
    )
    .expect_err("blocking issue must not return an outcome");
    assert_error_redacted(
        &error,
        &[
            "credential=payload",
            "credential=document-name",
            "futureDocument",
            "987654321098765432109876543210.0001",
            r"C:\M2_5D",
            "/home/private",
        ],
    );
    Ok(())
}

#[test]
fn fatal_errors_redact_canaries_from_every_storage_layer_without_shadowing(
) -> Result<(), Box<dyn Error>> {
    let (raw_template, raw_document) = full_redaction_values();
    assert_full_redaction_canaries(&raw_template, &raw_document);
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&raw_document);
    template.validate_storage()?;
    document.validate_storage()?;
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);
    let escaped_windows = WINDOWS_CANARY.escape_debug().to_string();
    let escaped_rich_windows = RICH_WINDOWS_CANARY.escape_debug().to_string();
    let forbidden = [
        "credential=",
        "template-root-secret",
        "document-root-secret",
        "field-definition-secret",
        "futureTemplate",
        "futureDocument",
        "futureField",
        "futureValueOuter",
        "futureKnownOption",
        "futureOption",
        "futureSnapshot",
        "futurePresentation",
        "futureConfiguration",
        "outer-secret",
        "futureEnvelope",
        "futureRoot",
        "futureContainer",
        "futureText",
        INITIAL_RICH_PAYLOAD,
        "private-singleLineText-token",
        "single-a",
        "credential=orphan-field-label",
        "credential=orphan-option-one",
        "credential=orphan-option-two",
        "credential=orphan-option-three",
        UNKNOWN_OPTION,
        UNKNOWN_OPTION_TWO,
        UNKNOWN_OPTION_THREE,
        "c0000000-0000",
        FIELD_INTEGER_NUMBER,
        ORPHAN_INTEGER_NUMBER,
        EXPONENT_NUMBER,
        "12345678901234567890.0001",
        ORPHAN_DECIMAL_NUMBER,
        DOCUMENT_DECIMAL_NUMBER,
        "https://example.invalid",
        WINDOWS_CANARY,
        escaped_windows.as_str(),
        RICH_WINDOWS_CANARY,
        escaped_rich_windows.as_str(),
        "M2_5D",
        "redaction-only",
        UNIX_CANARY,
        "srv/m2_5d",
    ];
    assert_redaction_fragments_are_observable(&forbidden);

    let mut invalid_template = template.clone();
    invalid_template.corrupt_field_order_for_test();
    let invalid_template_before = invalid_template.clone();
    let invalid_template_snapshot = template_bytes(&invalid_template);
    let invalid_template_error = materialize_document(
        &invalid_template,
        revision(99),
        &document,
        "malformed".into(),
    )
    .expect_err("invalid Template storage must have first priority");
    assert_eq!(
        invalid_template_error.category(),
        DocumentMaterializationErrorCategory::InvalidTemplate
    );
    assert_eq!(
        invalid_template_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_eq!(invalid_template_error.artifact_scalar_location(), None);
    assert_error_redacted(&invalid_template_error, &forbidden);
    assert_base_scalar_redacted(&invalid_template_error);
    assert_eq!(invalid_template, invalid_template_before);
    assert_eq!(template_bytes(&invalid_template), invalid_template_snapshot);

    let mut invalid_document = document.clone();
    invalid_document.corrupt_scalar_value_for_test(field_id(TEXT), INVALID_SCALAR_PAYLOAD);
    let invalid_document_before = invalid_document.clone();
    let invalid_document_snapshot = document_bytes(&invalid_document);
    let invalid_document_wire = document_wire_value(&invalid_document);
    assert_eq!(
        invalid_document_wire["fieldValues"][TEXT]["kind"],
        json!("text")
    );
    assert_eq!(
        invalid_document_wire["fieldValues"][TEXT]["value"],
        json!(INVALID_SCALAR_PAYLOAD)
    );
    assert_eq!(
        invalid_document_wire["fieldValues"][TEXT]["futureValueOuter"]["credential"],
        json!("field-value-secret")
    );
    let invalid_document_error = materialize_document(
        &template,
        revision(99),
        &invalid_document,
        "malformed".into(),
    )
    .expect_err("invalid Document storage must beat preconditions");
    assert_eq!(
        invalid_document_error.category(),
        DocumentMaterializationErrorCategory::InvalidDocument
    );
    assert_eq!(
        invalid_document_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        invalid_document_error.artifact_scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        invalid_document_error.artifact_scalar_location(),
        Some(ArtifactScalarValueLocation::DocumentField)
    );
    assert_error_redacted(&invalid_document_error, &forbidden);
    assert_error_redacted(
        &invalid_document_error,
        &[
            INVALID_SCALAR_PAYLOAD,
            INVALID_SCALAR_ESCAPED,
            "scalar payload",
            "field-value-secret",
        ],
    );
    assert_eq!(invalid_document, invalid_document_before);
    assert_eq!(document_bytes(&invalid_document), invalid_document_snapshot);

    let revision_error =
        materialize_document(&template, revision(2), &document, "malformed".into())
            .expect_err("expected revision mismatch must beat timestamp validation");
    assert_eq!(
        revision_error.category(),
        DocumentMaterializationErrorCategory::RevisionMismatch
    );
    assert_error_redacted(&revision_error, &forbidden);
    assert_base_scalar_redacted(&revision_error);

    let timestamp_error = materialize_document(
        &template,
        template.revision(),
        &document,
        "2026-09-03T00:00:00Z".into(),
    )
    .expect_err("non-millisecond timestamp must be rejected");
    assert_eq!(
        timestamp_error.category(),
        DocumentMaterializationErrorCategory::InvalidTimestamp
    );
    assert_error_redacted(&timestamp_error, &forbidden);
    assert_base_scalar_redacted(&timestamp_error);

    let regression_error =
        materialize_document(&template, template.revision(), &document, CREATED_AT.into())
            .expect_err("timestamp regression must be rejected");
    assert_eq!(
        regression_error.category(),
        DocumentMaterializationErrorCategory::TimestampRegression
    );
    assert_error_redacted(&regression_error, &forbidden);
    assert_base_scalar_redacted(&regression_error);

    let mut mismatch_value = raw_document.clone();
    mismatch_value["templateId"] = json!(OTHER_TEMPLATE_ID);
    let mismatch_document = decode_document_value(&mismatch_value);
    mismatch_document.validate_storage()?;
    let mismatch_before = mismatch_document.clone();
    let mismatch_snapshot = document_bytes(&mismatch_document);
    let mismatch_error = materialize_document(
        &template,
        template.revision(),
        &mismatch_document,
        LATER_AT.into(),
    )
    .expect_err("identity mismatch must precede reconciliation");
    assert_eq!(
        mismatch_error.category(),
        DocumentMaterializationErrorCategory::TemplateIdMismatch
    );
    assert_error_redacted(&mismatch_error, &forbidden);
    assert_base_scalar_redacted(&mismatch_error);
    assert_eq!(mismatch_document, mismatch_before);
    assert_eq!(document_bytes(&mismatch_document), mismatch_snapshot);

    let mut tombstone_value = raw_template.clone();
    tombstone_value["lifecycle"] = json!("deleted");
    let tombstone = decode_template_value(&tombstone_value);
    tombstone.validate_storage()?;
    let tombstone_before = tombstone.clone();
    let tombstone_snapshot = template_bytes(&tombstone);
    let tombstone_error =
        materialize_document(&tombstone, tombstone.revision(), &document, LATER_AT.into())
            .expect_err("tombstoned Template must reject materialization");
    assert_eq!(
        tombstone_error.category(),
        DocumentMaterializationErrorCategory::TemplateIsTombstoned
    );
    assert_error_redacted(&tombstone_error, &forbidden);
    assert_base_scalar_redacted(&tombstone_error);
    assert_eq!(tombstone, tombstone_before);
    assert_eq!(template_bytes(&tombstone), tombstone_snapshot);

    let mut future_value = raw_document.clone();
    future_value["templateRevision"] = json!(4);
    let future_document = decode_document_value(&future_value);
    future_document.validate_storage()?;
    let future_before = future_document.clone();
    let future_snapshot = document_bytes(&future_document);
    let future_error = materialize_document(
        &template,
        template.revision(),
        &future_document,
        LATER_AT.into(),
    )
    .expect_err("future Document revision must be rejected before blockers");
    assert_eq!(
        future_error.category(),
        DocumentMaterializationErrorCategory::FutureDocumentRevision
    );
    assert_error_redacted(&future_error, &forbidden);
    assert_base_scalar_redacted(&future_error);
    assert_eq!(future_document, future_before);
    assert_eq!(document_bytes(&future_document), future_snapshot);

    let mut blocked_value = raw_document.clone();
    blocked_value["templateRevision"] = json!(3);
    blocked_value["fieldValues"][TEXT] = json!({
        "kind":"unset",
        "futureValueOuter":{"credential":"blocked-value-secret"}
    });
    let mut required_value = raw_template.clone();
    required_value["fields"][TEXT]["required"] = json!(true);
    let required_template = decode_template_value(&required_value);
    let blocked_document = decode_document_value(&blocked_value);
    required_template.validate_storage()?;
    blocked_document.validate_storage()?;
    let required_before = required_template.clone();
    let blocked_before = blocked_document.clone();
    let required_snapshot = template_bytes(&required_template);
    let blocked_snapshot = document_bytes(&blocked_document);
    let blocked_error = materialize_document(
        &required_template,
        required_template.revision(),
        &blocked_document,
        LATER_AT.into(),
    )
    .expect_err("active required explicit unset must block");
    assert_eq!(
        blocked_error.category(),
        DocumentMaterializationErrorCategory::BlockingIssues
    );
    assert_eq!(
        blocked_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::RequiredValueUnset)
    );
    assert_eq!(
        blocked_error.issue_validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );
    assert_error_redacted(&blocked_error, &forbidden);
    assert_error_redacted(&blocked_error, &["blocked-value-secret"]);
    assert_sources_unchanged(
        &required_template,
        &required_before,
        &required_snapshot,
        &blocked_document,
        &blocked_before,
        &blocked_snapshot,
    );

    let candidate_error = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::ReservedRootKey("$serde_json::private::RedactionCanary".into()),
    )
    .expect_err("final candidate storage error must not expose source or candidate data");
    assert_eq!(
        candidate_error.category(),
        DocumentMaterializationErrorCategory::InvalidCandidate
    );
    assert_eq!(
        candidate_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::ReservedExtraKey)
    );
    assert_error_redacted(&candidate_error, &forbidden);
    assert_base_scalar_redacted(&candidate_error);
    assert_error_redacted(&candidate_error, &["$serde_json::private::RedactionCanary"]);

    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn finalization_rejects_semantic_and_whole_wire_corruption_with_typed_categories(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&historical_template_value(3));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let source_before = document.clone();
    let source_bytes = document_bytes(&document);

    let cases = [
        (
            FinalCandidateCorruption::RemoveKnownField(field_id(TEXT)),
            DocumentMaterializationErrorCategory::BlockingIssues,
            None,
            Some(DocumentReconciliationIssueCategory::MissingKnownFieldValue),
        ),
        (
            FinalCandidateCorruption::ReplaceWithNumber(field_id(TEXT), "1".into()),
            DocumentMaterializationErrorCategory::BlockingIssues,
            None,
            Some(DocumentReconciliationIssueCategory::InvalidKnownFieldValue),
        ),
        (
            FinalCandidateCorruption::ReplaceWithSingleChoice(
                field_id(SINGLE),
                option_id(UNKNOWN_OPTION),
            ),
            DocumentMaterializationErrorCategory::BlockingIssues,
            None,
            Some(DocumentReconciliationIssueCategory::UnknownSelectedOption),
        ),
        (
            FinalCandidateCorruption::NonCanonicalMultiChoice(
                field_id(MULTI),
                vec![option_id(MULTI_B), option_id(MULTI_A)],
            ),
            DocumentMaterializationErrorCategory::InvalidCandidate,
            Some(ArtifactValidationErrorCategory::InvalidChoiceValue),
            None,
        ),
        (
            FinalCandidateCorruption::SemanticEmptyRichText(field_id(RICH)),
            DocumentMaterializationErrorCategory::InvalidCandidate,
            Some(ArtifactValidationErrorCategory::InvalidRichTextValue),
            None,
        ),
        (
            FinalCandidateCorruption::ReservedRootKey(
                "$serde_json::private::FutureTransport".into(),
            ),
            DocumentMaterializationErrorCategory::InvalidCandidate,
            Some(ArtifactValidationErrorCategory::ReservedExtraKey),
            None,
        ),
        (
            FinalCandidateCorruption::Timestamp("2026-08-31T00:00:00.000Z".into()),
            DocumentMaterializationErrorCategory::InvalidCandidate,
            Some(ArtifactValidationErrorCategory::TimestampOrder),
            None,
        ),
    ];

    for (corruption, category, artifact_category, issue_category) in cases {
        let error = materialize_document_with_final_corruption_for_test(
            &template,
            &document,
            LATER_AT.into(),
            corruption,
        )
        .expect_err("final corruption must not return candidate/outcome");
        assert_eq!(error.category(), category);
        assert_eq!(error.artifact_validation_category(), artifact_category);
        assert_eq!(error.issue_category(), issue_category);
        assert_eq!(document, source_before);
        assert_eq!(document_bytes(&document), source_bytes);
        assert_error_redacted(
            &error,
            &[
                "credential=",
                UNKNOWN_OPTION,
                "$serde_json::private::FutureTransport",
            ],
        );
    }

    let valid_candidate = changed(materialize_document(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?);
    let mut probe = document_wire_value(&valid_candidate);
    let mut depth_value = Value::Null;
    for depth in 1..=MAX_JSON_NESTING_DEPTH {
        let candidate_value = nested_array(depth, json!("depth-canary"));
        probe["testOnlyDepth"] = candidate_value.clone();
        if maximum_container_depth(&probe) == MAX_JSON_NESTING_DEPTH + 1 {
            depth_value = candidate_value;
            break;
        }
    }
    assert_ne!(
        depth_value,
        Value::Null,
        "fixture must reach whole-wire depth 128"
    );
    let depth_error = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::WholeWireDepth(depth_value),
    )
    .expect_err("whole-wire depth 128 must fail final storage admission");
    assert_eq!(
        depth_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_error_redacted(&depth_error, &["depth-canary", "credential="]);

    let malformed = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::Timestamp("2026-09-03T00:00:00Z".into()),
    )
    .expect_err("malformed final timestamp must fail");
    assert_eq!(
        malformed.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidTimestamp)
    );

    let wrong_kind = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::ReplaceWithNumber(field_id(TEXT), "1".into()),
    )
    .expect_err("wrong kind must fail Template-bound final reconciliation");
    assert_eq!(
        wrong_kind.issue_validation_category(),
        Some(FieldValidationErrorCategory::FieldValueKindMismatch)
    );
    assert_eq!(
        wrong_kind.issue_validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );

    let unknown_choice = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::ReplaceWithSingleChoice(
            field_id(SINGLE),
            option_id(UNKNOWN_OPTION),
        ),
    )
    .expect_err("unknown Option must fail Template-bound final reconciliation");
    assert_eq!(
        unknown_choice.issue_choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );

    let noncanonical = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::NonCanonicalMultiChoice(
            field_id(MULTI),
            vec![option_id(MULTI_B), option_id(MULTI_A)],
        ),
    )
    .expect_err("noncanonical multiChoice must fail standalone candidate admission");
    assert_eq!(
        noncanonical.artifact_choice_category(),
        Some(ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder)
    );
    assert_eq!(
        noncanonical.artifact_choice_location(),
        Some(crate::data::artifact::ArtifactChoiceValueLocation::DocumentField)
    );
    Ok(())
}

#[test]
fn source_and_candidate_use_actual_whole_wire_depth_and_reserved_namespace_guards(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value([], &[], 2, "active"));
    let mut raw_document = document_value([], [], 1, TEMPLATE_ID);
    raw_document["safeMarkerValue"] = json!("$serde_json::private::Number");
    let mut accepted_extra = Value::Null;
    for depth in 1..=MAX_JSON_NESTING_DEPTH {
        let value = nested_array(depth, json!("depth-127-canary"));
        raw_document["depthProbe"] = value.clone();
        if maximum_container_depth(&raw_document) == MAX_JSON_NESTING_DEPTH {
            accepted_extra = value;
            break;
        }
    }
    assert_ne!(accepted_extra, Value::Null);
    raw_document["depthProbe"] = accepted_extra;
    assert_eq!(
        maximum_container_depth(&raw_document),
        MAX_JSON_NESTING_DEPTH
    );
    let document = decode_document_value(&raw_document);
    let candidate = changed(materialize_document(
        &template,
        template.revision(),
        &document,
        LATER_AT.into(),
    )?);
    assert_eq!(
        document_wire_value(&candidate)["depthProbe"],
        raw_document["depthProbe"]
    );
    assert_eq!(
        document_wire_value(&candidate)["safeMarkerValue"],
        json!("$serde_json::private::Number")
    );

    let mut invalid_depth = document.clone();
    invalid_depth.corrupt_extra_depth_for_test(nested_array(
        MAX_JSON_NESTING_DEPTH,
        json!("source-depth-128-canary"),
    ));
    let depth_error = materialize_document(
        &template,
        template.revision(),
        &invalid_depth,
        LATER_AT.into(),
    )
    .expect_err("source whole-wire depth 128 must fail before preconditions");
    assert_eq!(
        depth_error.category(),
        DocumentMaterializationErrorCategory::InvalidDocument
    );
    assert_eq!(
        depth_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_error_redacted(&depth_error, &["source-depth-128-canary"]);

    let mut invalid_reserved = document.clone();
    invalid_reserved.corrupt_extra_for_test(
        "$serde_json::private::Number",
        json!("marker is a value only elsewhere"),
    );
    let reserved_error = materialize_document(
        &template,
        template.revision(),
        &invalid_reserved,
        LATER_AT.into(),
    )
    .expect_err("reserved serde namespace key must fail source admission");
    assert_eq!(
        reserved_error.category(),
        DocumentMaterializationErrorCategory::InvalidDocument
    );
    assert_eq!(
        reserved_error.artifact_validation_category(),
        Some(ArtifactValidationErrorCategory::ReservedExtraKey)
    );
    assert_error_redacted(
        &reserved_error,
        &[
            "$serde_json::private::Number",
            "marker is a value only elsewhere",
        ],
    );
    Ok(())
}

#[test]
fn finalization_required_unset_reports_bound_location_and_rich_text_location_is_available(
) -> Result<(), Box<dyn Error>> {
    let mut raw_template = historical_template_value(3);
    raw_template["fields"][TEXT]["required"] = json!(true);
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let required_error = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::ReplaceWithUnset(field_id(TEXT)),
    )
    .expect_err("required unset must fail final reconciliation");
    assert_eq!(
        required_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::RequiredValueUnset)
    );
    assert_eq!(
        required_error.issue_validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );

    let rich_error = materialize_document_with_final_corruption_for_test(
        &template,
        &document,
        LATER_AT.into(),
        FinalCandidateCorruption::SemanticEmptyRichText(field_id(RICH)),
    )
    .expect_err("semantic empty rich-text must fail final storage validation");
    assert_eq!(
        rich_error.artifact_rich_text_category(),
        Some(RichTextValidationErrorCategory::SemanticEmpty)
    );
    assert_eq!(
        rich_error.artifact_rich_text_location(),
        Some(ArtifactRichTextValueLocation::DocumentField)
    );
    assert_eq!(
        rich_error.artifact_rich_text_structure_location(),
        Some(RichTextErrorLocation::Root)
    );
    Ok(())
}

#[test]
fn finalization_rejects_storage_valid_but_still_materializable_candidate(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        3,
        "active",
    ));
    let document = decode_document_value(&document_value(
        [(TEXT, json!({"kind":"text","value":"existing"}))],
        [(
            TEXT,
            orphan_snapshot("metadata-free", "singleLineText", json!({})),
        )],
        3,
        TEMPLATE_ID,
    ));
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let prepared =
        prepare_materialization(&template, template.revision(), &document, LATER_AT.into())?;
    let mut candidate = prepared
        .candidate
        .expect("normal preparation must remove the reattachable snapshot");
    let snapshot = document
        .orphaned_field_definitions
        .get(&field_id(TEXT))
        .expect("source must contain the same-ID snapshot")
        .clone();
    assert!(!snapshot.contains_unknown_storage_data());
    assert!(snapshot.options().is_empty());
    assert!(candidate
        .orphaned_field_definitions
        .insert(field_id(TEXT), snapshot)
        .is_none());
    candidate.validate_storage()?;
    let final_view = crate::data::artifact::reconcile_document(&template, &candidate)?;
    assert!(final_view.can_materialize());
    assert!(final_view.blocking_issues().is_empty());
    assert!(final_view.materialization_required());
    assert_eq!(final_view.warnings().len(), 0);
    assert_eq!(candidate.template_revision(), template.revision());
    assert_eq!(candidate.updated_at_utc(), LATER_AT);

    let error = finalize_materialization(&template, candidate, prepared.warnings)
        .expect_err("an unsettled final candidate must not produce an outcome");
    assert_eq!(
        error.category(),
        DocumentMaterializationErrorCategory::InternalInvariant
    );
    assert_eq!(error.artifact_validation_category(), None);
    assert_eq!(error.issue_category(), None);
    assert_eq!(error.issue_count(), 0);
    assert_error_redacted(&error, &["metadata-free", "existing"]);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn metadata_rich_final_incomplete_error_redacts_every_preserved_source_layer(
) -> Result<(), Box<dyn Error>> {
    let (raw_template, mut raw_document) = full_redaction_values();
    assert_full_redaction_canaries(&raw_template, &raw_document);
    raw_document["orphanedFieldDefinitions"][TEXT] =
        orphan_snapshot("metadata-free final guard", "singleLineText", json!({}));

    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&raw_document);
    template.validate_storage()?;
    document.validate_storage()?;
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let prepared =
        prepare_materialization(&template, template.revision(), &document, LATER_AT.into())?;
    assert!(prepared.warnings.is_empty());
    let mut candidate = prepared
        .candidate
        .expect("normal metadata-rich preparation must create a private candidate");
    let compatible_snapshot = document
        .orphaned_field_definitions
        .get(&field_id(TEXT))
        .expect("source must contain the compatible same-ID snapshot")
        .clone();
    assert!(!compatible_snapshot.contains_unknown_storage_data());
    assert!(compatible_snapshot.options().is_empty());
    assert!(candidate
        .orphaned_field_definitions
        .insert(field_id(TEXT), compatible_snapshot)
        .is_none());
    candidate.validate_storage()?;
    let final_view = crate::data::artifact::reconcile_document(&template, &candidate)?;
    assert!(final_view.can_materialize());
    assert!(final_view.blocking_issues().is_empty());
    assert!(final_view.warnings().is_empty());
    assert!(final_view.materialization_required());
    assert_eq!(candidate.template_revision(), template.revision());
    assert_eq!(candidate.updated_at_utc(), LATER_AT);

    let error = finalize_materialization(&template, candidate, prepared.warnings)
        .expect_err("metadata-rich unsettled candidate must not escape finalization");
    assert_eq!(
        error.category(),
        DocumentMaterializationErrorCategory::InternalInvariant
    );
    assert_eq!(error.artifact_validation_category(), None);
    assert_eq!(error.issue_category(), None);
    assert_eq!(error.issue_count(), 0);
    let escaped_windows = WINDOWS_CANARY.escape_debug().to_string();
    let escaped_rich_windows = RICH_WINDOWS_CANARY.escape_debug().to_string();
    let forbidden = [
        "template-root-secret",
        "document-root-secret",
        "field-definition-secret",
        "field-value-secret",
        BASE_SCALAR_PAYLOAD,
        "futureTemplate",
        "futureDocument",
        "futureField",
        "futureValueOuter",
        "futureKnownOption",
        "futurePresentation",
        "futureConfiguration",
        INITIAL_RICH_PAYLOAD,
        "outer-secret",
        "futureEnvelope",
        "futureRoot",
        "futureContainer",
        "futureText",
        "credential=orphan-field-label",
        "credential=orphan-option-one",
        "credential=orphan-option-two",
        "credential=orphan-option-three",
        UNKNOWN_OPTION,
        "c0000000-0000",
        FIELD_INTEGER_NUMBER,
        ORPHAN_INTEGER_NUMBER,
        ORPHAN_DECIMAL_NUMBER,
        EXPONENT_NUMBER,
        DOCUMENT_DECIMAL_NUMBER,
        "https://example.invalid",
        WINDOWS_CANARY,
        escaped_windows.as_str(),
        RICH_WINDOWS_CANARY,
        escaped_rich_windows.as_str(),
        "M2_5D",
        UNIX_CANARY,
        "srv/m2_5d",
    ];
    assert_redaction_fragments_are_observable(&forbidden);
    assert_error_redacted(&error, &forbidden);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn lossy_reattachment_checks_every_option_and_does_not_union_matching_template_extra(
) -> Result<(), Box<dyn Error>> {
    // U1: 세 선택이 모두 active일 때만 snapshot 제거가 필요하므로 마지막 Option의 extra를
    // 검사하는 기존 실패 경로도 all-active fixture에서 유지한다.
    let mut configuration = multi_configuration();
    configuration["options"][MULTI_ARCHIVED]["lifecycle"] = json!("active");
    configuration["optionOrder"] = json!([MULTI_A, MULTI_B, MULTI_ARCHIVED]);
    let template = decode_template_value(&template_value(
        [(
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[MULTI_A]}),
                json!({"kind":"unset"}),
                1,
                configuration,
            ),
        )],
        &[MULTI],
        3,
        "active",
    ));
    let trailing_key = "futureTrailingOption";
    let trailing_value = "credential=trailing-option-only";
    let document = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[MULTI_A,MULTI_B,MULTI_ARCHIVED]}),
        )],
        [(
            MULTI,
            json!({
                "label":"three selected options",
                "kind":"multiChoice",
                "options":{
                    (MULTI_A):{"label":"first"},
                    (MULTI_B):{"label":"second"},
                    (MULTI_ARCHIVED):{
                        "label":"third",
                        (trailing_key):{"value":trailing_value}
                    }
                }
            }),
        )],
        1,
        TEMPLATE_ID,
    ));
    document.validate_storage()?;
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);
    let wire = document_wire_value(&document);
    assert_eq!(
        wire["fieldValues"][MULTI]["optionIds"],
        json!([MULTI_A, MULTI_B, MULTI_ARCHIVED])
    );
    assert_eq!(
        wire["orphanedFieldDefinitions"][MULTI]["options"]
            .as_object()
            .expect("Option snapshots must be an object")
            .len(),
        3
    );
    assert_eq!(
        wire["orphanedFieldDefinitions"][MULTI]["options"][MULTI_ARCHIVED][trailing_key]["value"],
        json!(trailing_value)
    );
    let view = crate::data::artifact::reconcile_document(&template, &document)?;
    assert!(!view.can_materialize());
    assert_eq!(view.blocking_issues().len(), 1);
    assert_eq!(
        view.blocking_issues()[0].category(),
        DocumentReconciliationIssueCategory::LossyOrphanReattachment
    );
    let error = materialize_document(&template, template.revision(), &document, LATER_AT.into())
        .expect_err("unknown metadata on the last Option must block removal");
    assert_eq!(
        error.issue_category(),
        Some(DocumentReconciliationIssueCategory::LossyOrphanReattachment)
    );
    assert_error_redacted(&error, &[trailing_key, trailing_value, MULTI_ARCHIVED]);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );

    let shared_key = "futureSharedMetadata";
    let shared_value = exact_number(EXPONENT_NUMBER);
    let mut raw_template = template_value(
        [(
            TEXT,
            field(
                "singleLineText",
                "active",
                false,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("singleLineText"),
            ),
        )],
        &[TEXT],
        3,
        "active",
    );
    raw_template["fields"][TEXT][shared_key] = shared_value.clone();
    let shared_template = decode_template_value(&raw_template);
    let raw_document = document_value(
        [(TEXT, json!({"kind":"text","value":"compatible"}))],
        [(
            TEXT,
            json!({
                "label":"same metadata still has a distinct storage location",
                "kind":"singleLineText",
                "options":{},
                (shared_key):shared_value
            }),
        )],
        1,
        TEMPLATE_ID,
    );
    assert_eq!(
        raw_template["fields"][TEXT][shared_key],
        raw_document["orphanedFieldDefinitions"][TEXT][shared_key]
    );
    let shared_document = decode_document_value(&raw_document);
    shared_template.validate_storage()?;
    shared_document.validate_storage()?;
    let template_before = shared_template.clone();
    let document_before = shared_document.clone();
    let template_snapshot = template_bytes(&shared_template);
    let document_snapshot = document_bytes(&shared_document);
    let shared_error = materialize_document(
        &shared_template,
        shared_template.revision(),
        &shared_document,
        LATER_AT.into(),
    )
    .expect_err("matching unknown metadata in the Template is not the same storage location");
    assert_eq!(
        shared_error.issue_category(),
        Some(DocumentReconciliationIssueCategory::LossyOrphanReattachment)
    );
    assert_error_redacted(&shared_error, &[shared_key, EXPONENT_NUMBER]);
    assert_sources_unchanged(
        &shared_template,
        &template_before,
        &template_snapshot,
        &shared_document,
        &document_before,
        &document_snapshot,
    );
    Ok(())
}

#[test]
fn distinct_raw_object_member_orders_decode_and_materialize_identically(
) -> Result<(), Box<dyn Error>> {
    let raw_a = raw_order_document_bytes(false);
    let raw_b = raw_order_document_bytes(true);
    let raw_a_before = raw_a.clone();
    let raw_b_before = raw_b.clone();
    assert_ne!(raw_a, raw_b);
    assert_object_member_precedes(&raw_a, "fieldValues", TEXT, ORPHAN);
    assert_object_member_precedes(&raw_b, "fieldValues", ORPHAN, TEXT);
    assert_object_member_precedes(&raw_a, "orphanedFieldDefinitions", TEXT, ORPHAN);
    assert_object_member_precedes(&raw_b, "orphanedFieldDefinitions", ORPHAN, TEXT);
    assert_raw_member_precedes(&raw_a, "\"rawAlpha\"", "\"rawNumber\"");
    assert_raw_member_precedes(&raw_a, "\"rawNumber\"", "\"rawOmega\"");
    assert_raw_member_precedes(&raw_b, "\"rawOmega\"", "\"rawNumber\"");
    assert_raw_member_precedes(&raw_b, "\"rawNumber\"", "\"rawAlpha\"");

    let raw_value_a: Value = serde_json::from_slice(&raw_a)?;
    let raw_value_b: Value = serde_json::from_slice(&raw_b)?;
    assert_eq!(
        raw_value_a, raw_value_b,
        "object member order must be the only persisted input difference"
    );
    assert_eq!(
        raw_value_a["fieldValues"][ORPHAN]["futureValueOuter"]["array"],
        json!([3, 1, 2])
    );
    assert_eq!(
        raw_value_a["orphanedFieldDefinitions"][ORPHAN]["futureSnapshot"]["array"],
        json!([3, 1, 2])
    );
    assert_eq!(
        raw_value_a["futureRawOrder"]["rawNumber"]
            .as_number()
            .expect("raw nested exponent must be a Number")
            .to_string(),
        EXPONENT_NUMBER
    );

    let document_a = decode_document(&raw_a)?;
    let document_b = decode_document(&raw_b)?;
    assert_eq!(document_a, document_b);
    document_a.validate_storage()?;
    document_b.validate_storage()?;
    let document_a_before = document_a.clone();
    let document_b_before = document_b.clone();
    let canonical_a_before = encode_document(&document_a)?;
    let canonical_b_before = encode_document(&document_b)?;
    assert_eq!(canonical_a_before, canonical_b_before);
    assert!(String::from_utf8(canonical_a_before.clone())?.contains(EXPONENT_NUMBER));

    let template = decode_template_value(&template_value(
        [
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"current text"}),
                    json!({"kind":"unset"}),
                    1,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                NUMBER,
                field(
                    "number",
                    "active",
                    false,
                    json!({"kind":"number","value":"1"}),
                    json!({
                        "kind":"number",
                        "value":"2",
                        "futureExponent":exact_number(EXPONENT_NUMBER)
                    }),
                    2,
                    basic_configuration("number"),
                ),
            ),
        ],
        &[TEXT, NUMBER],
        3,
        "active",
    ));
    template.validate_storage()?;
    let template_before = template.clone();
    let template_snapshot = template_bytes(&template);

    let outcome_a =
        materialize_document(&template, template.revision(), &document_a, LATER_AT.into())?;
    let outcome_b =
        materialize_document(&template, template.revision(), &document_b, LATER_AT.into())?;
    assert_eq!(
        outcome_a.kind(),
        DocumentMaterializationOutcomeKind::Changed
    );
    assert_eq!(
        outcome_b.kind(),
        DocumentMaterializationOutcomeKind::Changed
    );
    assert_eq!(
        outcome_a.warnings().as_slice(),
        outcome_b.warnings().as_slice()
    );
    assert!(outcome_a.warnings().is_empty());
    let changed_a = outcome_a
        .into_changed()
        .expect("raw-order A must preserve candidate with warnings");
    let changed_b = outcome_b
        .into_changed()
        .expect("raw-order B must preserve candidate with warnings");
    assert_eq!(changed_a.document(), changed_b.document());
    let candidate_a = changed_a.document();
    let candidate_b = changed_b.document();
    let candidate_a_wire = document_wire_value(candidate_a);
    let source_wire = document_wire_value(&document_a);
    assert_eq!(
        candidate_a_wire["fieldValues"][NUMBER],
        template_wire_value(&template)["fields"][NUMBER]["initialDefaultValue"]
    );
    assert_eq!(
        candidate_a_wire["fieldValues"][TEXT],
        source_wire["fieldValues"][TEXT]
    );
    assert!(candidate_a_wire["orphanedFieldDefinitions"]
        .get(TEXT)
        .is_none());
    assert_eq!(
        candidate_a_wire["fieldValues"][ORPHAN],
        source_wire["fieldValues"][ORPHAN]
    );
    assert_eq!(
        candidate_a_wire["orphanedFieldDefinitions"][ORPHAN],
        source_wire["orphanedFieldDefinitions"][ORPHAN]
    );
    assert_eq!(
        candidate_a_wire["futureRawOrder"],
        source_wire["futureRawOrder"]
    );
    assert_eq!(
        candidate_a_wire["futureRawOrder"]["rawNumber"]
            .as_number()
            .expect("candidate nested exponent must remain a Number")
            .to_string(),
        EXPONENT_NUMBER
    );
    let candidate_bytes = encode_document(candidate_a)?;
    assert_eq!(encode_document(candidate_b)?, candidate_bytes);
    assert!(String::from_utf8(candidate_bytes.clone())?.contains(EXPONENT_NUMBER));
    assert_eq!(
        encode_document(&decode_document(&candidate_bytes)?)?,
        candidate_bytes
    );

    let second_a =
        materialize_document(&template, template.revision(), candidate_a, LATER_AT.into())?;
    let second_b =
        materialize_document(&template, template.revision(), candidate_b, LATER_AT.into())?;
    assert_eq!(
        second_a.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert_eq!(
        second_b.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(second_a.document().is_none());
    assert!(second_b.document().is_none());
    assert_eq!(
        second_a.warnings().as_slice(),
        second_b.warnings().as_slice()
    );
    assert_eq!(encode_document(candidate_a)?, candidate_bytes);
    assert_eq!(encode_document(candidate_b)?, candidate_bytes);
    assert_eq!(candidate_a.updated_at_utc(), LATER_AT);
    assert_eq!(candidate_b.updated_at_utc(), LATER_AT);

    assert_eq!(template, template_before);
    assert_eq!(template_bytes(&template), template_snapshot);
    assert_eq!(document_a, document_a_before);
    assert_eq!(document_b, document_b_before);
    assert_eq!(encode_document(&document_a)?, canonical_a_before);
    assert_eq!(encode_document(&document_b)?, canonical_b_before);
    assert_eq!(raw_a, raw_a_before);
    assert_eq!(raw_b, raw_b_before);
    Ok(())
}

#[test]
fn multiple_historical_reattachments_and_unknown_choice_orphan_are_lossless_and_deterministic(
) -> Result<(), Box<dyn Error>> {
    let (raw_template, raw_document) = multi_materialization_values(false);
    let source_exponent = &raw_template["fields"][NUMBER]["initialDefaultValue"]["futureExponent"];
    assert!(source_exponent.is_number());
    assert_eq!(
        source_exponent
            .as_number()
            .expect("exponent metadata must be a JSON Number")
            .to_string(),
        EXPONENT_NUMBER
    );
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&raw_document);
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);
    let source_wire = document_wire_value(&document);
    template.validate_storage()?;
    document.validate_storage()?;

    let outcome = materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert_eq!(outcome.warnings().len(), 1);
    assert_eq!(
        outcome.warnings()[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(outcome.warnings()[0].field_id(), field_id(SINGLE));
    let changed = outcome
        .into_changed()
        .expect("Changed must retain candidate and warnings in one bundle");
    assert_eq!(changed.warnings().len(), 1);
    let candidate = changed.document();
    let candidate_wire = document_wire_value(candidate);

    for historical in [TEXT, RICH, NUMBER] {
        assert_ne!(
            raw_template["fields"][historical]["defaultValue"],
            raw_template["fields"][historical]["initialDefaultValue"]
        );
        assert_eq!(
            candidate_wire["fieldValues"][historical],
            raw_template["fields"][historical]["initialDefaultValue"]
        );
    }
    for reattached in [SINGLE, MULTI, ARCHIVED] {
        assert_eq!(
            candidate_wire["fieldValues"][reattached],
            source_wire["fieldValues"][reattached]
        );
        if reattached == MULTI {
            assert!(candidate_wire["orphanedFieldDefinitions"]
                .get(reattached)
                .is_none());
        } else {
            assert!(
                candidate_wire["orphanedFieldDefinitions"][reattached]
                    == source_wire["orphanedFieldDefinitions"][reattached]
            );
        }
    }
    for preserved in [DURATION, TIME, ORPHAN] {
        assert_eq!(
            candidate_wire["fieldValues"][preserved],
            source_wire["fieldValues"][preserved]
        );
    }
    assert_eq!(
        candidate_wire["orphanedFieldDefinitions"][ORPHAN],
        source_wire["orphanedFieldDefinitions"][ORPHAN]
    );
    assert_eq!(
        candidate_wire["fieldValues"][ORPHAN]["optionIds"],
        json!([UNKNOWN_OPTION, UNKNOWN_OPTION_TWO, UNKNOWN_OPTION_THREE])
    );
    assert_eq!(
        candidate_wire["orphanedFieldDefinitions"][ORPHAN]["options"]
            .as_object()
            .expect("all unknown Option snapshots must remain")
            .len(),
        3
    );
    assert_eq!(candidate.template_revision(), template.revision());
    assert_eq!(candidate.updated_at_utc(), LATER_AT);
    candidate.validate_storage()?;
    let final_view = crate::data::artifact::reconcile_document(&template, candidate)?;
    assert!(final_view.can_materialize());
    assert!(final_view.blocking_issues().is_empty());
    assert!(!final_view.materialization_required());
    assert_eq!(final_view.warnings().len(), 1);
    let encoded = encode_document(candidate)?;
    let encoded_text = String::from_utf8(encoded.clone())?;
    assert!(encoded_text.contains(EXPONENT_NUMBER));
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);

    let second = materialize_document(&template, template.revision(), candidate, LATER_AT.into())?;
    assert_eq!(second.kind(), DocumentMaterializationOutcomeKind::Unchanged);
    assert_eq!(second.warnings().len(), 1);
    assert!(second.document().is_none());
    assert_eq!(document_bytes(candidate), encoded);
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );

    let (reverse_template_value, reverse_document_value) = multi_materialization_values(true);
    assert_ne!(
        raw_template["fieldOrder"],
        reverse_template_value["fieldOrder"]
    );
    let reverse_template = decode_template_value(&reverse_template_value);
    let reverse_document = decode_document_value(&reverse_document_value);
    let reverse_template_before = reverse_template.clone();
    let reverse_document_before = reverse_document.clone();
    let reverse_template_snapshot = template_bytes(&reverse_template);
    let reverse_document_snapshot = document_bytes(&reverse_document);
    let reverse_outcome = materialize_document(
        &reverse_template,
        reverse_template.revision(),
        &reverse_document,
        LATER_AT.into(),
    )?;
    assert_eq!(reverse_outcome.warnings().len(), 1);
    assert_eq!(
        reverse_outcome.warnings()[0].field_id(),
        outcome_warning_field_id(&second)
    );
    let reverse_changed = reverse_outcome
        .into_changed()
        .expect("reversed insertion/order fixture must also change");
    assert_eq!(document_bytes(reverse_changed.document()), encoded);
    assert_eq!(
        reverse_changed.warnings().as_slice(),
        second.warnings().as_slice()
    );
    let reverse_second = materialize_document(
        &reverse_template,
        reverse_template.revision(),
        reverse_changed.document(),
        LATER_AT.into(),
    )?;
    assert_eq!(
        reverse_second.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(reverse_second.document().is_none());
    assert_eq!(
        reverse_second.warnings().as_slice(),
        second.warnings().as_slice()
    );
    assert_eq!(document_bytes(reverse_changed.document()), encoded);
    assert_sources_unchanged(
        &reverse_template,
        &reverse_template_before,
        &reverse_template_snapshot,
        &reverse_document,
        &reverse_document_before,
        &reverse_document_snapshot,
    );
    Ok(())
}

#[test]
fn historical_materialization_transfers_initial_value_number_provenance_only(
) -> Result<(), Box<dyn Error>> {
    let mut text_initial = json!({
        "kind":"text",
        "value":"historical text",
        "futureOuter":{"nested":[0,{"value":0}]}
    });
    text_initial["futureOuter"]["nested"][0] = json!("__TEXT_EXPONENT__");
    text_initial["futureOuter"]["nested"][1]["value"] = json!("__TEXT_ZERO__");

    let mut rich_initial = rich_text("historical rich", true);
    rich_initial["document"]["futureEnvelope"]["decimal"] = json!("__RICH_ENVELOPE__");
    rich_initial["document"]["content"]["children"][0]["futureContainer"]["number"] =
        json!("__RICH_BLOCK__");
    rich_initial["document"]["content"]["children"][0]["children"][0]["futureText"]["number"] =
        json!("__RICH_NODE__");

    let mut raw_template = template_value(
        [
            (
                TEXT,
                field(
                    "singleLineText",
                    "active",
                    false,
                    json!({"kind":"text","value":"current text"}),
                    text_initial,
                    2,
                    basic_configuration("singleLineText"),
                ),
            ),
            (
                RICH,
                field(
                    "richText",
                    "active",
                    false,
                    rich_text("current rich", false),
                    rich_initial,
                    2,
                    basic_configuration("richText"),
                ),
            ),
        ],
        &[TEXT, RICH],
        2,
        "active",
    );
    raw_template["templateOnlyNumber"] = json!("__TEMPLATE_NUMBER__");
    let mut raw_template = String::from_utf8(to_deterministic_json_bytes(&raw_template)?)?;
    for (marker, token) in [
        ("__TEXT_EXPONENT__", "1E9223372036854775808"),
        ("__TEXT_ZERO__", "-0E-999999999999999999999"),
        ("__RICH_ENVELOPE__", "1e+0009223372036854775808"),
        ("__RICH_BLOCK__", "7E-100"),
        ("__RICH_NODE__", "6e+100"),
        ("__TEMPLATE_NUMBER__", "5E100"),
    ] {
        raw_template = raw_template.replace(&format!("\"{marker}\""), token);
    }
    let template = decode_template(raw_template.as_bytes())?;

    let mut raw_document = document_value([], [], 1, TEMPLATE_ID);
    raw_document["futureDocument"]["number"] = json!("__DOCUMENT_NUMBER__");
    let raw_document = String::from_utf8(to_deterministic_json_bytes(&raw_document)?)?
        .replace("\"__DOCUMENT_NUMBER__\"", "4E100");
    let document = decode_document(raw_document.as_bytes())?;
    let template_before = template.clone();
    let document_before = document.clone();
    let template_bytes = encode_template(&template)?;
    let document_bytes = encode_document(&document)?;

    let outcome = materialize_document(
        &template,
        template.revision(),
        &document,
        LATER_AT.to_owned(),
    )?;
    let candidate = outcome
        .document()
        .expect("historical values require a change");
    let encoded = encode_document(candidate)?;
    let text = std::str::from_utf8(&encoded)?;
    for token in [
        "1E9223372036854775808",
        "-0E-999999999999999999999",
        "1e+0009223372036854775808",
        "7E-100",
        "6e+100",
        "4E100",
    ] {
        assert!(text.contains(token), "materialized lexeme changed: {token}");
    }
    assert!(!text.contains("templateOnlyNumber"));
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    assert!(materialize_document(
        &template,
        template.revision(),
        candidate,
        LATER_AT.to_owned(),
    )?
    .document()
    .is_none());
    assert_eq!(template, template_before);
    assert_eq!(document, document_before);
    assert_eq!(encode_template(&template)?, template_bytes);
    assert_eq!(encode_document(&document)?, document_bytes);
    Ok(())
}

fn outcome_warning_field_id(outcome: &DocumentMaterializationOutcome) -> FieldId {
    assert_eq!(outcome.warnings().len(), 1);
    outcome.warnings()[0].field_id()
}

#[test]
fn large_archived_selection_preserves_bounded_warning_without_reordering(
) -> Result<(), Box<dyn Error>> {
    for (option_count, truncated) in [(1_024_u32, false), (1_025, true)] {
        let mut options = Map::new();
        let mut selected = Vec::new();
        for index in 0..option_count {
            let raw = format!("d0000000-{:04x}-4000-8000-{:012x}", index / 65_536, index);
            options.insert(
                raw.clone(),
                json!({"label":"private archived","lifecycle":"archived"}),
            );
            selected.push(raw);
        }
        let template = decode_template_value(&template_value(
            [(
                MULTI,
                field(
                    "multiChoice",
                    "active",
                    false,
                    json!({"kind":"unset"}),
                    json!({"kind":"unset"}),
                    1,
                    json!({"kind":"multiChoice","optionOrder":[],"options":options}),
                ),
            )],
            &[MULTI],
            1,
            "active",
        ));
        let document = decode_document_value(&document_value(
            [(MULTI, json!({"kind":"multiChoice","optionIds":selected}))],
            [],
            1,
            TEMPLATE_ID,
        ));
        let started = std::time::Instant::now();
        let outcome =
            materialize_document(&template, template.revision(), &document, LATER_AT.into())?;
        eprintln!(
            "M2-5d representative {option_count}-option reconciliation: {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
        assert_eq!(outcome.warnings().len(), 1);
        assert_eq!(outcome.warnings()[0].count(), 1_024);
        assert_eq!(outcome.warnings()[0].truncated(), truncated);
        let candidate = outcome.document().expect("U1 creates selected snapshots");
        assert_eq!(
            candidate.orphaned_field_definitions()[&field_id(MULTI)]
                .options()
                .len(),
            option_count as usize
        );
        assert!(
            candidate.field_values()[&field_id(MULTI)] == document.field_values()[&field_id(MULTI)]
        );
        let repeated =
            materialize_document(&template, template.revision(), candidate, LATER_AT.into())?;
        assert_eq!(
            repeated.kind(),
            DocumentMaterializationOutcomeKind::Unchanged
        );
        assert!(repeated.warnings() == outcome.warnings());
        assert_eq!(document_bytes(&document), encode_document(&document)?);
    }
    Ok(())
}
