use super::*;
fn request(h: &Harness, p: &str, r: Value) -> Value {
    h.work(json!({"kind":"document_workspace","project":p,"request":r}))
}
fn release_doc(h: &Harness, p: &str, d: &Value, edit: bool) {
    let r = request(
        h,
        p,
        json!({"action":if edit{"edit_release"}else{"release"},"owner":d["owner"],"generation":d["generation"],"discard":false}),
    );
    // edit_release has a closed wire shape without discard.
    assert_eq!(r["value"]["kind"], "released", "{r}");
}
#[test]
fn m37_template_document_gallery_shared_refs_and_missing_bytes_keep_canonical() {
    let h = Harness::new();
    let p = h.open();
    let s = begin(&h, &p, Value::Null);
    let mut body = content(&h, &p, &s)["body"].clone();
    body["name"] = "media definition".into();
    body["fields"]=Value::Array(["image","file","url"].into_iter().map(|kind|json!({"id":format!("new:{}",uuid::Uuid::new_v4()),"label":kind,"configuration":{"kind":kind},"required":kind=="image","presentation":{"intent":"keep"},"default":{"intent":"keep"},"archived":false})).collect());
    let saved = submit(&h, &p, &s, "2", &body, "save");
    assert_eq!(saved["phase"], "saved", "{saved}");
    let t = saved["outcome"]["artifact"].as_str().unwrap().to_owned();
    assert!(release(&h, &p, &saved, false)["error"].is_null());
    let template: Value =
        serde_json::from_slice(&fs::read(h.root.join(format!("templates/{t}.json"))).unwrap())
            .unwrap();
    assert_eq!(template["schemaVersion"], 7);
    let fields = template["fieldOrder"].as_array().unwrap();
    let mut png = vec![];
    {
        let mut e = png::Encoder::new(&mut png, 1, 1);
        e.set_color(png::ColorType::Rgb);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[1, 2, 3]).unwrap();
    }
    let source = h.root.join("source.png");
    fs::write(&source, &png).unwrap();
    let store = crate::data::assets::Store::open(&h.root, true).unwrap();
    let a = store
        .import(&source, true, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let b = store
        .import(&source, true, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let file = store
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(store);
    let values = json!([
        {"field":fields[0],"value":{"intent":"set","value":{"kind":"image","value":[a.id,b.id]}}},
        {"field":fields[1],"value":{"intent":"set","value":{"kind":"file","value":[file.id]}}},
        {"field":fields[2],"value":{"intent":"set","value":{"kind":"url","value":"https://example.com/clip.mp4"}}}
    ]);
    let mut ids = vec![];
    for n in 0..2 {
        let d = request(&h, &p, json!({"action":"begin","template":t}))["value"].clone();
        let mut body = d["body"].clone();
        body["name"] = format!("media {n}").into();
        let empty = request(
            &h,
            &p,
            json!({"action":"draft","owner":d["owner"],"generation":"2","body":body,"save":true}),
        );
        assert_eq!(empty["value"]["field"], fields[0], "{empty}");
        body["fields"] = values.clone();
        let r = request(
            &h,
            &p,
            json!({"action":"draft","owner":d["owner"],"generation":"3","body":body,"save":true}),
        );
        assert_eq!(r["value"]["outcome"]["disk"], "committed", "{r}");
        ids.push(
            r["value"]["outcome"]["artifact"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        release_doc(&h, &p, &r["value"], false);
    }
    let d = request(&h, &p, json!({"action":"edit_begin","document":ids[0]}))["value"].clone();
    let mut body = d["body"].clone();
    body["fields"] = json!([{"field":fields[0],"value":{"intent":"set","value":{"kind":"image","value":[b.id]}}}]);
    let changed = request(
        &h,
        &p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":"2","body":body,"save":true}),
    );
    assert_eq!(
        changed["value"]["outcome"]["disk"], "committed",
        "{changed}"
    );
    let released = request(
        &h,
        &p,
        json!({"action":"edit_release","owner":d["owner"],"generation":"2"}),
    );
    assert_eq!(released["value"]["kind"], "released", "{released}");
    let other = request(&h, &p, json!({"action":"read","document":ids[1]}));
    assert_eq!(other["value"]["schema"], 6);
    assert_eq!(
        other["value"]["fields"][0]["value"]["value"],
        json!([a.id, b.id])
    );
    let d = request(&h, &p, json!({"action":"edit_begin","document":ids[1]}))["value"].clone();
    let mut raw = d["body"].clone();
    raw["name"] = json!({"intent":"set","value":"retained after missing attachment"});
    let path = h.root.join(format!("documents/{}.json", ids[1]));
    let original = fs::read(&path).unwrap();
    let binary = h.root.join("assets").join(&a.id).join("content.png");
    fs::remove_file(&binary).unwrap();
    let rejected = request(
        &h,
        &p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":"2","body":raw,"save":true}),
    );
    assert!(!rejected["error"].is_null(), "{rejected}");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":"2"})
        )["error"]["code"],
        "no_receipt"
    );
    crate::data::assets::Store::open(&h.root, false)
        .unwrap()
        .put(&a, &png)
        .unwrap();
    let retry = request(
        &h,
        &p,
        json!({"action":"edit_draft","owner":d["owner"],"generation":"2","body":raw,"save":true}),
    );
    assert_eq!(retry["value"]["outcome"]["disk"], "committed", "{retry}");
    assert_eq!(
        request(
            &h,
            &p,
            json!({"action":"edit_release","owner":d["owner"],"generation":"2"})
        )["value"]["kind"],
        "released"
    );
    assert_eq!(fs::read(&source).unwrap(), png);
    // 입력 중의 짧은 YouTube 주소도 worker를 죽이지 않고 원문만 보존한다.
    let editing =
        request(&h, &p, json!({"action":"edit_begin","document":ids[0]}))["value"].clone();
    let path = h.root.join(format!("documents/{}.json", ids[0]));
    let original = fs::read(&path).unwrap();
    let mut raw = editing["body"].clone();
    for (i, url) in [
        "https://youtu.be",
        "https://youtu.be/",
        "https://youtu.be///?t=5",
        "https://youtu.be/M7lc1UVf-VE?t=%2B5",
        "https://example.com/a.png#",
    ]
    .iter()
    .enumerate()
    {
        raw["fields"] = json!([{"field":fields[2],"value":{"intent":"set","value":{"kind":"url","value":url}}}]);
        let rejected = request(
            &h,
            &p,
            json!({"action":"edit_draft","owner":editing["owner"],"generation":(i+2).to_string(),"body":raw,"save":true}),
        );
        assert_eq!(
            rejected["value"]["problem"], "InvalidEditValue",
            "{rejected}"
        );
        assert_eq!(rejected["value"]["body"], raw);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    raw["fields"][0]["value"]["value"]["value"] = "https://youtu.be/M7lc1UVf-VE?t=5".into();
    let saved = request(
        &h,
        &p,
        json!({"action":"edit_draft","owner":editing["owner"],"generation":"7","body":raw,"save":true}),
    );
    assert_eq!(saved["value"]["outcome"]["disk"], "committed", "{saved}");
    // 저장된 문서가 계속 참조하는 첨부가 휴지통으로 이동된 상태에서도 편집
    // 복구본은 같은 ID/bytes를 휴지통 namespace에서 보존할 수 있어야 한다.
    // 그렇지 않으면 보관 후 닫기가 실패하고 editor owner가 남아 복원까지 막는
    // 순환 상태가 된다.
    let active_asset = h.root.join("assets").join(&b.id);
    let trashed_asset = h.root.join("assets/.trash").join(&b.id);
    fs::create_dir_all(h.root.join("assets/.trash")).unwrap();
    fs::rename(&active_asset, &trashed_asset).unwrap();
    let deposited = request(
        &h,
        &p,
        json!({"action":"edit_deposit","owner":editing["owner"],"generation":"7","body":raw}),
    );
    assert_eq!(deposited["value"]["deposited"], true, "{deposited}");
    fs::rename(&trashed_asset, &active_asset).unwrap();
    request(
        &h,
        &p,
        json!({"action":"edit_release","owner":editing["owner"],"generation":"7"}),
    );
    assert_eq!(
        request(&h, &p, json!({"action":"read","document":ids[1]}))["value"]["kind"],
        "read"
    );
    h.close_clean();
}
