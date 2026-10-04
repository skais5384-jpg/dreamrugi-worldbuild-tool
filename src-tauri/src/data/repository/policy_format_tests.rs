use super::*;
use crate::data::{edit_recovery::Store, project_backup};

fn old_sources(fixture: &Fixture) -> (Vec<u8>, Vec<u8>) {
    let mut template = template_value();
    template["schemaVersion"] = 7.into();
    let mut document = document_value();
    document["schemaVersion"] = 6.into();
    let (template, document) = (raw(&template), raw(&document));
    fixture.write(ArtifactSourceId::Template(template_id()), &template);
    fixture.write(ArtifactSourceId::Document(document_id()), &document);
    (template, document)
}

#[test]
fn policy_formats_back_up_exact_all_bytes_hold_custody_and_commit_both_headers() -> TestResult {
    let fixture = Fixture::new();
    let (template, document) = old_sources(&fixture);
    fs::create_dir(fixture.root.join("assets"))?;
    fs::write(
        fixture.root.join("assets/owned-evidence.txt"),
        b"owned attachment bytes",
    )?;
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let store = Store::open(&fixture.base.join("profile/edit-recovery"))?;
    let prepared = store.prepare_policy_transition(&repo)?.unwrap();
    assert_eq!(prepared.sources.len(), 2);
    drop(prepared);
    let storage = fixture.base.join("profile/transition-backup-storage");
    let backups = project_backup::list_backups(repo.write_project().fingerprint(), &storage, None)?;
    assert_eq!(backups.backups.len(), 1);
    let prepared = store.prepare_policy_transition(&repo)?.unwrap();
    let package = Path::new(&backups.backups[0].locator);
    for relative in [
        "worldbuild-backup.json",
        "payload/assets/owned-evidence.txt",
    ] {
        assert!(fs::OpenOptions::new()
            .write(true)
            .open(package.join(relative))
            .is_err());
        assert!(fs::remove_file(package.join(relative)).is_err());
    }
    assert!(fs::rename(
        package.parent().unwrap(),
        package.parent().unwrap().with_extension("moved")
    )
    .is_err());
    let plan = super::super::format::build_policy_batch(&repo, &prepared.sources)?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let transaction = plan.prepare(&repo, permit)?;
        assert_eq!(transaction.inner_for_test().manifest().operations.len(), 2);
        prepared.validate()?;
        assert_eq!(
            transaction.commit()?.result_state(),
            CommitResultState::Committed
        );
        prepared.validate()?;
        Ok(())
    })?;
    assert!(repo.policy_transition_sources()?.is_empty());
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        artifact::transition_format(
            &template,
            &ArtifactSourceId::Template(template_id()).path()?
        )
        .map_err(io::Error::other)?
    );
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
        artifact::transition_format(
            &document,
            &ArtifactSourceId::Document(document_id()).path()?
        )
        .map_err(io::Error::other)?
    );
    drop(prepared);
    assert_eq!(
        fs::read(package.join(format!("payload/templates/{TEMPLATE}.json")))?,
        template
    );
    assert_eq!(
        fs::read(package.join(format!("payload/documents/{DOCUMENT}.json")))?,
        document
    );
    project_backup::verified_inventory(&project_backup::inspect_backup(package)?)?;
    assert!(store.prepare_policy_transition(&repo)?.is_none());
    Ok(())
}

#[test]
fn policy_failed_disk_admission_preserves_original_and_repeated_identical_snapshot_is_bounded(
) -> TestResult {
    let fixture = Fixture::new();
    let (template, document) = old_sources(&fixture);
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let store = Store::open(&fixture.base.join("profile/edit-recovery"))?;
    let storage = fixture.base.join("profile/transition-backup-storage");
    let mut counts = Vec::new();
    for _ in 0..4 {
        let preserved = store
            .prepare_policy_transition(&repo)
            .expect("policy backup preparation")
            .unwrap();
        let plan = super::super::format::build_policy_batch(&repo, &preserved.sources)?;
        let (error, _) = with_storage_response(
            TestStorageResponse::Available {
                available_bytes: 0,
                allocation_unit_bytes: 4096,
            },
            || {
                with_plan(plan, &repo, |plan, permit| {
                    plan.prepare(&repo, permit).unwrap_err()
                })
            },
        );
        checked_error(&error, ArtifactWriteCategory::StorageRejected);
        preserved.validate()?;
        assert_eq!(
            fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
            template
        );
        assert_eq!(
            fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
            document
        );
        drop(preserved);
        let backups =
            project_backup::list_backups(repo.write_project().fingerprint(), &storage, None)
                .expect("list after policy custody released");
        for backup in &backups.backups {
            project_backup::verified_inventory(&project_backup::inspect_backup(Path::new(
                &backup.locator,
            ))?)?;
        }
        counts.push(backups.backups.len());
    }
    // The first failed preparation durably adds the original content versions.
    // That different project inventory gets one additional complete snapshot;
    // identical further retries never accumulate packages or versions.
    assert_eq!(counts, vec![1, 2, 2, 2]);
    for id in [
        ArtifactSourceId::Template(template_id()),
        ArtifactSourceId::Document(document_id()),
    ] {
        assert_eq!(super::super::versions::list(&repo, id)?.len(), 1);
    }
    Ok(())
}

#[test]
fn policy_backup_failure_blocks_transition_and_known_corruption_remains_recoverable() -> TestResult
{
    let fixture = Fixture::new();
    let (template, _) = old_sources(&fixture);
    fixture.write(
        ArtifactSourceId::Document(document_id()),
        b"owned damaged source",
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let store = Store::open(&fixture.base.join("profile/edit-recovery"))?;
    assert_eq!(repo.policy_transition_sources()?.len(), 1);
    fs::write(
        fixture.base.join("profile/policy-backup-intents"),
        b"owned obstruction",
    )?;
    assert!(store.prepare_policy_transition(&repo).is_err());
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        template
    );
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
        b"owned damaged source"
    );
    fs::remove_file(fixture.base.join("profile/policy-backup-intents"))?;
    let prepared = store.prepare_policy_transition(&repo)?.unwrap();
    assert_eq!(prepared.sources.len(), 1);
    Ok(())
}

#[test]
fn policy_historical_document_six_repairs_current_missing_or_corrupt_to_seven() -> TestResult {
    for corrupt in [false, true] {
        let fixture = Fixture::new();
        let (_, original) = old_sources(&fixture);
        let id = ArtifactSourceId::Document(document_id());
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        super::super::versions::confirm(&repo, id, TIME)?;
        let historical = super::super::versions::snapshot(&repo, id, 1)?.0;
        assert_eq!(historical, original);
        if corrupt {
            fixture.write(id, b"owned damaged document six");
        } else {
            fs::remove_file(fixture.path(id))?;
        }
        let observation = super::super::versions::observation(&repo, id)?;
        let plan = super::super::versions::restore_plan(&repo, id, 1, &observation, TIME)?.unwrap();
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            assert_eq!(
                plan.prepare(&repo, permit)?.commit()?.result_state(),
                CommitResultState::Committed
            );
            Ok(())
        })?;
        let restored = repo.load_document(document_id())?;
        assert_eq!(
            restored.artifact().schema_version(),
            artifact::DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(restored.artifact().document_id(), document_id());
        assert_eq!(super::super::versions::snapshot(&repo, id, 1)?.0, original);
        let restored_raw: serde_json::Value = serde_json::from_slice(&fs::read(fixture.path(id))?)?;
        let old_raw: serde_json::Value = serde_json::from_slice(&original)?;
        assert_eq!(restored_raw["fields"], old_raw["fields"]);
    }
    Ok(())
}
