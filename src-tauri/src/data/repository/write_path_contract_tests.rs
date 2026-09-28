use super::*;
use crate::data::project_file::directory::{DirectoryIdentity, ProjectDirectory};
use std::process::Command;

fn identity(path: &Path) -> io::Result<DirectoryIdentity> {
    Ok(ProjectDirectory::open_root(path)?.identity())
}
fn junction(link: &Path, target: &Path) -> TestResult {
    let result = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()?;
    assert!(result.status.success(), "fixture junction creation failed");
    assert!(project_file::directory::is_reparse(&fs::symlink_metadata(
        link
    )?));
    Ok(())
}

#[test]
fn create_rejects_internal_external_empty_and_dangling_namespace_aliases() -> TestResult {
    for mode in ["internal", "external", "empty", "dangling"] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let target = if mode == "internal" {
            fixture.root.join("elsewhere")
        } else {
            fixture.base.join("outside")
        };
        fs::create_dir(&target)?;
        let original_id = identity(&target)?;
        if matches!(mode, "internal" | "external") {
            fs::write(target.join("sentinel.json"), CANARY)?;
        }
        let target_before = inventory(&target);
        let alias = fixture.root.join("templates");
        fs::remove_dir(&alias)?;
        junction(&alias, &target)?;
        if mode == "dangling" {
            fs::remove_dir(&target)?;
        }
        let before = inventory(&fixture.root);
        let candidate = template();
        let plan = CanonicalWritePlan::new().create_template(&candidate)?;
        let error = with_plan(plan, &repo, |plan, permit| {
            plan.prepare(&repo, permit).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::RepositoryRejected);
        let detail = error
            .diagnostic()
            .repository
            .expect("repository guard rejection");
        assert_eq!(detail.category, RepositoryCategory::NamespaceUnavailable);
        assert_eq!(detail.operation, RepositoryOperation::PrepareWrite);
        assert_eq!(detail.stage, RepositoryStage::Namespace);
        assert_eq!(detail.artifact_type, Some(ArtifactType::Template));
        assert_eq!(detail.io_kind, Some(io::ErrorKind::InvalidData));
        assert!(
            inventory(&fixture.root) == before,
            "Create changed project entries or bytes"
        );
        assert!(
            !repo.write_project().transactions_root().exists(),
            "alias rejection allocated a transaction"
        );
        assert!(project_file::directory::is_reparse(&fs::symlink_metadata(
            &alias
        )?));
        if mode == "dangling" {
            assert!(!target.exists());
        } else {
            assert!(inventory(&target) == target_before, "alias target changed");
            assert!(
                identity(&target)? == original_id,
                "alias target directory replaced"
            );
        }
        // 확인한 fixture junction 자체만 해제한다. target을 재귀 삭제하는 경로로 쓰지 않는다.
        fs::remove_dir(&alias)?;
    }
    Ok(())
}

#[test]
fn root_and_namespace_replacement_are_os_blocked_through_create_commit() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let original_root = identity(&fixture.root)?;
    let before = inventory(&fixture.root);
    // repository 자체가 이미 strong root guard를 보유한다. 경로 교체가 성공했다고 가정하지 않는다.
    let replacement = fixture.base.join("moved-root");
    let error =
        fs::rename(&fixture.root, &replacement).expect_err("root guard must block replacement");
    assert_eq!(error.raw_os_error(), Some(32));
    assert!(
        inventory(&fixture.root) == before,
        "root replacement attempt changed entries"
    );
    assert!(identity(&fixture.root)? == original_root);
    assert!(!replacement.exists());
    let t = template();
    let d = document();
    let plan = CanonicalWritePlan::new()
        .create_template(&t)?
        .create_document(&d)?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        for namespace in ["templates", "documents"] {
            let path = fixture.root.join(namespace);
            let old_id = identity(&path)?;
            let moved = fixture.root.join(format!("moved-{namespace}"));
            let error = fs::rename(&path, &moved)
                .expect_err("prepared namespace guard must block replacement");
            assert_eq!(error.raw_os_error(), Some(32));
            assert!(identity(&path)? == old_id);
            assert!(!moved.exists());
        }
        assert_eq!(
            prepared.commit()?.result_state(),
            CommitResultState::Committed
        );
        Ok(())
    })?;
    assert!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?
            == artifact::encode_template(&t)?,
        "normal Template control differs"
    );
    assert!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?
            == artifact::encode_document(&d)?,
        "normal Document control differs"
    );
    assert!(identity(&fixture.root)? == original_root);
    Ok(())
}
