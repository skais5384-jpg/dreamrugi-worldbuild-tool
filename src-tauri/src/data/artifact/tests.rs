use std::{any::TypeId, collections::BTreeMap, error::Error, str::FromStr};

use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use uuid::Version;

use super::*;
use crate::data::{
    field_engine::validation::{
        BoundDocumentValueContext, FieldDiagnosticCategory, FieldValidationErrorCategory,
        FieldValidationOutcome,
    },
    json::to_deterministic_json_bytes,
    project_lock::PROJECT_LOCK_METADATA_SCHEMA_VERSION,
    schema::CURRENT_SCHEMA_VERSION,
    transaction::TRANSACTION_SCHEMA_VERSION,
};

const TEMPLATE_ID: &str = "11111111-1111-4111-8111-111111111111";
const FIELD_TEXT: &str = "22222222-2222-4222-8222-222222222222";
const FIELD_CHOICE: &str = "33333333-3333-4333-8333-333333333333";
const OPTION_ACTIVE: &str = "44444444-4444-4444-8444-444444444444";
const OPTION_ACTIVE_SECOND: &str = "45454545-4545-4545-8545-454545454545";
const OPTION_ARCHIVED: &str = "55555555-5555-4555-8555-555555555555";
const OPTION_OTHER_FIELD_FIRST: &str = "60606060-6060-4060-8060-606060606060";
const OPTION_OTHER_FIELD_SECOND: &str = "61616161-6161-4161-8161-616161616161";
const OPTION_UNKNOWN: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const DOCUMENT_ID: &str = "66666666-6666-4666-8666-666666666666";
const TARGET_DOCUMENT_ID: &str = "69696969-6969-4969-8969-696969696969";
const REFERENCE_ID: &str = "68686868-6868-4868-8868-686868686868";
const FIELD_GROUP: &str = "90909090-9090-4090-8090-909090909090";
const FIELD_GROUP_RELATION: &str = "91919191-9191-4191-8191-919191919191";
const GROUP_INSTANCE: &str = "92929292-9292-4292-8292-929292929292";
const FIELD_ARCHIVED: &str = "77777777-7777-4777-8777-777777777777";
const FIELD_ORPHAN_CHOICE: &str = "88888888-8888-4888-8888-888888888888";
const SECRET_BODY: &str = "credential=artifact-secret&path=C:\\private\\artifact.json";

#[test]
fn m42_relation_and_document_link_versions_round_trip_and_reject_invalid_identity() {
    let relation = json!({
        "kind":"relation",
        "links":[{"id":REFERENCE_ID,"document":TARGET_DOCUMENT_ID,"oneWay":true,"name":"친구"}]
    });
    let mut template = template_value();
    template["schemaVersion"] = 7.into();
    template["fields"][FIELD_TEXT]["kind"] = json!("relation");
    template["fields"][FIELD_TEXT]["configuration"] = json!({
        "kind":"relation",
        "multiple":true,
        "allowedTemplates":[TEMPLATE_ID],
        "reciprocalNotice":true
    });
    template["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"unset"});
    template["fields"][FIELD_TEXT]["initialDefaultValue"] = json!({"kind":"unset"});
    let model =
        decode_template(&deterministic(&template)).expect("relation template should decode");
    let round: Value = serde_json::from_slice(&encode_template(&model).unwrap()).unwrap();
    assert_eq!(
        round["fields"][FIELD_TEXT]["configuration"]["multiple"],
        true
    );
    template["schemaVersion"] = 6.into();
    assert!(decode_template(&deterministic(&template)).is_ok());
    template["schemaVersion"] = 5.into();
    assert!(decode_template(&deterministic(&template)).is_err());

    let mut document = document_value();
    document["schemaVersion"] = 6.into();
    document["fieldValues"][FIELD_TEXT] = relation.clone();
    let model =
        decode_document(&deterministic(&document)).expect("relation document should decode");
    let round: Value = serde_json::from_slice(&encode_document(&model).unwrap()).unwrap();
    assert_eq!(round["fieldValues"][FIELD_TEXT], relation);
    document["schemaVersion"] = 5.into();
    assert!(decode_document(&deterministic(&document)).is_err());

    let mut unnamed = relation.clone();
    unnamed["links"][0].as_object_mut().unwrap().remove("name");
    document["fieldValues"][FIELD_TEXT] = unnamed;
    assert!(
        decode_document(&deterministic(&document)).is_ok(),
        "document v5 remains readable when relation names are absent"
    );

    document["schemaVersion"] = 6.into();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"relation",
        "links":[
            {"id":REFERENCE_ID,"document":TARGET_DOCUMENT_ID,"oneWay":false},
            {"id":OPTION_ACTIVE_SECOND,"document":TARGET_DOCUMENT_ID,"oneWay":false}
        ]
    });
    assert!(decode_document(&deterministic(&document)).is_err());
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"relation",
        "links":[{"id":REFERENCE_ID,"document":DOCUMENT_ID,"oneWay":false}]
    });
    assert!(decode_document(&deterministic(&document)).is_err());
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"documentLink",
        "documentIds":[DOCUMENT_ID]
    });
    assert!(
        decode_document(&deterministic(&document)).is_ok(),
        "document links intentionally allow self references"
    );

    let relation_member = field(
        "하위 관계",
        "active",
        "relation",
        2,
        json!({
            "kind":"relation",
            "multiple":true,
            "allowedTemplates":[],
            "reciprocalNotice":true
        }),
    );
    let mut group_template = template_value();
    group_template["schemaVersion"] = 6.into();
    group_template["fields"][FIELD_GROUP] = field(
        "관계 묶음",
        "active",
        "group",
        2,
        json!({
            "kind":"group",
            "memberOrder":[FIELD_GROUP_RELATION],
            "members":{(FIELD_GROUP_RELATION):relation_member}
        }),
    );
    group_template["fieldOrder"]
        .as_array_mut()
        .unwrap()
        .push(FIELD_GROUP.into());
    decode_template(&deterministic(&group_template))
        .expect("group relation template should decode");

    let mut group_document = document_value();
    group_document["schemaVersion"] = 6.into();
    group_document["fieldValues"][FIELD_GROUP] = json!({
        "kind":"group",
        "instanceOrder":[GROUP_INSTANCE],
        "instances":{
            (GROUP_INSTANCE):{
                "revision":2,
                "values":{
                    (FIELD_GROUP_RELATION):{
                        "kind":"relation",
                        "links":[{
                            "id":REFERENCE_ID,
                            "document":TARGET_DOCUMENT_ID,
                            "oneWay":false,
                            "name":"가족"
                        }]
                    }
                },
                "labels":{(FIELD_GROUP_RELATION):"하위 관계"}
            }
        }
    });
    let model = decode_document(&deterministic(&group_document))
        .expect("group child relation should decode");
    let round: Value = serde_json::from_slice(&encode_document(&model).unwrap()).unwrap();
    assert_eq!(
        round["fieldValues"][FIELD_GROUP]["instances"][GROUP_INSTANCE]["values"]
            [FIELD_GROUP_RELATION]["links"][0]["document"],
        TARGET_DOCUMENT_ID
    );
    assert_eq!(
        round["fieldValues"][FIELD_GROUP]["instances"][GROUP_INSTANCE]["values"]
            [FIELD_GROUP_RELATION]["links"][0]["name"],
        "가족"
    );
    group_document["fieldValues"][FIELD_GROUP]["instances"][GROUP_INSTANCE]["values"]
        [FIELD_GROUP_RELATION]["links"][0]["document"] = DOCUMENT_ID.into();
    assert!(
        decode_document(&deterministic(&group_document)).is_err(),
        "group child self relations are rejected recursively"
    );
}

#[test]
fn m42_fix002_editing_an_unnamed_v5_relation_upgrades_the_saved_document() {
    let mut template = template_value();
    template["schemaVersion"] = 6.into();
    template["fields"][FIELD_TEXT]["kind"] = json!("relation");
    template["fields"][FIELD_TEXT]["configuration"] = json!({
        "kind":"relation",
        "multiple":true,
        "allowedTemplates":[TEMPLATE_ID],
        "reciprocalNotice":true
    });
    template["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"unset"});
    template["fields"][FIELD_TEXT]["initialDefaultValue"] = json!({"kind":"unset"});
    let template = decode_template(&deterministic(&template)).expect("valid relation template");

    let mut document = document_value();
    document["schemaVersion"] = 5.into();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"relation",
        "links":[{"id":REFERENCE_ID,"document":TARGET_DOCUMENT_ID,"oneWay":false}]
    });
    let document =
        decode_document(&deterministic(&document)).expect("readable unnamed v5 document");
    let edits = DocumentEditSet::new(vec![DocumentEdit::SetValue(
        FieldId::from_str(FIELD_TEXT).unwrap(),
        DocumentValueEdit::relation(vec![(
            ReferenceId::from_str(REFERENCE_ID).unwrap(),
            DocumentId::from_str(TARGET_DOCUMENT_ID).unwrap(),
            false,
            "친구".into(),
        )]),
    )]);

    let outcome = prepare_document_save(
        &template,
        template.revision(),
        &document,
        &edits,
        "2026-09-20T00:00:00.000Z",
    )
    .expect("named relation edit should upgrade and save");
    let round: Value =
        serde_json::from_slice(&encode_document(outcome.document()).unwrap()).unwrap();
    assert_eq!(round["schemaVersion"], 6);
    assert_eq!(round["fieldValues"][FIELD_TEXT]["links"][0]["name"], "친구");
}

#[test]
fn m37_media_versions_references_unknown_extras_and_legacy_conflict() {
    for kind in ["image", "file", "url"] {
        let value = if kind == "url" {
            json!({"kind":kind,"value":"https://example.com/clip.mp4","future":{"nested":7}})
        } else {
            json!({"kind":kind,"value":[OPTION_ACTIVE,OPTION_ACTIVE_SECOND],"future":{"nested":7}})
        };
        let mut t = template_value();
        t["schemaVersion"] = 4.into();
        t["fields"][FIELD_TEXT]["kind"] = kind.into();
        t["fields"][FIELD_TEXT]["configuration"] = json!({"kind":kind,"future":"preserved"});
        t["fields"][FIELD_TEXT]["defaultValue"] = value.clone();
        t["fields"][FIELD_TEXT]["initialDefaultValue"] = value.clone();
        let model = decode_template(&deterministic(&t)).unwrap();
        let round: Value = serde_json::from_slice(&encode_template(&model).unwrap()).unwrap();
        assert_eq!(round["fields"][FIELD_TEXT]["defaultValue"], value);
        t["schemaVersion"] = 3.into();
        assert!(decode_template(&deterministic(&t)).is_err());
        let mut d = document_value();
        d["schemaVersion"] = 3.into();
        d["fieldValues"][FIELD_TEXT] = value.clone();
        let model = decode_document(&deterministic(&d)).unwrap();
        let round: Value = serde_json::from_slice(&encode_document(&model).unwrap()).unwrap();
        assert_eq!(round["fieldValues"][FIELD_TEXT], value);
        d["schemaVersion"] = 2.into();
        assert!(decode_document(&deterministic(&d)).is_err());
    }
    for invalid in [
        json!([OPTION_ACTIVE, OPTION_ACTIVE]),
        json!(["../escape"]),
        json!(["00000000-0000-0000-0000-000000000000"]),
    ] {
        let mut d = document_value();
        d["schemaVersion"] = 3.into();
        d["fieldValues"][FIELD_TEXT] = json!({"kind":"image","value":invalid});
        assert!(decode_document(&deterministic(&d)).is_err());
    }
}

fn deterministic(value: &Value) -> Vec<u8> {
    to_deterministic_json_bytes(value).expect("test JSON should serialize")
}

fn replace_number_markers(value: &Value, replacements: &[(&str, &str)]) -> Vec<u8> {
    let mut raw = String::from_utf8(deterministic(value)).expect("fixture JSON must be UTF-8");
    for (marker, lexeme) in replacements {
        let quoted = format!("\"{marker}\"");
        assert!(
            raw.contains(&quoted),
            "number marker {marker} must occur in the fixture"
        );
        assert!(
            serde_json::from_str::<Box<serde_json::value::RawValue>>(lexeme).is_ok(),
            "replacement must be a valid JSON number"
        );
    }
    for (marker, lexeme) in replacements {
        let quoted = format!("\"{marker}\"");
        raw = raw.replace(&quoted, lexeme);
    }
    raw.into_bytes()
}

fn assert_number_lexeme(output: &str, key: &str, lexeme: &str) {
    assert!(
        output.contains(&format!("\"{key}\": {lexeme}")),
        "number lexeme for {key} changed from {lexeme}"
    );
}

fn basic_configuration(kind: &str) -> Value {
    json!({ "kind": kind })
}

fn choice_configuration(kind: &str) -> Value {
    json!({
        "kind": kind,
        // 표시 순서는 의도적으로 OptionId 순서와 다르다.
        "optionOrder": [OPTION_ACTIVE_SECOND, OPTION_ACTIVE],
        "options": {
            (OPTION_ACTIVE): { "label": "첫 선택", "lifecycle": "active" },
            (OPTION_ACTIVE_SECOND): { "label": "둘째 선택", "lifecycle": "active" },
            (OPTION_ARCHIVED): { "label": "과거 선택", "lifecycle": "archived" }
        }
    })
}

fn choice_template_value(kind: &str, lifecycle: &str) -> Value {
    let mut template = minimal_template_value();
    template["fields"][FIELD_TEXT]["kind"] = json!(kind);
    template["fields"][FIELD_TEXT]["lifecycle"] = json!(lifecycle);
    template["fields"][FIELD_TEXT]["configuration"] = choice_configuration(kind);
    if lifecycle == "archived" {
        template["fieldOrder"] = json!([]);
    }
    template
}

fn field_local_choice_template_value(kind: &str, field_a_lifecycle: &str) -> Value {
    let mut template = choice_template_value(kind, field_a_lifecycle);
    template["fields"][FIELD_CHOICE] = field(
        "두 번째 선택 필드",
        "active",
        kind,
        1,
        json!({
            "kind": kind,
            // Field B도 표시 순서와 membership/canonical ID 순서가 무관함을 유지한다.
            "optionOrder": [OPTION_OTHER_FIELD_SECOND, OPTION_OTHER_FIELD_FIRST],
            "options": {
                (OPTION_OTHER_FIELD_FIRST): {
                    "label": "다른 필드의 첫 선택",
                    "lifecycle": "active"
                },
                (OPTION_OTHER_FIELD_SECOND): {
                    "label": "다른 필드의 둘째 선택",
                    "lifecycle": "active"
                }
            }
        }),
    );
    template["fieldOrder"] = if field_a_lifecycle == "active" {
        json!([FIELD_TEXT, FIELD_CHOICE])
    } else {
        json!([FIELD_CHOICE])
    };

    let field_b_default = match kind {
        "singleChoice" => {
            json!({"kind":"singleChoice","optionId":OPTION_OTHER_FIELD_FIRST})
        }
        "multiChoice" => json!({
            "kind":"multiChoice",
            "optionIds":[OPTION_OTHER_FIELD_FIRST,OPTION_OTHER_FIELD_SECOND]
        }),
        _ => panic!("field-local fixture requires a choice kind"),
    };
    template["fields"][FIELD_CHOICE]["defaultValue"] = field_b_default.clone();
    template["fields"][FIELD_CHOICE]["initialDefaultValue"] = field_b_default;
    template
}

fn rich_text_template_value(field_lifecycle: &str) -> Value {
    let mut template = minimal_template_value();
    template["fields"][FIELD_TEXT]["kind"] = json!("richText");
    template["fields"][FIELD_TEXT]["lifecycle"] = json!(field_lifecycle);
    template["fields"][FIELD_TEXT]["configuration"] = basic_configuration("richText");
    template["fields"][FIELD_TEXT]["defaultValue"] = rich_text_value("현재 기본값");
    template["fields"][FIELD_TEXT]["initialDefaultValue"] = rich_text_value("초기 기본값");
    if field_lifecycle == "archived" {
        template["fieldOrder"] = json!([]);
    }
    template
}

fn reserved_marker_object(reserved_key: &str) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        reserved_key.to_owned(),
        Value::String("1".to_owned()),
    )]))
}

fn nested_array_value(container_depth: usize, leaf: Value) -> Value {
    assert!(container_depth >= 1);
    let mut value = leaf;
    for _ in 0..container_depth {
        value = Value::Array(vec![value]);
    }
    value
}

fn rich_text_content(text: &str) -> Value {
    json!({
        "kind": "root",
        "children": [{
            "kind": "paragraph",
            "children": [{"kind": "text", "text": text}]
        }]
    })
}

fn hard_break_only_content() -> Value {
    json!({
        "kind": "root",
        "children": [{
            "kind": "paragraph",
            "children": [{"kind": "hardBreak"}]
        }]
    })
}

fn rich_text_value(text: &str) -> Value {
    json!({
        "kind": "richText",
        "document": {"schemaVersion": 1, "content": rich_text_content(text)}
    })
}

fn maximum_container_depth(value: &Value) -> usize {
    let mut maximum = 0;
    let mut pending = vec![(value, 0usize)];
    while let Some((value, parent_depth)) = pending.pop() {
        match value {
            Value::Object(object) => {
                let depth = parent_depth.checked_add(1).expect("test depth should fit");
                maximum = maximum.max(depth);
                pending.extend(object.values().map(|child| (child, depth)));
            }
            Value::Array(items) => {
                let depth = parent_depth.checked_add(1).expect("test depth should fit");
                maximum = maximum.max(depth);
                pending.extend(items.iter().map(|child| (child, depth)));
            }
            _ => {}
        }
    }
    maximum
}

fn document_wire_value(artifact: &DocumentArtifact) -> Value {
    serde_json::to_value(super::document::DocumentWire::from(artifact))
        .expect("test-only wire projection should serialize")
}

fn template_wire_value(artifact: &TemplateArtifact) -> Value {
    serde_json::to_value(super::template::TemplateWire::from(artifact))
        .expect("test-only wire projection should serialize")
}

fn field(
    label: &str,
    lifecycle: &str,
    kind: &str,
    introduced_revision: u32,
    configuration: Value,
) -> Value {
    json!({
        "configuration": configuration,
        "defaultValue": { "kind": "unset" },
        "initialDefaultValue": { "kind": "unset" },
        "introducedRevision": introduced_revision,
        "kind": kind,
        "label": label,
        "lifecycle": lifecycle,
        "presentation": {},
        "required": false
    })
}

fn template_value() -> Value {
    let mut template = json!({
        "artifactType": "template",
        "createdAtUtc": "2026-09-03T01:02:03.004Z",
        "fieldOrder": [FIELD_TEXT, FIELD_CHOICE],
        "fields": {
            (FIELD_TEXT): field(
                "이름",
                "active",
                "singleLineText",
                1,
                basic_configuration("singleLineText")
            ),
            (FIELD_CHOICE): field(
                "분류",
                "active",
                "singleChoice",
                2,
                json!({
                    "kind": "singleChoice",
                    "optionOrder": [OPTION_ACTIVE],
                    "options": {
                        (OPTION_ACTIVE): {
                            "label": "주인공",
                            "lifecycle": "active"
                        },
                        (OPTION_ARCHIVED): {
                            "label": "예전 분류",
                            "lifecycle": "archived"
                        }
                    }
                })
            ),
            (FIELD_ARCHIVED): field(
                "과거 기록",
                "archived",
                "richText",
                1,
                basic_configuration("richText")
            )
        },
        "lifecycle": "active",
        "name": "등장인물 템플릿",
        "presentation": { "token": "character" },
        "revision": 2,
        "schemaVersion": 1,
        "templateId": TEMPLATE_ID,
        "updatedAtUtc": "2026-09-03T02:03:04.005Z"
    });
    template["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"text","value":"현재 기본값"});
    template
}

fn minimal_template_value() -> Value {
    json!({
        "artifactType": "template",
        "createdAtUtc": "2026-09-03T01:02:03.004Z",
        "fieldOrder": [FIELD_TEXT],
        "fields": {
            (FIELD_TEXT): field(
                "이름",
                "active",
                "singleLineText",
                1,
                basic_configuration("singleLineText")
            )
        },
        "lifecycle": "active",
        "name": "등장인물 템플릿",
        "presentation": {},
        "revision": 1,
        "schemaVersion": 1,
        "templateId": TEMPLATE_ID,
        "updatedAtUtc": "2026-09-03T01:02:03.004Z"
    })
}

fn document_value() -> Value {
    json!({
        "artifactType": "document",
        "createdAtUtc": "2026-09-03T01:02:03.004Z",
        "documentId": DOCUMENT_ID,
        "fieldValues": {
            (FIELD_TEXT): { "kind": "text", "value": "한글 본문" },
            (FIELD_CHOICE): { "kind": "singleChoice", "optionId": OPTION_ACTIVE },
            (FIELD_ARCHIVED): { "kind": "unset" },
            (FIELD_ORPHAN_CHOICE): { "kind": "singleChoice", "optionId": OPTION_ARCHIVED }
        },
        "name": "아리아",
        "orphanedFieldDefinitions": {
            (FIELD_ARCHIVED): {
                "kind": "richText",
                "label": "과거 기록",
                "options": {}
            },
            (FIELD_ORPHAN_CHOICE): {
                "kind": "singleChoice",
                "label": "과거 분류",
                "options": {
                    (OPTION_ARCHIVED): { "label": "삭제된 선택지" }
                }
            }
        },
        "schemaVersion": 1,
        "templateId": TEMPLATE_ID,
        "templateRevision": 2,
        "updatedAtUtc": "2026-09-03T02:03:04.005Z"
    })
}

fn document_with_rich_text_content(content_value: Value) -> Value {
    let mut document = document_value();
    document["fieldValues"][FIELD_ARCHIVED] = json!({
        "kind": "richText",
        "document": {
            "schemaVersion": 1,
            "content": {
                "kind": "root",
                "children": [{
                    "kind": "paragraph",
                    "children": [{"kind": "text", "text": "depth boundary"}]
                }],
                "testOnlyDepth": content_value
            }
        }
    });
    document
}

fn template_with_rich_text_defaults() -> Value {
    let mut template = template_value();
    template["fields"][FIELD_ARCHIVED]["defaultValue"] = json!({
        "kind": "richText",
        "document": { "schemaVersion": 1, "content": rich_text_content("현재 기본값") }
    });
    template["fields"][FIELD_ARCHIVED]["initialDefaultValue"] = json!({
        "kind": "richText",
        "document": { "schemaVersion": 1, "content": rich_text_content("초기 기본값") }
    });
    template
}

fn assert_codec_category(
    result: Result<impl std::fmt::Debug, ArtifactCodecError>,
    expected: ArtifactCodecErrorCategory,
) {
    let error = result.expect_err("artifact input should be rejected");
    assert_eq!(error.category(), expected);
}

fn assert_scalar_codec_error(
    result: Result<impl std::fmt::Debug, ArtifactCodecError>,
    scalar_category: crate::data::field_engine::scalar::ScalarValueErrorCategory,
    location: ArtifactScalarValueLocation,
) -> ArtifactCodecError {
    let error = result.expect_err("invalid scalar artifact must be rejected");
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure
    );
    assert_eq!(error.stage(), ArtifactCodecStage::SemanticValidation);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidScalarValue)
    );
    assert_eq!(error.scalar_category(), Some(scalar_category));
    assert_eq!(error.scalar_location(), Some(location));
    error
}

fn assert_choice_codec_error(
    result: Result<impl std::fmt::Debug, ArtifactCodecError>,
    choice_category: crate::data::field_engine::choice::ChoiceValidationErrorCategory,
    location: ArtifactChoiceValueLocation,
) -> ArtifactCodecError {
    let error = result.expect_err("invalid choice artifact must be rejected");
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure
    );
    assert_eq!(error.stage(), ArtifactCodecStage::SemanticValidation);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(error.choice_category(), Some(choice_category));
    assert_eq!(error.choice_location(), Some(location));
    error
}

fn assert_rich_text_codec_error(
    result: Result<impl std::fmt::Debug, ArtifactCodecError>,
    rich_text_category: crate::data::field_engine::rich_text::RichTextValidationErrorCategory,
    location: ArtifactRichTextValueLocation,
) -> ArtifactCodecError {
    let error = result.expect_err("invalid rich-text artifact must be rejected");
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure
    );
    assert_eq!(error.stage(), ArtifactCodecStage::SemanticValidation);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidRichTextValue)
    );
    assert_eq!(error.rich_text_category(), Some(rich_text_category));
    assert_eq!(error.rich_text_location(), Some(location));
    error
}

fn assert_error_chain_redacted(error: &(dyn Error + 'static), forbidden: &[&str]) -> usize {
    let mut count = 0;
    let mut current = Some(error);
    while let Some(node) = current {
        let display = node.to_string();
        let debug = format!("{node:?}");
        for secret in forbidden {
            assert!(!display.contains(secret), "Display leaked {secret}");
            assert!(!debug.contains(secret), "Debug leaked {secret}");
        }
        count += 1;
        current = node.source();
    }
    count
}

fn assert_debug_redacted<T: std::fmt::Debug>(
    value: &T,
    expected_type: &str,
    forbidden: &[&str],
) -> String {
    let rendered = format!("{value:?}");
    assert!(
        rendered.contains(expected_type),
        "redacted Debug must retain its safe type name: {rendered}"
    );
    if let Some(sentinel) = first_leaked_fragment(&rendered, forbidden) {
        panic!("redacted Debug leaked sentinel {sentinel}");
    }
    rendered
}

fn first_leaked_fragment<'a>(rendered: &str, forbidden: &'a [&str]) -> Option<&'a str> {
    forbidden
        .iter()
        .copied()
        .find(|sentinel| rendered.contains(sentinel))
}

fn assert_json_object_has_key(value: &Value, key: &str) {
    assert!(
        value
            .as_object()
            .is_some_and(|object| object.contains_key(key)),
        "redaction fixture must contain its target extra key {key}"
    );
}

#[test]
fn debug_redaction_fragment_detection_rejects_raw_escaped_and_component_leaks() {
    const EXTRA_KEY: &str = "m2_5aSensitivityExtraKeyCanary4e8c20d1";
    const WINDOWS_PATH: &str = "C:\\M2_5A_DEBUG_REDACTION_5f9d31e2\\secret.txt";
    const PATH_COMPONENT: &str = "M2_5A_DEBUG_REDACTION_5f9d31e2";
    let escaped_windows_path = WINDOWS_PATH.escape_debug().to_string();

    for fragment in [
        EXTRA_KEY,
        WINDOWS_PATH,
        escaped_windows_path.as_str(),
        PATH_COMPONENT,
    ] {
        let deliberately_unsafe = format!("UnsafeDebug {{ payload: {fragment} }}");
        assert_eq!(
            first_leaked_fragment(&deliberately_unsafe, &[fragment]),
            Some(fragment)
        );
    }

    assert_eq!(
        first_leaked_fragment(
            "SafeDebug { extra_count: 1 }",
            &[
                EXTRA_KEY,
                WINDOWS_PATH,
                escaped_windows_path.as_str(),
                PATH_COMPONENT,
            ],
        ),
        None
    );
}

#[test]
fn every_persistent_id_generates_canonical_lowercase_uuid_v4() {
    let generated = [
        TemplateId::new().to_string(),
        FieldId::new().to_string(),
        OptionId::new().to_string(),
        DocumentId::new().to_string(),
    ];

    for value in generated {
        assert_eq!(value.len(), 36);
        assert_eq!(value, value.to_ascii_lowercase());
        assert_eq!(
            value.chars().filter(|character| *character == '-').count(),
            4
        );
        let uuid = uuid::Uuid::parse_str(&value).expect("generated UUID should parse");
        assert_eq!(uuid.get_version(), Some(Version::Random));
        assert!(!uuid.is_nil());
    }
}

#[test]
fn persistent_ids_share_strict_parse_display_and_serde_rules() -> Result<(), Box<dyn Error>> {
    let template = TemplateId::from_str(TEMPLATE_ID)?;
    let field = FieldId::from_str(FIELD_TEXT)?;
    let option = OptionId::from_str(OPTION_ACTIVE)?;
    let document = DocumentId::from_str(DOCUMENT_ID)?;

    assert_eq!(template.to_string(), TEMPLATE_ID);
    assert_eq!(template.as_uuid().to_string(), TEMPLATE_ID);
    assert_eq!(
        serde_json::from_str::<FieldId>(&serde_json::to_string(&field)?)?,
        field
    );
    assert_eq!(
        serde_json::from_str::<OptionId>(&serde_json::to_string(&option)?)?,
        option
    );
    assert_eq!(
        serde_json::from_str::<DocumentId>(&serde_json::to_string(&document)?)?,
        document
    );

    fn assert_invalid_id<T>(value: &str)
    where
        T: DeserializeOwned + FromStr + Ord,
        T::Err: std::fmt::Debug,
    {
        assert!(
            T::from_str(value).is_err(),
            "string parser accepted {value}"
        );
        assert!(
            serde_json::from_value::<T>(json!(value)).is_err(),
            "JSON value parser accepted {value}"
        );
        let map = format!(r#"{{"{value}":"value"}}"#);
        assert!(
            serde_json::from_str::<BTreeMap<T, String>>(&map).is_err(),
            "JSON map-key parser accepted {value}"
        );
    }

    for invalid in [
        "123E4567-E89B-42D3-A456-426614174000",
        &format!("{{{TEMPLATE_ID}}}"),
        &TEMPLATE_ID.replace('-', ""),
        "00000000-0000-0000-0000-000000000000",
        "11111111-1111-1111-8111-111111111111",
        "123e4567-e89b-42d3-0456-426614174000",
        "not-an-id",
    ] {
        assert_invalid_id::<TemplateId>(invalid);
        assert_invalid_id::<FieldId>(invalid);
        assert_invalid_id::<OptionId>(invalid);
        assert_invalid_id::<DocumentId>(invalid);
    }
    Ok(())
}

#[test]
fn persistent_id_types_are_distinct_and_work_as_json_map_keys() -> Result<(), Box<dyn Error>> {
    assert_ne!(TypeId::of::<TemplateId>(), TypeId::of::<FieldId>());
    assert_ne!(TypeId::of::<FieldId>(), TypeId::of::<OptionId>());
    assert_ne!(TypeId::of::<OptionId>(), TypeId::of::<DocumentId>());

    let mut map = BTreeMap::new();
    map.insert(FieldId::from_str(FIELD_TEXT)?, "값".to_owned());
    let encoded = serde_json::to_string(&map)?;
    let decoded: BTreeMap<FieldId, String> = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, map);
    Ok(())
}

#[test]
fn template_revision_is_positive_numeric_and_checked() -> Result<(), Box<dyn Error>> {
    assert_eq!(TemplateRevision::INITIAL.get(), 1);
    assert_eq!(serde_json::to_string(&TemplateRevision::INITIAL)?, "1");
    assert_eq!(
        TemplateRevision::try_from(0),
        Err(TemplateRevisionError::Zero)
    );
    assert_eq!(TemplateRevision::INITIAL.checked_increment()?.get(), 2);
    assert_eq!(
        TemplateRevision::try_from(u32::MAX)?.checked_increment(),
        Err(TemplateRevisionError::Overflow)
    );
    assert!(serde_json::from_str::<TemplateRevision>("0").is_err());
    assert!(serde_json::from_str::<TemplateRevision>("4294967296").is_err());
    assert_ne!(
        TypeId::of::<TemplateRevision>(),
        TypeId::of::<crate::data::schema::SchemaVersion>()
    );
    Ok(())
}

#[test]
fn artifact_schema_registries_admit_template_v7_and_document_v6() -> Result<(), Box<dyn Error>> {
    let template = template_migration_registry()?;
    let document = document_migration_registry()?;

    assert_eq!(TEMPLATE_SCHEMA_VERSION.get(), 7);
    assert_eq!(DOCUMENT_SCHEMA_VERSION.get(), 6);
    assert_eq!(template.current(), TEMPLATE_SCHEMA_VERSION);
    assert_eq!(template.minimum_migratable().get(), 1);
    assert_eq!(document.current(), DOCUMENT_SCHEMA_VERSION);
    assert_eq!(document.minimum_migratable().get(), 1);
    assert_eq!(template_migration_step_count(), 6);
    assert_eq!(document_migration_step_count(), 5);

    assert_eq!(CURRENT_SCHEMA_VERSION.get(), 1);
    assert_eq!(TRANSACTION_SCHEMA_VERSION.get(), 1);
    assert_eq!(PROJECT_LOCK_METADATA_SCHEMA_VERSION.get(), 1);
    Ok(())
}

#[test]
fn header_inspection_and_cross_loaders_enforce_artifact_identity() {
    let template_bytes = deterministic(&template_value());
    let document_bytes = deterministic(&document_value());
    let template_header = inspect_artifact_header(&template_bytes).expect("header should inspect");
    let document_header = inspect_artifact_header(&document_bytes).expect("header should inspect");

    assert_eq!(template_header.artifact_type(), ArtifactType::Template);
    assert_eq!(template_header.schema_version().get(), 1);
    assert_eq!(document_header.artifact_type(), ArtifactType::Document);
    assert_eq!(document_header.schema_version().get(), 1);
    assert_codec_category(
        decode_document(&template_bytes),
        ArtifactCodecErrorCategory::UnexpectedArtifactType,
    );
    assert_codec_category(
        decode_template(&document_bytes),
        ArtifactCodecErrorCategory::UnexpectedArtifactType,
    );
}

#[test]
fn headers_reject_missing_duplicate_wrong_and_future_values() {
    for input in [
        br#"{"schemaVersion":1}"#.as_slice(),
        br#"{"artifactType":7,"schemaVersion":1}"#,
        br#"{"artifactType":"future","schemaVersion":1}"#,
    ] {
        assert!(inspect_artifact_header(input).is_err());
    }
    assert_codec_category(
        inspect_artifact_header(
            br#"{"artifactType":"template","artifactType":"document","schemaVersion":1}"#,
        ),
        ArtifactCodecErrorCategory::DuplicateJsonKey,
    );
    assert_codec_category(
        inspect_artifact_header(
            br#"{"artifactType":"template","schemaVersion":1,"schemaVersion":1}"#,
        ),
        ArtifactCodecErrorCategory::DuplicateJsonKey,
    );
    assert_codec_category(
        decode_template(br#"{"artifactType":"template","schemaVersion":8}"#),
        ArtifactCodecErrorCategory::UnsupportedFuture,
    );
    assert_codec_category(
        decode_template(br#"{"artifactType":"template","schemaVersion":0}"#),
        ArtifactCodecErrorCategory::InvalidSchemaVersion,
    );

    let mut future_with_unknown = template_value();
    future_with_unknown["schemaVersion"] = json!(8);
    future_with_unknown["futureTop"] = json!({"mustNotOpenV2": true});
    assert_codec_category(
        decode_template(&deterministic(&future_with_unknown)),
        ArtifactCodecErrorCategory::UnsupportedFuture,
    );

    let mut future_document = document_value();
    future_document["schemaVersion"] = json!(7);
    future_document["futureTop"] = json!({"mustNotOpenV2": true});
    assert_codec_category(
        decode_document(&deterministic(&future_document)),
        ArtifactCodecErrorCategory::UnsupportedFuture,
    );
}

#[test]
fn active_and_deleted_templates_round_trip_with_order_and_korean() -> Result<(), Box<dyn Error>> {
    let original = template_value();
    let artifact = decode_template(&deterministic(&original))?;

    assert_eq!(artifact.template_id().to_string(), TEMPLATE_ID);
    assert_eq!(artifact.revision().get(), 2);
    assert_eq!(artifact.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(artifact.field_order().len(), 2);
    assert_eq!(artifact.fields().len(), 3);
    assert_eq!(artifact.name(), "등장인물 템플릿");
    assert_eq!(artifact.presentation().token(), Some("character"));
    assert_eq!(artifact.created_at_utc(), "2026-09-03T01:02:03.004Z");
    assert_eq!(artifact.updated_at_utc(), "2026-09-03T02:03:04.005Z");
    let text = artifact
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("text field should exist");
    assert_eq!(text.label(), "이름");
    assert_eq!(text.lifecycle(), FieldLifecycle::Active);
    assert_eq!(text.kind(), FieldKind::SingleLineText);
    assert!(!text.required());
    assert_eq!(text.introduced_revision().get(), 1);
    assert_eq!(text.default_value().text(), Some("현재 기본값"));
    assert!(text.initial_default_value().is_unset());
    assert_eq!(text.configuration().kind(), FieldKind::SingleLineText);
    assert_eq!(text.presentation().token(), None);

    let choice = artifact
        .fields()
        .get(&FieldId::from_str(FIELD_CHOICE)?)
        .expect("choice field should exist");
    let options = choice
        .configuration()
        .options()
        .expect("choice field should keep its choice configuration");
    let active_option = options
        .get(&OptionId::from_str(OPTION_ACTIVE)?)
        .expect("active option should exist");
    assert_eq!(active_option.label(), "주인공");
    assert_eq!(active_option.lifecycle(), OptionLifecycle::Active);
    let archived_option = options
        .get(&OptionId::from_str(OPTION_ARCHIVED)?)
        .expect("archived option should exist");
    assert_eq!(archived_option.label(), "예전 분류");
    assert_eq!(archived_option.lifecycle(), OptionLifecycle::Archived);
    assert_eq!(decode_template(&encode_template(&artifact)?)?, artifact);

    let mut deleted = original;
    deleted["lifecycle"] = json!("deleted");
    let deleted = decode_template(&deterministic(&deleted))?;
    assert_eq!(deleted.lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(deleted.fields().len(), 3);
    assert_eq!(decode_template(&encode_template(&deleted)?)?, deleted);
    Ok(())
}

#[test]
fn template_rejects_field_and_option_order_or_identity_corruption() {
    const UNKNOWN_FIELD: &str = "99999999-9999-4999-8999-999999999999";
    const UNKNOWN_OPTION: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    let mut duplicate_field_order = template_value();
    duplicate_field_order["fieldOrder"] = json!([FIELD_TEXT, FIELD_TEXT, FIELD_CHOICE]);
    assert_codec_category(
        decode_template(&deterministic(&duplicate_field_order)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut missing_active = template_value();
    missing_active["fieldOrder"] = json!([FIELD_TEXT]);
    assert_codec_category(
        decode_template(&deterministic(&missing_active)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut archived_in_order = template_value();
    archived_in_order["fieldOrder"] = json!([FIELD_TEXT, FIELD_CHOICE, FIELD_ARCHIVED]);
    assert_codec_category(
        decode_template(&deterministic(&archived_in_order)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut unknown_in_field_order = template_value();
    unknown_in_field_order["fieldOrder"] = json!([FIELD_TEXT, FIELD_CHOICE, UNKNOWN_FIELD]);
    assert_codec_category(
        decode_template(&deterministic(&unknown_in_field_order)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut duplicate_option_order = template_value();
    duplicate_option_order["fields"][FIELD_CHOICE]["configuration"]["optionOrder"] =
        json!([OPTION_ACTIVE, OPTION_ACTIVE]);
    assert_codec_category(
        decode_template(&deterministic(&duplicate_option_order)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    for option_order in [
        json!([]),
        json!([OPTION_ACTIVE, OPTION_ARCHIVED]),
        json!([OPTION_ACTIVE, UNKNOWN_OPTION]),
    ] {
        let mut invalid = template_value();
        invalid["fields"][FIELD_CHOICE]["configuration"]["optionOrder"] = option_order;
        assert_codec_category(
            decode_template(&deterministic(&invalid)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }

    let mut duplicate_global_option = template_value();
    duplicate_global_option["fields"][FIELD_ARCHIVED]["kind"] = json!("singleChoice");
    duplicate_global_option["fields"][FIELD_ARCHIVED]["configuration"] = json!({
        "kind": "singleChoice",
        "optionOrder": [OPTION_ACTIVE],
        "options": {
            (OPTION_ACTIVE): { "label": "충돌", "lifecycle": "active" }
        }
    });
    assert_codec_category(
        decode_template(&deterministic(&duplicate_global_option)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );
}

#[test]
fn template_rejects_invalid_introduced_revision_and_requires_both_defaults() {
    let mut zero = template_value();
    zero["fields"][FIELD_TEXT]["introducedRevision"] = json!(0);
    assert_codec_category(
        decode_template(&deterministic(&zero)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut future = template_value();
    future["fields"][FIELD_TEXT]["introducedRevision"] = json!(3);
    let error = decode_template(&deterministic(&future))
        .expect_err("future introduced revision should be rejected");
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::InvalidStructure
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::IntroducedRevisionOutOfRange)
    );

    for key in ["defaultValue", "initialDefaultValue"] {
        let mut missing = template_value();
        missing["fields"][FIELD_TEXT]
            .as_object_mut()
            .expect("field fixture should be an object")
            .remove(key);
        assert_codec_category(
            decode_template(&deterministic(&missing)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }
}

#[test]
fn document_round_trips_identity_orphans_and_all_tagged_value_shapes() -> Result<(), Box<dyn Error>>
{
    let mut document = document_value();
    let values = document["fieldValues"]
        .as_object_mut()
        .expect("fieldValues fixture should be an object");
    let cases = [
        (
            "80000000-0000-4000-8000-000000000001",
            json!({"kind":"unset"}),
        ),
        (
            "80000000-0000-4000-8000-000000000002",
            json!({"kind":"text","value":"한글"}),
        ),
        (
            "80000000-0000-4000-8000-000000000003",
            rich_text_value("한글 rich-text"),
        ),
        (
            "80000000-0000-4000-8000-000000000004",
            json!({"kind":"number","value":"100"}),
        ),
        (
            "80000000-0000-4000-8000-000000000005",
            json!({"kind":"date","value":"2026-09-04"}),
        ),
        (
            "80000000-0000-4000-8000-000000000006",
            json!({"kind":"time","value":"23:59:59.999"}),
        ),
        (
            "80000000-0000-4000-8000-000000000007",
            json!({"kind":"duration","milliseconds":"-1000"}),
        ),
        (
            "80000000-0000-4000-8000-000000000008",
            json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
        ),
        (
            "80000000-0000-4000-8000-000000000009",
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]}),
        ),
    ];
    for (id, value) in cases {
        values.insert(id.to_owned(), value);
    }

    let artifact = decode_document(&deterministic(&document))?;
    assert_eq!(artifact.document_id().to_string(), DOCUMENT_ID);
    assert_eq!(artifact.template_id().to_string(), TEMPLATE_ID);
    assert_eq!(artifact.template_revision().get(), 2);
    assert_eq!(artifact.name(), "아리아");
    assert_eq!(artifact.field_values().len(), 13);
    assert_eq!(artifact.created_at_utc(), "2026-09-03T01:02:03.004Z");
    assert_eq!(artifact.updated_at_utc(), "2026-09-03T02:03:04.005Z");
    let orphan = artifact
        .orphaned_field_definitions()
        .get(&FieldId::from_str(FIELD_ARCHIVED)?)
        .expect("orphan snapshot should exist");
    assert_eq!(orphan.label(), "과거 기록");
    assert_eq!(orphan.kind(), FieldKind::RichText);
    assert!(orphan.options().is_empty());
    let orphan_choice = artifact
        .orphaned_field_definitions()
        .get(&FieldId::from_str(FIELD_ORPHAN_CHOICE)?)
        .expect("orphan choice snapshot should exist");
    let orphan_option = orphan_choice
        .options()
        .get(&OptionId::from_str(OPTION_ARCHIVED)?)
        .expect("orphan option snapshot should exist");
    assert_eq!(orphan_option.label(), "삭제된 선택지");
    assert_eq!(decode_document(&encode_document(&artifact)?)?, artifact);
    Ok(())
}

#[test]
fn document_values_require_explicit_known_tags_and_preserve_orphan_ids() {
    for invalid_value in [
        Value::Null,
        json!(""),
        json!({"value":"missing kind"}),
        json!({"kind":"futureValue","value":"x"}),
    ] {
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] = invalid_value;
        assert_codec_category(
            decode_document(&deterministic(&document)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }

    let artifact = decode_document(&deterministic(&document_value())).expect("document is valid");
    assert!(artifact
        .field_values()
        .contains_key(&FieldId::from_str(FIELD_CHOICE).expect("fixture ID should parse")));
}

#[test]
fn field_value_variants_reject_every_foreign_v1_carrier_member() {
    let carriers = [
        ("value", json!("foreign")),
        (
            "document",
            json!({"schemaVersion":1,"content":rich_text_content("carrier")}),
        ),
        ("milliseconds", json!("1000")),
        ("optionId", json!(OPTION_ACTIVE)),
        ("optionIds", json!([OPTION_ACTIVE])),
    ];
    let variants: [(&str, Value, &[&str]); 9] = [
        ("unset", json!({"kind":"unset"}), &[]),
        ("text", json!({"kind":"text","value":"text"}), &["value"]),
        ("richText", rich_text_value("value"), &["document"]),
        ("number", json!({"kind":"number","value":"1"}), &["value"]),
        (
            "date",
            json!({"kind":"date","value":"2026-09-03"}),
            &["value"],
        ),
        (
            "time",
            json!({"kind":"time","value":"01:02:03.004"}),
            &["value"],
        ),
        (
            "duration",
            json!({"kind":"duration","milliseconds":"1000"}),
            &["milliseconds"],
        ),
        (
            "singleChoice",
            json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
            &["optionId"],
        ),
        (
            "multiChoice",
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
            &["optionIds"],
        ),
    ];

    for (_variant, base, owned) in variants {
        for (key, value) in &carriers {
            if owned.contains(key) {
                continue;
            }
            let mut invalid_value = base.clone();
            invalid_value
                .as_object_mut()
                .expect("value fixture is an object")
                .insert((*key).to_owned(), value.clone());
            let mut document = document_value();
            document["fieldValues"][FIELD_TEXT] = invalid_value;
            assert_codec_category(
                decode_document(&deterministic(&document)),
                ArtifactCodecErrorCategory::InvalidStructure,
            );
        }
    }
}

#[test]
fn field_configuration_variants_reserve_all_v1_choice_members() {
    let configurations = [
        (
            "singleLineText",
            basic_configuration("singleLineText"),
            false,
        ),
        ("richText", basic_configuration("richText"), false),
        ("number", basic_configuration("number"), false),
        ("date", basic_configuration("date"), false),
        ("time", basic_configuration("time"), false),
        ("duration", basic_configuration("duration"), false),
        (
            "singleChoice",
            json!({"kind":"singleChoice","optionOrder":[],"options":{}}),
            true,
        ),
        (
            "multiChoice",
            json!({"kind":"multiChoice","optionOrder":[],"options":{}}),
            true,
        ),
    ];
    for (kind, configuration, owns_choice_members) in configurations {
        let mut valid = minimal_template_value();
        valid["fields"][FIELD_TEXT]["kind"] = json!(kind);
        valid["fields"][FIELD_TEXT]["configuration"] = configuration;
        assert!(decode_template(&deterministic(&valid)).is_ok(), "{kind}");
        if owns_choice_members {
            continue;
        }
        for (key, value) in [("optionOrder", json!([])), ("options", json!({}))] {
            let mut template = valid.clone();
            template["fields"][FIELD_TEXT]["configuration"]
                .as_object_mut()
                .expect("configuration fixture is an object")
                .insert(key.to_owned(), value);
            assert_codec_category(
                decode_template(&deterministic(&template)),
                ArtifactCodecErrorCategory::InvalidStructure,
            );
        }
    }
}

#[test]
fn document_enforces_orphan_snapshot_relationships_without_template_context() {
    let mut missing_value = document_value();
    missing_value["fieldValues"]
        .as_object_mut()
        .expect("fieldValues fixture is an object")
        .remove(FIELD_ARCHIVED);
    let error = decode_document(&deterministic(&missing_value))
        .expect_err("orphan snapshot without value must fail");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanMissingValue)
    );

    let mut wrong_kind = document_value();
    wrong_kind["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"text","value":"wrong"});
    let error = decode_document(&deterministic(&wrong_kind))
        .expect_err("orphan value kind mismatch must fail");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanValueKindMismatch)
    );

    let mut non_choice_options = document_value();
    non_choice_options["orphanedFieldDefinitions"][FIELD_ARCHIVED]["options"][OPTION_ACTIVE] =
        json!({"label":"unrelated"});
    assert_codec_category(
        decode_document(&deterministic(&non_choice_options)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    for selected in [
        json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
        json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED]}),
    ] {
        let mut invalid = document_value();
        invalid["fieldValues"][FIELD_ORPHAN_CHOICE] = selected;
        assert_codec_category(
            decode_document(&deterministic(&invalid)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }

    let mut unrelated_snapshot = document_value();
    unrelated_snapshot["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ACTIVE] =
        json!({"label":"not selected"});
    assert_codec_category(
        decode_document(&deterministic(&unrelated_snapshot)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );

    let mut valid_multi = document_value();
    valid_multi["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["kind"] = json!("multiChoice");
    valid_multi["fieldValues"][FIELD_ORPHAN_CHOICE] =
        json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED]});
    assert!(decode_document(&deterministic(&valid_multi)).is_ok());

    const SECOND_ORPHAN: &str = "99999999-9999-4999-8999-999999999999";
    let mut duplicate_option_identity = document_value();
    duplicate_option_identity["fieldValues"][SECOND_ORPHAN] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    duplicate_option_identity["orphanedFieldDefinitions"][SECOND_ORPHAN] = json!({
        "kind":"singleChoice",
        "label":"second",
        "options":{(OPTION_ARCHIVED):{"label":"duplicate identity"}}
    });
    let error = decode_document(&deterministic(&duplicate_option_identity))
        .expect_err("orphan OptionId must be globally unique in the artifact");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::DuplicateOptionId)
    );

    let mut historical_absence = document_value();
    historical_absence["fieldValues"]
        .as_object_mut()
        .expect("fieldValues fixture is an object")
        .remove(FIELD_ARCHIVED);
    historical_absence["orphanedFieldDefinitions"]
        .as_object_mut()
        .expect("orphan fixture is an object")
        .remove(FIELD_ARCHIVED);
    let artifact = decode_document(&deterministic(&historical_absence))
        .expect("absence from both maps remains distinct from explicit unset");
    assert!(!artifact
        .field_values()
        .contains_key(&FieldId::from_str(FIELD_ARCHIVED).expect("fixture ID should parse")));
}

#[test]
fn unknown_fields_survive_at_every_v1_extension_boundary() -> Result<(), Box<dyn Error>> {
    let mut template = template_value();
    template["futureTop"] = json!({"nested":[{"z":2,"a":1}]});
    template["presentation"]["futurePresentation"] = json!({"array":[3, 1, 2]});
    template["fields"][FIELD_TEXT]["futureField"] = json!({"keep":true});
    template["fields"][FIELD_TEXT]["presentation"]["futureFieldPresentation"] =
        json!({"keep":"field"});
    template["fields"][FIELD_TEXT]["configuration"]["futureConfiguration"] = json!([3, 2, 1]);
    template["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]["futureOption"] =
        json!({"한글":"유지"});
    template["fields"][FIELD_TEXT]["defaultValue"]["futureValueMember"] = json!({"x":1});
    template["fields"][FIELD_TEXT]["initialDefaultValue"]["futureInitialDefault"] =
        json!({"keep":true});

    let artifact = decode_template(&deterministic(&template))?;
    assert_eq!(artifact.presentation().token(), Some("character"));
    assert_eq!(
        artifact
            .fields()
            .get(&FieldId::from_str(FIELD_TEXT)?)
            .expect("text field exists")
            .label(),
        "이름"
    );
    let encoded = encode_template(&artifact)?;
    let round_trip: Value = serde_json::from_slice(&encoded)?;
    assert_eq!(round_trip["futureTop"], template["futureTop"]);
    assert_eq!(round_trip["presentation"], template["presentation"]);
    assert_eq!(
        round_trip["fields"][FIELD_TEXT]["futureField"],
        template["fields"][FIELD_TEXT]["futureField"]
    );
    assert_eq!(
        round_trip["fields"][FIELD_TEXT]["presentation"],
        template["fields"][FIELD_TEXT]["presentation"]
    );
    assert_eq!(
        round_trip["fields"][FIELD_TEXT]["configuration"]["futureConfiguration"],
        template["fields"][FIELD_TEXT]["configuration"]["futureConfiguration"]
    );
    assert_eq!(
        round_trip["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]
            ["futureOption"],
        template["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]["futureOption"]
    );
    assert_eq!(
        round_trip["fields"][FIELD_TEXT]["defaultValue"]["futureValueMember"],
        template["fields"][FIELD_TEXT]["defaultValue"]["futureValueMember"]
    );
    assert_eq!(
        round_trip["fields"][FIELD_TEXT]["initialDefaultValue"]["futureInitialDefault"],
        template["fields"][FIELD_TEXT]["initialDefaultValue"]["futureInitialDefault"]
    );
    assert_eq!(encode_template(&decode_template(&encoded)?)?, encoded);

    let mut document = document_value();
    document["futureDocument"] = json!({"array":[1,{"b":2,"a":1}]});
    document["orphanedFieldDefinitions"][FIELD_ARCHIVED]["futureOrphanField"] =
        json!({"keep":"field snapshot"});
    document["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]
        ["futureOrphanOption"] = json!({"keep":"option snapshot"});
    document["fieldValues"][FIELD_TEXT]["futureValue"] = json!({"keep":"yes"});
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document": {
            "schemaVersion": 1,
            "content": {
                "kind":"root",
                "children":[{"kind":"paragraph","children":[{"kind":"text","text":"본문"}]}],
                "futureNested":{"b":2,"a":1}
            },
            "futureEnvelope": ["유지"]
        },
        "futureValue": true
    });
    let encoded = encode_document(&decode_document(&deterministic(&document))?)?;
    let round_trip: Value = serde_json::from_slice(&encoded)?;
    assert_eq!(round_trip["futureDocument"], document["futureDocument"]);
    assert_eq!(
        round_trip["fieldValues"][FIELD_TEXT],
        document["fieldValues"][FIELD_TEXT]
    );
    assert_eq!(
        round_trip["orphanedFieldDefinitions"][FIELD_ARCHIVED]["futureOrphanField"],
        document["orphanedFieldDefinitions"][FIELD_ARCHIVED]["futureOrphanField"]
    );
    assert_eq!(
        round_trip["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]
            ["futureOrphanOption"],
        document["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]
            ["futureOrphanOption"]
    );
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    Ok(())
}

#[test]
fn unknown_fields_survive_every_configuration_and_value_variant() -> Result<(), Box<dyn Error>> {
    let configurations = [
        ("singleLineText", basic_configuration("singleLineText")),
        ("richText", basic_configuration("richText")),
        ("number", basic_configuration("number")),
        ("date", basic_configuration("date")),
        ("time", basic_configuration("time")),
        ("duration", basic_configuration("duration")),
        (
            "singleChoice",
            json!({"kind":"singleChoice","optionOrder":[],"options":{}}),
        ),
        (
            "multiChoice",
            json!({"kind":"multiChoice","optionOrder":[],"options":{}}),
        ),
    ];
    for (kind, mut configuration) in configurations {
        configuration["futureConfiguration"] = json!({"nested":[3, 1, 2]});
        let mut template = minimal_template_value();
        template["fields"][FIELD_TEXT]["kind"] = json!(kind);
        template["fields"][FIELD_TEXT]["configuration"] = configuration.clone();
        let artifact = decode_template(&deterministic(&template))?;
        let field = artifact
            .fields()
            .get(&FieldId::from_str(FIELD_TEXT)?)
            .expect("field exists");
        assert_eq!(field.configuration().kind(), field.kind());
        let encoded = encode_template(&artifact)?;
        let round_trip: Value = serde_json::from_slice(&encoded)?;
        assert_eq!(
            round_trip["fields"][FIELD_TEXT]["configuration"]["futureConfiguration"],
            configuration["futureConfiguration"]
        );
        assert_eq!(encode_template(&decode_template(&encoded)?)?, encoded);
    }

    let values = [
        json!({"kind":"unset"}),
        json!({"kind":"text","value":"text"}),
        json!({
            "kind":"richText",
            "document":{
                "schemaVersion":1,
                "content":{
                    "kind":"root",
                    "children":[{"kind":"paragraph","children":[{"kind":"text","text":"본문"}]}],
                    "nodes":[3,1,2]
                }
            }
        }),
        json!({"kind":"number","value":"12345678901234567890.01"}),
        json!({"kind":"date","value":"2026-09-04"}),
        json!({"kind":"time","value":"09:30:00.000"}),
        json!({"kind":"duration","milliseconds":"-1000"}),
        json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
        json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]}),
    ];
    for mut value in values {
        value["futureValue"] = json!({"nested":[{"z":2,"a":1}, 3, 1]});
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] = value.clone();
        let artifact = decode_document(&deterministic(&document))?;
        assert!(artifact
            .field_values()
            .contains_key(&FieldId::from_str(FIELD_TEXT)?));
        let encoded = encode_document(&artifact)?;
        let round_trip: Value = serde_json::from_slice(&encoded)?;
        assert_eq!(round_trip["fieldValues"][FIELD_TEXT], value);
        assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    }
    Ok(())
}

#[test]
fn unknown_arbitrary_precision_numbers_preserve_their_lexemes_at_every_depth(
) -> Result<(), Box<dyn Error>> {
    let numbers = [
        "0.123456789012345678901234567890",
        "1234567890123456789012345678901234567890",
        "1.234567890123456789e+100",
        "-0.000000000000000000000000000000000000000123456789",
    ];
    let raw = format!(
        r#"{{"artifactType":"document","createdAtUtc":"2026-09-03T01:02:03.004Z","documentId":"{DOCUMENT_ID}","fieldValues":{{}},"futureNumbers":{{"decimal":{},"integer":{},"exponent":{},"nested":{{"array":[{},{},{{"small":{}}}]}}}},"name":"precision","orphanedFieldDefinitions":{{}},"schemaVersion":1,"templateId":"{TEMPLATE_ID}","templateRevision":1,"updatedAtUtc":"2026-09-03T01:02:03.004Z"}}"#,
        numbers[0], numbers[1], numbers[2], numbers[2], numbers[0], numbers[3]
    );

    let first = encode_document(&decode_document(raw.as_bytes())?)?;
    let text = std::str::from_utf8(&first)?;
    for number in numbers {
        assert!(
            text.contains(number),
            "number lexeme changed: {number}; output: {text}"
        );
    }
    let decoded = decode_document(&first)?;
    assert_eq!(decoded.name(), "precision");
    let second = encode_document(&decoded)?;
    assert_eq!(second, first);
    Ok(())
}

#[test]
fn unknown_number_lexemes_round_trip_at_every_artifact_location() -> Result<(), Box<dyn Error>> {
    const UPPER: &str = "__NUMBER_UPPER__";
    const LOWER: &str = "__NUMBER_LOWER__";
    const PLUS: &str = "__NUMBER_PLUS__";
    const NEGATIVE_ZERO: &str = "__NUMBER_NEGATIVE_ZERO__";
    const INTEGER: &str = "__NUMBER_INTEGER__";
    const DECIMAL: &str = "__NUMBER_DECIMAL__";
    const NESTED_OBJECT: &str = "__NUMBER_NESTED_OBJECT__";
    const NESTED_ARRAY: &str = "__NUMBER_NESTED_ARRAY__";
    const ORPHAN_OPTION: &str = "__NUMBER_ORPHAN_OPTION__";
    const NODE: &str = "__NUMBER_NODE__";
    const TOKENS: &[(&str, &str)] = &[
        (UPPER, "1E100"),
        (LOWER, "1e100"),
        (PLUS, "1e+100"),
        (NEGATIVE_ZERO, "-0"),
        (
            INTEGER,
            "12345678901234567890123456789012345678901234567890",
        ),
        (
            DECIMAL,
            "0.12345678901234567890123456789012345678901234567890",
        ),
        (
            NESTED_OBJECT,
            "9.87654321098765432109876543210987654321e-210",
        ),
        (NESTED_ARRAY, "7E-100"),
        (ORPHAN_OPTION, "6e100"),
        (NODE, "5e+100"),
    ];

    let mut template = template_with_rich_text_defaults();
    template["lexTemplateRoot"] = json!(UPPER);
    template["fields"][FIELD_TEXT]["lexFieldDefinition"] = json!(LOWER);
    template["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]
        ["lexChoiceOption"] = json!(PLUS);
    template["fields"][FIELD_CHOICE]["configuration"]["lexConfiguration"] = json!(NEGATIVE_ZERO);
    template["fields"][FIELD_TEXT]["defaultValue"]["lexFieldValue"] = json!(INTEGER);
    template["fields"][FIELD_ARCHIVED]["defaultValue"]["document"]["content"]["lexRichText"] =
        json!(DECIMAL);
    template["lexNested"] = json!({
        "zKey": {"lexNestedObject": NESTED_OBJECT},
        "aKey": [{"lexNestedArray": NESTED_ARRAY}]
    });
    let template_raw = replace_number_markers(&template, &TOKENS[..8]);
    let template_source = template_raw.clone();
    let template_artifact = decode_template(&template_raw)?;
    template_artifact.validate_storage()?;
    let template_first = encode_template(&template_artifact)?;
    let template_text = std::str::from_utf8(&template_first)?;
    for (key, lexeme) in [
        ("lexTemplateRoot", "1E100"),
        ("lexFieldDefinition", "1e100"),
        ("lexChoiceOption", "1e+100"),
        ("lexConfiguration", "-0"),
        (
            "lexFieldValue",
            "12345678901234567890123456789012345678901234567890",
        ),
        (
            "lexRichText",
            "0.12345678901234567890123456789012345678901234567890",
        ),
        (
            "lexNestedObject",
            "9.87654321098765432109876543210987654321e-210",
        ),
        ("lexNestedArray", "7E-100"),
    ] {
        assert_number_lexeme(template_text, key, lexeme);
    }
    assert!(template_text.find("\"aKey\"") < template_text.find("\"zKey\""));
    assert_eq!(template_raw, template_source);
    let template_second = encode_template(&decode_template(&template_first)?)?;
    assert_eq!(template_second, template_first);

    let mut document = document_with_rich_text_content(json!({
        "lexNestedObject": NESTED_OBJECT,
        "array": [{"lexNestedArray": NESTED_ARRAY}]
    }));
    document["lexDocumentRoot"] = json!(UPPER);
    document["lexLongInteger"] = json!(INTEGER);
    document["fieldValues"][FIELD_TEXT]["lexFieldValue"] = json!(LOWER);
    document["fieldValues"][FIELD_ARCHIVED]["document"]["lexEnvelope"] = json!(PLUS);
    document["fieldValues"][FIELD_ARCHIVED]["document"]["content"]["lexRichText"] =
        json!(NEGATIVE_ZERO);
    document["fieldValues"][FIELD_ARCHIVED]["document"]["content"]["children"][0]["children"][0]
        ["lexNode"] = json!(NODE);
    document["orphanedFieldDefinitions"][FIELD_ARCHIVED]["lexOrphanDefinition"] = json!(DECIMAL);
    document["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]
        ["lexOrphanOption"] = json!(ORPHAN_OPTION);
    document["lexEquivalentArray"] = json!([
        {"arrayA": UPPER},
        {"arrayB": LOWER},
        {"arrayC": PLUS},
        {"arrayD": NEGATIVE_ZERO}
    ]);

    let document_raw = replace_number_markers(&document, TOKENS);
    let document_source = document_raw.clone();
    let document_artifact = decode_document(&document_raw)?;
    document_artifact.validate_storage()?;
    let document_first = encode_document(&document_artifact)?;
    let document_text = std::str::from_utf8(&document_first)?;
    for (key, lexeme) in [
        ("lexDocumentRoot", "1E100"),
        (
            "lexLongInteger",
            "12345678901234567890123456789012345678901234567890",
        ),
        ("lexFieldValue", "1e100"),
        ("lexEnvelope", "1e+100"),
        ("lexRichText", "-0"),
        (
            "lexOrphanDefinition",
            "0.12345678901234567890123456789012345678901234567890",
        ),
        ("lexOrphanOption", "6e100"),
        ("lexNode", "5e+100"),
        ("lexNestedArray", "7E-100"),
    ] {
        assert_number_lexeme(document_text, key, lexeme);
    }
    let ordered = ["\"arrayA\"", "\"arrayB\"", "\"arrayC\"", "\"arrayD\""]
        .map(|key| document_text.find(key).expect("array item key must remain"));
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(document_raw, document_source);
    let document_second = encode_document(&decode_document(&document_first)?)?;
    assert_eq!(document_second, document_first);
    Ok(())
}

#[test]
fn known_integer_members_reject_noncanonical_json_number_spellings() {
    const MARKER: &str = "__KNOWN_INTEGER__";
    for lexeme in ["1e0", "1.0", "-0"] {
        let mut template_schema = template_value();
        template_schema["schemaVersion"] = json!(MARKER);
        assert!(decode_template(&replace_number_markers(
            &template_schema,
            &[(MARKER, lexeme)],
        ))
        .is_err());

        let mut template_revision = template_value();
        template_revision["revision"] = json!(MARKER);
        assert!(decode_template(&replace_number_markers(
            &template_revision,
            &[(MARKER, lexeme)],
        ))
        .is_err());

        let mut introduced_revision = template_value();
        introduced_revision["fields"][FIELD_TEXT]["introducedRevision"] = json!(MARKER);
        assert!(decode_template(&replace_number_markers(
            &introduced_revision,
            &[(MARKER, lexeme)],
        ))
        .is_err());

        let mut document_schema = document_value();
        document_schema["schemaVersion"] = json!(MARKER);
        assert!(decode_document(&replace_number_markers(
            &document_schema,
            &[(MARKER, lexeme)],
        ))
        .is_err());

        let mut document_revision = document_value();
        document_revision["templateRevision"] = json!(MARKER);
        assert!(decode_document(&replace_number_markers(
            &document_revision,
            &[(MARKER, lexeme)],
        ))
        .is_err());

        let mut rich_schema = template_with_rich_text_defaults();
        rich_schema["fields"][FIELD_ARCHIVED]["defaultValue"]["document"]["schemaVersion"] =
            json!(MARKER);
        assert!(
            decode_template(&replace_number_markers(&rich_schema, &[(MARKER, lexeme)],)).is_err()
        );

        let mut heading_level = template_with_rich_text_defaults();
        heading_level["fields"][FIELD_ARCHIVED]["defaultValue"]["document"]["content"]
            ["children"][0]["kind"] = json!("heading");
        heading_level["fields"][FIELD_ARCHIVED]["defaultValue"]["document"]["content"]
            ["children"][0]["level"] = json!(MARKER);
        assert!(
            decode_template(&replace_number_markers(&heading_level, &[(MARKER, lexeme)],)).is_err()
        );
    }
}

#[test]
fn reserved_marker_objects_fail_at_shared_artifact_boundaries() -> Result<(), Box<dyn Error>> {
    for reserved_key in [
        "$serde_json::private::Number",
        "$serde_json::private::RawValue",
        "$serde_json::private::FutureTransport",
    ] {
        let marker = reserved_marker_object(reserved_key);

        let mut template_cases = Vec::new();
        let mut top = template_value();
        top["futureTop"] = marker.clone();
        template_cases.push(top);

        let mut field = template_value();
        field["fields"][FIELD_TEXT]["futureField"] = marker.clone();
        template_cases.push(field);

        let mut configuration = template_value();
        configuration["fields"][FIELD_TEXT]["configuration"]["futureConfiguration"] =
            marker.clone();
        template_cases.push(configuration);

        let mut value = template_value();
        value["fields"][FIELD_TEXT]["defaultValue"]["futureValue"] = marker.clone();
        template_cases.push(value);

        let mut presentation = template_value();
        presentation["presentation"]["futurePresentation"] = marker.clone();
        template_cases.push(presentation);

        let mut option = template_value();
        option["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]["futureOption"] =
            marker.clone();
        template_cases.push(option);

        for template in template_cases {
            let raw = serde_json::to_vec(&template)?;
            assert_codec_category(
                decode_template(&raw),
                ArtifactCodecErrorCategory::InvalidStructure,
            );
        }

        let mut document_cases = Vec::new();
        let mut top = document_value();
        top["futureDocument"] = marker.clone();
        document_cases.push(top);

        let mut value = document_value();
        value["fieldValues"][FIELD_TEXT]["futureValue"] = marker.clone();
        document_cases.push(value);

        let mut orphan = document_value();
        orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["futureOrphan"] = marker.clone();
        document_cases.push(orphan);

        let mut orphan_option = document_value();
        orphan_option["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"]
            [OPTION_ARCHIVED]["futureOrphanOption"] = marker.clone();
        document_cases.push(orphan_option);

        let mut rich_text = document_value();
        rich_text["fieldValues"][FIELD_TEXT] = json!({
            "kind": "richText",
            "document": {
                "schemaVersion": 1,
                "content": {
                    "kind":"root",
                    "children":[{"kind":"paragraph","children":[{"kind":"text","text":"본문"}]}],
                    "futureNode": marker.clone()
                }
            }
        });
        document_cases.push(rich_text);

        let mut array = document_value();
        array["futureArray"] = Value::Array(vec![Value::from(1), marker]);
        document_cases.push(array);

        for document in document_cases {
            let raw = serde_json::to_vec(&document)?;
            assert_codec_category(
                decode_document(&raw),
                ArtifactCodecErrorCategory::InvalidStructure,
            );
        }
    }

    let compact = serde_json::to_string(&document_value())?;
    for escaped_key in [
        r#"\u0024serde_json::private::\u004eumber"#,
        r#"\u0024serde_json::private::\u0052awValue"#,
    ] {
        let escaped = compact.replacen(
            r#""name":"아리아""#,
            &format!(r#""name":"아리아","futureEscaped":{{"{escaped_key}":"1"}}"#),
            1,
        );
        assert_ne!(escaped, compact, "escaped fixture injection must succeed");
        assert_codec_category(
            decode_document(escaped.as_bytes()),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }

    let mut allowed = document_value();
    allowed["markerAsString"] = Value::String("$serde_json::private::Number".to_owned());
    allowed["similarKey"] = json!({"$serde_json::private":"1","ordinaryFuture":"2"});
    let first = encode_document(&decode_document(&deterministic(&allowed))?)?;
    assert_eq!(encode_document(&decode_document(&first)?)?, first);
    Ok(())
}

fn document_with_array_depth(container_depth: usize) -> Vec<u8> {
    assert!(container_depth >= 1);
    let mut json = serde_json::to_string(&document_value()).expect("fixture should serialize");
    assert_eq!(json.pop(), Some('}'));
    json.push_str(r#","futureDepth":"#);
    for _ in 1..container_depth {
        json.push('[');
    }
    json.push('0');
    for _ in 1..container_depth {
        json.push(']');
    }
    json.push('}');
    json.into_bytes()
}

#[test]
fn artifact_maps_global_depth_overflow_to_invalid_structure() {
    let boundary = document_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH);
    decode_document(&boundary).expect("documented depth boundary should remain decodable");

    let too_deep = document_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH + 1);
    assert_codec_category(
        decode_document(&too_deep),
        ArtifactCodecErrorCategory::InvalidStructure,
    );
}

#[test]
fn strict_json_rejects_duplicates_at_top_headers_and_every_nested_object() {
    let template_inputs: &[&[u8]] = &[
        br#"{"artifactType":"template","schemaVersion":1,"x":1,"x":2}"#,
        br#"{"artifactType":"template","schemaVersion":1,"fields":{"22222222-2222-4222-8222-222222222222":{},"22222222-2222-4222-8222-222222222222":{}}}"#,
        br#"{"artifactType":"template","schemaVersion":1,"fields":{"22222222-2222-4222-8222-222222222222":{"label":"a","label":"b"}}}"#,
        br#"{"artifactType":"template","schemaVersion":1,"fields":{"33333333-3333-4333-8333-333333333333":{"configuration":{"options":{"44444444-4444-4444-8444-444444444444":{},"44444444-4444-4444-8444-444444444444":{}}}}}}"#,
        br#"{"artifactType":"template","schemaVersion":1,"fields":{"33333333-3333-4333-8333-333333333333":{"configuration":{"options":{"44444444-4444-4444-8444-444444444444":{"label":"a","label":"b"}}}}}}"#,
        br#"{"artifactType":"template","schemaVersion":1,"future":{"same":1,"same":2}}"#,
    ];
    for input in template_inputs {
        assert_codec_category(
            decode_template(input),
            ArtifactCodecErrorCategory::DuplicateJsonKey,
        );
    }

    let document_inputs: &[&[u8]] = &[
        br#"{"artifactType":"document","schemaVersion":1,"fieldValues":{"22222222-2222-4222-8222-222222222222":{"kind":"text","value":"a","value":"b"}}}"#,
        br#"{"artifactType":"document","schemaVersion":1,"fieldValues":{"22222222-2222-4222-8222-222222222222":{"kind":"unset"},"22222222-2222-4222-8222-222222222222":{"kind":"unset"}}}"#,
    ];
    for input in document_inputs {
        assert_codec_category(
            decode_document(input),
            ArtifactCodecErrorCategory::DuplicateJsonKey,
        );
    }
}

#[test]
fn every_field_kind_requires_matching_configuration_and_default_variants() {
    let cases = [
        (
            "singleLineText",
            basic_configuration("singleLineText"),
            json!({"kind":"text","value":"value"}),
        ),
        (
            "richText",
            basic_configuration("richText"),
            rich_text_value("default"),
        ),
        (
            "number",
            basic_configuration("number"),
            json!({"kind":"number","value":"1"}),
        ),
        (
            "date",
            basic_configuration("date"),
            json!({"kind":"date","value":"2026-09-03"}),
        ),
        (
            "time",
            basic_configuration("time"),
            json!({"kind":"time","value":"01:02:03.004"}),
        ),
        (
            "duration",
            basic_configuration("duration"),
            json!({"kind":"duration","milliseconds":"1000"}),
        ),
        (
            "singleChoice",
            choice_configuration("singleChoice"),
            json!({"kind":"singleChoice","optionId":OPTION_ACTIVE}),
        ),
        (
            "multiChoice",
            choice_configuration("multiChoice"),
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]}),
        ),
    ];

    for (index, (kind, configuration, value)) in cases.iter().enumerate() {
        let mut valid = minimal_template_value();
        valid["fields"][FIELD_TEXT]["kind"] = json!(kind);
        valid["fields"][FIELD_TEXT]["configuration"] = configuration.clone();
        valid["fields"][FIELD_TEXT]["defaultValue"] = value.clone();
        valid["fields"][FIELD_TEXT]["initialDefaultValue"] = value.clone();
        assert!(decode_template(&deterministic(&valid)).is_ok(), "{kind}");

        let next = (index + 1) % cases.len();
        let mut wrong_configuration = valid.clone();
        wrong_configuration["fields"][FIELD_TEXT]["configuration"] = cases[next].1.clone();
        let error = decode_template(&deterministic(&wrong_configuration))
            .expect_err("configuration kind mismatch must fail");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::FieldConfigurationKindMismatch),
            "{kind}"
        );

        let mut wrong_default = valid.clone();
        wrong_default["fields"][FIELD_TEXT]["defaultValue"] = cases[next].2.clone();
        let error = decode_template(&deterministic(&wrong_default))
            .expect_err("default kind mismatch must fail");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::FieldValueKindMismatch),
            "{kind}"
        );

        let mut wrong_initial = valid;
        wrong_initial["fields"][FIELD_TEXT]["initialDefaultValue"] = cases[next].2.clone();
        let error = decode_template(&deterministic(&wrong_initial))
            .expect_err("initial default kind mismatch must fail");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::FieldValueKindMismatch),
            "{kind}"
        );
    }
}

#[test]
fn template_decode_validates_current_and_initial_scalar_defaults() {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    let cases = [
        (
            "singleLineText",
            json!({"kind":"text","value":""}),
            ScalarValueErrorCategory::NonCanonicalEmptyText,
        ),
        (
            "singleLineText",
            json!({"kind":"text","value":"line\nbreak"}),
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            "singleLineText",
            json!({"kind":"text","value":"line\u{2028}break"}),
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            "number",
            json!({"kind":"number","value":"01"}),
            ScalarValueErrorCategory::NonCanonicalDecimal,
        ),
        (
            "date",
            json!({"kind":"date","value":"2026-02-29"}),
            ScalarValueErrorCategory::InvalidCalendarDate,
        ),
        (
            "time",
            json!({"kind":"time","value":"09:30"}),
            ScalarValueErrorCategory::InvalidLocalTime,
        ),
        (
            "duration",
            json!({"kind":"duration","milliseconds":"9223372036854775808"}),
            ScalarValueErrorCategory::DurationOutOfRange,
        ),
    ];

    for (kind, invalid, expected) in cases {
        for (member, location) in [
            ("defaultValue", ArtifactScalarValueLocation::CurrentDefault),
            (
                "initialDefaultValue",
                ArtifactScalarValueLocation::InitialDefault,
            ),
        ] {
            let mut template = minimal_template_value();
            let forbidden_label = "credential=template-scalar-label-secret";
            template["fields"][FIELD_TEXT]["kind"] = json!(kind);
            template["fields"][FIELD_TEXT]["configuration"] = basic_configuration(kind);
            template["fields"][FIELD_TEXT]["label"] = json!(forbidden_label);
            template["fields"][FIELD_TEXT][member] = invalid.clone();
            let error = assert_scalar_codec_error(
                decode_template(&deterministic(&template)),
                expected,
                location,
            );
            assert_eq!(assert_error_chain_redacted(&error, &[forbidden_label]), 1);
        }
    }

    let mut archived = template_value();
    let forbidden_label = "credential=archived-template-label-secret";
    archived["fields"][FIELD_ARCHIVED]["kind"] = json!("singleLineText");
    archived["fields"][FIELD_ARCHIVED]["configuration"] = basic_configuration("singleLineText");
    archived["fields"][FIELD_ARCHIVED]["label"] = json!(forbidden_label);
    archived["fields"][FIELD_ARCHIVED]["defaultValue"] = json!({"kind":"text","value":""});
    let error = assert_scalar_codec_error(
        decode_template(&deterministic(&archived)),
        ScalarValueErrorCategory::NonCanonicalEmptyText,
        ArtifactScalarValueLocation::CurrentDefault,
    );
    assert_eq!(assert_error_chain_redacted(&error, &[forbidden_label]), 1);
}

#[test]
fn document_decode_validates_regular_and_orphan_scalar_values() {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    for (invalid, expected) in [
        (
            json!({"kind":"text","value":""}),
            ScalarValueErrorCategory::NonCanonicalEmptyText,
        ),
        (
            json!({"kind":"text","value":"line\rbreak"}),
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            json!({"kind":"text","value":"line\u{0085}break"}),
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            json!({"kind":"number","value":"1e3"}),
            ScalarValueErrorCategory::InvalidDecimalSyntax,
        ),
        (
            json!({"kind":"date","value":"1900-02-29"}),
            ScalarValueErrorCategory::InvalidCalendarDate,
        ),
        (
            json!({"kind":"time","value":"23:59:60.000"}),
            ScalarValueErrorCategory::InvalidLocalTime,
        ),
        (
            json!({"kind":"duration","milliseconds":"+1"}),
            ScalarValueErrorCategory::InvalidDurationSyntax,
        ),
    ] {
        let mut document = document_value();
        let forbidden_name = "credential=document-scalar-name-secret";
        document["name"] = json!(forbidden_name);
        document["fieldValues"][FIELD_TEXT] = invalid;
        let error = assert_scalar_codec_error(
            decode_document(&deterministic(&document)),
            expected,
            ArtifactScalarValueLocation::DocumentField,
        );
        assert_eq!(assert_error_chain_redacted(&error, &[forbidden_name]), 1);
    }

    let mut orphan = document_value();
    orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("number");
    orphan["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"number","value":"-0"});
    assert_scalar_codec_error(
        decode_document(&deterministic(&orphan)),
        ScalarValueErrorCategory::NonCanonicalDecimal,
        ArtifactScalarValueLocation::OrphanField,
    );

    for (value, expected) in [
        ("", ScalarValueErrorCategory::NonCanonicalEmptyText),
        (
            "line\u{2029}break",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
    ] {
        let mut orphan = document_value();
        let forbidden_label = "credential=orphan-text-label-secret";
        orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("singleLineText");
        orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["label"] = json!(forbidden_label);
        orphan["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"text","value":value});
        let error = assert_scalar_codec_error(
            decode_document(&deterministic(&orphan)),
            expected,
            ArtifactScalarValueLocation::OrphanField,
        );
        assert_eq!(assert_error_chain_redacted(&error, &[forbidden_label]), 1);
    }
}

#[test]
fn template_scalar_defaults_round_trip_without_normalizing_values_or_unknown_members(
) -> Result<(), Box<dyn Error>> {
    let cases = [
        ("singleLineText", json!({"kind":"text","value":"  한글  "})),
        (
            "number",
            json!({"kind":"number","value":"-123456789012345678901234567890.01"}),
        ),
        ("date", json!({"kind":"date","value":"0001-01-01"})),
        ("time", json!({"kind":"time","value":"00:00:00.000"})),
        (
            "duration",
            json!({"kind":"duration","milliseconds":"-9223372036854775808"}),
        ),
    ];

    for (kind, mut value) in cases {
        value["futureScalarMember"] = json!({"keep":[3, 1, 2]});
        let mut template = minimal_template_value();
        template["fields"][FIELD_TEXT]["kind"] = json!(kind);
        template["fields"][FIELD_TEXT]["configuration"] = basic_configuration(kind);
        template["fields"][FIELD_TEXT]["defaultValue"] = value.clone();
        template["fields"][FIELD_TEXT]["initialDefaultValue"] = value;

        let canonical_input = deterministic(&template);
        let decoded = decode_template(&canonical_input)?;
        let encoded = encode_template(&decoded)?;
        assert_eq!(encoded, canonical_input, "{kind}");
        assert_eq!(encode_template(&decode_template(&encoded)?)?, encoded);
    }

    Ok(())
}

#[test]
fn canonical_empty_keeps_unset_historical_absence_and_whitespace_distinct(
) -> Result<(), Box<dyn Error>> {
    let unset = minimal_template_value();
    let unset_bytes = deterministic(&unset);
    let decoded_unset = decode_template(&unset_bytes)?;
    let unset_field = decoded_unset
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("fixture field exists");
    assert!(unset_field.default_value().is_unset());
    assert!(unset_field.initial_default_value().is_unset());
    assert_eq!(encode_template(&decoded_unset)?, unset_bytes);

    let mut whitespace = minimal_template_value();
    whitespace["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"text","value":"  "});
    let whitespace_bytes = deterministic(&whitespace);
    let decoded_whitespace = decode_template(&whitespace_bytes)?;
    let whitespace_field = decoded_whitespace
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("fixture field exists");
    assert_eq!(whitespace_field.default_value().text(), Some("  "));
    assert_eq!(encode_template(&decoded_whitespace)?, whitespace_bytes);

    let mut historical_absence = document_value();
    historical_absence["fieldValues"]
        .as_object_mut()
        .expect("fieldValues fixture is an object")
        .remove(FIELD_ARCHIVED);
    historical_absence["orphanedFieldDefinitions"]
        .as_object_mut()
        .expect("orphan fixture is an object")
        .remove(FIELD_ARCHIVED);
    let decoded_absence = decode_document(&deterministic(&historical_absence))?;
    assert!(!decoded_absence
        .field_values()
        .contains_key(&FieldId::from_str(FIELD_ARCHIVED)?));

    let mut empty = minimal_template_value();
    empty["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"text","value":""});
    assert_scalar_codec_error(
        decode_template(&deterministic(&empty)),
        crate::data::field_engine::scalar::ScalarValueErrorCategory::NonCanonicalEmptyText,
        ArtifactScalarValueLocation::CurrentDefault,
    );
    Ok(())
}

#[test]
fn bound_document_required_policy_is_separate_from_template_defaults_and_standalone_codec(
) -> Result<(), Box<dyn Error>> {
    let mut template = minimal_template_value();
    template["fields"][FIELD_TEXT]["required"] = json!(true);
    let template = decode_template(&deterministic(&template))?;
    let field = template
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("required fixture field exists");
    assert!(field.default_value().is_unset());
    assert!(field.initial_default_value().is_unset());

    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = json!({"kind":"unset"});
    let document = decode_document(&deterministic(&document))
        .expect("standalone codec must not infer Template requiredness");
    let value = document
        .field_values()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("explicit unset remains materialized");
    for context in [
        BoundDocumentValueContext::NewDocumentValue,
        BoundDocumentValueContext::ExistingDocumentValue,
        BoundDocumentValueContext::MaterializedHistorical,
    ] {
        let error = field
            .validate_document_value(value, context)
            .expect_err("active required explicit unset must fail bound validation");
        assert_eq!(
            error.category(),
            FieldValidationErrorCategory::RequiredValueUnset
        );
        assert!(error.option_id().is_none());
        assert!(error.source().is_none());
    }

    let mut optional_template = minimal_template_value();
    optional_template["fields"][FIELD_TEXT]["required"] = json!(false);
    let optional_template = decode_template(&deterministic(&optional_template))?;
    let optional_field = optional_template
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("optional fixture field exists");
    assert_eq!(
        optional_field
            .validate_document_value(value, BoundDocumentValueContext::ExistingDocumentValue,)?,
        FieldValidationOutcome::Valid
    );

    let mut archived_template = minimal_template_value();
    archived_template["fields"][FIELD_TEXT]["required"] = json!(true);
    archived_template["fields"][FIELD_TEXT]["lifecycle"] = json!("archived");
    archived_template["fieldOrder"] = json!([]);
    let archived_template = decode_template(&deterministic(&archived_template))?;
    let archived_field = archived_template
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("archived fixture field exists");
    assert_eq!(
        archived_field
            .validate_document_value(value, BoundDocumentValueContext::ExistingDocumentValue,)?,
        FieldValidationOutcome::Valid
    );
    Ok(())
}

#[test]
fn bound_choice_validation_returns_archived_diagnostic_and_unknown_remains_error(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template(&deterministic(&choice_template_value(
        "multiChoice",
        "active",
    )))?;
    let field = template
        .fields()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("choice fixture field exists");

    let mut existing = document_value();
    existing["fieldValues"][FIELD_TEXT] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]
    });
    let existing = decode_document(&deterministic(&existing))
        .expect("standalone codec must preserve archived selections");
    let value = existing
        .field_values()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .expect("choice value exists");
    let outcome =
        field.validate_document_value(value, BoundDocumentValueContext::ExistingDocumentValue)?;
    let diagnostic = outcome
        .diagnostic()
        .expect("archived existing selection must be observable");
    assert_eq!(
        diagnostic.category(),
        FieldDiagnosticCategory::ExistingSelectionContainsArchivedOption
    );
    assert_eq!(diagnostic.archived_option_count(), 1);

    let mut unknown = document_value();
    unknown["fieldValues"][FIELD_TEXT] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ARCHIVED,OPTION_UNKNOWN]
    });
    let unknown = decode_document(&deterministic(&unknown))?;
    let error = field
        .validate_document_value(
            unknown
                .field_values()
                .get(&FieldId::from_str(FIELD_TEXT)?)
                .expect("unknown choice value exists"),
            BoundDocumentValueContext::ExistingDocumentValue,
        )
        .expect_err("unknown must outrank an archived diagnostic");
    assert_eq!(
        error.choice_category(),
        Some(
            crate::data::field_engine::choice::ChoiceValidationErrorCategory::UnknownSelectedOption
        )
    );
    assert_eq!(error.option_id(), Some(OptionId::from_str(OPTION_UNKNOWN)?));
    Ok(())
}

#[test]
fn document_scalar_values_and_orphan_snapshot_round_trip_byte_identically(
) -> Result<(), Box<dyn Error>> {
    let values = [
        json!({"kind":"text","value":"  "}),
        json!({"kind":"number","value":"0.000000000000000000000000000001"}),
        json!({"kind":"date","value":"9999-12-31"}),
        json!({"kind":"time","value":"23:59:59.999"}),
        json!({"kind":"duration","milliseconds":"9223372036854775807"}),
    ];

    for mut value in values {
        value["futureScalarMember"] = json!({"preserve":true});
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] = value;
        let canonical_input = deterministic(&document);
        let decoded = decode_document(&canonical_input)?;
        let encoded = encode_document(&decoded)?;
        assert_eq!(encoded, canonical_input);
        assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    }

    let mut orphan = document_value();
    orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("date");
    orphan["fieldValues"][FIELD_ARCHIVED] = json!({
        "kind":"date",
        "value":"2000-02-29",
        "futureScalarMember":{"preserve":"orphan"}
    });
    let canonical_input = deterministic(&orphan);
    let encoded = encode_document(&decode_document(&canonical_input)?)?;
    assert_eq!(encoded, canonical_input);
    assert_eq!(encode_document(&decode_document(&encoded)?)?, encoded);
    Ok(())
}

#[test]
fn template_encode_revalidates_every_scalar_default_slot_and_redacts_raw_values(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    let cases = [
        (
            "singleLineText",
            json!({"kind":"text","value":"valid"}),
            "credential=text-secret\nsecond-line",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            "singleLineText",
            json!({"kind":"text","value":"valid"}),
            "credential=unicode-text-secret\u{2028}second-line",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            "number",
            json!({"kind":"number","value":"1"}),
            "credential=number-secret",
            ScalarValueErrorCategory::InvalidDecimalSyntax,
        ),
        (
            "date",
            json!({"kind":"date","value":"2026-09-04"}),
            "credential=date-secret",
            ScalarValueErrorCategory::InvalidCalendarDate,
        ),
        (
            "time",
            json!({"kind":"time","value":"09:30:00.000"}),
            "credential=time-secret",
            ScalarValueErrorCategory::InvalidLocalTime,
        ),
        (
            "duration",
            json!({"kind":"duration","milliseconds":"1"}),
            "credential=duration-secret",
            ScalarValueErrorCategory::InvalidDurationSyntax,
        ),
    ];
    let field_id = FieldId::from_str(FIELD_TEXT)?;

    for (kind, valid, invalid_raw, expected) in cases {
        for (initial, location) in [
            (false, ArtifactScalarValueLocation::CurrentDefault),
            (true, ArtifactScalarValueLocation::InitialDefault),
        ] {
            let mut template = minimal_template_value();
            template["fields"][FIELD_TEXT]["kind"] = json!(kind);
            template["fields"][FIELD_TEXT]["configuration"] = basic_configuration(kind);
            template["fields"][FIELD_TEXT]["defaultValue"] = valid.clone();
            template["fields"][FIELD_TEXT]["initialDefaultValue"] = valid.clone();
            let mut artifact = decode_template(&deterministic(&template))?;
            artifact.corrupt_scalar_default_for_test(field_id, invalid_raw, initial);
            let error = assert_scalar_codec_error(encode_template(&artifact), expected, location);
            assert_error_chain_redacted(&error, &[invalid_raw]);
        }
    }

    for (initial, location) in [
        (false, ArtifactScalarValueLocation::CurrentDefault),
        (true, ArtifactScalarValueLocation::InitialDefault),
    ] {
        let mut template = minimal_template_value();
        let forbidden_label = "credential=empty-template-label-secret";
        template["fields"][FIELD_TEXT]["label"] = json!(forbidden_label);
        template["fields"][FIELD_TEXT]["defaultValue"] = json!({"kind":"text","value":"valid"});
        template["fields"][FIELD_TEXT]["initialDefaultValue"] =
            json!({"kind":"text","value":"valid"});
        let mut artifact = decode_template(&deterministic(&template))?;
        artifact.corrupt_scalar_default_for_test(field_id, "", initial);
        let error = assert_scalar_codec_error(
            encode_template(&artifact),
            ScalarValueErrorCategory::NonCanonicalEmptyText,
            location,
        );
        assert_eq!(assert_error_chain_redacted(&error, &[forbidden_label]), 1);
    }
    Ok(())
}

#[test]
fn document_encode_revalidates_regular_and_orphan_scalar_values() -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    let cases = [
        (
            json!({"kind":"text","value":"valid"}),
            "credential=text-secret\rsecond-line",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            json!({"kind":"text","value":"valid"}),
            "credential=unicode-text-secret\u{0085}second-line",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
        (
            json!({"kind":"number","value":"1"}),
            "01",
            ScalarValueErrorCategory::NonCanonicalDecimal,
        ),
        (
            json!({"kind":"date","value":"2026-09-04"}),
            "2026-04-31",
            ScalarValueErrorCategory::InvalidCalendarDate,
        ),
        (
            json!({"kind":"time","value":"09:30:00.000"}),
            "24:00:00.000",
            ScalarValueErrorCategory::InvalidLocalTime,
        ),
        (
            json!({"kind":"duration","milliseconds":"1"}),
            "-9223372036854775809",
            ScalarValueErrorCategory::DurationOutOfRange,
        ),
    ];
    let field_id = FieldId::from_str(FIELD_TEXT)?;

    for (valid, invalid_raw, expected) in cases {
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] = valid;
        let mut artifact = decode_document(&deterministic(&document))?;
        artifact.corrupt_scalar_value_for_test(field_id, invalid_raw);
        let error = assert_scalar_codec_error(
            encode_document(&artifact),
            expected,
            ArtifactScalarValueLocation::DocumentField,
        );
        assert_error_chain_redacted(&error, &[invalid_raw]);
    }

    let mut empty_document = document_value();
    let forbidden_name = "credential=empty-document-name-secret";
    empty_document["name"] = json!(forbidden_name);
    empty_document["fieldValues"][FIELD_TEXT] = json!({"kind":"text","value":"valid"});
    let mut empty_artifact = decode_document(&deterministic(&empty_document))?;
    empty_artifact.corrupt_scalar_value_for_test(field_id, "");
    let error = assert_scalar_codec_error(
        encode_document(&empty_artifact),
        ScalarValueErrorCategory::NonCanonicalEmptyText,
        ArtifactScalarValueLocation::DocumentField,
    );
    assert_eq!(assert_error_chain_redacted(&error, &[forbidden_name]), 1);

    let orphan_id = FieldId::from_str(FIELD_ARCHIVED)?;
    let mut orphan = document_value();
    orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("number");
    orphan["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"number","value":"1"});
    let mut artifact = decode_document(&deterministic(&orphan))?;
    artifact.corrupt_scalar_value_for_test(orphan_id, "1.0");
    let error = assert_scalar_codec_error(
        encode_document(&artifact),
        ScalarValueErrorCategory::NonCanonicalDecimal,
        ArtifactScalarValueLocation::OrphanField,
    );
    assert_error_chain_redacted(&error, &["1.0"]);

    for (invalid_raw, expected) in [
        ("", ScalarValueErrorCategory::NonCanonicalEmptyText),
        (
            "credential=orphan-unicode-secret\u{2029}second-line",
            ScalarValueErrorCategory::MultilineSingleLineText,
        ),
    ] {
        let mut orphan = document_value();
        let forbidden_label = "credential=encode-orphan-label-secret";
        orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("singleLineText");
        orphan["orphanedFieldDefinitions"][FIELD_ARCHIVED]["label"] = json!(forbidden_label);
        orphan["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"text","value":"valid"});
        let mut artifact = decode_document(&deterministic(&orphan))?;
        artifact.corrupt_scalar_value_for_test(orphan_id, invalid_raw);
        let error = assert_scalar_codec_error(
            encode_document(&artifact),
            expected,
            ArtifactScalarValueLocation::OrphanField,
        );
        assert_eq!(assert_error_chain_redacted(&error, &[forbidden_label]), 1);
        if !invalid_raw.is_empty() {
            assert_eq!(assert_error_chain_redacted(&error, &[invalid_raw]), 1);
        }
    }
    Ok(())
}

#[test]
fn template_choice_membership_is_field_local_during_decode() -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    // Field B가 자기 B1/B2를 참조하는 정상 fixture는 decode/encode와 byte identity를 통과한다.
    for kind in ["singleChoice", "multiChoice"] {
        let valid = field_local_choice_template_value(kind, "active");
        let bytes = deterministic(&valid);
        assert_eq!(encode_template(&decode_template(&bytes)?)?, bytes);
    }

    let mut single_current = field_local_choice_template_value("singleChoice", "active");
    single_current["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_OTHER_FIELD_FIRST});
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&single_current)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    assert_eq!(
        error.choice_option_id(),
        Some(OptionId::from_str(OPTION_OTHER_FIELD_FIRST)?)
    );
    assert_eq!(
        assert_error_chain_redacted(
            &error,
            &[
                "다른 필드의 첫 선택",
                &format!("[{OPTION_ACTIVE}, {OPTION_OTHER_FIELD_FIRST}]"),
                "credential=field-local-secret",
                "C:\\Users\\audit\\field-local.json",
                "/home/audit/field-local.json",
            ],
        ),
        1
    );

    let mut multi_current = field_local_choice_template_value("multiChoice", "active");
    multi_current["fields"][FIELD_TEXT]["defaultValue"] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ACTIVE,OPTION_OTHER_FIELD_FIRST]
    });
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&multi_current)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    assert_eq!(
        error.choice_option_id(),
        Some(OptionId::from_str(OPTION_OTHER_FIELD_FIRST)?)
    );

    let mut historical_initial = field_local_choice_template_value("singleChoice", "active");
    historical_initial["fields"][FIELD_TEXT]["initialDefaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_OTHER_FIELD_FIRST});
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&historical_initial)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::InitialDefault,
    );
    assert_eq!(
        error.choice_option_id(),
        Some(OptionId::from_str(OPTION_OTHER_FIELD_FIRST)?)
    );

    let mut archived_preserved = field_local_choice_template_value("singleChoice", "archived");
    archived_preserved["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_OTHER_FIELD_FIRST});
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&archived_preserved)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    assert_eq!(
        error.choice_option_id(),
        Some(OptionId::from_str(OPTION_OTHER_FIELD_FIRST)?)
    );
    Ok(())
}

#[test]
fn template_choice_membership_is_field_local_during_encode() -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let field_a = FieldId::from_str(FIELD_TEXT)?;
    let field_a_option = OptionId::from_str(OPTION_ACTIVE)?;
    let field_b_option = OptionId::from_str(OPTION_OTHER_FIELD_FIRST)?;
    for (lifecycle, initial, corrupted, expected_location) in [
        (
            "active",
            false,
            vec![field_a_option, field_b_option],
            ArtifactChoiceValueLocation::CurrentDefault,
        ),
        (
            "active",
            true,
            vec![field_b_option],
            ArtifactChoiceValueLocation::InitialDefault,
        ),
        (
            "archived",
            false,
            vec![field_b_option],
            ArtifactChoiceValueLocation::CurrentDefault,
        ),
        (
            "archived",
            true,
            vec![field_b_option],
            ArtifactChoiceValueLocation::InitialDefault,
        ),
    ] {
        let mut template = field_local_choice_template_value("multiChoice", lifecycle);
        template["fields"][FIELD_TEXT]["defaultValue"] =
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]});
        template["fields"][FIELD_TEXT]["initialDefaultValue"] =
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]});
        let mut artifact = decode_template(&deterministic(&template))?;
        artifact.corrupt_multi_choice_default_for_test(field_a, corrupted, initial);

        let error = assert_choice_codec_error(
            encode_template(&artifact),
            ChoiceValidationErrorCategory::UnknownSelectedOption,
            expected_location,
        );
        assert_eq!(error.choice_option_id(), Some(field_b_option));
    }
    Ok(())
}

#[test]
fn template_choice_defaults_apply_context_specific_membership_rules() -> Result<(), Box<dyn Error>>
{
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let mut active = choice_template_value("singleChoice", "active");
    active["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ACTIVE});
    decode_template(&deterministic(&active))?;

    let mut archived_current = active.clone();
    archived_current["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&archived_current)),
        ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    assert_eq!(
        error.choice_option_id(),
        Some(OptionId::from_str(OPTION_ARCHIVED)?)
    );

    let mut unknown_current = active.clone();
    unknown_current["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_UNKNOWN});
    assert_choice_codec_error(
        decode_template(&deterministic(&unknown_current)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::CurrentDefault,
    );

    let mut historical = active.clone();
    historical["fields"][FIELD_TEXT]["initialDefaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    decode_template(&deterministic(&historical))?;

    let mut unknown_historical = active;
    unknown_historical["fields"][FIELD_TEXT]["initialDefaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_UNKNOWN});
    assert_choice_codec_error(
        decode_template(&deterministic(&unknown_historical)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::InitialDefault,
    );

    let mut preserved = choice_template_value("singleChoice", "archived");
    preserved["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    preserved["fields"][FIELD_TEXT]["initialDefaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    decode_template(&deterministic(&preserved))?;

    let mut unknown_preserved = preserved;
    unknown_preserved["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_UNKNOWN});
    assert_choice_codec_error(
        decode_template(&deterministic(&unknown_preserved)),
        ChoiceValidationErrorCategory::UnknownSelectedOption,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    Ok(())
}

#[test]
fn template_multi_choice_defaults_require_canonical_arrays_without_using_display_order(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let mut template = choice_template_value("multiChoice", "active");
    template["fields"][FIELD_TEXT]["defaultValue"] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ACTIVE,OPTION_ACTIVE_SECOND],
        "futureChoiceValue":{"keep":[3,1,2]}
    });
    template["fields"][FIELD_TEXT]["initialDefaultValue"] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED],
        "futureInitialChoiceValue":{"keep":"history"}
    });
    let canonical_bytes = deterministic(&template);
    let artifact = decode_template(&canonical_bytes)?;
    assert_eq!(encode_template(&artifact)?, canonical_bytes);

    let mut reordered_display = template.clone();
    reordered_display["fields"][FIELD_TEXT]["configuration"]["optionOrder"] =
        json!([OPTION_ACTIVE, OPTION_ACTIVE_SECOND]);
    reordered_display["fields"][FIELD_TEXT]["configuration"]["options"][OPTION_ACTIVE]["label"] =
        json!("이름이 바뀌어도 value 정렬과 무관");
    let encoded = encode_template(&decode_template(&deterministic(&reordered_display))?)?;
    let encoded_value: Value = serde_json::from_slice(&encoded)?;
    assert_eq!(
        encoded_value["fields"][FIELD_TEXT]["defaultValue"]["optionIds"],
        json!([OPTION_ACTIVE, OPTION_ACTIVE_SECOND])
    );

    for (option_ids, category) in [
        (
            json!([]),
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
        ),
        (
            json!([OPTION_ACTIVE, OPTION_ACTIVE]),
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
        ),
        (
            json!([OPTION_ACTIVE_SECOND, OPTION_ACTIVE]),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
        (
            json!([OPTION_ACTIVE, OPTION_ARCHIVED, OPTION_ACTIVE_SECOND]),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
    ] {
        let mut invalid = choice_template_value("multiChoice", "active");
        invalid["fields"][FIELD_TEXT]["defaultValue"] =
            json!({"kind":"multiChoice","optionIds":option_ids});
        assert_choice_codec_error(
            decode_template(&deterministic(&invalid)),
            category,
            ArtifactChoiceValueLocation::CurrentDefault,
        );
    }

    let mut archived_in_current = choice_template_value("multiChoice", "active");
    archived_in_current["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]});
    assert_choice_codec_error(
        decode_template(&deterministic(&archived_in_current)),
        ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    Ok(())
}

#[test]
fn document_standalone_choice_validation_enforces_only_canonical_shape(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let mut canonical = document_value();
    canonical["fieldValues"][FIELD_TEXT] =
        json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]});
    let canonical_bytes = deterministic(&canonical);
    assert_eq!(
        encode_document(&decode_document(&canonical_bytes)?)?,
        canonical_bytes
    );

    // Template에 없는 ID도 standalone codec은 membership을 추측하지 않고 보존한다.
    let mut unknown = document_value();
    unknown["fieldValues"][FIELD_TEXT] = json!({"kind":"multiChoice","optionIds":[OPTION_UNKNOWN]});
    let unknown_bytes = deterministic(&unknown);
    assert_eq!(
        encode_document(&decode_document(&unknown_bytes)?)?,
        unknown_bytes
    );

    for (option_ids, category) in [
        (
            json!([]),
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
        ),
        (
            json!([OPTION_ACTIVE, OPTION_ACTIVE]),
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
        ),
        (
            json!([OPTION_ARCHIVED, OPTION_ACTIVE]),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
        (
            json!([OPTION_ACTIVE, OPTION_ARCHIVED, OPTION_ACTIVE_SECOND]),
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
    ] {
        let mut invalid = document_value();
        invalid["fieldValues"][FIELD_TEXT] = json!({"kind":"multiChoice","optionIds":option_ids});
        assert_choice_codec_error(
            decode_document(&deterministic(&invalid)),
            category,
            ArtifactChoiceValueLocation::DocumentField,
        );
    }
    Ok(())
}

#[test]
fn orphan_multi_choice_keeps_snapshot_equality_and_requires_canonical_value_order(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let mut canonical = document_value();
    canonical["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["kind"] = json!("multiChoice");
    canonical["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ACTIVE] =
        json!({"label":"선택 당시 이름"});
    canonical["fieldValues"][FIELD_ORPHAN_CHOICE] =
        json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED]});
    let bytes = deterministic(&canonical);
    assert_eq!(encode_document(&decode_document(&bytes)?)?, bytes);

    let mut reversed = canonical.clone();
    reversed["fieldValues"][FIELD_ORPHAN_CHOICE] =
        json!({"kind":"multiChoice","optionIds":[OPTION_ARCHIVED,OPTION_ACTIVE]});
    assert_choice_codec_error(
        decode_document(&deterministic(&reversed)),
        ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ArtifactChoiceValueLocation::OrphanField,
    );

    let mut missing_snapshot = canonical.clone();
    missing_snapshot["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"]
        .as_object_mut()
        .expect("snapshot options should be an object")
        .remove(OPTION_ACTIVE);
    let error = decode_document(&deterministic(&missing_snapshot))
        .expect_err("selected set and snapshot keys must remain equal");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::OrphanOptionMismatch)
    );

    let mut canonical_unset = document_value();
    canonical_unset["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["kind"] = json!("multiChoice");
    canonical_unset["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"] = json!({});
    canonical_unset["fieldValues"][FIELD_ORPHAN_CHOICE] = json!({"kind":"unset"});
    decode_document(&deterministic(&canonical_unset))?;

    let mut empty_variant = canonical_unset;
    empty_variant["fieldValues"][FIELD_ORPHAN_CHOICE] =
        json!({"kind":"multiChoice","optionIds":[]});
    assert_choice_codec_error(
        decode_document(&deterministic(&empty_variant)),
        ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
        ArtifactChoiceValueLocation::OrphanField,
    );
    Ok(())
}

#[test]
fn encode_revalidates_corrupted_template_and_document_multi_choice_values(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let field_id = FieldId::from_str(FIELD_TEXT)?;
    let active = OptionId::from_str(OPTION_ACTIVE)?;
    let second = OptionId::from_str(OPTION_ACTIVE_SECOND)?;

    for (initial, location) in [
        (false, ArtifactChoiceValueLocation::CurrentDefault),
        (true, ArtifactChoiceValueLocation::InitialDefault),
    ] {
        for (corruption, category) in [
            (
                Vec::new(),
                ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
            ),
            (
                vec![active, active],
                ChoiceValidationErrorCategory::DuplicateSelectedOption,
            ),
            (
                vec![second, active],
                ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
            ),
        ] {
            let mut template = choice_template_value("multiChoice", "active");
            template["fields"][FIELD_TEXT]["defaultValue"] =
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]});
            template["fields"][FIELD_TEXT]["initialDefaultValue"] =
                json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]});
            let mut artifact = decode_template(&deterministic(&template))?;
            artifact.corrupt_multi_choice_default_for_test(field_id, corruption, initial);
            assert_choice_codec_error(encode_template(&artifact), category, location);
        }
    }

    for (corruption, category) in [
        (
            Vec::new(),
            ChoiceValidationErrorCategory::EmptyMultiChoiceMustUseUnset,
        ),
        (
            vec![active, active],
            ChoiceValidationErrorCategory::DuplicateSelectedOption,
        ),
        (
            vec![second, active],
            ChoiceValidationErrorCategory::NonCanonicalSelectedOptionOrder,
        ),
    ] {
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] =
            json!({"kind":"multiChoice","optionIds":[OPTION_ACTIVE]});
        let mut artifact = decode_document(&deterministic(&document))?;
        artifact.corrupt_multi_choice_value_for_test(field_id, corruption);
        assert_choice_codec_error(
            encode_document(&artifact),
            category,
            ArtifactChoiceValueLocation::DocumentField,
        );
    }
    Ok(())
}

#[test]
fn choice_codec_errors_redact_labels_content_arrays_and_paths() {
    use crate::data::field_engine::choice::ChoiceValidationErrorCategory;

    let forbidden_label = "credential=choice-label-secret";
    let forbidden_path = "C:\\Users\\audit\\choice.json";
    let mut template = choice_template_value("singleChoice", "active");
    template["fields"][FIELD_TEXT]["label"] = json!(forbidden_label);
    template["futurePath"] = json!(forbidden_path);
    template["fields"][FIELD_TEXT]["defaultValue"] =
        json!({"kind":"singleChoice","optionId":OPTION_ARCHIVED});
    let error = assert_choice_codec_error(
        decode_template(&deterministic(&template)),
        ChoiceValidationErrorCategory::ArchivedOptionNotSelectable,
        ArtifactChoiceValueLocation::CurrentDefault,
    );
    assert_eq!(
        assert_error_chain_redacted(
            &error,
            &[
                forbidden_label,
                forbidden_path,
                "credential=",
                &format!("[{OPTION_ACTIVE}, {OPTION_ARCHIVED}]")
            ]
        ),
        1
    );
    assert!(error.source().is_none());
}

#[test]
fn rich_text_requires_a_v1_object_envelope_and_canonical_root() {
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind": "richText",
        "document": {
            "schemaVersion": 1,
            "content": {
                "kind": "root",
                "children": [{"kind":"paragraph","children":[{"kind":"text","text":"본문"}]}],
                "futureNode": [1, 2, 3]
            }
        }
    });
    assert!(decode_document(&deterministic(&document)).is_ok());

    for schema_version in [0, 2] {
        let mut invalid = document_value();
        invalid["fieldValues"][FIELD_TEXT] = json!({
            "kind":"richText",
            "document":{
                "schemaVersion":schema_version,
                "content":rich_text_content("본문")
            }
        });
        assert_rich_text_codec_error(
            decode_document(&deterministic(&invalid)),
            RichTextValidationErrorCategory::UnsupportedSchemaVersion,
            ArtifactRichTextValueLocation::DocumentField,
        );
    }

    let mut invalid_root = document_value();
    invalid_root["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document":{"schemaVersion":1,"content":{}}
    });
    assert_rich_text_codec_error(
        decode_document(&deterministic(&invalid_root)),
        RichTextValidationErrorCategory::InvalidRoot,
        ArtifactRichTextValueLocation::DocumentField,
    );

    for invalid_document in [
        json!({"schemaVersion":"1","content":{}}),
        json!({"content":{}}),
        json!({"schemaVersion":1}),
        json!({"schemaVersion":1,"content":"<p>raw HTML</p>"}),
        json!({"schemaVersion":1,"content":[]}),
        json!({"schemaVersion":1,"content":null}),
    ] {
        let mut invalid = document_value();
        invalid["fieldValues"][FIELD_TEXT] = json!({"kind":"richText","document":invalid_document});
        assert_codec_category(
            decode_document(&deterministic(&invalid)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }
}

#[test]
fn template_rich_text_validates_current_initial_and_archived_defaults() -> Result<(), Box<dyn Error>>
{
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    for lifecycle in ["active", "archived"] {
        let template = rich_text_template_value(lifecycle);
        let bytes = deterministic(&template);
        assert_eq!(encode_template(&decode_template(&bytes)?)?, bytes);

        for (slot, location) in [
            (
                "defaultValue",
                ArtifactRichTextValueLocation::CurrentDefault,
            ),
            (
                "initialDefaultValue",
                ArtifactRichTextValueLocation::InitialDefault,
            ),
        ] {
            for invalid_content in [
                json!({"kind":"root","children":[{"kind":"paragraph","children":[]}]}),
                hard_break_only_content(),
            ] {
                let mut invalid = template.clone();
                invalid["fields"][FIELD_TEXT][slot] = json!({
                    "kind":"richText",
                    "document":{"schemaVersion":1,"content":invalid_content}
                });
                let error = assert_rich_text_codec_error(
                    decode_template(&deterministic(&invalid)),
                    RichTextValidationErrorCategory::SemanticEmpty,
                    location,
                );
                assert_eq!(
                    error.rich_text_structure_location(),
                    Some(crate::data::field_engine::rich_text::RichTextErrorLocation::Root)
                );
            }
        }
    }
    Ok(())
}

#[test]
fn document_rich_text_validates_regular_and_orphan_slots_without_template_lookup(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    let mut valid = document_value();
    valid["fieldValues"][FIELD_TEXT] = rich_text_value("일반 본문");
    valid["fieldValues"][FIELD_ARCHIVED] = rich_text_value("보존 본문");
    let bytes = deterministic(&valid);
    assert_eq!(encode_document(&decode_document(&bytes)?)?, bytes);

    let mut whitespace_and_break = valid.clone();
    whitespace_and_break["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{"kind":"root","children":[{"kind":"paragraph","children":[
                {"kind":"text","text":" "},{"kind":"hardBreak"}
            ]}]}
        }
    });
    let whitespace_bytes = deterministic(&whitespace_and_break);
    assert_eq!(
        encode_document(&decode_document(&whitespace_bytes)?)?,
        whitespace_bytes
    );

    for (field_id, location) in [
        (FIELD_TEXT, ArtifactRichTextValueLocation::DocumentField),
        (FIELD_ARCHIVED, ArtifactRichTextValueLocation::OrphanField),
    ] {
        for invalid_content in [
            json!({"kind":"root","children":[]}),
            hard_break_only_content(),
        ] {
            let mut invalid = valid.clone();
            invalid["fieldValues"][field_id] = json!({
                "kind":"richText",
                "document":{"schemaVersion":1,"content":invalid_content}
            });
            assert_rich_text_codec_error(
                decode_document(&deterministic(&invalid)),
                RichTextValidationErrorCategory::SemanticEmpty,
                location,
            );
        }
    }
    Ok(())
}

#[test]
fn artifact_rich_text_rejects_noncanonical_ast_without_normalizing_persistent_bytes() {
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    for (content, category) in [
        (
            json!({"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":""}]}]}),
            RichTextValidationErrorCategory::EmptyTextNode,
        ),
        (
            json!({"kind":"root","children":[{"kind":"paragraph","children":[
                {"kind":"text","text":"a"},{"kind":"text","text":"b"}
            ]}]}),
            RichTextValidationErrorCategory::AdjacentEquivalentText,
        ),
        (
            json!({"kind":"root","children":[{"kind":"paragraph","children":[{
                "kind":"text","text":"본문","marks":["underline","bold"]
            }]}]}),
            RichTextValidationErrorCategory::NonCanonicalMarkOrder,
        ),
        (
            json!({"kind":"root","children":[{"kind":"image","src":"https://example.invalid/private"}]}),
            RichTextValidationErrorCategory::UnknownNodeType,
        ),
    ] {
        let mut document = document_value();
        document["fieldValues"][FIELD_TEXT] = json!({
            "kind":"richText",
            "document":{"schemaVersion":1,"content":content}
        });
        assert_rich_text_codec_error(
            decode_document(&deterministic(&document)),
            category,
            ArtifactRichTextValueLocation::DocumentField,
        );
    }
}

#[test]
fn rich_text_unknown_extras_and_numbers_round_trip_byte_identically() -> Result<(), Box<dyn Error>>
{
    let huge: Value = serde_json::from_str("1234567890123456789012345678901234567890")?;
    let other_huge: Value = serde_json::from_str("1234567890123456789012345678901234567891")?;
    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{
                "kind":"root",
                "children":[{
                    "kind":"paragraph",
                    "children":[
                        {"kind":"text","text":"가","marks":["bold"],"futureText":{"huge":huge.clone(),"nested":[{"b":2,"a":1}]}},
                        {"kind":"text","text":"나","marks":["bold"],"futureText":{"huge":huge.clone(),"nested":[{"b":2,"a":1}]}},
                        {"kind":"text","text":"다","marks":["bold"],"futureText":{"huge":other_huge,"nested":[{"b":2,"a":1}]}}
                    ],
                    "futureBlock":[3,1,2]
                }],
                "futureRoot":{"huge":huge,"nested":[{"z":2,"a":1}]}
            },
            "futureEnvelope":{"keep":true}
        },
        "futureValue":{"keep":"value"}
    });
    let bytes = deterministic(&document);
    let decoded = decode_document(&bytes)?;
    assert_eq!(encode_document(&decoded)?, bytes);
    assert_eq!(encode_document(&decode_document(&bytes)?)?, bytes);
    Ok(())
}

#[test]
fn encode_revalidates_corrupted_rich_text_slots_without_returning_bytes(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    let field_id = FieldId::from_str(FIELD_TEXT)?;
    let invalid_content = |hard_break_only: bool| {
        let value = if hard_break_only {
            hard_break_only_content()
        } else {
            json!({"kind":"root","children":[{"kind":"paragraph","children":[]}]})
        };
        value
            .as_object()
            .expect("fixture must be an object")
            .clone()
    };

    for (initial, location) in [
        (false, ArtifactRichTextValueLocation::CurrentDefault),
        (true, ArtifactRichTextValueLocation::InitialDefault),
    ] {
        for hard_break_only in [false, true] {
            let mut template =
                decode_template(&deterministic(&rich_text_template_value("active")))?;
            template.replace_rich_text_default_content_for_test(
                field_id,
                invalid_content(hard_break_only),
                initial,
            );
            assert_rich_text_codec_error(
                encode_template(&template),
                RichTextValidationErrorCategory::SemanticEmpty,
                location,
            );
        }
    }

    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = rich_text_value("본문");
    document["fieldValues"][FIELD_ARCHIVED] = rich_text_value("보존 본문");
    for (target, location) in [
        (field_id, ArtifactRichTextValueLocation::DocumentField),
        (
            FieldId::from_str(FIELD_ARCHIVED)?,
            ArtifactRichTextValueLocation::OrphanField,
        ),
    ] {
        for hard_break_only in [false, true] {
            let mut artifact = decode_document(&deterministic(&document))?;
            artifact.replace_rich_text_content_for_test(target, invalid_content(hard_break_only));
            assert_rich_text_codec_error(
                encode_document(&artifact),
                RichTextValidationErrorCategory::SemanticEmpty,
                location,
            );
        }
    }
    Ok(())
}

#[test]
fn rich_text_codec_errors_redact_body_discriminator_url_credentials_and_paths() {
    use crate::data::field_engine::rich_text::RichTextValidationErrorCategory;

    let body = "credential=rich-body-secret";
    let discriminator = "credential=attacker-node";
    let windows_path = "C:\\Users\\audit\\rich-text.json";
    let unix_path = "/home/audit/rich-text.json";
    let url = "https://example.invalid/private?token=secret";
    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{
                "kind":"root",
                "children":[{"kind":discriminator,"children":[],"raw":body}],
                "future":{"windows":windows_path,"unix":unix_path,"url":url}
            }
        }
    });
    let error = assert_rich_text_codec_error(
        decode_document(&deterministic(&document)),
        RichTextValidationErrorCategory::UnknownNodeType,
        ArtifactRichTextValueLocation::DocumentField,
    );
    assert_eq!(
        error.rich_text_structure_location(),
        Some(crate::data::field_engine::rich_text::RichTextErrorLocation::Block)
    );
    assert_eq!(error.rich_text_node_depth(), Some(2));
    assert_eq!(
        assert_error_chain_redacted(
            &error,
            &[
                body,
                discriminator,
                windows_path,
                unix_path,
                url,
                "token=secret"
            ]
        ),
        1
    );
}

#[test]
fn production_value_and_aggregate_debug_redact_payloads_and_unknown_extras(
) -> Result<(), Box<dyn Error>> {
    let text_secret = "credential=rich-text-debug-secret";
    let windows_path = "C:\\Users\\audit\\debug-rich-text.json";
    let unix_path = "/home/audit/debug-rich-text.json";
    let document_extra_secret = "credential=document-extra-debug-secret";
    let value_extra_secret = "credential=value-extra-debug-secret";
    let nested_secret = "credential=nested-ast-debug-secret";

    let mut document = document_value();
    document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "content":{
                "kind":"root",
                "children":[{"kind":"paragraph","children":[{
                    "kind":"text",
                    "text":text_secret,
                    "futureText":{"nested":[nested_secret]}
                }]}],
                "futureRoot":{"windows":windows_path,"unix":unix_path}
            },
            "futureEnvelope":document_extra_secret
        },
        "futureValue":value_extra_secret
    });
    let decoded = decode_document(&deterministic(&document))?;
    let field_id = FieldId::from_str(FIELD_TEXT)?;
    let field_value = decoded
        .field_values()
        .get(&field_id)
        .expect("fixture must contain the rich-text value");
    let rich_text = field_value
        .rich_text()
        .expect("fixture must contain a rich-text document");
    for debug in [
        format!("{rich_text:?}"),
        format!("{field_value:?}"),
        format!("{decoded:?}"),
    ] {
        for forbidden in [
            text_secret,
            windows_path,
            unix_path,
            document_extra_secret,
            value_extra_secret,
            nested_secret,
        ] {
            assert!(!debug.contains(forbidden), "Debug leaked {forbidden}");
        }
    }

    let scalar_secret = "credential=scalar-debug-secret";
    let scalar_extra_secret = "credential=scalar-extra-debug-secret";
    let mut scalar_document = document_value();
    scalar_document["fieldValues"][FIELD_TEXT] =
        json!({"kind":"number","value":"42","futureValue":scalar_extra_secret});
    let mut decoded_scalar = decode_document(&deterministic(&scalar_document))?;
    decoded_scalar.corrupt_scalar_value_for_test(field_id, scalar_secret);
    let scalar_value = decoded_scalar
        .field_values()
        .get(&field_id)
        .expect("fixture must contain the scalar value");
    let scalar_value_debug = format!("{scalar_value:?}");
    for forbidden in [scalar_secret, scalar_extra_secret, "42"] {
        assert!(
            !scalar_value_debug.contains(forbidden),
            "Debug leaked {forbidden}"
        );
    }
    let scalar_aggregate_debug = format!("{decoded_scalar:?}");
    for forbidden in [scalar_secret, scalar_extra_secret] {
        assert!(
            !scalar_aggregate_debug.contains(forbidden),
            "Debug leaked {forbidden}"
        );
    }

    let choice_extra_secret = "credential=choice-extra-debug-secret";
    let mut choice_document = document_value();
    choice_document["fieldValues"][FIELD_TEXT] = json!({
        "kind":"multiChoice",
        "optionIds":[OPTION_ACTIVE,OPTION_ARCHIVED],
        "futureValue":choice_extra_secret
    });
    let decoded_choice = decode_document(&deterministic(&choice_document))?;
    let choice_value = decoded_choice
        .field_values()
        .get(&field_id)
        .expect("fixture must contain the choice value");
    let choice_debug = format!("{choice_value:?}");
    for forbidden in [OPTION_ACTIVE, OPTION_ARCHIVED, choice_extra_secret] {
        assert!(
            !choice_debug.contains(forbidden),
            "Debug leaked {forbidden}"
        );
    }

    let mut template = rich_text_template_value("active");
    template["fields"][FIELD_TEXT]["defaultValue"] = rich_text_value(text_secret);
    template["fields"][FIELD_TEXT]["defaultValue"]["futureValue"] = json!(value_extra_secret);
    let decoded_template = decode_template(&deterministic(&template))?;
    let template_debug = format!("{decoded_template:?}");
    for forbidden in [text_secret, value_extra_secret] {
        assert!(
            !template_debug.contains(forbidden),
            "Debug leaked {forbidden}"
        );
    }

    Ok(())
}

#[test]
fn document_and_orphan_debug_redact_each_independent_storage_layer() -> Result<(), Box<dyn Error>> {
    const DOCUMENT_NAME: &str = "document-name-debug-canary-7d53f96f";
    const DOCUMENT_EXTRA_KEY: &str = "m2_5aDocumentRootExtraKeyCanary8a294c31";
    const DOCUMENT_EXTRA: &str = "document-extra-debug-canary-8a294c31";
    const VALUE_PAYLOAD: &str = "field-value-payload-debug-canary-02cd78ea";
    const VALUE_EXTRA_KEY: &str = "m2_5aFieldValueOuterExtraKeyCanary3cbf89d1";
    const VALUE_EXTRA: &str = "field-value-extra-debug-canary-3cbf89d1";
    const ORPHAN_FIELD_LABEL: &str = "orphan-field-label-debug-canary-45ea271c";
    const ORPHAN_FIELD_EXTRA_KEY: &str = "m2_5aOrphanFieldExtraKeyCanary56fb382d";
    const ORPHAN_FIELD_EXTRA: &str = "orphan-field-extra-debug-canary-56fb382d";
    const ORPHAN_OPTION_LABEL: &str = "orphan-option-label-debug-canary-67ac493e";
    const ORPHAN_OPTION_EXTRA_KEY: &str = "m2_5aOrphanOptionExtraKeyCanary78bd5a4f";
    const ORPHAN_OPTION_EXTRA: &str = "orphan-option-extra-debug-canary-78bd5a4f";
    const RICH_TEXT_BODY: &str = "rich-text-body-debug-canary-89ce6b50";
    const RICH_ENVELOPE_EXTRA_KEY: &str = "m2_5aRichEnvelopeExtraKeyCanary90df7c61";
    const RICH_ENVELOPE_EXTRA: &str = "rich-envelope-extra-debug-canary-90df7c61";
    const RICH_ROOT_EXTRA_KEY: &str = "m2_5aRichRootExtraKeyCanary91e08d72";
    const RICH_ROOT_EXTRA: &str = "rich-root-extra-debug-canary-91e08d72";
    const RICH_CONTAINER_EXTRA_KEY: &str = "m2_5aRichContainerExtraKeyCanary92f19e83";
    const RICH_CONTAINER_EXTRA: &str = "rich-container-extra-debug-canary-92f19e83";
    const RICH_TEXT_EXTRA_KEY: &str = "m2_5aRichTextNodeExtraKeyCanary9302af94";
    const RICH_TEXT_EXTRA: &str = "rich-text-node-extra-debug-canary-9302af94";
    const NUMBER_LEXEME: &str = "123456789012345678901234567890.1234567890123456789";
    const CREDENTIAL: &str = "credential=document-debug-canary-a1e08d72";
    const URL: &str = "https://debug.invalid/document?token=b2f19e83";
    const WINDOWS_PATH: &str = "C:\\Users\\debug-canary\\document-c3a20f94.json";
    const UNIX_PATH: &str = "/home/debug-canary/document-d4b310a5.json";

    let mut value = document_value();
    value["name"] = json!(DOCUMENT_NAME);
    let artifact = decode_document(&deterministic(&value))?;
    assert_debug_redacted(&artifact, "DocumentArtifact", &[DOCUMENT_NAME, DOCUMENT_ID]);

    let mut value = document_value();
    value[DOCUMENT_EXTRA_KEY] = json!({
        "sentinel": DOCUMENT_EXTRA,
        "precision": serde_json::from_str::<Value>(NUMBER_LEXEME)?,
        "credential": CREDENTIAL,
        "url": URL,
        "windows": WINDOWS_PATH,
        "unix": UNIX_PATH
    });
    assert_json_object_has_key(&value, DOCUMENT_EXTRA_KEY);
    let artifact = decode_document(&deterministic(&value))?;
    let aggregate_debug = assert_debug_redacted(
        &artifact,
        "DocumentArtifact",
        &[
            DOCUMENT_EXTRA_KEY,
            DOCUMENT_EXTRA,
            NUMBER_LEXEME,
            CREDENTIAL,
            URL,
            WINDOWS_PATH,
            UNIX_PATH,
            DOCUMENT_ID,
            TEMPLATE_ID,
        ],
    );
    assert!(aggregate_debug.contains("field_value_count: 4"));
    assert!(aggregate_debug.contains("orphaned_field_definition_count: 2"));
    assert!(aggregate_debug.contains("extra_count: 1"));

    let mut value = document_value();
    value["fieldValues"][FIELD_TEXT] =
        json!({"kind":"text","value":VALUE_PAYLOAD,VALUE_EXTRA_KEY:VALUE_EXTRA});
    assert_json_object_has_key(&value["fieldValues"][FIELD_TEXT], VALUE_EXTRA_KEY);
    let artifact = decode_document(&deterministic(&value))?;
    let field_value = artifact
        .field_values()
        .get(&FieldId::from_str(FIELD_TEXT)?)
        .unwrap();
    assert_debug_redacted(
        field_value,
        "FieldValue",
        &[VALUE_PAYLOAD, VALUE_EXTRA_KEY, VALUE_EXTRA],
    );
    assert_debug_redacted(
        &artifact,
        "DocumentArtifact",
        &[VALUE_PAYLOAD, VALUE_EXTRA_KEY, VALUE_EXTRA],
    );

    let mut value = document_value();
    value["orphanedFieldDefinitions"][FIELD_ARCHIVED]["label"] = json!(ORPHAN_FIELD_LABEL);
    let artifact = decode_document(&deterministic(&value))?;
    let orphan = artifact
        .orphaned_field_definitions()
        .get(&FieldId::from_str(FIELD_ARCHIVED)?)
        .unwrap();
    assert_debug_redacted(orphan, "OrphanedFieldDefinition", &[ORPHAN_FIELD_LABEL]);
    assert_debug_redacted(&artifact, "DocumentArtifact", &[ORPHAN_FIELD_LABEL]);

    let mut value = document_value();
    value["orphanedFieldDefinitions"][FIELD_ARCHIVED][ORPHAN_FIELD_EXTRA_KEY] =
        json!(ORPHAN_FIELD_EXTRA);
    assert_json_object_has_key(
        &value["orphanedFieldDefinitions"][FIELD_ARCHIVED],
        ORPHAN_FIELD_EXTRA_KEY,
    );
    let artifact = decode_document(&deterministic(&value))?;
    let orphan = artifact
        .orphaned_field_definitions()
        .get(&FieldId::from_str(FIELD_ARCHIVED)?)
        .unwrap();
    let orphan_debug = assert_debug_redacted(
        orphan,
        "OrphanedFieldDefinition",
        &[ORPHAN_FIELD_EXTRA_KEY, ORPHAN_FIELD_EXTRA],
    );
    assert!(orphan_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "DocumentArtifact",
        &[ORPHAN_FIELD_EXTRA_KEY, ORPHAN_FIELD_EXTRA],
    );

    let orphan_field_id = FieldId::from_str(FIELD_ORPHAN_CHOICE)?;
    let orphan_option_id = OptionId::from_str(OPTION_ARCHIVED)?;
    let mut value = document_value();
    value["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]["label"] =
        json!(ORPHAN_OPTION_LABEL);
    let artifact = decode_document(&deterministic(&value))?;
    let orphan_option = artifact.orphaned_field_definitions()[&orphan_field_id].options()
        [&orphan_option_id]
        .clone();
    assert_debug_redacted(
        &orphan_option,
        "OrphanedOptionDefinition",
        &[ORPHAN_OPTION_LABEL, OPTION_ARCHIVED],
    );
    assert_debug_redacted(&artifact, "DocumentArtifact", &[ORPHAN_OPTION_LABEL]);

    let mut value = document_value();
    value["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED]
        [ORPHAN_OPTION_EXTRA_KEY] = json!(ORPHAN_OPTION_EXTRA);
    assert_json_object_has_key(
        &value["orphanedFieldDefinitions"][FIELD_ORPHAN_CHOICE]["options"][OPTION_ARCHIVED],
        ORPHAN_OPTION_EXTRA_KEY,
    );
    let artifact = decode_document(&deterministic(&value))?;
    let orphan_option =
        &artifact.orphaned_field_definitions()[&orphan_field_id].options()[&orphan_option_id];
    let option_debug = assert_debug_redacted(
        orphan_option,
        "OrphanedOptionDefinition",
        &[ORPHAN_OPTION_EXTRA_KEY, ORPHAN_OPTION_EXTRA],
    );
    assert!(option_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "DocumentArtifact",
        &[ORPHAN_OPTION_EXTRA_KEY, ORPHAN_OPTION_EXTRA],
    );

    let mut value = document_value();
    value["fieldValues"][FIELD_ARCHIVED] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            RICH_ENVELOPE_EXTRA_KEY:RICH_ENVELOPE_EXTRA,
            "content":{
                "kind":"root",
                RICH_ROOT_EXTRA_KEY:RICH_ROOT_EXTRA,
                "children":[{"kind":"paragraph",RICH_CONTAINER_EXTRA_KEY:RICH_CONTAINER_EXTRA,"children":[{
                    "kind":"text","text":RICH_TEXT_BODY,RICH_TEXT_EXTRA_KEY:RICH_TEXT_EXTRA
                }]}]
            }
        }
    });
    let rich_document = &value["fieldValues"][FIELD_ARCHIVED]["document"];
    assert_json_object_has_key(rich_document, RICH_ENVELOPE_EXTRA_KEY);
    assert_json_object_has_key(&rich_document["content"], RICH_ROOT_EXTRA_KEY);
    assert_json_object_has_key(
        &rich_document["content"]["children"][0],
        RICH_CONTAINER_EXTRA_KEY,
    );
    assert_json_object_has_key(
        &rich_document["content"]["children"][0]["children"][0],
        RICH_TEXT_EXTRA_KEY,
    );
    let artifact = decode_document(&deterministic(&value))?;
    let field_value = &artifact.field_values()[&FieldId::from_str(FIELD_ARCHIVED)?];
    let rich_text = field_value.rich_text().unwrap();
    assert_debug_redacted(
        rich_text,
        "RichTextDocument",
        &[
            RICH_TEXT_BODY,
            RICH_ENVELOPE_EXTRA_KEY,
            RICH_ENVELOPE_EXTRA,
            RICH_ROOT_EXTRA_KEY,
            RICH_ROOT_EXTRA,
            RICH_CONTAINER_EXTRA_KEY,
            RICH_CONTAINER_EXTRA,
            RICH_TEXT_EXTRA_KEY,
            RICH_TEXT_EXTRA,
        ],
    );
    assert_debug_redacted(
        field_value,
        "FieldValue",
        &[
            RICH_TEXT_BODY,
            RICH_ENVELOPE_EXTRA_KEY,
            RICH_ENVELOPE_EXTRA,
            RICH_ROOT_EXTRA_KEY,
            RICH_ROOT_EXTRA,
            RICH_CONTAINER_EXTRA_KEY,
            RICH_CONTAINER_EXTRA,
            RICH_TEXT_EXTRA_KEY,
            RICH_TEXT_EXTRA,
        ],
    );
    assert_debug_redacted(
        &artifact,
        "DocumentArtifact",
        &[
            RICH_TEXT_BODY,
            RICH_ENVELOPE_EXTRA_KEY,
            RICH_ENVELOPE_EXTRA,
            RICH_ROOT_EXTRA_KEY,
            RICH_ROOT_EXTRA,
            RICH_CONTAINER_EXTRA_KEY,
            RICH_CONTAINER_EXTRA,
            RICH_TEXT_EXTRA_KEY,
            RICH_TEXT_EXTRA,
        ],
    );
    Ok(())
}

#[test]
fn document_debug_redacts_independent_windows_and_unix_path_extra_values(
) -> Result<(), Box<dyn Error>> {
    const WINDOWS_EXTRA_KEY: &str = "m2_5aWindowsPathExtraKeyCanarya4b5c6d7";
    const WINDOWS_PATH: &str = "C:\\M2_5A_DEBUG_REDACTION_b5c6d7e8\\secret.txt";
    const WINDOWS_COMPONENT: &str = "M2_5A_DEBUG_REDACTION_b5c6d7e8";
    const UNIX_EXTRA_KEY: &str = "m2_5aUnixPathExtraKeyCanaryc6d7e8f9";
    const UNIX_PATH: &str = "/tmp/M2_5A_DEBUG_REDACTION_d7e8f901/secret.txt";
    const UNIX_COMPONENT: &str = "M2_5A_DEBUG_REDACTION_d7e8f901";

    let field_id = FieldId::from_str(FIELD_TEXT)?;
    let escaped_windows_path = WINDOWS_PATH.escape_debug().to_string();

    let mut windows_value = document_value();
    windows_value["fieldValues"][FIELD_TEXT][WINDOWS_EXTRA_KEY] = json!(WINDOWS_PATH);
    assert_json_object_has_key(&windows_value["fieldValues"][FIELD_TEXT], WINDOWS_EXTRA_KEY);
    assert_eq!(
        windows_value["fieldValues"][FIELD_TEXT][WINDOWS_EXTRA_KEY],
        json!(WINDOWS_PATH)
    );
    let windows_artifact = decode_document(&deterministic(&windows_value))?;
    let windows_fragments = [
        WINDOWS_EXTRA_KEY,
        WINDOWS_PATH,
        escaped_windows_path.as_str(),
        WINDOWS_COMPONENT,
    ];
    assert_debug_redacted(
        &windows_artifact.field_values()[&field_id],
        "FieldValue",
        &windows_fragments,
    );
    assert_debug_redacted(&windows_artifact, "DocumentArtifact", &windows_fragments);

    let mut unix_value = document_value();
    unix_value["fieldValues"][FIELD_TEXT][UNIX_EXTRA_KEY] = json!(UNIX_PATH);
    assert_json_object_has_key(&unix_value["fieldValues"][FIELD_TEXT], UNIX_EXTRA_KEY);
    assert_eq!(
        unix_value["fieldValues"][FIELD_TEXT][UNIX_EXTRA_KEY],
        json!(UNIX_PATH)
    );
    let unix_artifact = decode_document(&deterministic(&unix_value))?;
    let unix_fragments = [UNIX_EXTRA_KEY, UNIX_PATH, UNIX_COMPONENT];
    assert_debug_redacted(
        &unix_artifact.field_values()[&field_id],
        "FieldValue",
        &unix_fragments,
    );
    assert_debug_redacted(&unix_artifact, "DocumentArtifact", &unix_fragments);
    Ok(())
}

#[test]
fn template_definition_debug_redacts_each_independent_storage_layer() -> Result<(), Box<dyn Error>>
{
    const TEMPLATE_NAME: &str = "template-name-debug-canary-e5c421b6";
    const TEMPLATE_EXTRA_KEY: &str = "m2_5aTemplateRootExtraKeyCanaryf6d532c7";
    const TEMPLATE_EXTRA: &str = "template-extra-debug-canary-f6d532c7";
    const FIELD_LABEL: &str = "field-label-debug-canary-07e643d8";
    const FIELD_EXTRA_KEY: &str = "m2_5aFieldDefinitionExtraKeyCanary18f754e9";
    const FIELD_EXTRA: &str = "field-extra-debug-canary-18f754e9";
    const OPTION_LABEL: &str = "option-label-debug-canary-290865fa";
    const OPTION_EXTRA_KEY: &str = "m2_5aChoiceOptionExtraKeyCanary3a19760b";
    const OPTION_EXTRA: &str = "option-extra-debug-canary-3a19760b";
    const PRESENTATION_TOKEN: &str = "presentation-token-debug-canary-4b2a871c";
    const PRESENTATION_EXTRA_KEY: &str = "m2_5aPresentationExtraKeyCanary5c3b982d";
    const PRESENTATION_EXTRA: &str = "presentation-extra-debug-canary-5c3b982d";
    const CONFIGURATION_EXTRA_KEY: &str = "m2_5aFieldConfigurationExtraKeyCanary6d4ca93e";
    const CONFIGURATION_EXTRA: &str = "configuration-extra-debug-canary-6d4ca93e";

    let mut value = template_value();
    value["name"] = json!(TEMPLATE_NAME);
    let artifact = decode_template(&deterministic(&value))?;
    assert_debug_redacted(&artifact, "TemplateArtifact", &[TEMPLATE_NAME, TEMPLATE_ID]);

    let mut value = template_value();
    value[TEMPLATE_EXTRA_KEY] = json!(TEMPLATE_EXTRA);
    assert_json_object_has_key(&value, TEMPLATE_EXTRA_KEY);
    let artifact = decode_template(&deterministic(&value))?;
    let aggregate_debug = assert_debug_redacted(
        &artifact,
        "TemplateArtifact",
        &[TEMPLATE_EXTRA_KEY, TEMPLATE_EXTRA, TEMPLATE_ID],
    );
    assert!(aggregate_debug.contains("field_count: 3"));
    assert!(aggregate_debug.contains("active_field_count: 2"));
    assert!(aggregate_debug.contains("archived_field_count: 1"));
    assert!(aggregate_debug.contains("extra_count: 1"));

    let field_id = FieldId::from_str(FIELD_CHOICE)?;
    let option_id = OptionId::from_str(OPTION_ACTIVE)?;

    let mut value = template_value();
    value["fields"][FIELD_CHOICE]["label"] = json!(FIELD_LABEL);
    let artifact = decode_template(&deterministic(&value))?;
    let field = &artifact.fields()[&field_id];
    assert_debug_redacted(field, "FieldDefinition", &[FIELD_LABEL]);
    assert_debug_redacted(&artifact, "TemplateArtifact", &[FIELD_LABEL]);

    let mut value = template_value();
    value["fields"][FIELD_CHOICE][FIELD_EXTRA_KEY] = json!(FIELD_EXTRA);
    assert_json_object_has_key(&value["fields"][FIELD_CHOICE], FIELD_EXTRA_KEY);
    let artifact = decode_template(&deterministic(&value))?;
    let field = &artifact.fields()[&field_id];
    let field_debug =
        assert_debug_redacted(field, "FieldDefinition", &[FIELD_EXTRA_KEY, FIELD_EXTRA]);
    assert!(field_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "TemplateArtifact",
        &[FIELD_EXTRA_KEY, FIELD_EXTRA],
    );

    let mut value = template_value();
    value["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE]["label"] =
        json!(OPTION_LABEL);
    let artifact = decode_template(&deterministic(&value))?;
    let option = &artifact.fields()[&field_id]
        .configuration()
        .options()
        .unwrap()[&option_id];
    assert_debug_redacted(option, "ChoiceOption", &[OPTION_LABEL, OPTION_ACTIVE]);
    assert_debug_redacted(&artifact, "TemplateArtifact", &[OPTION_LABEL]);

    let mut value = template_value();
    value["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE][OPTION_EXTRA_KEY] =
        json!(OPTION_EXTRA);
    assert_json_object_has_key(
        &value["fields"][FIELD_CHOICE]["configuration"]["options"][OPTION_ACTIVE],
        OPTION_EXTRA_KEY,
    );
    let artifact = decode_template(&deterministic(&value))?;
    let option = &artifact.fields()[&field_id]
        .configuration()
        .options()
        .unwrap()[&option_id];
    let option_debug =
        assert_debug_redacted(option, "ChoiceOption", &[OPTION_EXTRA_KEY, OPTION_EXTRA]);
    assert!(option_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "TemplateArtifact",
        &[OPTION_EXTRA_KEY, OPTION_EXTRA],
    );

    let mut value = template_value();
    value["fields"][FIELD_CHOICE]["presentation"]["token"] = json!(PRESENTATION_TOKEN);
    value["fields"][FIELD_CHOICE]["presentation"][PRESENTATION_EXTRA_KEY] =
        json!(PRESENTATION_EXTRA);
    assert_json_object_has_key(
        &value["fields"][FIELD_CHOICE]["presentation"],
        PRESENTATION_EXTRA_KEY,
    );
    let artifact = decode_template(&deterministic(&value))?;
    let presentation = artifact.fields()[&field_id].presentation();
    let presentation_debug = assert_debug_redacted(
        presentation,
        "Presentation",
        &[
            PRESENTATION_TOKEN,
            PRESENTATION_EXTRA_KEY,
            PRESENTATION_EXTRA,
        ],
    );
    assert!(presentation_debug.contains("has_token: true"));
    assert!(presentation_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "TemplateArtifact",
        &[
            PRESENTATION_TOKEN,
            PRESENTATION_EXTRA_KEY,
            PRESENTATION_EXTRA,
        ],
    );

    let mut value = template_value();
    value["fields"][FIELD_CHOICE]["configuration"][CONFIGURATION_EXTRA_KEY] =
        json!(CONFIGURATION_EXTRA);
    assert_json_object_has_key(
        &value["fields"][FIELD_CHOICE]["configuration"],
        CONFIGURATION_EXTRA_KEY,
    );
    let artifact = decode_template(&deterministic(&value))?;
    let configuration = artifact.fields()[&field_id].configuration();
    let configuration_debug = assert_debug_redacted(
        configuration,
        "FieldConfiguration",
        &[
            CONFIGURATION_EXTRA_KEY,
            CONFIGURATION_EXTRA,
            OPTION_ACTIVE,
            OPTION_ARCHIVED,
        ],
    );
    assert!(configuration_debug.contains("option_count: 2"));
    assert!(configuration_debug.contains("active_option_count: 1"));
    assert!(configuration_debug.contains("archived_option_count: 1"));
    assert!(configuration_debug.contains("extra_count: 1"));
    assert_debug_redacted(
        &artifact,
        "TemplateArtifact",
        &[CONFIGURATION_EXTRA_KEY, CONFIGURATION_EXTRA],
    );
    Ok(())
}

#[test]
fn document_storage_admission_preserves_arbitrary_precision_at_every_unknown_layer(
) -> Result<(), Box<dyn Error>> {
    let numbers = [
        "101010101010101010101010101010101010101",
        "2.02020202020202020202020202020202020202e+120",
        "-0.00000000000000000000000000000000030303",
        "404040404040404040404040404040404040404",
        "5.05050505050505050505050505050505050505e-120",
        "606060606060606060606060606060606060606",
    ];
    let parsed = numbers
        .iter()
        .map(|number| serde_json::from_str::<Value>(number))
        .collect::<Result<Vec<_>, _>>()?;
    let mut value = document_value();
    value["futureRootPrecision"] = parsed[0].clone();
    value["orphanedFieldDefinitions"][FIELD_ARCHIVED]["futurePrecision"] = parsed[1].clone();
    value["fieldValues"][FIELD_TEXT]["futurePrecision"] = parsed[2].clone();
    value["fieldValues"][FIELD_ARCHIVED] = json!({
        "kind":"richText",
        "document":{
            "schemaVersion":1,
            "futureEnvelopePrecision":parsed[3].clone(),
            "content":{
                "kind":"root",
                "futureRootPrecision":parsed[4].clone(),
                "children":[{"kind":"paragraph","children":[{
                    "kind":"text","text":"precision","futureNodePrecision":parsed[5].clone()
                }]}]
            }
        }
    });
    let raw = deterministic(&value);
    let artifact = decode_document(&raw)?;
    let source_before = artifact.clone();
    artifact.validate_storage()?;
    assert_eq!(artifact, source_before);
    let first = encode_document(&artifact)?;
    let encoded = std::str::from_utf8(&first)?;
    for number in numbers {
        assert!(encoded.contains(number), "number lexeme changed: {number}");
    }
    let decoded = decode_document(&first)?;
    decoded.validate_storage()?;
    assert_eq!(encode_document(&decoded)?, first);
    Ok(())
}

#[test]
fn codec_rejects_encoding_bom_trailing_garbage_and_wrong_json_types() {
    assert_codec_category(
        decode_template(&[0xff]),
        ArtifactCodecErrorCategory::InvalidEncoding,
    );
    assert_codec_category(
        decode_template(b"\xef\xbb\xbf{}"),
        ArtifactCodecErrorCategory::InvalidEncoding,
    );
    assert_codec_category(
        decode_template(br#"{} trailing"#),
        ArtifactCodecErrorCategory::MalformedJson,
    );
    assert_codec_category(
        inspect_artifact_header(br#"[]"#),
        ArtifactCodecErrorCategory::RootTypeMismatch,
    );

    let mut wrong = template_value();
    wrong["fieldOrder"] = json!("not-an-array");
    assert_codec_category(
        decode_template(&deterministic(&wrong)),
        ArtifactCodecErrorCategory::InvalidStructure,
    );
}

#[test]
fn timestamp_and_unknown_discriminator_corruption_fail_closed() {
    for (created, updated) in [
        ("2026-09-03T01:02:03Z", "2026-09-03T02:03:04.005Z"),
        ("2026-09-03T03:02:03.004Z", "2026-09-03T02:03:04.005Z"),
        ("2026-09-03T01:02:03.004+00:00", "2026-09-03T02:03:04.005Z"),
    ] {
        let mut template = template_value();
        template["createdAtUtc"] = json!(created);
        template["updatedAtUtc"] = json!(updated);
        assert_codec_category(
            decode_template(&deterministic(&template)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }

    for pointer in [
        "/lifecycle",
        "/fields/22222222-2222-4222-8222-222222222222/kind",
        "/fields/22222222-2222-4222-8222-222222222222/configuration/kind",
    ] {
        let mut template = template_value();
        *template
            .pointer_mut(pointer)
            .expect("fixture pointer should exist") = json!("futureDiscriminator");
        assert_codec_category(
            decode_template(&deterministic(&template)),
            ArtifactCodecErrorCategory::InvalidStructure,
        );
    }
}

#[test]
fn deterministic_encoding_sorts_maps_preserves_arrays_and_matches_golden(
) -> Result<(), Box<dyn Error>> {
    let template = decode_template(&deterministic(&template_value()))?;
    let first = encode_template(&template)?;
    let second = encode_template(&template)?;
    assert_eq!(first, second);
    assert!(!first.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(first.ends_with(b"\n"));
    assert!(!first.windows(2).any(|window| window == b"\r\n"));

    let encoded_value: Value = serde_json::from_slice(&first)?;
    assert_eq!(
        encoded_value["fieldOrder"],
        json!([FIELD_TEXT, FIELD_CHOICE])
    );

    let minimal_template = minimal_template_value();
    let encoded_template = encode_template(&decode_template(&deterministic(&minimal_template))?)?;
    assert_eq!(encoded_template, deterministic(&minimal_template));

    let minimal = json!({
        "artifactType": "document",
        "createdAtUtc": "2026-09-03T01:02:03.004Z",
        "documentId": DOCUMENT_ID,
        "fieldValues": {},
        "name": "세계",
        "orphanedFieldDefinitions": {},
        "schemaVersion": 1,
        "templateId": TEMPLATE_ID,
        "templateRevision": 1,
        "updatedAtUtc": "2026-09-03T01:02:03.004Z"
    });
    let encoded = encode_document(&decode_document(&deterministic(&minimal))?)?;
    let golden = concat!(
        "{\n",
        "  \"artifactType\": \"document\",\n",
        "  \"createdAtUtc\": \"2026-09-03T01:02:03.004Z\",\n",
        "  \"documentId\": \"66666666-6666-4666-8666-666666666666\",\n",
        "  \"fieldValues\": {},\n",
        "  \"name\": \"세계\",\n",
        "  \"orphanedFieldDefinitions\": {},\n",
        "  \"schemaVersion\": 1,\n",
        "  \"templateId\": \"11111111-1111-4111-8111-111111111111\",\n",
        "  \"templateRevision\": 1,\n",
        "  \"updatedAtUtc\": \"2026-09-03T01:02:03.004Z\"\n",
        "}\n"
    );
    assert_eq!(encoded, golden.as_bytes());
    Ok(())
}

#[test]
fn diagnostics_and_source_chain_never_echo_input_values_or_paths() {
    const WINDOWS_PATH: &str = "C:\\Users\\audit\\private-artifact.json";
    const UNIX_PATH: &str = "/home/audit/private-artifact.json";
    const LABEL: &str = "private-field-label";
    const OPTION_LABEL: &str = "private-option-label";
    const DISCRIMINATOR: &str = "attacker-controlled-discriminator";
    const DUPLICATE_KEY: &str = "attacker-controlled-duplicate-key";
    const RESERVED_KEY: &str = "$serde_json::private::Number";
    let forbidden = [
        SECRET_BODY,
        "credential=",
        WINDOWS_PATH,
        UNIX_PATH,
        LABEL,
        OPTION_LABEL,
        DISCRIMINATOR,
        DUPLICATE_KEY,
        RESERVED_KEY,
    ];
    let inputs = [
        format!(
            r#"{{"artifactType":"document","schemaVersion":1,"fieldValues":{{"x":{{"kind":"{SECRET_BODY}"}}}},"path":"{WINDOWS_PATH}","unix":"{UNIX_PATH}","label":"{LABEL}","optionLabel":"{OPTION_LABEL}"}}"#
        ),
        format!(
            r#"{{"artifactType":"document","schemaVersion":1,"nested":{{"{DUPLICATE_KEY}":1,"{DUPLICATE_KEY}":2}},"credential":"{SECRET_BODY}"}}"#
        ),
        format!(
            r#"{{"artifactType":"{DISCRIMINATOR}","schemaVersion":1,"label":"{LABEL}","optionLabel":"{OPTION_LABEL}","path":"{UNIX_PATH}"}}"#
        ),
        format!(
            r#"{{"artifactType":"document","schemaVersion":1,"unterminated":"{SECRET_BODY} {WINDOWS_PATH} {UNIX_PATH}""#
        ),
        format!(
            r#"{{"artifactType":"document","schemaVersion":1,"future":{{"{RESERVED_KEY}":"1"}},"credential":"{SECRET_BODY}"}}"#
        ),
    ];

    for input in inputs {
        let error = decode_document(input.as_bytes()).expect_err("secret fixture should fail");
        assert_eq!(assert_error_chain_redacted(&error, &forbidden), 1);
        assert!(error.to_string().contains("artifact codec failed"));
        assert!(error.source().is_none());
    }
}

#[test]
fn document_rejects_only_top_level_tree_authority_keys() -> Result<(), Box<dyn Error>> {
    for key in super::document::FORBIDDEN_TREE_KEYS {
        let mut document = document_value();
        document
            .as_object_mut()
            .expect("document fixture is an object")
            .insert((*key).to_owned(), json!({"attacker":"controlled"}));
        let error = decode_document(&deterministic(&document))
            .expect_err("tree placement authority key must fail closed");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::ForbiddenTreeKey),
            "{key}"
        );
    }

    let mut allowed = document_value();
    allowed["futureDocument"] = json!({
        "parentId":"nested is not placement authority",
        "parentDocumentId":"nested",
        "treeOrder":[3, 1, 2],
        "children":[{"parentId":"still nested"}]
    });
    let first = encode_document(&decode_document(&deterministic(&allowed))?)?;
    let round_trip: Value = serde_json::from_slice(&first)?;
    assert_eq!(round_trip["futureDocument"], allowed["futureDocument"]);
    assert_eq!(encode_document(&decode_document(&first)?)?, first);
    Ok(())
}

#[test]
fn document_optional_term_info_uses_the_common_single_line_codec_contract() {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    for field in ["englishName", "glossarySummary"] {
        for separator in [
            '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            let mut document = document_value();
            document[field] = json!(format!("앞{separator}뒤"));
            let error = decode_document(&deterministic(&document))
                .expect_err("hard line separators must fail artifact admission");
            assert_eq!(
                error.validation_category(),
                Some(ArtifactValidationErrorCategory::InvalidScalarValue),
                "{field} U+{:04X}",
                separator as u32
            );
            assert_eq!(
                error.scalar_category(),
                Some(ScalarValueErrorCategory::MultilineSingleLineText)
            );
            assert_eq!(
                error.scalar_location(),
                Some(ArtifactScalarValueLocation::DocumentField)
            );
        }
    }

    let mut allowed = document_value();
    allowed["englishName"] = json!("  한글\t🙂  ");
    allowed["glossarySummary"] = json!("");
    let decoded = decode_document(&deterministic(&allowed))
        .expect("empty and whitespace-preserving values are valid");
    assert_eq!(decoded.english_name(), "  한글\t🙂  ");
    assert_eq!(decoded.glossary_summary(), "");
}

#[test]
fn encode_revalidates_models_before_producing_persistent_bytes() {
    let mut artifact = decode_template(&deterministic(&minimal_template_value()))
        .expect("fixture should create a validated model");
    artifact.corrupt_field_order_for_test();
    let error = encode_template(&artifact).expect_err("invalid model must not become bytes");
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::InvalidStructure
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );

    let mut artifact = decode_template(&deterministic(&minimal_template_value()))
        .expect("fixture should create a validated model");
    artifact.corrupt_reserved_extra_for_test();
    let error = encode_template(&artifact).expect_err("reserved key must not become bytes");
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::ReservedExtraKey)
    );
}

#[test]
fn document_storage_admission_and_encode_share_scalar_categories_and_locations(
) -> Result<(), Box<dyn Error>> {
    use crate::data::field_engine::scalar::ScalarValueErrorCategory;

    let cases = [
        (FIELD_TEXT, ArtifactScalarValueLocation::DocumentField),
        (FIELD_ARCHIVED, ArtifactScalarValueLocation::OrphanField),
    ];
    for (field_id, expected_location) in cases {
        let mut value = document_value();
        if field_id == FIELD_ARCHIVED {
            value["fieldValues"][FIELD_ARCHIVED] = json!({"kind":"text","value":"valid"});
            value["orphanedFieldDefinitions"][FIELD_ARCHIVED]["kind"] = json!("singleLineText");
        }
        let mut artifact = decode_document(&deterministic(&value))?;
        let field_id = FieldId::from_str(field_id)?;
        const SECRET: &str = "line-one\ncredential=storage-scalar-canary-4e293b16";
        artifact.corrupt_scalar_value_for_test(field_id, SECRET);
        let source_before = artifact.clone();

        let validation = artifact
            .validate_storage()
            .expect_err("invalid scalar must fail common storage admission");
        assert_eq!(
            validation.category(),
            ArtifactValidationErrorCategory::InvalidScalarValue
        );
        assert_eq!(
            validation.scalar_category(),
            Some(ScalarValueErrorCategory::MultilineSingleLineText)
        );
        assert_eq!(validation.scalar_location(), Some(expected_location));
        assert_eq!(
            assert_error_chain_redacted(&validation, &[SECRET, "line-one"]),
            1
        );
        assert_eq!(artifact, source_before);

        let codec = encode_document(&artifact).expect_err("invalid scalar must not produce bytes");
        assert_eq!(
            codec.validation_category(),
            Some(ArtifactValidationErrorCategory::InvalidScalarValue)
        );
        assert_eq!(
            codec.scalar_category(),
            Some(ScalarValueErrorCategory::MultilineSingleLineText)
        );
        assert_eq!(codec.scalar_location(), Some(expected_location));
        assert_eq!(
            assert_error_chain_redacted(&codec, &[SECRET, "line-one"]),
            1
        );
        assert_eq!(artifact, source_before);
    }
    Ok(())
}

#[test]
fn document_storage_admission_rejects_reserved_root_markers_without_bytes(
) -> Result<(), Box<dyn Error>> {
    for marker in [
        "$serde_json::private::Number",
        "$serde_json::private::RawValue",
        "$serde_json::private::FutureTransport",
    ] {
        let mut artifact = decode_document(&deterministic(&document_value()))?;
        artifact.corrupt_extra_for_test(
            marker,
            json!({"credential":"reserved-storage-canary-5f3a4c27"}),
        );
        let source_before = artifact.clone();
        let validation = artifact
            .validate_storage()
            .expect_err("reserved marker must fail storage admission");
        assert_eq!(
            validation.category(),
            ArtifactValidationErrorCategory::ReservedExtraKey
        );
        assert_eq!(
            assert_error_chain_redacted(&validation, &[marker, "reserved-storage-canary-5f3a4c27"]),
            1
        );
        assert_eq!(artifact, source_before);

        let codec = encode_document(&artifact)
            .expect_err("reserved marker must not produce persistent bytes");
        assert_eq!(
            codec.validation_category(),
            Some(ArtifactValidationErrorCategory::ReservedExtraKey)
        );
        assert_eq!(
            assert_error_chain_redacted(&codec, &[marker, "reserved-storage-canary-5f3a4c27"]),
            1
        );
        assert_eq!(artifact, source_before);
    }

    let mut allowed = document_value();
    allowed["markerAsString"] = json!("$serde_json::private::RawValue");
    let allowed = decode_document(&deterministic(&allowed))?;
    allowed.validate_storage()?;
    let bytes = encode_document(&allowed)?;
    assert_eq!(encode_document(&decode_document(&bytes)?)?, bytes);
    Ok(())
}

#[test]
fn encode_rejects_programmatically_corrupted_rich_text_content_without_bytes() {
    let field_id = FieldId::from_str(FIELD_ARCHIVED).expect("fixture FieldId should be valid");
    let reserved_keys = [
        "$serde_json::private::Number",
        "$serde_json::private::RawValue",
        "$serde_json::private::FutureTransport",
    ];

    for reserved_key in reserved_keys {
        let mut document = document_value();
        document["fieldValues"][FIELD_ARCHIVED] = json!({
            "kind": "richText",
            "document": {"schemaVersion": 1, "content": rich_text_content("본문")}
        });
        let mut artifact = decode_document(&deterministic(&document))
            .expect("fixture should create a validated rich-text document");
        artifact.corrupt_rich_text_content_for_test(field_id, reserved_key);
        let error = encode_document(&artifact)
            .expect_err("corrupted rich-text content must not produce document bytes");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::ReservedExtraKey)
        );
        assert_error_chain_redacted(&error, &[reserved_key, "test-only payload"]);
    }

    for (reserved_key, initial) in [
        ("$serde_json::private::Number", false),
        ("$serde_json::private::RawValue", true),
        ("$serde_json::private::FutureTransport", false),
        ("$serde_json::private::FutureTransport", true),
    ] {
        let mut template = template_value();
        template["fields"][FIELD_ARCHIVED]["defaultValue"] = json!({
            "kind": "richText",
            "document": {"schemaVersion": 1, "content": rich_text_content("현재")}
        });
        template["fields"][FIELD_ARCHIVED]["initialDefaultValue"] = json!({
            "kind": "richText",
            "document": {"schemaVersion": 1, "content": rich_text_content("초기")}
        });
        let mut artifact = decode_template(&deterministic(&template))
            .expect("fixture should create validated rich-text defaults");
        artifact.corrupt_rich_text_default_for_test(field_id, reserved_key, initial);
        let error = encode_template(&artifact)
            .expect_err("corrupted rich-text default must not produce template bytes");
        assert_eq!(
            error.validation_category(),
            Some(ArtifactValidationErrorCategory::ReservedExtraKey)
        );
        assert_error_chain_redacted(&error, &[reserved_key, "test-only payload"]);
    }
}

fn assert_artifact_encode_depth_error(error: ArtifactCodecError) {
    assert_eq!(
        error.category(),
        ArtifactCodecErrorCategory::InvalidStructure
    );
    assert_eq!(error.stage(), ArtifactCodecStage::StructuralValidation);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::JsonNestingDepthExceeded)
    );
    assert_error_chain_redacted(
        &error,
        &["credential=artifact-depth-secret", "testOnlyDepth"],
    );
}

#[test]
fn document_encode_uses_the_full_artifact_root_depth_for_rich_text() -> Result<(), Box<dyn Error>> {
    const DOCUMENT_CONTENT_WRAPPER_DEPTH: usize = 5;
    let nested_depth = crate::data::json::MAX_JSON_NESTING_DEPTH - DOCUMENT_CONTENT_WRAPPER_DEPTH;
    let boundary_value = document_with_rich_text_content(nested_array_value(
        nested_depth,
        Value::String("boundary".to_owned()),
    ));
    assert_eq!(
        maximum_container_depth(&boundary_value),
        crate::data::json::MAX_JSON_NESTING_DEPTH
    );
    let boundary = decode_document(&serde_json::to_vec(&boundary_value)?)?;
    let boundary_before = boundary.clone();
    boundary.validate_storage()?;
    assert_eq!(boundary, boundary_before);
    let encoded = encode_document(&boundary)?;
    let decoded = decode_document(&encoded).expect("depth 127 output must remain strict-decodable");
    decoded.validate_storage()?;
    assert_eq!(encode_document(&decoded)?, encoded);

    let shallow = document_with_rich_text_content(Value::Null);
    let mut too_deep = decode_document(&deterministic(&shallow))?;
    too_deep.corrupt_rich_text_depth_for_test(
        FieldId::from_str(FIELD_ARCHIVED)?,
        nested_array_value(
            nested_depth + 1,
            Value::String("credential=artifact-depth-secret".to_owned()),
        ),
    );
    assert_eq!(
        maximum_container_depth(&document_wire_value(&too_deep)),
        crate::data::json::MAX_JSON_NESTING_DEPTH + 1
    );
    let too_deep_before = too_deep.clone();
    let validation = too_deep
        .validate_storage()
        .expect_err("full document depth 128 must fail common storage admission");
    assert_eq!(
        validation.category(),
        ArtifactValidationErrorCategory::JsonNestingDepthExceeded
    );
    assert_eq!(
        assert_error_chain_redacted(
            &validation,
            &["credential=artifact-depth-secret", "testOnlyDepth"]
        ),
        1
    );
    assert_eq!(too_deep, too_deep_before);
    let error = encode_document(&too_deep)
        .expect_err("full document depth 128 must not produce persistent bytes");
    assert_artifact_encode_depth_error(error);
    assert_eq!(too_deep, too_deep_before);
    Ok(())
}

#[test]
fn template_encode_uses_full_root_depth_boundaries_for_current_and_initial_rich_text_defaults(
) -> Result<(), Box<dyn Error>> {
    const TEMPLATE_CONTENT_WRAPPER_DEPTH: usize = 6;
    let boundary_nested_depth =
        crate::data::json::MAX_JSON_NESTING_DEPTH - TEMPLATE_CONTENT_WRAPPER_DEPTH;
    let field_id = FieldId::from_str(FIELD_ARCHIVED)?;

    for initial in [false, true] {
        let slot = if initial {
            "initialDefaultValue"
        } else {
            "defaultValue"
        };
        let boundary_content = nested_array_value(
            boundary_nested_depth,
            Value::String("unknown rich-text boundary content".to_owned()),
        );
        let mut boundary = decode_template(&deterministic(&template_with_rich_text_defaults()))?;
        boundary.corrupt_rich_text_default_depth_for_test(
            field_id,
            boundary_content.clone(),
            initial,
        );
        let boundary_wire = template_wire_value(&boundary);
        assert_eq!(
            maximum_container_depth(&boundary_wire),
            crate::data::json::MAX_JSON_NESTING_DEPTH,
            "{slot} should make the whole template wire depth exactly 127"
        );
        let encoded = encode_template(&boundary)?;
        let decoded = decode_template(&encoded)
            .unwrap_or_else(|error| panic!("{slot} depth-127 output should decode: {error}"));
        let decoded_wire = template_wire_value(&decoded);
        assert_eq!(
            decoded_wire, boundary_wire,
            "{slot} wire meaning changed after encode/decode"
        );
        assert_eq!(
            decoded_wire["fields"][FIELD_ARCHIVED][slot]["document"]["content"]["testOnlyDepth"],
            boundary_content,
            "{slot} unknown rich-text content was not preserved"
        );
        assert_eq!(
            encode_template(&decoded)?,
            encoded,
            "{slot} bytes changed after decode/re-encode"
        );

        let mut too_deep = decode_template(&deterministic(&template_with_rich_text_defaults()))?;
        too_deep.corrupt_rich_text_default_depth_for_test(
            field_id,
            nested_array_value(
                boundary_nested_depth + 1,
                Value::String("credential=artifact-depth-secret".to_owned()),
            ),
            initial,
        );
        assert_eq!(
            maximum_container_depth(&template_wire_value(&too_deep)),
            crate::data::json::MAX_JSON_NESTING_DEPTH + 1,
            "{slot} should make the whole template wire depth exactly 128"
        );
        let error = encode_template(&too_deep)
            .expect_err("full template depth 128 must not produce persistent bytes");
        assert_artifact_encode_depth_error(error);
    }
    Ok(())
}

#[test]
fn aggregate_extra_depth_is_rejected_without_a_production_mutator() -> Result<(), Box<dyn Error>> {
    let mut artifact = decode_document(&deterministic(&document_value()))?;
    artifact.corrupt_extra_depth_for_test(nested_array_value(
        crate::data::json::MAX_JSON_NESTING_DEPTH,
        Value::String("credential=artifact-depth-secret".to_owned()),
    ));
    assert_eq!(
        maximum_container_depth(&document_wire_value(&artifact)),
        crate::data::json::MAX_JSON_NESTING_DEPTH + 1
    );
    let source_before = artifact.clone();
    let validation = artifact
        .validate_storage()
        .expect_err("whole-wire root depth 128 must fail common storage admission");
    assert_eq!(
        validation.category(),
        ArtifactValidationErrorCategory::JsonNestingDepthExceeded
    );
    assert_eq!(artifact, source_before);
    let error = encode_document(&artifact)
        .expect_err("full aggregate depth 128 must not produce persistent bytes");
    assert_artifact_encode_depth_error(error);
    assert_eq!(artifact, source_before);
    Ok(())
}
