//! Handoff from the one known pre-fix MSIX LocalCache recovery directory.
//! Both Stores retain their files; a verified target is the only success.
use super::*;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSummary {
    pub(crate) found: usize,
    pub(crate) imported: usize,
    pub(crate) already_present: usize,
    pub(crate) needs_attention: usize,
    pub(crate) held: Vec<String>,
}

/// Safe startup result. No paths, draft contents, or account details cross the UI boundary.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandoffStatus {
    pub(crate) state: &'static str,
    pub(crate) summary: Option<ImportSummary>,
    pub(crate) error: Option<crate::data::edit_recovery::error::ErrorDto>,
}

impl HandoffStatus {
    pub(crate) fn pending() -> Self {
        Self {
            state: "pending",
            summary: None,
            error: None,
        }
    }
}

impl Store {
    pub(crate) fn import_legacy(
        &mut self,
        legacy_root: &Path,
    ) -> Result<Option<ImportSummary>, RecoveryError> {
        match fs::symlink_metadata(legacy_root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(RecoveryError::io(Stage::Initialize, error)),
            Ok(_) => (),
        }
        let mut source = Store::open(legacy_root)?;
        if source.root == self.root {
            return Err(RecoveryError::new(Category::Conflict, Stage::Initialize));
        }
        let mut summary = ImportSummary::default();
        let mut cursor = None;
        loop {
            let page = source.page(cursor.as_deref())?;
            for row in page.entries {
                let locator = row.row.locator_fingerprint.clone();
                let (Some(key), Some(deposit_id)) = (row.row.key, row.row.deposit_id) else {
                    // Asset directories are not deposit rows. All other invalid
                    // rows remain in the old Store for manual inspection.
                    if row.row.error.is_some() {
                        summary.needs_attention += 1;
                        if summary.held.len() < 8 {
                            summary.held.push(locator[..12].to_owned());
                        }
                    }
                    continue;
                };
                summary.found += 1;
                let result = (|| {
                    let deposit = source.read(&key, &deposit_id)?;
                    let target = self
                        .root
                        .join(&key.project_fingerprint)
                        .join(&key.draft_id)
                        .join(format!("{}.json", key.generation));
                    if target.exists() {
                        let existing = self.read(&key, &deposit_id)?;
                        if existing.bytes() != deposit.bytes() {
                            return Err(RecoveryError::new(Category::Conflict, Stage::Revalidate));
                        }
                        self.accept(&deposit)?;
                        return Ok(false);
                    }
                    self.import_assets(&source, &deposit)?;
                    self.accept(&deposit)?;
                    Ok(true)
                })();
                match result {
                    Ok(true) => summary.imported += 1,
                    Ok(false) => summary.already_present += 1,
                    Err(_) => {
                        summary.needs_attention += 1;
                        if summary.held.len() < 8 {
                            summary.held.push(locator[..12].to_owned());
                        }
                    }
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(Some(summary))
    }
}
