use super::*;
use crate::data::{
    artifact::{
        encode_document, encode_template,
        template_mutation::{
            apply_template_mutation, TemplateMutationCommand, TemplateMutationErrorCategory,
        },
        ArtifactCodecErrorCategory, ArtifactCodecStage,
    },
    project_runtime::{ProjectRuntime, RuntimeState},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn Error>>;
pub(super) const TEMPLATE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const FIELD: &str = "22222222-2222-4222-8222-222222222222";
const ORPHAN: &str = "33333333-3333-4333-8333-333333333333";
const CANARY: &str = "credential=repository-secret C:/Users/private-user/body.json?token=private";
const TIME: &str = "2026-09-07T01:02:03.004Z";
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Fixture {
    base: PathBuf,
    pub(super) root: PathBuf,
    locks: PathBuf,
}
impl Fixture {
    pub(super) fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let base = std::env::temp_dir().join(format!(
                "worldbuild-g3-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&base) {
                Ok(()) => {
                    let root = base.join("private-user-project");
                    fs::create_dir(&root)?;
                    return Ok(Self {
                        locks: base.join("locks"),
                        root,
                        base,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("G3 fixture directory collision"))
    }
    fn ready_runtime(&self) -> Result<ProjectRuntime, Box<dyn Error>> {
        let mut runtime = ProjectRuntime::acquire(&self.root, &self.locks)?;
        assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
        runtime.recover()?;
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        Ok(runtime)
    }
    fn write(&self, kind: ArtifactType, name: &str, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(self.root.join(namespace(kind)))?;
        fs::write(self.root.join(namespace(kind)).join(name), bytes)
    }
    fn template(&self, id: &str, lifecycle: &str) -> TestResult {
        self.write(
            ArtifactType::Template,
            &format!("{id}.json"),
            &template_bytes(id, lifecycle),
        )?;
        Ok(())
    }
    fn document(&self, n: usize, template: &str) -> TestResult {
        self.write(
            ArtifactType::Document,
            &format!("{}.json", document_id(n)),
            &document_bytes(n, template),
        )?;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.base) {
            eprintln!("G3 fixture cleanup failed: {:?}", e.kind());
        }
    }
}
fn document_id(n: usize) -> DocumentId {
    format!("e1000000-0000-4000-8000-{n:012x}")
        .parse()
        .expect("fixture UUID v4")
}
fn template_id() -> TemplateId {
    TEMPLATE.parse().expect("fixture TemplateId")
}
fn template_value(id: &str, lifecycle: &str) -> Value {
    json!({"artifactType":"template","schemaVersion":1,"templateId":id,"revision":7,
        "name":CANARY,"lifecycle":lifecycle,"presentation":{},"fieldOrder":[],"fields":{},
        "createdAtUtc":TIME,"updatedAtUtc":TIME,"future":null})
}
pub(super) fn template_bytes(id: &str, lifecycle: &str) -> Vec<u8> {
    let input = serde_json::to_vec(&template_value(id, lifecycle)).expect("fixture JSON");
    let artifact =
        artifact::decode_template(&input).expect("fixture full Template codec admission");
    encode_template(&artifact).expect("fixture canonical Template bytes")
}
fn document_value(n: usize, template: &str) -> Value {
    json!({"artifactType":"document","schemaVersion":1,"documentId":document_id(n),"templateId":template,
        "templateRevision":1,"name":CANARY,"fieldValues":{},"orphanedFieldDefinitions":{},
        "createdAtUtc":TIME,"updatedAtUtc":TIME,"future":null})
}
pub(super) fn document_bytes(n: usize, template: &str) -> Vec<u8> {
    let input = serde_json::to_vec(&document_value(n, template)).expect("fixture JSON");
    let artifact =
        artifact::decode_document(&input).expect("fixture full Document codec admission");
    encode_document(&artifact).expect("fixture canonical Document bytes")
}
fn inventory(root: &Path) -> io::Result<BTreeMap<PathBuf, Option<Vec<u8>>>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !project_file::directory::is_reparse(&metadata) {
            result.insert(entry.path(), None);
            result.extend(inventory(&entry.path())?);
        } else {
            result.insert(entry.path(), Some(fs::read(entry.path())?));
        }
    }
    Ok(result)
}
fn assert_error(
    error: RepositoryError,
    category: RepositoryCategory,
    stage: RepositoryStage,
) -> diagnostics::RepositoryDiagnostic {
    let diagnostic = error.diagnostic();
    assert_eq!(diagnostic.category, category, "{error:?}");
    assert_eq!(diagnostic.stage, stage, "{error:?}");
    assert!(!diagnostic.next_action().is_empty());
    assert_redacted(&format!("{error:?} {error} {diagnostic:?}"));
    assert!(error.source().is_none());
    diagnostic
}
fn assert_redacted(text: &str) {
    for secret in [
        CANARY,
        "private-user",
        "repository-secret",
        "body.json",
        "token=private",
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
}
fn junction(link: &Path, target: &Path) -> io::Result<()> {
    let result = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(0x0800_0000)
        .output()?;
    if result.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("test junction creation failed"))
    }
}

#[test]
fn pending_and_blocked_never_enter_repository_io() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    let mut runtime = ProjectRuntime::acquire(&fixture.root, &fixture.locks)?;
    let mut bodies = 0;
    assert!(runtime
        .ready()
        .map(|ready| {
            bodies += 1;
            ArtifactRepository::new(&ready).map(|repo| repo.scan_templates())
        })
        .is_err());
    runtime.recover()?;
    let moved = fixture.base.join("holding");
    fs::rename(&fixture.root, &moved)?;
    assert!(runtime.ready().is_err());
    fs::rename(&moved, &fixture.root)?;
    assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
    assert!(runtime
        .ready()
        .map(|ready| {
            bodies += 1;
            ArtifactRepository::new(&ready).map(|repo| repo.scan_templates())
        })
        .is_err());
    assert_eq!(bodies, 0);
    runtime.recover()?;
    {
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        assert_eq!(repo.scan_templates()?.records().len(), 1);
        repo.load_template(template_id())?;
    }
    runtime.close()?;
    Ok(())
}

#[test]
fn canonical_reads_bind_id_kind_path_original_digest_and_revision() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    fixture.document(1, TEMPLATE)?;
    let mut runtime = fixture.ready_runtime()?;
    let before = inventory(&fixture.root)?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let template = repo.load_template(template_id())?;
    let document = repo.load_document(document_id(1))?;
    for (source, bytes, id, revision) in [
        (
            template.source(),
            template_bytes(TEMPLATE, "active"),
            ArtifactSourceId::Template(template_id()),
            SourceRevision::Template(template.artifact().revision()),
        ),
        (
            document.source(),
            document_bytes(1, TEMPLATE),
            ArtifactSourceId::Document(document_id(1)),
            SourceRevision::DocumentTemplate(document.artifact().template_revision()),
        ),
    ] {
        assert_eq!(source.id(), id);
        assert_eq!(source.path(), &id.path()?);
        assert_eq!(source.byte_length(), bytes.len());
        assert_eq!(source.sha256(), &<[u8; 32]>::from(Sha256::digest(&bytes)));
        assert_eq!(source.schema().get(), 1);
        assert_eq!(source.revision(), revision);
        assert!(repo.reread_matches(source)?);
        let (matches, counts) =
            test_support::scoped(None, false, || repo.reread_bytes_match(source));
        assert!(matches?);
        assert_eq!((counts.read_calls, counts.bytes_read), (1, bytes.len()));
        assert_eq!((counts.decoded, counts.hashes), (0, 1));
    }
    assert_eq!(inventory(&fixture.root)?, before);
    Ok(())
}

#[test]
fn tombstones_remain_in_sorted_complete_template_inventory() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(OTHER, "deleted")?;
    fixture.template(TEMPLATE, "active")?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let scan = repo.scan_templates()?;
    assert_eq!(scan.records().len(), 2);
    assert_eq!(
        scan.records()[0].source().id(),
        ArtifactSourceId::Template(template_id())
    );
    assert_eq!(scan.records()[1].lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(
        repo.load_template(OTHER.parse()?)?.artifact().lifecycle(),
        TemplateLifecycle::Deleted
    );
    assert_redacted(&format!("{scan:?} {:?}", scan.records()));
    Ok(())
}

#[test]
fn missing_and_empty_namespaces_are_read_only_but_loader_is_not_found() -> TestResult {
    for present in [false, true] {
        let fixture = Fixture::new()?;
        if present {
            fs::create_dir(fixture.root.join("templates"))?;
            fs::create_dir(fixture.root.join("documents"))?;
        }
        let mut runtime = fixture.ready_runtime()?;
        let before = inventory(&fixture.root)?;
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        assert_eq!(repo.scan_templates()?.records().len(), 0);
        assert_eq!(repo.scan_documents()?.len(), 0);
        let stage = if present {
            RepositoryStage::EntryMetadata
        } else {
            RepositoryStage::Namespace
        };
        assert_error(
            repo.load_template(template_id()).unwrap_err(),
            RepositoryCategory::NotFound,
            stage,
        );
        assert_error(
            repo.load_document(document_id(1)).unwrap_err(),
            RepositoryCategory::NotFound,
            stage,
        );
        assert_eq!(inventory(&fixture.root)?, before);
    }
    Ok(())
}

#[test]
fn root_disappearance_and_file_replacement_are_not_empty_scans() -> TestResult {
    for replace_file in [false, true] {
        let fixture = Fixture::new()?;
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        fs::rename(&fixture.root, fixture.base.join("holding"))?;
        if replace_file {
            fs::write(&fixture.root, CANARY)?;
        }
        let error = ArtifactRepository::new(&ready).unwrap_err();
        assert_error(
            error,
            RepositoryCategory::RootUnavailable,
            RepositoryStage::Root,
        );
    }
    Ok(())
}

#[test]
fn file_namespace_and_empty_external_internal_dangling_junctions_are_rejected() -> TestResult {
    for mode in ["file", "external", "internal", "dangling"] {
        for kind in [ArtifactType::Template, ArtifactType::Document] {
            let fixture = Fixture::new()?;
            let target = if mode == "internal" {
                fixture.root.join("elsewhere")
            } else {
                fixture.base.join("outside")
            };
            fs::create_dir(&target)?;
            let path = fixture.root.join(namespace(kind));
            if mode == "file" {
                fs::write(&path, CANARY)?;
            } else {
                junction(&path, &target)?;
                if mode == "dangling" {
                    fs::remove_dir(&target)?;
                }
            }
            let mut runtime = fixture.ready_runtime()?;
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            let result = match kind {
                ArtifactType::DocumentLayout => repo.load_layout().map(|_| ()),
                ArtifactType::Template => repo.scan_templates().map(|_| ()),
                ArtifactType::Document => repo.scan_documents().map(|_| ()),
            };
            assert_error(
                result.unwrap_err(),
                RepositoryCategory::NamespaceUnavailable,
                RepositoryStage::Namespace,
            );
        }
    }
    Ok(())
}

#[test]
fn root_junction_alias_after_ready_is_rejected() -> TestResult {
    let fixture = Fixture::new()?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let moved = fixture.base.join("moved");
    fs::rename(&fixture.root, &moved)?;
    junction(&fixture.root, &moved)?;
    assert_error(
        ArtifactRepository::new(&ready).unwrap_err(),
        RepositoryCategory::RootUnavailable,
        RepositoryStage::Root,
    );
    Ok(())
}

#[test]
fn namespace_guard_blocks_real_rename_and_delete_during_empty_scan() -> TestResult {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.root.join("documents"))?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let root = fixture.root.clone();
    let directory = root.join("documents");
    let destination = fixture.base.join("moved");
    let (result, counts) = test_support::scoped(
        Some((
            RepositoryStage::ReadDirectory,
            0,
            Box::new(move |_| {
                assert_eq!(
                    fs::rename(&directory, &destination)
                        .unwrap_err()
                        .raw_os_error(),
                    Some(32)
                );
                assert_eq!(
                    fs::remove_dir(&directory).unwrap_err().raw_os_error(),
                    Some(32)
                );
                assert_eq!(
                    fs::rename(&root, &destination).unwrap_err().raw_os_error(),
                    Some(32)
                );
                Ok(())
            }),
        )),
        false,
        || repo.scan_documents(),
    );
    assert_eq!(result?.len(), 0);
    assert_eq!(counts.hooks, 1);
    Ok(())
}

#[test]
fn source_tokens_distinguish_same_revision_length_and_lexical_bytes() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    fixture.document(1, TEMPLATE)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let a = repo.load_template(template_id())?;
    let bytes = template_bytes(TEMPLATE, "active");
    let changed =
        String::from_utf8(bytes.clone())?.replace("repository-secret", "repository-change");
    assert_eq!(changed.len(), bytes.len());
    fixture.write(
        ArtifactType::Template,
        &format!("{TEMPLATE}.json"),
        changed.as_bytes(),
    )?;
    let b = repo.load_template(template_id())?;
    assert_eq!(a.artifact().revision(), b.artifact().revision());
    assert_ne!(a.source(), b.source());
    assert!(!repo.reread_matches(a.source())?);
    let compact = serde_json::to_vec(&template_value(TEMPLATE, "active"))?;
    fixture.write(
        ArtifactType::Template,
        &format!("{TEMPLATE}.json"),
        &compact,
    )?;
    let c = repo.load_template(template_id())?;
    assert_eq!(a.artifact(), c.artifact());
    assert_ne!(a.source(), c.source());
    let mut reordered = String::from_utf8(compact)?.trim_end_matches('}').to_owned();
    // 알려진 member의 순서만 바뀐 동일 모델도 원본 precondition은 달라야 한다.
    let prefix = "\"artifactType\":\"template\",";
    reordered = reordered.replacen(prefix, "", 1);
    reordered.push_str(",\"artifactType\":\"template\"}");
    fixture.write(
        ArtifactType::Template,
        &format!("{TEMPLATE}.json"),
        reordered.as_bytes(),
    )?;
    let d = repo.load_template(template_id())?;
    assert_eq!(c.artifact(), d.artifact());
    assert_ne!(c.source(), d.source());
    let old_doc = repo.load_document(document_id(1))?;
    let new_doc = String::from_utf8(document_bytes(1, TEMPLATE))?
        .replace("repository-secret", "repository-change");
    fixture.write(
        ArtifactType::Document,
        &format!("{}.json", document_id(1)),
        new_doc.as_bytes(),
    )?;
    let new_doc = repo.load_document(document_id(1))?;
    assert_eq!(
        old_doc.artifact().template_revision(),
        new_doc.artifact().template_revision()
    );
    assert!(!repo.reread_matches(old_doc.source())?);
    Ok(())
}

#[test]
fn project_identity_rejects_foreign_source_with_same_ids_and_bytes() -> TestResult {
    let a = Fixture::new()?;
    let b = Fixture::new()?;
    a.template(TEMPLATE, "active")?;
    b.template(TEMPLATE, "active")?;
    let mut ra = a.ready_runtime()?;
    let mut rb = b.ready_runtime()?;
    let aa = ra.ready()?;
    let ab = rb.ready()?;
    let pa = ArtifactRepository::new(&aa)?;
    let pb = ArtifactRepository::new(&ab)?;
    let source_a = pa.load_template(template_id())?;
    let source_b = pb.load_template(template_id())?;
    assert_eq!(source_a.source().sha256(), source_b.source().sha256());
    assert_ne!(source_a.source(), source_b.source());
    assert_error(
        pb.reread_matches(source_a.source()).unwrap_err(),
        RepositoryCategory::SourceMismatch,
        RepositoryStage::BindSource,
    );
    assert_eq!(
        pb.scan_template_references(template_id())?
            .source()
            .source(),
        source_b.source()
    );
    Ok(())
}

#[test]
fn reopened_repository_keeps_identity_but_replaced_root_does_not() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    let mut runtime = fixture.ready_runtime()?;
    let token = {
        let ready = runtime.ready()?;
        ArtifactRepository::new(&ready)?
            .load_template(template_id())?
            .source
    };
    {
        let ready = runtime.ready()?;
        assert!(ArtifactRepository::new(&ready)?.reread_matches(&token)?);
    }
    fs::rename(&fixture.root, fixture.base.join("holding"))?;
    fs::create_dir(&fixture.root)?;
    fixture.template(TEMPLATE, "active")?;
    runtime.recover()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    assert_error(
        repo.reread_matches(&token).unwrap_err(),
        RepositoryCategory::SourceMismatch,
        RepositoryStage::BindSource,
    );
    Ok(())
}

#[test]
fn lossless_unknown_lexemes_and_historical_unset_orphans_are_unchanged() -> TestResult {
    let fixture = Fixture::new()?;
    let lexemes = r#"{"a":1E100,"b":1e100,"c":-0,"d":0.12345678901234567890123456789,"nested":[{"x":1E100}]}"#;
    let mut template = template_value(TEMPLATE, "active");
    template["fieldOrder"] = json!([FIELD]);
    template["fields"][FIELD] = json!({"configuration":{"kind":"singleLineText"},"defaultValue":{"kind":"text","value":"default must not be added"},
        "initialDefaultValue":{"kind":"unset"},"introducedRevision":4,"kind":"singleLineText","label":CANARY,"lifecycle":"active","presentation":{},"required":true});
    let template_input = serde_json::to_string(&template)?
        .replace("\"future\":null", &format!("\"future\":{lexemes}"));
    artifact::decode_template(template_input.as_bytes())?;
    fixture.write(
        ArtifactType::Template,
        &format!("{TEMPLATE}.json"),
        template_input.as_bytes(),
    )?;
    for n in 1..=3 {
        let mut doc = document_value(n, TEMPLATE);
        if n == 2 {
            doc["fieldValues"][FIELD] = json!({"kind":"unset"});
        }
        if n == 3 {
            doc["fieldValues"][ORPHAN] = json!({"kind":"text","value":CANARY});
            doc["orphanedFieldDefinitions"][ORPHAN] =
                json!({"kind":"singleLineText","label":CANARY,"options":{},"future":null});
        }
        let input = serde_json::to_string(&doc)?
            .replace("\"future\":null", &format!("\"future\":{lexemes}"));
        artifact::decode_document(input.as_bytes())?;
        fixture.write(
            ArtifactType::Document,
            &format!("{}.json", document_id(n)),
            input.as_bytes(),
        )?;
    }
    let mut runtime = fixture.ready_runtime()?;
    let before = inventory(&fixture.root)?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let template = repo.load_template(template_id())?;
    for n in 1..=3 {
        let doc = repo.load_document(document_id(n))?;
        assert_eq!(doc.artifact().template_revision().get(), 1);
        let output = String::from_utf8(encode_document(doc.artifact())?)?;
        for value in ["1E100", "1e100", "-0", "0.12345678901234567890123456789"] {
            assert!(output.contains(value));
        }
        assert_eq!(doc.artifact().field_values().len(), usize::from(n != 1));
    }
    assert!(String::from_utf8(encode_template(template.artifact())?)?.contains("1E100"));
    let diagnostic = repo
        .assess_template_references(template_id())
        .unwrap_err()
        .diagnostic();
    assert_eq!(diagnostic.reference.unwrap().reference_count(), Some(3));
    assert_eq!(repo.scan_documents()?.len(), 3);
    assert_eq!(inventory(&fixture.root)?, before);
    Ok(())
}

#[test]
fn read_uses_verified_handle_while_real_target_replacement_is_blocked() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.document(1, TEMPLATE)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let (result, counts) = test_support::scoped(
        Some((
            RepositoryStage::ReadFile,
            0,
            Box::new(|path| {
                assert_eq!(
                    fs::rename(path, path.with_extension("held"))
                        .unwrap_err()
                        .raw_os_error(),
                    Some(32)
                );
                assert_eq!(
                    fs::write(path, b"replacement").unwrap_err().raw_os_error(),
                    Some(32)
                );
                Ok(())
            }),
        )),
        false,
        || repo.load_document(document_id(1)),
    );
    assert_eq!(
        result?.source().sha256(),
        &<[u8; 32]>::from(Sha256::digest(document_bytes(1, TEMPLATE)))
    );
    assert_eq!(counts.decoded, 1);
    assert_eq!(counts.hooks, 1);
    Ok(())
}

#[test]
fn large_derived_scan_reads_in_order_and_rejects_a_later_changed_source() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    for n in 1..=128 {
        fixture.document(n, TEMPLATE)?;
    }
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let ids = match repo.scan_documents_with(|document| Ok::<_, ()>(document.document_id())) {
        Ok(ids) => ids,
        Err(_) => panic!("valid large scan"),
    };
    assert_eq!(ids, (1..=128).map(document_id).collect::<Vec<_>>());

    let mut seen = 0;
    let result = repo.scan_documents_with(|_| {
        seen += 1;
        if seen == 1 {
            fs::write(
                fixture
                    .root
                    .join(format!("documents/{}.json", document_id(128))),
                b"invalid later document",
            )
            .unwrap();
        }
        if seen <= 64 {
            // Exercise the slow-namespace branch deterministically after the
            // prefix; the changed source must still reject the full result.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        Ok::<_, ()>(())
    });
    match result {
        Err(DocumentScanVisitError::Repository(error)) => {
            assert_error(
                error,
                RepositoryCategory::CodecRejected,
                RepositoryStage::Decode,
            );
        }
        _ => panic!("later changed source must reject the complete scan"),
    }
    assert!((64..128).contains(&seen));
    Ok(())
}

#[test]
fn svn_text_conflict_sidecars_leave_canonical_document_scannable() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.document(1, TEMPLATE)?;
    let name = format!("{}.json", document_id(1));
    for sidecar in [format!("{name}.mine"), format!("{name}.r12")] {
        fixture.write(ArtifactType::Document, &sidecar, b"SVN conflict source")?;
    }
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    assert_eq!(repo.scan_documents()?.len(), 1);
    fixture.write(ArtifactType::Document, &format!("{name}.bak"), b"not SVN")?;
    assert_error(
        repo.scan_documents().unwrap_err(),
        RepositoryCategory::InvalidEntry,
        RepositoryStage::EntryName,
    );
    Ok(())
}

#[test]
fn invalid_names_and_non_unicode_entries_are_never_skipped() -> TestResult {
    for name in [
        "backup.json".into(),
        format!("{}.JSON", document_id(1)),
        format!("{}.json.bak", document_id(1)),
        document_id(1).to_string().to_uppercase() + ".json",
        ".hidden".into(),
        "e1000000000040008000000000000001.json".into(),
    ] {
        let fixture = Fixture::new()?;
        fixture.write(ArtifactType::Document, &name, &document_bytes(1, TEMPLATE))?;
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        assert_error(
            repo.scan_documents().unwrap_err(),
            RepositoryCategory::InvalidEntry,
            RepositoryStage::EntryName,
        );
    }
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.root.join("documents"))?;
    let non_unicode =
        std::ffi::OsString::from_wide(&[0xd800, 0x002e, 0x006a, 0x0073, 0x006f, 0x006e]);
    fs::write(
        fixture.root.join("documents").join(non_unicode),
        document_bytes(1, TEMPLATE),
    )?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    assert_error(
        repo.scan_documents().unwrap_err(),
        RepositoryCategory::InvalidEntry,
        RepositoryStage::EntryName,
    );
    Ok(())
}

#[test]
fn canonical_entry_directories_and_reparse_points_are_rejected() -> TestResult {
    for link in [false, true] {
        let fixture = Fixture::new()?;
        let path = fixture
            .root
            .join("documents")
            .join(format!("{}.json", document_id(1)));
        fs::create_dir(fixture.root.join("documents"))?;
        if link {
            let outside = fixture.base.join("outside");
            fs::create_dir(&outside)?;
            junction(&path, &outside)?;
        } else {
            fs::create_dir(&path)?;
        }
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        assert_error(
            repo.scan_documents().unwrap_err(),
            RepositoryCategory::InvalidEntry,
            RepositoryStage::EntryMetadata,
        );
        assert_error(
            repo.load_document(document_id(1)).unwrap_err(),
            RepositoryCategory::InvalidEntry,
            RepositoryStage::EntryMetadata,
        );
    }
    Ok(())
}

#[test]
fn kind_and_body_id_mismatches_are_rejected_for_load_and_scan() -> TestResult {
    for kind in [ArtifactType::Template, ArtifactType::Document] {
        for wrong_kind in [false, true] {
            let fixture = Fixture::new()?;
            let (name, bytes) = match kind {
                ArtifactType::DocumentLayout => (
                    "document-layout.json".into(),
                    artifact::encode_layout(&artifact::layout::DocumentLayout::flat([])).unwrap(),
                ),
                ArtifactType::Template => (
                    format!("{TEMPLATE}.json"),
                    if wrong_kind {
                        document_bytes(1, TEMPLATE)
                    } else {
                        template_bytes(OTHER, "active")
                    },
                ),
                ArtifactType::Document => (
                    format!("{}.json", document_id(1)),
                    if wrong_kind {
                        template_bytes(TEMPLATE, "active")
                    } else {
                        document_bytes(2, TEMPLATE)
                    },
                ),
            };
            fixture.write(kind, &name, &bytes)?;
            let mut runtime = fixture.ready_runtime()?;
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            for scan in [false, true] {
                let result = match (kind, scan) {
                    (ArtifactType::DocumentLayout, _) => repo.load_layout().map(|_| ()),
                    (ArtifactType::Template, false) => {
                        repo.load_template(template_id()).map(|_| ())
                    }
                    (ArtifactType::Template, true) => repo.scan_templates().map(|_| ()),
                    (ArtifactType::Document, false) => {
                        repo.load_document(document_id(1)).map(|_| ())
                    }
                    (ArtifactType::Document, true) => repo.scan_documents().map(|_| ()),
                };
                let d = if wrong_kind {
                    assert_error(
                        result.unwrap_err(),
                        RepositoryCategory::CodecRejected,
                        RepositoryStage::Decode,
                    )
                } else {
                    assert_error(
                        result.unwrap_err(),
                        RepositoryCategory::IdMismatch,
                        RepositoryStage::BindSource,
                    )
                };
                if wrong_kind {
                    assert_eq!(
                        d.codec.unwrap().category(),
                        ArtifactCodecErrorCategory::UnexpectedArtifactType
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn corrupt_duplicate_future_and_invalid_header_preserve_codec_stage() -> TestResult {
    let valid = String::from_utf8(document_bytes(1, TEMPLATE))?;
    for (input, category, stage) in [
        (
            "{".to_owned(),
            ArtifactCodecErrorCategory::MalformedJson,
            ArtifactCodecStage::StrictJson,
        ),
        (
            valid.replacen('{', "{\"schemaVersion\":1,", 1),
            ArtifactCodecErrorCategory::DuplicateJsonKey,
            ArtifactCodecStage::StrictJson,
        ),
        (
            valid.replace("\"schemaVersion\": 1", "\"schemaVersion\": 8"),
            ArtifactCodecErrorCategory::UnsupportedFuture,
            ArtifactCodecStage::Compatibility,
        ),
        (
            valid.replace("\"schemaVersion\": 1", "\"schemaVersion\": 0"),
            ArtifactCodecErrorCategory::InvalidSchemaVersion,
            ArtifactCodecStage::Header,
        ),
        (
            valid.replace("\"templateRevision\": 1", "\"templateRevision\": 0"),
            ArtifactCodecErrorCategory::InvalidStructure,
            ArtifactCodecStage::Deserialize,
        ),
    ] {
        let fixture = Fixture::new()?;
        fixture.write(
            ArtifactType::Document,
            &format!("{}.json", document_id(1)),
            input.as_bytes(),
        )?;
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let d = assert_error(
            repo.scan_documents().unwrap_err(),
            RepositoryCategory::CodecRejected,
            RepositoryStage::Decode,
        );
        let codec = d.codec.unwrap();
        assert_eq!(codec.category(), category);
        assert_eq!(codec.stage(), stage);
    }
    Ok(())
}

#[test]
fn scoped_directory_iterator_metadata_open_and_read_failures_have_no_complete_result() -> TestResult
{
    for (stage, skip, category, decoded) in [
        (
            RepositoryStage::ReadDirectory,
            0,
            RepositoryCategory::IterationFailed,
            0,
        ),
        (
            RepositoryStage::Iterate,
            1,
            RepositoryCategory::IterationFailed,
            0,
        ),
        (
            RepositoryStage::EntryMetadata,
            1,
            RepositoryCategory::MetadataFailed,
            1,
        ),
        (
            RepositoryStage::OpenFile,
            1,
            RepositoryCategory::OpenFailed,
            1,
        ),
        (
            RepositoryStage::ReadFile,
            1,
            RepositoryCategory::ReadFailed,
            1,
        ),
    ] {
        let fixture = Fixture::new()?;
        fixture.document(1, TEMPLATE)?;
        fixture.document(2, TEMPLATE)?;
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        assert_eq!(repo.scan_documents()?.len(), 2);
        let before = inventory(&fixture.root)?;
        let (result, counts) = test_support::scoped(
            Some((
                stage,
                skip,
                Box::new(|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, CANARY))),
            )),
            false,
            || repo.scan_documents(),
        );
        let d = assert_error(result.unwrap_err(), category, stage);
        assert_eq!(d.io_kind, Some(io::ErrorKind::PermissionDenied));
        assert_eq!(counts.hooks, 1);
        assert_eq!(counts.decoded, decoded);
        if stage == RepositoryStage::Iterate {
            assert_eq!(counts.iterations, 2);
        }
        assert_eq!(inventory(&fixture.root)?, before);
        assert_eq!(repo.scan_documents()?.len(), 2);
    }
    Ok(())
}

#[test]
fn observed_entry_disappearance_is_incomplete_not_not_found() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.document(1, TEMPLATE)?;
    fixture.document(2, TEMPLATE)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let (result, counts) = test_support::scoped(
        Some((
            RepositoryStage::EntryMetadata,
            1,
            Box::new(|path| fs::remove_file(path)),
        )),
        false,
        || repo.scan_documents(),
    );
    let d = assert_error(
        result.unwrap_err(),
        RepositoryCategory::MetadataFailed,
        RepositoryStage::EntryMetadata,
    );
    assert_eq!(d.io_kind, Some(io::ErrorKind::NotFound));
    assert_eq!(counts.decoded, 1);
    Ok(())
}

#[test]
fn repeated_real_entry_is_rejected_and_physical_duplicate_body_fails_id_binding() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.document(1, TEMPLATE)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let (result, counts) = test_support::scoped(None, true, || repo.scan_documents());
    assert_error(
        result.unwrap_err(),
        RepositoryCategory::DuplicateSource,
        RepositoryStage::BindSource,
    );
    assert_eq!(counts.iterations, 1);
    assert_eq!(repo.scan_documents()?.len(), 1);
    fixture.write(
        ArtifactType::Document,
        &format!("{}.json", document_id(2)),
        &document_bytes(1, TEMPLATE),
    )?;
    assert_error(
        repo.scan_documents().unwrap_err(),
        RepositoryCategory::IdMismatch,
        RepositoryStage::BindSource,
    );
    Ok(())
}

#[test]
fn reference_after_unrelated_document_and_no_reference_pure_tombstone() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    fixture.document(1, OTHER)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let source = repo.load_template(template_id())?;
    let before = inventory(&fixture.root)?;
    let assessment = repo.assess_template_references(template_id())?;
    assert_eq!(assessment.template_id(), template_id());
    assert_eq!(assessment.source_revision(), source.artifact().revision());
    let candidate = apply_template_mutation(
        source.artifact(),
        source.artifact().revision(),
        "2026-09-07T02:02:03.004Z",
        TemplateMutationCommand::tombstone_template(assessment),
    )?
    .into_changed()
    .unwrap();
    assert_eq!(candidate.lifecycle(), TemplateLifecycle::Deleted);
    assert_eq!(inventory(&fixture.root)?, before);
    fixture.document(2, TEMPLATE)?;
    let (result, counts) = test_support::scoped(None, false, || {
        repo.assess_template_references(template_id())
    });
    let d = assert_error(
        result.unwrap_err(),
        RepositoryCategory::ReferenceBlocked,
        RepositoryStage::References,
    );
    assert_eq!(d.reference.unwrap().reference_count(), Some(1));
    assert_eq!(counts.decoded, 3);
    Ok(())
}

#[test]
fn late_corruption_future_and_semantic_error_override_reference_observations() -> TestResult {
    for references in [false, true] {
        for failure in ["corrupt", "future", "semantic"] {
            let fixture = Fixture::new()?;
            fixture.template(TEMPLATE, "active")?;
            fixture.document(1, if references { TEMPLATE } else { OTHER })?;
            let mut bad = document_value(2, OTHER);
            if failure == "future" {
                bad["schemaVersion"] =
                    json!(crate::data::artifact::DOCUMENT_SCHEMA_VERSION.get() + 1);
            }
            if failure == "semantic" {
                bad["fieldValues"][FIELD] = json!({"kind":"number","value":"not-a-number"});
            }
            let bytes = if failure == "corrupt" {
                b"{".to_vec()
            } else {
                serde_json::to_vec(&bad)?
            };
            fixture.write(
                ArtifactType::Document,
                &format!("{}.json", document_id(2)),
                &bytes,
            )?;
            let mut runtime = fixture.ready_runtime()?;
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            let (result, counts) = test_support::scoped(None, false, || {
                repo.assess_template_references(template_id())
            });
            let d = assert_error(
                result.unwrap_err(),
                RepositoryCategory::CodecRejected,
                RepositoryStage::Decode,
            );
            assert_eq!(counts.decoded, 2);
            assert!(d.reference.is_none());
            let expected = match failure {
                "future" => ArtifactCodecErrorCategory::UnsupportedFuture,
                "semantic" => ArtifactCodecErrorCategory::ArtifactSemanticValidationFailure,
                _ => ArtifactCodecErrorCategory::MalformedJson,
            };
            assert_eq!(d.codec.unwrap().category(), expected);
        }
    }
    Ok(())
}

#[test]
fn reference_limit_does_not_skip_later_validation_or_read_errors() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    for n in 1..=1025 {
        fixture.document(n, TEMPLATE)?;
    }
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let (result, counts) = test_support::scoped(None, false, || {
        repo.assess_template_references(template_id())
    });
    let reference = result.unwrap_err().diagnostic().reference.unwrap();
    assert_eq!(reference.reference_count(), Some(1024));
    assert!(reference.reference_count_truncated());
    assert_eq!(counts.decoded, 1026);
    fixture.document(1026, OTHER)?;
    let (result, counts) = test_support::scoped(
        Some((
            RepositoryStage::ReadFile,
            1026,
            Box::new(|_| Err(io::Error::from_raw_os_error(5))),
        )),
        false,
        || repo.assess_template_references(template_id()),
    );
    let d = assert_error(
        result.unwrap_err(),
        RepositoryCategory::ReadFailed,
        RepositoryStage::ReadFile,
    );
    assert_eq!(d.os_code, Some(5));
    assert_eq!(counts.decoded, 1026);
    fixture.write(
        ArtifactType::Document,
        &format!("{}.json", document_id(1026)),
        b"{",
    )?;
    let (result, counts) = test_support::scoped(None, false, || {
        repo.assess_template_references(template_id())
    });
    assert_error(
        result.unwrap_err(),
        RepositoryCategory::CodecRejected,
        RepositoryStage::Decode,
    );
    assert_eq!(counts.decoded, 1026);
    Ok(())
}

#[test]
fn repository_assessment_still_rejects_wrong_target_and_stale_revision() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    fixture.template(OTHER, "active")?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let other = repo.load_template(OTHER.parse()?)?;
    let mismatch = apply_template_mutation(
        other.artifact(),
        other.artifact().revision(),
        TIME,
        TemplateMutationCommand::tombstone_template(
            repo.assess_template_references(template_id())?,
        ),
    )
    .unwrap_err();
    assert_eq!(
        mismatch.category(),
        TemplateMutationErrorCategory::ReferenceAssessmentMismatch
    );
    let assessment = repo.assess_template_references(template_id())?;
    let mut new_template = template_value(TEMPLATE, "active");
    new_template["revision"] = json!(8);
    fixture.write(
        ArtifactType::Template,
        &format!("{TEMPLATE}.json"),
        &serde_json::to_vec(&new_template)?,
    )?;
    let source = repo.load_template(template_id())?;
    let stale = apply_template_mutation(
        source.artifact(),
        source.artifact().revision(),
        TIME,
        TemplateMutationCommand::tombstone_template(assessment),
    )
    .unwrap_err();
    assert_eq!(
        stale.category(),
        TemplateMutationErrorCategory::ReferenceAssessmentStale
    );
    Ok(())
}

#[test]
fn all_loaded_source_and_scan_debugs_are_redacted() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.template(TEMPLATE, "active")?;
    fixture.document(1, OTHER)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let template = repo.load_template(template_id())?;
    let document = repo.load_document(document_id(1))?;
    let t = repo.scan_templates()?;
    let d = repo.scan_documents()?;
    let r = repo.scan_template_references(template_id())?;
    let text = format!(
        "{repo:?} {template:?} {document:?} {:?} {:?} {t:?} {d:?} {r:?}",
        template.source(),
        document.source()
    );
    assert_redacted(&text);
    assert!(!text.contains(&template.source().path().to_string()));
    assert!(!text.contains(&format!(
        "{:x}",
        Sha256::digest(template_bytes(TEMPLATE, "active"))
    )));
    assert_eq!(d.sources().count(), 1);
    Ok(())
}

#[test]
fn directory_sharing_failures_are_not_reported_as_empty_namespaces() -> TestResult {
    use std::os::windows::fs::OpenOptionsExt;
    for root in [false, true] {
        let fixture = Fixture::new()?;
        fs::create_dir(fixture.root.join("documents"))?;
        let mut runtime = fixture.ready_runtime()?;
        let ready = runtime.ready()?;
        let target = if root {
            fixture.root.clone()
        } else {
            fixture.root.join("documents")
        };
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .custom_flags(0x0200_0000)
            .open(target)?;
        let error = if root {
            ArtifactRepository::new(&ready).unwrap_err()
        } else {
            ArtifactRepository::new(&ready)?
                .scan_documents()
                .unwrap_err()
        };
        let diagnostic = assert_error(
            error,
            if root {
                RepositoryCategory::RootUnavailable
            } else {
                RepositoryCategory::NamespaceUnavailable
            },
            if root {
                RepositoryStage::Root
            } else {
                RepositoryStage::Namespace
            },
        );
        assert_eq!(diagnostic.os_code, Some(32));
        drop(held);
        assert_eq!(ArtifactRepository::new(&ready)?.scan_documents()?.len(), 0);
    }
    Ok(())
}

#[test]
fn target_changed_to_junction_between_metadata_and_open_is_rejected() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.document(1, TEMPLATE)?;
    let outside = fixture.base.join("outside");
    fs::create_dir(&outside)?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let (result, counts) = test_support::scoped(
        Some((
            RepositoryStage::OpenFile,
            0,
            Box::new(move |path| {
                fs::remove_file(path)?;
                junction(path, &outside)
            }),
        )),
        false,
        || repo.load_document(document_id(1)),
    );
    let diagnostic = assert_error(
        result.unwrap_err(),
        RepositoryCategory::OpenFailed,
        RepositoryStage::OpenFile,
    );
    assert_eq!(
        diagnostic.source_id,
        Some(ArtifactSourceId::Document(document_id(1)))
    );
    assert_eq!(counts.hooks, 1);
    assert_eq!(counts.decoded, 0);
    Ok(())
}

#[test]
fn complete_document_order_is_deterministic_and_other_project_areas_are_ignored() -> TestResult {
    let fixture = Fixture::new()?;
    for n in [3, 1, 2] {
        fixture.document(n, OTHER)?;
    }
    for dir in ["assets", ".git", "other"] {
        fs::create_dir(fixture.root.join(dir))?;
        fs::write(fixture.root.join(dir).join("invalid.json"), b"{")?;
    }
    let mut runtime = fixture.ready_runtime()?;
    let before = inventory(&fixture.root)?;
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let scan = repo.scan_documents()?;
    assert_eq!(
        scan.sources().map(SourceToken::id).collect::<Vec<_>>(),
        (1..=3)
            .map(|n| ArtifactSourceId::Document(document_id(n)))
            .collect::<Vec<_>>()
    );
    assert_eq!(inventory(&fixture.root)?, before);
    Ok(())
}

#[test]
fn all_corrupt_content_stays_discoverable_without_granting_a_complete_scan() -> TestResult {
    let fixture = Fixture::new()?;
    let mut runtime = fixture.ready_runtime()?;
    let ready = runtime.ready()?;
    let repository = ArtifactRepository::new(&ready)?;
    let template_id = TEMPLATE.parse()?;
    fs::create_dir(fixture.root.join("templates"))?;
    fs::write(
        fixture.root.join(format!("templates/{TEMPLATE}.json")),
        template_bytes(TEMPLATE, "active"),
    )?;
    versions::confirm(&repository, ArtifactSourceId::Template(template_id), TIME)?;
    let canonical = fixture.root.join(format!("templates/{TEMPLATE}.json"));
    let historical = fixture.root.join(format!(
        ".worldbuild/content-versions/template-{TEMPLATE}/v00000000000000000001.json"
    ));
    fs::write(&canonical, b"damaged current source")?;
    fs::write(&historical, b"damaged historical source")?;
    let rows = repository.display_templates()?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, TEMPLATE);
    assert_eq!(rows[0].name, "내용을 읽을 수 없는 템플릿");
    assert!(repository.scan_templates().is_err());
    assert!(!versions::list(&repository, ArtifactSourceId::Template(template_id))?[0].available);
    assert_eq!(fs::read(canonical)?, b"damaged current source");
    assert_eq!(fs::read(historical)?, b"damaged historical source");
    Ok(())
}
