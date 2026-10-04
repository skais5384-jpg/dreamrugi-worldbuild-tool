use std::{error::Error, str::FromStr};

use serde_json::{json, Map, Value};

use super::*;
use crate::data::{
    artifact::{
        decode_document, decode_template, document::DocumentWire, encode_document, encode_template,
        template::TemplateWire, ArtifactValidationErrorCategory,
    },
    field_engine::{
        choice::ChoiceValidationErrorCategory,
        scalar::ScalarValueErrorCategory,
        validation::{FieldValidationErrorCategory, FieldValidationLocation},
    },
    json::to_deterministic_json_bytes,
};

const TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000001";
const OTHER_TEMPLATE_ID: &str = "10000000-0000-4000-8000-000000000002";
const DOCUMENT_ID: &str = "90000000-0000-4000-8000-000000000001";
const TIMESTAMP: &str = "2026-09-05T12:34:56.789Z";

const TEXT: &str = "20000000-0000-4000-8000-000000000001";
const RICH: &str = "20000000-0000-4000-8000-000000000002";
const NUMBER: &str = "20000000-0000-4000-8000-000000000003";
const DATE: &str = "20000000-0000-4000-8000-000000000004";
const TIME: &str = "20000000-0000-4000-8000-000000000005";
const DURATION: &str = "20000000-0000-4000-8000-000000000006";
const SINGLE: &str = "20000000-0000-4000-8000-000000000007";
const MULTI: &str = "20000000-0000-4000-8000-000000000008";
const ORPHAN_A: &str = "80000000-0000-4000-8000-000000000001";
const ORPHAN_B: &str = "80000000-0000-4000-8000-000000000002";
const ORPHAN_C: &str = "80000000-0000-4000-8000-000000000003";
const OPTION_ACTIVE: &str = "a0000000-0000-4000-8000-000000000001";
const OPTION_ARCHIVED: &str = "a0000000-0000-4000-8000-000000000002";
const OPTION_UNKNOWN: &str = "a0000000-0000-4000-8000-000000000003";
const OPTION_OTHER: &str = "a0000000-0000-4000-8000-000000000004";
const OPTION_SINGLE_ACTIVE: &str = "b0000000-0000-4000-8000-000000000001";
const OPTION_SINGLE_ARCHIVED: &str = "b0000000-0000-4000-8000-000000000002";
const OPTION_SINGLE_OTHER: &str = "b0000000-0000-4000-8000-000000000003";
const ORPHAN_OPTION: &str = "c0000000-0000-4000-8000-000000000001";

fn id(raw: &str) -> FieldId {
    FieldId::from_str(raw).expect("fixture FieldId must be valid")
}

fn deterministic(value: &Value) -> Vec<u8> {
    to_deterministic_json_bytes(value).expect("fixture JSON must encode")
}

fn basic_configuration(kind: &str) -> Value {
    json!({"kind":kind})
}

fn choice_configuration(kind: &str) -> Value {
    json!({
        "kind":kind,
        "optionOrder":[OPTION_ACTIVE,OPTION_OTHER],
        "options":{
            (OPTION_ACTIVE):{"label":"credential=active-label","lifecycle":"active"},
            (OPTION_ARCHIVED):{"label":"credential=archived-label","lifecycle":"archived"},
            (OPTION_OTHER):{"label":"other active","lifecycle":"active"}
        }
    })
}

fn single_choice_configuration() -> Value {
    json!({
        "kind":"singleChoice",
        "optionOrder":[OPTION_SINGLE_ACTIVE,OPTION_SINGLE_OTHER],
        "options":{
            (OPTION_SINGLE_ACTIVE):{"label":"single active","lifecycle":"active"},
            (OPTION_SINGLE_ARCHIVED):{"label":"single archived","lifecycle":"archived"},
            (OPTION_SINGLE_OTHER):{"label":"single other","lifecycle":"active"}
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
        "presentation":{}
    })
}

fn template_value(
    fields: impl IntoIterator<Item = (&'static str, Value)>,
    field_order: &[&str],
    revision: u32,
    lifecycle: &str,
) -> Value {
    let fields = fields
        .into_iter()
        .map(|(field_id, definition)| (field_id.to_owned(), definition))
        .collect::<Map<_, _>>();
    json!({
        "schemaVersion":1,
        "artifactType":"template",
        "templateId":TEMPLATE_ID,
        "revision":revision,
        "name":"credential=template-name",
        "lifecycle":lifecycle,
        "fieldOrder":field_order,
        "fields":fields,
        "presentation":{},
        "createdAtUtc":TIMESTAMP,
        "updatedAtUtc":TIMESTAMP
    })
}

fn document_value(
    field_values: impl IntoIterator<Item = (&'static str, Value)>,
    orphan_snapshots: impl IntoIterator<Item = (&'static str, Value)>,
    revision: u32,
    template_id: &str,
) -> Value {
    let field_values = field_values
        .into_iter()
        .map(|(field_id, value)| (field_id.to_owned(), value))
        .collect::<Map<_, _>>();
    let orphan_snapshots = orphan_snapshots
        .into_iter()
        .map(|(field_id, value)| (field_id.to_owned(), value))
        .collect::<Map<_, _>>();
    json!({
        "schemaVersion":1,
        "artifactType":"document",
        "documentId":DOCUMENT_ID,
        "templateId":template_id,
        "templateRevision":revision,
        "name":"credential=document-name C:\\private\\document.json",
        "fieldValues":field_values,
        "orphanedFieldDefinitions":orphan_snapshots,
        "createdAtUtc":TIMESTAMP,
        "updatedAtUtc":TIMESTAMP,
        "futureDocument":{"url":"https://private.example.invalid/token"}
    })
}

fn decode_template_value(value: &Value) -> TemplateArtifact {
    decode_template(&deterministic(value)).expect("fixture Template must be valid")
}

fn decode_document_value(value: &Value) -> DocumentArtifact {
    decode_document(&deterministic(value)).expect("fixture Document must be valid")
}

fn template_bytes(value: &TemplateArtifact) -> Vec<u8> {
    encode_template(value).expect("admitted Template must encode")
}

fn document_bytes(value: &DocumentArtifact) -> Vec<u8> {
    encode_document(value).expect("admitted Document must encode")
}

fn unchecked_template_bytes(value: &TemplateArtifact) -> Vec<u8> {
    serde_json::to_vec(&TemplateWire::from(value)).expect("test snapshot must serialize")
}

fn unchecked_document_bytes(value: &DocumentArtifact) -> Vec<u8> {
    serde_json::to_vec(&DocumentWire::from(value)).expect("test snapshot must serialize")
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

fn assert_redacted(rendered: &str) {
    for secret in [
        "credential=",
        "private.example.invalid",
        "C:\\private",
        "/private/",
        OPTION_ACTIVE,
        OPTION_ARCHIVED,
        OPTION_UNKNOWN,
        "123456789012345678901234567890",
        "rich-secret",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}");
    }
}

fn first_leak<'a>(rendered: &str, forbidden: &'a [&'a str]) -> Option<&'a str> {
    forbidden
        .iter()
        .copied()
        .find(|candidate| rendered.contains(candidate))
}

fn assert_redacted_from(rendered: &str, forbidden: &[&str]) {
    assert_eq!(first_leak(rendered, forbidden), None, "Debug leaked data");
}

fn assert_fatal_is_sourceless(
    error: DocumentReconciliationError,
    expected: DocumentReconciliationErrorCategory,
) {
    assert_eq!(error.category(), expected);
    assert!(error.source().is_none());
    assert_redacted(&format!("{error} {error:?}"));
}

fn assert_fatal_redacted_from(
    error: DocumentReconciliationError,
    expected: DocumentReconciliationErrorCategory,
    forbidden: &[&str],
) {
    assert_eq!(error.category(), expected);
    assert_redacted_from(&format!("{error}"), forbidden);
    assert_redacted_from(&format!("{error:?}"), forbidden);
    assert!(error.source().is_none());
}

fn exact_number(raw: &str) -> Value {
    serde_json::from_str(raw).expect("number canary must parse without an f64 conversion")
}

fn sensitive_template_value(lifecycle: &str) -> Value {
    let field_lifecycle = if lifecycle == "deleted" {
        "archived"
    } else {
        "active"
    };
    let order = if lifecycle == "deleted" {
        Vec::new()
    } else {
        vec![TEXT, SINGLE]
    };
    let mut choice = single_choice_configuration();
    choice["m2cConfigurationKey"] = json!("m2c-configuration-value");
    choice["options"][OPTION_SINGLE_ACTIVE]["m2cKnownOptionKey"] = json!("m2c-known-option-value");
    let mut text = text_field(field_lifecycle, false, 1);
    text["label"] = json!("m2c-sensitive-text-label");
    text["m2cFieldDefinitionKey"] = json!("m2c-field-definition-value");
    let mut choice_field = field(
        "singleChoice",
        field_lifecycle,
        false,
        json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
        json!({"kind":"unset"}),
        1,
        choice,
    );
    choice_field["label"] = json!("m2c-sensitive-choice-label");
    choice_field["configuration"]["options"][OPTION_SINGLE_ACTIVE]["label"] =
        json!("m2c-sensitive-known-option-label");
    choice_field["presentation"]["token"] = json!("m2c-field-presentation-token");
    choice_field["presentation"]["m2cFieldPresentationKey"] = json!("m2c-field-presentation-value");
    let mut raw = template_value([(TEXT, text), (SINGLE, choice_field)], &order, 1, lifecycle);
    raw["name"] = json!("credential=m2c-template-name");
    raw["presentation"]["token"] = json!("m2c-template-presentation-token");
    raw["presentation"]["m2cTemplatePresentationKey"] = json!("m2c-template-presentation-value");
    raw["m2cTemplateRootKey"] = json!({
        "value":"m2c-template-root-value",
        "url":"https://m2c-template.invalid/private",
        "windows":r"C:\M2C_TEMPLATE_PATH\secret.txt",
        "unix":"/tmp/M2C_TEMPLATE_UNIX/secret.txt",
        "integer":exact_number("123456789012345678901234567890123456789"),
        "decimal":exact_number("999.0001"),
        "exponent":exact_number("9.87654321098765432109876543210987654321e+210")
    });
    raw
}

fn assert_sensitive_template_fixture(raw: &Value) {
    assert_eq!(raw["name"], json!("credential=m2c-template-name"));
    assert_eq!(
        raw["m2cTemplateRootKey"]["value"],
        json!("m2c-template-root-value")
    );
    assert_eq!(
        raw["fields"][TEXT]["m2cFieldDefinitionKey"],
        json!("m2c-field-definition-value")
    );
    assert_eq!(
        raw["fields"][TEXT]["label"],
        json!("m2c-sensitive-text-label")
    );
    assert_eq!(
        raw["fields"][SINGLE]["label"],
        json!("m2c-sensitive-choice-label")
    );
    assert_eq!(
        raw["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_ACTIVE]
            ["m2cKnownOptionKey"],
        json!("m2c-known-option-value")
    );
    assert_eq!(
        raw["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_ACTIVE]["label"],
        json!("m2c-sensitive-known-option-label")
    );
    assert_eq!(
        raw["fields"][SINGLE]["presentation"]["token"],
        json!("m2c-field-presentation-token")
    );
    assert_eq!(
        raw["fields"][SINGLE]["presentation"]["m2cFieldPresentationKey"],
        json!("m2c-field-presentation-value")
    );
    assert_eq!(
        raw["fields"][SINGLE]["configuration"]["m2cConfigurationKey"],
        json!("m2c-configuration-value")
    );
    assert_eq!(
        raw["presentation"]["token"],
        json!("m2c-template-presentation-token")
    );
    assert_eq!(
        raw["presentation"]["m2cTemplatePresentationKey"],
        json!("m2c-template-presentation-value")
    );
    assert_eq!(
        raw["m2cTemplateRootKey"]["url"],
        json!("https://m2c-template.invalid/private")
    );
    assert_eq!(
        raw["m2cTemplateRootKey"]["windows"],
        json!(r"C:\M2C_TEMPLATE_PATH\secret.txt")
    );
    assert_eq!(
        raw["m2cTemplateRootKey"]["unix"],
        json!("/tmp/M2C_TEMPLATE_UNIX/secret.txt")
    );
    for (key, lexeme) in [
        ("integer", "123456789012345678901234567890123456789"),
        ("decimal", "999.0001"),
        ("exponent", "9.87654321098765432109876543210987654321e+210"),
    ] {
        let number = raw["m2cTemplateRootKey"][key]
            .as_number()
            .expect("canary must remain a JSON Number");
        assert_eq!(number.to_string(), lexeme);
    }
    let encoded = deterministic(raw);
    let encoded = std::str::from_utf8(&encoded).expect("fixture JSON is UTF-8");
    for lexeme in [
        "123456789012345678901234567890123456789",
        "999.0001",
        "9.87654321098765432109876543210987654321e+210",
    ] {
        assert!(encoded.contains(lexeme));
    }
}

fn sensitive_document_value(template_id: &str, revision: u32) -> Value {
    let mut scalar = json!({"kind":"text","value":"m2c-scalar-payload"});
    scalar["m2cFieldValueOuterKey"] = json!("m2c-field-value-outer-value");
    let choice = json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE});
    let mut rich = rich_text("m2c-rich-payload");
    rich["m2cRichOuterKey"] = json!("m2c-rich-outer-value");
    rich["document"]["m2cRichEnvelopeKey"] = json!("m2c-rich-envelope-value");
    rich["document"]["content"]["m2cRichRootKey"] = json!("m2c-rich-root-value");
    rich["document"]["content"]["children"][0]["m2cRichContainerKey"] =
        json!("m2c-rich-container-value");
    rich["document"]["content"]["children"][0]["children"][0]["m2cRichTextKey"] =
        json!("m2c-rich-text-value");
    let mut raw = document_value(
        [
            (TEXT, scalar),
            (SINGLE, choice),
            (ORPHAN_A, rich),
            (
                DURATION,
                json!({"kind":"multiChoice","optionIds":[ORPHAN_OPTION]}),
            ),
        ],
        [
            (
                ORPHAN_A,
                json!({
                    "label":"m2c-orphan-field-label",
                    "kind":"richText",
                    "options":{},
                    "m2cOrphanFieldKey":"m2c-orphan-field-value"
                }),
            ),
            (
                DURATION,
                json!({
                    "label":"m2c-orphan-choice-label",
                    "kind":"multiChoice",
                    "options":{
                        (ORPHAN_OPTION):{
                            "label":"m2c-orphan-option-label",
                            "m2cOrphanOptionKey":"m2c-orphan-option-value"
                        }
                    }
                }),
            ),
        ],
        revision,
        template_id,
    );
    raw["name"] = json!("credential=m2c-document-name");
    raw["m2cDocumentRootKey"] = json!({
        "value":"m2c-document-root-value",
        "url":"https://m2c-document.invalid/private",
        "windows":r"C:\M2C_DOCUMENT_PATH\secret.txt",
        "unix":"/tmp/M2C_DOCUMENT_UNIX/secret.txt",
        "decimal":exact_number("999.0001")
    });
    raw.as_object_mut()
        .expect("Document fixture must be an object")
        .remove("futureDocument");
    raw
}

fn assert_sensitive_document_fixture(raw: &Value) {
    assert_eq!(raw["name"], json!("credential=m2c-document-name"));
    assert_eq!(
        raw["m2cDocumentRootKey"]["value"],
        json!("m2c-document-root-value")
    );
    assert_eq!(
        raw["m2cDocumentRootKey"]["url"],
        json!("https://m2c-document.invalid/private")
    );
    assert_eq!(
        raw["m2cDocumentRootKey"]["windows"],
        json!(r"C:\M2C_DOCUMENT_PATH\secret.txt")
    );
    assert_eq!(
        raw["m2cDocumentRootKey"]["unix"],
        json!("/tmp/M2C_DOCUMENT_UNIX/secret.txt")
    );
    assert_eq!(
        raw["fieldValues"][TEXT]["m2cFieldValueOuterKey"],
        json!("m2c-field-value-outer-value")
    );
    assert_eq!(
        raw["fieldValues"][TEXT]["value"],
        json!("m2c-scalar-payload")
    );
    assert_eq!(
        raw["fieldValues"][SINGLE]["optionId"],
        json!(OPTION_SINGLE_ACTIVE)
    );
    assert_eq!(
        raw["fieldValues"][ORPHAN_A]["m2cRichOuterKey"],
        json!("m2c-rich-outer-value")
    );
    assert_eq!(
        raw["fieldValues"][ORPHAN_A]["document"]["m2cRichEnvelopeKey"],
        json!("m2c-rich-envelope-value")
    );
    assert_eq!(
        raw["fieldValues"][ORPHAN_A]["document"]["content"]["m2cRichRootKey"],
        json!("m2c-rich-root-value")
    );
    assert_eq!(
        raw["fieldValues"][ORPHAN_A]["document"]["content"]["children"][0]["m2cRichContainerKey"],
        json!("m2c-rich-container-value")
    );
    assert_eq!(
        raw["fieldValues"][ORPHAN_A]["document"]["content"]["children"][0]["children"][0]
            ["m2cRichTextKey"],
        json!("m2c-rich-text-value")
    );
    assert_eq!(
        raw["orphanedFieldDefinitions"][ORPHAN_A]["label"],
        json!("m2c-orphan-field-label")
    );
    assert_eq!(
        raw["orphanedFieldDefinitions"][ORPHAN_A]["m2cOrphanFieldKey"],
        json!("m2c-orphan-field-value")
    );
    assert_eq!(
        raw["orphanedFieldDefinitions"][DURATION]["options"][ORPHAN_OPTION]["label"],
        json!("m2c-orphan-option-label")
    );
    assert!(raw["orphanedFieldDefinitions"][DURATION]["options"]
        .as_object()
        .is_some_and(|options| options.contains_key(ORPHAN_OPTION)));
    assert_eq!(
        raw["orphanedFieldDefinitions"][DURATION]["options"][ORPHAN_OPTION]["m2cOrphanOptionKey"],
        json!("m2c-orphan-option-value")
    );
    assert_eq!(
        raw["fieldValues"][DURATION]["optionIds"],
        json!([ORPHAN_OPTION])
    );
    let decimal = raw["m2cDocumentRootKey"]["decimal"]
        .as_number()
        .expect("decimal canary must remain a JSON Number");
    assert_eq!(decimal.to_string(), "999.0001");
    let encoded = deterministic(raw);
    assert!(std::str::from_utf8(&encoded)
        .expect("fixture JSON is UTF-8")
        .contains("999.0001"));
}

fn template_sensitive_fragments() -> Vec<&'static str> {
    vec![
        "credential=m2c-template-name",
        "m2cTemplateRootKey",
        "m2c-template-root-value",
        "m2cFieldDefinitionKey",
        "m2c-field-definition-value",
        "m2c-sensitive-text-label",
        "m2c-sensitive-choice-label",
        "m2cKnownOptionKey",
        "m2c-known-option-value",
        "m2c-sensitive-known-option-label",
        "m2c-field-presentation-token",
        "m2cFieldPresentationKey",
        "m2c-field-presentation-value",
        "m2cConfigurationKey",
        "m2c-configuration-value",
        "m2c-template-presentation-token",
        "m2cTemplatePresentationKey",
        "m2c-template-presentation-value",
        "https://m2c-template.invalid/private",
        r"C:\M2C_TEMPLATE_PATH\secret.txt",
        r"C:\M2C_TEMPLATE_PATH",
        "M2C_TEMPLATE_PATH",
        "/tmp/M2C_TEMPLATE_UNIX/secret.txt",
        "M2C_TEMPLATE_UNIX",
        "123456789012345678901234567890123456789",
        "999.0001",
        "9.87654321098765432109876543210987654321e+210",
        OPTION_SINGLE_ACTIVE,
        "b0000000-0000-4000-8000-",
    ]
}

fn document_sensitive_fragments() -> Vec<&'static str> {
    vec![
        "credential=m2c-document-name",
        "m2cDocumentRootKey",
        "m2c-document-root-value",
        "m2cFieldValueOuterKey",
        "m2c-field-value-outer-value",
        "m2c-scalar-payload",
        "m2cRichOuterKey",
        "m2c-rich-outer-value",
        "m2cRichEnvelopeKey",
        "m2c-rich-envelope-value",
        "m2cRichRootKey",
        "m2c-rich-root-value",
        "m2cRichContainerKey",
        "m2c-rich-container-value",
        "m2cRichTextKey",
        "m2c-rich-text-value",
        "m2c-rich-payload",
        "m2c-orphan-field-label",
        "m2cOrphanFieldKey",
        "m2c-orphan-field-value",
        "m2c-orphan-option-label",
        "m2cOrphanOptionKey",
        "m2c-orphan-option-value",
        ORPHAN_OPTION,
        "c0000000-0000-4000-8000-",
        OPTION_SINGLE_ACTIVE,
        "b0000000-0000-4000-8000-",
        "https://m2c-document.invalid/private",
        r"C:\M2C_DOCUMENT_PATH\secret.txt",
        r"C:\M2C_DOCUMENT_PATH",
        "M2C_DOCUMENT_PATH",
        "/tmp/M2C_DOCUMENT_UNIX/secret.txt",
        "M2C_DOCUMENT_UNIX",
        "999.0001",
    ]
}

#[test]
fn reconciliation_leak_predicate_detects_every_sensitive_shape() {
    let windows_path = r"C:\M2_5C_PATH_CANARY\secret.txt";
    let escaped_windows_path = windows_path.escape_debug().to_string();
    for canary in [
        "m2_5cMetadataKeyCanary",
        "m2_5c-metadata-value-canary",
        "m2_5c-scalar-payload-canary",
        "123456789012345678901234567890123456789",
        "999.0001",
        "9.87654321098765432109876543210987654321e+210",
        OPTION_SINGLE_ACTIVE,
        "b0000000-0000-4000-8000-",
        "orphan option",
        ORPHAN_OPTION,
        "c0000000-0000-4000-8000-",
        windows_path,
        escaped_windows_path.as_str(),
        "M2_5C_PATH_CANARY",
        "/tmp/M2_5C_UNIX_CANARY/secret.txt",
        "M2_5C_UNIX_CANARY",
        "m2_5c-orphan-label-canary",
    ] {
        assert_eq!(first_leak(canary, &[canary]), Some(canary));
    }
    assert_eq!(
        first_leak(
            "ReconciledDocumentView { warning_count: 1, materialization_required: true }",
            &["credential=", "m2_5c-scalar-payload-canary"]
        ),
        None
    );
    assert_eq!(
        first_leak(
            "DocumentReconciliationWarning { category: ArchivedOptionSelected, count: 1024, truncated: false }",
            &["orphan option", ORPHAN_OPTION, "credential="]
        ),
        None
    );
}

#[test]
fn metadata_rich_sources_are_redacted_from_view_and_every_reachable_fatal_error(
) -> Result<(), Box<dyn Error>> {
    let raw_template = sensitive_template_value("active");
    let raw_document = sensitive_document_value(TEMPLATE_ID, 1);
    assert_sensitive_template_fixture(&raw_template);
    assert_sensitive_document_fixture(&raw_document);
    let template = decode_template_value(&raw_template);
    let document = decode_document_value(&raw_document);
    assert!(template.validate_storage().is_ok());
    assert!(document.validate_storage().is_ok());
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let template_escaped = r"C:\M2C_TEMPLATE_PATH\secret.txt"
        .escape_debug()
        .to_string();
    let document_escaped = r"C:\M2C_DOCUMENT_PATH\secret.txt"
        .escape_debug()
        .to_string();
    let mut forbidden = template_sensitive_fragments();
    forbidden.extend(document_sensitive_fragments());
    forbidden.push(template_escaped.as_str());
    forbidden.push(document_escaped.as_str());

    let view = reconcile_document(&template, &document)?;
    assert!(view.warnings().is_empty());
    assert!(view.blocking_issues().is_empty());
    assert!(view.can_materialize());
    let rich_orphan = view
        .orphan_fields()
        .iter()
        .find(|entry| entry.field_id() == id(ORPHAN_A))
        .expect("metadata-rich orphan must reach the production view");
    assert_eq!(
        rich_orphan.disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    for rendered in [
        format!("{view:?}"),
        format!("{:?}", view.known_fields()),
        format!("{:?}", view.orphan_fields()),
        format!("{rich_orphan:?}"),
        format!("{:?}", view.known_fields()[0].provenance()),
        format!("{:?}", rich_orphan.disposition()),
        format!("{:?}", view.warnings()),
        format!("{:?}", view.blocking_issues()),
    ] {
        assert_redacted_from(&rendered, &forbidden);
    }
    assert_sources_unchanged(
        &template,
        &template_before,
        &template_snapshot,
        &document,
        &document_before,
        &document_snapshot,
    );

    let mut invalid_template = template.clone();
    invalid_template.corrupt_field_order_for_test();
    let invalid_template_before = invalid_template.clone();
    let invalid_template_bytes = unchecked_template_bytes(&invalid_template);
    let error = reconcile_document(&invalid_template, &document)
        .expect_err("independent fieldOrder corruption must fail Template admission");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_fatal_redacted_from(
        error,
        DocumentReconciliationErrorCategory::InvalidTemplate,
        &forbidden,
    );
    assert_eq!(invalid_template, invalid_template_before);
    assert_eq!(
        unchecked_template_bytes(&invalid_template),
        invalid_template_bytes
    );
    assert_eq!(document, document_before);
    assert_eq!(document_bytes(&document), document_snapshot);

    let mut invalid_document = document.clone();
    invalid_document.corrupt_scalar_value_for_test(id(TEXT), "invalid\nscalar");
    let invalid_document_before = invalid_document.clone();
    let invalid_document_bytes = unchecked_document_bytes(&invalid_document);
    let error = reconcile_document(&template, &invalid_document)
        .expect_err("independent scalar corruption must fail Document admission");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        error.scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        error.scalar_location(),
        Some(crate::data::artifact::ArtifactScalarValueLocation::DocumentField)
    );
    assert_fatal_redacted_from(
        error,
        DocumentReconciliationErrorCategory::InvalidDocument,
        &forbidden,
    );
    assert_eq!(invalid_document, invalid_document_before);
    assert_eq!(
        unchecked_document_bytes(&invalid_document),
        invalid_document_bytes
    );
    assert_eq!(template, template_before);
    assert_eq!(template_bytes(&template), template_snapshot);

    let mismatch_raw = sensitive_document_value(OTHER_TEMPLATE_ID, 1);
    assert_sensitive_template_fixture(&raw_template);
    assert_sensitive_document_fixture(&mismatch_raw);
    let mismatch = decode_document_value(&mismatch_raw);
    let mismatch_before = mismatch.clone();
    let mismatch_bytes = document_bytes(&mismatch);
    assert_fatal_redacted_from(
        reconcile_document(&template, &mismatch).expect_err("TemplateId alone must mismatch"),
        DocumentReconciliationErrorCategory::TemplateIdMismatch,
        &forbidden,
    );
    assert_eq!(template, template_before);
    assert_eq!(template_bytes(&template), template_snapshot);
    assert_eq!(mismatch, mismatch_before);
    assert_eq!(document_bytes(&mismatch), mismatch_bytes);

    let deleted_raw = sensitive_template_value("deleted");
    let deleted_document_raw = sensitive_document_value(TEMPLATE_ID, 1);
    assert_sensitive_template_fixture(&deleted_raw);
    assert_sensitive_document_fixture(&deleted_document_raw);
    let deleted = decode_template_value(&deleted_raw);
    let deleted_document = decode_document_value(&deleted_document_raw);
    assert!(deleted.validate_storage().is_ok());
    assert!(deleted_document.validate_storage().is_ok());
    let deleted_before = deleted.clone();
    let deleted_document_before = deleted_document.clone();
    let deleted_bytes = template_bytes(&deleted);
    let deleted_document_bytes = document_bytes(&deleted_document);
    assert_fatal_redacted_from(
        reconcile_document(&deleted, &deleted_document)
            .expect_err("matching deleted Template must be rejected"),
        DocumentReconciliationErrorCategory::TemplateIsTombstoned,
        &forbidden,
    );
    let read_only = reconcile_document_for_read(&deleted, &deleted_document)
        .expect("a deleted Template must remain readable without becoming mutable");
    assert!(!read_only.known_fields().is_empty());
    assert_eq!(deleted, deleted_before);
    assert_eq!(template_bytes(&deleted), deleted_bytes);
    assert_eq!(deleted_document, deleted_document_before);
    assert_eq!(document_bytes(&deleted_document), deleted_document_bytes);

    let future_raw = sensitive_document_value(TEMPLATE_ID, 2);
    assert_sensitive_template_fixture(&raw_template);
    assert_sensitive_document_fixture(&future_raw);
    let future = decode_document_value(&future_raw);
    let future_before = future.clone();
    let future_bytes = document_bytes(&future);
    assert_fatal_redacted_from(
        reconcile_document(&template, &future).expect_err("future revision alone must fail"),
        DocumentReconciliationErrorCategory::FutureDocumentRevision,
        &forbidden,
    );
    assert_eq!(template, template_before);
    assert_eq!(template_bytes(&template), template_snapshot);
    assert_eq!(future, future_before);
    assert_eq!(document_bytes(&future), future_bytes);
    Ok(())
}

fn text_field(lifecycle: &str, required: bool, introduced: u32) -> Value {
    field(
        "singleLineText",
        lifecycle,
        required,
        json!({"kind":"text","value":"current-secret"}),
        json!({"kind":"text","value":"initial-secret"}),
        introduced,
        basic_configuration("singleLineText"),
    )
}

#[test]
fn admission_and_binding_priority_are_fail_closed_and_atomic() -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [(TEXT, text_field("active", false, 1))],
        &[TEXT],
        3,
        "active",
    ));
    let document = decode_document_value(&document_value(
        [(TEXT, json!({"kind":"text","value":"valid"}))],
        [],
        3,
        TEMPLATE_ID,
    ));
    let current_view = reconcile_document(&template, &document)?;
    assert!(!current_view.materialization_required());
    assert!(current_view.can_materialize());
    let older_document = decode_document_value(&document_value(
        [(TEXT, json!({"kind":"text","value":"valid"}))],
        [],
        2,
        TEMPLATE_ID,
    ));
    let older_view = reconcile_document(&template, &older_document)?;
    assert!(older_view.materialization_required());
    assert!(older_view.can_materialize());

    let mut invalid_template = template.clone();
    invalid_template.corrupt_field_order_for_test();
    let mut invalid_document = document.clone();
    invalid_document.corrupt_scalar_value_for_test(id(TEXT), "line one\ncredential=payload");
    let invalid_template_before = invalid_template.clone();
    let invalid_document_before = invalid_document.clone();
    let template_snapshot = unchecked_template_bytes(&invalid_template);
    let document_snapshot = unchecked_document_bytes(&invalid_document);
    let error = reconcile_document(&invalid_template, &invalid_document)
        .expect_err("invalid Template must precede invalid Document");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidTemplate
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert_fatal_is_sourceless(error, DocumentReconciliationErrorCategory::InvalidTemplate);
    assert_eq!(invalid_template, invalid_template_before);
    assert_eq!(invalid_document, invalid_document_before);
    assert_eq!(
        unchecked_template_bytes(&invalid_template),
        template_snapshot
    );
    assert_eq!(
        unchecked_document_bytes(&invalid_document),
        document_snapshot
    );

    let error = reconcile_document(&template, &invalid_document)
        .expect_err("invalid Document must precede binding checks");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidDocument
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(
        error.scalar_category(),
        Some(ScalarValueErrorCategory::MultilineSingleLineText)
    );
    assert_eq!(
        error.scalar_location(),
        Some(crate::data::artifact::ArtifactScalarValueLocation::DocumentField)
    );
    assert_fatal_is_sourceless(error, DocumentReconciliationErrorCategory::InvalidDocument);

    let mismatch = decode_document_value(&document_value([], [], 4, OTHER_TEMPLATE_ID));
    assert_fatal_is_sourceless(
        reconcile_document(&template, &mismatch).expect_err("identity mismatch must be fatal"),
        DocumentReconciliationErrorCategory::TemplateIdMismatch,
    );
    let deleted = decode_template_value(&template_value([], &[], 3, "deleted"));
    assert_fatal_is_sourceless(
        reconcile_document(&deleted, &mismatch)
            .expect_err("identity mismatch must precede lifecycle and revision"),
        DocumentReconciliationErrorCategory::TemplateIdMismatch,
    );
    assert_fatal_is_sourceless(
        reconcile_document(&deleted, &invalid_document)
            .expect_err("invalid Document must precede the lifecycle gate"),
        DocumentReconciliationErrorCategory::InvalidDocument,
    );
    let empty = decode_document_value(&document_value([], [], 3, TEMPLATE_ID));
    assert_fatal_is_sourceless(
        reconcile_document(&deleted, &empty).expect_err("tombstoned Template must be fatal"),
        DocumentReconciliationErrorCategory::TemplateIsTombstoned,
    );
    let future = decode_document_value(&document_value([], [], 4, TEMPLATE_ID));
    assert_fatal_is_sourceless(
        reconcile_document(&deleted, &future)
            .expect_err("tombstoned lifecycle must precede the future revision gate"),
        DocumentReconciliationErrorCategory::TemplateIsTombstoned,
    );
    let current = decode_template_value(&template_value([], &[], 3, "active"));
    assert_fatal_is_sourceless(
        reconcile_document(&current, &future).expect_err("future Document revision must be fatal"),
        DocumentReconciliationErrorCategory::FutureDocumentRevision,
    );
    Ok(())
}

#[test]
fn invalid_document_values_in_first_middle_and_last_positions_fail_admission() {
    let fields = [
        (TEXT, text_field("active", false, 1)),
        (RICH, text_field("active", false, 1)),
        (NUMBER, text_field("active", false, 1)),
    ];
    let template =
        decode_template_value(&template_value(fields, &[TEXT, RICH, NUMBER], 1, "active"));
    let raw = document_value(
        [
            (TEXT, json!({"kind":"text","value":"first"})),
            (RICH, json!({"kind":"text","value":"middle"})),
            (NUMBER, json!({"kind":"text","value":"last"})),
        ],
        [],
        1,
        TEMPLATE_ID,
    );
    for field_id in [TEXT, RICH, NUMBER] {
        let mut document = decode_document_value(&raw);
        document.corrupt_scalar_value_for_test(id(field_id), "invalid\ncredential=position");
        let before = document.clone();
        let bytes = unchecked_document_bytes(&document);
        let error = reconcile_document(&template, &document)
            .expect_err("whole Document admission must reject any invalid position");
        assert_eq!(
            error.category(),
            DocumentReconciliationErrorCategory::InvalidDocument
        );
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidScalarValue)
        );
        assert_eq!(
            error.scalar_category(),
            Some(ScalarValueErrorCategory::MultilineSingleLineText)
        );
        assert_eq!(
            error.scalar_location(),
            Some(crate::data::artifact::ArtifactScalarValueLocation::DocumentField)
        );
        assert_eq!(document, before);
        assert_eq!(unchecked_document_bytes(&document), bytes);
        assert_redacted(&format!("{error} {error:?}"));
    }
}

#[test]
fn all_eight_historical_kinds_borrow_initial_defaults_and_preserve_order(
) -> Result<(), Box<dyn Error>> {
    let fields = [
        (
            TEXT,
            field(
                "singleLineText",
                "active",
                true,
                json!({"kind":"text","value":"current"}),
                json!({"kind":"text","value":"initial"}),
                2,
                basic_configuration("singleLineText"),
            ),
        ),
        (
            RICH,
            field(
                "richText",
                "active",
                true,
                rich_text("current rich"),
                rich_text("initial rich"),
                2,
                basic_configuration("richText"),
            ),
        ),
        (
            NUMBER,
            field(
                "number",
                "active",
                true,
                json!({"kind":"number","value":"9"}),
                json!({"kind":"number","value":"123456789012345678901234567890"}),
                2,
                basic_configuration("number"),
            ),
        ),
        (
            DATE,
            field(
                "date",
                "active",
                true,
                json!({"kind":"date","value":"2026-09-05"}),
                json!({"kind":"date","value":"2020-01-01"}),
                2,
                basic_configuration("date"),
            ),
        ),
        (
            TIME,
            field(
                "time",
                "active",
                true,
                json!({"kind":"time","value":"12:34:56.789"}),
                json!({"kind":"time","value":"00:00:00.000"}),
                2,
                basic_configuration("time"),
            ),
        ),
        (
            DURATION,
            field(
                "duration",
                "active",
                true,
                json!({"kind":"duration","milliseconds":"9"}),
                json!({"kind":"duration","milliseconds":"8"}),
                2,
                basic_configuration("duration"),
            ),
        ),
        (
            SINGLE,
            field(
                "singleChoice",
                "active",
                true,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_OTHER}),
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
                2,
                single_choice_configuration(),
            ),
        ),
        (
            MULTI,
            field(
                "multiChoice",
                "active",
                true,
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_OTHER]}),
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
                2,
                choice_configuration("multiChoice"),
            ),
        ),
    ];
    let order = [MULTI, TEXT, RICH, NUMBER, DATE, TIME, DURATION, SINGLE];
    let template = decode_template_value(&template_value(fields, &order, 3, "active"));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let view = reconcile_document(&template, &document)?;
    assert_eq!(
        view.known_fields()
            .iter()
            .map(KnownFieldEntry::field_id)
            .collect::<Vec<_>>(),
        order.map(id)
    );
    assert!(view.materialization_required());
    assert!(view.can_materialize());
    assert!(view.blocking_issues().is_empty());
    for entry in view.known_fields() {
        assert_eq!(
            entry.provenance(),
            Some(ValueProvenance::HistoricalInitialDefault)
        );
        let initial = template.fields()[&entry.field_id()].initial_default_value();
        assert!(std::ptr::eq(
            entry.value().expect("historical value"),
            initial
        ));
        assert_ne!(
            entry.value(),
            Some(template.fields()[&entry.field_id()].default_value())
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
    Ok(())
}

#[test]
fn history_boundary_required_and_archived_policies_are_distinct() -> Result<(), Box<dyn Error>> {
    let fields = [
        (
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
        ),
        (
            RICH,
            field(
                "singleLineText",
                "archived",
                true,
                json!({"kind":"text","value":"current archived"}),
                json!({"kind":"unset"}),
                2,
                basic_configuration("singleLineText"),
            ),
        ),
        (NUMBER, text_field("active", false, 1)),
        (DATE, text_field("active", false, 1)),
        (
            TIME,
            field(
                "multiChoice",
                "archived",
                true,
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
                json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED]}),
                2,
                choice_configuration("multiChoice"),
            ),
        ),
    ];
    let template =
        decode_template_value(&template_value(fields, &[TEXT, NUMBER, DATE], 3, "active"));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let view = reconcile_document(&template, &document)?;
    assert_eq!(view.known_fields()[0].field_id(), id(TEXT));
    assert_eq!(view.known_fields()[3].field_id(), id(RICH));
    assert_eq!(
        view.blocking_issues()
            .iter()
            .map(|issue| (issue.field_id(), issue.category()))
            .collect::<Vec<_>>(),
        vec![
            (
                id(NUMBER),
                DocumentReconciliationIssueCategory::MissingKnownFieldValue
            ),
            (
                id(DATE),
                DocumentReconciliationIssueCategory::MissingKnownFieldValue
            ),
        ]
    );
    assert_eq!(
        view.known_fields()[0].provenance(),
        Some(ValueProvenance::HistoricalInitialDefault)
    );
    assert_eq!(
        view.known_fields()[3].provenance(),
        Some(ValueProvenance::HistoricalInitialDefault)
    );
    assert!(view.known_fields()[3]
        .value()
        .expect("archived historical")
        .is_unset());
    assert_eq!(view.known_fields()[4].field_id(), id(TIME));
    assert_eq!(
        view.known_fields()[4].provenance(),
        Some(ValueProvenance::HistoricalInitialDefault)
    );
    assert_eq!(view.warnings().len(), 1);
    assert_eq!(view.warnings()[0].field_id(), id(TIME));
    assert_eq!(view.warnings()[0].count(), 1);
    assert!(!view.can_materialize());

    let equal_revision = decode_document_value(&document_value([], [], 2, TEMPLATE_ID));
    let equal_view = reconcile_document(&template, &equal_revision)?;
    assert!(equal_view.blocking_issues().iter().any(|issue| {
        issue.field_id() == id(TEXT)
            && issue.category() == DocumentReconciliationIssueCategory::MissingKnownFieldValue
    }));

    let later_revision = decode_document_value(&document_value([], [], 3, TEMPLATE_ID));
    let later_view = reconcile_document(&template, &later_revision)?;
    assert!(later_view.blocking_issues().iter().any(|issue| {
        issue.field_id() == id(TEXT)
            && issue.category() == DocumentReconciliationIssueCategory::MissingKnownFieldValue
    }));
    Ok(())
}

#[test]
fn existing_values_and_explicit_unset_are_never_replaced_by_defaults() -> Result<(), Box<dyn Error>>
{
    let template = decode_template_value(&template_value(
        [
            (TEXT, text_field("active", true, 1)),
            (RICH, text_field("active", false, 1)),
            (DATE, text_field("active", false, 1)),
            (NUMBER, text_field("archived", true, 1)),
            (DURATION, text_field("archived", true, 1)),
        ],
        &[RICH, DATE, TEXT],
        3,
        "active",
    ));
    let document = decode_document_value(&document_value(
        [
            (
                TEXT,
                json!({"kind":"unset","futureValue":{"credential":"unset-extra"}}),
            ),
            (RICH, json!({"kind":"text","value":"persisted old value"})),
            (DATE, json!({"kind":"unset"})),
            (
                NUMBER,
                json!({"kind":"text","value":"archived preserved value"}),
            ),
            (DURATION, json!({"kind":"unset"})),
        ],
        [],
        3,
        TEMPLATE_ID,
    ));
    let view = reconcile_document(&template, &document)?;
    assert_eq!(
        view.known_fields()
            .iter()
            .map(KnownFieldEntry::field_id)
            .collect::<Vec<_>>(),
        vec![id(RICH), id(DATE), id(TEXT), id(NUMBER), id(DURATION)]
    );
    assert_eq!(
        view.known_fields()[0].provenance(),
        Some(ValueProvenance::ExistingValue)
    );
    assert_eq!(
        view.known_fields()[1].provenance(),
        Some(ValueProvenance::ExplicitUnset)
    );
    assert_eq!(
        view.known_fields()[2].provenance(),
        Some(ValueProvenance::ExplicitUnset)
    );
    assert_eq!(
        view.known_fields()[3].provenance(),
        Some(ValueProvenance::ExistingValue)
    );
    assert_eq!(
        view.known_fields()[4].provenance(),
        Some(ValueProvenance::ExplicitUnset)
    );
    for entry in view.known_fields() {
        assert!(std::ptr::eq(
            entry.value().expect("existing value"),
            &document.field_values()[&entry.field_id()]
        ));
    }
    assert!(view.blocking_issues().is_empty());
    assert!(view.materialization_required());
    Ok(())
}

#[test]
fn archived_choice_diagnostics_are_observable_and_unknown_is_blocking() -> Result<(), Box<dyn Error>>
{
    let mut configuration = choice_configuration("multiChoice");
    configuration["optionOrder"] = json!([OPTION_OTHER, OPTION_ACTIVE]);
    let template = decode_template_value(&template_value(
        [(
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]}),
                2,
                configuration,
            ),
        )],
        &[MULTI],
        3,
        "active",
    ));
    let active_only = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_OTHER]}),
        )],
        [],
        3,
        TEMPLATE_ID,
    ));
    let active_only_view = reconcile_document(&template, &active_only)?;
    assert!(active_only_view.warnings().is_empty());
    assert!(active_only_view.blocking_issues().is_empty());
    assert!(!active_only_view.materialization_required());
    assert_eq!(
        active_only_view.known_fields()[0]
            .value()
            .and_then(FieldValue::multi_choice),
        Some(
            [
                crate::data::artifact::OptionId::from_str(OPTION_ACTIVE)?,
                crate::data::artifact::OptionId::from_str(OPTION_OTHER)?,
            ]
            .as_slice()
        )
    );
    let existing = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]}),
        )],
        [],
        3,
        TEMPLATE_ID,
    ));
    let view = reconcile_document(&template, &existing)?;
    assert_eq!(view.warnings().len(), 1);
    assert_eq!(
        view.warnings()[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(view.warnings()[0].count(), 1);
    assert!(!view.warnings()[0].truncated());
    assert!(view.blocking_issues().is_empty());
    // U1: 경고 자체가 아니라 아직 없는 archived 선택 snapshot이 저장 계산을 요구한다.
    assert!(view.materialization_required());
    assert_eq!(
        view.orphan_fields()[0].disposition(),
        OrphanFieldDisposition::SnapshotRequired
    );
    assert!(view.can_materialize());
    assert_redacted(&format!("{:?}", view.warnings()[0]));
    assert_eq!(
        view.known_fields()[0]
            .value()
            .and_then(FieldValue::multi_choice),
        Some(
            [
                crate::data::artifact::OptionId::from_str(OPTION_ACTIVE)?,
                crate::data::artifact::OptionId::from_str(OPTION_ARCHIVED)?
            ]
            .as_slice()
        )
    );

    let historical = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    let historical_view = reconcile_document(&template, &historical)?;
    assert_eq!(historical_view.warnings()[0].count(), 1);

    let unknown = decode_document_value(&document_value(
        [(
            MULTI,
            json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED,OPTION_UNKNOWN]}),
        )],
        [],
        3,
        TEMPLATE_ID,
    ));
    let unknown_view = reconcile_document(&template, &unknown)?;
    assert!(unknown_view.warnings().is_empty());
    let issue = unknown_view.blocking_issues()[0];
    assert_eq!(
        issue.category(),
        DocumentReconciliationIssueCategory::UnknownSelectedOption
    );
    assert_eq!(
        issue.choice_category(),
        Some(ChoiceValidationErrorCategory::UnknownSelectedOption)
    );
    assert!(!unknown_view.can_materialize());
    Ok(())
}

fn historical_archived_option_fixture(
    archived_count: usize,
) -> (TemplateArtifact, DocumentArtifact) {
    let mut options = Map::new();
    let mut selected = Vec::new();
    for index in 0..archived_count {
        let option_id = format!("d0000000-0000-4000-8000-{index:012x}");
        options.insert(
            option_id.clone(),
            json!({"label":"credential=archived-option","lifecycle":"archived"}),
        );
        selected.push(option_id);
    }
    let template = decode_template_value(&template_value(
        [(
            MULTI,
            field(
                "multiChoice",
                "active",
                false,
                json!({"kind":"unset"}),
                json!({"kind":"multiChoice","optionIds":selected}),
                2,
                json!({"kind":"multiChoice","optionOrder":[],"options":options}),
            ),
        )],
        &[MULTI],
        2,
        "active",
    ));
    let document = decode_document_value(&document_value([], [], 1, TEMPLATE_ID));
    assert!(template.validate_storage().is_ok());
    assert!(document.validate_storage().is_ok());
    (template, document)
}

#[test]
fn exactly_1024_archived_options_are_not_truncated() -> Result<(), Box<dyn Error>> {
    let (template, document) = historical_archived_option_fixture(1_024);
    let view = reconcile_document(&template, &document)?;
    assert_eq!(view.warnings().len(), 1);
    assert_eq!(view.warnings()[0].count(), 1_024);
    assert!(!view.warnings()[0].truncated());
    assert!(view.blocking_issues().is_empty());
    assert_redacted(&format!("{:?}", view.warnings()[0]));
    Ok(())
}

#[test]
fn exactly_1025_archived_options_are_capped_and_truncated() -> Result<(), Box<dyn Error>> {
    let (template, document) = historical_archived_option_fixture(1_025);
    let view = reconcile_document(&template, &document)?;
    assert_eq!(view.warnings().len(), 1);
    assert_eq!(view.warnings()[0].count(), 1_024);
    assert!(view.warnings()[0].truncated());
    assert!(view.blocking_issues().is_empty());
    assert_redacted(&format!("{:?}", view.warnings()[0]));
    Ok(())
}

fn orphan_snapshot(label: &str, kind: &str, options: Value) -> Value {
    json!({
        "label":label,
        "kind":kind,
        "options":options
    })
}

fn metadata_orphan_snapshot(label: &str, kind: &str, options: Value) -> Value {
    json!({
        "label":label,
        "kind":kind,
        "options":options,
        "futureOrphan":{"credential":"orphan-extra","number":123456789012345678901234567890u128}
    })
}

#[test]
fn compatible_reattachment_is_revision_independent_and_borrows_both_sources(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [(TEXT, text_field("active", false, 1))],
        &[TEXT],
        3,
        "active",
    ));
    let template_before = template.clone();
    let template_snapshot = template_bytes(&template);

    for revision in [1, 2, 3] {
        let document = decode_document_value(&document_value(
            [(TEXT, json!({"kind":"text","value":"reattachment-value"}))],
            [(
                TEXT,
                orphan_snapshot("historical text label", "singleLineText", json!({})),
            )],
            revision,
            TEMPLATE_ID,
        ));
        let document_before = document.clone();
        let document_snapshot = document_bytes(&document);
        let view = reconcile_document(&template, &document)?;

        assert!(view.blocking_issues().is_empty());
        assert!(view.warnings().is_empty());
        assert!(view.materialization_required());
        assert!(view.can_materialize());
        assert_eq!(view.orphan_fields().len(), 1);
        let orphan = &view.orphan_fields()[0];
        assert_eq!(orphan.field_id(), id(TEXT));
        assert_eq!(
            orphan.disposition(),
            OrphanFieldDisposition::ReattachableOrphan
        );
        assert!(std::ptr::eq(
            view.known_fields()[0].value().expect("known value"),
            &document.field_values()[&id(TEXT)]
        ));
        assert!(std::ptr::eq(
            orphan.value(),
            &document.field_values()[&id(TEXT)]
        ));
        assert!(std::ptr::eq(
            orphan.snapshot().expect("historical snapshot"),
            &document.orphaned_field_definitions()[&id(TEXT)]
        ));
        assert_sources_unchanged(
            &template,
            &template_before,
            &template_snapshot,
            &document,
            &document_before,
            &document_snapshot,
        );
    }
    Ok(())
}

#[test]
fn reattachment_with_unknown_snapshot_metadata_is_lossless_blocking() -> Result<(), Box<dyn Error>>
{
    let template = decode_template_value(&template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
                json!({"kind":"unset"}),
                1,
                single_choice_configuration(),
            ),
        )],
        &[SINGLE],
        3,
        "active",
    ));
    let cases = [
        json!({
            "label":"historical field metadata",
            "kind":"singleChoice",
            "options":{(OPTION_SINGLE_ACTIVE):{"label":"known option"}},
            "futureSnapshot":{
                "nested":[{"exact":123456789012345678901234567890u128}]
            }
        }),
        json!({
            "label":"historical option metadata",
            "kind":"singleChoice",
            "options":{
                (OPTION_SINGLE_ACTIVE):{
                    "label":"known option",
                    "futureOption":{"nested":[123456789012345678901234567890u128]}
                }
            }
        }),
    ];

    for snapshot in cases {
        let document = decode_document_value(&document_value(
            [(
                SINGLE,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
            )],
            [(SINGLE, snapshot)],
            1,
            TEMPLATE_ID,
        ));
        let template_before = template.clone();
        let document_before = document.clone();
        let template_snapshot = template_bytes(&template);
        let document_snapshot = document_bytes(&document);

        let view = reconcile_document(&template, &document)?;
        assert!(view.materialization_required());
        assert!(!view.can_materialize());
        assert_eq!(view.blocking_issues().len(), 1);
        let issue = view.blocking_issues()[0];
        assert_eq!(
            issue.category(),
            DocumentReconciliationIssueCategory::LossyOrphanReattachment
        );
        assert_eq!(issue.field_id(), id(SINGLE));
        assert_eq!(issue.validation_category(), None);
        assert_eq!(
            view.orphan_fields()[0].disposition(),
            OrphanFieldDisposition::ReattachmentBlocked
        );
        for rendered in [format!("{view:?}"), format!("{issue:?}")] {
            assert!(!rendered.contains("futureSnapshot"));
            assert!(!rendered.contains("futureOption"));
            assert!(!rendered.contains("123456789012345678901234567890"));
        }
        assert_sources_unchanged(
            &template,
            &template_before,
            &template_snapshot,
            &document,
            &document_before,
            &document_snapshot,
        );
    }
    Ok(())
}

#[test]
fn reattachment_compatibility_cases_reach_exact_production_categories() -> Result<(), Box<dyn Error>>
{
    let number_template = decode_template_value(&template_value(
        [(
            NUMBER,
            field(
                "number",
                "active",
                false,
                json!({"kind":"number","value":"1"}),
                json!({"kind":"unset"}),
                1,
                basic_configuration("number"),
            ),
        )],
        &[NUMBER],
        1,
        "active",
    ));

    let snapshot_kind_mismatch = decode_document_value(&document_value(
        [(NUMBER, json!({"kind":"unset"}))],
        [(
            NUMBER,
            orphan_snapshot("historical text", "singleLineText", json!({})),
        )],
        1,
        TEMPLATE_ID,
    ));
    assert!(snapshot_kind_mismatch.validate_storage().is_ok());
    let view = reconcile_document(&number_template, &snapshot_kind_mismatch)?;
    assert_eq!(view.blocking_issues().len(), 1);
    assert_eq!(
        view.blocking_issues()[0].category(),
        DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict
    );
    assert_eq!(view.blocking_issues()[0].field_id(), id(NUMBER));
    assert_eq!(view.blocking_issues()[0].validation_category(), None);
    assert_eq!(
        view.orphan_fields()[0].disposition(),
        OrphanFieldDisposition::ReattachmentBlocked
    );

    let value_kind_mismatch = decode_document_value(&document_value(
        [(NUMBER, json!({"kind":"text","value":"wrong kind"}))],
        [],
        1,
        TEMPLATE_ID,
    ));
    assert!(value_kind_mismatch.validate_storage().is_ok());
    let view = reconcile_document(&number_template, &value_kind_mismatch)?;
    assert_eq!(view.blocking_issues().len(), 1);
    assert_eq!(
        view.blocking_issues()[0].category(),
        DocumentReconciliationIssueCategory::InvalidKnownFieldValue
    );
    assert_eq!(
        view.blocking_issues()[0].validation_category(),
        Some(FieldValidationErrorCategory::FieldValueKindMismatch)
    );
    assert_eq!(
        view.blocking_issues()[0].validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );

    let choice_template = decode_template_value(&template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
                json!({"kind":"unset"}),
                1,
                single_choice_configuration(),
            ),
        )],
        &[SINGLE],
        1,
        "active",
    ));
    let unknown_option = decode_document_value(&document_value(
        [(
            SINGLE,
            json!({"kind":"singleChoice","optionId":OPTION_UNKNOWN}),
        )],
        [(
            SINGLE,
            orphan_snapshot(
                "historical unknown choice",
                "singleChoice",
                json!({(OPTION_UNKNOWN):{"label":"historical unknown"}}),
            ),
        )],
        1,
        TEMPLATE_ID,
    ));
    assert!(unknown_option.validate_storage().is_ok());
    let view = reconcile_document(&choice_template, &unknown_option)?;
    assert!(view.warnings().is_empty());
    assert_eq!(view.blocking_issues().len(), 2);
    // label 차이가 아니라 현재 owner에 없는 Option이므로 snapshot 제거도 차단된다.
    assert_eq!(
        view.blocking_issues()[1].category(),
        DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict
    );
    assert_eq!(
        view.blocking_issues()[0].category(),
        DocumentReconciliationIssueCategory::UnknownSelectedOption
    );
    assert_eq!(
        view.blocking_issues()[0].validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );
    assert_eq!(
        view.orphan_fields()[0].disposition(),
        OrphanFieldDisposition::ReattachmentBlocked
    );

    let archived_option = decode_document_value(&document_value(
        [(
            SINGLE,
            json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ARCHIVED}),
        )],
        [(
            SINGLE,
            orphan_snapshot(
                "historical archived choice",
                "singleChoice",
                json!({(OPTION_SINGLE_ARCHIVED):{"label":"historical archived"}}),
            ),
        )],
        1,
        TEMPLATE_ID,
    ));
    assert!(archived_option.validate_storage().is_ok());
    let view = reconcile_document(&choice_template, &archived_option)?;
    assert!(view.blocking_issues().is_empty());
    assert_eq!(view.warnings().len(), 1);
    assert_eq!(
        view.warnings()[0].category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(view.warnings()[0].field_id(), id(SINGLE));
    assert_eq!(view.warnings()[0].count(), 1);
    assert!(!view.warnings()[0].truncated());
    assert_eq!(
        view.orphan_fields()[0].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );

    let mut snapshot_value_mismatch = archived_option;
    snapshot_value_mismatch.remove_orphan_option_for_test(
        id(SINGLE),
        crate::data::artifact::OptionId::from_str(OPTION_SINGLE_ARCHIVED)?,
    );
    let error = reconcile_document(&choice_template, &snapshot_value_mismatch)
        .expect_err("snapshot/value mismatch must fail whole Document admission");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidDocument
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanOptionMismatch)
    );
    assert!(error.source().is_none());
    Ok(())
}

#[test]
fn orphan_entries_are_globally_sorted_across_sources_and_input_orders() -> Result<(), Box<dyn Error>>
{
    const BEFORE: &str = "15000000-0000-4000-8000-000000000001";
    const MIDDLE: &str = "20000000-0000-4000-8000-000000000003";

    let fields = [
        (TEXT, text_field("active", false, 1)),
        (RICH, text_field("active", false, 1)),
        (TIME, text_field("active", false, 1)),
        (DATE, text_field("archived", false, 1)),
    ];
    let reverse_order = decode_template_value(&template_value(
        fields.clone(),
        &[TIME, RICH, TEXT],
        3,
        "active",
    ));
    let forward_order =
        decode_template_value(&template_value(fields, &[TEXT, RICH, TIME], 3, "active"));

    let values_a = [
        (TIME, json!({"kind":"text","value":"time"})),
        (BEFORE, json!({"kind":"text","value":"before"})),
        (DATE, json!({"kind":"text","value":"date"})),
        (MIDDLE, json!({"kind":"text","value":"middle"})),
        (RICH, json!({"kind":"text","value":"rich"})),
        (TEXT, json!({"kind":"text","value":"text"})),
    ];
    let snapshots_a = [
        (TIME, orphan_snapshot("time", "singleLineText", json!({}))),
        (
            BEFORE,
            orphan_snapshot("before", "singleLineText", json!({})),
        ),
        (DATE, orphan_snapshot("date", "singleLineText", json!({}))),
        (
            MIDDLE,
            orphan_snapshot("middle", "singleLineText", json!({})),
        ),
        (RICH, orphan_snapshot("rich", "singleLineText", json!({}))),
        (TEXT, orphan_snapshot("text", "singleLineText", json!({}))),
    ];
    let document_a = decode_document_value(&document_value(
        values_a.clone(),
        snapshots_a.clone(),
        1,
        TEMPLATE_ID,
    ));
    let document_b = decode_document_value(&document_value(
        values_a.into_iter().rev(),
        snapshots_a.into_iter().rev(),
        1,
        TEMPLATE_ID,
    ));
    let expected = vec![
        id(BEFORE),
        id(TEXT),
        id(RICH),
        id(MIDDLE),
        id(DATE),
        id(TIME),
    ];

    for (template, document) in [
        (&reverse_order, &document_a),
        (&reverse_order, &document_b),
        (&forward_order, &document_a),
        (&forward_order, &document_b),
    ] {
        let first = reconcile_document(template, document)?;
        let second = reconcile_document(template, document)?;
        assert_eq!(
            first
                .orphan_fields()
                .iter()
                .map(OrphanFieldEntry::field_id)
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            format!("{:?}", first.orphan_fields()),
            format!("{:?}", second.orphan_fields())
        );
    }
    assert_eq!(
        reconcile_document(&reverse_order, &document_a)?
            .known_fields()
            .iter()
            .map(KnownFieldEntry::field_id)
            .collect::<Vec<_>>(),
        vec![id(TIME), id(RICH), id(TEXT), id(DATE)]
    );
    assert_eq!(
        reconcile_document(&forward_order, &document_a)?
            .known_fields()
            .iter()
            .map(KnownFieldEntry::field_id)
            .collect::<Vec<_>>(),
        vec![id(TEXT), id(RICH), id(TIME), id(DATE)]
    );
    Ok(())
}

#[test]
fn orphan_preservation_reattachment_and_fallback_are_deterministic() -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [
            (TEXT, text_field("active", false, 2)),
            (
                RICH,
                field(
                    "number",
                    "active",
                    false,
                    json!({"kind":"number","value":"1"}),
                    json!({"kind":"unset"}),
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
                    json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
                    json!({"kind":"unset"}),
                    2,
                    single_choice_configuration(),
                ),
            ),
            (DATE, text_field("active", false, 1)),
            (
                TIME,
                field(
                    "multiChoice",
                    "active",
                    false,
                    json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
                    json!({"kind":"unset"}),
                    2,
                    choice_configuration("multiChoice"),
                ),
            ),
        ],
        &[TEXT, RICH, SINGLE, DATE, TIME],
        3,
        "active",
    ));
    let raw_document = document_value(
        [
            (TEXT, json!({"kind":"text","value":"reattachable"})),
            (RICH, json!({"kind":"text","value":"kind mismatch"})),
            (
                SINGLE,
                json!({"kind":"singleChoice","optionId":OPTION_UNKNOWN}),
            ),
            (
                DATE,
                json!({"kind":"text","value":"conflicting regular value"}),
            ),
            (
                TIME,
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]}),
            ),
            (
                DURATION,
                json!({"kind":"multiChoice","optionIds":[ORPHAN_OPTION]}),
            ),
            (MULTI, json!({"kind":"unset"})),
            (ORPHAN_A, rich_text("rich-secret")),
            (ORPHAN_B, json!({"kind":"number","value":"999.0001"})),
            (ORPHAN_C, json!({"kind":"text","value":"missing snapshot"})),
        ],
        [
            (
                TEXT,
                orphan_snapshot("old text label", "singleLineText", json!({})),
            ),
            (
                RICH,
                orphan_snapshot("old rich label", "singleLineText", json!({})),
            ),
            (
                SINGLE,
                orphan_snapshot(
                    "old choice label",
                    "singleChoice",
                    json!({(OPTION_UNKNOWN):{"label":"unknown historical"}}),
                ),
            ),
            (
                DATE,
                orphan_snapshot("conflicting snapshot", "singleLineText", json!({})),
            ),
            (
                TIME,
                orphan_snapshot(
                    "old multi label",
                    "multiChoice",
                    json!({
                        (OPTION_ACTIVE):{"label":"active historical"},
                        (OPTION_ARCHIVED):{"label":"archived historical"}
                    }),
                ),
            ),
            (
                DURATION,
                metadata_orphan_snapshot(
                    "orphan choice",
                    "multiChoice",
                    json!({(ORPHAN_OPTION):{
                        "label":"orphan option",
                        "futureOption":{"credential":"option-extra"}
                    }}),
                ),
            ),
            (
                MULTI,
                orphan_snapshot("orphan unset", "multiChoice", json!({})),
            ),
            (
                ORPHAN_A,
                orphan_snapshot("orphan rich label", "richText", json!({})),
            ),
            (
                ORPHAN_B,
                orphan_snapshot("orphan number label", "number", json!({})),
            ),
        ],
        1,
        TEMPLATE_ID,
    );
    assert!(raw_document["orphanedFieldDefinitions"]
        .as_object()
        .is_some_and(|snapshots| snapshots.contains_key(DURATION)));
    assert!(
        raw_document["orphanedFieldDefinitions"][DURATION]["options"]
            .as_object()
            .is_some_and(|options| options.contains_key(ORPHAN_OPTION))
    );
    assert_eq!(
        raw_document["orphanedFieldDefinitions"][DURATION]["options"][ORPHAN_OPTION]["label"],
        json!("orphan option")
    );
    assert_eq!(
        raw_document["orphanedFieldDefinitions"][DURATION]["label"],
        json!("orphan choice")
    );
    assert_eq!(
        raw_document["orphanedFieldDefinitions"][DURATION]["futureOrphan"],
        json!({"credential":"orphan-extra","number":123456789012345678901234567890u128})
    );
    assert_eq!(
        raw_document["orphanedFieldDefinitions"][DURATION]["options"][ORPHAN_OPTION]
            ["futureOption"],
        json!({"credential":"option-extra"})
    );
    assert_eq!(
        raw_document["fieldValues"][DURATION]["kind"],
        json!("multiChoice")
    );
    assert_eq!(
        raw_document["fieldValues"][DURATION]["optionIds"],
        json!([ORPHAN_OPTION])
    );
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["optionId"],
        json!(OPTION_UNKNOWN)
    );
    assert_eq!(
        raw_document["fieldValues"][ORPHAN_B]["value"],
        json!("999.0001")
    );
    assert_eq!(
        raw_document["fieldValues"][ORPHAN_A]["document"]["content"]["children"][0]["children"][0]
            ["text"],
        json!("rich-secret")
    );
    let document = decode_document_value(&raw_document);
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let first = reconcile_document(&template, &document)?;
    let second = reconcile_document(&template, &document)?;
    assert_eq!(format!("{first:?}"), format!("{second:?}"));
    assert_eq!(
        first
            .orphan_fields()
            .iter()
            .map(OrphanFieldEntry::field_id)
            .collect::<Vec<_>>(),
        vec![
            id(TEXT),
            id(RICH),
            id(DATE),
            id(TIME),
            id(DURATION),
            id(SINGLE),
            id(MULTI),
            id(ORPHAN_A),
            id(ORPHAN_B),
            id(ORPHAN_C),
        ]
    );
    assert_eq!(
        first.orphan_fields()[0].disposition(),
        OrphanFieldDisposition::ReattachableOrphan
    );
    assert_eq!(
        first.orphan_fields()[1].disposition(),
        OrphanFieldDisposition::ReattachmentBlocked
    );
    assert_eq!(
        first.orphan_fields()[2].disposition(),
        OrphanFieldDisposition::ReattachableOrphan
    );
    assert_eq!(
        first.orphan_fields()[3].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    assert_eq!(
        first.orphan_fields()[4].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    let orphan_choice = first
        .orphan_fields()
        .iter()
        .find(|entry| entry.field_id() == id(DURATION))
        .expect("selected orphan Option must reach the orphan view");
    assert_eq!(
        orphan_choice.disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    assert_eq!(orphan_choice.display_label(), Some("orphan choice"));
    assert_eq!(
        first.orphan_fields()[5].disposition(),
        OrphanFieldDisposition::ReattachmentBlocked
    );
    assert_eq!(
        first.orphan_fields()[6].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    assert_eq!(
        first.orphan_fields()[7].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    assert_eq!(
        first.orphan_fields()[8].disposition(),
        OrphanFieldDisposition::PreservedOrphan
    );
    assert_eq!(
        first.orphan_fields()[9].disposition(),
        OrphanFieldDisposition::SnapshotMissing
    );
    assert_eq!(
        first.orphan_fields()[0].display_label(),
        Some("credential=singleLineText-label")
    );
    assert_eq!(
        first.orphan_fields()[7].display_label(),
        Some("orphan rich label")
    );
    assert!(first.orphan_fields()[9].uses_id_fallback());
    assert_eq!(first.warnings().len(), 1);
    assert_eq!(first.warnings()[0].field_id(), id(TIME));
    assert_eq!(first.warnings()[0].count(), 1);
    assert!(first.materialization_required());
    assert!(!first.can_materialize());
    assert!(first
        .blocking_issues()
        .iter()
        .all(|issue| ![id(TEXT), id(DATE)].contains(&issue.field_id())));
    assert!(first.blocking_issues().iter().any(
        |issue| issue.category() == DocumentReconciliationIssueCategory::OrphanSnapshotMissing
    ));
    assert!(first.blocking_issues().iter().any(
        |issue| issue.category() == DocumentReconciliationIssueCategory::InvalidKnownFieldValue
    ));
    assert!(first.blocking_issues().iter().any(
        |issue| issue.category() == DocumentReconciliationIssueCategory::UnknownSelectedOption
    ));
    assert!(first.blocking_issues().iter().any(|issue| {
        issue.category() == DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict
    }));
    let rich_issues = first
        .blocking_issues()
        .iter()
        .filter(|issue| issue.field_id() == id(RICH))
        .map(|issue| issue.category())
        .collect::<Vec<_>>();
    assert_eq!(
        rich_issues,
        vec![
            DocumentReconciliationIssueCategory::InvalidKnownFieldValue,
            DocumentReconciliationIssueCategory::ReattachmentSnapshotConflict,
        ]
    );
    let single_issue = first
        .blocking_issues()
        .iter()
        .find(|issue| issue.field_id() == id(SINGLE))
        .expect("unknown reattachment selection must be blocking");
    assert_eq!(
        single_issue.category(),
        DocumentReconciliationIssueCategory::UnknownSelectedOption
    );
    assert_eq!(
        single_issue.validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );
    assert_eq!(
        first
            .blocking_issues()
            .iter()
            .map(|issue| (issue.field_id(), issue.category()))
            .collect::<Vec<_>>(),
        second
            .blocking_issues()
            .iter()
            .map(|issue| (issue.field_id(), issue.category()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first
            .warnings()
            .iter()
            .map(|warning| (warning.field_id(), warning.category(), warning.count()))
            .collect::<Vec<_>>(),
        second
            .warnings()
            .iter()
            .map(|warning| (warning.field_id(), warning.category(), warning.count()))
            .collect::<Vec<_>>()
    );
    for rendered in [
        format!("{first:?}"),
        format!("{:?}", first.known_fields()),
        format!("{:?}", first.orphan_fields()),
        format!("{:?}", first.warnings()),
        format!("{:?}", first.blocking_issues()),
        format!("{:?}", first.known_fields()[0].provenance()),
    ] {
        assert_redacted(&rendered);
        assert_redacted_from(
            &rendered,
            &[
                "futureDocument",
                "futureOrphan",
                "orphan-extra",
                "futureOption",
                "option-extra",
                "orphan option",
                ORPHAN_OPTION,
                "c0000000-0000-4000-8000-",
                "old text label",
                "old choice label",
                "unknown historical",
                "orphan rich label",
                "reattachable",
                "kind mismatch",
                "conflicting regular value",
                "999.0001",
                "credential=archived-label",
            ],
        );
    }
    for rendered in [
        format!("{orphan_choice:?}"),
        format!("{:?}", first.orphan_fields()),
    ] {
        assert_redacted_from(
            &rendered,
            &[
                "orphan option",
                ORPHAN_OPTION,
                "c0000000-0000-4000-8000-",
                "futureOption",
                "option-extra",
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
    Ok(())
}

#[test]
fn invalid_orphan_relationships_are_rejected_during_document_admission(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value([], &[], 1, "active"));
    let raw = document_value(
        [
            (
                ORPHAN_A,
                json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
            ),
            (
                ORPHAN_B,
                json!({"kind":"singleChoice","optionId":OPTION_OTHER}),
            ),
        ],
        [
            (
                ORPHAN_A,
                orphan_snapshot("a", "singleChoice", json!({(OPTION_ACTIVE):{"label":"a"}})),
            ),
            (
                ORPHAN_B,
                orphan_snapshot("b", "singleChoice", json!({(OPTION_OTHER):{"label":"b"}})),
            ),
        ],
        1,
        TEMPLATE_ID,
    );
    let mut kind_mismatch = decode_document_value(&raw);
    kind_mismatch.corrupt_orphan_kind_for_test(id(ORPHAN_A), FieldKind::Number);
    let error = reconcile_document(&template, &kind_mismatch)
        .expect_err("orphan kind mismatch must fail Document admission");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidDocument
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanValueKindMismatch)
    );

    let mut duplicate = decode_document_value(&raw);
    duplicate.duplicate_orphan_option_for_test(
        id(ORPHAN_A),
        id(ORPHAN_B),
        crate::data::artifact::OptionId::from_str(OPTION_ACTIVE)?,
    );
    let error = reconcile_document(&template, &duplicate)
        .expect_err("global orphan OptionId collision must fail Document admission");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidDocument
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::DuplicateOptionId)
    );

    let mut snapshot_mismatch = raw.clone();
    snapshot_mismatch["orphanedFieldDefinitions"][ORPHAN_A]["options"] = json!({});
    let error = decode_document(&deterministic(&snapshot_mismatch))
        .expect_err("selected Option and snapshot Option sets must match");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanOptionMismatch)
    );

    let mut missing_value = raw;
    missing_value["fieldValues"]
        .as_object_mut()
        .expect("fieldValues must be an object")
        .remove(ORPHAN_A);
    let error = decode_document(&deterministic(&missing_value))
        .expect_err("an orphan snapshot without a value must fail admission");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanMissingValue)
    );
    Ok(())
}

#[test]
fn borrowed_metadata_and_arbitrary_precision_remain_byte_identical_and_debug_redacted(
) -> Result<(), Box<dyn Error>> {
    let integer = serde_json::from_str::<Value>("123456789012345678901234567890123456789")?;
    let decimal = serde_json::from_str::<Value>("9.87654321098765432109876543210987654321e+210")?;
    let mut initial = rich_text("rich-secret credential=body");
    initial["futureOuter"] = json!({"nested":[integer.clone(),{"decimal":decimal.clone()}]});
    initial["document"]["futureEnvelope"] = json!("m2_5c-envelope-value-canary");
    initial["document"]["content"]["futureRoot"] = json!(["m2_5c-root-value-canary", 3, 2, 1]);
    initial["document"]["content"]["children"][0]["futureContainer"] =
        json!("m2_5c-container-value-canary");
    initial["document"]["content"]["children"][0]["children"][0]["futureText"] =
        json!("text-extra");
    assert_eq!(
        initial["futureOuter"]["nested"],
        json!([integer, {"decimal": decimal}])
    );
    assert_eq!(
        initial["document"]["futureEnvelope"],
        json!("m2_5c-envelope-value-canary")
    );
    assert_eq!(
        initial["document"]["content"]["futureRoot"][0],
        json!("m2_5c-root-value-canary")
    );
    assert_eq!(
        initial["document"]["content"]["children"][0]["futureContainer"],
        json!("m2_5c-container-value-canary")
    );
    assert_eq!(
        initial["document"]["content"]["children"][0]["children"][0]["futureText"],
        json!("text-extra")
    );
    assert_eq!(
        initial["document"]["content"]["children"][0]["children"][0]["text"],
        json!("rich-secret credential=body")
    );
    let mut raw_template = template_value(
        [(
            RICH,
            field(
                "richText",
                "active",
                false,
                rich_text("current"),
                initial,
                2,
                basic_configuration("richText"),
            ),
        )],
        &[RICH],
        2,
        "active",
    );
    raw_template["presentation"]["futureTemplatePresentation"] =
        json!("m2_5c-template-presentation-value");
    raw_template["fields"][RICH]["presentation"]["futureFieldPresentation"] =
        json!("m2_5c-field-presentation-value");
    raw_template["fields"][RICH]["configuration"]["futureConfiguration"] =
        json!("m2_5c-configuration-value");
    assert_eq!(raw_template["name"], json!("credential=template-name"));
    assert_eq!(
        raw_template["fields"][RICH]["label"],
        json!("credential=richText-label")
    );
    assert_eq!(
        raw_template["presentation"]["futureTemplatePresentation"],
        json!("m2_5c-template-presentation-value")
    );
    assert_eq!(
        raw_template["fields"][RICH]["presentation"]["futureFieldPresentation"],
        json!("m2_5c-field-presentation-value")
    );
    assert_eq!(
        raw_template["fields"][RICH]["configuration"]["futureConfiguration"],
        json!("m2_5c-configuration-value")
    );
    let template = decode_template_value(&raw_template);
    let raw_document = document_value([], [], 1, TEMPLATE_ID);
    assert_eq!(
        raw_document["name"],
        json!("credential=document-name C:\\private\\document.json")
    );
    assert_eq!(
        raw_document["futureDocument"]["url"],
        json!("https://private.example.invalid/token")
    );
    let document = decode_document_value(&raw_document);
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);
    let view = reconcile_document(&template, &document)?;
    let borrowed = view.known_fields()[0].value().expect("historical value");
    assert!(std::ptr::eq(
        borrowed,
        template.fields()[&id(RICH)].initial_default_value()
    ));
    assert_eq!(template_bytes(&template), template_snapshot);
    assert_eq!(document_bytes(&document), document_snapshot);
    let forbidden = [
        "futureOuter",
        "futureEnvelope",
        "futureRoot",
        "futureContainer",
        "futureText",
        "text-extra",
        "m2_5c-envelope-value-canary",
        "m2_5c-root-value-canary",
        "m2_5c-container-value-canary",
        "futureTemplatePresentation",
        "m2_5c-template-presentation-value",
        "futureFieldPresentation",
        "m2_5c-field-presentation-value",
        "futureConfiguration",
        "m2_5c-configuration-value",
        "123456789012345678901234567890123456789",
        "9.87654321098765432109876543210987654321e+210",
        "rich-secret",
        "credential=richText-label",
    ];
    for rendered in [
        format!("{view:?}"),
        format!("{:?}", view.known_fields()[0]),
        format!("{:?}", view.known_fields()[0].provenance()),
    ] {
        assert_redacted(&rendered);
        assert_redacted_from(&rendered, &forbidden);
    }
    assert_eq!(view.template_id(), template.template_id());
    assert_eq!(view.template_revision(), template.revision());
    assert_eq!(
        view.document_template_revision(),
        document.template_revision()
    );
    Ok(())
}

fn assert_reconciliation_path_redaction(
    path: &str,
    component: &str,
    escaped: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let template = decode_template_value(&template_value(
        [(TEXT, text_field("active", false, 1))],
        &[TEXT],
        1,
        "active",
    ));
    let mut known_value = json!({"kind":"text","value":"path-only-scalar-payload"});
    known_value["futureKnownPath"] = json!(path);
    let mut orphan_value = json!({"kind":"text","value":"path-only-orphan-payload"});
    orphan_value["futureOrphanValuePath"] = json!(path);
    let mut snapshot = orphan_snapshot("path-only orphan", "singleLineText", json!({}));
    snapshot["futureOrphanSnapshotPath"] = json!(path);
    let mut raw_document = document_value(
        [(TEXT, known_value), (ORPHAN_A, orphan_value)],
        [(ORPHAN_A, snapshot)],
        1,
        TEMPLATE_ID,
    );
    raw_document["name"] = json!("path-only document");
    raw_document["futureDocument"] = json!({"futureRootPath":path});
    assert_eq!(
        raw_document["fieldValues"][TEXT]["futureKnownPath"],
        json!(path)
    );
    assert_eq!(
        raw_document["fieldValues"][ORPHAN_A]["futureOrphanValuePath"],
        json!(path)
    );
    assert_eq!(
        raw_document["orphanedFieldDefinitions"][ORPHAN_A]["futureOrphanSnapshotPath"],
        json!(path)
    );
    assert_eq!(
        raw_document["futureDocument"]["futureRootPath"],
        json!(path)
    );

    let document = decode_document_value(&raw_document);
    let before = document.clone();
    let before_bytes = document_bytes(&document);
    let view = reconcile_document(&template, &document)?;
    let mut forbidden = vec![
        path,
        component,
        "path-only-scalar-payload",
        "path-only-orphan-payload",
        "futureKnownPath",
        "futureOrphanValuePath",
        "futureOrphanSnapshotPath",
        "futureRootPath",
    ];
    if let Some(escaped) = escaped {
        forbidden.push(escaped);
    }
    for rendered in [
        format!("{view:?}"),
        format!("{:?}", view.known_fields()[0]),
        format!("{:?}", view.orphan_fields()[0]),
        format!("{:?}", view.known_fields()[0].provenance()),
        format!("{:?}", view.warnings()),
        format!("{:?}", view.blocking_issues()),
    ] {
        assert_redacted_from(&rendered, &forbidden);
    }
    assert_eq!(document, before);
    assert_eq!(document_bytes(&document), before_bytes);

    let mut invalid = document.clone();
    invalid.corrupt_scalar_value_for_test(id(TEXT), "invalid\npath-only");
    let invalid_before = invalid.clone();
    let invalid_bytes = unchecked_document_bytes(&invalid);
    let error = reconcile_document(&template, &invalid)
        .expect_err("invalid path-bearing Document must fail admission");
    assert_eq!(
        error.category(),
        DocumentReconciliationErrorCategory::InvalidDocument
    );
    assert!(error.source().is_none());
    assert_redacted_from(&format!("{error} {error:?}"), &forbidden);
    assert_eq!(invalid, invalid_before);
    assert_eq!(unchecked_document_bytes(&invalid), invalid_bytes);
    Ok(())
}

fn path_forbidden<'a>(windows_path: &'a str, unix_path: &'a str) -> Vec<&'a str> {
    vec![
        windows_path,
        r"C:\M2_5C_DIAGNOSTIC_PATH_CANARY",
        "M2_5C_DIAGNOSTIC_PATH_CANARY",
        unix_path,
        "M2_5C_DIAGNOSTIC_UNIX_CANARY",
    ]
}

#[test]
fn non_empty_warning_debug_redacts_independent_path_metadata() -> Result<(), Box<dyn Error>> {
    let windows_path = r"C:\M2_5C_DIAGNOSTIC_PATH_CANARY\warning.txt";
    let unix_path = "/tmp/M2_5C_DIAGNOSTIC_UNIX_CANARY/warning.txt";
    let escaped_windows_path = windows_path.escape_debug().to_string();
    let mut raw_template = template_value(
        [(
            SINGLE,
            field(
                "singleChoice",
                "active",
                false,
                json!({"kind":"singleChoice","optionId":OPTION_SINGLE_ACTIVE}),
                json!({"kind":"unset"}),
                1,
                single_choice_configuration(),
            ),
        )],
        &[SINGLE],
        1,
        "active",
    );
    raw_template["name"] = json!("warning path template");
    raw_template["fields"][SINGLE]["label"] = json!("warning path field");
    raw_template["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_ACTIVE]["label"] =
        json!("active");
    raw_template["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_ARCHIVED]["label"] =
        json!("archived");
    raw_template["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_OTHER]["label"] =
        json!("other");
    assert_eq!(raw_template["name"], json!("warning path template"));
    assert_eq!(
        raw_template["fields"][SINGLE]["label"],
        json!("warning path field")
    );
    assert_eq!(
        raw_template["fields"][SINGLE]["configuration"]["options"][OPTION_SINGLE_ARCHIVED]["label"],
        json!("archived")
    );
    let template = decode_template_value(&raw_template);

    let mut selected = json!({
        "kind":"singleChoice",
        "optionId":OPTION_SINGLE_ARCHIVED,
    });
    selected["futureWarningWindowsPath"] = json!(windows_path);
    selected["futureWarningUnixPath"] = json!(unix_path);
    let mut raw_document = document_value([(SINGLE, selected)], [], 1, TEMPLATE_ID);
    raw_document["name"] = json!("warning path document");
    raw_document["futureDocument"] = json!({});
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["futureWarningWindowsPath"],
        json!(windows_path)
    );
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["futureWarningUnixPath"],
        json!(unix_path)
    );
    assert_eq!(
        raw_document["fieldValues"][SINGLE]["optionId"],
        json!(OPTION_SINGLE_ARCHIVED)
    );
    let document = decode_document_value(&raw_document);
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let view = reconcile_document(&template, &document)?;
    assert!(view.blocking_issues().is_empty());
    assert_eq!(view.warnings().len(), 1);
    let warning = view.warnings()[0];
    assert_eq!(
        warning.category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(warning.field_id(), id(SINGLE));
    assert_eq!(warning.count(), 1);
    assert!(!warning.truncated());

    let mut forbidden = path_forbidden(windows_path, unix_path);
    forbidden.push(&escaped_windows_path);
    for rendered in [
        format!("{view:?}"),
        format!("{:?}", view.known_fields()[0]),
        format!("{warning:?}"),
        format!("{:?}", view.warnings()),
    ] {
        assert_redacted_from(&rendered, &forbidden);
    }
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
fn non_empty_blocking_issue_debug_redacts_independent_path_metadata() -> Result<(), Box<dyn Error>>
{
    let windows_path = r"C:\M2_5C_DIAGNOSTIC_PATH_CANARY\blocking.txt";
    let unix_path = "/tmp/M2_5C_DIAGNOSTIC_UNIX_CANARY/blocking.txt";
    let escaped_windows_path = windows_path.escape_debug().to_string();
    let mut raw_template = template_value(
        [(TEXT, text_field("active", true, 1))],
        &[TEXT],
        1,
        "active",
    );
    raw_template["name"] = json!("blocking path template");
    raw_template["fields"][TEXT]["label"] = json!("blocking path field");
    let template = decode_template_value(&raw_template);

    let mut unset = json!({"kind":"number","value":"1"});
    unset["futureBlockingWindowsPath"] = json!(windows_path);
    unset["futureBlockingUnixPath"] = json!(unix_path);
    let mut raw_document = document_value([(TEXT, unset)], [], 1, TEMPLATE_ID);
    raw_document["name"] = json!("blocking path document");
    raw_document["futureDocument"] = json!({});
    assert_eq!(
        raw_document["fieldValues"][TEXT]["futureBlockingWindowsPath"],
        json!(windows_path)
    );
    assert_eq!(
        raw_document["fieldValues"][TEXT]["futureBlockingUnixPath"],
        json!(unix_path)
    );
    assert_eq!(raw_document["fieldValues"][TEXT]["kind"], json!("number"));
    let document = decode_document_value(&raw_document);
    let template_before = template.clone();
    let document_before = document.clone();
    let template_snapshot = template_bytes(&template);
    let document_snapshot = document_bytes(&document);

    let view = reconcile_document(&template, &document)?;
    assert!(view.warnings().is_empty());
    assert_eq!(view.blocking_issues().len(), 1);
    let issue = view.blocking_issues()[0];
    assert_eq!(
        issue.category(),
        DocumentReconciliationIssueCategory::InvalidKnownFieldValue
    );
    assert_eq!(issue.field_id(), id(TEXT));
    assert_eq!(
        issue.validation_category(),
        Some(FieldValidationErrorCategory::FieldValueKindMismatch)
    );
    assert_eq!(
        issue.validation_location(),
        Some(FieldValidationLocation::ExistingDocumentValue)
    );

    let mut forbidden = path_forbidden(windows_path, unix_path);
    forbidden.push(&escaped_windows_path);
    for rendered in [
        format!("{view:?}"),
        format!("{:?}", view.known_fields()[0]),
        format!("{issue:?}"),
        format!("{:?}", view.blocking_issues()),
    ] {
        assert_redacted_from(&rendered, &forbidden);
    }
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
fn reconciliation_debug_redacts_independent_windows_and_unix_path_values(
) -> Result<(), Box<dyn Error>> {
    let windows_path = r"C:\M2_5C_WINDOWS_PATH_CANARY\secret.txt";
    let escaped_windows_path = windows_path.escape_debug().to_string();
    assert_reconciliation_path_redaction(
        windows_path,
        "M2_5C_WINDOWS_PATH_CANARY",
        Some(&escaped_windows_path),
    )?;
    assert_reconciliation_path_redaction(
        "/tmp/M2_5C_UNIX_PATH_CANARY/secret.txt",
        "M2_5C_UNIX_PATH_CANARY",
        None,
    )?;
    Ok(())
}
