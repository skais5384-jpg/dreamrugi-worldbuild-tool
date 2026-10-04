use super::*;
use crate::data::{
    edit_session::{EditSessionService, RecoveryHandoffStatus},
    project_runtime::ProjectRuntime,
};
use std::fs;

#[test]
fn recovery_real_adapter_keeps_exact_p_and_lossless_sources() {
    let base = std::env::temp_dir().join(format!("worldbuild-adapter-{}", uuid::Uuid::new_v4()));
    let root = base.join("project");
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::create_dir(root.join("documents")).unwrap();
    let (tb, db) = crate::data::application::composite::tests::guarded_fixture_bytes();
    let template = artifact::decode_template(&tb).unwrap();
    let document = artifact::decode_document(&db).unwrap();
    let tid = template.template_id();
    let did = document.document_id();
    fs::write(root.join(format!("templates/{tid}.json")), &tb).unwrap();
    fs::write(root.join(format!("documents/{did}.json")), &db).unwrap();
    let mut runtime = ProjectRuntime::acquire(&root, &base.join("locks")).unwrap();
    runtime.recover().unwrap();
    let local = runtime
        .latest_input_sink(Arc::new(std::sync::atomic::AtomicU64::new(1)))
        .unwrap();
    let ready = runtime.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let views = vec![
        (
            Id::new(),
            Arc::new(View::Template(repo.load_template(tid).unwrap())),
        ),
        (
            Id::new(),
            Arc::new(View::Document(repo.load_document(did).unwrap())),
        ),
    ];
    let input = Arc::new(Work::SaveComposite {
        project: Id::new(),
        session: Id::new(),
        template: views[0].0,
        document: views[1].0,
        revision: "3".into(),
        edit: TemplateEdit::KeepDefault {
            field: "99999999-9999-4999-8999-000000000004".into(),
        },
        edits: vec![
            DocumentEdit::Set {
                field: "raw-invalid".into(),
                value: ValueDto::Number { value: "-".into() },
            },
            DocumentEdit::Unset {
                field: "explicit-unset".into(),
            },
        ],
    });
    let job = Job {
        progress: Arc::default(),
        workspace: Arc::new(std::sync::Mutex::new(
            super::super::workspace::Registry::default(),
        )),
        input: input.clone(),
        views,
        binding: None,
        provider: provider(),
        collaborative: false,
        allocated: Id::new(),
        operation: Id::new(),
        recovery: Arc::new(edit_recovery::Owner::new(base.join("edit-recovery"))),
    };
    let payload = PendingEdit::new(&job, &"a".repeat(64)).unwrap();
    assert!(payload
        .recovery
        .attempt
        .set(attempt(job.operation, None))
        .is_ok());
    let mut direct_envelope = payload.recovery.envelope.clone();
    direct_envelope.attempt = payload.recovery.attempt.get().cloned();
    let exact_direct = Deposit::freeze(direct_envelope.clone()).unwrap();
    assert!(same_durable_deposit(&payload, &exact_direct));
    direct_envelope.key.generation += 1;
    assert!(!same_durable_deposit(
        &payload,
        &Deposit::freeze(direct_envelope).unwrap()
    ));
    let original_key = payload.recovery.envelope.key.clone();
    let deposit_id = payload.recovery.envelope.deposit_id.clone();
    let store = job.recovery.connect().unwrap();
    store.lock().unwrap().fault = Some(Stage::Reopen);
    let targets = vec![
        ArtifactSourceId::Template(tid).path().unwrap(),
        ArtifactSourceId::Document(did).path().unwrap(),
    ];
    let mut service = EditSessionService::<PendingEdit, Receipt>::new(provider());
    service.begin_edit(&"a".repeat(64), targets).unwrap();
    let snapshot = service.snapshot();
    let sink = Sink {
        local,
        owner: job.recovery.clone(),
        project: "a".repeat(64),
        session: snapshot.session_id().unwrap().clone(),
        targets: snapshot.targets().to_vec(),
        observation: Arc::new(Observation::default()),
    };
    struct Probe {
        sink: Sink,
        work: Arc<Work>,
        views: Vec<Arc<View>>,
        digest: Option<String>,
    }
    impl DurableRecoverySink<PendingEdit> for Probe {
        type Receipt = Receipt;
        fn accept_durably(
            &mut self,
            e: &RecoveryEnvelope,
            p: PendingEdit,
        ) -> Result<Receipt, RecoverySinkFailure<PendingEdit>> {
            assert!(Arc::ptr_eq(&self.work, &p.input));
            assert!(self
                .views
                .iter()
                .zip(&p.views)
                .all(|(a, b)| Arc::ptr_eq(a, b)));
            if let Some(digest) = &self.digest {
                assert_eq!(p.recovery.frozen.as_ref().unwrap().payload_digest(), digest);
            }
            match self.sink.accept_durably(e, p) {
                Ok(r) => Ok(r),
                Err(f) => {
                    let (p, category) = f.into_parts();
                    assert!(Arc::ptr_eq(&self.work, &p.input));
                    self.digest = Some(
                        p.recovery
                            .frozen
                            .as_ref()
                            .unwrap()
                            .payload_digest()
                            .to_owned(),
                    );
                    Err(RecoverySinkFailure::new(p, category))
                }
            }
        }
    }
    let mut probe = Probe {
        sink,
        work: input,
        views: job.views.iter().map(|(_, v)| v.clone()).collect(),
        digest: None,
    };
    service.preserve_for_recovery(payload).unwrap();
    assert!(service.accept_recovery_durably(&mut probe).is_err());
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::Pending)
    );
    assert!(service.acknowledge_recovery().is_err());
    store.lock().unwrap().fault = None;
    service.accept_recovery_durably(&mut probe).unwrap();
    assert_eq!(
        service.snapshot().recovery_handoff(),
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert!(service.accept_recovery_durably(&mut probe).is_err()); // 중복 인수가 새 owner를 만들지 않는다.
    let receipt = service.acknowledge_recovery().unwrap();
    let proof = receipt
        ._proof
        .downcast_ref::<edit_recovery::Proof>()
        .unwrap();
    assert!(proof.matches(&original_key, &deposit_id, probe.digest.as_ref().unwrap()));
    service.end_edit().unwrap();
    let restored = store
        .lock()
        .unwrap()
        .read(&original_key, &deposit_id)
        .unwrap();
    assert_eq!(
        restored.envelope().attempt.as_ref().unwrap().result,
        SaveState::Unknown
    );
    assert!(
        matches!(&restored.envelope().draft, Draft::AdmittedComposite { edits, .. } if matches!(&edits[0], DocumentEdit::Set { value: ValueDto::Number { value }, .. } if value == "-"))
    );
    for (index, bytes) in [&tb, &db].into_iter().enumerate() {
        let original = &restored.envelope().originals[index];
        assert_eq!(original.source_digest, digest(bytes));
        let encoded = if index == 0 {
            artifact::encode_template(&template).unwrap()
        } else {
            artifact::encode_document(&document).unwrap()
        };
        assert_eq!(original.snapshot.as_bytes(), encoded);
        for lexeme in ["1E100", "1e100", "-0", "0.12345678901234567890123456789"] {
            assert!(original.snapshot.contains(lexeme));
        }
    }
    // 같은 저장소의 typed 전체 초안/생성 계약도 실제 파일 왕복한다. 제품 registry 연결은 2B다.
    for create in [false, true] {
        let mut full = restored.envelope().clone();
        full.key.draft_id = uuid::Uuid::new_v4().to_string();
        full.deposit_id = uuid::Uuid::new_v4().to_string();
        full.attempt = None;
        if create {
            full.originals.retain(|o| o.kind == OriginalKind::Template);
        }
        full.draft = Draft::Document {
            document: (!create).then(|| did.to_string()),
            template: tid.to_string(),
            name: Intent::Keep,
            english_name: Intent::Keep,
            glossary_summary: Intent::Keep,
            glossary_excluded: Intent::Keep,
            composing: true,
            fields: vec![
                DraftValue {
                    field: "raw".into(),
                    value: Intent::Set(ValueDto::Number { value: ".".into() }),
                },
                DraftValue {
                    field: "keep".into(),
                    value: Intent::Keep,
                },
                DraftValue {
                    field: "unset".into(),
                    value: Intent::Unset,
                },
            ],
        };
        let full = Deposit::freeze(full).unwrap();
        store.lock().unwrap().accept(&full).unwrap();
        assert!(
            store
                .lock()
                .unwrap()
                .read(full.key(), &full.envelope().deposit_id)
                .unwrap()
                .envelope()
                == full.envelope()
        );
    }
    assert_eq!(
        fs::read(root.join(format!("templates/{tid}.json"))).unwrap(),
        tb
    );
    assert_eq!(
        fs::read(root.join(format!("documents/{did}.json"))).unwrap(),
        db
    );
    println!(
        "recovery fixture: template={} document={} recovery={}",
        tb.len(),
        db.len(),
        restored.bytes().len()
    );
    drop(service);
    drop(probe);
    drop(store);
    drop(job);
    drop(repo);
    drop(ready);
    runtime.close().unwrap();
    fs::remove_dir_all(base).unwrap();
}
