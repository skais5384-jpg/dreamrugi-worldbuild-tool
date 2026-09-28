//! 전체 초안 receipt는 참조 ID뿐 아니라 별도 보관한 원본 bytes의 검증까지 포함한다.
use super::*;
use crate::data::assets;
pub(super) fn ids(deposit: &Deposit) -> Result<std::collections::BTreeSet<String>, RecoveryError> {
    let envelope = deposit.envelope();
    let raw = serde_json::to_value(&envelope.draft)
        .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
    let mut ids = assets::references(&raw)?;
    for original in &envelope.originals {
        let raw = crate::data::json::parse_strict_json_object(original.snapshot.as_bytes())
            .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
        ids.extend(assets::references(&raw)?);
    }
    Ok(ids)
}
impl Store {
    /// Copy referenced recovery assets before publishing the envelope. The
    /// source is retained even if a later step fails, making retries safe.
    pub(super) fn import_assets(
        &self,
        source: &Store,
        deposit: &Deposit,
    ) -> Result<(), RecoveryError> {
        let ids = ids(deposit)?;
        if ids.is_empty() {
            return Ok(());
        }
        source.verify_assets(deposit)?;
        let (source_path, _) = source.directory(deposit.key(), false)?;
        let (target_path, _) = self.directory(deposit.key(), true)?;
        let source_assets = assets::Store::open(&source_path, false)?;
        let target_assets = assets::Store::open(&target_path, true)?;
        for id in ids {
            let (meta, bytes) = source_assets.read(&id)?;
            target_assets.put(&meta, &bytes)?;
        }
        self.verify_assets(deposit)
    }
    pub(crate) fn capture_assets(
        &self,
        deposit: &Deposit,
        project: &Path,
    ) -> Result<(), RecoveryError> {
        let ids = ids(deposit)?;
        if ids.is_empty() {
            return Ok(());
        }
        let (path, _guards) = self.directory(deposit.key(), true)?;
        let target = assets::Store::open(&path, true)?;
        let source = assets::Store::open(project, false)?;
        let trash = project.join("assets").join(".trash");
        let raw = serde_json::to_value(&deposit.envelope().draft)
            .map_err(|e| RecoveryError::caused(Category::Corrupt, Stage::Validate, e))?;
        for id in ids {
            let (meta, bytes) = match source.read(&id) {
                Ok(value) => value,
                Err(error)
                    if error
                        .source
                        .as_ref()
                        .is_some_and(|source| source.kind() == std::io::ErrorKind::NotFound) =>
                {
                    // 휴지통 이동은 참조를 지우지 않는다. 편집 복구본도 같은 ID/bytes를
                    // 보존해야 하므로, 활성 package가 없을 때만 검증된 휴지통 namespace를
                    // 조회한다. 활성 package 손상은 휴지통 사본으로 가리지 않는다.
                    assets::Store::open_namespace(&trash)?.read(&id)?
                }
                Err(error) => return Err(error),
            };
            target.put(&meta, &bytes)?;
        }
        // 활성/휴지통 양쪽에서 모은 완성된 복구 namespace를 기준으로 이미지 종류를
        // 다시 검증한다. 휴지통의 이미지 참조도 파일 참조와 같은 보존 계약을 따른다.
        target.validate_image_references(&raw)?;
        self.verify_assets(deposit)
    }
    pub(super) fn verify_assets(&self, deposit: &Deposit) -> Result<(), RecoveryError> {
        let ids = ids(deposit)?;
        if ids.is_empty() {
            return Ok(());
        }
        let (path, _guards) = self.directory(deposit.key(), false)?;
        let store = assets::Store::open(&path, false)?;
        for id in ids {
            store.read(&id)?;
        }
        Ok(())
    }
    pub(crate) fn restore_assets(
        &self,
        deposit: &Deposit,
        project: &Path,
    ) -> Result<(), RecoveryError> {
        let ids = ids(deposit)?;
        if ids.is_empty() {
            return Ok(());
        }
        self.verify_assets(deposit)?;
        let (path, _guards) = self.directory(deposit.key(), false)?;
        let source = assets::Store::open(&path, false)?;
        let target = assets::Store::open(project, true)?;
        for id in ids {
            let (meta, bytes) = source.read(&id)?;
            target.put(&meta, &bytes)?;
        }
        Ok(())
    }
}
impl Deposit {
    pub(crate) fn has_assets(&self) -> Result<bool, RecoveryError> {
        Ok(!ids(self)?.is_empty())
    }
}
