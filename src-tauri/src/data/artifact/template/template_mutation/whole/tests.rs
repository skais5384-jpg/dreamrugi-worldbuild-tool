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

#[test]
fn explicit_restoration_keeps_identity_title_and_unknown_metadata_after_reopen() {
    let active = card_title_source(Some(G));
    let mut draft = card_title_draft(&active, None);
    draft.fields[0].archived = true;
    let archived = prepare_template_draft(&active, active.revision, LATER, draft)
        .unwrap()
        .into_changed()
        .unwrap();
    let mut restore = card_title_draft(&archived, None);
    assert!(prepare_template_draft(&archived, archived.revision, LATER, restore.clone()).is_err());
    restore.fields[0].restore = true;
    restore.fields[0].label = "restored and renamed".into();
    let restored = prepare_template_draft(&archived, archived.revision, LATER, restore)
        .unwrap()
        .into_changed()
        .unwrap();
    let reopened = decode_template(&encode_template(&restored).unwrap()).unwrap();
    let id: FieldId = F.parse().unwrap();
    assert_eq!(reopened.field_order, vec![id]);
    assert_eq!(
        reopened.fields[&id].initial_default_value,
        archived.fields[&id].initial_default_value
    );
    assert_eq!(
        reopened.fields[&id].presentation,
        archived.fields[&id].presentation
    );
    assert_eq!(
        reopened.fields[&id].configuration,
        archived.fields[&id].configuration
    );
    assert_eq!(
        reopened.fields[&id].introduced_revision,
        archived.fields[&id].introduced_revision
    );
    assert_eq!(reopened.fields[&id].extra, archived.fields[&id].extra);
    assert_eq!(archived.fields[&id].lifecycle, FieldLifecycle::Archived);
}

#[test]
fn restoration_is_scoped_and_does_not_implicitly_restore_archived_children() {
    let source = card_title_source(Some(G));
    let mut archive = card_title_draft(&source, None);
    archive.fields[0].members[0].archived = true;
    archive.fields[0].archived = true;
    let archived = prepare_template_draft(&source, source.revision, LATER, archive)
        .unwrap()
        .into_changed()
        .unwrap();
    let mut restore = card_title_draft(&archived, None);
    restore.fields[0].restore = true;
    restore.fields[0].members[0].archived = true;
    let restored = prepare_template_draft(&archived, archived.revision, LATER, restore)
        .unwrap()
        .into_changed()
        .unwrap();
    let (_, children) = restored.fields[&F.parse().unwrap()]
        .configuration
        .members()
        .unwrap();
    assert_eq!(
        children[&G.parse().unwrap()].lifecycle,
        FieldLifecycle::Archived
    );
    let mut child_restore = card_title_draft(&restored, None);
    assert!(
        prepare_template_draft(&restored, restored.revision, LATER, child_restore.clone()).is_err()
    );
    child_restore.fields[0].members[0].restore = true;
    let restored_child = prepare_template_draft(&restored, restored.revision, LATER, child_restore)
        .unwrap()
        .into_changed()
        .unwrap();
    let (order, children) = restored_child.fields[&F.parse().unwrap()]
        .configuration
        .members()
        .unwrap();
    assert_eq!(order, &[G.parse().unwrap()]);
    assert_eq!(
        children[&G.parse().unwrap()].lifecycle,
        FieldLifecycle::Active
    );
    let mut forged = card_title_draft(&restored_child, None);
    forged.fields[0].restore = true;
    assert!(
        prepare_template_draft(&restored_child, restored_child.revision, LATER, forged).is_err()
    );
}

fn card_title_source(title: Option<&str>) -> TemplateArtifact {
    let mut wire: serde_json::Value =
        serde_json::from_slice(&encode_template(&fixture()).unwrap()).unwrap();
    let child = json!({"label":"explicit title","kind":"richText","required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"presentation":{},"configuration":{"kind":"richText"}});
    wire["fields"] = json!({(F):{"label":"cards","kind":"group","required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"presentation":{},"configuration":{"kind":"group","memberOrder":[G],"members":{(G):child}}}});
    wire["fieldOrder"] = json!([F]);
    wire["schemaVersion"] = json!(5);
    if let Some(title) = title {
        wire["fields"][F]["presentation"]["cardTitleField"] = json!(title);
    }
    decode_template(&serde_json::to_vec(&wire).unwrap()).unwrap()
}

fn card_title_draft(source: &TemplateArtifact, title: Option<FieldId>) -> TemplateDraftInput {
    let child = FieldDraftInput {
        card_title_field: crate::data::edit_recovery::model::Intent::Keep,
        members: vec![],
        id: G.parse().unwrap(),
        label: "renamed title".into(),
        kind: FieldKind::RichText,
        configuration: NewFieldConfiguration::rich_text(),
        required: false,
        writing_guide: None,
        presentation_token: None,
        default: None,
        archived: false,
        restore: false,
        restored_options: Default::default(),
        archived_options: BTreeSet::new(),
    };
    TemplateDraftInput {
        sections: vec![],
        name: source.name.clone(),
        glossary_excluded: false,
        presentation_token: None,
        fields: vec![FieldDraftInput {
            card_title_field: title
                .map(crate::data::edit_recovery::model::Intent::Set)
                .unwrap_or(crate::data::edit_recovery::model::Intent::Keep),
            members: vec![child],
            id: F.parse().unwrap(),
            label: "cards".into(),
            kind: FieldKind::Group,
            configuration: NewFieldConfiguration::group(),
            required: false,
            writing_guide: None,
            presentation_token: None,
            default: None,
            archived: false,
            restore: false,
            restored_options: Default::default(),
            archived_options: BTreeSet::new(),
        }],
    }
}

#[test]
fn card_title_setting_saves_reopens_renames_and_template_clone_remaps_only_its_id() {
    let source = card_title_source(None);
    let draft = card_title_draft(&source, Some(G.parse().unwrap()));
    let saved = prepare_template_draft(&source, source.revision, LATER, draft)
        .unwrap()
        .into_changed()
        .unwrap();
    let opened = decode_template(&encode_template(&saved).unwrap()).unwrap();
    assert_eq!(
        opened.fields[&F.parse().unwrap()]
            .presentation
            .card_title_field(),
        Some(G.parse().unwrap())
    );
    let copied = crate::data::artifact::duplicate_template(&opened, LATER.into()).unwrap();
    let group = copied.fields.values().next().unwrap();
    let (_, children) = group.configuration.members().unwrap();
    let title = group.presentation.card_title_field().unwrap();
    assert!(children.contains_key(&title));
    assert_ne!(title.to_string(), G);
    assert_eq!(children[&title].label, "renamed title");
    assert_eq!(
        source.fields[&F.parse().unwrap()]
            .presentation
            .card_title_field(),
        None
    );
}

#[test]
fn card_title_invalid_cross_group_id_rejects_and_archived_target_retains_definition() {
    let source = card_title_source(Some(G));
    let mut draft = card_title_draft(&source, Some(A.parse().unwrap()));
    assert!(prepare_template_draft(&source, source.revision, LATER, draft.clone()).is_err());
    draft.fields[0].card_title_field = crate::data::edit_recovery::model::Intent::Keep;
    draft.fields[0].members[0].archived = true;
    let archived = prepare_template_draft(&source, source.revision, LATER, draft)
        .unwrap()
        .into_changed()
        .unwrap();
    let group = &archived.fields[&F.parse().unwrap()];
    assert_eq!(
        group.presentation.card_title_field(),
        Some(G.parse().unwrap())
    );
    assert_eq!(
        group.configuration.members().unwrap().1[&G.parse().unwrap()].lifecycle,
        FieldLifecycle::Archived
    );
    assert!(decode_template(&encode_template(&archived).unwrap()).is_ok());
}

#[test]
fn card_title_legacy_unknown_and_old_clone_inactive_ids_survive_unrelated_saves() {
    for metadata in [json!({"legacy":"opaque"}), json!("vendor-title"), json!(A)] {
        let original = card_title_source(None);
        let mut wire: serde_json::Value =
            serde_json::from_slice(&encode_template(&original).unwrap()).unwrap();
        wire["fields"][F]["presentation"]["cardTitleField"] = metadata.clone();
        let source = decode_template(&serde_json::to_vec(&wire).unwrap()).unwrap();
        let draft = card_title_draft(&source, None);
        let saved = prepare_template_draft(&source, source.revision, LATER, draft)
            .unwrap()
            .into_changed()
            .unwrap();
        let reopened: serde_json::Value =
            serde_json::from_slice(&encode_template(&saved).unwrap()).unwrap();
        assert_eq!(
            reopened["fields"][F]["presentation"]["cardTitleField"],
            metadata
        );
        assert_eq!(
            reopened["fields"][F]["configuration"]["members"][G]["label"],
            "renamed title"
        );
        if !metadata
            .as_str()
            .is_some_and(|v| v.parse::<FieldId>().is_ok())
        {
            assert!(prepare_template_draft(
                &source,
                source.revision,
                LATER,
                card_title_draft(&source, Some(G.parse().unwrap()))
            )
            .is_err());
        }
    }
}

#[test]
fn card_title_draft_recovery_roundtrip_keeps_explicit_selection_and_rejects_invalid_set() {
    use crate::data::edit_recovery::model::{DraftConfiguration, Intent};
    let raw = json!({"kind":"group","cardTitleField":{"intent":"set","value":G},"members":[]});
    let draft: DraftConfiguration = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(serde_json::to_value(&draft).unwrap(), raw);
    let old: DraftConfiguration =
        serde_json::from_value(json!({"kind":"group","members":[]})).unwrap();
    assert!(matches!(
        old,
        DraftConfiguration::Group {
            card_title_field: Intent::Keep,
            ..
        }
    ));
    let source = card_title_source(None);
    let mut input = card_title_draft(&source, Some(G.parse().unwrap()));
    input.fields[0].members[0].archived = true;
    assert!(prepare_template_draft(&source, source.revision, LATER, input).is_err());
}

#[test]
fn card_title_opaque_raw_number_is_preserved_and_explicit_unset_does_not_erase_it() {
    let mut wire: serde_json::Value =
        serde_json::from_slice(&encode_template(&card_title_source(None)).unwrap()).unwrap();
    wire["fields"][F]["presentation"]["cardTitleField"] = json!({"legacy":"EXP"});
    let raw = serde_json::to_string(&wire)
        .unwrap()
        .replace("\"EXP\"", "1E100");
    let source = decode_template(raw.as_bytes()).unwrap();
    let saved = prepare_template_draft(
        &source,
        source.revision,
        LATER,
        card_title_draft(&source, None),
    )
    .unwrap()
    .into_changed()
    .unwrap();
    assert!(String::from_utf8(encode_template(&saved).unwrap())
        .unwrap()
        .contains("1E100"));
    let mut input = card_title_draft(&source, None);
    input.fields[0].card_title_field = crate::data::edit_recovery::model::Intent::Unset;
    assert!(prepare_template_draft(&source, source.revision, LATER, input).is_err());
}
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
                    card_title_field: crate::data::edit_recovery::model::Intent::Keep,
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
                    restore: false,
                    restored_options: Default::default(),
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
        card_title_field: crate::data::edit_recovery::model::Intent::Keep,
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
        restore: false,
        restored_options: Default::default(),
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
            card_title_field: crate::data::edit_recovery::model::Intent::Keep,
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
            restore: false,
            restored_options: Default::default(),
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
    wire["schemaVersion"] = json!(crate::data::artifact::TEMPLATE_SCHEMA_VERSION.get() + 1);
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

#[test]
fn version_restore_is_a_forward_template_change_and_keeps_new_definitions_archived() {
    let historical = fixture();
    let mut draft = unchanged(&historical);
    draft.name = "현재 이름".into();
    draft.fields.push(FieldDraftInput {
        id: A.parse().unwrap(),
        label: "나중에 추가".into(),
        kind: FieldKind::SingleLineText,
        configuration: NewFieldConfiguration::single_line_text(),
        required: false,
        writing_guide: None,
        presentation_token: None,
        default: Some(FieldValueDraft::unset()),
        archived: false,
        restore: false,
        restored_options: Default::default(),
        archived_options: Default::default(),
        members: vec![],
        card_title_field: crate::data::edit_recovery::model::Intent::Keep,
    });
    let current = prepare_template_draft(&historical, historical.revision, LATER, draft)
        .unwrap()
        .into_changed()
        .unwrap();
    let restored = prepare_version_restore(&current, &historical, LATER)
        .unwrap()
        .into_changed()
        .unwrap();
    assert_eq!(restored.revision.get(), current.revision.get() + 1);
    assert_eq!(restored.template_id, current.template_id);
    assert_eq!(restored.name, historical.name);
    assert_eq!(
        restored.fields[&A.parse().unwrap()].lifecycle,
        FieldLifecycle::Archived
    );
    assert_eq!(
        restored.fields[&F.parse().unwrap()].initial_default_value,
        current.fields[&F.parse().unwrap()].initial_default_value
    );
    assert!(prepare_version_restore(&restored, &historical, LATER)
        .unwrap()
        .changed()
        .is_none());
}
