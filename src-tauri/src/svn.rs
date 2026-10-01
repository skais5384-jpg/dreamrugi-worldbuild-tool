//! M7 A native SVN adapter. Only the main webview can call these fixed actions.
//! No caller-controlled command name, shell, credential argument, or TLS bypass.
pub(crate) mod policy;

use crate::data::{
    collaboration_lock::{
        HeldLock, HeldLockState, LockAcquireRequest, LockCapabilities, LockError,
        LockErrorCategory, LockOperation, LockProviderInfo, LockProviderKind, LockService,
        LockSessionId,
    },
    project_relative_path::ProjectRelativePath,
    repository::ArtifactSourceId,
};
use quick_xml::{events::Event, name::QName, Reader, XmlVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::Any;
use std::cell::Cell;
use std::collections::BTreeMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{Runtime, State, Webview};
use url::Url;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const CLI_TIMEOUT: Duration = Duration::from_secs(120);
static SVN_PROVIDER_ID: AtomicU64 = AtomicU64::new(1);
static SVN_JOB_ID: AtomicU64 = AtomicU64::new(1);
thread_local! { static SVN_JOB_SCOPE: Cell<u64> = const { Cell::new(0) }; }

pub(crate) fn with_validation_scope<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            SVN_JOB_SCOPE.set(self.0);
        }
    }
    let previous = SVN_JOB_SCOPE.replace(SVN_JOB_ID.fetch_add(1, Ordering::Relaxed));
    let _restore = Restore(previous);
    work()
}

#[derive(Clone)]
pub(crate) struct Manager {
    inner: Arc<Inner>,
}
struct Inner {
    config_root: PathBuf,
    app_version: semver::Version,
    preferred_cli: Option<PathBuf>,
    preferred_gui: Option<PathBuf>,
    cli: Mutex<Option<PathBuf>>,
    gui: Mutex<Option<PathBuf>>,
    identity: Mutex<Option<Identity>>,
    active: Mutex<Option<Active>>,
    idle_wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    registration_attempt: Mutex<Option<RegistrationAttempt>>,
    update_sealed: AtomicBool,
    result_jobs: AtomicUsize,
    pending: crate::updater::handoff::Store,
}
#[derive(Clone)]
struct RegistrationAttempt {
    root: PathBuf,
    url: String,
    fingerprint: String,
    username: String,
    setup: RegistrationSetup,
}
#[derive(Clone)]
enum RegistrationSetup {
    Pending,
    Unknown,
    Created { revision: String },
}
impl RegistrationAttempt {
    fn matches(&self, other: &Self) -> bool {
        self.root == other.root
            && self.url == other.url
            && self.fingerprint == other.fingerprint
            && self.username == other.username
    }

    fn created_revision(&self) -> Option<&str> {
        match &self.setup {
            RegistrationSetup::Created { revision } => Some(revision),
            RegistrationSetup::Pending | RegistrationSetup::Unknown => None,
        }
    }
}
#[derive(Clone)]
struct Identity {
    origin: String,
    url: String,
    username: String,
    config: PathBuf,
    // Kept in process memory only for an explicitly non-persistent session.
    password: Option<String>,
    remember: bool,
}
#[derive(Serialize, Deserialize)]
struct Remembered {
    origin: String,
    url: String,
    username: String,
}
#[derive(Serialize, Deserialize)]
struct SavedTools {
    cli: PathBuf,
    gui: PathBuf,
}
const CREDENTIAL_FILE: &str = "credential.bin";
const TOOLS_FILE: &str = "tools.json";

#[cfg(windows)]
fn protect_password(secret: &str) -> Result<Vec<u8>, String> {
    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(|_| "svn_config_failed")?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI binds this blob to the current Windows user. Never request
    // CRYPTPROTECT_LOCAL_MACHINE or persist the original UTF-8 bytes.
    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| "svn_config_failed")?;
        let encrypted = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(encrypted)
    }
}

#[cfg(windows)]
fn unprotect_password(encrypted: &[u8]) -> Result<String, String> {
    if encrypted.is_empty() || encrypted.len() > 64 * 1024 {
        return Err("svn_config_failed".into());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted
            .len()
            .try_into()
            .map_err(|_| "svn_config_failed")?,
        pbData: encrypted.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|_| "svn_config_failed")?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        String::from_utf8(plain).map_err(|_| "svn_config_failed".into())
    }
}

#[cfg(not(windows))]
fn protect_password(_secret: &str) -> Result<Vec<u8>, String> {
    Err("svn_config_failed".into())
}
#[cfg(not(windows))]
fn unprotect_password(_encrypted: &[u8]) -> Result<String, String> {
    Err("svn_config_failed".into())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Session {
    connected: bool,
    url: Option<String>,
    username: Option<String>,
    remembered: bool,
}
struct Active {
    request: String,
    cancel: Arc<AtomicBool>,
}
struct ActiveGuard(Manager, String);
struct ResultCustody(Manager);
impl Drop for ResultCustody {
    fn drop(&mut self) {
        self.0.inner.result_jobs.fetch_sub(1, Ordering::AcqRel);
        if let Some(wake) = self
            .0
            .inner
            .idle_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            wake();
        }
    }
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut active = self
            .0
            .inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let became_idle = active.as_ref().is_some_and(|a| a.request == self.1);
        if became_idle {
            *active = None;
        }
        drop(active);
        if became_idle {
            let wake = self
                .0
                .inner
                .idle_wake
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(wake) = wake {
                wake();
            }
        }
    }
}
impl Manager {
    #[cfg(test)]
    pub(crate) fn set_owned_fsfs_identity(&self, url: String, username: &str) {
        let config = self.inner.config_root.join(format!("owned-{username}"));
        fs::create_dir_all(&config).unwrap();
        *self.inner.identity.lock().unwrap() = Some(Identity {
            origin: origin(&Url::parse(&url).unwrap()),
            url,
            username: username.into(),
            config,
            password: None,
            remember: false,
        });
    }
    #[cfg(test)]
    pub(crate) fn new(config_root: PathBuf) -> Self {
        Self::new_with_version(
            config_root,
            semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version"),
        )
    }
    pub(crate) fn new_with_version(config_root: PathBuf, app_version: semver::Version) -> Self {
        // AppLocalData has already been resolved to its physical directory.
        // SVN CLI config arguments need the ordinary spelling of that same
        // Windows path, including when the app was installed via NSIS/MSIX.
        let config_root = ordinary_windows_path(&config_root);
        // Non-persistent sessions use an attempt-* config so TortoiseProc can
        // authenticate in its own dialog. Remove only this app's abandoned
        // session children after restart; no external SVN cache is touched.
        let canonical_root = config_root.canonicalize().ok();
        if let Ok(children) = fs::read_dir(&config_root) {
            for child in children.flatten() {
                if child
                    .file_name()
                    .to_string_lossy()
                    .strip_prefix("attempt-")
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .is_some()
                    && child
                        .file_type()
                        .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                    && canonical_root.as_ref().is_some_and(|root| {
                        child
                            .path()
                            .canonicalize()
                            .is_ok_and(|path| path.parent() == Some(root.as_path()))
                    })
                {
                    let _ = fs::remove_dir_all(child.path());
                }
            }
        }
        let remembered: Option<Remembered> = fs::read(config_root.join("current.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let identity = remembered.and_then(|saved| {
            let parsed = secure_url(&saved.url).ok()?;
            if origin(&parsed) != saved.origin {
                return None;
            }
            let config = config_for_root(&config_root, &saved.origin, &saved.username);
            let password = fs::read(config.join(CREDENTIAL_FILE))
                .ok()
                .and_then(|bytes| unprotect_password(&bytes).ok())?;
            Some(Identity {
                origin: saved.origin,
                url: saved.url,
                username: saved.username,
                config,
                password: Some(password),
                remember: true,
            })
        });
        let tools: Option<SavedTools> = fs::read(config_root.join(TOOLS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Self {
            inner: Arc::new(Inner {
                pending: crate::updater::handoff::Store::new(config_root.join("update-pending")),
                result_jobs: AtomicUsize::new(0),
                config_root,
                preferred_cli: tools.as_ref().map(|saved| saved.cli.clone()),
                preferred_gui: tools.map(|saved| saved.gui),
                // A saved path is only a preference. Probe its executable name,
                // version and required options before any SVN operation uses it.
                cli: Mutex::new(None),
                gui: Mutex::new(None),
                identity: Mutex::new(identity),
                app_version,
                active: Mutex::new(None),
                idle_wake: Mutex::new(None),
                registration_attempt: Mutex::new(None),
                update_sealed: AtomicBool::new(false),
            }),
        }
    }
    fn cli(&self) -> Result<PathBuf, String> {
        self.inner
            .cli
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "svn_cli_missing".into())
    }
    fn gui(&self) -> Result<PathBuf, String> {
        self.inner
            .gui
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "svn_gui_missing".into())
    }
    pub(crate) fn has_active(&self) -> bool {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
            || self.inner.result_jobs.load(Ordering::Acquire) != 0
    }
    pub(crate) fn set_idle_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self
            .inner
            .idle_wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }
    fn identity(&self) -> Option<Identity> {
        self.inner
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn begin(&self, request: &str) -> Result<(ActiveGuard, Arc<AtomicBool>), String> {
        if uuid::Uuid::parse_str(request).is_err() {
            return Err("svn_invalid_request".into());
        }
        let mut active = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        if self.inner.update_sealed.load(Ordering::Acquire) {
            return Err("svn_close_project_first".into());
        }
        if active.is_some() {
            return Err("svn_busy".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *active = Some(Active {
            request: request.into(),
            cancel: cancel.clone(),
        });
        Ok((ActiveGuard(self.clone(), request.into()), cancel))
    }
    pub(crate) fn cancel_active(&self) {
        if let Some(active) = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            active.cancel.store(true, Ordering::Release);
        }
    }
    /// 같은 active mutex에서 새 SVN 작업과 설치 승인의 경쟁을 차단한다.
    pub(crate) fn seal_for_update(&self) -> bool {
        let active = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.is_some() || self.inner.result_jobs.load(Ordering::Acquire) != 0 {
            return false;
        }
        self.inner.update_sealed.store(true, Ordering::Release);
        true
    }
    pub(crate) fn unseal_update(&self) {
        self.inner.update_sealed.store(false, Ordering::Release);
    }
    fn result_custody(&self) -> Result<ResultCustody, String> {
        let _active = self.inner.active.lock().unwrap_or_else(|e| e.into_inner());
        self.inner
            .pending
            .admit(self.inner.result_jobs.load(Ordering::Acquire))
            .map_err(|_| "svn_result_handoff_unavailable")?;
        if self.inner.update_sealed.load(Ordering::Acquire) {
            return Err("svn_close_project_first".into());
        }
        self.inner.result_jobs.fetch_add(1, Ordering::AcqRel);
        Ok(ResultCustody(self.clone()))
    }
    fn preserve_uncertain(
        &self,
        operation: &str,
        root: &str,
        action: &str,
        url: Option<&str>,
        error: Option<&str>,
    ) {
        let Some(error) = error else {
            return;
        };
        let outcome = if error.ends_with("_partial") {
            "partial"
        } else if error.ends_with("_unverified") || error == "svn_task_failed" {
            "unknown"
        } else {
            return;
        };
        let record = crate::updater::handoff::PendingSvn {
            operation_id: operation.into(),
            project_fingerprint: format!("{:x}", Sha256::digest(root.as_bytes())),
            requested_paths: Vec::new(),
            outcome: outcome.into(),
            observed: serde_json::json!({"action":action,"error":error,"requestedUrl":url}),
        };
        if self.inner.pending.preserve(record).is_err() {
            crate::diagnostic_log::record(
                "svn",
                "handoff",
                "preserve_failed",
                "error",
                Some(operation.into()),
                None,
            );
        }
    }
    pub(crate) fn verify_update_handoff(
        &self,
    ) -> Result<Vec<crate::updater::handoff::PendingSvn>, &'static str> {
        if self.has_active() {
            return Err("handoff");
        }
        self.inner.pending.verify()
    }
}

fn main_only<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    use tauri::Manager as _;
    if webview
        .app_handle()
        .try_state::<Arc<crate::state::AppState>>()
        .is_some_and(|state| !state.update_work_allowed())
    {
        return Err("svn_startup_or_shutdown_pending".into());
    }
    if webview.label() == "main" && webview.window().label() == "main" {
        Ok(())
    } else {
        Err("svn_forbidden".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Probe {
    pub(crate) installed: bool,
    path: Option<String>,
    version: Option<String>,
    reason: Option<&'static str>,
    gui_installed: bool,
    gui_path: Option<String>,
}

fn candidate_paths(specified: Option<String>) -> Vec<PathBuf> {
    if let Some(path) = specified {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    for base in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(PathBuf::from(base).join("TortoiseSVN/bin/svn.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|p| p.join("svn.exe")));
    }
    candidates
}

pub(crate) fn probe(
    manager: &Manager,
    specified: Option<String>,
    specified_gui: Option<String>,
) -> Probe {
    let mut found = None;
    let mut cli_candidates = candidate_paths(specified.clone());
    if specified.is_none() {
        if let Some(previous) = &manager.inner.preferred_cli {
            cli_candidates.insert(0, previous.clone());
        }
        if let Ok(previous) = manager.cli() {
            cli_candidates.insert(0, previous);
        }
    }
    for candidate in cli_candidates {
        if !candidate.is_file()
            || !candidate
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("svn.exe"))
        {
            continue;
        }
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        let cancelled = AtomicBool::new(false);
        let Ok(value) = run_timeout(
            &canonical,
            &["--version".into(), "--quiet".into()],
            None,
            &cancelled,
            Duration::from_secs(5),
        ) else {
            continue;
        };
        let value = value.trim().to_owned();
        if value.len() > 32 || !value.chars().all(|ch| ch.is_ascii_digit() || ch == '.') {
            continue;
        }
        let Ok(help) = run_timeout(
            &canonical,
            &["help".into(), "-v".into(), "info".into()],
            None,
            &cancelled,
            Duration::from_secs(5),
        ) else {
            continue;
        };
        if !help.contains("--password-from-stdin")
            || !help.contains("--config-dir")
            || !help.contains("--non-interactive")
            || !help.contains("--xml")
        {
            continue;
        }
        found = Some((canonical, value));
        break;
    }
    let mut gui_candidates = if let Some(path) = specified_gui.clone() {
        vec![PathBuf::from(path)]
    } else {
        let mut paths = Vec::new();
        if let Some((cli, _)) = &found {
            if let Some(parent) = cli.parent() {
                paths.push(parent.join("TortoiseProc.exe"));
            }
        }
        for base in [
            std::env::var_os("ProgramFiles"),
            std::env::var_os("ProgramFiles(x86)"),
        ]
        .into_iter()
        .flatten()
        {
            paths.push(PathBuf::from(base).join("TortoiseSVN/bin/TortoiseProc.exe"));
        }
        paths
    };
    if specified_gui.is_none() {
        if let Some(previous) = &manager.inner.preferred_gui {
            gui_candidates.insert(0, previous.clone());
        }
        if let Ok(previous) = manager.gui() {
            gui_candidates.insert(0, previous);
        }
    }
    let gui = gui_candidates.drain(..).find_map(|candidate| {
        (candidate.is_file()
            && candidate.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("TortoiseProc.exe")
            }))
        .then(|| candidate.canonicalize().ok())
        .flatten()
    });
    *manager.inner.cli.lock().unwrap_or_else(|e| e.into_inner()) =
        found.as_ref().map(|(path, _)| path.clone());
    *manager.inner.gui.lock().unwrap_or_else(|e| e.into_inner()) = gui.clone();
    Probe {
        installed: found.is_some(),
        path: found
            .as_ref()
            .map(|(path, _)| path.to_string_lossy().into_owned()),
        version: found.map(|(_, version)| version),
        reason: if manager.cli().is_ok() {
            None
        } else {
            Some("svn_cli_missing_or_unsupported")
        },
        gui_installed: gui.is_some(),
        gui_path: gui.map(|path| path.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
pub(crate) async fn svn_probe<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    path: Option<String>,
    gui_path: Option<String>,
) -> Result<Probe, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let active = manager
            .inner
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active.is_some() || manager.inner.update_sealed.load(Ordering::Acquire) {
            return Err("svn_busy".into());
        }
        let result = probe(&manager, path, gui_path);
        if result.installed && result.gui_installed {
            let saved = SavedTools {
                cli: manager.cli().map_err(|_| "svn_config_failed")?,
                gui: manager.gui().map_err(|_| "svn_config_failed")?,
            };
            fs::create_dir_all(&manager.inner.config_root).map_err(|_| "svn_config_failed")?;
            fs::write(
                manager.inner.config_root.join(TOOLS_FILE),
                serde_json::to_vec(&saved).map_err(|_| "svn_config_failed")?,
            )
            .map_err(|_| "svn_config_failed")?;
        }
        drop(active);
        Ok(result)
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?
}

fn secure_url(value: &str) -> Result<Url, String> {
    let parsed = Url::parse(value).map_err(|_| "svn_invalid_url")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err("svn_https_required".into());
    }
    Ok(parsed)
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn config_for(manager: &Manager, origin: &str, username: &str) -> PathBuf {
    config_for_root(&manager.inner.config_root, origin, username)
}
fn config_for_root(config_root: &Path, origin: &str, username: &str) -> PathBuf {
    let hash = Sha256::digest(format!("{origin}\0{username}").as_bytes());
    config_root.join(format!("{hash:x}"))
}

#[tauri::command]
pub(crate) fn svn_session<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
) -> Result<Session, String> {
    main_only(&webview)?;
    let identity = manager.identity();
    Ok(Session {
        connected: identity.is_some(),
        url: identity.as_ref().map(|i| i.url.clone()),
        username: identity.as_ref().map(|i| i.username.clone()),
        remembered: identity.as_ref().is_some_and(|i| i.remember),
    })
}

fn read_capped(mut input: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut content = Vec::new();
    let mut truncated = false;
    let mut block = [0_u8; 8192];
    loop {
        let size = input.read(&mut block)?;
        if size == 0 {
            break;
        }
        if content.len() + size <= MAX_OUTPUT {
            content.extend_from_slice(&block[..size]);
        } else {
            truncated = true;
        }
    }
    Ok((content, truncated))
}

fn run(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    run_timeout(cli, args, secret, cancel, CLI_TIMEOUT)
}

fn run_in(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
    directory: &Path,
) -> Result<String, String> {
    String::from_utf8(run_bytes_timeout(
        cli,
        args,
        secret,
        cancel,
        CLI_TIMEOUT,
        Some(directory),
    )?)
    .map_err(|_| "svn_output_invalid".into())
}

fn run_timeout(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, String> {
    String::from_utf8(run_bytes_timeout(cli, args, secret, cancel, timeout, None)?)
        .map_err(|_| "svn_output_invalid".into())
}

#[cfg(test)]
fn run_command(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    run_bytes_timeout(cli, args, secret, cancel, CLI_TIMEOUT, None).map(|_| ())
}

fn run_command_in(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
    directory: &Path,
) -> Result<(), String> {
    run_bytes_timeout(cli, args, secret, cancel, CLI_TIMEOUT, Some(directory)).map(|_| ())
}

fn run_bytes_timeout(
    cli: &Path,
    args: &[String],
    secret: Option<&str>,
    cancel: &AtomicBool,
    timeout: Duration,
    directory: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(cli);
    command.args(args);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "svn_launch_failed")?;
    let stdout = child.stdout.take().ok_or("svn_launch_failed")?;
    let stderr = child.stderr.take().ok_or("svn_launch_failed")?;
    let out = thread::spawn(move || read_capped(stdout));
    let err = thread::spawn(move || read_capped(stderr));
    if let Some(secret) = secret {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(secret.as_bytes());
            // svn --password-from-stdin consumes stdin verbatim. A newline is
            // part of the password and makes even a correct secret fail.
        }
    }
    drop(child.stdin.take());
    let start = Instant::now();
    let (status, interrupted) = loop {
        if cancel.load(Ordering::Acquire) || start.elapsed() > timeout {
            let _ = child.kill();
            break (child.wait().map_err(|_| "svn_wait_failed")?, true);
        }
        if let Some(status) = child.try_wait().map_err(|_| "svn_wait_failed")? {
            break (status, false);
        }
        thread::sleep(Duration::from_millis(40));
    };
    let (stdout, output_truncated) = out
        .join()
        .map_err(|_| "svn_output_failed")?
        .map_err(|_| "svn_output_failed")?;
    let (stderr, error_truncated) = err
        .join()
        .map_err(|_| "svn_output_failed")?
        .map_err(|_| "svn_output_failed")?;
    if interrupted {
        return Err(if cancel.load(Ordering::Acquire) {
            "svn_cancelled"
        } else {
            "svn_timeout"
        }
        .into());
    }
    if output_truncated || error_truncated {
        return Err("svn_output_too_large".into());
    }
    if !status.success() {
        let diagnostic = String::from_utf8_lossy(&stderr).to_ascii_lowercase();
        return Err(if diagnostic.contains("e160020") {
            "svn_path_exists"
        } else if diagnostic.contains("certificate") || diagnostic.contains("ssl") {
            "svn_tls_failed"
        } else if diagnostic.contains("authorization")
            || diagnostic.contains("authentication")
            || diagnostic.contains("password")
            || diagnostic.contains("e170001")
        {
            "svn_auth_failed"
        } else if diagnostic.contains("connection") || diagnostic.contains("e170013") {
            "svn_connection_failed"
        } else {
            "svn_command_failed"
        }
        .into());
    }
    Ok(stdout)
}

fn args_with_auth(
    manager: &Manager,
    identity: Option<&Identity>,
    command: &str,
    options: &[&str],
    target: &str,
) -> Vec<String> {
    let mut args = vec![command.into()];
    args.extend(options.iter().map(|s| (*s).into()));
    let config = identity
        .map(|identity| identity.config.clone())
        .unwrap_or_else(|| manager.inner.config_root.join("anonymous"));
    args.extend([
        "--config-dir".into(),
        config.to_string_lossy().into_owned(),
        "--no-auth-cache".into(),
    ]);
    if let Some(identity) = identity {
        args.extend(["--username".into(), identity.username.clone()]);
        if identity.password.is_some() {
            args.push("--password-from-stdin".into());
        }
    }
    args.push("--non-interactive".into());
    args.push("--".into());
    // An explicit empty peg revision prevents a path containing @ from being
    // interpreted as a different repository revision.
    args.push(format!("{target}@"));
    args
}

/// The provider is scoped to one opened project. The map is a record of
/// locks acquired by this process, never a substitute for server validation.
pub(crate) struct SvnLockService {
    manager: Manager,
    project: PathBuf,
    instance: u64,
    held: Mutex<BTreeMap<ProjectRelativePath, String>>,
    confirmed_commits: Mutex<BTreeMap<String, (Vec<String>, Vec<String>)>>,
    policy_cache: Mutex<Option<(u64, String, Result<(), String>)>>,
    #[cfg(test)]
    post_commit_probe: Mutex<Option<Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitCandidate {
    path: String,
    name: Option<String>,
    local: String,
    properties: Option<String>,
    held: bool,
    new_path: bool,
    can_schedule_delete: bool,
    changed: bool,
    eligible: bool,
    blocked_reason: Option<String>,
    required: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitResult {
    revision: String,
    paths: Vec<String>,
    deleted: Vec<String>,
    unlock_pending: Vec<String>,
    verification_unknown: Vec<CommitVerificationUnknown>,
    #[serde(skip)]
    deleted_confirmed: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitVerificationUnknown {
    path: String,
    check: &'static str,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DocumentLockInfo {
    locked: bool,
    pub(crate) owner: Option<String>,
    pub(crate) observation: Option<String>,
}

fn lock_observation(url: &str, token: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("worldbuild-lock-observation-v1\0{url}\0{token}").as_bytes())
    )
}

struct AssetBasePackage {
    directory: String,
    metadata: String,
    content: String,
    name: String,
}

struct SvnHeld {
    instance: u64,
    project_fingerprint: String,
    session: LockSessionId,
    target: ProjectRelativePath,
    token: String,
    new_path: bool,
    state: HeldLockState,
    validated_scope: u64,
}

impl SvnLockService {
    pub(crate) fn new(manager: Manager, project: PathBuf) -> Self {
        Self {
            manager,
            project,
            instance: SVN_PROVIDER_ID.fetch_add(1, Ordering::Relaxed),
            held: Mutex::new(BTreeMap::new()),
            confirmed_commits: Mutex::new(BTreeMap::new()),
            policy_cache: Mutex::new(None),
            #[cfg(test)]
            post_commit_probe: Mutex::new(None),
        }
    }

    pub(crate) fn held_targets(&self) -> Vec<ProjectRelativePath> {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Read only the saved document's WC state. Automatic saves must not run
    /// `status -u` over the project or turn an in-memory draft into a disk fact.
    fn local_document_status(&self, document: &str) -> Result<StatusEntry, String> {
        let id = uuid::Uuid::parse_str(document).map_err(|_| "svn_invalid_target")?;
        let target = ProjectRelativePath::parse(&format!("documents/{id}.json"))
            .map_err(|_| "svn_invalid_target")?;
        let file = self.file(&target)?;
        self.with_operation(|manager, cancel| {
            let xml = self.run_for(manager, "status", &["--xml", "--verbose"], &file, cancel)?;
            let (rows, _) = parse_status(&xml)?;
            let mut entry = rows
                .into_iter()
                .find(|row| same_svn_local_path(&row.path, &file.to_string_lossy()))
                .ok_or("svn_status_unknown")?;
            let property = self.run_for(
                manager,
                "propget",
                &["svn:needs-lock", "--xml"],
                &file,
                cancel,
            )?;
            entry.needs_lock = !parse_needs_lock(&property)?.is_empty();
            Ok(entry)
        })
    }

    pub(crate) fn document_lock_owner(&self, document: &str) -> Result<DocumentLockInfo, String> {
        let id = uuid::Uuid::parse_str(document).map_err(|_| "svn_invalid_target")?;
        let target = ProjectRelativePath::parse(&format!("documents/{id}.json"))
            .map_err(|_| "svn_invalid_target")?;
        let file = self.file(&target)?;
        self.with_operation(|manager, cancel| {
            let (info, _) = self.inspect_file(manager, &file, cancel)?;
            let identity = manager.identity();
            let args = args_with_auth(manager, identity.as_ref(), "info", &["--xml"], &info.url);
            let xml = run(
                &manager.cli()?,
                &args,
                identity.as_ref().and_then(|i| i.password.as_deref()),
                cancel,
            )?;
            let mut result = parse_lock_owner(&xml)?;
            result.observation =
                parse_lock_token(&xml)?.map(|token| lock_observation(&info.url, &token));
            Ok(result)
        })
    }

    pub(crate) fn force_document_lock(
        &self,
        document: &str,
        expected_owner: &str,
        expected_observation: &str,
        expected_username: &str,
        reason: &str,
    ) -> Result<(), String> {
        let id = uuid::Uuid::parse_str(document).map_err(|_| "svn_invalid_target")?;
        let target = ProjectRelativePath::parse(&format!("documents/{id}.json"))
            .map_err(|_| "svn_invalid_target")?;
        let file = self.target_file(&target)?;
        let reason = reason.trim();
        if expected_owner.is_empty()
            || expected_owner.len() > 128
            || expected_owner.chars().any(char::is_control)
            || expected_observation.len() != 64
            || !expected_observation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || expected_username.is_empty()
            || reason.is_empty()
            || reason.len() > 200
            || reason.chars().any(char::is_control)
        {
            return Err("svn_force_invalid_request".into());
        }
        if self
            .held
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&target)
        {
            return Err("svn_busy".into());
        }
        self.with_operation(|manager, cancel| {
            self.policy_admit_inner(manager, cancel)?;
            let identity = manager.identity().ok_or("svn_login_required")?;
            if identity.username != expected_username {
                return Err("svn_force_context_changed".into());
            }
            let (info, local_token) = self.inspect_file(manager, &file, cancel)?;
            if local_token.is_some() {
                return Err("svn_force_context_changed".into());
            }
            let source_before = fs::read(&file).map_err(|_| "svn_status_unsafe")?;
            let current = status(manager, &file, cancel)?;
            let row = current
                .entries
                .iter()
                .find(|entry| same_svn_local_path(&entry.path, &file.to_string_lossy()))
                .ok_or("svn_status_unknown")?;
            if current.server_error.is_some()
                || current.recovery.is_some()
                || row.local != "normal"
                || row
                    .properties
                    .as_deref()
                    .is_some_and(|value| value != "none" && value != "normal")
                || row.working_copy_locked
                || row.wc_locked
                || row
                    .remote
                    .as_deref()
                    .is_some_and(|value| value != "none" && value != "normal")
                || row
                    .remote_properties
                    .as_deref()
                    .is_some_and(|value| value != "none" && value != "normal")
            {
                return Err("svn_status_unsafe".into());
            }
            let props = self.run_for(
                manager,
                "propget",
                &["svn:needs-lock", "--xml"],
                &file,
                cancel,
            )?;
            if parse_needs_lock(&props)?.is_empty() {
                return Err("svn_needs_lock_missing".into());
            }
            let args = args_with_auth(manager, Some(&identity), "info", &["--xml"], &info.url);
            let xml = run(&manager.cli()?, &args, identity.password.as_deref(), cancel)?;
            let observed = parse_lock_owner(&xml)?;
            let current_observation =
                parse_lock_token(&xml)?.map(|token| lock_observation(&info.url, &token));
            if !observed.locked
                || observed.owner.as_deref() != Some(expected_owner)
                || current_observation.as_deref() != Some(expected_observation)
            {
                return Err("svn_force_context_changed".into());
            }
            self.run_for(manager, "lock", &["--force", "-m", reason], &file, cancel)
                .map_err(|_| "svn_lock_unverified")?;
            let (after_info, local) = self
                .inspect_file(manager, &file, cancel)
                .map_err(|_| "svn_lock_unverified")?;
            let remote = self
                .remote_token(manager, &after_info.url, cancel)
                .map_err(|_| "svn_lock_unverified")?;
            let source_after = fs::read(&file).map_err(|_| "svn_lock_unverified")?;
            if local.is_none()
                || local != remote
                || info.url != after_info.url
                || info.revision != after_info.revision
                || source_before != source_after
            {
                return Err("svn_lock_unverified".into());
            }
            Ok(())
        })
    }

    fn candidates_inner(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
    ) -> Result<Vec<CommitCandidate>, String> {
        let current = status(manager, &self.project, cancel)?;
        if current.server_error.is_some() || current.recovery.is_some() {
            return Err("svn_server_unconfirmed".into());
        }
        let root = self
            .project
            .canonicalize()
            .map_err(|_| "svn_project_missing")?;
        let shareable =
            crate::svn_shared::inventory(&root).map_err(|_| "svn_commit_dependency_missing")?;
        let mut entries = current.entries.clone();
        for relative in &shareable.files {
            let Some(("assets", remainder)) = relative.split_once('/') else {
                continue;
            };
            let Some((id, _)) = remainder.split_once('/') else {
                continue;
            };
            let file = root.join(relative);
            if entries
                .iter()
                .any(|row| same_svn_local_path(&row.path, &file.to_string_lossy()))
            {
                continue;
            }
            let parent = root.join("assets").join(id);
            if entries.iter().any(|row| {
                (same_svn_local_path(&row.path, &parent.to_string_lossy())
                    || same_svn_local_path(&row.path, &root.join("assets").to_string_lossy()))
                    && row.local == "unversioned"
            }) {
                entries.push(StatusEntry {
                    path: file.to_string_lossy().into_owned(),
                    local: "unversioned".into(),
                    properties: None,
                    remote: None,
                    remote_properties: None,
                    lock_owner: None,
                    wc_locked: false,
                    working_copy_locked: false,
                    needs_lock: false,
                    remote_only: false,
                });
            }
        }
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut result = Vec::new();
        let mut asset_base = BTreeMap::<String, AssetBasePackage>::new();
        for row in &entries {
            let raw = PathBuf::from(&row.path);
            let path = if matches!(row.local.as_str(), "deleted" | "missing") {
                raw
            } else if let Ok(path) = raw.canonicalize() {
                path
            } else {
                continue;
            };
            let raw_relative = if matches!(row.local.as_str(), "deleted" | "missing") {
                svn_local_relative(&root, &row.path)
            } else {
                path.strip_prefix(&root)
                    .ok()
                    .map(|value| value.to_string_lossy().replace('\\', "/"))
            };
            let Some(raw_relative) = raw_relative else {
                continue;
            };
            if raw_relative.is_empty() || raw_relative == "." {
                continue;
            }
            if row.local == "normal"
                && row
                    .properties
                    .as_deref()
                    .is_none_or(|value| value == "none" || value == "normal")
                && shareable
                    .directories
                    .iter()
                    .any(|directory| directory == &raw_relative)
            {
                continue;
            }
            let Ok(relative) = ProjectRelativePath::parse(&raw_relative) else {
                continue;
            };
            let absent_asset = if matches!(row.local.as_str(), "deleted" | "missing") {
                if let Some(id) = asset_id_from_path(relative.as_str()) {
                    if !asset_base.contains_key(id) {
                        if let Ok(package) = self.asset_base_package(manager, cancel, id) {
                            asset_base.insert(id.to_owned(), package);
                        }
                    }
                    asset_base.get(id).is_some_and(|package| {
                        [
                            package.directory.as_str(),
                            package.metadata.as_str(),
                            package.content.as_str(),
                        ]
                        .contains(&relative.as_str())
                    })
                } else {
                    false
                }
            } else {
                false
            };
            let absent_artifact = matches!(row.local.as_str(), "deleted" | "missing")
                && ArtifactSourceId::from_target(&relative).is_some();
            if !absent_artifact
                && !absent_asset
                && !shareable.files.iter().any(|file| file == relative.as_str())
            {
                continue;
            }
            let changed = matches!(
                row.local.as_str(),
                "modified" | "added" | "unversioned" | "deleted" | "missing"
            ) || row
                .properties
                .as_deref()
                .is_some_and(|value| value != "none" && value != "normal");
            let locally_new = matches!(row.local.as_str(), "added" | "unversioned");
            let held_here = held.get(&relative).is_some_and(|token| !token.is_empty());
            if !changed && !held_here {
                continue;
            }
            let file = if ArtifactSourceId::from_target(&relative).is_some() {
                self.target_file(&relative)?
            } else {
                path
            };
            let mut blocked_reason = if row.local == "missing" {
                Some("svn_commit_selection_changed".to_owned())
            } else if !changed {
                Some("svn_commit_no_changes".to_owned())
            } else {
                None
            };
            let eligible = if relative.as_str() == policy::FILE {
                let checked = self.policy_validate_candidate(manager, cancel);
                let safe = changed
                    && row.local == "modified"
                    && !row.working_copy_locked
                    && row
                        .remote
                        .as_deref()
                        .is_none_or(|v| v == "none" || v == "normal")
                    && checked.is_ok();
                blocked_reason = checked
                    .err()
                    .or_else(|| (!safe).then(|| "svn_commit_selection_changed".into()));
                safe
            } else if row.local == "missing" {
                false
            } else if row.local == "deleted" {
                let (info, local) = self.inspect_file(manager, &file, cancel)?;
                let remote = self.remote_token(manager, &info.url, cancel)?;
                let lock_safe = remote.is_none() || remote == local;
                let references_safe = self.deletion_references_safe(&relative, &entries);
                let safe = lock_safe
                    && references_safe
                    && !row.working_copy_locked
                    && row.remote.as_deref().is_none_or(|value| value == "none");
                blocked_reason = (!safe).then(|| "svn_commit_selection_changed".to_owned());
                safe
            } else if locally_new {
                let safe = !row.working_copy_locked
                    && row.remote.as_deref().is_none_or(|value| value == "none")
                    && self.new_path_absent(manager, &file, cancel)?;
                let dependencies_ready = !matches!(
                    ArtifactSourceId::from_target(&relative),
                    Some(ArtifactSourceId::Document(_))
                ) || self
                    .document_dependencies_ready(&file, &entries, &shareable);
                if blocked_reason.is_none() {
                    blocked_reason = if !safe {
                        Some("svn_commit_selection_changed".to_owned())
                    } else if !dependencies_ready {
                        Some("svn_commit_dependency_missing".to_owned())
                    } else {
                        None
                    };
                }
                safe && dependencies_ready
            } else {
                let (info, local) = self.inspect_file(manager, &file, cancel)?;
                let remote = self.remote_token(manager, &info.url, cancel)?;
                let lock_valid = local.as_deref().is_some_and(|token| {
                    remote.as_deref() == Some(token)
                        && held.get(&relative).is_none_or(|owned| owned == token)
                }) && row.needs_lock;
                let status_safe = changed
                    && row.local != "added"
                    && row.local != "unversioned"
                    && !row.working_copy_locked
                    && row
                        .remote
                        .as_deref()
                        .is_none_or(|value| value == "none" || value == "normal");
                let dependencies_ready = !matches!(
                    ArtifactSourceId::from_target(&relative),
                    Some(ArtifactSourceId::Document(_))
                ) || self
                    .document_dependencies_ready(&file, &entries, &shareable);
                if blocked_reason.is_none() {
                    blocked_reason = if !lock_valid {
                        Some("svn_commit_lock_required".to_owned())
                    } else if !status_safe {
                        Some("svn_commit_selection_changed".to_owned())
                    } else if !dependencies_ready {
                        Some("svn_commit_dependency_missing".to_owned())
                    } else {
                        None
                    };
                }
                lock_valid && status_safe && dependencies_ready
            };
            result.push(CommitCandidate {
                path: relative.as_str().to_owned(),
                name: if relative.as_str() == policy::FILE {
                    Some("프로젝트 설정 — 최소 앱 버전".into())
                } else {
                    asset_id_from_path(relative.as_str())
                        .and_then(|id| asset_base.get(id).map(|package| package.name.clone()))
                        .or_else(|| {
                            self.candidate_name(manager, cancel, &relative, &file, absent_artifact)
                        })
                },
                local: row.local.clone(),
                properties: row.properties.clone(),
                held: held_here || (!locally_new && row.local != "deleted" && eligible),
                new_path: locally_new,
                can_schedule_delete: row.local == "missing"
                    && self.deletion_references_safe(&relative, &entries)
                    && (!absent_asset
                        || asset_id_from_path(relative.as_str())
                            .and_then(|id| asset_base.get(id))
                            .is_some_and(|package| package.directory == relative.as_str())),
                changed,
                eligible,
                blocked_reason,
                required: if absent_asset {
                    asset_id_from_path(relative.as_str())
                        .and_then(|id| asset_base.get(id))
                        .map(|package| {
                            [
                                package.directory.clone(),
                                package.metadata.clone(),
                                package.content.clone(),
                            ]
                            .into_iter()
                            .filter(|path| path != relative.as_str())
                            .collect()
                        })
                        .unwrap_or_default()
                } else if matches!(
                    ArtifactSourceId::from_target(&relative),
                    Some(ArtifactSourceId::Document(_))
                ) {
                    if row.local == "deleted" {
                        let layout = self.project.join("workspace/document-layout.json");
                        entries
                            .iter()
                            .any(|entry| {
                                same_svn_local_path(&entry.path, &layout.to_string_lossy())
                                    && (entry.local == "modified"
                                        || entry.properties.as_deref() == Some("modified"))
                            })
                            .then(|| vec!["workspace/document-layout.json".into()])
                            .unwrap_or_default()
                    } else {
                        self.document_required_dependencies(&file, &entries)
                            .unwrap_or_default()
                    }
                } else {
                    Vec::new()
                },
            });
        }
        let eligible_paths: std::collections::BTreeSet<_> = result
            .iter()
            .filter(|candidate| candidate.eligible)
            .map(|candidate| candidate.path.clone())
            .collect();
        for candidate in &mut result {
            if candidate
                .required
                .iter()
                .any(|required| !eligible_paths.contains(required))
            {
                candidate.eligible = false;
                candidate.blocked_reason = Some("svn_commit_dependency_missing".into());
            }
        }
        result.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(result)
    }

    fn candidate_name(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
        relative: &ProjectRelativePath,
        file: &Path,
        absent: bool,
    ) -> Option<String> {
        if matches!(
            ArtifactSourceId::from_target(relative),
            Some(ArtifactSourceId::DocumentLayout)
        ) {
            return Some("문서 구조".into());
        }
        if relative.as_str().starts_with("assets/") {
            let id = relative.as_str().split('/').nth(1)?;
            return crate::data::assets::Store::open(&self.project, false)
                .ok()?
                .read(id)
                .ok()
                .map(|(metadata, _)| metadata.name);
        }
        let bytes = if absent {
            self.run_for(manager, "cat", &["-r", "BASE"], file, cancel)
                .ok()?
                .into_bytes()
        } else {
            fs::read(file).ok()?
        };
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()?
            .get("name")?
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
    }

    fn asset_base_package(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
        id: &str,
    ) -> Result<AssetBasePackage, String> {
        if !crate::data::media::valid_id(id) {
            return Err("svn_invalid_target".into());
        }
        let directory = format!("assets/{id}");
        let metadata = format!("{directory}/metadata.json");
        let parent = self.project.join("assets");
        let root = self
            .project
            .canonicalize()
            .map_err(|_| "svn_project_missing")?;
        if !parent
            .canonicalize()
            .is_ok_and(|path| path.starts_with(&root))
            || self.project.join(&directory).exists()
        {
            return Err("svn_invalid_target".into());
        }
        let xml = self.run_for(
            manager,
            "list",
            &["-r", "BASE", "--xml"],
            &self.project.join(&directory),
            cancel,
        )?;
        let names = parse_list_names(&xml)?;
        let raw = self.run_for(
            manager,
            "cat",
            &["-r", "BASE"],
            &self.project.join(&metadata),
            cancel,
        )?;
        crate::data::json::parse_strict_json_object(raw.as_bytes())
            .map_err(|_| "svn_invalid_target")?;
        let parsed: crate::data::assets::Metadata =
            serde_json::from_str(&raw).map_err(|_| "svn_invalid_target")?;
        if parsed.schema_version != 1
            || parsed.id != id
            || !crate::data::assets::safe_name(&parsed.name)
        {
            return Err("svn_invalid_target".into());
        }
        let content = format!("{directory}/{}", crate::data::assets::filename(&parsed));
        let expected = ["metadata.json", content.rsplit('/').next().unwrap_or("")];
        if names.len() != 2 || names.iter().any(|name| !expected.contains(&name.as_str())) {
            return Err("svn_invalid_target".into());
        }
        Ok(AssetBasePackage {
            directory,
            metadata,
            content,
            name: parsed.name,
        })
    }

    fn document_dependency_files(&self, file: &Path) -> Result<Vec<PathBuf>, String> {
        let Ok(bytes) = fs::read(file) else {
            return Err("svn_commit_dependency_missing".into());
        };
        let Ok(decoded) = crate::data::artifact::decode_document(&bytes) else {
            return Err("svn_commit_dependency_missing".into());
        };
        if file.file_stem().and_then(|value| value.to_str())
            != Some(decoded.document_id().to_string().as_str())
        {
            return Err("svn_commit_dependency_missing".into());
        }
        let Ok(document) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Err("svn_commit_dependency_missing".into());
        };
        let Some(template) = document
            .get("templateId")
            .and_then(serde_json::Value::as_str)
        else {
            return Err("svn_commit_dependency_missing".into());
        };
        if uuid::Uuid::parse_str(template).is_err() {
            return Err("svn_commit_dependency_missing".into());
        }
        let template_file = self
            .project
            .join("templates")
            .join(format!("{template}.json"));
        let mut dependencies = vec![template_file];
        let mut assets = Vec::new();
        collect_asset_ids(&document, &mut assets);
        for asset in assets {
            if uuid::Uuid::parse_str(&asset).is_err() {
                return Err("svn_commit_dependency_missing".into());
            }
            let Ok(store) = crate::data::assets::Store::open(&self.project, false) else {
                return Err("svn_commit_dependency_missing".into());
            };
            let Ok((metadata, _)) = store.read(&asset) else {
                return Err("svn_commit_dependency_missing".into());
            };
            let directory = self.project.join("assets").join(&asset);
            dependencies.push(directory.join("metadata.json"));
            dependencies.push(directory.join(crate::data::assets::filename(&metadata)));
        }
        Ok(dependencies)
    }

    fn deletion_references_safe(
        &self,
        relative: &ProjectRelativePath,
        entries: &[StatusEntry],
    ) -> bool {
        if let Some(asset) = asset_id_from_path(relative.as_str()) {
            if entries.iter().any(|row| {
                matches!(row.local.as_str(), "deleted" | "missing")
                    && row.path.replace('\\', "/").contains("/documents/")
            }) {
                return false;
            }
            let Ok((_, files)) = crate::data::project_backup::snapshot_paths(&self.project) else {
                return false;
            };
            return files
                .iter()
                .filter(|path| path.starts_with("documents/") && path.ends_with(".json"))
                .all(|path| {
                    fs::read(self.project.join(path))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .is_some_and(|document| {
                            let mut used = Vec::new();
                            collect_asset_ids(&document, &mut used);
                            !used.iter().any(|id| id == asset)
                        })
                });
        }
        match ArtifactSourceId::from_target(relative) {
            Some(ArtifactSourceId::Document(id)) => {
                let layout = self.project.join("workspace/document-layout.json");
                fs::read(layout)
                    .ok()
                    .and_then(|bytes| crate::data::artifact::decode_layout(&bytes).ok())
                    .is_some_and(|layout| !layout.nodes.contains_key(&id))
            }
            Some(ArtifactSourceId::Template(id)) => {
                if entries.iter().any(|row| {
                    matches!(row.local.as_str(), "deleted" | "missing")
                        && row.path.replace('\\', "/").contains("/documents/")
                }) {
                    return false;
                }
                let Ok((_, files)) = crate::data::project_backup::snapshot_paths(&self.project)
                else {
                    return false;
                };
                files
                    .iter()
                    .filter(|path| path.starts_with("documents/") && path.ends_with(".json"))
                    .all(|path| {
                        fs::read(self.project.join(path))
                            .ok()
                            .and_then(|bytes| crate::data::artifact::decode_document(&bytes).ok())
                            .is_some_and(|document| document.template_id() != id)
                    })
            }
            _ => false,
        }
    }

    fn document_dependencies_ready(
        &self,
        file: &Path,
        entries: &[StatusEntry],
        shareable: &crate::svn_shared::Inventory,
    ) -> bool {
        self.document_dependency_files(file)
            .is_ok_and(|dependencies| {
                dependencies.iter().all(|dependency| {
                    svn_dependency_tracked(dependency, entries)
                        || dependency
                            .strip_prefix(&self.project)
                            .ok()
                            .is_some_and(|relative| {
                                shareable
                                    .files
                                    .iter()
                                    .any(|file| Path::new(file) == relative)
                                    && entries.iter().any(|row| {
                                        same_svn_local_path(
                                            &row.path,
                                            &dependency.to_string_lossy(),
                                        ) && matches!(
                                            row.local.as_str(),
                                            "modified" | "added" | "unversioned"
                                        )
                                    })
                            })
                })
            })
    }

    fn document_required_dependencies(
        &self,
        file: &Path,
        entries: &[StatusEntry],
    ) -> Result<Vec<String>, String> {
        let mut required = self
            .document_dependency_files(file)?
            .iter()
            .filter_map(|dependency| {
                let row = entries
                    .iter()
                    .find(|row| same_svn_local_path(&row.path, &dependency.to_string_lossy()))?;
                let changed = matches!(row.local.as_str(), "modified" | "added" | "unversioned")
                    || row.properties.as_deref() == Some("modified");
                changed.then(|| {
                    dependency
                        .strip_prefix(&self.project)
                        .ok()
                        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                })?
            })
            .collect::<Vec<_>>();
        let is_new = entries.iter().any(|row| {
            same_svn_local_path(&row.path, &file.to_string_lossy())
                && matches!(row.local.as_str(), "added" | "unversioned")
        });
        if is_new {
            let layout_file = self.project.join("workspace/document-layout.json");
            if let (Some(id), Ok(bytes)) = (
                file.file_stem()
                    .and_then(|value| value.to_str())
                    .and_then(|value| value.parse().ok()),
                fs::read(&layout_file),
            ) {
                if crate::data::artifact::decode_layout(&bytes)
                    .is_ok_and(|layout| layout.nodes.contains_key(&id))
                    && entries.iter().any(|row| {
                        same_svn_local_path(&row.path, &layout_file.to_string_lossy())
                            && (matches!(row.local.as_str(), "modified" | "added" | "unversioned")
                                || row.properties.as_deref() == Some("modified"))
                    })
                {
                    required.push("workspace/document-layout.json".into());
                }
            }
        }
        Ok(required)
    }

    pub(crate) fn candidates(&self) -> Result<Vec<CommitCandidate>, String> {
        self.with_operation(|manager, cancel| self.candidates_inner(manager, cancel))
    }

    pub(crate) fn schedule_delete(&self, raw: &str) -> Result<(), String> {
        let target = ProjectRelativePath::parse(raw).map_err(|_| "svn_invalid_target")?;
        let asset_id = asset_id_from_path(target.as_str());
        if target.as_str() != raw
            || (ArtifactSourceId::from_target(&target).is_none() && asset_id.is_none())
        {
            return Err("svn_invalid_target".into());
        }
        let file = if asset_id.is_some() {
            self.project.join(target.as_str())
        } else {
            self.target_file(&target)?
        };
        if file.exists() {
            return Err("svn_delete_not_missing".into());
        }
        self.with_operation(|manager, cancel| {
            self.policy_admit_inner(manager, cancel)?;
            let asset_package = asset_id
                .map(|id| self.asset_base_package(manager, cancel, id))
                .transpose()?;
            if asset_package
                .as_ref()
                .is_some_and(|package| package.directory != target.as_str())
            {
                return Err("svn_invalid_target".into());
            }
            let current = status(manager, &self.project, cancel)?;
            if current.server_error.is_some()
                || current.recovery.is_some()
                || !current.entries.iter().any(|row| {
                    same_svn_local_path(&row.path, &file.to_string_lossy())
                        && row.local == "missing"
                        && !row.working_copy_locked
                })
                || !self.deletion_references_safe(&target, &current.entries)
            {
                return Err("svn_delete_not_missing".into());
            }
            let (info, local) = self.inspect_file(manager, &file, cancel)?;
            let remote = self.remote_token(manager, &info.url, cancel)?;
            if remote.is_some() && remote != local {
                return Err("svn_commit_lock_required".into());
            }
            let valid = if asset_package.is_some() {
                true
            } else {
                let base = self.run_for(manager, "cat", &["-r", "BASE"], &file, cancel)?;
                match ArtifactSourceId::from_target(&target) {
                    Some(ArtifactSourceId::Document(id)) => {
                        crate::data::artifact::decode_document(base.as_bytes())
                            .is_ok_and(|document| document.document_id() == id)
                    }
                    Some(ArtifactSourceId::Template(id)) => {
                        crate::data::artifact::decode_template(base.as_bytes())
                            .is_ok_and(|template| template.template_id() == id)
                    }
                    _ => false,
                }
            };
            if !valid {
                return Err("svn_delete_not_missing".into());
            }
            self.run_for(manager, "delete", &["--force"], &file, cancel)
                .map_err(|_| "svn_delete_unverified")?;
            let current =
                status(manager, &self.project, cancel).map_err(|_| "svn_delete_unverified")?;
            let expected = asset_package
                .map(|package| {
                    vec![
                        self.project.join(package.directory),
                        self.project.join(package.metadata),
                        self.project.join(package.content),
                    ]
                })
                .unwrap_or_else(|| vec![file.clone()]);
            if expected.iter().any(|path| {
                !current.entries.iter().any(|row| {
                    same_svn_local_path(&row.path, &path.to_string_lossy())
                        && row.local == "deleted"
                })
            }) {
                return Err("svn_delete_unverified".into());
            }
            Ok(())
        })
    }

    pub(crate) fn commit(
        &self,
        request: String,
        paths: Vec<String>,
        message: String,
    ) -> Result<CommitResult, String> {
        if message.trim().is_empty()
            || message.len() > 4096
            || message.contains('\0')
            || paths.is_empty()
        {
            return Err("svn_commit_invalid".into());
        }
        let mut selected = Vec::new();
        for raw in &paths {
            let path = ProjectRelativePath::parse(raw).map_err(|_| "svn_invalid_target")?;
            if path.as_str() != raw || selected.contains(&path) {
                return Err("svn_invalid_target".into());
            }
            selected.push(path);
        }
        let committed = self.with_request(&request, |manager, cancel| {
            self.policy_admit_inner(manager, cancel)?;
            let candidates = self.candidates_inner(manager, cancel)?;
            let mut files = Vec::new();
            let mut newly_added = Vec::new();
            let mut added_directories = Vec::new();
            let mut deleted = Vec::new();
            for target in &selected {
                let candidate = candidates
                    .iter()
                    .find(|candidate| candidate.path == target.as_str() && candidate.eligible)
                    .ok_or("svn_commit_selection_changed")?;
                if candidate
                    .required
                    .iter()
                    .any(|required| !paths.iter().any(|path| path == required))
                {
                    return Err("svn_commit_dependency_missing".into());
                }
                let file = if ArtifactSourceId::from_target(target).is_some() {
                    if candidate.local == "deleted" {
                        let file = self.target_file(target)?;
                        if file.exists() {
                            return Err("svn_commit_selection_changed".into());
                        }
                        deleted.push(target.clone());
                        file
                    } else {
                        self.file(target)?
                    }
                } else {
                    let file = self.project.join(target.as_str());
                    if candidate.local == "deleted" && asset_id_from_path(target.as_str()).is_some()
                    {
                        if file.exists() {
                            return Err("svn_commit_selection_changed".into());
                        }
                        deleted.push(target.clone());
                        files.push(file);
                        continue;
                    }
                    let root = self
                        .project
                        .canonicalize()
                        .map_err(|_| "svn_project_missing")?;
                    let canonical = file.canonicalize().map_err(|_| "svn_invalid_target")?;
                    if !file.is_file() || !canonical.starts_with(root) {
                        return Err("svn_invalid_target".into());
                    }
                    canonical
                };
                if candidate.new_path {
                    newly_added.push(file.clone());
                }
                files.push(file);
            }
            // Stage only the validated new paths. A failed add/propset leaves
            // them visible in the WC for an explicit retry; no revert occurs.
            if !newly_added.is_empty() {
                let root = self
                    .project
                    .canonicalize()
                    .map_err(|_| "svn_project_missing")?;
                let mut parents = std::collections::BTreeSet::new();
                for file in &newly_added {
                    let relative = file.strip_prefix(&root).map_err(|_| "svn_invalid_target")?;
                    let parts: Vec<_> = relative.iter().collect();
                    if parts.len() == 3 && parts[0] == "assets" {
                        parents.insert(root.join("assets"));
                        parents.insert(root.join("assets").join(parts[1]));
                    }
                }
                for directory in parents {
                    let xml = self.run_for(
                        manager,
                        "status",
                        &["--xml", "--verbose"],
                        &directory,
                        cancel,
                    )?;
                    let (rows, _) = parse_status(&xml)?;
                    let added = rows.iter().any(|row| {
                        same_svn_local_path(&row.path, &directory.to_string_lossy())
                            && row.local == "added"
                    });
                    if !added && inspect(manager, &directory, cancel).is_err() {
                        let args = vec![
                            "add".into(),
                            "--depth".into(),
                            "empty".into(),
                            "--no-auto-props".into(),
                            "--".into(),
                            local_cli_target(&root, &directory)?,
                        ];
                        run_in(&manager.cli()?, &args, None, cancel, &root)
                            .map_err(|_| "svn_commit_local_partial")?;
                        added_directories.push(directory);
                    } else if added {
                        added_directories.push(directory);
                    }
                }
                let targets = CommitTargetsFile::write(&root, &newly_added)?;
                // Some selections were already scheduled for add. SVN add
                // accepts only unversioned paths, so handle each exact state.
                for (candidate, file) in selected.iter().filter_map(|path| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.path == path.as_str() && candidate.new_path)
                        .map(|candidate| (candidate, root.join(path.as_str())))
                }) {
                    if candidate.local == "unversioned" {
                        let args = vec![
                            "add".into(),
                            "--depth".into(),
                            "empty".into(),
                            "--no-auto-props".into(),
                            "--".into(),
                            local_cli_target(&root, &file)?,
                        ];
                        run_in(&manager.cli()?, &args, None, cancel, &root)
                            .map_err(|_| "svn_commit_local_partial")?;
                    }
                }
                let props = vec![
                    "propset".into(),
                    "svn:needs-lock".into(),
                    "yes".into(),
                    "--targets".into(),
                    targets.argument.clone(),
                ];
                run_in(&manager.cli()?, &props, None, cancel, &root)
                    .map_err(|_| "svn_commit_local_partial")?;
            }
            let identity = manager.identity();
            let config = identity
                .as_ref()
                .map(|value| value.config.clone())
                .unwrap_or_else(|| manager.inner.config_root.join("anonymous"));
            let mut args = vec![
                "commit".into(),
                "-m".into(),
                message.clone(),
                "--config-option".into(),
                "config:miscellany:no-unlock=no".into(),
                "--config-dir".into(),
                config.to_string_lossy().into_owned(),
                "--no-auth-cache".into(),
            ];
            if let Some(identity) = &identity {
                args.extend(["--username".into(), identity.username.clone()]);
                if identity.password.is_some() {
                    args.push("--password-from-stdin".into());
                }
            }
            // Windows has a short process command-line limit. `--targets`
            // keeps an exact multi-file selection in one SVN revision.
            added_directories.extend(files.iter().cloned());
            let root = self
                .project
                .canonicalize()
                .map_err(|_| "svn_project_missing")?;
            let targets_file = CommitTargetsFile::write(&root, &added_directories)?;
            args.extend([
                "--non-interactive".into(),
                "--depth".into(),
                "empty".into(),
                "--targets".into(),
                targets_file.argument.clone(),
            ]);
            let output = run_in(
                &manager.cli()?,
                &args,
                identity
                    .as_ref()
                    .and_then(|value| value.password.as_deref()),
                cancel,
                &root,
            )
            .map_err(|_| "svn_commit_unverified")?;
            let revision = output
                .lines()
                .rev()
                .find_map(|line| {
                    line.trim()
                        .strip_prefix("Committed revision ")
                        .and_then(|value| value.strip_suffix('.'))
                        .filter(|value| {
                            !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
                        })
                })
                .ok_or("svn_commit_applied_unverified")?
                .to_owned();
            self.verify_committed_inner(
                manager, cancel, revision, paths, &selected, &files, &deleted,
            )
        })?;
        self.confirmed_commits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                committed.revision.clone(),
                (committed.paths.clone(), committed.deleted.clone()),
            );
        // A physical purge retains its exact lock while the WC path is
        // missing. The verified delete commit ends that ownership.
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        for target in &committed.deleted_confirmed {
            if let Some(target) = selected.iter().find(|selected| selected.as_str() == target) {
                held.remove(target);
            }
        }
        Ok(committed)
    }

    fn post_commit_probe(&self, path: &str, check: &str) -> Result<(), String> {
        #[cfg(test)]
        if let Some(probe) = self
            .post_commit_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            probe(path, check)?;
        }
        #[cfg(not(test))]
        let _ = (path, check);
        Ok(())
    }

    fn verify_committed_inner(
        &self,
        manager: &Manager,
        cancel: &AtomicBool,
        revision: String,
        paths: Vec<String>,
        selected: &[ProjectRelativePath],
        files: &[PathBuf],
        deleted: &[ProjectRelativePath],
    ) -> Result<CommitResult, String> {
        let mut unlock_pending = Vec::new();
        let mut verification_unknown = Vec::new();
        let mut deleted_confirmed = Vec::new();
        for (target, file) in selected.iter().zip(files.iter()) {
            let path = target.as_str();
            if path == policy::FILE {
                let checked = self
                    .post_commit_probe(path, "policyHead")
                    .and_then(|()| self.policy_verify_applied(manager, cancel));
                if let Err(reason) = checked {
                    verification_unknown.push(CommitVerificationUnknown {
                        path: path.to_owned(),
                        check: "policyHead",
                        reason,
                    });
                }
                continue;
            }
            if deleted.contains(target) {
                let check = asset_id_from_path(path)
                    .map(|id| self.project.join("assets").join(id))
                    .unwrap_or_else(|| file.clone());
                let outcome = self
                    .post_commit_probe(path, "delete")
                    .and_then(|()| self.new_path_absent(manager, &check, cancel));
                match outcome {
                    Ok(true) => deleted_confirmed.push(path.to_owned()),
                    Ok(false) => verification_unknown.push(CommitVerificationUnknown {
                        path: path.to_owned(),
                        check: "delete",
                        reason: "svn_delete_unverified".into(),
                    }),
                    Err(reason) => verification_unknown.push(CommitVerificationUnknown {
                        path: path.to_owned(),
                        check: "delete",
                        reason,
                    }),
                }
                continue;
            }
            match self
                .post_commit_probe(path, "localInfo")
                .and_then(|()| self.inspect_file(manager, file, cancel))
            {
                Ok((info, _)) => match self
                    .post_commit_probe(path, "remoteLock")
                    .and_then(|()| self.remote_token(manager, &info.url, cancel))
                {
                    Ok(Some(_)) => unlock_pending.push(path.to_owned()),
                    Ok(None) => {}
                    Err(reason) => verification_unknown.push(CommitVerificationUnknown {
                        path: path.to_owned(),
                        check: "remoteLock",
                        reason,
                    }),
                },
                Err(reason) => verification_unknown.push(CommitVerificationUnknown {
                    path: path.to_owned(),
                    check: "localInfo",
                    reason,
                }),
            }
        }
        Ok(CommitResult {
            revision,
            paths,
            deleted: deleted
                .iter()
                .map(|path| path.as_str().to_owned())
                .collect(),
            unlock_pending,
            verification_unknown,
            deleted_confirmed,
        })
    }

    pub(crate) fn recheck_commit(
        &self,
        revision: String,
        paths: Vec<String>,
        deleted: Vec<String>,
    ) -> Result<CommitResult, String> {
        if revision.is_empty()
            || !revision.bytes().all(|byte| byte.is_ascii_digit())
            || paths.is_empty()
        {
            return Err("svn_commit_invalid".into());
        }
        if self
            .confirmed_commits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&revision)
            != Some(&(paths.clone(), deleted.clone()))
        {
            return Err("svn_commit_invalid".into());
        }
        let mut selected = Vec::new();
        for path in &paths {
            let target = ProjectRelativePath::parse(path).map_err(|_| "svn_invalid_target")?;
            if target.as_str() != path || selected.contains(&target) {
                return Err("svn_invalid_target".into());
            }
            selected.push(target);
        }
        let mut deleted_targets = Vec::new();
        for path in &deleted {
            let target = ProjectRelativePath::parse(path).map_err(|_| "svn_invalid_target")?;
            if !selected.contains(&target) || deleted_targets.contains(&target) {
                return Err("svn_invalid_target".into());
            }
            deleted_targets.push(target);
        }
        let files: Vec<PathBuf> = selected
            .iter()
            .map(|target| self.project.join(target.as_str()))
            .collect();
        let result = self.with_operation(|manager, cancel| {
            self.verify_committed_inner(
                manager,
                cancel,
                revision,
                paths,
                &selected,
                &files,
                &deleted_targets,
            )
        })?;
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        for path in &result.deleted_confirmed {
            if let Some(target) = selected.iter().find(|target| target.as_str() == path) {
                held.remove(target);
            }
        }
        Ok(result)
    }

    fn error(
        &self,
        category: LockErrorCategory,
        operation: LockOperation,
        project: &str,
        session: &LockSessionId,
        target: &ProjectRelativePath,
    ) -> LockError {
        LockError::for_held(
            category,
            LockProviderKind::Svn,
            operation,
            project,
            session,
            target,
        )
    }

    fn checked<'a>(
        &self,
        held: &'a mut dyn HeldLock,
        operation: LockOperation,
    ) -> Result<&'a mut SvnHeld, LockError> {
        if held.provider_kind() != LockProviderKind::Svn
            || held.provider_instance_id() != self.instance
        {
            return Err(self.error(
                LockErrorCategory::HeldLockProviderMismatch,
                operation,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ));
        }
        let project = held.project_fingerprint().to_owned();
        let session = held.session_id().clone();
        let target = held.target().clone();
        held.as_any_mut().downcast_mut::<SvnHeld>().ok_or_else(|| {
            self.error(
                LockErrorCategory::HeldLockProviderMismatch,
                operation,
                &project,
                &session,
                &target,
            )
        })
    }

    /// Canonical target plus component-level reparse checks prevent a selected
    /// canonical artifact path from escaping through an alias inside the WC.
    fn target_file(&self, target: &ProjectRelativePath) -> Result<PathBuf, String> {
        let asset_metadata = {
            let parts: Vec<_> = target.as_str().split('/').collect();
            parts.len() == 3
                && parts[0] == "assets"
                && parts[2] == "metadata.json"
                && crate::data::media::valid_id(parts[1])
                && crate::data::assets::Store::open(&self.project, false)
                    .and_then(|store| store.read(parts[1]).map(|_| ()))
                    .is_ok()
        };
        if !asset_metadata
            && !matches!(
                ArtifactSourceId::from_target(target),
                Some(ArtifactSourceId::Document(_))
                    | Some(ArtifactSourceId::Template(_))
                    | Some(ArtifactSourceId::DocumentLayout)
            )
        {
            return Err("svn_invalid_target".into());
        }
        let root = self
            .project
            .canonicalize()
            .map_err(|_| "svn_project_missing")?;
        let mut file = root.clone();
        let components: Vec<_> = target.as_str().split('/').collect();
        for (index, component) in components.iter().enumerate() {
            file.push(component);
            let metadata = match fs::symlink_metadata(&file) {
                Ok(metadata) => metadata,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && index + 1 == components.len() =>
                {
                    return Ok(file);
                }
                Err(_) => return Err("svn_invalid_target".into()),
            };
            if metadata.file_type().is_symlink() {
                return Err("svn_invalid_target".into());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err("svn_invalid_target".into());
                }
            }
        }
        if !file.is_file()
            || !file
                .canonicalize()
                .is_ok_and(|path| path.starts_with(&root))
        {
            return Err("svn_invalid_target".into());
        }
        Ok(file)
    }

    fn file(&self, target: &ProjectRelativePath) -> Result<PathBuf, String> {
        let file = self.target_file(target)?;
        if !file.is_file() {
            return Err("svn_invalid_target".into());
        }
        Ok(file)
    }

    fn valid_new_artifact(&self, target: &ProjectRelativePath, file: &Path) -> bool {
        let Ok(bytes) = fs::read(file) else {
            return false;
        };
        match ArtifactSourceId::from_target(target) {
            Some(ArtifactSourceId::Template(id)) => crate::data::artifact::decode_template(&bytes)
                .is_ok_and(|artifact| artifact.template_id() == id),
            Some(ArtifactSourceId::Document(id)) => {
                crate::data::artifact::decode_document(&bytes)
                    .is_ok_and(|artifact| artifact.document_id() == id)
                    && crate::data::assets::validate_document(&self.project, &bytes).is_ok()
            }
            Some(ArtifactSourceId::DocumentLayout) => {
                crate::data::artifact::decode_layout(&bytes).is_ok()
            }
            None => false,
        }
    }

    /// A new canonical path has no server lock. Its authority is confined to
    /// one verified absent child under this project's versioned parent.
    fn new_path_absent(
        &self,
        manager: &Manager,
        file: &Path,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        let parent = file.parent().ok_or("svn_invalid_target")?;
        let project = inspect(manager, &self.project, cancel)?;
        if project.url.starts_with("https://")
            && manager.identity().as_ref().is_none_or(|identity| {
                secure_url(&project.url)
                    .map(|url| origin(&url) != identity.origin)
                    .unwrap_or(true)
            })
        {
            return Err("svn_login_required".into());
        }
        let (info, name) = match inspect(manager, parent, cancel) {
            Ok(info) => (
                info,
                file.file_name()
                    .and_then(|value| value.to_str())
                    .ok_or("svn_invalid_target")?
                    .to_owned(),
            ),
            Err(_) => {
                // An imported asset lives below its new flat ID directory.
                // The directory itself is absent on the server, so list its
                // versioned assets parent rather than guessing a child URL.
                let root = self
                    .project
                    .canonicalize()
                    .map_err(|_| "svn_project_missing")?;
                let relative = parent
                    .strip_prefix(root)
                    .map_err(|_| "svn_invalid_target")?;
                let parts: Vec<_> = relative.iter().collect();
                if parts.len() != 2
                    || parts[0] != "assets"
                    || !parts[1].to_str().is_some_and(crate::data::media::valid_id)
                {
                    return Err("svn_invalid_target".into());
                }
                let current = status(manager, &self.project, cancel)?;
                let assets = self.project.join("assets");
                let parent_new = current.entries.iter().any(|row| {
                    same_svn_local_path(&row.path, &parent.to_string_lossy())
                        && matches!(row.local.as_str(), "unversioned" | "added")
                });
                let assets_new = current.entries.iter().any(|row| {
                    same_svn_local_path(&row.path, &assets.to_string_lossy())
                        && matches!(row.local.as_str(), "unversioned" | "added")
                });
                if current.server_error.is_some() || (!parent_new && !assets_new) {
                    return Err("svn_status_unsafe".into());
                }
                if assets_new && inspect(manager, &assets, cancel).is_err() {
                    let args = args_with_auth(
                        manager,
                        manager.identity().as_ref(),
                        "list",
                        &["--xml"],
                        &project.url,
                    );
                    let identity = manager.identity();
                    let xml = run(
                        &manager.cli()?,
                        &args,
                        identity
                            .as_ref()
                            .and_then(|value| value.password.as_deref()),
                        cancel,
                    )?;
                    return Ok(!parse_list_names(&xml)?
                        .iter()
                        .any(|entry| entry == "assets"));
                }
                (
                    inspect(manager, &assets, cancel)?,
                    parts[1].to_string_lossy().into_owned(),
                )
            }
        };
        if project.wc_root != info.wc_root
            || project.repository != info.repository
            || (info.url != project.url
                && !info
                    .url
                    .starts_with(&format!("{}/", project.url.trim_end_matches('/'))))
        {
            return Err("svn_other_working_copy".into());
        }
        let identity = manager.identity();
        if info.url.starts_with("https://")
            && identity.as_ref().is_none_or(|value| {
                secure_url(&info.url)
                    .map(|url| origin(&url) != value.origin)
                    .unwrap_or(true)
            })
        {
            return Err("svn_login_required".into());
        }
        let args = args_with_auth(manager, identity.as_ref(), "list", &["--xml"], &info.url);
        let xml = run(
            &manager.cli()?,
            &args,
            identity
                .as_ref()
                .and_then(|value| value.password.as_deref()),
            cancel,
        )?;
        Ok(!parse_list_names(&xml)?.iter().any(|entry| entry == &name))
    }

    fn with_operation<T>(
        &self,
        operation: impl FnOnce(&Manager, &AtomicBool) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_request(&uuid::Uuid::new_v4().to_string(), operation)
    }

    fn with_request<T>(
        &self,
        request: &str,
        operation: impl FnOnce(&Manager, &AtomicBool) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.manager.cli().is_err() {
            probe(&self.manager, None, None);
        }
        let (_guard, cancel) = self.manager.begin(request)?;
        operation(&self.manager, &cancel)
    }

    fn inspect_file(
        &self,
        manager: &Manager,
        file: &Path,
        cancel: &AtomicBool,
    ) -> Result<(Inspection, Option<String>), String> {
        let project = inspect(manager, &self.project, cancel)?;
        let info = if file.exists() {
            inspect(manager, file, cancel)?
        } else {
            let xml = self.run_for(manager, "info", &["--xml"], file, cancel)?;
            let anchor = file
                .ancestors()
                .find(|path| path.is_dir())
                .ok_or("svn_invalid_target")?;
            parse_info(&xml, anchor)?
        };
        if project.wc_root != info.wc_root
            || project.repository != info.repository
            || !info
                .url
                .starts_with(&format!("{}/", project.url.trim_end_matches('/')))
        {
            return Err("svn_other_working_copy".into());
        }
        let identity = manager.identity();
        if info.url.starts_with("https://") {
            if let Some(identity) = &identity {
                if origin(&secure_url(&info.url)?) != identity.origin {
                    return Err("svn_other_server".into());
                }
            } else {
                return Err("svn_login_required".into());
            }
        }
        let cli = manager.cli()?;
        let args = args_with_auth(
            manager,
            identity.as_ref(),
            "info",
            &["--xml"],
            &file.to_string_lossy(),
        );
        let xml = run(
            &cli,
            &args,
            identity.as_ref().and_then(|i| i.password.as_deref()),
            cancel,
        )?;
        let mut token = parse_lock_token(&xml)?;
        if token.is_none() && !file.exists() {
            // `svn info` omits a WC lock on scheduled-deleted paths, while
            // `svn status --xml` retains the exact local token until commit.
            let status = self.run_for(manager, "status", &["--xml", "--verbose"], file, cancel)?;
            token = parse_lock_token(&status)?;
        }
        Ok((info, token))
    }

    fn remote_token(
        &self,
        manager: &Manager,
        url: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<String>, String> {
        let identity = manager.identity();
        let cli = manager.cli()?;
        let args = args_with_auth(manager, identity.as_ref(), "info", &["--xml"], url);
        let xml = run(
            &cli,
            &args,
            identity.as_ref().and_then(|i| i.password.as_deref()),
            cancel,
        )?;
        parse_lock_token(&xml)
    }

    fn run_for(
        &self,
        manager: &Manager,
        command: &str,
        options: &[&str],
        file: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let identity = manager.identity();
        let local_mutation = matches!(command, "delete" | "lock" | "unlock");
        let root = self
            .project
            .canonicalize()
            .map_err(|_| "svn_project_missing")?;
        let target = if local_mutation {
            let relative = file
                .strip_prefix(&self.project)
                .or_else(|_| file.strip_prefix(&root))
                .map_err(|_| "svn_invalid_target")?;
            local_cli_path(&root, &root.join(relative))?
        } else {
            file.to_string_lossy().into_owned()
        };
        let args = args_with_auth(manager, identity.as_ref(), command, options, &target);
        let secret = identity.as_ref().and_then(|i| i.password.as_deref());
        if local_mutation {
            run_in(&manager.cli()?, &args, secret, cancel, &root)
        } else {
            run(&manager.cli()?, &args, secret, cancel)
        }
    }
}

struct CommitTargetsFile {
    path: PathBuf,
    argument: String,
}

impl CommitTargetsFile {
    fn write(root: &Path, files: &[PathBuf]) -> Result<Self, String> {
        // Keep both the --targets argument and its entries relative to the
        // verified WC. The installed Windows SVN CLI misreads a Unicode WC
        // prefix when it appears in a process argument or targets file.
        let directory = root.join(".worldbuild");
        fs::create_dir_all(&directory).map_err(|_| "svn_commit_invalid")?;
        let metadata = fs::symlink_metadata(&directory).map_err(|_| "svn_commit_invalid")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("svn_commit_invalid".into());
        }
        #[cfg(windows)]
        if std::os::windows::fs::MetadataExt::file_attributes(&metadata) & 0x400 != 0 {
            return Err("svn_commit_invalid".into());
        }
        let name = format!("commit-targets-{}.txt", uuid::Uuid::new_v4());
        let path = directory.join(&name);
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| "svn_commit_invalid")?;
        let guard = Self {
            path,
            argument: format!(".worldbuild/{name}"),
        };
        for file in files {
            let relative = file.strip_prefix(root).map_err(|_| "svn_invalid_target")?;
            let target = relative.to_str().ok_or("svn_invalid_target")?;
            if target.contains(['\r', '\n'])
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("svn_invalid_target".into());
            }
            if target.is_empty() {
                writeln!(output, ".").map_err(|_| "svn_commit_invalid")?;
            } else {
                writeln!(output, "{target}@").map_err(|_| "svn_commit_invalid")?;
            }
        }
        output.flush().map_err(|_| "svn_commit_invalid")?;
        Ok(guard)
    }
}

fn local_cli_target(root: &Path, file: &Path) -> Result<String, String> {
    Ok(format!("{}@", local_cli_path(root, file)?))
}

fn local_cli_path(root: &Path, file: &Path) -> Result<String, String> {
    let relative = file.strip_prefix(root).map_err(|_| "svn_invalid_target")?;
    let target = relative.to_str().ok_or("svn_invalid_target")?;
    if target.is_empty()
        || target.contains(['\r', '\n'])
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("svn_invalid_target".into());
    }
    Ok(target.to_owned())
}

impl Drop for CommitTargetsFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn parse_lock_token(xml: &str) -> Result<Option<String>, String> {
    let mut reader = Reader::from_str(xml);
    let mut inside_lock = false;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"lock") => inside_lock = true,
            Event::End(e) if e.name() == QName(b"lock") => inside_lock = false,
            Event::Start(e) if inside_lock && e.name() == QName(b"token") => {
                return read_xml_text(&mut reader, b"token").map(Some)
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

fn parse_lock_owner(xml: &str) -> Result<DocumentLockInfo, String> {
    let mut reader = Reader::from_str(xml);
    let mut inside_lock = false;
    let mut locked = false;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"lock") => {
                inside_lock = true;
                locked = true;
            }
            Event::End(e) if e.name() == QName(b"lock") => inside_lock = false,
            Event::Start(e) if inside_lock && e.name() == QName(b"owner") => {
                return read_xml_text(&mut reader, b"owner").map(|owner| DocumentLockInfo {
                    locked: true,
                    owner: Some(owner),
                    observation: None,
                })
            }
            Event::Eof => {
                return Ok(DocumentLockInfo {
                    locked,
                    owner: None,
                    observation: None,
                })
            }
            _ => {}
        }
    }
}

fn parse_list_names(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    let mut names = Vec::new();
    let mut in_entry = false;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(event) if event.name() == QName(b"entry") => in_entry = true,
            Event::End(event) if event.name() == QName(b"entry") => in_entry = false,
            Event::Start(event) if in_entry && event.name() == QName(b"name") => {
                names.push(read_xml_text(&mut reader, b"name")?);
            }
            Event::Eof => return Ok(names),
            _ => {}
        }
    }
}

impl LockService for SvnLockService {
    fn authorize_project_operation(&self) -> Result<(), String> {
        self.policy_admit()
    }

    fn provider_info(&self) -> LockProviderInfo {
        LockProviderInfo {
            kind: LockProviderKind::Svn,
            capabilities: LockCapabilities {
                distributed: true,
                validation: true,
                owner_diagnostics: true,
                steal: false,
            },
        }
    }

    fn authorize_new_asset(&self, id: &str) -> Result<(), String> {
        if !crate::data::media::valid_id(id) {
            return Err("svn_invalid_target".into());
        }
        self.with_operation(|manager, cancel| {
            self.policy_admit_inner(manager, cancel)?;
            let root = self
                .project
                .canonicalize()
                .map_err(|_| "svn_project_missing")?;
            let assets = root.join("assets");
            let child = assets.join(id);
            match fs::symlink_metadata(&child) {
                Ok(_) => return Err("svn_new_path_exists".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("svn_invalid_target".into()),
            }
            match fs::symlink_metadata(&assets) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("svn_invalid_target".into()),
                Ok(meta) => {
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Err("svn_invalid_target".into());
                    }
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        if meta.file_attributes() & 0x400 != 0 {
                            return Err("svn_invalid_target".into());
                        }
                    }
                }
            }
            let info = inspect(manager, &root, cancel)?;
            let current = status(manager, &root, cancel)?;
            if current.server_error.is_some() || current.recovery.is_some() {
                return Err("svn_server_unconfirmed".into());
            }
            let identity = manager.identity();
            if info.url.starts_with("https://")
                && identity.as_ref().is_none_or(|value| {
                    secure_url(&info.url)
                        .map(|url| origin(&url) != value.origin)
                        .unwrap_or(true)
                })
            {
                return Err("svn_login_required".into());
            }
            let list = |url: &str| -> Result<Vec<String>, String> {
                let args = args_with_auth(manager, identity.as_ref(), "list", &["--xml"], url);
                let xml = run(
                    &manager.cli()?,
                    &args,
                    identity
                        .as_ref()
                        .and_then(|value| value.password.as_deref()),
                    cancel,
                )?;
                parse_list_names(&xml)
            };
            if !list(&info.url)?.iter().any(|entry| entry == "assets") {
                return Ok(());
            }
            let local_assets = inspect(manager, &assets, cancel)?;
            if local_assets.wc_root != info.wc_root
                || local_assets.repository != info.repository
                || local_assets.url != format!("{}/assets", info.url.trim_end_matches('/'))
            {
                return Err("svn_other_working_copy".into());
            }
            if list(&local_assets.url)?.iter().any(|entry| entry == id) {
                return Err("svn_new_path_exists".into());
            }
            Ok(())
        })
    }

    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        let fail = |category| {
            LockError::for_request(
                category,
                LockProviderKind::Svn,
                LockOperation::Acquire,
                &request,
            )
        };
        self.policy_admit()
            .map_err(|_| fail(LockErrorCategory::LockStateUnknown))?;
        let file = self
            .target_file(request.target())
            .map_err(|_| fail(LockErrorCategory::InvalidLockTarget))?;
        if self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(request.target())
        {
            return Err(fail(LockErrorCategory::AlreadyLocked));
        }
        let (token, new_path) = self
            .with_operation(|manager, cancel| {
                let unregistered = if file.exists() {
                    let xml =
                        self.run_for(manager, "status", &["--xml", "--verbose"], &file, cancel)?;
                    let (rows, _) = parse_status(&xml)?;
                    rows.len() == 1
                        && matches!(rows[0].local.as_str(), "unversioned" | "added")
                        && self.valid_new_artifact(request.target(), &file)
                } else {
                    true
                };
                if unregistered {
                    if !self.new_path_absent(manager, &file, cancel)? {
                        return Err("svn_status_unsafe".into());
                    }
                    let xml =
                        self.run_for(manager, "status", &["--xml", "--verbose"], &file, cancel)?;
                    let (rows, _) = parse_status(&xml)?;
                    if !rows.is_empty()
                        && !(rows.len() == 1
                            && matches!(rows[0].local.as_str(), "unversioned" | "added")
                            && !rows[0].working_copy_locked)
                    {
                        return Err("svn_status_unsafe".into());
                    }
                    return Ok((String::new(), true));
                }
                let (info, current_lock) = self.inspect_file(manager, &file, cancel)?;
                let current = status(manager, &file, cancel)?;
                let entry = current
                    .entries
                    .iter()
                    .find(|row| same_svn_local_path(&row.path, &file.to_string_lossy()))
                    .ok_or("svn_status_unknown")?;
                let locally_editable = entry.local == "normal"
                    || (entry.local == "modified" && current_lock.is_some());
                if !locally_editable
                    || entry.working_copy_locked
                    || entry
                        .remote
                        .as_deref()
                        .is_some_and(|v| v != "none" && v != "normal")
                    || current.server_error.is_some()
                {
                    return Err("svn_status_unsafe".into());
                }
                let props = self.run_for(
                    manager,
                    "propget",
                    &["svn:needs-lock", "--xml"],
                    &file,
                    cancel,
                )?;
                if parse_needs_lock(&props)?.is_empty() {
                    return Err("svn_needs_lock_missing".into());
                }
                if let Some(local_token) = current_lock {
                    // A lock already present in this exact WC carries a token.
                    // A matching username in another WC does not.
                    return if self.remote_token(manager, &info.url, cancel)?.as_deref()
                        == Some(local_token.as_str())
                    {
                        Ok((local_token, false))
                    } else {
                        Err("svn_stale_lock_token".into())
                    };
                }
                self.run_for(
                    manager,
                    "lock",
                    &["-m", "Dreamrugi document editing"],
                    &file,
                    cancel,
                )?;
                let (_, local) = self.inspect_file(manager, &file, cancel)?;
                let remote = self.remote_token(manager, &info.url, cancel)?;
                match (local, remote) {
                    (Some(local), Some(remote)) if local == remote => Ok((local, false)),
                    _ => Err("svn_lock_unverified".into()),
                }
            })
            .map_err(|reason| {
                fail(match reason.as_str() {
                    "svn_already_locked" => LockErrorCategory::AlreadyLocked,
                    "svn_stale_lock_token" => LockErrorCategory::StaleLockToken,
                    "svn_login_required" | "svn_auth_failed" => {
                        LockErrorCategory::AuthenticationRequired
                    }
                    "svn_status_unsafe" | "svn_needs_lock_missing" => {
                        LockErrorCategory::InvalidLockTarget
                    }
                    "svn_lock_unverified" => LockErrorCategory::LockStateUnknown,
                    _ => LockErrorCategory::LockAcquireFailed,
                })
            })?;
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request.target().clone(), token.clone());
        Ok(Box::new(SvnHeld {
            instance: self.instance,
            project_fingerprint: request.project_fingerprint().to_owned(),
            session: request.session_id().clone(),
            target: request.target().clone(),
            token,
            new_path,
            state: HeldLockState::Active,
            validated_scope: 0,
        }))
    }

    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Validate)?;
        let fail = |category| {
            self.error(
                category,
                LockOperation::Validate,
                &held.project_fingerprint,
                &held.session,
                &held.target,
            )
        };
        if held.state != HeldLockState::Active {
            return Err(fail(LockErrorCategory::HeldLockAlreadyReleased));
        }
        let file = self
            .target_file(&held.target)
            .map_err(|_| fail(LockErrorCategory::LockLost))?;
        self.policy_admit()
            .map_err(|_| fail(LockErrorCategory::LockStateUnknown))?;
        let scope = SVN_JOB_SCOPE.get();
        if scope != 0 && held.validated_scope == scope {
            return Ok(());
        }
        let valid = self
            .with_operation(|manager, cancel| {
                if held.new_path {
                    if !self.new_path_absent(manager, &file, cancel)? {
                        return Ok(false);
                    }
                    let xml =
                        self.run_for(manager, "status", &["--xml", "--verbose"], &file, cancel)?;
                    let (rows, _) = parse_status(&xml)?;
                    return Ok(rows.is_empty()
                        || (rows.len() == 1
                            && matches!(rows[0].local.as_str(), "unversioned" | "added")
                            && !rows[0].working_copy_locked));
                }
                let (info, local) = self.inspect_file(manager, &file, cancel)?;
                let remote = self.remote_token(manager, &info.url, cancel)?;
                Ok(local.as_deref() == Some(held.token.as_str())
                    && remote.as_deref() == Some(held.token.as_str()))
            })
            .map_err(|_| fail(LockErrorCategory::LockStateUnknown))?;
        if !valid {
            return Err(fail(LockErrorCategory::LockLost));
        }
        held.validated_scope = scope;
        Ok(())
    }

    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked(held, LockOperation::Release)?;
        let fail = |category| {
            self.error(
                category,
                LockOperation::Release,
                &held.project_fingerprint,
                &held.session,
                &held.target,
            )
        };
        if held.state != HeldLockState::Active {
            return Err(fail(LockErrorCategory::HeldLockAlreadyReleased));
        }
        let file = self
            .target_file(&held.target)
            .map_err(|_| fail(LockErrorCategory::InvalidLockTarget))?;
        if held.new_path {
            self.held
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&held.target);
            held.state = HeldLockState::Released;
            return Ok(());
        }
        self.with_operation(|manager, cancel| {
            let (info, local_token) = self.inspect_file(manager, &file, cancel)?;
            match self.remote_token(manager, &info.url, cancel)? {
                Some(token) if token == held.token => {
                    if local_token.as_deref() != Some(held.token.as_str()) {
                        return Err("svn_lock_lost".into());
                    }
                    let xml =
                        self.run_for(manager, "status", &["--xml", "--verbose"], &file, cancel)?;
                    let (rows, _) = parse_status(&xml)?;
                    let row = rows
                        .iter()
                        .find(|row| same_svn_local_path(&row.path, &file.to_string_lossy()))
                        .ok_or("svn_release_status_unknown")?;
                    if keep_server_lock_after_release(row)? {
                        // Releasing an app edit session does not commit this WC.
                        // Its SVN token stays with this exact WC for a later edit.
                        return Ok(());
                    }
                    self.run_for(manager, "unlock", &[], &file, cancel)?;
                }
                Some(_) => {
                    // The server confirmed that this session's token no longer
                    // owns the lock. Finish only our app handle; never unlock
                    // the successor's lock or discard this WC's local changes.
                    return Ok(());
                }
                // A successful SVN commit normally releases the lock itself.
                None => {}
            }
            if self.remote_token(manager, &info.url, cancel)?.is_some() {
                return Err("svn_unlock_unverified".into());
            }
            Ok(())
        })
        .map_err(|reason| {
            fail(if reason == "svn_lock_lost" {
                LockErrorCategory::LockLost
            } else {
                LockErrorCategory::LockReleaseFailed
            })
        })?;
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&held.target);
        held.state = HeldLockState::Released;
        Ok(())
    }
}

fn keep_server_lock_after_release(row: &StatusEntry) -> Result<bool, String> {
    if row.working_copy_locked {
        return Err("svn_release_status_unknown".into());
    }
    match (
        row.local.as_str(),
        row.properties.as_deref().unwrap_or("none"),
    ) {
        ("modified", "none" | "normal" | "modified") | ("normal", "modified") => Ok(true),
        ("normal", "none" | "normal") => Ok(false),
        _ => Err("svn_release_status_unknown".into()),
    }
}

impl HeldLock for SvnHeld {
    fn provider_kind(&self) -> LockProviderKind {
        LockProviderKind::Svn
    }
    fn provider_instance_id(&self) -> u64 {
        self.instance
    }
    fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }
    fn session_id(&self) -> &LockSessionId {
        &self.session
    }
    fn target(&self) -> &ProjectRelativePath {
        &self.target
    }
    fn state(&self) -> HeldLockState {
        self.state
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn read_xml_text(reader: &mut Reader<&[u8]>, name: &'static [u8]) -> Result<String, String> {
    let text = reader
        .read_text(QName(name))
        .map_err(|_| "svn_xml_invalid")?;
    let decoded = text
        .xml_content(XmlVersion::Explicit1_0)
        .map_err(|_| "svn_xml_invalid")?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.into_owned())
        .map_err(|_| "svn_xml_invalid".into())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Inspection {
    root: String,
    wc_root: String,
    url: String,
    repository: String,
    repository_id: String,
    revision: String,
}

fn parse_info(xml: &str, project: &Path) -> Result<Inspection, String> {
    let mut reader = Reader::from_str(xml);
    let mut revision = None;
    let mut url = None;
    let mut repository = None;
    let mut repository_id = None;
    let mut wc_root = None;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"entry") => {
                for attr in e.attributes().flatten() {
                    if attr.key == QName(b"revision") {
                        revision = Some(
                            attr.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .map_err(|_| "svn_xml_invalid")?
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Start(e) if e.name() == QName(b"url") => {
                url = Some(read_xml_text(&mut reader, b"url")?);
            }
            Event::Start(e) if e.name() == QName(b"wcroot-abspath") => {
                wc_root = Some(read_xml_text(&mut reader, b"wcroot-abspath")?);
            }
            Event::Start(e) if e.name() == QName(b"uuid") => {
                let value = read_xml_text(&mut reader, b"uuid")?;
                uuid::Uuid::parse_str(&value).map_err(|_| "svn_xml_invalid")?;
                repository_id = Some(value);
            }
            Event::Start(e) if e.name() == QName(b"root") => {
                repository = Some(read_xml_text(&mut reader, b"root")?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = project.canonicalize().map_err(|_| "svn_project_missing")?;
    let wc = PathBuf::from(wc_root.ok_or("svn_not_working_copy")?)
        .canonicalize()
        .map_err(|_| "svn_not_working_copy")?;
    if !root.starts_with(&wc) {
        return Err("svn_not_working_copy".into());
    }
    Ok(Inspection {
        root: root.to_string_lossy().into_owned(),
        wc_root: wc.to_string_lossy().into_owned(),
        url: url.ok_or("svn_xml_invalid")?,
        repository: repository.ok_or("svn_xml_invalid")?,
        repository_id: repository_id.ok_or("svn_xml_invalid")?,
        revision: revision.ok_or("svn_xml_invalid")?,
    })
}

fn inspect(manager: &Manager, path: &Path, cancel: &AtomicBool) -> Result<Inspection, String> {
    let cli = manager.cli()?;
    let absolute = path.canonicalize().map_err(|_| "svn_project_missing")?;
    let target = absolute.to_string_lossy();
    let identity = manager.identity();
    let args = args_with_auth(manager, identity.as_ref(), "info", &["--xml"], &target);
    let output = run(
        &cli,
        &args,
        identity.as_ref().and_then(|i| i.password.as_deref()),
        cancel,
    )?;
    let info = parse_info(&output, &absolute)?;
    // A switched or nested checkout is not silently treated as an ordinary
    // private folder. The caller displays the actual URL and scope.
    Ok(info)
}

#[tauri::command]
pub(crate) async fn svn_inspect<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    request: String,
    path: String,
) -> Result<Inspection, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (_guard, cancel) = manager.begin(&request)?;
        inspect(&manager, Path::new(&path), &cancel)
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_login<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
    request: String,
    url: String,
    username: String,
    password: String,
    remember: bool,
) -> Result<Inspection, String> {
    main_only(&webview)?;
    if app.svn_lock_active() {
        return Err("svn_release_lock_first".into());
    }
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (_guard, cancel) = manager.begin(&request)?;
        let parsed = secure_url(&url)?;
        if username.is_empty()
            || username.len() > 256
            || password.is_empty()
            || password.len() > 4096
            || username.contains(['\r', '\n', '\0'])
            || password.contains(['\r', '\n', '\0'])
        {
            return Err("svn_credentials_invalid".into());
        }
        let cli = manager.cli()?;
        let origin = origin(&parsed);
        let config = config_for(&manager, &origin, &username);
        // An explicit login must test the supplied secret. A preexisting cache
        // could otherwise make a wrong password appear successful.
        let attempt = manager
            .inner
            .config_root
            .join(format!("attempt-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&attempt).map_err(|_| "svn_config_failed")?;
        let identity = Identity {
            origin,
            url: parsed.as_str().into(),
            username,
            config: attempt.clone(),
            password: Some(password),
            remember,
        };
        let args = args_with_auth(
            &manager,
            Some(&identity),
            "info",
            &["--xml"],
            parsed.as_str(),
        );
        let output = match run(&cli, &args, identity.password.as_deref(), &cancel) {
            Ok(output) => output,
            Err(error) => {
                let _ = fs::remove_dir_all(&attempt);
                return Err(error);
            }
        };
        // URL info lacks wcroot; parse its repository/URL without fabricating a
        // checked-out project. The response contains only URL and revision.
        let remote = match parse_remote_info(&output) {
            Ok(remote) => remote,
            Err(error) => {
                let _ = fs::remove_dir_all(&attempt);
                return Err(error);
            }
        };
        if remember {
            let encrypted = protect_password(identity.password.as_deref().unwrap())?;
            fs::write(attempt.join(CREDENTIAL_FILE), encrypted).map_err(|_| "svn_config_failed")?;
        }
        let current = manager.inner.config_root.join("current.json");
        let final_config = if remember {
            if config.exists() {
                fs::remove_dir_all(&config).map_err(|_| "svn_config_failed")?;
            }
            fs::rename(&attempt, &config).map_err(|_| "svn_config_failed")?;
            config
        } else {
            attempt
        };
        if remember {
            let metadata = Remembered {
                origin: identity.origin.clone(),
                url: identity.url.clone(),
                username: identity.username.clone(),
            };
            fs::write(
                &current,
                serde_json::to_vec(&metadata).map_err(|_| "svn_config_failed")?,
            )
            .map_err(|_| "svn_config_failed")?;
        } else if current.exists() {
            fs::remove_file(&current).map_err(|_| "svn_config_failed")?;
        }
        if let Some(previous) = manager.identity() {
            if previous.config != final_config && previous.config.exists() {
                fs::remove_dir_all(&previous.config).map_err(|_| "svn_config_failed")?;
            }
        }
        *manager
            .inner
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Identity {
            config: final_config,
            ..identity
        });
        Ok(remote)
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?
}

fn parse_remote_info(xml: &str) -> Result<Inspection, String> {
    let mut reader = Reader::from_str(xml);
    let mut url = None;
    let mut repository = None;
    let mut repository_id = None;
    let mut revision = None;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"entry") => {
                for attr in e.attributes().flatten() {
                    if attr.key == QName(b"revision") {
                        revision = Some(
                            attr.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .map_err(|_| "svn_xml_invalid")?
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Start(e) if e.name() == QName(b"url") => {
                url = Some(read_xml_text(&mut reader, b"url")?);
            }
            Event::Start(e) if e.name() == QName(b"uuid") => {
                let value = read_xml_text(&mut reader, b"uuid")?;
                uuid::Uuid::parse_str(&value).map_err(|_| "svn_xml_invalid")?;
                repository_id = Some(value);
            }
            Event::Start(e) if e.name() == QName(b"root") => {
                repository = Some(read_xml_text(&mut reader, b"root")?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(Inspection {
        root: String::new(),
        wc_root: String::new(),
        url: url.ok_or("svn_xml_invalid")?,
        repository: repository.ok_or("svn_xml_invalid")?,
        repository_id: repository_id.ok_or("svn_xml_invalid")?,
        revision: revision.ok_or("svn_xml_invalid")?,
    })
}

#[tauri::command]
pub(crate) fn svn_logout<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
) -> Result<(), String> {
    main_only(&webview)?;
    if app.svn_lock_active() {
        return Err("svn_release_lock_first".into());
    }
    let active = manager
        .inner
        .active
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if active.is_some() {
        return Err("svn_busy".into());
    }
    let identity = manager
        .inner
        .identity
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(identity) = identity {
        // The exact hash child belongs to this app/account. Never touch the
        // user's TortoiseSVN or default Subversion authentication cache.
        if identity.config.parent() != Some(manager.inner.config_root.as_path()) {
            return Err("svn_config_failed".into());
        }
        if identity.config.exists() {
            fs::remove_dir_all(&identity.config).map_err(|_| "svn_logout_incomplete")?;
        }
        let current = manager.inner.config_root.join("current.json");
        if current.exists() {
            fs::remove_file(current).map_err(|_| "svn_logout_incomplete")?;
        }
        *manager
            .inner
            .identity
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }
    drop(active);
    Ok(())
}

#[tauri::command]
pub(crate) fn svn_cancel<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    request: String,
) -> Result<bool, String> {
    main_only(&webview)?;
    let active = manager
        .inner
        .active
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(active) = active.as_ref().filter(|active| active.request == request) {
        active.cancel.store(true, Ordering::Release);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub(crate) async fn svn_checkout<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    request: String,
    url: String,
    destination: String,
) -> Result<Inspection, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    let _custody = manager.result_custody()?;
    let observer = manager.clone();
    let operation = request.clone();
    let target = destination.clone();
    let target_url = url.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (_guard, cancel) = manager.begin(&request)?;
        let parsed = secure_url(&url)?;
        let identity = manager.identity().ok_or("svn_login_required")?;
        if origin(&parsed) != identity.origin {
            return Err("svn_other_server".into());
        }
        let path = PathBuf::from(destination);
        if !path.is_absolute() || path.to_string_lossy().len() > 32768 {
            return Err("svn_invalid_destination".into());
        }
        if path.exists()
            && fs::read_dir(&path)
                .map_err(|_| "svn_destination_unavailable")?
                .next()
                .is_some()
        {
            return Err("svn_destination_not_empty".into());
        }
        if !path.parent().is_some_and(Path::is_dir) {
            return Err("svn_destination_unavailable".into());
        }
        if path.exists()
            && fs::symlink_metadata(&path)
                .map_err(|_| "svn_destination_unavailable")?
                .file_type()
                .is_symlink()
        {
            return Err("svn_destination_unavailable".into());
        }
        if !path.exists() {
            fs::create_dir(&path).map_err(|_| "svn_destination_unavailable")?;
        }
        let cli = manager.cli()?;
        let mut args = args_with_auth(
            &manager,
            Some(&identity),
            "checkout",
            &["--depth", "infinity", "--ignore-externals"],
            parsed.as_str(),
        );
        args.push(".".into());
        run_command_in(&cli, &args, identity.password.as_deref(), &cancel, &path)
            .map_err(|_| "svn_checkout_may_be_partial")?;
        inspect(&manager, &path, &cancel).map_err(|_| "svn_checkout_applied_unverified".into())
    })
    .await
    .unwrap_or_else(|_| Err("svn_task_failed".to_owned()));
    observer.preserve_uncertain(
        &operation,
        &target,
        "checkout",
        Some(&target_url),
        result.as_ref().err().map(String::as_str),
    );
    result
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegisterPreview {
    root: String,
    url: String,
    directories: Vec<String>,
    files: Vec<String>,
    excluded_files: usize,
    fingerprint: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RegisterResult {
    root: String,
    url: String,
    setup_revision: String,
    revision: String,
    files: usize,
}

fn registration_input(
    manager: &Manager,
    root: &str,
    url: &str,
) -> Result<(PathBuf, Url, Identity, crate::svn_shared::Inventory), String> {
    let identity = manager.identity().ok_or("svn_login_required")?;
    let base = secure_url(&identity.url)?;
    let target = secure_url(url)?;
    let base_parts: Vec<_> = base
        .path_segments()
        .ok_or("svn_register_target_invalid")?
        .filter(|part| !part.is_empty())
        .collect();
    let target_parts: Vec<_> = target
        .path_segments()
        .ok_or("svn_register_target_invalid")?
        .filter(|part| !part.is_empty())
        .collect();
    if base.query().is_some()
        || target.query().is_some()
        || target.origin() != base.origin()
        || target_parts.len() != base_parts.len() + 1
        || target_parts[..base_parts.len()] != base_parts
        || target_parts.last().is_some_and(|part| {
            let lower = part.to_ascii_lowercase();
            lower.contains("%2f") || lower.contains("%5c") || lower == "." || lower == ".."
        })
    {
        return Err("svn_register_target_invalid".into());
    }
    let path = PathBuf::from(root);
    if !path.is_absolute() || crate::svn_guard::working_copy_root(&path).is_some() {
        return Err("svn_register_source_invalid".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "svn_register_source_invalid")?;
    if !canonical.is_dir() {
        return Err("svn_register_source_invalid".into());
    }
    // Windows canonicalize() returns a verbatim \\?\ path. Subversion's
    // checkout CLI interprets that form as a different destination. Keep the
    // resolved path, but pass its ordinary Windows spelling to the CLI.
    let local = ordinary_windows_path(&canonical);
    let inventory = crate::svn_shared::registration_inventory(&local)?;
    Ok((local, target, identity, inventory))
}

fn ordinary_windows_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path.to_path_buf()
}

#[tauri::command]
pub(crate) async fn svn_register_preview<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
    request: String,
    root: String,
    url: String,
) -> Result<RegisterPreview, String> {
    main_only(&webview)?;
    if app
        .managed_project_roots()
        .iter()
        .any(|open| open.canonicalize().ok() == Path::new(&root).canonicalize().ok())
    {
        return Err("svn_close_project_first".into());
    }
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (_guard, _cancel) = manager.begin(&request)?;
        let (root, url, _, inventory) = registration_input(&manager, &root, &url)?;
        Ok(RegisterPreview {
            root: root.to_string_lossy().into_owned(),
            url: url.to_string(),
            directories: inventory.directories,
            files: inventory.files,
            excluded_files: inventory.excluded_files,
            fingerprint: inventory.fingerprint,
        })
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_register<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
    request: String,
    root: String,
    url: String,
    message: String,
    fingerprint: String,
    resume_empty_child: bool,
) -> Result<RegisterResult, String> {
    main_only(&webview)?;
    if app
        .managed_project_roots()
        .iter()
        .any(|open| open.canonicalize().ok() == Path::new(&root).canonicalize().ok())
    {
        return Err("svn_close_project_first".into());
    }
    let manager = manager.inner().clone();
    let _custody = manager.result_custody()?;
    let observer = manager.clone();
    let target = root.clone();
    let target_url = url.clone();
    let correlation = request.clone();
    crate::diagnostic_log::record(
        "svn",
        "register",
        "project",
        "started",
        Some(correlation.clone()),
        None,
    );
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<RegisterResult, String> {
        let (_guard, cancel) = manager.begin(&request)?;
        if message.trim().is_empty() || message.len() > 4096 || message.contains('\0') {
            return Err("svn_commit_invalid".into());
        }
        if manager.cli().is_err() {
            probe(&manager, None, None);
        }
        let (root, url, identity, inventory) = registration_input(&manager, &root, &url)?;
        if fingerprint != inventory.fingerprint {
            return Err("svn_commit_selection_changed".into());
        }
        let cli = manager.cli()?;
        // A local attempt records the binding. Resume authority is acquired
        // only after this exact mkdir's new child is verified on the server.
        let attempt = RegistrationAttempt {
            root: root.clone(),
            url: url.to_string(),
            fingerprint: fingerprint.clone(),
            username: identity.username.clone(),
            setup: RegistrationSetup::Pending,
        };
        let setup_revision = registration_setup(
            &manager,
            &cli,
            &identity,
            &url,
            attempt,
            resume_empty_child,
            &cancel,
        )?;
        registration_finish(
            &manager,
            &cli,
            identity,
            url,
            root,
            inventory,
            fingerprint,
            setup_revision,
            message,
            &cancel,
        )
    })
    .await
    .unwrap_or_else(|_| Err("svn_task_failed".to_owned()));
    observer.preserve_uncertain(
        &correlation,
        &target,
        "register",
        Some(&target_url),
        result.as_ref().err().map(String::as_str),
    );
    let (category, outcome) = match &result {
        Ok(_) => ("project", "verified"),
        Err(reason) if reason.ends_with("_unverified") => ("result_unverified", "unknown"),
        Err(reason) if reason.ends_with("_partial") => ("local_partial", "partial"),
        Err(_) => ("register_rejected", "failed"),
    };
    crate::diagnostic_log::record(
        "svn",
        "register",
        category,
        outcome,
        Some(correlation),
        None,
    );
    result
}

fn registration_finish(
    manager: &Manager,
    cli: &Path,
    identity: Identity,
    url: Url,
    root: PathBuf,
    inventory: crate::svn_shared::Inventory,
    fingerprint: String,
    setup_revision: String,
    message: String,
    cancel: &AtomicBool,
) -> Result<RegisterResult, String> {
    let mut checkout_args = args_with_auth(
        &manager,
        Some(&identity),
        "checkout",
        &["--force", "--depth", "empty", "--ignore-externals"],
        url.as_str(),
    );
    checkout_args.push(".".into());
    run_in(
        &cli,
        &checkout_args,
        identity.password.as_deref(),
        &cancel,
        &root,
    )
    .map_err(|_| "svn_register_checkout_unverified")?;
    let observed =
        inspect(&manager, &root, &cancel).map_err(|_| "svn_register_checkout_unverified")?;
    if observed.url.trim_end_matches('/') != url.as_str().trim_end_matches('/')
        || observed.root != observed.wc_root
    {
        return Err("svn_register_checkout_unverified".into());
    }
    for directory in &inventory.directories {
        fs::create_dir_all(root.join(directory)).map_err(|_| "svn_register_local_partial")?;
    }
    let all: Vec<_> = inventory
        .directories
        .iter()
        .chain(inventory.files.iter())
        .map(|path| root.join(path))
        .collect();
    if !all.is_empty() {
        let targets = CommitTargetsFile::write(&root, &all)?;
        let add_args = vec![
            "add".into(),
            "--depth".into(),
            "empty".into(),
            "--no-auto-props".into(),
            "--targets".into(),
            targets.argument.clone(),
        ];
        run_in(&cli, &add_args, None, &cancel, &root).map_err(|_| "svn_register_local_partial")?;
    }
    if !inventory.files.is_empty() {
        let files: Vec<_> = inventory.files.iter().map(|path| root.join(path)).collect();
        let targets = CommitTargetsFile::write(&root, &files)?;
        let props_args = vec![
            "propset".into(),
            "svn:needs-lock".into(),
            "yes".into(),
            "--targets".into(),
            targets.argument.clone(),
        ];
        run_in(&cli, &props_args, None, &cancel, &root)
            .map_err(|_| "svn_register_local_partial")?;
    }
    let ignore_args = args_with_auth(
        &manager,
        Some(&identity),
        "propset",
        &["svn:ignore", ".worldbuild\n.git\nlogs\ncache"],
        ".",
    );
    run_in(&cli, &ignore_args, None, &cancel, &root).map_err(|_| "svn_register_local_partial")?;
    if crate::svn_shared::registration_inventory(&root)?.fingerprint != fingerprint {
        return Err("svn_commit_selection_changed".into());
    }
    let mut commit_paths = Vec::with_capacity(all.len() + 1);
    commit_paths.push(root.clone());
    commit_paths.extend(all);
    let targets = CommitTargetsFile::write(&root, &commit_paths)?;
    let config = identity.config.to_string_lossy().into_owned();
    let mut commit_args = vec![
        "commit".into(),
        "-m".into(),
        message,
        "--depth".into(),
        "empty".into(),
        "--config-dir".into(),
        config,
        "--no-auth-cache".into(),
    ];
    commit_args.extend(["--username".into(), identity.username.clone()]);
    if identity.password.is_some() {
        commit_args.push("--password-from-stdin".into());
    }
    commit_args.extend([
        "--non-interactive".into(),
        "--targets".into(),
        targets.argument.clone(),
    ]);
    let commit_output = run_in(
        &cli,
        &commit_args,
        identity.password.as_deref(),
        &cancel,
        &root,
    )
    .map_err(|_| "svn_register_commit_unverified")?;
    let revision = committed_revision(&commit_output).ok_or("svn_register_commit_unverified")?;
    let current =
        status(&manager, &root, &cancel).map_err(|_| "svn_register_applied_unverified")?;
    if current.server_error.is_some()
        || current.info.url.trim_end_matches('/') != url.as_str().trim_end_matches('/')
        || inventory.files.iter().any(|path| {
            let absolute = root.join(path);
            !current.entries.iter().any(|entry| {
                same_svn_local_path(&entry.path, &absolute.to_string_lossy())
                    && entry.local == "normal"
                    && entry.needs_lock
            })
        })
    {
        return Err("svn_register_applied_unverified".into());
    }
    crate::svn_guard::remember(&root).map_err(|_| "svn_register_applied_unverified")?;
    *manager
        .inner
        .registration_attempt
        .lock()
        .map_err(|_| "svn_register_applied_unverified")? = None;
    Ok(RegisterResult {
        root: root.to_string_lossy().into_owned(),
        url: url.to_string(),
        setup_revision,
        revision,
        files: inventory.files.len(),
    })
}

fn registration_setup(
    manager: &Manager,
    cli: &Path,
    identity: &Identity,
    url: &Url,
    mut attempt: RegistrationAttempt,
    resume: bool,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if resume {
        let active = manager
            .inner
            .registration_attempt
            .lock()
            .map_err(|_| "svn_register_resume_denied")?;
        let revision = active
            .as_ref()
            .filter(|prior| prior.matches(&attempt))
            .and_then(RegistrationAttempt::created_revision)
            .ok_or("svn_register_resume_denied")?
            .to_owned();
        drop(active);
        verify_empty_registration_child(manager, cli, identity, url, &revision, cancel)?;
        return Ok(revision);
    }
    *manager
        .inner
        .registration_attempt
        .lock()
        .map_err(|_| "svn_register_setup_unverified")? = Some(attempt.clone());
    let mkdir_args = args_with_auth(
        manager,
        Some(identity),
        "mkdir",
        &["-m", "Create Dreamrugi project"],
        url.as_str(),
    );
    let setup_output = match run(cli, &mkdir_args, identity.password.as_deref(), cancel) {
        Ok(output) => output,
        Err(reason) => {
            let definite_failure = matches!(
                reason.as_str(),
                "svn_path_exists" | "svn_auth_failed" | "svn_tls_failed"
            );
            attempt.setup = RegistrationSetup::Unknown;
            *manager
                .inner
                .registration_attempt
                .lock()
                .map_err(|_| "svn_register_setup_unverified")? = if definite_failure {
                None
            } else {
                Some(attempt)
            };
            return Err(match reason.as_str() {
                "svn_path_exists" => "svn_register_path_exists",
                "svn_auth_failed" => "svn_auth_failed",
                "svn_tls_failed" => "svn_tls_failed",
                _ => "svn_register_setup_unverified",
            }
            .into());
        }
    };
    let revision = committed_revision(&setup_output);
    let verified = revision.as_deref().is_some_and(|revision| {
        verify_empty_registration_child(manager, cli, identity, url, revision, cancel).is_ok()
    });
    if !verified {
        attempt.setup = RegistrationSetup::Unknown;
        *manager
            .inner
            .registration_attempt
            .lock()
            .map_err(|_| "svn_register_setup_unverified")? = Some(attempt);
        return Err("svn_register_setup_unverified".into());
    }
    let revision = revision.ok_or("svn_register_setup_unverified")?;
    attempt.setup = RegistrationSetup::Created {
        revision: revision.clone(),
    };
    *manager
        .inner
        .registration_attempt
        .lock()
        .map_err(|_| "svn_register_setup_unverified")? = Some(attempt);
    Ok(revision)
}

fn verify_empty_registration_child(
    manager: &Manager,
    cli: &Path,
    identity: &Identity,
    url: &Url,
    expected_revision: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let check = |item| {
        let args = args_with_auth(
            manager,
            Some(identity),
            "info",
            &["--show-item", item],
            url.as_str(),
        );
        run(cli, &args, identity.password.as_deref(), cancel)
            .map(|value| value.trim().to_owned())
            .map_err(|_| "svn_register_resume_denied".to_owned())
    };
    let revision = check("last-changed-revision")?;
    if revision != expected_revision {
        return Err("svn_register_resume_denied".into());
    }
    if check("last-changed-author")? != identity.username {
        return Err("svn_register_resume_denied".into());
    }
    let args = args_with_auth(
        manager,
        Some(identity),
        "list",
        &["--depth", "immediates"],
        url.as_str(),
    );
    let listed = run(cli, &args, identity.password.as_deref(), cancel)
        .map_err(|_| "svn_register_resume_denied")?;
    if !listed.trim().is_empty() {
        return Err("svn_register_resume_denied".into());
    }
    verify_registration_creation_log(manager, cli, identity, url, expected_revision, cancel)
}

fn verify_registration_creation_log(
    manager: &Manager,
    cli: &Path,
    identity: &Identity,
    url: &Url,
    revision: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let relative_args = args_with_auth(
        manager,
        Some(identity),
        "info",
        &["--show-item", "relative-url"],
        url.as_str(),
    );
    let relative = run(cli, &relative_args, identity.password.as_deref(), cancel)
        .map_err(|_| "svn_register_resume_denied")?;
    let encoded = relative
        .trim()
        .strip_prefix('^')
        .ok_or("svn_register_resume_denied")?;
    let expected = decode_registration_relative_url(encoded)?;
    let args = args_with_auth(
        manager,
        Some(identity),
        "log",
        &["--xml", "-v", "-r", revision],
        url.as_str(),
    );
    let xml = run(cli, &args, identity.password.as_deref(), cancel)
        .map_err(|_| "svn_register_resume_denied")?;
    if registration_log_added_exact_child(&xml, revision, &identity.username, &expected)? {
        Ok(())
    } else {
        Err("svn_register_resume_denied".into())
    }
}

fn decode_registration_relative_url(encoded: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("svn_register_resume_denied".into());
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .map_err(|_| "svn_register_resume_denied")?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| "svn_register_resume_denied")?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| "svn_register_resume_denied".into())
}

fn registration_log_added_exact_child(
    xml: &str,
    revision: &str,
    author: &str,
    expected: &str,
) -> Result<bool, String> {
    let mut reader = Reader::from_str(xml);
    let mut revision_found = false;
    let mut author_found = false;
    let mut added = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| "svn_register_resume_denied")?
        {
            Event::Start(entry) if entry.name() == QName(b"logentry") => {
                revision_found = entry.attributes().flatten().any(|attribute| {
                    attribute.key == QName(b"revision")
                        && attribute.value.as_ref() == revision.as_bytes()
                });
            }
            Event::Start(entry) if entry.name() == QName(b"author") => {
                author_found = read_xml_text(&mut reader, b"author")? == author;
            }
            Event::Start(entry) if entry.name() == QName(b"path") => {
                let action_added = entry.attributes().flatten().any(|attribute| {
                    attribute.key == QName(b"action") && attribute.value.as_ref() == b"A"
                });
                let kind_directory = entry.attributes().flatten().any(|attribute| {
                    attribute.key == QName(b"kind") && attribute.value.as_ref() == b"dir"
                });
                let path = read_xml_text(&mut reader, b"path")?;
                added |= action_added && kind_directory && path == expected;
            }
            Event::Eof => return Ok(revision_found && author_found && added),
            _ => {}
        }
    }
}

fn committed_revision(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("Committed revision ")
            .and_then(|value| value.strip_suffix('.'))
            .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    info: Inspection,
    entries: Vec<StatusEntry>,
    server_revision: Option<String>,
    server_error: Option<String>,
    recovery: Option<String>,
    update_block: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusEntry {
    path: String,
    local: String,
    properties: Option<String>,
    remote: Option<String>,
    remote_properties: Option<String>,
    lock_owner: Option<String>,
    wc_locked: bool,
    working_copy_locked: bool,
    needs_lock: bool,
    remote_only: bool,
}

fn parse_status(xml: &str) -> Result<(Vec<StatusEntry>, Option<String>), String> {
    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut current: Option<StatusEntry> = None;
    let mut against = None;
    let mut in_wc_status = false;
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"entry") => {
                let path = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key == QName(b"path"))
                    .and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .ok()
                            .map(|v| v.into_owned())
                    })
                    .ok_or("svn_xml_invalid")?;
                current = Some(StatusEntry {
                    path,
                    local: "unknown".into(),
                    properties: None,
                    remote: None,
                    remote_properties: None,
                    lock_owner: None,
                    wc_locked: false,
                    working_copy_locked: false,
                    needs_lock: false,
                    remote_only: false,
                });
            }
            Event::Start(e) if e.name() == QName(b"wc-status") => {
                in_wc_status = true;
                if let Some(current) = current.as_mut() {
                    current.working_copy_locked = e
                        .attributes()
                        .flatten()
                        .any(|a| a.key == QName(b"wc-locked") && a.value.as_ref() == b"true");
                    current.local = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key == QName(b"item"))
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        })
                        .unwrap_or_else(|| "unknown".into());
                    current.properties = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key == QName(b"props"))
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        });
                }
            }
            Event::Empty(e) if e.name() == QName(b"wc-status") => {
                in_wc_status = false;
                if let Some(current) = current.as_mut() {
                    for attr in e.attributes().flatten() {
                        if attr.key == QName(b"item") {
                            current.local = attr
                                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .map_err(|_| "svn_xml_invalid")?
                                .into_owned();
                        } else if attr.key == QName(b"props") {
                            current.properties = Some(
                                attr.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                    .map_err(|_| "svn_xml_invalid")?
                                    .into_owned(),
                            );
                        } else if attr.key == QName(b"wc-locked") {
                            current.working_copy_locked = attr.value.as_ref() == b"true";
                        }
                    }
                }
            }
            Event::Start(e) | Event::Empty(e) if e.name() == QName(b"repos-status") => {
                in_wc_status = false;
                if let Some(current) = current.as_mut() {
                    current.remote = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key == QName(b"item"))
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        });
                    current.remote_properties = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key == QName(b"props"))
                        .and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                .ok()
                                .map(|v| v.into_owned())
                        });
                }
            }
            Event::Start(e) if e.name() == QName(b"lock") && in_wc_status => {
                if let Some(current) = current.as_mut() {
                    current.wc_locked = true;
                }
            }
            Event::End(e) if e.name() == QName(b"wc-status") => in_wc_status = false,
            Event::Start(e) | Event::Empty(e) if e.name() == QName(b"against") => {
                against = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key == QName(b"revision"))
                    .and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .ok()
                            .map(|v| v.into_owned())
                    });
            }
            Event::Start(e) if e.name() == QName(b"owner") => {
                if let Some(current) = current.as_mut() {
                    current.lock_owner = Some(read_xml_text(&mut reader, b"owner")?);
                }
            }
            Event::End(e) if e.name() == QName(b"entry") => {
                if let Some(entry) = current.take() {
                    entries.push(entry);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((entries, against))
}

fn parse_needs_lock(xml: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    let mut target = None;
    let mut paths = Vec::new();
    loop {
        match reader.read_event().map_err(|_| "svn_xml_invalid")? {
            Event::Start(e) if e.name() == QName(b"target") => {
                target = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key == QName(b"path"))
                    .and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                            .ok()
                            .map(|v| v.into_owned())
                    });
            }
            Event::Start(e) | Event::Empty(e) if e.name() == QName(b"property") => {
                let named = e
                    .attributes()
                    .flatten()
                    .any(|a| a.key == QName(b"name") && a.value.as_ref() == b"svn:needs-lock");
                if named {
                    paths.push(target.clone().ok_or("svn_xml_invalid")?);
                }
            }
            Event::End(e) if e.name() == QName(b"target") => target = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(paths)
}

fn same_svn_local_path(left: &str, right: &str) -> bool {
    fn normalized(path: &str) -> String {
        let path = path.replace('\\', "/").to_lowercase();
        if let Some(unc) = path.strip_prefix("//?/unc/") {
            format!("//{unc}")
        } else {
            path.strip_prefix("//?/").unwrap_or(&path).to_owned()
        }
    }
    normalized(left) == normalized(right)
}

fn svn_local_relative(root: &Path, value: &str) -> Option<String> {
    fn normalized(value: &str) -> String {
        let value = value.replace('\\', "/");
        if let Some(unc) = value.strip_prefix("//?/UNC/") {
            format!("//{unc}")
        } else {
            value.strip_prefix("//?/").unwrap_or(&value).to_owned()
        }
    }
    let root = normalized(&root.to_string_lossy());
    let value = normalized(value);
    let prefix = format!("{}/", root.trim_end_matches('/'));
    if !value
        .get(..prefix.len())
        .is_some_and(|leading| leading.eq_ignore_ascii_case(&prefix))
    {
        return None;
    }
    Some(value[prefix.len()..].to_owned())
}

fn private_unversioned_entry(root: &Path, entry: &StatusEntry) -> bool {
    if entry.local != "unversioned" {
        return false;
    }
    svn_local_relative(root, &entry.path).is_some_and(|relative| {
        let relative = relative.replace('\\', "/").to_ascii_lowercase();
        relative == ".worldbuild"
            || relative.starts_with(".worldbuild/")
            || relative == "assets/.trash"
            || relative.starts_with("assets/.trash/")
    })
}

fn asset_id_from_path(value: &str) -> Option<&str> {
    let mut parts = value.split('/');
    if parts.next()? != "assets" {
        return None;
    }
    let id = parts.next()?;
    if !crate::data::media::valid_id(id) || parts.clone().count() > 1 {
        return None;
    }
    Some(id)
}

fn svn_dependency_tracked(file: &Path, entries: &[StatusEntry]) -> bool {
    let path = file.to_string_lossy();
    entries.iter().any(|row| {
        same_svn_local_path(&row.path, &path)
            && row.local == "normal"
            && row
                .properties
                .as_deref()
                .is_none_or(|value| value == "none" || value == "normal")
            && row
                .remote
                .as_deref()
                .is_none_or(|value| value == "none" || value == "normal")
    })
}

fn collect_asset_ids(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if matches!(
                object.get("kind").and_then(serde_json::Value::as_str),
                Some("file" | "image")
            ) {
                if let Some(ids) = object.get("value").and_then(serde_json::Value::as_array) {
                    for id in ids {
                        if let Some(id) = id.as_str() {
                            found.push(id.to_owned());
                        }
                    }
                }
            }
            for nested in object.values() {
                collect_asset_ids(nested, found);
            }
        }
        serde_json::Value::Array(values) => {
            for nested in values {
                collect_asset_ids(nested, found);
            }
        }
        _ => {}
    }
}

fn status(manager: &Manager, path: &Path, cancel: &AtomicBool) -> Result<Status, String> {
    let info = inspect(manager, path, cancel)?;
    let cli = manager.cli()?;
    let identity = manager.identity();
    let local_args = args_with_auth(
        manager,
        identity.as_ref(),
        "status",
        &[
            "--xml",
            "--verbose",
            "--depth",
            "infinity",
            "--ignore-externals",
        ],
        &info.root,
    );
    let local = run(
        &cli,
        &local_args,
        identity.as_ref().and_then(|i| i.password.as_deref()),
        cancel,
    )?;
    let (mut entries, _) = parse_status(&local)?;
    // These folders hold local application state even in older working copies
    // that have no svn:ignore property for them.
    entries.retain(|entry| !private_unversioned_entry(Path::new(&info.root), entry));
    let property_args = args_with_auth(
        manager,
        identity.as_ref(),
        "propget",
        &["svn:needs-lock", "--xml", "--depth", "infinity"],
        &info.root,
    );
    let property_xml = run(
        &cli,
        &property_args,
        identity.as_ref().and_then(|i| i.password.as_deref()),
        cancel,
    )?;
    let needs_lock = parse_needs_lock(&property_xml)?;
    for entry in &mut entries {
        entry.needs_lock = needs_lock
            .iter()
            .any(|path| same_svn_local_path(path, &entry.path));
    }
    let server_args = args_with_auth(
        manager,
        identity.as_ref(),
        "status",
        &[
            "--xml",
            "--verbose",
            "-u",
            "--depth",
            "infinity",
            "--ignore-externals",
        ],
        &info.root,
    );
    let (server_revision, server_error) = match run(
        &cli,
        &server_args,
        identity.as_ref().and_then(|i| i.password.as_deref()),
        cancel,
    ) {
        Ok(xml) => {
            let (mut remote_entries, revision) = parse_status(&xml)?;
            remote_entries.retain(|entry| {
                !private_unversioned_entry(Path::new(&info.root), entry)
                    || entry.remote.is_some()
                    || entry.remote_properties.is_some()
            });
            merge_remote_entries(&mut entries, remote_entries);
            (revision, None)
        }
        Err(error) if error == "svn_cancelled" => return Err(error),
        Err(error) => (None, Some(error)),
    };
    let recovery = recovery_state(&entries);
    let update_block = update_block(&entries, server_error.as_deref(), recovery);
    Ok(Status {
        info,
        entries,
        server_revision,
        server_error,
        recovery: recovery.map(str::to_owned),
        update_block: update_block.map(str::to_owned),
    })
}

fn merge_remote_entries(entries: &mut Vec<StatusEntry>, remote_entries: Vec<StatusEntry>) {
    for mut entry in remote_entries {
        if let Some(existing) = entries.iter_mut().find(|e| e.path == entry.path) {
            existing.remote = entry.remote;
            existing.remote_properties = entry.remote_properties;
            if entry.lock_owner.is_some() {
                existing.lock_owner = entry.lock_owner;
            }
        } else {
            // The server can report a path that does not exist in this
            // working copy as wc-status="none", repos-status="added".
            // Only that observed server-only row is safe to exclude from
            // local dirt. A filesystem occupant is an obstacle.
            if entry.local == "none" && entry.remote.as_deref() == Some("added") {
                let target = Path::new(&entry.path);
                let missing = target.is_absolute()
                    && matches!(
                        fs::symlink_metadata(target),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    );
                if missing {
                    entry.remote_only = true;
                } else {
                    entry.local = "obstructed".into();
                }
            }
            entries.push(entry);
        }
    }
}

fn recovery_state(entries: &[StatusEntry]) -> Option<&'static str> {
    if entries.iter().any(|entry| entry.working_copy_locked) {
        Some("cleanupRequired")
    } else if entries.iter().any(|entry| entry.local == "incomplete") {
        Some("resumeRequired")
    } else {
        None
    }
}

fn entry_dirty(entry: &StatusEntry) -> bool {
    if entry.remote_only && entry.local == "none" && entry.remote.as_deref() == Some("added") {
        return false;
    }
    if entry
        .properties
        .as_deref()
        .is_some_and(|value| value != "none" && value != "normal")
    {
        return true;
    }
    if entry.local == "incomplete" && !entry.working_copy_locked {
        return false;
    }
    !matches!(entry.local.as_str(), "normal" | "ignored")
}

fn update_block(
    entries: &[StatusEntry],
    server_error: Option<&str>,
    recovery: Option<&str>,
) -> Option<&'static str> {
    if recovery == Some("cleanupRequired") {
        return Some("svn_working_copy_cleanup_required");
    }
    if entries.iter().any(|entry| {
        matches!(entry.local.as_str(), "conflicted" | "obstructed")
            || entry.properties.as_deref() == Some("conflicted")
    }) {
        return Some("svn_conflicted_working_copy");
    }
    if entries.iter().any(entry_dirty) {
        return Some("svn_dirty_working_copy");
    }
    if server_error.is_some() {
        return Some("svn_server_unconfirmed");
    }
    if recovery == Some("resumeRequired")
        && !entries.iter().any(|entry| {
            [entry.remote.as_deref(), entry.remote_properties.as_deref()]
                .into_iter()
                .flatten()
                .any(|value| value != "none" && value != "normal")
        })
    {
        return Some("svn_working_copy_incomplete");
    }
    None
}

#[tauri::command]
pub(crate) async fn svn_status<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    request: String,
    path: String,
) -> Result<Status, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    let correlation = request.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Status, String> {
        let (_guard, cancel) = manager.begin(&request)?;
        // A working copy can be opened without visiting the connection dialog.
        // Discover installed tools on that first status request as well.
        if manager.cli().is_err() {
            probe(&manager, None, None);
        }
        status(&manager, Path::new(&path), &cancel)
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?;
    let (category, outcome) = match &result {
        Ok(value) if value.server_error.is_some() => ("server_unconfirmed", "unknown"),
        Ok(value) if value.recovery.is_some() => ("working_copy_recovery", "warning"),
        Ok(_) => ("working_copy", "verified"),
        Err(_) => ("status_unavailable", "failed"),
    };
    crate::diagnostic_log::record("svn", "status", category, outcome, Some(correlation), None);
    result
}

#[tauri::command]
pub(crate) async fn svn_gui_update<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
    request: String,
    path: String,
) -> Result<Status, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    let _custody = manager.result_custody()?;
    let observer = manager.clone();
    let operation = request.clone();
    let target = path.clone();
    let app = app.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (_guard, cancel) = manager.begin(&request)?;
        let path = PathBuf::from(path);
        let current = path.canonicalize().map_err(|_| "svn_project_missing")?;
        app.svn_update_admission(&current)?;
        SvnLockService::new(manager.clone(), current.clone())
            .policy_admit_inner(&manager, &cancel)?;
        gui_update_closed(&manager, &current, &cancel)
    })
    .await
    .unwrap_or_else(|_| Err("svn_task_failed".to_owned()));
    observer.preserve_uncertain(
        &operation,
        &target,
        "gui_update",
        None,
        result.as_ref().err().map(String::as_str),
    );
    result
}

#[tauri::command]
pub(crate) async fn svn_gui_cleanup<R: Runtime>(
    webview: Webview<R>,
    manager: State<'_, Manager>,
    app: State<'_, Arc<crate::state::AppState>>,
    request: String,
    path: String,
) -> Result<Status, String> {
    main_only(&webview)?;
    let manager = manager.inner().clone();
    let _custody = manager.result_custody()?;
    let observer = manager.clone();
    let operation = request.clone();
    let target = path.clone();
    let app = app.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (_guard, cancel) = manager.begin(&request)?;
        let current = PathBuf::from(path)
            .canonicalize()
            .map_err(|_| "svn_project_missing")?;
        app.svn_update_admission(&current)?;
        gui_cleanup_closed(&manager, &current, &cancel)
    })
    .await
    .unwrap_or_else(|_| Err("svn_task_failed".to_owned()));
    observer.preserve_uncertain(
        &operation,
        &target,
        "gui_cleanup",
        None,
        result.as_ref().err().map(String::as_str),
    );
    result
}

fn preflight_update(
    manager: &Manager,
    current: &Path,
    cancel: &AtomicBool,
) -> Result<Status, String> {
    let before = status(manager, current, cancel)?;
    if let Some(reason) = before.update_block.as_ref() {
        return Err(reason.clone());
    }
    Ok(before)
}

fn gui_config(manager: &Manager, url: &str, require_login: bool) -> Result<PathBuf, String> {
    let parsed = Url::parse(url).map_err(|_| "svn_invalid_url")?;
    let config = if parsed.scheme() == "https" {
        match manager.identity() {
            Some(identity) if identity.origin == origin(&parsed) => identity.config,
            Some(_) if require_login => return Err("svn_other_server".into()),
            None if require_login => return Err("svn_login_required".into()),
            _ => manager.inner.config_root.join("gui-local"),
        }
    } else {
        manager.inner.config_root.join("gui-local")
    };
    fs::create_dir_all(&config).map_err(|_| "svn_config_failed")?;
    Ok(config)
}

fn gui_update_closed(
    manager: &Manager,
    current: &Path,
    cancel: &AtomicBool,
) -> Result<Status, String> {
    let before = preflight_update(manager, current, cancel)?;
    let config = gui_config(manager, &before.info.url, true)?;
    let mut command = Command::new(manager.gui()?);
    command.args([
        "/command:update".to_owned(),
        format!("/path:{}", current.display()),
        format!("/configdir:{}", config.display()),
        "/ignoreexternals".to_owned(),
        "/closeonend:0".to_owned(),
    ]);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "svn_gui_launch_failed")?;
    // TortoiseProc owns its progress and cancel UI. Keep this request (and
    // logout/app-exit exclusion) until that GUI process closes. Its exit code
    // does not establish whether the working copy is current or unchanged.
    child.wait().map_err(|_| "svn_update_applied_unverified")?;
    status(manager, current, cancel).map_err(|_| "svn_update_applied_unverified".into())
}

fn gui_cleanup_closed(
    manager: &Manager,
    current: &Path,
    cancel: &AtomicBool,
) -> Result<Status, String> {
    let before = status(manager, current, cancel)?;
    if before.recovery.as_deref() != Some("cleanupRequired") {
        return Err("svn_cleanup_not_required".into());
    }
    let target = Path::new(&before.info.wc_root);
    if !current.starts_with(target) {
        return Err("svn_not_working_copy".into());
    }
    // Cleanup repairs local WC bookkeeping and remains available after logout.
    let config = gui_config(manager, &before.info.url, false)?;
    let mut command = Command::new(manager.gui()?);
    // Keep the Cleanup options dialog visible. The user chooses whether to
    // execute cleanup; no revert, file deletion, or forced lock break is sent.
    command.args([
        "/command:cleanup".to_owned(),
        format!("/path:{}", target.display()),
        format!("/configdir:{}", config.display()),
        "/cleanup".to_owned(),
    ]);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "svn_gui_launch_failed")?;
    child.wait().map_err(|_| "svn_cleanup_unverified")?;
    // The dialog can be cancelled. Only the next observed status can clear
    // the recovery action or enable a continuation update.
    status(manager, current, cancel).map_err(|_| "svn_cleanup_unverified".into())
}

#[cfg(test)]
fn update_closed(manager: &Manager, current: &Path, cancel: &AtomicBool) -> Result<Status, String> {
    let before = preflight_update(manager, current, cancel)?;
    let cli = manager.cli()?;
    let identity = manager.identity();
    let args = args_with_auth(
        manager,
        identity.as_ref(),
        "update",
        &["--accept", "postpone", "--ignore-externals"],
        &before.info.root,
    );
    run_command(
        &cli,
        &args,
        identity.as_ref().and_then(|i| i.password.as_deref()),
        cancel,
    )
    .map_err(|_| "svn_update_may_be_partial")?;
    let after =
        status(manager, current, cancel).map_err(|_| "svn_update_applied_unverified".to_owned())?;
    if after.server_error.is_some() || after.entries.iter().any(entry_dirty) {
        return Err("svn_update_applied_unverified".into());
    }
    Ok(after)
}

#[tauri::command]
pub(crate) async fn svn_local_document_status<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    document: String,
) -> Result<StatusEntry, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    tauri::async_runtime::spawn_blocking(move || provider.local_document_status(&document))
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_document_lock_owner<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    document: String,
) -> Result<DocumentLockInfo, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    tauri::async_runtime::spawn_blocking(move || provider.document_lock_owner(&document))
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_force_document_lock<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    document: String,
    expected_owner: String,
    expected_observation: String,
    expected_username: String,
    reason: String,
) -> Result<(), String> {
    main_only(&webview)?;
    let mut permit = app
        .inner()
        .begin_force(Path::new(&path), document.clone())?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = permit.provider.force_document_lock(
            &document,
            &expected_owner,
            &expected_observation,
            &expected_username,
            &reason,
        );
        permit.finish(&result);
        result
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_commit_candidates<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
) -> Result<Vec<CommitCandidate>, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    tauri::async_runtime::spawn_blocking(move || provider.candidates())
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_schedule_delete<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    target: String,
) -> Result<(), String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    tauri::async_runtime::spawn_blocking(move || provider.schedule_delete(&target))
        .await
        .map_err(|_| "svn_task_failed".to_owned())?
}

#[tauri::command]
pub(crate) async fn svn_commit<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    manager: State<'_, Manager>,
    path: String,
    request: String,
    paths: Vec<String>,
    message: String,
) -> Result<CommitResult, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    let _custody = manager.result_custody()?;
    let requested_paths = paths.clone();
    crate::diagnostic_log::record(
        "svn",
        "commit",
        "selection",
        "started",
        Some(request.clone()),
        None,
    );
    let correlation = request.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || provider.commit(request, paths, message))
            .await
            .unwrap_or_else(|_| Err("svn_task_failed".to_owned()));
    let (category, outcome) = match &result {
        Ok(value) if !value.verification_unknown.is_empty() => ("post_commit_unknown", "partial"),
        Ok(value) if value.unlock_pending.is_empty() => ("selected_documents", "verified"),
        Ok(_) => ("unlock_pending", "partial"),
        Err(reason) if reason == "svn_commit_applied_unverified" => {
            ("applied_unverified", "unknown")
        }
        Err(reason) if reason == "svn_commit_unverified" || reason == "svn_task_failed" => {
            ("result_unverified", "unknown")
        }
        Err(reason) if reason == "svn_commit_selection_changed" => {
            ("selection_changed", "rejected")
        }
        Err(_) => ("commit_rejected", "failed"),
    };
    if matches!(outcome, "partial" | "unknown") {
        let observed = match &result {
            Ok(value) => serde_json::to_value(value)
                .unwrap_or_else(|_| serde_json::json!({"error":"svn_result_encoding_failed"})),
            Err(reason) => serde_json::json!({"error":reason}),
        };
        let record = crate::updater::handoff::PendingSvn {
            operation_id: correlation.clone(),
            project_fingerprint: format!("{:x}", Sha256::digest(path.as_bytes())),
            requested_paths,
            outcome: outcome.into(),
            observed,
        };
        if manager.inner.pending.preserve(record).is_err() {
            crate::diagnostic_log::record(
                "svn",
                "handoff",
                "preserve_failed",
                "error",
                Some(correlation.clone()),
                None,
            );
        }
    }
    crate::diagnostic_log::record("svn", "commit", category, outcome, Some(correlation), None);
    result
}

#[tauri::command]
pub(crate) async fn svn_commit_recheck<R: Runtime>(
    webview: Webview<R>,
    app: State<'_, Arc<crate::state::AppState>>,
    path: String,
    revision: String,
    paths: Vec<String>,
    deleted: Vec<String>,
) -> Result<CommitResult, String> {
    main_only(&webview)?;
    let provider = app.svn_commit_provider(Path::new(&path))?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        provider.recheck_commit(revision, paths, deleted)
    })
    .await
    .map_err(|_| "svn_task_failed".to_owned())?;
    crate::diagnostic_log::record(
        "svn",
        "commit_recheck",
        "post_commit",
        if result.is_ok() {
            "observed"
        } else {
            "unknown"
        },
        None,
        None,
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m8_result_custody_keeps_seal_blocked_until_durable_delivery() {
        let manager =
            Manager::new(std::env::temp_dir().join(format!("m8-custody-{}", uuid::Uuid::new_v4())));
        let result = manager.result_custody().unwrap();
        assert!(manager.has_active());
        assert!(!manager.seal_for_update());
        assert!(manager.verify_update_handoff().is_err());
        drop(result);
        assert!(manager.seal_for_update());
        assert!(manager.result_custody().is_err());
        assert!(manager.verify_update_handoff().unwrap().is_empty());
    }
    #[test]
    fn m8_unverified_register_checkout_and_update_survive_new_manager() {
        let root = std::env::temp_dir().join(format!("m8-svn-pending-{}", uuid::Uuid::new_v4()));
        let manager = Manager::new(root.clone());
        for (action, error) in [
            ("register", "svn_register_setup_unverified"),
            ("checkout", "svn_checkout_may_be_partial"),
            ("gui_update", "svn_update_applied_unverified"),
            ("gui_cleanup", "svn_task_failed"),
        ] {
            let custody = manager.result_custody().unwrap();
            manager.preserve_uncertain(
                &uuid::Uuid::new_v4().to_string(),
                "owned-root",
                action,
                None,
                Some(error),
            );
            assert!(manager.verify_update_handoff().is_err());
            drop(custody);
        }
        let reopened = Manager::new(root);
        let rows = reopened.verify_update_handoff().unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows
            .iter()
            .all(|row| matches!(row.outcome.as_str(), "unknown" | "partial")));
    }
    use base64::Engine as _;
    use std::{net::TcpListener, path::Path};
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        test::mock_builder,
        webview::InvokeRequest,
    };

    #[cfg(windows)]
    #[test]
    #[ignore = "run explicitly on an owned temporary FSFS fixture with installed TortoiseSVN CLI"]
    fn m76_fix001_confirmed_revision_survives_partial_post_commit_checks() {
        let cli = PathBuf::from(
            std::env::var("M7_SVN_TEST_CLI")
                .unwrap_or_else(|_| r"C:\Program Files\TortoiseSVN\bin\svn.exe".into()),
        );
        let admin = cli.with_file_name("svnadmin.exe");
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("logs")
            .join("M7-6-FIX-001")
            .join("fixtures")
            .join(format!("postcheck-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let base = ordinary_windows_path(&base.canonicalize().unwrap());
        let repo = base.join("repo");
        let wc = base.join("wc");
        let execute = |binary: &Path, args: &[&str]| {
            let output = Command::new(binary).args(args).output().unwrap();
            assert!(
                output.status.success(),
                "fixture svn command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        execute(&admin, &["create", repo.to_str().unwrap()]);
        let url = Url::from_directory_path(&repo).unwrap().to_string();
        execute(&cli, &["checkout", &url, wc.to_str().unwrap()]);
        execute(&cli, &["mkdir", wc.join("templates").to_str().unwrap()]);
        execute(
            &cli,
            &["commit", "-m", "fixture root", wc.to_str().unwrap()],
        );
        let manager = Manager::new(base.join("config"));
        assert!(probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
        let provider = SvnLockService::new(manager, wc.clone());
        let template = |id: uuid::Uuid| {
            serde_json::json!({
                "artifactType":"template", "schemaVersion":1, "templateId":id,
                "revision":1, "name":"Postcheck fixture", "lifecycle":"active",
                "presentation":{}, "fieldOrder":[], "fields":{},
                "createdAtUtc":"2026-09-03T01:02:03.004Z", "updatedAtUtc":"2026-09-03T01:02:03.004Z"
            })
        };
        let old = format!("templates/{}.json", uuid::Uuid::new_v4());
        fs::write(
            wc.join(&old),
            serde_json::to_vec(&template(
                uuid::Uuid::parse_str(
                    old.trim_start_matches("templates/")
                        .trim_end_matches(".json"),
                )
                .unwrap(),
            ))
            .unwrap(),
        )
        .unwrap();
        provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![old.clone()],
                "fixture old".into(),
            )
            .unwrap();
        let mut permissions = fs::metadata(wc.join(&old)).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(wc.join(&old), permissions).unwrap();
        fs::remove_file(wc.join(&old)).unwrap();
        provider.schedule_delete(&old).unwrap();
        let paths: Vec<String> = (0..3)
            .map(|_| {
                let id = uuid::Uuid::new_v4();
                let path = format!("templates/{id}.json");
                fs::write(wc.join(&path), serde_json::to_vec(&template(id)).unwrap()).unwrap();
                path
            })
            .collect();
        let mut selected = vec![old.clone()];
        selected.extend(paths.clone());
        let remote_unknown = paths[0].clone();
        let local_unknown = paths[1].clone();
        *provider.post_commit_probe.lock().unwrap() = Some(Arc::new(move |path, check| {
            if (check == "delete")
                || (path == remote_unknown && check == "remoteLock")
                || (path == local_unknown && check == "localInfo")
            {
                Err("owned_postcheck_fault".into())
            } else {
                Ok(())
            }
        }));
        let committed = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                selected.clone(),
                "postcheck fault".into(),
            )
            .unwrap();
        assert_eq!(committed.paths, selected);
        assert_eq!(committed.deleted, vec![old.clone()]);
        assert_eq!(committed.verification_unknown.len(), 3);
        assert!(committed
            .verification_unknown
            .iter()
            .any(|item| item.path == old && item.check == "delete"));
        assert!(committed
            .verification_unknown
            .iter()
            .any(|item| item.path == paths[0] && item.check == "remoteLock"));
        assert!(committed
            .verification_unknown
            .iter()
            .any(|item| item.path == paths[1] && item.check == "localInfo"));
        let revision = execute(&cli, &["info", "--show-item", "revision", &url])
            .trim()
            .to_owned();
        assert_eq!(committed.revision, revision);
        *provider.post_commit_probe.lock().unwrap() = None;
        let rechecked = provider
            .recheck_commit(
                committed.revision.clone(),
                committed.paths.clone(),
                committed.deleted.clone(),
            )
            .unwrap();
        assert_eq!(rechecked.revision, revision);
        assert!(rechecked.verification_unknown.is_empty());
        assert_eq!(rechecked.deleted_confirmed, vec![old]);
        assert_eq!(
            execute(&cli, &["info", "--show-item", "revision", &url]).trim(),
            revision
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "run explicitly with installed TortoiseSVN CLI and owned temporary FSFS"]
    fn m76_force_lock_rechecks_owner_account_and_previous_token() {
        let cli = PathBuf::from(
            std::env::var("M7_SVN_TEST_CLI")
                .unwrap_or_else(|_| r"C:\Program Files\TortoiseSVN\bin\svn.exe".into()),
        );
        let admin = cli.with_file_name("svnadmin.exe");
        let base =
            std::env::temp_dir().join(format!("worldbuild-m76-force-{}", uuid::Uuid::new_v4()));
        let repo = base.join("repo");
        let a = base.join("a");
        let b = base.join("b");
        fs::create_dir_all(&base).unwrap();
        let execute = |binary: &Path, args: &[&str]| {
            let result = Command::new(binary).args(args).output().unwrap();
            assert!(
                result.status.success(),
                "SVN fixture command {:?} {:?} failed: {}",
                binary,
                args,
                String::from_utf8_lossy(&result.stderr)
            );
        };
        execute(&admin, &["create", repo.to_str().unwrap()]);
        let url = Url::from_directory_path(&repo).unwrap().to_string();
        execute(&cli, &["checkout", &url, a.to_str().unwrap()]);
        let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let relative = format!("documents/{document}.json");
        let file_a = a.join(&relative);
        fs::create_dir_all(file_a.parent().unwrap()).unwrap();
        fs::write(&file_a, br#"{"name":"owned lock fixture"}"#).unwrap();
        execute(&cli, &["add", a.join("documents").to_str().unwrap()]);
        execute(
            &cli,
            &["propset", "svn:needs-lock", "yes", file_a.to_str().unwrap()],
        );
        execute(
            &cli,
            &["commit", "-m", "owned fixture", a.to_str().unwrap()],
        );
        execute(&cli, &["checkout", &url, b.to_str().unwrap()]);

        let first_manager = Manager::new(base.join("config-a"));
        let second_manager = Manager::new(base.join("config-b"));
        assert!(
            probe(
                &first_manager,
                Some(cli.to_string_lossy().into_owned()),
                None
            )
            .installed
        );
        assert!(
            probe(
                &second_manager,
                Some(cli.to_string_lossy().into_owned()),
                None
            )
            .installed
        );
        let first = SvnLockService::new(first_manager, a.clone());
        let second = SvnLockService::new(second_manager.clone(), b.clone());
        let target = ProjectRelativePath::parse(&relative).unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut previous = first
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .unwrap();
        let original_a = fs::read(&file_a).unwrap();
        let unavailable = base.join("repo-unavailable");
        fs::rename(&repo, &unavailable).unwrap();
        assert_eq!(
            first.validate(previous.as_mut()).unwrap_err().category,
            LockErrorCategory::LockStateUnknown
        );
        assert_eq!(
            first.release(previous.as_mut()).unwrap_err().category,
            LockErrorCategory::LockReleaseFailed
        );
        assert_eq!(first.held_targets(), vec![target.clone()]);
        assert_eq!(fs::read(&file_a).unwrap(), original_a);
        fs::rename(&unavailable, &repo).unwrap();
        first.validate(previous.as_mut()).unwrap();
        let observed = second.document_lock_owner(document).unwrap();
        let owner = observed.owner.unwrap();
        let observation = observed.observation.unwrap();
        let svn_metadata = b.join(".svn");
        let unavailable_metadata = b.join(".svn-unavailable");
        fs::rename(&svn_metadata, &unavailable_metadata).unwrap();
        assert!(second.document_lock_owner(document).is_err());
        fs::rename(&unavailable_metadata, &svn_metadata).unwrap();
        let config = base.join("config-b").join("owner-b");
        fs::create_dir_all(&config).unwrap();
        *second_manager.inner.identity.lock().unwrap() = Some(Identity {
            origin: "file://".into(),
            url,
            username: "owner-b".into(),
            config,
            password: None,
            remember: false,
        });
        assert_eq!(
            second.force_document_lock(
                document,
                &owner,
                &observation,
                "wrong-account",
                "owned test"
            ),
            Err("svn_force_context_changed".into())
        );
        assert_eq!(
            second.force_document_lock(
                document,
                "stale-owner",
                &observation,
                "owner-b",
                "owned test"
            ),
            Err("svn_force_context_changed".into())
        );
        assert_eq!(
            second.force_document_lock(document, &owner, &"0".repeat(64), "owner-b", "owned test"),
            Err("svn_force_context_changed".into())
        );
        assert_eq!(
            second.force_document_lock(document, &owner, &observation, "owner-b", " "),
            Err("svn_force_invalid_request".into())
        );
        let original = fs::read(b.join(&relative)).unwrap();
        let file_b = b.join(&relative);
        let mut permissions = fs::metadata(&file_b).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&file_b, permissions).unwrap();
        fs::write(&file_b, b"external uncommitted change").unwrap();
        assert_eq!(
            second.force_document_lock(document, &owner, &observation, "owner-b", "owned test"),
            Err("svn_status_unsafe".into())
        );
        assert_eq!(fs::read(&file_b).unwrap(), b"external uncommitted change");
        fs::write(&file_b, &original).unwrap();
        execute(
            &cli,
            &[
                "propset",
                "worldbuild:test",
                "changed",
                file_b.to_str().unwrap(),
            ],
        );
        assert_eq!(
            second.force_document_lock(document, &owner, &observation, "owner-b", "owned test"),
            Err("svn_status_unsafe".into())
        );
        execute(
            &cli,
            &["propdel", "worldbuild:test", file_b.to_str().unwrap()],
        );
        second
            .force_document_lock(
                document,
                &owner,
                &observation,
                "owner-b",
                "owned test handoff",
            )
            .unwrap();
        assert_eq!(fs::read(b.join(&relative)).unwrap(), original);
        assert_eq!(
            first.validate(previous.as_mut()).unwrap_err().category,
            LockErrorCategory::LockLost
        );
        let successor = second.document_lock_owner(document).unwrap();
        first.release(previous.as_mut()).unwrap();
        assert!(first.held_targets().is_empty());
        assert_eq!(previous.state(), HeldLockState::Released);
        let after_release = second.document_lock_owner(document).unwrap();
        assert_eq!(after_release.owner, successor.owner);
        assert_eq!(after_release.observation, successor.observation);
        assert_eq!(fs::read(&file_a).unwrap(), original_a);
        let mut acquired = second
            .acquire(
                LockAcquireRequest::new(
                    &"b".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .unwrap();
        second.validate(acquired.as_mut()).unwrap();
        drop(acquired);
        drop(previous);
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "run explicitly with an owned HTTPS WC and the current app SVN test identity"]
    fn m76_owned_https_force_uses_current_app_identity() {
        let config = PathBuf::from(std::env::var("M7_D_TEST_APP_CONFIG").unwrap());
        let wc = PathBuf::from(std::env::var("M7_D_TEST_WC").unwrap());
        let document = std::env::var("M7_D_TEST_DOCUMENT").unwrap();
        let manager = Manager::new(config);
        let username = manager.identity().unwrap().username;
        let provider = SvnLockService::new(manager, wc);
        let observed = provider.document_lock_owner(&document).unwrap();
        let result = provider.force_document_lock(
            &document,
            observed.owner.as_deref().unwrap(),
            observed.observation.as_deref().unwrap(),
            &username,
            "M7 D owned HTTPS native confirmation",
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn m745_fix003_active_svn_completion_wakes_waiting_exit_once() {
        use std::sync::atomic::AtomicUsize;
        let manager = Manager::new(std::env::temp_dir().join("m745-fix003-idle-wake"));
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&wakes);
        manager.set_idle_wake(Arc::new(move || {
            observed.fetch_add(1, Ordering::AcqRel);
        }));
        let (guard, _) = manager
            .begin("a5390d2a-4b53-4b17-9c4c-34b0b75e4010")
            .unwrap();
        assert!(manager.has_active());
        assert_eq!(wakes.load(Ordering::Acquire), 0);
        drop(guard);
        assert!(!manager.has_active());
        assert_eq!(wakes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn registration_resume_requires_the_same_attempt() {
        let original = RegistrationAttempt {
            root: PathBuf::from(r"C:\owned\copy"),
            url: "https://example.test/svn/copy".into(),
            fingerprint: "fingerprint".into(),
            username: "owner".into(),
            setup: RegistrationSetup::Created {
                revision: "7".into(),
            },
        };
        assert!(original.matches(&RegistrationAttempt {
            root: original.root.clone(),
            url: original.url.clone(),
            fingerprint: original.fingerprint.clone(),
            username: original.username.clone(),
            setup: original.setup.clone(),
        }));
        for changed in [
            RegistrationAttempt {
                root: PathBuf::from(r"C:\owned\other"),
                ..clone_attempt(&original)
            },
            RegistrationAttempt {
                url: "https://example.test/svn/other".into(),
                ..clone_attempt(&original)
            },
            RegistrationAttempt {
                fingerprint: "changed".into(),
                ..clone_attempt(&original)
            },
            RegistrationAttempt {
                username: "other".into(),
                ..clone_attempt(&original)
            },
        ] {
            assert!(!original.matches(&changed));
        }
    }

    #[test]
    #[ignore = "run with owned M7_FIX2_REPO_URL and M7_SVN_TEST_CLI"]
    fn owned_registration_rejects_prior_empty_child_and_resumes_only_created_child() {
        let base = std::env::var("M7_FIX2_REPO_URL").expect("owned FSFS URL");
        let cli = PathBuf::from(std::env::var("M7_SVN_TEST_CLI").expect("installed CLI"));
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-fix002-register-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
        let identity = Identity {
            origin: "file://".into(),
            url: base.clone(),
            username: "audit-user".into(),
            config: manager.inner.config_root.join("owned-test"),
            password: None,
            remember: false,
        };
        fs::create_dir_all(&identity.config).unwrap();
        let cancel = AtomicBool::new(false);
        let make_attempt = |url: &Url| RegistrationAttempt {
            root: PathBuf::from(r"C:\owned\fix002-copy"),
            url: url.to_string(),
            fingerprint: "owned-fingerprint".into(),
            username: identity.username.clone(),
            setup: RegistrationSetup::Pending,
        };

        let prior = Url::parse(&format!("{base}/prior-empty")).unwrap();
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &prior,
                make_attempt(&prior),
                false,
                &cancel,
            ),
            Err("svn_register_path_exists".into())
        );
        assert!(manager.inner.registration_attempt.lock().unwrap().is_none());
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &prior,
                make_attempt(&prior),
                true,
                &cancel,
            ),
            Err("svn_register_resume_denied".into())
        );

        let fresh = Url::parse(&format!("{base}/fix002-{}", uuid::Uuid::new_v4())).unwrap();
        let created = registration_setup(
            &manager,
            &cli,
            &identity,
            &fresh,
            make_attempt(&fresh),
            false,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &fresh,
                make_attempt(&fresh),
                true,
                &cancel,
            ),
            Ok(created.clone())
        );
        let nested = format!("{}/changed", fresh.as_str().trim_end_matches('/'));
        let args = args_with_auth(
            &manager,
            Some(&identity),
            "mkdir",
            &["-m", "Change the owned child"],
            &nested,
        );
        run(&cli, &args, None, &cancel).unwrap();
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &fresh,
                make_attempt(&fresh),
                true,
                &cancel,
            ),
            Err("svn_register_resume_denied".into())
        );
        manager
            .inner
            .registration_attempt
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .setup = RegistrationSetup::Unknown;
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &fresh,
                make_attempt(&fresh),
                true,
                &cancel,
            ),
            Err("svn_register_resume_denied".into())
        );

        let unicode = Url::parse(&format!(
            "{base}/%ED%95%9C%EA%B8%80-{}",
            uuid::Uuid::new_v4()
        ))
        .unwrap();
        let unicode_revision = registration_setup(
            &manager,
            &cli,
            &identity,
            &unicode,
            make_attempt(&unicode),
            false,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            registration_setup(
                &manager,
                &cli,
                &identity,
                &unicode,
                make_attempt(&unicode),
                true,
                &cancel,
            ),
            Ok(unicode_revision)
        );
    }

    #[test]
    #[ignore = "run with owned M7_FIX2_REPO_URL, M7_FIX2_REGISTER_ROOT and M7_SVN_TEST_CLI"]
    fn owned_registration_resumes_confirmed_child_and_finishes_empty_project() {
        let base = std::env::var("M7_FIX2_REPO_URL").expect("owned FSFS URL");
        let root = PathBuf::from(std::env::var("M7_FIX2_REGISTER_ROOT").expect("owned source"));
        let cli = PathBuf::from(std::env::var("M7_SVN_TEST_CLI").expect("installed CLI"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("workspace")).unwrap();
        let private = root.join("workspace/private.json");
        fs::write(&private, b"personal-only").unwrap();
        let inventory = crate::svn_shared::inventory(&root).unwrap();
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.excluded_files, 1);
        let fingerprint = inventory.fingerprint.clone();
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-fix002-register-complete-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli.to_string_lossy().into_owned()), None).installed);
        let identity = Identity {
            origin: "file://".into(),
            url: base.clone(),
            username: "audit-user".into(),
            config: manager.inner.config_root.join("owned-test"),
            password: None,
            remember: false,
        };
        fs::create_dir_all(&identity.config).unwrap();
        let url = Url::parse(&format!(
            "{base}/register-complete-{}",
            uuid::Uuid::new_v4()
        ))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let attempt = RegistrationAttempt {
            root: root.clone(),
            url: url.to_string(),
            fingerprint: fingerprint.clone(),
            username: identity.username.clone(),
            setup: RegistrationSetup::Pending,
        };
        let created = registration_setup(
            &manager,
            &cli,
            &identity,
            &url,
            attempt.clone(),
            false,
            &cancel,
        )
        .unwrap();
        // Simulate a definite setup followed by a checkout failure before WC creation.
        let resumed =
            registration_setup(&manager, &cli, &identity, &url, attempt, true, &cancel).unwrap();
        assert_eq!(created, resumed);
        let result = registration_finish(
            &manager,
            &cli,
            identity,
            url.clone(),
            root.clone(),
            inventory,
            fingerprint,
            resumed,
            "FIX002 owned empty registration".into(),
            &cancel,
        )
        .unwrap();
        assert_eq!(result.setup_revision, created);
        assert_eq!(result.files, 0);
        assert_eq!(fs::read(private).unwrap(), b"personal-only");
        assert_eq!(
            crate::svn_guard::working_copy_root(&root)
                .as_deref()
                .map(ordinary_windows_path),
            Some(root.clone())
        );
        assert!(manager.inner.registration_attempt.lock().unwrap().is_none());
        for directory in ["assets", "documents", "templates", "workspace"] {
            assert!(root.join(directory).is_dir());
        }
    }

    #[test]
    #[ignore = "run once with owned M7_FIX_MANY_WC and M7_SVN_TEST_CLI"]
    fn owned_native_commit_over_64_exact_targets_preserves_unselected() {
        let root = PathBuf::from(std::env::var("M7_FIX_MANY_WC").expect("owned fixture"));
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI");
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-fix001-many-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let candidates = provider.candidates().expect("native candidate path");
        let mut paths: Vec<_> = candidates
            .iter()
            .filter(|row| row.eligible && row.path.starts_with("templates/"))
            .map(|row| row.path.clone())
            .collect();
        paths.sort();
        assert_eq!(paths.len(), 71);
        let unselected = paths.pop().unwrap();
        let outcome = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                paths.clone(),
                "FIX001 owned native 70 target check".into(),
            )
            .expect("exact native commit");
        assert_eq!(outcome.paths.len(), 70);
        assert_eq!(outcome.paths, paths);
        assert!(!outcome.paths.contains(&unselected));
        let remaining = provider.candidates().expect("post-commit candidates");
        assert!(remaining.iter().any(|row| row.path == unselected));
        assert!(remaining.iter().all(|row| !paths.contains(&row.path)));
        assert!(root.join(unselected).exists());
    }

    fn clone_attempt(value: &RegistrationAttempt) -> RegistrationAttempt {
        RegistrationAttempt {
            root: value.root.clone(),
            url: value.url.clone(),
            fingerprint: value.fingerprint.clone(),
            username: value.username.clone(),
            setup: value.setup.clone(),
        }
    }
    #[cfg(windows)]
    #[test]
    fn registration_uses_an_ordinary_windows_cli_path() {
        assert_eq!(
            ordinary_windows_path(Path::new(r"\\?\C:\owned\new-project")),
            PathBuf::from(r"C:\owned\new-project")
        );
        assert_eq!(
            ordinary_windows_path(Path::new(r"\\?\UNC\server\share\new-project")),
            PathBuf::from(r"\\server\share\new-project")
        );
    }
    #[test]
    fn local_trash_is_not_a_shared_change() {
        let root = Path::new("C:/project");
        let mut entry = StatusEntry {
            path: "C:/project/assets/.trash".into(),
            local: "unversioned".into(),
            properties: None,
            remote: None,
            remote_properties: None,
            lock_owner: None,
            wc_locked: false,
            working_copy_locked: false,
            needs_lock: false,
            remote_only: false,
        };
        assert!(private_unversioned_entry(root, &entry));
        assert!(private_unversioned_entry(
            Path::new(r"\\?\C:\project"),
            &entry
        ));
        entry.path = "C:/project/.worldbuild".into();
        assert!(private_unversioned_entry(root, &entry));
        entry.path = "C:/project/assets/other".into();
        assert!(!private_unversioned_entry(root, &entry));
        entry.path = "C:/project/assets/.trash".into();
        entry.local = "added".into();
        assert!(!private_unversioned_entry(root, &entry));
    }
    #[test]
    fn validation_scope_is_per_worker_operation_and_restores_after_nesting() {
        assert_eq!(SVN_JOB_SCOPE.get(), 0);
        let first = with_validation_scope(|| {
            let first = SVN_JOB_SCOPE.get();
            assert_ne!(first, 0);
            with_validation_scope(|| assert_ne!(SVN_JOB_SCOPE.get(), first));
            assert_eq!(SVN_JOB_SCOPE.get(), first);
            first
        });
        assert_eq!(SVN_JOB_SCOPE.get(), 0);
        with_validation_scope(|| assert_ne!(SVN_JOB_SCOPE.get(), first));
    }
    #[test]
    fn commit_dependency_and_local_path_checks_are_exact() {
        let payload = serde_json::json!({
            "fieldValues": {"x": {"kind": "image", "value": ["aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"]}},
            "group": [{"kind": "file", "value": ["bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"]}]
        });
        let mut found = Vec::new();
        collect_asset_ids(&payload, &mut found);
        assert_eq!(found.len(), 2);
        assert!(same_svn_local_path(
            "C:\\project\\templates\\x.json",
            "C:/project/templates/x.json"
        ));
        assert!(same_svn_local_path(
            "C:\\project\\templates\\x.json",
            "\\\\?\\C:\\project\\templates\\x.json"
        ));
        assert!(!same_svn_local_path(
            "C:\\project\\templates\\x.json",
            "C:/project/.worldbuild/x.json"
        ));
        let rows = vec![StatusEntry {
            path: "C:\\project\\templates\\x.json".into(),
            local: "normal".into(),
            properties: None,
            remote: None,
            remote_properties: None,
            lock_owner: None,
            wc_locked: false,
            working_copy_locked: false,
            needs_lock: false,
            remote_only: false,
        }];
        assert!(svn_dependency_tracked(
            Path::new("C:/project/templates/x.json"),
            &rows
        ));
        let mut added = rows.clone();
        added[0].local = "added".into();
        assert!(!svn_dependency_tracked(
            Path::new("C:/project/templates/x.json"),
            &added
        ));
    }
    #[test]
    #[ignore = "run with owned M7_C_NEW_WC and M7_SVN_TEST_CLI"]
    fn owned_new_canonical_path_has_exact_absence_authority() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_NEW_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-c-new-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let target =
            ProjectRelativePath::parse(&format!("documents/{}.json", uuid::Uuid::new_v4()))
                .unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut held = provider
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .unwrap();
        provider.validate(held.as_mut()).unwrap();
        let path = root.join(target.as_str());
        fs::write(&path, b"{}").unwrap();
        provider.validate(held.as_mut()).unwrap();
        provider.release(held.as_mut()).unwrap();
        assert!(provider.held_targets().is_empty());
        let session = LockSessionId::generate().unwrap();
        let second =
            provider.acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap());
        assert!(
            second.is_err(),
            "an existing unversioned file is not a fresh path"
        );
        fs::remove_file(path).unwrap();
    }
    #[test]
    #[ignore = "run with owned M7_C_NEW_WC and M7_SVN_TEST_CLI"]
    fn owned_new_template_and_asset_commit_exact_paths() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_NEW_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-c-commit-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let id = uuid::Uuid::new_v4();
        let relative = format!("templates/{id}.json");
        let template = serde_json::json!({
            "artifactType":"template", "schemaVersion":1, "templateId":id,
            "revision":1, "name":"C owned template", "lifecycle":"active",
            "presentation":{}, "fieldOrder":[], "fields":{},
            "createdAtUtc":"2026-09-03T01:02:03.004Z",
            "updatedAtUtc":"2026-09-03T01:02:03.004Z"
        });
        fs::write(root.join(&relative), serde_json::to_vec(&template).unwrap()).unwrap();
        crate::data::artifact::decode_template(&fs::read(root.join(&relative)).unwrap()).unwrap();
        crate::svn_shared::inventory(&root).unwrap();
        let target = ProjectRelativePath::parse(&relative).unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut held = provider
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .unwrap();
        provider.validate(held.as_mut()).unwrap();
        provider.release(held.as_mut()).unwrap();
        let rows = provider.candidates().unwrap();
        assert!(rows
            .iter()
            .any(|row| row.path == relative && row.eligible && row.new_path));
        let result = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![relative.clone()],
                "C owned new template".into(),
            )
            .unwrap();
        assert_eq!(result.paths, [relative]);
        let source = root.join(format!("private-{}.txt", uuid::Uuid::new_v4()));
        fs::write(&source, b"owned asset content").unwrap();
        let asset = uuid::Uuid::new_v4().to_string();
        provider.authorize_new_asset(&asset).unwrap();
        let store = crate::data::assets::Store::open(&root, true).unwrap();
        let meta = store.import(&source, false, &asset).unwrap();
        let metadata_path = format!("assets/{asset}/metadata.json");
        let content_path = format!("assets/{asset}/{}", crate::data::assets::filename(&meta));
        let rows = provider.candidates().unwrap();
        assert!(rows
            .iter()
            .any(|row| row.path == metadata_path && row.eligible));
        assert!(rows
            .iter()
            .any(|row| row.path == content_path && row.eligible));
        let result = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![metadata_path.clone(), content_path.clone()],
                "C owned new asset".into(),
            )
            .unwrap();
        assert_eq!(result.paths, [metadata_path, content_path]);
        assert!(provider.authorize_new_asset(&asset).is_err());
        let status = provider.candidates().unwrap();
        assert!(!status.iter().any(|row| row.path.starts_with("private-")));
    }
    #[test]
    #[ignore = "run with owned M7_C_NEW_WC and M7_SVN_TEST_CLI"]
    fn owned_missing_template_requires_explicit_delete_then_exact_commit() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_NEW_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-c-delete-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let id = uuid::Uuid::new_v4();
        let relative = format!("templates/{id}.json");
        let payload = serde_json::json!({
            "artifactType":"template", "schemaVersion":1, "templateId":id,
            "revision":1, "name":"C delete", "lifecycle":"active",
            "presentation":{}, "fieldOrder":[], "fields":{},
            "createdAtUtc":"2026-09-03T01:02:03.004Z",
            "updatedAtUtc":"2026-09-03T01:02:03.004Z"
        });
        let file = root.join(&relative);
        fs::write(&file, serde_json::to_vec(&payload).unwrap()).unwrap();
        provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![relative.clone()],
                "C deletion setup".into(),
            )
            .unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&file, permissions).unwrap();
        fs::remove_file(&file).unwrap();
        let rows = provider.candidates().unwrap();
        assert!(rows.iter().any(|row| {
            row.path == relative
                && row.local == "missing"
                && !row.eligible
                && row.can_schedule_delete
        }));
        provider.schedule_delete(&relative).unwrap();
        let rows = provider.candidates().unwrap();
        assert!(rows
            .iter()
            .any(|row| row.path == relative && row.local == "deleted" && row.eligible));
        provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![relative.clone()],
                "C owned delete".into(),
            )
            .unwrap();
        assert!(!file.exists());
        assert!(!provider
            .candidates()
            .unwrap()
            .iter()
            .any(|row| row.path == relative));
    }
    #[test]
    #[ignore = "run with owned M7_C_NEW_WC and M7_SVN_TEST_CLI"]
    fn owned_unicode_existing_template_lock_and_selected_commit() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_NEW_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-fix002-lock-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let relative = fs::read_dir(root.join("templates"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".json") || !entry.metadata().ok()?.permissions().readonly() {
                    return None;
                }
                let relative = format!("templates/{name}");
                let source = ProjectRelativePath::parse(&relative).ok()?;
                (ArtifactSourceId::from_target(&source).is_some()).then_some(relative)
            })
            .next()
            .unwrap();
        let target = ProjectRelativePath::parse(&relative).unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut held = provider
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .unwrap();
        let file = root.join(&relative);
        let mut template: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        template["name"] =
            serde_json::Value::String(format!("FIX002 한글 잠금 {}", uuid::Uuid::new_v4()));
        let expected = serde_json::to_vec(&template).unwrap();
        fs::write(&file, &expected).unwrap();
        provider.validate(held.as_mut()).unwrap();
        assert!(provider
            .candidates()
            .unwrap()
            .iter()
            .any(|row| row.path == relative && row.eligible));
        let result = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![relative.clone()],
                "FIX002 Unicode existing template".into(),
            )
            .unwrap();
        assert_eq!(result.paths, [relative]);
        provider.release(held.as_mut()).unwrap();
        assert!(provider.held_targets().is_empty());
        assert_eq!(fs::read(file).unwrap(), expected);
    }
    #[test]
    #[ignore = "run with owned M7_C_ASSET_DELETE_WC and M7_SVN_TEST_CLI"]
    fn owned_missing_asset_package_deletes_only_its_base_paths() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_ASSET_DELETE_WC").unwrap());
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-c-asset-del-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager, root.clone());
        let trash = root.join("assets/.trash");
        let id = fs::read_dir(trash)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        let directory = format!("assets/{id}");
        let rows = provider.candidates().unwrap();
        let observed = rows.iter().find(|row| row.path == directory).unwrap();
        if observed.local == "missing" {
            assert!(observed.can_schedule_delete);
            provider.schedule_delete(&directory).unwrap();
        } else {
            assert_eq!(observed.local, "deleted");
        }
        let rows = provider.candidates().unwrap();
        let package = rows.iter().find(|row| row.path == directory).unwrap();
        assert!(package.eligible);
        assert_eq!(package.required.len(), 2);
        let selected = [vec![directory.clone()], package.required.clone()].concat();
        provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                selected,
                "C owned asset delete".into(),
            )
            .unwrap();
        assert!(!provider
            .candidates()
            .unwrap()
            .iter()
            .any(|row| row.path == directory));
    }
    #[test]
    #[ignore = "run once with owned M7_C_NEW_WC and M7_SVN_TEST_CLI"]
    fn owned_new_document_requires_its_layout_in_one_revision() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let root = PathBuf::from(std::env::var("M7_C_NEW_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-c-layout-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let provider = SvnLockService::new(manager.clone(), root.clone());
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let cancel = AtomicBool::new(false);
        if inspect(&manager, &workspace, &cancel).is_err() {
            let path = workspace.to_string_lossy();
            run(
                &manager.cli().unwrap(),
                &[
                    "add".into(),
                    "--depth".into(),
                    "empty".into(),
                    path.into_owned(),
                ],
                None,
                &cancel,
            )
            .unwrap();
            run(
                &manager.cli().unwrap(),
                &[
                    "commit".into(),
                    "-m".into(),
                    "C layout setup".into(),
                    "--depth".into(),
                    "empty".into(),
                    workspace.to_string_lossy().into_owned(),
                ],
                None,
                &cancel,
            )
            .unwrap();
        }
        let template = fs::read_dir(root.join("templates"))
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".json") || inspect(&manager, &entry.path(), &cancel).is_err() {
                    return None;
                }
                fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| crate::data::artifact::decode_template(&bytes).ok())
                    .map(|template| template.template_id().to_string())
            })
            .unwrap();
        let id = uuid::Uuid::new_v4();
        let document = format!("documents/{id}.json");
        let payload = serde_json::json!({
            "artifactType":"document", "schemaVersion":1,
            "documentId":id, "templateId":template, "templateRevision":1,
            "name":"C linked document", "fieldValues":{},
            "orphanedFieldDefinitions":{},
            "createdAtUtc":"2026-09-03T01:02:03.004Z",
            "updatedAtUtc":"2026-09-03T01:02:03.004Z"
        });
        fs::write(root.join(&document), serde_json::to_vec(&payload).unwrap()).unwrap();
        let layout =
            crate::data::artifact::layout::DocumentLayout::flat([id.to_string().parse().unwrap()]);
        fs::write(
            root.join("workspace/document-layout.json"),
            serde_json::to_vec(&layout).unwrap(),
        )
        .unwrap();
        let rows = provider.candidates().unwrap();
        let candidate = rows.iter().find(|row| row.path == document).unwrap();
        assert!(candidate.eligible);
        assert_eq!(candidate.required, ["workspace/document-layout.json"]);
        assert_eq!(
            provider
                .commit(
                    uuid::Uuid::new_v4().to_string(),
                    vec![document.clone()],
                    "incomplete".into(),
                )
                .err()
                .unwrap(),
            "svn_commit_dependency_missing"
        );
        let outcome = provider
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![document.clone(), "workspace/document-layout.json".into()],
                "C linked document and layout".into(),
            )
            .unwrap();
        assert_eq!(outcome.paths.len(), 2);
    }
    #[test]
    #[ignore = "run with owned M7_B_NESTED_PROJECT and M7_SVN_TEST_CLI"]
    fn owned_nested_project_acquires_document_lock_with_unrelated_private_change() {
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI");
        let root = PathBuf::from(std::env::var("M7_B_NESTED_PROJECT").expect("owned project"));
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-b-nested-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let config = manager.inner.config_root.join("foreign-https-session");
        *manager.inner.identity.lock().unwrap() = Some(Identity {
            origin: "https://unrelated.example".into(),
            url: "https://unrelated.example/svn/project".into(),
            username: "unrelated-user".into(),
            config,
            password: None,
            remember: true,
        });
        let provider = SvnLockService::new(manager, root);
        let target =
            ProjectRelativePath::parse("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json")
                .unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut held = provider
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .expect("nested project document lock");
        provider.validate(held.as_mut()).unwrap();
        provider.release(held.as_mut()).unwrap();
    }
    #[test]
    #[ignore = "run with owned M7_B_WC_A, M7_B_WC_B, and M7_SVN_TEST_CLI"]
    fn owned_existing_documents_lock_commit_and_exclude_internal_paths() {
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI");
        let a = PathBuf::from(std::env::var("M7_B_WC_A").expect("owned WC A"));
        let b = PathBuf::from(std::env::var("M7_B_WC_B").expect("owned WC B"));
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-b-live-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let first = SvnLockService::new(manager.clone(), a.clone());
        let second = SvnLockService::new(manager.clone(), b.clone());
        let target =
            ProjectRelativePath::parse("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json")
                .unwrap();
        let session = LockSessionId::generate().unwrap();
        let mut held = first
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .expect("first WC acquires the server lock");
        assert_eq!(first.held_targets(), vec![target.clone()]);
        assert!(
            second
                .acquire(
                    LockAcquireRequest::new(
                        &"b".repeat(64),
                        &LockSessionId::generate().unwrap(),
                        &target,
                    )
                    .unwrap()
                )
                .is_err(),
            "another WC cannot inherit this lock"
        );
        let file = a.join(target.as_str());
        let original = fs::read_to_string(&file).unwrap();
        let modified = original.replacen("A 출처 문서", "A 출처 문서 B 실제 커밋", 1);
        assert_ne!(original, modified);
        fs::write(&file, &modified).unwrap();
        first.validate(held.as_mut()).unwrap();
        let candidates = first.candidates().unwrap();
        assert!(candidates
            .iter()
            .any(|row| row.path == target.as_str() && row.eligible));
        assert!(!candidates
            .iter()
            .any(|row| row.path.starts_with(".worldbuild/")));
        assert!(first
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![".worldbuild/transactions/dummy.json".into()],
                "invalid".into()
            )
            .is_err());
        for excluded in [
            ".worldbuild/transactions/versioned-private.txt",
            ".worldbuild/transactions/scheduled-private.txt",
            ".worldbuild/transactions/unversioned-private.txt",
            "../wc-b/documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json",
        ] {
            assert!(first
                .commit(
                    uuid::Uuid::new_v4().to_string(),
                    vec![excluded.into()],
                    "forged selection".into()
                )
                .is_err());
        }
        let result = first
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![target.as_str().into()],
                "B owned selection".into(),
            )
            .unwrap();
        assert_eq!(result.paths, vec![target.as_str()]);
        assert!(result.unlock_pending.is_empty());
        first.release(held.as_mut()).unwrap();
        assert!(first.held_targets().is_empty());
        let args = vec![
            "update".into(),
            "--non-interactive".into(),
            "--".into(),
            format!("{}@", b.to_string_lossy()),
        ];
        run(
            &manager.cli().unwrap(),
            &args,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(b.join(target.as_str())).unwrap(),
            modified
        );
        assert_eq!(
            fs::read_to_string(b.join(".worldbuild/transactions/versioned-private.txt")).unwrap(),
            "before B selected commit"
        );
        assert!(!b
            .join(".worldbuild/transactions/scheduled-private.txt")
            .exists());
        assert!(!b
            .join(".worldbuild/transactions/unversioned-private.txt")
            .exists());
        let mut second_held = second
            .acquire(
                LockAcquireRequest::new(
                    &"b".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .expect("second WC can now acquire");
        second.release(second_held.as_mut()).unwrap();
    }
    #[test]
    fn status_xml_preserves_dirty_and_server_revision() {
        let xml = r#"<status><target path="x"><entry path="x/a"><wc-status item="modified"/><repos-status item="normal"/></entry><against revision="7"/></target></status>"#;
        let (entries, revision) = parse_status(xml).unwrap();
        assert_eq!(entries[0].local, "modified");
        assert_eq!(entries[0].remote.as_deref(), Some("normal"));
        assert_eq!(revision.as_deref(), Some("7"));
    }
    #[test]
    #[ignore = "run with owned M7_FIX3_WC_A/B and installed SVN CLI"]
    fn owned_uncommitted_release_preserves_lock_until_selected_commit() {
        let cli = std::env::var("M7_SVN_TEST_CLI").unwrap();
        let a = PathBuf::from(std::env::var("M7_FIX3_WC_A").unwrap());
        let b = PathBuf::from(std::env::var("M7_FIX3_WC_B").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-fix3-lock-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let target =
            ProjectRelativePath::parse("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json")
                .unwrap();
        let first = SvnLockService::new(manager.clone(), a.clone());
        let second = SvnLockService::new(manager.clone(), b.clone());
        let session = LockSessionId::generate().unwrap();
        let mut held = first
            .acquire(LockAcquireRequest::new(&"a".repeat(64), &session, &target).unwrap())
            .unwrap();
        let file = a.join(target.as_str());
        let original = fs::read_to_string(&file).unwrap();
        let modified = original.replacen("A 출처 문서", "A 출처 문서 잠금 유지", 1);
        assert_ne!(original, modified);
        fs::write(&file, &modified).unwrap();
        first.validate(held.as_mut()).unwrap();
        first.release(held.as_mut()).unwrap();
        assert!(first.held_targets().is_empty());
        assert!(
            second
                .acquire(
                    LockAcquireRequest::new(
                        &"b".repeat(64),
                        &LockSessionId::generate().unwrap(),
                        &target,
                    )
                    .unwrap()
                )
                .is_err(),
            "the other WC must not inherit the uncommitted lock"
        );
        let reopened = SvnLockService::new(manager.clone(), a.clone());
        let mut reused = reopened
            .acquire(
                LockAcquireRequest::new(
                    &"a".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .expect("same WC reuses the surviving token");
        reopened.validate(reused.as_mut()).unwrap();
        let committed = reopened
            .commit(
                uuid::Uuid::new_v4().to_string(),
                vec![target.as_str().into()],
                "owned lock lifecycle".into(),
            )
            .unwrap();
        assert!(committed.unlock_pending.is_empty());
        reopened.release(reused.as_mut()).unwrap();
        let args = vec![
            "update".into(),
            "--non-interactive".into(),
            "--".into(),
            format!("{}@", b.to_string_lossy()),
        ];
        run(
            &manager.cli().unwrap(),
            &args,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(b.join(target.as_str())).unwrap(),
            modified
        );
        let mut second_held = second
            .acquire(
                LockAcquireRequest::new(
                    &"b".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .unwrap();
        second.release(second_held.as_mut()).unwrap();
    }
    #[test]
    fn release_keeps_modified_documents_and_rejects_unknown_wc_state() {
        let mut row = StatusEntry {
            path: "document.json".into(),
            local: "normal".into(),
            properties: Some("normal".into()),
            remote: None,
            remote_properties: None,
            lock_owner: None,
            wc_locked: true,
            working_copy_locked: false,
            needs_lock: true,
            remote_only: false,
        };
        assert_eq!(keep_server_lock_after_release(&row).unwrap(), false);
        row.local = "modified".into();
        assert_eq!(keep_server_lock_after_release(&row).unwrap(), true);
        row.local = "normal".into();
        row.properties = Some("modified".into());
        assert_eq!(keep_server_lock_after_release(&row).unwrap(), true);
        row.local = "conflicted".into();
        assert!(keep_server_lock_after_release(&row).is_err());
        row.local = "normal".into();
        row.working_copy_locked = true;
        assert!(keep_server_lock_after_release(&row).is_err());
    }
    #[test]
    fn anonymous_cli_requests_are_confined_to_the_app_config() {
        let root =
            std::env::temp_dir().join(format!("worldbuild-m7-anonymous-{}", uuid::Uuid::new_v4()));
        let manager = Manager::new(root.clone());
        let args = args_with_auth(&manager, None, "status", &["--xml"], "C:\\project");
        let option = args.iter().position(|arg| arg == "--config-dir").unwrap();
        assert_eq!(args[option + 1], root.join("anonymous").to_string_lossy());
        assert!(args.contains(&"--no-auth-cache".to_owned()));
        assert!(!args.contains(&"--password-from-stdin".to_owned()));
    }
    #[test]
    fn status_xml_keeps_property_and_local_lock_cause() {
        let xml = r#"<status><target path="x"><entry path="x/a"><wc-status item="normal" props="modified"><lock><owner>finn001</owner></lock></wc-status><repos-status item="normal" props="modified"/></entry></target></status>"#;
        let (entries, _) = parse_status(xml).unwrap();
        assert_eq!(entries[0].properties.as_deref(), Some("modified"));
        assert_eq!(entries[0].remote_properties.as_deref(), Some("modified"));
        assert!(entries[0].wc_locked);
        assert_eq!(entries[0].lock_owner.as_deref(), Some("finn001"));
    }
    #[test]
    fn scheduled_delete_status_retains_local_lock_token() {
        let xml = r#"<status><target path="x"><entry path="x/templates/a.json"><wc-status item="deleted" props="none"><lock><token>opaque-local</token><owner>finn002</owner></lock></wc-status></entry></target></status>"#;
        assert_eq!(
            parse_lock_token(xml).unwrap().as_deref(),
            Some("opaque-local")
        );
    }
    #[test]
    fn remote_lock_diagnostic_exposes_owner_without_token() {
        let xml = r#"<info><entry><lock><token>secret-token</token><owner>finn002</owner></lock></entry></info>"#;
        let diagnostic = parse_lock_owner(xml).unwrap();
        assert!(diagnostic.locked);
        assert_eq!(diagnostic.owner.as_deref(), Some("finn002"));
        let absent = parse_lock_owner("<info><entry/></info>").unwrap();
        assert!(!absent.locked);
        assert!(absent.owner.is_none());
    }
    #[test]
    #[ignore = "run with owned M7_FIX_CONFLICT_WC and installed M7_SVN_TEST_CLI"]
    fn owned_conflicted_document_rejects_edit_lock() {
        let project = PathBuf::from(std::env::var("M7_FIX_CONFLICT_WC").unwrap());
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-fix-conflict-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(
            probe(
                &manager,
                Some(std::env::var("M7_SVN_TEST_CLI").unwrap()),
                None
            )
            .installed
        );
        let provider = SvnLockService::new(manager, project);
        let document = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let row = provider.local_document_status(document).unwrap();
        assert_eq!(row.local, "conflicted");
        let target = ProjectRelativePath::parse(&format!("documents/{document}.json")).unwrap();
        let result = provider.acquire(
            LockAcquireRequest::new(
                &"a".repeat(64),
                &LockSessionId::generate().unwrap(),
                &target,
            )
            .unwrap(),
        );
        assert!(result.is_err());
    }
    #[test]
    #[ignore = "run with owned M7_FIX_REUSE_WC and installed M7_SVN_TEST_CLI"]
    fn owned_existing_wc_token_can_be_reused_but_not_inferred_from_identity() {
        let project = PathBuf::from(std::env::var("M7_FIX_REUSE_WC").unwrap());
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-fix-reuse-{}", uuid::Uuid::new_v4())),
        );
        assert!(
            probe(
                &manager,
                Some(std::env::var("M7_SVN_TEST_CLI").unwrap()),
                None
            )
            .installed
        );
        let target =
            ProjectRelativePath::parse("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json")
                .unwrap();
        let first = SvnLockService::new(manager.clone(), project.clone());
        let mut first_lock = first
            .acquire(
                LockAcquireRequest::new(
                    &"a".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .unwrap();
        let reopened = SvnLockService::new(manager, project);
        let mut reused = reopened
            .acquire(
                LockAcquireRequest::new(
                    &"b".repeat(64),
                    &LockSessionId::generate().unwrap(),
                    &target,
                )
                .unwrap(),
            )
            .unwrap();
        reopened.validate(reused.as_mut()).unwrap();
        reopened.release(reused.as_mut()).unwrap();
        first.release(first_lock.as_mut()).unwrap();
    }
    #[test]
    fn server_added_paths_are_incoming_only_until_a_local_path_collides() {
        let root =
            std::env::temp_dir().join(format!("worldbuild-m7-remote-add-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let file = root.join("server-only.txt");
        let xml = format!(
            "<status><target path=\"x\"><entry path=\"{}\"><wc-status item=\"none\"/><repos-status item=\"added\"/></entry></target></status>",
            file.display()
        );
        let remote = parse_status(&xml).unwrap().0;
        let mut clean = Vec::new();
        merge_remote_entries(&mut clean, remote.clone());
        assert!(clean[0].remote_only);
        assert!(!entry_dirty(&clean[0]));
        assert_eq!(update_block(&clean, None, None), None);

        fs::write(&file, b"local occupant").unwrap();
        let mut collided = Vec::new();
        merge_remote_entries(&mut collided, remote);
        assert_eq!(collided[0].local, "obstructed");
        assert_eq!(
            update_block(&collided, None, None),
            Some("svn_conflicted_working_copy")
        );
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn cancelled_update_preserves_db_lock_and_only_unlocked_incoming_can_resume() {
        let xml = r#"<status><target path="x"><entry path="x"><wc-status item="incomplete" props="none" wc-locked="true"/></entry><entry path="x/payload"><wc-status item="incomplete" wc-locked="true"/><repos-status item="modified"/></entry><entry path="x/payload/a"><wc-status item="normal"/><repos-status item="modified"/></entry></target></status>"#;
        let (mut entries, _) = parse_status(xml).unwrap();
        assert!(entries[0].working_copy_locked);
        assert_eq!(recovery_state(&entries), Some("cleanupRequired"));
        assert_eq!(
            update_block(&entries, None, recovery_state(&entries)),
            Some("svn_working_copy_cleanup_required")
        );
        for entry in &mut entries {
            entry.working_copy_locked = false;
        }
        assert_eq!(recovery_state(&entries), Some("resumeRequired"));
        assert_eq!(update_block(&entries, None, recovery_state(&entries)), None);
        entries[2].local = "modified".into();
        assert_eq!(
            update_block(&entries, None, recovery_state(&entries)),
            Some("svn_dirty_working_copy")
        );
        entries[2].local = "normal".into();
        assert_eq!(
            update_block(
                &entries,
                Some("svn_connection_failed"),
                recovery_state(&entries)
            ),
            Some("svn_server_unconfirmed")
        );
    }
    #[test]
    #[ignore = "run explicitly with owned M7_FIX_TARGET, M7_FIX_PARTIAL, M7_FIX_DIRTY, M7_FIX_COLLISION, and installed M7_SVN_TEST_CLI"]
    fn owned_live_copies_report_remote_add_and_cancel_recovery_separately() {
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI");
        let target = std::env::var("M7_FIX_TARGET").expect("owned receiving copy");
        let partial = std::env::var("M7_FIX_PARTIAL").expect("owned interrupted copy");
        let dirty = std::env::var("M7_FIX_DIRTY").expect("owned modified copy");
        let collision = std::env::var("M7_FIX_COLLISION").expect("owned colliding copy");
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-fix-live-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let clean = status(&manager, Path::new(&target), &AtomicBool::new(false)).unwrap();
        assert!(clean.server_error.is_none());
        assert_eq!(clean.update_block, None);
        assert!(clean.entries.iter().any(|entry| {
            entry.remote_only && entry.local == "none" && entry.remote.as_deref() == Some("added")
        }));
        assert!(!clean.entries.iter().any(entry_dirty));

        let interrupted = status(&manager, Path::new(&partial), &AtomicBool::new(false)).unwrap();
        assert_eq!(interrupted.recovery.as_deref(), Some("cleanupRequired"));
        assert_eq!(
            interrupted.update_block.as_deref(),
            Some("svn_working_copy_cleanup_required")
        );
        assert!(interrupted
            .entries
            .iter()
            .any(|entry| { entry.local == "incomplete" && entry.working_copy_locked }));

        for root in [dirty, collision] {
            let actual = status(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
            assert_eq!(
                actual.update_block.as_deref(),
                Some("svn_dirty_working_copy")
            );
            assert!(actual
                .entries
                .iter()
                .any(|entry| entry.local == "unversioned"));
        }
    }
    #[test]
    fn rejects_http_and_url_credentials() {
        assert!(secure_url("http://example.org/svn").is_err());
        assert!(secure_url("https://user:password@example.org/svn").is_err());
    }
    #[test]
    fn cancelled_request_releases_single_owner_for_the_next_request() {
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-lifecycle-{}",
            uuid::Uuid::new_v4()
        )));
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let (guard, cancelled) = manager.begin(&first).unwrap();
        assert!(matches!(manager.begin(&second), Err(error) if error == "svn_busy"));
        manager.cancel_active();
        assert!(cancelled.load(Ordering::Acquire));
        drop(guard);
        let (_next, next_cancelled) = manager.begin(&second).unwrap();
        assert!(!next_cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn m8_install_seal_and_new_svn_start_share_the_active_mutex() {
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m8-svn-{}", uuid::Uuid::new_v4())),
        );
        let request = uuid::Uuid::new_v4().to_string();
        let (guard, _) = manager.begin(&request).unwrap();
        assert!(!manager.seal_for_update());
        drop(guard);
        assert!(manager.seal_for_update());
        assert!(matches!(manager.begin(&request),Err(reason) if reason=="svn_close_project_first"));
        manager.unseal_update();
        assert!(manager.begin(&request).is_ok());
    }
    #[test]
    #[cfg(windows)]
    fn remembered_password_survives_restart_only_as_protected_bytes() {
        let root = std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-remember-{}",
            uuid::Uuid::new_v4()
        ));
        let origin = "https://example.org";
        let config = config_for_root(&root, origin, "probe001");
        fs::create_dir_all(&config).unwrap();
        let encrypted = protect_password("비밀123!").unwrap();
        assert!(!encrypted
            .windows("비밀123!".len())
            .any(|part| part == "비밀123!".as_bytes()));
        fs::write(config.join(CREDENTIAL_FILE), encrypted).unwrap();
        fs::write(
            root.join("current.json"),
            serde_json::to_vec(&Remembered {
                origin: origin.into(),
                url: "https://example.org/svn/project".into(),
                username: "probe001".into(),
            })
            .unwrap(),
        )
        .unwrap();
        let restarted = Manager::new(root.clone());
        assert_eq!(
            restarted.identity().and_then(|i| i.password),
            Some("비밀123!".into())
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "run explicitly with installed M7_SVN_TEST_CLI"]
    fn cli_sends_exact_password_bytes_without_a_line_ending() {
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                assert!(Instant::now() < until, "SVN sent no Basic authorization");
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    Err(error) => panic!("HTTP accept failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..n]);
                    assert!(request.len() < 16 * 1024);
                    if request.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                let headers = String::from_utf8_lossy(&request);
                let authorization = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("Authorization")
                        .then_some(value.trim().to_owned())
                });
                if let Some(value) = authorization {
                    let encoded = value.strip_prefix("Basic ").unwrap();
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .unwrap();
                    stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                    return decoded;
                }
                stream
                    .write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"Probe\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
            }
        });
        let config = std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-password-{}",
            uuid::Uuid::new_v4()
        ));
        let args = vec![
            "info".into(),
            "--xml".into(),
            "--config-dir".into(),
            config.to_string_lossy().into_owned(),
            "--username".into(),
            "probe001".into(),
            "--password-from-stdin".into(),
            "--non-interactive".into(),
            "--".into(),
            format!("http://{address}/probe@"),
        ];
        let _ = run(
            Path::new(&cli),
            &args,
            Some("비밀123!"),
            &AtomicBool::new(false),
        );
        assert_eq!(server.join().unwrap(), "probe001:비밀123!".as_bytes());
        let _ = fs::remove_dir_all(config);
    }
    #[test]
    #[ignore = "run explicitly with an owned M7_SVN_TEST_ROOT and installed M7_SVN_TEST_CLI"]
    fn local_cli_status_reports_incoming_without_mutating_the_copy() {
        let root = std::env::var("M7_SVN_TEST_ROOT").expect("owned fixture path");
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-incoming-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli), None).installed);
        let before = inspect(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        let result = status(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        let after = inspect(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        assert_eq!(before.revision, after.revision);
        assert!(result.server_error.is_none());
        assert!(result.entries.iter().any(|entry| {
            entry.local == "normal" && entry.remote.as_deref() == Some("modified")
        }));
    }
    #[test]
    #[ignore = "run explicitly with an owned M7_SVN_TEST_ROOT and installed M7_SVN_TEST_CLI"]
    fn local_cli_reports_repository_changes_without_https_claim() {
        let root = std::env::var("M7_SVN_TEST_ROOT").expect("owned fixture path");
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let manager = Manager::new(
            std::env::temp_dir().join(format!("worldbuild-m7-svn-test-{}", uuid::Uuid::new_v4())),
        );
        assert!(probe(&manager, Some(cli), None).installed);
        let inspected = inspect(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        assert!(inspected.root.ends_with("project"));
        assert!(inspected.wc_root != inspected.root);
        let status = status(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        assert!(status.server_revision.is_some());
        assert!(status.server_error.is_none());
        assert!(status
            .entries
            .iter()
            .any(|entry| entry.remote.as_deref() == Some("modified")));
        let after = update_closed(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        assert!(after.entries.iter().all(|entry| entry.remote.is_none()));
        let updated = fs::read_to_string(
            Path::new(&root).join("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json"),
        )
        .unwrap();
        assert!(updated.contains("M7 업데이트"));
    }
    #[test]
    #[ignore = "run explicitly with the owned M7 SVN fixture and installed CLI"]
    fn local_cli_rejects_dirty_update_without_changing_the_file() {
        let root = std::env::var("M7_SVN_TEST_ROOT").expect("owned fixture path");
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-dirty-test-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli), None).installed);
        let file = Path::new(&root).join("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json");
        let original = fs::read(&file).unwrap();
        let mut dirty = original.clone();
        dirty.extend_from_slice(b"\n");
        fs::write(&file, &dirty).unwrap();
        let result = update_closed(&manager, Path::new(&root), &AtomicBool::new(false));
        assert!(matches!(result, Err(error) if error == "svn_dirty_working_copy"));
        assert_eq!(fs::read(&file).unwrap(), dirty);
        fs::write(&file, original).unwrap();
    }
    #[test]
    #[ignore = "run explicitly with the owned M7 SVN fixture and installed CLI"]
    fn local_cli_checkout_into_new_owned_folder() {
        let root = std::env::var("M7_SVN_TEST_ROOT").expect("owned fixture path");
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let manager = Manager::new(std::env::temp_dir().join(format!(
            "worldbuild-m7-svn-checkout-test-{}",
            uuid::Uuid::new_v4()
        )));
        assert!(probe(&manager, Some(cli), None).installed);
        let info = inspect(&manager, Path::new(&root), &AtomicBool::new(false)).unwrap();
        let destination = Path::new(&root)
            .parent()
            .and_then(Path::parent)
            .expect("owned fixture parent")
            .join(format!("-한글 공백 @-checkout-{}", uuid::Uuid::new_v4()));
        let args = args_with_auth(
            &manager,
            None,
            "checkout",
            &["--depth", "infinity", "--ignore-externals"],
            &info.url,
        );
        let mut args = args;
        fs::create_dir(&destination).unwrap();
        args.push(".".into());
        run_command_in(
            &manager.cli().unwrap(),
            &args,
            None,
            &AtomicBool::new(false),
            &destination,
        )
        .unwrap();
        let received = inspect(&manager, &destination, &AtomicBool::new(false)).unwrap();
        assert_eq!(received.wc_root, received.root);
        let current = status(&manager, &destination, &AtomicBool::new(false)).unwrap();
        assert!(current.server_error.is_none());
        assert!(current.entries.iter().all(|entry| entry.local == "normal"));
        let updated = update_closed(&manager, &destination, &AtomicBool::new(false)).unwrap();
        assert!(updated.server_error.is_none());
        assert!(destination
            .join("documents/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.json")
            .is_file());
    }
    #[test]
    #[ignore = "run explicitly with installed M7_SVN_TEST_CLI and generated Tauri permissions"]
    fn app_ipc_permission_reaches_actual_svn_probe() {
        let cli = std::env::var("M7_SVN_TEST_CLI").expect("installed CLI path");
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        let app = crate::commands::register(mock_builder().plugin(tauri_plugin_dialog::init()))
            .manage(Manager::new(std::env::temp_dir().join(format!(
                "worldbuild-m7-svn-ipc-test-{}",
                uuid::Uuid::new_v4()
            ))))
            .build(context)
            .unwrap();
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let request = InvokeRequest {
            cmd: "svn_probe".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(serde_json::json!({"path":cli})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        };
        let response: serde_json::Value = tauri::test::get_ipc_response(&main, request)
            .unwrap()
            .deserialize()
            .unwrap();
        assert_eq!(response["installed"], true);
        assert!(response["version"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
    }
}
