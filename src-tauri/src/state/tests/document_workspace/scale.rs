//! M3-9의 명시적 end-to-end 규모 측정. 일반 test suite에서는 실행하지 않는다.
use super::*;
use std::{collections::BTreeMap, path::PathBuf, time::Instant};

fn timed<T>(run: impl FnOnce() -> T) -> (T, f64) {
    let started = Instant::now();
    let result = run();
    (result, started.elapsed().as_secs_f64() * 1000.0)
}

fn emit(value: Value) {
    println!("M39_JSON {value}");
}

fn emit_stage(stage: &str, phase: &str, operation: &str) {
    println!(
        "M39_STAGE {}",
        json!({"stage": stage, "phase": phase, "operation": operation})
    );
}

fn counts_value(counts: crate::data::repository::test_support::Counts) -> Value {
    json!({
        "document_scans": counts.document_scans,
        "template_scans": counts.template_scans,
        "directory_iterations": counts.iterations,
        "file_body_reads": counts.read_calls,
        "bytes_read": counts.bytes_read,
        "decodes": counts.decoded,
        "hashes": counts.hashes,
        "reference_scans": counts.reference_scans,
        "assessments": counts.assessments
    })
}

fn measured_request(h: &Harness, project: &str, stage: &str, value: Value) -> (Value, f64, Value) {
    crate::data::repository::test_support::begin_global();
    let started = Instant::now();
    let operation = h.submit(json!({
        "kind": "document_workspace",
        "project": project,
        "request": value
    }));
    emit_stage(stage, "begin", &operation);
    let result = h.result(&operation);
    emit_stage(stage, "result", &operation);
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let counts = counts_value(crate::data::repository::test_support::take_global());
    h.ack(&operation);
    emit_stage(stage, "release", &operation);
    (result, elapsed, counts)
}

#[test]
#[ignore = "explicit M3-9 scale runner; use scripts/measure-m3.py"]
fn m39_scale_runner() {
    let base = PathBuf::from(std::env::var_os("M39_SCALE_BASE").expect("synthetic base"));
    let size: usize = std::env::var("M39_SCALE_SIZE")
        .expect("scale size")
        .parse()
        .expect("numeric scale size");
    assert!([1_500, 3_000, 5_000, 10_000, 15_000, 20_000].contains(&size));
    assert_eq!(
        fs::read(base.join("M39-SYNTHETIC-ROOT")).expect("owned marker"),
        b"m39-v1-20260917"
    );
    assert!(!fs::symlink_metadata(&base)
        .expect("owned base")
        .file_type()
        .is_symlink());
    let provider = Arc::new(Provider::new());
    let (h, open_ms) = timed(|| {
        let h = Harness::at(base.clone(), provider.clone(), false);
        let project = h.open();
        (h, project)
    });
    let (h, project) = h;
    let (initial_response, initial_list_ms, initial_counts) =
        measured_request(&h, &project, "initial_list", json!({"action":"list"}));
    let initial = initial_response["value"].clone();
    let new_process_open_to_list_ms = open_ms + initial_list_ms;
    assert_eq!(initial["documents"].as_array().unwrap().len(), size);
    assert_eq!(initial["initial"], true);
    let (adopt, adopt_ms, adopt_counts) = measured_request(
        &h,
        &project,
        "adopt_layout",
        json!({"action":"mutate","snapshot":initial["snapshot"],"edit":{"kind":"adopt"}}),
    );
    assert_eq!(adopt["disk"], "committed", "{adopt}");
    let (ready_response, tree_list_ms, tree_list_counts) =
        measured_request(&h, &project, "tree_list", json!({"action":"list"}));
    let ready = ready_response["value"].clone();
    assert_eq!(ready["unplaced"], json!([]));
    assert_eq!(ready["layout"]["rootOrder"].as_array().unwrap().len(), size);
    let template = ready["documents"][0]["template"]
        .as_str()
        .expect("fixture template")
        .to_owned();
    let existing = ready["documents"][0]["id"]
        .as_str()
        .expect("fixture document")
        .to_owned();
    let mut current_snapshot = ready["snapshot"].clone();

    let mut created = Vec::new();
    let mut creation_rows = Vec::new();
    for n in 0..3 {
        let creation_started = Instant::now();
        let (draft_response, begin_ms, begin_counts) = measured_request(
            &h,
            &project,
            &format!("create_{n}_begin"),
            json!({"action":"begin","template":template,"snapshot":current_snapshot}),
        );
        let draft = draft_response["value"].clone();
        assert_eq!(draft["kind"], "draft", "{draft_response}");
        let mut body = draft["body"].clone();
        body["name"] = format!("M3-9 규모 생성 {size}-{n}").into();
        let (saved_response, save_ms, save_counts) = measured_request(
            &h,
            &project,
            &format!("create_{n}_save_commit"),
            json!({"action":"draft","owner":draft["owner"],"generation":"2","body":body,"save":true}),
        );
        let saved = saved_response["value"].clone();
        assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
        let id = saved["outcome"]["artifact"]
            .as_str()
            .expect("created id")
            .to_owned();
        assert_eq!(saved["commit"]["read"]["id"], id, "{saved}");
        let projected_documents = saved["commit"]["documentCount"]
            .as_u64()
            .expect("post-commit document count") as usize;
        current_snapshot = saved["commit"]["snapshot"].clone();
        assert_eq!(projected_documents, size + n + 1);
        let create_to_save_confirmed_ms = creation_started.elapsed().as_secs_f64() * 1000.0;
        let (_, release_ms, release_counts) = measured_request(
            &h,
            &project,
            &format!("create_{n}_release"),
            json!({"action":"release","owner":saved["owner"],"generation":saved["generation"],"discard":false}),
        );
        // 실제 frontend도 release 응답 뒤 같은 commit payload에서 읽기와 목록 delta를
        // 연속 반영한다. 둘을 별도 누적 checkpoint로 남기되 서로 더하지 않는다.
        assert_eq!(saved["commit"]["read"]["id"], id);
        assert!(saved["commit"]["changedDocuments"]
            .as_array()
            .expect("changed documents")
            .iter()
            .any(|summary| summary["id"] == id));
        let create_to_document_available_ms = creation_started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(saved["commit"]["documentCount"], projected_documents);
        let create_to_list_match_ms = creation_started.elapsed().as_secs_f64() * 1000.0;
        created.push(id);
        creation_rows.push(json!({
            "sample": n,
            "begin_ms": begin_ms,
            "scan_prepare_commit_ms": save_ms,
            "save_request_to_durable_result_ms": save_ms,
            "release_ms": release_ms,
            "create_to_save_confirmed_ms": create_to_save_confirmed_ms,
            "create_to_document_available_ms": create_to_document_available_ms,
            "create_to_list_match_ms": create_to_list_match_ms,
            "post_commit_documents": projected_documents,
            "followup_list_or_read_calls": 0,
            "counts": {"begin":begin_counts,"save_commit":save_counts,"release":release_counts}
        }));
    }

    let (move_base_response, _, _) =
        measured_request(&h, &project, "tree_move_base", json!({"action":"list"}));
    let move_base = &move_base_response["value"];
    let (moved, tree_move_ms, tree_move_counts) = measured_request(
        &h,
        &project,
        "tree_move",
        json!({"action":"mutate","snapshot":move_base["snapshot"],"edit":{"kind":"move","document":created[1],"parent":null,"index":0}}),
    );
    assert_eq!(moved["disk"], "committed", "{moved}");

    let (read_response, read_existing_ms, read_existing_counts) = measured_request(
        &h,
        &project,
        "read_existing",
        json!({"action":"read","document":existing}),
    );
    assert_eq!(read_response["value"]["kind"], "read", "{read_response}");
    // 편집 직전 목록 snapshot을 그대로 보관해, 커밋 뒤 첫 생성/트리 요청이
    // 전체 재목록 없이 갱신된 strict scan을 사용하는지 대표 규모에서 측정한다.
    let (post_edit_base_response, post_edit_base_ms, post_edit_base_counts) = measured_request(
        &h,
        &project,
        "post_edit_base_list",
        json!({"action":"list"}),
    );
    let post_edit_base = post_edit_base_response["value"].clone();
    let edited_id = created[0].clone();
    let edited_path = h.root.join(format!("documents/{edited_id}.json"));
    let before_edit = fs::read(&edited_path).expect("created source bytes");
    let (editing_response, edit_begin_ms, edit_begin_counts) = measured_request(
        &h,
        &project,
        "edit_begin",
        json!({"action":"edit_begin","document":edited_id}),
    );
    let editing = editing_response["value"].clone();
    let mut edit_body = editing["body"].clone();
    edit_body["name"] = json!({"intent":"set","value":"M3-9 저장 왕복"});
    let (saved_edit_response, edit_save_ms, edit_save_counts) = measured_request(
        &h,
        &project,
        "edit_save_commit",
        json!({"action":"edit_draft","owner":editing["owner"],"generation":"2","body":edit_body,"save":true}),
    );
    let saved_edit = saved_edit_response["value"].clone();
    assert_eq!(saved_edit["outcome"]["disk"], "committed", "{saved_edit}");
    let after_edit = fs::read(&edited_path).expect("saved source bytes");
    assert_ne!(before_edit, after_edit);
    let (_, edit_release_ms, edit_release_counts) = measured_request(
        &h,
        &project,
        "edit_release",
        json!({"action":"edit_release","owner":saved_edit["owner"],"generation":saved_edit["generation"]}),
    );

    let (after_edit_draft_response, after_edit_begin_ms, after_edit_begin_counts) =
        measured_request(
            &h,
            &project,
            "after_edit_create_begin",
            json!({"action":"begin","template":template,"snapshot":post_edit_base["snapshot"]}),
        );
    let after_edit_draft = after_edit_draft_response["value"].clone();
    let mut after_edit_body = after_edit_draft["body"].clone();
    after_edit_body["name"] = format!("M3-9 편집 후 생성 {size}").into();
    let (after_edit_saved_response, after_edit_save_ms, after_edit_save_counts) = measured_request(
        &h,
        &project,
        "after_edit_create_save",
        json!({"action":"draft","owner":after_edit_draft["owner"],"generation":"2","body":after_edit_body,"save":true}),
    );
    let after_edit_saved = after_edit_saved_response["value"].clone();
    assert_eq!(
        after_edit_saved["outcome"]["disk"], "committed",
        "{after_edit_saved}"
    );
    let after_edit_created = after_edit_saved["outcome"]["artifact"]
        .as_str()
        .expect("created after edit id")
        .to_owned();
    let after_edit_snapshot = after_edit_saved["commit"]["snapshot"].clone();
    let (_, after_edit_release_ms, after_edit_release_counts) = measured_request(
        &h,
        &project,
        "after_edit_create_release",
        json!({"action":"release","owner":after_edit_saved["owner"],"generation":after_edit_saved["generation"],"discard":false}),
    );
    created.push(after_edit_created.clone());
    let (after_edit_tree, after_edit_tree_ms, after_edit_tree_counts) = measured_request(
        &h,
        &project,
        "after_edit_tree",
        json!({"action":"mutate","snapshot":after_edit_snapshot,"edit":{"kind":"move","document":after_edit_created,"parent":null,"index":0}}),
    );
    assert_eq!(after_edit_tree["disk"], "committed", "{after_edit_tree}");

    let cancel_draft = begin(&h, &project, &template);
    let mut cancel_body = cancel_draft["body"].clone();
    cancel_body["name"] = format!("M3-9 취소 {size}").into();
    let (entered, release_gate) = provider.hold();
    let cancel_started = Instant::now();
    crate::data::repository::test_support::begin_global();
    let operation = h.submit(json!({"kind":"document_workspace","project":project,"request":{
        "action":"draft","owner":cancel_draft["owner"],"generation":"2","body":cancel_body,"save":true
    }}));
    emit_stage("cancel_before_commit", "begin", &operation);
    entered
        .recv_timeout(LIMIT)
        .expect("worker reached safe gate");
    let signal = h.call(json!({"action":"document_progress","operation":operation,"cancel":true}));
    assert_eq!(signal["requested"], true);
    release_gate.send(()).expect("release controlled worker");
    let terminal = h.result(&operation);
    emit_stage("cancel_before_commit", "result", &operation);
    let cancel_counts = counts_value(crate::data::repository::test_support::take_global());
    let cancel_terminal_ms = cancel_started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(terminal["error"]["code"], "cancelled", "{terminal}");
    assert_eq!(h.result(&operation), terminal);
    h.ack(&operation);
    emit_stage("cancel_before_commit", "release", &operation);
    // 검증 단계에 진입한 저장 요청은 취소되더라도 현재 소유권 세대가 2로 전진한다.
    // 본문은 커밋되지 않았지만, 최신 세대로만 초안을 안전하게 폐기할 수 있다.
    let (cancelled_release, cancel_release_ms, cancel_release_counts) = measured_request(
        &h,
        &project,
        "cancelled_draft_release",
        json!({"action":"release","owner":cancel_draft["owner"],"generation":"2","discard":true}),
    );
    assert_eq!(
        cancelled_release["value"]["kind"], "released",
        "{cancelled_release}"
    );

    let layout_path = h.root.join("workspace/document-layout.json");
    let layout_bytes = fs::metadata(&layout_path).expect("layout bytes").len();
    let (_, close_ms) = timed(|| h.close_clean());
    drop(h);

    let reopened = Harness::at(base.clone(), provider, false);
    let reopen_started = Instant::now();
    let reopened_project = reopened.open();
    let (reopened_response, _, reopen_counts) = measured_request(
        &reopened,
        &reopened_project,
        "reopen_list",
        json!({"action":"list"}),
    );
    let reopened_list = reopened_response["value"].clone();
    let reopen_to_list_ms = reopen_started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        reopened_list["documents"].as_array().unwrap().len(),
        size + created.len()
    );
    assert_eq!(
        reopened_list["layout"]["rootOrder"]
            .as_array()
            .unwrap()
            .len(),
        size + created.len()
    );
    reopened.close_clean();
    drop(reopened);

    // 다음 규모는 같은 synthetic 계열을 확장한다. 이번 실행이 만든 정확한 파일만 제거한다.
    for id in &created {
        fs::remove_file(base.join(format!("project/documents/{id}.json")))
            .expect("remove owned created document");
    }
    fs::remove_file(&layout_path).expect("remove owned layout");
    let remaining = fs::read_dir(base.join("project/documents"))
        .expect("fixture documents")
        .count();
    assert_eq!(remaining, size);

    let phases = BTreeMap::from([
        ("open_ms", open_ms),
        ("initial_list_ms", initial_list_ms),
        ("new_process_open_to_list_ms", new_process_open_to_list_ms),
        ("reopen_to_list_ms", reopen_to_list_ms),
        ("adopt_layout_ms", adopt_ms),
        ("tree_list_ms", tree_list_ms),
        ("read_existing_ms", read_existing_ms),
        ("edit_begin_ms", edit_begin_ms),
        ("edit_save_ms", edit_save_ms),
        ("edit_release_ms", edit_release_ms),
        ("post_edit_base_ms", post_edit_base_ms),
        ("after_edit_begin_ms", after_edit_begin_ms),
        ("after_edit_save_ms", after_edit_save_ms),
        ("after_edit_release_ms", after_edit_release_ms),
        ("after_edit_tree_ms", after_edit_tree_ms),
        ("tree_move_ms", tree_move_ms),
        ("cancel_release_ms", cancel_release_ms),
        ("cancel_terminal_ms", cancel_terminal_ms),
        ("close_ms", close_ms),
    ]);
    emit(json!({
        "scale": size,
        "documents_before": size,
        "documents_after_cleanup": remaining,
        "layout_bytes": layout_bytes,
        "edited_bytes_before": before_edit.len(),
        "edited_bytes_after": after_edit.len(),
        "phases_ms": phases,
        "creation": creation_rows,
        "autosave": {
            "scheduled_wait_ms": 1000,
            "actual_save_request_to_durable_result_ms": edit_save_ms,
            "note": "scheduled debounce is intentionally excluded from the native save duration"
        },
        "counts": {
            "initial_list": initial_counts,
            "adopt_layout": adopt_counts,
            "tree_list": tree_list_counts,
            "tree_move": tree_move_counts,
            "read_existing": read_existing_counts,
            "edit_begin": edit_begin_counts,
            "edit_save_commit": edit_save_counts,
            "edit_release": edit_release_counts,
            "post_edit_base": post_edit_base_counts,
            "after_edit_begin": after_edit_begin_counts,
            "after_edit_save": after_edit_save_counts,
            "after_edit_release": after_edit_release_counts,
            "after_edit_tree": after_edit_tree_counts,
            "cancel": cancel_counts,
            "cancel_release": cancel_release_counts,
            "reopen_list": reopen_counts
        },
        "cancel": {"requested": true, "terminal": "cancelled", "requery_equal": true},
        "source_preservation": {"synthetic_original_count_restored": true, "created_files_removed": created.len()}
    }));
}

#[test]
#[ignore = "explicit M4-1 search scale runner; requires an owned M3-9 content fixture"]
fn m41_search_scale_runner() {
    let base = PathBuf::from(std::env::var_os("M41_SCALE_BASE").expect("synthetic base"));
    let size: usize = std::env::var("M41_SCALE_SIZE")
        .expect("scale size")
        .parse()
        .expect("numeric scale size");
    assert!([1_500, 3_000, 5_000].contains(&size));
    assert_eq!(
        fs::read(base.join("M39-SYNTHETIC-ROOT")).expect("owned marker"),
        b"m39-v1-20260917"
    );
    let provider = Arc::new(Provider::new());
    let h = Harness::at(base, provider, false);
    let project = h.open();
    let (prepared, prepare_ms, prepare_counts) = measured_request(
        &h,
        &project,
        "m41_search_prepare",
        json!({"action":"search","query":"바람의 도시와 숲","template":null,
          "offset":0,"limit":100,"refresh":true}),
    );
    assert_eq!(prepared["value"]["kind"], "search", "{prepared}");
    assert_eq!(prepared["value"]["total"], size, "{prepared}");
    let (warm, warm_ms, warm_counts) = measured_request(
        &h,
        &project,
        "m41_search_warm",
        // 마지막 문서의 11번 필드는 fixture 규칙상 unset이므로 바로 앞 문서의
        // 활성 텍스트를 사용해 warm cache의 단일 결과를 측정한다.
        json!({"action":"search","query":format!("한국어 값 {}-11", size - 2),
          "template":null,"offset":0,"limit":100,"refresh":false}),
    );
    assert_eq!(warm["value"]["kind"], "search", "{warm}");
    assert_eq!(warm["value"]["total"], 1, "{warm}");
    let edited_id = prepared["value"]["results"][0]["id"]
        .as_str()
        .expect("prepared search result")
        .to_owned();
    let edited_path = h.root.join(format!("documents/{edited_id}.json"));
    let original_bytes = fs::read(&edited_path).expect("source before measured edit");
    let (editing_response, edit_begin_ms, edit_begin_counts) = measured_request(
        &h,
        &project,
        "m41_search_edit_begin",
        json!({"action":"edit_begin","document":edited_id}),
    );
    let editing = editing_response["value"].clone();
    let mut edit_body = editing["body"].clone();
    let updated_name = format!("M4-1 부분 갱신 {size}");
    edit_body["name"] = json!({"intent":"set","value":updated_name.clone()});
    let (saved_response, edit_save_ms, edit_save_counts) = measured_request(
        &h,
        &project,
        "m41_search_edit_save",
        json!({"action":"edit_draft","owner":editing["owner"],"generation":"2",
          "body":edit_body,"save":true}),
    );
    let saved = saved_response["value"].clone();
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let (after_save, after_save_search_ms, after_save_search_counts) = measured_request(
        &h,
        &project,
        "m41_search_after_save",
        json!({"action":"search","query":updated_name,"template":null,
          "offset":0,"limit":100,"refresh":false}),
    );
    assert_eq!(after_save["value"]["total"], 1, "{after_save}");
    assert_eq!(
        after_save_search_counts["document_scans"], 0,
        "search after a verified edit must not rebuild the full cache"
    );
    assert_eq!(after_save_search_counts["decodes"], 0);
    let (_, edit_release_ms, edit_release_counts) = measured_request(
        &h,
        &project,
        "m41_search_edit_release",
        json!({"action":"edit_release","owner":saved["owner"],
            "generation":saved["generation"]}),
    );
    let cancelled_operation = h.submit(json!({
        "kind":"document_workspace","project":project,
        "request":{"action":"search","query":"바람의 도시와 숲","template":null,
          "offset":0,"limit":100,"refresh":true}
    }));
    let cancel_wait = std::time::Instant::now();
    let reached_files = loop {
        let progress = h.call(json!({
            "action":"document_progress","operation":cancelled_operation,"cancel":false
        }));
        let files = progress["files"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        if files >= 64 {
            break files;
        }
        assert!(
            cancel_wait.elapsed() < std::time::Duration::from_secs(30),
            "search must reach an in-progress file checkpoint: {progress}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    let cancel_signal = h.call(json!({
        "action":"document_progress","operation":cancelled_operation,"cancel":true
    }));
    assert_eq!(cancel_signal["requested"], true, "{cancel_signal}");
    let cancelled = h.result(&cancelled_operation);
    assert_eq!(cancelled["error"]["code"], "cancelled", "{cancelled}");
    assert_eq!(h.result(&cancelled_operation), cancelled);
    h.ack(&cancelled_operation);
    println!(
        "M41_JSON {}",
        json!({
            "scale": size,
            "fixture":"M3-9 content-rich synthetic project",
            "prepare_ms":prepare_ms,
            "warm_ms":warm_ms,
            "edit_begin_ms":edit_begin_ms,
            "edit_save_ms":edit_save_ms,
            "after_save_search_ms":after_save_search_ms,
            "edit_release_ms":edit_release_ms,
            "prepare_counts":prepare_counts,
            "warm_counts":warm_counts,
            "edit_begin_counts":edit_begin_counts,
            "edit_save_counts":edit_save_counts,
            "after_save_search_counts":after_save_search_counts,
            "edit_release_counts":edit_release_counts,
            "cold_cancel":{"requested":true,"terminal":"cancelled",
              "files_before_cancel":reached_files,"terminal_requery_equal":true,
              "project_close_after_terminal":true}
        })
    );
    h.close_clean();
    drop(h);
    fs::write(&edited_path, original_bytes).expect("restore owned scale fixture source");
}

#[test]
#[ignore = "explicit M5-6 first References and refresh measurement"]
fn m56_references_scale_runner() {
    let base = PathBuf::from(std::env::var_os("M41_SCALE_BASE").expect("owned fixture"));
    let size: usize = std::env::var("M41_SCALE_SIZE")
        .expect("fixture size")
        .parse()
        .unwrap();
    assert_eq!(size, 5000);
    assert_eq!(
        fs::read(base.join("M39-SYNTHETIC-ROOT")).unwrap(),
        b"m39-v1-20260917"
    );
    let h = Harness::at(base, Arc::new(Provider::new()), false);
    let project = h.open();
    let document = fs::read_dir(h.root.join("documents"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .to_string_lossy()
        .trim_end_matches(".json")
        .to_owned();
    let (first, first_ms, first_counts) = measured_request(
        &h,
        &project,
        "m56_first_references",
        json!({"action":"references","document":document}),
    );
    assert_eq!(first["value"]["kind"], "references", "{first}");
    let (warm, warm_ms, warm_counts) = measured_request(
        &h,
        &project,
        "m56_warm_after_references",
        json!({"action":"search","query":"바람의 도시와 숲","template":null,
          "offset":0,"limit":100,"refresh":false}),
    );
    assert_eq!(warm["value"]["total"], size, "{warm}");
    let (rebuilt, rebuild_ms, rebuild_counts) = measured_request(
        &h,
        &project,
        "m56_explicit_refresh",
        json!({"action":"search","query":"바람의 도시와 숲","template":null,
          "offset":0,"limit":100,"refresh":true}),
    );
    assert_eq!(rebuilt["value"]["total"], size, "{rebuilt}");
    println!(
        "M56_REFERENCES_JSON {}",
        json!({
            "scale": size,
            "first_ms": first_ms,
            "first_counts": first_counts,
            "warm_ms": warm_ms,
            "warm_counts": warm_counts,
            "refresh_ms": rebuild_ms,
            "refresh_counts": rebuild_counts
        })
    );
    h.close_clean();
}

#[test]
#[ignore = "explicit M9 ordinary-save cold-cache measurement on owned 5000 document fixture"]
fn m9_plain_save_scale_runner() {
    let base = PathBuf::from(std::env::var_os("M41_SCALE_BASE").expect("owned fixture"));
    assert_eq!(
        fs::read(base.join("M39-SYNTHETIC-ROOT")).unwrap(),
        b"m39-v1-20260917"
    );
    let h = Harness::at(base, Arc::new(Provider::new()), false);
    let project = h.open();
    let source = fs::read_dir(h.root.join("documents"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let document = source.file_stem().unwrap().to_string_lossy().to_string();
    let original = fs::read(&source).unwrap();
    let (editing, begin_ms, begin_counts) = measured_request(
        &h,
        &project,
        "m9_plain_begin",
        json!({"action":"edit_begin","document":document}),
    );
    let editing = editing["value"].clone();
    let mut body = editing["body"].clone();
    body["name"] = json!({"intent":"set","value":"M9 ordinary-save owned measurement"});
    let (saved, save_ms, save_counts) = measured_request(
        &h,
        &project,
        "m9_plain_save",
        json!({"action":"edit_draft","owner":editing["owner"],"generation":"2","body":body,"save":true}),
    );
    let saved = saved["value"].clone();
    assert_eq!(saved["outcome"]["disk"], "committed", "{saved}");
    let (_, release_ms, release_counts) = measured_request(
        &h,
        &project,
        "m9_plain_release",
        json!({"action":"edit_release","owner":saved["owner"],"generation":saved["generation"]}),
    );
    println!(
        "M9_PLAIN_SAVE_JSON {}",
        json!({"begin_ms":begin_ms,"begin_counts":begin_counts,"save_ms":save_ms,
        "save_counts":save_counts,"release_ms":release_ms,"release_counts":release_counts})
    );
    h.close_clean();
    drop(h);
    fs::write(source, original).unwrap();
}
