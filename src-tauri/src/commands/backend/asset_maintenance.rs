use super::*;
use crate::data::asset_maintenance::{self, Category};
use crate::data::{
    collaboration_lock::{HeldLock, LockAcquireRequest, LockSessionId},
    project_relative_path::ProjectRelativePath,
};
use std::path::Path;

fn code(error: &asset_maintenance::Error) -> Code {
    match error.category() {
        Category::Cancelled => Code::Cancelled,
        Category::InvalidInput => Code::InvalidInput,
        Category::Stale | Category::Conflict | Category::Incomplete => Code::AssetMaintenanceStale,
        Category::Io | Category::Corrupt | Category::TooLarge => Code::AssetMaintenanceRejected,
    }
}

fn inspected(mut inspection: asset_maintenance::Inspection, owners: bool) -> ResultDto {
    if owners {
        inspection.mark_uncommitted_protected();
    }
    ResultDto::AssetMaintenance {
        action: "inspect",
        inspection,
        completed: Vec::new(),
        completed_count: 0,
        failures: Vec::new(),
        partial: false,
        cleanup_required: Vec::new(),
    }
}

fn action(result: asset_maintenance::ActionResult) -> ResultDto {
    ResultDto::AssetMaintenance {
        action: result.action,
        inspection: result.inspection,
        completed: result.completed,
        completed_count: result.completed_count,
        failures: result.failures,
        partial: result.partial,
        cleanup_required: result.cleanup_required,
    }
}

pub(crate) fn execute(ctx: &mut Context, job: &Job) -> Reply<Completed> {
    let owners = ctx.has_project_owners();
    if owners
        && matches!(
            &*job.input,
            Work::AssetTrashMove { .. }
                | Work::AssetRename { .. }
                | Work::AssetTrashRestore { .. }
                | Work::AssetTrashPurge { .. }
                | Work::TemplatePurge { .. }
        )
    {
        return Err(Code::OwnersRemain.into());
    }
    let project_root = ctx
        .read(|ready| ready.locked_project().canonical_root().to_path_buf())
        .map_err(|_| Code::RuntimeRejected)?;
    let targets = if job.collaborative {
        match &*job.input {
            Work::AssetRename { asset, .. } => {
                vec![
                    ProjectRelativePath::parse(&format!("assets/{asset}/metadata.json"))
                        .map_err(|_| Code::InvalidInput)?,
                ]
            }
            Work::TemplatePurge { templates, .. } => templates
                .iter()
                .map(|id| {
                    ProjectRelativePath::parse(&format!("templates/{id}.json"))
                        .map_err(|_| Code::InvalidInput.into())
                })
                .collect::<Reply<Vec<_>>>()?,
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let mut held: Vec<(ProjectRelativePath, Box<dyn HeldLock>)> = Vec::new();
    for target in targets {
        let session = LockSessionId::generate().map_err(|_| Code::SessionRejected)?;
        let request = LockAcquireRequest::new(
            ctx.project_fingerprint().ok_or(Code::Unavailable)?,
            &session,
            &target,
        )
        .map_err(|_| Code::SessionRejected)?;
        let lock = match job.provider.acquire(request) {
            Ok(lock) => lock,
            Err(_) => {
                for (_, previous) in &mut held {
                    let _ = job.provider.release(previous.as_mut());
                }
                return Err(Code::SessionRejected.into());
            }
        };
        held.push((target, lock));
    }
    let result = ctx.read(|ready| {
        let project = ready.locked_project();
        let root = project.canonical_root();
        match &*job.input {
            Work::AssetInspect { .. } => {
                asset_maintenance::inspect(root).map(|value| inspected(value, owners))
            }
            Work::AssetTrashMove {
                inspection_token,
                assets,
                ..
            } => asset_maintenance::move_to_trash(root, inspection_token, assets).map(action),
            Work::AssetTrashRestore { assets, .. } => {
                asset_maintenance::restore_from_trash(root, assets).map(action)
            }
            Work::AssetRename {
                inspection_token,
                asset,
                name,
                ..
            } => asset_maintenance::rename_asset(root, inspection_token, asset, name).map(action),
            Work::AssetTrashPurge {
                inspection_token,
                assets,
                empty,
                ..
            } => asset_maintenance::purge_trash(root, inspection_token, assets, *empty).map(action),
            Work::TemplatePurge {
                inspection_token,
                templates,
                ..
            } => asset_maintenance::purge_templates(root, inspection_token, templates).map(action),
            Work::DiagnosticExport { destination, .. } => {
                let inspection = asset_maintenance::inspect(root)?;
                let snapshot = crate::diagnostic_log::ProjectSnapshot {
                    fingerprint: project.fingerprint().to_owned(),
                    observed_at_utc: inspection.observed_at_utc.clone(),
                    inspection_complete: inspection.complete,
                    scanned_files: inspection.scanned_files,
                    used_assets: inspection.used_assets,
                    unused_assets: inspection.unused_assets,
                    missing_assets: inspection.missing_assets,
                    corrupt_assets: inspection.corrupt_assets,
                    uncertain_assets: inspection.uncertain_assets,
                    trash_assets: inspection.trash.len(),
                };
                crate::diagnostic_log::export(Path::new(destination), Some(snapshot), Some(root))
                    .map(|result| ResultDto::DiagnosticExport { result })
                    .map_err(|_| asset_maintenance::Error::diagnostic_export())
            }
            _ => Err(asset_maintenance::Error::invalid_input()),
        }
    });
    for (target, lock) in &mut held {
        // A purged versioned Template is still a WC missing path. Keep its
        // exact server token until the user schedules and commits that D.
        if matches!(&*job.input, Work::TemplatePurge { .. })
            && !project_root.join(target.as_str()).exists()
        {
            continue;
        }
        if job.provider.release(lock.as_mut()).is_err() {
            return Ok(Completed::reject(
                (result, held),
                Code::ReleaseRejected.into(),
            ));
        }
    }
    let result = result.map_err(|_| Code::RuntimeRejected)?;
    match result {
        Ok(dto) => Ok(Completed::new((), Ok(dto))),
        Err(error) => {
            let rejected = if matches!(&*job.input, Work::DiagnosticExport { .. }) {
                Code::DiagnosticExportRejected
            } else {
                code(&error)
            };
            Ok(Completed::reject(error, rejected.into()))
        }
    }
}
