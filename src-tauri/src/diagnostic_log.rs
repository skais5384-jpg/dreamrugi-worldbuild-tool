//! M5-5 제한 진단. caller 문자열이나 오류 Display/Debug를 받지 않고 닫힌 안전 필드만 기록한다.
use crate::data::{edit_recovery::native, project_file::directory::ProjectDirectory};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LOG_FILES: usize = 5;
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_MEMORY_EVENTS: usize = 256;
const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeEvent {
    schema_version: u32,
    session_id: String,
    observed_at_utc: Option<String>,
    feature: &'static str,
    stage: &'static str,
    category: &'static str,
    outcome: &'static str,
    correlation: Option<String>,
    project_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub(crate) available: bool,
    pub(crate) previous_exit_unconfirmed: usize,
    pub(crate) retained_events: usize,
    pub(crate) dropped_events: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectSnapshot {
    pub(crate) fingerprint: String,
    pub(crate) observed_at_utc: String,
    pub(crate) inspection_complete: bool,
    pub(crate) scanned_files: usize,
    pub(crate) used_assets: usize,
    pub(crate) unused_assets: usize,
    pub(crate) missing_assets: usize,
    pub(crate) corrupt_assets: usize,
    pub(crate) uncertain_assets: usize,
    pub(crate) trash_assets: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportResult {
    pub(crate) outcome: &'static str,
    pub(crate) size: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) event_count: usize,
    pub(crate) excluded: Vec<&'static str>,
    pub(crate) cleanup_required: bool,
}

struct State {
    root: PathBuf,
    guard: Option<ProjectDirectory>,
    session_id: String,
    marker: Option<File>,
    marker_name: String,
    segment: u32,
    events: VecDeque<SafeEvent>,
    dropped: usize,
    previous_exit_unconfirmed: usize,
    available: bool,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentEvents {
    events: Vec<SafeEvent>,
    dropped_events: usize,
}

#[tauri::command]
pub(crate) fn diagnostic_recent_events<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<RecentEvents, &'static str> {
    if webview.label() != "main" || webview.window().label() != "main" {
        return Err("invalid_caller");
    }
    let owner = STATE.get().ok_or("diagnostic_unavailable")?;
    let state = owner.lock().map_err(|_| "diagnostic_unavailable")?;
    Ok(RecentEvents {
        events: state.events.iter().cloned().collect(),
        dropped_events: state.dropped,
    })
}

pub(crate) fn initialize(root: PathBuf) {
    let session_id = uuid::Uuid::new_v4().to_string();
    let marker_name = format!("session-{session_id}.active");
    let mut state = State {
        root: root.clone(),
        guard: None,
        session_id,
        marker: None,
        marker_name,
        segment: 0,
        events: VecDeque::new(),
        dropped: 0,
        previous_exit_unconfirmed: 0,
        available: false,
    };
    if initialize_inner(&mut state).is_ok() {
        state.available = true;
    }
    let _ = STATE.set(Mutex::new(state));
    record("app", "startup", "session", "started", None, None);
}

fn initialize_inner(state: &mut State) -> io::Result<()> {
    fs::create_dir_all(&state.root)?;
    let guard = ProjectDirectory::open_mutable_root(&state.root)?;
    let mut entries = guard.read_dir()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.len() > 10_000 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic root limit",
        ));
    }
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_marker_name(&name) {
            continue;
        }
        match native::open_for_discard(&entry.path()) {
            Ok(file) => {
                guard.validate_file(&file, &name)?;
                native::cleanup(&file)?;
                state.previous_exit_unconfirmed += 1;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
                ) || matches!(error.raw_os_error(), Some(32 | 33)) => {}
            Err(error) => return Err(error),
        }
    }
    let marker_path = state.root.join(&state.marker_name);
    let mut marker = native::open(&marker_path, true, true)?;
    guard.validate_file(&marker, &state.marker_name)?;
    let marker_body = format!(
        "{{\"schemaVersion\":1,\"sessionId\":\"{}\"}}",
        state.session_id
    );
    marker.write_all(marker_body.as_bytes())?;
    marker.sync_all()?;
    state.marker = Some(marker);
    state.guard = Some(guard);
    rotate(state)?;
    Ok(())
}

fn valid_marker_name(name: &str) -> bool {
    name.strip_prefix("session-")
        .and_then(|value| value.strip_suffix(".active"))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some_and(|id| !id.is_nil() && id.to_string() == &name[8..name.len() - 7])
}

fn valid_event_name(name: &str) -> bool {
    let Some(value) = name
        .strip_prefix("events-")
        .and_then(|value| value.strip_suffix(".jsonl"))
    else {
        return false;
    };
    let Some((id, segment)) = value.rsplit_once('-') else {
        return false;
    };
    uuid::Uuid::parse_str(id).is_ok()
        && segment.len() == 6
        && segment.bytes().all(|byte| byte.is_ascii_digit())
}

fn log_name(state: &State) -> String {
    format!("events-{}-{:06}.jsonl", state.session_id, state.segment)
}

pub(crate) fn record(
    feature: &'static str,
    stage: &'static str,
    category: &'static str,
    outcome: &'static str,
    correlation: Option<String>,
    project_fingerprint: Option<String>,
) {
    let Some(owner) = STATE.get() else {
        return;
    };
    let mut state = owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let event = SafeEvent {
        schema_version: 1,
        session_id: state.session_id.clone(),
        observed_at_utc: crate::data::utc_time::now_utc_milliseconds().ok(),
        feature,
        stage,
        category,
        outcome,
        correlation: correlation.filter(|value| safe_id(value)),
        project_fingerprint: project_fingerprint.filter(|value| valid_fingerprint(value)),
    };
    retain_event(&mut state, event);
}

fn retain_event(state: &mut State, event: SafeEvent) {
    if state.events.len() == MAX_MEMORY_EVENTS {
        state.events.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    state.events.push_back(event.clone());
    if state.available && append(state, &event).is_err() {
        state.available = false;
        state.dropped = state.dropped.saturating_add(1);
    }
}

fn safe_id(value: &str) -> bool {
    value.len() <= 64
        && !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn append(state: &mut State, event: &SafeEvent) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(event)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "diagnostic encode"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_EVENT_BYTES {
        state.dropped = state.dropped.saturating_add(1);
        return Ok(());
    }
    let mut name = log_name(state);
    let mut path = state.root.join(&name);
    let current = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if current
        .checked_add(bytes.len() as u64)
        .is_none_or(|total| total > MAX_LOG_BYTES)
    {
        log_test_failure(LogTestPoint::Rotate)?;
        state.segment = state.segment.saturating_add(1);
        name = log_name(state);
        path = state.root.join(&name);
    }
    log_test_failure(LogTestPoint::Write)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    state
        .guard
        .as_ref()
        .ok_or_else(|| io::Error::other("diagnostic guard unavailable"))?
        .validate_file(&file, &name)?;
    if native::link_count(&file)? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "diagnostic alias",
        ));
    }
    file.write_all(&bytes)?;
    file.flush()?;
    rotate(state)
}

fn rotate(state: &mut State) -> io::Result<()> {
    let guard = state
        .guard
        .as_ref()
        .ok_or_else(|| io::Error::other("diagnostic guard unavailable"))?;
    let mut names = guard
        .read_dir()?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            valid_event_name(&name).then(|| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (modified, name, entry.path())
            })
        })
        .collect::<Vec<_>>();
    names.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let remove = names.len().saturating_sub(MAX_LOG_FILES);
    for (_, name, path) in names.into_iter().take(remove) {
        let file = native::open_for_discard(&path)?;
        guard.validate_file(&file, &name)?;
        native::cleanup(&file)?;
    }
    Ok(())
}

pub(crate) fn status() -> Status {
    let Some(owner) = STATE.get() else {
        return Status::default();
    };
    let state = owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Status {
        available: state.available,
        previous_exit_unconfirmed: state.previous_exit_unconfirmed,
        retained_events: state.events.len(),
        dropped_events: state.dropped,
    }
}

pub(crate) fn mark_clean() {
    let Some(owner) = STATE.get() else {
        return;
    };
    record("app", "shutdown", "session", "clean", None, None);
    let mut state = owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    mark_clean_state(&mut state);
}

fn mark_clean_state(state: &mut State) {
    if let Some(marker) = state.marker.take() {
        if log_test_failure(LogTestPoint::MarkerCleanup).is_err()
            || native::cleanup(&marker).is_err()
        {
            state.available = false;
            state.dropped = state.dropped.saturating_add(1);
        }
    }
}

/// updater의 동기 process::exit 전 경계. 설치 성공/재실행을 주장하지 않는다.
pub(crate) fn mark_update_handoff() {
    let Some(owner) = STATE.get() else {
        return;
    };
    record("app", "shutdown", "updater", "handoff", None, None);
    let mut state = owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    mark_clean_state(&mut state);
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LogTestPoint {
    Write,
    Rotate,
    MarkerCleanup,
}

#[cfg(test)]
thread_local! {
    static LOG_TEST_FAILURES: std::cell::RefCell<Vec<LogTestPoint>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn log_test_failure(point: LogTestPoint) -> io::Result<()> {
    LOG_TEST_FAILURES.with(|failures| {
        if failures.borrow().contains(&point) {
            Err(io::Error::other("injected diagnostic logger failure"))
        } else {
            Ok(())
        }
    })
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum LogTestPoint {
    Write,
    Rotate,
    MarkerCleanup,
}

#[cfg(not(test))]
fn log_test_failure(_: LogTestPoint) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn with_log_test_failures<T>(points: &[LogTestPoint], run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LOG_TEST_FAILURES.with(|failures| failures.borrow_mut().clear());
        }
    }
    LOG_TEST_FAILURES.with(|failures| *failures.borrow_mut() = points.to_vec());
    let _reset = Reset;
    run()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportManifest {
    schema_version: u32,
    created_at_utc: Option<String>,
    application: &'static str,
    version: &'static str,
    os: &'static str,
    architecture: &'static str,
    status: Status,
    project: Option<ProjectSnapshot>,
    events: Vec<SafeEvent>,
    excluded: Vec<&'static str>,
}

pub(crate) fn export(
    destination: &Path,
    project: Option<ProjectSnapshot>,
    project_root: Option<&Path>,
) -> io::Result<ExportResult> {
    if !destination.is_absolute() || destination.to_string_lossy().contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "diagnostic destination",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "diagnostic parent"))?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && value.len() <= 240)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "diagnostic filename"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    if managed_roots(project_root)?
        .iter()
        .any(|root| path_within(&canonical_parent, root))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "diagnostic managed destination",
        ));
    }
    let (status, events) = STATE.get().map_or_else(
        || (Status::default(), Vec::new()),
        |owner| {
            let state = owner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                Status {
                    available: state.available,
                    previous_exit_unconfirmed: state.previous_exit_unconfirmed,
                    retained_events: state.events.len(),
                    dropped_events: state.dropped,
                },
                state.events.iter().cloned().collect(),
            )
        },
    );
    let excluded = vec![
        "document_content",
        "asset_bytes",
        "filesystem_paths",
        "user_filenames",
        "environment_variables",
        "credentials",
        "url_details",
        "panic_payloads",
        "memory_dumps",
    ];
    let report = ExportManifest {
        schema_version: 1,
        created_at_utc: crate::data::utc_time::now_utc_milliseconds().ok(),
        application: "Dreamrugi Worldbuild Tool",
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        status,
        project,
        events,
        excluded: excluded.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "diagnostic encode"))?;
    if bytes.len() > MAX_EXPORT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "diagnostic export limit",
        ));
    }
    let guard = ProjectDirectory::open_move_destination(&canonical_parent)?;
    let temp_name = format!(".diagnostic-export-{}.tmp", uuid::Uuid::new_v4());
    let temp_path = canonical_parent.join(&temp_name);
    let mut file = match native::open(&temp_path, true, true) {
        Ok(file) => file,
        Err(error) => return Err(error),
    };
    let before_publish = (|| -> io::Result<()> {
        guard.validate_file(&file, &temp_name)?;
        export_test_failure(ExportTestPoint::Write)?;
        file.write_all(&bytes)?;
        export_test_failure(ExportTestPoint::Sync)?;
        file.sync_all()?;
        let mut actual = Vec::new();
        file.seek(io::SeekFrom::Start(0))?;
        export_test_failure(ExportTestPoint::Readback)?;
        Read::by_ref(&mut file)
            .take(MAX_EXPORT_BYTES as u64 + 1)
            .read_to_end(&mut actual)?;
        export_test_failure(ExportTestPoint::Compare)?;
        if actual != bytes {
            return Err(io::Error::other("diagnostic temp verification"));
        }
        Ok(())
    })();
    if before_publish.is_err() {
        let cleanup_required = export_test_failure(ExportTestPoint::Cleanup).is_err()
            || native::cleanup(&file).is_err();
        return Ok(ExportResult {
            outcome: if cleanup_required {
                "not_applied_cleanup_required"
            } else {
                "not_applied"
            },
            size: None,
            sha256: None,
            event_count: report.events.len(),
            excluded,
            cleanup_required,
        });
    }
    let publish =
        export_test_failure(ExportTestPoint::Publish).and_then(|()| native::publish(&file, name));
    if let Err(error) = publish {
        let cleanup_required = export_test_failure(ExportTestPoint::Cleanup).is_err()
            || native::cleanup(&file).is_err();
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(error);
        }
        return Ok(ExportResult {
            outcome: if cleanup_required {
                "not_applied_cleanup_required"
            } else {
                "not_applied"
            },
            size: None,
            sha256: None,
            event_count: report.events.len(),
            excluded,
            cleanup_required,
        });
    }
    let sha256 = crate::data::edit_recovery::model::digest(&bytes);
    let size = bytes.len() as u64;
    drop(file);
    Ok(ExportResult {
        // 같은 handle의 write/sync/readback/compare가 끝난 뒤 no-replace publish가
        // 마지막 fallible 단계다. 게시 뒤 경로 재개방 단계는 두지 않는다.
        outcome: "published",
        size: Some(size),
        sha256: Some(sha256),
        event_count: report.events.len(),
        excluded,
        cleanup_required: false,
    })
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportTestPoint {
    Write,
    Sync,
    Readback,
    Compare,
    Publish,
    Cleanup,
}

#[cfg(test)]
thread_local! {
    static EXPORT_TEST_FAILURES: std::cell::RefCell<Vec<ExportTestPoint>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn export_test_failure(point: ExportTestPoint) -> io::Result<()> {
    EXPORT_TEST_FAILURES.with(|failures| {
        if failures.borrow().contains(&point) {
            Err(io::Error::other("injected diagnostic export failure"))
        } else {
            Ok(())
        }
    })
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum ExportTestPoint {
    Write,
    Sync,
    Readback,
    Compare,
    Publish,
    Cleanup,
}

#[cfg(not(test))]
fn export_test_failure(_: ExportTestPoint) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn with_export_test_failures<T>(points: &[ExportTestPoint], run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            EXPORT_TEST_FAILURES.with(|failures| failures.borrow_mut().clear());
        }
    }
    EXPORT_TEST_FAILURES.with(|failures| *failures.borrow_mut() = points.to_vec());
    let _reset = Reset;
    run()
}

fn managed_roots(project_root: Option<&Path>) -> io::Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Some(root) = project_root {
        roots.push(fs::canonicalize(root)?);
    }
    if let Some(owner) = STATE.get() {
        let state = owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let app_root = state.root.parent().unwrap_or(&state.root);
        roots.push(fs::canonicalize(app_root)?);
    }
    Ok(roots)
}

fn path_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let root = root
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(test)]
mod tests;
