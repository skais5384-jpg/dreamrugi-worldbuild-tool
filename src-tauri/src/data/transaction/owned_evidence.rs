//! 정리 전 검증한 namespace와 열린 대상 handle을 journal 삭제가 끝날 때까지 보유한다.
use super::super::model::OriginalTarget;
use super::*;

pub(in crate::data::transaction) struct TargetEvidence {
    objects: Vec<(GuardedTarget, Option<File>)>,
}
pub(in crate::data::transaction) fn verify_original_targets(
    project: &LockedProject<'_>,
    id: &TransactionId,
    targets: &[OriginalTarget],
) -> io::Result<TargetEvidence> {
    let mut objects = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let guard = GuardedTarget::open_path(project, &target.target_path)?;
        let file = match project_file::open_existing_project_file(
            project.canonical_root(),
            &target.target_path,
        ) {
            Ok(mut file) => {
                guard.validate(&file, &guard.name)?;
                let size = target
                    .original_size
                    .ok_or_else(|| invalid("preparing create target is present"))?;
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(
                        size.checked_add(1)
                            .ok_or_else(|| invalid("target size overflow"))?,
                    )
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 != size
                    || target.original_sha256.as_deref() != Some(sha256(&bytes).as_str())
                    || target.original_schema_version
                        != Some(
                            super::super::prepare::managed_schema_version(&bytes)
                                .map_err(|_| invalid("target schema differs"))?,
                        )
                {
                    return Err(invalid("actual target contradicts cleanup evidence"));
                }
                guard.validate(&file, &guard.name)?;
                Some(file)
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && target.original_size.is_none() =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        // record directory의 상태와 무관하게 닫힌 이름 둘을 실제로 확인한다.
        for phase in [Phase::Apply, Phase::Restore] {
            let name = format!(".wb-{}-{index:06}-{}.tmp", id.as_str(), phase.name());
            if exists(&guard.path.join(name))? {
                return Err(invalid("namespace temporary contradicts journal cleanup"));
            }
        }
        guard.root.validate()?;
        guard.namespace.validate()?;
        objects.push((guard, file));
    }
    Ok(TargetEvidence { objects })
}
pub(in crate::data::transaction) fn verify_completed_targets(
    project: &LockedProject<'_>,
    directory: &Path,
    manifest: &TransactionManifest,
    committed: bool,
    intact_owned: bool,
) -> io::Result<TargetEvidence> {
    let targets = if committed {
        manifest
            .operations
            .iter()
            .map(|op| OriginalTarget {
                target_path: op.target_path.clone(),
                original_size: Some(op.staged_size),
                original_sha256: Some(op.staged_sha256.clone()),
                original_schema_version: op.staged_schema_version,
            })
            .collect()
    } else {
        manifest
            .original_targets()
            .ok_or_else(|| invalid("completion needs owned evidence"))?
    };
    let evidence = verify_original_targets(project, &manifest.transaction_id, &targets)?;
    if !exists(&directory.join(DIRECTORY))? {
        return Ok(evidence);
    }
    let mut allowed = std::collections::BTreeSet::new();
    for (op, (_, target_file)) in manifest.operations.iter().zip(&evidence.objects) {
        for phase in [Phase::Apply, Phase::Restore] {
            let i = intent(&manifest.transaction_id, op, phase);
            let mut has_intent = false;
            let mut has_receipt = false;
            for acquired in [false, true] {
                let path = record_path(directory, &i, acquired);
                allowed.insert(path.clone());
                let Some(bytes) = read_record(directory, &path)? else {
                    continue;
                };
                if acquired {
                    has_receipt = true;
                    match serde_json::from_slice::<Receipt>(&bytes) {
                        Ok(receipt) => {
                            if receipt.intent != i {
                                return Err(invalid("remaining receipt contradicts completion"));
                            }
                            if (committed && matches!(phase, Phase::Apply))
                                || (!committed && matches!(phase, Phase::Restore))
                            {
                                let file = target_file
                                    .as_ref()
                                    .ok_or_else(|| invalid("completed receipt has no target"))?;
                                if native::identity(file)? != receipt.object {
                                    return Err(invalid("completed target identity changed"));
                                }
                            }
                        }
                        // rollback이 원본/부재와 temp 부재를 입증한 apply 부분 생성만 남을 수 있다.
                        Err(error)
                            if !committed && matches!(phase, Phase::Apply) && error.is_eof() => {}
                        Err(_) => return Err(invalid("invalid remaining completion receipt")),
                    }
                } else {
                    has_intent = true;
                    match serde_json::from_slice::<Intent>(&bytes) {
                        Ok(value) if value == i => {}
                        Err(error)
                            if !committed && matches!(phase, Phase::Apply) && error.is_eof() => {}
                        _ => return Err(invalid("invalid remaining completion intent")),
                    }
                }
            }
            if committed && matches!(phase, Phase::Restore) && (has_intent || has_receipt) {
                return Err(invalid("restore record contradicts committed completion"));
            }
            if intact_owned
                && committed
                && matches!(phase, Phase::Apply)
                && !(has_intent && has_receipt)
            {
                return Err(invalid(
                    "ownership records removed before auxiliary cleanup",
                ));
            }
        }
    }
    for entry in fs::read_dir(directory.join(DIRECTORY))? {
        if !allowed.contains(&entry?.path()) {
            return Err(invalid("unknown completion ownership record"));
        }
    }
    Ok(evidence)
}
