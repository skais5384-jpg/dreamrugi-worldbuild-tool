use super::mutations::{defined, field, install, option};
use super::*;
use crate::data::json::parse_strict_lossless_json_object;
use std::collections::BTreeSet;

#[test]
fn g6_duplicate_full_identity_lossless_owners_and_deleted_source() {
    for deleted in [false, true] {
        let fixture = Fixture::new();
        let old = defined();
        let old = template_mutation::apply_template_mutation(
            &old,
            old.revision(),
            TIME,
            TemplateMutationCommand::archive_option(
                field(1),
                option(1),
                Some(FieldValueDraft::single_choice(option(2))),
            ),
        )
        .unwrap()
        .into_changed()
        .unwrap();
        let old = template_mutation::apply_template_mutation(
            &old,
            old.revision(),
            TIME,
            TemplateMutationCommand::archive_field(field(2)),
        )
        .unwrap()
        .into_changed()
        .unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&artifact::encode_template(&old).unwrap()).unwrap();
        json["futureRoot"] =
            serde_json::json!({"number":"__NUMBER__","opaqueId":old.template_id().to_string()});
        json["fields"][field(1).to_string()]["futureField"] =
            serde_json::json!({"number":"__NUMBER__"});
        json["fields"][field(1).to_string()]["configuration"]["options"][option(1).to_string()]
            ["futureOption"] = serde_json::json!({"number":"__NUMBER__"});
        json["fields"][field(1).to_string()]["defaultValue"]["futureCurrent"] =
            serde_json::json!({"number":"__NUMBER__"});
        json["fields"][field(1).to_string()]["initialDefaultValue"]["futureInitial"] =
            serde_json::json!({"number":"__NUMBER__"});
        if deleted {
            json["lifecycle"] = "deleted".into();
            json["revision"] = u32::MAX.into();
        }
        let bytes = serde_json::to_string(&json)
            .unwrap()
            .replace("\"__NUMBER__\"", "1E+100")
            .into_bytes();
        let old = artifact::decode_template(&bytes).unwrap();
        install(&fixture, &old);
        fs::write(path(&fixture, old.template_id()), &bytes).unwrap();
        let mut runtime = fixture.runtime();
        let input = DuplicateTemplateInput {
            source: source(&mut runtime, old.template_id()),
            timestamp_utc: LATER.into(),
        };
        let mut ticket = prepare_duplicate_template(&mut runtime, &input).unwrap();
        let new_id = ticket.template_id();
        let ptr = ticket.candidate() as *const _;
        assert_ne!(new_id, old.template_id());
        assert_eq!(ticket.session_targets().len(), 1);
        assert_eq!(
            ticket.session_targets()[0],
            ArtifactSourceId::Template(new_id).path().unwrap()
        );
        let mut session: Session = begin(&runtime, ticket.session_targets());
        let snap = session.snapshot();
        let (result, counts, commits) =
            observe(|| duplicate_template(&mut runtime, &mut session, context(&snap), &mut ticket));
        assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
        assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
        assert_eq!(ticket.candidate() as *const _, ptr);
        let new = load(&mut runtime, new_id);
        assert_eq!(new.revision(), TemplateRevision::INITIAL);
        assert_eq!(new.lifecycle(), artifact::TemplateLifecycle::Active);
        assert_eq!(new.created_at_utc(), LATER);
        assert_eq!(new.updated_at_utc(), LATER);
        assert!(new.name() == old.name());
        let identities = |t: &TemplateArtifact| {
            let mut ids = BTreeSet::new();
            ids.insert(t.template_id().as_uuid());
            for (id, f) in t.fields() {
                ids.insert(id.as_uuid());
                if let Some(options) = f.configuration().options() {
                    for id in options.keys() {
                        ids.insert(id.as_uuid());
                    }
                }
            }
            ids
        };
        let a = identities(&old);
        let b = identities(&new);
        assert_eq!(a.len(), 5);
        assert_eq!(b.len(), 5);
        assert!(a.is_disjoint(&b));
        let new_field = new.field_order()[0];
        let new_choice = &new.fields()[&new_field];
        let old_choice = &old.fields()[&field(1)];
        let new_archived_option = *new_choice
            .configuration()
            .options()
            .unwrap()
            .iter()
            .find(|(_, o)| o.lifecycle() == artifact::OptionLifecycle::Archived)
            .unwrap()
            .0;
        assert_eq!(
            new.fields()
                .values()
                .filter(|f| f.lifecycle() == artifact::FieldLifecycle::Archived)
                .count(),
            1
        );
        assert!(new
            .fields()
            .values()
            .all(|f| f.introduced_revision() == TemplateRevision::INITIAL));
        let before = parse_strict_lossless_json_object(&bytes).unwrap();
        let after_bytes = fs::read(path(&fixture, new_id)).unwrap();
        let after = parse_strict_lossless_json_object(&after_bytes).unwrap();
        let old_field = field(1).to_string();
        let new_field = new_field.to_string();
        let old_option = option(1).to_string();
        let new_option = new_archived_option.to_string();
        let pairs = vec![
            (vec!["futureRoot"], vec!["futureRoot"]),
            (
                vec!["fields", &old_field, "futureField"],
                vec!["fields", &new_field, "futureField"],
            ),
            (
                vec![
                    "fields",
                    &old_field,
                    "configuration",
                    "options",
                    &old_option,
                    "futureOption",
                ],
                vec![
                    "fields",
                    &new_field,
                    "configuration",
                    "options",
                    &new_option,
                    "futureOption",
                ],
            ),
            (
                vec!["fields", &old_field, "defaultValue", "futureCurrent"],
                vec!["fields", &new_field, "defaultValue", "futureCurrent"],
            ),
            (
                vec!["fields", &old_field, "initialDefaultValue", "futureInitial"],
                vec!["fields", &new_field, "initialDefaultValue", "futureInitial"],
            ),
        ];
        for (src, dst) in pairs {
            let original = before.object_path(&src);
            assert!(original.is_some());
            let copied = after.object_path(&dst);
            assert!(copied.is_some());
            let original = serde_json::to_vec(original.unwrap()).unwrap();
            let copied = serde_json::to_vec(copied.unwrap()).unwrap();
            assert!(original.windows(6).any(|w| w == b"1E+100"));
            assert!(original == copied);
        }
        assert!(old_choice.default_value() != old_choice.initial_default_value());
        assert!(new_choice.default_value() != new_choice.initial_default_value());
        let decoded: serde_json::Value = serde_json::from_slice(&after_bytes).unwrap();
        assert!(
            decoded["fields"][&new_field]["initialDefaultValue"]["optionId"].as_str()
                == Some(new_option.as_str())
        );
        assert!(fs::read(path(&fixture, old.template_id())).unwrap() == bytes);
        for printed in [
            format!("{input:?}"),
            format!("{ticket:?}"),
            format!("{result:?}"),
        ] {
            assert!(!printed.contains(CANARY));
            assert!(!printed.contains("futureRoot"));
            assert!(!printed.contains("1E+100"));
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
#[test]
fn g6_duplicate_preparation_and_execution_reread_exact_source_bytes() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let id = seed(&mut runtime);
    let original = fs::read(path(&fixture, id)).unwrap();
    let input = DuplicateTemplateInput {
        source: source(&mut runtime, id),
        timestamp_utc: LATER.into(),
    };
    let mut ticket = prepare_duplicate_template(&mut runtime, &input).unwrap();
    let new_id = ticket.template_id();
    let ptr = ticket.candidate() as *const _;
    let mut changed = original.clone();
    changed.push(b' ');
    fs::write(path(&fixture, id), &changed).unwrap();
    assert!(prepare_duplicate_template(&mut runtime, &input).is_err());
    let mut session: Session = begin(&runtime, ticket.session_targets());
    let snap = session.snapshot();
    let (result, c, k) =
        observe(|| duplicate_template(&mut runtime, &mut session, context(&snap), &mut ticket));
    assert_no_io(&result, c, k);
    assert_eq!(ticket.state(), CreationState::Uncommitted);
    assert_eq!(new_id, ticket.template_id());
    assert_eq!(ptr, ticket.candidate() as *const _);
    assert!(!path(&fixture, new_id).exists());
    assert!(fs::read(path(&fixture, id)).unwrap() == changed);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
