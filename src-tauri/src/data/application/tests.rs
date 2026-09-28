mod boundaries;
mod diagnostics;
mod failures;
mod ownership;
use super::{diagnostics::*, write::*};
use crate::data::transaction::{
    test_support::{
        with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint, PrepareCounts,
    },
    PrepareFailPoint,
};
use crate::data::{
    artifact::{self, DocumentArtifact, DocumentId, TemplateArtifact, TemplateId},
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockService, NoLockService,
    },
    edit_session::{EditSessionService, EditSessionState},
    project_runtime::{ProjectRuntime, RuntimeState},
    repository::{ArtifactRepository, ArtifactSourceId, CanonicalWritePlan},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const TEMPLATE: &str = "11111111-1111-4111-8111-111111111111";
const DOCUMENT: &str = "22222222-2222-4222-8222-222222222222";
const TIME: &str = "2026-09-08T01:02:03.004Z";
const CANARY: &str = "G5_PRIVATE_CANARY credential=secret C:/Users/private/body.json?token=private";
fn template_id() -> TemplateId {
    TEMPLATE.parse().unwrap()
}
fn document_id() -> DocumentId {
    DOCUMENT.parse().unwrap()
}
fn tid() -> ArtifactSourceId {
    ArtifactSourceId::Template(template_id())
}
fn did() -> ArtifactSourceId {
    ArtifactSourceId::Document(document_id())
}
fn document() -> DocumentArtifact {
    artifact::decode_document(
        &serde_json::to_vec(&serde_json::json!({
            "artifactType":"document","schemaVersion":1,"documentId":DOCUMENT,"templateId":TEMPLATE,
            "templateRevision":7,"name":CANARY,"fieldValues":{},"orphanedFieldDefinitions":{},
            "createdAtUtc":TIME,"updatedAtUtc":TIME
        }))
        .unwrap(),
    )
    .unwrap()
}
fn template(name: &str) -> TemplateArtifact {
    artifact::decode_template(
        &serde_json::to_vec(&serde_json::json!({
            "artifactType":"template", "schemaVersion":1, "templateId":TEMPLATE, "revision":7,
            "name":name,"lifecycle":"active","presentation":{},"fieldOrder":[],"fields":{},
            "createdAtUtc":TIME,"updatedAtUtc":TIME
        }))
        .unwrap(),
    )
    .unwrap()
}
struct Fixture {
    base: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g5-{}", uuid::Uuid::new_v4()));
        let root = base.join("project");
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::create_dir(root.join("documents")).unwrap();
        Self { base, root }
    }
    fn runtime(&self) -> ProjectRuntime {
        let mut runtime = ProjectRuntime::acquire(&self.root, &self.base.join("locks")).unwrap();
        runtime.recover().unwrap();
        runtime
    }
    fn path(&self, id: ArtifactSourceId) -> PathBuf {
        self.root.join(id.path().unwrap().as_str())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.base) {
            eprintln!("G5 fixture cleanup: {:?}", error.kind());
        }
    }
}
type Session = EditSessionService<(), ()>;
fn session(runtime: &ProjectRuntime, targets: &ExactWriteTargets) -> Session {
    let mut session = Session::new(Arc::new(NoLockService::new()));
    session
        .begin_edit(
            runtime.project_fingerprint(),
            targets.session_targets().to_vec(),
        )
        .unwrap();
    session
}

fn run<I, T, E>(
    runtime: &mut ProjectRuntime,
    session: &mut Session,
    targets: &ExactWriteTargets,
    input: &mut I,
    build: &mut impl FnMut(
        &ArtifactRepository<'_, '_>,
        &mut I,
    ) -> Result<WriteDecision<T>, BuildError<E>>,
) -> WriteExecution<T, E> {
    let snapshot = session.snapshot();
    execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: snapshot.project_fingerprint().unwrap(),
            session: snapshot.session_id().unwrap(),
            targets,
        },
        input,
        build,
    )
}
fn replace(
    repo: &ArtifactRepository<'_, '_>,
    candidate: &mut TemplateArtifact,
) -> Result<WriteDecision<()>, BuildError<()>> {
    let source = repo.load_template(template_id())?;
    Ok(WriteDecision::Write {
        plan: CanonicalWritePlan::new().replace_template(candidate, source.source())?,
        value: (),
    })
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            result.extend(inventory(&entry.path()));
        } else {
            result.insert(entry.path(), fs::read(entry.path()).unwrap());
        }
    }
    result
}
fn journals(fixture: &Fixture) -> usize {
    let path = fixture.root.join(".worldbuild/transactions");
    if path.exists() {
        fs::read_dir(path).unwrap().count()
    } else {
        0
    }
}
fn observe<T>(body: impl FnOnce() -> T) -> (T, PrepareCounts, usize) {
    let commits = Rc::new(Cell::new(0));
    let counter = Rc::clone(&commits);
    let (value, prepares) = with_canonical_prepare_hooks(
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
    (value, prepares, commits.get())
}
struct Provider {
    inner: NoLockService,
    validations: AtomicUsize,
    fail_at: AtomicUsize,
}
impl Provider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: NoLockService::new(),
            validations: AtomicUsize::new(0),
            fail_at: AtomicUsize::new(0),
        })
    }
    fn reset(&self, fail_at: usize) {
        self.validations.store(0, Ordering::SeqCst);
        self.fail_at.store(fail_at, Ordering::SeqCst);
    }
}
impl LockService for Provider {
    fn provider_info(&self) -> LockProviderInfo {
        self.inner.provider_info()
    }
    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        self.inner.acquire(request)
    }
    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let call = self.validations.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_at.load(Ordering::SeqCst) {
            return Err(LockError::for_held(
                LockErrorCategory::LockLost,
                held.provider_kind(),
                LockOperation::Validate,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ));
        }
        self.inner.validate(held)
    }
    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        self.inner.release(held)
    }
}

#[test]
fn real_replace_connects_runtime_session_repository_and_commit() {
    let fixture = Fixture::new();
    let id = ArtifactSourceId::Template(template_id());
    let original = artifact::encode_template(&template("old")).unwrap();
    fs::write(fixture.path(id), &original).unwrap();
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([id]).unwrap();
    let mut session = session(&runtime, &targets);
    let snapshot = session.snapshot();
    let mut input = template(CANARY);
    let expected = artifact::encode_template(&input).unwrap();
    let result = execute_write_operation(
        &mut runtime,
        &mut session,
        WriteRequest {
            project: snapshot.project_fingerprint().unwrap(),
            session: snapshot.session_id().unwrap(),
            targets: &targets,
        },
        &mut input,
        &mut |repository: &ArtifactRepository<'_, '_>,
              candidate: &mut TemplateArtifact|
         -> Result<_, BuildError<()>> {
            let loaded = repository.load_template(template_id())?;
            let plan = CanonicalWritePlan::new().replace_template(candidate, loaded.source())?;
            Ok(WriteDecision::Write { plan, value: () })
        },
    );
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    assert!(!result.diagnostic().recovery_required);
    assert_eq!(fs::read(fixture.path(id)).unwrap(), expected);
    assert_eq!(artifact::decode_template(&expected).unwrap(), input);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    {
        let ready = runtime.ready().unwrap();
        let repo = ArtifactRepository::new(&ready).unwrap();
        assert_eq!(
            repo.load_template(template_id()).unwrap().artifact(),
            &input
        );
    }
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
