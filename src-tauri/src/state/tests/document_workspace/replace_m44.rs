use super::*;

fn create_terms(
    h: &Harness,
    project: &str,
    template: &str,
    name: &str,
    english: &str,
    summary: &str,
) -> String {
    let draft = begin(h, project, template);
    let mut body = draft["body"].clone();
    body["name"] = name.into();
    body["englishName"] = english.into();
    body["glossarySummary"] = summary.into();
    let saved = save(h, project, &draft, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let id = saved["outcome"]["artifact"].as_str().unwrap().to_owned();
    release(h, project, &saved, false);
    id
}

fn preview(h: &Harness, project: &str, find: &str, replacement: &str, scopes: Value) -> Value {
    request(
        h,
        project,
        json!({
            "action":"replace_preview",
            "find":find,
            "replacement":replacement,
            "template":null,
            "scopes":scopes,
            "caseSensitive":false,
            "wholeWord":false,
            "offset":0,
            "limit":100
        }),
    )["value"]
        .clone()
}

#[test]
fn m44_attribute_only_preview_and_apply_use_one_complete_write_set() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let a = create_terms(&h, &project, &template, "A", "Old Name", "keep");
    let b = create_terms(&h, &project, &template, "B", "keep", "old summary");
    let before_a = fs::read(h.root.join(format!("documents/{a}.json"))).unwrap();
    let before_b = fs::read(h.root.join(format!("documents/{b}.json"))).unwrap();

    let english = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":false,"body":false,"englishName":true,"glossarySummary":false}),
    );
    assert_eq!(english["totalDocuments"], 1, "{english}");
    assert_eq!(english["totalChanges"], 1, "{english}");
    assert_eq!(english["changes"][0]["scope"], "english_name");
    assert_eq!(english["changes"][0]["beforePrefix"], "");
    assert_eq!(english["changes"][0]["beforeMatch"], "Old");
    assert_eq!(english["changes"][0]["beforeSuffix"], " Name");
    assert_eq!(english["changes"][0]["afterPrefix"], "");
    assert_eq!(english["changes"][0]["afterMatch"], "new");
    assert_eq!(english["changes"][0]["afterSuffix"], " Name");
    assert!(english["blockers"].as_array().unwrap().is_empty());
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":english["preview"]}),
    );
    assert_eq!(applied["kind"], "write", "{applied}");
    assert_eq!(applied["disk"], "committed", "{applied}");
    assert_ne!(
        fs::read(h.root.join(format!("documents/{a}.json"))).unwrap(),
        before_a
    );
    assert_eq!(
        fs::read(h.root.join(format!("documents/{b}.json"))).unwrap(),
        before_b
    );
    let read_a = request(&h, &project, json!({"action":"read","document":a}));
    assert_eq!(read_a["value"]["englishName"], "new Name", "{read_a}");

    let summary = preview(
        &h,
        &project,
        "old summary",
        "",
        json!({"title":false,"body":false,"englishName":false,"glossarySummary":true}),
    );
    assert_eq!(summary["totalDocuments"], 1, "{summary}");
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":summary["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    let bytes = fs::read(h.root.join(format!("documents/{b}.json"))).unwrap();
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire.get("glossarySummary").is_none(), "{wire}");
    h.close_clean();
}

#[test]
fn m44_template_filter_and_scope_preserve_unselected_properties() {
    let h = Harness::new();
    let project = h.open();
    let (template_a, _) = h.template(&project);
    let (template_b, _) = h.template(&project);
    let a = create_terms(
        &h,
        &project,
        &template_a,
        "old title A",
        "old English A",
        "old summary A",
    );
    let b = create_terms(
        &h,
        &project,
        &template_b,
        "old title B",
        "old English B",
        "old summary B",
    );
    let prepared = request(
        &h,
        &project,
        json!({
            "action":"replace_preview","find":"old","replacement":"new",
            "template":template_a,
            "scopes":{"title":false,"body":false,"englishName":true,"glossarySummary":false},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }),
    )["value"]
        .clone();
    assert_eq!(prepared["totalDocuments"], 1, "{prepared}");
    assert_eq!(prepared["totalChanges"], 1, "{prepared}");
    assert_eq!(prepared["changes"][0]["document"], a, "{prepared}");
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");

    let read_a = request(&h, &project, json!({"action":"read","document":a}));
    assert_eq!(read_a["value"]["name"], "old title A", "{read_a}");
    assert_eq!(read_a["value"]["englishName"], "new English A", "{read_a}");
    assert_eq!(
        read_a["value"]["glossarySummary"], "old summary A",
        "{read_a}"
    );
    let read_b = request(&h, &project, json!({"action":"read","document":b}));
    assert_eq!(read_b["value"]["name"], "old title B", "{read_b}");
    assert_eq!(read_b["value"]["englishName"], "old English B", "{read_b}");
    assert_eq!(
        read_b["value"]["glossarySummary"], "old summary B",
        "{read_b}"
    );
    h.close_clean();
}

#[test]
fn m44_open_target_owner_blocks_every_document_before_write() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let a = create_terms(&h, &project, &template, "A", "old A", "keep");
    let b = create_terms(&h, &project, &template, "B", "old B", "keep");
    let before_a = fs::read(h.root.join(format!("documents/{a}.json"))).unwrap();
    let before_b = fs::read(h.root.join(format!("documents/{b}.json"))).unwrap();
    let editing = edit_begin(&h, &project, &a);
    let result = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":false,"body":false,"englishName":true,"glossarySummary":false}),
    );
    assert_eq!(result["totalDocuments"], 2, "{result}");
    assert!(!result["blockers"].as_array().unwrap().is_empty());
    let rejected = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":result["preview"]}),
    );
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    assert_eq!(
        fs::read(h.root.join(format!("documents/{a}.json"))).unwrap(),
        before_a
    );
    assert_eq!(
        fs::read(h.root.join(format!("documents/{b}.json"))).unwrap(),
        before_b
    );
    edit_release(&h, &project, &editing);
    let retried = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":false,"body":false,"englishName":true,"glossarySummary":false}),
    );
    assert!(
        retried["blockers"].as_array().unwrap().is_empty(),
        "{retried}"
    );
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":retried["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    for (id, english_name) in [(a, "new A"), (b, "new B")] {
        let read = request(&h, &project, json!({"action":"read","document":id}));
        assert_eq!(read["value"]["englishName"], english_name, "{read}");
        assert_eq!(read["value"]["glossarySummary"], "keep", "{read}");
    }
    h.close_clean();
}

#[test]
fn m44_general_search_finds_english_name_and_summary() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    create_terms(
        &h,
        &project,
        &template,
        "제목 불일치",
        "Unique English",
        "Unique Summary",
    );
    for (query, label) in [
        ("unique english", "영어 이름"),
        ("unique summary", "한줄 설명"),
    ] {
        let result = request(
            &h,
            &project,
            json!({"action":"search","query":query,"template":null,"offset":0,"limit":100,"refresh":true}),
        );
        assert_eq!(result["value"]["total"], 1, "{result}");
        assert_eq!(result["value"]["results"][0]["excerpt"]["label"], label);
    }
    h.close_clean();
}

#[test]
fn m44_two_documents_commit_together_and_a_preview_cannot_apply_twice() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let a = create(&h, &project, &template, "old A");
    let b = create(&h, &project, &template, "old B");
    let prepared = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":true,"body":false,"englishName":false,"glossarySummary":false}),
    );
    assert_eq!(prepared["totalDocuments"], 2, "{prepared}");
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    for (id, name) in [(a, "new A"), (b, "new B")] {
        let read = request(&h, &project, json!({"action":"read","document":id}));
        assert_eq!(read["value"]["name"], name, "{read}");
    }
    let duplicate = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(duplicate["kind"], "rejected", "{duplicate}");
    h.close_clean();
}

#[test]
fn m44_discarded_or_superseded_capability_cannot_apply_or_delete_the_new_one() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create(&h, &project, &template, "old title");
    let scopes = json!({
        "title":true,"body":false,"englishName":false,"glossarySummary":false
    });
    let old = preview(&h, &project, "old", "first", scopes.clone());
    let discarded = request(
        &h,
        &project,
        json!({"action":"replace_discard","preview":old["preview"]}),
    );
    assert_eq!(discarded["value"]["kind"], "released", "{discarded}");
    let rejected = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":old["preview"]}),
    );
    assert_eq!(rejected["error"]["code"], "wrong_binding", "{rejected}");

    let current = preview(&h, &project, "old", "current", scopes);
    let stale_discard = request(
        &h,
        &project,
        json!({"action":"replace_discard","preview":old["preview"]}),
    );
    assert_eq!(
        stale_discard["error"]["code"], "wrong_binding",
        "{stale_discard}"
    );
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":current["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    let read = request(&h, &project, json!({"action":"read","document":document}));
    assert_eq!(read["value"]["name"], "current title", "{read}");
    h.close_clean();
}

#[test]
fn m44_failed_new_preview_does_not_restore_the_previous_capability() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    create(&h, &project, &template, "old title");
    let old = preview(
        &h,
        &project,
        "old",
        "first",
        json!({"title":true,"body":false,"englishName":false,"glossarySummary":false}),
    );
    let failed = request(
        &h,
        &project,
        json!({
            "action":"replace_preview","find":"","replacement":"never",
            "template":null,
            "scopes":{"title":true,"body":false,"englishName":false,"glossarySummary":false},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }),
    );
    assert_eq!(failed["error"]["code"], "invalid_input", "{failed}");
    let stale = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":old["preview"]}),
    );
    assert_eq!(stale["error"]["code"], "wrong_binding", "{stale}");
    h.close_clean();
}

#[test]
fn m44_source_change_after_preview_rejects_the_whole_plan() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let a = create(&h, &project, &template, "old A");
    let b = create(&h, &project, &template, "old B");
    let prepared = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":true,"body":false,"englishName":false,"glossarySummary":false}),
    );
    let editor = edit_begin(&h, &project, &a);
    let mut body = editor["body"].clone();
    body["name"] = json!({"intent":"set","value":"external change"});
    let saved = edit_save(&h, &project, &editor, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    edit_release(&h, &project, &saved);
    let before_b = fs::read(h.root.join(format!("documents/{b}.json"))).unwrap();
    let rejected = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(rejected["disk"], "not_applied", "{rejected}");
    assert_eq!(
        fs::read(h.root.join(format!("documents/{b}.json"))).unwrap(),
        before_b
    );
    h.close_clean();
}

#[test]
fn m44_rejects_multiline_conditions_and_an_empty_scope_without_writes() {
    let h = Harness::new();
    let project = h.open();
    let rejected = request(
        &h,
        &project,
        json!({
            "action":"replace_preview","find":"bad\u{2028}line","replacement":"ok",
            "template":null,
            "scopes":{"title":true,"body":true,"englishName":true,"glossarySummary":true},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }),
    );
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    let rejected = request(
        &h,
        &project,
        json!({
            "action":"replace_preview","find":"x","replacement":"bad\u{2028}line",
            "template":null,
            "scopes":{"title":true,"body":true,"englishName":true,"glossarySummary":true},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }),
    );
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    let rejected = request(
        &h,
        &project,
        json!({
            "action":"replace_preview","find":"x","replacement":"y",
            "template":null,
            "scopes":{"title":false,"body":false,"englishName":false,"glossarySummary":false},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }),
    );
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    h.close_clean();
}

#[test]
fn m44_preview_cancellation_is_terminal_before_any_write() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let document = create_terms(&h, &project, &template, "old", "old", "old");
    let before = fs::read(h.root.join(format!("documents/{document}.json"))).unwrap();
    let operation = h.reserve("ordinary");
    let operation_id: Id = serde_json::from_value(json!(operation)).unwrap();
    let (entered, release) = h.state.lock().operations[&operation_id]
        .progress
        .hold_checkpoint();
    h.call(json!({
        "action":"submit","operation":operation,
        "input":{"kind":"document_workspace","project":project,"request":{
            "action":"replace_preview","find":"old","replacement":"new","template":null,
            "scopes":{"title":true,"body":true,"englishName":true,"glossarySummary":true},
            "caseSensitive":false,"wholeWord":false,"offset":0,"limit":100
        }}
    }));
    entered.recv_timeout(LIMIT).unwrap();
    assert_eq!(
        h.call(json!({"action":"document_progress","operation":operation,"cancel":true}))
            ["requested"],
        true
    );
    release.send(()).unwrap();
    let cancelled = h.result(&operation);
    assert_eq!(cancelled["error"]["code"], "cancelled", "{cancelled}");
    h.ack(&operation);
    assert_eq!(
        fs::read(h.root.join(format!("documents/{document}.json"))).unwrap(),
        before
    );
    h.close_clean();
}

#[test]
fn m44_attribute_matches_after_the_first_hundred_are_in_the_same_apply() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let mut ids = Vec::new();
    for index in 0..101 {
        ids.push(create_terms(
            &h,
            &project,
            &template,
            &format!("문서 {index:03}"),
            &format!("old {index:03}"),
            "",
        ));
    }
    let prepared = preview(
        &h,
        &project,
        "old",
        "new",
        json!({"title":false,"body":false,"englishName":true,"glossarySummary":false}),
    );
    assert_eq!(prepared["totalDocuments"], 101, "{prepared}");
    assert_eq!(prepared["totalChanges"], 101, "{prepared}");
    assert_eq!(prepared["changes"].as_array().unwrap().len(), 100);
    assert_eq!(prepared["hasMore"], true);
    let second_page = request(
        &h,
        &project,
        json!({"action":"replace_page","preview":prepared["preview"],"offset":100,"limit":100}),
    );
    assert_eq!(second_page["value"]["changes"].as_array().unwrap().len(), 1);
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    let last = request(
        &h,
        &project,
        json!({"action":"read","document":ids.last().unwrap()}),
    );
    assert_eq!(last["value"]["englishName"], "new 100", "{last}");
    h.close_clean();
}

#[test]
fn m44_root_group_and_rich_body_values_preserve_structure() {
    let h = Harness::new();
    let project = h.open();
    let (template, _) = h.template(&project);
    let template_path = h.root.join(format!("templates/{template}.json"));
    let mut wire: Value = serde_json::from_slice(&fs::read(&template_path).unwrap()).unwrap();
    let single = "aaaaaaaa-aaaa-4aaa-8aaa-100000000001";
    let rich = "aaaaaaaa-aaaa-4aaa-8aaa-100000000002";
    let group = "aaaaaaaa-aaaa-4aaa-8aaa-100000000003";
    let child = "aaaaaaaa-aaaa-4aaa-8aaa-100000000004";
    let instance = "bbbbbbbb-bbbb-4bbb-8bbb-100000000001";
    let definition = |label: &str, kind: &str| {
        json!({
            "label":label,"kind":kind,"required":false,"lifecycle":"active",
            "introducedRevision":1,"defaultValue":{"kind":"unset"},
            "initialDefaultValue":{"kind":"unset"},
            "configuration":{"kind":kind},"presentation":{}
        })
    };
    wire["fieldOrder"] = json!([single, rich, group]);
    wire["fields"] = json!({});
    wire["fields"][single] = definition("단일행", "singleLineText");
    wire["fields"][rich] = definition("서식 본문", "richText");
    wire["fields"][group] = definition("반복", "group");
    wire["fields"][group]["configuration"] = json!({
        "kind":"group","memberOrder":[child],
        "members":{(child):definition("반복 서식 본문", "richText")}
    });
    artifact::decode_template(&serde_json::to_vec(&wire).unwrap()).unwrap();
    fs::write(&template_path, serde_json::to_vec(&wire).unwrap()).unwrap();

    let draft = begin(&h, &project, &template);
    let mut body = draft["body"].clone();
    body["name"] = "본문 문서".into();
    body["fields"] = json!([
        {"field":single,"value":{"intent":"set","value":{"kind":"single_line_text","value":"Old Name"}}},
        {"field":rich,"value":{"intent":"set","value":{"kind":"rich_text","content":{"kind":"root","children":[
            {"kind":"paragraph","children":[
                {"kind":"text","text":"Old ","marks":["bold"]},
                {"kind":"text","text":"Name","marks":["italic"]}
            ]},
            {"kind":"paragraph","children":[{"kind":"text","text":"보존 문단","marks":[]}]}
        ]}}}},
        {"field":group,"value":{"intent":"set","value":{"kind":"group","instances":[{
            "id":instance,"source":null,"fields":[
                {"field":child,"value":{"intent":"set","value":{"kind":"rich_text","content":{"kind":"root","children":[
                    {"kind":"paragraph","children":[{"kind":"text","text":"Old Name","marks":["underline"]}]}
                ]}}}}
            ]
        }]}}}
    ]);
    let saved = save(&h, &project, &draft, "2", body);
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let document = saved["outcome"]["artifact"].as_str().unwrap().to_owned();
    release(&h, &project, &saved, false);

    let prepared = preview(
        &h,
        &project,
        "Old Name",
        "New",
        json!({"title":false,"body":true,"englishName":false,"glossarySummary":false}),
    );
    assert_eq!(prepared["totalChanges"], 3, "{prepared}");
    let applied = request(
        &h,
        &project,
        json!({"action":"replace_apply","preview":prepared["preview"]}),
    );
    assert_eq!(applied["disk"], "committed", "{applied}");
    let read = request(&h, &project, json!({"action":"read","document":document}))["value"].clone();
    let fields = read["fields"].as_array().unwrap();
    let value = |id: &str| &fields.iter().find(|field| field["id"] == id).unwrap()["value"];
    assert_eq!(value(single)["value"], "New");
    assert_eq!(
        value(rich)["content"]["children"][0]["children"][0]["text"],
        "New"
    );
    assert_eq!(
        value(rich)["content"]["children"][0]["children"][0]["marks"],
        json!(["bold"])
    );
    assert_eq!(
        value(rich)["content"]["children"][1]["children"][0]["text"],
        "보존 문단"
    );
    assert_eq!(
        value(group)["instances"][0]["fields"][0]["value"]["value"]["content"]["children"][0]
            ["children"][0]["text"],
        "New"
    );
    assert_eq!(
        value(group)["instances"][0]["fields"][0]["value"]["value"]["content"]["children"][0]
            ["children"][0]["marks"],
        json!(["underline"])
    );
    h.close_clean();
}

#[test]
#[ignore = "explicit owned M4-4 native GUI fixture"]
fn m44_gui_fixture() {
    let base = PathBuf::from(std::env::var_os("M44_GUI_BASE").expect("owned GUI base"));
    assert!(
        !base.join("project").exists(),
        "never replace an existing fixture"
    );
    let h = Harness::at(base, backend::provider(), false);
    let project = h.open();
    let (template_a, _) = h.template(&project);
    let (template_b, _) = h.template(&project);
    let single = "aaaaaaaa-aaaa-4aaa-8aaa-440000000001";
    let rich = "aaaaaaaa-aaaa-4aaa-8aaa-440000000002";
    let definition = |label: &str, kind: &str| {
        json!({
            "label":label,"kind":kind,"required":false,"lifecycle":"active",
            "introducedRevision":1,"defaultValue":{"kind":"unset"},
            "initialDefaultValue":{"kind":"unset"},
            "configuration":{"kind":kind},"presentation":{}
        })
    };
    for template in [&template_a, &template_b] {
        let path = h.root.join(format!("templates/{template}.json"));
        let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        wire["fieldOrder"] = json!([single, rich]);
        wire["fields"][single] = definition("본문 단일행", "singleLineText");
        wire["fields"][rich] = definition("서식 본문", "richText");
        artifact::decode_template(&serde_json::to_vec(&wire).unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&wire).unwrap()).unwrap();
    }
    let create_sample = |template: &str,
                         name: &str,
                         english: &str,
                         summary: &str,
                         single_value: &str,
                         rich_value: &str| {
        let draft = begin(&h, &project, template);
        let mut body = draft["body"].clone();
        body["name"] = name.into();
        body["englishName"] = english.into();
        body["glossarySummary"] = summary.into();
        body["fields"] = json!([
            {"field":single,"value":{"intent":"set","value":{"kind":"single_line_text","value":single_value}}},
            {"field":rich,"value":{"intent":"set","value":{"kind":"rich_text","content":{"kind":"root","children":[
                {"kind":"paragraph","children":[
                    {"kind":"text","text":rich_value,"marks":["bold"]},
                    {"kind":"text","text":" · 보존할 뒤 문장","marks":[]}
                ]}
            ]}}}}
        ]);
        let saved = save(&h, &project, &draft, "2", body);
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        let id = saved["outcome"]["artifact"].as_str().unwrap().to_owned();
        release(&h, &project, &saved, false);
        id
    };
    let a = create_sample(
        &template_a,
        "Old Kingdom A",
        "Old Realm A",
        "old summary A",
        "Old body A",
        "Old rich A",
    );
    let b = create_sample(
        &template_a,
        "Old Kingdom B",
        "Old Realm B",
        "old summary B",
        "Old body B",
        "Old rich B",
    );
    let c = create_sample(
        &template_b,
        "Old Kingdom C",
        "Old Realm C",
        "old summary C",
        "Old body C",
        "Old rich C",
    );
    let keep = create_sample(
        &template_b,
        "보존 문서",
        "Keep Realm",
        "보존 설명",
        "보존 본문",
        "보존 서식",
    );
    println!(
        "M44_GUI {}",
        json!({"a":a,"b":b,"c":c,"keep":keep,"templateA":template_a,"templateB":template_b})
    );
    h.close_clean();
}
