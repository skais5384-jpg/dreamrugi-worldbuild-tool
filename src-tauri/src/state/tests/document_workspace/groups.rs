use super::*;

const GROUP: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000001";
const NUMBER: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000002";
const RICH: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000003";
const IMAGE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000004";
const FILE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000005";
const RELATION: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000006";
const LINK: &str = "aaaaaaaa-aaaa-4aaa-8aaa-000000000007";
const A: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000001";
const B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000002";
const C: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000003";

fn definition(kind: &str) -> Value {
    json!({"label":"동일 이름","kind":kind,"required":false,"lifecycle":"active","introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"configuration":{"kind":kind},"presentation":{}})
}
pub(super) fn template(h: &Harness, p: &str) -> String {
    let (id, _) = h.template(p);
    let path = h.root.join(format!("templates/{id}.json"));
    let mut t: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    t["fieldOrder"] = json!([GROUP]);
    let mut group = definition("group");
    group["configuration"] = json!({"kind":"group","memberOrder":[RICH,NUMBER,IMAGE,FILE],"members":{RICH:definition("richText"),NUMBER:definition("number"),IMAGE:definition("image"),FILE:definition("file")}});
    t["fields"][GROUP] = group;
    fs::write(path, serde_json::to_vec(&t).unwrap()).unwrap();
    id
}
fn reference_template(h: &Harness, p: &str) -> String {
    let id = template(h, p);
    let path = h.root.join(format!("templates/{id}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["fields"][GROUP]["configuration"]["memberOrder"] =
        json!([RICH, NUMBER, IMAGE, FILE, RELATION, LINK]);
    let mut relation = definition("relation");
    relation["configuration"] = json!({
        "kind":"relation",
        "multiple":true,
        "allowedTemplates":[],
        "reciprocalNotice":true
    });
    let mut link = definition("documentLink");
    link["configuration"] = json!({"kind":"documentLink"});
    wire["fields"][GROUP]["configuration"]["members"][RELATION] = relation;
    wire["fields"][GROUP]["configuration"]["members"][LINK] = link;
    fs::write(path, serde_json::to_vec(&wire).unwrap()).unwrap();
    id
}
fn card(id: &str, source: Option<&str>, n: Option<&str>) -> Value {
    json!({"id":id,"source":source,"fields":n.map(|n| vec![json!({"field":NUMBER,"value":{"intent":"set","value":{"kind":"number","value":n}}})]).unwrap_or_default()})
}
fn body(cards: Vec<Value>) -> Value {
    json!({"name":{"intent":"keep"},"fields":[{"field":GROUP,"value":{"intent":"set","value":{"kind":"group","instances":cards}}}],"composing":false})
}
fn disk(h: &Harness, id: &str) -> Value {
    serde_json::from_slice(&fs::read(h.root.join(format!("documents/{id}.json"))).unwrap()).unwrap()
}

#[test]
fn edit_begin_persists_missing_optional_group_as_canonical_empty_value() {
    let h = Harness::new();
    let project = h.open();
    let template = template(&h, &project);
    let document = create(&h, &project, &template, "missing optional group");
    let path = h.root.join(format!("documents/{document}.json"));
    let mut raw = disk(&h, &document);
    raw["fieldValues"].as_object_mut().unwrap().remove(GROUP);
    fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();

    let editing = edit_begin(&h, &project, &document);
    assert_eq!(editing["problem"], Value::Null, "{editing}");
    assert_eq!(
        editing["read"]["fields"][0]["value"]["kind"], "group",
        "{editing}"
    );
    let repaired = disk(&h, &document);
    assert_eq!(
        repaired["fieldValues"][GROUP],
        json!({"kind":"group","instanceOrder":[],"instances":{}})
    );
    edit_release(&h, &project, &editing);
    h.close_clean();
}

#[test]
fn older_template_binding_with_present_group_can_edit_and_save() {
    let h = Harness::new();
    let project = h.open();
    let template = template(&h, &project);
    let document = create(&h, &project, &template, "older group document");
    let document_path = h.root.join(format!("documents/{document}.json"));
    let mut old = disk(&h, &document);
    old["fieldValues"][GROUP] = json!({"kind":"group","instanceOrder":[],"instances":{}});
    fs::write(&document_path, serde_json::to_vec(&old).unwrap()).unwrap();

    let template_path = h.root.join(format!("templates/{template}.json"));
    let mut current: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    current["revision"] = 2.into();
    fs::write(&template_path, serde_json::to_vec(&current).unwrap()).unwrap();

    let editing = edit_begin(&h, &project, &document);
    assert_eq!(editing["problem"], Value::Null, "{editing}");
    assert_eq!(
        disk(&h, &document),
        old,
        "edit begin must not rewrite historical data"
    );
    let saved = edit_save(
        &h,
        &project,
        &editing,
        "2",
        body(vec![card(A, None, Some("1"))]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(disk(&h, &document)["templateRevision"], 2);
    edit_release(&h, &project, &saved);
    h.close_clean();
}

#[test]
fn m42_fix001_group_references_save_reopen_and_copy_with_fresh_connection_identity() {
    const OLD_CONNECTION: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    const NEW_CONNECTION: &str = "cccccccc-cccc-4ccc-8ccc-000000000002";
    let h = Harness::new();
    let p = h.open();
    let t = reference_template(&h, &p);

    let target = create(&h, &p, &t, "target");
    let source = create(&h, &p, &t, "source");
    let relation_cell = |connection: &str, one_way: bool, name: &str| {
        json!({"field":RELATION,"value":{"intent":"set","value":{
            "kind":"relation",
            "links":[{"id":connection,"document":target,"oneWay":one_way,"name":name}]
        }}})
    };
    let link_cell = || {
        json!({"field":LINK,"value":{"intent":"set","value":{
            "kind":"document_link","documents":[target]
        }}})
    };

    let editing = edit_begin(&h, &p, &source);
    let mut original = card(A, None, None);
    original["fields"] = json!([relation_cell(OLD_CONNECTION, true, "친구"), link_cell()]);
    let saved = edit_save(&h, &p, &editing, "2", body(vec![original]));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);

    let editing = edit_begin(&h, &p, &source);
    let mut copied = card(B, Some(A), None);
    copied["fields"] = json!([relation_cell(NEW_CONNECTION, false, "친구")]);
    let saved = edit_save(
        &h,
        &p,
        &editing,
        "2",
        body(vec![card(A, Some(A), None), copied]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let raw = disk(&h, &source);
    assert_eq!(
        raw["fieldValues"][GROUP]["instances"][A]["values"][RELATION]["links"][0]["id"],
        OLD_CONNECTION
    );
    assert_eq!(
        raw["fieldValues"][GROUP]["instances"][B]["values"][RELATION]["links"][0]["id"],
        NEW_CONNECTION
    );
    assert_eq!(
        raw["fieldValues"][GROUP]["instances"][B]["values"][RELATION]["links"][0]["oneWay"],
        false
    );
    assert_eq!(
        raw["fieldValues"][GROUP]["instances"][B]["values"][RELATION]["links"][0]["name"],
        "친구"
    );
    assert_eq!(
        raw["fieldValues"][GROUP]["instances"][B]["values"][LINK]["documentIds"][0],
        target
    );
    let reopened = edit_begin(&h, &p, &source);
    assert_eq!(
        reopened["read"]["fields"][0]["value"]["instances"][1]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["field"] == RELATION)
            .unwrap()["value"]["value"]["links"][0]["name"],
        "친구"
    );
    edit_release(&h, &p, &reopened);

    let references =
        request(&h, &p, json!({"action":"references","document":target}))["value"].clone();
    let incoming = references["incoming"].as_array().unwrap();
    assert_eq!(incoming.len(), 4, "{references}");
    assert_eq!(
        incoming
            .iter()
            .filter(|entry| entry["kind"] == "relation")
            .count(),
        2
    );
    assert_eq!(
        incoming
            .iter()
            .filter(|entry| entry["kind"] == "document_link")
            .count(),
        2
    );
    assert!(incoming
        .iter()
        .all(|entry| entry["instance"] == A || entry["instance"] == B));

    let editing = edit_begin(&h, &p, &source);
    let mut renamed = card(A, Some(A), None);
    renamed["fields"] = json!([relation_cell(OLD_CONNECTION, true, "단짝")]);
    let renamed = edit_save(
        &h,
        &p,
        &editing,
        "2",
        body(vec![renamed, card(B, Some(B), None)]),
    );
    assert_eq!(renamed["outcome"]["disk"], "committed", "{renamed}");
    edit_release(&h, &p, &renamed);
    let references =
        request(&h, &p, json!({"action":"references","document":target}))["value"].clone();
    let roles = references["incoming"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "relation")
        .map(|entry| entry["relationName"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(roles.contains(&"단짝"), "{references}");
    assert!(roles.contains(&"친구"), "{references}");

    let editing = edit_begin(&h, &p, &source);
    let before = fs::read(h.root.join(format!("documents/{source}.json"))).unwrap();
    let mut self_card = card(A, Some(A), None);
    self_card["fields"] = json!([{
        "field":RELATION,
        "value":{"intent":"set","value":{
            "kind":"relation",
            "links":[{
                "id":"cccccccc-cccc-4ccc-8ccc-000000000003",
                "document":source,
                "oneWay":false
            }]
        }}
    }]);
    let self_denied = edit_save(
        &h,
        &p,
        &editing,
        "2",
        body(vec![self_card, card(B, Some(B), None)]),
    );
    assert_eq!(self_denied["problem"], "SelfRelation", "{self_denied}");
    assert_eq!(
        fs::read(h.root.join(format!("documents/{source}.json"))).unwrap(),
        before
    );
    let denied = edit_save(
        &h,
        &p,
        &self_denied,
        "3",
        body(vec![
            card(A, Some(A), None),
            card(B, Some(B), None),
            card(C, Some(A), None),
        ]),
    );
    assert_eq!(denied["problem"], "InvalidCopiedRelation", "{denied}");
    assert_eq!(
        fs::read(h.root.join(format!("documents/{source}.json"))).unwrap(),
        before
    );
    let deposited = request(
        &h,
        &p,
        json!({
            "action":"edit_deposit",
            "owner":denied["owner"],
            "generation":"4",
            "body":denied["body"]
        }),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &p, &deposited);
    h.close_clean();
}

#[test]
fn removing_one_relation_preserves_other_incoming_references_and_target() {
    const FIRST: &str = "cccccccc-cccc-4ccc-8ccc-000000000020";
    const SECOND: &str = "cccccccc-cccc-4ccc-8ccc-000000000021";
    let h = Harness::new();
    let project = h.open();
    let template = reference_template(&h, &project);
    let target = create(&h, &project, &template, "target");
    let source = create(&h, &project, &template, "source");
    let target_path = h.root.join(format!("documents/{target}.json"));
    let target_before = fs::read(&target_path).unwrap();
    let relation = |id: &str| {
        json!({"field":RELATION,"value":{"intent":"set","value":{
            "kind":"relation","links":[{"id":id,"document":target,"oneWay":false,"name":"known"}]
        }}})
    };
    let mut first = card(A, None, None);
    first["fields"] = json!([
        relation(FIRST),
        {"field":LINK,"value":{"intent":"set","value":{"kind":"document_link","documents":[target]}}}
    ]);
    let mut second = card(B, None, None);
    second["fields"] = json!([relation(SECOND)]);
    let editing = edit_begin(&h, &project, &source);
    let saved = edit_save(&h, &project, &editing, "2", body(vec![first, second]));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &project, &saved);
    let incoming = request(
        &h,
        &project,
        json!({"action":"references","document":target}),
    )["value"]["incoming"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(incoming.len(), 3);

    let editing = edit_begin(&h, &project, &source);
    let mut first = card(A, Some(A), None);
    first["fields"] = json!([{"field":RELATION,"value":{"intent":"set","value":{
        "kind":"relation","links":[]
    }}}]);
    let saved = edit_save(
        &h,
        &project,
        &editing,
        "2",
        body(vec![first, card(B, Some(B), None)]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &project, &saved);
    let stored = disk(&h, &source);
    assert!(!stored.to_string().contains(FIRST));
    assert_eq!(
        stored["fieldValues"][GROUP]["instances"][B]["values"][RELATION]["links"][0]["id"],
        SECOND
    );
    assert_eq!(
        stored["fieldValues"][GROUP]["instances"][A]["values"][LINK]["documentIds"][0],
        target
    );
    assert_eq!(fs::read(&target_path).unwrap(), target_before);
    let reopened = edit_begin(&h, &project, &source);
    assert_eq!(reopened["problem"], Value::Null, "{reopened}");
    edit_release(&h, &project, &reopened);
    let incoming = request(
        &h,
        &project,
        json!({"action":"references","document":target}),
    )["value"]["incoming"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(incoming.len(), 2);
    let relations: Vec<_> = incoming
        .iter()
        .filter(|item| item["kind"] == "relation")
        .collect();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0]["connection"], SECOND);
    assert_eq!(relations[0]["source"], source);
    assert_eq!(relations[0]["target"], target);
    let links: Vec<_> = incoming
        .iter()
        .filter(|item| item["kind"] == "document_link")
        .collect();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["source"], source);
    assert_eq!(links[0]["target"], target);
    h.close_clean();
}

#[test]
fn m42_fix002_group_relation_name_survives_cold_recovery_and_save() {
    const CONNECTION: &str = "cccccccc-cccc-4ccc-8ccc-000000000010";
    let base = std::env::temp_dir().join(format!(
        "worldbuild-m42-fix002-relation-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let project = h.open();
    let template = reference_template(&h, &project);
    let target = create(&h, &project, &template, "target");
    let source = create(&h, &project, &template, "source");
    let relation = |name: &str| {
        json!({"field":RELATION,"value":{"intent":"set","value":{
            "kind":"relation",
            "links":[{
                "id":CONNECTION,
                "document":target,
                "oneWay":false,
                "name":name
            }]
        }}})
    };
    let editing = edit_begin(&h, &project, &source);
    let mut original = card(A, None, None);
    original["fields"] = json!([relation("친구")]);
    let saved = edit_save(&h, &project, &editing, "2", body(vec![original]));
    edit_release(&h, &project, &saved);

    let editing = edit_begin(&h, &project, &source);
    let mut changed = card(A, Some(A), None);
    changed["fields"] = json!([relation("단짝")]);
    let recovered_body = body(vec![changed]);
    let deposited = request(
        &h,
        &project,
        json!({
            "action":"edit_deposit",
            "owner":editing["owner"],
            "generation":"2",
            "body":recovered_body
        }),
    )["value"]
        .clone();
    assert_eq!(deposited["deposited"], true, "{deposited}");
    edit_release(&h, &project, &deposited);
    h.close_clean();
    drop(h);

    let h = Harness::at(base, backend::provider(), true);
    let project = h.open();
    let restored = edit_begin(&h, &project, &source);
    let relation = restored["body"]["fields"][0]["value"]["value"]["instances"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["field"] == RELATION)
        .unwrap();
    assert_eq!(
        relation["value"]["value"]["links"][0]["name"], "단짝",
        "{restored}"
    );
    let generation = (restored["generation"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        + 1)
    .to_string();
    let saved = edit_save(
        &h,
        &project,
        &restored,
        &generation,
        restored["body"].clone(),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, &source)["fieldValues"][GROUP]["instances"][A]["values"][RELATION]["links"][0]
            ["name"],
        "단짝"
    );
    edit_release(&h, &project, &saved);
    h.close_clean();
}

#[test]
fn m38_fix_required_absent_child_allows_save_without_changing_required_definition() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "required absence");
    let e = edit_begin(&h, &p, &id);
    let saved = edit_save(&h, &p, &e, "2", body(vec![card(A, None, Some("1"))]));
    edit_release(&h, &p, &saved);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut tmpl: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    let new_child = "aaaaaaaa-aaaa-4aaa-8aaa-000000000006";
    tmpl["revision"] = 2.into();
    let mut required = definition("number");
    required["required"] = true.into();
    required["introducedRevision"] = 2.into();
    tmpl["fields"][GROUP]["configuration"]["members"][new_child] = required;
    tmpl["fields"][GROUP]["configuration"]["memberOrder"]
        .as_array_mut()
        .unwrap()
        .push(new_child.into());
    fs::write(&tp, serde_json::to_vec(&tmpl).unwrap()).unwrap();
    let path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&path).unwrap();
    let e = edit_begin(&h, &p, &id);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "read must not materialize"
    );
    let b = json!({"name":{"intent":"set","value":"renamed"},"fields":[],"composing":false});
    let denied = edit_save(&h, &p, &e, "2", b.clone());
    assert_eq!(denied["saved_generation"], denied["generation"], "{denied}");
    assert_eq!(denied["body"], b);
    assert!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][A]["values"]
            .get(new_child)
            .is_none(),
        "name-only save preserves absent historical child instead of inventing its value"
    );
    let read = request(&h, &p, json!({"action":"read","document":id}));
    assert!(
        read["value"]["fields"].to_string().contains(new_child),
        "missing required field remains visible in normal read"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&tp).unwrap()).unwrap()["fields"][GROUP]
            ["configuration"]["members"][new_child]["required"],
        true
    );
    let mut fixed = body(vec![card(A, Some(A), None)]);
    fixed["name"] = b["name"].clone();
    fixed["fields"][0]["value"]["value"]["instances"][0]["fields"] =
        json!([{"field":new_child,"value":{"intent":"set","value":{"kind":"number","value":"0"}}}]);
    let saved = edit_save(&h, &p, &denied, "3", fixed);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][A]["values"][new_child]["value"],
        "0"
    );
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_fix_final_save_checks_effective_absence_and_preserves_historical_controls() {
    use crate::data::artifact::{
        decode_document, decode_template, encode_document, prepare_document_save,
        DocumentEdit as Edit, DocumentEditSet, DocumentValueEdit,
    };
    const NEW: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    const OTHER: &str = "cccccccc-cccc-4ccc-8ccc-000000000002";
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "final boundary");
    let e = edit_begin(&h, &p, &id);
    let saved = edit_save(&h, &p, &e, "2", body(vec![card(A, None, Some("1"))]));
    edit_release(&h, &p, &saved);
    let original = disk(&h, &id);
    let template: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("templates/{t}.json"))).unwrap())
            .unwrap();
    for case in [
        "missing",
        "explicit unset",
        "optional",
        "empty group",
        "historical zero",
        "historical unknown",
        "current missing",
        "archived",
    ] {
        let mut tmpl = template.clone();
        let mut doc = original.clone();
        tmpl["revision"] = 2.into();
        let mut child = definition("number");
        child["required"] = true.into();
        child["introducedRevision"] = 2.into();
        if case == "optional" {
            child["required"] = false.into();
        }
        if case == "historical zero" {
            child["initialDefaultValue"] = json!({"kind":"number","value":"0"});
        }
        if case == "historical unknown" {
            child["initialDefaultValue"] = json!({"kind":"numberUnknown"});
        }
        if case == "current missing" {
            child["introducedRevision"] = 1.into();
            child["initialDefaultValue"] = json!({"kind":"number","value":"9"});
        }
        if case == "archived" {
            child["lifecycle"] = "archived".into();
        }
        tmpl["fields"][GROUP]["configuration"]["members"][NEW] = child;
        if case != "archived" {
            tmpl["fields"][GROUP]["configuration"]["memberOrder"]
                .as_array_mut()
                .unwrap()
                .push(NEW.into());
        }
        let mut other = definition("singleLineText");
        other["introducedRevision"] = 2.into();
        tmpl["fields"][OTHER] = other;
        tmpl["fieldOrder"]
            .as_array_mut()
            .unwrap()
            .push(OTHER.into());
        if case == "explicit unset" {
            doc["fieldValues"][GROUP]["instances"][A]["values"][NEW] = json!({"kind":"unset"});
            doc["fieldValues"][GROUP]["instances"][A]["labels"][NEW] = "required".into();
        }
        if case == "empty group" {
            doc["fieldValues"][GROUP]["instanceOrder"] = json!([]);
            doc["fieldValues"][GROUP]["instances"] = json!({});
        }
        let template = decode_template(&serde_json::to_vec(&tmpl).unwrap()).unwrap();
        let document = decode_document(&serde_json::to_vec(&doc).unwrap()).unwrap();
        let before = encode_document(&document).unwrap();
        for edit in [
            Edit::Rename("changed name".into()),
            Edit::SetValue(
                OTHER.parse().unwrap(),
                DocumentValueEdit::single_line_text("other value".into()),
            ),
        ] {
            let result = prepare_document_save(
                &template,
                template.revision(),
                &document,
                &DocumentEditSet::new(vec![edit]),
                // 이 회귀는 실행 시각에 의존하지 않도록 생성 fixture보다 확실히 뒤인 시각을 쓴다.
                "2099-09-18T00:00:00.000Z",
            );
            assert_eq!(result.is_ok(), true, "{case}: {result:?}");
            assert_eq!(
                encode_document(&document).unwrap(),
                before,
                "{case}: input changed"
            );
            if let Ok(saved) = result {
                let bytes = encode_document(saved.document()).unwrap();
                assert!(decode_document(&bytes).is_ok(), "{case}");
            }
        }
    }
    h.close_clean();
}

#[test]
fn m38_cards_create_duplicate_independent_reorder_delete_zero() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let d = begin(&h, &p, &t);
    let mut b = d["body"].clone();
    b["name"] = "반복".into();
    b["fields"] = body(vec![card(A, None, Some("0"))])["fields"].clone();
    let saved = save(&h, &p, &d, "2", b);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let id = saved["outcome"]["artifact"].as_str().unwrap();
    release(&h, &p, &saved, false);
    let e = edit_begin(&h, &p, id);
    let saved = edit_save(
        &h,
        &p,
        &e,
        "2",
        body(vec![card(B, Some(A), Some("2")), card(A, Some(A), None)]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let raw = disk(&h, id);
    let g = &raw["fieldValues"][GROUP];
    assert_eq!(g["instanceOrder"], json!([B, A]));
    assert_eq!(g["instances"][A]["values"][NUMBER]["value"], "0");
    assert_eq!(g["instances"][B]["values"][NUMBER]["value"], "2");
    let saved = edit_save(&h, &p, &saved, "3", body(vec![]));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, id)["fieldValues"][GROUP]["instanceOrder"],
        json!([])
    );
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_fix_clone_next_save_raw_assets_and_cold_draft_preserve_captured_value() {
    const C: &str = "bbbbbbbb-bbbb-4bbb-8bbb-000000000003";
    let base = std::env::temp_dir().join(format!("worldbuild-m38-fix-{}", uuid::Uuid::new_v4()));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "clone checkpoint");
    let e = edit_begin(&h, &p, &id);
    let source = h.root.join("source.txt");
    fs::write(&source, b"shared clone attachment").unwrap();
    let store = crate::data::assets::Store::open(&h.root, true).unwrap();
    let asset = store
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let mut a = card(A, None, Some("1"));
    a["fields"].as_array_mut().unwrap().push(
        json!({"field":FILE,"value":{"intent":"set","value":{"kind":"file","value":[asset.id]}}}),
    );
    let saved = edit_save(&h, &p, &e, "2", body(vec![a]));
    edit_release(&h, &p, &saved);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut raw = disk(&h, &id);
    raw["fieldValues"][GROUP]["instances"][A]["future"] = "RAW_NUMBER".into();
    fs::write(
        &path,
        serde_json::to_string(&raw)
            .unwrap()
            .replace("\"RAW_NUMBER\"", "1.2300e+10"),
    )
    .unwrap();
    let e = edit_begin(&h, &p, &id);
    let saved = edit_save(
        &h,
        &p,
        &e,
        "2",
        body(vec![card(A, Some(A), Some("2")), card(B, Some(A), None)]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][B]["values"][NUMBER]["value"],
        "1"
    );
    let mut c = card(C, Some(B), None);
    c["lineage"] = json!([B]);
    // 응답에서 결정한 exact parent/source를 typed 초안에 보관한다.
    let b = body(vec![c, card(B, Some(B), None), card(A, Some(A), None)]);
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":saved["owner"],"generation":"3","body":b}),
    )["value"]
        .clone();
    assert_eq!(dep["deposited"], true, "{dep}");
    edit_release(&h, &p, &dep);
    h.close_clean();
    drop(store);
    drop(h);
    let h = Harness::at(base, backend::provider(), true);
    let p = h.open();
    let restored = edit_begin(&h, &p, &id);
    let cards = &restored["body"]["fields"][0]["value"]["value"]["instances"];
    assert_eq!(cards[0]["id"], C);
    assert_eq!(cards[0]["source"], B);
    assert_eq!(cards[1]["id"], B);
    assert_eq!(cards[2]["id"], A);

    let generation = (restored["generation"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        + 1)
    .to_string();
    let saved = edit_save(&h, &p, &restored, &generation, restored["body"].clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let result = disk(&h, &id);
    assert_eq!(
        result["fieldValues"][GROUP]["instances"][C]["values"][NUMBER]["value"],
        "1"
    );
    assert_eq!(
        result["fieldValues"][GROUP]["instances"][C]["values"][FILE]["value"],
        json!([asset.id])
    );
    assert_eq!(
        fs::read_to_string(h.root.join(format!("documents/{id}.json")))
            .unwrap()
            .matches("1.2300e+10")
            .count(),
        3
    );
    let store = crate::data::assets::Store::open(&h.root, true).unwrap();
    assert_eq!(store.read(&asset.id).unwrap().1, b"shared clone attachment");
    drop(store);
    edit_release(&h, &p, &saved);
    let reopened = edit_begin(&h, &p, &id);
    assert_eq!(
        reopened["read"]["fields"][0]["value"]["instances"][0]["id"],
        C
    );
    edit_release(&h, &p, &reopened);
    h.close_clean();
}

#[test]
fn m38_fix_committed_clone_receipt_rebases_only_with_own_commit_proof() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "committed receipt");
    let e = edit_begin(&h, &p, &id);
    let initial = edit_save(&h, &p, &e, "2", body(vec![card(A, None, Some("1"))]));
    edit_release(&h, &p, &initial);
    let e = edit_begin(&h, &p, &id);
    let sent = body(vec![card(A, Some(A), Some("2")), card(B, Some(A), None)]);
    let saved = edit_save(&h, &p, &e, "2", sent.clone());
    let before = fs::read(h.root.join(format!("documents/{id}.json"))).unwrap();
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":saved["owner"],"generation":"2","body":sent}),
    )["value"]
        .clone();
    assert_eq!(dep["deposited"], true, "{dep}");
    edit_release(&h, &p, &dep);
    let restored = edit_begin(&h, &p, &id);
    assert_eq!(
        restored["read"]["fields"][0]["value"]["instances"][1]["source"], B,
        "{restored}"
    );
    assert_eq!(
        fs::read(h.root.join(format!("documents/{id}.json"))).unwrap(),
        before
    );
    assert_eq!(
        restored["body"]["fields"],
        json!([]),
        "already-committed content has no pending edits"
    );
    assert_eq!(restored["saved_generation"], restored["generation"]);
    let saved = edit_save(&h, &p, &restored, "4", restored["body"].clone());
    assert!(saved["problem"].is_null(), "{saved}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][B]["values"][NUMBER]["value"],
        "1"
    );
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_invalid_raw_required_duplicate_foreign_and_nested_keep_original() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "empty");
    let path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&path).unwrap();
    let e = edit_begin(&h, &p, &id);
    let cases = vec![
        body(vec![card(A, None, Some("-"))]),
        body(vec![card(A, None, None), card(A, None, None)]),
        body(vec![card(A, Some(B), None)]),
        body(vec![
            json!({"id":A,"source":null,"fields":[{"field":GROUP,"value":{"intent":"unset"}}]}),
        ]),
        body(vec![
            json!({"id":A,"source":null,"fields":[{"field":NUMBER,"value":{"intent":"set","value":{"kind":"group","instances":[]}}}]}),
        ]),
    ];
    let mut current = e;
    for (index, b) in cases.into_iter().enumerate() {
        let response = request(
            &h,
            &p,
            json!({"action":"edit_draft","owner":current["owner"],"generation":(index + 2).to_string(),"body":b,"save":true}),
        );
        let r = response["value"].clone();
        if r["kind"] == "editing" {
            assert!(r["saved_generation"] != r["generation"], "{r}");
            assert_eq!(r["body"], b);
            current = r;
        } else {
            // Forged nested groups exceed the admitted raw envelope shape. The
            // owner retains the input; the previous durable checkpoint stays valid.
            assert_eq!(response["error"]["code"], "recovery_rejected", "{response}");
            assert_eq!(response["input_retained"], true);
        }
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let saved = edit_save(&h, &p, &current, "20", body(vec![]));
    edit_release(&h, &p, &saved);
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut v: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    v["fields"][GROUP]["configuration"]["members"][NUMBER]["required"] = true.into();
    fs::write(&tp, serde_json::to_vec(&v).unwrap()).unwrap();
    let e = edit_begin(&h, &p, &id);
    let denied = edit_save(&h, &p, &e, "2", body(vec![card(A, None, None)]));
    assert_eq!(denied["saved_generation"], denied["generation"], "{denied}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][A]["values"][NUMBER],
        json!({"kind":"unset"})
    );
    let saved = edit_save(&h, &p, &denied, "3", body(vec![]));
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_duplicate_preserves_unknown_raw_and_protects_rich_child() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "raw");
    let e = edit_begin(&h, &p, &id);
    let saved = edit_save(&h, &p, &e, "2", body(vec![card(A, None, Some("0"))]));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &p, &saved);
    let path = h.root.join(format!("documents/{id}.json"));
    let mut raw = disk(&h, &id);
    raw["fieldValues"][GROUP]["instances"][A]["future"] = json!("RAW_NUMBER");
    raw["fieldValues"][GROUP]["instances"][A]["values"][RICH] = json!({"kind":"richText","document":{"schemaVersion":1,"content":{"kind":"root","children":[{"kind":"paragraph","future":"opaque","children":[{"kind":"text","text":"원문"}]}]}}});
    let bytes = serde_json::to_string(&raw)
        .unwrap()
        .replace("\"RAW_NUMBER\"", "1.2300e+10");
    fs::write(&path, bytes.as_bytes()).unwrap();
    let e = edit_begin(&h, &p, &id);
    assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());
    let saved = edit_save(
        &h,
        &p,
        &e,
        "2",
        body(vec![card(A, Some(A), None), card(B, Some(A), Some("7"))]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let raw = fs::read_to_string(&path).unwrap();
    assert_eq!(raw.matches("1.2300e+10").count(), 2);
    assert_eq!(raw.matches("opaque").count(), 2);
    let mut bad = body(vec![card(A, Some(A), None), card(B, Some(B), None)]);
    bad["fields"][0]["value"]["value"]["instances"][1]["fields"] =
        json!([{"field":RICH,"value":{"intent":"unset"}}]);
    let denied = edit_save(&h, &p, &saved, "3", bad);
    assert_eq!(denied["problem"], "ProtectedChild");
    assert_eq!(fs::read_to_string(&path).unwrap(), raw);
    let saved = edit_save(&h, &p, &denied, "4", body(vec![card(B, Some(B), None)]));
    assert_eq!(saved["outcome"]["disk"], "committed");
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_group_definition_rejects_duplicate_nested_and_legacy_headers() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let path = h.root.join(format!("templates/{t}.json"));
    let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let decode =
        |v: &Value| crate::data::artifact::decode_template(&serde_json::to_vec(v).unwrap());
    assert!(decode(&raw).is_ok());
    let mut old = raw.clone();
    old["schemaVersion"] = 4.into();
    assert!(decode(&old).is_err());
    let mut dup = raw.clone();
    dup["fields"][NUMBER] = definition("number");
    dup["fieldOrder"] = json!([GROUP, NUMBER]);
    assert!(decode(&dup).is_err());
    let mut nested = raw.clone();
    nested["fields"][GROUP]["configuration"]["members"][NUMBER] = raw["fields"][GROUP].clone();
    assert!(decode(&nested).is_err());
    let mut order = raw.clone();
    order["fields"][GROUP]["configuration"]["memberOrder"] =
        json!([NUMBER, NUMBER, RICH, IMAGE, FILE]);
    assert!(decode(&order).is_err());
    h.close_clean();
}

#[test]
fn m38_draft_store_restart_preserves_invalid_raw_order_and_restores_save() {
    let base = std::env::temp_dir().join(format!("worldbuild-m38-{}", uuid::Uuid::new_v4()));
    let h = Harness::at(base.clone(), backend::provider(), false);
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "recovery");
    let e = edit_begin(&h, &p, &id);
    let b = body(vec![card(B, None, Some("-")), card(A, None, Some("0"))]);
    let dep = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":e["owner"],"generation":"2","body":b}),
    )["value"]
        .clone();
    assert_eq!(dep["deposited"], true, "{dep}");
    edit_release(&h, &p, &dep);
    h.close_clean();
    drop(h);
    let h = Harness::at(base, backend::provider(), true);
    let p = h.open();
    let restored = edit_begin(&h, &p, &id);
    assert_eq!(restored["body"], b, "{restored}");
    let mut fixed = b;
    fixed["fields"][0]["value"]["value"]["instances"][0]["fields"][0]["value"]["value"]["value"] =
        "8".into();
    let generation = (restored["generation"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        + 1)
    .to_string();
    let saved = edit_save(&h, &p, &restored, &generation, fixed);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instanceOrder"],
        json!([B, A])
    );
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn m38_shared_assets_missing_bytes_and_scoped_import_admission() {
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "assets");
    let e = edit_begin(&h, &p, &id);
    let source = h.root.join("source.txt");
    fs::write(&source, b"group attachment").unwrap();
    let store = crate::data::assets::Store::open(&h.root, true).unwrap();
    let asset = store
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let mut a = card(A, None, Some("0"));
    a["fields"].as_array_mut().unwrap().push(
        json!({"field":FILE,"value":{"intent":"set","value":{"kind":"file","value":[asset.id]}}}),
    );
    let saved = edit_save(&h, &p, &e, "2", body(vec![a]));
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let saved = edit_save(
        &h,
        &p,
        &saved,
        "3",
        body(vec![card(A, Some(A), None), card(B, Some(A), None)]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let saved = edit_save(&h, &p, &saved, "4", body(vec![card(B, Some(B), None)]));
    assert_eq!(saved["outcome"]["disk"], "committed");
    assert_eq!(store.read(&asset.id).unwrap().1, b"group attachment");
    let import = |instance: &str, child: &str, image: bool| {
        request(
            &h,
            &p,
            json!({"action":"asset_import","owner":saved["owner"],"generation":"4","field":GROUP,"cell":{"instance":instance,"child":child},"image":image}),
        )
    };
    assert_eq!(import(A, FILE, false)["error"]["code"], "wrong_binding");
    assert_eq!(import(B, IMAGE, false)["error"]["code"], "wrong_binding");
    assert_eq!(import(B, NUMBER, false)["error"]["code"], "wrong_binding");
    // A valid scoped target reaches the absent test picker; wrong targets are denied before it.
    assert_eq!(import(B, FILE, false)["error"]["code"], "unavailable");
    let path = h.root.join(format!("documents/{id}.json"));
    let binary = h.root.join("assets").join(&asset.id).join("content.txt");
    fs::remove_file(&binary).unwrap();
    let changed = body(vec![card(B, Some(B), Some("7"))]);
    let saved = edit_save(&h, &p, &saved, "5", changed.clone());
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    assert_eq!(
        disk(&h, &id)["fieldValues"][GROUP]["instances"][B]["values"][FILE]["value"],
        json!([asset.id])
    );
    assert!(
        !binary.exists(),
        "keeping a missing attachment must not recreate or pin bytes"
    );
    let before = fs::read(&path).unwrap();
    // A new attachment reference requires actual bytes, unlike Keep above.
    let missing = store
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    fs::remove_file(h.root.join("assets").join(&missing.id).join("content.txt")).unwrap();
    let mut new_reference = changed.clone();
    new_reference["fields"][0]["value"]["value"]["instances"][0]["fields"].as_array_mut().unwrap().push(
        json!({"field":FILE,"value":{"intent":"set","value":{"kind":"file","value":[missing.id]}}})
    );
    let failed = edit_save(&h, &p, &saved, "6", new_reference);
    assert_ne!(failed["outcome"]["disk"], "committed", "{failed}");
    assert_eq!(fs::read(&path).unwrap(), before);
    store.put(&asset, b"group attachment").unwrap();
    let kept = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":saved["owner"],"generation":"7","body":changed}),
    )["value"]
        .clone();
    assert_eq!(kept["deposited"], true, "{kept}");
    edit_release(&h, &p, &kept);
    h.close_clean();
}

#[test]
fn m38_child_history_absence_raw_and_archived_label_survive_read_then_save() {
    const NEW: &str = "cccccccc-cccc-4ccc-8ccc-000000000001";
    let h = Harness::new();
    let p = h.open();
    let t = template(&h, &p);
    let id = create(&h, &p, &t, "history");
    let e = edit_begin(&h, &p, &id);
    let saved = edit_save(&h, &p, &e, "2", body(vec![card(A, None, Some("3"))]));
    edit_release(&h, &p, &saved);
    let path = h.root.join(format!("documents/{id}.json"));
    let before = fs::read(&path).unwrap();
    let tp = h.root.join(format!("templates/{t}.json"));
    let mut raw: Value = serde_json::from_slice(&fs::read(&tp).unwrap()).unwrap();
    raw["revision"] = 2.into();
    let members = &mut raw["fields"][GROUP]["configuration"]["members"];
    members[NUMBER]["lifecycle"] = "archived".into();
    members[NUMBER]["label"] = "현재 보관 이름".into();
    let mut new = definition("number");
    new["introducedRevision"] = 2.into();
    new["initialDefaultValue"] = json!({"kind":"number","value":"7","future":"RAW_NUMBER"});
    new["defaultValue"] = json!({"kind":"number","value":"9"});
    members[NEW] = new;
    raw["fields"][GROUP]["configuration"]["memberOrder"] = json!([NEW, RICH, FILE, IMAGE]);
    fs::write(
        &tp,
        serde_json::to_string(&raw)
            .unwrap()
            .replace("\"RAW_NUMBER\"", "4.500e+3"),
    )
    .unwrap();
    let e = edit_begin(&h, &p, &id);
    assert_eq!(fs::read(&path).unwrap(), before);
    let group = &e["read"]["fields"][0]["value"];
    let fields = group["instances"][0]["fields"].as_array().unwrap();
    assert!(fields
        .iter()
        .any(|f| f["field"] == NEW && f["value"]["value"]["value"] == "7"));
    let saved = edit_save(
        &h,
        &p,
        &e,
        "2",
        body(vec![card(A, Some(A), None), card(B, None, None)]),
    );
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let raw = disk(&h, &id);
    let instances = &raw["fieldValues"][GROUP]["instances"];
    assert_eq!(instances[A]["values"][NEW]["value"], "7");
    assert_eq!(instances[B]["values"][NEW]["kind"], "unset");
    assert_eq!(instances[A]["values"][NUMBER]["value"], "3");
    assert_eq!(instances[A]["labels"][NUMBER], "동일 이름");
    assert!(fs::read_to_string(path).unwrap().contains("4.500e+3"));
    edit_release(&h, &p, &saved);
    h.close_clean();
}

#[test]
fn missing_optional_group_repair_never_writes_through_unavailable_local_input() {
    for marker in [
        "foreign-canary.json",
        "pending-owned-incomplete.json",
        "d00000000000000000001.json",
    ] {
        let h = Harness::new();
        let p = h.open();
        let t = template(&h, &p);
        let id = create(
            &h,
            &p,
            &t,
            "optional group must not repair before admission",
        );
        let path = h.root.join(format!("documents/{id}.json"));
        let mut raw = disk(&h, &id);
        raw["fieldValues"].as_object_mut().unwrap().remove(GROUP);
        let before = serde_json::to_vec(&raw).unwrap();
        fs::write(&path, &before).unwrap();
        let target = h
            .root
            .join(format!(".worldbuild/latest-drafts/document-{id}"));
        fs::create_dir_all(&target).unwrap();
        let obstacle = target.join(marker);
        fs::write(&obstacle, b"owned invalid input must remain intact").unwrap();
        let denied = request(&h, &p, json!({"action":"edit_begin","document":id}));
        assert_eq!(denied["error"]["code"], "sink_unavailable", "{denied}");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::read(&obstacle).unwrap(),
            b"owned invalid input must remain intact"
        );
        fs::remove_file(&obstacle).unwrap();
        let editing = edit_begin(&h, &p, &id);
        assert!(editing["problem"].is_null(), "{editing}");
        assert_eq!(
            disk(&h, &id)["fieldValues"][GROUP]["instanceOrder"],
            json!([])
        );
        edit_release(&h, &p, &editing);
        h.close_clean();
    }
}
