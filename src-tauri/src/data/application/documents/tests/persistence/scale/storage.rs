use super::*;

pub(super) fn single() {
    for large in [false, true] {
        let f = Fixture::new();
        let (tb, db) = representative(4, large);
        let t = artifact::decode_template(&tb).unwrap();
        let d = artifact::decode_document(&db).unwrap();
        let tid = t.template_id();
        let did = d.document_id();
        fs::create_dir(f.root.join("templates")).unwrap();
        fs::create_dir(f.root.join("documents")).unwrap();
        fs::write(template_path(&f, tid), &tb).unwrap();
        fs::write(document_path(&f, did), &db).unwrap();
        let other = super::did(90000);
        let mut unrelated: Value = serde_json::from_slice(&db).unwrap();
        unrelated["documentId"] = other.to_string().into();
        fs::write(
            document_path(&f, other),
            serde_json::to_vec(&unrelated).unwrap(),
        )
        .unwrap();
        let other_before = disk(&document_path(&f, other));
        let t_before = disk(&template_path(&f, tid));
        let edits = DocumentEditSet::new(vec![DocumentEdit::Rename("새 문서 이름".into())]);
        emit(
            json!({"operation":"single-input","large":large,"document_bytes":db.len(),"template_bytes":tb.len(),"baseline":memory()}),
        );
        for sample in 0..4 {
            measure(
                if large {
                    "pure-reconcile-large"
                } else {
                    "pure-reconcile"
                },
                sample,
                || artifact::reconcile_document(&t, &d).unwrap(),
                |_, c| assert_eq!(c.bytes_read, 0),
            );
            measure(
                if large {
                    "pure-save-large"
                } else {
                    "pure-save"
                },
                sample,
                || {
                    artifact::prepare_document_save(&t, t.revision(), &d, &edits, SAVE_TIME)
                        .unwrap()
                },
                |r, c| {
                    assert_eq!(r.kind(), DocumentSaveOutcomeKind::Changed);
                    assert_eq!(c.bytes_read, 0);
                    assert_scale_numbers(&artifact::encode_document(r.document()).unwrap());
                },
            );
        }
        assert!(
            artifact::encode_document(&d).unwrap()
                == artifact::encode_document(&artifact::decode_document(&db).unwrap()).unwrap()
        );
        let mut rt = f.runtime();
        let targets = doc_targets(did);
        let mut session: Session = begin(&rt, targets.session_targets());
        let snap = session.snapshot();
        // Changed와 Unchanged를 따로 기록한다. 반복 저장은 전체 원자적 저장이 아니다.
        for (sample, expected) in [(0, DiskState::Committed), (1, DiskState::NoWrite)] {
            let input = save_input(
                &mut rt,
                tid,
                did,
                vec![DocumentEdit::Rename("새 문서 이름".into())],
            );
            let before = disk(&document_path(&f, did));
            measure(
                if large { "g8-save-large" } else { "g8-save" },
                sample,
                || {
                    observe(|| {
                        save_document(&mut rt, &mut session, context(&snap), &input).unwrap()
                    })
                },
                |(r, p, k), c| {
                    assert_eq!(r.execution.diagnostic().disk, expected);
                    assert_eq!(
                        (p.calls, p.allocations, *k),
                        if sample == 0 { (1, 1, 1) } else { (0, 0, 0) }
                    );
                    assert_eq!(c.document_scans, 0);
                    emit(
                        json!({"operation":"g8-write-counts","large":large,"sample":sample,
                        "prepare_calls":p.calls,"candidate_operation_allocations":p.allocations,"commit_calls":k}),
                    );
                    assert_scale_numbers(&fs::read(document_path(&f, did)).unwrap());
                    if sample == 1 {
                        assert_disk(&document_path(&f, did), &before);
                    }
                },
            );
        }
        assert_disk(&template_path(&f, tid), &t_before);
        assert_disk(&document_path(&f, other), &other_before);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

pub(super) fn late(base: &Path, size: usize) {
    assert_eq!(size, 20_000);
    let root = base.join("project");
    let path = root.join(format!("documents/{}.json", did(size - 1)));
    let before = fs::read(&path).unwrap();
    // 마지막 정렬 ID만 손상시킨다. 실패해도 Drop이 합성 원본을 복원한다.
    struct Restore(PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Err(error) = fs::write(&self.0, &self.1) {
                if std::thread::panicking() {
                    eprintln!("M2-8 synthetic tail restore failed: {:?}", error.kind());
                } else {
                    panic!("M2-8 synthetic tail restore failed: {:?}", error.kind());
                }
            }
        }
    }
    let _restore = Restore(path.clone(), before);
    fs::write(&path, b"{").unwrap();
    let mut rt = ProjectRuntime::acquire(&root, &base.join("locks")).unwrap();
    rt.recover().unwrap();
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    for target in [0, 8] {
        let source = root.join(format!("templates/{}.json", tid(target)));
        let original = disk(&source);
        measure(
            if target == 0 {
                "late-used-refused"
            } else {
                "late-unused-refused"
            },
            0,
            || repo.assess_template_references(tid(target)),
            |r, c| {
                assert_eq!(
                    r.as_ref().unwrap_err().diagnostic().category,
                    RepositoryCategory::CodecRejected
                );
                assert_eq!(
                    (
                        c.decoded,
                        c.document_scans,
                        c.reference_scans,
                        c.assessments
                    ),
                    (size, 1, 1, 1)
                );
                assert_eq!(c.iterations, size);
                assert_disk(&source, &original);
                assert!(fs::read(&path).unwrap() == b"{");
            },
        );
    }
    drop(repo);
    drop(ready);
    rt.close().unwrap();
}
