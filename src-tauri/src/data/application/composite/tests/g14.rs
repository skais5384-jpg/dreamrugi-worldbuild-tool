//! G14 child의 내부 G9 진입점. 운영 dirty-save capability 또는 durable receipt를 발급하지 않는다.
use super::*;

pub(crate) fn seed(base: &Path) {
    let root = base.join("project");
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::create_dir_all(root.join("documents")).unwrap();
    let (t, mut d) = fixture_raw();
    fs::write(
        root.join(ArtifactSourceId::Template(tid()).path().unwrap().as_str()),
        raw_bytes(&t),
    )
    .unwrap();
    fs::write(
        root.join(ArtifactSourceId::Document(did()).path().unwrap().as_str()),
        raw_bytes(&d),
    )
    .unwrap();
    d["documentId"] = json!(key(201));
    fs::write(
        root.join(
            ArtifactSourceId::Document(sentinel_id())
                .path()
                .unwrap()
                .as_str(),
        ),
        raw_bytes(&d),
    )
    .unwrap();
    let mut runtime = ProjectRuntime::acquire(&root, &base.join("locks")).unwrap();
    runtime.recover().unwrap();
    add_historical(&mut runtime);
    runtime.close().unwrap();
}

pub(crate) fn store(base: &Path) {
    let root = base.join("project");
    let mut runtime = ProjectRuntime::acquire(&root, &base.join("locks")).unwrap();
    runtime.recover().unwrap();
    let input = load_input(&mut runtime, required_intent(), vec![filled_edit()]);
    let request = CompositeWriteRequest::new(&input).unwrap();
    assert!(
        request.session_targets()
            == [
                ArtifactSourceId::Document(did()).path().unwrap(),
                ArtifactSourceId::Template(tid()).path().unwrap(),
            ],
        "actual canonical pair order"
    );
    let mut session = begin(&runtime, request.session_targets());
    let snapshot = session.snapshot();
    let result =
        update_template_and_save_document(&mut runtime, &mut session, context(&snapshot), &request);
    assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
    let (template, document) = candidate_pair(&result);
    assert!(
        fs::read(root.join(request.session_targets()[0].as_str())).unwrap() == document,
        "actual Document candidate saved"
    );
    assert!(
        fs::read(root.join(request.session_targets()[1].as_str())).unwrap() == template,
        "actual Template candidate saved"
    );
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

pub(crate) fn preserved_pair(base: &Path) {
    let root = base.join("project");
    preserved(
        &fs::read(root.join(ArtifactSourceId::Template(tid()).path().unwrap().as_str())).unwrap(),
        &fs::read(root.join(ArtifactSourceId::Document(did()).path().unwrap().as_str())).unwrap(),
    );
}
