//! 신뢰 후보와 다운로드를 native가 소유한다. JS에서 URL/키/실행 인자를 받지 않는다.
pub(crate) mod handoff;
mod policy;
#[cfg(test)]
mod tests;
use crate::state::AppState;
use policy::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tauri::{Emitter, Manager as _, Runtime, State, Webview};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    revision: u64,
    test_mode: bool,
    install_test: bool,
    distribution: &'static str,
    current_version: String,
    channel: &'static str,
    phase: &'static str,
    candidate: Option<String>,
    version: Option<String>,
    notes: String,
    downloaded: u64,
    total: Option<u64>,
    error: Option<&'static str>,
    notify: bool,
    startup_complete: bool,
    release_url: Option<String>,
    handoff: Option<handoff::Snapshot>,
}
struct Inner {
    status: Status,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
    epoch: u64,
    cancel: Option<Arc<tokio::sync::Notify>>,
    background_checked: bool,
    consented: bool,
}
pub(crate) struct Manager {
    inner: Mutex<Inner>,
    settings: PathBuf,
    handoff_load_failed: bool,
    #[cfg(feature = "updater-test")]
    test_boundary_calls: std::sync::atomic::AtomicUsize,
}
#[cfg(feature = "updater-test")]
pub(crate) fn record_window_metrics<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Some(manager) = app.try_state::<Arc<Manager>>() else {
        return;
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) {
        let result = serde_json::json!({"width":size.width,"height":size.height,"scaleFactor":scale,"physicalClient":true,"settingsRoot":manager.settings.to_string_lossy()});
        if std::fs::create_dir_all(&manager.settings)
            .and_then(|_| {
                std::fs::write(
                    manager.settings.join("updater-test-window.json"),
                    result.to_string(),
                )
            })
            .is_err()
        {
            crate::diagnostic_log::record(
                "updater",
                "fixture",
                "window_metrics",
                "error",
                None,
                None,
            );
        }
    }
}
impl Manager {
    pub(crate) fn new(settings: PathBuf, version: String) -> Self {
        let previous = handoff::previous(&settings);
        let handoff_load_failed = previous.is_err();
        Self {
            handoff_load_failed,
            settings: settings.clone(),
            #[cfg(feature = "updater-test")]
            test_boundary_calls: std::sync::atomic::AtomicUsize::new(0),
            inner: Mutex::new(Inner {
                status: Status {
                    revision: 0,
                    test_mode: cfg!(feature = "updater-test"),
                    install_test: cfg!(feature = "updater-integration-test"),
                    distribution: if cfg!(feature = "updater-integration-test")
                        && packaged() == Some(false)
                    {
                        "github"
                    } else {
                        distribution(
                            env!("WORLDBUILD_BUILD_CHANNEL"),
                            env!("WORLDBUILD_PACKAGE_MODE"),
                            packaged(),
                        )
                    },
                    current_version: version,
                    channel: if cfg!(any(
                        feature = "updater-test",
                        feature = "updater-integration-test"
                    )) {
                        "test"
                    } else {
                        "stable"
                    },
                    phase: "idle",
                    candidate: None,
                    version: None,
                    notes: String::new(),
                    downloaded: 0,
                    total: None,
                    error: None,
                    notify: false,
                    startup_complete: false,
                    release_url: None,
                    handoff: previous.ok().flatten(),
                },
                update: None,
                bytes: None,
                epoch: 0,
                cancel: None,
                background_checked: false,
                consented: false,
            }),
        }
    }
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn snapshot(&self, state: &AppState) -> Status {
        let mut inner = self.lock();
        if inner.status.phase == "preparing" && state.update_intent().is_none() {
            inner.status.phase = "cancelled";
            inner.consented = false;
            inner.bytes = None;
            inner.status.revision = inner.status.revision.saturating_add(1);
        }
        inner.status.clone()
    }
    pub(crate) fn observe<R: Runtime>(&self, app: &tauri::AppHandle<R>, state: &AppState) {
        let revision = self.lock().status.revision;
        let status = self.snapshot(state);
        if status.revision != revision {
            emit(app, &status);
        }
    }
    pub(crate) fn cancel_all(&self) {
        let mut inner = self.lock();
        if inner.cancel.is_none() && inner.update.is_none() && inner.bytes.is_none() {
            return;
        }
        inner.epoch = inner.epoch.saturating_add(1);
        if let Some(cancel) = inner.cancel.take() {
            cancel.notify_one();
        }
        inner.bytes = None;
        inner.update = None;
        inner.consented = false;
        inner.status.phase = "cancelled";
        inner.status.notify = false;
        inner.status.revision = inner.status.revision.saturating_add(1);
    }
}

fn allowed_distribution(status: &Status) -> bool {
    status.distribution == "github"
        || (cfg!(feature = "updater-test")
            && status.distribution == "dev"
            && packaged() == Some(false))
}

#[cfg(windows)]
fn packaged() -> Option<bool> {
    use windows::Win32::{
        Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER},
        Storage::Packaging::Appx::GetCurrentPackageFullName,
    };
    let mut length = 0;
    let result = unsafe { GetCurrentPackageFullName(&mut length, None) };
    if result == APPMODEL_ERROR_NO_PACKAGE {
        Some(false)
    } else if result == ERROR_INSUFFICIENT_BUFFER && length > 0 {
        Some(true)
    } else {
        None
    }
}
#[cfg(not(windows))]
fn packaged() -> Option<bool> {
    None
}

fn caller<R: Runtime>(webview: &Webview<R>, state: &AppState) -> Result<(), &'static str> {
    state
        .caller(webview.label(), webview.window().label())
        .map(|_| ())
        .map_err(|_| "forbidden")
}
fn emit<R: Runtime>(app: &tauri::AppHandle<R>, status: &Status) {
    if app.emit_to("main", "updater-state", status).is_err() {
        crate::diagnostic_log::record("updater", "event", "delivery", "error", None, None);
    }
}
fn failure(error: &tauri_plugin_updater::Error) -> &'static str {
    use tauri_plugin_updater::Error::*;
    match error {
        Minisign(_)
        | Base64(_)
        | SignatureUtf8(_)
        | MissingSignedVersion
        | SignedVersionMismatch { .. } => "signature",
        TargetNotFound(_) | TargetsNotFound(_) | UnsupportedArch | UnsupportedOs
        | InvalidUpdaterFormat => "target",
        Serialization(_) | Semver(_) => "metadata",
        _ => "network",
    }
}
#[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
fn client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent("Dreamrugi-Worldbuild-Updater/0.2.0")
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !redirect(attempt.url()) {
                attempt.error("updater_redirect_rejected")
            } else {
                attempt.follow()
            }
        }))
}
#[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
async fn release_matches(update: &Update) -> Result<(), &'static str> {
    let url = format!(
        "https://api.github.com/repos/{REPOSITORY}/releases/tags/v{}",
        update.version
    );
    let response = client()
        .build()
        .map_err(|_| "network")?
        .get(url)
        .send()
        .await
        .map_err(|_| "network")?;
    if !response.status().is_success() {
        return Err("network");
    }
    let raw: serde_json::Value = response.json().await.map_err(|_| "metadata")?;
    if raw["draft"].as_bool() != Some(false)
        || raw["prerelease"].as_bool() != update.raw_json["prerelease"].as_bool()
        || raw["tag_name"].as_str() != Some(&format!("v{}", update.version))
        || raw["assets"].as_array().is_none_or(|assets| {
            !assets.iter().any(|a| {
                a["browser_download_url"].as_str() == Some(update.download_url.as_str())
                    && a["state"] == "uploaded"
            })
        })
    {
        return Err("target");
    }
    Ok(())
}

fn allowed_redirect(url: &url::Url) -> bool {
    #[cfg(any(feature = "updater-test", feature = "updater-integration-test"))]
    {
        if let Ok(endpoint) = url::Url::parse(env!("WORLDBUILD_UPDATER_TEST_ENDPOINT")) {
            if url.origin() == endpoint.origin()
                && url.username().is_empty()
                && url.password().is_none()
            {
                return true;
            }
        }
    }
    redirect(url)
}

fn candidate_metadata(
    current: &semver::Version,
    version: &str,
    url: &url::Url,
    raw: &serde_json::Value,
) -> Result<bool, &'static str> {
    #[cfg(any(feature = "updater-test", feature = "updater-integration-test"))]
    {
        let endpoint =
            url::Url::parse(env!("WORLDBUILD_UPDATER_TEST_ENDPOINT")).map_err(|_| "target")?;
        if url.origin() != endpoint.origin() || url.query().is_some() || url.fragment().is_some() {
            return Err("target");
        }
        let mut official = url.clone();
        official.set_scheme("https").map_err(|_| "target")?;
        official
            .set_host(Some("github.com"))
            .map_err(|_| "target")?;
        official.set_port(None).map_err(|_| "target")?;
        return metadata(current, version, &official, raw, true);
    }
    #[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
    metadata(current, version, url, raw, false)
}

#[tauri::command]
pub(crate) fn updater_status<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    Ok(manager.snapshot(&state))
}
#[tauri::command]
pub(crate) async fn updater_check<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
    retry: bool,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    let (epoch, cancel, current, cached) = {
        let mut inner = manager.lock();
        if !allowed_distribution(&inner.status) {
            inner.status.phase = "unavailable";
            inner.status.revision = inner.status.revision.saturating_add(1);
            return Ok(inner.status.clone());
        }
        if !state.update_check_allowed() {
            return Err("shutdown_blocked");
        }
        if inner.status.startup_complete || (!retry && inner.background_checked) {
            return Ok(inner.status.clone());
        }
        if retry && inner.status.phase != "failed" {
            return Err("target");
        }
        inner.background_checked = true;
        if matches!(
            inner.status.phase,
            "checking" | "downloading" | "preparing" | "installing"
        ) {
            return Ok(inner.status.clone());
        }
        let cached = if inner.status.phase == "ready" {
            inner.status.candidate.clone().zip(inner.bytes.take())
        } else {
            None
        };
        inner.epoch += 1;
        inner.update = None;
        inner.bytes = None;
        inner.consented = false;
        inner.status.phase = "checking";
        inner.status.error = None;
        inner.status.candidate = None;
        inner.status.version = None;
        inner.status.notes.clear();
        inner.status.downloaded = 0;
        inner.status.total = None;
        inner.status.notify = false;
        let cancel = Arc::new(tokio::sync::Notify::new());
        inner.cancel = Some(cancel.clone());
        inner.status.revision += 1;
        emit(&app, &inner.status);
        (
            inner.epoch,
            cancel,
            inner.status.current_version.clone(),
            cached,
        )
    };
    #[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
    let endpoint =
        format!("https://raw.githubusercontent.com/{REPOSITORY}/main/updates/stable.json");
    #[cfg(any(feature = "updater-test", feature = "updater-integration-test"))]
    let endpoint = { env!("WORLDBUILD_UPDATER_TEST_ENDPOINT").to_string() };
    #[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
    let public_key = include_str!("../../packaging/updater-public.key.pub").trim();
    #[cfg(any(feature = "updater-test", feature = "updater-integration-test"))]
    let public_key = env!("WORLDBUILD_UPDATER_TEST_PUBLIC_KEY");
    let result = async {
        let before_exit_app = app.clone();
        let updater = app
            .updater_builder()
            .endpoints(vec![endpoint.parse().map_err(|_| "metadata")?])
            .map_err(|_| "metadata")?
            .pubkey(public_key)
            .timeout(Duration::from_secs(30))
            .target(TARGET)
            .clear_installer_args()
            .on_before_exit(move || {
                // 저장/보관은 이미 완료됐다. plugin의 동기 process::exit 경로만 기록한다.
                before_exit_app.cleanup_before_exit();
                crate::diagnostic_log::mark_update_handoff();
            })
            .version_comparator(|_, _| true)
            .configure_client(|builder| {
                builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 5 || !allowed_redirect(attempt.url()) {
                        attempt.error("updater_redirect_rejected")
                    } else {
                        attempt.follow()
                    }
                }))
            })
            .build()
            .map_err(|e| failure(&e))?;
        let update = updater.check().await.map_err(|e| failure(&e))?;
        let Some(update) = update else {
            return Ok(None);
        };
        let current = semver::Version::parse(&current).map_err(|_| "metadata")?;
        if !candidate_metadata(
            &current,
            &update.version,
            &update.download_url,
            &update.raw_json,
        )? {
            return Ok(None);
        }
        #[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
        release_matches(&update).await?;
        Ok::<_, &'static str>(Some(update))
    };
    let result = tokio::select! { value = tokio::time::timeout(CHECK_TIMEOUT,result) => value.unwrap_or(Err("timeout")), _ = cancel.notified() => Err("cancelled") };
    let mut inner = manager.lock();
    if inner.epoch != epoch {
        return Ok(inner.status.clone());
    }
    inner.cancel = None;
    match result {
        Ok(Some(update)) => {
            let candidate = format!(
                "{:x}",
                Sha256::digest(format!(
                    "{}\n{}\n{}",
                    update.version, update.download_url, update.signature
                ))
            );
            inner.status.phase = "available";
            if let Some((cached_candidate, bytes)) = cached {
                if cached_candidate == candidate {
                    inner.status.downloaded = bytes.len() as u64;
                    inner.status.total = Some(bytes.len() as u64);
                    inner.bytes = Some(bytes);
                    inner.status.phase = "ready";
                }
            }
            inner.status.candidate = Some(candidate);
            inner.status.version = Some(update.version.clone());
            inner.status.notes = update.body.clone().unwrap_or_default();
            inner.status.notify = true;
            inner.status.release_url = if cfg!(feature = "updater-integration-test") {
                None
            } else {
                Some(release_url(&update.version)?)
            };
            inner.update = Some(update);
        }
        Ok(None) => {
            inner.status.phase = "latest";
            inner.status.notify = true;
        }
        Err(code) => {
            inner.status.phase = "failed";
            inner.status.error = Some(code);
            inner.status.notify = true;
            crate::diagnostic_log::record("updater", "check", code, "error", None, None);
        }
    }
    inner.status.revision = inner.status.revision.saturating_add(1);
    let status = inner.status.clone();
    drop(inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub(crate) async fn updater_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
    candidate: String,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    let (update, epoch, cancel) = {
        let mut inner = manager.lock();
        if !state.update_check_allowed() {
            return Err("shutdown_blocked");
        }
        if inner.status.startup_complete
            || !allowed_distribution(&inner.status)
            || inner.status.candidate.as_deref() != Some(&candidate)
        {
            return Err("target");
        }
        if inner.status.phase == "ready" {
            if inner.update.is_none() || inner.bytes.is_none() {
                return Err("target");
            }
            // Reusing verified bytes still requires this new explicit consent.
            inner.consented = true;
            inner.status.revision = inner.status.revision.saturating_add(1);
            return Ok(inner.status.clone());
        }
        if inner.status.phase == "downloading" {
            return Ok(inner.status.clone());
        }
        if !matches!(inner.status.phase, "available" | "cancelled" | "failed")
            || matches!(
                inner.status.error,
                Some("signature" | "target" | "metadata")
            )
        {
            return Err("target");
        }
        let update = inner.update.clone().ok_or("target")?;
        inner.consented = true;
        inner.epoch += 1;
        inner.bytes = None;
        inner.status.phase = "downloading";
        inner.status.downloaded = 0;
        inner.status.total = None;
        inner.status.error = None;
        inner.status.notify = true;
        let cancel = Arc::new(tokio::sync::Notify::new());
        inner.cancel = Some(cancel.clone());
        inner.status.revision += 1;
        emit(&app, &inner.status);
        (update, inner.epoch, cancel)
    };
    let oversized = Arc::new(tokio::sync::Notify::new());
    let limit = oversized.clone();
    let result = tokio::select! {
        value = update.download(|chunk,total| {
            let mut inner = manager.lock();
            if inner.epoch != epoch { return; }
            inner.status.downloaded = inner.status.downloaded.saturating_add(chunk as u64); inner.status.total = total;
            inner.status.revision = inner.status.revision.saturating_add(1);
            if inner.status.downloaded > MAX_BYTES || total.is_some_and(|n|n > MAX_BYTES) { limit.notify_one(); }
            emit(&app,&inner.status);
        },||{}) => value.map_err(|e|failure(&e)),
        _ = cancel.notified() => Err("cancelled"),
        _ = oversized.notified() => Err("target"),
    };
    let mut inner = manager.lock();
    if inner.epoch != epoch {
        return Ok(inner.status.clone());
    }
    inner.cancel = None;
    match result {
        Ok(bytes)
            if bytes.len() > 2 && bytes.len() as u64 <= MAX_BYTES && bytes.starts_with(b"MZ") =>
        {
            inner.bytes = Some(bytes);
            inner.status.phase = "ready";
        }
        Ok(_) => {
            inner.status.phase = "failed";
            inner.status.error = Some("target");
            inner.update = None;
        }
        Err(code) => {
            inner.status.phase = if code == "cancelled" {
                "cancelled"
            } else {
                "failed"
            };
            inner.status.error = Some(code);
            if matches!(code, "signature" | "target" | "metadata") {
                inner.update = None;
            }
            crate::diagnostic_log::record("updater", "download", code, "error", None, None);
        }
    }
    inner.status.revision = inner.status.revision.saturating_add(1);
    let status = inner.status.clone();
    drop(inner);
    emit(&app, &status);
    Ok(status)
}

#[tauri::command]
pub(crate) fn updater_cancel<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
    candidate: String,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    let mut inner = manager.lock();
    cancel_download(&mut inner, &candidate)?;
    let status = inner.status.clone();
    drop(inner);
    emit(&app, &status);
    Ok(status)
}
fn cancel_download(inner: &mut Inner, candidate: &str) -> Result<(), &'static str> {
    if inner.status.startup_complete
        || inner.status.candidate.as_deref() != Some(candidate)
        || !matches!(inner.status.phase, "downloading" | "ready" | "cancelled")
    {
        return Err("target");
    }
    if inner.status.phase == "cancelled" {
        return Ok(());
    }
    inner.epoch = inner.epoch.saturating_add(1);
    if let Some(cancel) = inner.cancel.take() {
        cancel.notify_one();
    }
    inner.bytes = None;
    inner.consented = false;
    inner.status.phase = "cancelled";
    inner.status.error = Some("cancelled");
    inner.status.downloaded = 0;
    inner.status.total = None;
    inner.status.revision = inner.status.revision.saturating_add(1);
    Ok(())
}
#[tauri::command]
pub(crate) fn updater_continue<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    let mut inner = manager.lock();
    if matches!(
        inner.status.phase,
        "preparing" | "installing" | "install_failed"
    ) {
        return Err("shutdown_blocked");
    }
    inner.epoch = inner.epoch.saturating_add(1);
    if let Some(cancel) = inner.cancel.take() {
        cancel.notify_one();
    }
    inner.consented = false;
    inner.update = None;
    inner.bytes = None;
    inner.status.startup_complete = true;
    if !matches!(inner.status.phase, "latest" | "unavailable") {
        inner.status.phase = "skipped";
    }
    inner.status.notify = false;
    inner.status.revision = inner.status.revision.saturating_add(1);
    state.allow_project_startup();
    let status = inner.status.clone();
    drop(inner);
    emit(&app, &status);
    Ok(status)
}
#[tauri::command]
pub(crate) fn updater_release<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
    candidate: String,
) -> Result<(), &'static str> {
    caller(&webview, &state)?;
    let inner = manager.lock();
    if inner.status.candidate.as_deref() != Some(&candidate) {
        return Err("target");
    }
    let url = release_url(inner.status.version.as_deref().ok_or("target")?)?;
    crate::about::open(&url).map_err(|_| "link")
}
#[tauri::command]
pub(crate) fn updater_prepare<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
    candidate: String,
) -> Result<Status, &'static str> {
    caller(&webview, &state)?;
    let mut inner = manager.lock();
    if manager.handoff_load_failed {
        return Err("handoff");
    }
    if inner.status.startup_complete
        || !inner.consented
        || !allowed_distribution(&inner.status)
        || inner.status.candidate.as_deref() != Some(&candidate)
        || inner.status.phase != "ready"
        || inner.bytes.is_none()
    {
        return Err("target");
    }
    let svn = app.state::<crate::svn::Manager>();
    if svn.has_active() {
        return Err("active_work");
    }
    let recovery = state.verify_recovery_update_handoff()?;
    let pending = svn.verify_update_handoff()?;
    inner.status.handoff = Some(handoff::preserve_snapshot(
        &manager.settings,
        &candidate,
        recovery,
        pending,
    )?);
    state.prepare_update(&candidate)?;
    inner.status.phase = "preparing";
    inner.status.revision = inner.status.revision.saturating_add(1);
    let status = inner.status.clone();
    drop(inner);
    emit(&app, &status);
    Ok(status)
}
#[tauri::command]
pub(crate) fn updater_close_failed<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    manager: State<'_, Arc<Manager>>,
) -> Result<(), &'static str> {
    caller(&webview, &state)?;
    if manager.lock().status.phase != "install_failed" {
        return Err("target");
    }
    state.close_after_update_failure();
    Ok(())
}

/// 이벤트 소유 thread에서 기존 종료 완료 뒤 호출한다. 실제 설치 API는 한 번만 호출한다.
pub(crate) fn handoff<R: Runtime>(app: &tauri::AppHandle<R>, state: &AppState) {
    let Some(candidate) = state.update_intent() else {
        return;
    };
    if !state.exit_approved() {
        return;
    }
    let manager = app.state::<Arc<Manager>>();
    let mut inner = manager.lock();
    if inner.status.phase != "preparing" || inner.status.candidate.as_deref() != Some(&candidate) {
        return;
    }
    let svn = app.state::<crate::svn::Manager>();
    if !svn.seal_for_update() {
        return;
    }
    let proof = (|| {
        let recovery = state.verify_recovery_update_handoff()?;
        let pending = svn.verify_update_handoff()?;
        handoff::preserve_snapshot(&manager.settings, &candidate, recovery, pending)
    })();
    match proof {
        Ok(snapshot) => inner.status.handoff = Some(snapshot),
        Err(code) => {
            state.finish_failed_update();
            inner.status.phase = "install_failed";
            inner.status.error = Some(code);
            inner.status.revision += 1;
            let status = inner.status.clone();
            drop(inner);
            emit(app, &status);
            return;
        }
    }
    if !state.consume_update_approval(&candidate) {
        svn.unseal_update();
        return;
    }
    let update = inner.update.take();
    let bytes = inner.bytes.take();
    inner.status.phase = "installing";
    inner.status.revision = inner.status.revision.saturating_add(1);
    let status = inner.status.clone();
    drop(inner);
    emit(app, &status);
    #[cfg(not(feature = "updater-test"))]
    let result = match (update, bytes) {
        (Some(update), Some(bytes)) => update.install(bytes),
        _ => Err(tauri_plugin_updater::Error::InvalidUpdaterFormat),
    };
    #[cfg(feature = "updater-test")]
    let result = {
        let calls = manager
            .test_boundary_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let receipt = serde_json::json!({"candidate":candidate,"nativeApprovalConsumed":true,"boundaryCalls":calls,"actualInstallerCalls":0,"bytesSha256":bytes.as_ref().map(|b|format!("{:x}",Sha256::digest(b))),"hasUpdate":update.is_some()});
        std::fs::create_dir_all(&manager.settings)
            .and_then(|_| {
                std::fs::write(
                    manager.settings.join("updater-test-receipt.json"),
                    receipt.to_string(),
                )
            })
            .map_err(tauri_plugin_updater::Error::Io)
    };
    // Windows 성공은 process::exit으로 반환하지 않는다. 실패/미확인을 성공으로 표시하지 않는다.
    state.finish_failed_update();
    let mut inner = manager.lock();
    inner.status.phase = "install_failed";
    inner.status.error = Some(if cfg!(feature = "updater-test") && result.is_ok() {
        "test_handoff"
    } else if result.is_err() {
        "install"
    } else {
        "unconfirmed"
    });
    crate::diagnostic_log::record("updater", "install", "handoff", "error", None, None);
    inner.status.revision = inner.status.revision.saturating_add(1);
    let status = inner.status.clone();
    drop(inner);
    emit(app, &status);
}
