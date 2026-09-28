use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use super::project_relative_path::ProjectRelativePath;

pub(crate) mod directory;

/// 프로젝트 파일을 연 뒤, 읽기 전에 열린 handle의 실제 대상을 project root와 대조한다.
/// 검증이 끝난 같은 handle만 호출자에게 넘겨 경로 재개방 경쟁을 만들지 않는다.
pub(crate) fn open_existing_project_file(
    canonical_project_root: &Path,
    target: &ProjectRelativePath,
) -> io::Result<File> {
    let requested = canonical_project_root.join(target.as_str());
    open_verified(canonical_project_root, &requested)
}

/// transaction journal처럼 프로젝트 예약 영역에 있는 private 파일을 연다.
///
/// 일반 문서용 `ProjectRelativePath`의 `.worldbuild` 거부 규칙은 그대로 두고,
/// 호출자가 이미 검증한 private directory 경계 안에서만 별도 open을 허용한다.
/// 반환된 같은 handle에서 bytes를 읽어 경로 재개방 경쟁을 피해야 한다.
pub(super) fn open_existing_private_file(
    canonical_private_root: &Path,
    requested: &Path,
) -> io::Result<File> {
    let relative = requested
        .strip_prefix(canonical_private_root)
        .map_err(|_| outside_project())?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(outside_project());
    }
    open_verified(canonical_private_root, requested)
}

#[cfg(test)]
pub(super) fn open_existing_private_file_after_open_for_test(
    canonical_private_root: &Path,
    requested: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    let relative = requested
        .strip_prefix(canonical_private_root)
        .map_err(|_| outside_project())?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(outside_project());
    }
    open_verified_after_open_for_test(canonical_private_root, requested, after_open)
}

#[cfg(windows)]
fn open_verified(canonical_project_root: &Path, requested: &Path) -> io::Result<File> {
    // 마지막 component가 reparse point여도 따라가지 않고 그 자체 handle을 연다.
    // parent가 동시에 junction으로 바뀌면 handle final path 검사가 외부 이탈을 잡는다.
    let file = open_windows_candidate(requested)?;
    validate_windows_handle(canonical_project_root, file)
}

#[cfg(all(test, windows))]
fn open_verified_after_open_for_test(
    canonical_project_root: &Path,
    requested: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    let file = open_windows_candidate(requested)?;
    after_open()?;
    validate_windows_handle(canonical_project_root, file)
}

#[cfg(windows)]
fn open_windows_candidate(requested: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    // 읽는 동안 새 write/delete handle과 rename을 막아 검증된 파일 객체도 안정적으로 유지한다.
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(requested)
}

#[cfg(windows)]
fn validate_windows_handle(canonical_project_root: &Path, file: File) -> io::Result<File> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(unsafe_file_type());
    }

    let actual = windows_final_path(&file)?;
    let root = normalize_windows_verbatim_path(canonical_project_root);
    let actual = normalize_windows_verbatim_path(&actual);
    if !actual.starts_with(&root) || actual == root {
        return Err(outside_project());
    }
    Ok(file)
}

#[cfg(all(test, windows))]
pub(crate) fn open_existing_project_file_after_open_for_test(
    canonical_project_root: &Path,
    target: &ProjectRelativePath,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    let requested = canonical_project_root.join(target.as_str());
    open_verified_after_open_for_test(canonical_project_root, &requested, after_open)
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> io::Result<PathBuf> {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut c_void,
            path: *mut u16,
            path_length: u32,
            flags: u32,
        ) -> u32;
    }

    let handle = file.as_raw_handle();
    // FILE_NAME_NORMALIZED | VOLUME_NAME_DOS는 모두 값 0이다.
    let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if required == 0 {
        return Err(io::Error::last_os_error());
    }
    let capacity = usize::try_from(required)
        .map_err(|_| io::Error::other("final path length cannot be represented"))?;
    let mut buffer = vec![0_u16; capacity];
    let written = unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), required, 0) };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    if written >= required {
        return Err(io::Error::other("final path changed while it was queried"));
    }
    buffer.truncate(
        usize::try_from(written)
            .map_err(|_| io::Error::other("final path length cannot be represented"))?,
    );
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

#[cfg(windows)]
fn normalize_windows_verbatim_path(path: &Path) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    const VERBATIM: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    let value: Vec<u16> = path.as_os_str().encode_wide().collect();
    let normalized = if value.starts_with(VERBATIM_UNC) {
        let mut result = vec![b'\\' as u16, b'\\' as u16];
        result.extend_from_slice(&value[VERBATIM_UNC.len()..]);
        result
    } else if value.starts_with(VERBATIM) {
        value[VERBATIM.len()..].to_vec()
    } else {
        value
    };
    PathBuf::from(OsString::from_wide(&normalized))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_verified(canonical_project_root: &Path, requested: &Path) -> io::Result<File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    const O_NOFOLLOW: i32 = 0x0002_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(requested)?;
    if !file.metadata()?.is_file() {
        return Err(unsafe_file_type());
    }
    let actual = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    if !actual.starts_with(canonical_project_root) || actual == canonical_project_root {
        return Err(outside_project());
    }
    Ok(file)
}

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
fn open_verified_after_open_for_test(
    canonical_project_root: &Path,
    requested: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    const O_NOFOLLOW: i32 = 0x0002_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(requested)?;
    after_open()?;
    if !file.metadata()?.is_file() {
        return Err(unsafe_file_type());
    }
    let actual = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    if !actual.starts_with(canonical_project_root) || actual == canonical_project_root {
        return Err(outside_project());
    }
    Ok(file)
}

#[cfg(target_os = "macos")]
fn open_verified(canonical_project_root: &Path, requested: &Path) -> io::Result<File> {
    use std::ffi::{c_char, c_int, CStr};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    const O_NOFOLLOW: i32 = 0x0000_0100;
    const F_GETPATH: c_int = 50;
    const MACOS_PATH_BUFFER_SIZE: usize = 1024;
    extern "C" {
        fn fcntl(file_descriptor: c_int, command: c_int, ...) -> c_int;
    }

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(requested)?;
    if !file.metadata()?.is_file() {
        return Err(unsafe_file_type());
    }
    let mut buffer = [0 as c_char; MACOS_PATH_BUFFER_SIZE];
    let result = unsafe { fcntl(file.as_raw_fd(), F_GETPATH, buffer.as_mut_ptr()) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    let actual = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    let actual = Path::new(std::ffi::OsStr::from_bytes(actual.to_bytes()));
    if !actual.starts_with(canonical_project_root) || actual == canonical_project_root {
        return Err(outside_project());
    }
    Ok(file)
}

#[cfg(all(test, target_os = "macos"))]
fn open_verified_after_open_for_test(
    canonical_project_root: &Path,
    requested: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    let file = open_verified(canonical_project_root, requested)?;
    after_open()?;
    Ok(file)
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos"
)))]
fn open_verified(_canonical_project_root: &Path, _requested: &Path) -> io::Result<File> {
    // 열린 handle의 실제 경로를 std만으로 확인할 수 없는 플랫폼에서는 fail closed한다.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe opened-file path validation is unavailable on this platform",
    ))
}

#[cfg(all(
    test,
    not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    ))
))]
fn open_verified_after_open_for_test(
    _canonical_project_root: &Path,
    _requested: &Path,
    _after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe opened-file path validation is unavailable on this platform",
    ))
}

fn unsafe_file_type() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "project target is not a regular non-reparse file",
    )
}

fn outside_project() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "opened project target resolves outside the project",
    )
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::fs;
    use std::io::{self, Read};
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-safe-project-file-{}-{count}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "test directory collision",
            ))
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn create_junction(link: &Path, destination: &Path) -> io::Result<()> {
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(destination)
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other("could not create test junction"))
        }
    }

    fn target(value: &str) -> ProjectRelativePath {
        ProjectRelativePath::parse(value).expect("test target is valid")
    }

    #[test]
    fn rejects_parent_junction_that_resolves_outside_before_read() -> io::Result<()> {
        let temp = Temp::new()?;
        let root = temp.0.join("project");
        let outside = temp.0.join("outside");
        fs::create_dir(&root)?;
        fs::create_dir(&outside)?;
        fs::write(outside.join("secret.json"), b"outside-secret")?;
        create_junction(&root.join("linked"), &outside)?;
        let canonical_root = fs::canonicalize(&root)?;

        let error = open_existing_project_file(&canonical_root, &target("linked/secret.json"))
            .expect_err("outside junction must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        Ok(())
    }

    #[test]
    fn rejects_target_junction_reparse_point() -> io::Result<()> {
        let temp = Temp::new()?;
        let root = temp.0.join("project");
        let data = root.join("data");
        let outside = temp.0.join("outside");
        fs::create_dir_all(&data)?;
        fs::create_dir(&outside)?;
        fs::write(outside.join("secret.json"), b"outside-secret")?;
        create_junction(&data.join("item.json"), &outside)?;
        let canonical_root = fs::canonicalize(&root)?;

        open_existing_project_file(&canonical_root, &target("data/item.json"))
            .expect_err("target reparse point must be rejected");
        Ok(())
    }

    #[test]
    fn target_replacement_after_open_cannot_redirect_the_verified_handle() -> io::Result<()> {
        let temp = Temp::new()?;
        let root = temp.0.join("project");
        let data = root.join("data");
        fs::create_dir_all(&data)?;
        fs::write(data.join("item.json"), b"inside-original")?;
        let canonical_root = fs::canonicalize(&root)?;
        let replacement_was_blocked = std::cell::Cell::new(false);

        let mut file = open_existing_project_file_after_open_for_test(
            &canonical_root,
            &target("data/item.json"),
            || {
                fs::rename(data.join("item.json"), data.join("held-item.json"))
                    .expect_err("open read handle must block target replacement");
                replacement_was_blocked.set(true);
                Ok(())
            },
        )?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        assert!(replacement_was_blocked.get());
        assert_eq!(bytes, b"inside-original");
        assert_eq!(fs::read(data.join("item.json"))?, b"inside-original");
        Ok(())
    }

    #[test]
    fn parent_junction_replacement_after_open_cannot_redirect_the_handle() -> io::Result<()> {
        let temp = Temp::new()?;
        let root = temp.0.join("project");
        let data = root.join("data");
        let inside = root.join("inside");
        let outside = temp.0.join("outside");
        fs::create_dir_all(&inside)?;
        fs::create_dir(&outside)?;
        fs::write(inside.join("item.json"), b"inside-original")?;
        fs::write(outside.join("item.json"), b"outside-secret")?;
        create_junction(&data, &inside)?;
        let canonical_root = fs::canonicalize(&root)?;

        let mut file = open_existing_project_file_after_open_for_test(
            &canonical_root,
            &target("data/item.json"),
            || {
                fs::remove_dir(&data)?;
                create_junction(&data, &outside)
            },
        )?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"inside-original");
        assert_eq!(fs::read(data.join("item.json"))?, b"outside-secret");
        Ok(())
    }
}
