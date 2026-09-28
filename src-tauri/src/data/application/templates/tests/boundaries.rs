use super::*;
use crate::data::{
    application::diagnostics::ApplicationCategory, transaction::test_support::with_commit_failures,
};

#[test]
fn g6_creation_exact_preflight_and_nonclone_owners_are_preserved() {
    struct Payload(Box<u8>);
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let input = input();
    let pointer = input.name.as_ptr();
    let mut ticket = prepare_create_template(&mut runtime, &input).unwrap();
    let id = ticket.template_id();
    let candidate = ticket.candidate() as *const _;
    let mut session = begin::<Payload>(&runtime, ticket.session_targets());
    // 이미 Editing인 세션의 begin 실패에서도 caller의 준비/입력은 소비되지 않는다.
    assert!(session
        .begin_edit(
            runtime.project_fingerprint(),
            ticket.session_targets().to_vec()
        )
        .is_err());
    let snapshot = session.snapshot();
    let (result, counts, commits) = observe(|| {
        create_template(
            &mut runtime,
            &mut session,
            TemplateWriteContext {
                project: "wrong-project",
                session: snapshot.session_id().unwrap(),
            },
            &mut ticket,
        )
    });
    assert_no_io(&result, counts, commits);
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::ProjectMismatch)
    );
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    assert_eq!(ticket.template_id(), id);
    assert_eq!(candidate, ticket.candidate() as *const _);
    assert_eq!(input.name.as_ptr(), pointer);
    assert!(!fixture.root.join("templates").exists());
    let payload = Payload(Box::new(79));
    let payload_ptr = &*payload.0 as *const _;
    let mut idle = EditSessionService::<Payload, ()>::new(Arc::new(NoLockService::new()));
    let failure = idle.preserve_for_recovery(payload).unwrap_err();
    let payload = failure.into_payload();
    assert_eq!(&*payload.0 as *const _, payload_ptr);
    let wrong = ExactWriteTargets::new([ArtifactSourceId::Template(TemplateId::new())]).unwrap();
    session
        .change_targets(wrong.session_targets().to_vec())
        .unwrap();
    let snapshot = session.snapshot();
    let (result, counts, commits) =
        observe(|| create_template(&mut runtime, &mut session, context(&snapshot), &mut ticket));
    assert_no_io(&result, counts, commits);
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::SessionTargetsMismatch)
    );
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g6_create_existing_namespace_and_id_occupancy_fail_closed() {
    for occupied in [
        "absent", "active", "deleted", "corrupt", "future", "wrong-id",
    ] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        fs::create_dir(fixture.root.join("templates")).unwrap();
        let mut ticket = prepare_create_template(&mut runtime, &input()).unwrap();
        let id = ticket.template_id();
        if occupied != "absent" {
            let mut value: serde_json::Value =
                serde_json::from_slice(&artifact::encode_template(ticket.candidate()).unwrap())
                    .unwrap();
            match occupied {
                "deleted" => value["lifecycle"] = "deleted".into(),
                "future" => value["schemaVersion"] = 99.into(),
                "wrong-id" => value["templateId"] = TemplateId::new().to_string().into(),
                _ => {}
            }
            let bytes = if occupied == "corrupt" {
                b"{".to_vec()
            } else {
                serde_json::to_vec(&value).unwrap()
            };
            fs::write(path(&fixture, id), bytes).unwrap();
        }
        let old = fs::read(path(&fixture, id)).ok();
        let mut session: Session = begin(&runtime, ticket.session_targets());
        let snap = session.snapshot();
        let (result, counts, commits) =
            observe(|| create_template(&mut runtime, &mut session, context(&snap), &mut ticket));
        if occupied == "absent" {
            assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
            assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
        } else {
            assert_no_io(&result, counts, commits);
            assert!(fs::read(path(&fixture, id)).unwrap() == old.unwrap());
            assert_eq!(ticket.state(), CreationState::Uncommitted);
        }
        assert_eq!(ticket.template_id(), id);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn g6_namespace_race_revalidation_and_invalid_entries() {
    for kind in ["directory", "file", "junction", "denied"] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let mut ticket = prepare_create_template(&mut runtime, &input()).unwrap();
        let mut session: Session = begin(&runtime, ticket.session_targets());
        let snap = session.snapshot();
        let outside = fixture.base.join("outside");
        fs::create_dir(&outside).unwrap();
        let ((result, counts, commits), hook) = repository_test::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(move |target| {
                    match kind {
                        "directory" => fs::create_dir(target)?,
                        "file" => fs::write(target, b"external entry")?,
                        "junction" => {
                            let output = std::process::Command::new("cmd.exe")
                                .args(["/C", "mklink", "/J"])
                                .arg(target)
                                .arg(&outside)
                                .output()?;
                            assert!(output.status.success());
                        }
                        _ => return Err(io::Error::from_raw_os_error(5)),
                    }
                    Ok(())
                }),
            )),
            false,
            || observe(|| create_template(&mut runtime, &mut session, context(&snap), &mut ticket)),
        );
        assert_eq!(hook.hooks, 1);
        if kind == "directory" {
            assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
            assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
        } else {
            assert_no_io(&result, counts, commits);
            assert_eq!(journals(&fixture), 0);
            assert!(!path(&fixture, ticket.template_id()).exists());
            assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn g6_create_prepare_failure_retains_candidate_and_empty_namespace() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let mut ticket = prepare_create_template(&mut runtime, &input()).unwrap();
    let ptr = ticket.candidate() as *const _;
    let id = ticket.template_id();
    let mut session: Session = begin(&runtime, ticket.session_targets());
    let snap = session.snapshot();
    let (result, counts) = with_canonical_prepare_hooks(
        |point, _| {
            if point == PrepareFailPoint::StagedWrite {
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        },
        || create_template(&mut runtime, &mut session, context(&snap), &mut ticket),
    );
    assert_eq!((counts.calls, counts.allocations), (1, 1));
    assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
    assert_eq!(ticket.state(), CreationState::Uncommitted);
    assert_eq!(ticket.candidate() as *const _, ptr);
    assert_eq!(ticket.template_id(), id);
    assert_eq!(
        fs::read_dir(fixture.root.join("templates"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(journals(&fixture), 0);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    let result = create_template(&mut runtime, &mut session, context(&snap), &mut ticket);
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    let (again, counts, commits) =
        observe(|| create_template(&mut runtime, &mut session, context(&snap), &mut ticket));
    assert_no_io(&again, counts, commits);
    assert_eq!(ticket.state(), CreationState::Committed);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g6_create_commit_not_applied_requires_explicit_recovery_without_reissue() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let mut ticket = prepare_create_template(&mut runtime, &input()).unwrap();
    let id = ticket.template_id();
    let mut session: Session = begin(&runtime, ticket.session_targets());
    let snap = session.snapshot();
    let result = with_commit_failures(Some(CommitTestPoint::ManifestRevalidation), None, || {
        create_template(&mut runtime, &mut session, context(&snap), &mut ticket)
    });
    assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
    assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
    assert_eq!(ticket.state(), CreationState::RecoveryRequired);
    assert!(!path(&fixture, id).exists());
    runtime.recover().unwrap();
    let (again, counts, commits) =
        observe(|| create_template(&mut runtime, &mut session, context(&snap), &mut ticket));
    assert_no_io(&again, counts, commits);
    assert_eq!(ticket.template_id(), id);
    assert!(!path(&fixture, id).exists());
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g6_wrong_session_project_and_exact_plan_do_not_create_namespace() {
    let a = Fixture::new();
    let b = Fixture::new();
    let mut ra = a.runtime();
    let mut rb = b.runtime();
    let mut ticket = prepare_create_template(&mut ra, &input()).unwrap();
    let mut sa: Session = begin(&ra, ticket.session_targets());
    let mut sb: Session = begin(&rb, ticket.session_targets());
    let snap_a = sa.snapshot();
    let snap_b = sb.snapshot();
    let (result, c, k) = observe(|| {
        create_template(
            &mut ra,
            &mut sa,
            TemplateWriteContext {
                project: snap_a.project_fingerprint().unwrap(),
                session: snap_b.session_id().unwrap(),
            },
            &mut ticket,
        )
    });
    assert_no_io(&result, c, k);
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::SessionMismatch)
    );
    let (result, c, k) =
        observe(|| create_template(&mut rb, &mut sb, context(&snap_b), &mut ticket));
    assert_no_io(&result, c, k);
    assert_eq!(ticket.state(), CreationState::Uncommitted);
    let other = artifact::create_template("other".into(), None, TIME.into()).unwrap();
    let (result, c, k) = observe(|| {
        execute_write_operation(
            &mut ra,
            &mut sa,
            WriteRequest {
                project: snap_a.project_fingerprint().unwrap(),
                session: snap_a.session_id().unwrap(),
                targets: &ticket.targets,
            },
            &mut (),
            &mut |_, _| -> Result<_, BuildError<TemplateUseCaseError>> {
                Ok(WriteDecision::Write {
                    plan: CanonicalWritePlan::new().create_template(&other)?,
                    value: (),
                })
            },
        )
    });
    assert_no_io(&result, c, k);
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::PlanTargetsMismatch)
    );
    assert!(!a.root.join("templates").exists());
    assert!(!b.root.join("templates").exists());
    sa.end_edit().unwrap();
    sb.end_edit().unwrap();
    ra.close().unwrap();
    rb.close().unwrap();
}

#[test]
fn g6_namespace_guard_remains_locked_through_prepare_and_commit() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let mut ticket = prepare_create_template(&mut runtime, &input()).unwrap();
    let mut session: Session = begin(&runtime, ticket.session_targets());
    let snap = session.snapshot();
    let from = fixture.root.join("templates");
    let to = fixture.root.join("replaced");
    let from_commit = from.clone();
    let to_commit = to.clone();
    let hit = Rc::new(Cell::new(0));
    let prepare_hit = Rc::clone(&hit);
    let commit_hit = Rc::clone(&hit);
    let (result, counts) = with_canonical_prepare_hooks(
        move |point, _| {
            if point == PrepareFailPoint::BeforeOriginalRead {
                let error = fs::rename(&from, &to).unwrap_err();
                assert_eq!(error.raw_os_error(), Some(32));
                prepare_hit.set(prepare_hit.get() + 1);
            }
            Ok(())
        },
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        let error = fs::rename(&from_commit, &to_commit).unwrap_err();
                        assert_eq!(error.raw_os_error(), Some(32));
                        commit_hit.set(commit_hit.get() + 1);
                    }
                    Ok(())
                },
                || create_template(&mut runtime, &mut session, context(&snap), &mut ticket),
            )
        },
    );
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    assert_eq!((counts.calls, counts.allocations, hit.get()), (1, 1, 2));
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
