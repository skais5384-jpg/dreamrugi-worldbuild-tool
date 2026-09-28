use super::*;
use crate::data::artifact::{decode_template, encode_template};
use serde_json::json;

const TIME: &str = "2026-09-14T01:00:00.000Z";
const LATER: &str = "2026-09-14T02:00:00.000Z";
const T: &str = "11111111-1111-4111-8111-111111111111";
const F: &str = "22222222-2222-4222-8222-222222222222";
const G: &str = "33333333-3333-4333-8333-333333333333";
const A: &str = "44444444-4444-4444-8444-444444444444";
const B: &str = "55555555-5555-4555-8555-555555555555";
fn fixture() -> TemplateArtifact {
    let wire = json!({"schemaVersion":1,"artifactType":"template","templateId":T,"revision":12,"name":"region","lifecycle":"active","createdAtUtc":TIME,"updatedAtUtc":TIME,"presentation":{"future":"root-token"},"fieldOrder":[F,G],"fields":{
        (F):{"label":"choice","kind":"singleChoice","required":false,"lifecycle":"active","introducedRevision":1,"presentation":{},"configuration":{"kind":"singleChoice","optionOrder":[A,B],"options":{(A):{"label":"A","lifecycle":"active","future":"option-token"},(B):{"label":"B","lifecycle":"active"}}},"defaultValue":{"kind":"singleChoice","optionId":A,"future":"envelope-token"},"initialDefaultValue":{"kind":"singleChoice","optionId":A}},
        (G):{"label":"number","kind":"number","required":false,"lifecycle":"active","introducedRevision":2,"presentation":{},"configuration":{"kind":"number"},"defaultValue":{"kind":"number","value":"10"},"initialDefaultValue":{"kind":"number","value":"5"},"future":"field-token"}},"futureNumber":"RAW"});
    decode_template(wire.to_string().replace("\"RAW\"", "1E100").as_bytes()).unwrap()
}
fn unchanged(source: &TemplateArtifact) -> TemplateDraftInput {
    TemplateDraftInput {
        sections: vec![],
        name: source.name.clone(),
        glossary_excluded: source.glossary_excluded(),
        presentation_token: source.presentation.token.clone(),
        fields: source
            .field_order
            .iter()
            .map(|id| {
                let f = &source.fields[id];
                let config = match &f.configuration.variant {
                    FieldConfigurationVariant::Number { minimum, maximum } => {
                        NewFieldConfiguration::bounded_number(minimum.clone(), maximum.clone())
                    }
                    FieldConfigurationVariant::SingleChoice {
                        option_order,
                        options,
                    } => NewFieldConfiguration::single_choice(
                        option_order.clone(),
                        options
                            .iter()
                            .map(|(id, o)| NewChoiceOptionDraft::new(*id, o.label.clone()))
                            .collect(),
                    ),
                    _ => panic!("fixture kind"),
                };
                FieldDraftInput {
                    members: vec![],
                    writing_guide: None,
                    id: *id,
                    label: f.label.clone(),
                    kind: f.kind,
                    configuration: config,
                    required: f.required,
                    presentation_token: f.presentation.token.clone(),
                    default: None,
                    archived: false,
                    archived_options: BTreeSet::new(),
                }
            })
            .collect(),
    }
}
#[test]
fn whole_one_revision_final_option_default_and_lossless_history() {
    let source = fixture();
    let before = encode_template(&source).unwrap();
    let mut input = unchanged(&source);
    input.fields[0].label = "new choice".into();
    input.fields[0].archived_options.insert(A.parse().unwrap());
    input.fields[0].default = Some(FieldValueDraft::single_choice(B.parse().unwrap()));
    input.fields[1].label = "new number".into();
    input.fields[1].required = true;
    input.fields.swap(0, 1);
    let changed = prepare_template_draft(&source, source.revision, LATER, input)
        .unwrap()
        .into_changed()
        .unwrap();
    assert_eq!(changed.revision.get(), 13);
    assert_eq!(changed.updated_at_utc, LATER);
    assert_eq!(
        changed.field_order,
        vec![G.parse().unwrap(), F.parse().unwrap()]
    );
    let f = &changed.fields[&F.parse().unwrap()];
    assert_eq!(
        f.initial_default_value.single_choice(),
        Some(A.parse().unwrap())
    );
    assert_eq!(f.default_value.single_choice(), Some(B.parse().unwrap()));
    assert_eq!(
        f.configuration.options().unwrap()[&A.parse().unwrap()].lifecycle,
        OptionLifecycle::Archived
    );
    let bytes = encode_template(&changed).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    for token in [
        "1E100",
        "option-token",
        "envelope-token",
        "field-token",
        "root-token",
    ] {
        assert!(text.contains(token));
    }
    assert_eq!(encode_template(&source).unwrap(), before);
}
#[test]
fn whole_new_field_first_default_is_final_value_and_net_noop_stays_noop() {
    let source = fixture();
    let input = unchanged(&source);
    assert!(
        prepare_template_draft(&source, source.revision, LATER, input.clone())
            .unwrap()
            .is_unchanged()
    );
    let mut draft = input;
    let id = FieldId::new();
    draft.fields.push(FieldDraftInput {
        members: vec![],
        writing_guide: None,
        id,
        label: "new".into(),
        kind: FieldKind::Number,
        configuration: NewFieldConfiguration::number(),
        required: false,
        presentation_token: None,
        default: Some(FieldValueDraft::number("20".into())),
        archived: false,
        archived_options: BTreeSet::new(),
    });
    let added = prepare_template_draft(&source, source.revision, LATER, draft.clone())
        .unwrap()
        .into_changed()
        .unwrap();
    let f = &added.fields[&id];
    assert_eq!(f.introduced_revision.get(), 13);
    assert_eq!(f.initial_default_value, FieldValue::number("20".into()));
    assert_eq!(f.initial_default_value, f.default_value);
    draft.fields.pop();
    assert!(
        prepare_template_draft(&source, source.revision, LATER, draft)
            .unwrap()
            .is_unchanged()
    );
}
#[test]
fn whole_invalid_final_default_and_missing_owner_keep_source() {
    let source = fixture();
    let before = encode_template(&source).unwrap();
    let mut input = unchanged(&source);
    input.fields[0].archived_options.insert(A.parse().unwrap());
    assert!(prepare_template_draft(&source, source.revision, LATER, input).is_err());
    let mut input = unchanged(&source);
    input.fields[1].default = Some(FieldValueDraft::number("-".into()));
    assert!(prepare_template_draft(&source, source.revision, LATER, input).is_err());
    let mut input = unchanged(&source);
    input.fields.pop();
    assert_eq!(
        prepare_template_draft(&source, source.revision, LATER, input)
            .unwrap_err()
            .category(),
        TemplateMutationErrorCategory::FieldRemoved
    );
    assert_eq!(encode_template(&source).unwrap(), before);
}
#[test]
fn whole_archived_definition_and_foreign_keep_cannot_change() {
    let source = fixture();
    let mut input = unchanged(&source);
    input.fields[1].archived = true;
    let source = prepare_template_draft(&source, source.revision, LATER, input)
        .unwrap()
        .into_changed()
        .unwrap();
    let mut input = unchanged(&fixture());
    input.fields[1].archived = true;
    for f in &mut input.fields {
        f.default = None;
    }
    input.fields[1].label = "tampered".into();
    assert!(prepare_template_draft(&source, source.revision, LATER, input).is_err());
    let a = fixture();
    let b = fixture();
    let mut input = unchanged(&b);
    input.fields[0].default = Some(b.current_default_draft(F.parse().unwrap()).unwrap());
    assert_eq!(
        prepare_template_draft(&a, a.revision, LATER, input)
            .unwrap_err()
            .category(),
        TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch
    );
}
#[test]
fn whole_create_is_one_unpublished_candidate_revision_one() {
    let seed = crate::data::artifact::create_template("seed".into(), None, TIME.into()).unwrap();
    let id = FieldId::new();
    let draft = TemplateDraftInput {
        sections: vec![],
        name: "full new".into(),
        glossary_excluded: false,
        presentation_token: None,
        fields: vec![FieldDraftInput {
            members: vec![],
            writing_guide: None,
            id,
            label: "new".into(),
            kind: FieldKind::Number,
            configuration: NewFieldConfiguration::number(),
            required: true,
            presentation_token: None,
            default: Some(FieldValueDraft::number("20".into())),
            archived: false,
            archived_options: BTreeSet::new(),
        }],
    };
    let result = prepare_new_template_draft(&seed, LATER, draft).unwrap();
    assert_eq!(result.template_id, seed.template_id);
    assert_eq!(result.revision.get(), 1);
    assert_eq!(result.fields[&id].introduced_revision.get(), 1);
    assert_eq!(
        result.fields[&id].default_value,
        result.fields[&id].initial_default_value
    );
    assert!(seed.fields.is_empty());
}

#[test]
fn writing_guide_upgrades_only_on_edit_and_preserves_defaults_and_unknown_bytes() {
    let source = fixture();
    let before = encode_template(&source).unwrap();
    assert!(
        prepare_template_draft(&source, source.revision, LATER, unchanged(&source))
            .unwrap()
            .is_unchanged()
    );
    let mut input = unchanged(&source);
    input.fields[0].writing_guide = Some("예: 수도의 정치적 역할".into());
    let candidate = prepare_template_draft(&source, source.revision, LATER, input)
        .unwrap()
        .into_changed()
        .unwrap();
    assert_eq!(candidate.schema_version.get(), 2);
    assert_eq!(candidate.revision.get(), source.revision.get() + 1);
    for (id, old) in &source.fields {
        let next = &candidate.fields[id];
        assert_eq!(next.default_value, old.default_value);
        assert_eq!(next.initial_default_value, old.initial_default_value);
        assert_eq!(next.introduced_revision, old.introduced_revision);
    }
    let bytes = encode_template(&candidate).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("1E100"));
    let decoded = decode_template(&bytes).unwrap();
    assert_eq!(
        decoded.fields[&F.parse().unwrap()].writing_guide(),
        Some("예: 수도의 정치적 역할")
    );
    assert_eq!(encode_template(&source).unwrap(), before);
}

#[test]
fn writing_guide_legacy_unknown_member_is_never_claimed_even_null() {
    for unknown in [
        json!(null),
        json!(false),
        json!("legacy"),
        json!({"unknown": 4}),
    ] {
        let mut wire: serde_json::Value =
            serde_json::from_slice(&encode_template(&fixture()).unwrap()).unwrap();
        wire["fields"][F]["writingGuide"] = unknown.clone();
        let source = decode_template(&serde_json::to_vec(&wire).unwrap()).unwrap();
        assert_eq!(source.fields[&F.parse().unwrap()].writing_guide(), None);
        let encoded: serde_json::Value =
            serde_json::from_slice(&encode_template(&source).unwrap()).unwrap();
        assert_eq!(encoded["fields"][F]["writingGuide"], unknown);
        let mut input = unchanged(&source);
        input.fields[1].writing_guide = Some("new".into());
        assert_eq!(
            prepare_template_draft(&source, source.revision, LATER, input)
                .unwrap_err()
                .category(),
            TemplateMutationErrorCategory::GuideMetadataConflict
        );
    }
}

#[test]
fn writing_guide_schema_validates_typed_slot_and_blocks_future() {
    let mut wire: serde_json::Value =
        serde_json::from_slice(&encode_template(&fixture()).unwrap()).unwrap();
    wire["schemaVersion"] = json!(2);
    for invalid in [json!(null), json!(false), json!(42), json!([])] {
        wire["fields"][F]["writingGuide"] = invalid;
        assert!(decode_template(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    wire["fields"][F]["writingGuide"] = json!("가이드");
    assert!(decode_template(&serde_json::to_vec(&wire).unwrap()).is_ok());
    wire["schemaVersion"] = json!(8);
    assert!(decode_template(&serde_json::to_vec(&wire).unwrap()).is_err());
}

#[test]
fn m36_bounds_preserve_historical_defaults_but_reject_new_outside_default() {
    let bytes = crate::data::artifact::transition_format(
        &encode_template(&fixture()).unwrap(),
        &crate::data::project_relative_path::ProjectRelativePath::parse("templates/test.json")
            .unwrap(),
    )
    .unwrap();
    let source = decode_template(&bytes).unwrap();
    let mut draft = unchanged(&source);
    draft.fields[1].configuration =
        NewFieldConfiguration::bounded_number(Some("-1".into()), Some("1".into()));
    let narrowed = prepare_template_draft(&source, source.revision, LATER, draft.clone())
        .unwrap()
        .into_changed()
        .unwrap();
    assert_eq!(
        narrowed.fields[&G.parse().unwrap()].default_value,
        source.fields[&G.parse().unwrap()].default_value
    );
    assert_eq!(
        narrowed.fields[&G.parse().unwrap()].initial_default_value,
        source.fields[&G.parse().unwrap()].initial_default_value
    );
    draft.fields[1].default = Some(FieldValueDraft::number("2".into()));
    assert!(prepare_template_draft(&source, source.revision, LATER, draft.clone()).is_err());
    draft.fields[1].default = Some(FieldValueDraft::number_unknown());
    assert!(prepare_template_draft(&source, source.revision, LATER, draft).is_ok());
}
#[test]
fn m36_sections_reorder_unknown_numbers_by_identity_and_remove_without_values() {
    let mut wire: serde_json::Value = serde_json::from_slice(
        &crate::data::artifact::transition_format(
            &encode_template(&fixture()).unwrap(),
            &crate::data::project_relative_path::ProjectRelativePath::parse("templates/test.json")
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    wire["sections"] = json!([
        {"id":A,"title":"상황","beforeField":F,"future":"UPPER"},
        {"id":B,"title":"정보","beforeField":G,"future":"LOWER"}
    ]);
    let raw = wire
        .to_string()
        .replace("\"UPPER\"", "1E100")
        .replace("\"LOWER\"", "1e100");
    let source = decode_template(raw.as_bytes()).unwrap();
    let mut draft = unchanged(&source);
    draft.sections = source.sections.iter().rev().map(|s| s.input()).collect();
    draft.sections[0].before_field = Some(F.into());
    draft.sections[1].before_field = None;
    let candidate = prepare_template_draft(&source, source.revision, LATER, draft)
        .unwrap()
        .into_changed()
        .unwrap();
    let encoded = String::from_utf8(encode_template(&candidate).unwrap()).unwrap();
    let sections = encoded.find("\"sections\"").unwrap();
    let b = encoded[sections..].find(B).unwrap() + sections;
    let a = encoded[b..].find(A).unwrap() + b;
    assert!(encoded[sections..b].contains("1e100"), "{encoded}");
    assert!(encoded[b..a].contains("1E100"));
    assert!(candidate.fields == source.fields);
    let mut remove = unchanged(&candidate);
    remove.sections = vec![];
    let removed = prepare_template_draft(&candidate, candidate.revision, LATER, remove)
        .unwrap()
        .into_changed()
        .unwrap();
    assert!(removed.sections.is_empty());
    assert!(removed.fields == source.fields);
}
#[test]
fn m36_format_conflict_rejects_new_known_key_even_matching_old_unknown_type() {
    for key in ["sections", "minimum", "maximum"] {
        let mut wire: serde_json::Value =
            serde_json::from_slice(&encode_template(&fixture()).unwrap()).unwrap();
        if key == "sections" {
            wire[key] = json!([]);
        } else {
            wire["fields"][G]["configuration"][key] = json!("0");
        }
        let bytes = serde_json::to_vec(&wire).unwrap();
        assert!(crate::data::artifact::transition_format(
            &bytes,
            &crate::data::project_relative_path::ProjectRelativePath::parse("templates/test.json")
                .unwrap()
        )
        .is_err());
    }
}
