//! M2-8 명시적 규모 runner. 일반 suite는 작은 생성기 계약만 검사한다.
use super::*;
use crate::data::repository::{test_support::Counts, RepositoryCategory};
use std::{path::Path, time::Instant};

const SEED: &str = "m28-v1-20260913";
const TEMPLATES: usize = 9;
mod storage;

fn assert_scale_numbers(bytes: &[u8]) {
    let tree = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
    for path in [
        vec!["future".to_owned()],
        vec!["fieldValues".into(), key(1), "future".into()],
        vec![
            "fieldValues".into(),
            key(2),
            "document".into(),
            "future".into(),
        ],
        vec![
            "orphanedFieldDefinitions".into(),
            key(1),
            "options".into(),
            key(11),
            "future".into(),
        ],
    ] {
        let path: Vec<_> = path.iter().map(String::as_str).collect();
        assert!(
            tree.object_path(&path).unwrap() == &expected,
            "exact numeric owner tokens"
        );
    }
}

fn did(n: usize) -> DocumentId {
    format!("e2800000-0000-4000-8000-{n:012x}").parse().unwrap()
}
fn tid(n: usize) -> TemplateId {
    key(100 + n as u32).parse().unwrap()
}

// 기존 G8의 선택값·역사 snapshot·raw 숫자 fixture를 확대한다. 새 production schema는 없다.
fn representative(n: usize, large: bool) -> (Vec<u8>, Vec<u8>) {
    let (mut t, mut d) = fixture_raw();
    let template = n % 8;
    t["templateId"] = tid(template).to_string().into();
    t["name"] = format!("설정 분류 {template}").into();
    d["templateId"] = tid(template).to_string().into();
    d["documentId"] = did(n).to_string().into();
    d["name"] = format!("세계관 기록 {n:05} — 인물과 장소").into();
    d["fieldValues"][key(2)]["document"]["content"]["children"][0]["children"][0]["text"] =
        "바람의 도시와 숲을 잇는 이야기. "
            .repeat(if large { 4096 } else { 1 + n % 8 })
            .into();
    for f in 3..=12 {
        t["fieldOrder"].as_array_mut().unwrap().push(key(f).into());
        t["fields"][key(f)] = json!({"label":format!("속성 {f}"),"kind":"singleLineText",
            "lifecycle":"active","required":false,"introducedRevision":if f==12 {3} else {1},
            "defaultValue":{"kind":"text","value":"현재 기본값"},
            "initialDefaultValue":{"kind":"text","value":"최초 기본값","future":"__numbers__"},
            "presentation":{},"configuration":{"kind":"singleLineText"}});
        d["fieldValues"][key(f)] = if (n + f as usize).is_multiple_of(5) {
            json!({"kind":"unset","future":"__numbers__"})
        } else {
            json!({"kind":"text","value":format!("한국어 값 {n}-{f}"),"future":"__numbers__"})
        };
    }
    if n.is_multiple_of(4) {
        d["templateRevision"] = 2.into();
        d["fieldValues"].as_object_mut().unwrap().remove(&key(12));
    }
    (raw_bytes(&t), raw_bytes(&d))
}

fn counts(c: Counts) -> Value {
    json!({"decoded":c.decoded,"iterations":c.iterations,"bytes_read":c.bytes_read,
        "document_scans":c.document_scans,"template_scans":c.template_scans,
        "reference_scans":c.reference_scans,"assessments":c.assessments,"hooks":c.hooks})
}

// 테스트 프로세스의 OS 계측값이다. allocator 호출 수나 live heap 크기로 해석하지 않는다.
#[repr(C)]
#[derive(Default)]
struct Memory {
    cb: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    peak_paged_pool: usize,
    paged_pool: usize,
    peak_nonpaged_pool: usize,
    nonpaged_pool: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private_bytes: usize,
}
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn K32GetProcessMemoryInfo(
        process: *mut std::ffi::c_void,
        counters: *mut Memory,
        size: u32,
    ) -> i32;
}
fn memory() -> Value {
    let mut m = Memory {
        cb: std::mem::size_of::<Memory>() as u32,
        ..Default::default()
    };
    // 유효한 현재 process pseudo-handle과 API 규격 크기의 쓰기 가능한 구조체다.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut m,
            std::mem::size_of::<Memory>() as u32,
        )
    };
    assert_ne!(ok, 0, "process memory observation failed");
    json!({"working_set":m.working_set,"private_bytes":m.private_bytes,"process_peak_working_set":m.peak_working_set})
}
fn emit(value: Value) {
    println!("M28_JSON {value}");
}
fn measure<T>(
    operation: &str,
    sample: usize,
    run: impl FnOnce() -> T,
    verify: impl FnOnce(&T, Counts),
) {
    let before = memory();
    let started = Instant::now();
    let (result, observed) = repo_hooks::scoped(None, false, run);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let retained = memory();
    verify(&result, observed);
    drop(result);
    emit(
        json!({"operation":operation,"sample":sample,"elapsed_ms":elapsed_ms,"counts":counts(observed),
        "baseline":before,"retained":retained,"released":memory()}),
    );
}

fn generate(base: &Path, size: usize) {
    assert!([1_500, 3_000, 5_000, 10_000, 15_000, 20_000].contains(&size));
    let root = base.join("project");
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::create_dir_all(root.join("documents")).unwrap();
    let started = Instant::now();
    for t in 0..TEMPLATES {
        let (bytes, _) = representative(t, false);
        let bytes = if t == 8 {
            String::from_utf8(bytes)
                .unwrap()
                .replace(&tid(0).to_string(), &tid(8).to_string())
                .into_bytes()
        } else {
            bytes
        };
        artifact::decode_template(&bytes).unwrap();
        fs::write(root.join(format!("templates/{}.json", tid(t))), bytes).unwrap();
    }
    let mut total = 0;
    let mut maximum = 0;
    let mut digest = sha2::Sha256::new();
    use sha2::Digest;
    for n in 0..size {
        let (_, bytes) = representative(n, false);
        let path = root.join(format!("documents/{}.json", did(n)));
        if path.exists() {
            assert!(fs::read(&path).unwrap() == bytes);
        } else {
            fs::write(path, &bytes).unwrap();
        }
        if n < 8 {
            artifact::decode_document(&bytes).unwrap();
        }
        total += bytes.len();
        maximum = maximum.max(bytes.len());
        digest.update(&bytes);
    }
    let info = json!({"seed":SEED,"schema_version":1,"documents":size,"templates":TEMPLATES,
        "fields":12,"historical_absence":size/4,"used_templates":8,"references_each":size/8,
        "total_document_bytes":total,"mean_document_bytes":total as f64/size as f64,
        "max_document_bytes":maximum,"ordered_document_sha256":format!("{:x}",digest.finalize()),
        "generation_ms":started.elapsed().as_secs_f64()*1000.0});
    fs::write(
        base.join(format!("fixture-{size}.json")),
        serde_json::to_vec_pretty(&info).unwrap(),
    )
    .unwrap();
    fs::write(
        base.join("representative-template.json"),
        representative(0, false).0,
    )
    .unwrap();
    fs::write(
        base.join("representative-document.json"),
        representative(0, false).1,
    )
    .unwrap();
    emit(info);
}

fn scan(base: &Path, size: usize, operation: &str) {
    let root = base.join("project");
    let mut rt = ProjectRuntime::acquire(&root, &base.join("locks")).unwrap();
    rt.recover().unwrap();
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let info: Value =
        serde_json::from_slice(&fs::read(base.join(format!("fixture-{size}.json"))).unwrap())
            .unwrap();
    let total = info["total_document_bytes"].as_u64().unwrap() as usize;
    let target = tid(if operation.ends_with("unused") { 8 } else { 0 });
    let template_bytes = fs::metadata(root.join(format!("templates/{target}.json")))
        .unwrap()
        .len() as usize;
    for sample in 0..4 {
        match operation {
            "documents" => measure(
                operation,
                sample,
                || repo.scan_documents().unwrap(),
                |r, c| {
                    assert_eq!(r.len(), size);
                    assert_eq!(c.decoded, size);
                    assert_eq!(c.bytes_read, total);
                    assert_eq!((c.document_scans, c.iterations), (1, size));
                },
            ),
            "templates" => measure(
                operation,
                sample,
                || repo.scan_templates().unwrap(),
                |r, c| {
                    assert_eq!(r.records().len(), TEMPLATES);
                    assert_eq!(c.decoded, TEMPLATES);
                    let expected: usize = (0..TEMPLATES)
                        .map(|n| {
                            fs::metadata(root.join(format!("templates/{}.json", tid(n))))
                                .unwrap()
                                .len() as usize
                        })
                        .sum();
                    assert_eq!(c.bytes_read, expected);
                    assert_eq!(c.template_scans, 1);
                },
            ),
            "references-used" | "references-unused" => measure(
                operation,
                sample,
                || repo.scan_template_references(target).unwrap(),
                |r, c| {
                    assert_eq!(r.template_ids().count(), size);
                    assert_eq!(
                        r.template_ids().filter(|id| *id == target).count(),
                        if operation.ends_with("unused") {
                            0
                        } else {
                            size / 8
                        }
                    );
                    assert_eq!(c.decoded, size + 1);
                    assert_eq!(c.bytes_read, total + template_bytes);
                    assert_eq!(
                        (c.document_scans, c.reference_scans, c.iterations),
                        (1, 1, size)
                    );
                },
            ),
            "assess-used" | "assess-unused" => measure(
                operation,
                sample,
                || repo.assess_template_references(target),
                |r, c| {
                    if operation.ends_with("unused") {
                        assert!(r.is_ok());
                    } else {
                        assert_eq!(
                            r.as_ref().unwrap_err().diagnostic().category,
                            RepositoryCategory::ReferenceBlocked
                        );
                    }
                    assert_eq!(c.decoded, size + 1);
                    assert_eq!(c.bytes_read, total + template_bytes);
                    assert_eq!(
                        (
                            c.document_scans,
                            c.reference_scans,
                            c.assessments,
                            c.iterations
                        ),
                        (1, 1, 1, size)
                    );
                },
            ),
            _ => panic!("unknown scale operation"),
        }
    }
    drop(repo);
    drop(ready);
    rt.close().unwrap();
}

#[test]
fn m28_representative_v1_fixture_is_deterministic_lossless_and_materializable() {
    for n in 0..8 {
        let (tb, db) = representative(n, false);
        assert!(representative(n, false) == (tb.clone(), db.clone()));
        let t = artifact::decode_template(&tb).unwrap();
        let d = artifact::decode_document(&db).unwrap();
        assert_eq!(t.field_order().len(), 12);
        assert_scale_numbers(&db);
        let encoded = artifact::encode_document(&d).unwrap();
        assert_scale_numbers(&encoded);
        assert!(
            artifact::encode_document(&artifact::decode_document(&encoded).unwrap()).unwrap()
                == encoded
        );
        let saved = artifact::prepare_document_save(
            &t,
            t.revision(),
            &d,
            &DocumentEditSet::new(vec![]),
            SAVE_TIME,
        )
        .unwrap();
        assert_scale_numbers(&artifact::encode_document(saved.document()).unwrap());
        assert!(
            artifact::prepare_document_save(
                &t,
                t.revision(),
                saved.document(),
                &DocumentEditSet::new(vec![]),
                AGAIN
            )
            .unwrap()
            .kind()
                == DocumentSaveOutcomeKind::Unchanged
        );
    }
}

#[test]
#[ignore = "explicit M2-8 scale runner; use scripts/measure-m2.py"]
fn m28_scale_runner() {
    let base = PathBuf::from(std::env::var_os("M28_SCALE_BASE").expect("explicit synthetic base"));
    // runner만 만든 marker가 있어야 한다. 사용자 project를 인자로 받지 않는다.
    assert!(fs::read(base.join("M28-SYNTHETIC-ROOT")).unwrap() == SEED.as_bytes());
    assert!(!fs::symlink_metadata(&base)
        .unwrap()
        .file_type()
        .is_symlink());
    let size = std::env::var("M28_SCALE_SIZE").unwrap().parse().unwrap();
    let operation = std::env::var("M28_SCALE_OPERATION").unwrap();
    if operation == "generate" {
        generate(&base, size);
    } else if operation == "single" {
        storage::single();
    } else if operation == "late" {
        storage::late(&base, size);
    } else {
        scan(&base, size, &operation);
    }
}
