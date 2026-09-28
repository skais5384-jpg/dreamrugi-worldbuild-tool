use std::{collections::VecDeque, error::Error, str::FromStr};

use serde_json::{json, Value};
use uuid::{Variant, Version};

use super::*;
use crate::data::artifact::{
    decode_template, encode_template,
    template_mutation::{apply_template_mutation, TemplateMutationCommand},
    FieldLifecycle, OptionLifecycle,
};
use crate::data::json::{parse_strict_lossless_json_object, LosslessJsonValue};

const NOW: &str = "2026-09-06T12:34:56.789Z";
const SECRET: &str = "credential=creation-secret path=C:\\private\\template.json";
const BIG: &str = "123456789012345678901234567890123456789012345678901234567890";
const EXP: &str = "1E+100";
const HUGE_EXP: &str = "1e99999999999999999999999999999999999999999999999999";
const DUPLICATION_REDACTION_CANARIES: &[&str] = &[
    SECRET,
    "현재",
    "초기",
    "futureOuter",
    "futureRoot",
    "optionIds",
    "templateId",
    BIG,
    HUGE_EXP,
];

fn uuid(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012x}")
}
fn field_id(n: u32) -> FieldId {
    FieldId::from_str(&uuid(n)).unwrap()
}
fn option_id(n: u32) -> OptionId {
    OptionId::from_str(&uuid(n)).unwrap()
}

struct FixedIds {
    values: VecDeque<String>,
    calls: Vec<&'static str>,
}

impl FixedIds {
    fn new(values: impl IntoIterator<Item = u32>) -> Self {
        Self {
            values: values.into_iter().map(uuid).collect(),
            calls: Vec::new(),
        }
    }

    fn complete() -> Self {
        Self::new((982..=1000).rev())
    }

    fn next<T: FromStr>(&mut self, kind: &'static str) -> Result<T, ()> {
        self.calls.push(kind);
        self.values.pop_front().ok_or(())?.parse().map_err(|_| ())
    }
}

impl TemplateIdentitySource for FixedIds {
    fn template_id(&mut self) -> Result<TemplateId, ()> {
        self.next("template")
    }
    fn field_id(&mut self) -> Result<FieldId, ()> {
        self.next("field")
    }
    fn option_id(&mut self) -> Result<OptionId, ()> {
        self.next("option")
    }
}

fn rich(text: &str) -> Value {
    json!({"kind":"richText", "futureValue": {"big":"BIG"}, "document": {
        "schemaVersion":1, "futureEnvelope":{"exponent":"EXP"}, "content": {
            "kind":"root", "futureRoot":{"negativeZero":"ZERO"}, "children":[{
                "kind":"paragraph", "futureContainer":{"huge":"HUGE"}, "children":[{
                    "kind":"text", "text":text, "futureNode":[{"uuid":uuid(100), "big":"BIG"}]
                }]
            }]
        }
    }})
}

fn fixture_value() -> Value {
    let kinds = [
        "singleLineText",
        "richText",
        "number",
        "date",
        "time",
        "duration",
        "singleChoice",
        "multiChoice",
        "singleChoice",
    ];
    let current = [
        json!({"kind":"text","value":uuid(100)}),
        rich("현재"),
        json!({"kind":"number","value":"123.45"}),
        json!({"kind":"unset","futureUnset":"ZERO"}),
        json!({"kind":"time","value":"12:34:56.789"}),
        json!({"kind":"duration","milliseconds":"123456"}),
        json!({"kind":"singleChoice","optionId":uuid(100)}),
        json!({"kind":"multiChoice","optionIds":[uuid(103),uuid(104)]}),
        json!({"kind":"singleChoice","optionId":uuid(108)}),
    ];
    let initial = [
        json!({"kind":"text","value":"이전"}),
        rich("초기"),
        json!({"kind":"number","value":"-42"}),
        json!({"kind":"date","value":"2026-09-01"}),
        json!({"kind":"time","value":"01:02:03.004"}),
        json!({"kind":"duration","milliseconds":"-1000"}),
        json!({"kind":"singleChoice","optionId":uuid(102)}),
        json!({"kind":"multiChoice","optionIds":[uuid(103),uuid(105)]}),
        json!({"kind":"singleChoice","optionId":uuid(106)}),
    ];
    let mut fields = serde_json::Map::new();
    // insertion order와 표시 order를 ID 정렬과 다르게 만든다.
    for i in (0..9).rev() {
        let mut configuration = json!({"kind":kinds[i], "futureConfiguration":{"exponent":"EXP"}});
        if i >= 6 {
            let first = 100 + (i as u32 - 6) * 3;
            configuration["optionOrder"] = json!([uuid(first + 1), uuid(first)]);
            configuration["options"] = json!({
                (uuid(first+2)): {"label":SECRET,"lifecycle":"archived","futureOption":{"big":"BIG"}},
                (uuid(first+1)): {"label":"두 번째","lifecycle":"active","futureOption":{"exponent":"EXP"}},
                (uuid(first)): {"label":uuid(10),"lifecycle":"active","futureOption":{"negativeZero":"ZERO"}}
            });
        }
        let mut current = current[i].clone();
        let mut initial = initial[i].clone();
        current["futureOuter"] = json!({"exponent":"EXP","uuid":uuid(100),"secret":SECRET});
        initial["futureOuter"] = json!({"negativeZero":"ZERO","huge":"HUGE","uuid":uuid(10)});
        fields.insert(uuid(10+i as u32), json!({
            "label":format!("{SECRET} {i}"), "lifecycle":if i == 8 {"archived"} else {"active"},
            "kind":kinds[i], "required":i % 2 == 0,
            "defaultValue":current, "initialDefaultValue":initial, "introducedRevision":i+1,
            "configuration":configuration, "presentation":{"token":uuid(100),"futurePresentation":{"big":"BIG"}},
            "futureField":{"huge":"HUGE", "fields":{(uuid(10)):uuid(100)}}
        }));
    }
    json!({"schemaVersion":1,"artifactType":"template","templateId":uuid(1),"revision":20,
        "name":SECRET,"lifecycle":"active","fields":fields,
        "fieldOrder":[uuid(17),uuid(13),uuid(10),uuid(16),uuid(15),uuid(12),uuid(11),uuid(14)],
        "presentation":{"token":uuid(1),"futurePresentation":{"exponent":"EXP"}},
        "createdAtUtc":"2026-01-01T00:00:00.000Z","updatedAtUtc":"2026-09-01T00:00:00.000Z",
        "futureRoot":{"big":"BIG","exponent":"EXP","negativeZero":"ZERO","huge":"HUGE",
            "uuid":uuid(1),"fields":{(uuid(10)):{"optionId":uuid(100)}},"secret":SECRET}
    })
}

fn fixture_from_value(value: &Value) -> TemplateArtifact {
    let raw = serde_json::to_string(value)
        .unwrap()
        .replace("\"BIG\"", BIG)
        .replace("\"EXP\"", EXP)
        .replace("\"ZERO\"", "-0")
        .replace("\"HUGE\"", HUGE_EXP);
    decode_template(raw.as_bytes()).unwrap()
}

fn fixture() -> TemplateArtifact {
    fixture_from_value(&fixture_value())
}

fn duplicate(source: &TemplateArtifact) -> TemplateArtifact {
    duplicate_template_with_ids(source, NOW.to_owned(), &mut FixedIds::complete()).unwrap()
}

fn round_trip(value: &TemplateArtifact) -> Vec<u8> {
    value.validate_storage().unwrap();
    let bytes = encode_template(value).unwrap();
    let decoded = decode_template(&bytes).unwrap();
    assert!(value == &decoded, "round-trip aggregate mismatch");
    assert!(
        bytes == encode_template(&decoded).unwrap(),
        "round-trip bytes mismatch"
    );
    bytes
}

fn assert_redacted(error: &TemplateCreationError, canaries: &[&str]) {
    let output = format!("{error} {error:?}");
    assert_redacted_output(&output, canaries);
    assert!(error.source().is_none());
}

fn assert_redacted_output(output: &str, canaries: &[&str]) {
    assert!(!canaries.is_empty(), "redaction requires an input canary");
    for secret in canaries {
        assert!(!secret.is_empty(), "redaction canary must not be empty");
        assert!(
            !output.contains(secret),
            "redaction canary exposed in diagnostic"
        );
    }
}

fn assert_creation_canary(value: Option<&str>, location: &'static str) {
    assert!(
        value == Some(SECRET),
        "creation canary mismatch at {location}"
    );
}

fn expect_creation_error<T>(result: Result<T, TemplateCreationError>) -> TemplateCreationError {
    // expect_err는 예상 밖 성공값까지 Debug로 출력하므로 payload를 버리는 분기를 직접 둔다.
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected creation or duplication failure"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanaryAssertionFailure {
    SourcePathMismatch,
    TargetPathMismatch,
    InventoryMismatch,
    DuplicateCanary,
    MissingSource,
    MissingTarget,
    SourceMismatch,
    TargetMismatch,
    SourceTargetMismatch,
}

fn expected_canary(raw: &str) -> LosslessJsonValue {
    let wrapped = format!(r#"{{"expected":{raw}}}"#);
    let root = parse_strict_lossless_json_object(wrapped.as_bytes())
        .expect("independent expected canary must be valid JSON");
    root.object_path(&["expected"])
        .expect("expected canary wrapper must contain its fixed value")
        .clone()
}

fn verify_preserved_canary(
    source: &LosslessJsonValue,
    target: &LosslessJsonValue,
    source_path: &[String],
    target_path: &[String],
    expected: &LosslessJsonValue,
    intended: &CanaryPaths,
) -> Result<(), CanaryAssertionFailure> {
    verify_canary_paths(source_path, target_path, intended)?;
    let source_path = source_path.iter().map(String::as_str).collect::<Vec<_>>();
    let target_path = target_path.iter().map(String::as_str).collect::<Vec<_>>();
    let source_value = source
        .object_path(&source_path)
        .ok_or(CanaryAssertionFailure::MissingSource)?;
    let target_value = target
        .object_path(&target_path)
        .ok_or(CanaryAssertionFailure::MissingTarget)?;
    if source_value != expected {
        return Err(CanaryAssertionFailure::SourceMismatch);
    }
    if target_value != expected {
        return Err(CanaryAssertionFailure::TargetMismatch);
    }
    if source_value != target_value {
        return Err(CanaryAssertionFailure::SourceTargetMismatch);
    }
    Ok(())
}

fn assert_preserved_canary(
    source: &LosslessJsonValue,
    target: &LosslessJsonValue,
    source_path: &[String],
    target_path: &[String],
    expected_raw: &str,
    safe_location: &str,
    contract: &CanaryContract,
) {
    let result = verify_preserved_canary(
        source,
        target,
        source_path,
        target_path,
        &expected_canary(expected_raw),
        contract
            .get(safe_location)
            .expect("unknown canary location"),
    );
    assert!(
        result.is_ok(),
        "opaque canary assertion failed at {safe_location}: {:?}",
        result.err()
    );
}

fn assert_source_canary(
    source: &LosslessJsonValue,
    source_path: &[String],
    expected_raw: &str,
    safe_location: &str,
    contract: &CanaryContract,
) {
    let intended = contract
        .get(safe_location)
        .expect("unknown canary location");
    assert!(
        source_path == intended.0,
        "source canary path mismatch at {safe_location}"
    );
    assert_source_value(source, source_path, expected_raw, safe_location);
}

fn assert_source_value(
    source: &LosslessJsonValue,
    source_path: &[String],
    expected_raw: &str,
    safe_location: &str,
) {
    let source_path = source_path.iter().map(String::as_str).collect::<Vec<_>>();
    let source_value = source
        .object_path(&source_path)
        .unwrap_or_else(|| panic!("source canary missing at {safe_location}"));
    assert!(
        source_value == &expected_canary(expected_raw),
        "source canary mismatch at {safe_location}"
    );
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("fixed test string must serialize")
}

fn expected_rich_default(text: &str, outer: &str) -> String {
    format!(
        r#"{{"kind":"richText","futureValue":{{"big":{BIG}}},"document":{{"schemaVersion":1,"futureEnvelope":{{"exponent":{EXP}}},"content":{{"kind":"root","futureRoot":{{"negativeZero":-0}},"children":[{{"kind":"paragraph","futureContainer":{{"huge":{HUGE_EXP}}},"children":[{{"kind":"text","text":{},"futureNode":[{{"uuid":{},"big":{BIG}}}]}}]}}]}}}},"futureOuter":{outer}}}"#,
        json_string(text),
        json_string(&uuid(100)),
    )
}

fn expected_option(option: u32) -> String {
    match (option - 100) % 3 {
        0 => format!(
            r#"{{"label":{},"lifecycle":"active","futureOption":{{"negativeZero":-0}}}}"#,
            json_string(&uuid(10))
        ),
        1 => format!(
            r#"{{"label":"두 번째","lifecycle":"active","futureOption":{{"exponent":{EXP}}}}}"#
        ),
        2 => format!(
            r#"{{"label":{},"lifecycle":"archived","futureOption":{{"big":{BIG}}}}}"#,
            json_string(SECRET)
        ),
        _ => unreachable!("modulo result is exhaustive"),
    }
}

type CanaryPaths = (Vec<String>, Vec<String>);
type CanaryContract = BTreeMap<String, CanaryPaths>;

// inventory 생성식이나 production mapping을 재사용하지 않는 fixture 소유권 계약이다.
// 값이 같은 두 canary도 이 표의 owner/property 위치를 서로 대신할 수 없다.
fn fixture_canary_contract() -> CanaryContract {
    let mut contract = BTreeMap::new();
    let mut add = |location: String, source: String, target: String| {
        let paths = (
            source.split('/').map(str::to_owned).collect(),
            target.split('/').map(str::to_owned).collect(),
        );
        assert!(
            contract.insert(location, paths).is_none(),
            "duplicate fixture contract"
        );
    };
    for member in ["futureRoot", "presentation", "name"] {
        add(
            format!("Template.{member}"),
            member.to_owned(),
            member.to_owned(),
        );
    }
    for (index, (source, target)) in [
        (10, 999),
        (11, 998),
        (12, 997),
        (13, 996),
        (14, 995),
        (15, 994),
        (16, 993),
        (17, 992),
        (18, 991),
    ]
    .into_iter()
    .enumerate()
    {
        for (label, suffix) in [
            ("futureField", "futureField"),
            ("presentation", "presentation"),
            ("label", "label"),
            ("configuration.extra", "configuration/futureConfiguration"),
            ("defaultValue.extra", "defaultValue/futureOuter"),
            (
                "initialDefaultValue.extra",
                "initialDefaultValue/futureOuter",
            ),
        ] {
            add(
                format!("Field[{index}].{label}"),
                format!("fields/{}/{suffix}", uuid(source)),
                format!("fields/{}/{suffix}", uuid(target)),
            );
        }
    }
    for member in ["defaultValue", "initialDefaultValue"] {
        add(
            format!("Field[1].{member}.richText"),
            format!("fields/{}/{member}", uuid(11)),
            format!("fields/{}/{member}", uuid(998)),
        );
    }
    add(
        "Field[3].defaultValue.futureUnset".to_owned(),
        format!("fields/{}/defaultValue/futureUnset", uuid(13)),
        format!("fields/{}/defaultValue/futureUnset", uuid(996)),
    );
    for (index, (field, target_field, option, target_option)) in [
        (16, 993, 100, 990),
        (16, 993, 101, 989),
        (16, 993, 102, 988),
        (17, 992, 103, 987),
        (17, 992, 104, 986),
        (17, 992, 105, 985),
        (18, 991, 106, 984),
        (18, 991, 107, 983),
        (18, 991, 108, 982),
    ]
    .into_iter()
    .enumerate()
    {
        add(
            format!("Field[{}].Option[{index}]", field - 10),
            format!(
                "fields/{}/configuration/options/{}",
                uuid(field),
                uuid(option)
            ),
            format!(
                "fields/{}/configuration/options/{}",
                uuid(target_field),
                uuid(target_option)
            ),
        );
    }
    contract
}

fn verify_canary_paths(
    source: &[String],
    target: &[String],
    intended: &CanaryPaths,
) -> Result<(), CanaryAssertionFailure> {
    if source != intended.0 {
        return Err(CanaryAssertionFailure::SourcePathMismatch);
    }
    if target != intended.1 {
        return Err(CanaryAssertionFailure::TargetPathMismatch);
    }
    Ok(())
}

fn verify_canary_inventory(
    canaries: &[CanaryCase],
    contract: &CanaryContract,
) -> Result<(), CanaryAssertionFailure> {
    let mut locations = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for canary in canaries {
        if !locations.insert(canary.safe_location.clone())
            || !sources.insert(canary.source_path.clone())
            || !targets.insert(canary.target_path.clone())
        {
            return Err(CanaryAssertionFailure::DuplicateCanary);
        }
        let intended = contract
            .get(&canary.safe_location)
            .ok_or(CanaryAssertionFailure::InventoryMismatch)?;
        verify_canary_paths(&canary.source_path, &canary.target_path, intended)?;
    }
    if locations != contract.keys().cloned().collect() {
        return Err(CanaryAssertionFailure::InventoryMismatch);
    }
    Ok(())
}

// 독립 sentinel 검사와 별개로, 목록에 없는 unknown member까지 전체 복사를 검사한다.
fn verify_non_choice_defaults(
    source: &LosslessJsonValue,
    target: &LosslessJsonValue,
) -> Result<(), CanaryAssertionFailure> {
    for (old, new) in [
        (10, 999),
        (11, 998),
        (12, 997),
        (13, 996),
        (14, 995),
        (15, 994),
    ] {
        for member in ["defaultValue", "initialDefaultValue"] {
            let source_value = source
                .object_path(&["fields", &uuid(old), member])
                .ok_or(CanaryAssertionFailure::MissingSource)?;
            let target_value = target
                .object_path(&["fields", &uuid(new), member])
                .ok_or(CanaryAssertionFailure::MissingTarget)?;
            if source_value != target_value {
                return Err(CanaryAssertionFailure::SourceTargetMismatch);
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct CanaryCase {
    source_path: Vec<String>,
    target_path: Vec<String>,
    expected_raw: String,
    safe_location: String,
}

impl CanaryCase {
    fn new(
        source_path: Vec<String>,
        target_path: Vec<String>,
        expected_raw: String,
        safe_location: impl Into<String>,
    ) -> Self {
        Self {
            source_path,
            target_path,
            expected_raw,
            safe_location: safe_location.into(),
        }
    }
}

fn fixture_canaries() -> Vec<CanaryCase> {
    let current_outer = format!(
        r#"{{"exponent":{EXP},"uuid":{},"secret":{}}}"#,
        json_string(&uuid(100)),
        json_string(SECRET)
    );
    let initial_outer = format!(
        r#"{{"negativeZero":-0,"huge":{HUGE_EXP},"uuid":{}}}"#,
        json_string(&uuid(10))
    );
    let mut cases = vec![
        CanaryCase::new(
            vec!["futureRoot".to_owned()],
            vec!["futureRoot".to_owned()],
            format!(
                r#"{{"big":{BIG},"exponent":{EXP},"negativeZero":-0,"huge":{HUGE_EXP},"uuid":{},"fields":{{{}:{{"optionId":{}}}}},"secret":{}}}"#,
                json_string(&uuid(1)),
                json_string(&uuid(10)),
                json_string(&uuid(100)),
                json_string(SECRET)
            ),
            "Template.futureRoot",
        ),
        CanaryCase::new(
            vec!["presentation".to_owned()],
            vec!["presentation".to_owned()],
            format!(
                r#"{{"token":{},"futurePresentation":{{"exponent":{EXP}}}}}"#,
                json_string(&uuid(1))
            ),
            "Template.presentation",
        ),
        CanaryCase::new(
            vec!["name".to_owned()],
            vec!["name".to_owned()],
            json_string(SECRET),
            "Template.name",
        ),
    ];

    for field in 10..19 {
        let source_id = uuid(field);
        let target_id = uuid(1009 - field);
        let field_location = field - 10;
        let field_paths = |member: &str| {
            (
                vec!["fields".to_owned(), source_id.clone(), member.to_owned()],
                vec!["fields".to_owned(), target_id.clone(), member.to_owned()],
            )
        };
        let (source_path, target_path) = field_paths("futureField");
        cases.push(CanaryCase::new(
            source_path,
            target_path,
            format!(
                r#"{{"huge":{HUGE_EXP},"fields":{{{}:{}}}}}"#,
                json_string(&uuid(10)),
                json_string(&uuid(100))
            ),
            format!("Field[{field_location}].futureField"),
        ));
        let (source_path, target_path) = field_paths("presentation");
        cases.push(CanaryCase::new(
            source_path,
            target_path,
            format!(
                r#"{{"token":{},"futurePresentation":{{"big":{BIG}}}}}"#,
                json_string(&uuid(100))
            ),
            format!("Field[{field_location}].presentation"),
        ));
        let (source_path, target_path) = field_paths("label");
        cases.push(CanaryCase::new(
            source_path,
            target_path,
            json_string(&format!("{SECRET} {field_location}")),
            format!("Field[{field_location}].label"),
        ));
        cases.push(CanaryCase::new(
            vec![
                "fields".to_owned(),
                source_id.clone(),
                "configuration".to_owned(),
                "futureConfiguration".to_owned(),
            ],
            vec![
                "fields".to_owned(),
                target_id.clone(),
                "configuration".to_owned(),
                "futureConfiguration".to_owned(),
            ],
            format!(r#"{{"exponent":{EXP}}}"#),
            format!("Field[{field_location}].configuration.extra"),
        ));
        for (default, expected) in [
            ("defaultValue", current_outer.as_str()),
            ("initialDefaultValue", initial_outer.as_str()),
        ] {
            cases.push(CanaryCase::new(
                vec![
                    "fields".to_owned(),
                    source_id.clone(),
                    default.to_owned(),
                    "futureOuter".to_owned(),
                ],
                vec![
                    "fields".to_owned(),
                    target_id.clone(),
                    default.to_owned(),
                    "futureOuter".to_owned(),
                ],
                expected.to_owned(),
                format!("Field[{field_location}].{default}.extra"),
            ));
        }
        if field == 11 {
            for (default, text, outer) in [
                ("defaultValue", "현재", current_outer.as_str()),
                ("initialDefaultValue", "초기", initial_outer.as_str()),
            ] {
                cases.push(CanaryCase::new(
                    vec!["fields".to_owned(), source_id.clone(), default.to_owned()],
                    vec!["fields".to_owned(), target_id.clone(), default.to_owned()],
                    expected_rich_default(text, outer),
                    format!("Field[{field_location}].{default}.richText"),
                ));
            }
        }
        if field == 13 {
            cases.push(CanaryCase::new(
                vec![
                    "fields".to_owned(),
                    source_id.clone(),
                    "defaultValue".to_owned(),
                    "futureUnset".to_owned(),
                ],
                vec![
                    "fields".to_owned(),
                    target_id.clone(),
                    "defaultValue".to_owned(),
                    "futureUnset".to_owned(),
                ],
                "-0".to_owned(),
                format!("Field[{field_location}].defaultValue.futureUnset"),
            ));
        }
        if field >= 16 {
            for option in 100 + (field - 16) * 3..103 + (field - 16) * 3 {
                cases.push(CanaryCase::new(
                    vec![
                        "fields".to_owned(),
                        source_id.clone(),
                        "configuration".to_owned(),
                        "options".to_owned(),
                        uuid(option),
                    ],
                    vec![
                        "fields".to_owned(),
                        target_id.clone(),
                        "configuration".to_owned(),
                        "options".to_owned(),
                        uuid(1090 - option),
                    ],
                    expected_option(option),
                    format!("Field[{field_location}].Option[{}]", option - 100),
                ));
            }
        }
    }
    cases
}

fn assert_fixture_redaction_canaries(source: &TemplateArtifact) {
    let source_lossless = source
        .lossless_source()
        .expect("decoded fixture must retain its lossless source");
    let canaries = fixture_canaries();
    let contract = fixture_canary_contract();
    assert_eq!(verify_canary_inventory(&canaries, &contract), Ok(()));
    for canary in &canaries {
        assert_source_canary(
            source_lossless,
            &canary.source_path,
            &canary.expected_raw,
            &canary.safe_location,
            &contract,
        );
    }
    // 키 자체가 진단에 노출되는 회귀도 검사하므로, 실제 입력 위치와 값부터 확인한다.
    assert_source_value(
        source_lossless,
        &["templateId".to_owned()],
        &json_string(&uuid(1)),
        "Template.templateId",
    );
    for (member, options) in [
        ("defaultValue", [103, 104]),
        ("initialDefaultValue", [103, 105]),
    ] {
        assert_source_value(
            source_lossless,
            &[
                "fields".to_owned(),
                uuid(17),
                member.to_owned(),
                "optionIds".to_owned(),
            ],
            &format!(
                "[{},{}]",
                json_string(&uuid(options[0])),
                json_string(&uuid(options[1]))
            ),
            "Field[7].choice.optionIds",
        );
    }
}

fn failed_duplicate(
    source: &TemplateArtifact,
    ids: &mut FixedIds,
    timestamp: &str,
    category: TemplateCreationErrorCategory,
) -> TemplateCreationError {
    assert_fixture_redaction_canaries(source);
    let before = source.clone();
    let bytes_before = encode_template(source);
    let error = expect_creation_error(duplicate_template_with_ids(
        source,
        timestamp.to_owned(),
        ids,
    ));
    assert_eq!(error.category(), category);
    assert!(
        source == &before,
        "failed duplication changed source aggregate"
    );
    assert!(
        encode_template(source) == bytes_before,
        "failed duplication changed codec result"
    );
    assert_redacted(&error, DUPLICATION_REDACTION_CANARIES);
    error
}

#[test]
fn creation_empty_template_has_initial_metadata_and_canonical_round_trip() {
    let mut ids = FixedIds::new([999]);
    let created = create_template_with_ids(
        "새 Template".to_owned(),
        Some("character".to_owned()),
        NOW.to_owned(),
        &mut ids,
    )
    .unwrap();
    assert_eq!(created.template_id().to_string(), uuid(999));
    assert_eq!(
        created.template_id().as_uuid().get_version(),
        Some(Version::Random)
    );
    assert_eq!(
        created.template_id().as_uuid().get_variant(),
        Variant::RFC4122
    );
    assert_eq!(created.revision(), TemplateRevision::INITIAL);
    assert_eq!(created.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(created.created_at_utc(), NOW);
    assert_eq!(created.updated_at_utc(), NOW);
    assert!(created.name() == "새 Template", "created name mismatch");
    assert!(
        created.presentation().token() == Some("character"),
        "created presentation mismatch"
    );
    assert!(created.fields().is_empty());
    assert!(created.field_order().is_empty());
    assert!(created.extra.is_empty());
    assert!(created.presentation.extra.is_empty());
    assert_eq!(ids.calls, ["template"]);
    round_trip(&created);
}

#[test]
fn creation_preserves_all_v1_opaque_names_and_tokens() {
    for name in [
        "",
        " ",
        "\r\n",
        "\u{85}\u{2028}\u{2029}",
        "한글 e\u{301}",
        SECRET,
    ] {
        let created = create_template_with_ids(
            name.to_owned(),
            Some(name.to_owned()),
            NOW.to_owned(),
            &mut FixedIds::new([999]),
        )
        .unwrap();
        assert!(created.name() == name, "opaque creation name mismatch");
        assert!(
            created.presentation().token() == Some(name),
            "opaque creation token mismatch"
        );
        round_trip(&created);
    }
}

#[test]
fn creation_rejects_malformed_timestamps_before_allocating_ids() {
    for timestamp in [
        "",
        SECRET,
        "2026-02-30T12:34:56.789Z",
        "2026-09-06T12:34:56Z",
        "2026-09-06T12:34:56.789+00:00",
    ] {
        let mut ids = FixedIds::new([]);
        let name = SECRET.to_owned();
        let presentation_token = Some(SECRET.to_owned());
        assert_creation_canary(Some(&name), "name");
        assert_creation_canary(presentation_token.as_deref(), "presentation");
        let error = expect_creation_error(create_template_with_ids(
            name,
            presentation_token,
            timestamp.to_owned(),
            &mut ids,
        ));
        assert_eq!(
            error.category(),
            TemplateCreationErrorCategory::InvalidTimestamp
        );
        assert_eq!(error.stage(), TemplateCreationStage::InputValidation);
        assert!(ids.calls.is_empty());
        assert_redacted(&error, &[SECRET]);
    }
}

#[test]
fn creation_fails_closed_on_id_exhaustion_and_invalid_generated_identity() {
    for values in [
        VecDeque::new(),
        VecDeque::from(["not-an-id".to_owned()]),
        VecDeque::from(["00000000-0000-0000-0000-000000000000".to_owned()]),
    ] {
        let mut ids = FixedIds {
            values,
            calls: Vec::new(),
        };
        let name = SECRET.to_owned();
        assert_creation_canary(Some(&name), "name");
        let error = expect_creation_error(create_template_with_ids(
            name,
            None,
            NOW.to_owned(),
            &mut ids,
        ));
        assert_eq!(
            error.category(),
            TemplateCreationErrorCategory::IdGenerationFailed
        );
        assert_eq!(error.stage(), TemplateCreationStage::IdentityAllocation);
        assert_redacted(&error, &[SECRET]);
    }
}

#[test]
fn creation_same_id_and_time_inputs_have_identical_bytes() {
    let make = || {
        create_template_with_ids(
            "이름".to_owned(),
            None,
            NOW.to_owned(),
            &mut FixedIds::new([999]),
        )
        .unwrap()
    };
    assert!(
        round_trip(&make()) == round_trip(&make()),
        "creation bytes differ"
    );
}

#[test]
fn creation_production_api_uses_uuid_v4_and_accepts_injected_time() {
    let created = create_template("".to_owned(), None, NOW.to_owned()).unwrap();
    assert_eq!(
        created.template_id().as_uuid().get_version(),
        Some(Version::Random)
    );
    assert_eq!(
        created.template_id().as_uuid().get_variant(),
        Variant::RFC4122
    );
    assert!(!created.template_id().as_uuid().is_nil());
    round_trip(&created);
}

#[test]
fn duplication_all_identity_sets_are_disjoint_and_globally_unique() {
    let source = fixture();
    let target = duplicate(&source);
    let all = |template: &TemplateArtifact| {
        let mut ids = vec![template.template_id().as_uuid()];
        ids.extend(template.fields.keys().map(|id| id.as_uuid()));
        ids.extend(
            template
                .fields
                .values()
                .filter_map(|f| f.configuration.options())
                .flat_map(|o| o.keys().map(|id| id.as_uuid())),
        );
        ids
    };
    let old = all(&source).into_iter().collect::<BTreeSet<_>>();
    let new = all(&target);
    assert_ne!(source.template_id(), target.template_id());
    assert_eq!(old.len(), 19);
    assert_eq!(new.len(), 19);
    let new = new.into_iter().collect::<BTreeSet<_>>();
    assert_eq!(new.len(), 19);
    assert!(old.is_disjoint(&new));
    assert!(source
        .fields
        .keys()
        .all(|id| !target.fields.contains_key(id)));
    for id in new {
        assert_eq!(id.get_version(), Some(Version::Random));
        assert_eq!(id.get_variant(), Variant::RFC4122);
    }
}

#[test]
fn duplication_preserves_all_kinds_lifecycle_counts_and_semantic_orders() {
    let source = fixture();
    let target = duplicate(&source);
    assert_eq!(source.fields.len(), target.fields.len());
    assert_eq!(source.fields.len(), 9);
    let mut kinds = BTreeSet::new();
    for (old_id, old) in &source.fields {
        let new_id = field_id(999 - (old_id.as_uuid().as_u128() as u32 - 10));
        let new = &target.fields[&new_id];
        kinds.insert(format!("{:?}", new.kind()));
        assert!(old.label() == new.label(), "field label mismatch");
        assert_eq!(old.lifecycle(), new.lifecycle());
        assert_eq!(old.kind(), new.kind());
        assert_eq!(old.required, new.required);
        assert!(
            old.presentation == new.presentation,
            "field presentation mismatch"
        );
        assert!(old.extra == new.extra, "field extra mismatch");
        assert!(
            old.configuration.extra == new.configuration.extra,
            "configuration extra mismatch"
        );
        if let Some(options) = old.configuration.options() {
            let new_options = new.configuration.options().unwrap();
            assert_eq!(options.len(), new_options.len());
            for (id, option) in options {
                let new_id = option_id(990 - (id.as_uuid().as_u128() as u32 - 100));
                assert!(option == &new_options[&new_id], "option subtree mismatch");
                assert!(!options.contains_key(&new_id));
            }
            let labels = |field: &FieldDefinition| {
                field
                    .configuration
                    .option_order()
                    .unwrap()
                    .iter()
                    .map(|id| {
                        field.configuration.options().unwrap()[id]
                            .label()
                            .to_owned()
                    })
                    .collect::<Vec<_>>()
            };
            assert!(labels(old) == labels(new), "option label order mismatch");
        }
    }
    assert_eq!(kinds.len(), 8);
    assert_eq!(
        target
            .fields
            .values()
            .filter(|f| f.lifecycle() == FieldLifecycle::Archived)
            .count(),
        1
    );
    let field_labels = |t: &TemplateArtifact| {
        t.field_order()
            .iter()
            .map(|id| t.fields[id].label().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(
        field_labels(&source) == field_labels(&target),
        "field label order mismatch"
    );
    assert_eq!(
        target.field_order(),
        &[
            field_id(992),
            field_id(996),
            field_id(999),
            field_id(993),
            field_id(994),
            field_id(997),
            field_id(998),
            field_id(995)
        ]
    );
}

#[test]
fn duplication_remaps_choice_current_and_history_and_sorts_multi_choice() {
    let source = fixture();
    let target = duplicate(&source);
    let single = &target.fields[&field_id(993)];
    assert_eq!(single.default_value.single_choice(), Some(option_id(990)));
    assert_eq!(
        single.initial_default_value.single_choice(),
        Some(option_id(988))
    );
    assert_eq!(
        single.configuration.option_order().unwrap(),
        &[option_id(989), option_id(990)]
    );
    let multi = &target.fields[&field_id(992)];
    assert_eq!(
        multi.default_value.multi_choice().unwrap(),
        &[option_id(986), option_id(987)]
    );
    assert_eq!(
        multi.initial_default_value.multi_choice().unwrap(),
        &[option_id(985), option_id(987)]
    );
    assert_eq!(
        multi.configuration.options().unwrap()[&option_id(985)].lifecycle(),
        OptionLifecycle::Archived
    );
    let archived = &target.fields[&field_id(991)];
    assert_eq!(archived.default_value.single_choice(), Some(option_id(982)));
    assert_eq!(
        archived.initial_default_value.single_choice(),
        Some(option_id(984))
    );
    round_trip(&target);
}

#[test]
fn duplication_preserves_unset_and_non_choice_defaults_exactly() {
    let source = fixture();
    let target = duplicate(&source);
    for n in 10..16 {
        let old = &source.fields[&field_id(n)];
        let new = &target.fields[&field_id(1009 - n)];
        assert!(
            old.default_value == new.default_value,
            "current default mismatch"
        );
        assert!(
            old.initial_default_value == new.initial_default_value,
            "initial default mismatch"
        );
    }
    assert!(target.fields[&field_id(996)].default_value.is_unset());
    assert!(
        target.fields[&field_id(999)].default_value.text() == Some(uuid(100).as_str()),
        "UUID-like ordinary text mismatch"
    );
}

#[test]
fn duplication_preserves_all_opaque_subtrees_and_number_lexemes() {
    let source = fixture();
    let source_lossless = parse_strict_lossless_json_object(&round_trip(&source)).unwrap();
    let canaries = fixture_canaries();
    let contract = fixture_canary_contract();
    assert_eq!(verify_canary_inventory(&canaries, &contract), Ok(()));

    // 복제 호출 전에 fixture의 각 소유 위치와 sentinel 값을 독립 expected 값으로 고정한다.
    for canary in &canaries {
        assert_source_canary(
            &source_lossless,
            &canary.source_path,
            &canary.expected_raw,
            &canary.safe_location,
            &contract,
        );
    }

    // 양쪽 경로가 모두 사라진 경우와 target만 사라진 경우가 성공으로 보이지 않는 음성 probe다.
    let absent = parse_strict_lossless_json_object(b"{}").unwrap();
    let present = parse_strict_lossless_json_object(br#"{"canary":"fixed"}"#).unwrap();
    let path = vec!["canary".to_owned()];
    let fixed = expected_canary(r#""fixed""#);
    assert_eq!(
        verify_preserved_canary(
            &absent,
            &absent,
            &path,
            &path,
            &fixed,
            &(path.clone(), path.clone())
        ),
        Err(CanaryAssertionFailure::MissingSource)
    );
    assert_eq!(
        verify_preserved_canary(
            &present,
            &absent,
            &path,
            &path,
            &fixed,
            &(path.clone(), path.clone())
        ),
        Err(CanaryAssertionFailure::MissingTarget)
    );

    let target = duplicate(&source);
    let target_lossless = parse_strict_lossless_json_object(&round_trip(&target)).unwrap();
    for canary in &canaries {
        assert_preserved_canary(
            &source_lossless,
            &target_lossless,
            &canary.source_path,
            &canary.target_path,
            &canary.expected_raw,
            &canary.safe_location,
            &contract,
        );
    }
    assert_eq!(
        verify_non_choice_defaults(&source_lossless, &target_lossless),
        Ok(())
    );

    let text = String::from_utf8(round_trip(&target)).unwrap();
    for token in [BIG, EXP, HUGE_EXP, "-0"] {
        assert!(text.contains(token));
    }
}

#[test]
fn canary_regression_owner_paths_and_inventory_are_exact() {
    let source = fixture();
    let target = duplicate(&source);
    let old = parse_strict_lossless_json_object(&round_trip(&source)).unwrap();
    let new = parse_strict_lossless_json_object(&round_trip(&target)).unwrap();
    let inventory = fixture_canaries();
    let contract = fixture_canary_contract();
    assert_eq!(verify_canary_inventory(&inventory, &contract), Ok(()));
    let find = |location: &str| {
        inventory
            .iter()
            .find(|c| c.safe_location == location)
            .unwrap()
    };
    for (intended, replacement) in [
        ("Field[0].futureField", "Field[1].futureField"),
        ("Field[6].Option[0]", "Field[7].Option[3]"),
    ] {
        let first = find(intended);
        let other = find(replacement);
        let expected = expected_canary(&first.expected_raw);
        let paths = &contract[intended];
        assert_eq!(
            verify_preserved_canary(
                &old,
                &new,
                &first.source_path,
                &first.target_path,
                &expected,
                paths
            ),
            Ok(())
        );
        assert_eq!(
            verify_preserved_canary(
                &old,
                &new,
                &other.source_path,
                &other.target_path,
                &expected,
                paths
            ),
            Err(CanaryAssertionFailure::SourcePathMismatch)
        );
        assert_eq!(
            verify_preserved_canary(
                &old,
                &new,
                &first.source_path,
                &other.target_path,
                &expected,
                paths
            ),
            Err(CanaryAssertionFailure::TargetPathMismatch)
        );
    }
    let first = find("Field[0].futureField");
    let other = find("Field[1].futureField");
    let mut value = fixture_value();
    value["fields"][uuid(10)]
        .as_object_mut()
        .unwrap()
        .remove("futureField");
    let missing = fixture_from_value(&value);
    let missing_target = duplicate(&missing);
    let missing = parse_strict_lossless_json_object(&round_trip(&missing)).unwrap();
    let missing_target = parse_strict_lossless_json_object(&round_trip(&missing_target)).unwrap();
    assert_eq!(
        verify_preserved_canary(
            &missing,
            &missing_target,
            &other.source_path,
            &other.target_path,
            &expected_canary(&first.expected_raw),
            &contract[&first.safe_location]
        ),
        Err(CanaryAssertionFailure::SourcePathMismatch)
    );

    let mut changed = inventory.clone();
    changed.pop();
    assert_eq!(
        verify_canary_inventory(&changed, &contract),
        Err(CanaryAssertionFailure::InventoryMismatch)
    );
    let mut changed = inventory.clone();
    changed.push(inventory[0].clone());
    assert_eq!(
        verify_canary_inventory(&changed, &contract),
        Err(CanaryAssertionFailure::DuplicateCanary)
    );
    let mut changed = inventory.clone();
    changed[0] = inventory[1].clone();
    assert_eq!(changed.len(), inventory.len());
    assert_eq!(
        verify_canary_inventory(&changed, &contract),
        Err(CanaryAssertionFailure::DuplicateCanary)
    );
    let mut changed = inventory.clone();
    let entry = changed
        .iter_mut()
        .find(|c| c.safe_location == first.safe_location)
        .unwrap();
    entry.source_path = other.source_path.clone();
    entry.target_path = other.target_path.clone();
    assert!(
        verify_canary_inventory(&changed, &contract).is_err(),
        "substituted owner passed inventory"
    );
    let mut reordered = inventory.clone();
    reordered.reverse();
    assert_eq!(verify_canary_inventory(&reordered, &contract), Ok(()));
}

#[test]
fn canary_regression_missing_and_wrong_values_fail_closed() {
    let parse = |raw: &[u8]| parse_strict_lossless_json_object(raw).unwrap();
    let good = parse(br#"{"canary":"fixed","other":"kept"}"#);
    let missing = parse(br#"{"other":"kept"}"#);
    let wrong = parse(br#"{"canary":"wrong","other":"kept"}"#);
    let path = vec!["canary".to_owned()];
    let intended = (path.clone(), path.clone());
    let expected = expected_canary(r#""fixed""#);
    for (source, target, result) in [
        (&good, &good, Ok(())),
        (&missing, &good, Err(CanaryAssertionFailure::MissingSource)),
        (&good, &missing, Err(CanaryAssertionFailure::MissingTarget)),
        (
            &missing,
            &missing,
            Err(CanaryAssertionFailure::MissingSource),
        ),
        (&wrong, &wrong, Err(CanaryAssertionFailure::SourceMismatch)),
        (&wrong, &good, Err(CanaryAssertionFailure::SourceMismatch)),
        (&good, &wrong, Err(CanaryAssertionFailure::TargetMismatch)),
    ] {
        assert_eq!(
            verify_preserved_canary(source, target, &path, &path, &expected, &intended),
            result
        );
    }
}

#[test]
fn canary_regression_non_choice_defaults_include_unlisted_members() {
    // probe가 다른 숫자 lexeme을 바꾸지 않도록 지정한 object member만 raw 상태로 편집한다.
    fn edit_member(raw: &[u8], path: &[&str], replacement: Option<&str>) -> Vec<u8> {
        use serde_json::value::RawValue;
        let (key, tail) = path.split_first().expect("probe path must not be empty");
        let mut object: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(raw).unwrap();
        let previous = object.remove(*key).expect("probe member missing");
        if tail.is_empty() {
            if let Some(value) = replacement {
                object.insert(
                    (*key).to_owned(),
                    RawValue::from_string(value.to_owned()).unwrap(),
                );
            }
        } else {
            let edited = edit_member(previous.get().as_bytes(), tail, replacement);
            object.insert(
                (*key).to_owned(),
                RawValue::from_string(String::from_utf8(edited).unwrap()).unwrap(),
            );
        }
        serde_json::to_vec(&object).unwrap()
    }
    let mut value = fixture_value();
    for field in 10..16 {
        for member in ["defaultValue", "initialDefaultValue"] {
            value["fields"][uuid(field)][member]["probeOnlyExtra"] =
                json!({"payload":SECRET, "exponent":"EXP"});
        }
    }
    let source = fixture_from_value(&value);
    let expected_extra = expected_canary(&format!(
        r#"{{"payload":{},"exponent":{EXP}}}"#,
        json_string(SECRET)
    ));
    for field in 10..16 {
        for member in ["defaultValue", "initialDefaultValue"] {
            let extra = source
                .lossless_source()
                .unwrap()
                .object_path(&["fields", &uuid(field), member, "probeOnlyExtra"])
                .expect("unlisted source probe member missing");
            assert!(
                extra == &expected_extra,
                "unlisted source probe value mismatch"
            );
        }
    }
    let target = duplicate(&source);
    let old = parse_strict_lossless_json_object(&round_trip(&source)).unwrap();
    let bytes = round_trip(&target);
    let new = parse_strict_lossless_json_object(&bytes).unwrap();
    assert_eq!(verify_non_choice_defaults(&old, &new), Ok(()));
    let original_extra = serde_json::to_string(&expected_extra).unwrap();
    for field in 994..1000 {
        for member in ["defaultValue", "initialDefaultValue"] {
            let id = uuid(field);
            let path = ["fields", &id, member, "probeOnlyExtra"];
            let unchanged = edit_member(&bytes, &path, Some(&original_extra));
            let unchanged = parse_strict_lossless_json_object(&unchanged).unwrap();
            assert_eq!(verify_non_choice_defaults(&old, &unchanged), Ok(()));
            for replacement in [None, Some(r#""changed""#)] {
                let changed = edit_member(&bytes, &path, replacement);
                let changed = parse_strict_lossless_json_object(&changed).unwrap();
                assert_eq!(
                    verify_non_choice_defaults(&old, &changed),
                    Err(CanaryAssertionFailure::SourceTargetMismatch)
                );
            }
        }
    }
}

#[test]
fn canary_regression_diagnostics_are_payload_free() {
    const CHILD: &str = "WORLDBUILD_CANARY_DIAGNOSTIC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // panic hook은 프로세스 전역이므로, 이 테스트 하나만 실행하는 자식에서만 바꾼다.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "data::artifact::template::template_creation::tests::canary_regression_diagnostics_are_payload_free",
                "--test-threads=1", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "isolated diagnostic probe failed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "diagnostic probe did not execute"
        );
        for bytes in [&output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(bytes).contains(SECRET),
                "diagnostic probe emitted payload"
            );
        }
        return;
    }
    fn safe_panic(expected_category: &str, action: impl FnOnce() + std::panic::UnwindSafe) {
        let result = std::panic::catch_unwind(action);
        let payload = result.expect_err("expected diagnostic rejection");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .expect("panic should contain a safe message");
        assert!(
            message.contains(expected_category),
            "unexpected panic category"
        );
        for forbidden in [
            SECRET,
            "creation-secret",
            "unexpected-private-payload",
            BIG,
            HUGE_EXP,
        ] {
            assert!(
                !message.contains(forbidden),
                "panic message exposed payload"
            );
        }
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    safe_panic("creation canary mismatch", || {
        assert_creation_canary(Some("unexpected-private-payload"), "name")
    });
    safe_panic("creation canary mismatch", || {
        assert_creation_canary(None, "presentation")
    });
    safe_panic("expected creation or duplication failure", || {
        expect_creation_error(Ok(fixture()));
    });
    for forbidden in DUPLICATION_REDACTION_CANARIES {
        safe_panic("redaction canary exposed", || {
            assert_redacted_output(forbidden, DUPLICATION_REDACTION_CANARIES)
        });
    }
    assert_redacted_output("safe diagnostic", DUPLICATION_REDACTION_CANARIES);
    let source = fixture();
    let target = duplicate(&source);
    let old = parse_strict_lossless_json_object(&round_trip(&source)).unwrap();
    let new = parse_strict_lossless_json_object(&round_trip(&target)).unwrap();
    let contract = fixture_canary_contract();
    let canary = fixture_canaries().remove(0);
    let mut wrong = fixture_value();
    wrong["futureRoot"] = json!("unexpected-private-payload");
    let wrong = fixture_from_value(&wrong);
    let wrong = parse_strict_lossless_json_object(&round_trip(&wrong)).unwrap();
    safe_panic("source canary mismatch", || {
        assert_source_canary(
            &wrong,
            &canary.source_path,
            &canary.expected_raw,
            &canary.safe_location,
            &contract,
        )
    });
    for (source, target, category) in [
        (&wrong, &new, "SourceMismatch"),
        (&old, &wrong, "TargetMismatch"),
    ] {
        safe_panic(category, || {
            assert_preserved_canary(
                source,
                target,
                &canary.source_path,
                &canary.target_path,
                &canary.expected_raw,
                &canary.safe_location,
                &contract,
            )
        });
    }
    std::panic::set_hook(hook);
}

#[test]
fn duplication_resets_revision_times_and_snapshot_but_preserves_name_and_history() {
    let source = fixture();
    let target = duplicate(&source);
    assert_eq!(target.revision(), TemplateRevision::INITIAL);
    assert_eq!(target.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(target.created_at_utc(), NOW);
    assert_eq!(target.updated_at_utc(), NOW);
    assert!(target.name() == source.name(), "duplicate name mismatch");
    assert!(target
        .fields
        .values()
        .all(|f| f.introduced_revision == TemplateRevision::INITIAL));
    assert!(!source
        .default_draft_snapshot
        .is_same_snapshot(&target.default_draft_snapshot));
    let cloned = source.clone();
    assert!(cloned == source, "clone aggregate mismatch");
    assert!(source
        .default_draft_snapshot
        .is_same_snapshot(&cloned.default_draft_snapshot));
    assert!(
        round_trip(&cloned) == round_trip(&source),
        "clone bytes mismatch"
    );
}

#[test]
fn duplication_accepts_deleted_source_and_max_revision_as_new_active_identity() {
    let mut value = fixture_value();
    value["lifecycle"] = json!("deleted");
    value["revision"] = json!(u32::MAX);
    let source = fixture_from_value(&value);
    let before = round_trip(&source);
    let target = duplicate(&source);
    assert_eq!(target.lifecycle(), TemplateLifecycle::Active);
    assert_eq!(target.revision(), TemplateRevision::INITIAL);
    assert_eq!(source.lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(source.revision().get(), u32::MAX);
    assert!(
        before == round_trip(&source),
        "deleted source bytes changed"
    );
    round_trip(&target);
}

#[test]
fn duplication_is_deterministic_and_source_is_immutable_on_success() {
    let source = fixture();
    let snapshot = source.clone();
    let before = round_trip(&source);
    assert!(
        round_trip(&duplicate(&source)) == round_trip(&duplicate(&source)),
        "duplicate bytes differ"
    );
    assert!(source == snapshot, "source aggregate changed");
    assert!(before == round_trip(&source), "source bytes changed");
}

#[test]
fn duplication_rejects_collisions_with_every_source_identity_across_id_types() {
    let source = fixture();
    for old in [1, 10, 18, 100, 108] {
        failed_duplicate(
            &source,
            &mut FixedIds::new([old]),
            NOW,
            TemplateCreationErrorCategory::IdCollision,
        );
        failed_duplicate(
            &source,
            &mut FixedIds::new([1000, old]),
            NOW,
            TemplateCreationErrorCategory::IdCollision,
        );
        let mut prefix = (991..=1000).rev().collect::<Vec<_>>();
        prefix.push(old);
        failed_duplicate(
            &source,
            &mut FixedIds::new(prefix),
            NOW,
            TemplateCreationErrorCategory::IdCollision,
        );
    }
}

#[test]
fn duplication_rejects_duplicates_within_and_across_generated_id_types() {
    let source = fixture();
    for suffix in [1000, 999, 990] {
        let mut prefix = (990..=1000).rev().collect::<Vec<_>>();
        prefix.push(suffix);
        failed_duplicate(
            &source,
            &mut FixedIds::new(prefix),
            NOW,
            TemplateCreationErrorCategory::IdCollision,
        );
    }
    failed_duplicate(
        &source,
        &mut FixedIds::new([1000, 1000]),
        NOW,
        TemplateCreationErrorCategory::IdCollision,
    );
    failed_duplicate(
        &source,
        &mut FixedIds::new([1000, 999, 999]),
        NOW,
        TemplateCreationErrorCategory::IdCollision,
    );
}

#[test]
fn duplication_exhaustion_at_every_allocation_is_atomic() {
    let source = fixture();
    for count in 0..19 {
        let mut ids = FixedIds::new((982..=1000).rev().take(count));
        let error = failed_duplicate(
            &source,
            &mut ids,
            NOW,
            TemplateCreationErrorCategory::IdGenerationFailed,
        );
        assert_eq!(ids.calls.len(), count + 1);
        assert_eq!(error.stage(), TemplateCreationStage::IdentityAllocation);
        let expected = if count == 0 {
            TemplateCreationLocation::Template
        } else if count <= 9 {
            TemplateCreationLocation::Field(field_id(9 + count as u32))
        } else {
            TemplateCreationLocation::Option(option_id(90 + count as u32))
        };
        assert_eq!(error.location(), expected);
    }
}

#[test]
fn duplication_invalid_source_precedes_timestamp_and_id_generation() {
    let mut source = fixture();
    source.field_order.clear();
    let mut ids = FixedIds::new([]);
    let error = failed_duplicate(
        &source,
        &mut ids,
        SECRET,
        TemplateCreationErrorCategory::InvalidSource,
    );
    assert_eq!(error.stage(), TemplateCreationStage::SourceValidation);
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::FieldOrderMismatch)
    );
    assert!(ids.calls.is_empty());
}

#[test]
fn duplication_rejects_invalid_timestamp_before_id_generation() {
    let source = fixture();
    let mut ids = FixedIds::new([]);
    failed_duplicate(
        &source,
        &mut ids,
        SECRET,
        TemplateCreationErrorCategory::InvalidTimestamp,
    );
    assert!(ids.calls.is_empty());
}

#[test]
fn duplication_rejects_storage_invalid_source_with_redacted_diagnostics() {
    let mut source = fixture();
    source
        .extra
        .insert("$serde_json::private::secret".to_owned(), json!(SECRET));
    let error = failed_duplicate(
        &source,
        &mut FixedIds::new([]),
        NOW,
        TemplateCreationErrorCategory::InvalidSource,
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::ReservedExtraKey)
    );
    let mut source = fixture();
    source.corrupt_multi_choice_default_for_test(
        field_id(17),
        vec![option_id(103), option_id(103)],
        false,
    );
    let error = failed_duplicate(
        &source,
        &mut FixedIds::new([]),
        NOW,
        TemplateCreationErrorCategory::InvalidSource,
    );
    assert_eq!(
        error.validation_category(),
        Some(ArtifactValidationErrorCategory::InvalidChoiceValue)
    );
    assert_eq!(
        error.validation.unwrap().choice_location,
        Some(ArtifactChoiceValueLocation::CurrentDefault)
    );
}

#[test]
fn duplication_of_empty_new_template_and_production_uuid_boundary() {
    let source = create_template_with_ids(
        "이름".to_owned(),
        None,
        NOW.to_owned(),
        &mut FixedIds::new([1]),
    )
    .unwrap();
    let target = duplicate_template(&source, NOW.to_owned()).unwrap();
    assert_ne!(target.template_id(), source.template_id());
    assert_eq!(
        target.template_id().as_uuid().get_version(),
        Some(Version::Random)
    );
    assert_eq!(
        target.template_id().as_uuid().get_variant(),
        Variant::RFC4122
    );
    assert!(target.fields().is_empty());
    round_trip(&target);
}

#[test]
fn duplication_preserves_provenance_after_existing_mutation_and_repeated_duplication() {
    let source = fixture();
    let changed = apply_template_mutation(
        &source,
        source.revision(),
        NOW,
        TemplateMutationCommand::set_template_name("바뀐 이름".to_owned()),
    )
    .unwrap()
    .into_changed()
    .unwrap();
    let target = duplicate(&changed);
    let second = duplicate_template_with_ids(
        &target,
        NOW.to_owned(),
        &mut FixedIds::new((1982..=2000).rev()),
    )
    .unwrap();
    for template in [&changed, &target, &second] {
        assert!(template.name() == "바뀐 이름", "mutated name mismatch");
        let bytes = String::from_utf8(round_trip(template)).unwrap();
        assert!(bytes.contains(EXP));
        assert!(bytes.contains(HUGE_EXP));
        assert!(bytes.contains("-0"));
    }
}

#[test]
fn duplication_missing_internal_mapping_fails_closed_without_payload() {
    let source = fixture();
    assert_fixture_redaction_canaries(&source);
    let before = round_trip(&source);
    let mut mapping = IdentityMapping::allocate(&source, &mut FixedIds::complete()).unwrap();
    mapping.options.remove(&option_id(100));
    let error = expect_creation_error(duplicate_default(
        &source.fields[&field_id(16)].default_value,
        &mapping,
        TemplateCreationLocation::CurrentDefault(field_id(16)),
    ));
    assert_eq!(
        error.category(),
        TemplateCreationErrorCategory::IncompleteIdMapping
    );
    assert_eq!(
        error.location(),
        TemplateCreationLocation::CurrentDefault(field_id(16))
    );
    assert_redacted(&error, DUPLICATION_REDACTION_CANARIES);
    mapping.fields.remove(&field_id(10));
    assert_eq!(
        expect_creation_error(provenance::relocate(&source, &mapping)).category(),
        TemplateCreationErrorCategory::IncompleteIdMapping
    );
    assert!(
        before == round_trip(&source),
        "mapping failure changed source bytes"
    );
}
