use crate::data::{
    assets::{self, Metadata},
    edit_recovery::error::{Category, RecoveryError, Stage},
    project_file::directory::ProjectDirectory,
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
pub(super) fn open_asset(meta: &Metadata, bytes: &[u8]) -> Result<(), RecoveryError> {
    let extension = Path::new(&meta.name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 실행 파일/HTML/스크립트/바로가기 등은 보관 가능하지만 실행하지 않는다.
    if ![
        "txt", "md", "pdf", "png", "apng", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp",
        "docx", "xlsx", "pptx",
    ]
    .contains(&extension.as_str())
    {
        return Err(RecoveryError::new(
            Category::UnsupportedKind,
            Stage::Validate,
        ));
    }
    if bytes.len() as u64 != meta.size
        || crate::data::edit_recovery::model::digest(bytes) != meta.sha256
    {
        return Err(RecoveryError::new(
            Category::DigestMismatch,
            Stage::Validate,
        ));
    }
    let root = std::env::temp_dir()
        .join("Dreamrugi Worldbuild Tool")
        .join("view-copies");
    fs::create_dir_all(&root).map_err(|e| RecoveryError::io(Stage::Create, e))?;
    let guard = ProjectDirectory::open_move_destination(&root)
        .map_err(|e| RecoveryError::io(Stage::Validate, e))?;
    cleanup_owned_view_copies(&guard);
    let filename = format!("{}-{}.{}", meta.id, uuid::Uuid::new_v4(), extension);
    let temp_name = format!(".{filename}.tmp");
    let temp_path = root.join(&temp_name);
    let mut file = crate::data::edit_recovery::native::open(&temp_path, true, true)
        .map_err(|e| RecoveryError::io(Stage::Create, e))?;
    let write = (|| -> Result<(), RecoveryError> {
        guard
            .validate_file(&file, &temp_name)
            .map_err(|e| RecoveryError::io(Stage::Validate, e))?;
        file.write_all(bytes)
            .map_err(|e| RecoveryError::io(Stage::Write, e))?;
        file.sync_all()
            .map_err(|e| RecoveryError::io(Stage::Sync, e))?;
        crate::data::edit_recovery::native::publish(&file, &filename)
            .map_err(|e| RecoveryError::io(Stage::Publish, e))
    })();
    if let Err(error) = write {
        let _ = crate::data::edit_recovery::native::cleanup(&file);
        return Err(error);
    }
    drop(file);
    let path = root.join(&filename);
    let verify = crate::data::edit_recovery::native::open(&path, false, false)
        .map_err(|e| RecoveryError::io(Stage::Read, e))?;
    guard
        .validate_file(&verify, &filename)
        .map_err(|e| RecoveryError::io(Stage::Validate, e))?;
    let mut actual = Vec::new();
    verify
        .take(assets::MAX_BYTES as u64 + 1)
        .read_to_end(&mut actual)
        .map_err(|e| RecoveryError::io(Stage::Read, e))?;
    if actual != bytes {
        return Err(RecoveryError::new(Category::DigestMismatch, Stage::Read));
    }
    launch(path.as_os_str())
}

fn cleanup_owned_view_copies(guard: &ProjectDirectory) {
    let Ok(entries) = guard.read_dir() else {
        return;
    };
    let mut owned = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let metadata = entry.metadata().ok()?;
            let modified = metadata.modified().ok()?;
            let valid = name.len() <= 320
                && name.get(..36).is_some_and(crate::data::media::valid_id)
                && name.as_bytes().get(36) == Some(&b'-')
                && metadata.is_file();
            valid.then_some((modified, name, entry.path()))
        })
        .collect::<Vec<_>>();
    owned.sort_by_key(|row| row.0);
    let remove = owned.len().saturating_sub(64);
    for (_, name, path) in owned.into_iter().take(remove) {
        if let Ok(file) = crate::data::edit_recovery::native::open_for_discard(&path) {
            if guard.validate_file(&file, &name).is_ok() {
                let _ = crate::data::edit_recovery::native::cleanup(&file);
            }
        }
    }
}
pub(super) fn open_url(url: &str) -> Result<(), RecoveryError> {
    launch(std::ffi::OsStr::new(url))
}
#[cfg(windows)]
fn launch(target: &std::ffi::OsStr) -> Result<(), RecoveryError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    // shell command 문자열이나 인수를 구성하지 않는다. 검증한 대상만 OS 연결 프로그램에 전달한다.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err(RecoveryError::io(
            Stage::Read,
            std::io::Error::from_raw_os_error(result.0 as i32),
        ));
    }
    Ok(())
}
#[cfg(not(windows))]
fn launch(_: &std::ffi::OsStr) -> Result<(), RecoveryError> {
    Err(RecoveryError::new(Category::Unavailable, Stage::Read))
}
