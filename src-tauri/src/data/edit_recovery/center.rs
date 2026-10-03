//! 한 페이지의 방문·읽기 예산과 재개 iterator를 분리해 첫 4,096개 밖에도 도달한다.
use super::*;

pub(super) struct Cursor {
    stack: Vec<Level>,
    deferred: Option<fs::DirEntry>,
}
struct Level {
    guard: ProjectDirectory,
    entries: fs::ReadDir,
    parts: Vec<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageEntry {
    pub(crate) row: Entry,
    pub(crate) version: Option<String>,
    pub(crate) created_at_utc: Option<String>,
    pub(crate) artifact: Option<String>,
    pub(crate) name: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub(crate) entries: Vec<PageEntry>,
    pub(crate) next: Option<String>,
    pub(crate) visited_nodes: usize,
    pub(crate) bytes_read: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) legacy: Option<super::legacy::ImportSummary>,
}
fn level(path: &Path, parts: Vec<String>) -> Result<Level, RecoveryError> {
    let guard = guard_directory(path, false, Stage::List)?;
    let entries = guard
        .read_dir()
        .map_err(|e| RecoveryError::io(Stage::List, e))?;
    Ok(Level {
        guard,
        entries,
        parts,
    })
}
impl Store {
    pub(crate) fn page(&mut self, cursor: Option<&str>) -> Result<Page, RecoveryError> {
        self.validate_root()?;
        let resuming = cursor.is_some();
        let (token, mut cursor) = match cursor {
            Some(token) => (
                token.to_owned(),
                self.cursors
                    .remove(token)
                    .ok_or_else(|| RecoveryError::new(Category::Conflict, Stage::List))?,
            ),
            None => {
                if self.cursors.len() >= 8 {
                    return Err(RecoveryError::new(Category::Capacity, Stage::List));
                }
                (
                    uuid::Uuid::new_v4().to_string(),
                    Cursor {
                        stack: vec![level(&self.root, vec![])?],
                        deferred: None,
                    },
                )
            }
        };
        let result = self.page_inner(&mut cursor);
        // 실패에도 이미 도달한 iterator를 소유한다. 같은 token으로 명시적으로 재시도한다.
        match result {
            Ok(mut page) => {
                if !cursor.stack.is_empty() {
                    page.next = Some(token.clone());
                    self.cursors.insert(token, cursor);
                }
                Ok(page)
            }
            Err(e) => {
                // 첫 페이지 실패에는 호출자가 token을 받지 못한다. 새로고침으로 재시도하며 미공개 cursor는 남기지 않는다.
                if resuming {
                    self.cursors.insert(token, cursor);
                }
                Err(e)
            }
        }
    }
    pub(crate) fn close_cursor(&mut self, token: &str) {
        self.cursors.remove(token);
    }
    fn page_inner(&self, cursor: &mut Cursor) -> Result<Page, RecoveryError> {
        let mut page = Page {
            entries: vec![],
            next: None,
            visited_nodes: 0,
            bytes_read: 0,
            legacy: None,
        };
        while page.entries.len() < 128
            && page.visited_nodes < MAX_LIST_ENTRIES
            && page.bytes_read < MAX_LIST_READ_BYTES
        {
            let Some(level) = cursor.stack.last_mut() else {
                break;
            };
            level
                .guard
                .validate()
                .map_err(|e| RecoveryError::io(Stage::List, e))?;
            let entry = match cursor.deferred.take() {
                Some(e) => e,
                None => match level.entries.next() {
                    Some(e) => e.map_err(|e| RecoveryError::io(Stage::List, e))?,
                    None => {
                        cursor.stack.pop();
                        continue;
                    }
                },
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if level.parts.is_empty() && name == ".store-lock" {
                continue;
            }
            if level.parts.len() == 2 && name == "assets" {
                // Referenced assets are validated with their owning deposit.
                continue;
            }
            page.visited_nodes += 1;
            let mut parts = level.parts.clone();
            parts.push(name.clone());
            let mut row = PageEntry {
                row: Entry {
                    locator_fingerprint: model::digest(parts.join("/").as_bytes()),
                    key: None,
                    deposit_id: None,
                    payload_kind: None,
                    payload_digest: None,
                    error: None,
                },
                version: None,
                created_at_utc: None,
                artifact: None,
                name: None,
            };
            if level.parts.len() < 2 {
                let valid = if level.parts.is_empty() {
                    model::valid_digest(&name)
                } else {
                    model::valid_id(&name)
                };
                match if valid {
                    self::level(&entry.path(), parts)
                } else {
                    Err(RecoveryError::new(Category::InvalidId, Stage::List))
                } {
                    Ok(next) => cursor.stack.push(next),
                    Err(e) => {
                        row.row.error = Some(e.dto());
                        page.entries.push(row);
                    }
                }
                continue;
            }
            let result = (|| {
                let generation = generation(&name)
                    .ok_or_else(|| RecoveryError::new(Category::InvalidId, Stage::List))?;
                let key = Key {
                    project_fingerprint: level.parts[0].clone(),
                    draft_id: level.parts[1].clone(),
                    generation,
                };
                row.row.key = Some(key.clone());
                let file = native::open(&entry.path(), false, false)
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?;
                level
                    .guard
                    .validate_file(&file, &name)
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?;
                let size = file
                    .metadata()
                    .map_err(|e| RecoveryError::io(Stage::Read, e))?
                    .len();
                if size > MAX_FILE_BYTES as u64 {
                    return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
                }
                if size as usize > MAX_LIST_READ_BYTES - page.bytes_read {
                    return Ok(false);
                }
                let bytes = bounded_bytes(&file)?;
                page.bytes_read += bytes.len();
                row.version = Some(model::digest(&bytes));
                let deposit = Deposit::decode(&bytes)?;
                if deposit.key() != &key {
                    return Err(RecoveryError::new(Category::InvalidEnvelope, Stage::Read));
                }
                row.row.deposit_id = Some(deposit.envelope().deposit_id.clone());
                row.row.payload_digest = Some(deposit.payload_digest().into());
                row.row.payload_kind = Some(deposit.envelope().draft.kind().into());
                row.created_at_utc = Some(deposit.envelope().created_at_utc.clone());
                row.name = match &deposit.envelope().draft {
                    model::Draft::Template { name, .. }
                    | model::Draft::Document {
                        name: model::Intent::Set(name),
                        ..
                    } => Some(name.clone()),
                    model::Draft::Document {
                        document: Some(id),
                        name: model::Intent::Keep,
                        ..
                    } => original_document_name(deposit.envelope(), id),
                    model::Draft::AdmittedDocument {
                        document, edits, ..
                    }
                    | model::Draft::AdmittedComposite {
                        document, edits, ..
                    } => edits
                        .iter()
                        .rev()
                        .find_map(|edit| match edit {
                            crate::data::edit_input::DocumentEdit::Rename { name } => {
                                Some(name.clone())
                            }
                            _ => None,
                        })
                        .or_else(|| original_document_name(deposit.envelope(), document)),
                    _ => None,
                };
                row.artifact = deposit
                    .envelope()
                    .originals
                    .first()
                    .map(|o| o.artifact_id.clone());
                Ok(true)
            })();
            match result {
                Ok(false) => {
                    cursor.deferred = Some(entry);
                    break;
                }
                Ok(true) => page.entries.push(row),
                Err(e) => {
                    row.row.error = Some(e.dto());
                    page.entries.push(row);
                }
            }
        }
        Ok(page)
    }
    /// 읽을 때 선택한 파일 버전을 exclusive handle로 다시 검증하고 그 한 파일만 지운다.
    pub(crate) fn discard(&mut self, key: &Key, version: &str) -> Result<(), RecoveryError> {
        if !model::valid_digest(version) {
            return Err(RecoveryError::new(Category::InvalidId, Stage::Discard));
        }
        let (path, guards) = self.directory(key, false)?;
        let name = format!("{}.json", key.generation);
        let file = native::open_for_discard(&path.join(&name))
            .map_err(|e| RecoveryError::io(Stage::Discard, e))?;
        guards[1]
            .validate_file(&file, &name)
            .map_err(|e| RecoveryError::io(Stage::Discard, e))?;
        if model::digest(&bounded_bytes(&file)?) != version {
            return Err(RecoveryError::new(Category::Conflict, Stage::Discard));
        }
        self.checkpoint(Stage::Discard)?;
        native::cleanup(&file).map_err(|e| RecoveryError::io(Stage::Discard, e))
    }
}

// The admitted snapshot supplies a label even when the draft did not rename the
// document. Do not read the user's current project or change recovery admission.
fn original_document_name(envelope: &model::Envelope, id: &str) -> Option<String> {
    envelope
        .originals
        .iter()
        .find(|original| {
            original.kind == model::OriginalKind::Document && original.artifact_id == id
        })
        .and_then(|original| {
            crate::data::artifact::decode_document(original.snapshot.as_bytes()).ok()
        })
        .map(|document| document.name().to_owned())
}
fn bounded_bytes(file: &File) -> Result<Vec<u8>, RecoveryError> {
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RecoveryError::io(Stage::Read, e))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(RecoveryError::new(Category::TooLarge, Stage::Read));
    }
    Ok(bytes)
}
