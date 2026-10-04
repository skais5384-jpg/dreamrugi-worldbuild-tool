//! Verified independent backup precedes the bounded persistence-policy header.
use super::*;
use crate::data::{
    project_backup,
    repository::{ArtifactRepository, ArtifactSourceId},
};
use std::io;
use transition::{backup_intent, owned_directory};

pub(crate) struct Prepared {
    pub(crate) sources: Vec<(ArtifactSourceId, String)>,
    _directories: Vec<ProjectDirectory>,
    backup: project_backup::TransitionCustody,
}
impl Prepared {
    pub(crate) fn validate(&self) -> io::Result<()> {
        self.backup.validate().map_err(io::Error::other)
    }
}
fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

impl Store {
    pub(crate) fn prepare_policy_transition(
        &self,
        repository: &ArtifactRepository<'_, '_>,
    ) -> io::Result<Option<Prepared>> {
        let sources = repository
            .policy_transition_sources()
            .map_err(io::Error::other)?;
        if sources.is_empty() {
            return Ok(None);
        }
        let project = repository.write_project();
        let profile = self
            .root
            .parent()
            .ok_or_else(|| invalid("policy backup profile unavailable"))?;
        let (intents, intents_guard) = owned_directory(profile, "policy-backup-intents")?;
        let project_guard = ProjectDirectory::open_root(project.canonical_root())?;
        let (snapshot, expected_inventory) =
            project_backup::transition_inventory(project.canonical_root())
                .map_err(io::Error::other)?;
        let cohort = model::digest(
            format!(
                "{}:{:?}:{snapshot}",
                project.fingerprint(),
                project_guard.identity()
            )
            .as_bytes(),
        );
        let (intent, intent_guard) = owned_directory(&intents, &cohort)?;
        let (storage, storage_guard) = owned_directory(profile, "transition-backup-storage")?;
        let id = backup_intent(&intent, &intent_guard, project.fingerprint())?;
        let backup = project_backup::create_transition_backup(
            project.canonical_root(),
            project.fingerprint(),
            &storage,
            "저장 규칙 전환 전",
            &id,
        )
        .map_err(io::Error::other)?;
        let custody =
            project_backup::hold_transition_backup(&backup, &storage, project.fingerprint())
                .map_err(io::Error::other)?;
        let inventory = custody.inventory();
        if inventory != expected_inventory {
            return Err(invalid("policy snapshot changed before backup admission"));
        }
        let directories = vec![project_guard, intents_guard, intent_guard, storage_guard];
        let mut bound = Vec::new();
        for source in sources {
            let relative = source.path().as_str();
            // The source token already contains the digest; never hash that digest
            // a second time when binding the independent original.
            let digest = source
                .sha256()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let expected = (source.byte_length() as u64, digest.clone());
            if inventory.get(relative) != Some(&expected) {
                return Err(invalid("policy backup original differs; preserve files"));
            }
            if !repository
                .reread_bytes_match(&source)
                .map_err(io::Error::other)?
            {
                return Err(invalid("policy source changed before admission"));
            }
            bound.push((source.id(), digest));
        }
        for guard in &directories {
            guard.validate()?;
        }
        Ok(Some(Prepared {
            sources: bound,
            _directories: directories,
            backup: custody,
        }))
    }
}
