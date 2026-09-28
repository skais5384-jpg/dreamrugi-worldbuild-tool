use super::*;
use crate::data::{
    application::layout::{self, LayoutSnapshot, LayoutWrite},
    artifact::layout::{DocumentLayout, LayoutEdit},
    transaction::PrepareFailPoint,
};
use std::{collections::BTreeMap, io, sync::Arc};
fn input(rt: &mut ProjectRuntime, t: TemplateId) -> LayoutWrite {
    let source = source(rt, t);
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let documents = Arc::new(repo.scan_documents().unwrap());
    let template = repo.load_template(t).unwrap();
    let document = artifact::create_document(
        template.artifact(),
        source.expected_revision,
        DocumentId::new(),
        "pair".into(),
        TIME.into(),
    )
    .unwrap()
    .into_document();
    LayoutWrite {
        base: LayoutSnapshot {
            layout: DocumentLayout::flat([]),
            source: None,
        },
        documents,
        edit: LayoutEdit::Adopt {},
        timestamp: TIME.into(),
        create: Some((document, source, None)),
    }
}
#[test]
fn m39_fix002_creation_reuses_the_prevalidated_scan_without_a_second_full_scan() {
    let f = Fixture::new();
    let mut runtime = f.runtime();
    let template = seed_template(&mut runtime);
    let input = input(&mut runtime, template);
    let targets = input.targets().unwrap();
    let mut session: Session = begin(&runtime, &targets);
    let snapshot = session.snapshot();
    let (execution, counts) = crate::data::repository::test_support::scoped(None, false, || {
        layout::execute(&mut runtime, &mut session, context(&snapshot), &input).unwrap()
    });
    assert_eq!(execution.diagnostic().disk, DiskState::Committed);
    assert_eq!(counts.document_scans, 0);
    let commit = execution
        .value()
        .expect("authoritative post-commit projection");
    assert_eq!(commit.documents.len(), 1);
    assert!(commit.unplaced.is_empty());
    assert_eq!(commit.layout.nodes.len(), 1);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
#[test]
fn layout_pair_second_prepare_failure_is_no_write_and_retry_uses_same_id() {
    let f = Fixture::new();
    let mut rt = f.runtime();
    let t = seed_template(&mut rt);
    let input = input(&mut rt, t);
    let targets = input.targets().unwrap();
    let mut session: Session = begin(&rt, &targets);
    let snapshot = session.snapshot();
    let hits = Rc::new(Cell::new(0));
    let h = hits.clone();
    let (result, _) = with_canonical_prepare_hooks(
        move |point, _| {
            if point == PrepareFailPoint::StagedWrite {
                h.set(h.get() + 1);
                if h.get() == 2 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "second staged target",
                    ));
                }
            }
            Ok(())
        },
        || layout::execute(&mut rt, &mut session, context(&snapshot), &input).unwrap(),
    );
    assert_ne!(result.diagnostic().disk, DiskState::Committed);
    assert!(!f.root.join("workspace/document-layout.json").exists());
    let id = input.create.as_ref().unwrap().0.document_id();
    assert!(!document_path(&f, id).exists());
    let result = layout::execute(&mut rt, &mut session, context(&snapshot), &input).unwrap();
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    let ready = rt.ready().unwrap();
    let r = ArtifactRepository::new(&ready).unwrap();
    assert!(r
        .load_layout()
        .unwrap()
        .unwrap()
        .artifact()
        .nodes
        .contains_key(&id));
    assert_eq!(r.load_document(id).unwrap().artifact().document_id(), id);
    drop(r);
    drop(ready);
    session.end_edit().unwrap();
    rt.close().unwrap();
}
#[test]
fn layout_required_unset_accepts_typed_initial_values_without_invalid_intermediate() {
    let f = Fixture::new();
    let mut rt = f.runtime();
    let t = seed_template(&mut rt);
    add_field(
        &mut rt,
        t,
        NewFieldDraft::new(
            field(1),
            "required".into(),
            FieldKind::Number,
            NewFieldConfiguration::number(),
            true,
            None,
            FieldValueDraft::unset(),
        ),
        LATER,
    );
    let ready = rt.ready().unwrap();
    let r = ArtifactRepository::new(&ready).unwrap();
    let template = r.load_template(t).unwrap();
    let id = DocumentId::new();
    assert!(artifact::create_document(
        template.artifact(),
        template.artifact().revision(),
        id,
        "required".into(),
        LATER.into()
    )
    .is_err());
    for raw in ["-", "."] {
        let values = BTreeMap::from([(
            field(1),
            artifact::DocumentValueEdit::number(raw.into()).into_field_value(),
        )]);
        assert!(artifact::create_document_with_values(
            template.artifact(),
            template.artifact().revision(),
            id,
            "required".into(),
            false,
            LATER.into(),
            &values
        )
        .is_err());
    }
    let values = BTreeMap::from([(
        field(1),
        artifact::DocumentValueEdit::number("12".into()).into_field_value(),
    )]);
    let candidate = artifact::create_document_with_values(
        template.artifact(),
        template.artifact().revision(),
        id,
        "required".into(),
        false,
        LATER.into(),
        &values,
    )
    .unwrap();
    assert_eq!(candidate.document().document_id(), id);
    assert!(!document_path(&f, id).exists());
    drop(r);
    drop(ready);
    rt.close().unwrap();
}
#[test]
fn layout_cancelled_scan_does_not_issue_completeness_and_retry_completes() {
    let f = Fixture::new();
    let mut rt = f.runtime();
    let t = seed_template(&mut rt);
    let input = input(&mut rt, t);
    let mut s: Session = begin(&rt, &input.targets().unwrap());
    let snap = s.snapshot();
    assert_eq!(
        layout::execute(&mut rt, &mut s, context(&snap), &input)
            .unwrap()
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    s.end_edit().unwrap();
    let ready = rt.ready().unwrap();
    let r = ArtifactRepository::new(&ready).unwrap();
    let progress = Arc::new(crate::data::repository::progress::Progress::default());
    let scope = crate::data::repository::progress::scope(progress.clone());
    progress.cancel();
    let failed = r.scan_documents().unwrap_err();
    assert_eq!(
        failed.diagnostic().category,
        crate::data::repository::RepositoryCategory::Cancelled
    );
    drop(scope);
    assert_eq!(r.scan_documents().unwrap().len(), 1);
    drop(r);
    drop(ready);
    rt.close().unwrap();
}

#[test]
fn layout_pair_apply_marker_cleanup_failures_recover_to_consistent_files() {
    for fail in [
        CommitTestPoint::Rename,
        CommitTestPoint::CommittedMarkerVerify,
        CommitTestPoint::Cleanup,
    ] {
        let f = Fixture::new();
        let mut rt = f.runtime();
        let t = seed_template(&mut rt);
        let input = input(&mut rt, t);
        let id = input.create.as_ref().unwrap().0.document_id();
        let mut session: Session = begin(&rt, &input.targets().unwrap());
        let snap = session.snapshot();
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        let result = with_commit_io_factory(
            move |point, _| {
                if point == Some(fail) {
                    h.set(h.get() + 1);
                    if (fail == CommitTestPoint::Rename && h.get() == 2)
                        || (fail != CommitTestPoint::Rename && h.get() == 1)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "layout pair injected failure",
                        ));
                    }
                }
                Ok(())
            },
            || layout::execute(&mut rt, &mut session, context(&snap), &input).unwrap(),
        );
        assert!(hits.get() > 0);
        if fail == CommitTestPoint::Cleanup {
            assert_eq!(result.diagnostic().disk, DiskState::Committed);
            assert!(result
                .diagnostic()
                .artifact_commit
                .as_ref()
                .is_some_and(|c| c.cleanup_failed));
        } else {
            assert_ne!(result.diagnostic().disk, DiskState::NotAttempted);
        }
        session.end_edit().unwrap();
        rt.close().unwrap();
        let mut rt = f.runtime();
        let ready = rt.ready().unwrap();
        let repo = ArtifactRepository::new(&ready).unwrap();
        let layout = repo.load_layout().unwrap();
        let document = repo.load_document(id).ok();
        assert_eq!(
            layout
                .as_ref()
                .is_some_and(|l| l.artifact().nodes.contains_key(&id)),
            document.is_some(),
            "pair must agree after recovery: {fail:?}"
        );
        if fail != CommitTestPoint::Rename {
            assert!(document.is_some());
        }
        drop(repo);
        drop(ready);
        rt.close().unwrap();
    }
}
#[test]
fn layout_pair_rejects_stale_template_layout_occupied_id_and_namespace_race() {
    for case in ["template", "layout", "occupied", "namespace"] {
        let f = Fixture::new();
        let mut rt = f.runtime();
        let t = seed_template(&mut rt);
        let input = input(&mut rt, t);
        let id = input.create.as_ref().unwrap().0.document_id();
        let mut session: Session = begin(&rt, &input.targets().unwrap());
        let snap = session.snapshot();
        if case == "template" {
            let path = template_path(&f, t);
            let mut b = fs::read(&path).unwrap();
            b.push(b' ');
            fs::write(path, b).unwrap();
        }
        if case == "layout" {
            fs::create_dir_all(f.root.join("workspace")).unwrap();
            fs::write(
                f.root.join("workspace/document-layout.json"),
                artifact::encode_layout(&DocumentLayout::flat([])).unwrap(),
            )
            .unwrap();
        }
        if case == "occupied" {
            fs::create_dir_all(f.root.join("documents")).unwrap();
            fs::write(
                document_path(&f, id),
                artifact::encode_document(&input.create.as_ref().unwrap().0).unwrap(),
            )
            .unwrap();
        }
        let mut run = || layout::execute(&mut rt, &mut session, context(&snap), &input).unwrap();
        let result = if case == "namespace" {
            let root = f.root.clone();
            let bytes = artifact::encode_document(&input.create.as_ref().unwrap().0).unwrap();
            crate::data::repository::test_support::scoped(
                Some((
                    crate::data::repository::RepositoryStage::Namespace,
                    0,
                    Box::new(move |_| {
                        fs::create_dir_all(root.join("documents"))?;
                        fs::write(root.join(format!("documents/{id}.json")), &bytes)?;
                        Ok(())
                    }),
                )),
                false,
                run,
            )
            .0
        } else {
            run()
        };
        assert_ne!(result.diagnostic().disk, DiskState::Committed, "{case}");
        assert!(
            !f.root.join("workspace/document-layout.json").exists() || case == "layout",
            "{case}"
        );
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}
#[test]
fn layout_trash_blocks_legacy_save_without_document_scan_and_keeps_template_reference() {
    use crate::data::application::documents::persistence::*;
    let f = Fixture::new();
    let mut rt = f.runtime();
    let t = seed_template(&mut rt);
    let initial = input(&mut rt, t);
    let id = initial.create.as_ref().unwrap().0.document_id();
    let mut s: Session = begin(&rt, &initial.targets().unwrap());
    let snap = s.snapshot();
    assert_eq!(
        layout::execute(&mut rt, &mut s, context(&snap), &initial)
            .unwrap()
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    s.end_edit().unwrap();
    let ready = rt.ready().unwrap();
    let r = ArtifactRepository::new(&ready).unwrap();
    let loaded = r.load_layout().unwrap().unwrap();
    let document_token = r.load_document(id).unwrap().source().clone();
    let documents = Arc::new(r.scan_documents().unwrap());
    let base = LayoutSnapshot {
        source: Some(loaded.source().clone()),
        layout: loaded.into_artifact(),
    };
    drop(r);
    drop(ready);
    let trash = LayoutWrite {
        base,
        documents,
        edit: LayoutEdit::Trash { document: id },
        timestamp: LATER.into(),
        create: None,
    };
    let mut s: Session = begin(&rt, &trash.targets().unwrap());
    let snap = s.snapshot();
    assert_eq!(
        layout::execute(&mut rt, &mut s, context(&snap), &trash)
            .unwrap()
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    s.end_edit().unwrap();
    let before = fs::read(document_path(&f, id)).unwrap();
    let input = SaveDocumentInput {
        document: DocumentSource {
            id,
            token: document_token,
        },
        template: source(&mut rt, t),
        edits: artifact::DocumentEditSet::new(vec![artifact::DocumentEdit::Rename(
            "blocked".into(),
        )]),
        timestamp_utc: LATER.into(),
    };
    let mut s: Session = begin(&rt, &[ArtifactSourceId::Document(id).path().unwrap()]);
    let snap = s.snapshot();
    let (result, counts) = crate::data::repository::test_support::scoped(None, false, || {
        save_document(&mut rt, &mut s, context(&snap), &input).unwrap()
    });
    assert_ne!(result.execution.diagnostic().disk, DiskState::Committed);
    assert_eq!(counts.document_scans, 0);
    assert_eq!(fs::read(document_path(&f, id)).unwrap(), before);
    s.end_edit().unwrap();
    let ready = rt.ready().unwrap();
    let r = ArtifactRepository::new(&ready).unwrap();
    assert_eq!(r.scan_documents().unwrap().len(), 1);
    assert!(r.assess_template_references(t).is_err());
    drop(r);
    drop(ready);
    rt.close().unwrap();
}
