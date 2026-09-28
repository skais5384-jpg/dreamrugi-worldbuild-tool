use super::*;
use crate::data::{
    collaboration_lock::NoLockService, edit_session::EditSessionState,
    project_runtime::RuntimeState,
};
use crate::data::{
    repository::{test_support as repository_test, RepositoryStage},
    transaction::{
        test_support::{
            with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint, PrepareCounts,
        },
        PrepareFailPoint,
    },
};
use std::{cell::Cell, io, rc::Rc};
use std::{fs, path::PathBuf, sync::Arc};
mod boundaries;
mod duplication;
mod mutations;

type Session = EditSessionService<(), ()>;
const LATER: &str = "2026-09-09T02:03:04.005Z";
const CANARY: &str = "G6_private_credential C:/Users/hidden/value.json?token=secret";
fn context(snapshot: &crate::data::edit_session::EditSessionSnapshot) -> TemplateWriteContext<'_> {
    TemplateWriteContext {
        project: snapshot.project_fingerprint().unwrap(),
        session: snapshot.session_id().unwrap(),
    }
}
fn begin<P>(
    runtime: &ProjectRuntime,
    targets: &[ProjectRelativePath],
) -> EditSessionService<P, ()> {
    let mut session = EditSessionService::new(Arc::new(NoLockService::new()));
    session
        .begin_edit(runtime.project_fingerprint(), targets.to_vec())
        .unwrap();
    session
}
fn source(runtime: &mut ProjectRuntime, id: TemplateId) -> TemplateSource {
    let ready = runtime.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let loaded = repo.load_template(id).unwrap();
    TemplateSource {
        id,
        token: loaded.source().clone(),
        expected_revision: loaded.artifact().revision(),
    }
}
fn load(runtime: &mut ProjectRuntime, id: TemplateId) -> TemplateArtifact {
    let ready = runtime.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    repo.load_template(id).unwrap().into_artifact()
}
fn seed(runtime: &mut ProjectRuntime) -> TemplateId {
    let mut prepared = prepare_create_template(runtime, &input()).unwrap();
    let mut session: Session = begin(runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    let result = create_template(runtime, &mut session, context(&snapshot), &mut prepared);
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    session.end_edit().unwrap();
    prepared.template_id()
}
fn path(fixture: &Fixture, id: TemplateId) -> PathBuf {
    fixture
        .root
        .join(ArtifactSourceId::Template(id).path().unwrap().as_str())
}
fn journals(fixture: &Fixture) -> usize {
    let dir = fixture.root.join(".worldbuild/transactions");
    if dir.exists() {
        fs::read_dir(dir).unwrap().count()
    } else {
        0
    }
}
fn observe<T>(body: impl FnOnce() -> T) -> (T, PrepareCounts, usize) {
    let count = Rc::new(Cell::new(0));
    let counter = Rc::clone(&count);
    let (result, prepare) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        counter.set(counter.get() + 1);
                    }
                    Ok(())
                },
                body,
            )
        },
    );
    (result, prepare, count.get())
}
fn assert_no_io<T, E>(result: &WriteExecution<T, E>, prepare: PrepareCounts, commits: usize) {
    assert_eq!(
        (prepare.calls, prepare.allocations, commits),
        (0, 0, 0),
        "{result:?}"
    );
}

const TIME: &str = "2026-09-09T01:02:03.004Z";
struct Fixture {
    base: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g6-{}", uuid::Uuid::new_v4()));
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        Self { base, root }
    }
    fn runtime(&self) -> ProjectRuntime {
        let mut runtime = ProjectRuntime::acquire(&self.root, &self.base.join("locks")).unwrap();
        runtime.recover().unwrap();
        runtime
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.base) {
            eprintln!("G6 fixture cleanup: {:?}", e.kind());
        }
    }
}
fn input() -> CreateTemplateInput {
    CreateTemplateInput {
        name: " exact name ".into(),
        presentation_token: Some(" opaque ".into()),
        timestamp_utc: TIME.into(),
    }
}
#[test]
fn g6_first_create_without_namespace_reopens_through_real_repository() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    assert!(!fixture.root.join("templates").exists());
    let input = input();
    let mut prepared = prepare_create_template(&mut runtime, &input).unwrap();
    let id = prepared.template_id();
    let mut session = EditSessionService::<(), ()>::new(Arc::new(NoLockService::new()));
    session
        .begin_edit(
            runtime.project_fingerprint(),
            prepared.session_targets().to_vec(),
        )
        .unwrap();
    let snapshot = session.snapshot();
    let execution = create_template(
        &mut runtime,
        &mut session,
        TemplateWriteContext {
            project: snapshot.project_fingerprint().unwrap(),
            session: snapshot.session_id().unwrap(),
        },
        &mut prepared,
    );
    assert_eq!(
        execution.diagnostic().disk,
        DiskState::Committed,
        "{execution:?}"
    );
    assert_eq!(prepared.state(), CreationState::Committed);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    let ready = runtime.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let saved = repository.load_template(id).unwrap();
    assert_eq!(saved.artifact().template_id(), id);
    assert_eq!(saved.artifact().revision(), TemplateRevision::INITIAL);
    assert_eq!(
        saved.artifact().lifecycle(),
        artifact::TemplateLifecycle::Active
    );
    assert!(saved.artifact().name() == input.name);
    assert_eq!(saved.artifact().created_at_utc(), TIME);
    assert_eq!(saved.artifact().updated_at_utc(), TIME);
    assert!(!fixture.root.join("documents").exists());
    drop(repository);
    drop(ready);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
