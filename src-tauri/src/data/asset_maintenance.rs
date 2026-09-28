//! M5-4: 저장된 자산의 읽기 전용 검사와 명시적 휴지통 이동/복원.
//! 화면의 검사 결과는 권한이 아니며 mutation 직전에 같은 worker에서 다시 검사한다.
use crate::data::{
    artifact::{decode_template, TemplateLifecycle},
    assets::{self, Store},
    edit_recovery::{error::RecoveryError, model::digest, native},
    project_file::directory::{is_reparse, ProjectDirectory},
    repository::progress,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAX_SOURCE_FILES: usize = 100_000;
const MAX_SOURCE_ENTRIES: usize = 200_000;
const MAX_SOURCE_DIRECTORIES: usize = 25_000;
const MAX_SOURCE_DEPTH: usize = 128;
const MAX_HELD_SOURCE_HANDLES: usize = 100_000;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REFERENCES: usize = 100_000;
const MAX_ACTION_ITEMS: usize = 100;
const TRASH_DIRECTORY: &str = ".trash";
const TRASH_MANIFEST: &str = "trash.json";
const OPERATIONS_DIRECTORY: &str = ".operations";
const OPERATION_SCHEMA: u32 = 1;

#[derive(Clone, Copy)]
struct InspectionLimits {
    files: usize,
    entries: usize,
    directories: usize,
    depth: usize,
    handles: usize,
    file_bytes: usize,
    total_bytes: u64,
    references: usize,
    buffer_bytes: usize,
}

const DEFAULT_INSPECTION_LIMITS: InspectionLimits = InspectionLimits {
    files: MAX_SOURCE_FILES,
    entries: MAX_SOURCE_ENTRIES,
    directories: MAX_SOURCE_DIRECTORIES,
    depth: MAX_SOURCE_DEPTH,
    handles: MAX_HELD_SOURCE_HANDLES,
    file_bytes: MAX_SOURCE_BYTES,
    total_bytes: MAX_TOTAL_SOURCE_BYTES,
    references: MAX_REFERENCES,
    buffer_bytes: MAX_SOURCE_BYTES,
};

#[cfg(test)]
thread_local! {
    static TEST_INSPECTION_LIMITS: std::cell::Cell<Option<InspectionLimits>> = const { std::cell::Cell::new(None) };
}

fn inspection_limits() -> InspectionLimits {
    #[cfg(test)]
    if let Some(limits) = TEST_INSPECTION_LIMITS.with(std::cell::Cell::get) {
        return limits;
    }
    DEFAULT_INSPECTION_LIMITS
}

#[cfg(test)]
fn with_inspection_limits<T>(limits: InspectionLimits, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_INSPECTION_LIMITS.with(|value| value.set(None));
        }
    }
    TEST_INSPECTION_LIMITS.with(|value| value.set(Some(limits)));
    let _reset = Reset;
    run()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssetStatus {
    Used,
    Unused,
    InTrash,
    Missing,
    Corrupt,
    Uncertain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssetRow {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) size: Option<u64>,
    pub(crate) status: AssetStatus,
    pub(crate) reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) reclaimable_size: Option<u64>,
    pub(crate) moved_at_utc: Option<String>,
    pub(crate) protected: bool,
    pub(crate) reason: Option<&'static str>,
    #[serde(skip)]
    sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeletedTemplateRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) removable: bool,
    pub(crate) reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentIssueRow {
    pub(crate) document_id: String,
    pub(crate) reasons: Vec<&'static str>,
    pub(crate) related_resource_ids: Vec<String>,
    pub(crate) related_template_ids: Vec<String>,
    pub(crate) targets: Vec<DocumentIssueTargets>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentIssueTargets {
    pub(crate) reason: &'static str,
    pub(crate) resource_ids: Vec<String>,
    pub(crate) template_ids: Vec<String>,
}

#[derive(Default)]
struct DocumentReferences {
    assets: BTreeSet<String>,
    templates: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Inspection {
    pub(crate) token: String,
    pub(crate) observed_at_utc: String,
    pub(crate) complete: bool,
    pub(crate) scanned_files: usize,
    pub(crate) referenced_assets: usize,
    pub(crate) used_assets: usize,
    pub(crate) unused_assets: usize,
    pub(crate) missing_assets: usize,
    pub(crate) corrupt_assets: usize,
    pub(crate) uncertain_assets: usize,
    pub(crate) rows: Vec<AssetRow>,
    pub(crate) trash: Vec<TrashRow>,
    pub(crate) deleted_templates: Vec<DeletedTemplateRow>,
    pub(crate) document_issues: Vec<DocumentIssueRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionFailure {
    pub(crate) id: String,
    pub(crate) category: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionResult {
    pub(crate) action: &'static str,
    pub(crate) completed: Vec<String>,
    pub(crate) failures: Vec<ActionFailure>,
    pub(crate) partial: bool,
    pub(crate) completed_count: usize,
    pub(crate) cleanup_required: Vec<String>,
    pub(crate) inspection: Inspection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IntentAction {
    TrashMove,
    TrashRestore,
    TrashPurge,
    TemplatePurge,
    DocumentPurge,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    schema_version: u32,
    operation_id: String,
    action: IntentAction,
    item_id: String,
    metadata: Option<assets::Metadata>,
    source_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    InvalidInput,
    Incomplete,
    Stale,
    Conflict,
    Io,
    Corrupt,
    TooLarge,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct Error {
    category: Category,
}
impl Error {
    fn new(category: Category) -> Self {
        Self { category }
    }
    fn io(error: io::Error) -> Self {
        let _kind = error.kind();
        Self::new(Category::Io)
    }
    pub(crate) fn category(&self) -> Category {
        self.category
    }
    pub(crate) fn invalid_input() -> Self {
        Self::new(Category::InvalidInput)
    }
    pub(crate) fn diagnostic_export() -> Self {
        Self::new(Category::Io)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "asset maintenance failed ({:?})", self.category)
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::io(value)
    }
}
impl From<RecoveryError> for Error {
    fn from(_: RecoveryError) -> Self {
        Self::new(Category::Corrupt)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrashManifest {
    schema_version: u32,
    asset_id: String,
    moved_at_utc: String,
}

struct Evidence {
    inspection: Inspection,
    // mutation 동안 저장 source가 교체되지 않게 실제 file/directory handle을 유지한다.
    source_files: Vec<SourceFile>,
    _source_directories: Vec<ProjectDirectory>,
    active_metadata: BTreeMap<String, assets::Metadata>,
    trash_metadata: BTreeMap<String, assets::Metadata>,
}

struct SourceFile {
    path: PathBuf,
    _file: File,
}

/// layout canonical commit과 문서 file 삭제 사이의 권한/복구 증거를 함께 보유한다.
/// 생성자는 휴지통 상태와 문서 bytes를 확인하고 delete handle을 연 뒤 intent를 게시한다.
pub(crate) struct DocumentPurge {
    intent: Intent,
    document: crate::data::artifact::DocumentId,
    file: File,
    directory: ProjectDirectory,
}

fn cancelled() -> Result<(), Error> {
    progress::checkpoint(false).map_err(|_| Error::new(Category::Cancelled))
}

#[cfg(test)]
fn process_crash_checkpoint(point: &str) {
    if std::env::var("WB_M545_ASSET_CRASH_POINT").as_deref() != Ok(point) {
        return;
    }
    let ready = std::path::PathBuf::from(
        std::env::var_os("WB_M545_ASSET_CRASH_READY")
            .expect("asset crash checkpoint requires a handshake path"),
    );
    fs::write(ready, point.as_bytes()).expect("asset crash checkpoint signal");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(not(test))]
fn process_crash_checkpoint(_: &str) {}

pub(crate) fn inspect(root: &Path) -> Result<Inspection, Error> {
    Ok(inspect_with_evidence(root)?.inspection)
}

impl Inspection {
    /// 저장되지 않은 editor/import owner가 있으면 저장본만으로 미사용 권한을 만들지 않는다.
    pub(crate) fn mark_uncommitted_protected(&mut self) {
        self.complete = false;
        for row in &mut self.rows {
            if row.status == AssetStatus::Unused {
                row.status = AssetStatus::Uncertain;
                row.reason = Some("active_edit_or_import_owner");
            }
        }
        self.unused_assets = 0;
        self.uncertain_assets = self
            .rows
            .iter()
            .filter(|row| row.status == AssetStatus::Uncertain)
            .count();
    }
}

fn inspect_with_evidence(root: &Path) -> Result<Evidence, Error> {
    let limits = inspection_limits();
    let root_guard = ProjectDirectory::open_root(root)?;
    let mut files = Vec::new();
    let mut directories = vec![root_guard];
    let mut references = BTreeSet::new();
    let mut template_references = BTreeSet::new();
    let mut template_lifecycles = BTreeMap::<String, String>::new();
    let mut document_references = BTreeMap::<String, DocumentReferences>::new();
    let mut source_fingerprints = Vec::new();
    let mut scanned_files = 0usize;
    let mut scanned_entries = 0usize;
    let mut scanned_directories = 0usize;
    let mut total_bytes = 0u64;
    let mut complete = true;

    for namespace in ["templates", "documents", "workspace"] {
        let path = root.join(namespace);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !is_reparse(&metadata) => scan_json_tree(
                root,
                &path,
                &mut references,
                &mut template_references,
                namespace != "templates",
                namespace == "documents",
                namespace == "templates",
                &mut document_references,
                &mut template_lifecycles,
                &mut source_fingerprints,
                &mut files,
                &mut directories,
                &mut scanned_files,
                &mut scanned_entries,
                &mut scanned_directories,
                &mut total_bytes,
                &mut complete,
                limits,
            )?,
            Ok(_) => complete = false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => complete = false,
        }
    }
    let history = root.join(".worldbuild").join("format-history");
    match fs::symlink_metadata(&history) {
        Ok(metadata) if metadata.is_dir() && !is_reparse(&metadata) => scan_json_tree(
            root,
            &history,
            &mut references,
            &mut template_references,
            true,
            false,
            false,
            &mut document_references,
            &mut template_lifecycles,
            &mut source_fingerprints,
            &mut files,
            &mut directories,
            &mut scanned_files,
            &mut scanned_entries,
            &mut scanned_directories,
            &mut total_bytes,
            &mut complete,
            limits,
        )?,
        Ok(_) => complete = false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => complete = false,
    }

    if references.len() > limits.references {
        return Err(Error::new(Category::TooLarge));
    }
    let (mut rows, mut trash, assets_complete, asset_fingerprints, active_metadata, trash_metadata) =
        scan_assets(root, &references)?;
    complete &= assets_complete;
    for row in &mut trash {
        if references.contains(&row.id) {
            row.protected = true;
            row.reason = Some("stored_reference");
        }
    }
    for id in &references {
        if !rows.iter().any(|row| row.id == *id) {
            if let Some(trashed) = trash.iter().find(|row| row.id == *id) {
                rows.push(AssetRow {
                    id: id.clone(),
                    name: Some(trashed.name.clone()),
                    size: Some(trashed.size),
                    status: AssetStatus::InTrash,
                    reason: Some("referenced_asset_in_trash"),
                });
            } else {
                rows.push(AssetRow {
                    id: id.clone(),
                    name: None,
                    size: None,
                    status: AssetStatus::Missing,
                    reason: Some("referenced_asset_missing"),
                });
            }
        }
    }
    if !complete {
        for row in &mut rows {
            if row.status == AssetStatus::Unused {
                row.status = AssetStatus::Uncertain;
                row.reason = Some("reference_scan_incomplete");
            }
        }
    }
    rows.sort_by(|left, right| left.id.cmp(&right.id));
    let deleted_templates = scan_deleted_templates(root, &template_references, complete)?;
    let problem_assets = rows
        .iter()
        .filter_map(|row| match row.status {
            AssetStatus::InTrash => Some((row.id.clone(), "resource_in_trash")),
            AssetStatus::Missing => Some((row.id.clone(), "resource_missing")),
            AssetStatus::Corrupt => Some((row.id.clone(), "resource_corrupt")),
            AssetStatus::Uncertain => Some((row.id.clone(), "resource_uncertain")),
            AssetStatus::Used | AssetStatus::Unused => None,
        })
        .collect::<BTreeMap<_, _>>();
    let deleted_template_ids = deleted_templates
        .iter()
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();
    let document_issues = document_references
        .into_iter()
        .filter_map(|(document_id, references)| {
            let mut targets: BTreeMap<&'static str, (BTreeSet<String>, BTreeSet<String>)> =
                BTreeMap::new();
            let mut related_resource_ids = BTreeSet::new();
            let mut related_template_ids = BTreeSet::new();
            for asset in references.assets {
                if let Some(reason) = problem_assets.get(&asset) {
                    targets.entry(*reason).or_default().0.insert(asset.clone());
                    related_resource_ids.insert(asset);
                }
            }
            for template in references.templates {
                if deleted_template_ids.contains(template.as_str()) {
                    targets
                        .entry("deleted_template")
                        .or_default()
                        .1
                        .insert(template.clone());
                    related_template_ids.insert(template);
                } else if !template_lifecycles.contains_key(&template) {
                    targets
                        .entry("missing_template")
                        .or_default()
                        .1
                        .insert(template.clone());
                    related_template_ids.insert(template);
                }
            }
            (!targets.is_empty()).then(|| DocumentIssueRow {
                document_id,
                reasons: targets.keys().copied().collect(),
                related_resource_ids: related_resource_ids.into_iter().collect(),
                related_template_ids: related_template_ids.into_iter().collect(),
                targets: targets
                    .into_iter()
                    .map(
                        |(reason, (resource_ids, template_ids))| DocumentIssueTargets {
                            reason,
                            resource_ids: resource_ids.into_iter().collect(),
                            template_ids: template_ids.into_iter().collect(),
                        },
                    )
                    .collect(),
            })
        })
        .collect();
    source_fingerprints.extend(asset_fingerprints);
    source_fingerprints.sort();
    let token = digest(source_fingerprints.join("\n").as_bytes());
    let inspection = Inspection {
        token,
        observed_at_utc: crate::data::utc_time::now_utc_milliseconds()
            .map_err(|_| Error::new(Category::Io))?,
        complete,
        scanned_files,
        referenced_assets: references.len(),
        used_assets: rows
            .iter()
            .filter(|row| row.status == AssetStatus::Used)
            .count(),
        unused_assets: rows
            .iter()
            .filter(|row| row.status == AssetStatus::Unused)
            .count(),
        missing_assets: rows
            .iter()
            .filter(|row| row.status == AssetStatus::Missing)
            .count(),
        corrupt_assets: rows
            .iter()
            .filter(|row| row.status == AssetStatus::Corrupt)
            .count(),
        uncertain_assets: rows
            .iter()
            .filter(|row| row.status == AssetStatus::Uncertain)
            .count(),
        rows,
        trash,
        deleted_templates,
        document_issues,
    };
    Ok(Evidence {
        inspection,
        source_files: files,
        _source_directories: directories,
        active_metadata,
        trash_metadata,
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_json_tree(
    scan_root: &Path,
    directory: &Path,
    references: &mut BTreeSet<String>,
    template_references: &mut BTreeSet<String>,
    collect_template_references: bool,
    collect_document_references: bool,
    collect_template_lifecycles: bool,
    document_references: &mut BTreeMap<String, DocumentReferences>,
    template_lifecycles: &mut BTreeMap<String, String>,
    fingerprints: &mut Vec<String>,
    files: &mut Vec<SourceFile>,
    directories: &mut Vec<ProjectDirectory>,
    scanned_files: &mut usize,
    scanned_entries: &mut usize,
    scanned_directories: &mut usize,
    total_bytes: &mut u64,
    complete: &mut bool,
    limits: InspectionLimits,
) -> Result<(), Error> {
    let mut pending = vec![(directory.to_owned(), 0usize)];
    while let Some((current, depth)) = pending.pop() {
        cancelled()?;
        if depth > limits.depth || *scanned_directories >= limits.directories {
            return Err(Error::new(Category::TooLarge));
        }
        let guard = ProjectDirectory::open_root(&current)?;
        *scanned_directories += 1;
        if files
            .len()
            .saturating_add(directories.len())
            .saturating_add(1)
            > limits.handles
        {
            return Err(Error::new(Category::TooLarge));
        }
        let mut entries = Vec::new();
        for entry in guard.read_dir()? {
            if *scanned_entries >= limits.entries {
                return Err(Error::new(Category::TooLarge));
            }
            *scanned_entries += 1;
            entries.push(entry?);
        }
        entries.sort_by_key(|entry| entry.file_name());
        let mut child_directories = Vec::new();
        for entry in entries {
            cancelled()?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse(&metadata) {
                *complete = false;
                continue;
            }
            if metadata.is_dir() {
                child_directories.push((path, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                *complete = false;
                continue;
            }
            if metadata.len() > limits.file_bytes as u64 {
                return Err(Error::new(Category::TooLarge));
            }
            if *scanned_files >= limits.files
                || files
                    .len()
                    .saturating_add(directories.len())
                    .saturating_add(2)
                    > limits.handles
            {
                return Err(Error::new(Category::TooLarge));
            }
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| Error::new(Category::TooLarge))?;
            if *total_bytes > limits.total_bytes {
                return Err(Error::new(Category::TooLarge));
            }
            let name = entry
                .file_name()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::new(Category::Corrupt))?;
            let mut file = open_scan_file(&path)?;
            guard.validate_file(&file, &name)?;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            (&mut file)
                .take(limits.buffer_bytes as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > limits.buffer_bytes {
                return Err(Error::new(Category::TooLarge));
            }
            *scanned_files += 1;
            fingerprints.push(format!(
                "source:{}:{}",
                digest(path_key(scan_root, &path).as_bytes()),
                digest(&bytes)
            ));
            match crate::data::json::parse_strict_json_object(&bytes) {
                Ok(value) => {
                    let asset_ids = match assets::references(&value) {
                        Ok(ids) => {
                            references.extend(ids.iter().cloned());
                            Some(ids)
                        }
                        Err(_) => {
                            *complete = false;
                            None
                        }
                    };
                    if collect_document_references
                        && value.get("artifactType").and_then(|value| value.as_str())
                            == Some("document")
                    {
                        if let Some(document_id) = value
                            .get("documentId")
                            .and_then(|value| value.as_str())
                            .filter(|id| id.parse::<crate::data::artifact::DocumentId>().is_ok())
                        {
                            let entry = document_references
                                .entry(document_id.to_owned())
                                .or_default();
                            if let Some(ids) = asset_ids {
                                entry.assets.extend(ids);
                            }
                            if let Some(template_id) = value
                                .get("templateId")
                                .and_then(|value| value.as_str())
                                .filter(|id| {
                                    id.parse::<crate::data::artifact::TemplateId>().is_ok()
                                })
                            {
                                entry.templates.insert(template_id.to_owned());
                            }
                        }
                    }
                    if collect_template_lifecycles
                        && value.get("artifactType").and_then(|value| value.as_str())
                            == Some("template")
                    {
                        if let (Some(template_id), Some(lifecycle)) = (
                            value.get("templateId").and_then(|value| value.as_str()),
                            value.get("lifecycle").and_then(|value| value.as_str()),
                        ) {
                            if template_id
                                .parse::<crate::data::artifact::TemplateId>()
                                .is_ok()
                            {
                                template_lifecycles
                                    .insert(template_id.to_owned(), lifecycle.to_owned());
                            }
                        }
                    }
                    if collect_template_references {
                        collect_explicit_template_references(&value, template_references);
                    }
                }
                Err(_) => *complete = false,
            }
            if references.len() > limits.references {
                return Err(Error::new(Category::TooLarge));
            }
            files.push(SourceFile { path, _file: file });
        }
        directories.push(guard);
        // stack에서도 정렬된 방문 순서를 유지한다.
        for child in child_directories.into_iter().rev() {
            pending.push(child);
        }
    }
    Ok(())
}

fn collect_explicit_template_references(
    value: &serde_json::Value,
    references: &mut BTreeSet<String>,
) {
    match value {
        serde_json::Value::Object(values) => {
            if let Some(id) = values.get("templateId").and_then(|value| value.as_str()) {
                if id.parse::<crate::data::artifact::TemplateId>().is_ok() {
                    references.insert(id.to_owned());
                }
            }
            for value in values.values() {
                collect_explicit_template_references(value, references);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_explicit_template_references(value, references);
            }
        }
        _ => {}
    }
}

fn scan_deleted_templates(
    root: &Path,
    references: &BTreeSet<String>,
    complete: bool,
) -> Result<Vec<DeletedTemplateRow>, Error> {
    let path = root.join("templates");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    let guard = ProjectDirectory::open_root(&path)?;
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.len() > MAX_SOURCE_FILES {
        return Err(Error::new(Category::TooLarge));
    }
    let mut rows = Vec::new();
    for entry in entries {
        cancelled()?;
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::new(Category::Corrupt))?;
        let Some(id) = name.strip_suffix(".json") else {
            return Err(Error::new(Category::Corrupt));
        };
        if id.parse::<crate::data::artifact::TemplateId>().is_err() {
            return Err(Error::new(Category::Corrupt));
        }
        let mut file = open_scan_file(&entry.path())?;
        guard.validate_file(&file, &name)?;
        let size = file.metadata()?.len();
        if size > MAX_SOURCE_BYTES as u64 {
            return Err(Error::new(Category::TooLarge));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.read_to_end(&mut bytes)?;
        let template = decode_template(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
        if template.template_id().to_string() != id {
            return Err(Error::new(Category::Corrupt));
        }
        if template.lifecycle() == TemplateLifecycle::Deleted {
            let referenced = references.contains(id);
            rows.push(DeletedTemplateRow {
                id: id.to_owned(),
                name: template.name().to_owned(),
                size,
                removable: complete && !referenced,
                reason: if !complete {
                    Some("reference_scan_incomplete")
                } else if referenced {
                    Some("stored_reference")
                } else {
                    None
                },
            });
        }
    }
    rows.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(rows)
}

fn path_key(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(windows)]
fn open_scan_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        // 읽는 동안 내용 교체/쓰기는 막되 parent의 handle 기반 이동은 허용한다.
        .share_mode(0x0000_0001 | 0x0000_0004)
        .custom_flags(0x0020_0000);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_reparse(&metadata) || native::link_count(&file)? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsafe scan file",
        ));
    }
    Ok(file)
}

#[cfg(not(windows))]
fn open_scan_file(_: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "guarded asset inspection requires Windows",
    ))
}

fn scan_assets(
    root: &Path,
    references: &BTreeSet<String>,
) -> Result<
    (
        Vec<AssetRow>,
        Vec<TrashRow>,
        bool,
        Vec<String>,
        BTreeMap<String, assets::Metadata>,
        BTreeMap<String, assets::Metadata>,
    ),
    Error,
> {
    let assets_path = root.join("assets");
    let metadata = match fs::symlink_metadata(&assets_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((
                Vec::new(),
                Vec::new(),
                true,
                vec!["assets:none".into()],
                BTreeMap::new(),
                BTreeMap::new(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    let guard = ProjectDirectory::open_root(&assets_path)?;
    let store = Store::open(root, false)?;
    let mut rows = Vec::new();
    let mut fingerprints = Vec::new();
    let mut active_metadata = BTreeMap::new();
    let mut complete = true;
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.len() > MAX_SOURCE_FILES {
        return Err(Error::new(Category::TooLarge));
    }
    for entry in entries {
        cancelled()?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if id == OPERATIONS_DIRECTORY {
            continue;
        }
        if id == TRASH_DIRECTORY {
            continue;
        }
        if !crate::data::media::valid_id(&id) {
            complete = false;
            fingerprints.push(format!("invalid:{}", digest(id.as_bytes())));
            continue;
        }
        match store.read(&id) {
            Ok((metadata, bytes)) => {
                let metadata_fingerprint =
                    serde_json::to_vec(&metadata).map_err(|_| Error::new(Category::Corrupt))?;
                fingerprints.push(format!("asset:{id}:{}", digest(&metadata_fingerprint)));
                active_metadata.insert(id.clone(), metadata.clone());
                rows.push(AssetRow {
                    id: id.clone(),
                    name: Some(metadata.name),
                    size: Some(metadata.size),
                    status: if references.contains(&id) {
                        AssetStatus::Used
                    } else {
                        AssetStatus::Unused
                    },
                    reason: references.contains(&id).then_some("stored_reference"),
                });
                drop(bytes);
            }
            Err(_) => {
                fingerprints.push(format!("corrupt:{id}"));
                rows.push(AssetRow {
                    id,
                    name: None,
                    size: None,
                    status: AssetStatus::Corrupt,
                    reason: Some("asset_validation_failed"),
                });
            }
        }
    }
    let (trash, trash_metadata) = list_trash_path(&assets_path.join(TRASH_DIRECTORY))?;
    for row in &trash {
        fingerprints.push(format!(
            "trash:{}:{}:{}:{}:{}:{}",
            row.id,
            row.size,
            row.reclaimable_size.unwrap_or_default(),
            row.sha256,
            digest(row.name.as_bytes()),
            digest(row.moved_at_utc.as_deref().unwrap_or_default().as_bytes())
        ));
    }
    Ok((
        rows,
        trash,
        complete,
        fingerprints,
        active_metadata,
        trash_metadata,
    ))
}

fn list_trash_path(
    path: &Path,
) -> Result<(Vec<TrashRow>, BTreeMap<String, assets::Metadata>), Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), BTreeMap::new()))
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    let guard = ProjectDirectory::open_root(path)?;
    let store = Store::open_namespace(path)?;
    let mut rows = Vec::new();
    let mut metadata_by_id = BTreeMap::new();
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.len() > MAX_SOURCE_FILES {
        return Err(Error::new(Category::TooLarge));
    }
    for entry in entries {
        cancelled()?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if id == OPERATIONS_DIRECTORY {
            continue;
        }
        if !crate::data::media::valid_id(&id) {
            return Err(Error::new(Category::Corrupt));
        }
        let (metadata, _) = store.read(&id)?;
        let moved_at_utc = Some(read_manifest(&entry.path(), &id)?);
        let reclaimable_size = package_size(&entry.path(), &metadata)?;
        metadata_by_id.insert(id.clone(), metadata.clone());
        rows.push(TrashRow {
            id,
            name: metadata.name,
            size: metadata.size,
            reclaimable_size: Some(reclaimable_size),
            moved_at_utc,
            protected: false,
            reason: None,
            sha256: metadata.sha256,
        });
    }
    Ok((rows, metadata_by_id))
}

fn package_size(path: &Path, metadata: &assets::Metadata) -> Result<u64, Error> {
    let guard = ProjectDirectory::open_root(path)?;
    let mut total = 0u64;
    let content_name = assets::filename(metadata);
    let allowed = ["metadata.json", content_name.as_str(), TRASH_MANIFEST]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut observed = BTreeSet::new();
    for entry in guard.read_dir()? {
        let entry = entry?;
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::new(Category::Corrupt))?;
        if !allowed.contains(name.as_str()) || !observed.insert(name.clone()) {
            return Err(Error::new(Category::Corrupt));
        }
        let file = open_scan_file(&entry.path())?;
        guard.validate_file(&file, &name)?;
        total = total
            .checked_add(file.metadata()?.len())
            .ok_or_else(|| Error::new(Category::TooLarge))?;
    }
    if observed.len() != allowed.len() {
        return Err(Error::new(Category::Corrupt));
    }
    Ok(total)
}

fn read_manifest(path: &Path, expected_id: &str) -> Result<String, Error> {
    let guard = ProjectDirectory::open_root(path)?;
    let mut file = native::open(&path.join(TRASH_MANIFEST), false, false)?;
    guard.validate_file(&file, TRASH_MANIFEST)?;
    if file.metadata()?.len() > 4096 {
        return Err(Error::new(Category::Corrupt));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let manifest: TrashManifest =
        serde_json::from_slice(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
    if manifest.schema_version != 1 || manifest.asset_id != expected_id {
        return Err(Error::new(Category::Corrupt));
    }
    Ok(manifest.moved_at_utc)
}

fn ensure_trash(assets_path: &Path) -> Result<(PathBuf, ProjectDirectory), Error> {
    let path = assets_path.join(TRASH_DIRECTORY);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok((
        path.clone(),
        ProjectDirectory::open_move_destination(&path)?,
    ))
}

fn operation_directory(
    root: &Path,
    create: bool,
) -> Result<Option<(PathBuf, ProjectDirectory)>, Error> {
    let root_guard = ProjectDirectory::open_mutable_root(root)?;
    let assets = root.join("assets");
    if create {
        match fs::create_dir(&assets) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let assets_metadata = match fs::symlink_metadata(&assets) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !assets_metadata.is_dir() || is_reparse(&assets_metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    let assets_guard = ProjectDirectory::open_mutable_root(&assets)?;
    let trash = assets.join(TRASH_DIRECTORY);
    if create {
        match fs::create_dir(&trash) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let trash_metadata = match fs::symlink_metadata(&trash) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !trash_metadata.is_dir() || is_reparse(&trash_metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    let trash_guard = ProjectDirectory::open_mutable_root(&trash)?;
    let operations = trash.join(OPERATIONS_DIRECTORY);
    if create {
        match fs::create_dir(&operations) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let operations_metadata = match fs::symlink_metadata(&operations) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !operations_metadata.is_dir() || is_reparse(&operations_metadata) {
        return Err(Error::new(Category::Corrupt));
    }
    root_guard.validate()?;
    assets_guard.validate()?;
    trash_guard.validate()?;
    Ok(Some((
        operations.clone(),
        ProjectDirectory::open_mutable_root(&operations)?,
    )))
}

fn begin_intent(
    root: &Path,
    action: IntentAction,
    item_id: &str,
    metadata: Option<assets::Metadata>,
    source_sha256: Option<String>,
) -> Result<Intent, Error> {
    let (path, guard) = operation_directory(root, true)?.ok_or_else(|| Error::new(Category::Io))?;
    let operation_id = uuid::Uuid::new_v4().hyphenated().to_string();
    let intent = Intent {
        schema_version: OPERATION_SCHEMA,
        operation_id: operation_id.clone(),
        action,
        item_id: item_id.to_owned(),
        metadata,
        source_sha256,
    };
    let bytes = serde_json::to_vec(&intent).map_err(|_| Error::new(Category::Corrupt))?;
    let temp = format!(".{operation_id}.tmp");
    let mut file = native::open(&path.join(&temp), true, true)?;
    let result = (|| -> Result<(), Error> {
        guard.validate_file(&file, &temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        native::publish(&file, &format!("{operation_id}.json"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = native::cleanup(&file);
    }
    result.map(|()| intent)
}

fn finish_intent(root: &Path, intent: &Intent) -> Result<(), Error> {
    #[cfg(test)]
    if TEST_FINISH_FAILURE.with(|selected| selected.get() == Some(intent.action)) {
        TEST_FINISH_FAILURE.with(|selected| selected.set(None));
        return Err(Error::new(Category::Io));
    }
    let (path, guard) =
        operation_directory(root, false)?.ok_or_else(|| Error::new(Category::Corrupt))?;
    let name = format!("{}.json", intent.operation_id);
    let file = native::open_for_discard(&path.join(&name))?;
    guard.validate_file(&file, &name)?;
    native::cleanup(&file)?;
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_FINISH_FAILURE: std::cell::Cell<Option<IntentAction>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn fail_next_finish(action: IntentAction) {
    TEST_FINISH_FAILURE.with(|selected| selected.set(Some(action)));
}

pub(crate) fn has_pending(root: &Path) -> Result<bool, Error> {
    let Some((_, guard)) = operation_directory(root, false)? else {
        return Ok(false);
    };
    Ok(guard.read_dir()?.next().transpose()?.is_some())
}

pub(crate) fn recover_pending(root: &Path) -> Result<(), Error> {
    let Some((_path, guard)) = operation_directory(root, false)? else {
        return Ok(());
    };
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.len() > MAX_SOURCE_FILES {
        return Err(Error::new(Category::TooLarge));
    }
    for entry in entries {
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::new(Category::Corrupt))?;
        let operation_id = name
            .strip_suffix(".json")
            .ok_or_else(|| Error::new(Category::Corrupt))?;
        if uuid::Uuid::parse_str(operation_id).is_err() {
            return Err(Error::new(Category::Corrupt));
        }
        let mut file = native::open(&entry.path(), false, false)?;
        guard.validate_file(&file, &name)?;
        if file.metadata()?.len() > 16 * 1024 {
            return Err(Error::new(Category::TooLarge));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let intent: Intent =
            serde_json::from_slice(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
        if intent.schema_version != OPERATION_SCHEMA || intent.operation_id != operation_id {
            return Err(Error::new(Category::Corrupt));
        }
        drop(file);
        recover_intent(root, &intent)?;
        finish_intent(root, &intent)?;
    }
    Ok(())
}

fn recover_intent(root: &Path, intent: &Intent) -> Result<(), Error> {
    match intent.action {
        IntentAction::TrashMove => recover_move(root, intent),
        IntentAction::TrashRestore => recover_restore(root, intent),
        IntentAction::TrashPurge => recover_purge(root, intent),
        IntentAction::TemplatePurge => recover_template_purge(root, intent),
        IntentAction::DocumentPurge => recover_document_purge(root, intent),
    }
}

fn document_layout_state(
    root: &Path,
    document: crate::data::artifact::DocumentId,
) -> Result<Option<crate::data::artifact::layout::LayoutState>, Error> {
    let workspace = root.join("workspace");
    let guard = ProjectDirectory::open_root(&workspace)?;
    let path = workspace.join("document-layout.json");
    let mut file = native::open(&path, false, false)?;
    guard.validate_file(&file, "document-layout.json")?;
    let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
    guard.validate()?;
    let layout =
        crate::data::artifact::decode_layout(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
    Ok(layout.nodes.get(&document).map(|node| node.state))
}

fn recover_document_purge(root: &Path, intent: &Intent) -> Result<(), Error> {
    let document: crate::data::artifact::DocumentId = intent
        .item_id
        .parse()
        .map_err(|_| Error::new(Category::Corrupt))?;
    if document.to_string() != intent.item_id || intent.metadata.is_some() {
        return Err(Error::new(Category::Corrupt));
    }
    let expected = intent
        .source_sha256
        .as_deref()
        .ok_or_else(|| Error::new(Category::Corrupt))?;
    let documents = root.join("documents");
    let filename = format!("{document}.json");
    let path = documents.join(&filename);
    match document_layout_state(root, document)? {
        // canonical transaction이 rollback된 경우다. 문서와 intent만 검증하고 작업을
        // 미적용으로 끝낸다. 활성 상태는 원래 승인 집합이 아니므로 거절한다.
        Some(crate::data::artifact::layout::LayoutState::Trashed) => {
            let guard = ProjectDirectory::open_root(&documents)?;
            let mut file = native::open(&path, false, false)?;
            guard.validate_file(&file, &filename)?;
            let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
            let value = crate::data::artifact::decode_document(&bytes)
                .map_err(|_| Error::new(Category::Corrupt))?;
            if value.document_id() != document || digest(&bytes) != expected {
                return Err(Error::new(Category::Conflict));
            }
            Ok(())
        }
        Some(crate::data::artifact::layout::LayoutState::Active) => {
            Err(Error::new(Category::Conflict))
        }
        None => match native::open_for_discard_shared_read(&path) {
            Ok(mut file) => {
                let guard = ProjectDirectory::open_mutable_root(&documents)?;
                guard.validate_file(&file, &filename)?;
                let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
                let value = crate::data::artifact::decode_document(&bytes)
                    .map_err(|_| Error::new(Category::Corrupt))?;
                if value.document_id() != document || digest(&bytes) != expected {
                    return Err(Error::new(Category::Conflict));
                }
                native::cleanup(&file)?;
                drop(file);
                guard.validate()?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

pub(crate) fn begin_document_purge(root: &Path, id: &str) -> Result<DocumentPurge, Error> {
    let document: crate::data::artifact::DocumentId =
        id.parse().map_err(|_| Error::new(Category::InvalidInput))?;
    if document.to_string() != id {
        return Err(Error::new(Category::InvalidInput));
    }
    if document_layout_state(root, document)?
        != Some(crate::data::artifact::layout::LayoutState::Trashed)
    {
        return Err(Error::new(Category::Conflict));
    }
    let documents = root.join("documents");
    let directory = ProjectDirectory::open_mutable_root(&documents)?;
    let filename = format!("{document}.json");
    let path = documents.join(&filename);
    let mut file = native::open_for_discard_shared_read(&path)?;
    directory.validate_file(&file, &filename)?;
    let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
    let value = crate::data::artifact::decode_document(&bytes)
        .map_err(|_| Error::new(Category::Corrupt))?;
    if value.document_id() != document {
        return Err(Error::new(Category::Conflict));
    }
    let source_sha256 = digest(&bytes);
    let intent = begin_intent(
        root,
        IntentAction::DocumentPurge,
        id,
        None,
        Some(source_sha256),
    )?;
    Ok(DocumentPurge {
        intent,
        document,
        file,
        directory,
    })
}

/// `committed`는 canonical layout transaction의 실제 결과다. false면 source를
/// 건드리지 않고 intent만 정리한다. true면 layout 부재를 다시 확인한 뒤 같은
/// delete handle로 문서를 제거한다. 반환값은 cleanup 재시도 필요 여부다.
pub(crate) fn finish_document_purge(
    root: &Path,
    mut purge: DocumentPurge,
    committed: bool,
) -> Result<bool, Error> {
    if !committed {
        return finish_intent(root, &purge.intent).map(|()| false);
    }
    if document_layout_state(root, purge.document)?.is_some() {
        return Err(Error::new(Category::Stale));
    }
    let filename = format!("{}.json", purge.document);
    purge.directory.validate_file(&purge.file, &filename)?;
    purge.file.seek(SeekFrom::Start(0))?;
    let bytes = read_opened(&mut purge.file, MAX_SOURCE_BYTES)?;
    let expected = purge
        .intent
        .source_sha256
        .as_deref()
        .ok_or_else(|| Error::new(Category::Corrupt))?;
    if digest(&bytes) != expected {
        return Err(Error::new(Category::Stale));
    }
    native::cleanup(&purge.file)?;
    drop(purge.file);
    purge.directory.validate()?;
    process_crash_checkpoint("after_document_delete");
    Ok(finish_intent(root, &purge.intent).is_err())
}

fn matches_intent(store: &Store, intent: &Intent) -> Result<bool, Error> {
    let expected = intent
        .metadata
        .as_ref()
        .ok_or_else(|| Error::new(Category::Corrupt))?;
    let (metadata, bytes) = store.read(&intent.item_id)?;
    Ok(&metadata == expected && digest(&bytes) == expected.sha256)
}

fn exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_reparse(&metadata) {
                Err(Error::new(Category::Corrupt))
            } else {
                Ok(true)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn recover_move(root: &Path, intent: &Intent) -> Result<(), Error> {
    let assets_path = root.join("assets");
    let trash_path = assets_path.join(TRASH_DIRECTORY);
    let active = exists(&assets_path.join(&intent.item_id))?;
    let trashed = exists(&trash_path.join(&intent.item_id))?;
    match (active, trashed) {
        (true, false) => {
            let store = Store::open(root, false)?;
            if !matches_intent(&store, intent)? {
                return Err(Error::new(Category::Conflict));
            }
            let (_, _, source) = store.read_owned(&intent.item_id)?;
            let assets_guard = ProjectDirectory::open_mutable_root(&assets_path)?;
            let (_, trash_guard) = ensure_trash(&assets_path)?;
            assets_guard.validate()?;
            source.rename_owned_into(&trash_guard, &intent.item_id)?;
            publish_manifest(&trash_path.join(&intent.item_id), &intent.item_id)?;
        }
        (false, true) => {
            let store = Store::open_namespace(&trash_path)?;
            if !matches_intent(&store, intent)? {
                return Err(Error::new(Category::Conflict));
            }
            match read_manifest(&trash_path.join(&intent.item_id), &intent.item_id) {
                Ok(_) => {}
                Err(error) if !exists(&trash_path.join(&intent.item_id).join(TRASH_MANIFEST))? => {
                    let _ = error;
                    publish_manifest(&trash_path.join(&intent.item_id), &intent.item_id)?;
                }
                Err(error) => return Err(error),
            }
        }
        _ => return Err(Error::new(Category::Conflict)),
    }
    Ok(())
}

fn recover_restore(root: &Path, intent: &Intent) -> Result<(), Error> {
    let assets_path = root.join("assets");
    let trash_path = assets_path.join(TRASH_DIRECTORY);
    let active = exists(&assets_path.join(&intent.item_id))?;
    let trashed = exists(&trash_path.join(&intent.item_id))?;
    match (active, trashed) {
        (false, true) => {
            let store = Store::open_namespace(&trash_path)?;
            if !matches_intent(&store, intent)? {
                return Err(Error::new(Category::Conflict));
            }
            let (_, _, source) = store.read_owned(&intent.item_id)?;
            let source_path = trash_path.join(&intent.item_id);
            match native::open_for_discard(&source_path.join(TRASH_MANIFEST)) {
                Ok(file) => {
                    source.validate_file(&file, TRASH_MANIFEST)?;
                    native::cleanup(&file)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let assets_guard = ProjectDirectory::open_move_destination(&assets_path)?;
            source.rename_owned_into(&assets_guard, &intent.item_id)?;
        }
        (true, false) => {
            let store = Store::open(root, false)?;
            if !matches_intent(&store, intent)? {
                return Err(Error::new(Category::Conflict));
            }
        }
        _ => return Err(Error::new(Category::Conflict)),
    }
    Ok(())
}

pub(crate) fn move_to_trash(
    root: &Path,
    expected_token: &str,
    ids: &[String],
) -> Result<ActionResult, Error> {
    if ids.is_empty()
        || ids.len() > MAX_ACTION_ITEMS
        || ids.iter().any(|id| !crate::data::media::valid_id(id))
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(Error::new(Category::InvalidInput));
    }
    let evidence = inspect_with_evidence(root)?;
    if !evidence.inspection.complete {
        return Err(Error::new(Category::Incomplete));
    }
    if evidence.inspection.token != expected_token {
        return Err(Error::new(Category::Stale));
    }
    let allowed = evidence
        .inspection
        .rows
        .iter()
        .filter(|row| row.status == AssetStatus::Unused)
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.iter().any(|id| !allowed.contains(id.as_str())) {
        return Err(Error::new(Category::Conflict));
    }

    let assets_path = root.join("assets");
    let assets_guard = ProjectDirectory::open_mutable_root(&assets_path)?;
    let (trash_path, trash_guard) = ensure_trash(&assets_path)?;
    let store = Store::open(root, false)?;
    let mut completed = Vec::new();
    let mut failures = Vec::new();
    let mut cleanup_required = Vec::new();
    let mut expected = BTreeMap::new();
    for id in ids {
        cancelled()?;
        let expected_metadata = evidence.active_metadata.get(id);
        let verified =
            store
                .read_owned(id)
                .map_err(Error::from)
                .and_then(|(metadata, bytes, package)| {
                    if expected_metadata != Some(&metadata) {
                        return Err(Error::new(Category::Stale));
                    }
                    Ok((metadata, bytes, package))
                });
        match verified.and_then(|(metadata, bytes, package)| {
            move_one(
                root,
                &assets_guard,
                &trash_path,
                &trash_guard,
                id,
                metadata,
                bytes,
                package,
            )
        }) {
            Ok((value, cleanup)) => {
                completed.push(id.clone());
                expected.insert(id.clone(), value);
                if cleanup {
                    cleanup_required.push(id.clone());
                }
            }
            Err(error) => failures.push(ActionFailure {
                id: id.clone(),
                category: category_name(error.category()),
            }),
        }
    }
    drop((store, trash_guard, assets_guard));
    drop(evidence);
    match Store::open_namespace(&trash_path) {
        Ok(store) => {
            for (id, (metadata, bytes)) in &expected {
                match store.read(id) {
                    Ok((actual_metadata, actual_bytes))
                        if &actual_metadata == metadata && &actual_bytes == bytes => {}
                    _ => failures.push(ActionFailure {
                        id: id.clone(),
                        category: "post_move_verification_failed",
                    }),
                }
            }
        }
        Err(_) => failures.extend(expected.keys().map(|id| ActionFailure {
            id: id.clone(),
            category: "post_move_verification_failed",
        })),
    }
    let inspection = inspect(root)?;
    Ok(ActionResult {
        action: "trash_move",
        partial: !completed.is_empty() && !failures.is_empty(),
        completed_count: completed.len(),
        completed,
        failures,
        cleanup_required,
        inspection,
    })
}

fn move_one(
    root: &Path,
    assets_guard: &ProjectDirectory,
    trash_path: &Path,
    trash_guard: &ProjectDirectory,
    id: &str,
    metadata: assets::Metadata,
    bytes: Vec<u8>,
    source: ProjectDirectory,
) -> Result<((assets::Metadata, Vec<u8>), bool), Error> {
    if fs::symlink_metadata(trash_path.join(id)).is_ok() {
        return Err(Error::new(Category::Conflict));
    }
    let intent = begin_intent(
        root,
        IntentAction::TrashMove,
        id,
        Some(metadata.clone()),
        None,
    )?;
    assets_guard.validate()?;
    trash_guard.validate()?;
    let result = source
        .rename_owned_into(trash_guard, id)
        .map_err(Error::from)
        .and_then(|()| {
            process_crash_checkpoint("after_asset_rename");
            cancelled()?;
            publish_manifest(&trash_path.join(id), id)?;
            process_crash_checkpoint("after_trash_manifest");
            Ok(())
        });
    let cleanup_required = if let Err(error) = result {
        recover_intent(root, &intent)?;
        let cleanup = finish_intent(root, &intent).is_err();
        let _ = error;
        cleanup
    } else {
        finish_intent(root, &intent).is_err()
    };
    Ok(((metadata, bytes), cleanup_required))
}

fn publish_manifest(path: &Path, id: &str) -> Result<(), Error> {
    let guard = ProjectDirectory::open_root(path)?;
    let manifest = TrashManifest {
        schema_version: 1,
        asset_id: id.to_owned(),
        moved_at_utc: crate::data::utc_time::now_utc_milliseconds()
            .map_err(|_| Error::new(Category::Io))?,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::new(Category::Corrupt))?;
    let temp = format!(".{}.tmp", uuid::Uuid::new_v4());
    let mut file = native::open(&path.join(&temp), true, true)?;
    let result = (|| -> Result<(), Error> {
        guard.validate_file(&file, &temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        native::publish(&file, TRASH_MANIFEST)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = native::cleanup(&file);
    }
    result
}

pub(crate) fn restore_from_trash(root: &Path, ids: &[String]) -> Result<ActionResult, Error> {
    if ids.is_empty()
        || ids.len() > MAX_ACTION_ITEMS
        || ids.iter().any(|id| !crate::data::media::valid_id(id))
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(Error::new(Category::InvalidInput));
    }
    let evidence = inspect_with_evidence(root)?;
    let assets_path = root.join("assets");
    let trash_path = assets_path.join(TRASH_DIRECTORY);
    let assets_guard = ProjectDirectory::open_move_destination(&assets_path)?;
    let trash_guard = ProjectDirectory::open_mutable_root(&trash_path)?;
    let trash_store = Store::open_namespace(&trash_path)?;
    let mut completed = Vec::new();
    let mut failures = Vec::new();
    let mut cleanup_required = Vec::new();
    let mut expected = BTreeMap::new();
    for id in ids {
        cancelled()?;
        let expected_metadata = evidence.trash_metadata.get(id);
        let verified = trash_store.read_owned(id).map_err(Error::from).and_then(
            |(metadata, bytes, package)| {
                if expected_metadata != Some(&metadata) {
                    return Err(Error::new(Category::Stale));
                }
                Ok((metadata, bytes, package))
            },
        );
        match verified.and_then(|(metadata, bytes, package)| {
            restore_one(
                root,
                &assets_path,
                &assets_guard,
                &trash_path,
                &trash_guard,
                id,
                metadata,
                bytes,
                package,
            )
        }) {
            Ok((value, cleanup)) => {
                completed.push(id.clone());
                expected.insert(id.clone(), value);
                if cleanup {
                    cleanup_required.push(id.clone());
                }
            }
            Err(error) => failures.push(ActionFailure {
                id: id.clone(),
                category: category_name(error.category()),
            }),
        }
    }
    drop((trash_store, trash_guard, assets_guard));
    match Store::open(root, false) {
        Ok(store) => {
            for (id, (metadata, bytes)) in &expected {
                match store.read(id) {
                    Ok((actual_metadata, actual_bytes))
                        if &actual_metadata == metadata && &actual_bytes == bytes => {}
                    _ => failures.push(ActionFailure {
                        id: id.clone(),
                        category: "post_restore_verification_failed",
                    }),
                }
            }
        }
        Err(_) => failures.extend(expected.keys().map(|id| ActionFailure {
            id: id.clone(),
            category: "post_restore_verification_failed",
        })),
    }
    let inspection = inspect(root)?;
    Ok(ActionResult {
        action: "trash_restore",
        partial: !completed.is_empty() && !failures.is_empty(),
        completed_count: completed.len(),
        completed,
        failures,
        cleanup_required,
        inspection,
    })
}

pub(crate) fn rename_asset(
    root: &Path,
    expected_token: &str,
    id: &str,
    requested_name: &str,
) -> Result<ActionResult, Error> {
    if !crate::data::media::valid_id(id) || !assets::safe_name(requested_name) {
        return Err(Error::new(Category::InvalidInput));
    }
    let evidence = inspect_with_evidence(root)?;
    if !evidence.inspection.complete {
        return Err(Error::new(Category::Incomplete));
    }
    if evidence.inspection.token != expected_token {
        return Err(Error::new(Category::Stale));
    }
    let expected = evidence
        .active_metadata
        .get(id)
        .ok_or_else(|| Error::new(Category::Conflict))?;
    let current_extension = Path::new(&expected.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let requested_extension = Path::new(requested_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !current_extension.eq_ignore_ascii_case(requested_extension) {
        return Err(Error::new(Category::InvalidInput));
    }
    let occupied = evidence.active_metadata.iter().any(|(other_id, metadata)| {
        other_id != id && metadata.name.eq_ignore_ascii_case(requested_name)
    });
    if occupied {
        return Err(Error::new(Category::Conflict));
    }
    let store = Store::open(root, false)?;
    let (metadata, bytes, package) = store.read_owned(id)?;
    if &metadata != expected {
        return Err(Error::new(Category::Stale));
    }
    let mut replacement = metadata.clone();
    replacement.name = requested_name.to_owned();
    let encoded = serde_json::to_vec(&replacement).map_err(|_| Error::new(Category::Corrupt))?;
    let temp_name = format!(".metadata-{}.tmp", uuid::Uuid::new_v4());
    let package_path = root.join("assets").join(id);
    let mut temp = native::open(&package_path.join(&temp_name), true, true)?;
    let write = (|| -> Result<(), Error> {
        package.validate_file(&temp, &temp_name)?;
        temp.write_all(&encoded)?;
        temp.sync_all()?;
        crate::data::atomic_file::owned::rename(&temp, "metadata.json", true)?;
        package.validate()?;
        Ok(())
    })();
    if write.is_err() {
        let _ = native::cleanup(&temp);
        return Err(Error::new(Category::Io));
    }
    drop((temp, package, store, evidence));
    let (actual, actual_bytes) = Store::open(root, false)?.read(id)?;
    if actual != replacement || actual_bytes != bytes {
        return Err(Error::new(Category::Conflict));
    }
    let inspection = inspect(root)?;
    Ok(ActionResult {
        action: "asset_rename",
        completed: vec![id.to_owned()],
        failures: Vec::new(),
        partial: false,
        completed_count: 1,
        cleanup_required: Vec::new(),
        inspection,
    })
}

fn restore_one(
    root: &Path,
    assets_path: &Path,
    assets_guard: &ProjectDirectory,
    trash_path: &Path,
    trash_guard: &ProjectDirectory,
    id: &str,
    metadata: assets::Metadata,
    bytes: Vec<u8>,
    source: ProjectDirectory,
) -> Result<((assets::Metadata, Vec<u8>), bool), Error> {
    match fs::symlink_metadata(assets_path.join(id)) {
        Ok(_) => return Err(Error::new(Category::Conflict)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let intent = begin_intent(
        root,
        IntentAction::TrashRestore,
        id,
        Some(metadata.clone()),
        None,
    )?;
    let source_path = trash_path.join(id);
    // 진단 manifest는 원 asset package의 일부가 아니므로 이동 전에 같은 handle로 제거한다.
    match native::open_for_discard(&source_path.join(TRASH_MANIFEST)) {
        Ok(file) => {
            source.validate_file(&file, TRASH_MANIFEST)?;
            if let Err(error) = native::cleanup(&file) {
                let _ = recover_intent(root, &intent);
                return Err(error.into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    process_crash_checkpoint("after_restore_manifest_removed");
    trash_guard.validate()?;
    assets_guard.validate()?;
    let result = source
        .rename_owned_into(assets_guard, id)
        .map_err(Error::from);
    if result.is_ok() {
        process_crash_checkpoint("after_asset_restore_rename");
    }
    let cleanup_required = if let Err(error) = result {
        recover_intent(root, &intent)?;
        let cleanup = finish_intent(root, &intent).is_err();
        let _ = error;
        cleanup
    } else {
        finish_intent(root, &intent).is_err()
    };
    Ok(((metadata, bytes), cleanup_required))
}

fn read_opened(file: &mut File, limit: usize) -> Result<Vec<u8>, Error> {
    if file.metadata()?.len() > limit as u64 {
        return Err(Error::new(Category::TooLarge));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::new(Category::TooLarge));
    }
    Ok(bytes)
}

fn purge_owned_package(
    package_path: &Path,
    package: ProjectDirectory,
    expected: &assets::Metadata,
) -> Result<(), Error> {
    let content_name = assets::filename(expected);
    let allowed = ["metadata.json", content_name.as_str(), TRASH_MANIFEST]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut entries = package.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.iter().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !allowed.contains(name))
    }) {
        return Err(Error::new(Category::Corrupt));
    }
    let mut opened = Vec::new();
    for entry in entries {
        let name = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::new(Category::Corrupt))?;
        let mut file = native::open_for_discard(&entry.path())?;
        package.validate_file(&file, &name)?;
        let bytes = read_opened(
            &mut file,
            if name == content_name {
                assets::MAX_BYTES
            } else {
                16 * 1024
            },
        )?;
        if name == "metadata.json" {
            crate::data::json::parse_strict_json_object(&bytes)
                .map_err(|_| Error::new(Category::Corrupt))?;
            let actual: assets::Metadata =
                serde_json::from_slice(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
            if &actual != expected {
                return Err(Error::new(Category::Conflict));
            }
        } else if name == content_name {
            if bytes.len() as u64 != expected.size || digest(&bytes) != expected.sha256 {
                return Err(Error::new(Category::Conflict));
            }
        } else {
            let manifest: TrashManifest =
                serde_json::from_slice(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
            if manifest.schema_version != 1 || manifest.asset_id != expected.id {
                return Err(Error::new(Category::Conflict));
            }
        }
        opened.push(file);
    }
    let mut first_deleted = false;
    for file in opened {
        native::cleanup(&file)?;
        drop(file);
        if !first_deleted {
            first_deleted = true;
            process_crash_checkpoint("after_purge_file");
        }
    }
    package.validate()?;
    package.delete_owned()?;
    let _ = package_path;
    Ok(())
}

fn recover_purge(root: &Path, intent: &Intent) -> Result<(), Error> {
    let package_path = root
        .join("assets")
        .join(TRASH_DIRECTORY)
        .join(&intent.item_id);
    if !exists(&package_path)? {
        return Ok(());
    }
    let expected = intent
        .metadata
        .as_ref()
        .ok_or_else(|| Error::new(Category::Corrupt))?;
    let package = ProjectDirectory::open_owned_root(&package_path)?;
    purge_owned_package(&package_path, package, expected)
}

pub(crate) fn purge_trash(
    root: &Path,
    expected_token: &str,
    ids: &[String],
    empty: bool,
) -> Result<ActionResult, Error> {
    if (!empty && ids.is_empty())
        || (empty && !ids.is_empty())
        || (!empty && ids.len() > MAX_ACTION_ITEMS)
        || ids.iter().any(|id| !crate::data::media::valid_id(id))
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(Error::new(Category::InvalidInput));
    }
    let evidence = inspect_with_evidence(root)?;
    if !evidence.inspection.complete {
        return Err(Error::new(Category::Incomplete));
    }
    if evidence.inspection.token != expected_token {
        return Err(Error::new(Category::Stale));
    }
    let allowed = evidence
        .inspection
        .trash
        .iter()
        .filter(|row| !row.protected)
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();
    let targets = if empty {
        allowed
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>()
    } else {
        if ids.iter().any(|id| !allowed.contains(id.as_str())) {
            return Err(Error::new(Category::Conflict));
        }
        ids.to_vec()
    };
    let trash_path = root.join("assets").join(TRASH_DIRECTORY);
    let store = Store::open_namespace(&trash_path)?;
    let mut completed = Vec::new();
    let mut completed_count = 0usize;
    let mut failures = Vec::new();
    let mut cleanup_required = Vec::new();
    for id in targets {
        cancelled()?;
        let result = (|| -> Result<bool, Error> {
            let (metadata, _, package) = store.read_owned(&id)?;
            if evidence.trash_metadata.get(&id) != Some(&metadata) {
                return Err(Error::new(Category::Stale));
            }
            let intent = begin_intent(
                root,
                IntentAction::TrashPurge,
                &id,
                Some(metadata.clone()),
                None,
            )?;
            if purge_owned_package(&trash_path.join(&id), package, &metadata).is_err() {
                recover_intent(root, &intent)?;
            }
            Ok(finish_intent(root, &intent).is_err())
        })();
        match result {
            Ok(cleanup) => {
                completed_count += 1;
                if completed.len() < MAX_ACTION_ITEMS {
                    completed.push(id.clone());
                }
                if cleanup && cleanup_required.len() < MAX_ACTION_ITEMS {
                    cleanup_required.push(id);
                }
            }
            Err(error) => failures.push(ActionFailure {
                id,
                category: category_name(error.category()),
            }),
        }
    }
    drop((store, evidence));
    let inspection = inspect(root)?;
    Ok(ActionResult {
        action: "trash_purge",
        partial: completed_count > 0 && !failures.is_empty(),
        completed_count,
        completed,
        failures,
        cleanup_required,
        inspection,
    })
}

fn delete_template_file(root: &Path, intent: &Intent) -> Result<(), Error> {
    let path = root.join("templates");
    let filename = format!("{}.json", intent.item_id);
    let target = path.join(&filename);
    if !exists(&target)? {
        return Ok(());
    }
    let guard = ProjectDirectory::open_mutable_root(&path)?;
    let mut file = native::open_for_discard_shared_read(&target)?;
    guard.validate_file(&file, &filename)?;
    let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
    let actual_sha256 = digest(&bytes);
    if intent.source_sha256.as_deref() != Some(actual_sha256.as_str()) {
        return Err(Error::new(Category::Conflict));
    }
    let template = decode_template(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
    if template.template_id().to_string() != intent.item_id
        || template.lifecycle() != TemplateLifecycle::Deleted
    {
        return Err(Error::new(Category::Conflict));
    }
    native::cleanup(&file)?;
    process_crash_checkpoint("after_template_delete");
    Ok(())
}

fn recover_template_purge(root: &Path, intent: &Intent) -> Result<(), Error> {
    match delete_template_file(root, intent) {
        Ok(()) => Ok(()),
        Err(error) => {
            // A needs-lock file may have become read-only again after a
            // failed purge or restart. If the exact original file is still
            // present, cancel this intent and require an explicit retry with
            // a fresh lock. A changed or unreadable file remains blocked.
            if template_source_unchanged(root, intent)? {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

fn template_source_unchanged(root: &Path, intent: &Intent) -> Result<bool, Error> {
    let templates = root.join("templates");
    let filename = format!("{}.json", intent.item_id);
    let path = templates.join(&filename);
    if !exists(&path)? {
        return Ok(false);
    }
    let guard = ProjectDirectory::open_root(&templates)?;
    let mut file = native::open(&path, false, false)?;
    guard.validate_file(&file, &filename)?;
    let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
    guard.validate()?;
    let matches_source = intent.source_sha256.as_deref() == Some(digest(&bytes).as_str());
    let matches_template = decode_template(&bytes).is_ok_and(|template| {
        template.template_id().to_string() == intent.item_id
            && template.lifecycle() == TemplateLifecycle::Deleted
    });
    Ok(matches_source && matches_template)
}

pub(crate) fn purge_templates(
    root: &Path,
    expected_token: &str,
    ids: &[String],
) -> Result<ActionResult, Error> {
    if ids.is_empty()
        || ids.len() > MAX_ACTION_ITEMS
        || ids
            .iter()
            .any(|id| id.parse::<crate::data::artifact::TemplateId>().is_err())
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(Error::new(Category::InvalidInput));
    }
    let mut evidence = inspect_with_evidence(root)?;
    if !evidence.inspection.complete {
        return Err(Error::new(Category::Incomplete));
    }
    if evidence.inspection.token != expected_token {
        return Err(Error::new(Category::Stale));
    }
    let allowed = evidence
        .inspection
        .deleted_templates
        .iter()
        .filter(|row| row.removable)
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.iter().any(|id| !allowed.contains(id.as_str())) {
        return Err(Error::new(Category::Conflict));
    }
    let mut completed = Vec::new();
    let mut failures = Vec::new();
    let mut cleanup_required = Vec::new();
    for id in ids {
        cancelled()?;
        let result = (|| -> Result<bool, Error> {
            let path = root.join("templates").join(format!("{id}.json"));
            let templates = root.join("templates");
            let guard = ProjectDirectory::open_mutable_root(&templates)?;
            let filename = format!("{id}.json");
            let mut file = native::open_for_discard_shared_read(&path)?;
            guard.validate_file(&file, &filename)?;
            let bytes = read_opened(&mut file, MAX_SOURCE_BYTES)?;
            // 검사 때의 read handle을 닫은 뒤 이미 확보한 delete handle을 유지한다.
            // 다른 source/parent 증거는 mutation이 끝날 때까지 계속 열린 상태다.
            if let Some(index) = evidence
                .source_files
                .iter()
                .position(|source| source.path == path)
            {
                drop(evidence.source_files.swap_remove(index));
            }
            let intent = begin_intent(
                root,
                IntentAction::TemplatePurge,
                id,
                None,
                Some(digest(&bytes)),
            )?;
            let actual_sha256 = digest(&bytes);
            if intent.source_sha256.as_deref() != Some(actual_sha256.as_str()) {
                return Err(Error::new(Category::Stale));
            }
            let template = decode_template(&bytes).map_err(|_| Error::new(Category::Corrupt))?;
            if template.template_id().to_string() != *id
                || template.lifecycle() != TemplateLifecycle::Deleted
            {
                return Err(Error::new(Category::Conflict));
            }
            guard.validate()?;
            let delete_result = native::cleanup(&file);
            drop(file);
            if delete_result.is_err() {
                recover_intent(root, &intent)?;
                if exists(&path)? {
                    // Recovery observed the untouched source and cancelled
                    // the pending operation. Keep it in the trash for a
                    // user-initiated retry instead of reporting success.
                    finish_intent(root, &intent)?;
                    return Err(Error::new(Category::Io));
                }
            }
            process_crash_checkpoint("after_template_delete");
            Ok(finish_intent(root, &intent).is_err())
        })();
        match result {
            Ok(cleanup) => {
                completed.push(id.clone());
                if cleanup {
                    cleanup_required.push(id.clone());
                }
            }
            Err(error) => failures.push(ActionFailure {
                id: id.clone(),
                category: category_name(error.category()),
            }),
        }
    }
    drop(evidence);
    let inspection = inspect(root)?;
    Ok(ActionResult {
        action: "template_purge",
        partial: !completed.is_empty() && !failures.is_empty(),
        completed_count: completed.len(),
        completed,
        failures,
        cleanup_required,
        inspection,
    })
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::InvalidInput => "invalid_input",
        Category::Incomplete => "inspection_incomplete",
        Category::Stale => "inspection_stale",
        Category::Conflict => "destination_conflict",
        Category::Io => "io_failed",
        Category::Corrupt => "validation_failed",
        Category::TooLarge => "limit_exceeded",
        Category::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests;
