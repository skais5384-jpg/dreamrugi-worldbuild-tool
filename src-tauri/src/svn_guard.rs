//! Native admission for SVN working-copy projects. Canonical writes must also
//! pass the exact target lock or new-path check in the SVN provider.
use crate::commands::{document_workspace::Request, dto::Work};
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

static KNOWN: OnceLock<PathBuf> = OnceLock::new();
pub(crate) fn configure(config_root: PathBuf) {
    let _ = KNOWN.set(config_root.join("known-roots"));
}
fn identity(path: &Path) -> String {
    let canonical = path.to_string_lossy().to_lowercase();
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}
fn remembered(path: &Path) -> bool {
    KNOWN
        .get()
        .and_then(|known| fs::read_to_string(known).ok())
        .is_some_and(|known| {
            path.ancestors().any(|ancestor| {
                let hash = identity(ancestor);
                known.lines().any(|line| line == hash)
            })
        })
}
pub(crate) fn remember(project: &Path) -> std::io::Result<()> {
    let Some(known) = KNOWN.get() else {
        return Ok(());
    };
    let canonical = project.canonicalize()?;
    if remembered(&canonical) {
        return Ok(());
    }
    fs::create_dir_all(known.parent().expect("known roots parent"))?;
    let mut file = OpenOptions::new().create(true).append(true).open(known)?;
    writeln!(file, "{}", identity(&canonical))?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn working_copy_root(project: &Path) -> Option<PathBuf> {
    // A project may live below the checkout root. Walk only ancestors of the
    // selected project, never follow a sibling or an external definition.
    let canonical = project.canonicalize().ok()?;
    if remembered(&canonical) {
        return Some(canonical);
    }
    canonical
        .ancestors()
        .find(|ancestor| ancestor.join(".svn").is_dir())
        .map(Path::to_path_buf)
}

pub(crate) fn permitted_in_read_only(work: &Work) -> bool {
    match work {
        Work::Open { .. }
        | Work::CreateProject { .. }
        | Work::ProjectSettingsRead {}
        | Work::ProjectSettingsWrite { .. }
        | Work::RetireProject { .. }
        | Work::BackupInspect { .. }
        | Work::RestoreNew { .. } => true,
        Work::Close { .. }
        | Work::Recover { .. }
        | Work::ListTemplates { .. }
        | Work::ReadTemplate { .. }
        | Work::ReadDocument { .. }
        | Work::AssetInspect { .. }
        | Work::BackupList { .. }
        | Work::BackupDeletedList { .. }
        | Work::DiagnosticExport { .. } => true,
        // Existing, identity-checked M1 recovery remains possible. It is not a
        // general edit admission and never creates a new SVN edit session.
        Work::RecoveryRestore { .. }
        | Work::RecoveryPage { .. }
        | Work::RecoveryRead { .. }
        | Work::RecoveryContent { .. }
        | Work::RecoveryCloseCursor { .. }
        | Work::RecoveryRevalidate { .. }
        | Work::RecoveryDiscard { .. }
        | Work::RecoveryReleaseSelection { .. } => true,
        Work::DocumentWorkspace { request, .. } => matches!(
            request,
            Request::AssetRead { .. }
                | Request::AssetChunk { .. }
                | Request::AssetOpen { .. }
                | Request::UrlOpen { .. }
                | Request::FormatInspect { .. }
                | Request::Search { .. }
                | Request::SearchRead { .. }
                | Request::ReplacePreview { .. }
                | Request::ReplacePage { .. }
                | Request::ReplaceDiscard { .. }
                | Request::References { .. }
                | Request::List { .. }
                | Request::Read { .. }
                | Request::PdfInspect { .. }
                | Request::PdfExport { .. }
        ),
        // Other operations can create a session, change canonical files, or
        // hide an edit inside a maintenance or backup path.
        _ => false,
    }
}

/// Canonical artifact writes enter the existing exact-target session gate.
/// The SVN provider still rejects missing targets and files without a valid
/// needs-lock property; admission alone is never write authority.
pub(crate) fn permitted_in_collaboration(work: &Work, svn_provider: bool) -> bool {
    if permitted_in_read_only(work) {
        return true;
    }
    if matches!(work, Work::ProjectCopy { .. }) {
        return true;
    }
    svn_provider
        && (matches!(
            work,
            Work::DocumentWorkspace {
                request: Request::EditBegin { .. }
                    | Request::AssetImport { .. }
                    | Request::EditDraft { .. }
                    | Request::EditDeposit { .. }
                    | Request::EditRelease { .. }
                    | Request::EditRefresh { .. }
                    | Request::EditRetry { .. }
                    | Request::EditRestore { .. }
                    | Request::Mutate { .. }
                    | Request::ReplaceApply { .. }
                    | Request::Begin { .. }
                    | Request::Draft { .. }
                    | Request::Deposit { .. }
                    | Request::Release { .. }
                    | Request::Restore { .. },
                ..
            }
        ) || matches!(
            work,
            Work::BeginTemplateDraft { .. }
                | Work::TemplateDraft { .. }
                | Work::TemplateDraftContent { .. }
                | Work::RefreshTemplateDraft { .. }
                | Work::ReleaseTemplateDraft { .. }
                | Work::BeginSession { .. }
                | Work::CreateTemplate { .. }
                | Work::DuplicateTemplate { .. }
                | Work::CreateDocument { .. }
                | Work::UpdateTemplate { .. }
                | Work::TombstoneTemplate { .. }
                | Work::TemplateRestore { .. }
                | Work::AssetRename { .. }
                | Work::AssetTrashMove { .. }
                | Work::AssetTrashRestore { .. }
                | Work::AssetTrashPurge { .. }
                | Work::TemplatePurge { .. }
                | Work::MaterializeDocument { .. }
                | Work::SaveDocument { .. }
                | Work::SaveComposite { .. }
                | Work::SessionControl { .. },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::dto::Id;

    #[test]
    fn collaborative_admission_allows_reads_but_denies_canonical_mutations() {
        let project = Id::new();
        assert!(permitted_in_read_only(&Work::DocumentWorkspace {
            project,
            request: Request::Read {
                document: "document".into(),
            },
        }));
        assert!(!permitted_in_read_only(&Work::DocumentWorkspace {
            project,
            request: Request::EditBegin {
                document: "document".into(),
            },
        }));
        assert!(!permitted_in_read_only(&Work::BackupCreate {
            project,
            storage: "storage".into(),
            label: None,
        }));
        assert!(!permitted_in_read_only(&Work::ProjectCopy {
            project,
            parent: "parent".into(),
            name: "copy".into(),
        }));
    }

    #[test]
    fn collaborative_document_edit_requires_the_real_provider() {
        let project = Id::new();
        let edit = Work::DocumentWorkspace {
            project,
            request: Request::EditBegin {
                document: "document".into(),
            },
        };
        assert!(!permitted_in_collaboration(&edit, false));
        assert!(permitted_in_collaboration(&edit, true));
        assert!(!permitted_in_collaboration(
            &Work::BackupCreate {
                project,
                storage: "storage".into(),
                label: None,
            },
            true
        ));
        assert!(permitted_in_collaboration(
            &Work::ProjectCopy {
                project,
                parent: "parent".into(),
                name: "copy".into(),
            },
            true
        ));
        assert!(permitted_in_collaboration(
            &Work::ProjectCopy {
                project,
                parent: "parent".into(),
                name: "copy".into(),
            },
            false
        ));
    }
}

/// Cleanup/deposit must remain reachable when compatibility rejects new work.
pub(crate) fn policy_required(work: &Work) -> bool {
    if matches!(work, Work::RecoveryRestore { .. } | Work::Recover { .. }) {
        return true;
    }
    if matches!(
        work,
        Work::ReleaseTemplateDraft { .. }
            | Work::TemplateDraft {
                action: crate::commands::workspace::DraftAction::Deposit,
                ..
            }
            | Work::DocumentWorkspace {
                request: Request::EditDeposit { .. }
                    | Request::Deposit { .. }
                    | Request::EditRelease { .. }
                    | Request::Release { .. },
                ..
            }
    ) {
        return false;
    }
    !permitted_in_read_only(work)
}
