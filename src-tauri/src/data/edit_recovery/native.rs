//! 앱 소유 파일은 삭제 권한이 있는 배타 handle로 만들고 같은 객체만 게시/정리한다.
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

#[cfg(windows)]
pub(crate) fn open(path: &Path, create_new: bool, writable: bool) -> io::Result<File> {
    open_with_access(path, create_new, writable, create_new)
}
#[cfg(windows)]
pub(crate) fn open_for_discard(path: &Path) -> io::Result<File> {
    open_with_access(path, false, false, true)
}
/// 이미 read+delete sharing으로 고정해 둔 검증 handle이 있는 파일을 같은 identity로
/// 삭제할 때 사용한다. 새 handle도 기존 read access를 허용하되 write sharing은 열지 않는다.
#[cfg(windows)]
pub(crate) fn open_for_discard_shared_read(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001)
        .custom_flags(0x0020_0000)
        .access_mode(0x8001_0000)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || crate::data::project_file::directory::is_reparse(&metadata)
        || link_count(&file)? != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovery file boundary",
        ));
    }
    Ok(file)
}
#[cfg(windows)]
fn open_with_access(
    path: &Path,
    create_new: bool,
    writable: bool,
    deletable: bool,
) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .create_new(create_new)
        .share_mode(0)
        .custom_flags(0x0020_0000);
    if deletable {
        // 명시적 discard만 기존 파일의 DELETE 권한을 요청한다. 읽기/재검증의 권한은 유지한다.
        options.access_mode(if writable { 0xc001_0000 } else { 0x8001_0000 });
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || crate::data::project_file::directory::is_reparse(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovery file boundary",
        ));
    }
    // hard link를 통한 앱 경계 밖 파일 쓰기도 허용하지 않는다.
    if link_count(&file)? != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovery file alias",
        ));
    }
    Ok(file)
}
#[cfg(windows)]
pub(crate) fn link_count(file: &File) -> io::Result<u32> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct Information {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(h: *mut c_void, info: *mut Information) -> i32;
    }
    // SAFETY: 모든 필드가 정수인 repr(C) 구조를 0으로 초기화하고 정확한 크기의 출력 공간을 빌린다.
    let mut info: Information = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.links)
}

#[cfg(windows)]
pub(crate) fn publish(file: &File, name: &str) -> io::Result<()> {
    crate::data::atomic_file::owned::rename(file, name, false)
}
#[cfg(windows)]
pub(crate) fn cleanup(file: &File) -> io::Result<()> {
    crate::data::atomic_file::owned::delete_owned(file)
}

#[cfg(not(windows))]
fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "guarded recovery requires Windows",
    ))
}
#[cfg(not(windows))]
pub(crate) fn link_count(_: &File) -> io::Result<u32> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn open(_: &Path, _: bool, _: bool) -> io::Result<File> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn open_for_discard(_: &Path) -> io::Result<File> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn open_for_discard_shared_read(_: &Path) -> io::Result<File> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn publish(_: &File, _: &str) -> io::Result<()> {
    unsupported()
}
#[cfg(not(windows))]
pub(crate) fn cleanup(_: &File) -> io::Result<()> {
    unsupported()
}
