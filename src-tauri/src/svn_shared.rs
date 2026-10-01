//! Exact project-owned paths eligible for first SVN registration.
//! The personal snapshot may contain format history and private extras; none
//! of those gain SVN authority through their filename extension alone.
use crate::data::{
    artifact::{decode_document, decode_layout, decode_template},
    assets, media, project_backup,
    project_relative_path::ProjectRelativePath,
    repository::ArtifactSourceId,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};

pub(crate) struct Inventory {
    pub(crate) directories: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) excluded_files: usize,
    pub(crate) fingerprint: String,
}

pub(crate) fn inventory(root: &Path) -> Result<Inventory, &'static str> {
    inventory_inner(root, true)
}
pub(crate) fn registration_inventory(root: &Path) -> Result<Inventory, &'static str> {
    inventory_inner(root, false)
}
fn inventory_inner(root: &Path, include_policy: bool) -> Result<Inventory, &'static str> {
    let (all_directories, all_files) =
        project_backup::snapshot_paths(root).map_err(|_| "svn_share_invalid")?;
    let present: BTreeSet<_> = all_files.iter().map(String::as_str).collect();
    let mut selected = BTreeSet::new();
    let mut templates = BTreeSet::new();
    let mut document_templates = Vec::new();
    for raw in &all_files {
        if raw.starts_with(".worldbuild/") {
            continue;
        }
        let relative = ProjectRelativePath::parse(raw).map_err(|_| "svn_share_invalid")?;
        let file = root.join(raw);
        if raw == crate::svn::policy::FILE {
            crate::svn::policy::validate_file(&file).map_err(|_| "svn_share_invalid")?;
            if include_policy {
                selected.insert(raw.clone());
            }
            continue;
        }
        match ArtifactSourceId::from_target(&relative) {
            Some(ArtifactSourceId::Template(id)) => {
                let artifact = decode_template(&fs::read(&file).map_err(|_| "svn_share_invalid")?)
                    .map_err(|_| "svn_share_invalid")?;
                if artifact.template_id() != id {
                    return Err("svn_share_invalid");
                }
                templates.insert(id.to_string());
                selected.insert(raw.clone());
            }
            Some(ArtifactSourceId::Document(id)) => {
                let bytes = fs::read(&file).map_err(|_| "svn_share_invalid")?;
                let artifact = decode_document(&bytes).map_err(|_| "svn_share_invalid")?;
                if artifact.document_id() != id {
                    return Err("svn_share_invalid");
                }
                assets::validate_document(root, &bytes).map_err(|_| "svn_share_invalid")?;
                document_templates.push(artifact.template_id().to_string());
                selected.insert(raw.clone());
            }
            Some(ArtifactSourceId::DocumentLayout) => {
                decode_layout(&fs::read(&file).map_err(|_| "svn_share_invalid")?)
                    .map_err(|_| "svn_share_invalid")?;
                selected.insert(raw.clone());
            }
            None => {}
        }
    }
    if document_templates
        .iter()
        .any(|template| !templates.contains(template))
    {
        return Err("svn_share_dependency_missing");
    }
    if all_directories.iter().any(|path| path == "assets") {
        let store = assets::Store::open(root, false).map_err(|_| "svn_share_invalid")?;
        for directory in &all_directories {
            let Some(id) = directory.strip_prefix("assets/") else {
                continue;
            };
            if id.contains('/') || !media::valid_id(id) {
                continue;
            }
            let (metadata, _) = store.read(id).map_err(|_| "svn_share_invalid")?;
            for suffix in ["metadata.json".to_owned(), assets::filename(&metadata)] {
                let path = format!("assets/{id}/{suffix}");
                if !present.contains(path.as_str()) {
                    return Err("svn_share_invalid");
                }
                selected.insert(path);
            }
        }
    }
    // A new empty collaborative project still needs versioned parents before
    // the first Template, document, layout or asset can acquire authority.
    let mut directories = BTreeSet::from([
        "assets".to_owned(),
        "documents".to_owned(),
        "templates".to_owned(),
        "workspace".to_owned(),
    ]);
    for raw in &all_directories {
        if matches!(
            raw.as_str(),
            "templates" | "documents" | "workspace" | "assets"
        ) || (raw.starts_with("assets/")
            && !raw[7..].contains('/')
            && selected
                .iter()
                .any(|file| file.starts_with(&format!("{raw}/"))))
        {
            directories.insert(raw.clone());
        }
    }
    let mut fingerprint = Sha256::new();
    for directory in &directories {
        fingerprint.update(directory.as_bytes());
        fingerprint.update([0]);
    }
    for path in &selected {
        fingerprint.update(path.as_bytes());
        fingerprint.update([0]);
        fingerprint.update(fs::read(root.join(path)).map_err(|_| "svn_share_invalid")?);
        fingerprint.update([0]);
    }
    Ok(Inventory {
        directories: directories.into_iter().collect(),
        excluded_files: all_files.len() - selected.len(),
        files: selected.into_iter().collect(),
        fingerprint: format!("{:x}", fingerprint.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_project_previews_versioned_creation_parents() {
        let root = std::env::temp_dir().join(format!("svn-share-empty-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let inventory = inventory(&root).unwrap();
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.excluded_files, 0);
        assert_eq!(
            inventory.directories,
            ["assets", "documents", "templates", "workspace"]
        );
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn private_history_and_unrecognized_workspace_file_never_enter_registration() {
        let root = std::env::temp_dir().join(format!("svn-share-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for directory in [
            "templates",
            "documents",
            "workspace",
            ".worldbuild/format-history",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        fs::write(root.join("workspace/private.json"), b"personal").unwrap();
        fs::write(
            root.join(".worldbuild/format-history/example.json"),
            b"history",
        )
        .unwrap();
        let inventory = inventory(&root).unwrap();
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.excluded_files, 2);
        assert_eq!(
            inventory.directories,
            ["assets", "documents", "templates", "workspace"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
