//! Resolve the app-owned data directory to the physical directory observed by
//! opened handles. Packaged Windows parents can redirect LocalAppData into a
//! package namespace; the recovery Store must use that exact physical path.
use std::{fs, io, path::PathBuf};
#[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
use tauri::Manager;
use tauri::Runtime;

pub(crate) fn local_data<R: Runtime>(app: &tauri::AppHandle<R>) -> io::Result<PathBuf> {
    #[cfg(not(any(feature = "updater-test", feature = "updater-integration-test")))]
    let logical = app.path().app_local_data_dir().map_err(io::Error::other)?;
    #[cfg(any(feature = "updater-test", feature = "updater-integration-test"))]
    let logical = {
        // The same owned fixture must survive launches from a packaged parent
        // and Explorer. Production keeps its normal per-application directory.
        let _ = app;
        PathBuf::from(env!("WORLDBUILD_UPDATER_TEST_DATA_ROOT"))
    };
    fs::create_dir_all(&logical)?;
    fs::canonicalize(logical)
}

/// The project owner must be shared by both installed channels. AppLocalData
/// is package redirected for MSIX, so it cannot coordinate with NSIS.
pub(crate) fn project_lock_root() -> io::Result<PathBuf> {
    let profile = std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Windows user profile unavailable")
        })?;
    let root = PathBuf::from(profile)
        .join(".worldbuild-tool")
        .join("project-locks");
    fs::create_dir_all(&root)?;
    fs::canonicalize(root)
}

/// Only the known pre-fix Store location is eligible for handoff. The caller
/// checks existence before opening it, so an absent old package stays absent.
pub(crate) fn legacy_recovery_root<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> io::Result<Option<PathBuf>> {
    if env!("WORLDBUILD_BUILD_CHANNEL") != "store" || env!("WORLDBUILD_PACKAGE_MODE") != "test" {
        return Ok(None);
    }
    let local = local_data(app)?;
    let physical = local.to_string_lossy().to_ascii_lowercase();
    if !physical.contains("\\packages\\") {
        // A Store test binary can also run unpackaged during development.
        // There is no package-local predecessor to import in that case.
        return Ok(None);
    }
    if !physical.contains("\\packages\\dreamrugi.worldbuildtool.elocaltest_")
        || !physical.ends_with("\\localcache\\local\\com.dreamrugi.worldbuildtool.e.localtest")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected Store test package location",
        ));
    }
    Ok(Some(local.join("edit-recovery")))
}

/// MSIX deletes package-local data on uninstall. Keep archived edit recovery
/// under the user's profile so removing the package cannot delete it.
pub(crate) fn recovery_root<R: Runtime>(app: &tauri::AppHandle<R>) -> io::Result<PathBuf> {
    if env!("WORLDBUILD_BUILD_CHANNEL") != "store" {
        return Ok(local_data(app)?.join("edit-recovery"));
    }
    let profile = std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Windows user profile unavailable")
        })?;
    let identity = if env!("WORLDBUILD_PACKAGE_MODE") == "test" {
        "store-local-test"
    } else {
        "store"
    };
    let root = PathBuf::from(profile)
        .join(".worldbuild-tool")
        .join(identity)
        .join("edit-recovery");
    fs::create_dir_all(&root)?;
    fs::canonicalize(root)
}
