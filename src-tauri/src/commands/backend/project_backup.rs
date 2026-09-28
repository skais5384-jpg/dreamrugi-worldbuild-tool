use super::*;
use crate::data::project_backup::{self, BackupCategory, BackupKind, BackupRow, DeletedBackupRow};
use std::{path::Path, sync::Arc};

fn code(error: &project_backup::BackupError, input: &Work) -> Code {
    let restore = matches!(input, Work::RestoreCurrent { .. } | Work::RestoreNew { .. });
    let deleted = matches!(
        input,
        Work::BackupDelete { .. }
            | Work::BackupDeletedList { .. }
            | Work::BackupDeletedRestore { .. }
            | Work::BackupDeletedPurge { .. }
    );
    match error.category() {
        BackupCategory::Cancelled => Code::Cancelled,
        BackupCategory::RetentionFull => Code::BackupRetentionFull,
        BackupCategory::Corrupt | BackupCategory::Unsupported => Code::BackupCorrupt,
        BackupCategory::RecoveryRequired if restore => Code::RestoreRecoveryRequired,
        BackupCategory::RecoveryRequired if deleted => Code::BackupDeleteRejected,
        _ if restore => Code::RestoreRejected,
        _ if deleted => Code::BackupDeleteRejected,
        BackupCategory::DestinationOccupied | BackupCategory::SourceChanged => {
            Code::SnapshotRejected
        }
        _ => Code::BackupRejected,
    }
}

fn data(
    action: &'static str,
    root: Option<String>,
    backup: Option<BackupRow>,
    safety: Option<BackupRow>,
    backups: Vec<BackupRow>,
    next_cursor: Option<String>,
    recovery_required: bool,
    outcome: Option<&'static str>,
    warning: Option<&'static str>,
) -> ResultDto {
    ResultDto::ProjectData {
        action,
        root,
        backup,
        safety,
        backups,
        deleted: None,
        deleted_backups: vec![],
        next_cursor,
        recovery_required,
        outcome,
        warning,
    }
}

fn with_deleted(
    mut dto: ResultDto,
    deleted: Option<DeletedBackupRow>,
    deleted_backups: Vec<DeletedBackupRow>,
) -> ResultDto {
    if let ResultDto::ProjectData {
        deleted: item,
        deleted_backups: items,
        ..
    } = &mut dto
    {
        *item = deleted;
        *items = deleted_backups;
    }
    dto
}

#[cfg(test)]
mod serialization_tests {
    use super::*;

    #[test]
    fn deleted_list_uses_frontend_field_name() {
        let dto = with_deleted(
            data(
                "backup_deleted_list",
                None,
                None,
                None,
                vec![],
                None,
                false,
                None,
                None,
            ),
            None,
            vec![],
        );
        let value = serde_json::to_value(dto).unwrap();
        assert_eq!(value["action"], "backup_deleted_list");
        assert!(value.get("deletedBackups").unwrap().is_array());
        assert!(value.get("deleted_backups").is_none());
    }
}

pub(crate) fn is_local(work: &Work) -> bool {
    matches!(work, Work::BackupInspect { .. } | Work::RestoreNew { .. })
}

pub(crate) fn run(input: Arc<Work>) -> Completed {
    let result = match &*input {
        Work::BackupInspect { locator } => {
            project_backup::inspect_backup(Path::new(locator)).map(|backup| {
                Completed::new(
                    (),
                    Ok(data(
                        "inspect",
                        None,
                        Some(backup),
                        None,
                        vec![],
                        None,
                        false,
                        None,
                        None,
                    )),
                )
            })
        }
        Work::RestoreNew {
            locator,
            parent,
            name,
        } => {
            project_backup::restore_new(Path::new(locator), Path::new(parent), name).map(|result| {
                Completed::new(
                    (),
                    Ok(data(
                        "restore_new",
                        Some(result.root.to_string_lossy().into_owned()),
                        result.row,
                        None,
                        vec![],
                        None,
                        false,
                        Some(result.outcome),
                        result.warning,
                    )),
                )
            })
        }
        _ => return Completed::reject(input, Code::InvalidInput.into()),
    };
    result.unwrap_or_else(|error| {
        let rejected = code(&error, &input);
        Completed::reject(error, rejected.into())
    })
}

pub(crate) fn execute(ctx: &mut Context, job: &Job) -> Reply<Completed> {
    if matches!(&*job.input, Work::RestoreCurrent { .. }) && ctx.has_project_owners() {
        return Err(Code::OwnersRemain.into());
    }
    let mut restore_root = None;
    let result = ctx
        .read(|ready| {
            let project = ready.locked_project();
            let root = project.canonical_root();
            let fingerprint = project.fingerprint();
            match &*job.input {
                Work::ProjectCopy { parent, name, .. } => {
                    project_backup::export_project(root, Path::new(parent), name).map(|result| {
                        data(
                            "copy",
                            Some(result.root.to_string_lossy().into_owned()),
                            None,
                            None,
                            vec![],
                            None,
                            false,
                            Some(result.outcome),
                            result.warning,
                        )
                    })
                }
                Work::BackupCreate { storage, label, .. } => project_backup::create_backup(
                    root,
                    fingerprint,
                    Path::new(storage),
                    label.as_deref(),
                    BackupKind::Manual,
                )
                .map(|backup| {
                    data(
                        "backup_create",
                        None,
                        Some(backup),
                        None,
                        vec![],
                        None,
                        false,
                        Some("published_verified"),
                        None,
                    )
                }),
                Work::BackupList {
                    storage, cursor, ..
                } => {
                    project_backup::list_backups(fingerprint, Path::new(storage), cursor.as_deref())
                        .map(|page| {
                            data(
                                "backup_list",
                                None,
                                None,
                                None,
                                page.backups,
                                page.next_cursor,
                                false,
                                None,
                                None,
                            )
                        })
                }
                Work::BackupDelete {
                    storage, locator, ..
                } => project_backup::quarantine_backup(
                    Path::new(storage),
                    Path::new(locator),
                    fingerprint,
                )
                .map(|result| {
                    with_deleted(
                        data(
                            "backup_delete",
                            None,
                            None,
                            None,
                            vec![],
                            None,
                            false,
                            Some(result.outcome),
                            result.warning,
                        ),
                        result.deleted,
                        vec![],
                    )
                }),
                Work::BackupDeletedList { storage, .. } => {
                    let cleanup =
                        project_backup::cleanup_one_expired_backup(Path::new(storage), fingerprint);
                    project_backup::list_deleted_backups(fingerprint, Path::new(storage)).map(
                        |rows| {
                            with_deleted(
                                data(
                                    "backup_deleted_list",
                                    None,
                                    None,
                                    None,
                                    vec![],
                                    None,
                                    false,
                                    None,
                                    if cleanup.is_err()
                                        || cleanup
                                            .ok()
                                            .flatten()
                                            .is_some_and(|result| result.outcome != "deleted")
                                    {
                                        Some("backup_expired_cleanup_required")
                                    } else {
                                        None
                                    },
                                ),
                                None,
                                rows,
                            )
                        },
                    )
                }
                Work::BackupDeletedRestore {
                    storage,
                    id,
                    operation,
                    ..
                } => project_backup::restore_deleted_backup(
                    Path::new(storage),
                    fingerprint,
                    id,
                    operation,
                )
                .map(|result| {
                    data(
                        "backup_deleted_restore",
                        None,
                        None,
                        None,
                        vec![],
                        None,
                        false,
                        Some(result.outcome),
                        result.warning,
                    )
                }),
                Work::BackupDeletedPurge {
                    storage,
                    id,
                    operation,
                    ..
                } => project_backup::purge_deleted_backup(
                    Path::new(storage),
                    fingerprint,
                    id,
                    operation,
                    true,
                )
                .map(|result| {
                    data(
                        "backup_deleted_purge",
                        None,
                        None,
                        None,
                        vec![],
                        None,
                        false,
                        Some(result.outcome),
                        result.warning,
                    )
                }),
                Work::RestoreCurrent {
                    locator,
                    safety_storage,
                    ..
                } => {
                    restore_root = Some(root.to_string_lossy().into_owned());
                    project_backup::restore_current(
                        root,
                        fingerprint,
                        Path::new(locator),
                        Path::new(safety_storage),
                    )
                    .map(|result| {
                        data(
                            "restore_current",
                            Some(result.root.to_string_lossy().into_owned()),
                            result.row,
                            result.safety,
                            vec![],
                            None,
                            false,
                            Some(result.outcome),
                            result.warning,
                        )
                    })
                }
                _ => Err(project_backup::BackupError::from(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid project data operation",
                ))),
            }
        })
        .map_err(|_| Code::RuntimeRejected)?;
    match result {
        Ok(dto) => {
            if matches!(
                &dto,
                ResultDto::ProjectData {
                    outcome: Some("applied_verified" | "applied_recovered"),
                    ..
                }
            ) {
                ctx.invalidate_runtime()
                    .map_err(|_| Code::RuntimeRejected)?;
            }
            Ok(Completed::new((), Ok(dto)))
        }
        Err(error) => {
            let restore = matches!(&*job.input, Work::RestoreCurrent { .. });
            if restore && error.category() == BackupCategory::RecoveryRequired {
                let safety = error.safety();
                let recovery = ctx.recover();
                if recovery.is_ok() {
                    ctx.invalidate_runtime()
                        .map_err(|_| Code::RuntimeRejected)?;
                    return Ok(Completed::new(
                        (error, recovery),
                        Ok(data(
                            "restore_current",
                            restore_root,
                            None,
                            safety,
                            vec![],
                            None,
                            false,
                            Some("applied_recovered"),
                            Some("post_commit_recovery_completed"),
                        )),
                    ));
                }
                return Ok(Completed::new(
                    (error, recovery),
                    Ok(data(
                        "restore_current",
                        restore_root,
                        None,
                        safety,
                        vec![],
                        None,
                        true,
                        Some("recovery_required"),
                        Some("post_commit_recovery_failed"),
                    )),
                ));
            }
            let rejected = code(&error, &job.input);
            Ok(Completed::reject(error, rejected.into()))
        }
    }
}
